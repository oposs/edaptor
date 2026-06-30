//! `Shuttle` — a domain-free two-list "transfer" widget.
//!
//! The clean re-incubation of `dual_list.rs`: instead of a controller that
//! reaches into the host's `Dialog`, the `Shuttle` *is* a `View` embedding a
//! `Group` and owning its children (two columns, optional search box, Add/Remove
//! buttons), notifying the owner by broadcast (`CMD_SHUTTLE_CHANGED` /
//! `CMD_SHUTTLE_SEARCH`) rather than a bespoke return-value channel. The pure
//! column logic — move / de-dup / lock — lives in [`ShuttleModel`], which is
//! tvision-free and unit-testable without a `Dialog`.

use tvision_rs::{
    self as tv, delegate, Button, ButtonFlags, Command, Context, Event, FieldValue, Group,
    InputLine, Key, Label, ListBox, Rect, ScrollBar, SortedListBox, View, ViewId,
};

/// Broadcast (with the Shuttle's own `ViewId` as `source`) when the Selected set
/// changes via a move. The owner re-reads [`Shuttle::selected`] and reacts.
pub(crate) const CMD_SHUTTLE_CHANGED: Command = Command::custom("shuttle.changed");
/// Broadcast when the search box text changes. The owner re-reads
/// [`Shuttle::search_text`] and re-publishes the Available column.
pub(crate) const CMD_SHUTTLE_SEARCH: Command = Command::custom("shuttle.search");

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

/// The two-list transfer widget. Embeds a `Group` that owns the column lists,
/// their scroll bars, the optional search box and the Add/Remove buttons; the
/// move logic lives in [`ShuttleModel`]. A `View` in its own right (the impl is
/// delegated to the group, with only `handle_event`, `as_any_mut` and the
/// gather/scatter methods hand-written) rather than a controller poking a host's
/// `Dialog`.
pub(crate) struct Shuttle {
    group: Group,
    model: ShuttleModel,
    avail_id: ViewId,
    selected_id: ViewId,
    search_id: Option<ViewId>,
    /// Last-observed search-box text (the value returned by `search_text`).
    last_search: String,
}

impl Shuttle {
    /// Build the two columns (each a list + a right-lane scroll bar) inside an
    /// owned `Group`, plus an optional search box above the Available column.
    /// `selected_on_left` only flips the rendered layout — move semantics are
    /// unchanged. Geometry: headers at row 1, search at rows 2..3, lists at
    /// rows 4..(height-4), 2-cell margins and a 4-cell gutter.
    pub(crate) fn new(
        area: Rect,
        left_title: &str,
        right_title: &str,
        with_search: bool,
        selected_on_left: bool,
    ) -> Shuttle {
        let (x0, y0, x1, y1) = (area.a.x, area.a.y, area.b.x, area.b.y);
        let mid = (x0 + x1) / 2;
        let left = (x0 + 2, mid - 2);
        let right = (mid + 2, x1 - 2);
        let head_y = y0 + 1;
        let search_y = (y0 + 2, y0 + 3);
        let list_y = (y0 + 4, y1 - 4);
        let (avail_col, sel_col) = if selected_on_left {
            (right, left)
        } else {
            (left, right)
        };

        let mut group = Group::new(area);

        // Static column headers over the physical left/right columns.
        group.insert(Box::new(Label::new(
            Rect::new(left.0, head_y, left.1, head_y + 1),
            left_title,
            None,
        )));
        group.insert(Box::new(Label::new(
            Rect::new(right.0, head_y, right.1, head_y + 1),
            right_title,
            None,
        )));

        // Search box above the Available column.
        let search_id = if with_search {
            Some(group.insert(Box::new(InputLine::with_limit(
                Rect::new(avail_col.0, search_y.0, avail_col.1, search_y.1),
                128,
            ))))
        } else {
            None
        };

        // Available column: a SortedListBox (type-to-search) wired to a bar.
        let avail_bar = group.insert(Box::new(ScrollBar::new(Rect::new(
            avail_col.1 - 1,
            list_y.0,
            avail_col.1,
            list_y.1,
        ))));
        let avail_id = group.insert(Box::new(SortedListBox::new(
            Rect::new(avail_col.0, list_y.0, avail_col.1 - 1, list_y.1),
            1,
            None,
            Some(avail_bar),
        )));

        // Selected column: a plain ListBox (insertion order) wired to a bar.
        let selected_bar = group.insert(Box::new(ScrollBar::new(Rect::new(
            sel_col.1 - 1,
            list_y.0,
            sel_col.1,
            list_y.1,
        ))));
        let selected_id = group.insert(Box::new(ListBox::new(
            Rect::new(sel_col.0, list_y.0, sel_col.1 - 1, list_y.1),
            1,
            None,
            Some(selected_bar),
        )));

        // On-screen affordances for the move keys (which are not discoverable),
        // left-aligned on the bottom button row. Alt-A / Alt-R shortcuts work
        // even while another child holds focus.
        let btn_top = y1 - 3;
        group.insert(Box::new(Button::new(
            Rect::new(x0 + 2, btn_top, x0 + 12, btn_top + 2),
            "~A~dd",
            CMD_ADD,
            ButtonFlags::new(),
        )));
        group.insert(Box::new(Button::new(
            Rect::new(x0 + 14, btn_top, x0 + 26, btn_top + 2),
            "~R~emove",
            CMD_REMOVE,
            ButtonFlags::new(),
        )));

        Shuttle {
            group,
            model: ShuttleModel::default(),
            avail_id,
            selected_id,
            search_id,
            last_search: String::new(),
        }
    }

