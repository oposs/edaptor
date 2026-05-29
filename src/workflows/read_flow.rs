//! The M3 read flow: turn a selected entry DN into a read-only [`FormModel`].
//!
//! This is the tty-free spine of the milestone. When the user opens a browser
//! node, [`ReadFlow::request_entry`] submits a base-scope search for that DN
//! (all user attributes). The manual loop's idle hook polls the worker and
//! feeds responses to [`ReadFlow::on_response`], which correlates by id (D4),
//! builds the schema-driven [`FormModel`], and hands it back for the facade to
//! display. The facade dialog itself is the only tty-bound piece and lives in
//! [`crate::ui::facade::build_entry_dialog`].

use std::collections::HashMap;

use anyhow::Result;

use crate::config::EntryProfile;
use crate::ldap::worker::{LdapEntry, Request, Response, SearchScope, WorkerHandle};
use crate::schema::SchemaModel;
use crate::ui::form::{build_form_model, FormModel};

/// Outcome of feeding a polled [`Response`] to the read flow.
pub enum ReadOutcome {
    /// A built form to display, plus the entry's objectClass values (needed by
    /// the write path's client-side validation, which `FormModel` does not carry).
    Form {
        /// The schema-driven form model.
        model: FormModel,
        /// The entry's objectClass values.
        object_classes: Vec<String>,
    },
    /// A user-facing error string to surface (e.g. via `facade::confirm_error`).
    Error(String),
    /// The response was not for this flow (unknown id / unrelated variant).
    Ignored,
}

/// Tracks in-flight base reads and turns their results into form models, using
/// the server schema for typing and the active profile for field ordering.
pub struct ReadFlow {
    schema: SchemaModel,
    /// in-flight read id -> the active profile's `show` ordering (empty for the
    /// generic tier).
    pending: HashMap<u64, Vec<String>>,
    next_id: u64,
}

impl ReadFlow {
    /// Create a read flow over the given server schema.
    pub fn new(schema: SchemaModel) -> Self {
        ReadFlow {
            schema,
            pending: HashMap::new(),
            next_id: 1,
        }
    }

    /// Submit a base-scope read of `dn` (all user attributes). `profile` selects
    /// the field ordering: its `show` list when supplied, else the generic
    /// (empty) ordering. Returns the correlation id.
    pub fn request_entry(
        &mut self,
        worker: &WorkerHandle,
        dn: &str,
        profile: Option<&EntryProfile>,
    ) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        worker.submit(Request::Search {
            id,
            base: dn.to_string(),
            scope: SearchScope::Base,
            filter: "(objectClass=*)".to_string(),
            attrs: vec!["*".to_string()],
        })?;
        let show = profile.map(|p| p.show.clone()).unwrap_or_default();
        self.pending.insert(id, show);
        Ok(id)
    }

    /// Correlate a polled [`Response`] with a pending read. On a matching
    /// `Entries` with at least one entry, build and return the [`FormModel`]; on
    /// a matching `SearchError`, return the error; otherwise `Ignored`.
    pub fn on_response(&mut self, resp: &Response) -> ReadOutcome {
        match resp {
            Response::Entries { id, entries } => {
                let Some(show) = self.pending.remove(id) else {
                    return ReadOutcome::Ignored;
                };
                let Some(entry) = entries.first() else {
                    return ReadOutcome::Error("entry not found".to_string());
                };
                ReadOutcome::Form {
                    model: self.form_for(entry, &show),
                    object_classes: object_classes_of(entry),
                }
            }
            Response::SearchError { id, msg } => {
                if self.pending.remove(id).is_some() {
                    ReadOutcome::Error(msg.clone())
                } else {
                    ReadOutcome::Ignored
                }
            }
            _ => ReadOutcome::Ignored,
        }
    }

    /// The server schema this flow was built with — needed by the write path's
    /// client-side validation (`form::validate::validate`).
    pub fn schema(&self) -> &SchemaModel {
        &self.schema
    }

    /// Build the form for an already-fetched entry (objectClasses come from the
    /// entry itself). Exposed for the integration test's end-to-end check.
    pub fn form_for(&self, entry: &LdapEntry, profile_show: &[String]) -> FormModel {
        let object_classes = object_classes_of(entry);
        let oc_refs: Vec<&str> = object_classes.iter().map(|s| s.as_str()).collect();
        build_form_model(&self.schema, &oc_refs, entry, profile_show)
    }
}

