//! Profile-chooser dialog: lists profile names for the user to pick one when
//! Alt+N finds more than one matching profile for a container.
//!
//! Pattern mirrors `src/tui/oc_picker.rs`: a `Dialog`-wrapping `View` with
//! `#[delegate(to = dlg)]`, a `ListBox` seeded in `reset_current` (NOT in
//! `new()` — the 2a borrow lesson: never `borrow_mut` shared during
//! construction), and the highlighted index written to
//! `shared.borrow_mut().chosen_profile` in `reset_current` (initial 0) and
//! updated in `handle_event` after nav.

use tvision_rs::{
    self as tv, delegate, ButtonFlags, ButtonRowAlign, Command, Context, Dialog, Event, FieldValue,
    Key, ListBox, Rect, View,
};

use crate::tui::Shared;

/// The profile chooser dialog.
pub struct ProfileChooser {
    dlg: Dialog,
    list_id: tv::ViewId,
    shared: Shared,
    names: Vec<String>,
}

impl ProfileChooser {
    fn new(names: Vec<String>, shared: Shared) -> Self {
        // Height: 1 (frame top) + 1 (frame inner padding) + list rows + 1 (padding) + 2 (buttons) + 1 (frame bottom)
        // Minimum visible list rows = names.len(), clamped to a reasonable max.
        let list_rows = names.len().clamp(3, 16) as i32;
        let height = 1 + 1 + list_rows + 1 + 2 + 1; // ~= 7 + list_rows
        let mut dlg = Dialog::new(
            Rect::new(0, 0, 42, height),
            Some("Choose profile".to_string()),
        );
        dlg.state_mut().options.center_x = true;
        dlg.state_mut().options.center_y = true;

        // List occupies the inner body (y=1 .. 1+list_rows).
        let list = ListBox::new(Rect::new(2, 1, 40, 1 + list_rows), 1, None, None);
        let list_id = dlg.insert_child(Box::new(list));

        dlg.button_row(
            &[
                (
                    "~O~K",
                    Command::OK,
                    ButtonFlags {
                        default: true,
                        ..ButtonFlags::new()
                    },
                ),
                ("~C~ancel", Command::CANCEL, ButtonFlags::new()),
            ],
            ButtonRowAlign::Right,
        );

        ProfileChooser {
            dlg,
            list_id,
            shared,
            names,
        }
    }

    /// Read the current list-highlight index.
    fn current_index(&mut self) -> Option<usize> {
        match self.dlg.child_mut(self.list_id).and_then(|v| v.value()) {
            Some(FieldValue::Int(i)) if i >= 0 => Some(i as usize),
            _ => None,
        }
    }

    /// Write the current highlight index to shared state (take + drop borrow here).
    fn stage_index(&mut self) {
        if let Some(idx) = self.current_index() {
            self.shared.borrow_mut().chosen_profile = Some(idx);
        }
    }
}

#[delegate(to = dlg)]
impl View for ProfileChooser {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    /// Seed the list on first open. `exec_view` calls `reset_current` with a
    /// `Context` right after modal insertion — this is the deterministic hook.
    /// Never call `borrow_mut` on shared during construction; do it here instead.
    fn reset_current(&mut self, ctx: &mut Context) {
        self.dlg.reset_current(ctx);
        // Seed the list unconditionally (idempotent: names don't change).
        let rows: Vec<String> = self.names.clone();
        if let Some(list) = self.dlg.child_mut(self.list_id) {
            if let Some(lb) = list.as_any_mut().and_then(|a| a.downcast_mut::<ListBox>()) {
                lb.new_list(rows, ctx);
            }
        }
        // Write the initial index (0) into shared state.
        self.shared.borrow_mut().chosen_profile = Some(0);
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        let nav = matches!(
            ev,
            Event::KeyDown(k)
                if matches!(k.key, Key::Up | Key::Down | Key::PageUp | Key::PageDown)
        );
        if nav {
            // Forward nav events directly to the list so the highlight moves.
            if let Some(list) = self.dlg.child_mut(self.list_id) {
                list.handle_event(ev, ctx);
            }
            // Update shared state with the new highlight position.
            self.stage_index();
        } else {
            self.dlg.handle_event(ev, ctx);
        }
    }
}

