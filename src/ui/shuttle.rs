//! `Shuttle` — a domain-free two-list "transfer" widget.
//!
//! The clean re-incubation of `dual_list.rs`: instead of a controller that
//! reaches into the host's `Dialog`, the `Shuttle` *is* a `View` embedding a
//! `Group` and owning its children (two columns + Add/Remove buttons),
//! notifying the owner by broadcast (`CMD_SHUTTLE_CHANGED`) rather than a
//! bespoke return-value channel. Incremental search is each list's own built-in
//! find mode (`FindMode`): typing while a list is focused narrows/highlights it
//! and it broadcasts `Command::LIST_FIND_CHANGED`. The Available list's mode is
//! the consumer's choice (`Highlight` for a server-backed host, which reads
//! [`Shuttle::find_query`] to re-run its query, or `Filter` for a local set); the
//! Selected list always uses `Filter` (a local narrow) so both columns search the
//! same way and letters never leak to the Add/Remove buttons. The pure column
//! logic — move / de-dup / lock — lives in [`ShuttleModel`], which is
//! tvision-free and unit-testable without a `Dialog`.

use tvision_rs::{
    self as tv, delegate, Button, ButtonFlags, Command, Context, Event, FieldValue, FindMode,
    Group, Key, Label, ListBox, ListViewer, Rect, ScrollBar, SortedListBox, View, ViewId,
};

/// Broadcast (with the Shuttle's own `ViewId` as `source`) when the Selected set
/// changes via a move. The owner re-reads [`Shuttle::selected`] and reacts.
pub(crate) const CMD_SHUTTLE_CHANGED: Command = Command::custom("shuttle.changed");

/// Internal: the on-screen [Add] button posts this; handled exactly like Insert.
const CMD_ADD: Command = Command::custom("shuttle.add");
/// Internal: the on-screen [Remove] button posts this; handled like Delete.
const CMD_REMOVE: Command = Command::custom("shuttle.remove");

/// Marker prefix for a Selected row that may **not** be removed (locked).
const MARK_LOCKED: &str = "* ";
/// Marker prefix for a plain row — keeps Selected rows aligned with locked ones.
const MARK_PLAIN: &str = "  ";

/// One row in either column. Domain-free: `key` is the host's stable identity
/// (a DN, an object-class name, …) used for de-duplication; `label` is the
/// display text; `locked` blocks moving the row out of the Selected column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShuttleRow {
    pub key: String,
    pub label: String,
    pub locked: bool,
}

/// The pure, tvision-free model of the two columns. The `Shuttle` `View` will
/// wrap one of these plus the child views; all move/de-dup/lock logic lives
/// here so it is exercised without a `Dialog`.
#[derive(Default)]
pub(crate) struct ShuttleModel {
    available: Vec<ShuttleRow>,
    selected: Vec<ShuttleRow>,
}

impl ShuttleModel {
    fn available(&self) -> &[ShuttleRow] {
        &self.available
    }

    fn selected(&self) -> &[ShuttleRow] {
        &self.selected
    }

    fn set_available(&mut self, rows: Vec<ShuttleRow>) {
        self.available = rows;
    }

    fn set_selected(&mut self, rows: Vec<ShuttleRow>) {
        self.selected = rows;
    }

    /// Move `available[idx]` into Selected. No-op (returns `false`) when `idx`
    /// is out of range or the key is already selected (case-insensitive).
    /// Returns `true` when the Selected set changed.
    fn move_in(&mut self, idx: usize) -> bool {
        let Some(row) = self.available.get(idx) else {
            return false;
        };
        if self.is_selected(&row.key) {
            return false;
        }
        let row = self.available[idx].clone();
        self.selected.push(row);
        true
    }

    /// Whether `key` is already in the Selected set (case-insensitive).
    fn is_selected(&self, key: &str) -> bool {
        self.selected
            .iter()
            .any(|r| r.key.eq_ignore_ascii_case(key))
    }

    /// Remove `selected[idx]` from Selected. No-op (returns `false`) when `idx`
    /// is out of range or the row is `locked`. Returns `true` when the Selected
    /// set changed.
    fn move_out(&mut self, idx: usize) -> bool {
        let Some(row) = self.selected.get(idx) else {
            return false;
        };
        if row.locked {
            return false;
        }
        self.selected.remove(idx);
        true
    }
}

/// The rect of every `Shuttle` child, computed from the widget area by
/// [`Shuttle::layout`]. Kept as a plain record so `new` and `change_bounds`
/// derive identical geometry.
struct ShuttleLayout {
    left_header: Rect,
    right_header: Rect,
    avail_list: Rect,
    avail_bar: Rect,
    sel_list: Rect,
    sel_bar: Rect,
    add_btn: Rect,
    remove_btn: Rect,
}

/// The two-list transfer widget. Embeds a `Group` that owns the column lists,
/// their scroll bars and the Add/Remove buttons; the
/// move logic lives in [`ShuttleModel`]. A `View` in its own right (the impl is
/// delegated to the group, with only `handle_event`, `as_any_mut` and the
/// gather/scatter methods hand-written) rather than a controller poking a host's
/// `Dialog`.
pub(crate) struct Shuttle {
    group: Group,
    model: ShuttleModel,
    avail_id: ViewId,
    selected_id: ViewId,
    // Stored for change_bounds geometry reflow (Task 4); not yet read.
    #[allow(dead_code)]
    left_header_id: ViewId,
    #[allow(dead_code)]
    right_header_id: ViewId,
    #[allow(dead_code)]
    avail_bar_id: ViewId,
    #[allow(dead_code)]
    sel_bar_id: ViewId,
    // Used in tests and will be used in change_bounds (Task 4).
    #[allow(dead_code)]
    add_id: ViewId,
    #[allow(dead_code)]
    remove_id: ViewId,
}

