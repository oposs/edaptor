//! The save + combined-save flow (single-entry save, membership fan-out, number allocation).

use std::collections::HashMap;

use anyhow::Result;

use super::overlay::{GuardIntent, Overlay, PendingAction, PostWrite};
use super::{
    build_loaded_form, next_id, object_classes_of, perform_guard_intent, rebind_selection, App,
};
use crate::config::widget::{password_widget_for, ResolvedWidget};
use crate::form::changeset::{diff, ChangeSet, EditEntry, ModOp};
use crate::form::validate::format_validation_errors;
use crate::form::validate::{validate, SavePlan, ValidationError};
use crate::ldap::ldif::render_changesets;
use crate::ldap::worker::{Request, Response, SearchScope, WorkerHandle};
use crate::schema::SchemaModel;
use crate::ui::edit_form::{value_set_eq, EditForm};
use crate::workflows::create::now_unix_secs_or_zero;
use crate::workflows::read_flow::ReadFlow;
use crate::workflows::save::{
    compose_renamed_dn, decide_allocation, mask_changeset_secrets, membership_fanout, prepare_save,
    stage_pending_password, would_empty, PrepareSave,
};

/// Build the `(original, edited, object_classes)` for a single-entry edit save,
/// fold in any password change when the loaded entry matches a password-profile,
/// and return the resulting [`PrepareSave`]. `Err(text)` signals a confirm
/// mismatch (the caller surfaces it as an Error overlay). `now_secs` is injected
/// so the planning stays testable. Used by both the plain Alt+S save and the
/// guard-resume save so password edits work from either entry point.
pub(crate) fn prepare_edit_save(
    form: &EditForm,
    schema: &SchemaModel,
    widgets: &[ResolvedWidget],
    connection_encrypted: bool,
    now_secs: u64,
) -> Result<PrepareSave, String> {
    // Defence in depth: never stage a cleartext password change over a plain link.
    if form.pending_password.is_some() && !connection_encrypted {
        return Err("password change requires an encrypted connection".into());
    }
    // Strip fan-out labels from the baseline so `diff` does not emit a spurious
    // Delete for attrs whose changes drive the per-candidate fan-out save.
    let fanout_lbls = form.fanout_labels();
    let mut original = EditEntry {
        dn: form.dn.clone(),
        attrs: form.baseline.clone(),
    };
    for l in &fanout_lbls {
        original.attrs.remove(l);
    }
    let mut edited = form.to_edit_entry();
    let object_classes = object_classes_of(form);
    // Derive the password contribution from the staged `pending_password` (set by
    // the set-password popup), stripping primary + derived from BOTH sides BEFORE
    // the plain diff so they never double-write. Always strip even when no password
    // is pending, so a derived attr present on the baseline can never diff.
    let (password_mods, mask_attrs) = match password_widget_for(widgets, &object_classes) {
        Some(pw) => stage_pending_password(
            form.pending_password.as_deref(),
            &pw.primary,
            &pw.derived,
            pw.samba,
            now_secs,
            &mut original.attrs,
            &mut edited.attrs,
        ),
        None => (Vec::new(), Vec::new()),
    };
    Ok(prepare_save(
        schema,
        &original,
        &edited,
        &object_classes,
        &password_mods,
        &mask_attrs,
    ))
}

