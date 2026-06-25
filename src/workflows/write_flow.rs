//! Async write flow: validate + diff an [`EditForm`] save (via
//! [`crate::workflows::save::prepare_save`]) and correlate the worker's write
//! responses. `prepare` and `on_response` are pure; `submit`/`submit_followup`
//! are thin worker wrappers. Mirrors `read_flow` but for writes; the two never
//! collide because read and write responses are disjoint `Response` variants.

use std::collections::HashMap;

use anyhow::Result;

use crate::form::changeset::{EditEntry, ModOp};
use crate::form::validate::SavePlan;
use crate::ldap::worker::{Request, Response, WorkerHandle};
use crate::schema::SchemaModel;
use crate::workflows::edit_form::EditForm;
use crate::workflows::save::{compose_renamed_dn, prepare_save, PrepareSave};

/// What a pending write means once its `WriteOk` arrives.
#[derive(Debug, Clone)]
enum WriteIntent {
    /// A plain save (or a rename's final leg): re-read `reread_dn` afterwards.
    Save { reread_dn: String, quit_after: bool },
    /// A rename's first leg: on success, submit `mods` against `new_dn`.
    RenameThenModify {
        new_dn: String,
        mods: Vec<ModOp>,
        quit_after: bool,
    },
}

/// The app-facing result of correlating one write response.
#[derive(Debug, Clone)]
pub enum WriteOutcome {
    /// Not one of our pending writes.
    Ignored,
    /// A write completed; re-read `reread_dn` (unless quitting).
    Saved { reread_dn: String, quit_after: bool },
    /// A rename's MODRDN landed; caller must submit the deferred `mods` via
    /// [`WriteFlow::submit_followup`].
    NeedFollowupModify {
        dn: String,
        mods: Vec<ModOp>,
        quit_after: bool,
    },
    /// A write failed; `msg` is already human-mapped by the worker.
    Error(String),
}

/// Tracks in-flight writes and turns the edit form into a save plan.
pub struct WriteFlow {
    next_id: u64,
    pending: HashMap<u64, WriteIntent>,
}

impl Default for WriteFlow {
    fn default() -> Self {
        Self::new()
    }
}

impl WriteFlow {
    pub fn new() -> Self {
        // Start above ReadFlow's range as defence in depth; correctness does not
        // rely on it (read/write response variants are disjoint).
        WriteFlow {
            next_id: 1_000_000,
            pending: HashMap::new(),
        }
    }

