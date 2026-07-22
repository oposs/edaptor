//! Leaf list pane: a `ListBox` of the current branch's leaves with the list's
//! own incremental find (`FindMode::Highlight`) — type while it is focused to
//! filter and highlight, no separate search box. While a query is active, the
//! rows come from a live one-level search under the branch (see
//! `UiState::set_leaf_search`), not from the cached projection — an entry
//! another client just created is findable without a restart.

use tvision_rs::{
    self as tv, delegate, Context, Event, FieldValue, FindMode, Group, Key, ListBox, ListViewer,
    Rect, ScrollBar, View,
};

use crate::ui::state::HighlightPlan;
use crate::ui::{Shared, REFRESH};

/// A `ListBox` (with incremental find) of the current branch's leaves. Recomputes
/// rows from the shared state on REFRESH and whenever the find query changes;
/// submits a base read via ReadFlow when the selection moves to a new leaf.
pub(crate) struct LeafPane {
    group: Group,
    list_id: tv::ViewId,
    /// Vertical scroll bar in the right column. Wired as the list's `v_bar`, so
    /// the list widget publishes its range/value/page; this pane owns only the
    /// focus + overflow visibility gate and the matching width reflow (Task 7).
    v_bar: tv::ViewId,
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
        let h = bounds.b.y - bounds.a.y;
        // Vertical scroll bar in the right column (width 1 ⇒ vertical, which
        // ScrollBar::new detects and gives the right grow_mode). Hidden until the
        // pane is focused AND the list overflows — sync_scrollbar() owns that
        // gate and the matching one-column width reflow.
        let mut v_bar = ScrollBar::new(Rect::new(w - 1, 0, w, h));
        v_bar.state_mut().state.visible = false;
        let v_bar = group.insert(Box::new(v_bar));
        // List fills the pane height and — while the bar is hidden — the full
        // width. `FindMode::Highlight` enables type-to-find: typing accumulates a
        // query and highlights matches; the pane supplies the already-filtered
        // rows (see `handle_event`). It is wired as the list's vertical bar, so
        // the list widget publishes the bar's range/value/page itself on every
        // (re)population and a bar drag scrolls the list; this pane only gates
        // visibility + width.
        let mut list = ListBox::new(Rect::new(0, 0, w, h), 1, None, Some(v_bar))
            .with_find(FindMode::Highlight);
        list.state_mut().grow_mode.hi_x = true;
        list.state_mut().grow_mode.hi_y = true;
        let list_id = group.insert(Box::new(list));
        LeafPane {
            group,
            list_id,
            v_bar,
            state,
            last_sel: -1,
            last_search: String::new(),
            seeded: false,
        }
    }

    fn repopulate(&mut self, ctx: &mut Context) {
        // Snapshot the shared search first (borrow dropped before any ctx call).
        let search = self.state.borrow().search.clone();
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
                // Keep the list's own incremental find query in step with the
                // shared search string. Tree navigation clears `state.search`
                // (see `UiState::commit_branch`); drop the now-stale find query
                // so the new branch's leaves are shown unfiltered instead of
                // staying narrowed by the previous branch's search.
                if search.is_empty() && lb.find_query().is_some() {
                    lb.clear_find(ctx);
                }
            }
        }
        self.last_search = search;
        self.apply_highlight_plan(ctx);
        self.sync_scrollbar(ctx);
    }

    /// Resolve the controller's [`HighlightPlan`] against the rows just installed,
    /// set the widget focus, and resync `last_sel` **silently** so the move is not
    /// re-reported as an operator navigation. `Follow` additionally asks the
    /// controller to move the form, which it only ever does for a clean form.
    fn apply_highlight_plan(&mut self, ctx: &mut Context) {
        let (plan, rows) = {
            let st = self.state.borrow();
            (st.leaf_highlight_plan(), st.leaf_rows())
        };
        let dn = match &plan {
            HighlightPlan::Pin(dn) | HighlightPlan::Follow(dn) => dn.clone(),
            HighlightPlan::Clear => {
                self.last_sel = -1;
                return;
            }
        };
        let row = rows
            .iter()
            .position(|(_, d)| d.eq_ignore_ascii_case(&dn))
            .map(|i| i as i32)
            .unwrap_or(-1);
        if row >= 0 {
            if let Some(list) = self.group.child_mut(self.list_id) {
                list.set_value_ctx(FieldValue::Int(row), ctx);
            }
        }
        // Silently: `report_selection` compares against `last_sel`, so syncing it
        // here is what makes the rebuild invisible to the controller.
        self.last_sel = row;
        if let HighlightPlan::Follow(dn) = plan {
            let ocs = {
                let st = self.state.borrow();
                st.structure
                    .get(&dn)
                    .map(|n| n.object_classes.clone())
                    .unwrap_or_default()
            };
            self.state.borrow_mut().request_leaf(dn, ocs);
        }
    }

    /// Pure layout decision for the focus-gated scroll bar. Given the list's
    /// total `rows`, the visible `page` (rows that fit), whether the pane is
    /// `active`, and the pane width `w`, decide whether the bar is shown and how
    /// wide the list should be. The bar is shown iff the pane is active AND the
    /// content overflows the page; when shown the list yields one column to the
    /// bar, otherwise it spans the full width. Kept side-effect free so the
    /// behaviour is unit-testable (mirrors scroll_group.rs's test-seam style).
    fn bar_layout(rows: i32, page: i32, active: bool, w: i32) -> (bool, i32) {
        let overflow = rows > page;
        let show = active && overflow;
        let list_w = if show { (w - 1).max(0) } else { w };
        (show, list_w)
    }

    /// Focus + overflow gate for the scroll bar, plus the one-column width
    /// reflow. The wired list already publishes the bar's range/value/page, so
    /// this never touches scroll params — it only decides visibility/geometry.
    ///
    /// The list's own `set_state` toggles the bar visible on `active` alone
    /// (overflow-blind) via a deferred `SetVisible`; we re-assert the
    /// overflow-aware decision through `request_set_visible` so a focused but
    /// non-overflowing list still hides the bar (later deferred op wins).
    fn sync_scrollbar(&mut self, ctx: &mut Context) {
        let active = self.group.state().state.active;
        let extent = self.group.state().get_extent();
        let (w, h) = (extent.b.x, extent.b.y);
        // The list occupies the full pane height, so its visible page height is h.
        let page = h.max(0);
        let rows = self
            .group
            .child_mut(self.list_id)
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<ListBox>())
            .map(|lb| lb.list().len() as i32)
            .unwrap_or(0);
        let (show, list_w) = Self::bar_layout(rows, page, active, w);
        // Reserve / reclaim the bar lane.
        if let Some(list) = self.group.child_mut(self.list_id) {
            list.change_bounds(Rect::new(0, 0, list_w, h));
        }
        // Pin the bar to the right column (idempotent with its grow_mode) and
        // toggle its visibility directly for the steady state.
        if let Some(bar) = self.group.child_mut(self.v_bar) {
            bar.change_bounds(Rect::new(w - 1, 0, w, h));
            bar.state_mut().state.visible = show;
        }
        // Compete with the list's overflow-blind SetVisible in the deferred drain.
        ctx.request_set_visible(self.v_bar, show);
    }

    #[cfg(test)]
    pub(crate) fn list_bounds_for_test(&mut self) -> Rect {
        self.group
            .child_mut(self.list_id)
            .unwrap()
            .state()
            .get_bounds()
    }

    #[cfg(test)]
    pub(crate) fn list_text_for_test(&mut self) -> Vec<String> {
        self.group
            .child_mut(self.list_id)
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<ListBox>())
            .map(|lb| lb.list().to_vec())
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub(crate) fn find_query_for_test(&mut self) -> Option<String> {
        self.group
            .child_mut(self.list_id)
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<ListBox>())
            .and_then(|lb| lb.find_query().map(str::to_string))
    }

    #[cfg(test)]
    pub(crate) fn selected_row_for_test(&mut self) -> i32 {
        match self.group.child_mut(self.list_id).and_then(|v| v.value()) {
            Some(FieldValue::Int(i)) => i,
            _ => -1,
        }
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
}

