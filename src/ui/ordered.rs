//! X-ORDERED multi-value editor: like the free-text multi-value editor, but it
//! owns the OpenLDAP `X-ORDERED 'VALUES'` `{n}` ordering prefix. Values are shown
//! with the `{n}` stripped; on commit the prefix is reconstructed from the current
//! row order, so reordering rows is the central operation. Staged values carry
//! `{n}`, so the neutral `form::changeset::diff` (which special-cases x-ordered
//! attrs into a single `Replace`) is unchanged. First save after editing may emit
//! one normalizing `Replace` if the server's stored indices were not `{0..n-1}`;
//! the server re-normalizes, so this is harmless. Capability: `Static`.

/// Drop a leading `{<digits>}` ordering prefix; return everything else unchanged.
/// A `{` not followed by one-or-more ASCII digits and a `}` is NOT a prefix.
pub(crate) fn strip_ordering(s: &str) -> &str {
    let bytes = s.as_bytes();
    if bytes.first() != Some(&b'{') {
        return s;
    }
    let mut i = 1;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    // Need at least one digit (i > 1) and a closing '}' right after.
    if i > 1 && bytes.get(i) == Some(&b'}') {
        &s[i + 1..]
    } else {
        s
    }
}

/// Prepend `{i}` (contiguous row index) to each row, in order.
pub(crate) fn reconstruct(rows: &[String]) -> Vec<String> {
    rows.iter()
        .enumerate()
        .map(|(i, r)| format!("{{{i}}}{r}"))
        .collect()
}

#[cfg(test)]
mod helper_tests {
    use super::*;

    #[test]
    fn strip_removes_leading_index_only() {
        assert_eq!(strip_ordering("{0}read by self"), "read by self");
        assert_eq!(strip_ordering("{12}write"), "write");
        assert_eq!(strip_ordering("{0}"), "");
    }

    #[test]
    fn strip_leaves_non_index_braces() {
        assert_eq!(strip_ordering("plain"), "plain");
        assert_eq!(strip_ordering("{}empty"), "{}empty");
        assert_eq!(strip_ordering("{a}x"), "{a}x");
        assert_eq!(strip_ordering("by group/{0}"), "by group/{0}");
        assert_eq!(strip_ordering(""), "");
    }

    #[test]
    fn reconstruct_numbers_rows_in_order() {
        assert_eq!(
            reconstruct(&["write".to_string(), "read".to_string()]),
            vec!["{0}write".to_string(), "{1}read".to_string()]
        );
    }

    #[test]
    fn strip_then_reconstruct_round_trips_order() {
        let stored = ["{0}a".to_string(), "{1}b".to_string()];
        let display: Vec<String> = stored
            .iter()
            .map(|s| strip_ordering(s).to_string())
            .collect();
        assert_eq!(
            reconstruct(&display),
            vec!["{0}a".to_string(), "{1}b".to_string()]
        );
    }
}

use tvision_rs::{
    self as tv, delegate, ButtonFlags, ButtonRowAlign, Command, Context, Dialog, Event, FieldValue,
    InputLine, Key, ListBox, Rect, View,
};

use crate::schema::SchemaModel;
use crate::ui::widget::{Activation, Capability, CommitOutcome, FieldEditor, FieldWidget};
use crate::ui::Shared;
use crate::workflows::edit_form::EditField;

/// Plugin for X-ORDERED editable multi-value fields (`WidgetKind::XOrdered`).
/// Presents the values with `{n}` stripped and opens the ordered modal editor.
pub(crate) struct OrderedWidget;

impl FieldWidget for OrderedWidget {
    fn capability(&self) -> Capability {
        Capability::Static
    }

    fn present(&self, field: &EditField) -> String {
        if field
            .values
            .iter()
            .all(|v| strip_ordering(v).trim().is_empty())
        {
            "\u{2014}".to_string() // em dash
        } else {
            field
                .values
                .iter()
                .map(|s| strip_ordering(s))
                .collect::<Vec<_>>()
                .join(", ")
        }
    }

    fn activate(&self, field: &EditField) -> Activation {
        Activation::Modal(Box::new(OrderedEditor {
            label: field.label.clone(),
            values: field.values.clone(),
        }))
    }
}

/// Carries the field's current (`{n}`-prefixed) values into the dialog builder.
pub(crate) struct OrderedEditor {
    pub label: String,
    pub values: Vec<String>,
}

impl FieldEditor for OrderedEditor {
    fn into_view(
        self: Box<Self>,
        _schema: &SchemaModel,
        shared: Shared,
    ) -> (Box<dyn View>, tv::ViewId) {
        let OrderedEditor { label, values } = *self;
        let dlg = OrderedDialog::new(label, values, shared);
        let focus = dlg.input_id;
        (Box::new(dlg), focus)
    }
}

/// The interactive dialog. `rows` holds the DISPLAY (stripped) values; the
/// `InputLine` mirrors the selected row. Staged values are reconstructed with
/// `{n}` from the current row order.
pub(crate) struct OrderedDialog {
    dlg: Dialog,
    list_id: tv::ViewId,
    input_id: tv::ViewId,
    shared: Shared,
    rows: Vec<String>,
    sel: usize,
}

