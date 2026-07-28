//! Inline bullet-list editor view — wraps [`ListModel`] in a tvision [`View`].
//!
//! [`ListValueView`] renders the bulleted item list with a hardware text cursor,
//! maps keystrokes to [`ListModel`] ops, and exposes a boundary-exit signal for
//! the pane to detect when the user navigates out of the top or bottom edge.
//!
//! # Key map
//!
//! | Key | Action |
//! |-----|--------|
//! | Printable char (no ctrl/alt) | insert character at cursor |
//! | Enter | split current item (new item below) |
//! | Ctrl+J (or Ctrl+Enter where the terminal supports it) | insert `\n` continuation within current item |
//! | Backspace | delete previous grapheme / merge into previous item |
//! | Delete | delete next grapheme / pull next item up |
//! | Left / Right | move cursor by one grapheme / wrap across items |
//! | Home / End | jump to start / end of current display line |
//! | Up / Down | move one display row; sets `boundary_exit` at edges |
//! | Ctrl+Up / Ctrl+Down (ordered only) | reorder the current item |
//! | Bracketed paste | insert the clipboard string; each line starts a new value |
//! | Tab | **not consumed** — reserved for pane-level focus switching |

use tvision_rs::{DrawCtx, Event, HelpCtx, Key, Point, Rect, Role, SurfaceRoles, View, ViewState};
use unicode_width::UnicodeWidthStr;

use crate::ui::panes::list_model::{ListModel, Move};
use crate::ui::panes::value_lines::VALUE_INDENT;

/// Display width of an item's `"- "` bullet prefix — the left context kept on
/// screen when the horizontal scroll walks back toward the start of a line.
const BULLET_W: i32 = 2;

/// Surface roles for the value block — mirrors `InputLine::SURFACE_ROLES` so
/// the inline editor blends with the form palette.
const SURFACE_ROLES: SurfaceRoles = SurfaceRoles {
    normal: Role::InputNormal,
    surface: Role::InputSurface,
    inactive: Role::InputInactive,
};

/// Inline bullet-list editor view.
///
/// Wraps a [`ListModel`] and implements [`View`] to:
/// - draw the bulleted item list with a text cursor,
/// - route keystrokes to model ops,
/// - signal boundary exits when Up/Down hits an edge (the pane reads
///   [`take_boundary_exit`](Self::take_boundary_exit) to decide whether to
///   move focus to the previous / next field).
///
/// The form pane (Task 8) constructs this for every multi-value `List` field.
pub(crate) struct ListValueView {
    model: ListModel,
    state: ViewState,
    /// When true, Ctrl+Up/Down reorders items and `to_values` reconstructs
    /// ordering prefixes.
    ordered: bool,
    /// One-shot boundary signal: `Some(-1)` = Up hit top; `Some(1)` = Down hit
    /// bottom. Cleared by [`take_boundary_exit`](Self::take_boundary_exit).
    boundary_exit: Option<i32>,
    /// Help context active while the cursor is in normal editing mode.
    help_ctx_body: HelpCtx,
    /// Help context active while the cursor sits on the reorder handle.
    help_ctx_handle: HelpCtx,
    /// Horizontal scroll offset in display columns — how far the block is
    /// scrolled right. Derived from the caret (scroll-follow, exactly like
    /// `InputLine`), so a value longer than the cell can be walked to its end
    /// instead of being cut at the edge. `◄`/`►` mark hidden text either side.
    h_off: i32,
}

impl ListValueView {
    /// Build a `ListValueView` from an initial value slice.
    ///
    /// `ordered` enables Ctrl+Up/Down reorder and `{n}` prefix reconstruction.
    /// The model is seeded via [`ListModel::from_values`] (strip ordering when
    /// `ordered` is true). The hardware cursor is enabled from construction.
    pub(crate) fn new(
        bounds: Rect,
        values: &[String],
        ordered: bool,
        help_ctx_body: HelpCtx,
        help_ctx_handle: HelpCtx,
    ) -> Self {
        let mut state = ViewState::new(bounds);
        state.options.selectable = true;
        // Deliver the focusing click to `handle_event` (like InputLine) so the
        // very first click positions the caret, instead of only focusing the
        // field and requiring a second click to move within it.
        state.options.first_click = true;
        state.help_ctx = help_ctx_body;
        // Enable the hardware text cursor; the position is updated in `draw`.
        state.show_cursor();
        Self {
            model: ListModel::from_values(values, ordered),
            state,
            ordered,
            boundary_exit: None,
            help_ctx_body,
            help_ctx_handle,
            h_off: 0,
        }
    }