#[delegate(to = group)]
impl View for LeafPane {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        // Only scroll on the wheel when the cursor is over this pane — tvision
        // delivers the wheel non-positionally, so otherwise the inner list would
        // grab a wheel meant for a sibling pane. Left unconsumed, it propagates.
        if super::wheel_misses_pane(self.group.state(), ev) {
            return;
        }
        let is_refresh = matches!(ev, Event::Broadcast { command, .. } if *command == REFRESH);
        let first_seed = !self.seeded;
        if first_seed || (is_refresh && self.state.borrow().list_dirty) {
            self.seeded = true;
            self.repopulate(ctx);
            self.state.borrow_mut().list_dirty = false;
        }
        if first_seed {
            // Make the list the group's current child so arrows and type-to-find
            // reach it once the pane gains focus. The pump's currency settle does
            // this in the running app; do it deterministically here too (there is
            // no longer a search box competing for the pane's open-time focus).
            self.group.focus_child(self.list_id, ctx);
        }

        // Tab is reserved for switching panes. Do not let the inner group consume
        // it for intra-pane focus cycling — return without clearing so the parent
        // Splitter receives it and moves to the next pane.
        if matches!(ev, Event::KeyDown(k) if k.key == Key::Tab) {
            return;
        }

        // Delegate: the focused ListBox handles arrow/page navigation and, via its
        // find mode, accumulates the type-to-find query (Backspace widens, Esc
        // clears) and broadcasts `Command::LIST_FIND_CHANGED`.
        self.group.handle_event(ev, ctx);

