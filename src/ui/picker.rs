//! Picker field widget (single / multi select over live LDAP results). A
//! `WidgetKind::Picker` binding with `fanout_attr == None` opens a modal with a
//! search `InputLine` on top and a `ListBox` below: typing in the search box
//! submits an async candidate search (`SearchFlow`) via the worker; results
//! arrive on the next pump tick and are broadcast as `REFRESH`, which the dialog
//! copies into its neutral `PickState` and re-renders. Insert ticks the
//! highlighted row (radio for single, checkbox for multi); OK commits
//! `SetValues(pick_state.selected_values())`. Space is intentionally NOT
//! intercepted so it can be typed into the search box for multi-word queries.
//!
//! Mirrors the `oc_picker` / `choice` modal structure: one file holds
//! `PickerWidget` (FieldWidget), `PickerEditor` (FieldEditor) and `PickerDialog`
//! (the interactive `Dialog` view). Membership (fan-out) pickers are a later
//! task and are NOT handled here.

use tvision_rs::{
    self as tv, delegate, ButtonFlags, ButtonRowAlign, Command, Context, Dialog, Event, FieldValue,
    InputLine, Key, ListBox, Rect, View,
};

use crate::config::relation::{Cardinality, PickerBinding, StoreKey};
use crate::config::widget::WidgetKind;
use crate::schema::SchemaModel;
use crate::ui::widget::{Activation, Capability, CommitOutcome, FieldEditor, FieldWidget};
use crate::ui::{Shared, REFRESH};
use crate::workflows::edit_form::EditField;
use crate::workflows::pick_state::{Candidate, PickState};

// ---------------------------------------------------------------------------
// PickerWidget — FieldWidget plugin
// ---------------------------------------------------------------------------

/// The plugin for `WidgetKind::Picker`-bound fields (non-fan-out). `present`
/// joins the selected store values; `activate` opens a `PickerDialog`.
pub(crate) struct PickerWidget;

impl FieldWidget for PickerWidget {
    fn capability(&self) -> Capability {
        Capability::NeedsWorkerSearch
    }

    fn present(&self, field: &EditField) -> String {
        if field.values.is_empty() {
            "\u{2039}none\u{203a}".to_string() // ‹none›
        } else {
            field.values.join(", ")
        }
    }

    fn activate(&self, field: &EditField) -> Activation {
        match &field.widget_binding {
            Some(WidgetKind::Picker(b)) if b.fanout_attr.is_none() => {
                Activation::Modal(Box::new(PickerEditor {
                    label: field.label.clone(),
                    binding: b.clone(),
                    current: field.values.clone(),
                    multi: field.multi,
                }))
            }
            _ => Activation::Inline,
        }
    }
}

// ---------------------------------------------------------------------------
// PickerEditor — FieldEditor (carries state into the dialog builder)
// ---------------------------------------------------------------------------

/// Carries the field's binding + current values into the dialog builder.
pub(crate) struct PickerEditor {
    label: String,
    binding: PickerBinding,
    current: Vec<String>,
    /// The field's schema arity, used to derive cardinality when `select = auto`.
    multi: bool,
}

impl FieldEditor for PickerEditor {
    fn into_view(
        self: Box<Self>,
        _schema: &SchemaModel,
        shared: Shared,
    ) -> (Box<dyn View>, tv::ViewId) {
        let PickerEditor {
            label,
            binding,
            current,
            multi,
        } = *self;
        let cardinality = binding.select.unwrap_or(if multi {
            Cardinality::Multi
        } else {
            Cardinality::Single
        });
        let dlg = PickerDialog::new(label, binding, current, cardinality, shared);
        // Focus the search box so typing searches immediately (search-as-you-type);
        // arrow keys are forwarded to the list by `handle_event` (the search-over-
        // list idiom, mirroring `LeafPane`).
        let focus = dlg.search_id;
        (Box::new(dlg), focus)
    }
}

// ---------------------------------------------------------------------------
// PickerDialog — the interactive modal with live search
// ---------------------------------------------------------------------------

