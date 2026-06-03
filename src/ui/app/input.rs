//! Key-dispatch and the value/picker editor cluster for the `app` submodule.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tui_prompts::{State, TextState};

use crate::app::UiAction;
use crate::ldap::worker::{Request, SearchScope, WorkerHandle};
use crate::ui::edit_form::ValueEditor;
use crate::ui::form_state::{guard_decision, GuardChoice, GuardOutcome};
use crate::ui::picker::PICKER_SEARCH_CAP;
use crate::workflows::structure::Structure;

use super::overlay::{GuardIntent, Overlay, PendingAction};
use super::{guard_if_dirty, next_id, App, Pane};

/// Sentinel `picker_last_query` set when a picker opens so the first tick's empty
/// search box (`""`) compares unequal and fires exactly one initial search. A NUL
/// can never be typed into the search box, so `""` never matches it.
const PICKER_INIT_QUERY: &str = "\u{0}";

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
            KeyCode::Up => {
                app.tree_state.key_up();
            }
            KeyCode::Down => {
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
                KeyCode::Enter => open_value_editor(app, structure),
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
            if field.editable && !field.multi {
                field.editor.handle_key_event(key);
            }
        }
    }
}

/// Open the multi-value popup over the focused field. Picker-bound fields open in
/// picker mode; plain multi-valued fields open in free-text mode.
fn open_value_editor(app: &mut App, _structure: &Structure) {
    let focus = app.form_focus;
    let Some(form) = app.form.as_ref() else {
        return;
    };
    let Some(field) = form.fields.get(focus) else {
        return;
    };

    if let Some(binding) = field.picker.clone().filter(|_| field.editable) {
        // Unified picker: open from the resolved binding. Labels and real DNs are
        // upgraded from search results in the `Response::Entries` intercept.
        let ve = ValueEditor::open(focus, field, &binding);
        app.overlay = Some(Overlay::ValueEditor(ve));
        app.picker_last_query = PICKER_INIT_QUERY.to_string();
        app.picker_search_id = None;
    } else if field.multi && field.editable {
        let ve = ValueEditor::open_plain(focus, field);
        app.overlay = Some(Overlay::ValueEditor(ve));
    }
}

