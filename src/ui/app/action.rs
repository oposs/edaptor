//! Action handling plus the navigation / dirty-guard glue: services [`UiAction`]s
//! that need the worker or schema, runs confirmed [`PendingAction`]s, and drives
//! the reconcile / guard-intent navigation between leaf selections.

use tui_prompts::{State, TextState};

use crate::app::UiAction;
use crate::config::EntryProfile;
use crate::ldap::worker::{Request, Response, WorkerHandle};
use crate::schema::SchemaModel;
use crate::ui::edit_form::{build_edit_form, EditForm};
use crate::workflows::create::profiles_for_container;
use crate::workflows::read_flow::ReadFlow;
use crate::workflows::structure::Structure;

use super::overlay::{GuardIntent, Overlay, PendingAction, PostWrite};
use super::structure_view::{
    compute_rows, structure_input_from_attrs, structure_inputs, structure_scan_attrs,
};
use super::{
    combined_save_overlay, next_id, open_create_form, prepare_create, prepare_edit_save,
    submit_prepared, App,
};
use crate::form::validate::format_validation_errors;
use crate::workflows::create::{now_unix_secs_or_zero, profile_for_entry};
use crate::workflows::save::PrepareSave;

/// Service a [`UiAction`] that needs the worker / schema. Save and cancel build
/// confirm overlays; create opens an editable overlay; delete opens a confirm;
/// refresh re-runs the eager scan synchronously and rebuilds the panes.
impl super::Ctx<'_> {
    pub(crate) fn handle_action(
        &mut self,
        action: UiAction,
        structure: &mut Structure,
        profiles: &[EntryProfile],
        base_dn: &str,
    ) {
        let app = &mut *self.app;
        let worker = self.worker;
        let read_flow = &mut *self.read_flow;
        match action {
            UiAction::FormSave => {
                let Some(form) = app.form.as_ref() else {
                    return;
                };
                if form.is_new() {
                    prepare_create(app, worker, read_flow, profiles, base_dn);
                    return;
                }
                // Try the combined membership path first; fall back to the single-entry
                // path when no backref field actually changed. No guard intent here —
                // a plain Alt+S save has nothing to resume afterward.
                if let Some(ov) = combined_save_overlay(form, read_flow.schema(), profiles, None) {
                    app.overlay = Some(ov);
                    return;
                }
                // Normal single-entry save (folds in any password change when the
                // entry matches a password-profile).
                let prep = match prepare_edit_save(
                    form,
                    read_flow.schema(),
                    profiles,
                    now_unix_secs_or_zero(),
                ) {
                    Ok(p) => p,
                    Err(text) => {
                        app.overlay = Some(Overlay::Error { text });
                        return;
                    }
                };
                match prep {
                    PrepareSave::Ready { plan, dn, ldif } => {
                        app.overlay = Some(Overlay::Confirm {
                            title: "Apply these changes?".to_string(),
                            body: ldif,
                            action: PendingAction::Save {
                                plan,
                                dn,
                                nav: None,
                            },
                        });
                    }
                    PrepareSave::NoChanges => app.status = "No changes.".to_string(),
                    PrepareSave::Invalid(errs) => {
                        app.overlay = Some(Overlay::Error {
                            text: format_validation_errors(&errs),
                        })
                    }
                    PrepareSave::DiffError(e) => app.overlay = Some(Overlay::Error { text: e }),
                }
            }
            UiAction::FormCancel => revert_form(app),
            UiAction::NewEntry(i) => open_create_form(app, read_flow, profiles, i, base_dn),
            UiAction::NewEntryChoose => {
                // Offer profiles whose search_base matches the current container;
                // fall back to all profiles so Alt+N always works.
                let mut matches = profiles_for_container(profiles, &app.current_branch);
                if matches.is_empty() {
                    matches = (0..profiles.len()).collect();
                }
                match matches.len() {
                    0 => {}
                    1 => open_create_form(app, read_flow, profiles, matches[0], base_dn),
                    _ => {
                        let entries = matches
                            .iter()
                            .map(|&i| (i, profiles[i].name.clone()))
                            .collect();
                        app.overlay = Some(Overlay::ChooseProfile { entries, sel: 0 })
                    }
                }
            }
            UiAction::DeleteEntry(dn) => {
                if !dn.is_empty() {
                    app.overlay = Some(Overlay::Confirm {
                        title: "Delete this entry?".to_string(),
                        body: dn.clone(),
                        action: PendingAction::Delete { dn },
                    });
                }
            }
            UiAction::Refresh => refresh_structure(app, worker, structure, base_dn),
            UiAction::None => {}
        }
    }
}

