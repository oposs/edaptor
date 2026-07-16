//! Two-row container chooser: when New is invoked *above* a profile's home OU, ask
//! whether to create at the current branch ("Here") or the profile's search_base
//! ("In <home>"). Mirrors `profile_chooser` — a `Dialog`-wrapping `View` with a
//! `ListBox` seeded in `reset_current` (never `borrow_mut` shared during `new`), the
//! highlighted row written to `shared.chosen_container` (0 = here, 1 = home).

use tvision_rs::{
    self as tv, delegate, ButtonFlags, ButtonRowAlign, Command, Context, Dialog, Event, FieldValue,
    Key, ListBox, Rect, View,
};

use crate::ui::Shared;

/// The container chooser dialog.
pub struct ContainerChooser {
    dlg: Dialog,
    list_id: tv::ViewId,
    shared: Shared,
    rows: Vec<String>,
}

impl ContainerChooser {
    fn new(here: String, home: String, shared: Shared) -> Self {
        let rows = vec![format!("Here — {here}"), format!("In {home}")];
        let list_rows = rows.len() as i32; // 2
        let height = 1 + 1 + list_rows + 1 + 2 + 1; // frame + list + pad + buttons
        let width = 64;
        let mut dlg = Dialog::new(
            Rect::new(0, 0, width, height),
            Some("Create where?".to_string()),
        );
        dlg.state_mut().options.center_x = true;
        dlg.state_mut().options.center_y = true;

        let list = ListBox::new(Rect::new(2, 1, width - 2, 1 + list_rows), 1, None, None);
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

        ContainerChooser {
            dlg,
            list_id,
            shared,
            rows,
        }
    }

    fn current_index(&mut self) -> Option<usize> {
        match self.dlg.child_mut(self.list_id).and_then(|v| v.value()) {
            Some(FieldValue::Int(i)) if i >= 0 => Some(i as usize),
            _ => None,
        }
    }

    fn stage_index(&mut self) {
        if let Some(idx) = self.current_index() {
            self.shared.borrow_mut().chosen_container = Some(idx);
        }
    }
}

#[delegate(to = dlg)]
impl View for ContainerChooser {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    fn reset_current(&mut self, ctx: &mut Context) {
        self.dlg.reset_current(ctx);
        let rows = self.rows.clone();
        if let Some(list) = self.dlg.child_mut(self.list_id) {
            if let Some(lb) = list.as_any_mut().and_then(|a| a.downcast_mut::<ListBox>()) {
                lb.new_list(rows, ctx);
            }
        }
        self.shared.borrow_mut().chosen_container = Some(0);
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        let nav = matches!(
            ev,
            Event::KeyDown(k)
                if matches!(k.key, Key::Up | Key::Down | Key::PageUp | Key::PageDown)
        );
        if nav {
            if let Some(list) = self.dlg.child_mut(self.list_id) {
                list.handle_event(ev, ctx);
            }
        } else {
            self.dlg.handle_event(ev, ctx);
        }
        // Stage regardless of branch: mouse clicks routed through the else-branch
        // also move the ListBox highlight and must be reflected in shared state.
        self.stage_index();
    }
}

/// Build the container chooser. Returns `(view, list_view_id)` — pass the id as the
/// focus target to `exec_view_focused` so nav starts on the list.
pub fn build(here: String, home: String, shared: Shared) -> (Box<dyn View>, tv::ViewId) {
    let chooser = ContainerChooser::new(here, home, shared);
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
        let st = crate::ui::state::UiState::new_for_test(
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

    #[test]
    fn reset_current_sets_chosen_container_zero_then_down_updates() {
        use tvision_rs::{Deferred, KeyEvent};
        let sh = shared();
        let (mut view, _id) = build(
            "dc=example,dc=org".into(),
            "ou=people,dc=example,dc=org".into(),
            sh.clone(),
        );
        assert_eq!(sh.borrow().chosen_container, None);

        let mut out = std::collections::VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<Deferred> = Vec::new();
        let mut ctx = make_ctx(&mut out, &mut timers, &mut deferred);
        view.reset_current(&mut ctx);
        assert_eq!(sh.borrow().chosen_container, Some(0));

        let chooser = view
            .as_any_mut()
            .and_then(|a| a.downcast_mut::<ContainerChooser>())
            .expect("downcast");
        let mut ev = Event::KeyDown(KeyEvent::from(Key::Down));
        chooser.handle_event(&mut ev, &mut ctx);
        assert_eq!(sh.borrow().chosen_container, Some(1));
    }

    /// TDD (review follow-up): a mouse click on the second row must stage it,
    /// not just Up/Down keyboard nav. Before the fix, `handle_event`'s `else`
    /// branch forwarded the click to `self.dlg` (which moves the `ListBox`
    /// highlight) but never called `stage_index()`, so `chosen_container`
    /// stayed at the `reset_current` default (`Some(0)`).
    ///
    /// Coordinates: the dialog is never inserted into an owning `Group` in
    /// this harness, so `center_x`/`center_y` never fire and the dialog's own
    /// bounds stay exactly `Rect::new(0, 0, width, height)` as constructed —
    /// no desktop/layout pass needed to make screen coords deterministic. The
    /// `ListBox` sits at `Rect::new(2, 1, width - 2, 1 + list_rows)` (dialog-
    /// local coordinates, per `ListViewerState::new`, which reads size/origin
    /// straight from the constructor `Rect` with no draw pass required).
    /// `Group::deliver` subtracts the child's origin before forwarding a
    /// positional event, so a `MouseDown` at dialog-local `(3, 2)` becomes
    /// list-local `(1, 1)`, which `list_viewer::handle_event` maps to
    /// `new_item = mouse.y + size.y * (mouse.x / col_width) + top_item = 1`
    /// — the second row.
    #[test]
    fn mouse_click_on_second_row_stages_it_without_nav() {
        use tvision_rs::Deferred;
        let sh = shared();
        let (mut view, _id) = build(
            "dc=example,dc=org".into(),
            "ou=people,dc=example,dc=org".into(),
            sh.clone(),
        );

        let mut out = std::collections::VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<Deferred> = Vec::new();
        let mut ctx = make_ctx(&mut out, &mut timers, &mut deferred);
        view.reset_current(&mut ctx);
        assert_eq!(sh.borrow().chosen_container, Some(0));

        let chooser = view
            .as_any_mut()
            .and_then(|a| a.downcast_mut::<ContainerChooser>())
            .expect("downcast");

        let mut ev = Event::MouseDown(tv::MouseEvent {
            position: tv::Point::new(3, 2),
            ..Default::default()
        });
        chooser.handle_event(&mut ev, &mut ctx);

        assert_eq!(
            sh.borrow().chosen_container,
            Some(1),
            "a mouse click on the second row must stage it, matching the moved highlight"
        );
    }
}
