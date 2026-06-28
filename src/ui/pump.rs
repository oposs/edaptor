//! Zero-area timer view that drains the async LDAP worker into shared state.

use tvision_rs::{self as tv, Context, DrawCtx, Event, View};

use crate::ui::{Shared, REFRESH};

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
            let need_branch_guard = self.state.borrow_mut().reconcile_branch();
            // A clean branch switch (reconcile_branch) or a post-guard branch switch
            // sets list_dirty without any worker activity (r.changed = false). Broadcast
            // REFRESH so the leaf pane rebuilds. The leaf clears list_dirty on rebuild,
            // so this is a single idempotent refresh per dirty-marking, not a loop.
            let list_dirty = self.state.borrow().list_dirty;
            if r.changed || list_dirty {
                ctx.broadcast(REFRESH, None);
            }
            if need_guard || need_branch_guard {
                ctx.post(crate::ui::GUARD_NAV);
            }
            if r.error {
                ctx.post(crate::ui::SHOW_ERROR);
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

    /// Regression: a clean branch switch sets `list_dirty` but produces no worker
    /// activity (`r.changed = false`). The pump must broadcast `REFRESH` so the leaf
    /// pane rebuilds. Previously the tree pane did this, but after it became a pure
    /// selector (Task 10) nothing replaced that trigger.
    ///
    /// RED (before fix): no `REFRESH` broadcast emitted → leaf stays `<empty>`.
    /// GREEN (after fix): pump broadcasts `REFRESH` whenever `list_dirty` is set.
    #[test]
    fn broadcasts_refresh_on_clean_branch_switch() {
        use std::time::Duration;

        let structure = Structure::build("dc=x", Vec::new());
        let schema = SchemaModel::from_raw(&crate::ldap::worker::RawSubschema::default());
        let mut state = crate::ui::state::UiState::new_for_test(
            structure,
            schema,
            "dc=x".into(),
            Vec::new(),
            Vec::new(),
        );
        // Two-branch setup, currently on p, requesting q; no worker, form is clean.
        state.branch_dns = vec!["ou=p,dc=x".into(), "ou=q,dc=x".into()];
        state.current_branch = Some("ou=p,dc=x".into());
        state.edit_form = None;
        state.request_branch("ou=q,dc=x".into());

        let shared: Shared = Rc::new(RefCell::new(state));
        let mut pump = PumpView::new(shared.clone());

        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<tv::Deferred> = Vec::new();
        // Obtain a valid TimerId by arming a real timer; the pump ignores which id fired.
        let timer_id = timers.set_timer(
            0,
            Duration::from_millis(50),
            Some(Duration::from_millis(50)),
        );
        let mut ctx = headless(&mut out, &mut timers, &mut deferred);
        let mut ev = Event::Timer(timer_id);
        pump.handle_event(&mut ev, &mut ctx);

        // reconcile_branch must have switched the branch.
        assert_eq!(
            shared.borrow().current_branch.as_deref(),
            Some("ou=q,dc=x"),
            "reconcile_branch should switch current_branch"
        );

        // The pump must have broadcast REFRESH so the leaf pane reloads.
        let refresh_count = out
            .iter()
            .filter(|e| matches!(e, Event::Broadcast { command, .. } if *command == REFRESH))
            .count();
        assert_eq!(
            refresh_count, 1,
            "pump must broadcast exactly one REFRESH on a clean branch switch"
        );
    }

    /// The pump posts `Command::FULLSCREEN` exactly once (Off → Desktop); the
    /// window's own handler does the border-drop + cross-tree layout from there.
    #[test]
    fn posts_fullscreen_command_once() {
        let structure = Structure::build("dc=x", Vec::new());
        let schema = SchemaModel::from_raw(&crate::ldap::worker::RawSubschema::default());
        let state = crate::ui::state::UiState::new_for_test(
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

    /// Regression: when BOTH `requested_leaf` and `requested_branch` are pending and
    /// the form is dirty, both `reconcile_selection` and `reconcile_branch` return
    /// `true` in the same tick. Before the fix two `GUARD_NAV` posts were emitted;
    /// after the fix exactly one must be posted (the first guard handles the single
    /// `guard_target` — the second would fire `run_guard` on a cleared `None` target).
    #[test]
    fn guard_nav_posted_at_most_once_when_both_reconciles_dirty() {
        use crate::schema::FieldKind;
        use crate::workflows::edit_form::{EditField, EditForm, FormMode};
        use crate::workflows::form_model::WidgetSpec;
        use std::time::Duration;

        let structure = crate::workflows::structure::Structure::build("dc=x", Vec::new());
        let schema = SchemaModel::from_raw(&crate::ldap::worker::RawSubschema::default());
        let mut state = crate::ui::state::UiState::new_for_test(
            structure,
            schema,
            "dc=x".into(),
            Vec::new(),
            Vec::new(),
        );

        let field = EditField {
            label: "cn".into(),
            must: true,
            editable: true,
            multi: false,
            secret: false,
            ordered: false,
            orphaned: false,
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: None,
            values: vec!["new".into()],
            baseline: vec!["base".into()],
        };
        state.edit_form = Some(EditForm {
            dn: "cn=a,dc=x".into(),
            mode: FormMode::Edit,
            object_classes: vec!["top".into()],
            fields: vec![field],
        });

        state.current_leaf = Some("cn=old,dc=x".into());
        state.current_branch = Some("ou=p,dc=x".into());
        state.branch_dns = vec!["ou=p,dc=x".into(), "ou=q,dc=x".into()];
        state.request_leaf("cn=new,dc=x".into(), vec![]);
        state.request_branch("ou=q,dc=x".into());

        let shared: Shared = Rc::new(RefCell::new(state));
        let mut pump = PumpView::new(shared);

        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<tv::Deferred> = Vec::new();
        let timer_id = timers.set_timer(
            0,
            Duration::from_millis(50),
            Some(Duration::from_millis(50)),
        );
        let mut ctx = headless(&mut out, &mut timers, &mut deferred);
        let mut ev = Event::Timer(timer_id);
        pump.handle_event(&mut ev, &mut ctx);

        let guard_count = out
            .iter()
            .filter(|e| matches!(e, Event::Command(c) if *c == crate::ui::GUARD_NAV))
            .count();
        assert_eq!(
            guard_count,
            1,
            "must post GUARD_NAV at most once even when both reconciles return true; got {guard_count}"
        );
    }
}