/// Re-run the eager structure scan (synchronous, like startup) and rebuild the
/// tree + leaf panes. Keeps the current branch if it still exists, else falls
/// back to the base DN. (Port of the old `UiAction::Refresh` arm.)
fn refresh_structure(
    app: &mut App,
    worker: &WorkerHandle,
    structure: &mut Structure,
    base_dn: &str,
) {
    match worker.request(Request::LoadStructure {
        id: 0,
        base: base_dn.to_string(),
        page_size: 500,
        attrs: structure_scan_attrs(&app.label_rules, &app.tree_rules),
    }) {
        Ok(Response::StructureEntries { nodes, .. }) => {
            *structure = Structure::build(base_dn, structure_inputs(nodes));
            if structure.get(&app.current_branch).is_none() {
                app.current_branch = base_dn.to_string();
            }
            app.rows = compute_rows(
                structure,
                &app.current_branch,
                &app.last_search,
                &app.label_rules,
            );
            app.leaf_sel = 0;
            app.last_seen_leaf = None;
            app.status = "Refreshed.".to_string();
        }
        Ok(Response::StructureError { msg, .. }) => {
            app.overlay = Some(Overlay::Error { text: msg })
        }
        _ => {
            app.overlay = Some(Overlay::Error {
                text: "refresh failed".to_string(),
            })
        }
    }
}

/// Whether a freshly base-read form for `title` should replace `app.form` now:
/// it must match the entry the user is currently on, no overlay may be open, and
/// an in-progress (unsaved) create form must not be clobbered by a late read of
/// the previous selection.
pub(crate) fn should_install_form(app: &App, title: &str) -> bool {
    app.last_seen_leaf
        .as_deref()
        .map(|dn| dn.eq_ignore_ascii_case(title))
        .unwrap_or(false)
        && app.overlay.is_none()
        && !app.form.as_ref().map(|f| f.is_new()).unwrap_or(false)
}

/// Revert every field to its baseline (Alt+C cancel): drop multi-value edits and
/// reseed each single-value editor from the original values. An unsaved create
/// form has no baseline to revert to, so cancel simply discards it.
fn revert_form(app: &mut App) {
    if app.form.as_ref().map(|f| f.is_new()).unwrap_or(false) {
        app.form = None;
        app.form_focus = 0;
        app.form_scroll = 0;
        app.status.clear();
        // Forget the awaited DN so the next reconcile tick re-reads the currently
        // selected leaf into the form pane (instead of leaving it blank).
        app.last_seen_leaf = None;
        return;
    }
    if let Some(form) = app.form.as_mut() {
        for field in &mut form.fields {
            let base = form.baseline.get(&field.label).cloned().unwrap_or_default();
            field.editor = TextState::new().with_value(base.first().cloned().unwrap_or_default());
            field.values = base;
        }
        app.status = "Reverted.".to_string();
    }
}

/// After a save re-reads `dn` (possibly a rename's new DN), point both the
/// awaited DN and the current leaf row at it, so the post-save base-read passes
/// the DN gate and `reconcile` does not fire a competing read of the old DN.
/// Only the selected row's DN is rebound; the eager `Structure` is not updated on
/// a rename, so the leaf label / tree fully re-sync on the next Refresh (Alt+R).
pub(crate) fn rebind_selection(app: &mut App, dn: &str) {
    app.last_seen_leaf = Some(dn.to_string());
    if let Some(row) = app.rows.get_mut(app.leaf_sel) {
        row.1 = dn.to_string();
    }
}

