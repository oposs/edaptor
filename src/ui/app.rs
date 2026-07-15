//! Program assembly: desktop, menu bar, status line, three-pane splitter, pump.

use tvision_rs::{
    self as tv, alt, Command, Constraints, Desktop, Program, Rect, Splitter, StatusDef, StatusLine,
    SystemClock, View, Window,
};

use crate::form::validate::format_validation_errors;
use crate::ui::dialog::{confirm, error, guard, guard_decision, GuardDecision};
use crate::ui::help_ctx::hint_for;
use crate::ui::panes::{
    form::FormPane,
    leaf::LeafPane,
    tree::{build_branch_nodes, TreePane},
};
use crate::ui::pump::PumpView;
use crate::ui::state::GuardTarget;
use crate::ui::widget::{widget_for, Activation};
use crate::ui::{Shared, ACTIVATE, CREATE, GUARD_NAV, REQUEST_QUIT, SAVE, SHOW_ERROR};
use crate::workflows::save::PrepareSave;

fn init_status_line(r: Rect) -> Option<Box<dyn View>> {
    let mut r = r;
    r.a.y = r.b.y - 1;
    let defs = StatusDef::list()
        .def_all(|d| {
            d.item("~Alt-N~ New", alt('n'), CREATE)
                .item("~Alt-S~ Save", alt('s'), SAVE)
                .item("~Alt-X~ Exit", alt('x'), REQUEST_QUIT)
        })
        .build();
    Some(Box::new(StatusLine::new(r, defs).with_hint(hint_for)))
}

fn init_menu_bar(r: Rect) -> Option<Box<dyn View>> {
    let mut r = r;
    r.b.y = r.a.y + 1;
    let menu = tv::Menu::builder()
        .submenu("~F~ile", alt('f'), |m| {
            m.command_key("~N~ew", CREATE, alt('n'), "Alt-N")
                .command_key("~S~ave", SAVE, alt('s'), "Alt-S")
                .command_key("E~x~it", REQUEST_QUIT, alt('x'), "Alt-X")
        })
        .build();
    Some(Box::new(tv::MenuBar::new(r, menu)))
}

/// Whether `do_save` reached the point of submitting the write to the worker.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SaveOutcome {
    /// The write was submitted (user confirmed the LDIF preview).
    Submitted,
    /// The save did not proceed: no form, no changes, validation error, or the
    /// user cancelled the confirm dialog.
    NotSubmitted,
}

/// Snap the leaf highlight back to the pinned form and clear nav targets.
/// Called when the guard→Save path does not submit (cancelled confirm == Stay).
/// Pure (no ctx); unit-tested.
pub(crate) fn apply_cancelled_guard_save(st: &mut crate::ui::state::UiState) {
    st.set_leaf_row = st.current_leaf_row();
    st.guard_target = None;
    st.pending_nav = None;
}

/// Snap the tree highlight back to `current_branch` and clear the guard target.
/// Called on guard "Stay" for a Branch target. Pure (no ctx); unit-tested.
pub(crate) fn apply_branch_guard_stay(st: &mut crate::ui::state::UiState) {
    st.set_tree_row = st.current_branch_row();
    st.guard_target = None;
}

/// What the save flow should do for a given prepare result.
pub(crate) enum SaveAction {
    Status(String),
    Error(String),
    Confirm(String), // the LDIF to preview
}

/// Pure classification of a `PrepareSave` into a dispatch action.
pub(crate) fn save_flow_action(prepared: &PrepareSave) -> SaveAction {
    match prepared {
        PrepareSave::NoChanges => SaveAction::Status("No changes.".to_string()),
        PrepareSave::Invalid(errs) => SaveAction::Error(format_validation_errors(errs)),
        PrepareSave::DiffError(e) => SaveAction::Error(e.clone()),
        PrepareSave::Ready { ldif, .. } => SaveAction::Confirm(ldif.clone()),
    }
}

