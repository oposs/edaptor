//! DIT tree pane: an `Outline` over the structure's branch hierarchy.

use tvision_rs::{
    self as tv, delegate, Context, DrawCtx, Event, FieldValue, OutlineViewer, Rect, Role, View,
};

use crate::config::tree_label::{eval_tree_label, fit_label};
use crate::ui::state::UiState;
use crate::ui::Shared;

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
/// whether to commit the navigation. Also honours `set_tree_row` to snap the
/// outline highlight back when a guard returns "Stay". (Auto-seeds on first event;
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

    /// Test seam: directly set the outline's focused row (bypasses `set_value_ctx`
    /// which is a no-op on `Outline` since it does not override `set_value`).
    #[cfg(test)]
    pub(crate) fn select_row_for_test(&mut self, row: i32, _ctx: &mut Context) {
        if let Some(outline) = self.outline_mut() {
            outline.ov_mut().foc = row;
        }
    }
}

#[delegate(to = group)]
impl View for TreePane {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    fn draw(&mut self, ctx: &mut DrawCtx) {
        // Focused pane = brightest (base3); the others recede (inactive surface).
        // Key off `focused`, not `active`: `active` fans out to every pane in the
        // window (so it is uniformly true and never distinguishes focus), whereas
        // `focused` follows only the current-child chain. Mirrors the form and leaf
        // panes. The Outline itself already keys its current-node colour on
        // `focused`, so this only governs the area not covered by outline rows.
        let role = if self.group.state().state.focused {
            Role::ListNormalActive
        } else {
            Role::ListNormalInactive
        };
        let style = ctx.style(role);
        let extent = self.group.state().get_extent();
        ctx.fill(extent, ' ', style);
        self.group.draw(ctx);
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        // Only scroll on the wheel when the cursor is over this pane — tvision
        // delivers the wheel non-positionally, so otherwise the outline would grab
        // a wheel meant for a sibling pane. Left unconsumed, it propagates.
        if super::wheel_misses_pane(self.group.state(), ev) {
            return;
        }
        // Controller → pane: snap the selection back (guard "Stay") before reporting.
        let snap = self.state.borrow_mut().set_tree_row.take();
        if let Some(row) = snap {
            if let Some(outline) = self.outline_mut() {
                tv::widgets::outline::adjust_focus(outline, row, ctx);
            }
            self.last_sel = row;
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
    use crate::ui::REFRESH;
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
    /// with `OutlineNormal` when it holds focus and `OutlineNormalInactive` when
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
        let inactive_bg = theme.style(Role::OutlineNormalInactive).bg;
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
            "unfocused tree: normal rows recede to OutlineNormalInactive"
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
}
