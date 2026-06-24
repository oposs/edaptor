//! Tree builder helper: builds the pane-1 tree items from the eager Structure.
//! Pure structure/label helpers have moved to crate::workflows::labels.

use tui_tree_widget::TreeItem;

use crate::config::tree_label::CompiledTreeRule;
use crate::workflows::labels::node_label;
use crate::workflows::structure::Structure;

pub(crate) use crate::workflows::labels::{
    compute_rows, label_rules, structure_input_from_attrs, structure_inputs, structure_scan_attrs,
    LabelRule,
};

/// Build the pane-1 tree items from the eager [`Structure`], rendering each
/// branch node's label via the compiled tree rules and width-fitting it to the
/// pane's inner width. Only branch nodes appear (leaves live in pane 2); the id
/// is the DN so `tree_state.selected()` yields the branch DN.
pub(crate) fn build_tree_items(
    structure: &Structure,
    rules: &[CompiledTreeRule],
    inner_width: usize,
) -> Vec<TreeItem<'static, String>> {
    fn build(
        structure: &Structure,
        dn: &str,
        rules: &[CompiledTreeRule],
        inner_width: usize,
        depth: usize,
    ) -> TreeItem<'static, String> {
        let label = node_label(structure, dn, rules, inner_width, depth);
        let mut children = Vec::new();
        if let Some(n) = structure.get(dn) {
            for child_dn in &n.children {
                if structure
                    .get(child_dn)
                    .map(|c| c.is_branch())
                    .unwrap_or(false)
                {
                    children.push(build(structure, child_dn, rules, inner_width, depth + 1));
                }
            }
        }
        if children.is_empty() {
            TreeItem::new_leaf(dn.to_string(), label)
        } else {
            TreeItem::new(dn.to_string(), label, children).expect("DNs are unique ids")
        }
    }
    vec![build(structure, structure.root_dn(), rules, inner_width, 0)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::app::test_support::*;

    #[test]
    fn structure_scan_attrs_includes_custom_tree_template_attrs() {
        let label_rules: Vec<LabelRule> = vec![];
        let tree_rules = vec![crate::config::tree_label::CompiledTreeRule {
            when: vec![],
            template: crate::config::label::parse_label_template("{rdn} [{ou}]"),
        }];
        let attrs = structure_scan_attrs(&label_rules, &tree_rules);
        // The custom template's `{ou}` is scanned; the reserved `{rdn}` is excluded.
        assert!(attrs.iter().any(|a| a.eq_ignore_ascii_case("ou")));
        assert!(!attrs.iter().any(|a| a.eq_ignore_ascii_case("rdn")));
    }

    #[test]
    fn tree_items_contain_only_branches() {
        let s = structure();
        let rules = crate::config::tree_label::default_tree_rules();
        let items = build_tree_items(&s, &rules, 80);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].children().len(), 1);
    }

    #[test]
    fn deepest_visible_node_still_shows_its_rdn_when_narrow() {
        let s = structure();
        let rules = crate::config::tree_label::default_tree_rules();
        // The deepest visible BRANCH is ou=users at depth 1, where the per-depth
        // indent term (depth*2) actually bites. node_label subtracts depth*2 + 2
        // (indent + node symbol) from inner_width.
        let branch = "ou=users,dc=example,dc=org"; // RDN "ou=users", width 8
                                                   // inner 12, depth 1 -> avail = 12 - (1*2 + 2) = 8 -> RDN fits EXACTLY.
        assert_eq!(node_label(&s, branch, &rules, 12, 1), "ou=users");
        // One column narrower -> avail 7 -> RDN ellipsized. This pins the indent
        // math from BOTH sides (an over- or under-subtracted constant fails one).
        assert_eq!(node_label(&s, branch, &rules, 11, 1), "ou=use…");
    }
}