/// Strip the staged-password sentinel from `pending_pw_attrs` entries in `attrs`.
/// Called when `fold_create_password` returns `None` (no matching widget) but a
/// password was staged. Restricted to the staged attr names so a real user value
/// that coincidentally matches the sentinel in another attr is never touched.
pub(crate) fn strip_sentinel_from_attrs(
    attrs: &mut std::collections::BTreeMap<String, Vec<String>>,
    pending_pw_attrs: &[String],
) {
    use crate::workflows::write_flow::STAGED_PASSWORD_SENTINEL;
    for attr in pending_pw_attrs {
        let has_sentinel = attrs
            .get(attr)
            .map(|vs| vs.iter().any(|v| v == STAGED_PASSWORD_SENTINEL))
            .unwrap_or(false);
        if has_sentinel {
            attrs.remove(attr);
        }
    }
}

/// The single seam that opens modal dialogs (has `&mut Program`). Triggered by
/// commands posted from panes / the pump.
pub(crate) fn dispatch(prog: &mut Program, cmd: Command, state: &Shared) {
    if cmd == SAVE {
        let is_create = matches!(
            state.borrow().edit_form.as_ref().map(|f| &f.mode),
            Some(crate::workflows::edit_form::FormMode::Create { .. })
        );
        if is_create {
            do_create(prog, state);
        } else {
            let _ = do_save(prog, state, None, false);
        }
    } else if cmd == ACTIVATE {
        // Open a field's modal editor. The pane recorded which field.
        let idx = state.borrow_mut().activate_field.take();
        let Some(idx) = idx else {
            return;
        };
        // sambaSID: immediate compute, no modal. Needs the sibling `uidNumber`
        // value + `samba_domain` — context the FieldWidget trait can't see — so
        // it's handled here as a dispatch special-case rather than via `activate`.
        let is_sid = {
            let st = state.borrow();
            st.edit_form
                .as_ref()
                .and_then(|f| f.fields.get(idx))
                .map(|f| {
                    matches!(
                        f.widget_binding,
                        Some(crate::config::widget::WidgetKind::SambaSid)
                    )
                })
                .unwrap_or(false)
        };
        if is_sid {
            // Compute into a local, dropping the borrow before any mutation/exec.
            let res = {
                let st = state.borrow();
                st.edit_form.as_ref().map(|form| {
                    crate::workflows::samba_compute::samba_sid_for_form(
                        form,
                        st.samba_domain.as_ref(),
                    )
                })
            };
            match res {
                Some(Ok(sid)) => {
                    state
                        .borrow_mut()
                        .apply_commit(idx, crate::ui::widget::CommitOutcome::SetValues(vec![sid]));
                }
                Some(Err(msg)) => {
                    let (view, ok) = error::build(&msg);
                    prog.exec_view_focused(view, ok);
                }
                None => {}
            }
            return;
        }
        // Build the editor from the field (drops the borrow before exec_view).
        let editor = {
            let st = state.borrow();
            st.edit_form
                .as_ref()
                .and_then(|f| f.fields.get(idx))
                .and_then(|field| match widget_for(field).activate(field) {
                    Activation::Modal(ed) => Some(ed),
                    Activation::Inline => None,
                })
        };
        let Some(editor) = editor else {
            return;
        };
        // Build the view (schema borrowed; Shared is an Rc clone, not a borrow).
        let (view, focus) = {
            let st = state.borrow();
            editor.into_view(st.read_flow.schema(), state.clone())
        };
        let answer = prog.exec_view_focused(view, focus);
        if answer == Command::OK {
            let outcome = state.borrow_mut().staged_commit.take();
            if let Some(outcome) = outcome {
                state.borrow_mut().apply_commit(idx, outcome);
            }
        } else {
            state.borrow_mut().staged_commit = None;
        }
    } else if cmd == GUARD_NAV {
        // A dirty-blocked navigation: ask, then act on the stashed target per variant.
        let target = state.borrow().guard_target.clone(); // Option<GuardTarget>
        match run_guard(prog) {
            GuardDecision::Save => {
                // For Leaf: pass dn+ocs as the post-save nav; for Branch: just persist.
                let nav = match &target {
                    Some(GuardTarget::Leaf(dn, ocs)) => Some((dn.clone(), ocs.clone())),
                    _ => None, // branch save: persist, then tree re-requests
                };
                if do_save(prog, state, nav, false) == SaveOutcome::NotSubmitted {
                    // Cancelled confirm or no-op: revert highlight to the pinned form.
                    let mut st = state.borrow_mut();
                    match target {
                        Some(GuardTarget::Branch(_)) => apply_branch_guard_stay(&mut st),
                        _ => apply_cancelled_guard_save(&mut st),
                    }
                } else if let Some(GuardTarget::Branch(dn)) = target {
                    // Save submitted: switch the branch now (form will reload clean).
                    let mut st = state.borrow_mut();
                    st.commit_branch(dn);
                    st.guard_target = None;
                }
            }
            GuardDecision::Discard => {
                // discard_edits sets form_needs_render; re-read drives REFRESH via pump.
                discard_edits(state);
                match target {
                    Some(GuardTarget::Leaf(dn, ocs)) => state.borrow_mut().reread_public(&dn, &ocs),
                    Some(GuardTarget::Branch(dn)) => {
                        state.borrow_mut().commit_branch(dn);
                    }
                    None => {}
                }
                state.borrow_mut().guard_target = None;
            }
            GuardDecision::Stay => {
                // Keep editing the pinned form; snap the highlight back so it agrees.
                let mut st = state.borrow_mut();
                match target {
                    Some(GuardTarget::Branch(_)) => apply_branch_guard_stay(&mut st),
                    _ => {
                        st.set_leaf_row = st.current_leaf_row();
                    }
                }
                st.guard_target = None;
            }
        }
    } else if cmd == REQUEST_QUIT {
        let dirty = state
            .borrow()
            .edit_form
            .as_ref()
            .map(|f| f.is_dirty())
            .unwrap_or(false);
        if !dirty {
            prog.end_modal(Command::QUIT); // sets end_state → run loop ends
            return;
        }
        match run_guard(prog) {
            GuardDecision::Save => {
                let _ = do_save(prog, state, None, true);
            }
            GuardDecision::Discard => prog.end_modal(Command::QUIT),
            GuardDecision::Stay => {}
        }
    } else if cmd == CREATE {
        // Container = the current branch.
        let container = state.borrow().current_branch.clone();
        let Some(container) = container else {
            state.borrow_mut().status = "Select a container first.".into();
            return;
        };
        let idxs = {
            let st = state.borrow();
            crate::workflows::create::profiles_for_container(&st.profiles, &container)
        };
        match idxs.as_slice() {
            [] => {
                state.borrow_mut().status = "No profile for this container.".into();
            }
            [only] => open_create(state, *only, &container),
            _ => {
                // >1: run the chooser, then open the chosen profile.
                let names: Vec<String> = {
                    let st = state.borrow();
                    idxs.iter().map(|i| st.profiles[*i].name.clone()).collect()
                };
                let (view, focus) = crate::ui::dialog::profile_chooser::build(names, state.clone());
                if prog.exec_view_focused(view, focus) == Command::OK {
                    let chosen = state.borrow_mut().chosen_profile.take();
                    if let Some(rel) = chosen {
                        if let Some(idx) = idxs.get(rel) {
                            open_create(state, *idx, &container);
                        }
                    }
                } else {
                    state.borrow_mut().chosen_profile = None;
                }
            }
        }
    } else if cmd == SHOW_ERROR {
        let msg = state.borrow_mut().last_write_error.take();
        if let Some(msg) = msg {
            let (view, ok) = error::build(&msg);
            prog.exec_view_focused(view, ok);
        }
    }
}