/// Search box (row 1) over a ticked candidate `ListBox`. Maintains a neutral
/// `PickState`; results arrive via the pump and the `REFRESH` broadcast.
pub(crate) struct PickerDialog {
    dlg: Dialog,
    search_id: tv::ViewId,
    list_id: tv::ViewId,
    shared: Shared,
    pick: PickState,
    cardinality: Cardinality,
    /// Resolved candidate-search scope.
    base: String,
    oc: String,
    attrs: Vec<String>,
    /// `Some(attr)` for a scalar store; `None` for a DN store.
    store_attr: Option<String>,
    last_search: String,
    seeded: bool,
}

impl PickerDialog {
    fn new(
        label: String,
        binding: PickerBinding,
        current: Vec<String>,
        cardinality: Cardinality,
        shared: Shared,
    ) -> Self {
        let title = format!("Select {label}");
        let mut dlg = Dialog::new(Rect::new(0, 0, 60, 22), Some(title));
        dlg.state_mut().options.center_x = true;
        dlg.state_mut().options.center_y = true;
        // Search box (row 1) + list (rows 3..18) inside the dialog frame.
        let search = InputLine::with_limit(Rect::new(2, 1, 58, 2), 128);
        let search_id = dlg.insert_child(Box::new(search));
        let list = ListBox::new(Rect::new(2, 3, 58, 18), 1, None, None);
        let list_id = dlg.insert_child(Box::new(list));
        dlg.button_row(
            &[
                (
                    "~O~K",
                    Command::OK,
                    ButtonFlags {
                        default: true,
                        ..ButtonFlags::new()
                    },
                ),
                ("~C~ancel", Command::CANCEL, ButtonFlags::new()),
            ],
            ButtonRowAlign::Right,
        );

        // Resolve the search scope from the binding. The candidate filter searches
        // the first object class (the structural class for the candidate profile);
        // the requested attrs cover cn/uid (the filter dimensions), the label
        // attrs, and the scalar store attribute when present.
        let store_attr = match &binding.store {
            StoreKey::Dn => None,
            StoreKey::Attr(a) => Some(a.clone()),
        };
        let key_ci = matches!(binding.store, StoreKey::Dn);
        let oc = binding
            .scope
            .object_classes
            .first()
            .cloned()
            .unwrap_or_default();
        let mut attrs: Vec<String> = vec!["cn".to_string(), "uid".to_string()];
        for a in &binding.scope.search_attrs {
            if !attrs.iter().any(|x| x.eq_ignore_ascii_case(a)) {
                attrs.push(a.clone());
            }
        }
        if let Some(a) = &store_attr {
            if !attrs.iter().any(|x| x.eq_ignore_ascii_case(a)) {
                attrs.push(a.clone());
            }
        }

        // Seed the selection from the field's current values. For a DN store the
        // value is the DN; for a scalar store it is the scalar — either way it is
        // both the `store_value` (commit key) and a placeholder label until a
        // search reveals the friendly one.
        let selected: Vec<Candidate> = current
            .into_iter()
            .map(|v| Candidate {
                dn: v.clone(),
                label: v.clone(),
                store_value: v,
            })
            .collect();
        let pick = PickState::new(selected, key_ci);

        PickerDialog {
            dlg,
            search_id,
            list_id,
            shared,
            pick,
            cardinality,
            base: binding.scope.base.clone(),
            oc,
            attrs,
            store_attr,
            last_search: String::new(),
            seeded: false,
        }
    }

