//! A reusable, domain-free two-column "mover" widget.
//!
//! `DualList` owns the geometry and interaction logic of the side-by-side
//! list mover that `membership.rs` pioneered: an **Available** column (with an
//! optional search box on top) and a **Selected** column, each a `ListBox`, plus
//! the column-header `Label`s. It inserts those child views into a `Dialog` that
//! the *host* owns, and remembers their `ViewId`s.
//!
//! Responsibilities are split cleanly:
//! - **`DualList` owns the move actions**: Insert (or the [Add] button) moves the
//!   highlighted Available row toward Selected; Delete (or the [Remove] button)
//!   removes the highlighted Selected row (rejected when the row is not
//!   `removable`); search-box edits are reported. Everything else — **Tab/Shift-Tab
//!   focus traversal** between the search box, the two lists and the buttons, and
//!   the arrow keys driving whichever list is focused — is left to the dialog, so
//!   focus moves between elements the standard way.
//! - **The host owns the data**: it decides what rows each column shows (an async
//!   candidate search for membership, a static class list for object-classes) and
//!   reacts to the [`DualEvent`]s by re-publishing rows via [`DualList::set_available`]
//!   / [`DualList::set_selected`].
//!
//! Move *semantics* are framed as "toward Selected" / "away from Selected" and do
//! not depend on which physical column Selected sits in: `selected_on_left`
//! only flips the rendered layout (membership wants Available-left; the
//! object-class picker wants Selected/active-left). Insert/Right always means
//! `MovedIn`, Delete/Left always means `MovedOut`.

use tvision_rs::{
    Button, ButtonFlags, Command, Context, Dialog, Event, FieldValue, InputLine, Key, Label,
    ListBox, Rect, ScrollBar, View, ViewId,
};

/// Button command: move the highlighted Available row into Selected (same as
/// Insert / Right). Posted by the on-screen "Add" button.
const CMD_DUAL_IN: Command = Command::custom("edaptor.dual.in");
/// Button command: remove the highlighted Selected row (same as Delete / Left).
/// Posted by the on-screen "Remove" button.
const CMD_DUAL_OUT: Command = Command::custom("edaptor.dual.out");

/// Marker prefix for an Available row that is already in the Selected set.
const MARK_ALREADY_SELECTED: &str = "\u{2713} "; // "✓ "
/// Marker prefix for a plain (movable) row — keeps columns aligned with the
/// two-cell markers above.
const MARK_PLAIN: &str = "  ";
/// Marker prefix for a Selected row that may **not** be removed (locked).
const MARK_LOCKED: &str = "* ";

/// One row in either column. Domain-free: `key` is the host's stable identity
/// (a DN, an object-class name, …) used for de-duplication and reported back in
/// [`DualEvent`]s; `label` is the display text; `removable` gates Delete/Left.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DualRow {
    pub key: String,
    pub label: String,
    pub removable: bool,
}

/// What a `handle_event` call did, for the host to react to. At most one is
/// reported per event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DualEvent {
    /// Nothing the host needs to act on (also: a rejected/no-op move).
    None,
    /// The Available row with this `key` moved into Selected.
    MovedIn(String),
    /// The Selected row with this `key` was removed.
    MovedOut(String),
    /// The search box text changed to this value.
    SearchChanged(String),
    /// Reserved: the active column was flipped. Retained so existing host match
    /// arms keep compiling; no longer emitted (focus moves via standard Tab
    /// traversal now).
    #[allow(dead_code)]
    FlippedFocus,
}

/// The two-column mover. Not a `View` itself — it manipulates child views inside
/// the host's `Dialog`, identified by the `ViewId`s captured in [`DualList::new`].
pub(crate) struct DualList {
    /// `ListBox` showing the Available rows.
    avail_id: ViewId,
    /// `ListBox` showing the Selected rows.
    selected_id: ViewId,
    /// Vertical scroll bar in the Available column's right lane.
    avail_bar: ViewId,
    /// Vertical scroll bar in the Selected column's right lane.
    selected_bar: ViewId,
    /// Visible rows per list column — the overflow threshold for the bars.
    list_page: i32,
    /// Search `InputLine` above the Available column (only when `with_search`).
    search_id: Option<ViewId>,
    /// The Available rows (host-supplied candidates).
    available: Vec<DualRow>,
    /// The Selected rows (the staged set).
    selected: Vec<DualRow>,
    /// Last-observed search-box text (also the value returned by `search_text`).
    last_search: String,
    /// Whether a search box exists.
    with_search: bool,
}

