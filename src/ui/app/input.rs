//! Key-dispatch and the value/picker editor cluster for the `app` submodule.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tui_prompts::State;

use crate::app::UiAction;
use crate::ui::form_state::{guard_decision, GuardChoice, GuardOutcome};
use crate::workflows::structure::Structure;

use super::overlay::{GuardIntent, Overlay, PendingAction};
use super::value_editor::{open_value_editor, value_editor_key};
use super::{guard_if_dirty, App, Pane};

/// Translate a key into an `App` mutation (gated by the focused pane), returning
/// a [`UiAction`] for the few keys the loop must service with the worker.
pub(crate) fn dispatch_key(
    app: &mut App,
    key: KeyEvent,
    structure: &Structure,
) -> Option<UiAction> {
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    // Global quit. Quitting while the form has unsaved edits opens the guard
    // first (the user picks Save / Discard / Stay) rather than dropping them.
    if (alt && matches!(key.code, KeyCode::Char('x') | KeyCode::Char('X')))
        || (ctrl && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C')))
    {
        if !guard_if_dirty(app, GuardIntent::Quit) {
            app.should_quit = true;
        }
        return None;
    }
    // Focus cycle: Tab forward, Shift-Tab (BackTab) backward. Moving focus OFF a
    // dirty form opens the guard, carrying the destination pane.
    if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
        let dest = if key.code == KeyCode::BackTab {
            prev_pane(app.focus)
        } else {
            next_pane(app.focus)
        };
        if app.focus == Pane::Form && guard_if_dirty(app, GuardIntent::Focus(dest)) {
            return None;
        }
        app.focus = dest;
        return None;
    }
    // Refresh (Alt+R) is allowed even in read-only mode (it only re-reads).
    if alt && matches!(key.code, KeyCode::Char('r') | KeyCode::Char('R')) {
        return Some(UiAction::Refresh);
    }
    // Bare `q` quits, but ONLY from the Tree pane — the search box and form need
    // the key for text entry. Guarded when the form has unsaved edits.
    if app.focus == Pane::Tree && key.code == KeyCode::Char('q') {
        if !guard_if_dirty(app, GuardIntent::Quit) {
            app.should_quit = true;
        }
        return None;
    }
    // Save / Cancel / Create / Delete (writable mode only). Read-only mode
    // suppresses every write affordance (P4-T4). These keys are surfaced in the
    // status-line hints (view::pane_hints); Alt+N creates via the profile chooser.
    // Each arm is Alt-gated so a bare letter still falls through to text entry /
    // type-to-search in the focused-pane match below.
    if !app.read_only && alt {
        match key.code {
            KeyCode::Char('s') | KeyCode::Char('S') => return Some(UiAction::FormSave),
            KeyCode::Char('c') | KeyCode::Char('C') => return Some(UiAction::FormCancel),
            KeyCode::Char('n') | KeyCode::Char('N') => return Some(UiAction::NewEntryChoose),
            // Alt+D deletes the entry currently shown in the form pane (spec §12).
            KeyCode::Char('d') | KeyCode::Char('D') => {
                return app
                    .form
                    .as_ref()
                    .filter(|f| !f.dn.is_empty())
                    .map(|f| UiAction::DeleteEntry(f.dn.clone()));
            }
            _ => {}
        }
    }

    match app.focus {
        Pane::Tree => match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                app.tree_state.key_up();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                app.tree_state.key_down();
            }
            KeyCode::Left => {
                app.tree_state.key_left();
            }
            KeyCode::Right => {
                app.tree_state.key_right();
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                app.tree_state.toggle_selected();
            }
            _ => {}
        },
        Pane::Leaf => match key.code {
            KeyCode::Up => app.leaf_sel = app.leaf_sel.saturating_sub(1),
            KeyCode::Down => app.leaf_sel = next_index(app.leaf_sel, app.rows.len()),
            KeyCode::PageUp => app.leaf_sel = app.leaf_sel.saturating_sub(10),
            KeyCode::PageDown => {
                app.leaf_sel = (app.leaf_sel + 10).min(app.rows.len().saturating_sub(1))
            }
            // Everything else (text, backspace, …) edits the search box.
            _ => {
                app.search.handle_key_event(key);
            }
        },
        Pane::Form => {
            let n = app.form.as_ref().map(|f| f.fields.len()).unwrap_or(0);
            match key.code {
                KeyCode::Up => app.form_focus = app.form_focus.saturating_sub(1),
                KeyCode::Down => app.form_focus = next_index(app.form_focus, n),
                // Scroll follows focus: clamp_scroll (in render_form) is the sole
                // authority for form_scroll, so paging only moves the focus.
                KeyCode::PageUp => app.form_focus = app.form_focus.saturating_sub(10),
                KeyCode::PageDown => {
                    app.form_focus = (app.form_focus + 10).min(n.saturating_sub(1))
                }
                // Enter opens the value-editor popup: free-text rows for a plain
                // multi-value field, or a picker for a picker-bound field
                // (single- or multi-select). Plain single fields: a no-op.
                // A NextNumber field instead allocates via the worker, so it
                // round-trips through the action handler.
                KeyCode::Enter => {
                    if let Some(field_idx) = next_number_field_focused(app) {
                        return Some(UiAction::AllocateNextNumber { field_idx });
                    }
                    open_value_editor(app, structure);
                }
                // Esc cancels a create form (parity with the old modal's Esc); on
                // an edit form Esc is a no-op (Alt+C reverts edits).
                KeyCode::Esc if app.form.as_ref().map(|f| f.is_new()).unwrap_or(false) => {
                    return Some(UiAction::FormCancel)
                }
                // Otherwise edit the focused single-value field inline.
                _ => edit_focused_field(app, key),
            }
        }
    }
    None
}

