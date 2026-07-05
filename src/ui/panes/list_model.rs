//! Pure, view-free core of the inline multi-value editor.
//!
//! [`ListModel`] owns a `Vec<String>` (each value may contain embedded `\n`
//! continuation lines), a cursor `(item, off)` where `off` is a BYTE offset into
//! `items[item]`, and a reorder-handle flag. It implements every cursor move and
//! edit operation with zero `Context`/`View` coupling, so it can be exhaustively
//! unit-tested. A later task wraps it in a tvision `View`.
//!
//! All grapheme stepping goes through tvision's [`text::next`]/[`text::prev`]
//! (mirroring `input_line.rs`) so the cursor never lands inside a multi-byte
//! codepoint or a combining sequence.

use crate::ui::panes::value_lines;
use tvision_rs::text;

/// Result of a cursor move: `Moved` when the cursor changed inside the model,
/// `Boundary` when it was already at an edge (the view bubbles field navigation).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Move {
    Moved,
    Boundary,
}

/// Byte ranges of the `\n`-split display lines of `s`. Always at least one range
/// (the empty string yields `[(0, 0)]`). The `\n` byte itself is excluded from
/// the range that precedes it.
fn line_ranges(s: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    for (idx, b) in s.bytes().enumerate() {
        if b == b'\n' {
            out.push((start, idx));
            start = idx + 1;
        }
    }
    out.push((start, s.len()));
    out
}

#[derive(Debug, Clone)]
pub(crate) struct ListModel {
    items: Vec<String>,
    item: usize,
    /// Byte offset into `items[item]`.
    off: usize,
    on_handle: bool,
}

impl ListModel {
    /// Build from stored values. When `strip_ordering`, drop each value's `{n}`
    /// ordering prefix. Blank values are filtered out, so empty/all-blank input
    /// yields an empty model (`is_empty()` true). Cursor at item 0, offset 0.
    pub(crate) fn from_values(values: &[String], strip_ordering: bool) -> Self {
        let items: Vec<String> = values
            .iter()
            .map(|v| {
                if strip_ordering {
                    crate::ui::ordered::strip_ordering(v).to_string()
                } else {
                    v.clone()
                }
            })
            .filter(|v| !v.trim().is_empty())
            .collect();
        ListModel {
            items,
            item: 0,
            off: 0,
            on_handle: false,
        }
    }

    /// Trim each item and drop the blanks. When `reconstruct_ordering`, prepend a
    /// contiguous `{0}`, `{1}`, … prefix to the survivors by position.
    pub(crate) fn to_values(&self, reconstruct_ordering: bool) -> Vec<String> {
        let survivors: Vec<String> = self
            .items
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if reconstruct_ordering {
            crate::ui::ordered::reconstruct(&survivors)
        } else {
            survivors
        }
    }

    /// True when the model holds no list items at all — the `<not set>` state.
    /// Blank items created while editing (e.g. by Enter) are NOT "empty": they
    /// render as blank `-` bullets. A field returns to this state only when its
    /// last value's content is deleted (see `collapse_if_single_blank`).
    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// After deleting a value's content, a lone empty item means the field is
    /// now unset: drop it so the display reverts to `<not set>` (spec: removing
    /// the last value reverts to the unset state). Only called from the
    /// content-deleting branches of `backspace`/`delete`, never after `enter`
    /// (which deliberately creates blank lines to edit).
    fn collapse_if_single_blank(&mut self) {
        if self.items.len() == 1 && self.items[0].is_empty() {
            self.items.clear();
            self.item = 0;
            self.off = 0;
        }
    }

    /// Byte length of the current item (0 when the model is empty).
    fn cur_len(&self) -> usize {
        self.items.get(self.item).map(|s| s.len()).unwrap_or(0)
    }

    // --- cursor moves -----------------------------------------------------