impl Shuttle {
    /// Minimum interior the two columns + button rows need before they overlap.
    const MIN_W: i32 = 60;
    const MIN_H: i32 = 20;

    /// Every child rect derived purely from the widget's `area`. Extracted from
    /// `new` so a resize (`change_bounds`) can recompute the same geometry.
    fn layout(area: Rect) -> ShuttleLayout {
        // Clamp the working extent so a too-small area never yields overlapping
        // or inverted rects (the window's drag limit is the first defence; this
        // is the backstop).
        let x0 = area.a.x;
        let y0 = area.a.y;
        let x1 = x0 + (area.b.x - x0).max(Self::MIN_W);
        let y1 = y0 + (area.b.y - y0).max(Self::MIN_H);

        let mid = (x0 + x1) / 2;
        let left = (x0 + 2, mid - 2);
        let right = (mid + 2, x1 - 2);
        let head_y = y0 + 1;
        // Reserve the bottom: OK/Cancel land at y1-3 (dialog button_row); the
        // wide Add/Remove row sits two rows above at y1-6..y1-4, so the lists end
        // at y1-7.
        let list_top = y0 + 2;
        let list_bot = y1 - 7;
        let btn_top = y1 - 6;
        let btn_bot = y1 - 4;

        ShuttleLayout {
            left_header: Rect::new(left.0, head_y, left.1, head_y + 1),
            right_header: Rect::new(right.0, head_y, right.1, head_y + 1),
            avail_list: Rect::new(left.0, list_top, left.1 - 1, list_bot),
            avail_bar: Rect::new(left.1 - 1, list_top, left.1, list_bot),
            sel_list: Rect::new(right.0, list_top, right.1 - 1, list_bot),
            sel_bar: Rect::new(right.1 - 1, list_top, right.1, list_bot),
            add_btn: Rect::new(left.0, btn_top, left.1, btn_bot),
            remove_btn: Rect::new(right.0, btn_top, right.1, btn_bot),
        }
    }

    /// Build the two columns (each a list + a right-lane scroll bar) inside an
    /// owned `Group`. `find_mode` enables the Available list's built-in
    /// incremental search ([`FindMode::Off`] for none). The Available column is
    /// always rendered on the LEFT, the Selected column on the RIGHT (the
    /// conventional transfer-widget layout). Geometry: headers at row 1, lists at
    /// rows 2..(height-7), 2-cell margins and a 4-cell gutter; a wide Add/Remove
    /// button row sits at height-6..height-4.
    pub(crate) fn new(
        area: Rect,
        left_title: &str,
        right_title: &str,
        find_mode: FindMode,
    ) -> Shuttle {
        let l = Self::layout(area);
        let mut group = Group::new(area);
        // Fill the owner on resize: the dialog's change_bounds cascade resizes
        // this widget, and our own change_bounds reflows the children.
        group.state_mut().grow_mode.hi_x = true;
        group.state_mut().grow_mode.hi_y = true;

        let left_header_id = group.insert(Box::new(Label::new(l.left_header, left_title, None)));
        let right_header_id = group.insert(Box::new(Label::new(l.right_header, right_title, None)));

        // Available column (left): SortedListBox + scroll bar.
        let avail_bar_id = group.insert(Box::new(ScrollBar::new(l.avail_bar)));
        let avail_id = group.insert(Box::new(
            SortedListBox::new(l.avail_list, 1, None, Some(avail_bar_id)).with_find(find_mode),
        ));

        // Selected column (right): plain ListBox (insertion order) + scroll bar.
        // FindMode::Filter narrows the local staged set as the user types (so
        // letters never leak to the Add/Remove hotkeys).
        let sel_bar_id = group.insert(Box::new(ScrollBar::new(l.sel_bar)));
        let selected_id = group.insert(Box::new(
            ListBox::new(l.sel_list, 1, None, Some(sel_bar_id)).with_find(FindMode::Filter),
        ));

        // Wide move buttons, each spanning the column it acts on: Add under the
        // Available (left) column, Remove under the Selected (right). Both are
        // marked non-selectable so Tab skips them (they stay operable by click,
        // Alt-A / Alt-R, and Insert/Delete/Enter on the focused list). Non-
        // selectable does not disable pre/post-process, so the Alt hotkey still
        // fires.
        let mut add = Button::new(l.add_btn, "~A~dd", CMD_ADD, ButtonFlags::new());
        add.state_mut().options.selectable = false;
        let add_id = group.insert(Box::new(add));

        let mut remove = Button::new(l.remove_btn, "~R~emove", CMD_REMOVE, ButtonFlags::new());
        remove.state_mut().options.selectable = false;
        let remove_id = group.insert(Box::new(remove));

        Shuttle {
            group,
            model: ShuttleModel::default(),
            avail_id,
            selected_id,
            left_header_id,
            right_header_id,
            avail_bar_id,
            sel_bar_id,
            add_id,
            remove_id,
        }
    }