/// Build a create-mode form for `profile_idx` under `container`, install it, and
/// post a background scan for every autonumber field (`‹allocating…›` placeholder
/// while the scan is in flight).
fn open_create(state: &Shared, profile_idx: usize, container: &str) {
    let form_and_reqs = {
        let st = state.borrow();
        let schema = st.read_flow.schema();
        let profile = &st.profiles[profile_idx];
        crate::workflows::create::build_create_form(schema, profile, profile_idx, container)
    };
    let (mut form, autonum) = form_and_reqs;
    // Set placeholder text in each autonumber field before installing the form.
    for (attr, _, _) in &autonum {
        if let Some(f) = form
            .fields
            .iter_mut()
            .find(|f| f.label.eq_ignore_ascii_case(attr))
        {
            f.values = vec![crate::ui::state::ALLOC_PLACEHOLDER.to_string()];
        }
    }
    // Apply profile-driven widget bindings (Password / Choice / Picker / …) before
    // installing the form. The borrow is released before the mut-borrow below.
    {
        let st = state.borrow();
        let ocs = form.object_classes.clone();
        let samba_enabled = st.samba_domain.is_some();
        let resolver = crate::config::resolver::WidgetResolver::new(
            st.read_flow.schema(),
            &st.profiles,
            &st.resolved_widgets,
            samba_enabled,
        );
        crate::workflows::widget_bind::apply_widget_bindings(&mut form, &resolver, &ocs);
    }
    // Build the create-mode live-template latches from the profile's defaults.
    let live = {
        let st = state.borrow();
        crate::config::defaults::live_templates(&st.profiles[profile_idx].defaults)
    };
    let mut st = state.borrow_mut();
    st.edit_form = Some(form);
    st.live_templates = live;
    st.form_needs_render = true;
    // Post a background scan for each autonumber field (split-borrow idiom: worker
    // and alloc_flow are borrowed disjointly from st).
    if !autonum.is_empty() {
        let base_dn = st.base_dn.clone();
        let crate::ui::state::UiState {
            worker, alloc_flow, ..
        } = &mut *st;
        if let Some(w) = worker.as_ref() {
            for (attr, min, max) in &autonum {
                let _ = alloc_flow.request(w, &base_dn, attr, *min, *max);
            }
        }
    }
}