/// Submit the worker request(s) for a prepared [`SavePlan`] and record how to
/// react to the resulting `WriteOk`. A rename with follow-up mods defers them to
/// the rename's `WriteOk` (the MODIFY must target the post-rename DN).
pub(crate) fn submit_prepared(
    plan: SavePlan,
    old_dn: &str,
    nav: Option<String>,
    then_quit: bool,
    worker: &WorkerHandle,
    post: &mut HashMap<u64, PostWrite>,
    pending_followups: &mut HashMap<u64, (String, Vec<ModOp>, Option<String>)>,
) {
    match plan {
        SavePlan::Nothing => {}
        SavePlan::Modify(mods) => {
            let id = next_id();
            let _ = worker.submit(Request::Modify {
                id,
                dn: old_dn.to_string(),
                changes: mods,
            });
            post.insert(
                id,
                PostWrite::Save {
                    reread_dn: old_dn.to_string(),
                    nav,
                    then_quit,
                },
            );
        }
        SavePlan::RenameOnly(modrdn) => {
            let id = next_id();
            let new_dn = compose_renamed_dn(old_dn, &modrdn.new_rdn);
            let _ = worker.submit(Request::ModRdn {
                id,
                dn: old_dn.to_string(),
                new_rdn: modrdn.new_rdn,
                delete_old: modrdn.delete_old,
                new_superior: modrdn.new_superior,
            });
            post.insert(
                id,
                PostWrite::Save {
                    reread_dn: new_dn,
                    nav,
                    then_quit,
                },
            );
        }
        SavePlan::Rename { modrdn, then_mods } => {
            let id = next_id();
            let new_dn = compose_renamed_dn(old_dn, &modrdn.new_rdn);
            pending_followups.insert(id, (new_dn, then_mods, nav));
            let _ = worker.submit(Request::ModRdn {
                id,
                dn: old_dn.to_string(),
                new_rdn: modrdn.new_rdn,
                delete_old: modrdn.delete_old,
                new_superior: modrdn.new_superior,
            });
        }
    }
}

/// Outcome of planning a save for a form that has BackRef (membership) changes.
#[derive(Debug)]
enum CombinedPlan {
    /// No BackRef field changed → caller uses the normal single-entry path.
    NoMembershipChange,
    /// Own-entry mods + per-holder fan-out, with the combined LDIF preview.
    Ready {
        entry_dn: String,
        own_mods: Vec<ModOp>,
        fanout: Vec<(String, ModOp)>,
        ldif: String,
    },
    /// Rename combined with a membership change — not supported in v1 (spec §6.3).
    Blocked(String),
    /// Client-side validation failed.
    Invalid(Vec<ValidationError>),
    /// The own-entry diff could not be computed (e.g. multi-valued RDN).
    DiffError(String),
}

