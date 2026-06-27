//! Editable entry form pane: a header row (DN + dirty marker) over per-field rows,
//! each a static label column + a value `InputLine`. Plain single-value fields are
//! editable; the rest stay disabled (read-only). On every event the editable
//! `InputLine`s are synced into the shared `EditForm` so a `SAVE` sees current
//! values, and the header's dirty marker is refreshed.
//!
//! The form is built on a `ScrollGroup` (rows 1..h) so arbitrarily many fields are
//! supported — there is no longer a fixed `FORM_ROWS` cap. Cells are rebuilt from
//! `EditForm.fields` whenever the shown entry changes.

use tvision_rs::{
    self as tv, delegate, Context, Event, FieldValue, Group, InputLine, Key, Rect, View,
};

use crate::tui::scroll_group::ScrollGroup;
use crate::tui::widget::{inline_editable, is_modal_field, present_field};
use crate::tui::{Shared, ACTIVATE, REFRESH};
use crate::workflows::edit_form::EditForm;

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

/// A field's value cell is focusable if it is inline-editable OR a modal-activated
/// field (objectClass): the latter is read-only text but must accept focus + Enter.
fn cell_focusable(f: &crate::workflows::edit_form::EditField) -> bool {
    inline_editable(f) || is_modal_field(f)
}

pub(crate) struct FormPane {
    /// Outer container: header (row 0) + ScrollGroup (rows 1..h).
    group: Group,
    header_id: tv::ViewId,
    scroll_id: tv::ViewId,
    /// One value `InputLine` id per field, in field order (full-length; no cap).
    value_ids: Vec<tv::ViewId>,
    /// One label `InputLine` (ro) id per field, parallel to `value_ids`.
    label_ids: Vec<tv::ViewId>,
    /// DN of the entry whose cells are currently built; `None` before first render.
    built_dn: Option<String>,
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
        // ofFirstClick: a single click into this pane (from another pane) both
        // focuses the pane and lands on the clicked field, rather than needing a
        // second click.
        group.state_mut().options.first_click = true;
        let w = bounds.b.x - bounds.a.x;
        let h = bounds.b.y - bounds.a.y;

        // Row 0: header (read-only cell). grow_mode hi_x so it widens with the pane.
        let mut header = ro_cell(Rect::new(0, 0, w, 1));
        header.state.grow_mode.hi_x = true;
        let header_id = group.insert(Box::new(header));
        // Rows 1..h: scrollable content pane. grow_mode hi_x+hi_y so it fills the pane.
        let mut sg = ScrollGroup::new(Rect::new(0, 1, w, h));
        sg.state_mut().grow_mode.hi_x = true;
        sg.state_mut().grow_mode.hi_y = true;
        let scroll_id = group.insert(Box::new(sg));

