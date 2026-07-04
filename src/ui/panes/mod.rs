pub(crate) mod field_label;
pub(crate) mod form;
pub(crate) mod launch_view;
pub(crate) mod leaf;
pub(crate) mod list_model;
pub(crate) mod list_view;
pub(crate) mod tree;
pub(crate) mod value_lines;

use tvision_rs::{Event, ViewState};

/// True when `ev` is a mouse wheel whose (pane-local) position falls OUTSIDE the
/// pane's own extent — i.e. the cursor is over a *different* pane.
///
/// tvision delivers `MouseWheel` **non-positionally**: the splitter's group
/// offers it to each pane in reverse order until one consumes it (`route_event`'s
/// `MouseWheel` arm), so without an explicit cursor test whichever pane grabs the
/// wheel first scrolls regardless of where the pointer is. Each pane calls this
/// at the top of `handle_event` and returns — leaving the event unconsumed —
/// when it is true, so the wheel propagates to the sibling actually under the
/// cursor. The event position is already pane-local (`Group::deliver` subtracts
/// the child's origin before delivery), so it is tested against the 0-based
/// `get_extent()`.
pub(crate) fn wheel_misses_pane(state: &ViewState, ev: &Event) -> bool {
    match ev {
        Event::MouseWheel(me) => !state.get_extent().contains(me.position),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tvision_rs::event::{MouseEvent, MouseWheel};
    use tvision_rs::{KeyEvent, Point, Rect, ViewState};

    fn state_10x5() -> ViewState {
        ViewState::new(Rect::new(0, 0, 10, 5)) // extent 0..10 × 0..5
    }

    fn wheel_at(x: i32, y: i32) -> Event {
        Event::MouseWheel(MouseEvent {
            position: Point::new(x, y),
            wheel: MouseWheel::Down,
            ..Default::default()
        })
    }

    #[test]
    fn wheel_inside_extent_is_for_this_pane() {
        assert!(!wheel_misses_pane(&state_10x5(), &wheel_at(3, 2)));
        // Boundary: the top-left corner is inside, the bottom-right corner is not.
        assert!(!wheel_misses_pane(&state_10x5(), &wheel_at(0, 0)));
        assert!(wheel_misses_pane(&state_10x5(), &wheel_at(10, 5)));
    }

    #[test]
    fn wheel_outside_extent_misses_this_pane() {
        // Cursor over a sibling to the left (negative local x) or below/right.
        assert!(wheel_misses_pane(&state_10x5(), &wheel_at(-1, 2)));
        assert!(wheel_misses_pane(&state_10x5(), &wheel_at(20, 2)));
        assert!(wheel_misses_pane(&state_10x5(), &wheel_at(3, 9)));
    }

    #[test]
    fn non_wheel_events_are_never_treated_as_misses() {
        // A key event must pass the gate (return false) regardless of geometry,
        // so the pane keeps handling its keys.
        let key = Event::KeyDown(KeyEvent::from(tvision_rs::Key::Down));
        assert!(!wheel_misses_pane(&state_10x5(), &key));
    }
}
