//! DIT tree pane: an `Outline` over the structure's branch hierarchy.

use tvision_rs::{self as tv, delegate, Context, Event, FieldValue, Rect, View};

use crate::config::tree_label::{eval_tree_label, fit_label};
use crate::tui::state::UiState;
use crate::tui::{Shared, REFRESH};

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

/// Outline pane: updates `current_branch` + `list_dirty` and broadcasts REFRESH
/// when the selected branch changes. (0.1.2 auto-seeds; read selection via
/// `Outline::value()`; call `ov_update` only after a tree mutation — none here.)
pub(crate) struct TreePane {
    outline: tv::Outline,
    state: Shared,
    last_sel: i32,
}

impl TreePane {
    pub(crate) fn new(bounds: Rect, root: Option<Box<tv::Node>>, state: Shared) -> Self {
        TreePane {
            outline: tv::Outline::new(bounds, None, None, root),
            state,
            last_sel: -1,
        }
    }
}

#[delegate(to = outline)]
impl View for TreePane {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        self.outline.handle_event(ev, ctx);
        let sel = match self.outline.value() {
            Some(FieldValue::Int(i)) => i,
            _ => 0,
        };
        if sel != self.last_sel {
            self.last_sel = sel;
            let mut updated = false;
            if sel >= 0 {
                let mut st = self.state.borrow_mut();
                if let Some(dn) = st.branch_dns.get(sel as usize).cloned() {
                    st.current_branch = Some(dn);
                    st.list_dirty = true;
                    updated = true;
                }
            } // borrow dropped before broadcast
            if updated {
                ctx.broadcast(REFRESH, None);
            }
        }
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
}
