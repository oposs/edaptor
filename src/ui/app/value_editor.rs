//! The multi-value / picker value-editor overlay for the `app` submodule:
//! opening the editor over a focused field, its two key maps (free-text rows and
//! the picker search/selection), the tick-driven candidate search, and the
//! candidate-label rendering.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tui_prompts::{State, TextState};

use crate::ldap::worker::{Request, SearchScope, WorkerHandle};
use crate::ui::edit_form::ValueEditor;
use crate::ui::picker::PICKER_SEARCH_CAP;
use crate::workflows::structure::Structure;

use super::overlay::Overlay;
use super::{next_id, App};

/// Sentinel `picker_last_query` set when a picker opens so the first tick's empty
/// search box (`""`) compares unequal and fires exactly one initial search. A NUL
/// can never be typed into the search box, so `""` never matches it.
const PICKER_INIT_QUERY: &str = "\u{0}";

/// Open the multi-value popup over the focused field. Picker-bound fields open in
/// picker mode; plain multi-valued fields open in free-text mode.
pub(crate) fn open_value_editor(app: &mut App, _structure: &Structure) {
    let focus = app.form_focus;
    let Some(form) = app.form.as_ref() else {
        return;
    };
    let Some(field) = form.fields.get(focus) else {
        return;
    };

    // A password-bound field opens the dedicated set-password popup (the field is
    // read-only; the new value is staged into `pending_password`, not the editor).
    // Read the binding kind, drop the `form` borrow, then re-enter via the popup.
    if matches!(
        field.widget_binding,
        Some(crate::config::widget::WidgetKind::Password(_))
    ) {
        super::password_editor::open_password_editor(app);
        return;
    }

    if let Some(crate::config::widget::WidgetKind::Choice(w)) =
        field.widget_binding.clone().filter(|_| field.editable)
    {
        // A `[profile.widget.<attr>]` choice field opens a static choice overlay
        // (the picker UI seeded from fixed options, no LDAP search).
        let ve = ValueEditor::open_choice(focus, field, &w);
        app.overlay = Some(Overlay::ValueEditor(ve));
    } else if let Some(crate::config::widget::WidgetKind::Picker(binding)) =
        field.widget_binding.clone().filter(|_| field.editable)
    {
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
            // A choice widget commits the assembled (lossless merge-from-original)
            // encoded string into the inline editor — a single-valued field reads
            // `current_values()` from `editor`, NOT `values`.
            if let Some(Overlay::ValueEditor(ve)) = app.overlay.take() {
                if let Some(w) = ve.choice.as_ref() {
                    let checked: Vec<String> = ve
                        .picker
                        .as_ref()
                        .map(|p| p.selected_values())
                        .unwrap_or_default();
                    let value = w.commit_value(&ve.choice_original, &checked);
                    if let Some(field) = app.form.as_mut().and_then(|f| f.fields.get_mut(ve.field))
                    {
                        field.editor = TextState::new().with_value(value.clone());
                        field.values = if value.is_empty() {
                            vec![]
                        } else {
                            vec![value]
                        };
                    }
                    return;
                }
                // Not a choice editor: put the overlay back for the picker path below.
                app.overlay = Some(Overlay::ValueEditor(ve));
            }
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
                // A choice widget drives cardinality from its own `select`; a
                // picker uses the binding's select with an `auto` field-arity
                // fallback.
                let single = if let Some(w) = ve.choice.as_ref() {
                    matches!(w.select, crate::config::relation::Cardinality::Single)
                } else {
                    match ve.binding.as_deref().and_then(|b| b.select) {
                        Some(crate::config::relation::Cardinality::Single) => true,
                        Some(crate::config::relation::Cardinality::Multi) => false,
                        None => field_single,
                    }
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
                // A static choice editor has no search box — ignore char keys so
                // they cannot type into (or fire a search against) a fixed list.
                if ve.choice.is_none() {
                    ve.search.handle_key_event(key);
                }
            }
        }
    }
}

