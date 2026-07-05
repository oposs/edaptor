//! The `lookup` widget: a scalar field shown as `<value> (<name>)` and edited via
//! an editable-combobox popup. This module holds the pure input model (parse /
//! validity / filter / display) plus the FieldWidget/editor/dialog.
//! The value in the input is authoritative: its leading integer is the
//! committed value; picking a candidate writes `<value> (<name>)` back into it.

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

/// List-filter predicate: empty filter matches all; otherwise the candidate
/// matches when its label contains `filter` (case-insensitive) OR its value
/// starts with `filter` (numeric-prefix search when the user types digits).
pub(crate) fn row_matches(label: &str, value: &str, filter: &str) -> bool {
    let f = filter.trim();
    if f.is_empty() {
        return true;
    }
    label.to_ascii_lowercase().contains(&f.to_ascii_lowercase()) || value.starts_with(f)
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
    InputLine, Key, ListBox, Rect, View,
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

/// The interactive combobox: an input + OK/Cancel on row 1, a filtered candidate
/// list below. Candidates load once (empty-term search) and filter locally.
pub(crate) struct LookupDialog {
    dlg: Dialog,
    input_id: tv::ViewId,
    list_id: tv::ViewId,
    shared: Shared,
    binding: LookupBinding,
    /// All loaded candidates (value, label), unfiltered.
    all: Vec<(String, String)>,
    /// Current filtered view (indices into `all`), parallel to the ListBox rows.
    filtered: Vec<usize>,
    last_input: String,
    /// Set true right after a programmatic pick so the input-change detector does
    /// not immediately re-filter from the auto-filled `<value> (<name>)` text.
    suppress_filter: bool,
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

        // Row 1: input on the left, OK/Cancel buttons to its right (same row).
        // Buttons use Button::new + insert_child (like guard.rs) so we can control
        // exact placement — button_row always appends to the dialog's bottom row.
        let input = InputLine::with_limit(Rect::new(2, 1, 40, 2), 128);
        let input_id = dlg.insert_child(Box::new(input));
        let ok = Button::new(
            Rect::new(41, 1, 51, 3),
            "~O~K",
            Command::OK,
            ButtonFlags {
                default: true,
                ..ButtonFlags::new()
            },
        );
        dlg.insert_child(Box::new(ok));
        let cancel = Button::new(
            Rect::new(52, 1, 62, 3),
            "~C~ancel",
            Command::CANCEL,
            ButtonFlags::new(),
        );
        dlg.insert_child(Box::new(cancel));

        // Rows 3..: the list spans the full inner width below the input row.
        let list = ListBox::new(Rect::new(2, 3, 62, 18), 1, None, None);
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
            filtered: Vec::new(),
            last_input: current,
            suppress_filter: false,
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
    /// binding's template, then refilter.
    fn sync_candidates(&mut self, ctx: &mut Context) {
        let results: Vec<Candidate> = self.shared.borrow().search_results.clone();
        self.all = results
            .into_iter()
            .map(|c| (c.store_value, c.label))
            .collect();
        self.apply_filter(ctx);
    }

    /// Rebuild the ListBox from `all` filtered by the current input text.
    fn apply_filter(&mut self, ctx: &mut Context) {
        let filter = self.current_input();
        self.filtered = self
            .all
            .iter()
            .enumerate()
            .filter(|(_, (value, label))| row_matches(label, value, &filter))
            .map(|(i, _)| i)
            .collect();
        let rows: Vec<String> = self
            .filtered
            .iter()
            .map(|&i| {
                let (value, label) = &self.all[i];
                row_display(value, label)
            })
            .collect();
        if let Some(lb) = self
            .dlg
            .child_mut(self.list_id)
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<ListBox>())
        {
            lb.new_list(rows, ctx);
        }
        // Highlight the exact numeric match, if any.
        let rows_ref: Vec<(String, String)> =
            self.filtered.iter().map(|&i| self.all[i].clone()).collect();
        if let Some(idx) = highlight_index(&rows_ref, &filter) {
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

    /// Pick the highlighted row: write `<value> (<name>)` into the input.
    fn pick_highlighted(&mut self, ctx: &mut Context) {
        let idx = match self.dlg.child_mut(self.list_id).and_then(|v| v.value()) {
            Some(FieldValue::Int(i)) if i >= 0 => i as usize,
            _ => return,
        };
        let Some(&ai) = self.filtered.get(idx) else {
            return;
        };
        let (value, label) = self.all[ai].clone();
        let text = input_after_pick(&value, &label);
        self.set_input(&text, ctx);
        self.last_input = text;
        self.suppress_filter = true;
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

        // Nav keys are always forwarded directly to the list so the user can
        // browse candidates regardless of which widget currently holds focus.
        let nav = matches!(
            ev,
            Event::KeyDown(k)
                if matches!(k.key, Key::Up | Key::Down | Key::PageUp | Key::PageDown)
        );

        if list_selected {
            self.pick_highlighted(ctx);
            ev.clear();
        } else if nav {
            if let Some(list) = self.dlg.child_mut(self.list_id) {
                list.handle_event(ev, ctx);
            }
        } else {
            self.dlg.handle_event(ev, ctx);
        }

        // Detect input changes → refilter (unless a pick just set the text) + re-stage.
        let cur = self.current_input();
        if cur != self.last_input {
            self.last_input = cur;
            if self.suppress_filter {
                self.suppress_filter = false;
            } else {
                self.apply_filter(ctx);
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
    fn row_matches_by_label_substring_and_value_prefix() {
        assert!(row_matches("staff", "5000", "")); // empty → all
        assert!(row_matches("staff", "5000", "sta")); // label substring, ci
        assert!(row_matches("Staff", "5000", "aff"));
        assert!(row_matches("staff", "5000", "50")); // numeric prefix on value
        assert!(!row_matches("staff", "5000", "99"));
        assert!(!row_matches("users", "100", "xyz"));
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