        // A find-query edit (the list broadcasts LIST_FIND_CHANGED on change):
        // submit it to shared state, which answers from the directory (or falls
        // back to the cached projection while the search is in flight) and
        // recomputes the rows. `Highlight` mode does not self-filter — the pane
        // supplies the already-filtered rows (`leaf_rows()`), keeping the list
        // index aligned 1:1 with `leaf_rows()` so the selection→DN mapping stays
        // correct, while the list highlights the matched substring.
        let cur = self
            .group
            .child_mut(self.list_id)
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<ListBox>())
            .and_then(|lb| lb.find_query().map(str::to_string))
            .unwrap_or_default();
        if cur != self.last_search {
            self.last_search = cur.clone();
            // Answer the find from the directory (superseding any in-flight one),
            // never from the cached projection: an entry another client created must
            // be findable without a restart.
            self.state.borrow_mut().set_leaf_search(cur);
            self.repopulate(ctx);
        }

        // Report a new highlight. The ListBox CONSUMES (clears) Up/Down keys, so
        // detection compares the list's `value()` to `last_sel` (a cheap no-op
        // when unchanged) rather than inspecting the cleared event.
        self.report_selection();

        // Re-evaluate the bar's focus + overflow gate now that the list length
        // and the pane's active state are current for this frame.
        self.sync_scrollbar(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldap::worker::RawSubschema;
    use crate::schema::SchemaModel;
    use crate::ui::state::UiState;
    use crate::workflows::structure::{Structure, StructureInput};
    use std::cell::RefCell;
    use std::collections::{BTreeMap, VecDeque};
    use std::rc::Rc;

    /// A `StructureInput` for `dn`, with the RDN value as its `cn` so the row
    /// renders a label. Note: `state.rs`'s test module has a two-argument `si`;
    /// this one is local to this file.
    fn si(dn: &str) -> StructureInput {
        let cn = dn.split('=').nth(1).and_then(|s| s.split(',').next());
        StructureInput {
            dn: dn.into(),
            cn: cn.map(str::to_string),
            description: None,
            object_classes: vec![],
            attrs: BTreeMap::new(),
        }
    }

    /// Shared state on branch `ou=p,dc=x` holding `dns` as its leaves.
    fn shared_with_rows(dns: &[&str]) -> Shared {
        let mut inputs = vec![si("dc=x"), si("ou=p,dc=x")];
        inputs.extend(dns.iter().map(|d| si(d)));
        let structure = Structure::build("dc=x", inputs);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.current_branch = Some("ou=p,dc=x".into());
        st.list_dirty = true;
        Rc::new(RefCell::new(st))
    }

    /// Drive one `REFRESH` broadcast through the pane.
    fn refresh(pane: &mut LeafPane) {
        let mut out: VecDeque<Event> = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<tv::Deferred> = Vec::new();
        let mut ctx = tv::Context::new(&mut out, &mut timers, 0, &mut deferred);
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut ev, &mut ctx);
    }

    /// Regression: children must grow via grow_mode when the Splitter drives
    /// Group::change_bounds — NOT via an on_bounds_changed override (which the
    /// framework never calls for Splitter-nested panes).
    ///
    /// TDD evidence: before grow_mode flags were set (hi_x+hi_y on the list),
    /// this test FAILED — the list kept its original Rect. After setting the
    /// flags, Group::change_bounds propagates the delta and this PASSES.
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
        // The list fills the whole pane (no search row above it any more) and its
        // hi_x+hi_y grow_mode tracks the pane's bottom-right.
        assert_eq!(
            pane.list_bounds_for_test(),
            Rect::new(0, 0, 50, 20),
            "list ListBox must fill width+height (hi_x+hi_y)"
        );
    }

    /// Pure focus + overflow gate (Task 7). The bar shows only when the pane is
    /// active AND the rows overflow the page; when shown the list yields one
    /// column, otherwise it keeps the full width.
    #[test]
    fn bar_layout_gates_on_focus_and_overflow() {
        // Active + overflow → bar shown, list narrowed by one column.
        assert_eq!(LeafPane::bar_layout(20, 9, true, 30), (true, 29));
        // Active but fits exactly (rows == page) → no overflow → hidden, full width.
        assert_eq!(LeafPane::bar_layout(9, 9, true, 30), (false, 30));
        // Overflow but pane not active → hidden, list reclaims full width.
        assert_eq!(LeafPane::bar_layout(20, 9, false, 30), (false, 30));
        // Empty list while active → hidden, full width.
        assert_eq!(LeafPane::bar_layout(0, 9, true, 30), (false, 30));
        // Degenerate width clamps to 0 rather than going negative.
        assert_eq!(LeafPane::bar_layout(20, 9, true, 0), (true, 0));
    }

    /// Focus visualization: the leaf `ListViewer` keys its row *surface* on
    /// `ctx.owner_active()` (the pane group's focus, fanned by `Group::draw`) and
    /// its current-row *highlight* on the list's own `state.focused`. So a focused
    /// leaf renders the active surface with the blue cursor; an unfocused leaf
    /// recedes to the inactive surface and its current row shows the faded-blue
    /// selected colour — no pane-side focus mirroring needed (the framework drives
    /// both axes). The helper sets the pane group's *and* the current list's
    /// `focused` together, mirroring the real focus chain.
    #[test]
    fn unfocused_leaf_recedes_and_current_row_fades() {
        use tvision_rs::{Buffer, DrawCtx, Point, Role};

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

        // Seed the list (‹self› + two leaves); no current_leaf, so the highlight
        // plan settles currency on row 1, the first real leaf (row 0 is ‹self›).
        shared.borrow_mut().list_dirty = true;
        let mut seed = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut seed, &mut ctx);
        assert_eq!(pane.last_sel, 1, "seeding settles on the first real row");

        let theme = crate::ui::theme::edaptor_theme();
        let active_bg = theme.style(Role::ListNormal).bg;
        let inactive_bg = theme.style(Role::ListInactive).bg;
        let cursor_bg = theme.style(Role::ListFocused).bg;
        let faded_bg = theme.style(Role::ListSelected).bg;
        assert_ne!(
            active_bg, inactive_bg,
            "test premise: active vs inactive surfaces must differ"
        );

        // Helper: draw the pane with the given focus and read back cell backgrounds.
        let draw_bgs = |pane: &mut LeafPane, focused: bool| -> (tv::Color, tv::Color) {
            pane.group.state_mut().state.focused = focused;
            // The current list is the pane group's focused child on the real focus
            // chain; mirror that so its own `state.focused` (the highlight axis)
            // tracks the pane, as the running app's focus propagation would.
            let list_id = pane.list_id;
            if let Some(list) = pane.group.child_mut(list_id) {
                list.state_mut().state.focused = focused;
            }
            let mut buf = Buffer::new(30, 10);
            {
                let mut dctx =
                    DrawCtx::new(&mut buf, &theme, Rect::new(0, 0, 30, 10), Point::new(0, 0));
                <LeafPane as View>::draw(pane, &mut dctx);
            }
            // Row 1 is the current row; row 0 (‹self›) is a normal content row.
            (buf.get(0, 1).style().bg, buf.get(0, 0).style().bg)
        };

        let (cur_focused, row_focused) = draw_bgs(&mut pane, true);
        assert_eq!(
            row_focused, active_bg,
            "focused leaf: normal rows use the active (bright) surface"
        );
        assert_eq!(
            cur_focused, cursor_bg,
            "focused leaf: the current row uses the blue cursor colour"
        );

        let (cur_unfocused, row_unfocused) = draw_bgs(&mut pane, false);
        assert_eq!(
            row_unfocused, inactive_bg,
            "unfocused leaf: normal rows recede to the inactive (darker) surface"
        );
        assert_eq!(
            cur_unfocused, faded_bg,
            "unfocused leaf: the current row shows the faded-blue selected colour"
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
        // the (already-cleared) event. Here the selection moves to row 2, then a
        // non-Up/Down event is delivered (standing in for the consumed key); the pane
        // must still pick up the new selection (last_sel advances to 2).
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

        // Seed (no current_leaf, so the highlight plan skips the ‹self› row and
        // settles on the first real entry, row 1).
        shared.borrow_mut().list_dirty = true;
        let mut seed = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut seed, &mut ctx);
        assert_eq!(pane.last_sel, 1, "seeding settles on the first real row");

        // Move the list selection to row 2 (as a consumed Up/Down would).
        if let Some(list) = pane.group.child_mut(pane.list_id) {
            list.set_value_ctx(FieldValue::Int(2), &mut ctx);
        }

        // Deliver an event that is NOT a live Up/Down key (the real key was cleared).
        let mut other = Event::Broadcast {
            command: tv::Command::custom("test.noop"),
            source: None,
        };
        pane.handle_event(&mut other, &mut ctx);

        assert_eq!(
            pane.last_sel, 2,
            "leaf pane must detect the new selection from value(), not the cleared key event"
        );
    }

    #[test]
    fn arrow_key_navigates_the_list() {
        // The list is the focused child, so a Down advances the list selection and
        // submit_selected loads the new leaf.
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

        // Seed (no current_leaf, so the highlight plan settles on the first real
        // row instead of the ‹self› row).
        shared.borrow_mut().list_dirty = true;
        let mut seed = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut seed, &mut ctx);
        assert_eq!(pane.last_sel, 1, "seeding settles on the first real row");

        // A Down key reaches the focused list and moves the selection, which
        // submit_selected then picks up.
        let mut down = Event::KeyDown(tv::KeyEvent::from(tv::Key::Down));
        pane.handle_event(&mut down, &mut ctx);
        assert_eq!(
            pane.last_sel, 2,
            "Down advances the list selection (forwarded to the list)"
        );
    }

    #[test]
    fn typing_a_find_query_filters_the_leaf_rows() {
        // Branch ou=p has two leaves, cn=a and cn=b. Typing "a" into the focused
        // list accumulates the find query, which the pane mirrors into
        // `state.search` and uses to narrow `leaf_rows()` — so the displayed rows
        // become the ‹self› row plus the matching leaf "a" (label "b" drops out).
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

        shared.borrow_mut().list_dirty = true;
        let mut seed = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut seed, &mut ctx);
        assert_eq!(
            pane.list_text_for_test().len(),
            3,
            "unfiltered: ‹self› + two leaves"
        );

        // Type "a" into the focused list (its find mode captures the key).
        let mut k = Event::KeyDown(tv::KeyEvent::from(tv::Key::Char('a')));
        pane.handle_event(&mut k, &mut ctx);

        assert_eq!(shared.borrow().search, "a", "find query mirrors into state");
        let rows = pane.list_text_for_test();
        assert_eq!(
            rows,
            vec!["a".to_string()],
            "only the matching leaf remains ('b' and the non-matching ‹self› row \
             are filtered out), got {rows:?}"
        );

        // A query that matches nothing leaves the list empty, so the ListBox's
        // find mode renders the "No match: <query>" placeholder (drawn when the
        // view is empty and a find query is active).
        for ch in ['z', 'z', 'z'] {
            let mut k = Event::KeyDown(tv::KeyEvent::from(tv::Key::Char(ch)));
            pane.handle_event(&mut k, &mut ctx);
        }
        assert_eq!(shared.borrow().search, "azzz", "query accumulates");
        assert!(
            pane.list_text_for_test().is_empty(),
            "a non-matching query leaves zero rows so 'No match' can render"
        );
    }

    #[test]
    fn navigating_the_tree_resets_the_leaf_find_query() {
        // Two sibling branches, each with two leaves. Type a find query while
        // ou=p is shown, then navigate to ou=q (as the tree pane does, via
        // `commit_branch`) and drive the REFRESH. The leaf list must forget the
        // previous branch's search: `state.search` clears, the ListBox find
        // query clears, and ou=q's leaves list unfiltered.
        let leaf = |dn: &str, cn: &str| StructureInput {
            dn: dn.into(),
            cn: Some(cn.into()),
            description: None,
            object_classes: vec![],
            attrs: BTreeMap::new(),
        };
        let branch = |dn: &str| StructureInput {
            dn: dn.into(),
            cn: None,
            description: None,
            object_classes: vec![],
            attrs: BTreeMap::new(),
        };
        let inputs = vec![
            branch("dc=x"),
            branch("ou=p,dc=x"),
            leaf("cn=alice,ou=p,dc=x", "alice"),
            leaf("cn=bob,ou=p,dc=x", "bob"),
            branch("ou=q,dc=x"),
            leaf("cn=carol,ou=q,dc=x", "carol"),
            leaf("cn=dave,ou=q,dc=x", "dave"),
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

        shared.borrow_mut().list_dirty = true;
        let mut seed = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut seed, &mut ctx);

        // Narrow ou=p to just "alice".
        let mut k = Event::KeyDown(tv::KeyEvent::from(tv::Key::Char('a')));
        pane.handle_event(&mut k, &mut ctx);
        let mut k = Event::KeyDown(tv::KeyEvent::from(tv::Key::Char('l')));
        pane.handle_event(&mut k, &mut ctx);
        assert_eq!(shared.borrow().search, "al", "find query is active on ou=p");
        assert_eq!(pane.find_query_for_test().as_deref(), Some("al"));

        // Navigate the tree to ou=q (clean form → immediate commit).
        shared.borrow_mut().commit_branch("ou=q,dc=x".into());
        let mut refresh = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut refresh, &mut ctx);

        assert_eq!(
            shared.borrow().search,
            "",
            "tree navigation clears the shared leaf search"
        );
        assert_eq!(
            pane.find_query_for_test(),
            None,
            "the ListBox's own incremental find query is dropped too"
        );
        // ‹self› row + both leaves — unfiltered, not narrowed by ou=p's "al".
        let rows = pane.list_text_for_test();
        assert_eq!(rows.len(), 3, "‹self› + two ou=q leaves, got {rows:?}");
        assert!(
            rows.contains(&"carol".to_string()) && rows.contains(&"dave".to_string()),
            "both ou=q leaves are listed, got {rows:?}"
        );
    }

    /// I4 regression, pane level. `adopt_structure` rebuilds the leaf list from
    /// scratch: `repopulate` resets `last_sel` to -1 and `ListBox::new_list`
    /// re-focuses row 0 — the ‹self› container row. Without a snap-back
    /// (`set_leaf_row`), the next REFRESH's `report_selection` reads that row-0
    /// refocus as a FRESH selection and records it as `requested_leaf`, which
    /// would drag the form off the entry the operator was on. The reviewer's
    /// probe observed exactly this before the fix: `requested_leaf` became
    /// `Some(("ou=p,dc=x", []))` while `current_leaf` was
    /// `Some("cn=b,ou=p,dc=x")`.
    #[test]
    fn reload_rebuild_does_not_report_the_self_row_as_a_fresh_selection() {
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
                dn: "cn=b,ou=p,dc=x".into(),
                cn: Some("b".into()),
                description: None,
                object_classes: vec![],
                attrs: BTreeMap::new(),
            },
        ];
        let structure = Structure::build("dc=x", inputs.clone());
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut state =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        state.current_branch = Some("ou=p,dc=x".into());
        // The operator has "b" open in the form — the entry a rebuild must not
        // navigate away from.
        state.current_leaf = Some("cn=b,ou=p,dc=x".into());
        let shared: Shared = Rc::new(RefCell::new(state));

        let mut pane = LeafPane::new(Rect::new(0, 0, 30, 10), shared.clone());
        let mut out: VecDeque<Event> = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<tv::Deferred> = Vec::new();
        let mut ctx = tv::Context::new(&mut out, &mut timers, 0, &mut deferred);

        // Seed the pane once (an ordinary fresh-load row-0 report is not what is
        // under test here); clear the requested_leaf it produces so the
        // assertion below is only about the rebuild that follows.
        shared.borrow_mut().list_dirty = true;
        let mut seed = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut seed, &mut ctx);
        shared.borrow_mut().requested_leaf = None;

        // Simulate Alt+R: a fresh scan of the same structure is adopted
        // mid-session, exactly as `reload_structure` does after a successful
        // blocking scan.
        let fresh = Structure::build("dc=x", inputs);
        shared.borrow_mut().adopt_structure(fresh);

        let mut reload_refresh = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut reload_refresh, &mut ctx);

        assert_ne!(
            shared.borrow().requested_leaf,
            Some(("ou=p,dc=x".to_string(), Vec::new())),
            "the rebuild's row-0 refocus must not be reported as a fresh \
             selection onto the container's ‹self› row"
        );
    }

    /// A rebuild that moves the pinned entry to a different index must follow the
    /// DN — and must NOT look like a navigation. This is the general form of the
    /// I1 fix: the index is never computed against the pre-rebuild row source.
    ///
    /// The shift is produced by REMOVING a row that precedes the pinned one
    /// (another admin deleting an entry). Do not try to produce it by upserting a
    /// new entry: `leaf_rows` is `‹self›` followed by `leaves_of`, which returns
    /// children in INSERTION order with no sort, so an upsert appends last and the
    /// pinned row's index would not move at all — the test would pass against a
    /// stale-index implementation and prove nothing.
    #[test]
    fn rebuild_keeps_the_highlight_on_the_pinned_dn_without_reporting() {
        let shared = shared_with_rows(&["cn=a,ou=p,dc=x", "cn=b,ou=p,dc=x"]);
        shared.borrow_mut().current_leaf = Some("cn=b,ou=p,dc=x".to_string());
        let mut pane = LeafPane::new(Rect::new(0, 0, 30, 10), shared.clone());
        refresh(&mut pane);
        shared.borrow_mut().requested_leaf = None;
        // Precondition: rows are [‹self› ou=p, cn=a, cn=b], so cn=b is row 2.
        assert_eq!(
            pane.selected_row_for_test(),
            2,
            "test premise: the pinned entry starts at row 2 (row 0 is ‹self›)"
        );

        // cn=a disappears → rows become [‹self› ou=p, cn=b] → cn=b moves to row 1.
        {
            let mut st = shared.borrow_mut();
            st.structure.remove("cn=a,ou=p,dc=x");
            st.list_dirty = true;
        }
        refresh(&mut pane);

        let st = shared.borrow();
        assert_eq!(
            pane.selected_row_for_test(),
            1,
            "the highlight follows the DN across the renumbering"
        );
        assert_eq!(
            st.requested_leaf, None,
            "a rebuild must never be reported as an operator navigation"
        );
    }

    /// A rebuild where the open entry is no longer among the rows must move the
    /// highlight but leave a DIRTY form pinned — so the controller is never asked
    /// to navigate and the dirty guard cannot fire. This is the pane-level half of
    /// the modal-mid-keystroke fix.
    ///
    /// NOTE ON SETUP: do not simulate the find by assigning `st.search` +
    /// `st.leaf_search_rows` directly. After repopulating, `handle_event` compares
    /// the ListBox's own find query against `last_search`; the ListBox's query is
    /// still empty, so it would call `set_leaf_search("")`, wipe
    /// `leaf_search_rows`, and repopulate a second time against the full row set —
    /// and the assertions below would be measuring that second pass, not the
    /// policy. Making the open entry absent from the structure exercises the same
    /// `HighlightPlan` arms without fighting the find-query sync.
    #[test]
    fn rebuild_does_not_move_a_dirty_form_off_a_vanished_entry() {
        let shared = shared_with_rows(&["cn=a,ou=p,dc=x", "cn=b,ou=p,dc=x"]);
        {
            let mut st = shared.borrow_mut();
            // Open an entry that is not in this container's rows at all.
            st.current_leaf = Some("cn=gone,ou=p,dc=x".to_string());
            st.edit_form = Some(crate::ui::test_support::dirty_form("cn=gone,ou=p,dc=x"));
            st.list_dirty = true;
        }
        let mut pane = LeafPane::new(Rect::new(0, 0, 30, 10), shared.clone());
        refresh(&mut pane);

        assert_eq!(
            shared.borrow().requested_leaf,
            None,
            "a dirty form is never dragged along by a rebuild"
        );
        assert_eq!(
            pane.selected_row_for_test(),
            1,
            "the highlight still moves to the first real row (row 0 is ‹self›)"
        );
    }

    /// The clean counterpart: the form follows, because following is only ever
    /// withheld to protect unsaved edits.
    #[test]
    fn rebuild_follows_the_first_row_when_the_form_is_clean() {
        let shared = shared_with_rows(&["cn=a,ou=p,dc=x", "cn=b,ou=p,dc=x"]);
        {
            let mut st = shared.borrow_mut();
            st.current_leaf = Some("cn=gone,ou=p,dc=x".to_string());
            // No edit_form at all == clean.
            st.list_dirty = true;
        }
        let mut pane = LeafPane::new(Rect::new(0, 0, 30, 10), shared.clone());
        refresh(&mut pane);

        assert_eq!(
            shared
                .borrow()
                .requested_leaf
                .as_ref()
                .map(|(dn, _)| dn.as_str()),
            Some("cn=a,ou=p,dc=x"),
            "a clean form follows the rebuild to the first real row"
        );
    }
}