    /// Number of display lines currently produced by the model (including the
    /// `<not set>` placeholder when empty). The form pane uses this to size
    /// the allocated area.
    pub(crate) fn line_count(&self) -> i32 {
        self.model.display_lines().len() as i32
    }

    /// Collect the current values. For ordered fields this reconstructs the
    /// `{n}` ordering prefixes; for unordered it returns plain trimmed strings.
    /// Blank items are always dropped. Delegates to [`ListModel::to_values`].
    pub(crate) fn to_values(&self) -> Vec<String> {
        self.model.to_values(self.ordered)
    }

    /// Clamp [`h_off`](Self::h_off) so the caret is on screen, then place the
    /// hardware cursor at its scrolled column.
    ///
    /// The block scrolls to follow the caret rather than on its own keys: Left /
    /// Right already walk the text, so an item wider than the cell simply carries
    /// the view with it — the same contract `InputLine` has.
    fn sync_cursor(&mut self) {
        let (col, row) = self.model.cursor_xy();
        let vis = (self.state.size.x - VALUE_INDENT).max(1);
        if col < self.h_off + BULLET_W {
            // Scrolling back keeps the `- ` bullet in view — it is what marks
            // where the item starts — so Home lands the line fully home.
            self.h_off = (col - BULLET_W).max(0);
        } else if col >= self.h_off + vis {
            self.h_off = col - vis + 1;
        }
        self.state.set_cursor(col - self.h_off + VALUE_INDENT, row);
    }

    /// Scroll the block back to its left edge. The pane calls this when focus
    /// lands on the field, so a value is never met mid-scroll.
    pub(crate) fn scroll_home(&mut self) {
        self.h_off = 0;
    }

    /// Take and clear the pending boundary-exit signal.
    ///
    /// Returns:
    /// - `Some(-1)` if the most recent Up keystroke tried to move above the
    ///   first display line (pane should focus the previous field),
    /// - `Some(1)` if the most recent Down keystroke tried to move below the
    ///   last display line (pane should focus the next field),
    /// - `None` if no boundary was hit since the last call.
    ///
    /// The signal is cleared on read (one-shot).
    pub(crate) fn take_boundary_exit(&mut self) -> Option<i32> {
        self.boundary_exit.take()
    }

    /// Rebuild the model from `values` (used by the pane's render path when an
    /// external value change arrives while the view is live). Resets the cursor
    /// to item 0, offset 0.
    pub(crate) fn resync(&mut self, values: &[String]) {
        self.model = ListModel::from_values(values, self.ordered);
    }

    /// Move the caret to the first display line. The form calls this when focus
    /// lands on the field (via `place_cursor_home`) so a multi-value field always
    /// opens at the top, matching the single-line fields' home-on-focus.
    pub(crate) fn cursor_home(&mut self) {
        self.model.cursor_to_start();
        self.sync_help_ctx();
    }

    /// Position the caret at a view-local click `(x, y)`. Pure logic layer,
    /// exercised directly in unit tests; called from [`View::handle_event`].
    fn on_mouse(&mut self, x: i32, y: i32) {
        // Content is drawn inset by `VALUE_INDENT`; map the click back before it
        // reaches the model so a click lands on the character under the cursor.
        self.model
            .set_cursor_at_display((x - VALUE_INDENT).max(0), y);
        self.sync_help_ctx();
    }

    /// Update `state.help_ctx` from the current model state: the handle context
    /// while the cursor sits on the reorder handle, else the body context.
    fn sync_help_ctx(&mut self) {
        self.state.help_ctx = if self.model.on_handle() {
            self.help_ctx_handle
        } else {
            self.help_ctx_body
        };
    }

