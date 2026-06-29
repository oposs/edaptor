//! A reusable, domain-free two-column "mover" widget.
//!
//! `DualList` owns the geometry and interaction logic of the side-by-side
//! list mover that `membership.rs` pioneered: an **Available** column (with an
//! optional search box on top) and a **Selected** column, each a `ListBox`, plus
//! the column-header `Label`s. It inserts those child views into a `Dialog` that
//! the *host* owns, and remembers their `ViewId`s.
//!
//! Responsibilities are split cleanly:
//! - **`DualList` owns the interaction**: column geometry, Tab-to-flip which
//!   column the arrow keys drive, Insert/Right to move a highlighted Available
//!   row toward Selected, Delete/Left to remove a highlighted Selected row
//!   (rejected when the row is not `removable`), and reporting search-box edits.
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
    Context, Dialog, Event, FieldValue, InputLine, Key, Label, ListBox, Rect, View, ViewId,
};

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
    /// Tab flipped which column the arrow keys drive.
    FlippedFocus,
    /// The search box text changed to this value.
    SearchChanged(String),
}

/// The two-column mover. Not a `View` itself — it manipulates child views inside
/// the host's `Dialog`, identified by the `ViewId`s captured in [`DualList::new`].
pub(crate) struct DualList {
    /// `ListBox` showing the Available rows.
    avail_id: ViewId,
    /// `ListBox` showing the Selected rows.
    selected_id: ViewId,
    /// Search `InputLine` above the Available column (only when `with_search`).
    search_id: Option<ViewId>,
    /// The Available rows (host-supplied candidates).
    available: Vec<DualRow>,
    /// The Selected rows (the staged set).
    selected: Vec<DualRow>,
    /// Which column the Up/Down/PageUp/PageDown keys drive. `false` ⇒ Available.
    focus_selected: bool,
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

        let avail = ListBox::new(
            Rect::new(avail_col.0, list_y.0, avail_col.1, list_y.1),
            1,
            None,
            None,
        );
        let avail_id = dlg.insert_child(Box::new(avail));

        let sel = ListBox::new(
            Rect::new(sel_col.0, list_y.0, sel_col.1, list_y.1),
            1,
            None,
            None,
        );
        let selected_id = dlg.insert_child(Box::new(sel));