/// Route a key to the focused field's inline editor, if it is an editable
/// single-value field (multi-value fields are edited via the popup).
fn edit_focused_field(app: &mut App, key: KeyEvent) {
    let focus = app.form_focus;
    if let Some(form) = app.form.as_mut() {
        if let Some(field) = form.fields.get_mut(focus) {
            // The sambaSID / next-number auto-fill widgets are still plain
            // editors — Enter fills, but the user may also type to override.
            let inline_editable = field.widget_binding.is_none()
                || matches!(
                    field.widget_binding,
                    Some(
                        crate::config::widget::WidgetKind::SambaSid
                            | crate::config::widget::WidgetKind::NextNumber { .. }
                    )
                );
            if field.editable && !field.multi && inline_editable {
                field.editor.handle_key_event(key);
            }
        }
    }
}

/// The focused form field's index if it is bound to a [`WidgetKind::NextNumber`]
/// and currently empty. An already-filled field is left alone (Enter is a no-op,
/// like any plain single field) so a re-press cannot silently re-scan to a
/// different number; clear it to re-allocate. Used by the form Enter handler to
/// route allocation through the action layer.
fn next_number_field_focused(app: &App) -> Option<usize> {
    let form = app.form.as_ref()?;
    let idx = app.form_focus;
    let field = form.fields.get(idx)?;
    let is_next_number = matches!(
        field.widget_binding,
        Some(crate::config::widget::WidgetKind::NextNumber { .. })
    );
    if is_next_number && field.editable && field.editor.value().trim().is_empty() {
        Some(idx)
    } else {
        None
    }
}

/// Handle a key while an overlay is open. Returns the action to run when the
/// user confirms a [`Overlay::Confirm`] or resolves a [`Overlay::Guard`];
/// otherwise dismisses / consumes the key.
pub(crate) fn overlay_key(app: &mut App, key: KeyEvent) -> Option<PendingAction> {
    match &app.overlay {
        Some(Overlay::Confirm { .. }) => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                // Take the overlay out to move its action.
                if let Some(Overlay::Confirm { action, .. }) = app.overlay.take() {
                    return Some(action);
                }
                None
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                app.overlay = None;
                None
            }
            _ => None,
        },
        Some(Overlay::Error { .. }) => {
            // Any key dismisses an error.
            app.overlay = None;
            None
        }
        Some(Overlay::ValueEditor(_)) => {
            value_editor_key(app, key);
            None
        }
        Some(Overlay::PasswordEditor(_)) => {
            super::password_editor::password_editor_key(app, key);
            None
        }
        Some(Overlay::Guard { .. }) => guard_key(app, key),
        Some(Overlay::ChooseProfile { .. }) => choose_profile_key(app, key),
        None => None,
    }
}

