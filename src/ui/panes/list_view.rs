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
//! | Ctrl+Enter | insert `\n` continuation within current item |
//! | Backspace | delete previous grapheme / merge into previous item |
//! | Delete | delete next grapheme / pull next item up |
//! | Left / Right | move cursor by one grapheme / wrap across items |
//! | Home / End | jump to start / end of current display line |
//! | Up / Down | move one display row; sets `boundary_exit` at edges |
//! | Ctrl+Up / Ctrl+Down (ordered only) | reorder the current item |
//! | Tab | **not consumed** — reserved for pane-level focus switching |

use tvision_rs::{DrawCtx, Event, HelpCtx, Key, Point, Rect, Role, SurfaceRoles, View, ViewState};

use crate::ui::panes::list_model::{ListModel, Move};

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
/// Task 8 wires this into the form pane; until then `#[allow(dead_code)]` is
/// present to silence the "never constructed" lint.
#[allow(dead_code)]
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
}

#[allow(dead_code)]
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
    /// terminal emulator and whether it supports the Kitty keyboard protocol;
    /// on bare VT100 terminals Ctrl+Enter is indistinguishable from Enter.
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

            // Enter splits the current item; Ctrl+Enter inserts a `\n`
            // continuation within the current item.
            (Key::Enter, false, _) => {
                self.model.enter();
                ev.clear();
            }
            (Key::Enter, true, _) => {
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
                self.model.left();
                // Unordered lists must never enter the reorder handle: if the
                // model transitioned onto the handle (item 0, offset 0 → Left),
                // pull it back immediately so on_handle() stays false for the
                // help_ctx sync below and for any subsequent key handler.
                if !self.ordered && self.model.on_handle() {
                    self.model.leave_handle();
                }
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
        self.state.help_ctx = if self.model.on_handle() {
            self.help_ctx_handle
        } else {
            self.help_ctx_body
        };
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
        for (row, line) in self.model.display_lines().iter().enumerate() {
            if (row as i32) < size.y {
                ctx.put_str(0, row as i32, line, color);
            }
        }
        // Update the cursor position; `cursor_request` reads it on the next pump.
        let (col, row) = self.model.cursor_xy();
        self.state.set_cursor(col, row);
    }

    fn handle_event(&mut self, ev: &mut Event, _ctx: &mut tvision_rs::Context) {
        if matches!(ev, Event::KeyDown(_)) {
            self.on_key(ev);
        }
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

    // --- tests from the task brief ---

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
