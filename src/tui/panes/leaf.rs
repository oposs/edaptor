//! Leaf list pane: a search box over a ListBox of the current branch's leaves.

use tvision_rs::{
    self as tv, delegate, Context, Event, FieldValue, Group, InputLine, ListBox, Rect, View,
};

use crate::tui::state::profile_for;
use crate::tui::{Shared, GUARD_NAV, REFRESH};

/// A search `InputLine` (row 0) above a `ListBox`. Recomputes rows from the
/// shared state on REFRESH and whenever the search text changes; submits a base
/// read via ReadFlow when the selection moves to a new leaf.
pub(crate) struct LeafPane {
    group: Group,
    search_id: tv::ViewId,
    list_id: tv::ViewId,
    state: Shared,
    last_sel: i32,
    last_search: String,
    seeded: bool,
}

impl LeafPane {
    pub(crate) fn new(bounds: Rect, state: Shared) -> Self {
        let mut group = Group::new(bounds);
        let w = bounds.b.x - bounds.a.x;
        let search = InputLine::with_limit(Rect::new(0, 0, w, 1), 256);
        let search_id = group.insert(Box::new(search));
        let list = ListBox::new(Rect::new(0, 1, w, bounds.b.y - bounds.a.y), 1, None, None);
        let list_id = group.insert(Box::new(list));
        LeafPane {
            group,
            search_id,
            list_id,
            state,
            last_sel: -1,
            last_search: String::new(),
            seeded: false,
        }
    }

    fn repopulate(&mut self, ctx: &mut Context) {
        let rows: Vec<String> = self
            .state
            .borrow()
            .leaf_rows()
            .into_iter()
            .map(|(l, _)| l)
            .collect();
        if let Some(list) = self.group.child_mut(self.list_id) {
            if let Some(lb) = list.as_any_mut().and_then(|a| a.downcast_mut::<ListBox>()) {
                lb.new_list(rows, ctx);
            }
        }
        self.last_sel = -1;
    }

    fn submit_selected(&mut self, ctx: &mut Context) {
        let sel = match self.group.child_mut(self.list_id).and_then(|v| v.value()) {
            Some(FieldValue::Int(i)) => i,
            _ => return,
        };
        if sel == self.last_sel {
            return;
        }
        self.last_sel = sel;

        // Collect dn + objectClasses outside any long-lived borrow.
        let target: Option<(String, Vec<String>)> = {
            let st = self.state.borrow();
            st.leaf_rows().get(sel as usize).map(|(_l, dn)| {
                let ocs = st
                    .structure
                    .get(dn)
                    .map(|n| n.object_classes.clone())
                    .unwrap_or_default();
                (dn.clone(), ocs)
            })
        };
        let Some((dn, ocs)) = target else { return };

        // Check dirty before navigating: if dirty, stash the target and post
        // GUARD_NAV for the dispatch closure to handle.
        let dirty = {
            let st = self.state.borrow();
            st.edit_form.as_ref().map(|f| f.is_dirty()).unwrap_or(false)
        };
        if dirty {
            {
                let mut st = self.state.borrow_mut();
                st.guard_target = Some((dn.clone(), ocs.clone()));
            }
            ctx.post(GUARD_NAV);
            return;
        }

        let mut st = self.state.borrow_mut();
        if st.current_leaf.as_deref() == Some(dn.as_str()) {
            return;
        }
        // Disjoint field borrows: worker (read) + read_flow (mut) + profiles (read).
        let crate::tui::state::UiState {
            worker,
            read_flow,
            profiles,
            current_leaf,
            ..
        } = &mut *st;
        if let Some(w) = worker.as_ref() {
            let profile = profile_for(profiles, &ocs);
            if read_flow.request_entry(w, &dn, profile).is_ok() {
                *current_leaf = Some(dn);
            }
        }
    }
}

#[delegate(to = group)]
impl View for LeafPane {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        let is_refresh = matches!(ev, Event::Broadcast { command, .. } if *command == REFRESH);
        if !self.seeded || (is_refresh && self.state.borrow().list_dirty) {
            self.seeded = true;
            self.repopulate(ctx);
            self.state.borrow_mut().list_dirty = false;
        }

        self.group.handle_event(ev, ctx);

        // Sync search text from the InputLine into shared state; recompute on change.
        let cur = match self.group.child_mut(self.search_id).and_then(|v| v.value()) {
            Some(FieldValue::Text(s)) => s,
            _ => String::new(),
        };
        if cur != self.last_search {
            self.last_search = cur.clone();
            self.state.borrow_mut().search = cur;
            self.repopulate(ctx);
        }

