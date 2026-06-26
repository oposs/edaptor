//! Program assembly: desktop, menu bar, status line, three-pane splitter, pump.

use tvision_rs::{
    self as tv, alt, Command, Constraints, Desktop, Program, Rect, Splitter, StatusDef, StatusLine,
    SystemClock, Theme, View, Window,
};

use crate::form::validate::format_validation_errors;
use crate::tui::dialog::{confirm, error, guard, guard_decision, GuardDecision};
use crate::tui::panes::{
    form::FormPane,
    leaf::LeafPane,
    tree::{build_branch_nodes, TreePane},
};
use crate::tui::pump::PumpView;
use crate::tui::{Shared, GUARD_NAV, REQUEST_QUIT, SAVE, SHOW_ERROR};
use crate::workflows::save::PrepareSave;

fn init_status_line(r: Rect) -> Option<Box<dyn View>> {
    let mut r = r;
    r.a.y = r.b.y - 1;
    let defs = StatusDef::list()
        .def_all(|d| {
            d.item("~Alt-S~ Save", alt('s'), SAVE)
                .item("~Alt-X~ Exit", alt('x'), REQUEST_QUIT)
        })
        .build();
    Some(Box::new(StatusLine::new(r, defs)))
}

fn init_menu_bar(r: Rect) -> Option<Box<dyn View>> {
    let mut r = r;
    r.b.y = r.a.y + 1;
    let menu = tv::Menu::builder()
        .submenu("~F~ile", alt('f'), |m| {
            m.command_key("~S~ave", SAVE, alt('s'), "Alt-S")
                .command_key("E~x~it", REQUEST_QUIT, alt('x'), "Alt-X")
        })
        .build();
    Some(Box::new(tv::MenuBar::new(r, menu)))
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

/// The single seam that opens modal dialogs (has `&mut Program`). Triggered by
/// commands posted from panes / the pump.
pub(crate) fn dispatch(prog: &mut Program, cmd: Command, state: &Shared) {
    if cmd == SAVE {
        do_save(prog, state, None, false);
    } else if cmd == GUARD_NAV {
        // A dirty-blocked navigation: ask, then act on the stashed target.
        let target = state.borrow().guard_target.clone();
        match run_guard(prog) {
            GuardDecision::Save => do_save(prog, state, target, false),
            GuardDecision::Discard => {
                // discard_edits sets form_needs_render; the re-read's worker
                // response drives a REFRESH via the pump — no Program broadcast.
                discard_edits(state);
                if let Some((dn, ocs)) = target {
                    state.borrow_mut().reread_public(&dn, &ocs);
                }
                state.borrow_mut().guard_target = None;
            }
            GuardDecision::Stay => {
                // Keep editing the pinned form; snap the list highlight back to it
                // so highlight and form agree (the move is cancelled).
                let mut st = state.borrow_mut();
                st.guard_target = None;
                st.set_leaf_row = st.current_leaf_row();
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
            GuardDecision::Save => do_save(prog, state, None, true),
            GuardDecision::Discard => prog.end_modal(Command::QUIT),
            GuardDecision::Stay => {}
        }
    } else if cmd == SHOW_ERROR {
        let msg = state.borrow_mut().last_write_error.take();
        if let Some(msg) = msg {
            let (view, ok) = error::build(&msg);
            prog.exec_view_focused(view, ok);
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
fn do_save(
    prog: &mut Program,
    state: &Shared,
    nav: Option<(String, Vec<String>)>,
    quit_after: bool,
) {
    // 1. Prepare (borrow, compute, drop borrow before any exec_view / submit).
    let prepared = {
        let st = state.borrow();
        match st.edit_form.as_ref() {
            None => return,
            Some(form) => st.write_flow.prepare(form, st.read_flow.schema()),
        }
    };
    match save_flow_action(&prepared) {
        SaveAction::Status(s) => {
            let mut st = state.borrow_mut();
            st.status = s;
            st.guard_target = None;
            st.form_needs_render = true; // repaints on the next pump tick
        }
        SaveAction::Error(text) => {
            let (view, ok) = error::build(&text);
            prog.exec_view_focused(view, ok);
        }
        SaveAction::Confirm(ldif) => {
            // Focus the Save button so Enter confirms — without it the modal opens
            // with Cancel focused (firstMatch picks the last-inserted selectable).
            let (view, save) = confirm::build(&ldif);
            if prog.exec_view_focused(view, save) != Command::OK {
                return; // Cancel: keep editing.
            }
            // 2. Submit the plan we prepared. Re-extract Ready for the plan/dn.
            if let PrepareSave::Ready { plan, dn, .. } = prepared {
                let mut st = state.borrow_mut();
                st.pending_nav = nav;
                st.guard_target = None;
                let crate::tui::state::UiState {
                    worker, write_flow, ..
                } = &mut *st;
                if let Some(w) = worker.as_ref() {
                    let _ = write_flow.submit(w, plan, &dn, quit_after);
                }
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

    let split = Splitter::cols()
        .pane(tree, Constraints::flex().min(16))
        .pane(leaf, Constraints::flex().min(16))
        .pane(form, Constraints::flex().min(20))
        .joined();

    let split_id = win.insert_child(Box::new(split));
    if let Some(v) = win.child_mut(split_id) {
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
        Theme::classic_blue(),
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