    /// The Available list's current incremental-find query (empty when find mode
    /// is off or nothing has been typed). A server-backed host reads this after a
    /// `Command::LIST_FIND_CHANGED` broadcast to re-run its candidate query.
    pub(crate) fn find_query(&mut self) -> String {
        self.group
            .child_mut(self.avail_id)
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<SortedListBox>())
            .and_then(|lb| lb.find_query().map(str::to_string))
            .unwrap_or_default()
    }

    /// The current Selected set — the owner reads this after a
    /// `CMD_SHUTTLE_CHANGED` broadcast to stage its commit.
    pub(crate) fn selected(&self) -> &[ShuttleRow] {
        self.model.selected()
    }

    /// The `ViewId` of the Available list. Both lists have their own find mode and
    /// each broadcasts `Command::LIST_FIND_CHANGED` with its own id as `source`; a
    /// server-backed owner filters on this so only the Available list's find drives
    /// a re-query (the Selected list's find is a purely local highlight).
    pub(crate) fn available_id(&self) -> ViewId {
        self.avail_id
    }

    /// The list of the column whose horizontal extent contains the Shuttle-local
    /// `x`, or `None` when `x` is over neither list. A wheel scroll is routed here
    /// so the column *under the cursor* scrolls: `ScrollBar` consumes `MouseWheel`
    /// non-positionally (any visible bar grabs it) and the group offers a fixed bar
    /// first, so without explicit routing one column would scroll regardless of the
    /// cursor (the reported "always the left panel" bug).
    fn wheel_target_list(&mut self, x: i32) -> Option<ViewId> {
        if self.x_in_child(self.avail_id, x) {
            Some(self.avail_id)
        } else if self.x_in_child(self.selected_id, x) {
            Some(self.selected_id)
        } else {
            None
        }
    }

    /// Whether `x` (Shuttle-local) falls within child `id`'s horizontal extent.
    /// `get_bounds` is owner-relative — the same frame a delivered mouse event's
    /// position carries here — so the comparison holds wherever the host places
    /// the Shuttle.
    fn x_in_child(&mut self, id: ViewId, x: i32) -> bool {
        match self.group.child_mut(id) {
            Some(c) => {
                let b = c.state().get_bounds();
                x >= b.a.x && x < b.b.x
            }
            None => false,
        }
    }

    /// Scroll list `id` by `delta` rows by nudging its focused item (clamped at the
    /// top; the list clamps the bottom). This moves the highlight *and* scrolls the
    /// viewport via the list's own `ensure_visible`, so it works regardless of which
    /// child holds focus or whether the column's scroll bar is currently visible.
    fn scroll_list(&mut self, id: ViewId, delta: i32, ctx: &mut Context) {
        if let Some(c) = self.group.child_mut(id) {
            let cur = match c.value() {
                Some(FieldValue::Int(i)) => i,
                _ => 0,
            };
            let next = (cur + delta).max(0);
            c.set_value_ctx(FieldValue::Int(next), ctx);
        }
    }

    /// Replace the Available rows and re-render the Available column.
    pub(crate) fn set_available(&mut self, rows: Vec<ShuttleRow>, ctx: &mut Context) {
        self.model.set_available(rows);
        self.rebuild_avail(ctx);
    }

    /// Re-render the Available column from the model (plain labels — Available
    /// rows carry no marker; the consumers filter already-selected rows out).
    fn rebuild_avail(&mut self, ctx: &mut Context) {
        let rows: Vec<String> = self
            .model
            .available()
            .iter()
            .map(|r| r.label.clone())
            .collect();
        if let Some(lb) = self
            .group
            .child_mut(self.avail_id)
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<SortedListBox>())
        {
            lb.new_list(rows, ctx);
        }
    }

    /// Replace the Selected rows and re-render the Selected column.
    pub(crate) fn set_selected(&mut self, rows: Vec<ShuttleRow>, ctx: &mut Context) {
        self.model.set_selected(rows);
        self.rebuild_selected(ctx);
    }

    /// Re-render the Selected column from the model (lock marker + label).
    fn rebuild_selected(&mut self, ctx: &mut Context) {
        let rows: Vec<String> = self
            .model
            .selected()
            .iter()
            .map(Self::selected_display)
            .collect();
        if let Some(lb) = self
            .group
            .child_mut(self.selected_id)
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<ListBox>())
        {
            lb.new_list(rows, ctx);
        }
    }

    /// Move the highlighted Available row into Selected, re-rendering both
    /// columns on success. Returns whether the Selected set changed.
    fn move_in_highlighted(&mut self, ctx: &mut Context) -> bool {
        let Some(idx) = self.avail_highlight() else {
            return false;
        };
        if self.model.move_in(idx) {
            self.rebuild_avail(ctx);
            self.rebuild_selected(ctx);
            true
        } else {
            false
        }
    }

    /// Move the highlighted Selected row out, re-rendering both columns on
    /// success. Returns whether the Selected set changed (a locked row is
    /// rejected by the model and reports `false`).
    fn move_out_highlighted(&mut self, ctx: &mut Context) -> bool {
        let Some(idx) = self.selected_highlight() else {
            return false;
        };
        if self.model.move_out(idx) {
            self.rebuild_avail(ctx);
            self.rebuild_selected(ctx);
            true
        } else {
            false
        }
    }

    /// Index into the model's Selected rows of the highlighted row, resolved by
    /// matching the focused display row's label back to the model. The Selected
    /// list is unsorted but its `FindMode::Filter` can narrow the display, so the
    /// focused index is a *display* index, not the model index — map it by label
    /// (mirroring [`avail_highlight`]).
    fn selected_highlight(&mut self) -> Option<usize> {
        let label = self.selected_focused_label()?;
        self.model
            .selected()
            .iter()
            .position(|r| r.label.eq_ignore_ascii_case(&label))
    }

    /// The display label currently focused in the Selected `ListBox`, with the
    /// 2-char lock/plain marker that [`selected_display`] prepends stripped off.
    fn selected_focused_label(&mut self) -> Option<String> {
        let lb = self
            .group
            .child_mut(self.selected_id)?
            .as_any_mut()?
            .downcast_mut::<ListBox>()?;
        let idx = match lb.value() {
            Some(FieldValue::Int(i)) if i >= 0 => i as usize,
            _ => return None,
        };
        let disp = lb.list().get(idx).cloned()?;
        Some(disp.chars().skip(2).collect())
    }

    /// Index into the model's Available rows of the highlighted row, resolved by
    /// matching the `SortedListBox`'s focused display label back to the model
    /// (the list is sorted, so its index is in display order).
    fn avail_highlight(&mut self) -> Option<usize> {
        let label = self.avail_focused_label()?;
        self.model
            .available()
            .iter()
            .position(|r| r.label.eq_ignore_ascii_case(&label))
    }

    /// The display label currently focused in the Available `SortedListBox`.
    fn avail_focused_label(&mut self) -> Option<String> {
        let lb = self
            .group
            .child_mut(self.avail_id)?
            .as_any_mut()?
            .downcast_mut::<SortedListBox>()?;
        let idx = match lb.value() {
            Some(FieldValue::Int(i)) if i >= 0 => i as usize,
            _ => return None,
        };
        lb.list().get(idx).cloned()
    }

    /// The exact display string a Selected row renders to (lock marker + label).
    fn selected_display(r: &ShuttleRow) -> String {
        let mark = if r.locked { MARK_LOCKED } else { MARK_PLAIN };
        format!("{mark}{}", r.label)
    }
}