impl DualList {
    /// Build the two columns inside `dlg`, laid out within `area` (typically the
    /// dialog's content rect). `left_title`/`right_title` label the physical
    /// columns; `with_search` adds a search box above the Available column;
    /// `selected_on_left` puts the Selected column on the left (Available on the
    /// right) — move semantics are unchanged either way.
    ///
    /// The column geometry mirrors `membership.rs` exactly when `area` is the full
    /// `0,0,80,22` dialog rect: headers at row 1, search at rows 2..3, lists at
    /// rows 4..(height-4); two columns with a 4-cell gutter and 2-cell margins.
    pub(crate) fn new(
        dlg: &mut Dialog,
        area: Rect,
        left_title: &str,
        right_title: &str,
        with_search: bool,
        selected_on_left: bool,
    ) -> DualList {
        let x0 = area.a.x;
        let y0 = area.a.y;
        let x1 = area.b.x;
        let y1 = area.b.y;
        let mid = (x0 + x1) / 2;

        // Column horizontal extents (2-cell margins, 4-cell gutter).
        let left = (x0 + 2, mid - 2);
        let right = (mid + 2, x1 - 2);

        let head_y = y0 + 1;
        let search_y = (y0 + 2, y0 + 3);
        let list_y = (y0 + 4, y1 - 4);

        // Column headers.
        dlg.insert_child(Box::new(Label::new(
            Rect::new(left.0, head_y, left.1, head_y + 1),
            left_title,
            None,
        )));
        dlg.insert_child(Box::new(Label::new(
            Rect::new(right.0, head_y, right.1, head_y + 1),
            right_title,
            None,
        )));

        // Decide which physical column is Available vs Selected.
        let (avail_col, sel_col) = if selected_on_left {
            (right, left)
        } else {
            (left, right)
        };

        // Search box sits above the Available column.
        let search_id = if with_search {
            let search = InputLine::with_limit(
                Rect::new(avail_col.0, search_y.0, avail_col.1, search_y.1),
                128,
            );
            Some(dlg.insert_child(Box::new(search)))
        } else {
            None
        };

        // Each column reserves its right-most cell for a vertical scroll bar; the
        // ListBox is wired to the bar so it publishes range/value/page (the bar
        // thumb tracks the cursor). Visibility is driven by `sync_bars` (overflow
        // gated) since the lists never take framework focus — it stays on the
        // search box — so the ListBox's own focus-based toggle never fires.
        let avail_bar = dlg.insert_child(Box::new(ScrollBar::new(Rect::new(
            avail_col.1 - 1,
            list_y.0,
            avail_col.1,
            list_y.1,
        ))));
        let avail = ListBox::new(
            Rect::new(avail_col.0, list_y.0, avail_col.1 - 1, list_y.1),
            1,
            None,
            Some(avail_bar),
        );
        let avail_id = dlg.insert_child(Box::new(avail));

        let selected_bar = dlg.insert_child(Box::new(ScrollBar::new(Rect::new(
            sel_col.1 - 1,
            list_y.0,
            sel_col.1,
            list_y.1,
        ))));
        let sel = ListBox::new(
            Rect::new(sel_col.0, list_y.0, sel_col.1 - 1, list_y.1),
            1,
            None,
            Some(selected_bar),
        );
        let selected_id = dlg.insert_child(Box::new(sel));

        // Visible on-screen affordances for the Insert/Delete move keys, which are
        // not discoverable. Left-aligned on the dialog's button row (the host adds
        // OK/Cancel right-aligned afterwards, so they never collide). The Alt-key
        // shortcuts work even while the search box holds focus.
        let btn_top = y1 - 3;
        dlg.insert_child(Box::new(Button::new(
            Rect::new(x0 + 2, btn_top, x0 + 12, btn_top + 2),
            "~A~dd",
            CMD_DUAL_IN,
            ButtonFlags::new(),
        )));
        dlg.insert_child(Box::new(Button::new(
            Rect::new(x0 + 14, btn_top, x0 + 26, btn_top + 2),
            "~R~emove",
            CMD_DUAL_OUT,
            ButtonFlags::new(),
        )));

        DualList {
            avail_id,
            selected_id,
            avail_bar,
            selected_bar,
            list_page: (list_y.1 - list_y.0).max(0),
            search_id,
            available: Vec::new(),
            selected: Vec::new(),
            last_search: String::new(),
            with_search,
        }
    }