/// Run a confirmed [`PendingAction`] (submits to the worker / navigates).
impl super::Ctx<'_> {
    pub(crate) fn execute_pending(
        &mut self,
        action: PendingAction,
        profiles: &[EntryProfile],
        base_dn: &str,
    ) {
        let app = &mut *self.app;
        let worker = self.worker;
        let read_flow = &mut *self.read_flow;
        let post = &mut *self.post;
        let pending_followups = &mut *self.pending_followups;
        match action {
            PendingAction::Save { plan, dn, nav } => {
                submit_prepared(plan, &dn, nav, false, worker, post, pending_followups);
                app.status = "Saving…".to_string();
            }
            PendingAction::Create { dn, attrs, parent } => {
                let id = next_id();
                let input = structure_input_from_attrs(&dn, &attrs);
                let _ = worker.submit(Request::Add { id, dn, attrs });
                post.insert(id, PostWrite::Created { parent, input });
                app.status = "Creating…".to_string();
            }
            PendingAction::Delete { dn } => {
                let id = next_id();
                let _ = worker.submit(Request::Delete { id, dn: dn.clone() });
                post.insert(id, PostWrite::Deleted { dn });
                app.status = "Deleting…".to_string();
            }
            PendingAction::OpenCreate { profile_idx } => {
                open_create_form(app, read_flow, profiles, profile_idx, base_dn);
            }
            PendingAction::ResolveGuard {
                intent,
                save: false,
            } => {
                // Discard: drop the edits and perform the intent now.
                perform_guard_intent(app, worker, read_flow, intent);
            }
            PendingAction::ResolveGuard { intent, save: true } => {
                // Save, then perform the intent. Navigation defers to the write's
                // WriteOk (the re-read must target the post-save DN); a focus change
                // applies immediately; a quit defers to the WriteOk so it isn't lost.
                let Some(form) = app.form.as_ref() else {
                    perform_guard_intent(app, worker, read_flow, intent);
                    return;
                };
                // A create form has no diff baseline: route "Save" to the create
                // confirm (an Add). The pending guard intent is dropped — the user
                // confirms the create, then navigates explicitly (full
                // save-then-resume for create is out of scope here).
                if form.is_new() {
                    prepare_create(app, worker, read_flow, profiles, base_dn);
                    return;
                }
                // A membership-bearing save runs synchronously through CombinedSave;
                // the pending guard intent rides along and is performed on success.
                if let Some(ov) =
                    combined_save_overlay(form, read_flow.schema(), profiles, Some(intent.clone()))
                {
                    app.overlay = Some(ov);
                    return;
                }
                // Normal single-entry save (folds in any password change).
                let prep = match prepare_edit_save(
                    form,
                    read_flow.schema(),
                    profiles,
                    now_unix_secs_or_zero(),
                ) {
                    Ok(p) => p,
                    Err(text) => {
                        app.overlay = Some(Overlay::Error { text });
                        return;
                    }
                };
                match prep {
                    PrepareSave::Ready { plan, dn, .. } => {
                        app.status = "Saving…".to_string();
                        match intent {
                            GuardIntent::Nav(target) => {
                                // Advance the awaited DN ONLY now that we commit, so a
                                // later failure cannot silence the dirty guard.
                                app.last_seen_leaf = target.clone();
                                submit_prepared(
                                    plan,
                                    &dn,
                                    target,
                                    false,
                                    worker,
                                    post,
                                    pending_followups,
                                );
                            }
                            GuardIntent::Focus(pane) => {
                                submit_prepared(
                                    plan,
                                    &dn,
                                    None,
                                    false,
                                    worker,
                                    post,
                                    pending_followups,
                                );
                                app.focus = pane;
                            }
                            GuardIntent::Quit => {
                                submit_prepared(
                                    plan,
                                    &dn,
                                    None,
                                    true,
                                    worker,
                                    post,
                                    pending_followups,
                                );
                            }
                        }
                    }
                    // Nothing to save after all → just perform the intent.
                    PrepareSave::NoChanges => perform_guard_intent(app, worker, read_flow, intent),
                    PrepareSave::Invalid(errs) => {
                        app.overlay = Some(Overlay::Error {
                            text: format_validation_errors(&errs),
                        })
                    }
                    PrepareSave::DiffError(e) => app.overlay = Some(Overlay::Error { text: e }),
                }
            }
            PendingAction::CombinedSave {
                entry_dn,
                own_mods,
                fanout,
                then_intent,
            } => {
                self.apply_combined_save(profiles, &entry_dn, own_mods, fanout, then_intent);
            }
        }
    }
}