/// Handle a key in the Alt+N profile chooser: ↑↓ move the selection, Enter
/// resolves to [`PendingAction::OpenCreate`] for the chosen profile, Esc / Alt+C
/// dismisses.
fn choose_profile_key(app: &mut App, key: KeyEvent) -> Option<PendingAction> {
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let Some(Overlay::ChooseProfile { entries, sel }) = app.overlay.as_mut() else {
        return None;
    };
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            *sel = sel.saturating_sub(1);
            None
        }
        KeyCode::Down | KeyCode::Char('j') => {
            *sel = (*sel + 1).min(entries.len().saturating_sub(1));
            None
        }
        KeyCode::Enter => {
            let profile_idx = entries.get(*sel).map(|(i, _)| *i);
            app.overlay = None;
            profile_idx.map(|profile_idx| PendingAction::OpenCreate { profile_idx })
        }
        KeyCode::Esc => {
            app.overlay = None;
            None
        }
        KeyCode::Char('c') | KeyCode::Char('C') if alt => {
            app.overlay = None;
            None
        }
        _ => None,
    }
}

/// Resolve the Save/Discard/Stay guard. Maps the key to a [`GuardChoice`], runs
/// the pure [`guard_decision`], and turns the outcome into a [`PendingAction`]
/// that performs the pending [`GuardIntent`] (or, for Stay, keeps editing).
fn guard_key(app: &mut App, key: KeyEvent) -> Option<PendingAction> {
    // Plain letters and Alt-modified letters both resolve the guard (the modal
    // shows [S]ave / [D]iscard / [C]ancel; Alt+S / Alt+C mirror the global keys).
    let choice = match key.code {
        KeyCode::Char('s') | KeyCode::Char('S') => GuardChoice::Save,
        KeyCode::Char('d') | KeyCode::Char('D') => GuardChoice::Discard,
        KeyCode::Char('c') | KeyCode::Char('C') | KeyCode::Esc => GuardChoice::Stay,
        _ => return None, // ignore unrelated keys; the guard stays open
    };
    let intent = match &app.overlay {
        Some(Overlay::Guard { intent }) => intent.clone(),
        _ => return None,
    };
    app.overlay = None;
    match guard_decision(true, Some(choice)) {
        GuardOutcome::Cancel => {
            // Stay: keep editing. For a selection-change guard, advance last_seen
            // to the moved-to entry so it does not re-fire every tick (the
            // highlight now differs from the form — a known wrinkle). For a
            // focus/quit guard there is nothing to advance.
            if let GuardIntent::Nav(target) = intent {
                app.last_seen_leaf = target;
            }
            None
        }
        GuardOutcome::Proceed => Some(PendingAction::ResolveGuard {
            intent,
            save: false,
        }),
        GuardOutcome::SaveThenProceed => Some(PendingAction::ResolveGuard { intent, save: true }),
    }
}

/// The forward focus cycle: Tree → Leaf → Form → Tree.
fn next_pane(focus: Pane) -> Pane {
    match focus {
        Pane::Tree => Pane::Leaf,
        Pane::Leaf => Pane::Form,
        Pane::Form => Pane::Tree,
    }
}

/// The backward focus cycle (Shift-Tab): Tree → Form → Leaf → Tree.
fn prev_pane(focus: Pane) -> Pane {
    match focus {
        Pane::Tree => Pane::Form,
        Pane::Form => Pane::Leaf,
        Pane::Leaf => Pane::Tree,
    }
}

