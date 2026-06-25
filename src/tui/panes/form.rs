//! Editable entry form pane: a header row (DN + dirty marker) over per-field rows,
//! each a static label column + a value `InputLine`. Plain single-value fields are
//! editable; the rest stay disabled (read-only). On every event the editable
//! `InputLine`s are synced into the shared `EditForm` so a `SAVE` sees current
//! values, and the header's dirty marker is refreshed.

use tvision_rs::{self as tv, delegate, Context, Event, FieldValue, Group, InputLine, Rect, View};

use crate::tui::widget::{inline_editable, present_field};
use crate::tui::{Shared, REFRESH};
use crate::workflows::edit_form::EditForm;

const FORM_ROWS: usize = 32;
/// Columns reserved for the label before the value `InputLine`.
const LABEL_W: i32 = 22;

/// A disabled (read-only, skip-focus) `InputLine` used for header/label cells.
/// `StaticText` has no `set_value`, so we reuse the M1 disabled-InputLine idiom
/// for any cell whose text we update at render time.
fn ro_cell(bounds: Rect) -> InputLine {
    let mut il = InputLine::with_limit(bounds, 1024);
    il.state.state.disabled = true;
    il
}

pub(crate) struct FormPane {
    group: Group,
    header_id: tv::ViewId,
    /// Per field row: the value `InputLine` id (label is a disabled InputLine).
    value_ids: Vec<tv::ViewId>,
    label_ids: Vec<tv::ViewId>,
    state: Shared,
}

/// `"DN"` plus a ` *` marker when dirty.
fn header_text(form: &EditForm) -> String {
    let mark = if form.is_dirty() { " *" } else { "" };
    format!("{}{}", form.dn, mark)
}

impl FormPane {
    pub(crate) fn new(bounds: Rect, state: Shared) -> Self {
        let mut group = Group::new(bounds);
        let w = bounds.b.x - bounds.a.x;

        // Row 0: header (read-only cell).
        let header_id = group.insert(Box::new(ro_cell(Rect::new(0, 0, w, 1))));

        let mut value_ids = Vec::new();
        let mut label_ids = Vec::new();
        for i in 0..FORM_ROWS {
            let y = i as i32 + 1; // rows start below the header
            label_ids.push(group.insert(Box::new(ro_cell(Rect::new(0, y, LABEL_W, y + 1)))));
            let mut il = InputLine::with_limit(Rect::new(LABEL_W, y, w, y + 1), 1024);
            il.state.state.disabled = true; // default read-only; refresh enables editable rows
            value_ids.push(group.insert(Box::new(il)));
        }
        FormPane {
            group,
            header_id,
            value_ids,
            label_ids,
            state,
        }
    }

    /// Test seam: is the value InputLine for field `i` disabled?
    #[cfg(test)]
    pub(crate) fn value_disabled(&mut self, i: usize) -> bool {
        self.group
            .child_mut(self.value_ids[i])
            .map(|c| c.state().state.disabled)
            .unwrap_or(true)
    }

    /// Test seam: set the value InputLine text for field `i`.
    #[cfg(test)]
    pub(crate) fn set_value_text(&mut self, i: usize, text: String) {
        if let Some(c) = self.group.child_mut(self.value_ids[i]) {
            c.set_value(FieldValue::Text(text));
        }
    }

    /// Repaint header + all rows from `edit_form`.
    fn render(&mut self, ctx: &mut Context) {
        let _ = ctx;
        let (header, rows): (String, Vec<(String, String, bool)>) = {
            let st = self.state.borrow();
            match st.edit_form.as_ref() {
                None => (String::new(), Vec::new()),
                Some(form) => {
                    let header = header_text(form);
                    let rows = form
                        .fields
                        .iter()
                        .map(|f| {
                            let marker = if f.must { "*" } else { "" };
                            let label = format!("{}{}", f.label, marker);
                            (label, present_field(f), inline_editable(f))
                        })
                        .collect();
                    (header, rows)
                }
            }
        }; // borrow dropped

        if let Some(h) = self.group.child_mut(self.header_id) {
            h.set_value(FieldValue::Text(header));
        }
        for i in 0..FORM_ROWS {
            let (label, value, editable) = rows
                .get(i)
                .cloned()
                .unwrap_or_else(|| (String::new(), String::new(), false));
            if let Some(l) = self.group.child_mut(self.label_ids[i]) {
                l.set_value(FieldValue::Text(label));
            }
            if let Some(v) = self.group.child_mut(self.value_ids[i]) {
                v.set_value(FieldValue::Text(value));
                v.state_mut().state.disabled = !editable;
            }
        }
    }

    /// Sync each editable value InputLine's text into `edit_form`; refresh header.
    fn sync_into_form(&mut self) {
        // Collect (idx) for editable rows, then collect (idx, text) without holding borrow.
        let editable: Vec<usize> = {
            let st = self.state.borrow();
            match st.edit_form.as_ref() {
                None => Vec::new(),
                // Only the first FORM_ROWS fields have a value cell; bound the
                // index so a longer entry truncates instead of indexing past the
                // fixed cell pool. (Scrolling for >FORM_ROWS fields is M3 work.)
                Some(form) => form
                    .fields
                    .iter()
                    .enumerate()
                    .take(FORM_ROWS)
                    .filter(|(_, f)| inline_editable(f))
                    .map(|(i, _)| i)
                    .collect(),
            }
        };
        let mut edits: Vec<(usize, String)> = Vec::new();
        for &i in &editable {
            if let Some(FieldValue::Text(s)) = self
                .group
                .child_mut(self.value_ids[i])
                .and_then(|v| v.value())
            {
                edits.push((i, s));
            }
        }
        let header = {
            let mut st = self.state.borrow_mut();
            if let Some(form) = st.edit_form.as_mut() {
                for (i, s) in edits {
                    if form
                        .fields
                        .get(i)
                        .map(|f| f.values.first().map(String::as_str))
                        != Some(Some(s.as_str()))
                    {
                        form.set_value(i, s);
                    }
                }
                Some(header_text(form))
            } else {
                None
            }
        };
        if let (Some(text), Some(h)) = (header, self.group.child_mut(self.header_id)) {
            h.set_value(FieldValue::Text(text));
        }
    }
}