    /// Classify and apply a key event — the pure logic layer.
    ///
    /// Called from [`View::handle_event`] and exercised directly in unit tests
    /// (no `Context` required). After processing, updates `state.help_ctx` to
    /// the handle context when the cursor is on the reorder handle, else the
    /// body context.
    ///
    /// Note on Ctrl+Enter / Ctrl+Up / Ctrl+Down: the framework encodes these as
    /// `KeyEvent { key: Key::Enter/Up/Down, modifiers: KeyModifiers { ctrl: true, .. } }`.
    /// Whether the terminal actually delivers these combinations depends on the
    /// terminal emulator and whether it supports the Kitty keyboard protocol
    /// (edaptor negotiates none); on bare VT100 terminals Ctrl+Enter is
    /// indistinguishable from Enter. For the continuation-line action there is a
    /// portable fallback: Ctrl+J, which the terminal delivers in raw mode as a
    /// literal LF and crossterm surfaces as `Key::Char('j')` + Ctrl — distinct
    /// from Enter on every terminal. (Ctrl+M is a carriage return === Enter, so it
    /// is not a usable alternative.)
    /// Tests cover the framework-level encoding regardless of terminal support.
    pub(crate) fn on_key(&mut self, ev: &mut Event) {
        let Event::KeyDown(k) = ev else { return };
        let key = k.key;
        let ctrl = k.modifiers.ctrl;
        let alt = k.modifiers.alt;

        match (key, ctrl, alt) {
            // Printable character: no ctrl/alt (those belong to accelerators /
            // word-edit shortcuts and must not be silently consumed here).
            (Key::Char(c), false, false) => {
                self.model.insert_char(c);
                ev.clear();
            }

            // Enter splits the current item into a new value.
            (Key::Enter, false, _) => {
                self.model.enter();
                ev.clear();
            }
            // Insert a `\n` continuation line WITHIN the current value. Ctrl+Enter
            // is the intuitive binding, but most terminals cannot distinguish it
            // from plain Enter (no Kitty keyboard protocol is negotiated), so it
            // only works where supported. Ctrl+J is the portable equivalent: in
            // raw mode the terminal delivers it as a literal LF, which crossterm
            // surfaces as `Key::Char('j')` + Ctrl — reliably distinct from Enter.
            // (Ctrl+M is NOT an option: it is a carriage return, i.e. Enter.)
            (Key::Enter, true, _) | (Key::Char('j'), true, false) => {
                self.model.newline();
                ev.clear();
            }

            (Key::Backspace, _, _) => {
                self.model.backspace();
                ev.clear();
            }
            (Key::Delete, _, _) => {
                self.model.delete();
                ev.clear();
            }

            (Key::Left, _, _) => {
                // The model routes the handle vs. previous-item step by `ordered`:
                // ordered lists step onto each item's reorder handle first,
                // unordered lists skip the handle entirely.
                self.model.left(self.ordered);
                ev.clear();
            }
            (Key::Right, _, _) => {
                self.model.right();
                ev.clear();
            }
            (Key::Home, _, _) => {
                self.model.home();
                ev.clear();
            }
            (Key::End, _, _) => {
                self.model.end();
                ev.clear();
            }

            // Up/Down: always consumed. The pane reads `take_boundary_exit()`
            // rather than inspecting the event state for field navigation.
            // Plain Up/Down navigate the display rows; Ctrl+Up/Down (ordered
            // only) reorder items (handled in the arms below so they don't fall
            // through to the boundary-checking plain-Up/Down arms).
            (Key::Up, true, _) if self.ordered => {
                self.model.move_item(-1);
                ev.clear();
            }
            (Key::Down, true, _) if self.ordered => {
                self.model.move_item(1);
                ev.clear();
            }
            (Key::Up, false, _) => {
                if self.model.up() == Move::Boundary {
                    self.boundary_exit = Some(-1);
                }
                ev.clear();
            }
            (Key::Down, false, _) => {
                if self.model.down() == Move::Boundary {
                    self.boundary_exit = Some(1);
                }
                ev.clear();
            }

            // Tab and every other key: leave unconsumed for the pane.
            _ => {}
        }

        // Keep the status-line help context in sync with the model state.
        self.sync_help_ctx();
    }