/// Next selectable index, clamped to `[0, len)` (saturating at the bottom).
fn next_index(cur: usize, len: usize) -> usize {
    (cur + 1).min(len.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::app::test_support::*;

    #[test]
    fn focus_cycles_tree_leaf_form() {
        assert_eq!(next_pane(Pane::Tree), Pane::Leaf);
        assert_eq!(next_pane(Pane::Leaf), Pane::Form);
        assert_eq!(next_pane(Pane::Form), Pane::Tree);
    }

    #[test]
    fn alt_r_refreshes_even_in_read_only() {
        let s = empty_structure();
        assert_eq!(
            dispatch_key(&mut bare_app(true), alt(KeyCode::Char('r')), &s),
            Some(UiAction::Refresh)
        );
        assert_eq!(
            dispatch_key(&mut bare_app(false), alt(KeyCode::Char('r')), &s),
            Some(UiAction::Refresh)
        );
    }

    #[test]
    fn alt_n_opens_the_profile_chooser_when_writable_only() {
        let s = empty_structure();
        assert_eq!(
            dispatch_key(&mut bare_app(false), alt(KeyCode::Char('n')), &s),
            Some(UiAction::NewEntryChoose)
        );
        // Read-only mode suppresses create (P4-T4); the key falls through to nav.
        assert_eq!(
            dispatch_key(&mut bare_app(true), alt(KeyCode::Char('n')), &s),
            None
        );
    }

    #[test]
    fn choose_profile_key_navigates_and_resolves_to_open_create() {
        let mut app = bare_app(false);
        app.overlay = Some(Overlay::ChooseProfile {
            entries: vec![(0, "User".into()), (2, "Group".into())],
            sel: 0,
        });
        // Down moves the selection.
        assert!(choose_profile_key(&mut app, key(KeyCode::Down)).is_none());
        match &app.overlay {
            Some(Overlay::ChooseProfile { sel, .. }) => assert_eq!(*sel, 1),
            _ => panic!("chooser still open"),
        }
        // Enter resolves to OpenCreate for the chosen profile index (2), closing it.
        match choose_profile_key(&mut app, key(KeyCode::Enter)) {
            Some(PendingAction::OpenCreate { profile_idx }) => assert_eq!(profile_idx, 2),
            _ => panic!("expected OpenCreate"),
        }
        assert!(app.overlay.is_none());
    }

    #[test]
    fn choose_profile_key_esc_dismisses() {
        let mut app = bare_app(false);
        app.overlay = Some(Overlay::ChooseProfile {
            entries: vec![(0, "User".into())],
            sel: 0,
        });
        assert!(choose_profile_key(&mut app, key(KeyCode::Esc)).is_none());
        assert!(app.overlay.is_none());
    }

    #[test]
    fn alt_d_deletes_the_form_entry_when_writable() {
        let s = empty_structure();
        let mut app = with_form(bare_app(false), "cn=Alice,dc=example,dc=org");
        assert_eq!(
            dispatch_key(&mut app, alt(KeyCode::Char('d')), &s),
            Some(UiAction::DeleteEntry(
                "cn=Alice,dc=example,dc=org".to_string()
            ))
        );
        // No form → nothing to delete.
        assert_eq!(
            dispatch_key(&mut bare_app(false), alt(KeyCode::Char('d')), &s),
            None
        );
        // Read-only suppresses delete.
        let mut ro = with_form(bare_app(true), "cn=Alice,dc=example,dc=org");
        assert_eq!(dispatch_key(&mut ro, alt(KeyCode::Char('d')), &s), None);
    }

    #[test]
    fn focus_cycles_both_directions() {
        let s = empty_structure();
        assert_eq!(prev_pane(Pane::Tree), Pane::Form);
        assert_eq!(prev_pane(Pane::Form), Pane::Leaf);
        assert_eq!(prev_pane(Pane::Leaf), Pane::Tree);
        // Tab forward and Shift-Tab back are inverses through dispatch_key.
        let mut app = bare_app(false); // no form → no guard
        dispatch_key(
            &mut app,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            &s,
        );
        assert_eq!(app.focus, Pane::Leaf);
        dispatch_key(
            &mut app,
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE),
            &s,
        );
        assert_eq!(app.focus, Pane::Tree);
        dispatch_key(
            &mut app,
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE),
            &s,
        );
        assert_eq!(app.focus, Pane::Form);
    }

    #[test]
    fn tab_off_a_dirty_form_opens_the_focus_guard() {
        // with_form has a value but an empty baseline → it is dirty.
        let mut app = with_form(bare_app(false), "cn=Alice,dc=example,dc=org");
        app.focus = Pane::Form;
        dispatch_key(
            &mut app,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            &empty_structure(),
        );
        // Focus did NOT move; the guard opened carrying the destination pane.
        assert_eq!(app.focus, Pane::Form);
        assert!(matches!(
            app.overlay,
            Some(Overlay::Guard {
                intent: GuardIntent::Focus(Pane::Tree)
            })
        ));
    }

    #[test]
    fn quit_while_dirty_opens_the_guard_else_quits() {
        let altx = || KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT);
        // Clean (no form) → Alt+X quits immediately.
        let mut clean = bare_app(false);
        dispatch_key(&mut clean, altx(), &empty_structure());
        assert!(clean.should_quit);
        // Dirty form → Alt+X opens the quit guard, does NOT quit yet.
        let mut dirty = with_form(bare_app(false), "cn=Alice,dc=example,dc=org");
        dispatch_key(&mut dirty, altx(), &empty_structure());
        assert!(!dirty.should_quit);
        assert!(matches!(
            dirty.overlay,
            Some(Overlay::Guard {
                intent: GuardIntent::Quit
            })
        ));
    }

    #[test]
    fn guard_key_maps_choices_to_intents() {
        let plain = |c| KeyEvent::new(c, KeyModifiers::NONE);
        let nav_guard = || Overlay::Guard {
            intent: GuardIntent::Nav(Some("cn=next".to_string())),
        };

        // Stay (Cancel): no action; for a Nav intent, last_seen advances to the
        // target so the guard does not re-fire.
        let mut app = bare_app(false);
        app.overlay = Some(nav_guard());
        assert!(guard_key(&mut app, plain(KeyCode::Char('c'))).is_none());
        assert_eq!(app.last_seen_leaf.as_deref(), Some("cn=next"));
        assert!(app.overlay.is_none());

        // Discard → ResolveGuard { save: false }.
        let mut app = bare_app(false);
        app.overlay = Some(nav_guard());
        assert!(matches!(
            guard_key(&mut app, plain(KeyCode::Char('d'))),
            Some(PendingAction::ResolveGuard {
                intent: GuardIntent::Nav(Some(t)),
                save: false,
            }) if t == "cn=next"
        ));

        // Save → ResolveGuard { save: true }.
        let mut app = bare_app(false);
        app.overlay = Some(nav_guard());
        assert!(matches!(
            guard_key(&mut app, plain(KeyCode::Char('s'))),
            Some(PendingAction::ResolveGuard { save: true, .. })
        ));
    }

    #[test]
    fn next_index_clamps_at_end() {
        assert_eq!(next_index(0, 3), 1);
        assert_eq!(next_index(2, 3), 2);
        assert_eq!(next_index(0, 0), 0);
    }

    /// Choice fields must NOT accept inline key input — they are edited exclusively
    /// via the overlay opened by Enter. A plain (non-choice) editable single field
    /// must still accept inline edits.
    #[test]
    fn choice_field_ignores_inline_key_input() {
        use crate::config::relation::Cardinality;
        use crate::config::widget::{ChoiceFormat, ChoiceWidget};
        use crate::config::ChoiceOption;
        use crate::schema::FieldKind;
        use crate::ui::edit_form::{EditField, EditForm, FormMode};
        use crate::workflows::form_model::WidgetSpec;

        let s = empty_structure();

        let make_choice_widget = || ChoiceWidget {
            select: Cardinality::Single,
            format: ChoiceFormat::Plain,
            options: vec![
                ChoiceOption {
                    value: "/bin/bash".into(),
                    label: "Bash".into(),
                },
                ChoiceOption {
                    value: "/bin/sh".into(),
                    label: "sh".into(),
                },
            ],
        };

        // Build an app with two fields: index 0 is a choice field, index 1 is plain.
        let mut app = bare_app(false);
        app.focus = Pane::Form;
        app.form = Some(EditForm {
            dn: "cn=Alice,dc=example,dc=org".to_string(),
            fields: vec![
                EditField {
                    label: "loginShell".to_string(),
                    must: false,
                    editable: true,
                    multi: false,
                    secret: false,
                    ordered: false,
                    values: vec!["/bin/bash".to_string()],
                    kind: FieldKind::Text,
                    widget: WidgetSpec::ReadOnlyText,
                    editor: tui_prompts::TextState::new().with_value("/bin/bash".to_string()),
                    widget_binding: Some(crate::config::widget::WidgetKind::Choice(
                        make_choice_widget(),
                    )),
                    orphaned: false,
                },
                EditField {
                    label: "sn".to_string(),
                    must: false,
                    editable: true,
                    multi: false,
                    secret: false,
                    ordered: false,
                    values: vec!["Smith".to_string()],
                    kind: FieldKind::Text,
                    widget: WidgetSpec::ReadOnlyText,
                    editor: tui_prompts::TextState::new().with_value("Smith".to_string()),
                    widget_binding: None,
                    orphaned: false,
                },
            ],
            baseline: Default::default(),
            mode: FormMode::Edit,
            pending_password: None,
        });

        // Focus the choice field (index 0) and type a character.
        app.form_focus = 0;
        dispatch_key(&mut app, key(KeyCode::Char('x')), &s);
        let choice_value = app.form.as_ref().unwrap().fields[0]
            .editor
            .value()
            .to_string();
        assert_eq!(
            choice_value, "/bin/bash",
            "choice field must NOT accept inline key input"
        );

        // Now focus the plain field (index 1) and type a character — it SHOULD change.
        app.form_focus = 1;
        dispatch_key(&mut app, key(KeyCode::Char('!')), &s);
        let plain_value = app.form.as_ref().unwrap().fields[1]
            .editor
            .value()
            .to_string();
        assert_ne!(
            plain_value, "Smith",
            "plain editable field MUST accept inline key input"
        );
    }

    /// A create form whose only field is an empty, NextNumber-bound `uidNumber`,
    /// focused in the form pane.
    fn app_with_next_number_field() -> App {
        use crate::config::widget::WidgetKind;
        use crate::schema::FieldKind;
        use crate::ui::edit_form::{EditField, EditForm, FormMode};
        use crate::workflows::form_model::WidgetSpec;
        use tui_prompts::TextState;
        let field = EditField {
            label: "uidNumber".into(),
            must: true,
            editable: true,
            multi: false,
            secret: false,
            ordered: false,
            values: vec![],
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            editor: TextState::new(),
            widget_binding: Some(WidgetKind::NextNumber {
                min: 10000,
                max: 60000,
            }),
            orphaned: false,
        };
        let mut app = bare_app(false);
        app.focus = Pane::Form;
        app.form = Some(EditForm {
            dn: "uid=new,ou=people,dc=example,dc=org".into(),
            fields: vec![field],
            baseline: Default::default(),
            mode: FormMode::Create {
                profile_idx: 0,
                container: "ou=people,dc=example,dc=org".into(),
            },
            pending_password: None,
        });
        app.form_focus = 0;
        app
    }

    #[test]
    fn enter_on_empty_next_number_field_requests_allocation() {
        let s = empty_structure();
        let mut app = app_with_next_number_field();
        assert_eq!(
            dispatch_key(&mut app, key(KeyCode::Enter), &s),
            Some(UiAction::AllocateNextNumber { field_idx: 0 }),
            "Enter on an empty next-number field requests allocation"
        );
    }

    #[test]
    fn enter_on_filled_next_number_field_does_not_reallocate() {
        use tui_prompts::TextState;
        let s = empty_structure();
        let mut app = app_with_next_number_field();
        // Simulate an already-allocated value.
        if let Some(f) = app.form.as_mut().and_then(|fm| fm.fields.get_mut(0)) {
            f.editor = TextState::new().with_value("10001".to_string());
        }
        assert_eq!(
            dispatch_key(&mut app, key(KeyCode::Enter), &s),
            None,
            "a filled next-number field does not re-allocate on Enter"
        );
    }

    #[test]
    fn typing_overrides_a_next_number_field_inline() {
        let s = empty_structure();
        let mut app = app_with_next_number_field();
        dispatch_key(&mut app, key(KeyCode::Char('7')), &s);
        assert_eq!(
            app.form.as_ref().unwrap().fields[0].editor.value(),
            "7",
            "typing edits the next-number field inline (manual override)"
        );
    }

    #[test]
    fn esc_cancels_a_create_form_but_is_a_noop_on_an_edit_form() {
        use super::super::build_new_entry_form;
        let s = empty_structure();
        // A create form: Esc requests cancel.
        let mut app = bare_app(false);
        app.focus = Pane::Form;
        app.form = Some(build_new_entry_form(
            &user_schema(),
            &create_user_profile(),
            &[],
            0,
            "ou=people,dc=example,dc=org".to_string(),
            false,
        ));
        assert_eq!(
            dispatch_key(&mut app, key(KeyCode::Esc), &s),
            Some(UiAction::FormCancel)
        );
        // An edit form: Esc does not cancel (Alt+C reverts instead).
        let mut app2 = with_form(bare_app(false), "cn=Alice,dc=example,dc=org");
        app2.focus = Pane::Form;
        assert_eq!(dispatch_key(&mut app2, key(KeyCode::Esc), &s), None);
    }
}