/// Run the guard modal and decode the answer. (`exec_view` re-enters the loop; the
/// pump keeps draining, so an in-flight write still completes.)
fn run_guard(prog: &mut Program) -> GuardDecision {
    let (view, save) = guard::build();
    let answer = prog.exec_view_focused(view, save);
    guard_decision(answer)
}

/// Prepare → (Status | Error | Confirm→submit). `nav` is a post-save navigation
/// target (guard-nav case); `quit_after` defers a quit until the write lands.
/// Returns `SaveOutcome::Submitted` only when the write was handed off to the
/// worker; every other path returns `SaveOutcome::NotSubmitted`.
fn do_save(
    prog: &mut Program,
    state: &Shared,
    nav: Option<(String, Vec<String>)>,
    quit_after: bool,
) -> SaveOutcome {
    // Fix 3: TLS gate — belt-and-suspenders (editor already refuses when unencrypted).
    {
        let st = state.borrow();
        if st.pending_password.is_some() && !st.connection_encrypted {
            drop(st);
            let (view, ok) = error::build("Changing a password requires an encrypted connection.");
            prog.exec_view_focused(view, ok);
            return SaveOutcome::NotSubmitted;
        }
    }
    // Combined membership save: when a fan-out (membership) field changed, plan a
    // multi-entry write (own MODIFY + one MODIFY per touched group) instead of a
    // single-entry MODIFY. A membership change MUST take this branch — the
    // overlay-maintained back-ref (e.g. `memberOf`) is never written directly.
    let is_combined = {
        let st = state.borrow();
        st.edit_form
            .as_ref()
            .map(form_has_fanout_change)
            .unwrap_or(false)
    };
    if is_combined {
        return do_combined_save(prog, state, nav, quit_after);
    }
    // 1. Prepare (borrow, compute, drop borrow before any exec_view / submit).
    let prepared = {
        let st = state.borrow();
        match st.edit_form.as_ref() {
            None => return SaveOutcome::NotSubmitted,
            Some(form) => st.write_flow.prepare(
                form,
                st.read_flow.schema(),
                st.pending_password.as_deref(),
                &st.resolved_widgets,
            ),
        }
    };
    match save_flow_action(&prepared) {
        SaveAction::Status(s) => {
            let mut st = state.borrow_mut();
            st.status = s;
            st.guard_target = None;
            st.form_needs_render = true; // repaints on the next pump tick
            SaveOutcome::NotSubmitted
        }
        SaveAction::Error(text) => {
            let (view, ok) = error::build(&text);
            prog.exec_view_focused(view, ok);
            SaveOutcome::NotSubmitted
        }
        SaveAction::Confirm(ldif) => {
            // Focus the Save button so Enter confirms — without it the modal opens
            // with Cancel focused (firstMatch picks the last-inserted selectable).
            let (view, save) = confirm::build(&ldif);
            if prog.exec_view_focused(view, save) != Command::OK {
                return SaveOutcome::NotSubmitted; // Cancel: keep editing.
            }
            // 2. Submit the plan we prepared. Re-extract Ready for the plan/dn.
            if let PrepareSave::Ready { plan, dn, .. } = prepared {
                let mut st = state.borrow_mut();
                st.pending_nav = nav;
                st.guard_target = None;
                st.pending_password = None; // cleartext consumed; clear before worker picks it up
                let crate::ui::state::UiState {
                    worker, write_flow, ..
                } = &mut *st;
                if let Some(w) = worker.as_ref() {
                    let _ = write_flow.submit(w, plan, &dn, quit_after);
                } else {
                    return SaveOutcome::NotSubmitted;
                }
            }
            SaveOutcome::Submitted
        }
    }
}