#[delegate(to = group)]
impl View for FormPane {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        // Render whenever the form needs it, on ANY event. The dispatch closure
        // (Discard, re-read) only sets `form_needs_render` — it cannot broadcast
        // REFRESH (Program has no broadcast) — and the 50ms pump timer reaches
        // this view, so a flagged re-render repaints within one tick.
        if self.state.borrow().form_needs_render {
            self.state.borrow_mut().form_needs_render = false;
            self.render(ctx);
        }
        let _ = REFRESH; // REFRESH still drives other panes; retained import
        self.group.handle_event(ev, ctx);
        // Keep edit_form current with the on-screen editors.
        self.sync_into_form();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{FieldKind, SchemaModel};
    use crate::tui::UiState;
    use crate::workflows::edit_form::{EditField, EditForm, FormMode};
    use crate::workflows::form_model::WidgetSpec;
    use crate::workflows::structure::Structure;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    fn ef(label: &str, val: &str, editable: bool) -> EditField {
        EditField {
            label: label.into(),
            must: false,
            editable,
            multi: false,
            secret: false,
            ordered: false,
            orphaned: false,
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: None,
            values: vec![val.into()],
            baseline: vec![val.into()],
        }
    }

    fn state_with_form() -> Shared {
        use crate::ldap::worker::RawSubschema;
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let structure = Structure::build("dc=x", vec![]);
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.edit_form = Some(EditForm {
            dn: "cn=a,dc=x".into(),
            mode: FormMode::Edit,
            object_classes: vec![],
            fields: vec![ef("cn", "a", true), ef("creatorsName", "admin", false)],
        });
        st.form_needs_render = true;
        Rc::new(RefCell::new(st))
    }

    fn headless_ctx<'a>(
        out: &'a mut VecDeque<Event>,
        timers: &'a mut tv::timer::TimerQueue,
        deferred: &'a mut Vec<tv::Deferred>,
    ) -> Context<'a> {
        Context::new(out, timers, 0, deferred)
    }

    #[test]
    fn more_fields_than_rows_truncates_without_panic() {
        // An entry with more attributes than FORM_ROWS must truncate gracefully —
        // sync_into_form/render must never index past the fixed value-cell pool.
        // Regression: panic "index out of bounds: len is 32 but index is 39".
        use crate::ldap::worker::RawSubschema;
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let structure = Structure::build("dc=x", vec![]);
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        let fields: Vec<EditField> = (0..FORM_ROWS + 8)
            .map(|i| ef(&format!("attr{i}"), "v", true))
            .collect();
        st.edit_form = Some(EditForm {
            dn: "cn=a,dc=x".into(),
            mode: FormMode::Edit,
            object_classes: vec![],
            fields,
        });
        st.form_needs_render = true;
        let shared: Shared = Rc::new(RefCell::new(st));
        let mut pane = FormPane::new(Rect::new(0, 0, 80, FORM_ROWS as i32 + 1), shared.clone());
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        // Must not panic (renders + syncs only the first FORM_ROWS fields).
        pane.handle_event(&mut ev, &mut ctx);
    }

    #[test]
    fn editable_rows_enabled_static_rows_disabled() {
        let shared = state_with_form();
        let mut pane = FormPane::new(Rect::new(0, 0, 80, FORM_ROWS as i32 + 1), shared.clone());
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut ev, &mut ctx);
        // value row 0 (cn) editable → enabled; value row 1 (creatorsName) disabled.
        assert!(!pane.value_disabled(0));
        assert!(pane.value_disabled(1));
    }

    #[test]
    fn editing_value_inputline_marks_form_dirty() {
        let shared = state_with_form();
        let mut pane = FormPane::new(Rect::new(0, 0, 80, FORM_ROWS as i32 + 1), shared.clone());
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut ev, &mut ctx);
        // Simulate a committed edit by writing the value InputLine's data directly.
        pane.set_value_text(0, "abc".into());
        let mut ev2 = Event::KeyDown(tv::KeyEvent::from(tv::Key::Char('x')));
        pane.handle_event(&mut ev2, &mut ctx);
        assert!(shared.borrow().edit_form.as_ref().unwrap().is_dirty());
    }

    #[test]
    fn umlaut_edit_roundtrips_graphemes() {
        // Grapheme-correct edit regression (folded from the spike umlaut test):
        // a multibyte value set into the InputLine survives the sync into edit_form.
        let shared = state_with_form();
        let mut pane = FormPane::new(Rect::new(0, 0, 80, FORM_ROWS as i32 + 1), shared.clone());
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut ev, &mut ctx);
        pane.set_value_text(0, "Müller-Lüdenscheidt".into());
        let mut ev2 = Event::KeyDown(tv::KeyEvent::from(tv::Key::Char('x')));
        pane.handle_event(&mut ev2, &mut ctx);
        let st = shared.borrow();
        assert_eq!(
            st.edit_form.as_ref().unwrap().fields[0].values,
            vec!["Müller-Lüdenscheidt".to_string()]
        );
    }
}
