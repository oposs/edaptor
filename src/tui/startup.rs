//! Pre-TUI startup sequence: resolve the config path (explicit flag / discovery /
//! picker) before the main program connects. The picker runs in its own
//! short-lived tvision `Program` because the main program is built from an
//! already-bootstrapped state — there is no connection yet when the picker shows.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use anyhow::Result;
use tvision_rs::{self as tv, Context, DrawCtx, Event, View};

use crate::tui::dialog::config_picker::{self, PickerItem};

/// Posted once by `PickerTrigger` on the first timer tick; the picker program's
/// `run_app` closure responds by exec-viewing the dialog.
pub(crate) const SHOW_PICKER: tv::Command = tv::Command::custom("edaptor.show_picker");

/// Zero-area view that arms a timer on its first event and posts `SHOW_PICKER`
/// exactly once. Mirrors `pump::PumpView`'s one-shot `FULLSCREEN` post.
struct PickerTrigger {
    vs: tv::ViewState,
    armed: bool,
    posted: bool,
}

impl PickerTrigger {
    fn new() -> Self {
        PickerTrigger {
            vs: tv::ViewState::new(tv::Rect::new(0, 0, 0, 0)),
            armed: false,
            posted: false,
        }
    }

    /// One-shot: post `SHOW_PICKER` the first time this is reached.
    fn post_once(&mut self, ctx: &mut Context) {
        if self.posted {
            return;
        }
        ctx.post(SHOW_PICKER);
        self.posted = true;
    }
}

impl View for PickerTrigger {
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
            self.post_once(ctx);
        }
    }
}

/// Run the config picker in its own short-lived `Program`. Returns the chosen
/// path, or `None` if the user cancelled (caller exits cleanly).
///
/// The main program is built from an already-bootstrapped state, so the picker
/// cannot be a modal inside it. This minimal program's desktop holds only a
/// `PickerTrigger`; on its first tick the trigger posts `SHOW_PICKER`, the
/// `run_app` closure exec-views the dialog, reads the staged index, and ends.
pub fn run_config_picker(items: Vec<PickerItem>) -> Result<Option<PathBuf>> {
    use tvision_rs::{Command, CrosstermBackend, Desktop, Program, Rect, SystemClock, Theme, View};

    let paths: Vec<PathBuf> = items.iter().map(|it| it.path.clone()).collect();
    let cell: Rc<RefCell<Option<usize>>> = Rc::new(RefCell::new(None));
    let chosen: Rc<RefCell<Option<usize>>> = Rc::new(RefCell::new(None));

    let backend = Box::new(CrosstermBackend::new()?);
    let mut program = Program::new(
        backend,
        Box::new(SystemClock::new()),
        Theme::classic_blue(),
        |r: Rect| -> Option<Box<dyn View>> {
            let mut desktop = Desktop::new(r, |br| Some(Desktop::init_background(br)));
            desktop.insert_view(Box::new(PickerTrigger::new()));
            Some(Box::new(desktop))
        },
        |_r| None,
        |_r| None,
    );

    // `items`/`cell` clone move into the closure; `chosen` is cloned out for reading.
    let chosen_w = chosen.clone();
    let mut items_opt = Some(items);
    program.run_app(move |prog, cmd| {
        if cmd == SHOW_PICKER {
            if let Some(items) = items_opt.take() {
                let (view, focus) = config_picker::build(items, cell.clone());
                let answer = prog.exec_view_focused(view, focus);
                if answer == Command::OK {
                    *chosen_w.borrow_mut() = *cell.borrow();
                }
            }
            prog.end_modal(Command::QUIT);
        }
    });

    let idx = *chosen.borrow();
    Ok(idx.and_then(|i| paths.get(i).cloned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn headless<'a>(
        out: &'a mut VecDeque<Event>,
        timers: &'a mut tv::timer::TimerQueue,
        deferred: &'a mut Vec<tv::Deferred>,
    ) -> Context<'a> {
        Context::new(out, timers, 0, deferred)
    }

    /// PickerTrigger posts SHOW_PICKER exactly once, then never again.
    #[test]
    fn picker_trigger_posts_show_picker_once() {
        let mut trigger = PickerTrigger::new();
        let count = |out: &VecDeque<Event>| {
            out.iter()
                .filter(|e| matches!(e, Event::Command(c) if *c == SHOW_PICKER))
                .count()
        };

        {
            let mut out = VecDeque::new();
            let mut timers = tv::timer::TimerQueue::new();
            let mut deferred: Vec<tv::Deferred> = Vec::new();
            let mut ctx = headless(&mut out, &mut timers, &mut deferred);
            trigger.post_once(&mut ctx);
            assert_eq!(count(&out), 1, "first post emits exactly one SHOW_PICKER");
        }
        {
            let mut out = VecDeque::new();
            let mut timers = tv::timer::TimerQueue::new();
            let mut deferred: Vec<tv::Deferred> = Vec::new();
            let mut ctx = headless(&mut out, &mut timers, &mut deferred);
            trigger.post_once(&mut ctx);
            assert_eq!(count(&out), 0, "idempotent: no further posts");
        }
    }
}