/// Handle a key inside the multi-value popup (spike `popup_key`): nav (↑↓),
/// reorder (Alt+↑↓), insert (Alt+a / Insert), delete (Alt+d), commit (Alt+S,
/// dropping empties), cancel (Esc / Alt+C); any other key edits the selected row.
pub(crate) fn value_editor_key(app: &mut App, key: KeyEvent) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::app::test_support::*;
    use crate::ui::app::Pane;
    use crate::ui::edit_form::{EditForm, FormMode};
    use tui_tree_widget::TreeState;

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
            choice: None,
            choice_original: String::new(),
        };
        App {
            focus: Pane::Form,
            should_quit: false,
            read_only: false,
            connection_encrypted: false,
            tree_state: TreeState::default(),
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
            widgets: vec![],
            label_rules: vec![],
            tree_rules: Vec::new(),
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
            widget_binding: Some(crate::config::widget::WidgetKind::Picker(
                member_dn_binding(),
            )),
        };
        let mut app = bare_app(false);
        app.form = Some(EditForm {
            dn: "cn=g1,ou=groups".into(),
            fields: vec![field],
            baseline: Default::default(),
            mode: FormMode::Edit,
            pending_password: None,
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
            choice: None,
            choice_original: String::new(),
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
            widget_binding: Some(crate::config::widget::WidgetKind::Picker(
                gid_picker_binding(),
            )),
        };
        let mut app = bare_app(false);
        app.form = Some(EditForm {
            dn: "uid=alice,ou=people,dc=test".into(),
            fields: vec![field],
            baseline: Default::default(),
            mode: FormMode::Edit,
            pending_password: None,
        });
        app.form_focus = 0;
        app
    }

    #[test]
    fn open_value_editor_opens_picker_on_single_value_lookup_field() {
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

    /// App with a one-field form whose single field is bound to a choice widget.
    /// The field is single-valued (`multi=false`) so `current_values()` reads the
    /// editor; `editor`/`values` are both seeded with `value`.
    fn app_with_choice_field(
        attr: &str,
        value: &str,
        widget: &crate::config::widget::ChoiceWidget,
    ) -> App {
        use crate::schema::FieldKind;
        use crate::ui::edit_form::EditField;
        use crate::ui::form::WidgetSpec;
        let field = EditField {
            label: attr.into(),
            must: false,
            editable: true,
            multi: false,
            secret: false,
            ordered: false,
            values: vec![value.to_string()],
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            editor: TextState::new().with_value(value.to_string()),
            widget_binding: Some(crate::config::widget::WidgetKind::Choice(widget.clone())),
        };
        let mut app = bare_app(false);
        app.form = Some(EditForm {
            dn: "uid=alice,ou=people,dc=test".into(),
            fields: vec![field],
            baseline: Default::default(),
            mode: FormMode::Edit,
            pending_password: None,
        });
        app.focus = Pane::Form;
        app.form_focus = 0;
        app
    }

    #[test]
    fn choice_commit_writes_assembled_string_to_editor() {
        use crate::config::relation::Cardinality;
        use crate::config::widget::{ChoiceFormat, ChoiceWidget};
        use crate::config::ChoiceOption;
        let widget = ChoiceWidget {
            select: Cardinality::Multi,
            format: ChoiceFormat::Bracketed,
            options: vec![ChoiceOption {
                value: "D".into(),
                label: "Disabled".into(),
            }],
        };
        let mut app = app_with_choice_field("sambaAcctFlags", "[U          ]", &widget);
        open_value_editor(&mut app, &empty_structure());
        value_editor_key(&mut app, key(KeyCode::Enter)); // toggle D in
        value_editor_key(&mut app, alt(KeyCode::Char('s'))); // commit
        assert!(app.overlay.is_none(), "overlay closes on commit");
        let f = &app.form.as_ref().unwrap().fields[0];
        assert_eq!(f.editor.value(), "[DU         ]");
        assert_eq!(f.current_values(), vec!["[DU         ]".to_string()]);
    }

    #[test]
    fn choice_single_select_radio_replaces_and_commits_to_editor() {
        // A single-select Plain choice: Enter radio-selects, Alt+S commits the
        // chosen option's value into the editor (replacing the original).
        use crate::config::relation::Cardinality;
        use crate::config::widget::{ChoiceFormat, ChoiceWidget};
        use crate::config::ChoiceOption;
        let widget = ChoiceWidget {
            select: Cardinality::Single,
            format: ChoiceFormat::Plain,
            options: vec![
                ChoiceOption {
                    value: "/bin/bash".into(),
                    label: "Bash".into(),
                },
                ChoiceOption {
                    value: "/bin/sh".into(),
                    label: "POSIX sh".into(),
                },
            ],
        };
        let mut app = app_with_choice_field("loginShell", "/bin/bash", &widget);
        open_value_editor(&mut app, &empty_structure());
        // Move cursor to the second option (POSIX sh) and radio-select it.
        value_editor_key(&mut app, key(KeyCode::Down));
        value_editor_key(&mut app, key(KeyCode::Enter));
        value_editor_key(&mut app, alt(KeyCode::Char('s')));
        let f = &app.form.as_ref().unwrap().fields[0];
        assert_eq!(f.editor.value(), "/bin/sh");
        assert_eq!(f.current_values(), vec!["/bin/sh".to_string()]);
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
}