    /// Move the caret one position left. `reorder` enables the per-item reorder
    /// handle (ordered lists):
    /// - on the handle → step off to the **previous item's end** (or `Boundary`
    ///   at the very top, where there is no previous line);
    /// - mid-text → step over one grapheme;
    /// - at the start of a bullet (`off == 0`) → step onto **this item's** handle
    ///   (every item, not just the first), so the next Left reaches the previous
    ///   line.
    ///
    /// Unordered lists (`reorder = false`) have no handle: at `off == 0` they step
    /// straight to the previous item's end.
    pub(crate) fn left(&mut self, reorder: bool) -> Move {
        if self.on_handle {
            // Leaving the handle leftward = go to the previous item's end. At the
            // top of the list there is nothing further left.
            if self.item == 0 {
                return Move::Boundary;
            }
            self.on_handle = false;
            self.item -= 1;
            self.off = self.items[self.item].len();
            return Move::Moved;
        }
        if self.off > 0 {
            self.off -= text::prev(&self.items[self.item], self.off);
            return Move::Moved;
        }
        // off == 0: at the start of this bullet's text.
        if reorder {
            // Ordered: step onto this item's reorder handle first.
            self.on_handle = true;
            return Move::Moved;
        }
        // Unordered: no handle — step straight to the previous item's end.
        if self.item == 0 {
            return Move::Boundary;
        }
        self.item -= 1;
        self.off = self.items[self.item].len();
        Move::Moved
    }

    pub(crate) fn right(&mut self) -> Move {
        if self.on_handle {
            self.on_handle = false;
            return Move::Moved;
        }
        let len = self.cur_len();
        if self.off < len {
            let step = text::next(&self.items[self.item][self.off..])
                .map(|(l, _)| l)
                .unwrap_or(0);
            self.off += step;
            return Move::Moved;
        }
        if self.item + 1 < self.items.len() {
            self.item += 1;
            self.off = 0;
            return Move::Moved;
        }
        Move::Boundary
    }

    pub(crate) fn up(&mut self) -> Move {
        if self.on_handle {
            return if self.move_item(-1) {
                Move::Moved
            } else {
                Move::Boundary
            };
        }
        self.vertical(-1)
    }

    pub(crate) fn down(&mut self) -> Move {
        if self.on_handle {
            return if self.move_item(1) {
                Move::Moved
            } else {
                Move::Boundary
            };
        }
        self.vertical(1)
    }

    /// Move the cursor to the previous/next DISPLAY line (continuation-aware),
    /// keeping the desired display column. Returns `Boundary` at the top/bottom.
    fn vertical(&mut self, dir: i32) -> Move {
        if self.items.is_empty() {
            return Move::Boundary;
        }
        let rows = self.rows();
        let (cur, desired_col) = self.locate();
        let target = cur as i32 + dir;
        if target < 0 || target >= rows.len() as i32 {
            return Move::Boundary;
        }
        let (item, start, end) = rows[target as usize];
        let seg = &self.items[item][start..end];
        let (bytes, _) = text::scroll(seg, desired_col as i32, false);
        self.item = item;
        self.off = start + bytes;
        Move::Moved
    }

    /// Flatten the model into display rows: `(item, seg_start_byte, seg_end_byte)`
    /// where the byte range is relative to `items[item]`.
    fn rows(&self) -> Vec<(usize, usize, usize)> {
        let mut v = Vec::new();
        for (i, it) in self.items.iter().enumerate() {
            for (s, e) in line_ranges(it) {
                v.push((i, s, e));
            }
        }
        v
    }

    /// Current display-row index and the desired display column (width of the
    /// text before the cursor within its segment).
    fn locate(&self) -> (usize, usize) {
        let mut row = 0;
        for it in &self.items[..self.item] {
            row += line_ranges(it).len();
        }
        let item_text = &self.items[self.item];
        let ranges = line_ranges(item_text);
        let mut seg_start = 0;
        for (i, (s, e)) in ranges.iter().enumerate() {
            if self.off <= *e {
                row += i;
                seg_start = *s;
                break;
            }
        }
        let col = text::width(&item_text[seg_start..self.off]);
        (row, col)
    }

    // --- edits ------------------------------------------------------------

    /// Ensure there is at least one item to edit into (used by the insert ops
    /// when the model is empty).
    fn ensure_item(&mut self) {
        if self.items.is_empty() {
            self.items.push(String::new());
            self.item = 0;
            self.off = 0;
        }
    }

