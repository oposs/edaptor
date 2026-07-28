//! Read-only value block for modal ("launch") fields: renders a bulleted list
//! (or `*****` / `<not set>`), highlights as a whole when focused, and reports an
//! activation request when the user presses any action key. Editing happens in
//! the modal the pane opens — this view never mutates values.

use crate::ui::panes::value_lines::VALUE_INDENT;
use tvision_rs::{
    self as tv, DrawCtx, Event, HelpCtx, Key, Point, Rect, Role, SurfaceRoles, View, ViewState,
};
use unicode_width::UnicodeWidthStr;

pub(crate) struct LaunchValueView {
    state: ViewState,
    lines: Vec<String>,
    activate: bool,
    /// Horizontal scroll offset in display columns. The block has no caret to
    /// follow, so `←`/`→` move it directly — that is the only way to read a
    /// member DN longer than the cell. `◄`/`►` mark the hidden text.
    h_off: i32,
}

/// Surface role triple for the value block — follows the same convention as
/// `InputLine::SURFACE_ROLES` so it integrates naturally with the form palette.
const SURFACE_ROLES: SurfaceRoles = SurfaceRoles {
    normal: Role::InputNormal,
    surface: Role::InputSurface,
    inactive: Role::InputInactive,
};

impl LaunchValueView {
    pub(crate) fn new(bounds: Rect, help_ctx: HelpCtx) -> Self {
        let mut state = ViewState::new(bounds);
        state.options.selectable = true;
        state.help_ctx = help_ctx;
        Self {
            state,
            lines: vec!["<not set>".to_string()],
            activate: false,
            h_off: 0,
        }
    }

    /// Replace the display lines. Pass the already-formatted strings (bullets,
    /// `*****`, or `<not set>`); the pane owns the formatting logic.
    pub(crate) fn set_lines(&mut self, lines: Vec<String>) {
        self.lines = if lines.is_empty() {
            vec!["<not set>".to_string()]
        } else {
            lines
        };
    }

    /// The widest display line, in columns — the extent horizontal scrolling may
    /// reach.
    fn content_width(&self) -> i32 {
        self.lines
            .iter()
            .map(|l| UnicodeWidthStr::width(l.as_str()) as i32)
            .max()
            .unwrap_or(0)
    }

    /// Scroll the block back to its left edge. The pane calls this when focus
    /// lands on the field, so a block is never met mid-scroll.
    pub(crate) fn scroll_home(&mut self) {
        self.h_off = 0;
    }

    /// Test seam: the current horizontal scroll offset.
    #[cfg(test)]
    pub(crate) fn h_off_for_test(&self) -> i32 {
        self.h_off
    }

    /// Returns `true` once if the last event was an action key (the pane then
    /// posts `ACTIVATE` to open the editor modal). Resets the flag on read.
    pub(crate) fn take_activate(&mut self) -> bool {
        std::mem::take(&mut self.activate)
    }

    /// Test seam: the first display line, or `None` when no lines are set.
    #[cfg(test)]
    pub(crate) fn first_line_for_test(&self) -> Option<String> {
        self.lines.first().cloned()
    }

    /// Classify a key: nav keys pass through (leave `ev` untouched so the pane
    /// can move focus between fields); any other key marks an activation request
    /// and consumes the event.
    pub(crate) fn on_key(&mut self, ev: &mut Event) {
        let Event::KeyDown(k) = ev else { return };
        // Left/Right scroll the block sideways while there is anything to reveal;
        // a long member DN is otherwise unreadable past the cell edge. Once the
        // block is back at its left edge, Left falls through to the pane so
        // horizontal keys still leave the field when there is nothing to scroll.
        let vis = (self.state.size.x - VALUE_INDENT).max(1);
        let max_off = (self.content_width() - vis).max(0);
        match k.key {
            Key::Right if self.h_off < max_off => {
                self.h_off += 1;
                ev.clear();
                return;
            }
            Key::Left if self.h_off > 0 => {
                self.h_off -= 1;
                ev.clear();
                return;
            }
            _ => {}
        }

        let is_nav = matches!(
            k.key,
            Key::Up
                | Key::Down
                | Key::Left
                | Key::Right
                | Key::Home
                | Key::End
                | Key::PageUp
                | Key::PageDown
                | Key::Tab
        );
        if is_nav {
            return; // leave for the pane's field-navigation logic
        }
        self.activate = true;
        ev.clear();
    }
}

impl View for LaunchValueView {
    fn state(&self) -> &ViewState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut ViewState {
        &mut self.state
    }