/// True when the form has a fan-out (membership) field whose current value set
/// differs from its baseline — the trigger for the combined-save path.
fn form_has_fanout_change(form: &crate::workflows::edit_form::EditForm) -> bool {
    let fanout = form.fanout_labels();
    if fanout.is_empty() {
        return false;
    }
    form.fields.iter().any(|f| {
        fanout.contains(&f.label)
            && !crate::workflows::edit_form::value_set_eq(&f.current_values(), &f.baseline)
    })
}

/// Combined membership save: plan a multi-entry write (own MODIFY + one MODIFY per
/// touched group), confirm the combined LDIF, then submit the fan-out batch.
/// Mirrors [`do_save`]'s borrow discipline — the planning borrow drops before any
/// `exec_view_focused`; the submit takes a fresh `borrow_mut` scoped so the
/// `UiState` destructure drops before the dialog calls.
fn do_combined_save(
    prog: &mut Program,
    state: &Shared,
    nav: Option<(String, Vec<String>)>,
    quit_after: bool,
) -> SaveOutcome {
    use crate::workflows::save::PlanCombined;
    // 1. Plan (borrow, compute, drop borrow before any exec_view / submit).
    let plan = {
        let st = state.borrow();
        match st.edit_form.as_ref() {
            None => return SaveOutcome::NotSubmitted,
            Some(form) => st.write_flow.prepare_combined(
                form,
                st.read_flow.schema(),
                st.pending_password.as_deref(),
                &st.resolved_widgets,
            ),
        }
    };
    match plan {
        PlanCombined::NoChanges => {
            let mut st = state.borrow_mut();
            st.status = "No changes.".to_string();
            st.guard_target = None;
            st.form_needs_render = true; // repaints on the next pump tick
            SaveOutcome::NotSubmitted
        }
        PlanCombined::Invalid(errs) => {
            let (view, ok) = error::build(&format_validation_errors(&errs));
            prog.exec_view_focused(view, ok);
            SaveOutcome::NotSubmitted
        }
        PlanCombined::DiffError(e) => {
            let (view, ok) = error::build(&e);
            prog.exec_view_focused(view, ok);
            SaveOutcome::NotSubmitted
        }
        PlanCombined::RenameWithMembershipUnsupported => {
            let (view, ok) = error::build(
                "Rename and membership changes can't be saved together — \
                 do them as separate saves.",
            );
            prog.exec_view_focused(view, ok);
            SaveOutcome::NotSubmitted
        }
        PlanCombined::Ready(combined) => {
            let reread_dn = combined.own_dn.clone();
            // M5c: live, schema-gated group-member fetch (blocking) so last-member
            // pre-validation runs client-side. Only MUST-membership groups are
            // populated; MAY groups (e.g. posixGroup memberUid) are exempt.
            let group_members = {
                // Safe to hold this read borrow across the blocking worker.request: it's a
                // synchronous channel round-trip in dispatch (no event-loop pump → no reentrant borrow).
                let st = state.borrow();
                match (st.worker.as_ref(), st.edit_form.as_ref()) {
                    (Some(w), Some(_)) => {
                        crate::workflows::write_flow::fetch_group_members_for_must(
                            w,
                            st.read_flow.schema(),
                            &combined.fanout,
                        )
                    }
                    _ => std::collections::HashMap::new(),
                }
            };
            // Refuse BEFORE showing the confirm if a removal would empty a MUST group.
            if let Some(msg) = crate::workflows::save::last_member_block(
                &combined.fanout,
                &group_members,
                &combined.own_dn,
            ) {
                let (view, ok) = error::build(&msg);
                prog.exec_view_focused(view, ok);
                return SaveOutcome::NotSubmitted;
            }
            // Focus the Save button so Enter confirms (firstMatch would pick Cancel).
            let (view, save) = confirm::build(&combined.ldif);
            if prog.exec_view_focused(view, save) != Command::OK {
                return SaveOutcome::NotSubmitted; // Cancel: keep editing.
            }
            // Submit the batch. Scope the borrow so the `UiState` destructure drops
            // before any error dialog `exec_view_focused`. `submit_combined` re-runs
            // `last_member_block` as defense-in-depth.
            let submit_result = {
                let mut st = state.borrow_mut();
                st.pending_nav = nav;
                st.guard_target = None;
                st.pending_password = None; // cleartext consumed; clear before worker picks it up
                let crate::ui::state::UiState {
                    worker, write_flow, ..
                } = &mut *st;
                worker.as_ref().map(|w| {
                    write_flow.submit_combined(w, combined, &group_members, &reread_dn, quit_after)
                })
            };
            match submit_result {
                Some(Ok(())) => SaveOutcome::Submitted,
                Some(Err(msg)) => {
                    let (view, ok) = error::build(&msg);
                    prog.exec_view_focused(view, ok);
                    SaveOutcome::NotSubmitted
                }
                None => SaveOutcome::NotSubmitted,
            }
        }
    }
}