/// Plan a combined save: own-entry diff (backref stripped from BOTH sides) plus
/// the fan-out from each BackRef field's baseline→selection delta. Blocks a
/// rename combined with a membership change (v1 simplification, spec §6.3).
///
/// Returns `NoMembershipChange` when no backref field actually changed value,
/// so the caller can fall through to the normal single-entry `prepare_save` path.
fn plan_combined_save(
    form: &EditForm,
    schema: &SchemaModel,
    widgets: &[ResolvedWidget],
    connection_encrypted: bool,
    now_secs: u64,
) -> CombinedPlan {
    let fanout = form.fanout_labels();
    if fanout.is_empty() {
        return CombinedPlan::NoMembershipChange;
    }

    // Did any fan-out field actually change its value set?
    let changed = form.fields.iter().any(|f| {
        if !fanout.contains(&f.label) {
            return false;
        }
        let base = form.baseline.get(&f.label).cloned().unwrap_or_default();
        !value_set_eq(&f.current_values(), &base)
    });
    if !changed {
        return CombinedPlan::NoMembershipChange;
    }

    // Own-entry: strip fan-out labels from both sides, validate + diff.
    let object_classes = object_classes_of(form);
    let oc_refs: Vec<&str> = object_classes.iter().map(|s| s.as_str()).collect();
    let mut original = EditEntry {
        dn: form.dn.clone(),
        attrs: form.baseline.clone(),
    };
    let mut edited = form.to_edit_entry(); // already omits fan-out fields
    for l in &fanout {
        original.attrs.remove(l);
        edited.attrs.remove(l);
    }

    // Stage any password change the same way the single-entry path does: strip the
    // primary + derived password attrs from BOTH sides (so a blank field never diffs
    // to a Delete that would clobber the stored password) and, when a cleartext is
    // staged via `pending_password`, collect the REPLACE mods to fold into the
    // own-entry MODIFY. A staged change over a plain link blocks the combined save.
    let (password_mods, mask_attrs) = match password_widget_for(widgets, &object_classes) {
        Some(pw) => {
            if form.pending_password.is_some() && !connection_encrypted {
                return CombinedPlan::Blocked(
                    "Password change requires an encrypted connection (ldaps:// or start_tls)."
                        .into(),
                );
            }
            stage_pending_password(
                form.pending_password.as_deref(),
                &pw.primary,
                &pw.derived,
                pw.samba,
                now_secs,
                &mut original.attrs,
                &mut edited.attrs,
            )
        }
        None => (Vec::new(), Vec::new()),
    };

    let errors = validate(&edited, schema, &oc_refs);
    if !errors.is_empty() {
        return CombinedPlan::Invalid(errors);
    }
    let mut own_cs = match diff(&original, &edited) {
        Ok(c) => c,
        Err(e) => return CombinedPlan::DiffError(e.to_string()),
    };
    if own_cs.modrdn.is_some() {
        return CombinedPlan::Blocked(
            "Rename and membership changes can't be saved together — \
             do them in separate saves."
                .into(),
        );
    }
    own_cs.mods.extend(password_mods);

    // Fan-out: one set of Add/Delete MODIFYs per fan-out field that changed.
    let mut fanout_ops: Vec<(String, ModOp)> = Vec::new();
    let mut preview_sets: Vec<ChangeSet> = Vec::new();
    if !own_cs.is_empty() {
        // Mask the password values in the preview only; `own_mods` keeps the real
        // cleartext/hash for the apply.
        preview_sets.push(mask_changeset_secrets(&own_cs, &mask_attrs));
    }
    for f in form.fields.iter().filter(|f| fanout.contains(&f.label)) {
        let Some(attr) = (match &f.widget_binding {
            Some(crate::config::widget::WidgetKind::Picker(b)) => b.fanout_attr.clone(),
            _ => None,
        }) else {
            continue;
        };
        let base = form.baseline.get(&f.label).cloned().unwrap_or_default();
        let ops = membership_fanout(&form.dn, &base, &f.current_values(), &attr);
        for (gdn, op) in ops {
            preview_sets.push(ChangeSet {
                dn: gdn.clone(),
                modrdn: None,
                mods: vec![op.clone()],
            });
            fanout_ops.push((gdn, op));
        }
    }

    CombinedPlan::Ready {
        entry_dn: form.dn.clone(),
        own_mods: own_cs.mods,
        fanout: fanout_ops,
        ldif: render_changesets(&preview_sets),
    }
}

/// Map a `CombinedPlan` to the overlay that should be shown, or `None` when
/// there is no membership change (caller falls through to the single-entry save
/// path). Extracted to avoid duplicating the match in `FormSave` and
/// `SaveThenNavigate`.
pub(crate) fn combined_save_overlay(
    form: &EditForm,
    schema: &SchemaModel,
    widgets: &[ResolvedWidget],
    connection_encrypted: bool,
    then_intent: Option<GuardIntent>,
) -> Option<Overlay> {
    match plan_combined_save(
        form,
        schema,
        widgets,
        connection_encrypted,
        now_unix_secs_or_zero(),
    ) {
        CombinedPlan::Ready {
            entry_dn,
            own_mods,
            fanout,
            ldif,
        } => Some(Overlay::Confirm {
            title: "Apply these changes?".to_string(),
            body: ldif,
            action: PendingAction::CombinedSave {
                entry_dn,
                own_mods,
                fanout,
                then_intent,
            },
        }),
        CombinedPlan::Blocked(msg) => Some(Overlay::Error { text: msg }),
        CombinedPlan::Invalid(errs) => Some(Overlay::Error {
            text: format_validation_errors(&errs),
        }),
        CombinedPlan::DiffError(e) => Some(Overlay::Error { text: e }),
        CombinedPlan::NoMembershipChange => None,
    }
}