#[delegate(to = group, skip(handle_event, as_any_mut, reset_current, value, set_value, set_value_ctx))]
impl View for Shuttle {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    /// Open-time focus inside the Shuttle: the Available list, so type-to-search
    /// / incremental find works immediately. The host embeds the Shuttle and
    /// focuses the Shuttle itself (so the dialog routes events here); this decides
    /// which child lands focus when that happens. Runs before the first draw.
    fn reset_current(&mut self, ctx: &mut Context) {
        self.group.reset_current(ctx);
        self.group.focus_child(self.avail_id, ctx);
    }

    /// Gather: the Selected set as a list of keys, so the Shuttle participates
    /// in a Dialog's data exchange like any other field.
    fn value(&self) -> Option<FieldValue> {
        Some(FieldValue::List(
            self.model
                .selected()
                .iter()
                .map(|r| FieldValue::Text(r.key.clone()))
                .collect(),
        ))
    }

    /// Scatter (no `Context`): seed the Selected set from a list of keys. Rows
    /// are reconstructed with `label == key` and unlocked — the lossy generic
    /// path; rich seeding (labels, locks) uses [`Shuttle::set_selected`].
    fn set_value(&mut self, v: FieldValue) {
        if let FieldValue::List(items) = v {
            let rows = items
                .into_iter()
                .filter_map(|fv| match fv {
                    FieldValue::Text(s) => Some(ShuttleRow {
                        key: s.clone(),
                        label: s,
                        locked: false,
                    }),
                    _ => None,
                })
                .collect();
            self.model.set_selected(rows);
        }
    }

    /// Scatter with rendering: seed the model, then re-render the Selected list.
    fn set_value_ctx(&mut self, v: FieldValue, ctx: &mut Context) {
        self.set_value(v);
        self.rebuild_selected(ctx);
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        // Mouse wheel: scroll the column the cursor is OVER. `ScrollBar` consumes
        // the wheel non-positionally (any visible bar grabs it) and the group offers
        // a fixed bar first, so without routing here one column would scroll
        // regardless of the cursor. Scroll the hovered column's list directly (if
        // any) and always consume, so the group's non-positional default never fires.
        if matches!(ev, Event::MouseWheel(_)) {
            let (x, delta) = match &*ev {
                Event::MouseWheel(me) => (
                    me.position.x,
                    match me.wheel {
                        tv::event::MouseWheel::Down => 3,
                        tv::event::MouseWheel::Up => -3,
                        _ => 0,
                    },
                ),
                _ => (0, 0),
            };
            if delta != 0 {
                if let Some(list) = self.wheel_target_list(x) {
                    self.scroll_list(list, delta, ctx);
                }
            }
            ev.clear();
            return;
        }

        // Insert moves the highlighted Available row toward Selected. Everything
        // else (Tab/arrows/typing) delegates to the group so focus and list
        // navigation work the standard way.
        // Enter is a move only while a list holds focus (Available → move in,
        // Selected → move out). Enter elsewhere (the buttons) passes through so
        // the dialog's default OK still fires.
        let enter = matches!(ev, Event::KeyDown(k) if k.key == Key::Enter);
        let enter_in = enter && self.group.current() == Some(self.avail_id);
        let enter_out = enter && self.group.current() == Some(self.selected_id);

        let move_in = enter_in
            || matches!(ev, Event::KeyDown(k) if k.key == Key::Insert)
            || matches!(ev, Event::Command(c) if *c == CMD_ADD);
        let move_out = enter_out
            || matches!(ev, Event::KeyDown(k) if k.key == Key::Delete)
            || matches!(ev, Event::Command(c) if *c == CMD_REMOVE);
        if move_in {
            if self.move_in_highlighted(ctx) {
                ctx.broadcast(CMD_SHUTTLE_CHANGED, self.state().id());
            }
            ev.clear();
            return;
        }
        if move_out {
            if self.move_out_highlighted(ctx) {
                ctx.broadcast(CMD_SHUTTLE_CHANGED, self.state().id());
            }
            ev.clear();
            return;
        }
        self.group.handle_event(ev, ctx);
        // Incremental search is the Available list's own find mode: when focused
        // it consumes query keys and broadcasts `Command::LIST_FIND_CHANGED`
        // itself, so the Shuttle has nothing to report here.
    }
}

#[cfg(test)]
impl Shuttle {
    /// The Available `SortedListBox`'s `ViewId` — lets a host's tests drive the
    /// real column (set a highlight, dispatch a key) through the embedded widget.
    pub(crate) fn avail_id_for_test(&self) -> ViewId {
        self.avail_id
    }

    /// The Selected `ListBox`'s `ViewId` (see [`Shuttle::avail_id_for_test`]).
    pub(crate) fn selected_id_for_test(&self) -> ViewId {
        self.selected_id
    }

