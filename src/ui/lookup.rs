//! The `lookup` widget: a scalar field shown as `<value> (<name>)` and edited via
//! an editable-combobox popup. This module holds the pure input model (parse /
//! validity / display) plus the FieldWidget/editor/dialog.
//! The value in the input is authoritative: its leading integer is the
//! committed value; picking a candidate writes `<value> (<name>)` back into it.
//!
//! The dialog is a combobox: an `InputLine` on top drives the candidate `ListBox`
//! below via the list's own incremental find (`FindMode::Filter`, fed with
//! [`ListViewer::set_find_query`] on 0.12+). Typing narrows the list in place;
//! navigating the list copies the focused row's `<value> (<name>)` back into the
//! input, which enables OK via the leading number.

/// The pending value = the leading run of ASCII digits in `input`, if any.
/// `"5000"` → `Some("5000")`; `"5000 (staff)"` → `Some("5000")`; `"staff"` → `None`;
/// `""` → `None`.
pub(crate) fn leading_number(input: &str) -> Option<String> {
    let digits: String = input
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        None
    } else {
        Some(digits)
    }
}

/// OK is enabled iff the input yields a committable value.
pub(crate) fn ok_enabled(input: &str) -> bool {
    leading_number(input).is_some()
}

/// A list row renders as `"{label} ({value})"`, e.g. `"staff (5000)"`.
pub(crate) fn row_display(value: &str, label: &str) -> String {
    format!("{label} ({value})")
}

/// Picking a row fills the input with `"{value} ({label})"`, e.g. `"5000 (staff)"`.
pub(crate) fn input_after_pick(value: &str, label: &str) -> String {
    format!("{value} ({label})")
}

/// The index of the row whose value exactly equals the input's leading number,
/// so a typed number highlights its matching group.
pub(crate) fn highlight_index(rows: &[(String, String)], input: &str) -> Option<usize> {
    let n = leading_number(input)?;
    rows.iter().position(|(value, _label)| *value == n)
}

// ---------------------------------------------------------------------------
// LookupWidget / LookupEditor / LookupDialog
// ---------------------------------------------------------------------------

use tvision_rs::{
    self as tv, delegate, Button, ButtonFlags, Command, Context, Dialog, Event, FieldValue,
    FindMode, InputLine, Key, ListBox, ListViewer, Rect, View,
};

use crate::config::relation::LookupBinding;
use crate::config::widget::WidgetKind;
use crate::schema::SchemaModel;
use crate::ui::widget::{Activation, Capability, CommitOutcome, FieldEditor, FieldWidget};
use crate::ui::{Shared, REFRESH};
use crate::workflows::edit_form::EditField;
use crate::workflows::pick_state::Candidate;

/// FieldWidget plugin for `WidgetKind::Lookup`. `present` returns the bare stored
/// value (the form pane enriches it to `<value> (<name>)` from the resolution
/// cache); `activate` opens a `LookupDialog`.
pub(crate) struct LookupWidget;

impl FieldWidget for LookupWidget {
    fn capability(&self) -> Capability {
        Capability::NeedsWorkerSearch
    }

    fn present(&self, field: &EditField) -> String {
        field.values.first().cloned().unwrap_or_default()
    }

    fn activate(&self, field: &EditField) -> Activation {
        match &field.widget_binding {
            Some(WidgetKind::Lookup(b)) => Activation::Modal(Box::new(LookupEditor {
                binding: b.clone(),
                current: field.values.first().cloned().unwrap_or_default(),
            })),
            _ => Activation::Inline,
        }
    }
}

/// Carries the binding + current value into the dialog builder.
pub(crate) struct LookupEditor {
    binding: LookupBinding,
    current: String,
}

impl FieldEditor for LookupEditor {
    fn into_view(
        self: Box<Self>,
        _schema: &SchemaModel,
        shared: Shared,
    ) -> (Box<dyn View>, tv::ViewId) {
        let LookupEditor { binding, current } = *self;
        let dlg = LookupDialog::new(binding, current, shared);
        let focus = dlg.input_id;
        (Box::new(dlg), focus)
    }
}