    fn alloc(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Validate + diff `form` into a [`PrepareSave`]. Pure (no worker, no clock).
    /// Password staging is M4, so `password_mods`/`mask_attrs` are empty here.
    pub fn prepare(&self, form: &EditForm, schema: &SchemaModel) -> PrepareSave {
        // Uniform field handling: the MUST objectClass attribute is itself a form
        // field (build_form_model emits every MUST/MAY attr, and objectClass is
        // MUST via top), so `original`/`edited` carry it like any other field —
        // no attribute-specific special-casing in the neutral form core.
        let original = EditEntry {
            dn: form.dn.clone(),
            attrs: form
                .fields
                .iter()
                .map(|f| (f.label.clone(), f.baseline.clone()))
                .collect(),
        };
        let edited = form.to_edit_entry();
        let secret_attrs: Vec<String> = form
            .fields
            .iter()
            .filter(|f| f.secret)
            .map(|f| f.label.clone())
            .collect();
        let orphaned: Vec<&str> = form
            .fields
            .iter()
            .filter(|f| f.orphaned)
            .map(|f| f.label.as_str())
            .collect();
        let x_ordered: std::collections::HashSet<String> = form
            .fields
            .iter()
            .filter(|f| f.ordered)
            .map(|f| f.label.clone())
            .collect();
        prepare_save(
            schema,
            &original,
            &edited,
            &form.object_classes,
            &[], // password_mods (M4)
            &[], // mask_attrs (M4)
            &secret_attrs,
            &orphaned,
            &x_ordered,
        )
    }

    /// Submit a [`SavePlan`] to the worker, tracking what each id means.
    pub fn submit(
        &mut self,
        worker: &WorkerHandle,
        plan: SavePlan,
        old_dn: &str,
        quit_after: bool,
    ) -> Result<()> {
        match plan {
            SavePlan::Nothing => {}
            SavePlan::Modify(mods) => {
                let id = self.alloc();
                worker.submit(Request::Modify {
                    id,
                    dn: old_dn.to_string(),
                    changes: mods,
                })?;
                self.pending.insert(
                    id,
                    WriteIntent::Save {
                        reread_dn: old_dn.to_string(),
                        quit_after,
                    },
                );
            }
            SavePlan::RenameOnly(modrdn) => {
                let id = self.alloc();
                let new_dn = compose_renamed_dn(old_dn, &modrdn.new_rdn);
                worker.submit(Request::ModRdn {
                    id,
                    dn: old_dn.to_string(),
                    new_rdn: modrdn.new_rdn,
                    delete_old: modrdn.delete_old,
                    new_superior: modrdn.new_superior,
                })?;
                self.pending.insert(
                    id,
                    WriteIntent::Save {
                        reread_dn: new_dn,
                        quit_after,
                    },
                );
            }
            SavePlan::Rename { modrdn, then_mods } => {
                let id = self.alloc();
                let new_dn = compose_renamed_dn(old_dn, &modrdn.new_rdn);
                worker.submit(Request::ModRdn {
                    id,
                    dn: old_dn.to_string(),
                    new_rdn: modrdn.new_rdn,
                    delete_old: modrdn.delete_old,
                    new_superior: modrdn.new_superior,
                })?;
                self.pending.insert(
                    id,
                    WriteIntent::RenameThenModify {
                        new_dn,
                        mods: then_mods,
                        quit_after,
                    },
                );
            }
        }
        Ok(())
    }

    /// Submit the deferred modifications of a rename's second leg.
    pub fn submit_followup(
        &mut self,
        worker: &WorkerHandle,
        dn: &str,
        mods: Vec<ModOp>,
        quit_after: bool,
    ) -> Result<()> {
        let id = self.alloc();
        worker.submit(Request::Modify {
            id,
            dn: dn.to_string(),
            changes: mods,
        })?;
        self.pending.insert(
            id,
            WriteIntent::Save {
                reread_dn: dn.to_string(),
                quit_after,
            },
        );
        Ok(())
    }

    /// Correlate one polled [`Response`]. Pure; ignores non-write variants.
    pub fn on_response(&mut self, resp: &Response) -> WriteOutcome {
        match resp {
            Response::WriteOk { id, .. } => match self.pending.remove(id) {
                Some(WriteIntent::Save {
                    reread_dn,
                    quit_after,
                }) => WriteOutcome::Saved {
                    reread_dn,
                    quit_after,
                },
                Some(WriteIntent::RenameThenModify {
                    new_dn,
                    mods,
                    quit_after,
                }) => WriteOutcome::NeedFollowupModify {
                    dn: new_dn,
                    mods,
                    quit_after,
                },
                None => WriteOutcome::Ignored,
            },
            Response::WriteError { id, msg } => {
                if self.pending.remove(id).is_some() {
                    WriteOutcome::Error(msg.clone())
                } else {
                    WriteOutcome::Ignored
                }
            }
            _ => WriteOutcome::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldap::worker::{RawSubschema, Response};
    use crate::schema::{FieldKind, SchemaModel};
    use crate::workflows::edit_form::{EditField, EditForm, FormMode};
    use crate::workflows::form_model::WidgetSpec;

    fn schema() -> SchemaModel {
        SchemaModel::from_raw(&RawSubschema {
            object_classes: vec![
                "( 2.5.6.0 NAME 'top' ABSTRACT MUST objectClass )".to_string(),
                "( 2.5.6.6 NAME 'person' SUP top STRUCTURAL MUST ( sn $ cn ) MAY description )"
                    .to_string(),
            ],
            attribute_types: vec![
                "( 2.5.4.3 NAME 'cn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".to_string(),
                "( 2.5.4.4 NAME 'sn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".to_string(),
            ],
            ldap_syntaxes: vec![],
        })
    }

    fn field(label: &str, val: &str, base: &str) -> EditField {
        EditField {
            label: label.into(),
            must: true,
            editable: true,
            multi: false,
            secret: false,
            ordered: false,
            orphaned: false,
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: None,
            values: vec![val.into()],
            baseline: vec![base.into()],
        }
    }

    fn form_with(fields: Vec<EditField>) -> EditForm {
        EditForm {
            dn: "cn=Alice,dc=example,dc=org".into(),
            mode: FormMode::Edit,
            object_classes: vec!["top".into(), "person".into()],
            fields,
        }
    }

    /// The MUST objectClass attribute is a real form field (multi-valued,
    /// read-only in M2), exactly as `build_form_model` emits it.
    fn oc_field() -> EditField {
        EditField {
            label: "objectClass".into(),
            must: true,
            editable: false,
            multi: true,
            secret: false,
            ordered: false,
            orphaned: false,
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: None,
            values: vec!["top".into(), "person".into()],
            baseline: vec!["top".into(), "person".into()],
        }
    }

    #[test]
    fn prepare_no_change_is_nochanges() {
        let wf = WriteFlow::new();
        let f = form_with(vec![
            oc_field(),
            field("cn", "Alice", "Alice"),
            field("sn", "Adams", "Adams"),
        ]);
        assert!(matches!(wf.prepare(&f, &schema()), PrepareSave::NoChanges));
    }

    #[test]
    fn prepare_modify_yields_ready() {
        let wf = WriteFlow::new();
        let f = form_with(vec![
            oc_field(),
            field("cn", "Alice", "Alice"),
            field("sn", "Allen", "Adams"),
        ]);
        match wf.prepare(&f, &schema()) {
            PrepareSave::Ready { dn, ldif, .. } => {
                assert_eq!(dn, "cn=Alice,dc=example,dc=org");
                assert!(ldif.contains("sn"));
            }
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    #[test]
    fn write_ok_for_save_intent_reads_back() {
        let mut wf = WriteFlow::new();
        wf.pending.insert(
            7,
            WriteIntent::Save {
                reread_dn: "cn=Bob,dc=x".into(),
                quit_after: false,
            },
        );
        match wf.on_response(&Response::WriteOk {
            id: 7,
            dn: "cn=Bob,dc=x".into(),
        }) {
            WriteOutcome::Saved {
                reread_dn,
                quit_after,
            } => {
                assert_eq!(reread_dn, "cn=Bob,dc=x");
                assert!(!quit_after);
            }
            other => panic!("expected Saved, got {other:?}"),
        }
        assert!(wf.pending.is_empty());
    }

    #[test]
    fn write_ok_for_rename_then_modify_requests_followup() {
        let mut wf = WriteFlow::new();
        wf.pending.insert(
            3,
            WriteIntent::RenameThenModify {
                new_dn: "cn=New,dc=x".into(),
                mods: vec![ModOp::Replace {
                    attr: "sn".into(),
                    values: vec!["Z".into()],
                }],
                quit_after: true,
            },
        );
        match wf.on_response(&Response::WriteOk {
            id: 3,
            dn: "cn=New,dc=x".into(),
        }) {
            WriteOutcome::NeedFollowupModify {
                dn,
                mods,
                quit_after,
            } => {
                assert_eq!(dn, "cn=New,dc=x");
                assert_eq!(mods.len(), 1);
                assert!(quit_after);
            }
            other => panic!("expected NeedFollowupModify, got {other:?}"),
        }
    }

    #[test]
    fn write_error_surfaces_message() {
        let mut wf = WriteFlow::new();
        wf.pending.insert(
            9,
            WriteIntent::Save {
                reread_dn: "x".into(),
                quit_after: false,
            },
        );
        match wf.on_response(&Response::WriteError {
            id: 9,
            msg: "constraint".into(),
        }) {
            WriteOutcome::Error(m) => assert_eq!(m, "constraint"),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn entries_response_is_ignored_even_on_id_overlap() {
        // A read response with the same id as a pending write must NOT be consumed.
        let mut wf = WriteFlow::new();
        wf.pending.insert(
            1,
            WriteIntent::Save {
                reread_dn: "x".into(),
                quit_after: false,
            },
        );
        assert!(matches!(
            wf.on_response(&Response::Entries {
                id: 1,
                entries: vec![],
                truncated: false
            }),
            WriteOutcome::Ignored
        ));
        assert_eq!(wf.pending.len(), 1);
    }
}