    fn draw(&mut self, ctx: &mut DrawCtx) {
        let size = self.state.size;
        // `content_surface` picks InputNormal (focused), InputSurface (unfocused
        // but selectable), or InputInactive (owner pane not active) — the whole
        // block changes colour, giving a clear whole-block highlight on focus.
        let color = ctx.content_surface(SURFACE_ROLES, self.state.state.focused, true);
        ctx.fill(Rect::new(0, 0, size.x, size.y), ' ', color);
        let vis = (size.x - VALUE_INDENT).max(1);
        let rows = size.y.min(self.lines.len() as i32);
        for (row, line) in self.lines.iter().enumerate() {
            if (row as i32) < size.y {
                ctx.put_str_part(VALUE_INDENT, row as i32, line, self.h_off, color);
            }
        }
        // Mark hidden text either side, so a cut value never looks complete.
        if self.h_off > 0 {
            for row in 0..rows {
                ctx.put_char(0, row, '◄', color);
            }
        }
        if self.content_width() > self.h_off + vis {
            for row in 0..rows {
                ctx.put_char(size.x - 1, row, '►', color);
            }
        }
    }

    fn handle_event(&mut self, ev: &mut Event, _ctx: &mut tv::Context) {
        if matches!(ev, Event::KeyDown(_)) {
            self.on_key(ev);
        }
    }

    /// No text cursor: the whole-block highlight is the focus indicator.
    fn cursor_request(&self) -> Option<Point> {
        None
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tvision_rs::HelpCtx;
    use tvision_rs::{Event, Key, KeyEvent, Rect};

    fn view() -> LaunchValueView {
        LaunchValueView::new(
            Rect::new(0, 0, 20, 1),
            HelpCtx::custom("edaptor.field.launch"),
        )
    }

    /// A view whose lines are wider than its 20-column cell — a group's member
    /// DNs are the real case.
    fn wide_view() -> LaunchValueView {
        let mut v = view();
        v.set_lines(vec![
            "- uid=alice,ou=people,dc=example,dc=org".to_string(),
            "- uid=bob,ou=people,dc=example,dc=org".to_string(),
        ]);
        v
    }

    #[test]
    fn right_scrolls_a_block_wider_than_its_cell() {
        // Without this a long member DN is cut at the cell edge with no way to
        // see the rest — the block has no caret to carry the view along.
        let mut v = wide_view();
        for _ in 0..3 {
            let mut ev = Event::KeyDown(KeyEvent::from(Key::Right));
            v.on_key(&mut ev);
            assert!(ev.is_nothing(), "the scroll consumes the key");
        }
        assert_eq!(v.h_off_for_test(), 3);

        // Left walks it back, and stops at the left edge.
        for _ in 0..5 {
            let mut ev = Event::KeyDown(KeyEvent::from(Key::Left));
            v.on_key(&mut ev);
        }
        assert_eq!(v.h_off_for_test(), 0);
    }

    #[test]
    fn scrolling_stops_at_the_widest_line() {
        let mut v = wide_view();
        for _ in 0..200 {
            let mut ev = Event::KeyDown(KeyEvent::from(Key::Right));
            v.on_key(&mut ev);
        }
        // Widest line is 39 columns, the visible width is 19 (20 less the indent).
        assert_eq!(v.h_off_for_test(), 39 - 19);
        // At the end, Right is no longer consumed — it belongs to the pane again.
        let mut ev = Event::KeyDown(KeyEvent::from(Key::Right));
        v.on_key(&mut ev);
        assert!(
            !ev.is_nothing(),
            "nothing left to reveal: the key passes on"
        );
    }

    #[test]
    fn a_block_that_fits_does_not_scroll() {
        let mut v = view();
        v.set_lines(vec!["- short".to_string()]);
        let mut ev = Event::KeyDown(KeyEvent::from(Key::Right));
        v.on_key(&mut ev);
        assert_eq!(v.h_off_for_test(), 0);
        assert!(
            !ev.is_nothing(),
            "a fitting block leaves horizontal keys to the pane"
        );
    }

    #[test]
    fn scroll_home_returns_to_the_left_edge() {
        let mut v = wide_view();
        let mut ev = Event::KeyDown(KeyEvent::from(Key::Right));
        v.on_key(&mut ev);
        assert_eq!(v.h_off_for_test(), 1);
        v.scroll_home();
        assert_eq!(v.h_off_for_test(), 0, "focus landing resets the scroll");
    }

    #[test]
    fn printable_key_requests_activation_and_is_consumed() {
        let mut v = view();
        let mut ev = Event::KeyDown(KeyEvent::from(Key::Char('x')));
        v.on_key(&mut ev);
        assert!(ev.is_nothing(), "action key consumed");
        assert!(v.take_activate());
        assert!(!v.take_activate(), "flag clears after one take");
    }

    #[test]
    fn enter_requests_activation() {
        let mut v = view();
        let mut ev = Event::KeyDown(KeyEvent::from(Key::Enter));
        v.on_key(&mut ev);
        assert!(v.take_activate());
    }

    #[test]
    fn arrow_keys_pass_through_for_field_nav() {
        let mut v = view();
        let mut ev = Event::KeyDown(KeyEvent::from(Key::Down));
        v.on_key(&mut ev);
        assert!(!ev.is_nothing(), "nav key left for the pane");
        assert!(!v.take_activate());
    }
}