/// The interactive combobox: an input + OK/Cancel on row 2 (one blank row below
/// the title), a candidate list below. Candidates load once (empty-term search);
/// the list narrows itself via `FindMode::Filter`, fed from the input.
pub(crate) struct LookupDialog {
    dlg: Dialog,
    input_id: tv::ViewId,
    list_id: tv::ViewId,
    shared: Shared,
    binding: LookupBinding,
    /// All loaded candidates (value, label). Fed to the list as its find source;
    /// the list narrows the *displayed* rows itself (`FindMode::Filter`).
    all: Vec<(String, String)>,
    last_input: String,
    seeded: bool,
}

impl LookupDialog {
    pub(crate) fn new(binding: LookupBinding, current: String, shared: Shared) -> Self {
        let title = format!("Select {}", binding.attr);
        // Dialog: 64 wide, 20 tall. Inner usable area is inside the frame,
        // rows 1..18 (row 0 = title bar, row 19 = bottom border).
        let mut dlg = Dialog::new(Rect::new(0, 0, 64, 20), Some(title));
        dlg.state_mut().options.center_x = true;
        dlg.state_mut().options.center_y = true;

        // Row 2 (one blank row below the title): input on the left, OK/Cancel
        // buttons to its right (same row). Buttons use Button::new + insert_child
        // (like guard.rs) so we can control exact placement — button_row always
        // appends to the dialog's bottom row.
        let input = InputLine::with_limit(Rect::new(2, 2, 40, 3), 128);
        let input_id = dlg.insert_child(Box::new(input));
        let ok = Button::new(
            Rect::new(41, 2, 51, 4),
            "~O~K",
            Command::OK,
            ButtonFlags {
                default: true,
                ..ButtonFlags::new()
            },
        );
        dlg.insert_child(Box::new(ok));
        let cancel = Button::new(
            Rect::new(52, 2, 62, 4),
            "~C~ancel",
            Command::CANCEL,
            ButtonFlags::new(),
        );
        dlg.insert_child(Box::new(cancel));

        // Rows 4..: the list spans the full inner width below the input row. It
        // owns filtering: `FindMode::Filter` narrows the displayed rows to those
        // matching the query the input feeds via `set_find_query`.
        let list = ListBox::new(Rect::new(2, 4, 62, 18), 1, None, None).with_find(FindMode::Filter);
        let list_id = dlg.insert_child(Box::new(list));

        // Seed the InputLine with the current value (dialog scatter protocol;
        // set_value does not need a Context).
        if !current.is_empty() {
            if let Some(v) = dlg.child_mut(input_id) {
                v.set_value(FieldValue::Text(current.clone()));
            }
        }

        LookupDialog {
            dlg,
            input_id,
            list_id,
            shared,
            binding,
            all: Vec::new(),
            last_input: current,
            seeded: false,
        }
    }

    /// The label-template attributes the candidate search must fetch.
    fn label_attrs(&self) -> Vec<String> {
        let mut attrs = crate::config::label::template_attrs(&self.binding.label_template);
        if !attrs
            .iter()
            .any(|a| a.eq_ignore_ascii_case(&self.binding.store))
        {
            attrs.push(self.binding.store.clone());
        }
        if !attrs.iter().any(|a| a.eq_ignore_ascii_case("cn")) {
            attrs.push("cn".into());
        }
        attrs
    }

    /// Submit the one-shot candidate load (empty term = all candidates).
    fn submit_load(&self) {
        let attrs = self.label_attrs();
        self.shared.borrow_mut().submit_search(
            &self.binding.scope.base,
            self.binding.object_class(),
            "",
            &attrs,
            Some(&self.binding.store),
        );
    }