    /// The display strings currently in list `id` (empty if it does not resolve
    /// to a `ListBox`).
    fn list_text(&mut self, id: ViewId) -> Vec<String> {
        self.group
            .child_mut(id)
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<ListBox>())
            .map(|lb| lb.list().to_vec())
            .unwrap_or_default()
    }

    pub(crate) fn selected_text(&mut self) -> Vec<String> {
        self.list_text(self.selected_id)
    }

    /// The display strings in the Available column (a `SortedListBox`).
    pub(crate) fn avail_text(&mut self) -> Vec<String> {
        self.group
            .child_mut(self.avail_id)
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<SortedListBox>())
            .map(|lb| lb.list().to_vec())
            .unwrap_or_default()
    }

    /// Focus the child `id` within the embedded group (mirrors the dialog's
    /// open-time focus cascade, which a headless host test cannot drive).
    pub(crate) fn focus_for_test(&mut self, id: ViewId, ctx: &mut Context) {
        self.group.focus_child(id, ctx);
    }

    /// Set the highlighted (focused) row of list `id` to display index `idx`.
    pub(crate) fn highlight(&mut self, id: ViewId, idx: i32, ctx: &mut Context) {
        if let Some(c) = self.group.child_mut(id) {
            c.set_value_ctx(FieldValue::Int(idx), ctx);
        }
    }

    /// Simulate the user typing `text` into the Available list's find box: focus
    /// the list, then dispatch one `KeyDown(Char)` per character so the list's
    /// own find state machine accumulates the query.
    pub(crate) fn type_find(&mut self, text: &str, ctx: &mut Context) {
        self.group.focus_child(self.avail_id, ctx);
        for ch in text.chars() {
            let mut ev = Event::KeyDown(tv::KeyEvent::from(Key::Char(ch)));
            self.handle_event(&mut ev, ctx);
        }
    }

    /// Like [`type_find`], but focuses and types into the *Selected* list.
    pub(crate) fn type_find_selected(&mut self, text: &str, ctx: &mut Context) {
        self.group.focus_child(self.selected_id, ctx);
        for ch in text.chars() {
            let mut ev = Event::KeyDown(tv::KeyEvent::from(Key::Char(ch)));
            self.handle_event(&mut ev, ctx);
        }
    }

    /// The Selected list's own incremental-find query.
    pub(crate) fn selected_find_query(&mut self) -> String {
        self.group
            .child_mut(self.selected_id)
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<ListBox>())
            .and_then(|lb| lb.find_query().map(str::to_string))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use tvision_rs::{timer::TimerQueue, Deferred, Event, KeyEvent};

    /// A headless `Context` for driving the View seams without a running loop
    /// (mirrors the `oc_picker` tests). Returns owned backing stores the caller
    /// must keep alive alongside the borrowed `Context`.
    struct Harness {
        out: VecDeque<Event>,
        timers: TimerQueue,
        deferred: Vec<Deferred>,
    }
    impl Harness {
        fn new() -> Harness {
            Harness {
                out: VecDeque::new(),
                timers: TimerQueue::new(),
                deferred: Vec::new(),
            }
        }
        fn ctx(&mut self) -> Context<'_> {
            Context::new(&mut self.out, &mut self.timers, 0, &mut self.deferred)
        }
        /// Whether a broadcast of `cmd` was queued into the loop output.
        fn broadcast_seen(&self, cmd: Command) -> bool {
            self.out
                .iter()
                .any(|e| matches!(e, Event::Broadcast { command, .. } if *command == cmd))
        }
    }

    fn row(key: &str) -> ShuttleRow {
        ShuttleRow {
            key: key.into(),
            label: key.into(),
            locked: false,
        }
    }

    fn locked(key: &str) -> ShuttleRow {
        ShuttleRow {
            key: key.into(),
            label: key.into(),
            locked: true,
        }
    }

    fn keys(rows: &[ShuttleRow]) -> Vec<&str> {
        rows.iter().map(|r| r.key.as_str()).collect()
    }

    #[test]
    fn layout_splits_columns_and_places_wide_buttons() {
        let l = Shuttle::layout(Rect::new(0, 0, 72, 25));
        // Two columns split at the midpoint (36), 2-cell margins, 4-cell gutter.
        assert_eq!(l.avail_list.a.x, 2, "Available list starts at left margin");
        assert!(
            l.avail_list.b.x <= 34,
            "Available list ends before the gutter"
        );
        assert!(
            l.sel_list.a.x >= 38,
            "Selected list starts after the gutter"
        );
        assert_eq!(
            l.sel_list.b.x, 69,
            "Selected list ends before its scrollbar"
        );
        // Add spans the Available (left) column; Remove spans the Selected (right).
        assert_eq!(
            l.add_btn.a.x, l.avail_list.a.x,
            "Add left edge aligns Available column"
        );
        assert_eq!(
            l.remove_btn.a.x, l.sel_list.a.x,
            "Remove left edge aligns Selected column"
        );
        assert!(
            l.add_btn.b.x - l.add_btn.a.x >= 20,
            "Add is a wide button, got {}",
            l.add_btn.b.x - l.add_btn.a.x
        );
        assert!(
            l.remove_btn.b.x - l.remove_btn.a.x >= 20,
            "Remove is a wide button"
        );
        // The button row sits above where the dialog's OK/Cancel row lands (y-3),
        // with a spacer: buttons top at height-6.
        assert_eq!(l.add_btn.a.y, 25 - 6, "button row two rows above OK/Cancel");
        assert_eq!(l.remove_btn.a.y, 25 - 6);
        // Lists end above the button row.
        assert!(
            l.avail_list.b.y <= l.add_btn.a.y,
            "lists clear the button row"
        );
    }

    #[test]
    fn move_in_appends_available_row_to_selected() {
        let mut m = ShuttleModel::default();
        m.set_available(vec![row("a"), row("b")]);
        assert!(
            m.move_in(0),
            "moving an available row must change the Selected set"
        );
        assert_eq!(keys(m.selected()), ["a"]);
    }

    #[test]
    fn move_in_is_deduped_case_insensitively() {
        let mut m = ShuttleModel::default();
        m.set_available(vec![row("Alice")]);
        m.set_selected(vec![row("alice")]);
        assert!(
            !m.move_in(0),
            "a key already selected (any case) must not be added again"
        );
        assert_eq!(keys(m.selected()), ["alice"], "no duplicate appended");
    }

    #[test]
    fn move_out_removes_a_removable_selected_row() {
        let mut m = ShuttleModel::default();
        m.set_selected(vec![row("x"), row("y")]);
        assert!(
            m.move_out(0),
            "removing an unlocked row must change the Selected set"
        );
        assert_eq!(keys(m.selected()), ["y"]);
    }

    #[test]
    fn move_out_rejects_a_locked_row() {
        let mut m = ShuttleModel::default();
        m.set_selected(vec![locked("top")]);
        assert!(!m.move_out(0), "a locked row must not be removable");
        assert_eq!(keys(m.selected()), ["top"], "locked row stays put");
    }

    #[test]
    fn out_of_range_moves_are_noops() {
        let mut m = ShuttleModel::default();
        m.set_available(vec![row("a")]);
        m.set_selected(vec![row("b")]);
        assert!(!m.move_in(9), "out-of-range move_in is a no-op");
        assert!(!m.move_out(9), "out-of-range move_out is a no-op");
        assert_eq!(keys(m.available()), ["a"]);
        assert_eq!(keys(m.selected()), ["b"]);
    }

    // -- View seams --------------------------------------------------------

    fn shuttle() -> Shuttle {
        Shuttle::new(
            Rect::new(0, 0, 72, 25),
            "Active",
            "Available",
            FindMode::Filter,
        )
    }

    #[test]
    fn insert_moves_highlighted_available_row_in_and_broadcasts() {
        let mut sh = shuttle();
        let mut h = Harness::new();
        // Model order [b, a]; the SortedListBox displays [a, b]. Highlighting
        // display index 0 ("a") must move the *model* row "a" (model index 1),
        // proving the sorted-display index is mapped back to the model.
        sh.set_available(vec![row("b"), row("a")], &mut h.ctx());
        sh.set_selected(vec![], &mut h.ctx());
        let aid = sh.avail_id;
        sh.highlight(aid, 0, &mut h.ctx());

        let mut ev = Event::KeyDown(KeyEvent::from(Key::Insert));
        sh.handle_event(&mut ev, &mut h.ctx());

        assert_eq!(
            keys(sh.model.selected()),
            ["a"],
            "the highlighted display row 'a' must move in"
        );
        assert!(
            h.broadcast_seen(CMD_SHUTTLE_CHANGED),
            "a move must broadcast CMD_SHUTTLE_CHANGED"
        );
    }

    #[test]
    fn enter_on_available_list_moves_in() {
        let mut sh = shuttle();
        let mut h = Harness::new();
        sh.set_available(vec![row("a")], &mut h.ctx());
        sh.set_selected(vec![], &mut h.ctx());
        let aid = sh.avail_id;
        {
            let mut ctx = h.ctx();
            sh.group.focus_child(aid, &mut ctx);
        }
        sh.highlight(aid, 0, &mut h.ctx());
        let mut ev = Event::KeyDown(KeyEvent::from(Key::Enter));
        sh.handle_event(&mut ev, &mut h.ctx());
        assert_eq!(
            keys(sh.model.selected()),
            ["a"],
            "Enter on the Available list moves the row in"
        );
    }

    #[test]
    fn enter_on_selected_list_moves_out() {
        let mut sh = shuttle();
        let mut h = Harness::new();
        sh.set_selected(vec![row("x"), row("y")], &mut h.ctx());
        let sid = sh.selected_id;
        {
            let mut ctx = h.ctx();
            sh.group.focus_child(sid, &mut ctx);
        }
        sh.highlight(sid, 0, &mut h.ctx());
        let mut ev = Event::KeyDown(KeyEvent::from(Key::Enter));
        sh.handle_event(&mut ev, &mut h.ctx());
        assert_eq!(
            keys(sh.model.selected()),
            ["y"],
            "Enter on the Selected list moves the highlighted row out"
        );
    }

    #[test]
    fn the_focused_list_is_bright_and_its_sibling_recedes() {
        // #1: the two lists no longer share one surface. tvision 0.9's default
        // three-surface rule keys each list's surface on its own `state.focused`
        // (owner-active × self-focus), so the focused list paints ListNormal
        // (bright) and the non-focused sibling paints ListSurface (receded) — the
        // shuttle's active/passive columns, driven by the framework, not by hand.
        use crate::ui::theme::edaptor_theme;
        use tvision_rs::{Buffer, DrawCtx, Point, Role};

        let mut sh = shuttle(); // selected_on_left = false → Available LEFT, Selected RIGHT
        let mut h = Harness::new();
        sh.set_available(vec![row("a"), row("b"), row("c")], &mut h.ctx());
        sh.set_selected(vec![row("x"), row("y"), row("z")], &mut h.ctx());
        // The dialog has focused the shuttle, with the Available list current.
        sh.group.state_mut().state.focused = true;
        if let Some(a) = sh.group.child_mut(sh.avail_id) {
            a.state_mut().state.focused = true;
        }
        if let Some(s) = sh.group.child_mut(sh.selected_id) {
            s.state_mut().state.focused = false;
        }

        let theme = edaptor_theme();
        let normal = theme.style(Role::ListNormal).bg; // bright (base3)
        let passive = theme.style(Role::ListSurface).bg; // receded (desktop)
        assert_ne!(
            normal, passive,
            "test premise: the two surfaces must differ"
        );

        let mut buf = Buffer::new(72, 25);
        {
            let mut dc = DrawCtx::new(&mut buf, &theme, Rect::new(0, 0, 72, 25), Point::new(0, 0));
            dc.set_owner_active(true); // the dialog/pane is active
            sh.draw(&mut dc);
        }
        // Read a non-current content row (row 1) in each column's list area. Lists
        // span y 2..18; Available x ~2..33, Selected x ~38..67 (see Shuttle::new).
        let avail_bg = buf.get(4, 3).style().bg;
        let selected_bg = buf.get(50, 3).style().bg;
        assert_eq!(avail_bg, normal, "the focused Available list is bright");
        assert_eq!(
            selected_bg, passive,
            "the non-focused Selected list recedes to the passive surface"
        );
    }

    #[test]
    fn typing_on_the_selected_list_narrows_it_like_the_available_list() {
        // Regression: with no find mode on the Selected list, letters bubbled to
        // the dialog and fired the ~A~dd / ~R~emove button hotkeys; a later fix
        // used Highlight, which only highlights (a different feature). The Selected
        // list now uses `FindMode::Filter` — the same narrow-as-you-type
        // incremental search the Available list does.
        let mut sh = shuttle();
        let mut h = Harness::new();
        sh.set_selected(vec![row("alpha"), row("beta"), row("gamma")], &mut h.ctx());
        sh.type_find_selected("bet", &mut h.ctx());
        assert_eq!(sh.selected_find_query(), "bet", "the query accumulates");
        // Filter narrows the display to the matching row (marker + label).
        let shown = sh.selected_text();
        assert_eq!(
            shown.len(),
            1,
            "Filter narrows to the one match, got {shown:?}"
        );
        assert!(shown[0].ends_with("beta"), "got {shown:?}");
        // The find is a search, not a move — the model is unchanged.
        assert_eq!(keys(sh.model.selected()), ["alpha", "beta", "gamma"]);
    }

    #[test]
    fn deleting_while_the_selected_list_is_filtered_removes_the_right_model_row() {
        // With the display narrowed, the focused row is a *display* index; the
        // model row to remove is resolved by label, so Delete removes the searched
        // row (not whatever model index the display position happens to be).
        let mut sh = shuttle();
        let mut h = Harness::new();
        sh.set_selected(vec![row("alpha"), row("beta"), row("gamma")], &mut h.ctx());
        sh.type_find_selected("beta", &mut h.ctx()); // narrows to ["beta"], focuses the list
        let sid = sh.selected_id;
        sh.highlight(sid, 0, &mut h.ctx()); // display row 0 == beta
        let mut ev = Event::KeyDown(KeyEvent::from(Key::Delete));
        sh.handle_event(&mut ev, &mut h.ctx());
        assert_eq!(
            keys(sh.model.selected()),
            ["alpha", "gamma"],
            "Delete while filtered must remove the model row matching the focused display row"
        );
    }

    #[test]
    fn typing_filters_the_available_list_in_filter_mode() {
        // `shuttle()` builds the Available list with FindMode::Filter. Typing
        // narrows it to rows containing the query; clearing restores the set.
        let mut sh = shuttle();
        let mut h = Harness::new();
        sh.set_available(vec![row("alpha"), row("beta"), row("gamma")], &mut h.ctx());
        sh.type_find("gam", &mut h.ctx());
        assert_eq!(sh.find_query(), "gam", "find_query tracks the typed query");
        assert_eq!(
            sh.avail_text(),
            vec!["gamma".to_string()],
            "Filter mode narrows the Available list to the substring match"
        );
    }

    #[test]
    fn public_accessors_expose_selection() {
        let mut sh = shuttle();
        let mut h = Harness::new();
        sh.set_available(vec![row("a")], &mut h.ctx());
        sh.set_selected(vec![], &mut h.ctx());
        let aid = sh.avail_id;
        sh.highlight(aid, 0, &mut h.ctx());
        let mut ev = Event::KeyDown(KeyEvent::from(Key::Insert));
        sh.handle_event(&mut ev, &mut h.ctx());
        assert_eq!(
            keys(sh.selected()),
            ["a"],
            "selected() reflects the staged set after a move"
        );
    }

    #[test]
    fn value_reports_selected_keys_as_a_list() {
        let mut sh = shuttle();
        let mut h = Harness::new();
        sh.set_selected(vec![row("a"), row("b")], &mut h.ctx());
        assert_eq!(
            View::value(&sh),
            Some(FieldValue::List(vec![
                FieldValue::Text("a".into()),
                FieldValue::Text("b".into()),
            ])),
            "value() must gather the selected keys for dialog data exchange"
        );
    }

    #[test]
    fn set_value_ctx_seeds_and_renders_selected() {
        let mut sh = shuttle();
        let mut h = Harness::new();
        let v = FieldValue::List(vec![
            FieldValue::Text("x".into()),
            FieldValue::Text("y".into()),
        ]);
        sh.set_value_ctx(v, &mut h.ctx());
        assert_eq!(
            keys(sh.model.selected()),
            ["x", "y"],
            "set_value seeds Selected"
        );
        assert_eq!(
            sh.selected_text().len(),
            2,
            "set_value_ctx must also render the Selected column"
        );
    }

    #[test]
    fn typing_a_query_broadcasts_list_find_changed() {
        // The Available list owns the find: typing must broadcast the upstream
        // `Command::LIST_FIND_CHANGED` (so a server-backed host re-runs its
        // query) and `find_query` must report the accumulated text.
        let mut sh = shuttle();
        let mut h = Harness::new();
        sh.set_available(vec![row("foobar"), row("baz")], &mut h.ctx());
        sh.type_find("foo", &mut h.ctx());
        assert_eq!(sh.find_query(), "foo", "find_query must track the query");
        assert!(
            h.broadcast_seen(Command::LIST_FIND_CHANGED),
            "a find edit must broadcast Command::LIST_FIND_CHANGED"
        );
    }

    #[test]
    fn add_button_command_moves_highlighted_available_in() {
        let mut sh = shuttle();
        let mut h = Harness::new();
        sh.set_available(vec![row("a")], &mut h.ctx());
        sh.set_selected(vec![], &mut h.ctx());
        let aid = sh.avail_id;
        sh.highlight(aid, 0, &mut h.ctx());

        let mut ev = Event::Command(CMD_ADD);
        sh.handle_event(&mut ev, &mut h.ctx());

        assert_eq!(
            keys(sh.model.selected()),
            ["a"],
            "[Add] must move the row in"
        );
        assert!(h.broadcast_seen(CMD_SHUTTLE_CHANGED));
    }

    #[test]
    fn remove_button_command_moves_highlighted_selected_out() {
        let mut sh = shuttle();
        let mut h = Harness::new();
        sh.set_selected(vec![row("x"), row("y")], &mut h.ctx());
        let sid = sh.selected_id;
        sh.highlight(sid, 1, &mut h.ctx());

        let mut ev = Event::Command(CMD_REMOVE);
        sh.handle_event(&mut ev, &mut h.ctx());

        assert_eq!(
            keys(sh.model.selected()),
            ["x"],
            "[Remove] must move 'y' out"
        );
        assert!(h.broadcast_seen(CMD_SHUTTLE_CHANGED));
    }

    #[test]
    fn delete_moves_highlighted_selected_row_out_and_broadcasts() {
        let mut sh = shuttle();
        let mut h = Harness::new();
        sh.set_selected(vec![row("x"), row("y")], &mut h.ctx());
        let sid = sh.selected_id;
        sh.highlight(sid, 0, &mut h.ctx()); // unsorted list → display index == model index

        let mut ev = Event::KeyDown(KeyEvent::from(Key::Delete));
        sh.handle_event(&mut ev, &mut h.ctx());

        assert_eq!(
            keys(sh.model.selected()),
            ["y"],
            "highlighted 'x' must move out"
        );
        assert!(
            h.broadcast_seen(CMD_SHUTTLE_CHANGED),
            "a move must broadcast CMD_SHUTTLE_CHANGED"
        );
    }

    #[test]
    fn set_available_populates_the_available_list_unmarked() {
        let mut sh = shuttle();
        let mut h = Harness::new();
        sh.set_available(vec![row("gamma"), row("delta")], &mut h.ctx());
        let text = sh.avail_text();
        assert_eq!(text.len(), 2, "both available rows must render");
        assert!(
            text.iter().any(|s| s == "gamma"),
            "available rows render plain (no marker), got {text:?}"
        );
        assert!(text.iter().any(|s| s == "delta"), "got {text:?}");
    }

    #[test]
    fn set_selected_populates_the_selected_list() {
        let mut sh = shuttle();
        let mut h = Harness::new();
        sh.set_selected(vec![row("alpha"), row("beta")], &mut h.ctx());
        let text = sh.selected_text();
        assert_eq!(text.len(), 2, "both selected rows must render");
        assert!(
            text.iter().any(|s| s.ends_with("alpha")),
            "alpha must appear, got {text:?}"
        );
        assert!(
            text.iter().any(|s| s.ends_with("beta")),
            "beta must appear, got {text:?}"
        );
    }

    #[test]
    fn move_buttons_are_not_tab_stops() {
        let mut sh = shuttle();
        for id in [sh.add_id, sh.remove_id] {
            let selectable = sh
                .group
                .child_mut(id)
                .map(|c| c.state().options.selectable)
                .expect("button present");
            assert!(!selectable, "move buttons must be skipped by Tab traversal");
        }
    }

    #[test]
    fn mouse_wheel_routes_to_the_column_under_the_cursor() {
        // `shuttle()` builds with selected_on_left = false → Available is the LEFT
        // column, Selected the RIGHT. A wheel must scroll the column the cursor is
        // OVER, not whichever scrollbar the group happens to offer first (the
        // non-positional `ScrollBar` wheel grab that made one fixed column eat
        // every wheel event regardless of the cursor).
        let mut sh = shuttle();
        assert_eq!(
            sh.wheel_target_list(10),
            Some(sh.avail_id),
            "a wheel over the left column scrolls the left (Available) list"
        );
        assert_eq!(
            sh.wheel_target_list(50),
            Some(sh.selected_id),
            "a wheel over the right column scrolls the right (Selected) list"
        );
        // Outside both list columns (e.g. the far gutter) → no column to scroll.
        assert_eq!(
            sh.wheel_target_list(0),
            None,
            "left of both columns: no target"
        );
    }

    #[test]
    fn mouse_wheel_scrolls_the_hovered_column_only() {
        use tv::event::{MouseEvent, MouseWheel};
        use tv::Point;
        // selected_on_left = false → Available LEFT, Selected RIGHT.
        let mut sh = shuttle();
        let mut h = Harness::new();
        let many: Vec<ShuttleRow> = (0..30).map(|i| row(&format!("r{i:02}"))).collect();
        sh.set_available(many.clone(), &mut h.ctx());
        sh.set_selected(many, &mut h.ctx());

        // A wheel-down over the LEFT (Available) column advances the Available
        // list's focused row, and leaves the RIGHT (Selected) list untouched.
        let mut ev = Event::MouseWheel(MouseEvent {
            position: Point::new(10, 12),
            wheel: MouseWheel::Down,
            ..Default::default()
        });
        sh.handle_event(&mut ev, &mut h.ctx());

        let avail_focus = match sh.group.child_mut(sh.avail_id).and_then(|v| v.value()) {
            Some(FieldValue::Int(i)) => i,
            _ => -1,
        };
        let selected_focus = match sh.group.child_mut(sh.selected_id).and_then(|v| v.value()) {
            Some(FieldValue::Int(i)) => i,
            _ => -1,
        };
        assert!(
            avail_focus > 0,
            "a wheel over the left column scrolls the Available list, got {avail_focus}"
        );
        assert_eq!(
            selected_focus, 0,
            "the right (Selected) column must NOT scroll, got {selected_focus}"
        );
    }
}