/// Key handling for the in-overlay picker. The two picker modes commit
/// differently based on arity (driven by the binding's `select` field, or the
/// field's schema arity when `auto`):
/// - Multi-select: Alt+Space or Enter toggles the highlighted candidate;
///   Alt+S commits the selected store-value set into the field.
/// - Single-select: Enter radio-selects the highlighted candidate; Alt+S
///   commits that candidate's scalar value into the field; Alt+Space is a
///   no-op (nothing to toggle in single-select mode).
///
/// In BOTH modes bare Space is a literal search character (group names may
/// contain spaces). ↑↓ move the cursor; Alt+C / Esc cancel. Any other key edits
/// the search box (the tick-based `service_picker_search` turns a changed query
/// into a live search).
fn picker_editor_key(app: &mut App, key: KeyEvent) {
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
        KeyCode::Esc => {
            app.overlay = None;
            app.picker_search_id = None;
            app.picker_last_query.clear();
        }
        KeyCode::Char('c') | KeyCode::Char('C') if alt => {
            app.overlay = None;
            app.picker_search_id = None;
            app.picker_last_query.clear();
        }
        KeyCode::Char('s') | KeyCode::Char('S') if alt => {
            // Alt+S commits the selection into the field, driven by the binding's
            // cardinality. A single-select commit writes the chosen scalar into the
            // inline editor (a single-value field saves from `editor`, NOT `values`);
            // a multi-select commit writes the selected store-value set.
            if let Some(Overlay::ValueEditor(ve)) = app.overlay.take() {
                if let (Some(binding), Some(picker)) = (ve.binding.as_deref(), ve.picker.as_ref()) {
                    let single = matches!(
                        binding.select,
                        Some(crate::config::relation::Cardinality::Single)
                    ) || (binding.select.is_none()
                        && app
                            .form
                            .as_ref()
                            .and_then(|f| f.fields.get(ve.field))
                            .map(|f| !f.multi)
                            .unwrap_or(false));
                    let values = picker.selected_values();
                    if let Some(field) = app.form.as_mut().and_then(|f| f.fields.get_mut(ve.field))
                    {
                        if single {
                            // Commit the radio-selected row (set by Enter); if none was
                            // explicitly picked, fall back to the highlighted row so a
                            // quick ↑↓ + Alt+S still commits without requiring Enter.
                            let v = values
                                .into_iter()
                                .next()
                                .or_else(|| {
                                    picker
                                        .visible()
                                        .get(picker.cursor)
                                        .map(|row| row.candidate.store_value.clone())
                                })
                                .unwrap_or_default();
                            field.editor = TextState::new().with_value(v.clone());
                            field.values = if v.is_empty() { vec![] } else { vec![v] };
                        } else {
                            field.values = values;
                        }
                    }
                }
                app.picker_search_id = None;
                app.picker_last_query.clear();
            }
        }
        KeyCode::Enter => {
            // Enter "checks" the highlighted candidate: toggle it in/out of a
            // membership selection (multi-select), or set it as the single radio
            // selection for a value-lookup picker. (Alt+Space is avoided — it is a
            // desktop hotkey.) Alt+S then commits.
            // Read the field arity into a local FIRST so we can mutably borrow the
            // overlay (which also borrows `app`) without a second `app.form` borrow.
            let field_single = app
                .overlay
                .as_ref()
                .and_then(|o| match o {
                    Overlay::ValueEditor(ve) => Some(ve.field),
                    _ => None,
                })
                .and_then(|fi| {
                    app.form
                        .as_ref()
                        .and_then(|f| f.fields.get(fi))
                        .map(|f| !f.multi)
                })
                .unwrap_or(false);
            if let Some(Overlay::ValueEditor(ve)) = app.overlay.as_mut() {
                let single = match ve.binding.as_deref().and_then(|b| b.select) {
                    Some(crate::config::relation::Cardinality::Single) => true,
                    Some(crate::config::relation::Cardinality::Multi) => false,
                    None => field_single,
                };
                if let Some(p) = ve.picker.as_mut() {
                    if single {
                        let chosen = p.visible().get(p.cursor).map(|row| row.candidate.clone());
                        if let Some(c) = chosen {
                            p.selected = vec![c];
                        }
                    } else {
                        p.toggle_cursor();
                    }
                }
            }
        }
        KeyCode::Up => {
            if let Some(Overlay::ValueEditor(ve)) = app.overlay.as_mut() {
                if let Some(p) = ve.picker.as_mut() {
                    p.move_cursor(-1);
                }
            }
        }
        KeyCode::Down => {
            if let Some(Overlay::ValueEditor(ve)) = app.overlay.as_mut() {
                if let Some(p) = ve.picker.as_mut() {
                    p.move_cursor(1);
                }
            }
        }
        _ => {
            if let Some(Overlay::ValueEditor(ve)) = app.overlay.as_mut() {
                ve.search.handle_key_event(key);
            }
        }
    }
}

/// Handle a key inside the multi-value popup (spike `popup_key`): nav (↑↓),
/// reorder (Alt+↑↓), insert (Alt+a / Insert), delete (Alt+d), commit (Alt+S,
/// dropping empties), cancel (Esc / Alt+C); any other key edits the selected row.
fn value_editor_key(app: &mut App, key: KeyEvent) {
    // Picker mode has its own key map (search box + selection toggle).
    if matches!(&app.overlay, Some(Overlay::ValueEditor(ve)) if ve.picker.is_some()) {
        picker_editor_key(app, key);
        return;
    }
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match (key.code, alt) {
        (KeyCode::Esc, _) | (KeyCode::Char('c'), true) | (KeyCode::Char('C'), true) => {
            app.overlay = None;
        }
        (KeyCode::Char('s'), true) | (KeyCode::Char('S'), true) => {
            // Commit: write the trimmed, non-empty values back into the field.
            if let Some(Overlay::ValueEditor(ve)) = app.overlay.take() {
                let values = ve.committed_values();
                if let Some(field) = app.form.as_mut().and_then(|f| f.fields.get_mut(ve.field)) {
                    field.values = values;
                }
            }
        }
        _ => {
            let Some(Overlay::ValueEditor(ve)) = app.overlay.as_mut() else {
                return;
            };
            // Invariant: sel is always a valid index (or 0 when empty). Every arm
            // below preserves it, but normalize defensively so the reorder swaps
            // can never index out of bounds regardless of edit history.
            ve.sel = ve.sel.min(ve.rows.len().saturating_sub(1));
            match (key.code, alt) {
                (KeyCode::Up, false) => ve.sel = ve.sel.saturating_sub(1),
                (KeyCode::Down, false) => {
                    ve.sel = (ve.sel + 1).min(ve.rows.len().saturating_sub(1))
                }
                (KeyCode::Up, true) => {
                    if ve.sel > 0 {
                        ve.rows.swap(ve.sel, ve.sel - 1);
                        ve.sel -= 1;
                    }
                }
                (KeyCode::Down, true) => {
                    if ve.sel + 1 < ve.rows.len() {
                        ve.rows.swap(ve.sel, ve.sel + 1);
                        ve.sel += 1;
                    }
                }
                (KeyCode::Char('a'), true) | (KeyCode::Insert, _) => {
                    let at = (ve.sel + 1).min(ve.rows.len());
                    ve.rows.insert(at, TextState::new());
                    ve.sel = at;
                }
                (KeyCode::Char('d'), true) => {
                    if !ve.rows.is_empty() {
                        ve.rows.remove(ve.sel);
                        ve.sel = ve.sel.min(ve.rows.len().saturating_sub(1));
                    }
                }
                // Any other key edits the selected row's text.
                _ => {
                    if let Some(row) = ve.rows.get_mut(ve.sel) {
                        row.handle_key_event(key);
                    }
                }
            }
        }
    }
}