/// Perform a guard intent WITHOUT saving (Discard, or a save that turned out to
/// be a no-op): navigate / change focus (dropping the form's edits) / quit.
pub(crate) fn perform_guard_intent(
    app: &mut App,
    worker: &WorkerHandle,
    read_flow: &mut ReadFlow,
    intent: GuardIntent,
) {
    match intent {
        GuardIntent::Nav(target) => navigate_to(app, worker, read_flow, target),
        GuardIntent::Focus(pane) => {
            // Drop the edits so the (still-shown) form is clean, then move focus.
            revert_form(app);
            app.focus = pane;
        }
        GuardIntent::Quit => app.should_quit = true,
    }
}

/// Navigate the form pane to `target`: base-read the DN, or clear the form when
/// the target is `None` (empty leaf list). Records `target` as the awaited DN.
fn navigate_to(
    app: &mut App,
    worker: &WorkerHandle,
    read_flow: &mut ReadFlow,
    target: Option<String>,
) {
    // Leaving an unsaved create form discards it, so the clobber guard in
    // `should_install_form` does not then block the destination entry's form.
    if app.form.as_ref().map(|f| f.is_new()).unwrap_or(false) {
        app.form = None;
    }
    app.last_seen_leaf = target.clone();
    match target {
        Some(dn) => {
            let _ = read_flow.request_entry(worker, &dn, None);
        }
        None => app.form = None,
    }
}

/// Reconcile UI deltas each tick: a tree-selection branch switch, a search
/// filter change, and a leaf-selection change (which fires a base-read whose
/// result fills the form). No dirty guard yet (that is P4).
impl super::Ctx<'_> {
    pub(crate) fn reconcile(&mut self, structure: &Structure) {
        let app = &mut *self.app;
        let worker = self.worker;
        let read_flow = &mut *self.read_flow;
        let search = app.search.value().to_string();

        // 1) Tree selection changed → switch the leaf pane to that branch.
        if let Some(sel) = app.tree_state.selected().last().cloned() {
            if sel != app.current_branch && structure.get(&sel).is_some() {
                app.current_branch = sel;
                app.rows = compute_rows(structure, &app.current_branch, &search, &app.label_rules);
                app.leaf_sel = 0;
                app.last_seen_leaf = None;
            }
        }

        // 2) Search string changed → recompute the rows, keep the selection in range.
        if search != app.last_search {
            app.last_search = search.clone();
            app.rows = compute_rows(structure, &app.current_branch, &search, &app.label_rules);
            if app.leaf_sel >= app.rows.len() {
                app.leaf_sel = app.rows.len().saturating_sub(1);
            }
        }

        // 3) Selected leaf DN changed → dirty guard, then base-read it into the form
        //    (or clear it). A dirty form opens the Save/Discard/Stay guard instead of
        //    navigating; the guard carries the target and resolves in `guard_key`.
        let sel_dn = app.rows.get(app.leaf_sel).map(|(_, dn)| dn.clone());
        if sel_dn != app.last_seen_leaf {
            let dirty = app.form.as_ref().map(|f| f.is_dirty()).unwrap_or(false);
            if dirty {
                // Do NOT advance last_seen yet — the guard remembers the target.
                app.overlay = Some(Overlay::Guard {
                    intent: GuardIntent::Nav(sel_dn),
                });
                return;
            }
            navigate_to(app, worker, read_flow, sel_dn);
        }
    }
}