    /// Copy the pump-delivered candidates into `all`, rendering labels via the
    /// binding's template, then hand the full row set to the list as its find
    /// source and focus the row that matches the seeded value.
    fn sync_candidates(&mut self, ctx: &mut Context) {
        let results: Vec<Candidate> = self.shared.borrow().search_results.clone();
        self.all = results
            .into_iter()
            .map(|c| (c.store_value, c.label))
            .collect();
        // Feed the full candidate set as the list's source. `FindMode::Filter`
        // shows all rows until a query narrows them; the list owns the narrowing.
        let rows: Vec<String> = self
            .all
            .iter()
            .map(|(value, label)| row_display(value, label))
            .collect();
        if let Some(lb) = self
            .dlg
            .child_mut(self.list_id)
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<ListBox>())
        {
            lb.new_list(rows, ctx);
        }
        // Focus the row whose value matches the seeded input's leading number, so
        // the current selection is highlighted when the dialog opens. With an
        // empty query the displayed rows equal `all`, so the index lines up.
        if let Some(idx) = highlight_index(&self.all, &self.last_input) {
            if let Some(list) = self.dlg.child_mut(self.list_id) {
                list.set_value_ctx(FieldValue::Int(idx as i32), ctx);
            }
        }
    }

    fn current_input(&mut self) -> String {
        match self.dlg.child_mut(self.input_id).and_then(|v| v.value()) {
            Some(FieldValue::Text(s)) => s,
            _ => String::new(),
        }
    }

    fn set_input(&mut self, text: &str, ctx: &mut Context) {
        if let Some(v) = self.dlg.child_mut(self.input_id) {
            v.set_value_ctx(FieldValue::Text(text.to_string()), ctx);
        }
    }

    /// Reflect validity into the OK command and stage the commit.
    fn sync_ok(&mut self, ctx: &mut Context) {
        let input = self.current_input();
        if ok_enabled(&input) {
            // ok_enabled guarantees a leading number exists.
            let value = leading_number(&input).expect("ok_enabled → leading number");
            ctx.enable_command(Command::OK);
            self.shared.borrow_mut().staged_commit = Some(CommitOutcome::SetValues(vec![value]));
        } else {
            ctx.disable_command(Command::OK);
            self.shared.borrow_mut().staged_commit = None;
        }
    }

    /// Copy the focused row into the input as `<value> (<name>)`. Called when the
    /// user engages the list (navigation or selection); the leading number then
    /// enables OK. Maps the focused *displayed* row (the list narrows its own
    /// rows) back to a candidate by its rendered text.
    fn mirror_focused(&mut self, ctx: &mut Context) {
        let text = {
            let Some(lb) = self
                .dlg
                .child_mut(self.list_id)
                .and_then(|v| v.as_any_mut())
                .and_then(|a| a.downcast_mut::<ListBox>())
            else {
                return;
            };
            let idx = match lb.value() {
                Some(FieldValue::Int(i)) if i >= 0 => i as usize,
                _ => return,
            };
            match lb.list().get(idx) {
                Some(row) => row.clone(),
                None => return,
            }
        };
        let Some((value, label)) = self.all.iter().find(|(v, l)| row_display(v, l) == text) else {
            return;
        };
        let new_text = input_after_pick(value, label);
        self.set_input(&new_text, ctx);
        // last_input is updated to match the text just written so the end-of-event
        // change detector sees no change and does not feed it back as a query.
        self.last_input = new_text;
        self.sync_ok(ctx);
    }
}

#[delegate(to = dlg)]
impl View for LookupDialog {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    fn reset_current(&mut self, ctx: &mut Context) {
        self.dlg.reset_current(ctx);
        if !self.seeded {
            self.seeded = true;
            self.sync_candidates(ctx);
            self.submit_load();
            self.sync_ok(ctx);
        }
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        if !self.seeded {
            self.seeded = true;
            self.sync_candidates(ctx);
            self.submit_load();
            self.sync_ok(ctx);
        }

        // Pump-delivered candidate results.
        if matches!(ev, Event::Broadcast { command, .. } if *command == REFRESH) {
            self.sync_candidates(ctx);
            self.sync_ok(ctx);
            self.dlg.handle_event(ev, ctx);
            return;
        }