impl OrderedDialog {
    fn new(label: String, values: Vec<String>, shared: Shared) -> Self {
        let title = format!("Edit {label} (ordered)");
        let mut dlg = Dialog::new(Rect::new(0, 0, 60, 20), Some(title));
        dlg.state_mut().options.center_x = true;
        dlg.state_mut().options.center_y = true;
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
        // Strip {n} on load: the dialog edits display values only.
        let rows = values
            .iter()
            .map(|v| strip_ordering(v).to_string())
            .collect();
        OrderedDialog {
            dlg,
            list_id,
            input_id,
            shared,
            rows,
            sel: 0,
        }
    }

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

    fn load_input(&mut self) {
        let text = self.rows.get(self.sel).cloned().unwrap_or_default();
        if let Some(c) = self.dlg.child_mut(self.input_id) {
            c.set_value(FieldValue::Text(text));
        }
    }

    /// Reconstruct `{n}` from the trimmed, non-empty rows in order and stage it.
    fn update_staged(&self) {
        let trimmed: Vec<String> = self
            .rows
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        self.shared.borrow_mut().staged_commit =
            Some(CommitOutcome::SetValues(reconstruct(&trimmed)));
    }

    fn refresh_all(&mut self, ctx: &mut Context) {
        self.refresh_list(ctx);
        self.load_input();
        self.update_staged();
    }

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

    fn type_char(&mut self, c: char, ctx: &mut Context) {
        if self.rows.is_empty() {
            self.rows.push(String::new());
            self.sel = 0;
        }
        self.rows[self.sel].push(c);
        self.refresh_all(ctx);
    }

    fn backspace(&mut self, ctx: &mut Context) {
        if let Some(row) = self.rows.get_mut(self.sel) {
            row.pop();
        }
        self.refresh_all(ctx);
    }
}

#[delegate(to = dlg)]
impl View for OrderedDialog {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

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
mod editor_tests {
    use super::*;
    use crate::config::widget::WidgetKind;
    use crate::ldap::worker::RawSubschema;
    use crate::schema::{FieldKind, SchemaModel};
    use crate::workflows::form_model::WidgetSpec;
    use std::cell::RefCell;
    use std::rc::Rc;
    use tvision_rs::{timer::TimerQueue, Deferred, KeyEvent, KeyModifiers};

    fn xordered_field(label: &str, vals: &[&str]) -> EditField {
        EditField {
            label: label.into(),
            must: false,
            editable: true,
            multi: true,
            secret: false,
            ordered: true,
            orphaned: false,
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: Some(WidgetKind::XOrdered),
            values: vals.iter().map(|s| s.to_string()).collect(),
            baseline: vals.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn schema_for_test() -> SchemaModel {
        SchemaModel::from_raw(&RawSubschema::default())
    }

    fn test_shared() -> Shared {
        use crate::workflows::structure::Structure;
        let st = crate::ui::state::UiState::new_for_test(
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
    fn present_strips_ordering_prefixes() {
        let w = OrderedWidget;
        let f = xordered_field("olcAccess", &["{0}read", "{1}write"]);
        assert_eq!(w.present(&f), "read, write");
    }

    #[test]
    fn open_stages_reconstructed_values_unchanged() {
        let shared = test_shared();
        let ed = Box::new(OrderedEditor {
            label: "olcAccess".into(),
            values: vec!["{0}read".into(), "{1}write".into()],
        });
        let (mut view, _focus) = ed.into_view(&schema_for_test(), shared.clone());
        let mut out = std::collections::VecDeque::new();
        let mut timers = TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless(&mut out, &mut timers, &mut deferred);
        view.reset_current(&mut ctx);
        // No edit: staged equals the original {n} values.
        assert_eq!(
            staged(&shared),
            Some(CommitOutcome::SetValues(vec![
                "{0}read".into(),
                "{1}write".into()
            ]))
        );
    }

    #[test]
    fn reorder_reassigns_indices() {
        let shared = test_shared();
        let ed = Box::new(OrderedEditor {
            label: "olcAccess".into(),
            values: vec!["{0}read".into(), "{1}write".into()],
        });
        let (mut view, _focus) = ed.into_view(&schema_for_test(), shared.clone());
        let mut out = std::collections::VecDeque::new();
        let mut timers = TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless(&mut out, &mut timers, &mut deferred);
        view.reset_current(&mut ctx);
        // sel = 0; Alt+Down swaps rows 0 and 1 → indices reassigned by order.
        key(view.as_mut(), &mut ctx, Key::Down, true);
        assert_eq!(
            staged(&shared),
            Some(CommitOutcome::SetValues(vec![
                "{0}write".into(),
                "{1}read".into()
            ]))
        );
    }

    #[test]
    fn delete_then_add_renumbers_contiguously() {
        let shared = test_shared();
        let ed = Box::new(OrderedEditor {
            label: "olcAccess".into(),
            values: vec!["{0}a".into(), "{1}b".into(), "{2}c".into()],
        });
        let (mut view, _focus) = ed.into_view(&schema_for_test(), shared.clone());
        let mut out = std::collections::VecDeque::new();
        let mut timers = TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless(&mut out, &mut timers, &mut deferred);
        view.reset_current(&mut ctx);
        // sel = 0; Alt+d deletes "a" → contiguous {0}b {1}c.
        key(view.as_mut(), &mut ctx, Key::Char('d'), true);
        assert_eq!(
            staged(&shared),
            Some(CommitOutcome::SetValues(vec!["{0}b".into(), "{1}c".into()]))
        );
    }
}