/// If the form has unsaved edits, open the Save/Discard/Stay guard carrying
/// `intent` and return `true` (the caller should stop). Otherwise return `false`.
pub(crate) fn guard_if_dirty(app: &mut App, intent: GuardIntent) -> bool {
    if app.form.as_ref().map(|f| f.is_dirty()).unwrap_or(false) {
        app.overlay = Some(Overlay::Guard { intent });
        true
    } else {
        false
    }
}

/// The entry's objectClass values, read from a built form's baseline
/// (case-insensitive). Needed by the write path's client-side validation.
pub(crate) fn object_classes_of(form: &EditForm) -> Vec<String> {
    form.baseline
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("objectClass"))
        .map(|(_, v)| v.clone())
        .unwrap_or_default()
}

/// Build an edit form for a loaded entry and, when the entry is an instance of a
/// password-profile, inject the masked password + confirm fields (suppressing the
/// schema's password field), so the password can be changed inline. Skipped in
/// read-only sessions — the injected field is editable. The single edit-form
/// build seam used by the read flow and the post-combined-save reload.
pub(crate) fn build_loaded_form(
    model: &crate::ui::form::FormModel,
    schema: &SchemaModel,
    read_only: bool,
    pickers: &[crate::config::relation::ResolvedPicker],
    profiles: &[EntryProfile],
) -> EditForm {
    let mut form = build_edit_form(model, schema, read_only);
    if !read_only {
        let ocs = object_classes_of(&form);
        if let Some(spec) = profile_for_entry(profiles, &ocs).and_then(|p| p.password.as_ref()) {
            crate::ui::edit_form::inject_password_fields(&mut form, spec);
        }
    }
    // Tag picker-bound fields so Enter opens the unified picker overlay.
    let ocs = object_classes_of(&form);
    crate::ui::edit_form::tag_picker_fields(&mut form, pickers, &ocs, read_only);
    // Final step: order fields after injection/tagging set secret/picker flags.
    crate::ui::edit_form::order_fields(&mut form);
    form
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::app::build_new_entry_form;
    use crate::ui::app::test_support::*;

    #[test]
    fn should_install_blocks_a_late_read_over_a_create_form() {
        let mut app = bare_app(false);
        app.last_seen_leaf = Some("uid=bob,ou=people,dc=example,dc=org".to_string());
        app.form = Some(build_new_entry_form(
            &user_schema(),
            &create_user_profile(),
            &[],
            0,
            "ou=people,dc=example,dc=org".to_string(),
        ));
        // A base-read for the prior selection must NOT clobber the create form.
        assert!(!should_install_form(
            &app,
            "uid=bob,ou=people,dc=example,dc=org"
        ));
    }

    #[test]
    fn should_install_allows_matching_edit_form_without_overlay() {
        let mut app = with_form(bare_app(false), "cn=Alice,dc=example,dc=org");
        app.last_seen_leaf = Some("cn=Alice,dc=example,dc=org".to_string());
        assert!(should_install_form(&app, "cn=Alice,dc=example,dc=org"));
        // An open overlay blocks installation.
        app.overlay = Some(Overlay::Error {
            text: "x".to_string(),
        });
        assert!(!should_install_form(&app, "cn=Alice,dc=example,dc=org"));
    }

    #[test]
    fn revert_discards_an_unsaved_create_form() {
        let mut app = bare_app(false);
        app.form = Some(build_new_entry_form(
            &user_schema(),
            &create_user_profile(),
            &[],
            0,
            "ou=people,dc=example,dc=org".to_string(),
        ));
        revert_form(&mut app);
        assert!(app.form.is_none(), "create form discarded on cancel");
    }
}