/// Validate the create-mode form, confirm with the user, then submit an ADD.
/// Borrow discipline: the `plan_create` borrow drops before any `exec_view_focused`
/// call; on OK a fresh `borrow_mut` is taken using the split-borrow idiom.
fn do_create(prog: &mut Program, state: &Shared) {
    use crate::workflows::create::{
        fold_create_password, now_unix_secs_or_zero, plan_create, CreatePrep,
    };
    use crate::workflows::edit_form::FormMode;
    // 1. Compute the plan + extract pending password (borrow drops before exec_view).
    let (prep, pending, pending_pw_attrs, resolved_widgets) = {
        let st = state.borrow();
        let Some(form) = st.edit_form.as_ref() else {
            return;
        };
        let FormMode::Create {
            profile_idx,
            container,
        } = &form.mode
        else {
            return;
        };
        let profile = &st.profiles[*profile_idx];
        let prep = plan_create(
            st.read_flow.schema(),
            profile,
            container,
            &form.to_edit_entry(),
        );
        let pending = st.pending_password.clone();
        let pending_pw_attrs = st.pending_password_attrs.clone();
        let resolved_widgets = st.resolved_widgets.clone();
        (prep, pending, pending_pw_attrs, resolved_widgets)
    };
    match prep {
        CreatePrep::Error(msg) => {
            let (view, ok) = crate::ui::dialog::error::build(&msg);
            prog.exec_view_focused(view, ok);
        }
        CreatePrep::Confirm {
            dn,
            mut attrs,
            ldif,
            ..
        } => {
            // Fix 3: TLS gate — belt-and-suspenders (editor already refuses when unencrypted).
            if pending.is_some() && !state.borrow().connection_encrypted {
                let (view, ok) = crate::ui::dialog::error::build(
                    "Changing a password requires an encrypted connection.",
                );
                prog.exec_view_focused(view, ok);
                return;
            }
            // Fold any staged password into attrs; get the masked LDIF preview.
            let masked = fold_create_password(
                &dn,
                &mut attrs,
                pending.as_deref(),
                &resolved_widgets,
                now_unix_secs_or_zero(),
            );
            // Fix 1: if no password widget matched the entry's object classes, the
            // sentinel was NOT replaced by the real password — strip it so the ADD
            // never sends "••••••" as an attribute value.
            if masked.is_none() && pending.is_some() {
                strip_sentinel_from_attrs(&mut attrs, &pending_pw_attrs);
            }
            let ldif = masked.unwrap_or(ldif);
            let (view, save) = crate::ui::dialog::confirm::build(&ldif);
            if prog.exec_view_focused(view, save) != Command::OK {
                return; // cancel: keep editing the create form.
            }
            let mut st = state.borrow_mut();
            st.pending_password = None; // cleartext consumed; clear before worker picks it up
            let crate::ui::state::UiState {
                worker, write_flow, ..
            } = &mut *st;
            if let Some(w) = worker.as_ref() {
                let _ = write_flow.submit_create(w, &dn, attrs, false);
            }
        }
    }
}