/// When a picker is open and its search term changed, submit a fresh size-capped
/// candidate search (stale ids are discarded in `handle_worker_response`). An
/// empty term still searches — `build_member_filter` produces an objectClass-only
/// filter that loads up to `PICKER_SEARCH_CAP` candidates (so the picker is
/// populated on open). Mirrors the leaf incremental search.
pub(crate) fn service_picker_search(app: &mut App, worker: &WorkerHandle) {
    let Some(Overlay::ValueEditor(ve)) = app.overlay.as_ref() else {
        return;
    };
    let Some(binding) = ve.binding.as_deref() else {
        return;
    };
    if ve.picker.is_none() {
        return;
    }

    let query = ve.search.value().to_string();
    if query == app.picker_last_query {
        return;
    }
    app.picker_last_query = query.clone();

    let scope = &binding.scope;
    let filter =
        crate::ui::picker::build_member_filter(&scope.object_classes, &scope.search_attrs, &query);
    // Request the label-template attrs (always including `cn`, the fallback label
    // attr) plus the scalar store attr when storing one. An empty term yields an
    // objectClass-only filter that loads up to PICKER_SEARCH_CAP.
    let mut attrs: Vec<String> = scope
        .label_template
        .as_deref()
        .map(crate::config::label::template_attrs)
        .unwrap_or_default();
    attrs.push("cn".to_string());
    if let crate::config::relation::StoreKey::Attr(a) = &binding.store {
        attrs.push(a.clone());
    }
    dedupe_ci(&mut attrs);

    let id = next_id();
    app.picker_search_id = Some(id);
    let _ = worker.submit(Request::Search {
        id,
        base: scope.base.clone(),
        scope: SearchScope::Subtree,
        filter,
        attrs,
        size_limit: Some(PICKER_SEARCH_CAP),
    });
}

/// A membership candidate's display label. With a `label_template` the template
/// is rendered against the entry's attrs (e.g. `"Bob Baker (bob)"`); if that
/// render is empty/blank (a missing field, or a present-but-whitespace one) it
/// falls back to the generic `cn`/DN label so a row is never blank. Without a
/// template it is the plain `cn`/DN fallback.
pub(crate) fn membership_candidate_label(
    label_template: Option<&[crate::config::label::LabelSeg]>,
    dn: &str,
    attrs: &std::collections::BTreeMap<String, Vec<String>>,
) -> String {
    if let Some(segs) = label_template {
        let rendered = crate::config::label::render_label(segs, attrs);
        if !rendered.trim().is_empty() {
            return rendered;
        }
    }
    crate::ui::picker::candidate_label(dn, attrs)
}