    /// The current search-box text (empty when there is no search box). Updated
    /// as the box is edited; the owner reads it after a `CMD_SHUTTLE_SEARCH`
    /// broadcast to re-publish the Available column.
    pub(crate) fn search_text(&self) -> &str {
        &self.last_search
    }

    /// The current Selected set — the owner reads this after a
    /// `CMD_SHUTTLE_CHANGED` broadcast to stage its commit.
    pub(crate) fn selected(&self) -> &[ShuttleRow] {
        self.model.selected()
    }

    /// The `ViewId` of the search box, if any. The owner typically focuses it so
    /// typing searches immediately.
    pub(crate) fn search_id(&self) -> Option<ViewId> {
        self.search_id
    }

    /// The current search-box text read live from the child `InputLine`.
    fn current_search(&mut self) -> String {
        let Some(id) = self.search_id else {
            return String::new();
        };
        match self.group.child_mut(id).and_then(|v| v.value()) {
            Some(FieldValue::Text(s)) => s,
            _ => String::new(),
        }
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

    /// Index into the model's Selected rows of the highlighted row. The Selected
    /// `ListBox` is unsorted, so its focused index *is* the model index — no
    /// display-string round-trip needed.
    fn selected_highlight(&mut self) -> Option<usize> {
        let lb = self
            .group
            .child_mut(self.selected_id)?
            .as_any_mut()?
            .downcast_mut::<ListBox>()?;
        match lb.value() {
            Some(FieldValue::Int(i)) if i >= 0 => Some(i as usize),
            _ => None,
        }
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

    /// Open-time focus inside the Shuttle: the search box (search-as-you-type)
    /// when present, otherwise the Available list. The host embeds the Shuttle and
    /// focuses the Shuttle itself (so the dialog routes events here); this decides
    /// which child lands focus when that happens. Runs before the first draw.
    fn reset_current(&mut self, ctx: &mut Context) {
        self.group.reset_current(ctx);
        let target = self.search_id().unwrap_or(self.avail_id);
        self.group.focus_child(target, ctx);
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
        // Selected → move out). Enter elsewhere (search box, buttons) passes
        // through so the dialog's default OK still fires.
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

        // Report a search-box edit. The owner re-reads `search_text` and
        // re-publishes the Available column.
        if self.search_id.is_some() {
            let cur = self.current_search();
            if cur != self.last_search {
                self.last_search = cur;
                ctx.broadcast(CMD_SHUTTLE_SEARCH, self.state().id());
            }
        }
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

    /// Simulate the user having typed `text` into the search box.
    pub(crate) fn set_search_text(&mut self, text: &str, ctx: &mut Context) {
        if let Some(id) = self.search_id {
            if let Some(c) = self.group.child_mut(id) {
                c.set_value_ctx(FieldValue::Text(text.into()), ctx);
            }
        }
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
        Shuttle::new(Rect::new(0, 0, 72, 22), "Active", "Available", true, false)
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
    fn enter_on_search_box_does_not_move() {
        let mut sh = shuttle();
        let mut h = Harness::new();
        sh.set_available(vec![row("a")], &mut h.ctx());
        sh.set_selected(vec![row("k")], &mut h.ctx());
        let search = sh.search_id().unwrap();
        {
            let mut ctx = h.ctx();
            sh.group.focus_child(search, &mut ctx);
        }
        let mut ev = Event::KeyDown(KeyEvent::from(Key::Enter));
        sh.handle_event(&mut ev, &mut h.ctx());
        assert_eq!(
            keys(sh.model.selected()),
            ["k"],
            "Enter on the search box must NOT move — it passes through to the dialog (OK)"
        );
    }

    #[test]
    fn public_accessors_expose_selection_and_search_id() {
        let mut sh = shuttle();
        let mut h = Harness::new();
        assert!(sh.search_id().is_some(), "built with a search box");
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
    fn search_box_edit_broadcasts_search_changed() {
        let mut sh = shuttle();
        let mut h = Harness::new();
        sh.set_search_text("foo", &mut h.ctx());
        // Any delegating (non-move) event triggers the post-delegation check.
        let mut ev = Event::KeyDown(KeyEvent::from(Key::End));
        sh.handle_event(&mut ev, &mut h.ctx());
        assert_eq!(sh.search_text(), "foo", "search_text must track the box");
        assert!(
            h.broadcast_seen(CMD_SHUTTLE_SEARCH),
            "a search edit must broadcast CMD_SHUTTLE_SEARCH"
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
