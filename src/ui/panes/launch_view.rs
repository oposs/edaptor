//! Read-only value block for modal ("launch") fields: renders a bulleted list
//! (or `*****` / `<not set>`), highlights as a whole when focused, and reports an
//! activation request when the user presses any action key. Editing happens in
//! the modal the pane opens — this view never mutates values.

use crate::ui::panes::value_lines::VALUE_INDENT;
use tvision_rs::{
    self as tv, DrawCtx, Event, HelpCtx, Key, Point, Rect, Role, SurfaceRoles, View, ViewState,
};

pub(crate) struct LaunchValueView {
    state: ViewState,
    lines: Vec<String>,
    activate: bool,
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
        for (row, line) in self.lines.iter().enumerate() {
            if (row as i32) < size.y {
                ctx.put_str(VALUE_INDENT, row as i32, line, color);
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