    pub(crate) fn insert_char(&mut self, c: char) {
        self.on_handle = false;
        self.ensure_item();
        self.items[self.item].insert(self.off, c);
        self.off += c.len_utf8();
    }

    /// Split the current item at the cursor; the tail becomes a new item below,
    /// cursor to its start. At end-of-item this inserts an empty item below. On
    /// an unset field, Enter creates a single blank line to type into (not two).
    pub(crate) fn enter(&mut self) {
        self.on_handle = false;
        if self.items.is_empty() {
            self.items.push(String::new());
            self.item = 0;
            self.off = 0;
            return;
        }
        let tail = self.items[self.item].split_off(self.off);
        self.items.insert(self.item + 1, tail);
        self.item += 1;
        self.off = 0;
    }

    /// Insert a `\n` continuation line within the current value.
    pub(crate) fn newline(&mut self) {
        self.on_handle = false;
        self.ensure_item();
        self.items[self.item].insert(self.off, '\n');
        self.off += 1;
    }

    pub(crate) fn backspace(&mut self) {
        self.on_handle = false;
        if self.items.is_empty() {
            return;
        }
        if self.off > 0 {
            let step = text::prev(&self.items[self.item], self.off);
            let start = self.off - step;
            self.items[self.item].replace_range(start..self.off, "");
            self.off = start;
            self.collapse_if_single_blank();
            return;
        }
        // off == 0: merge into the previous item (removes an empty item's marker).
        if self.item == 0 {
            return;
        }
        let cur = self.items.remove(self.item);
        self.item -= 1;
        self.off = self.items[self.item].len();
        self.items[self.item].push_str(&cur);
    }

    pub(crate) fn delete(&mut self) {
        self.on_handle = false;
        if self.items.is_empty() {
            return;
        }
        let len = self.cur_len();
        if self.off < len {
            let step = text::next(&self.items[self.item][self.off..])
                .map(|(l, _)| l)
                .unwrap_or(0);
            self.items[self.item].replace_range(self.off..self.off + step, "");
            self.collapse_if_single_blank();
            return;
        }
        // end-of-item: pull the next item up.
        if self.item + 1 < self.items.len() {
            let next = self.items.remove(self.item + 1);
            self.items[self.item].push_str(&next);
        }
    }

    // --- reorder / handle -------------------------------------------------

    /// Swap the current item with its neighbour in `dir` (clamped at the ends).
    /// Returns whether a swap happened.
    pub(crate) fn move_item(&mut self, dir: i32) -> bool {
        let target = self.item as i32 + dir;
        if target < 0 || target >= self.items.len() as i32 {
            return false;
        }
        let t = target as usize;
        self.items.swap(self.item, t);
        self.item = t;
        true
    }

    #[allow(dead_code)] // Task 8 may use this; keep for API completeness
    pub(crate) fn enter_handle(&mut self) {
        self.on_handle = true;
    }

    pub(crate) fn on_handle(&self) -> bool {
        self.on_handle
    }

    // --- display ----------------------------------------------------------

    /// The value list rendered as bullet + continuation rows (same formatting as
    /// `value_lines::bullet_lines`), except the current item's marker becomes the
    /// reorder hamburger while `on_handle`. Empty model → `["<not set>"]`.
    pub(crate) fn display_lines(&self) -> Vec<String> {
        if self.is_empty() {
            return vec![value_lines::NOT_SET.to_string()];
        }
        let handle = if self.on_handle {
            Some(self.item)
        } else {
            None
        };
        value_lines::format_items(&self.items, handle)
    }

    /// Move the cursor to the start of the current display line (Home key).
    /// Clears the handle flag. No-op when the model is empty.
    pub(crate) fn home(&mut self) {
        self.on_handle = false;
        if self.items.is_empty() {
            return;
        }
        let rows = self.rows();
        let cur_row = self.locate().0;
        self.off = rows[cur_row].1; // seg_start for this display line
    }