/// Synchronously re-read `dn` and rebuild the form so it reflects the directory
/// after a combined save. Installs the fresh form directly without depending on
/// the async poll loop or the overlay-gated install path.
fn reload_form_sync(app: &mut App, worker: &WorkerHandle, read_flow: &ReadFlow, dn: &str) {
    rebind_selection(app, dn);
    if let Ok(Response::Entries { entries, .. }) = worker.request(Request::Search {
        id: next_id(),
        base: dn.to_string(),
        scope: SearchScope::Base,
        filter: "(objectClass=*)".to_string(),
        attrs: vec!["*".to_string()],
        size_limit: None,
    }) {
        if let Some(entry) = entries.first() {
            let model = read_flow.form_for(entry, &[]);
            app.form = Some(build_loaded_form(
                &model,
                read_flow.schema(),
                app.read_only,
                &app.pickers,
                &app.widgets,
            ));
            app.form_focus = 0;
            app.form_scroll = 0;
        }
    }
}

/// Apply a combined membership save SYNCHRONOUSLY (mirrors `refresh_structure`):
/// pre-validate last-member on every removal, abort the whole batch if any would
/// empty a group, then apply own-entry mods + each fan-out MODIFY, collecting a
/// partial-failure report, and finally re-read the edited entry (synchronous).
impl super::Ctx<'_> {
    pub(crate) fn apply_combined_save(
        &mut self,
        entry_dn: &str,
        own_mods: Vec<ModOp>,
        fanout: Vec<(String, ModOp)>,
        then_intent: Option<GuardIntent>,
    ) {
        let app = &mut *self.app;
        let worker = self.worker;
        let read_flow = &mut *self.read_flow;
        // 1. Pre-validate: for each Delete, Base-read the group's current holder_attr
        //    values; block the whole batch if any removal would empty a group.
        //    A read failure is treated conservatively — also blocked.
        let mut blocked: Vec<String> = Vec::new();
        for (gdn, op) in &fanout {
            if let ModOp::Delete { attr, values } = op {
                match read_group_members(worker, gdn, attr) {
                    None => {
                        blocked.push(format!("{gdn}: could not verify members"));
                    }
                    Some(members) => {
                        if let Some(member) = values.first() {
                            if would_empty(&members, member) {
                                blocked.push(format!("{gdn}: would remove last member"));
                            }
                        }
                    }
                }
            }
        }
        if !blocked.is_empty() {
            // No write happened — leave form and user's edits intact, no re-read.
            app.overlay = Some(Overlay::Error {
                text: format!(
                    "Cannot save — membership change blocked:\n- {}",
                    blocked.join("\n- ")
                ),
            });
            return;
        }

        // 2. Apply own-entry mods, then each fan-out MODIFY; collect failures.
        let mut failures: Vec<String> = Vec::new();
        if !own_mods.is_empty() {
            if let Some(msg) = apply_one_modify(worker, entry_dn, own_mods) {
                failures.push(format!("{entry_dn}: {msg}"));
            }
        }
        for (gdn, op) in fanout {
            if let Some(msg) = apply_one_modify(worker, &gdn, vec![op]) {
                failures.push(format!("{gdn}: {msg}"));
            }
        }

        // 3. Re-read the entry synchronously so the form reflects the directory
        // state immediately (before setting status/overlay). This avoids the
        // async install gate clearing the partial-failure message on the next
        // poll iteration.
        reload_form_sync(app, worker, read_flow, entry_dn);

        if failures.is_empty() {
            app.status = "Saved.".to_string();
            // Resume the pending guard intent (focus change / navigation / quit) only
            // on a clean save; on partial failure keep the user on the entry with the
            // error visible.
            if let Some(intent) = then_intent {
                perform_guard_intent(app, worker, read_flow, intent);
            }
        } else {
            app.overlay = Some(Overlay::Error {
                text: format!("Saved with errors:\n- {}", failures.join("\n- ")),
            });
        }
    }
}