        // LIST_ITEM_SELECTED is broadcast by the ListBox when the user commits to
        // a row (Enter / Space / double-click). Intercept it to pick the row and
        // prevent the dialog from treating it as OK.
        let list_selected = matches!(
            ev,
            Event::Broadcast { command, source }
                if *command == Command::LIST_ITEM_SELECTED && *source == Some(self.list_id)
        );

        // Deliberate combobox idiom: nav keys are always routed to the list so
        // the user can browse candidates while the input stays focused and
        // editable. This mirrors the search-over-list routing in `picker.rs`.
        let nav = matches!(
            ev,
            Event::KeyDown(k)
                if matches!(k.key, Key::Up | Key::Down | Key::PageUp | Key::PageDown)
        );

        if list_selected {
            self.mirror_focused(ctx);
            ev.clear();
        } else if nav {
            // Route the nav key to the list (it moves the focused row), then copy
            // the now-focused row into the input — engaging the list is what fills
            // the field and enables OK.
            if let Some(list) = self.dlg.child_mut(self.list_id) {
                list.handle_event(ev, ctx);
            }
            self.mirror_focused(ctx);
        } else {
            self.dlg.handle_event(ev, ctx);
        }

        // Detect typed input changes → drive the list's incremental find + re-stage.
        // A change from `mirror_focused` is already reflected in `last_input`, so it
        // is not fed back here as a query.
        let cur = self.current_input();
        if cur != self.last_input {
            self.last_input = cur.clone();
            if let Some(lb) = self
                .dlg
                .child_mut(self.list_id)
                .and_then(|v| v.as_any_mut())
                .and_then(|a| a.downcast_mut::<ListBox>())
            {
                lb.set_find_query(&cur, ctx);
            }
            self.sync_ok(ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leading_number_extracts_prefix_digits() {
        assert_eq!(leading_number("5000"), Some("5000".into()));
        assert_eq!(leading_number("5000 (staff)"), Some("5000".into()));
        assert_eq!(leading_number("staff"), None);
        assert_eq!(leading_number(""), None);
        assert_eq!(leading_number("  42x"), Some("42".into()));
    }

    #[test]
    fn ok_enabled_requires_leading_number() {
        assert!(ok_enabled("5000"));
        assert!(ok_enabled("5000 (staff)"));
        assert!(!ok_enabled("staff"));
        assert!(!ok_enabled(""));
    }

    #[test]
    fn display_helpers_use_opposite_orders() {
        assert_eq!(row_display("5000", "staff"), "staff (5000)");
        assert_eq!(input_after_pick("5000", "staff"), "5000 (staff)");
    }

    #[test]
    fn highlight_matches_exact_value() {
        let rows = vec![
            ("100".to_string(), "users".to_string()),
            ("5000".to_string(), "staff".to_string()),
        ];
        assert_eq!(highlight_index(&rows, "5000"), Some(1));
        assert_eq!(highlight_index(&rows, "5000 (staff)"), Some(1));
        assert_eq!(highlight_index(&rows, "50"), None); // prefix, not exact
        assert_eq!(highlight_index(&rows, "staff"), None); // no leading number
    }
}

#[cfg(test)]
mod dialog_tests {
    use super::*;
    use crate::config::relation::{CandidateScope, LookupBinding};
    use crate::ui::widget::CommitOutcome;
    use crate::workflows::pick_state::Candidate;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;
    use tvision_rs::{timer::TimerQueue, Deferred, Event, View};

    fn shared_with_candidates(cands: Vec<(&str, &str)>) -> crate::ui::Shared {
        let mut st = crate::ui::state::UiState::new_for_test(
            crate::workflows::structure::Structure::build("dc=x", vec![]),
            crate::schema::model::SchemaModel::from_raw(
                &crate::ldap::worker::RawSubschema::default(),
            ),
            "dc=x".into(),
            Vec::new(),
            Vec::new(),
        );
        st.search_results = cands
            .into_iter()
            .map(|(v, l)| Candidate {
                dn: format!("cn={l},dc=x"),
                label: l.into(),
                store_value: v.into(),
            })
            .collect();
        Rc::new(RefCell::new(st))
    }

    fn binding() -> LookupBinding {
        LookupBinding {
            attr: "gidNumber".into(),
            scope: CandidateScope {
                base: "ou=groups,dc=x".into(),
                object_classes: vec!["posixGroup".into()],
                search_attrs: vec!["cn".into()],
                label_template: None,
            },
            store: "gidNumber".into(),
            label_template: crate::config::label::parse_label_template("{cn}"),
        }
    }

    /// Read the list's currently displayed (narrowed) row count.
    fn list_row_count(dlg: &mut LookupDialog) -> usize {
        dlg.dlg
            .child_mut(dlg.list_id)
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<ListBox>())
            .map(|lb| lb.list().len())
            .unwrap_or(0)
    }

    /// After a pick (LIST_ITEM_SELECTED) copies a row into the input, the next
    /// typed change must feed the list's find immediately — no 1-keystroke lag.
    /// The `last_input` guard alone suffices because `set_value_ctx` on InputLine
    /// is synchronous.
    #[test]
    fn typing_after_pick_narrows_the_list_immediately() {
        let shared = shared_with_candidates(vec![("100", "users"), ("5000", "staff")]);
        let mut dlg = LookupDialog::new(binding(), String::new(), shared.clone());
        let mut out = VecDeque::new();
        let mut timers = TimerQueue::new();
        let mut deferred: Vec<Deferred> = Vec::new();
        let mut ctx = tvision_rs::Context::new(&mut out, &mut timers, 0, &mut deferred);

        // Seed candidates via REFRESH.
        let mut ev = Event::Broadcast {
            command: crate::ui::REFRESH,
            source: None,
        };
        dlg.handle_event(&mut ev, &mut ctx);
        // All 2 candidates displayed initially (empty query).
        assert_eq!(
            list_row_count(&mut dlg),
            2,
            "both candidates visible before pick"
        );

        // Simulate a pick by broadcasting LIST_ITEM_SELECTED for the list_id.
        // mirror_focused copies the focused row ("100 (users)") into the input and
        // sets last_input to the same string.
        let list_id = dlg.list_id;
        let mut ev = Event::Broadcast {
            command: tvision_rs::Command::LIST_ITEM_SELECTED,
            source: Some(list_id),
        };
        dlg.handle_event(&mut ev, &mut ctx);

        // Now simulate the user typing a query ("staff") by setting the input
        // value directly, then firing a benign event so the end-of-event change
        // detector runs (REFRESH returns early, so use an unrelated broadcast).
        if let Some(v) = dlg.dlg.child_mut(dlg.input_id) {
            v.set_value_ctx(tvision_rs::FieldValue::Text("staff".into()), &mut ctx);
        }
        let mut ev = Event::Broadcast {
            command: tvision_rs::Command::custom("test.tick"),
            source: None,
        };
        dlg.handle_event(&mut ev, &mut ctx);

        // set_find_query("staff") must have fired: only the staff row survives.
        assert_eq!(
            list_row_count(&mut dlg),
            1,
            "typed query must narrow the list immediately after a pick, no lag"
        );
    }

    #[test]
    fn seeded_numeric_input_stages_commit_and_enables_ok() {
        let shared = shared_with_candidates(vec![("100", "users"), ("5000", "staff")]);
        let mut dlg = LookupDialog::new(binding(), "5000".into(), shared.clone());
        let mut out = VecDeque::new();
        let mut timers = TimerQueue::new();
        let mut deferred: Vec<Deferred> = Vec::new();
        let mut ctx = tvision_rs::Context::new(&mut out, &mut timers, 0, &mut deferred);
        // First event seeds candidates + stages from the seeded "5000".
        let mut ev = Event::Broadcast {
            command: crate::ui::REFRESH,
            source: None,
        };
        dlg.handle_event(&mut ev, &mut ctx);
        assert_eq!(
            shared.borrow().staged_commit,
            Some(CommitOutcome::SetValues(vec!["5000".to_string()])),
            "seeded numeric input stages its value"
        );
    }
}