    /// Marker prefix for a row: radio for single, checkbox for multi.
    fn marker(&self, selected: bool, saved: bool) -> &'static str {
        match (self.cardinality, selected, saved) {
            (Cardinality::Single, true, _) => "(\u{2022}) ",
            (Cardinality::Single, false, _) => "( ) ",
            (Cardinality::Multi, true, _) => "[x] ",
            // saved-but-removed (will be deleted on save): mark distinctly.
            (Cardinality::Multi, false, true) => "[-] ",
            (Cardinality::Multi, false, false) => "[ ] ",
        }
    }

    /// Rebuild the ListBox rows from `pick.visible()`. Preserve the cursor on a
    /// toggle (rows keep order); reset to top when the result set changes.
    fn rebuild_list(&mut self, ctx: &mut Context, preserve_cursor: bool) {
        let rows: Vec<String> = self
            .pick
            .visible()
            .iter()
            .map(|r| format!("{}{}", self.marker(r.selected, r.saved), r.candidate.label))
            .collect();
        let rows_len = rows.len();
        if let Some(list) = self.dlg.child_mut(self.list_id) {
            let saved_sel: Option<i32> = if preserve_cursor {
                match list.value() {
                    Some(FieldValue::Int(i)) => Some(i),
                    _ => None,
                }
            } else {
                None
            };
            if let Some(lb) = list.as_any_mut().and_then(|a| a.downcast_mut::<ListBox>()) {
                lb.new_list(rows, ctx);
            }
            if let Some(sel) = saved_sel {
                let clamped = sel.min((rows_len.saturating_sub(1)) as i32).max(0);
                list.set_value_ctx(FieldValue::Int(clamped), ctx);
            }
        }
    }

    /// Copy the latest pump-delivered search results into `pick` and re-render.
    /// Borrow-safe: clones out of `shared` then drops the borrow before mutating.
    fn sync_results(&mut self, ctx: &mut Context) {
        let (results, truncated) = {
            let st = self.shared.borrow();
            (st.search_results.clone(), st.search_truncated)
        };
        self.pick.set_results(results);
        self.pick.truncated = truncated;
        self.pick.search_active = !self.last_search.is_empty();
        self.rebuild_list(ctx, false);
    }

    /// Write the prospective commit (the selected store values) into shared state.
    fn update_staged(&self) {
        let values = self.pick.selected_values();
        self.shared.borrow_mut().staged_commit = Some(CommitOutcome::SetValues(values));
    }

    /// Submit a candidate search for `term` via the worker. One atomic borrow.
    fn submit_search(&self, term: &str) {
        self.shared.borrow_mut().submit_search(
            &self.base,
            &self.oc,
            term,
            &self.attrs,
            self.store_attr.as_deref(),
        );
    }

    /// The current search-box text.
    fn current_search(&mut self) -> String {
        match self.dlg.child_mut(self.search_id).and_then(|v| v.value()) {
            Some(FieldValue::Text(s)) => s,
            _ => String::new(),
        }
    }

    /// The list-highlight index, if any.
    fn highlighted_index(&mut self) -> Option<usize> {
        match self.dlg.child_mut(self.list_id).and_then(|v| v.value()) {
            Some(FieldValue::Int(i)) if i >= 0 => Some(i as usize),
            _ => None,
        }
    }

    /// Pick the candidate at visible-row `idx`: replace (single) or toggle (multi).
    fn pick_at(&mut self, idx: usize, ctx: &mut Context) {
        let rows = self.pick.visible();
        let Some(row) = rows.get(idx) else {
            return;
        };
        let cand = row.candidate.clone();
        match self.cardinality {
            Cardinality::Single => {
                // Radio: the pick replaces the whole selection.
                self.pick.selected = vec![cand];
            }
            Cardinality::Multi => {
                // Checkbox: drive PickState's keyed toggle at this row.
                self.pick.cursor = idx;
                self.pick.toggle_cursor();
            }
        }
        self.rebuild_list(ctx, true);
        self.update_staged();
    }
}