    /// Insert a bracketed-paste string at the cursor — the pure logic layer for
    /// [`View::handle_event`]'s `Event::Paste` arm (exercised directly in tests).
    ///
    /// The multi-value field is edaptor's own view, not an `InputLine`, so it
    /// must apply the paste itself. Printable characters are inserted like typed
    /// text; a line break starts a **new value** (Enter semantics), so pasting a
    /// newline-separated list fills several values at once. CRLF/CR are
    /// normalised to LF first. (Ctrl+V / Shift+Insert — the framework's internal
    /// clipboard — is a separate `Command::PASTE` path this view does not join;
    /// it has no cut/copy of its own. External clipboard text arrives here as a
    /// terminal bracketed paste.)
    pub(crate) fn on_paste(&mut self, text: &str) {
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        for (i, line) in normalized.split('\n').enumerate() {
            if i > 0 {
                self.model.enter();
            }
            for c in line.chars() {
                self.model.insert_char(c);
            }
        }
        self.sync_help_ctx();
    }
}

impl View for ListValueView {
    fn state(&self) -> &ViewState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut ViewState {
        &mut self.state
    }

    /// Fill the focused surface, draw the bulleted display lines, and position
    /// the hardware text cursor at the model's `cursor_xy()`.
    fn draw(&mut self, ctx: &mut DrawCtx) {
        let size = self.state.size;
        let color = ctx.content_surface(SURFACE_ROLES, self.state.state.focused, true);
        ctx.fill(Rect::new(0, 0, size.x, size.y), ' ', color);
        self.sync_cursor();
        let vis = (size.x - VALUE_INDENT).max(1);
        let mut overflow_right = false;
        for (row, line) in self.model.display_lines().iter().enumerate() {
            if (row as i32) < size.y {
                ctx.put_str_part(VALUE_INDENT, row as i32, line, self.h_off, color);
                overflow_right |= UnicodeWidthStr::width(line.as_str()) as i32 > self.h_off + vis;
            }
        }
        // Mark hidden text on either side, so a cut value never looks complete.
        if self.h_off > 0 {
            for row in 0..size.y.min(self.model.display_lines().len() as i32) {
                ctx.put_char(0, row, '◄', color);
            }
        }
        if overflow_right {
            for row in 0..size.y.min(self.model.display_lines().len() as i32) {
                ctx.put_char(size.x - 1, row, '►', color);
            }
        }
    }

    fn handle_event(&mut self, ev: &mut Event, _ctx: &mut tvision_rs::Context) {
        match ev {
            Event::KeyDown(_) => self.on_key(ev),
            // A click positions the caret at the clicked line/column. `position`
            // is view-local (the group translates it). The event is left
            // unconsumed so the group still focuses this view on the click.
            Event::MouseDown(m) => self.on_mouse(m.position.x, m.position.y),
            // Bracketed paste: the whole external-clipboard string, delivered to
            // the focused view like a key event. Insert it and consume the event
            // (InputLine does the same at editor.rs `Event::Paste`).
            Event::Paste(text) => {
                let text = std::mem::take(text);
                self.on_paste(&text);
                ev.clear();
            }
            _ => {}
        }
        // Keep the hardware cursor in sync with the model *now*, not only at the
        // next `draw`: the enclosing ScrollGroup reads `cursor_request()` right
        // after this event to scroll a tall block so the caret stays visible, so a
        // stale cursor would lag the scroll by a frame while arrowing through it.
        self.sync_cursor();
    }

