//! DIT tree pane: an `Outline` over the structure's branch hierarchy.

use tvision_rs::{
    self as tv, delegate, Context, Event, FieldValue, OutlineViewer, Rect, Role, View,
};

use crate::config::tree_label::{eval_tree_label, fit_label};
use crate::ui::state::{HighlightPlan, UiState};
use crate::ui::{Shared, REFRESH};

/// Build a tvision `Node` tree and a parallel DFS pre-order DN index from the
/// structure's branch hierarchy. Only branches (nodes with ≥1 child) appear;
/// leaves live in pane 2. Labels come from the compiled tree rules, width-fit to
/// `width`. Pre-order matches the `foc` index `Outline` assigns.
pub(crate) fn build_branch_nodes(
    state: &UiState,
    width: usize,
) -> (Option<Box<tv::Node>>, Vec<String>) {
    use std::collections::HashSet;
    let branches: HashSet<String> = state.structure.branch_dns().into_iter().collect();
    let mut dns = Vec::new();

    fn rdn_of(dn: &str) -> &str {
        dn.split_once(',').map(|(h, _)| h).unwrap_or(dn)
    }

    fn build(
        dn: &str,
        state: &UiState,
        branches: &std::collections::HashSet<String>,
        width: usize,
        dns: &mut Vec<String>,
    ) -> tv::Node {
        dns.push(dn.to_string());
        let node = state.structure.get(dn);
        let label = match node {
            Some(n) => {
                let segs = eval_tree_label(&state.tree_rules, &n.attrs, rdn_of(dn));
                let fit = fit_label(&segs, width.max(4));
                if fit.is_empty() {
                    rdn_of(dn).to_string()
                } else {
                    fit
                }
            }
            None => rdn_of(dn).to_string(),
        };
        let mut tnode = tv::Node::new(&label).with_expanded(true);

        let child_branches: Vec<String> = node
            .map(|n| {
                n.children
                    .iter()
                    .filter(|c| branches.contains(*c))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();

        // Build children in FORWARD order so `dns` is pushed in display/foc order
        // (the Outline numbers visible lines in pre-order). Linking the `with_next`
        // chain needs reverse folding, but that is done AFTER the recursion so it
        // does not affect `dns` ordering.
        let mut child_nodes: Vec<tv::Node> = Vec::with_capacity(child_branches.len());
        for cb in &child_branches {
            child_nodes.push(build(cb, state, branches, width, dns));
        }
        let mut chain: Option<Box<tv::Node>> = None;
        for child in child_nodes.into_iter().rev() {
            let child = match chain.take() {
                Some(next) => child.with_next(next),
                None => child,
            };
            chain = Some(Box::new(child));
        }
        if let Some(children) = chain {
            tnode = tnode.with_children(children);
        }
        tnode
    }

    let root_dn = state.base_dn.clone();
    if branches.contains(&root_dn) || state.structure.get(&root_dn).is_some() {
        let root = build(&root_dn, state, &branches, width, &mut dns);
        (Some(Box::new(root)), dns)
    } else {
        (None, dns)
    }
}

/// Outline pane: pure selector — records `requested_branch` when the highlighted
/// branch changes. Never mutates `current_branch` or `list_dirty` directly, and
/// never broadcasts; the controller ([`UiState::reconcile_branch`]) decides
/// whether to commit the navigation. After every rebuild it resolves the
/// controller's [`HighlightPlan`] (via `apply_highlight_plan`) and resyncs to it,
/// so a guard "Stay" is just "mark tree_dirty" — no index-based snap-back
/// survives a rebuild it was not computed against. (Auto-seeds on first event;
/// call `ov_update` only after a tree mutation — none here.)
pub(crate) struct TreePane {
    /// Owning container holding the `Outline` plus its vertical `ScrollBar` as
    /// siblings (an `Outline` cannot host a sibling bar by itself). The pane
    /// delegates the `View` surface to this group (Task 8 restructure).
    group: tv::Group,
    /// Child id of the `Outline` inside `group` (reached via `child_mut` +
    /// downcast for the selection-tracking and snap-back logic).
    outline_id: tv::ViewId,
    /// Vertical scroll bar in the right column. Wired as the outline's `v_bar`,
    /// so the outline widget publishes its range/value/page itself; this pane
    /// owns only the focus + overflow visibility gate and the matching one-column
    /// width reflow (Task 8, mirroring the leaf pane's Task 7).
    v_bar: tv::ViewId,
    state: Shared,
    last_sel: i32,
}

impl TreePane {
    pub(crate) fn new(bounds: Rect, root: Option<Box<tv::Node>>, state: Shared) -> Self {
        let w = bounds.b.x - bounds.a.x;
        let h = bounds.b.y - bounds.a.y;
        let mut group = tv::Group::new(bounds);
        // ofFirstClick: the click that focuses this pane (when another pane was
        // active) must ALSO reach the outline, so a single click both focuses and
        // moves the highlight. The bare `Outline` set this for the pane via the old
        // delegation; a plain `Group` does not, so set it explicitly (matches the
        // leaf/form panes).
        group.state_mut().options.first_click = true;
        // The pane paints its own background (tvision 0.8 `Group::set_surface`):
        // bright when focused, receded to the desktop tone when not — the cells
        // the outline rows do not cover. Replaces the hand-rolled fill in `draw`.
        group.set_surface(Role::ListNormal, Role::ListInactive);
        // Vertical scroll bar in the right column (width 1 ⇒ vertical, which
        // ScrollBar::new detects and gives the right grow_mode). Hidden until the
        // pane is focused AND the tree overflows — sync_scrollbar() owns that gate
        // and the matching one-column width reflow.
        let mut v_bar = tv::ScrollBar::new(Rect::new(w - 1, 0, w, h));
        v_bar.state_mut().state.visible = false;
        let v_bar = group.insert(Box::new(v_bar));
        // The outline fills the full height and — while the bar is hidden — the
        // full width. It is wired as the outline's vertical bar, so the outline
        // widget publishes the bar's range/value/page itself on every (re)count
        // and a bar drag scrolls it; this pane only gates visibility + width.
        let mut outline = tv::Outline::new(Rect::new(0, 0, w, h), None, Some(v_bar), root);
        outline.state_mut().grow_mode.hi_x = true;
        outline.state_mut().grow_mode.hi_y = true;
        let outline_id = group.insert(Box::new(outline));
        TreePane {
            group,
            outline_id,
            v_bar,
            state,
            last_sel: -1,
        }
    }

    /// Typed accessor for the `Outline` child (downcast through the group). All
    /// selection-tracking, snap-back and overflow reads go through here so the
    /// data-logic stays byte-for-byte equivalent to the pre-restructure pane.
    fn outline_mut(&mut self) -> Option<&mut tv::Outline> {
        self.group
            .child_mut(self.outline_id)
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<tv::Outline>())
    }

    /// Pure layout decision for the focus-gated scroll bar (mirrors the leaf
    /// pane's Task 7 helper). Given the outline's total `rows`, the visible
    /// `page` (rows that fit), whether the pane is `active`, and the pane width
    /// `w`, decide whether the bar is shown and how wide the outline should be.
    /// The bar is shown iff the pane is active AND the content overflows the
    /// page; when shown the outline yields one column to the bar, otherwise it
    /// spans the full width. Side-effect free so it is unit-testable.
    fn bar_layout(rows: i32, page: i32, active: bool, w: i32) -> (bool, i32) {
        let overflow = rows > page;
        let show = active && overflow;
        let outline_w = if show { (w - 1).max(0) } else { w };
        (show, outline_w)
    }

    /// Focus + overflow gate for the scroll bar, plus the one-column width
    /// reflow. The wired outline already publishes the bar's range/value/page, so
    /// this never touches scroll params — it only decides visibility/geometry.
    ///
    /// The outline toggles the bar visible on `active` alone (overflow-blind) via
    /// a deferred `SetVisible`; we re-assert the overflow-aware decision through
    /// `request_set_visible` so a focused but non-overflowing tree still hides the
    /// bar (the later deferred op wins).
    fn sync_scrollbar(&mut self, ctx: &mut Context) {
        let active = self.group.state().state.active;
        let extent = self.group.state().get_extent();
        let (w, h) = (extent.b.x, extent.b.y);
        // The outline fills the full pane height, so its visible page is h rows.
        let page = h.max(0);
        // Total visible node count published by the outline's last (re)count.
        let rows = self.outline_mut().map(|o| o.ov().limit.y).unwrap_or(0);
        let (show, outline_w) = Self::bar_layout(rows, page, active, w);
        // Reserve / reclaim the bar lane.
        if let Some(outline) = self.group.child_mut(self.outline_id) {
            outline.change_bounds(Rect::new(0, 0, outline_w, h));
        }
        // Pin the bar to the right column (idempotent with its grow_mode) and
        // toggle its visibility directly for the steady state.
        if let Some(bar) = self.group.child_mut(self.v_bar) {
            bar.change_bounds(Rect::new(w - 1, 0, w, h));
            bar.state_mut().state.visible = show;
        }
        // Compete with the outline's overflow-blind SetVisible in the deferred drain.
        ctx.request_set_visible(self.v_bar, show);
    }

    /// Rebuild the outline's node set from the current structure.
    ///
    /// Called when `tree_dirty` is set — a branch appeared, disappeared, or changed
    /// its rendered label. Only replaces the node set and refreshes `branch_dns`;
    /// the caller resolves the highlight afterward via [`Self::apply_highlight_plan`]
    /// (by DN, never by row index — a rebuild shifts every index below the change).
    fn rebuild(&mut self, ctx: &mut Context) {
        let width = (self.group.state().get_extent().b.x).max(4) as usize;
        let (root, dns) = {
            let st = self.state.borrow();
            build_branch_nodes(&st, width)
        };
        self.state.borrow_mut().branch_dns = dns;
        if let Some(outline) = self.outline_mut() {
            outline.root = root;
            tv::widgets::outline::ov_update(outline, ctx);
        }
    }

    /// Resolve the controller's [`HighlightPlan`] against the DFS index just
    /// rebuilt and resync `last_sel` silently. The tree never produces `Follow`,
    /// so this only ever moves the highlight — never the form.
    fn apply_highlight_plan(&mut self, ctx: &mut Context) {
        let (plan, dns) = {
            let st = self.state.borrow();
            (st.branch_highlight_plan(), st.branch_dns.clone())
        };
        let row = match plan {
            HighlightPlan::Pin(dn) | HighlightPlan::Follow(dn) => dns
                .iter()
                .position(|d| d.eq_ignore_ascii_case(&dn))
                .map(|i| i as i32)
                .unwrap_or(-1),
            HighlightPlan::Clear => -1,
        };
        if row >= 0 {
            if let Some(outline) = self.outline_mut() {
                tv::widgets::outline::adjust_focus(outline, row, ctx);
            }
        }
        // Finding 2: `ov_update` re-clamps `foc` internally, so the value we asked
        // for may NOT be the value the widget now holds. Read the widget's ACTUAL
        // value back — never assume `row` stuck — so `report_selection`'s next
        // comparison is a guaranteed no-op for every arm, including a vanished
        // branch (row -1) and the Clear case. This is the same invariant the leaf
        // pane's `apply_highlight_plan` relies on: resync to the widget's truth,
        // do not trust the value you pushed.
        self.last_sel = match self.outline_mut().and_then(|o| o.value()) {
            Some(FieldValue::Int(i)) => i,
            _ => 0,
        };
    }

    /// Test seam: directly set the outline's focused row (bypasses `set_value_ctx`
    /// which is a no-op on `Outline` since it does not override `set_value`).
    #[cfg(test)]
    pub(crate) fn select_row_for_test(&mut self, row: i32, _ctx: &mut Context) {
        if let Some(outline) = self.outline_mut() {
            outline.ov_mut().foc = row;
        }
    }

    /// Test seam: read the outline's actual focused row.
    #[cfg(test)]
    pub(crate) fn selected_row_for_test(&mut self) -> i32 {
        match self.outline_mut().and_then(|o| o.value()) {
            Some(FieldValue::Int(i)) => i,
            _ => -1,
        }
    }
}

#[delegate(to = group)]
impl View for TreePane {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        // Only scroll on the wheel when the cursor is over this pane — tvision
        // delivers the wheel non-positionally, so otherwise the outline would grab
        // a wheel meant for a sibling pane. Left unconsumed, it propagates.
        if super::wheel_misses_pane(self.group.state(), ev) {
            return;
        }
        // A structure change (create, rename, delete, refresh) marked the tree stale:
        // rebuild before this event is processed, so the DFS index the selection
        // logic below reads is the current one.
        let needs_rebuild = matches!(ev, Event::Broadcast { command, .. } if *command == REFRESH)
            && self.state.borrow().tree_dirty;
        if needs_rebuild {
            self.rebuild(ctx);
            self.state.borrow_mut().tree_dirty = false;
            // Resolve the highlight by DN against the index just rebuilt — a guard
            // "Stay" is now just "mark tree_dirty", which lands here.
            self.apply_highlight_plan(ctx);
        }

        self.group.handle_event(ev, ctx);
        let sel = match self.outline_mut().and_then(|o| o.value()) {
            Some(FieldValue::Int(i)) => i,
            _ => 0,
        };
        if sel != self.last_sel {
            self.last_sel = sel;
            if sel >= 0 {
                let dn = self.state.borrow().branch_dns.get(sel as usize).cloned();
                if let Some(dn) = dn {
                    self.state.borrow_mut().request_branch(dn); // pure selector
                }
            }
        }

        // Re-evaluate the bar's focus + overflow gate now that the node count and
        // the pane's active state are current for this frame.
        self.sync_scrollbar(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tree_label::compile_tree_rules;
    use crate::config::TreeConfig;
    use crate::ldap::worker::RawSubschema;
    use crate::schema::SchemaModel;
    use crate::workflows::structure::{Structure, StructureInput};
    use std::collections::BTreeMap;

    fn si(dn: &str) -> StructureInput {
        StructureInput {
            dn: dn.into(),
            cn: None,
            description: None,
            object_classes: vec![],
            attrs: BTreeMap::new(),
        }
    }

    /// Drive one `REFRESH` broadcast through the pane.
    fn refresh_tree(pane: &mut TreePane) {
        let mut out = std::collections::VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<tv::Deferred> = Vec::new();
        let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut ev, &mut ctx);
    }

    /// Pure focus + overflow gate (mirrors the leaf pane's Task 7 test). The bar
    /// shows only when the pane is active AND the nodes overflow the page; when
    /// shown the outline yields one column, otherwise it keeps the full width.
    #[test]
    fn bar_layout_gates_on_focus_and_overflow() {
        // Active + overflow → bar shown, outline narrowed by one column.
        assert_eq!(TreePane::bar_layout(20, 10, true, 30), (true, 29));
        // Active but fits exactly (rows == page) → no overflow → hidden, full width.
        assert_eq!(TreePane::bar_layout(10, 10, true, 30), (false, 30));
        // Overflow but pane not active → hidden, outline reclaims full width.
        assert_eq!(TreePane::bar_layout(20, 10, false, 30), (false, 30));
        // Empty tree while active → hidden, full width.
        assert_eq!(TreePane::bar_layout(0, 10, true, 30), (false, 30));
        // Degenerate width clamps to 0 rather than going negative.
        assert_eq!(TreePane::bar_layout(20, 10, true, 0), (true, 0));
    }

    /// Focus visualization (tvision-rs 0.5): the Outline now paints normal rows
    /// with `OutlineNormal` when it holds focus and `OutlineInactive` when
    /// it does not. edaptor themes those to base3 (bright) vs the desktop tone
    /// (dim), so the tree pane brightens on focus and recedes when a sibling pane
    /// takes over — mirroring the leaf list. Row 0 is the current node (blue /
    /// faded); row 1 is a normal row, which is what we assert here.
    #[test]
    fn focused_tree_brightens_normal_rows_and_dims_when_unfocused() {
        use tvision_rs::{Buffer, DrawCtx, Point};

        let inputs = vec![
            si("dc=x"),
            si("ou=a,dc=x"),
            si("ou=b,dc=x"),
            si("cn=1,ou=a,dc=x"),
            si("cn=1,ou=b,dc=x"),
        ];
        let structure = Structure::build("dc=x", inputs);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st = UiState::new_for_test(
            structure,
            schema,
            "dc=x".into(),
            Vec::new(),
            compile_tree_rules(&TreeConfig::default()),
        );
        let (root, dns) = build_branch_nodes(&st, 40);
        st.branch_dns = dns;
        let shared: std::rc::Rc<std::cell::RefCell<UiState>> =
            std::rc::Rc::new(std::cell::RefCell::new(st));
        let mut pane = TreePane::new(Rect::new(0, 0, 30, 10), root, shared);

        let theme = crate::ui::theme::edaptor_theme();
        let active_bg = theme.style(Role::OutlineNormal).bg;
        let inactive_bg = theme.style(Role::OutlineInactive).bg;
        assert_ne!(
            active_bg, inactive_bg,
            "test premise: focused vs unfocused outline surfaces must differ"
        );

        // Draw with the outline focused / unfocused and read row 1's background.
        let row1_bg = |pane: &mut TreePane, focused: bool| -> tv::Color {
            pane.group.state_mut().state.focused = focused;
            if let Some(outline) = pane.outline_mut() {
                outline.state_mut().state.focused = focused;
            }
            let mut buf = Buffer::new(30, 10);
            {
                let mut dctx =
                    DrawCtx::new(&mut buf, &theme, Rect::new(0, 0, 30, 10), Point::new(0, 0));
                <TreePane as View>::draw(pane, &mut dctx);
            }
            buf.get(0, 1).style().bg
        };

        assert_eq!(
            row1_bg(&mut pane, true),
            active_bg,
            "focused tree: normal rows use the bright OutlineNormal surface"
        );
        assert_eq!(
            row1_bg(&mut pane, false),
            inactive_bg,
            "unfocused tree: normal rows recede to OutlineInactive"
        );
    }

    #[test]
    fn test_branch_nodes_sibling_order_matches_display() {
        // Root with THREE sibling branches a,b,c (each has a leaf child, so each is
        // a branch). The Outline displays them a,b,c (with_next chain order), so the
        // parallel branch_dns index MUST be forward pre-order [root, a, b, c] — NOT
        // reversed. Regression guard for the `foc -> branch_dns` mismatch.
        let inputs = vec![
            si("dc=x"),
            si("ou=a,dc=x"),
            si("ou=b,dc=x"),
            si("ou=c,dc=x"),
            si("cn=1,ou=a,dc=x"),
            si("cn=1,ou=b,dc=x"),
            si("cn=1,ou=c,dc=x"),
        ];
        let structure = Structure::build("dc=x", inputs);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let st = UiState::new_for_test(
            structure,
            schema,
            "dc=x".into(),
            Vec::new(),
            compile_tree_rules(&TreeConfig::default()),
        );
        let (_root, dns) = build_branch_nodes(&st, 40);
        assert_eq!(
            dns,
            vec![
                "dc=x".to_string(),
                "ou=a,dc=x".to_string(),
                "ou=b,dc=x".to_string(),
                "ou=c,dc=x".to_string(),
            ],
            "branch_dns must be in forward display/foc order, not reversed per sibling"
        );
    }

    #[test]
    fn test_branch_nodes_dfs_preorder_excludes_leaves() {
        // dc=x (root) -> ou=a (branch, has child) ; ou=b is a childless leaf.
        let inputs = vec![
            si("dc=x"),
            si("ou=a,dc=x"),
            si("ou=b,dc=x"),
            si("cn=1,ou=a,dc=x"),
        ];
        let structure = Structure::build("dc=x", inputs);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let st = UiState::new_for_test(
            structure,
            schema,
            "dc=x".into(),
            Vec::new(),
            compile_tree_rules(&TreeConfig::default()),
        );
        let (root, dns) = build_branch_nodes(&st, 40);
        assert!(root.is_some());
        assert_eq!(dns, vec!["dc=x".to_string(), "ou=a,dc=x".to_string()]);
        assert!(!dns.contains(&"ou=b,dc=x".to_string()));
    }

    /// Pure-selector contract: moving the tree highlight records `requested_branch`
    /// but never touches `current_branch` or `list_dirty`, and never broadcasts.
    #[test]
    fn tree_records_requested_branch_only() {
        let inputs = vec![
            si("dc=x"),
            si("ou=a,dc=x"),
            si("ou=b,dc=x"),
            si("cn=1,ou=a,dc=x"),
            si("cn=1,ou=b,dc=x"),
        ];
        let structure = Structure::build("dc=x", inputs);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st = UiState::new_for_test(
            structure,
            schema,
            "dc=x".into(),
            Vec::new(),
            compile_tree_rules(&TreeConfig::default()),
        );
        let (root, dns) = build_branch_nodes(&st, 40);
        st.branch_dns = dns;
        let shared: std::rc::Rc<std::cell::RefCell<UiState>> =
            std::rc::Rc::new(std::cell::RefCell::new(st));
        let mut pane = TreePane::new(Rect::new(0, 0, 30, 10), root, shared.clone());

        let mut out = std::collections::VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<tv::Deferred> = Vec::new();
        let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);
        // Move selection to row 1 (ou=a) and deliver an event.
        pane.select_row_for_test(1, &mut ctx);
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut ev, &mut ctx);

        let st = shared.borrow();
        assert_eq!(
            st.requested_branch.as_deref(),
            Some("ou=a,dc=x"),
            "tree must record requested_branch on selection change"
        );
        assert_eq!(
            st.current_branch, None,
            "pure selector: never switches current_branch inline"
        );
    }

    /// A structure change that promotes a leaf to a branch must reach the outline:
    /// on REFRESH with `tree_dirty` set the pane rebuilds its node set, refreshes
    /// `branch_dns`, and keeps the highlight on the same DN (row indices shift).
    ///
    /// The selected DN (`ou=b,dc=x`) is placed in the MIDDLE of the rebuilt DFS
    /// index (`ou=c,dc=x` follows it) — not the last row — so this only passes if
    /// the highlight is genuinely restored by DN; an "always clamp to the last
    /// row" bug would fail it.
    #[test]
    fn refresh_with_tree_dirty_rebuilds_and_keeps_the_selected_dn() {
        let inputs = vec![
            si("dc=x"),
            si("ou=a,dc=x"),
            si("ou=b,dc=x"),
            si("cn=1,ou=b,dc=x"),
            si("ou=c,dc=x"),
            si("cn=1,ou=c,dc=x"),
        ];
        let structure = Structure::build("dc=x", inputs);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st = UiState::new_for_test(
            structure,
            schema,
            "dc=x".into(),
            Vec::new(),
            compile_tree_rules(&TreeConfig::default()),
        );
        let (root, dns) = build_branch_nodes(&st, 40);
        // dc=x, ou=b and ou=c are branches at build time; ou=a is a childless leaf.
        assert_eq!(
            dns,
            vec![
                "dc=x".to_string(),
                "ou=b,dc=x".to_string(),
                "ou=c,dc=x".to_string()
            ]
        );
        st.branch_dns = dns;
        st.current_branch = Some("ou=b,dc=x".into());
        let shared: std::rc::Rc<std::cell::RefCell<UiState>> =
            std::rc::Rc::new(std::cell::RefCell::new(st));
        let mut pane = TreePane::new(Rect::new(0, 0, 30, 10), root, shared.clone());

        // A new entry lands under ou=a → it becomes a branch → the tree must change.
        {
            let mut st = shared.borrow_mut();
            st.structure.upsert(si("cn=2,ou=a,dc=x"));
            st.tree_dirty = true;
        }

        let mut out = std::collections::VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<tv::Deferred> = Vec::new();
        let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut ev, &mut ctx);

        let st = shared.borrow();
        assert_eq!(
            st.branch_dns,
            vec![
                "dc=x".to_string(),
                "ou=a,dc=x".to_string(),
                "ou=b,dc=x".to_string(),
                "ou=c,dc=x".to_string(),
            ],
            "the promoted container must appear in the DFS index, ou=b in the middle"
        );
        assert!(!st.tree_dirty, "the pane clears the flag it consumed");
        assert_eq!(
            st.requested_branch.as_deref(),
            None,
            "restoring the highlight by DN must not look like a user navigation"
        );
    }

    /// Finding 1 regression: when the previously selected branch vanishes from the
    /// rebuilt index entirely (not just shifts row), tvision's `ov_update` still
    /// re-clamps `foc` internally (via `adjust_focus`). The pane must resync
    /// `last_sel` to that clamped value unconditionally — otherwise the next
    /// selection check sees `sel != last_sel` and reports a branch the operator
    /// never selected, violating the pure-selector contract.
    #[test]
    fn rebuild_with_a_vanished_branch_does_not_report_a_navigation() {
        let inputs = vec![
            si("dc=x"),
            si("ou=a,dc=x"),
            si("cn=1,ou=a,dc=x"),
            si("ou=b,dc=x"),
            si("cn=1,ou=b,dc=x"),
            si("ou=c,dc=x"),
            si("cn=1,ou=c,dc=x"),
        ];
        let structure = Structure::build("dc=x", inputs);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st = UiState::new_for_test(
            structure,
            schema,
            "dc=x".into(),
            Vec::new(),
            compile_tree_rules(&TreeConfig::default()),
        );
        let (root, dns) = build_branch_nodes(&st, 40);
        assert_eq!(
            dns,
            vec![
                "dc=x".to_string(),
                "ou=a,dc=x".to_string(),
                "ou=b,dc=x".to_string(),
                "ou=c,dc=x".to_string(),
            ]
        );
        st.branch_dns = dns;
        st.current_branch = Some("ou=c,dc=x".into());
        let shared: std::rc::Rc<std::cell::RefCell<UiState>> =
            std::rc::Rc::new(std::cell::RefCell::new(st));
        let mut pane = TreePane::new(Rect::new(0, 0, 30, 10), root, shared.clone());

        let mut out = std::collections::VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<tv::Deferred> = Vec::new();
        let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);
        // Select row 3 (ou=c,dc=x) as the operator's current highlight.
        pane.select_row_for_test(3, &mut ctx);

        // ou=b and ou=c both lose their only child → both drop out of the rebuilt
        // index, leaving [dc=x, ou=a,dc=x].
        {
            let mut st = shared.borrow_mut();
            st.structure.remove("cn=1,ou=b,dc=x");
            st.structure.remove("cn=1,ou=c,dc=x");
            st.tree_dirty = true;
        }

        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut ev, &mut ctx);

        let st = shared.borrow();
        assert_eq!(
            st.branch_dns,
            vec!["dc=x".to_string(), "ou=a,dc=x".to_string()],
            "ou=b and ou=c must have dropped out of the rebuilt index"
        );
        assert_eq!(
            st.requested_branch, None,
            "a vanished branch's clamped focus must not be reported as a navigation"
        );
        assert_eq!(
            st.current_branch.as_deref(),
            Some("ou=c,dc=x"),
            "the pane must never mutate current_branch itself"
        );
    }

    /// Follow-up #1: the guard "Stay" snap used to be an index resolved against
    /// the PRE-rebuild `branch_dns`. With a rebuild pending, that index described
    /// the old numbering. Resolving by DN after the rebuild removes the class.
    /// SETUP NOTE: `ou=a,dc=x` must exist as a childless leaf in the initial
    /// inputs. `Structure::upsert` links a node under its parent only when that
    /// parent is already a known node, so upserting `cn=2,ou=a,dc=x` into a
    /// structure that has never heard of `ou=a` leaves the child unlinked and
    /// invisible — `branch_dns` would not change and this test would fail for a
    /// reason that has nothing to do with the highlight. This mirrors the setup
    /// of the existing `refresh_with_tree_dirty_rebuilds_and_keeps_the_selected_dn`.
    #[test]
    fn guard_stay_snaps_by_dn_across_a_pending_rebuild() {
        let inputs = vec![
            si("dc=x"),
            si("ou=a,dc=x"),
            si("ou=b,dc=x"),
            si("cn=1,ou=b,dc=x"),
        ];
        let structure = Structure::build("dc=x", inputs);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st = UiState::new_for_test(
            structure,
            schema,
            "dc=x".into(),
            Vec::new(),
            compile_tree_rules(&TreeConfig::default()),
        );
        let (root, dns) = build_branch_nodes(&st, 40);
        st.branch_dns = dns; // ["dc=x", "ou=b,dc=x"]
        st.current_branch = Some("ou=b,dc=x".into());
        let shared: std::rc::Rc<std::cell::RefCell<UiState>> =
            std::rc::Rc::new(std::cell::RefCell::new(st));
        let mut pane = TreePane::new(Rect::new(0, 0, 30, 10), root, shared.clone());

        // A child lands under the childless ou=a, promoting it leaf -> branch, so
        // ou=b moves from row 1 to row 2 — and the rebuild has NOT happened yet
        // when the guard resolves.
        {
            let mut st = shared.borrow_mut();
            st.structure.upsert(si("cn=2,ou=a,dc=x"));
            st.tree_dirty = true;
        }
        refresh_tree(&mut pane);

        let st = shared.borrow();
        assert_eq!(
            st.branch_dns,
            vec![
                "dc=x".to_string(),
                "ou=a,dc=x".to_string(),
                "ou=b,dc=x".to_string()
            ],
        );
        assert_eq!(
            pane.selected_row_for_test(),
            2,
            "the highlight resolves to ou=b's NEW row, not its pre-rebuild index"
        );
        assert_eq!(
            st.requested_branch, None,
            "restoring the highlight must not look like a navigation"
        );
    }
}