#[delegate(to = dlg)]
impl View for PickerDialog {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    /// Seed on first open: copy any already-delivered results into `pick`, render
    /// the selection (selected-first), stage the current selection, and kick off
    /// an initial (empty-term) candidate search so the list fills in.
    fn reset_current(&mut self, ctx: &mut Context) {
        self.dlg.reset_current(ctx);
        if !self.seeded {
            self.seeded = true;
            self.sync_results(ctx);
            self.update_staged();
            self.submit_search("");
        }
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        // Fallback seed for paths that deliver events without reset_current.
        if !self.seeded {
            self.seeded = true;
            self.sync_results(ctx);
            self.update_staged();
            self.submit_search("");
        }

        // Pump-delivered results: refresh the list from shared state.
        if matches!(ev, Event::Broadcast { command, .. } if *command == REFRESH) {
            self.sync_results(ctx);
            self.dlg.handle_event(ev, ctx);
            return;
        }

        // Insert toggles/selects the highlighted candidate; Space is NOT
        // intercepted here so it falls through to the search InputLine.
        let insert = matches!(ev, Event::KeyDown(k) if k.key == Key::Insert);
        let nav = matches!(
            ev,
            Event::KeyDown(k)
                if matches!(k.key, Key::Up | Key::Down | Key::PageUp | Key::PageDown)
        );

        if insert {
            if let Some(idx) = self.highlighted_index() {
                self.pick_at(idx, ctx);
            }
            ev.clear();
        } else if nav {
            if let Some(list) = self.dlg.child_mut(self.list_id) {
                list.handle_event(ev, ctx);
            }
        } else {
            self.dlg.handle_event(ev, ctx);
        }

        // Submit a fresh search when the search text changed.
        let cur = self.current_search();
        if cur != self.last_search {
            self.last_search = cur.clone();
            self.pick.search_active = !cur.is_empty();
            self.submit_search(&cur);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::relation::CandidateScope;
    use crate::ldap::worker::RawSubschema;
    use crate::schema::FieldKind;
    use crate::workflows::form_model::WidgetSpec;
    use std::cell::RefCell;
    use std::rc::Rc;
    use tvision_rs::{timer::TimerQueue, Deferred, KeyEvent};

    fn schema() -> SchemaModel {
        SchemaModel::from_raw(&RawSubschema::default())
    }

    fn test_shared() -> Shared {
        use crate::workflows::structure::Structure;
        let st = crate::ui::state::UiState::new_for_test(
            Structure::build("dc=example,dc=org", vec![]),
            schema(),
            "dc=example,dc=org".into(),
            Vec::new(),
            Vec::new(),
        );
        Rc::new(RefCell::new(st))
    }

    fn headless_ctx<'a>(
        out: &'a mut std::collections::VecDeque<tv::Event>,
        timers: &'a mut TimerQueue,
        deferred: &'a mut Vec<Deferred>,
    ) -> Context<'a> {
        Context::new(out, timers, 0, deferred)
    }

    fn dn_scope() -> CandidateScope {
        CandidateScope {
            base: "ou=people,dc=example,dc=org".into(),
            object_classes: vec!["inetOrgPerson".into()],
            search_attrs: vec!["uid".into(), "cn".into()],
            label_template: None,
        }
    }

