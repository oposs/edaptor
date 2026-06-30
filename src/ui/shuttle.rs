//! `Shuttle` — a domain-free two-list "transfer" widget (work in progress).
//!
//! This is the clean re-incubation of `dual_list.rs`: instead of a controller
//! that reaches into the host's `Dialog`, the `Shuttle` will *be* a `View`
//! embedding a `Group` and owning its children, notifying the owner by
//! broadcast. The pure column logic — move / de-dup / lock — lives in
//! [`ShuttleModel`], which is tvision-free and unit-testable without a `Dialog`.

use tvision_rs::{
    delegate, Command, Context, Event, FieldValue, Group, InputLine, Key, ListBox, Rect, ScrollBar,
    SortedListBox, View, ViewId,
};

/// Broadcast (with the Shuttle's own `ViewId` as `source`) when the Selected set
/// changes via a move. The owner re-reads [`Shuttle::selected`] and reacts.
pub(crate) const CMD_SHUTTLE_CHANGED: Command = Command::custom("shuttle.changed");
/// Broadcast when the search box text changes. The owner re-reads
/// [`Shuttle::search_text`] and re-publishes the Available column.
#[allow(dead_code)] // wired when the search seam lands
pub(crate) const CMD_SHUTTLE_SEARCH: Command = Command::custom("shuttle.search");

/// Marker prefix for a Selected row that may **not** be removed (locked).
const MARK_LOCKED: &str = "* ";
/// Marker prefix for a plain row — keeps Selected rows aligned with locked ones.
const MARK_PLAIN: &str = "  ";

/// One row in either column. Domain-free: `key` is the host's stable identity
/// (a DN, an object-class name, …) used for de-duplication; `label` is the
/// display text; `locked` blocks moving the row out of the Selected column.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(dead_code)] // fields wired in as the model grows
pub(crate) struct ShuttleRow {
    pub key: String,
    pub label: String,
    pub locked: bool,
}

/// The pure, tvision-free model of the two columns. The `Shuttle` `View` will
/// wrap one of these plus the child views; all move/de-dup/lock logic lives
/// here so it is exercised without a `Dialog`.
#[derive(Default)]
#[allow(dead_code)] // grows under TDD; trimmed before the View is wired up
pub(crate) struct ShuttleModel {
    available: Vec<ShuttleRow>,
    selected: Vec<ShuttleRow>,
}

#[allow(dead_code)]
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

/// The two-list transfer widget. Embeds a `Group` that owns the column lists
/// (and, later, the scroll bars, search box and Add/Remove buttons); the move
/// logic lives in [`ShuttleModel`]. Built to *be* a `View` (the `View` impl is
/// wired up in a later step) rather than poking a host's `Dialog`.
#[allow(dead_code)] // View impl + remaining children wired up under TDD
pub(crate) struct Shuttle {
    group: Group,
    model: ShuttleModel,
    avail_id: ViewId,
    selected_id: ViewId,
    search_id: Option<ViewId>,
    selected_on_left: bool,
}

#[allow(dead_code)]
impl Shuttle {
    /// Build the two columns (each a list + a right-lane scroll bar) inside an
    /// owned `Group`, plus an optional search box above the Available column.
    /// `selected_on_left` only flips the rendered layout — move semantics are
    /// unchanged. Geometry: headers at row 1, search at rows 2..3, lists at
    /// rows 4..(height-4), 2-cell margins and a 4-cell gutter.
    pub(crate) fn new(
        area: Rect,
        _left_title: &str,
        _right_title: &str,
        with_search: bool,
        selected_on_left: bool,
    ) -> Shuttle {
        let (x0, y0, x1, y1) = (area.a.x, area.a.y, area.b.x, area.b.y);
        let mid = (x0 + x1) / 2;
        let left = (x0 + 2, mid - 2);
        let right = (mid + 2, x1 - 2);
        let search_y = (y0 + 2, y0 + 3);
        let list_y = (y0 + 4, y1 - 4);
        let (avail_col, sel_col) = if selected_on_left {
            (right, left)
        } else {
            (left, right)
        };

        let mut group = Group::new(area);

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

        Shuttle {
            group,
            model: ShuttleModel::default(),
            avail_id,
            selected_id,
            search_id,
            selected_on_left,
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

#[delegate(to = group, skip(handle_event, as_any_mut))]
impl View for Shuttle {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        // Insert moves the highlighted Available row toward Selected. Everything
        // else (Tab/arrows/typing) delegates to the group so focus and list
        // navigation work the standard way.
        let move_in = matches!(ev, Event::KeyDown(k) if k.key == Key::Insert);
        let move_out = matches!(ev, Event::KeyDown(k) if k.key == Key::Delete);
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
    }
}

#[cfg(test)]
impl Shuttle {
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

    fn selected_text(&mut self) -> Vec<String> {
        self.list_text(self.selected_id)
    }

    /// The display strings in the Available column (a `SortedListBox`).
    fn avail_text(&mut self) -> Vec<String> {
        self.group
            .child_mut(self.avail_id)
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<SortedListBox>())
            .map(|lb| lb.list().to_vec())
            .unwrap_or_default()
    }

    /// Set the highlighted (focused) row of list `id` to display index `idx`.
    fn highlight(&mut self, id: ViewId, idx: i32, ctx: &mut Context) {
        if let Some(c) = self.group.child_mut(id) {
            c.set_value_ctx(FieldValue::Int(idx), ctx);
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
}