/// Extract an entry's objectClass values (case-insensitive attribute lookup).
fn object_classes_of(entry: &LdapEntry) -> Vec<String> {
    entry
        .attrs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("objectClass"))
        .map(|(_, v)| v.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldap::worker::RawSubschema;
    use std::collections::BTreeMap;

    fn schema() -> SchemaModel {
        let raw = RawSubschema {
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
        };
        SchemaModel::from_raw(&raw)
    }

    fn entry() -> LdapEntry {
        let mut attrs = BTreeMap::new();
        attrs.insert(
            "objectClass".to_string(),
            vec!["top".to_string(), "person".to_string()],
        );
        attrs.insert("cn".to_string(), vec!["Alice".to_string()]);
        attrs.insert("sn".to_string(), vec!["Adams".to_string()]);
        LdapEntry {
            dn: "cn=Alice,dc=example,dc=org".to_string(),
            attrs,
            bin_attrs: BTreeMap::new(),
        }
    }

    #[test]
    fn unknown_id_is_ignored() {
        let mut flow = ReadFlow::new(schema());
        flow.pending.insert(1, vec![]);
        let resp = Response::Entries {
            id: 999,
            entries: vec![entry()],
        };
        assert!(matches!(flow.on_response(&resp), ReadOutcome::Ignored));
        assert_eq!(flow.pending.len(), 1);
    }

    #[test]
    fn matching_entries_build_a_form() {
        let mut flow = ReadFlow::new(schema());
        flow.pending.insert(3, vec!["cn".to_string()]);
        let resp = Response::Entries {
            id: 3,
            entries: vec![entry()],
        };
        match flow.on_response(&resp) {
            ReadOutcome::Form {
                model,
                object_classes,
            } => {
                assert_eq!(model.title, "cn=Alice,dc=example,dc=org");
                assert_eq!(model.fields[0].label, "cn"); // profile_show first
                assert!(model.fields.iter().any(|f| f.label == "sn" && f.is_must));
                assert!(object_classes.iter().any(|o| o == "person"));
            }
            _ => panic!("expected a form"),
        }
        assert!(flow.pending.is_empty());
    }

    #[test]
    fn interleaved_ids_resolve_independently() {
        let mut flow = ReadFlow::new(schema());
        flow.pending.insert(10, vec![]);
        flow.pending.insert(11, vec![]);
        // Resolve the second one first.
        let r11 = flow.on_response(&Response::Entries {
            id: 11,
            entries: vec![entry()],
        });
        assert!(matches!(r11, ReadOutcome::Form { .. }));
        assert_eq!(flow.pending.len(), 1);
        let r10 = flow.on_response(&Response::Entries {
            id: 10,
            entries: vec![entry()],
        });
        assert!(matches!(r10, ReadOutcome::Form { .. }));
        assert!(flow.pending.is_empty());
    }

    #[test]
    fn search_error_surfaces_message() {
        let mut flow = ReadFlow::new(schema());
        flow.pending.insert(4, vec![]);
        let resp = Response::SearchError {
            id: 4,
            msg: "no such object".to_string(),
        };
        match flow.on_response(&resp) {
            ReadOutcome::Error(m) => assert_eq!(m, "no such object"),
            _ => panic!("expected an error"),
        }
    }

    #[test]
    fn empty_entries_is_an_error() {
        let mut flow = ReadFlow::new(schema());
        flow.pending.insert(5, vec![]);
        let resp = Response::Entries {
            id: 5,
            entries: vec![],
        };
        assert!(matches!(flow.on_response(&resp), ReadOutcome::Error(_)));
    }

    #[test]
    fn form_for_uses_entry_object_classes() {
        let flow = ReadFlow::new(schema());
        let model = flow.form_for(&entry(), &[]);
        assert!(model.fields.iter().any(|f| f.label == "cn" && f.is_must));
        assert!(model.fields.iter().any(|f| f.label == "sn" && f.is_must));
    }
}
