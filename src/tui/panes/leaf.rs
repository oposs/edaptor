//! Leaf list pane: a search box over a ListBox of the current branch's leaves.

use tvision_rs::{
    self as tv, delegate, Context, Event, FieldValue, Group, InputLine, Key, ListBox, Rect, View,
};

use crate::tui::{Shared, REFRESH};

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
        // ofFirstClick: the click that focuses this pane (when another pane was
        // active) must ALSO reach the list, so a single click both focuses and
        // moves the highlight. Without it the parent group consumes the selecting
        // click (group.rs auto-select) and the list only responds to the 2nd click.
        // The tree pane gets this for free by delegating to the Outline (which sets
        // it); a plain Group does not, so set it explicitly.
        group.state_mut().options.first_click = true;
        let w = bounds.b.x - bounds.a.x;
        // grow_mode so Group::change_bounds (driven by the Splitter) resizes children:
        // search bar widens with the pane (stays at row 0, height 1).
        let mut search = InputLine::with_limit(Rect::new(0, 0, w, 1), 256);
        search.state.grow_mode.hi_x = true;
        let search_id = group.insert(Box::new(search));
        // list fills all remaining width + height.
        let mut list = ListBox::new(Rect::new(0, 1, w, bounds.b.y - bounds.a.y), 1, None, None);
        list.state_mut().grow_mode.hi_x = true;
        list.state_mut().grow_mode.hi_y = true;
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

    #[cfg(test)]
    pub(crate) fn search_bounds_for_test(&mut self) -> Rect {
        self.group
            .child_mut(self.search_id)
            .unwrap()
            .state()
            .get_bounds()
    }

    #[cfg(test)]
    pub(crate) fn list_bounds_for_test(&mut self) -> Rect {
        self.group
            .child_mut(self.list_id)
            .unwrap()
            .state()
            .get_bounds()
    }

    /// Pure selector: when the highlight lands on a new row, record the requested
    /// leaf in shared state. The controller (the pump's `reconcile_selection`)
    /// decides whether to load it or raise the dirty guard — the pane never loads,
    /// guards, or posts a command (which is what made this fail under the mouse
    /// capture, where pane-posted commands are swallowed before the app handler).
    fn report_selection(&mut self) {
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
        if let Some((dn, ocs)) = target {
            self.state.borrow_mut().request_leaf(dn, ocs);
        }
    }

    /// Controller → pane: if `set_leaf_row` was set (a guard "Stay" snapping the
    /// highlight back to the pinned form), apply it to the list and sync `last_sel`
    /// so it is not re-reported as a fresh move.
    fn apply_set_row(&mut self, ctx: &mut Context) {
        let row = self.state.borrow_mut().set_leaf_row.take();
        if let Some(row) = row {
            if let Some(list) = self.group.child_mut(self.list_id) {
                list.set_value_ctx(FieldValue::Int(row), ctx);
            }
            self.last_sel = row;
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

        // Arrow/page keys navigate the LIST even while the search box holds text
        // focus (the search-over-list idiom): forward them straight to the list so
        // the user can move the selection while typing a filter. Tab is reserved for
        // switching panes (consumed by the Splitter), so intra-pane navigation uses
        // the arrows — exactly as the tree pane does.
        let nav_key = matches!(
            ev,
            Event::KeyDown(k)
                if matches!(k.key, Key::Up | Key::Down | Key::PageUp | Key::PageDown)
        );
        if nav_key {
            if let Some(list) = self.group.child_mut(self.list_id) {
                list.handle_event(ev, ctx);
            }
        } else {
            self.group.handle_event(ev, ctx);
        }

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

        // First honour any controller-requested highlight (snap-back on guard
        // "Stay"), then report a new highlight. The ListBox CONSUMES (clears)
        // Up/Down keys, so detection compares the list's `value()` to `last_sel`
        // (a cheap no-op when unchanged) rather than inspecting the cleared event.
        self.apply_set_row(ctx);
        self.report_selection();
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

    /// Regression: children must grow via grow_mode when the Splitter drives
    /// Group::change_bounds — NOT via an on_bounds_changed override (which the
    /// framework never calls for Splitter-nested panes).
    ///
    /// TDD evidence: before grow_mode flags were set (hi_x on search, hi_x+hi_y
    /// on list), this test FAILED — children kept their original Rect. After
    /// setting the flags, Group::change_bounds propagates the delta and this PASSES.
    #[test]
    fn grow_mode_resize_fills_pane() {
        let inputs = vec![StructureInput {
            dn: "dc=x".into(),
            cn: None,
            description: None,
            object_classes: vec![],
            attrs: BTreeMap::new(),
        }];
        let structure = Structure::build("dc=x", inputs);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let st = UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        let shared: Shared = Rc::new(RefCell::new(st));
        let mut pane = LeafPane::new(Rect::new(0, 0, 30, 10), shared);
        // Simulate Splitter driving a resize: just change_bounds, no on_bounds_changed.
        <LeafPane as View>::change_bounds(&mut pane, Rect::new(0, 0, 50, 20));
        assert_eq!(
            pane.search_bounds_for_test(),
            Rect::new(0, 0, 50, 1),
            "search InputLine must widen (hi_x)"
        );
        assert_eq!(
            pane.list_bounds_for_test(),
            Rect::new(0, 1, 50, 20),
            "list ListBox must fill width+height (hi_x+hi_y)"
        );
    }

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

    #[test]
    fn arrow_key_navigates_the_list() {
        // Arrows are forwarded to the list (the search box keeps text focus), so a
        // Down advances the list selection and submit_selected loads the new leaf.
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

        // Seed (selection settles on row 0).
        shared.borrow_mut().list_dirty = true;
        let mut seed = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut seed, &mut ctx);
        assert_eq!(pane.last_sel, 0, "seeding selects row 0");

        // A Down key is forwarded to the list (not the search box) and moves the
        // selection, which submit_selected then picks up.
        let mut down = Event::KeyDown(tv::KeyEvent::from(tv::Key::Down));
        pane.handle_event(&mut down, &mut ctx);
        assert_eq!(
            pane.last_sel, 1,
            "Down advances the list selection (forwarded to the list)"
        );
    }
}