    /// The `ViewId` of the search box, if any. The host typically focuses it so
    /// typing searches immediately.
    pub(crate) fn search_id(&self) -> Option<ViewId> {
        self.search_id
    }

    /// The current Selected set.
    pub(crate) fn selected(&self) -> &[DualRow] {
        &self.selected
    }

    /// Replace the Available rows and re-render the column (marking rows already
    /// in Selected with a ✓).
    pub(crate) fn set_available(
        &mut self,
        rows: Vec<DualRow>,
        dlg: &mut Dialog,
        ctx: &mut Context,
    ) {
        self.available = rows;
        self.rebuild_avail(dlg, ctx, false);
        self.sync_bars(dlg, ctx);
    }

    /// Replace the Selected rows and re-render both columns (non-removable rows
    /// get a lock marker; the ✓ markers on Available are refreshed).
    pub(crate) fn set_selected(&mut self, rows: Vec<DualRow>, dlg: &mut Dialog, ctx: &mut Context) {
        self.selected = rows;
        self.rebuild_selected(dlg, ctx, false);
        self.rebuild_avail(dlg, ctx, true);
        self.sync_bars(dlg, ctx);
    }

    /// Route one event. Insert/Right (or the [Add] button) moves the highlighted
    /// Available row toward Selected (`MovedIn`); Delete/Left (or the [Remove]
    /// button) removes the highlighted Selected row if removable (`MovedOut`, else
    /// `None`). Everything else — Tab/Shift-Tab focus traversal, Up/Down driving
    /// whichever list is focused, typing in the search box, Space/Enter, and
    /// button clicks — falls through to the dialog; a resulting search-box edit is
    /// reported as `SearchChanged`.
    pub(crate) fn handle_event(
        &mut self,
        ev: &mut Event,
        dlg: &mut Dialog,
        ctx: &mut Context,
    ) -> DualEvent {
        // Insert/Right and the [Add] button move the highlighted Available row
        // toward Selected; Delete/Left and the [Remove] button move the
        // highlighted Selected row away. Everything else — Tab/Shift-Tab focus
        // traversal between the search box, the two lists and the buttons; Up/Down
        // driving whichever list is focused; typing in the search box; button
        // clicks — is left to the dialog so focus moves between elements the
        // standard way.
        let move_in = matches!(ev, Event::KeyDown(k) if matches!(k.key, Key::Insert | Key::Right))
            || matches!(ev, Event::Command(c) if *c == CMD_DUAL_IN);
        let move_out = matches!(ev, Event::KeyDown(k) if matches!(k.key, Key::Delete | Key::Left))
            || matches!(ev, Event::Command(c) if *c == CMD_DUAL_OUT);

        let outcome = if move_in {
            let out = self.perform_move_in(dlg, ctx);
            ev.clear();
            out
        } else if move_out {
            let out = self.perform_move_out(dlg, ctx);
            ev.clear();
            out
        } else {
            dlg.handle_event(ev, ctx);
            DualEvent::None
        };

        // Keep each column's scroll bar in sync with its (possibly just-changed)
        // length. Cheap and idempotent; runs every event so a bar appears/hides
        // as soon as a move or a host-driven rebuild changes the row counts.
        self.sync_bars(dlg, ctx);

        // Report a search-box edit (only when nothing else happened — move/flip
        // keys never change the search text).
        if self.with_search {
            let cur = self.current_search(dlg);
            if cur != self.last_search {
                self.last_search = cur.clone();
                if matches!(outcome, DualEvent::None) {
                    return DualEvent::SearchChanged(cur);
                }
            }
        }
        outcome
    }

