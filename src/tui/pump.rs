//! Zero-area timer view that drains the async LDAP worker into shared state.

use tvision_rs::{self as tv, Context, DrawCtx, Event, View};

use crate::tui::{Shared, REFRESH};

/// Arms a ~20Hz periodic timer on its first event, then drains the worker each
/// tick. `Event::Timer` is broadcast-class in tvision-rs, so this zero-area,
/// never-drawn view still receives every tick.
pub(crate) struct PumpView {
    vs: tv::ViewState,
    state: Shared,
    armed: bool,
}

impl PumpView {
    pub(crate) fn new(state: Shared) -> Self {
        PumpView {
            vs: tv::ViewState::new(tv::Rect::new(0, 0, 0, 0)),
            state,
            armed: false,
        }
    }
}

impl View for PumpView {
    fn state(&self) -> &tv::ViewState {
        &self.vs
    }
    fn state_mut(&mut self) -> &mut tv::ViewState {
        &mut self.vs
    }
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }
    fn draw(&mut self, _ctx: &mut DrawCtx) {}

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        if !self.armed {
            self.armed = true;
            ctx.set_timer(
                std::time::Duration::from_millis(50),
                Some(std::time::Duration::from_millis(50)),
            );
        }
        if matches!(ev, Event::Timer(_)) {
            let r = self.state.borrow_mut().pump_worker();
            // Reconcile a pending leaf selection: load it (clean) or, if the form is
            // dirty, ask the dispatch closure to raise the guard. Posting from the
            // pump's clean top-level tick is reliable — a pane posting the same
            // command is swallowed when a list mouse-track capture is active.
            let need_guard = self.state.borrow_mut().reconcile_selection();
            if r.changed {
                ctx.broadcast(REFRESH, None);
            }
            if need_guard {
                ctx.post(crate::tui::GUARD_NAV);
            }
            if r.error {
                ctx.post(crate::tui::SHOW_ERROR);
            }
            if r.quit {
                ctx.post(tv::Command::QUIT);
            }
        }
    }
}