/// Build the profile chooser dialog.
///
/// Returns `(view, list_view_id)` — pass `list_view_id` as the focus target to
/// `exec_view` so keyboard navigation starts on the list immediately.
pub fn build(names: Vec<String>, shared: Shared) -> (Box<dyn View>, tv::ViewId) {
    let chooser = ProfileChooser::new(names, shared);
    let list_id = chooser.list_id;
    (Box::new(chooser), list_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldap::worker::RawSubschema;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn shared() -> Shared {
        use crate::workflows::structure::Structure;
        let schema = crate::schema::SchemaModel::from_raw(&RawSubschema::default());
        let st = crate::tui::state::UiState::new_for_test(
            Structure::build("dc=example,dc=org", vec![]),
            schema,
            "dc=example,dc=org".into(),
            Vec::new(),
            Vec::new(),
        );
        Rc::new(RefCell::new(st))
    }

    fn make_ctx<'a>(
        out: &'a mut std::collections::VecDeque<tv::Event>,
        timers: &'a mut tv::timer::TimerQueue,
        deferred: &'a mut Vec<tv::Deferred>,
    ) -> Context<'a> {
        Context::new(out, timers, 0, deferred)
    }

    /// TDD Step 1 (RED before implementation):
    /// `reset_current` must seed the list AND set `chosen_profile = Some(0)`.
    /// A subsequent `Down` event must update `chosen_profile` to `Some(1)`.
    #[test]
    fn reset_current_seeds_list_and_chosen_profile_down_updates_it() {
        use tvision_rs::{Deferred, KeyEvent};

        let sh = shared();
        let names = vec!["People".to_string(), "Groups".to_string()];
        let (mut view, _list_id) = build(names, sh.clone());

        // Before reset_current: chosen_profile must still be None.
        assert_eq!(
            sh.borrow().chosen_profile,
            None,
            "chosen_profile must be None before reset_current"
        );

        // Fire reset_current (mirrors exec_view's call).
        let mut out = std::collections::VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<Deferred> = Vec::new();
        let mut ctx = make_ctx(&mut out, &mut timers, &mut deferred);
        view.reset_current(&mut ctx);

        // After reset_current: chosen_profile must be Some(0).
        assert_eq!(
            sh.borrow().chosen_profile,
            Some(0),
            "chosen_profile must be Some(0) after reset_current"
        );

        // Downcast to ProfileChooser to verify list length.
        {
            let chooser = view
                .as_any_mut()
                .and_then(|a| a.downcast_mut::<ProfileChooser>())
                .expect("must downcast to ProfileChooser");
            assert_eq!(chooser.names.len(), 2, "two names must be registered");
        }

        // Send a Down key — chosen_profile must become Some(1).
        {
            let chooser = view
                .as_any_mut()
                .and_then(|a| a.downcast_mut::<ProfileChooser>())
                .expect("must downcast to ProfileChooser");
            let mut ev = Event::KeyDown(KeyEvent::from(Key::Down));
            chooser.handle_event(&mut ev, &mut ctx);
        }

        assert_eq!(
            sh.borrow().chosen_profile,
            Some(1),
            "chosen_profile must be Some(1) after Down"
        );
    }

    /// Smoke: `build` must not panic and the returned view must downcast.
    #[test]
    fn build_returns_downcastable_view() {
        let sh = shared();
        let names = vec![
            "Admin".to_string(),
            "Users".to_string(),
            "Groups".to_string(),
        ];
        let (mut view, _list_id) = build(names, sh);
        assert!(
            view.as_any_mut()
                .and_then(|a| a.downcast_mut::<ProfileChooser>())
                .is_some(),
            "view must downcast to ProfileChooser"
        );
    }
}
