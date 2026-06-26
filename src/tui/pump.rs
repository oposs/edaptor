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
    fullscreen_applied: bool,
}

impl PumpView {
    pub(crate) fn new(state: Shared) -> Self {
        PumpView {
            vs: tv::ViewState::new(tv::Rect::new(0, 0, 0, 0)),
            state,
            armed: false,
            fullscreen_applied: false,
        }
    }

    /// One-shot: switch the base window to frameless fullscreen on the first tick.
    /// We post `Command::FULLSCREEN` (Off → Desktop) rather than calling the API
    /// directly: the window's own `handle_event` owns the border-drop + maximize
    /// (the pump cannot downcast to `Window`), and routing reaches the desktop's
    /// only window. `Desktop` mode keeps the menu bar and status line, just drops
    /// the border/title/inset. Idempotent — posted at most once.
    fn apply_fullscreen_once(&mut self, ctx: &mut Context) {
        if self.fullscreen_applied {
            return;
        }
        ctx.post(tv::Command::FULLSCREEN);
        self.fullscreen_applied = true;
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
            self.apply_fullscreen_once(ctx);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::SchemaModel;
    use crate::workflows::structure::Structure;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    fn headless<'a>(
        out: &'a mut VecDeque<Event>,
        timers: &'a mut tv::timer::TimerQueue,
        deferred: &'a mut Vec<tv::Deferred>,
    ) -> Context<'a> {
        Context::new(out, timers, 0, deferred)
    }

    /// The pump posts `Command::FULLSCREEN` exactly once (Off → Desktop); the
    /// window's own handler does the border-drop + cross-tree layout from there.
    #[test]
    fn posts_fullscreen_command_once() {
        let structure = Structure::build("dc=x", Vec::new());
        let schema = SchemaModel::from_raw(&crate::ldap::worker::RawSubschema::default());
        let state = crate::tui::state::UiState::new_for_test(
            structure,
            schema,
            "dc=x".into(),
            Vec::new(),
            Vec::new(),
        );
        let shared: Shared = Rc::new(RefCell::new(state));
        let mut pump = PumpView::new(shared);

        let count_fullscreen = |out: &VecDeque<Event>| {
            out.iter()
                .filter(|e| matches!(e, Event::Command(c) if *c == tv::Command::FULLSCREEN))
                .count()
        };

        // First call posts the command exactly once.
        {
            let mut out = VecDeque::new();
            let mut timers = tv::timer::TimerQueue::new();
            let mut deferred: Vec<tv::Deferred> = Vec::new();
            let mut ctx = headless(&mut out, &mut timers, &mut deferred);
            pump.apply_fullscreen_once(&mut ctx);
            assert_eq!(count_fullscreen(&out), 1);
        }

        // Idempotent: a second call posts nothing further.
        {
            let mut out = VecDeque::new();
            let mut timers = tv::timer::TimerQueue::new();
            let mut deferred: Vec<tv::Deferred> = Vec::new();
            let mut ctx = headless(&mut out, &mut timers, &mut deferred);
            pump.apply_fullscreen_once(&mut ctx);
            assert_eq!(
                count_fullscreen(&out),
                0,
                "fullscreen is posted at most once"
            );
        }
    }
}