/// Base-read a group's current `holder_attr` values (synchronous).
/// Returns `None` on read error or unexpected response (caller treats this
/// conservatively), `Some(members)` on a successful read.
fn read_group_members(
    worker: &WorkerHandle,
    group_dn: &str,
    holder_attr: &str,
) -> Option<Vec<String>> {
    match worker.request(Request::Search {
        id: next_id(),
        base: group_dn.to_string(),
        scope: SearchScope::Base,
        filter: "(objectClass=*)".to_string(),
        attrs: vec![holder_attr.to_string()],
        size_limit: None,
    }) {
        Ok(Response::Entries { entries, .. }) => Some(
            entries
                .into_iter()
                .next()
                .and_then(|e| {
                    e.attrs
                        .into_iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case(holder_attr))
                        .map(|(_, v)| v)
                })
                .unwrap_or_default(),
        ),
        _ => None,
    }
}

/// Allocate the next free numeric `attr` in `[min,max]` by scanning the whole
/// subtree from `base_dn`. Refuses if the scan was truncated (spec D6). Synchronous.
pub(crate) fn allocate_number(
    worker: &WorkerHandle,
    base_dn: &str,
    attr: &str,
    min: u64,
    max: u64,
) -> Result<u64, String> {
    let resp = worker
        .request(Request::Search {
            id: next_id(),
            base: base_dn.to_string(),
            scope: SearchScope::Subtree,
            filter: format!("({attr}=*)"),
            attrs: vec![attr.to_string()],
            size_limit: None,
        })
        .map_err(|e| e.to_string())?;
    let (entries, truncated) = match resp {
        Response::Entries {
            entries, truncated, ..
        } => (entries, truncated),
        Response::SearchError { msg, .. } => return Err(msg),
        _ => return Err("unexpected response while allocating".to_string()),
    };
    let mut values: Vec<u64> = Vec::new();
    for e in &entries {
        if let Some((_, vs)) = e.attrs.iter().find(|(k, _)| k.eq_ignore_ascii_case(attr)) {
            for v in vs {
                if let Ok(n) = v.trim().parse::<u64>() {
                    values.push(n);
                }
            }
        }
    }
    decide_allocation(&values, truncated, min, max)
}