/// Reset every field's edited values back to baseline (drop unsaved edits).
fn discard_edits(state: &Shared) {
    let mut st = state.borrow_mut();
    if let Some(form) = st.edit_form.as_mut() {
        for f in &mut form.fields {
            f.values = f.baseline.clone();
        }
    }
    st.form_needs_render = true;
}

fn init_desktop(r: Rect, state: Shared) -> Option<Box<dyn View>> {
    let mut r = r;
    r.a.y += 1; // below menu bar
    r.b.y -= 1; // above status line
    let mut desktop = Desktop::new(r, |br| Some(Desktop::init_background(br)));

    let win_rect = Rect::new(r.a.x + 1, r.a.y, r.b.x - 1, r.b.y);
    let mut win = Window::new(win_rect, Some("edaptor".to_string()), 1);
    // No drop shadow: as a desktop-filling frameless window it has nothing to cast
    // onto, and the shadow would otherwise paint a one-cell strip over the desktop
    // background along the right and bottom edges.
    win.state_mut().state.shadow = false;
    let ext = win.state().get_extent();
    let interior = Rect::new(1, 1, ext.b.x - 1, ext.b.y - 1);
    let width = (interior.b.x - interior.a.x).max(8) as usize;

    // Build the branch tree and record the DFS DN map.
    // Take the immutable borrow, destructure the result, DROP borrow, then mutate.
    let (root, dn_map) = {
        let st = state.borrow();
        build_branch_nodes(&st, width / 3)
    }; // borrow dropped here
    state.borrow_mut().branch_dns = dn_map;

    let tree: Box<dyn View> = Box::new(TreePane::new(interior, root, state.clone()));
    let leaf: Box<dyn View> = Box::new(LeafPane::new(interior, state.clone()));
    let form: Box<dyn View> = Box::new(FormPane::new(interior, state.clone()));

    // Left column: the branch tree (top) stacked over the selected branch's
    // members (bottom); the form fills the right column. A nested rows-splitter
    // inside the outer cols-splitter — `.joined()` on the outer cascades to it.
    let left: Box<dyn View> = Box::new(
        Splitter::rows()
            .pane(tree, Constraints::flex().min(3))
            .pane(leaf, Constraints::flex().min(3)),
    );

    // Split the width one-third / two-thirds: the left column (tree + members)
    // takes a single share, the form takes two — so the entry editor gets the
    // larger 2/3 of the horizontal axis.
    let split = Splitter::cols()
        .pane(left, Constraints::weight(1).min(16))
        .pane(form, Constraints::weight(2).min(20))
        .joined();

    let split_id = win.insert_child(Box::new(split));
    if let Some(v) = win.child_mut(split_id) {
        // The splitter's grow_mode defaults to { hi_x, hi_y }, so its bottom-right
        // tracks the window when the pump flips it to frameless fullscreen.
        v.change_bounds(interior);
    }
    win.insert_child(Box::new(PumpView::new(state.clone())));

    desktop.insert_view(Box::new(win));
    Some(Box::new(desktop))
}

