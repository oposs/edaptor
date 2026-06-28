//! Free-text multi-value editor: a `ListBox` of rows plus an `InputLine` for the
//! selected row, with add / edit / delete / reorder. Stages
//! `CommitOutcome::SetValues` (rows trimmed, empties dropped) into
//! `UiState::staged_commit` live, so the OK path applies it. Capability:
//! `Static` (no worker, no schema). Mirrors the `oc_picker` modal pattern.

use tvision_rs::{
    self as tv, delegate, ButtonFlags, ButtonRowAlign, Command, Context, Dialog, Event, FieldValue,
    InputLine, Key, ListBox, Rect, View,
};

use crate::schema::SchemaModel;
use crate::tui::widget::{Activation, Capability, CommitOutcome, FieldEditor, FieldWidget};
use crate::tui::Shared;
use crate::workflows::edit_form::EditField;

/// The plugin for plain editable multi-value fields (no widget binding, not
/// objectClass / password). Presents a comma-joined summary and opens a modal.
pub(crate) struct MultiValueWidget;

impl FieldWidget for MultiValueWidget {
    fn capability(&self) -> Capability {
        Capability::Static
    }

    fn present(&self, field: &EditField) -> String {
        if field.values.iter().all(|v| v.trim().is_empty()) {
            "\u{2014}".to_string() // em dash
        } else {
            field
                .values
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        }
    }

    fn activate(&self, field: &EditField) -> Activation {
        Activation::Modal(Box::new(MultiValueEditor {
            label: field.label.clone(),
            values: field.values.clone(),
            ordered: field.ordered,
        }))
    }
}

/// Carries the field's current values into the dialog builder.
pub(crate) struct MultiValueEditor {
    pub label: String,
    pub values: Vec<String>,
    pub ordered: bool,
}

impl FieldEditor for MultiValueEditor {
    fn into_view(
        self: Box<Self>,
        _schema: &SchemaModel,
        shared: Shared,
    ) -> (Box<dyn View>, tv::ViewId) {
        let MultiValueEditor {
            label,
            values,
            ordered,
        } = *self;
        // `ordered` is informational only: row order is preserved by the Vec, and
        // ordered-vs-set dirty detection lives in the diff layer (not here).
        let _ = ordered;
        let dlg = MultiValueDialog::new(label, values, shared);
        let focus = dlg.input_id;
        (Box::new(dlg), focus)
    }
}

/// The interactive dialog: a row list + an edit line + OK/Cancel. `rows` is the
/// source of truth; the `InputLine` mirrors the selected row.
pub(crate) struct MultiValueDialog {
    dlg: Dialog,
    list_id: tv::ViewId,
    input_id: tv::ViewId,
    shared: Shared,
    rows: Vec<String>,
    sel: usize,
}

impl MultiValueDialog {
    fn new(label: String, values: Vec<String>, shared: Shared) -> Self {
        let title = format!("Edit {label}");
        let mut dlg = Dialog::new(Rect::new(0, 0, 60, 20), Some(title));
        dlg.state_mut().options.center_x = true;
        dlg.state_mut().options.center_y = true;
        // Row list (rows 1..15) and the edit line (row 16) inside the frame.
        let list = ListBox::new(Rect::new(2, 1, 58, 15), 1, None, None);
        let list_id = dlg.insert_child(Box::new(list));
        let input = InputLine::with_limit(Rect::new(2, 16, 58, 17), 1024);
        let input_id = dlg.insert_child(Box::new(input));
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
        MultiValueDialog {
            dlg,
            list_id,
            input_id,
            shared,
            rows: values,
            sel: 0,
        }
    }

    /// Rebuild the visible list from `rows` and restore the highlight to `sel`
    /// (clamped to the list length). Empty lists leave the highlight at 0.
    fn refresh_list(&mut self, ctx: &mut Context) {
        let rows = self.rows.clone();
        let len = rows.len();
        if let Some(list) = self.dlg.child_mut(self.list_id) {
            if let Some(lb) = list.as_any_mut().and_then(|a| a.downcast_mut::<ListBox>()) {
                lb.new_list(rows, ctx);
            }
            if len > 0 {
                let clamped = self.sel.min(len - 1) as i32;
                list.set_value_ctx(FieldValue::Int(clamped), ctx);
            }
        }
    }

    /// Mirror the selected row's text into the edit line (empty when no rows).
    fn load_input(&mut self) {
        let text = self.rows.get(self.sel).cloned().unwrap_or_default();
        if let Some(c) = self.dlg.child_mut(self.input_id) {
            c.set_value(FieldValue::Text(text));
        }
    }

    /// Write the prospective commit (rows trimmed, empties dropped) into shared
    /// state. Short borrow, taken and dropped here only.
    fn update_staged(&self) {
        let values: Vec<String> = self
            .rows
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        self.shared.borrow_mut().staged_commit = Some(CommitOutcome::SetValues(values));
    }

    /// Refresh list + edit line + staged commit after any mutation.
    fn refresh_all(&mut self, ctx: &mut Context) {
        self.refresh_list(ctx);
        self.load_input();
        self.update_staged();
    }