        // Submit a read when the selection lands on a new leaf. The ListBox CONSUMES
        // (clears) Up/Down keys, so we must NOT gate on `ev` still being a KeyDown
        // (it has been cleared by the time we get here). Instead, like the tree pane
        // reads `outline.value()`, `submit_selected` compares the list's `value()` to
        // `last_sel` and is a cheap no-op when the selection is unchanged.
        self.submit_selected(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldap::worker::RawSubschema;
    use crate::schema::SchemaModel;
    use crate::tui::state::UiState;
    use crate::workflows::structure::{Structure, StructureInput};
    use std::cell::RefCell;
    use std::collections::{BTreeMap, VecDeque};
    use std::rc::Rc;

    #[test]
    fn test_leaf_pane_lists_rows_for_selected_branch() {
        let inputs = vec![
            StructureInput {
                dn: "dc=x".into(),
                cn: None,
                description: None,
                object_classes: vec![],
                attrs: BTreeMap::new(),
            },
            StructureInput {
                dn: "ou=p,dc=x".into(),
                cn: None,
                description: None,
                object_classes: vec![],
                attrs: BTreeMap::new(),
            },
            StructureInput {
                dn: "cn=a,ou=p,dc=x".into(),
                cn: Some("a".into()),
                description: None,
                object_classes: vec![],
                attrs: BTreeMap::new(),
            },
            StructureInput {
                dn: "cn=b,ou=p,dc=x".into(),
                cn: Some("b".into()),
                description: None,
                object_classes: vec![],
                attrs: BTreeMap::new(),
            },
        ];
        let structure = Structure::build("dc=x", inputs);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut state =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        state.current_branch = Some("ou=p,dc=x".into());
        let shared: Shared = Rc::new(RefCell::new(state));

        // Two leaves + the ‹self› row = 3 rows expected from leaf_rows.
        assert_eq!(shared.borrow().leaf_rows().len(), 3);

        let mut pane = LeafPane::new(Rect::new(0, 0, 30, 10), shared.clone());

        // Drive one timer/refresh-free event through a headless Context to seed.
        let mut out: VecDeque<Event> = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<tv::Deferred> = Vec::new();
        let mut ctx = tv::Context::new(&mut out, &mut timers, 0, &mut deferred);
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        shared.borrow_mut().list_dirty = true;
        pane.handle_event(&mut ev, &mut ctx);
        // No panic, borrow discipline held; list_dirty cleared.
        assert!(!shared.borrow().list_dirty);
    }

    #[test]
    fn test_leaf_selection_change_detected_when_key_was_consumed() {
        // Regression: the `ListBox` CONSUMES (clears) Up/Down keys, so the leaf pane
        // must detect a selection change from the list's value() — NOT by inspecting
        // the (already-cleared) event. Here the selection moves to row 1, then a
        // non-Up/Down event is delivered (standing in for the consumed key); the pane
        // must still pick up the new selection (last_sel advances to 1).
        let inputs = vec![
            StructureInput {
                dn: "dc=x".into(),
                cn: None,
                description: None,
                object_classes: vec![],
                attrs: BTreeMap::new(),
            },
            StructureInput {
                dn: "ou=p,dc=x".into(),
                cn: None,
                description: None,
                object_classes: vec![],
                attrs: BTreeMap::new(),
            },
            StructureInput {
                dn: "cn=a,ou=p,dc=x".into(),
                cn: Some("a".into()),
                description: None,
                object_classes: vec![],
                attrs: BTreeMap::new(),
            },
            StructureInput {
                dn: "cn=b,ou=p,dc=x".into(),
                cn: Some("b".into()),
                description: None,
                object_classes: vec![],
                attrs: BTreeMap::new(),
            },
        ];
        let structure = Structure::build("dc=x", inputs);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut state =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        state.current_branch = Some("ou=p,dc=x".into());
        let shared: Shared = Rc::new(RefCell::new(state));

        let mut pane = LeafPane::new(Rect::new(0, 0, 30, 10), shared.clone());
        let mut out: VecDeque<Event> = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<tv::Deferred> = Vec::new();
        let mut ctx = tv::Context::new(&mut out, &mut timers, 0, &mut deferred);

        // Seed (initial selection settles on row 0).
        shared.borrow_mut().list_dirty = true;
        let mut seed = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut seed, &mut ctx);
        assert_eq!(pane.last_sel, 0, "seeding selects row 0");

        // Move the list selection to row 1 (as a consumed Up/Down would).
        if let Some(list) = pane.group.child_mut(pane.list_id) {
            list.set_value_ctx(FieldValue::Int(1), &mut ctx);
        }

        // Deliver an event that is NOT a live Up/Down key (the real key was cleared).
        let mut other = Event::Broadcast {
            command: tv::Command::custom("test.noop"),
            source: None,
        };
        pane.handle_event(&mut other, &mut ctx);

        assert_eq!(
            pane.last_sel, 1,
            "leaf pane must detect the new selection from value(), not the cleared key event"
        );
    }
}