    fn picker_field(
        label: &str,
        values: &[&str],
        binding: PickerBinding,
        multi: bool,
    ) -> EditField {
        EditField {
            label: label.into(),
            must: false,
            editable: true,
            multi,
            secret: false,
            ordered: false,
            orphaned: false,
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: Some(WidgetKind::Picker(binding)),
            values: values.iter().map(|s| s.to_string()).collect(),
            baseline: values.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn multi_dn_binding() -> PickerBinding {
        PickerBinding {
            attr: "member".into(),
            scope: dn_scope(),
            store: StoreKey::Dn,
            select: Some(Cardinality::Multi),
            fanout_attr: None,
        }
    }

    fn cand(dn: &str, store: &str, label: &str) -> Candidate {
        Candidate {
            dn: dn.into(),
            label: label.into(),
            store_value: store.into(),
        }
    }

    // -- Task 13: present joins selected store values ----------------------

    #[test]
    fn present_joins_values_or_none() {
        let w = PickerWidget;
        let mut f = picker_field("member", &[], multi_dn_binding(), true);
        assert_eq!(w.present(&f), "\u{2039}none\u{203a}");
        f.values = vec!["uid=a,ou=people,dc=example,dc=org".into()];
        assert_eq!(w.present(&f), "uid=a,ou=people,dc=example,dc=org");
        f.values = vec!["uid=a,o=x".into(), "uid=b,o=x".into()];
        assert_eq!(w.present(&f), "uid=a,o=x, uid=b,o=x");
    }

    #[test]
    fn non_fanout_picker_activates_modal() {
        let f = picker_field("member", &[], multi_dn_binding(), true);
        assert!(matches!(PickerWidget.activate(&f), Activation::Modal(_)));
    }

    #[test]
    fn fanout_picker_does_not_activate_here() {
        // A fan-out (membership) binding is a later task — this widget yields Inline.
        let mut b = multi_dn_binding();
        b.fanout_attr = Some("member".into());
        let f = picker_field("memberOf", &[], b, true);
        assert!(matches!(PickerWidget.activate(&f), Activation::Inline));
    }

    // -- Task 14: headless dialog — seed results, toggle, assert staged ----

    /// RED→GREEN: seed `search_results` with two candidates, `reset_current`
    /// builds the list, toggling one (multi) with Insert stages
    /// `SetValues([store_value])`.
    #[test]
    fn multi_toggle_stages_selected_store_value() {
        let shared = test_shared();
        // Two delivered candidates (DN store: store_value == dn).
        shared.borrow_mut().search_results = vec![
            cand(
                "uid=alice,ou=people,dc=example,dc=org",
                "uid=alice,ou=people,dc=example,dc=org",
                "Alice",
            ),
            cand(
                "uid=bob,ou=people,dc=example,dc=org",
                "uid=bob,ou=people,dc=example,dc=org",
                "Bob",
            ),
        ];

        let ed: Box<dyn FieldEditor> = Box::new(PickerEditor {
            label: "member".into(),
            binding: multi_dn_binding(),
            current: vec![],
            multi: true,
        });
        let (mut view, _focus) = ed.into_view(&schema(), shared.clone());

        let mut out = std::collections::VecDeque::new();
        let mut timers = TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        view.reset_current(&mut ctx);

        // Initially nothing selected.
        assert_eq!(
            shared.borrow().staged_commit,
            Some(CommitOutcome::SetValues(vec![]))
        );

        // Highlight row 0 (Alice) and press Insert to tick it.
        let dlg = view
            .as_any_mut()
            .and_then(|a| a.downcast_mut::<PickerDialog>())
            .expect("downcast PickerDialog");
        if let Some(list) = dlg.dlg.child_mut(dlg.list_id) {
            list.set_value_ctx(FieldValue::Int(0), &mut ctx);
        }
        let mut ev = Event::KeyDown(KeyEvent::from(Key::Insert));
        dlg.handle_event(&mut ev, &mut ctx);

        assert_eq!(
            shared.borrow().staged_commit,
            Some(CommitOutcome::SetValues(vec![
                "uid=alice,ou=people,dc=example,dc=org".to_string()
            ])),
            "Insert on Alice must stage her DN as the only selected value"
        );
    }

    /// Space must NOT toggle any candidate — it must pass through to the search
    /// box so users can type multi-word queries like "Ann Smith" or "van der".
    #[test]
    fn space_does_not_toggle_candidate() {
        let shared = test_shared();
        shared.borrow_mut().search_results = vec![cand(
            "uid=alice,ou=people,dc=example,dc=org",
            "uid=alice,ou=people,dc=example,dc=org",
            "Alice",
        )];

        let ed: Box<dyn FieldEditor> = Box::new(PickerEditor {
            label: "member".into(),
            binding: multi_dn_binding(),
            current: vec![],
            multi: true,
        });
        let (mut view, _focus) = ed.into_view(&schema(), shared.clone());

        let mut out = std::collections::VecDeque::new();
        let mut timers = TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        view.reset_current(&mut ctx);

        let dlg = view
            .as_any_mut()
            .and_then(|a| a.downcast_mut::<PickerDialog>())
            .expect("downcast PickerDialog");
        // Highlight row 0.
        if let Some(list) = dlg.dlg.child_mut(dlg.list_id) {
            list.set_value_ctx(FieldValue::Int(0), &mut ctx);
        }
        // Press Space — must NOT toggle Alice.
        let mut ev = Event::KeyDown(KeyEvent::from(Key::Char(' ')));
        dlg.handle_event(&mut ev, &mut ctx);

        assert_eq!(
            shared.borrow().staged_commit,
            Some(CommitOutcome::SetValues(vec![])),
            "Space must not toggle any candidate — it should reach the search box"
        );
    }

    /// Single-select radio: a pick replaces the selection (does not accumulate).
    #[test]
    fn single_pick_replaces_selection() {
        let shared = test_shared();
        shared.borrow_mut().search_results = vec![
            cand("cn=devs,ou=groups,dc=example,dc=org", "1001", "devs"),
            cand("cn=ops,ou=groups,dc=example,dc=org", "1002", "ops"),
        ];
        let binding = PickerBinding {
            attr: "gidNumber".into(),
            scope: CandidateScope {
                base: "ou=groups,dc=example,dc=org".into(),
                object_classes: vec!["posixGroup".into()],
                search_attrs: vec!["cn".into()],
                label_template: None,
            },
            store: StoreKey::Attr("gidNumber".into()),
            select: Some(Cardinality::Single),
            fanout_attr: None,
        };
        let ed: Box<dyn FieldEditor> = Box::new(PickerEditor {
            label: "gidNumber".into(),
            binding,
            current: vec![],
            multi: false,
        });
        let (mut view, _focus) = ed.into_view(&schema(), shared.clone());

        let mut out = std::collections::VecDeque::new();
        let mut timers = TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        view.reset_current(&mut ctx);

        let dlg = view
            .as_any_mut()
            .and_then(|a| a.downcast_mut::<PickerDialog>())
            .expect("downcast");

        // Pick row 0 (devs → 1001) via Insert.
        if let Some(list) = dlg.dlg.child_mut(dlg.list_id) {
            list.set_value_ctx(FieldValue::Int(0), &mut ctx);
        }
        let mut ev = Event::KeyDown(KeyEvent::from(Key::Insert));
        dlg.handle_event(&mut ev, &mut ctx);
        assert_eq!(
            shared.borrow().staged_commit,
            Some(CommitOutcome::SetValues(vec!["1001".to_string()]))
        );

        // Pick row 1 (ops → 1002) via Insert: single-select must REPLACE, not add.
        if let Some(list) = dlg.dlg.child_mut(dlg.list_id) {
            list.set_value_ctx(FieldValue::Int(1), &mut ctx);
        }
        let mut ev = Event::KeyDown(KeyEvent::from(Key::Insert));
        dlg.handle_event(&mut ev, &mut ctx);
        assert_eq!(
            shared.borrow().staged_commit,
            Some(CommitOutcome::SetValues(vec!["1002".to_string()])),
            "single-select replaces the prior pick"
        );
    }

    /// Seeding from current values shows them selected-first before any search.
    #[test]
    fn reset_seeds_current_selection() {
        let shared = test_shared();
        let ed: Box<dyn FieldEditor> = Box::new(PickerEditor {
            label: "member".into(),
            binding: multi_dn_binding(),
            current: vec!["uid=carol,ou=people,dc=example,dc=org".into()],
            multi: true,
        });
        let (mut view, _focus) = ed.into_view(&schema(), shared.clone());
        let mut out = std::collections::VecDeque::new();
        let mut timers = TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        view.reset_current(&mut ctx);
        assert_eq!(
            shared.borrow().staged_commit,
            Some(CommitOutcome::SetValues(vec![
                "uid=carol,ou=people,dc=example,dc=org".to_string()
            ])),
            "the seeded selection is staged on open"
        );
    }
}