pub(crate) fn build_program(backend: Box<dyn tv::Backend>, state: Shared) -> Program {
    let s = state.clone();
    Program::new(
        backend,
        Box::new(SystemClock::new()),
        crate::ui::theme::edaptor_theme(),
        move |r| init_desktop(r, s.clone()),
        init_status_line,
        init_menu_bar,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::form::validate::ValidationError;
    use crate::workflows::save::PrepareSave;

    #[test]
    fn guard_stay_on_branch_target_reverts_tree() {
        use crate::ldap::worker::RawSubschema;
        use crate::schema::SchemaModel;
        use crate::ui::state::{GuardTarget, UiState};
        use crate::workflows::structure::Structure;
        let structure = Structure::build("dc=x", vec![]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.branch_dns = vec!["dc=x".into(), "ou=p,dc=x".into(), "ou=q,dc=x".into()];
        st.current_branch = Some("ou=p,dc=x".into());
        st.guard_target = Some(GuardTarget::Branch("ou=q,dc=x".into()));

        apply_branch_guard_stay(&mut st);

        assert_eq!(
            st.set_tree_row,
            st.current_branch_row(),
            "revert tree to current branch"
        );
        assert!(st.guard_target.is_none());
    }

    #[test]
    fn cancelled_guard_save_snaps_highlight_back() {
        // The guard→Save path that does NOT submit must request a snap-back to the
        // pinned form's row and clear the stashed nav targets (like Stay).
        use crate::ldap::worker::RawSubschema;
        use crate::schema::SchemaModel;
        use crate::ui::state::UiState;
        use crate::workflows::structure::{Structure, StructureInput};
        use std::collections::BTreeMap;

        let inputs = vec![
            StructureInput {
                dn: "dc=x".into(),
                cn: None,
                description: None,
                object_classes: vec![],
                attrs: BTreeMap::new(),
            },
            StructureInput {
                dn: "ou=p,dc=x".into(),
                cn: None,
                description: None,
                object_classes: vec![],
                attrs: BTreeMap::new(),
            },
            StructureInput {
                dn: "cn=a,ou=p,dc=x".into(),
                cn: Some("a".into()),
                description: None,
                object_classes: vec![],
                attrs: BTreeMap::new(),
            },
        ];
        let structure = Structure::build("dc=x", inputs);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.current_branch = Some("ou=p,dc=x".into());
        st.current_leaf = Some("cn=a,ou=p,dc=x".into());
        st.guard_target = Some(crate::ui::state::GuardTarget::Leaf(
            "cn=b,ou=p,dc=x".into(),
            vec![],
        ));
        st.pending_nav = Some(("cn=b,ou=p,dc=x".into(), vec![]));

        apply_cancelled_guard_save(&mut st);

        assert_eq!(
            st.set_leaf_row,
            st.current_leaf_row(),
            "snap back to the pinned form's row"
        );
        assert!(st.guard_target.is_none());
        assert!(st.pending_nav.is_none());
    }

    /// Fix 1 TDD (RED→GREEN): strip_sentinel_from_attrs removes the sentinel
    /// from staged password attrs and leaves real values and other attrs untouched.
    #[test]
    fn strip_sentinel_removes_only_staged_sentinel_attrs() {
        use crate::workflows::write_flow::STAGED_PASSWORD_SENTINEL;
        use std::collections::BTreeMap;

        let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        attrs.insert(
            "userPassword".into(),
            vec![STAGED_PASSWORD_SENTINEL.to_string()],
        );
        attrs.insert("cn".into(), vec!["Alice".into()]);

        strip_sentinel_from_attrs(&mut attrs, &["userPassword".to_string()]);

        assert!(
            !attrs.contains_key("userPassword"),
            "sentinel attr must be stripped from attrs"
        );
        assert_eq!(
            attrs.get("cn"),
            Some(&vec!["Alice".into()]),
            "cn must be untouched"
        );
    }

    /// Fix 1 TDD: a real password value (not the sentinel) must never be stripped.
    #[test]
    fn strip_sentinel_preserves_real_password_value() {
        use std::collections::BTreeMap;

        let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        attrs.insert("userPassword".into(), vec!["{SSHA}realhash".to_string()]);

        strip_sentinel_from_attrs(&mut attrs, &["userPassword".to_string()]);

        assert!(
            attrs.contains_key("userPassword"),
            "real hash must not be stripped"
        );
        assert_eq!(
            attrs.get("userPassword"),
            Some(&vec!["{SSHA}realhash".to_string()])
        );
    }

    #[test]
    fn save_flow_action_classifies_prepare() {
        assert!(matches!(
            save_flow_action(&PrepareSave::NoChanges),
            SaveAction::Status(_)
        ));
        assert!(matches!(
            save_flow_action(&PrepareSave::Invalid(vec![ValidationError::MissingMust(
                "cn".into()
            )])),
            SaveAction::Error(_)
        ));
        assert!(matches!(
            save_flow_action(&PrepareSave::DiffError("bad".into())),
            SaveAction::Error(_)
        ));
        let ready = PrepareSave::Ready {
            plan: crate::form::validate::SavePlan::Nothing,
            dn: "d".into(),
            ldif: "L".into(),
        };
        assert!(matches!(save_flow_action(&ready), SaveAction::Confirm(_)));
    }
}