    /// Move the highlighted Available row into Selected, re-rendering both
    /// columns on success. Shared by the Insert/Right keys and the [Add] button.
    fn perform_move_in(&mut self, dlg: &mut Dialog, ctx: &mut Context) -> DualEvent {
        let out = match self.avail_highlight(dlg) {
            Some(idx) => self.do_move_in(idx),
            None => DualEvent::None,
        };
        if matches!(out, DualEvent::MovedIn(_)) {
            self.rebuild_selected(dlg, ctx, false);
            self.rebuild_avail(dlg, ctx, true); // refresh ✓ on the moved row
        }
        out
    }

    /// Remove the highlighted Selected row (rejected for locked rows), re-rendering
    /// both columns on success. Shared by the Delete/Left keys and the [Remove]
    /// button.
    fn perform_move_out(&mut self, dlg: &mut Dialog, ctx: &mut Context) -> DualEvent {
        let out = match self.selected_highlight(dlg) {
            Some(idx) => self.do_move_out(idx),
            None => DualEvent::None,
        };
        if matches!(out, DualEvent::MovedOut(_)) {
            self.rebuild_selected(dlg, ctx, true);
            self.rebuild_avail(dlg, ctx, true); // drop ✓ on the removed row
        }
        out
    }

    /// Show each column's scroll bar only when that column overflows its visible
    /// rows. The wired `ListBox` already publishes the bar's range/value/page, so
    /// this only owns visibility — the lists never take framework focus (it stays
    /// on the search box), so their own focus-based toggle never fires.
    /// `request_set_visible` wins the deferred drain, mirroring the leaf/tree panes.
    fn sync_bars(&self, dlg: &mut Dialog, ctx: &mut Context) {
        let show_avail = self.available.len() as i32 > self.list_page;
        let show_sel = self.selected.len() as i32 > self.list_page;
        if let Some(b) = dlg.child_mut(self.avail_bar) {
            b.state_mut().state.visible = show_avail;
        }
        ctx.request_set_visible(self.avail_bar, show_avail);
        if let Some(b) = dlg.child_mut(self.selected_bar) {
            b.state_mut().state.visible = show_sel;
        }
        ctx.request_set_visible(self.selected_bar, show_sel);
    }

    // -- pure list logic (no Dialog/Context) ------------------------------

    /// Whether `key` is already in the Selected set (case-insensitive).
    fn is_selected(&self, key: &str) -> bool {
        self.selected
            .iter()
            .any(|r| r.key.eq_ignore_ascii_case(key))
    }

    /// The exact display string an Available row renders to (marker + label). The
    /// single source of truth shared by `rebuild_avail` and the highlight matcher,
    /// so an index can be recovered from the ListBox's (re-sorted) display text.
    fn avail_display(&self, r: &DualRow) -> String {
        let mark = if self.is_selected(&r.key) {
            MARK_ALREADY_SELECTED
        } else {
            MARK_PLAIN
        };
        format!("{mark}{}", r.label)
    }

    /// The exact display string a Selected row renders to (marker + label).
    fn selected_display(r: &DualRow) -> String {
        let mark = if r.removable { MARK_PLAIN } else { MARK_LOCKED };
        format!("{mark}{}", r.label)
    }

    /// Move the Available row at `idx` into Selected. De-duped: a no-op (`None`)
    /// if already selected or `idx` is out of range. Otherwise appends and
    /// returns `MovedIn(key)`.
    fn do_move_in(&mut self, idx: usize) -> DualEvent {
        let Some(row) = self.available.get(idx).cloned() else {
            return DualEvent::None;
        };
        if self.is_selected(&row.key) {
            return DualEvent::None;
        }
        let key = row.key.clone();
        self.selected.push(row);
        DualEvent::MovedIn(key)
    }