    /// Return the view-local hardware-cursor position when focused with a
    /// visible cursor, otherwise `None`. Mirrors `input_line.rs`.
    fn cursor_request(&self) -> Option<Point> {
        if self.state.state.focused && self.state.state.cursor_vis {
            Some(self.state.cursor)
        } else {
            None
        }
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tvision_rs::{Event, HelpCtx, Key, KeyEvent, KeyModifiers, Rect};

    fn body() -> HelpCtx {
        HelpCtx::custom("edaptor.test.list_view.body")
    }

    fn handle() -> HelpCtx {
        HelpCtx::custom("edaptor.test.list_view.handle")
    }

    fn key(k: Key) -> Event {
        Event::KeyDown(KeyEvent::from(k))
    }

    fn ctrl_key(k: Key) -> Event {
        Event::KeyDown(KeyEvent::new(
            k,
            KeyModifiers {
                ctrl: true,
                ..KeyModifiers::default()
            },
        ))
    }

    /// Run `f` with a fresh headless `Context` (bracketed-paste tests need one
    /// because `Event::Paste` is delivered through `View::handle_event`).
    fn with_ctx<R>(f: impl FnOnce(&mut tvision_rs::Context) -> R) -> R {
        let mut out = std::collections::VecDeque::new();
        let mut timers = tvision_rs::timer::TimerQueue::new();
        let mut deferred: Vec<tvision_rs::Deferred> = Vec::new();
        let mut ctx = tvision_rs::Context::new(&mut out, &mut timers, 0, &mut deferred);
        f(&mut ctx)
    }

    // --- horizontal scroll ---

    /// The inline editor scrolls to follow its caret, so an item longer than the
    /// cell can be walked (and read) to its end.
    #[test]
    fn the_view_follows_the_caret_past_the_right_edge() {
        let long = "uid=alice,ou=people,dc=example,dc=org";
        let mut v = ListValueView::new(
            Rect::new(0, 0, 20, 1),
            &[long.to_string()],
            false,
            body(),
            handle(),
        );
        // Visible width is 19 (20 less the one-column indent); the caret starts
        // after the "- " bullet, so it is on screen and nothing is scrolled.
        v.on_key(&mut key(Key::Home));
        v.sync_cursor();
        assert_eq!(v.h_off, 0, "a fresh field starts at its left edge");

        v.on_key(&mut key(Key::End)); // caret to the end of the item
        v.sync_cursor();
        assert!(
            v.h_off > 0,
            "the view scrolled to keep the caret visible (h_off {})",
            v.h_off
        );
        let (col, _) = v.model.cursor_xy();
        let screen_col = col - v.h_off + VALUE_INDENT;
        assert!(
            (VALUE_INDENT..20).contains(&screen_col),
            "the caret lands inside the cell, not past it (col {screen_col})"
        );

        // Walking back left brings the view home again.
        v.on_key(&mut key(Key::Home));
        v.sync_cursor();
        assert_eq!(v.h_off, 0, "Home scrolls the view back to the start");
    }

    #[test]
    fn scroll_home_resets_the_offset() {
        let long = "uid=alice,ou=people,dc=example,dc=org";
        let mut v = ListValueView::new(
            Rect::new(0, 0, 20, 1),
            &[long.to_string()],
            false,
            body(),
            handle(),
        );
        v.on_key(&mut key(Key::End));
        v.sync_cursor();
        assert!(v.h_off > 0);
        v.scroll_home();
        assert_eq!(v.h_off, 0, "focus landing resets the scroll");
    }

    // --- bracketed-paste ---

    #[test]
    fn paste_inserts_text_at_the_cursor() {
        // Bracketed paste (Event::Paste) must be delivered to the model like typed
        // text — the multi-value field is edaptor's own view, not an InputLine, so
        // it has to handle the paste itself.
        let mut v = ListValueView::new(
            Rect::new(0, 0, 20, 1),
            &["ab".into()],
            false,
            body(),
            handle(),
        );
        v.on_key(&mut key(Key::End)); // caret after "ab"
        let mut ev = Event::Paste("cd".to_string());
        with_ctx(|ctx| <ListValueView as View>::handle_event(&mut v, &mut ev, ctx));
        assert!(ev.is_nothing(), "a paste event must be consumed");
        assert_eq!(v.to_values(), vec!["abcd".to_string()]);
    }

    #[test]
    fn paste_with_newlines_splits_into_separate_values() {
        // A newline-separated clipboard string pastes as one value per line — the
        // natural mapping for a multi-value attribute (Enter semantics per line).
        let mut v = ListValueView::new(Rect::new(0, 0, 20, 1), &[], false, body(), handle());
        let mut ev = Event::Paste("x\r\ny\nz".to_string());
        with_ctx(|ctx| <ListValueView as View>::handle_event(&mut v, &mut ev, ctx));
        assert_eq!(
            v.to_values(),
            vec!["x".to_string(), "y".to_string(), "z".to_string()],
            "CRLF/LF newlines each start a new value"
        );
    }

    // --- tests from the task brief ---

    #[test]
    fn cursor_home_lands_on_first_line() {
        // Home-on-focus: after moving into the second line, cursor_home returns
        // to the first item's start, so the next keystroke inserts there.
        let mut v = ListValueView::new(
            Rect::new(0, 0, 20, 2),
            &["ab".into(), "cd".into()],
            false,
            body(),
            handle(),
        );
        v.on_key(&mut key(Key::Down)); // to item 1 "cd"
        v.cursor_home();
        v.on_key(&mut key(Key::Char('X')));
        assert_eq!(v.to_values(), vec!["Xab".to_string(), "cd".to_string()]);
    }

    #[test]
    fn new_enables_first_click_so_the_focusing_click_positions() {
        // Without `first_click`, the tvision group clears the focusing click
        // instead of delivering it, so the first click only focuses and a second
        // is needed to move the caret. InputLine sets it; so must we.
        let v = ListValueView::new(
            Rect::new(0, 0, 20, 2),
            &["a".into()],
            false,
            body(),
            handle(),
        );
        assert!(v.state.options.first_click);
    }

    #[test]
    fn click_positions_caret_for_next_keystroke() {
        // A click at (col, row) places the caret so a following character is
        // inserted at the clicked spot. Row 1 = "cd" shown as "- cd"; the content is
        // drawn inset by `VALUE_INDENT`, so a click at display col `3 + VALUE_INDENT`
        // lands just after 'c'.
        let mut v = ListValueView::new(
            Rect::new(0, 0, 20, 2),
            &["ab".into(), "cd".into()],
            false,
            body(),
            handle(),
        );
        v.on_mouse(3 + VALUE_INDENT, 1);
        v.on_key(&mut key(Key::Char('X')));
        assert_eq!(v.to_values(), vec!["ab".to_string(), "cXd".to_string()]);
    }

    #[test]
    fn down_at_bottom_sets_boundary_exit() {
        let mut v = ListValueView::new(
            Rect::new(0, 0, 20, 2),
            &["a".into(), "b".into()],
            false,
            body(),
            handle(),
        );
        // First Down: cursor moves from item 0 to item 1 (Moved).
        v.on_key(&mut key(Key::Down));
        // Second Down: already at the last item → Boundary.
        v.on_key(&mut key(Key::Down));
        assert_eq!(v.take_boundary_exit(), Some(1));
    }

    #[test]
    fn enter_adds_item_and_grows_line_count() {
        let mut v = ListValueView::new(
            Rect::new(0, 0, 20, 1),
            &["a".into()],
            false,
            body(),
            handle(),
        );
        assert_eq!(v.line_count(), 1);
        v.on_key(&mut key(Key::End)); // move to end of "a"
        v.on_key(&mut key(Key::Enter)); // split → ["a", ""]
        assert_eq!(v.line_count(), 2);
        // Trailing empty item is dropped by to_values.
        assert_eq!(v.to_values(), vec!["a".to_string()]);
    }

    // --- additional coverage ---

    #[test]
    fn up_at_top_sets_boundary_exit() {
        let mut v = ListValueView::new(
            Rect::new(0, 0, 20, 1),
            &["a".into()],
            false,
            body(),
            handle(),
        );
        v.on_key(&mut key(Key::Up));
        assert_eq!(v.take_boundary_exit(), Some(-1));
    }

    #[test]
    fn take_boundary_exit_is_one_shot() {
        let mut v = ListValueView::new(
            Rect::new(0, 0, 20, 1),
            &["a".into()],
            false,
            body(),
            handle(),
        );
        v.on_key(&mut key(Key::Up));
        assert_eq!(v.take_boundary_exit(), Some(-1));
        assert_eq!(
            v.take_boundary_exit(),
            None,
            "signal must be cleared after the first take"
        );
    }

    #[test]
    fn down_in_middle_does_not_set_boundary_exit() {
        let mut v = ListValueView::new(
            Rect::new(0, 0, 20, 2),
            &["a".into(), "b".into()],
            false,
            body(),
            handle(),
        );
        v.on_key(&mut key(Key::Down)); // Moved, not Boundary
        assert_eq!(v.take_boundary_exit(), None);
    }

    #[test]
    fn down_always_consumed_even_on_boundary() {
        let mut v = ListValueView::new(
            Rect::new(0, 0, 20, 1),
            &["a".into()],
            false,
            body(),
            handle(),
        );
        let mut ev = key(Key::Down);
        v.on_key(&mut ev);
        assert!(ev.is_nothing(), "Down on boundary must still be consumed");
    }

    #[test]
    fn printable_char_inserts_and_grows_content() {
        let mut v = ListValueView::new(
            Rect::new(0, 0, 20, 1),
            &["x".into()],
            false,
            body(),
            handle(),
        );
        // Cursor is at offset 0; 'y' is inserted before 'x'.
        v.on_key(&mut key(Key::Char('y')));
        assert_eq!(v.to_values(), vec!["yx".to_string()]);
    }

    #[test]
    fn ctrl_enter_inserts_continuation_newline() {
        let mut v = ListValueView::new(
            Rect::new(0, 0, 20, 2),
            &["ab".into()],
            false,
            body(),
            handle(),
        );
        v.on_key(&mut key(Key::End)); // move to end of "ab"
        let mut ev = ctrl_key(Key::Enter);
        v.on_key(&mut ev);
        assert!(ev.is_nothing(), "Ctrl+Enter must be consumed");
        // The single item now contains an embedded newline → two display rows.
        assert_eq!(v.line_count(), 2);
    }

    #[test]
    fn ctrl_j_inserts_continuation_newline() {
        // Portable fallback for Ctrl+Enter: in raw mode the terminal delivers
        // Ctrl+J as a literal LF, surfaced by crossterm as `Key::Char('j')` +
        // Ctrl. It must insert a continuation line, exactly like Ctrl+Enter, so
        // multi-line values stay editable where Ctrl+Enter is swallowed as Enter.
        let mut v = ListValueView::new(
            Rect::new(0, 0, 20, 2),
            &["ab".into()],
            false,
            body(),
            handle(),
        );
        // Cursor after 'a' (mid-value, so the newline survives to_values' trim).
        v.on_key(&mut key(Key::Home));
        v.on_key(&mut key(Key::Right));
        let mut ev = ctrl_key(Key::Char('j'));
        v.on_key(&mut ev);
        assert!(ev.is_nothing(), "Ctrl+J must be consumed");
        assert_eq!(
            v.line_count(),
            2,
            "Ctrl+J inserts a `\\n` continuation line"
        );
        assert_eq!(v.to_values(), vec!["a\nb".to_string()]);
    }

    #[test]
    fn tab_is_not_consumed() {
        let mut v = ListValueView::new(
            Rect::new(0, 0, 20, 1),
            &["a".into()],
            false,
            body(),
            handle(),
        );
        let mut ev = key(Key::Tab);
        v.on_key(&mut ev);
        assert!(
            !ev.is_nothing(),
            "Tab must not be consumed — it belongs to the pane"
        );
    }

    #[test]
    fn ctrl_up_ordered_reorders_items() {
        // For ordered fields, to_values reconstructs {n} prefixes.
        let mut v = ListValueView::new(
            Rect::new(0, 0, 20, 2),
            &["a".into(), "b".into()],
            true, // ordered
            body(),
            handle(),
        );
        v.on_key(&mut key(Key::Down)); // move to item "b"
        let mut ev = ctrl_key(Key::Up);
        v.on_key(&mut ev);
        assert!(ev.is_nothing(), "Ctrl+Up on ordered field must be consumed");
        // Items swapped: ["b", "a"]; to_values reconstructs ordering prefixes.
        assert_eq!(v.to_values(), vec!["{0}b".to_string(), "{1}a".to_string()]);
    }

    #[test]
    fn ctrl_up_unordered_passes_through() {
        let mut v = ListValueView::new(
            Rect::new(0, 0, 20, 2),
            &["a".into(), "b".into()],
            false, // not ordered
            body(),
            handle(),
        );
        v.on_key(&mut key(Key::Down)); // move to item "b"
        let mut ev = ctrl_key(Key::Up);
        v.on_key(&mut ev);
        // Not consumed: Ctrl+Up is a no-op on unordered fields.
        assert!(
            !ev.is_nothing(),
            "Ctrl+Up on unordered field must pass through"
        );
        // Order must be unchanged.
        assert_eq!(v.to_values(), vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn resync_replaces_model_content() {
        let mut v = ListValueView::new(
            Rect::new(0, 0, 20, 1),
            &["a".into()],
            false,
            body(),
            handle(),
        );
        v.resync(&["x".into(), "y".into()]);
        assert_eq!(v.to_values(), vec!["x".to_string(), "y".to_string()]);
        assert_eq!(v.line_count(), 2);
    }

    #[test]
    fn line_count_empty_model_is_one() {
        // An empty model shows the "<not set>" placeholder — still 1 display line.
        let v = ListValueView::new(Rect::new(0, 0, 20, 1), &[], false, body(), handle());
        assert_eq!(v.line_count(), 1);
    }

    #[test]
    fn home_moves_to_start_of_display_line() {
        let mut v = ListValueView::new(
            Rect::new(0, 0, 20, 1),
            &["abc".into()],
            false,
            body(),
            handle(),
        );
        v.on_key(&mut key(Key::End)); // move to end of "abc"
        v.on_key(&mut key(Key::Char('x'))); // insert 'x' at end
                                            // Cursor is now after 'x'; Home brings it back to the start.
        v.on_key(&mut key(Key::Home));
        // Typing here should prepend.
        v.on_key(&mut key(Key::Char('z')));
        assert!(
            v.to_values()[0].starts_with('z'),
            "Home should move to start of item"
        );
    }

    #[test]
    fn help_ctx_switches_to_handle_on_handle_position() {
        // ORDERED view: Left at (item 0, offset 0) enters the handle; the
        // help context must flip to the handle context. A subsequent edit key
        // (Right) leaves the handle and the context must revert to body.
        let mut v = ListValueView::new(
            Rect::new(0, 0, 20, 1),
            &["abc".into()],
            true, // ordered
            body(),
            handle(),
        );
        // Cursor starts at (0, 0); Left moves it onto the handle.
        v.on_key(&mut key(Key::Left));
        assert!(
            v.model.on_handle(),
            "model must be on the handle after Left at origin"
        );
        assert_eq!(
            v.state.help_ctx,
            handle(),
            "help_ctx must switch to handle context while on the handle"
        );
        // Right leaves the handle; context must revert.
        v.on_key(&mut key(Key::Right));
        assert!(
            !v.model.on_handle(),
            "model must leave the handle after Right"
        );
        assert_eq!(
            v.state.help_ctx,
            body(),
            "help_ctx must revert to body context after leaving the handle"
        );
    }

    #[test]
    fn unordered_list_never_enters_handle() {
        // Regression test for Fix 1: an UNORDERED view must never end an event
        // with on_handle()==true, and the help context must stay on body.
        let mut v = ListValueView::new(
            Rect::new(0, 0, 20, 1),
            &["abc".into()],
            false, // unordered
            body(),
            handle(),
        );
        // Drive the cursor to (item 0, offset 0) by pressing Home.
        v.on_key(&mut key(Key::Home));
        // Left at offset 0 would enter the handle on an ordered field;
        // for unordered it must be blocked.
        v.on_key(&mut key(Key::Left));
        assert!(
            !v.model.on_handle(),
            "unordered list must never enter handle mode"
        );
        assert_eq!(
            v.state.help_ctx,
            body(),
            "help_ctx must remain on body for unordered list after Left at origin"
        );
        // Verify the display still shows the plain bullet, not the hamburger.
        let lines = v.model.display_lines();
        assert!(
            lines[0].starts_with("- "),
            "unordered display must show '- ' bullet, not '≡': got {:?}",
            lines[0]
        );
    }
}