/// Apply one MODIFY synchronously; return `Some(human message)` on failure.
fn apply_one_modify(worker: &WorkerHandle, dn: &str, changes: Vec<ModOp>) -> Option<String> {
    match worker.request(Request::Modify {
        id: next_id(),
        dn: dn.to_string(),
        changes,
    }) {
        Ok(Response::WriteOk { .. }) => None,
        Ok(Response::WriteError { msg, .. }) => Some(msg),
        Ok(_) => Some("unexpected response".to_string()),
        Err(e) => Some(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::app::test_support::*;

    use std::collections::BTreeMap;

    use tui_prompts::TextState;

    use crate::schema::FieldKind;
    use crate::ui::edit_form::{EditField, FormMode};
    use crate::ui::form::WidgetSpec;

    /// Build a user EditForm with:
    /// - own change: description baseline→["old desc"], values→["new desc"]
    /// - memberOf change: baseline→[g1], values→[g2]
    fn user_form_own_and_memberof_change() -> EditForm {
        use crate::config::relation::CandidateScope;

        let scope = CandidateScope {
            base: "ou=groups,dc=x".into(),
            object_classes: vec!["groupOfNames".into()],
            search_attrs: vec!["cn".into()],
            label_template: None,
        };

        let uid_field = EditField {
            label: "uid".into(),
            must: true,
            editable: true,
            multi: false,
            secret: false,
            ordered: false,
            values: vec!["ann".into()],
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            editor: TextState::new().with_value("ann".to_string()),
            picker: None,
            widget_binding: None,
        };

        let desc_field = EditField {
            label: "description".into(),
            must: false,
            editable: true,
            multi: true,
            secret: false,
            ordered: false,
            values: vec!["new desc".into()],
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            editor: TextState::new(),
            picker: None,
            widget_binding: None,
        };

        let memberof_field = EditField {
            label: "memberOf".into(),
            must: false,
            editable: true,
            multi: true,
            secret: false,
            ordered: false,
            values: vec!["cn=g2,ou=groups,dc=x".into()],
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            editor: TextState::new(),
            picker: None,
            widget_binding: Some(crate::config::widget::WidgetKind::Picker(
                crate::config::relation::PickerBinding {
                    attr: "memberOf".into(),
                    scope: scope.clone(),
                    store: crate::config::relation::StoreKey::Dn,
                    select: None,
                    fanout_attr: Some("member".into()),
                },
            )),
        };

        let mut baseline = BTreeMap::new();
        baseline.insert("objectClass".into(), vec!["testUser".into()]);
        baseline.insert("uid".into(), vec!["ann".into()]);
        baseline.insert("description".into(), vec!["old desc".into()]);
        baseline.insert("memberOf".into(), vec!["cn=g1,ou=groups,dc=x".into()]);

        EditForm {
            dn: "uid=ann,ou=people,dc=x".into(),
            fields: vec![uid_field, desc_field, memberof_field],
            baseline,
            mode: FormMode::Edit,
            pending_password: None,
        }
    }

    /// Build a user EditForm where the RDN attr (uid) is changed AND memberOf changes.
    fn user_form_rename_and_memberof_change() -> EditForm {
        use crate::config::relation::CandidateScope;

        let scope = CandidateScope {
            base: "ou=groups,dc=x".into(),
            object_classes: vec!["groupOfNames".into()],
            search_attrs: vec!["cn".into()],
            label_template: None,
        };

        // uid changed from "ann" → "bob" (triggers modrdn in diff)
        let uid_field = EditField {
            label: "uid".into(),
            must: true,
            editable: true,
            multi: false,
            secret: false,
            ordered: false,
            values: vec!["ann".into()],
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            editor: TextState::new().with_value("bob".to_string()),
            picker: None,
            widget_binding: None,
        };

        let memberof_field = EditField {
            label: "memberOf".into(),
            must: false,
            editable: true,
            multi: true,
            secret: false,
            ordered: false,
            values: vec!["cn=g2,ou=groups,dc=x".into()],
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            editor: TextState::new(),
            picker: None,
            widget_binding: Some(crate::config::widget::WidgetKind::Picker(
                crate::config::relation::PickerBinding {
                    attr: "memberOf".into(),
                    scope,
                    store: crate::config::relation::StoreKey::Dn,
                    select: None,
                    fanout_attr: Some("member".into()),
                },
            )),
        };

        let mut baseline = BTreeMap::new();
        baseline.insert("objectClass".into(), vec!["testUser".into()]);
        baseline.insert("uid".into(), vec!["ann".into()]);
        baseline.insert("memberOf".into(), vec!["cn=g1,ou=groups,dc=x".into()]);

        EditForm {
            dn: "uid=ann,ou=people,dc=x".into(),
            fields: vec![uid_field, memberof_field],
            baseline,
            mode: FormMode::Edit,
            pending_password: None,
        }
    }

    #[test]
    fn plan_combined_save_splits_own_and_fanout() {
        let form = user_form_own_and_memberof_change();
        let schema = user_schema();
        let plan = plan_combined_save(&form, &schema, &[], true, 0);
        let (own_mods, fanout, _entry_dn) = match plan {
            CombinedPlan::Ready {
                own_mods,
                fanout,
                entry_dn,
                ..
            } => (own_mods, fanout, entry_dn),
            other => panic!("expected Ready, got {:?}", other),
        };
        // own_mods touches description, NOT memberOf.
        assert!(
            own_mods.iter().all(|m| {
                let attr = match m {
                    ModOp::Add { attr, .. }
                    | ModOp::Delete { attr, .. }
                    | ModOp::Replace { attr, .. } => attr,
                };
                !attr.eq_ignore_ascii_case("memberOf")
            }),
            "own_mods must not contain memberOf"
        );
        // fanout: g2 gains the user, g1 loses the user.
        assert_eq!(fanout.len(), 2, "expected 2 fanout ops (add g2, delete g1)");
    }

    #[test]
    fn rename_plus_membership_is_blocked() {
        let form = user_form_rename_and_memberof_change();
        let schema = user_schema();
        assert!(
            matches!(
                plan_combined_save(&form, &schema, &[], true, 0),
                CombinedPlan::Blocked(_)
            ),
            "rename + membership change must be Blocked"
        );
    }

    /// A password-widget entry edited via the combined (membership) save path,
    /// carrying the directory's stored hash on the baseline. `pending` is the
    /// cleartext staged by the set-password popup (None = no password change).
    /// Returns the form plus the resolved password widget bound to `testUser`.
    fn pw_user_form_with_memberof_change(pending: Option<&str>) -> (EditForm, Vec<ResolvedWidget>) {
        let mut form = user_form_own_and_memberof_change();
        // The directory returned the stored password hash on the entry.
        form.baseline
            .insert("userPassword".into(), vec!["{SSHA}old".into()]);
        form.pending_password = pending.map(|s| s.to_string());
        (form, password_widgets())
    }

    fn own_mods_touch(mods: &[ModOp], attr: &str) -> bool {
        mods.iter().any(|m| {
            let a = match m {
                ModOp::Add { attr, .. }
                | ModOp::Delete { attr, .. }
                | ModOp::Replace { attr, .. } => attr,
            };
            a.eq_ignore_ascii_case(attr)
        })
    }

    #[test]
    fn combined_save_no_pending_password_does_not_clobber() {
        // No password staged: the combined-save path must strip the baseline hash
        // (so it never diffs to a Delete) and emit no userPassword mod.
        let (form, widgets) = pw_user_form_with_memberof_change(None);
        match plan_combined_save(&form, &user_schema(), &widgets, true, 1_700_000_000) {
            CombinedPlan::Ready { own_mods, .. } => {
                assert!(
                    !own_mods_touch(&own_mods, "userPassword"),
                    "no staged password must not emit a userPassword mod (clobber!)"
                );
            }
            _ => panic!("expected Ready"),
        }
    }

    /// A user EditForm with no fan-out field (so it goes through the single-entry
    /// `prepare_edit_save` path), carrying the directory's stored hash on the
    /// baseline and `pending_password` already staged by the set-password popup.
    fn pw_user_form_pending(pending: Option<&str>) -> EditForm {
        let oc_field = EditField {
            label: "objectClass".into(),
            must: true,
            editable: false,
            multi: true,
            secret: false,
            ordered: false,
            values: vec!["testUser".into()],
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            editor: TextState::new(),
            picker: None,
            widget_binding: None,
        };
        let uid_field = EditField {
            label: "uid".into(),
            must: true,
            editable: true,
            multi: false,
            secret: false,
            ordered: false,
            values: vec!["ann".into()],
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            editor: TextState::new().with_value("ann".to_string()),
            picker: None,
            widget_binding: None,
        };
        let mut baseline = BTreeMap::new();
        baseline.insert("objectClass".into(), vec!["testUser".into()]);
        baseline.insert("uid".into(), vec!["ann".into()]);
        baseline.insert("userPassword".into(), vec!["{SSHA}old".into()]);
        EditForm {
            dn: "uid=ann,ou=people,dc=x".into(),
            fields: vec![oc_field, uid_field],
            baseline,
            mode: FormMode::Edit,
            pending_password: pending.map(|s| s.to_string()),
        }
    }

    /// A resolved password widget bound to `testUser`.
    fn password_widgets() -> Vec<ResolvedWidget> {
        use crate::config::widget::{PasswordWidget, WidgetKind};
        vec![ResolvedWidget {
            owner_object_classes: vec!["testUser".into()],
            attr: "userPassword".into(),
            kind: WidgetKind::Password(PasswordWidget {
                primary: "userPassword".into(),
                derived: vec![],
                samba: false,
            }),
        }]
    }

    #[test]
    fn prepare_edit_save_pending_password_replaces_and_masks() {
        let form = pw_user_form_pending(Some("hunter2"));
        let prep = prepare_edit_save(
            &form,
            &user_schema(),
            &password_widgets(),
            true,
            1_700_000_000,
        )
        .expect("encrypted: ok");
        match prep {
            PrepareSave::Ready { plan, ldif, .. } => {
                assert!(ldif.contains("********"), "preview masks the password");
                assert!(!ldif.contains("hunter2"), "cleartext must not appear");
                match plan {
                    SavePlan::Modify(mods) => assert!(
                        mods.contains(&ModOp::Replace {
                            attr: "userPassword".into(),
                            values: vec!["hunter2".into()],
                        }),
                        "new password is a REPLACE in the plan"
                    ),
                    _ => panic!("expected Modify"),
                }
            }
            _ => panic!("expected Ready"),
        }
    }

    #[test]
    fn prepare_edit_save_no_pending_strips_baseline_hash_no_change() {
        // Blank password (no pending): the baseline hash must be stripped so it
        // never diffs to a Delete; with no other edit this is NoChanges.
        let form = pw_user_form_pending(None);
        let prep = prepare_edit_save(
            &form,
            &user_schema(),
            &password_widgets(),
            true,
            1_700_000_000,
        )
        .expect("ok");
        assert!(
            matches!(prep, PrepareSave::NoChanges),
            "blank password + no other edit is NoChanges (baseline hash stripped)"
        );
    }

    #[test]
    fn prepare_edit_save_pending_on_plain_connection_is_rejected() {
        let form = pw_user_form_pending(Some("hunter2"));
        match prepare_edit_save(&form, &user_schema(), &password_widgets(), false, 0) {
            Err(err) => assert!(err.contains("encrypted"), "TLS guard message: {err}"),
            Ok(_) => panic!("plain connection must be rejected"),
        }
    }

    #[test]
    fn combined_save_sets_password_as_replace_and_masks_preview() {
        // Operator staged a new password via the set-password popup, then a
        // membership change. The combined own-entry MODIFY must carry the password
        // REPLACE (masked in the preview) alongside the description change.
        let (form, widgets) = pw_user_form_with_memberof_change(Some("hunter2"));
        match plan_combined_save(&form, &user_schema(), &widgets, true, 1_700_000_000) {
            CombinedPlan::Ready { own_mods, ldif, .. } => {
                assert!(
                    own_mods.contains(&ModOp::Replace {
                        attr: "userPassword".into(),
                        values: vec!["hunter2".into()],
                    }),
                    "new password must be a REPLACE in own_mods"
                );
                assert!(ldif.contains("********"), "preview masks the password");
                assert!(
                    !ldif.contains("hunter2"),
                    "cleartext must not appear in preview"
                );
            }
            _ => panic!("expected Ready"),
        }
    }

    #[test]
    fn combined_save_pending_password_on_plain_connection_is_blocked() {
        let (form, widgets) = pw_user_form_with_memberof_change(Some("hunter2"));
        assert!(
            matches!(
                plan_combined_save(&form, &user_schema(), &widgets, false, 1_700_000_000),
                CombinedPlan::Blocked(_)
            ),
            "a staged password over a plain connection must be Blocked"
        );
    }
}