    /// Move the cursor to the end of the current display line (End key).
    /// Clears the handle flag. No-op when the model is empty.
    pub(crate) fn end(&mut self) {
        self.on_handle = false;
        if self.items.is_empty() {
            return;
        }
        let rows = self.rows();
        let cur_row = self.locate().0;
        self.off = rows[cur_row].2; // seg_end for this display line
    }

    /// Reset the caret to the very first display line (item 0, offset 0) and
    /// leave the reorder handle. Used when focus enters the field so it always
    /// opens at the top, mirroring the single-line fields' home-on-focus.
    pub(crate) fn cursor_to_start(&mut self) {
        self.on_handle = false;
        self.item = 0;
        self.off = 0;
    }

    /// Place the caret at display position `(col, row)` — used for mouse clicks.
    /// `row` selects the display line (clamped); `col` maps into the line's text
    /// past the 2-column `"- "`/indent prefix, grapheme-aware and clamped to the
    /// line end. No-op when the model has no items.
    pub(crate) fn set_cursor_at_display(&mut self, col: i32, row: i32) {
        self.on_handle = false;
        if self.items.is_empty() {
            self.item = 0;
            self.off = 0;
            return;
        }
        let rows = self.rows();
        let r = (row.max(0) as usize).min(rows.len() - 1);
        let (item, start, end) = rows[r];
        let seg = &self.items[item][start..end];
        let desired_col = (col - 2).max(0);
        let (bytes, _) = text::scroll(seg, desired_col, false);
        self.item = item;
        self.off = start + bytes;
    }