    /// Move the highlight by `delta`, clamped to the list bounds.
    fn move_sel(&mut self, delta: i32, ctx: &mut Context) {
        if self.rows.is_empty() {
            self.sel = 0;
            return;
        }
        let len = self.rows.len() as i32;
        let mut s = self.sel as i32 + delta;
        if s < 0 {
            s = 0;
        }
        if s >= len {
            s = len - 1;
        }
        self.sel = s as usize;
        self.refresh_list(ctx);
        self.load_input();
    }

    /// Swap the selected row with its neighbour (bounded), following the move.
    fn swap_row(&mut self, delta: i32, ctx: &mut Context) {
        if self.rows.len() < 2 {
            return;
        }
        let j = self.sel as i32 + delta;
        if j < 0 || j >= self.rows.len() as i32 {
            return;
        }
        let j = j as usize;
        self.rows.swap(self.sel, j);
        self.sel = j;
        self.refresh_all(ctx);
    }

    /// Insert a fresh empty row at `sel + 1` and move there.
    fn add_row(&mut self, ctx: &mut Context) {
        let at = if self.rows.is_empty() {
            0
        } else {
            self.sel + 1
        };
        self.rows.insert(at, String::new());
        self.sel = at;
        self.refresh_all(ctx);
    }

    /// Remove the selected row and clamp the highlight.
    fn delete_row(&mut self, ctx: &mut Context) {
        if self.rows.is_empty() {
            return;
        }
        self.rows.remove(self.sel);
        if self.rows.is_empty() {
            self.sel = 0;
        } else if self.sel >= self.rows.len() {
            self.sel = self.rows.len() - 1;
        }
        self.refresh_all(ctx);
    }

    /// Append a character to the selected row (creating a first row if empty).
    fn type_char(&mut self, c: char, ctx: &mut Context) {
        if self.rows.is_empty() {
            self.rows.push(String::new());
            self.sel = 0;
        }
        self.rows[self.sel].push(c);
        self.refresh_all(ctx);
    }

    /// Drop the last character of the selected row.
    fn backspace(&mut self, ctx: &mut Context) {
        if let Some(row) = self.rows.get_mut(self.sel) {
            row.pop();
        }
        self.refresh_all(ctx);
    }
}