    /// Remove the Selected row at `idx`. Rejected (`None`) if the row is not
    /// `removable` or `idx` is out of range. Otherwise removes and returns
    /// `MovedOut(key)`.
    fn do_move_out(&mut self, idx: usize) -> DualEvent {
        let Some(row) = self.selected.get(idx) else {
            return DualEvent::None;
        };
        if !row.removable {
            return DualEvent::None; // locked row stays put
        }
        let key = self.selected.remove(idx).key;
        DualEvent::MovedOut(key)
    }

    // -- Dialog-facing rendering ------------------------------------------

    fn rebuild_avail(&mut self, dlg: &mut Dialog, ctx: &mut Context, preserve_cursor: bool) {
        let rows: Vec<String> = self
            .available
            .iter()
            .map(|r| self.avail_display(r))
            .collect();
        Self::repopulate(dlg, self.avail_id, rows, ctx, preserve_cursor);
    }

    fn rebuild_selected(&mut self, dlg: &mut Dialog, ctx: &mut Context, preserve_cursor: bool) {
        let rows: Vec<String> = self.selected.iter().map(Self::selected_display).collect();
        Self::repopulate(dlg, self.selected_id, rows, ctx, preserve_cursor);
    }

    /// Replace a list's rows, optionally preserving (and clamping) the cursor.
    /// Lifted verbatim from `membership.rs`.
    fn repopulate(
        dlg: &mut Dialog,
        id: ViewId,
        rows: Vec<String>,
        ctx: &mut Context,
        preserve_cursor: bool,
    ) {
        let rows_len = rows.len();
        if let Some(list) = dlg.child_mut(id) {
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

    /// The display string currently highlighted in list `id`. A `ListBox`
    /// **re-sorts** the rows it is given (`new_list` sorts case-insensitively),
    /// so its focused *index* is in display order — which differs from our row
    /// `Vec` order because of the marker prefixes (`"* "` locked sorts after
    /// `"  "` plain). We therefore read the highlighted *text* and map it back to
    /// a row, rather than trusting the index across the two orderings.
    fn highlighted_text(&self, dlg: &mut Dialog, id: ViewId) -> Option<String> {
        let lb = dlg.child_mut(id)?.as_any_mut()?.downcast_mut::<ListBox>()?;
        let focused = match lb.value() {
            Some(FieldValue::Int(i)) if i >= 0 => i as usize,
            _ => return None,
        };
        lb.list().get(focused).cloned()
    }

    /// Index into `self.available` of the row highlighted in the Available list,
    /// resolved by matching its rendered display string (re-sort-proof).
    fn avail_highlight(&self, dlg: &mut Dialog) -> Option<usize> {
        let text = self.highlighted_text(dlg, self.avail_id)?;
        self.available
            .iter()
            .position(|r| self.avail_display(r) == text)
    }

    /// Index into `self.selected` of the row highlighted in the Selected list,
    /// resolved by matching its rendered display string (re-sort-proof).
    fn selected_highlight(&self, dlg: &mut Dialog) -> Option<usize> {
        let text = self.highlighted_text(dlg, self.selected_id)?;
        self.selected
            .iter()
            .position(|r| Self::selected_display(r) == text)
    }

    /// The current search-box text (empty when there is no search box).
    fn current_search(&self, dlg: &mut Dialog) -> String {
        let Some(id) = self.search_id else {
            return String::new();
        };
        match dlg.child_mut(id).and_then(|v| v.value()) {
            Some(FieldValue::Text(s)) => s,
            _ => String::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests — exercise the pure list logic through headless seams (no Dialog).
// ---------------------------------------------------------------------------

#[cfg(test)]
impl DualList {
    /// Build a `DualList` with no backing `Dialog` — only the in-memory row
    /// state is usable. Mirrors `ScrollGroup`'s `*_for_test` seams: the `ViewId`s
    /// are minted but resolve to nothing, so only the pure logic is exercised.
    fn headless_for_test() -> DualList {
        DualList {
            avail_id: ViewId::next(),
            selected_id: ViewId::next(),
            avail_bar: ViewId::next(),
            selected_bar: ViewId::next(),
            list_page: 10,
            search_id: Some(ViewId::next()),
            available: Vec::new(),
            selected: Vec::new(),
            last_search: String::new(),
            with_search: true,
        }
    }

    fn set_available_rows(&mut self, rows: Vec<DualRow>) {
        self.available = rows;
    }

    fn set_selected_rows(&mut self, rows: Vec<DualRow>) {
        self.selected = rows;
    }

    fn move_in_highlighted_for_test(&mut self, idx: usize) -> DualEvent {
        self.do_move_in(idx)
    }

    fn move_out_highlighted_for_test(&mut self, idx: usize) -> DualEvent {
        self.do_move_out(idx)
    }

    /// The Available `ListBox`'s `ViewId` — lets a host's tests drive the real
    /// column (set the highlight, dispatch a key event) through `handle_event`.
    pub(crate) fn avail_id_for_test(&self) -> ViewId {
        self.avail_id
    }

    /// The Selected `ListBox`'s `ViewId` (see [`DualList::avail_id_for_test`]).
    pub(crate) fn selected_id_for_test(&self) -> ViewId {
        self.selected_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(key: &str) -> DualRow {
        DualRow {
            key: key.into(),
            label: key.into(),
            removable: true,
        }
    }

    fn locked(key: &str) -> DualRow {
        DualRow {
            key: key.into(),
            label: key.into(),
            removable: false,
        }
    }

    fn keys(rows: &[DualRow]) -> Vec<&str> {
        rows.iter().map(|r| r.key.as_str()).collect()
    }

    #[test]
    fn move_in_appends_to_selected_and_reports() {
        let mut dl = DualList::headless_for_test();
        dl.set_available_rows(vec![row("a"), row("b")]);
        dl.set_selected_rows(vec![]);
        let ev = dl.move_in_highlighted_for_test(0);
        assert!(matches!(ev, DualEvent::MovedIn(ref k) if k == "a"));
        assert_eq!(keys(dl.selected()), ["a"]);
    }

    #[test]
    fn move_in_is_deduped() {
        let mut dl = DualList::headless_for_test();
        dl.set_available_rows(vec![row("a")]);
        dl.set_selected_rows(vec![row("a")]);
        let ev = dl.move_in_highlighted_for_test(0);
        assert_eq!(
            ev,
            DualEvent::None,
            "already-selected row must not duplicate"
        );
        assert_eq!(keys(dl.selected()), ["a"]);
    }

    #[test]
    fn move_out_respects_removable_flag() {
        let mut dl = DualList::headless_for_test();
        dl.set_selected_rows(vec![locked("top")]);
        let ev = dl.move_out_highlighted_for_test(0);
        assert_eq!(ev, DualEvent::None, "non-removable row stays");
        assert_eq!(dl.selected().len(), 1);
    }

    #[test]
    fn move_out_removes_removable_and_reports() {
        let mut dl = DualList::headless_for_test();
        dl.set_selected_rows(vec![row("x"), row("y")]);
        let ev = dl.move_out_highlighted_for_test(0);
        assert!(matches!(ev, DualEvent::MovedOut(ref k) if k == "x"));
        assert_eq!(keys(dl.selected()), ["y"]);
    }

    #[test]
    fn out_of_range_moves_are_noops() {
        let mut dl = DualList::headless_for_test();
        dl.set_available_rows(vec![row("a")]);
        assert_eq!(dl.move_in_highlighted_for_test(9), DualEvent::None);
        assert_eq!(dl.move_out_highlighted_for_test(9), DualEvent::None);
    }

    /// `selected_on_left` is a layout flag only; the move semantics (which key
    /// means "toward Selected") are identical regardless of which side renders
    /// the Selected column.
    #[test]
    fn selected_on_left_does_not_change_move_semantics() {
        let mut dl = DualList::headless_for_test();
        dl.set_available_rows(vec![row("a")]);
        dl.set_selected_rows(vec![]);
        let ev = dl.move_in_highlighted_for_test(0);
        assert!(matches!(ev, DualEvent::MovedIn(ref k) if k == "a"));
        assert_eq!(keys(dl.selected()), ["a"]);
    }
}