        DualList {
            avail_id,
            selected_id,
            search_id,
            available: Vec::new(),
            selected: Vec::new(),
            focus_selected: false,
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

    /// The current Available set.
    #[allow(dead_code)] // used by the object-class picker (Task 12), not membership
    pub(crate) fn available(&self) -> &[DualRow] {
        &self.available
    }

    /// The last-observed search-box text.
    #[allow(dead_code)] // used by the object-class picker (Task 12), not membership
    pub(crate) fn search_text(&self) -> String {
        self.last_search.clone()
    }

    /// Replace the Available rows and re-render the column (marking rows already
    /// in Selected with a ✓).
    pub(crate) fn set_available(&mut self, rows: Vec<DualRow>, dlg: &mut Dialog, ctx: &mut Context) {
        self.available = rows;
        self.rebuild_avail(dlg, ctx, false);
    }

    /// Replace the Selected rows and re-render both columns (non-removable rows
    /// get a lock marker; the ✓ markers on Available are refreshed).
    pub(crate) fn set_selected(&mut self, rows: Vec<DualRow>, dlg: &mut Dialog, ctx: &mut Context) {
        self.selected = rows;
        self.rebuild_selected(dlg, ctx, false);
        self.rebuild_avail(dlg, ctx, true);
    }

    /// Route one event. Tab flips the active column (`FlippedFocus`); Up/Down/
    /// PageUp/PageDown drive the active column; Insert/Right move the highlighted
    /// Available row toward Selected (`MovedIn`); Delete/Left remove the
    /// highlighted Selected row if removable (`MovedOut`, else `None`); any other
    /// event falls through to the dialog, and a resulting search-box edit is
    /// reported as `SearchChanged`. Space and Enter are intentionally not
    /// intercepted (they reach the search box and the default OK button).
    pub(crate) fn handle_event(
        &mut self,
        ev: &mut Event,
        dlg: &mut Dialog,
        ctx: &mut Context,
    ) -> DualEvent {
        let move_in =
            matches!(ev, Event::KeyDown(k) if matches!(k.key, Key::Insert | Key::Right));
        let move_out =
            matches!(ev, Event::KeyDown(k) if matches!(k.key, Key::Delete | Key::Left));
        let toggle_focus = matches!(ev, Event::KeyDown(k) if k.key == Key::Tab);
        let nav = matches!(
            ev,
            Event::KeyDown(k) if matches!(k.key, Key::Up | Key::Down | Key::PageUp | Key::PageDown)
        );

        let outcome = if move_in {
            let out = match self.highlighted(dlg, self.avail_id) {
                Some(idx) => self.do_move_in(idx),
                None => DualEvent::None,
            };
            if matches!(out, DualEvent::MovedIn(_)) {
                self.rebuild_selected(dlg, ctx, false);
                self.rebuild_avail(dlg, ctx, true); // refresh ✓ on the moved row
            }
            ev.clear();
            out
        } else if move_out {
            let out = match self.highlighted(dlg, self.selected_id) {
                Some(idx) => self.do_move_out(idx),
                None => DualEvent::None,
            };
            if matches!(out, DualEvent::MovedOut(_)) {
                self.rebuild_selected(dlg, ctx, true);
                self.rebuild_avail(dlg, ctx, true); // drop ✓ on the removed row
            }
            ev.clear();
            out
        } else if toggle_focus {
            // Flip which column the arrows drive; keep framework focus where it is
            // (on the search box) so typing keeps working.
            self.focus_selected = !self.focus_selected;
            ev.clear();
            DualEvent::FlippedFocus
        } else if nav {
            let id = if self.focus_selected {
                self.selected_id
            } else {
                self.avail_id
            };
            if let Some(list) = dlg.child_mut(id) {
                list.handle_event(ev, ctx);
            }
            DualEvent::None
        } else {
            dlg.handle_event(ev, ctx);
            DualEvent::None
        };

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

    // -- pure list logic (no Dialog/Context) ------------------------------

    /// Whether `key` is already in the Selected set (case-insensitive).
    fn is_selected(&self, key: &str) -> bool {
        self.selected.iter().any(|r| r.key.eq_ignore_ascii_case(key))
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
            .map(|r| {
                let mark = if self.is_selected(&r.key) {
                    MARK_ALREADY_SELECTED
                } else {
                    MARK_PLAIN
                };
                format!("{mark}{}", r.label)
            })
            .collect();
        Self::repopulate(dlg, self.avail_id, rows, ctx, preserve_cursor);
    }

    fn rebuild_selected(&mut self, dlg: &mut Dialog, ctx: &mut Context, preserve_cursor: bool) {
        let rows: Vec<String> = self
            .selected
            .iter()
            .map(|r| {
                let mark = if r.removable { MARK_PLAIN } else { MARK_LOCKED };
                format!("{mark}{}", r.label)
            })
            .collect();
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

    /// The highlight index of list `id`, if any.
    fn highlighted(&self, dlg: &mut Dialog, id: ViewId) -> Option<usize> {
        match dlg.child_mut(id).and_then(|v| v.value()) {
            Some(FieldValue::Int(i)) if i >= 0 => Some(i as usize),
            _ => None,
        }
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
            search_id: Some(ViewId::next()),
            available: Vec::new(),
            selected: Vec::new(),
            focus_selected: false,
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
        assert_eq!(ev, DualEvent::None, "already-selected row must not duplicate");
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
        dl.focus_selected = true; // simulate the flipped layout's active column
        dl.set_available_rows(vec![row("a")]);
        dl.set_selected_rows(vec![]);
        let ev = dl.move_in_highlighted_for_test(0);
        assert!(matches!(ev, DualEvent::MovedIn(ref k) if k == "a"));
        assert_eq!(keys(dl.selected()), ["a"]);
    }
}