#[delegate(to = dlg)]
impl View for MultiValueDialog {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    /// Seed the row list on first open (the deterministic pre-draw hook, same as
    /// `oc_picker`). Sets the initial selection, mirrors the first row into the
    /// edit line, and stages the trimmed values.
    fn reset_current(&mut self, ctx: &mut Context) {
        self.dlg.reset_current(ctx);
        self.sel = 0;
        self.refresh_all(ctx);
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        let (key, alt) = match ev {
            Event::KeyDown(k) => (k.key, k.modifiers.alt),
            _ => {
                self.dlg.handle_event(ev, ctx);
                return;
            }
        };

        match (key, alt) {
            (Key::Up, false) => {
                self.move_sel(-1, ctx);
                ev.clear();
            }
            (Key::Down, false) => {
                self.move_sel(1, ctx);
                ev.clear();
            }
            (Key::Up, true) => {
                self.swap_row(-1, ctx);
                ev.clear();
            }
            (Key::Down, true) => {
                self.swap_row(1, ctx);
                ev.clear();
            }
            (Key::Char('a'), true) | (Key::Insert, _) => {
                self.add_row(ctx);
                ev.clear();
            }
            (Key::Char('d'), true) | (Key::Delete, _) => {
                self.delete_row(ctx);
                ev.clear();
            }
            (Key::Char(c), false) => {
                self.type_char(c, ctx);
                ev.clear();
            }
            (Key::Backspace, _) => {
                self.backspace(ctx);
                ev.clear();
            }
            _ => {
                self.dlg.handle_event(ev, ctx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldap::worker::RawSubschema;
    use crate::schema::{FieldKind, SchemaModel};
    use crate::workflows::form_model::WidgetSpec;
    use std::cell::RefCell;
    use std::rc::Rc;
    use tvision_rs::{timer::TimerQueue, Deferred, KeyEvent, KeyModifiers};

    // ----- Task 2: widget present -----------------------------------------

    fn multi_field(label: &str, vals: &[&str]) -> EditField {
        EditField {
            label: label.into(),
            must: false,
            editable: true,
            multi: true,
            secret: false,
            ordered: false,
            orphaned: false,
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: None,
            values: vals.iter().map(|s| s.to_string()).collect(),
            baseline: vals.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn present_lists_values_joined() {
        let w = MultiValueWidget;
        let f = multi_field("mail", &["a@x", "b@x"]);
        assert_eq!(w.present(&f), "a@x, b@x");
    }

    #[test]
    fn present_empty_is_dash() {
        let w = MultiValueWidget;
        let f = multi_field("mail", &[]);
        assert_eq!(w.present(&f), "\u{2014}");
    }

    // ----- Task 3: dialog --------------------------------------------------

    fn schema_for_test() -> SchemaModel {
        SchemaModel::from_raw(&RawSubschema::default())
    }

    fn test_shared() -> Shared {
        use crate::workflows::structure::Structure;
        let st = crate::tui::state::UiState::new_for_test(
            Structure::build("dc=example,dc=org", vec![]),
            schema_for_test(),
            "dc=example,dc=org".into(),
            Vec::new(),
            Vec::new(),
        );
        Rc::new(RefCell::new(st))
    }

    fn headless<'a>(
        out: &'a mut std::collections::VecDeque<tv::Event>,
        timers: &'a mut TimerQueue,
        deferred: &'a mut Vec<Deferred>,
    ) -> Context<'a> {
        Context::new(out, timers, 0, deferred)
    }

    fn key(view: &mut dyn View, ctx: &mut Context, k: Key, alt: bool) {
        let mut ev = Event::KeyDown(KeyEvent::new(
            k,
            KeyModifiers {
                alt,
                ..KeyModifiers::default()
            },
        ));
        view.handle_event(&mut ev, ctx);
    }

    fn staged(shared: &Shared) -> Option<CommitOutcome> {
        shared.borrow().staged_commit.clone()
    }

    #[test]
    fn ok_stages_trimmed_nonempty_values() {
        let shared = test_shared();
        let ed = Box::new(MultiValueEditor {
            label: "mail".into(),
            values: vec!["a@x".into(), "  ".into(), "b@x".into()],
            ordered: false,
        });
        let (mut view, _focus) = ed.into_view(&schema_for_test(), shared.clone());
        let mut out = std::collections::VecDeque::new();
        let mut timers = TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless(&mut out, &mut timers, &mut deferred);
        view.reset_current(&mut ctx);
        assert_eq!(
            staged(&shared),
            Some(CommitOutcome::SetValues(vec!["a@x".into(), "b@x".into()]))
        );
    }

    #[test]
    fn alt_down_swaps_rows() {
        let shared = test_shared();
        let ed = Box::new(MultiValueEditor {
            label: "mail".into(),
            values: vec!["a".into(), "b".into(), "c".into()],
            ordered: true,
        });
        let (mut view, _focus) = ed.into_view(&schema_for_test(), shared.clone());
        let mut out = std::collections::VecDeque::new();
        let mut timers = TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless(&mut out, &mut timers, &mut deferred);
        view.reset_current(&mut ctx);
        // sel = 0; Alt+Down swaps rows 0 and 1.
        key(view.as_mut(), &mut ctx, Key::Down, true);
        assert_eq!(
            staged(&shared),
            Some(CommitOutcome::SetValues(vec![
                "b".into(),
                "a".into(),
                "c".into()
            ]))
        );
    }

    #[test]
    fn alt_d_deletes_selected_row() {
        let shared = test_shared();
        let ed = Box::new(MultiValueEditor {
            label: "mail".into(),
            values: vec!["a".into(), "b".into(), "c".into()],
            ordered: false,
        });
        let (mut view, _focus) = ed.into_view(&schema_for_test(), shared.clone());
        let mut out = std::collections::VecDeque::new();
        let mut timers = TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless(&mut out, &mut timers, &mut deferred);
        view.reset_current(&mut ctx);
        // sel = 0; Alt+d removes "a".
        key(view.as_mut(), &mut ctx, Key::Char('d'), true);
        assert_eq!(
            staged(&shared),
            Some(CommitOutcome::SetValues(vec!["b".into(), "c".into()]))
        );
    }

    #[test]
    fn delete_all_then_navigate_does_not_panic() {
        let shared = test_shared();
        let ed = Box::new(MultiValueEditor {
            label: "mail".into(),
            values: vec!["a".into(), "b".into(), "c".into()],
            ordered: false,
        });
        let (mut view, _focus) = ed.into_view(&schema_for_test(), shared.clone());
        let mut out = std::collections::VecDeque::new();
        let mut timers = TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless(&mut out, &mut timers, &mut deferred);
        view.reset_current(&mut ctx);
        // Delete all three rows, then navigate up and down — must not panic.
        key(view.as_mut(), &mut ctx, Key::Delete, false);
        key(view.as_mut(), &mut ctx, Key::Delete, false);
        key(view.as_mut(), &mut ctx, Key::Delete, false);
        key(view.as_mut(), &mut ctx, Key::Down, false);
        key(view.as_mut(), &mut ctx, Key::Up, false);
        assert_eq!(staged(&shared), Some(CommitOutcome::SetValues(Vec::new())));
    }

    #[test]
    fn add_then_type_edits_new_row() {
        let shared = test_shared();
        let ed = Box::new(MultiValueEditor {
            label: "mail".into(),
            values: vec!["a".into()],
            ordered: false,
        });
        let (mut view, _focus) = ed.into_view(&schema_for_test(), shared.clone());
        let mut out = std::collections::VecDeque::new();
        let mut timers = TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless(&mut out, &mut timers, &mut deferred);
        view.reset_current(&mut ctx);
        // Add a row at sel+1, then type "z" into it.
        key(view.as_mut(), &mut ctx, Key::Insert, false);
        key(view.as_mut(), &mut ctx, Key::Char('z'), false);
        assert_eq!(
            staged(&shared),
            Some(CommitOutcome::SetValues(vec!["a".into(), "z".into()]))
        );
    }
}