        FormPane {
            group,
            header_id,
            scroll_id,
            value_ids: Vec::new(),
            label_ids: Vec::new(),
            built_dn: None,
            state,
        }
    }

    /// Return a mutable reference to the inner `ScrollGroup`.
    fn scroll_mut(&mut self) -> Option<&mut ScrollGroup> {
        self.group
            .child_mut(self.scroll_id)
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<ScrollGroup>())
    }

    /// Test seam: number of value InputLine cells currently built (one per field,
    /// uncapped after the ScrollGroup rewrite).
    #[cfg(test)]
    pub(crate) fn field_cell_count(&self) -> usize {
        self.value_ids.len()
    }

    /// Test seam: is the value InputLine for field `i` disabled?
    #[cfg(test)]
    pub(crate) fn value_disabled(&mut self, i: usize) -> bool {
        let vid = self.value_ids[i];
        self.scroll_mut()
            .and_then(|sg| sg.child_mut(vid))
            .map(|c| c.state().state.disabled)
            .unwrap_or(true)
    }

    /// Test seam: set the value InputLine text for field `i`.
    #[cfg(test)]
    pub(crate) fn set_value_text(&mut self, i: usize, text: String) {
        let vid = self.value_ids[i];
        if let Some(sg) = self.scroll_mut() {
            if let Some(c) = sg.child_mut(vid) {
                c.set_value(FieldValue::Text(text));
            }
        }
    }

    /// Test seam: current bounds of the header cell.
    #[cfg(test)]
    pub(crate) fn header_bounds_for_test(&mut self) -> Rect {
        self.group
            .child_mut(self.header_id)
            .unwrap()
            .state()
            .get_bounds()
    }

    /// Test seam: current bounds of the ScrollGroup child.
    #[cfg(test)]
    pub(crate) fn scroll_bounds_for_test(&mut self) -> Rect {
        self.group
            .child_mut(self.scroll_id)
            .unwrap()
            .state()
            .get_bounds()
    }

    /// Rebuild one label+value cell pair per field into the `ScrollGroup`. Called
    /// when the shown entry changes (different `dn`). Borrow discipline: collect
    /// field metadata, drop the state borrow, then mutate the scroll group.
    fn rebuild_cells(&mut self, ctx: &mut Context) {
        // Collect field metadata (drop state borrow before touching views).
        let fields: Vec<(String, bool)> = {
            let st = self.state.borrow();
            match st.edit_form.as_ref() {
                None => Vec::new(),
                Some(form) => form
                    .fields
                    .iter()
                    .map(|f| {
                        let marker = if f.must { "*" } else { "" };
                        (format!("{}{}", f.label, marker), cell_focusable(f))
                    })
                    .collect(),
            }
        }; // state borrow dropped

        // Build all cells. Accumulate IDs into locals so the `sg` borrow (from
        // `self.group`) does not overlap with writing `self.label_ids`/`self.value_ids`.
        let mut new_lids: Vec<tv::ViewId> = Vec::with_capacity(fields.len());
        let mut new_vids: Vec<tv::ViewId> = Vec::with_capacity(fields.len());
        {
            let Some(sg) = self.scroll_mut() else { return };
            sg.clear_content(ctx);
            let w = sg.inner_width();
            for (row, (_label, editable)) in fields.iter().enumerate() {
                let y = row as i32;
                let lid = sg.add_content(
                    Box::new(ro_cell(Rect::new(0, y, LABEL_W, y + 1))),
                    Rect::new(0, y, LABEL_W, y + 1),
                );
                let mut il = InputLine::with_limit(Rect::new(LABEL_W, y, w, y + 1), 1024);
                il.state.state.disabled = !editable;
                let vid = sg.add_content(Box::new(il), Rect::new(LABEL_W, y, w, y + 1));
                new_lids.push(lid);
                new_vids.push(vid);
            }
        } // sg borrow released; self is free again
        self.label_ids = new_lids;
        self.value_ids = new_vids;
    }

    /// Repaint header + all cell texts from `edit_form`; rebuild cells first if
    /// the shown entry changed (different `dn`).
    fn render(&mut self, ctx: &mut Context) {
        let cur_dn = self.state.borrow().edit_form.as_ref().map(|f| f.dn.clone());
        if cur_dn != self.built_dn {
            self.rebuild_cells(ctx);
            self.built_dn = cur_dn;
        }

        // Collect display data (drop state borrow before touching views).
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
                            (
                                format!("{}{}", f.label, marker),
                                present_field(f),
                                cell_focusable(f),
                            )
                        })
                        .collect();
                    (header, rows)
                }
            }
        }; // state borrow dropped

        if let Some(h) = self.group.child_mut(self.header_id) {
            h.set_value(FieldValue::Text(header));
        }

        // Update each cell via the scroll group. Clone IDs before borrowing sg.
        let (label_ids, value_ids) = (self.label_ids.clone(), self.value_ids.clone());
        {
            let Some(sg) = self.scroll_mut() else {
                return;
            };
            for (i, (label, value, editable)) in rows.iter().enumerate() {
                if let (Some(&lid), Some(&vid)) = (label_ids.get(i), value_ids.get(i)) {
                    if let Some(l) = sg.child_mut(lid) {
                        l.set_value(FieldValue::Text(label.clone()));
                    }
                    if let Some(v) = sg.child_mut(vid) {
                        v.set_value(FieldValue::Text(value.clone()));
                        v.state_mut().state.disabled = !editable;
                    }
                }
            }
        } // sg borrow dropped

        // Land focus on the first editable field. Tab switches panes; Up/Down
        // move between fields. Also ensures the outer Group routes events to the
        // ScrollGroup.
        if let Some(first) = self.focusable_value_ids().first().copied() {
            let scroll_id = self.scroll_id;
            self.group.focus_child(scroll_id, ctx);
            if let Some(sg) = self.scroll_mut() {
                sg.focus_child(first, ctx);
            }
        }
    }

    /// The value-cell view ids of focusable rows (inline-editable OR modal), in
    /// display order. Full-length — no `FORM_ROWS` cap.
    fn focusable_value_ids(&self) -> Vec<tv::ViewId> {
        let st = self.state.borrow();
        match st.edit_form.as_ref() {
            None => Vec::new(),
            Some(form) => form
                .fields
                .iter()
                .enumerate()
                .filter(|(_, f)| cell_focusable(f))
                .map(|(i, _)| self.value_ids[i])
                .collect(),
        }
    }

    /// Move focus to the prev/next focusable field, wrapping. Focuses the first
    /// focusable when none is currently focused.
    fn focus_field(&mut self, delta: i32, ctx: &mut Context) {
        let ids = self.focusable_value_ids();
        if ids.is_empty() {
            return;
        }
        // Current focused field lives inside the ScrollGroup.
        let cur = self.scroll_mut().and_then(|sg| sg.current());
        let pos = cur.and_then(|c| ids.iter().position(|id| *id == c));
        let next = match pos {
            Some(p) => (p as i32 + delta).rem_euclid(ids.len() as i32) as usize,
            None if delta >= 0 => 0,
            None => ids.len() - 1,
        };
        let next_id = ids[next];
        if let Some(sg) = self.scroll_mut() {
            sg.focus_child(next_id, ctx);
        }
    }

    /// The field index whose value cell currently holds focus, if any.
    fn focused_field_idx(&mut self) -> Option<usize> {
        let cur = self.scroll_mut().and_then(|sg| sg.current())?;
        self.value_ids.iter().position(|id| *id == cur)
    }

    /// Whether the focused field opens a modal editor (objectClass).
    fn focused_is_modal(&mut self) -> bool {
        let Some(idx) = self.focused_field_idx() else {
            return false;
        };
        let st = self.state.borrow();
        st.edit_form
            .as_ref()
            .and_then(|f| f.fields.get(idx))
            .map(is_modal_field)
            .unwrap_or(false)
    }

    /// Sync each editable value InputLine's text into `edit_form`; refresh header.
    /// Borrow discipline: collect indices (drop state borrow), read from scroll group
    /// (drop sg borrow), then mutate state (drop before touching group header).
    fn sync_into_form(&mut self) {
        // Collect editable field indices; drop borrow before accessing views.
        let editable: Vec<usize> = {
            let st = self.state.borrow();
            match st.edit_form.as_ref() {
                None => Vec::new(),
                Some(form) => form
                    .fields
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| inline_editable(f))
                    .map(|(i, _)| i)
                    .collect(),
            }
        }; // state borrow dropped

        // Read current InputLine texts from the ScrollGroup.
        let value_ids = self.value_ids.clone();
        let mut edits: Vec<(usize, String)> = Vec::new();
        if let Some(sg) = self.scroll_mut() {
            for &i in &editable {
                if let Some(&vid) = value_ids.get(i) {
                    if let Some(FieldValue::Text(s)) = sg.child_mut(vid).and_then(|v| v.value()) {
                        edits.push((i, s));
                    }
                }
            }
        } // sg borrow dropped

        // Write edits back into the form; compute the new header text.
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
        }; // borrow_mut dropped

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

        // Enter on a modal row (objectClass) opens its editor via the controller:
        // record the field index, post ACTIVATE (capture-free), consume the key.
        let enter = matches!(ev, Event::KeyDown(k) if k.key == Key::Enter);
        if enter && self.focused_is_modal() {
            if let Some(idx) = self.focused_field_idx() {
                self.state.borrow_mut().activate_field = Some(idx);
                ctx.post(ACTIVATE);
            }
            ev.clear();
            return;
        }
        // Swallow text edits on a modal row: its value comes from the picker, not
        // typing. (The cell is enabled only so it can take focus + Enter.)
        let edit_key = matches!(
            ev,
            Event::KeyDown(k) if matches!(k.key, Key::Char(_) | Key::Backspace | Key::Delete)
        );
        if edit_key && self.focused_is_modal() {
            ev.clear();
            return;
        }

        // Up/Down move focus between focusable fields (Tab is reserved for switching
        // panes, consumed by the Splitter). Consume the key so it stays in this pane.
        let nav = matches!(ev, Event::KeyDown(k) if matches!(k.key, Key::Up | Key::Down));
        if nav {
            let down = matches!(ev, Event::KeyDown(k) if k.key == Key::Down);
            self.focus_field(if down { 1 } else { -1 }, ctx);
            ev.clear();
        } else {
            self.group.handle_event(ev, ctx);
        }
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

    /// Build a FormPane over a Shared state seeded with the given fields.
    /// Returns `(shared, pane)`. The caller creates its own headless context.
    fn build_pane_with_form(fields: Vec<EditField>) -> (Shared, FormPane) {
        use crate::ldap::worker::RawSubschema;
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let structure = Structure::build("dc=x", vec![]);
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.edit_form = Some(EditForm {
            dn: "cn=test,dc=x".into(),
            mode: FormMode::Edit,
            object_classes: vec![],
            fields,
        });
        st.form_needs_render = true;
        let shared: Shared = Rc::new(RefCell::new(st));
        let pane = FormPane::new(Rect::new(0, 0, 80, 20), shared.clone());
        (shared, pane)
    }

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

    /// Regression: children must grow via grow_mode when the Splitter drives
    /// Group::change_bounds — NOT via an on_bounds_changed override (which the
    /// framework never calls for Splitter-nested panes).
    ///
    /// TDD evidence: before grow_mode flags were set (hi_x on header, hi_x+hi_y
    /// on scroll), this test FAILED — children kept their original Rect. After
    /// setting the flags, Group::change_bounds propagates the delta and this PASSES.
    #[test]
    fn grow_mode_resize_fills_pane() {
        let shared = state_with_form();
        let mut pane = FormPane::new(Rect::new(0, 0, 40, 6), shared);
        // Simulate Splitter driving a resize: just change_bounds, no on_bounds_changed.
        <FormPane as View>::change_bounds(&mut pane, Rect::new(0, 0, 80, 20));
        // Header spans the new full width; scroll child fills rows 1..20.
        assert_eq!(
            pane.header_bounds_for_test().b.x,
            80,
            "header must widen to new width (hi_x)"
        );
        assert_eq!(
            pane.scroll_bounds_for_test(),
            Rect::new(0, 1, 80, 20),
            "scroll group must fill remaining width+height (hi_x+hi_y)"
        );
    }

    #[test]
    fn updown_cycles_focus_among_editable_fields() {
        // Tab switches panes (consumed by the Splitter), so intra-pane field
        // navigation uses Up/Down. Render focuses the first editable field; Up/Down
        // cycle among editable rows, skipping read-only ones.
        use crate::ldap::worker::RawSubschema;
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let structure = Structure::build("dc=x", vec![]);
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        let mut cn = ef("cn", "a", true);
        cn.multi = true; // multi-valued → not inline-editable
        st.edit_form = Some(EditForm {
            dn: "cn=a,dc=x".into(),
            mode: FormMode::Edit,
            object_classes: vec![],
            fields: vec![cn, ef("gidNumber", "1001", true), ef("sn", "Bar", true)],
        });
        st.form_needs_render = true;
        let shared: Shared = Rc::new(RefCell::new(st));
        // Use a concrete height (no longer references the removed FORM_ROWS constant).
        let mut pane = FormPane::new(Rect::new(0, 0, 80, 20), shared.clone());
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut ev, &mut ctx); // render + focus first editable

        let focusable = pane.focusable_value_ids();
        assert_eq!(
            focusable.len(),
            2,
            "cn (multi, non-modal) is not focusable; gidNumber+sn are"
        );
        // Focus lives inside the ScrollGroup; query it via scroll_mut().
        let cur = pane.scroll_mut().and_then(|sg| sg.current());
        assert_eq!(
            cur,
            Some(focusable[0]),
            "render focuses the first focusable field"
        );

        let mut d = Event::KeyDown(tv::KeyEvent::from(tv::Key::Down));
        pane.handle_event(&mut d, &mut ctx);
        let cur = pane.scroll_mut().and_then(|sg| sg.current());
        assert_eq!(cur, Some(focusable[1]), "Down → next focusable field");

        let mut d = Event::KeyDown(tv::KeyEvent::from(tv::Key::Down));
        pane.handle_event(&mut d, &mut ctx);
        let cur = pane.scroll_mut().and_then(|sg| sg.current());
        assert_eq!(
            cur,
            Some(focusable[0]),
            "Down wraps to the first focusable field"
        );

        let mut u = Event::KeyDown(tv::KeyEvent::from(tv::Key::Up));
        pane.handle_event(&mut u, &mut ctx);
        let cur = pane.scroll_mut().and_then(|sg| sg.current());
        assert_eq!(cur, Some(focusable[1]), "Up → previous focusable field");
    }

    #[test]
    fn builds_a_cell_per_field_no_row_cap() {
        // Each field must get its own value cell; there must be no fixed row cap.
        // Regression guard: old code capped at FORM_ROWS=32 and panicked beyond that.
        use crate::ldap::worker::RawSubschema;
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let structure = Structure::build("dc=x", vec![]);
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        let fields: Vec<EditField> = (0..40)
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
        let mut pane = FormPane::new(Rect::new(0, 0, 80, 12), shared.clone()); // small viewport
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut ev, &mut ctx); // must not panic; builds 40 rows
        assert_eq!(
            pane.field_cell_count(),
            40,
            "one value cell per field, uncapped"
        );
    }

    #[test]
    fn editable_rows_enabled_static_rows_disabled() {
        let shared = state_with_form();
        let mut pane = FormPane::new(Rect::new(0, 0, 80, 20), shared.clone());
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
        let mut pane = FormPane::new(Rect::new(0, 0, 80, 20), shared.clone());
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
    fn enter_on_objectclass_row_posts_activate() {
        // TDD for Task 6: objectClass row must be focusable; Enter on it sets
        // activate_field and posts ACTIVATE.
        let (shared, mut pane) = build_pane_with_form(vec![
            EditField {
                label: "cn".into(),
                must: true,
                editable: true,
                multi: false,
                secret: false,
                ordered: false,
                orphaned: false,
                kind: FieldKind::Text,
                widget: WidgetSpec::ReadOnlyText,
                widget_binding: None,
                values: vec!["Bob".into()],
                baseline: vec!["Bob".into()],
            },
            EditField {
                label: "objectClass".into(),
                must: true,
                editable: false,
                multi: true,
                secret: false,
                ordered: false,
                orphaned: false,
                kind: FieldKind::Text,
                widget: WidgetSpec::ReadOnlyText,
                widget_binding: None,
                values: vec!["top".into(), "person".into()],
                baseline: vec!["top".into(), "person".into()],
            },
        ]);
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();

        // Initial render + focus first focusable (cn).
        let mut tick = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(
            &mut tick,
            &mut headless_ctx(&mut out, &mut timers, &mut deferred),
        );

        // Move focus down to the objectClass row.
        let mut down = Event::KeyDown(tv::KeyEvent::from(tv::Key::Down));
        pane.handle_event(
            &mut down,
            &mut headless_ctx(&mut out, &mut timers, &mut deferred),
        );

        // Press Enter — must record activate_field = Some(1) and post ACTIVATE.
        let mut enter = Event::KeyDown(tv::KeyEvent::from(tv::Key::Enter));
        pane.handle_event(
            &mut enter,
            &mut headless_ctx(&mut out, &mut timers, &mut deferred),
        );

        assert_eq!(shared.borrow().activate_field, Some(1));
        // T6: ACTIVATE command must also be posted into the event queue so the
        // controller (app::dispatch) can open the modal editor.
        assert!(
            out.iter()
                .any(|e| matches!(e, Event::Command(cmd) if *cmd == ACTIVATE)),
            "ACTIVATE command must be posted to the event queue after Enter on objectClass"
        );
    }

    #[test]
    fn umlaut_edit_roundtrips_graphemes() {
        // Grapheme-correct edit regression (folded from the spike umlaut test):
        // a multibyte value set into the InputLine survives the sync into edit_form.
        let shared = state_with_form();
        let mut pane = FormPane::new(Rect::new(0, 0, 80, 20), shared.clone());
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
        // Trigger sync with a non-editing event (a live keystroke would land in the
        // now-focused field and replace it — that is correct edit behaviour, not what
        // this grapheme regression checks).
        let mut ev2 = Event::Broadcast {
            command: tv::Command::custom("test.noop"),
            source: None,
        };
        pane.handle_event(&mut ev2, &mut ctx);
        let st = shared.borrow();
        assert_eq!(
            st.edit_form.as_ref().unwrap().fields[0].values,
            vec!["Müller-Lüdenscheidt".to_string()]
        );
    }
}