    /// `(col, row)` of the cursor in display space. `row` is the display-line
    /// index; `col` accounts for the 2-column `"- "`/indent prefix. While
    /// `on_handle`, `col` is 0 (the marker cell). Returns `(0, 0)` when the
    /// model has no items (the `<not set>` state) so the caret does not land on
    /// the placeholder line; a blank item (e.g. from Enter) renders as a `-`
    /// bullet and the caret sits just after the marker.
    pub(crate) fn cursor_xy(&self) -> (i32, i32) {
        if self.is_empty() {
            return (0, 0);
        }
        let mut row = 0i32;
        for it in &self.items[..self.item] {
            row += line_ranges(it).len() as i32;
        }
        let item_text = &self.items[self.item];
        let ranges = line_ranges(item_text);
        let mut seg_start = 0;
        for (i, (s, e)) in ranges.iter().enumerate() {
            if self.off <= *e {
                row += i as i32;
                seg_start = *s;
                break;
            }
        }
        let col = if self.on_handle {
            0
        } else {
            2 + text::width(&item_text[seg_start..self.off]) as i32
        };
        (col, row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(vals: &[&str]) -> ListModel {
        ListModel::from_values(
            &vals.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            false,
        )
    }

    fn v(vals: &[&str]) -> Vec<String> {
        vals.iter().map(|s| s.to_string()).collect()
    }

    // --- from_values / to_values / is_empty -------------------------------

    #[test]
    fn empty_model_is_not_set() {
        let m = ListModel::from_values(&[], false);
        assert!(m.is_empty());
        assert_eq!(m.display_lines(), v(&["<not set>"]));
    }

    #[test]
    fn all_blank_input_is_empty() {
        let m = ListModel::from_values(&["  ".into(), "\t".into()], false);
        assert!(m.is_empty());
        assert_eq!(m.display_lines(), v(&["<not set>"]));
    }

    #[test]
    fn from_values_strips_ordering_prefixes() {
        let m = ListModel::from_values(&["{0}read".into(), "{1}write".into()], true);
        assert_eq!(m.items, v(&["read", "write"]));
        assert_eq!(m.display_lines(), v(&["- read", "- write"]));
    }

    #[test]
    fn to_values_trims_and_drops_empties() {
        let mut m = m(&["  a  ", "b"]);
        // Splice in a blank middle item via enter at end of item 0.
        m.item = 0;
        m.items.insert(1, "   ".into());
        assert_eq!(m.to_values(false), v(&["a", "b"]));
    }

    #[test]
    fn is_empty_transitions_on_edit() {
        let mut m = ListModel::from_values(&[], false);
        assert!(m.is_empty());
        m.insert_char('a');
        assert!(!m.is_empty());
        m.backspace();
        assert!(m.is_empty());
    }

    #[test]
    fn enter_on_empty_creates_one_blank_bullet() {
        // A single Enter on an unset field creates exactly ONE visible blank
        // bullet (not two, and not a hidden item).
        let mut m = ListModel::from_values(&[], false);
        m.enter();
        assert_eq!(m.display_lines(), v(&["- "]));
    }

    #[test]
    fn repeated_enter_shows_blank_bullets_not_collapsed() {
        // Regression (user report "adds N entries at once"): blank items created
        // by Enter must DISPLAY as blank bullets immediately, not collapse to a
        // single `<not set>` line that only reveals them once a letter is typed.
        let mut m = ListModel::from_values(&[], false);
        m.enter();
        m.enter();
        m.enter();
        m.enter();
        assert_eq!(m.display_lines(), v(&["- ", "- ", "- ", "- "]));
    }

    #[test]
    fn blank_lines_then_letter_do_not_multiply() {
        let mut m = ListModel::from_values(&[], false);
        m.enter();
        m.enter();
        assert_eq!(m.display_lines(), v(&["- ", "- "])); // visible BEFORE typing
        m.insert_char('x'); // into the current (last) item
        assert_eq!(m.display_lines(), v(&["- ", "- x"]));
    }

    // --- inserts ----------------------------------------------------------

    #[test]
    fn typing_into_empty_creates_first_item() {
        let mut m = ListModel::from_values(&[], false);
        m.insert_char('a');
        assert_eq!(m.to_values(false), v(&["a"]));
        assert_eq!(m.display_lines(), v(&["- a"]));
    }

    #[test]
    fn multibyte_grapheme_stepping() {
        // Insert an accented (2-byte) char between ASCII, then step left over
        // each grapheme: 'b' (1 byte), 'é' (2 bytes), 'a' (1 byte).
        let mut m = ListModel::from_values(&[], false);
        m.insert_char('a');
        m.insert_char('é');
        m.insert_char('b');
        assert_eq!(m.items[0], "aéb");
        assert_eq!(m.off, 4); // 1 + 2 + 1
        assert_eq!(m.left(false), Move::Moved);
        assert_eq!(m.off, 3); // stepped over 'b'
        assert_eq!(m.left(false), Move::Moved);
        assert_eq!(m.off, 1); // stepped over 'é' (2 bytes), never mid-codepoint
        assert_eq!(m.left(false), Move::Moved);
        assert_eq!(m.off, 0);
        // Now at (0,0): with reorder enabled, one more left enters the handle.
        assert_eq!(m.left(true), Move::Moved);
        assert!(m.on_handle());
    }

    #[test]
    fn emoji_grapheme_stepping() {
        // A ZWJ family emoji is a single grapheme cluster.
        let fam = "👨\u{200d}👩\u{200d}👧";
        let mut m = ListModel::from_values(&[], false);
        for c in format!("x{fam}").chars() {
            m.insert_char(c);
        }
        let end = m.off;
        assert_eq!(m.left(false), Move::Moved);
        assert_eq!(end - m.off, fam.len()); // stepped the whole cluster at once
    }

    // --- enter / newline --------------------------------------------------

    #[test]
    fn enter_splits_current_item() {
        let mut m = m(&["ab"]);
        m.right(); // after 'a'
        m.enter();
        assert_eq!(m.to_values(false), v(&["a", "b"]));
        assert_eq!(m.item, 1);
        assert_eq!(m.off, 0);
    }

    #[test]
    fn enter_at_start_makes_empty_item_above() {
        let mut m = m(&["ab"]);
        m.enter(); // off 0
        assert_eq!(m.items, v(&["", "ab"]));
        assert_eq!(m.item, 1);
        assert_eq!(m.to_values(false), v(&["ab"]));
        assert_eq!(m.display_lines(), v(&["- ", "- ab"]));
    }

    #[test]
    fn enter_at_end_makes_empty_item_below() {
        let mut m = m(&["ab"]);
        m.right();
        m.right(); // end
        m.enter();
        assert_eq!(m.items, v(&["ab", ""]));
        assert_eq!(m.item, 1);
        assert_eq!(m.off, 0);
        assert_eq!(m.display_lines(), v(&["- ab", "- "]));
    }

    #[test]
    fn ctrl_enter_inserts_newline_within_item() {
        let mut m = m(&["ab"]);
        m.right();
        m.newline();
        assert_eq!(m.to_values(false), v(&["a\nb"]));
        assert_eq!(m.display_lines(), v(&["- a", "  b"]));
    }

    // --- backspace --------------------------------------------------------

    #[test]
    fn backspace_mid_item_deletes_previous_grapheme() {
        let mut m = m(&["abc"]);
        m.right();
        m.right(); // after 'b'
        m.backspace();
        assert_eq!(m.items[0], "ac");
        assert_eq!(m.off, 1);
    }

    #[test]
    fn backspace_at_item_start_merges_into_previous() {
        let mut m = m(&["a", "b"]);
        m.down(); // item 1, off 0
        m.backspace();
        assert_eq!(m.to_values(false), v(&["ab"]));
        assert_eq!(m.item, 0);
        assert_eq!(m.off, 1); // at the seam
    }

    #[test]
    fn backspace_at_start_of_empty_first_item_after_split() {
        // enter() at (0,0) makes item 0 an empty marker; Backspace at the start
        // of item 1 merges it back, removing the stray empty marker.
        let mut m = m(&["ab"]);
        m.enter(); // (0,0) split -> ["", "ab"], cursor (1,0)
        assert_eq!(m.items, v(&["", "ab"]));
        m.backspace(); // merge item 1 into empty item 0
        assert_eq!(m.items, v(&["ab"]));
        assert_eq!((m.item, m.off), (0, 0));
    }

    #[test]
    fn emptying_item_then_backspace_removes_marker() {
        // Semantic: emptying an item's content then Backspace removes its marker
        // via the merge path. `down` is column-preserving (see
        // `down_keeps_desired_column_across_items`), so it lands at (item 1, off 0);
        // `delete` empties item 1, then `backspace` at line start merges the now
        // blank marker away.
        let mut m = m(&["a", "x", "c"]);
        m.down(); // item 1, off 0
        m.delete(); // remove 'x' -> item 1 empty
        m.backspace(); // merge empty item 1 into item 0 -> marker gone
        assert_eq!(m.to_values(false), v(&["a", "c"]));
    }

    #[test]
    fn backspace_no_op_at_origin() {
        let mut m = m(&["abc"]);
        m.backspace(); // off 0, item 0 -> no-op
        assert_eq!(m.items, v(&["abc"]));
        assert_eq!((m.item, m.off), (0, 0));
    }

    // --- delete -----------------------------------------------------------

    #[test]
    fn delete_mid_item_removes_next_grapheme() {
        let mut m = m(&["abc"]);
        m.delete(); // remove 'a'
        assert_eq!(m.items[0], "bc");
        assert_eq!(m.off, 0);
    }

    #[test]
    fn delete_at_item_end_pulls_next_up() {
        let mut m = m(&["ab", "cd"]);
        m.right();
        m.right(); // end of item 0
        m.delete();
        assert_eq!(m.to_values(false), v(&["abcd"]));
        assert_eq!((m.item, m.off), (0, 2));
    }

    // --- up / down --------------------------------------------------------

    #[test]
    fn up_down_report_boundary_at_edges() {
        let mut m = m(&["a", "b"]);
        assert_eq!(m.up(), Move::Boundary); // already at top
        assert_eq!(m.down(), Move::Moved);
        assert_eq!(m.down(), Move::Boundary); // at bottom
    }

    #[test]
    fn down_keeps_desired_column_across_items() {
        let mut m = m(&["abc", "defg"]);
        m.right();
        m.right(); // col 2 (before 'c')
        assert_eq!(m.down(), Move::Moved);
        assert_eq!(m.item, 1);
        assert_eq!(m.off, 2); // column 2 of "defg" -> before 'f'
    }

    #[test]
    fn up_down_across_continuation_lines_within_one_item() {
        let mut m = m(&["abc\ndef"]); // one item, two display rows
        m.right();
        m.right(); // col 2 on first line, before 'c'
        assert_eq!(m.down(), Move::Moved);
        assert_eq!(m.item, 0);
        assert_eq!(m.off, 6); // "abc\ndef": index 6 is before 'f' (col 2 of "def")
        assert_eq!(m.cursor_xy(), (4, 1)); // 2 prefix + 2 text, row 1
        assert_eq!(m.up(), Move::Moved);
        assert_eq!(m.off, 2); // back to col 2 of first line
        assert_eq!(m.up(), Move::Boundary);
    }

    #[test]
    fn down_shorter_target_clamps_column() {
        let mut m = m(&["abcd", "e"]);
        m.right();
        m.right();
        m.right(); // col 3
        assert_eq!(m.down(), Move::Moved);
        assert_eq!(m.item, 1);
        assert_eq!(m.off, 1); // "e" is 1 wide -> clamp to end
    }

    // --- reorder / handle -------------------------------------------------

    #[test]
    fn move_item_reorders() {
        let mut m = m(&["a", "b", "c"]);
        m.down(); // item 1 = "b"
        assert!(m.move_item(1));
        assert_eq!(m.to_values(false), v(&["a", "c", "b"]));
        assert_eq!(m.item, 2);
    }

    #[test]
    fn move_item_clamps_at_ends() {
        let mut m = m(&["a", "b"]);
        assert!(!m.move_item(-1)); // already at top
        assert_eq!(m.item, 0);
        m.down();
        assert!(!m.move_item(1)); // already at bottom
        assert_eq!(m.item, 1);
    }

    #[test]
    fn left_at_start_enters_handle_then_right_leaves() {
        let mut m = m(&["a"]);
        assert_eq!(m.left(true), Move::Moved); // onto handle
        assert!(m.on_handle());
        m.right();
        assert!(!m.on_handle());
        assert_eq!((m.item, m.off), (0, 0));
    }

    #[test]
    fn handle_marker_renders_hamburger() {
        let mut m = m(&["a", "b"]);
        m.enter_handle();
        assert_eq!(m.display_lines(), v(&["≡ a", "- b"]));
        assert_eq!(m.cursor_xy(), (0, 0)); // marker cell
    }

    #[test]
    fn on_handle_up_down_reorders() {
        let mut m = m(&["a", "b", "c"]);
        m.enter_handle(); // item 0
        assert_eq!(m.down(), Move::Moved); // swap 0,1 -> ["b","a","c"], item 1
        assert_eq!(m.down(), Move::Moved); // swap 1,2 -> ["b","c","a"], item 2
        assert_eq!(m.to_values(false), v(&["b", "c", "a"]));
        assert_eq!(m.down(), Move::Boundary); // at bottom
        assert!(m.on_handle()); // still on the handle
        assert_eq!(m.up(), Move::Moved); // swap back up
        assert_eq!(m.to_values(false), v(&["b", "a", "c"]));
    }

    #[test]
    fn edit_leaves_handle() {
        let mut m = m(&["a"]);
        m.enter_handle();
        m.insert_char('z');
        assert!(!m.on_handle());
        assert_eq!(m.items[0], "za");
    }

    #[test]
    fn left_moves_to_end_of_previous_item() {
        // Unordered: Left at the start of item 1 steps to the previous item's end.
        let mut m = m(&["ab", "cd"]);
        m.down(); // item 1, off 0
        assert_eq!(m.left(false), Move::Moved);
        assert_eq!((m.item, m.off), (0, 2)); // end of "ab"
    }

    #[test]
    fn ordered_left_at_later_item_start_enters_that_items_handle_first() {
        // Regression: with reorder enabled, Left at the start of a NON-first item
        // must step onto THAT item's handle before moving to the previous line —
        // previously only the first line's bullet could be "hamburgerized".
        let mut m = m(&["ab", "cd"]);
        m.down(); // item 1, off 0
        assert_eq!(m.left(true), Move::Moved);
        assert!(
            m.on_handle(),
            "Left at item 1 start must enter item 1's handle"
        );
        assert_eq!(m.item, 1, "handle belongs to item 1, not the previous item");
        // A further Left steps off the handle to the previous item's end.
        assert_eq!(m.left(true), Move::Moved);
        assert!(!m.on_handle());
        assert_eq!((m.item, m.off), (0, 2), "end of the previous item \"ab\"");
    }

    #[test]
    fn right_at_item_end_moves_to_next_item_start() {
        let mut m = m(&["ab", "cd"]);
        m.right();
        m.right(); // end of item 0
        assert_eq!(m.right(), Move::Moved);
        assert_eq!((m.item, m.off), (1, 0));
        // and Boundary at the very end
        m.right();
        m.right(); // end of item 1
        assert_eq!(m.right(), Move::Boundary);
    }

    // --- ordering round-trip ---------------------------------------------

    #[test]
    fn to_values_reconstructs_ordering_prefixes() {
        let mut m = ListModel::from_values(&["{0}a".into(), "{1}b".into(), "{2}c".into()], true);
        // Move item 0 ("a") down one, then renumber contiguously by new position.
        assert!(m.move_item(1));
        assert_eq!(m.to_values(true), v(&["{0}b", "{1}a", "{2}c"]));
    }

    #[test]
    fn removing_last_item_reverts_to_not_set() {
        let mut m = m(&["only"]);
        // Delete the content forward until the single item is empty.
        for _ in 0..4 {
            m.delete();
        }
        assert!(m.is_empty());
        assert_eq!(m.display_lines(), v(&["<not set>"]));
    }

    // --- cursor_xy --------------------------------------------------------

    #[test]
    fn cursor_to_start_resets_to_first_line() {
        let mut m = m(&["ab", "cd"]);
        m.down();
        m.right(); // move away: item 1, past 'c'
        m.cursor_to_start();
        assert_eq!(m.cursor_xy(), (2, 0)); // first line, caret just after "- "
    }

    #[test]
    fn set_cursor_at_display_maps_click_position() {
        let mut m = m(&["abc", "de"]);
        // Row 1 = item 1 "de" shown as "- de"; col 3 lands after 'd'.
        m.set_cursor_at_display(3, 1);
        assert_eq!(m.cursor_xy(), (3, 1));
        // Clicking past the end of a shorter line clamps to the line end.
        m.set_cursor_at_display(99, 0); // row 0 = "abc"
        assert_eq!(m.cursor_xy(), (5, 0)); // 2 prefix + width("abc")
    }

    #[test]
    fn set_cursor_at_display_clamps_row() {
        let mut m = m(&["a", "b"]);
        m.set_cursor_at_display(2, 99); // row past the end clamps to last line
        assert_eq!(m.cursor_xy(), (2, 1));
    }

    #[test]
    fn cursor_xy_accounts_for_prefix() {
        let mut m = m(&["ab", "cd"]);
        assert_eq!(m.cursor_xy(), (2, 0)); // start of item 0 -> after "- "
        m.right(); // after 'a'
        assert_eq!(m.cursor_xy(), (3, 0));
        m.down(); // item 1, col 1
        assert_eq!(m.cursor_xy(), (3, 1));
    }

    #[test]
    fn cursor_xy_is_origin_when_logically_empty() {
        // Deleting the last value's content collapses the lone empty item back to
        // the unset state (items == []): the field reverts to `<not set>` and the
        // caret returns to the origin rather than landing on the placeholder line.
        let mut m = m(&["a"]);
        m.delete(); // remove 'a' → items collapse to [] (unset)
        assert!(m.is_empty());
        assert_eq!(m.display_lines(), v(&["<not set>"]));
        assert_eq!(m.cursor_xy(), (0, 0));
    }

    #[test]
    fn delete_at_end_of_last_item_is_noop() {
        // Pressing Delete at the very end of the last item must not panic and
        // must leave the model unchanged.
        let mut m = m(&["abc"]);
        m.right();
        m.right();
        m.right(); // position at end of "abc" (off = 3)
        let before = m.items.clone();
        m.delete(); // end-of-item with no next item → no-op
        assert_eq!(m.items, before);
        assert_eq!(m.off, 3);
    }
}