/// Case-insensitively dedupe a list of attribute names in place, keeping first
/// occurrence and dropping empties.
fn dedupe_ci(attrs: &mut Vec<String>) {
    let mut seen: Vec<String> = Vec::new();
    attrs.retain(|a| {
        if a.is_empty() || seen.iter().any(|s| s.eq_ignore_ascii_case(a)) {
            false
        } else {
            seen.push(a.clone());
            true
        }
    });
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
        KeyCode::Up => {
            *sel = sel.saturating_sub(1);
            None
        }
        KeyCode::Down => {
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
    use crate::ui::edit_form::{EditForm, FormMode};
    use tui_tree_widget::TreeState;

    #[test]
    fn focus_cycles_tree_leaf_form() {
        assert_eq!(next_pane(Pane::Tree), Pane::Leaf);
        assert_eq!(next_pane(Pane::Leaf), Pane::Form);
        assert_eq!(next_pane(Pane::Form), Pane::Tree);
    }

    /// A bare App carrying a value-editor overlay of `rows` rows (form left None;
    /// the popup nav/reorder paths only touch the overlay).
    fn app_with_value_editor(rows: usize) -> App {
        use crate::ui::edit_form::ValueEditor;
        let ve = ValueEditor {
            field: 0,
            label: "mail".into(),
            ordered: false,
            secret: false,
            rows: (0..rows)
                .map(|i| TextState::new().with_value(format!("v{i}")))
                .collect(),
            sel: 0,
            scroll: 0,
            picker: None,
            search: TextState::new(),
            binding: None,
        };
        App {
            focus: Pane::Form,
            should_quit: false,
            read_only: false,
            tree_state: TreeState::default(),
            tree_items: vec![],
            current_branch: String::new(),
            last_search: String::new(),
            rows: vec![],
            leaf_sel: 0,
            search: TextState::new(),
            last_seen_leaf: None,
            form: None,
            form_focus: 0,
            form_scroll: 0,
            overlay: Some(Overlay::ValueEditor(ve)),
            status: String::new(),
            pickers: vec![],
            label_rules: vec![],
            picker_search_id: None,
            picker_last_query: String::new(),
        }
    }

    /// Regression (P3 review): deleting every row and then navigating/reordering
    /// must not panic. The reorder swaps are bounded and `sel` is normalized, so
    /// `Vec::swap` is never called out of range on an empty popup.
    #[test]
    fn value_editor_delete_all_then_navigate_does_not_panic() {
        let alt = |c| KeyEvent::new(c, KeyModifiers::ALT);
        let plain = |c| KeyEvent::new(c, KeyModifiers::NONE);
        let mut app = app_with_value_editor(1);
        value_editor_key(&mut app, alt(KeyCode::Char('d'))); // delete the only row → empty
        value_editor_key(&mut app, plain(KeyCode::Down)); // must not advance sel out of range
        value_editor_key(&mut app, alt(KeyCode::Up)); // reorder on empty → no-op, no panic
        value_editor_key(&mut app, alt(KeyCode::Down)); // no panic
        value_editor_key(&mut app, alt(KeyCode::Char('a'))); // add a row back
        let Some(Overlay::ValueEditor(ve)) = &app.overlay else {
            panic!("popup should still be open");
        };
        assert_eq!(ve.rows.len(), 1);
        assert!(ve.sel < ve.rows.len());
    }

    /// Reorder swaps stay in range across a delete-driven shrink, too.
    #[test]
    fn value_editor_reorder_after_delete_is_bounded() {
        let alt = |c| KeyEvent::new(c, KeyModifiers::ALT);
        let mut app = app_with_value_editor(3);
        // Move to the last row, delete it, then try to reorder down (past the end).
        value_editor_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        value_editor_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        value_editor_key(&mut app, alt(KeyCode::Char('d'))); // delete last → sel clamps
        value_editor_key(&mut app, alt(KeyCode::Down)); // no panic
        value_editor_key(&mut app, alt(KeyCode::Up)); // no panic
        let Some(Overlay::ValueEditor(ve)) = &app.overlay else {
            panic!("popup should still be open");
        };
        assert!(ve.sel < ve.rows.len());
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

    // ── 4.4 helpers ────────────────────────────────────────────────────────────

    /// A multi-select, DN-stored `member` picker binding (the membership case).
    fn member_dn_binding() -> crate::config::relation::PickerBinding {
        use crate::config::relation::{CandidateScope, Cardinality, PickerBinding, StoreKey};
        PickerBinding {
            attr: "member".into(),
            scope: CandidateScope {
                base: "ou=people".into(),
                object_classes: vec!["inetOrgPerson".into()],
                search_attrs: vec!["uid".into()],
                label_template: None,
            },
            store: StoreKey::Dn,
            select: Some(Cardinality::Multi),
            fanout_attr: None,
        }
    }

    /// App with a one-field `member` form (index 0) and no overlay.
    fn test_app_with_form_field_member() -> App {
        use crate::schema::FieldKind;
        use crate::ui::edit_form::EditField;
        use crate::ui::form::WidgetSpec;
        let field = EditField {
            label: "member".into(),
            must: false,
            editable: true,
            multi: true,
            secret: false,
            ordered: false,
            values: vec![],
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            editor: TextState::new(),
            picker: Some(member_dn_binding()),
        };
        let mut app = bare_app(false);
        app.form = Some(EditForm {
            dn: "cn=g1,ou=groups".into(),
            fields: vec![field],
            baseline: Default::default(),
            mode: FormMode::Edit,
        });
        app
    }

    /// A ValueEditor in picker mode over field `idx`, empty selection, bound to a
    /// multi-select DN-stored `member` picker.
    fn make_picker_ve(idx: usize) -> ValueEditor {
        ValueEditor {
            field: idx,
            label: "member".into(),
            ordered: false,
            secret: false,
            rows: vec![],
            sel: 0,
            scroll: 0,
            picker: Some(crate::ui::picker::PickerState::new(vec![], true)),
            search: TextState::new(),
            binding: Some(Box::new(member_dn_binding())),
        }
    }

    #[test]
    fn picker_enter_toggles_and_alt_s_commits_dns() {
        use crate::ui::picker::Candidate;
        let mut app = test_app_with_form_field_member();
        let mut ve = make_picker_ve(0);
        ve.picker.as_mut().unwrap().set_results(vec![Candidate {
            dn: "uid=a,ou=people".into(),
            label: "a".into(),
            store_value: "uid=a,ou=people".into(),
        }]);
        app.overlay = Some(Overlay::ValueEditor(ve));
        // Enter toggles the cursor row (a) into the selection.
        value_editor_key(&mut app, key(KeyCode::Enter));
        // Alt+S commits the selected DNs into the field.
        value_editor_key(&mut app, alt(KeyCode::Char('s')));
        let f = &app.form.as_ref().unwrap().fields[0];
        assert_eq!(f.values, vec!["uid=a,ou=people".to_string()]);
        assert!(app.overlay.is_none());
    }

    // ── 5.2 value-lookup picker ──────────────────────────────────────────────

    /// A single-select picker binding for `gidNumber`: store the candidate's
    /// scalar `gidNumber` attribute, search posixGroups under `ou=groups,dc=test`.
    fn gid_picker_binding() -> crate::config::relation::PickerBinding {
        use crate::config::relation::{CandidateScope, Cardinality, PickerBinding, StoreKey};
        PickerBinding {
            attr: "gidNumber".into(),
            scope: CandidateScope {
                base: "ou=groups,dc=test".into(),
                object_classes: vec!["posixGroup".into()],
                search_attrs: vec!["cn".into()],
                label_template: None,
            },
            store: StoreKey::Attr("gidNumber".into()),
            select: Some(Cardinality::Single),
            fanout_attr: None,
        }
    }

    #[test]
    fn open_value_editor_arms_initial_search() {
        // Opening a membership picker arms the sentinel so the first tick fires an
        // (empty-box) search instead of waiting for the user to type.
        let mut app = test_app_with_form_field_member();
        app.form_focus = 0;
        let s = empty_structure();
        open_value_editor(&mut app, &s);
        assert!(
            matches!(&app.overlay, Some(Overlay::ValueEditor(ve)) if ve.picker.is_some()),
            "membership picker installed"
        );
        assert_eq!(app.picker_last_query, "\u{0}");
    }

    #[test]
    fn membership_candidate_label_template_blank_and_fallback() {
        use crate::config::label::parse_label_template;
        use std::collections::BTreeMap;
        let dn = "uid=bob,ou=people,dc=test";
        let tmpl = parse_label_template("{cn} ({uid})");

        // Template → rendered.
        let mut attrs = BTreeMap::new();
        attrs.insert("cn".to_string(), vec!["Bob Baker".to_string()]);
        attrs.insert("uid".to_string(), vec!["bob".to_string()]);
        assert_eq!(
            membership_candidate_label(Some(&tmpl), dn, &attrs),
            "Bob Baker (bob)"
        );

        // Blank render (a present-but-whitespace template result) → cn fallback.
        let mut blank = BTreeMap::new();
        blank.insert("cn".to_string(), vec!["bobby".to_string()]);
        let ws_tmpl = parse_label_template("{nope}");
        assert_eq!(
            membership_candidate_label(Some(&ws_tmpl), dn, &blank),
            "bobby"
        );

        // No template → cn fallback; no cn → DN.
        assert_eq!(membership_candidate_label(None, dn, &blank), "bobby");
        let empty: BTreeMap<String, Vec<String>> = BTreeMap::new();
        assert_eq!(membership_candidate_label(None, dn, &empty), dn);
    }

    /// App with a single-value (`multi=false`) `gidNumber` field already tagged
    /// with a lookup spec, focused (index 0), no overlay.
    fn app_with_lookup_field() -> App {
        use crate::schema::FieldKind;
        use crate::ui::edit_form::EditField;
        use crate::ui::form::WidgetSpec;
        let field = EditField {
            label: "gidNumber".into(),
            must: false,
            editable: true,
            multi: false, // single-value — the trap-1 regression guard
            secret: false,
            ordered: false,
            values: vec![],
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            editor: TextState::new(),
            picker: Some(gid_picker_binding()),
        };
        let mut app = bare_app(false);
        app.form = Some(EditForm {
            dn: "uid=alice,ou=people,dc=test".into(),
            fields: vec![field],
            baseline: Default::default(),
            mode: FormMode::Edit,
        });
        app.form_focus = 0;
        app
    }

    #[test]
    fn open_value_editor_opens_picker_for_single_value_lookup_field() {
        // Trap 1: a scalar (multi=false) picker-bound field must still open a picker.
        let mut app = app_with_lookup_field();
        let s = empty_structure(); // root = dc=test
        open_value_editor(&mut app, &s);
        match &app.overlay {
            Some(Overlay::ValueEditor(ve)) => {
                let binding = ve.binding.as_deref().expect("binding installed");
                assert!(
                    matches!(
                        binding.select,
                        Some(crate::config::relation::Cardinality::Single)
                    ),
                    "single-select binding"
                );
                assert!(ve.picker.is_some(), "picker state present");
                // Search runs under the binding's scope base.
                assert_eq!(binding.scope.base, "ou=groups,dc=test");
                // Single-select has no pre-pinned selection.
                assert!(ve.picker.as_ref().unwrap().selected.is_empty());
            }
            other => panic!("expected a ValueEditor overlay, got {:?}", other.is_some()),
        }
    }

    #[test]
    fn lookup_alt_s_commits_scalar_to_field_editor() {
        use crate::ui::picker::Candidate;
        let mut app = app_with_lookup_field();
        let s = empty_structure();
        open_value_editor(&mut app, &s);
        // Seed one candidate carrying the scalar value_attr.
        if let Some(Overlay::ValueEditor(ve)) = app.overlay.as_mut() {
            ve.picker.as_mut().unwrap().set_results(vec![Candidate {
                dn: "cn=staff,ou=groups,dc=test".into(),
                label: "staff".into(),
                store_value: "5001".into(),
            }]);
        }
        // Bare Alt+S commits the highlighted row without a preceding Enter — the
        // cursor-fallback path.
        picker_editor_key(&mut app, alt(KeyCode::Char('s')));
        assert!(app.overlay.is_none(), "overlay closes on commit");
        let f = &app.form.as_ref().unwrap().fields[0];
        assert_eq!(f.editor.value(), "5001");
        // A single-value field saves from `editor`, so current_values reflects it.
        assert_eq!(f.current_values(), vec!["5001".to_string()]);
    }

    #[test]
    fn single_select_enter_then_alt_s_also_commits() {
        // Prove that Enter-then-Alt+S works: Enter radio-selects a row explicitly,
        // then Alt+S commits it (via the `selected_values()` path, not the cursor
        // fallback).
        use crate::ui::picker::Candidate;
        let mut app = app_with_lookup_field();
        let s = empty_structure();
        open_value_editor(&mut app, &s);
        if let Some(Overlay::ValueEditor(ve)) = app.overlay.as_mut() {
            ve.picker.as_mut().unwrap().set_results(vec![Candidate {
                dn: "cn=staff,ou=groups,dc=test".into(),
                label: "staff".into(),
                store_value: "5001".into(),
            }]);
        }
        // Enter radio-selects the highlighted row explicitly.
        picker_editor_key(&mut app, key(KeyCode::Enter));
        // Confirm the radio selection is in place before Alt+S.
        if let Some(Overlay::ValueEditor(ve)) = app.overlay.as_ref() {
            let p = ve.picker.as_ref().unwrap();
            assert_eq!(p.selected.len(), 1, "Enter radio-selects exactly one row");
            assert_eq!(p.selected[0].store_value, "5001");
        } else {
            panic!("picker overlay gone after Enter");
        }
        // Alt+S now commits via selected_values(), not the cursor fallback.
        picker_editor_key(&mut app, alt(KeyCode::Char('s')));
        assert!(app.overlay.is_none(), "overlay closes on commit");
        let f = &app.form.as_ref().unwrap().fields[0];
        assert_eq!(f.editor.value(), "5001");
        assert_eq!(f.current_values(), vec!["5001".to_string()]);
    }

    #[test]
    fn lookup_enter_radio_selects_then_alt_s_commits() {
        use crate::ui::picker::Candidate;
        let mut app = app_with_lookup_field();
        let s = empty_structure();
        open_value_editor(&mut app, &s);
        if let Some(Overlay::ValueEditor(ve)) = app.overlay.as_mut() {
            ve.picker.as_mut().unwrap().set_results(vec![
                Candidate {
                    dn: "cn=staff,ou=groups,dc=test".into(),
                    label: "staff".into(),
                    store_value: "5000".into(),
                },
                Candidate {
                    dn: "cn=dev,ou=groups,dc=test".into(),
                    label: "dev".into(),
                    store_value: "5001".into(),
                },
            ]);
        }
        // Enter marks the highlighted (first) row as the single radio selection.
        picker_editor_key(&mut app, key(KeyCode::Enter));
        if let Some(Overlay::ValueEditor(ve)) = app.overlay.as_ref() {
            let p = ve.picker.as_ref().unwrap();
            assert_eq!(p.selected.len(), 1, "single-select holds exactly one");
            assert_eq!(p.selected[0].store_value, "5000");
        } else {
            panic!("picker overlay gone");
        }
        // Alt+S commits the radio-selected scalar, not just the highlighted row.
        picker_editor_key(&mut app, alt(KeyCode::Char('s')));
        assert!(app.overlay.is_none());
        assert_eq!(app.form.as_ref().unwrap().fields[0].editor.value(), "5000");
    }

    #[test]
    fn lookup_alt_s_with_no_value_leaves_field_unchanged() {
        use crate::ui::picker::Candidate;
        let mut app = app_with_lookup_field();
        let s = empty_structure();
        open_value_editor(&mut app, &s);
        if let Some(Overlay::ValueEditor(ve)) = app.overlay.as_mut() {
            ve.picker.as_mut().unwrap().set_results(vec![Candidate {
                dn: "cn=staff,ou=groups,dc=test".into(),
                label: "staff".into(),
                store_value: String::new(), // candidate lacked value_attr
            }]);
        }
        // Bare Alt+S falls back to the highlighted cursor row; its store_value is
        // empty, so no write happens.
        picker_editor_key(&mut app, alt(KeyCode::Char('s')));
        assert!(app.overlay.is_none());
        // No write happened — the editor stays empty.
        let f = &app.form.as_ref().unwrap().fields[0];
        assert_eq!(f.editor.value(), "");
        assert!(f.values.is_empty(), "empty value commits nothing");
    }

    #[test]
    fn select_auto_derives_cardinality_from_field_arity() {
        // When a picker binding carries `select: None`, the Enter handler derives
        // cardinality from the field's own arity (multi=true → toggle, multi=false
        // → radio/replace). This test verifies both sub-cases.

        // --- Sub-case A: multi=true field + select:None → toggle semantics ---
        {
            use crate::ui::picker::Candidate;
            let mut app = test_app_with_form_field_member(); // multi=true
            let mut ve = make_picker_ve(0);
            // Override the binding to select: None so auto-derivation kicks in.
            if let Some(b) = ve.binding.as_mut() {
                b.select = None;
            }
            ve.picker.as_mut().unwrap().set_results(vec![Candidate {
                dn: "uid=a,ou=people".into(),
                label: "a".into(),
                store_value: "uid=a,ou=people".into(),
            }]);
            app.overlay = Some(Overlay::ValueEditor(ve));
            // First Enter toggles the cursor row INTO the selection.
            picker_editor_key(&mut app, key(KeyCode::Enter));
            if let Some(Overlay::ValueEditor(ve)) = app.overlay.as_ref() {
                assert_eq!(
                    ve.picker.as_ref().unwrap().selected.len(),
                    1,
                    "multi/auto: first Enter toggles candidate in"
                );
            } else {
                panic!("overlay gone after first Enter");
            }
            // Second Enter toggles the same row back OUT.
            picker_editor_key(&mut app, key(KeyCode::Enter));
            if let Some(Overlay::ValueEditor(ve)) = app.overlay.as_ref() {
                assert!(
                    ve.picker.as_ref().unwrap().selected.is_empty(),
                    "multi/auto: second Enter toggles candidate back out"
                );
            } else {
                panic!("overlay gone after second Enter");
            }
        }

        // --- Sub-case B: multi=false field + select:None → radio/replace semantics ---
        {
            use crate::ui::picker::Candidate;
            let mut app = app_with_lookup_field(); // multi=false
            let s = empty_structure();
            open_value_editor(&mut app, &s);
            // Override the binding to select: None so auto-derivation kicks in.
            if let Some(Overlay::ValueEditor(ve)) = app.overlay.as_mut() {
                if let Some(b) = ve.binding.as_mut() {
                    b.select = None;
                }
                ve.picker.as_mut().unwrap().set_results(vec![
                    Candidate {
                        dn: "cn=row0,ou=groups,dc=test".into(),
                        label: "row0".into(),
                        store_value: "1000".into(),
                    },
                    Candidate {
                        dn: "cn=row1,ou=groups,dc=test".into(),
                        label: "row1".into(),
                        store_value: "2000".into(),
                    },
                ]);
            }
            // Enter on cursor row 0 → radio-selects row0 (len 1).
            picker_editor_key(&mut app, key(KeyCode::Enter));
            if let Some(Overlay::ValueEditor(ve)) = app.overlay.as_ref() {
                let p = ve.picker.as_ref().unwrap();
                assert_eq!(
                    p.selected.len(),
                    1,
                    "single/auto: Enter radio-selects one row"
                );
                assert_eq!(p.selected[0].store_value, "1000");
            } else {
                panic!("overlay gone after first Enter");
            }
            // Move cursor to row 1, Enter again → REPLACES selection (still len 1).
            if let Some(Overlay::ValueEditor(ve)) = app.overlay.as_mut() {
                ve.picker.as_mut().unwrap().cursor = 1;
            }
            picker_editor_key(&mut app, key(KeyCode::Enter));
            if let Some(Overlay::ValueEditor(ve)) = app.overlay.as_ref() {
                let p = ve.picker.as_ref().unwrap();
                assert_eq!(
                    p.selected.len(),
                    1,
                    "single/auto: second Enter replaces, not appends"
                );
                assert_eq!(
                    p.selected[0].store_value, "2000",
                    "single/auto: second Enter selects the new row"
                );
            } else {
                panic!("overlay gone after second Enter");
            }
        }
    }

    #[test]
    fn lookup_bare_space_types_into_search_and_alt_space_is_ignored() {
        // The picker is shared with membership mode (Alt+Space toggles, Alt+S
        // commits a DN set). In a single-select lookup picker: bare Space is a
        // literal search char (group names may contain spaces); Alt+Space is a
        // no-op (single-select has nothing to toggle, and must not leak a DN).
        use crate::ui::picker::Candidate;
        let mut app = app_with_lookup_field();
        let s = empty_structure();
        open_value_editor(&mut app, &s);
        if let Some(Overlay::ValueEditor(ve)) = app.overlay.as_mut() {
            ve.picker.as_mut().unwrap().set_results(vec![Candidate {
                dn: "cn=staff group,ou=groups,dc=test".into(),
                label: "staff group".into(),
                store_value: "5001".into(),
            }]);
        }
        // Bare Space → search box, not a selection toggle.
        picker_editor_key(&mut app, key(KeyCode::Char(' ')));
        match app.overlay.as_ref() {
            Some(Overlay::ValueEditor(ve)) => {
                assert_eq!(
                    ve.search.value(),
                    " ",
                    "bare Space is typed into the search box"
                );
                assert!(
                    ve.picker.as_ref().unwrap().selected.is_empty(),
                    "bare Space must not toggle a selection in lookup mode"
                );
            }
            _ => panic!("overlay must stay open after Space"),
        }
        // Alt+Space → no-op for a lookup picker (single-select, nothing to toggle).
        picker_editor_key(&mut app, alt(KeyCode::Char(' ')));
        assert!(
            app.overlay.is_some(),
            "Alt+Space is ignored for a lookup picker — overlay stays open"
        );
        match app.overlay.as_ref() {
            Some(Overlay::ValueEditor(ve)) => assert!(
                ve.picker.as_ref().unwrap().selected.is_empty(),
                "Alt+Space must not toggle a selection in lookup mode"
            ),
            _ => panic!("overlay must stay open after Alt+Space"),
        }
        let f = &app.form.as_ref().unwrap().fields[0];
        assert_eq!(
            f.editor.value(),
            "",
            "Alt+Space must not write a DN into a scalar field"
        );
        assert!(
            f.values.is_empty(),
            "Alt+Space must not populate values with a DN"
        );
    }

    #[test]
    fn dedupe_ci_drops_empties_and_case_dups() {
        let mut attrs = vec![
            "gidNumber".to_string(),
            "cn".to_string(),
            "CN".to_string(),
            String::new(),
            "gidnumber".to_string(),
        ];
        dedupe_ci(&mut attrs);
        assert_eq!(attrs, vec!["gidNumber".to_string(), "cn".to_string()]);
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
