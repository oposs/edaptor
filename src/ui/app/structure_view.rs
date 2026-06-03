//! Structure/label/tree builder helpers: compile per-profile label rules, render
//! node labels, compute the pane-2 leaf rows, and build the pane-1 tree items.

use std::collections::BTreeMap;

use tui_tree_widget::TreeItem;

use crate::config::EntryProfile;
use crate::ldap::worker::StructureNodeRaw;
use crate::workflows::structure::{Structure, StructureInput};

/// The pane-2 rows for `branch` filtered by `search`: a `‹self›` row for the
/// branch entry itself, then its leaf children `(label, dn)`. Pure.
/// A compiled column-2 label rule: a profile's object classes + parsed template.
pub(crate) struct LabelRule {
    pub(crate) object_classes: Vec<String>,
    pub(crate) template: Vec<crate::config::label::LabelSeg>,
}

/// Compile the profiles that declare a `label` into rules (config order). Profiles
/// without a `label` are skipped, so an empty result reproduces the old behavior.
pub(crate) fn label_rules(profiles: &[EntryProfile]) -> Vec<LabelRule> {
    profiles
        .iter()
        .filter_map(|p| {
            p.label.as_ref().map(|tmpl| LabelRule {
                object_classes: p.object_classes.clone(),
                template: crate::config::label::parse_label_template(tmpl),
            })
        })
        .collect()
}

/// The union of attributes all rules reference (for the structure scan to fetch),
/// de-duplicated case-insensitively (config order preserved).
pub(crate) fn label_rule_attrs(rules: &[LabelRule]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for rule in rules {
        for attr in crate::config::label::template_attrs(&rule.template) {
            if !out.iter().any(|a| a.eq_ignore_ascii_case(&attr)) {
                out.push(attr);
            }
        }
    }
    out
}

/// Render a node's display label: the FIRST rule whose object_classes are all
/// present in `node_ocs` (case-insensitive), rendered against `attrs`. If no rule
/// matches or the render is blank, return `fallback` (the structural label).
fn render_node_label(
    rules: &[LabelRule],
    node_ocs: &[String],
    attrs: &BTreeMap<String, Vec<String>>,
    fallback: &str,
) -> String {
    for rule in rules {
        let all_present = rule
            .object_classes
            .iter()
            .all(|want| node_ocs.iter().any(|have| have.eq_ignore_ascii_case(want)));
        if all_present {
            let rendered = crate::config::label::render_label(&rule.template, attrs);
            if !rendered.is_empty() {
                return rendered;
            }
            return fallback.to_string();
        }
    }
    fallback.to_string()
}

pub(crate) fn compute_rows(
    structure: &Structure,
    branch: &str,
    search: &str,
    rules: &[LabelRule],
) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    if let Some(node) = structure.get(branch) {
        rows.push((format!("‹self› {}", node.label), branch.to_string()));
    }
    // Match the incremental search against the RENDERED label (what the operator
    // sees) — which contains every property used in the profile's label template —
    // not just the structural cn. Get every leaf (empty query) and filter here.
    let q = search.to_lowercase();
    for leaf in structure.filter_leaves(branch, "") {
        let label = render_node_label(rules, &leaf.object_classes, &leaf.attrs, &leaf.label);
        if q.is_empty() || label.to_lowercase().contains(&q) {
            rows.push((label, leaf.dn.clone()));
        }
    }
    rows
}

/// Build the eager-[`Structure`] input row for a freshly created entry from its
/// DN and the attributes that were sent (the structure model derives the display
/// label from cn → description → RDN). Pure.
pub(crate) fn structure_input_from_attrs(
    dn: &str,
    attrs: &BTreeMap<String, Vec<String>>,
) -> StructureInput {
    let first = |name: &str| {
        attrs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .and_then(|(_, v)| v.first().cloned())
    };
    let object_classes = attrs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("objectClass"))
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    StructureInput {
        dn: dn.to_string(),
        cn: first("cn"),
        description: first("description"),
        object_classes,
        attrs: attrs.clone(),
    }
}

/// Map the worker's raw structure rows into the pure model's input rows. Pure.
pub(crate) fn structure_inputs(nodes: Vec<StructureNodeRaw>) -> Vec<StructureInput> {
    nodes
        .into_iter()
        .map(|n| StructureInput {
            dn: n.dn,
            cn: n.cn,
            description: n.description,
            attrs: n.attrs,
            object_classes: n.object_classes,
        })
        .collect()
}

/// Build the pane-1 tree items from the eager [`Structure`]. Only branch nodes
/// appear in the tree (leaves are listed in pane 2); the identifier is the DN so
/// `tree_state.selected()` yields the branch DN. (Port of the facade's
/// `build_structure_tree`.)
pub(crate) fn build_tree_items(structure: &Structure) -> Vec<TreeItem<'static, String>> {
    fn build(structure: &Structure, dn: &str) -> TreeItem<'static, String> {
        let label = structure
            .get(dn)
            .map(|n| n.label.clone())
            .unwrap_or_else(|| dn.split(',').next().unwrap_or(dn).trim().to_string());
        let mut children = Vec::new();
        if let Some(n) = structure.get(dn) {
            for child_dn in &n.children {
                if structure
                    .get(child_dn)
                    .map(|c| c.is_branch())
                    .unwrap_or(false)
                {
                    children.push(build(structure, child_dn));
                }
            }
        }
        if children.is_empty() {
            TreeItem::new_leaf(dn.to_string(), label)
        } else {
            TreeItem::new(dn.to_string(), label, children).expect("DNs are unique ids")
        }
    }
    vec![build(structure, structure.root_dn())]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::app::test_support::*;

    #[test]
    fn compute_rows_lists_self_then_leaves() {
        let s = structure();
        // Empty rules → today's behavior: the leaf label is the structural cn.
        let rows = compute_rows(&s, "ou=users,dc=example,dc=org", "", &[]);
        assert_eq!(rows[0].0, "‹self› ou=users");
        assert_eq!(
            rows[1],
            (
                "Jane".to_string(),
                "uid=jane,ou=users,dc=example,dc=org".to_string()
            )
        );
        assert_eq!(
            compute_rows(&s, "ou=users,dc=example,dc=org", "zzz", &[]).len(),
            1
        );
    }

    #[test]
    fn compute_rows_renders_leaf_via_matching_label_rule() {
        let s = structure();
        let rules = vec![LabelRule {
            object_classes: vec!["inetOrgPerson".into()],
            template: crate::config::label::parse_label_template("{cn} ({uid})"),
        }];
        let rows = compute_rows(&s, "ou=users,dc=example,dc=org", "", &rules);
        // The ‹self› container row is never templated.
        assert_eq!(rows[0].0, "‹self› ou=users");
        // The leaf renders via its profile's template.
        assert_eq!(rows[1].0, "Jane (jane)");
    }

    #[test]
    fn compute_rows_search_matches_template_properties() {
        let s = structure();
        let rules = vec![LabelRule {
            object_classes: vec!["inetOrgPerson".into()],
            template: crate::config::label::parse_label_template("{cn} ({uid})"),
        }];
        // Searching the uid (only visible inside the rendered "(jane)") matches,
        // even though the structural label is "Jane".
        let hit = compute_rows(&s, "ou=users,dc=example,dc=org", "jane", &rules);
        assert_eq!(hit.len(), 2, "self row + the matched leaf");
        assert_eq!(hit[1].0, "Jane (jane)");
        // A term in neither the cn nor the uid still filters the leaf out.
        let miss = compute_rows(&s, "ou=users,dc=example,dc=org", "zzz", &rules);
        assert_eq!(miss.len(), 1, "only the self row");
    }

    #[test]
    fn tree_items_contain_only_branches() {
        let s = structure();
        let items = build_tree_items(&s);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].children().len(), 1);
    }

    // ── per-profile label rules (pure) ───────────────────────────────────────────

    #[test]
    fn label_rules_compiles_only_profiles_with_a_label() {
        let mut with = bare_profile("user");
        with.object_classes = vec!["inetOrgPerson".into()];
        with.label = Some("{cn} ({uid})".into());
        let without = bare_profile("group"); // label = None
        let rules = label_rules(&[with, without]);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].object_classes, vec!["inetOrgPerson".to_string()]);
    }

    #[test]
    fn label_rule_attrs_unions_and_dedups_case_insensitively() {
        let rules = vec![
            rule(&["inetOrgPerson"], "{cn} ({uid})"),
            rule(&["device"], "{CN}-{serial}"),
        ];
        let attrs = label_rule_attrs(&rules);
        // cn (case-folded dup dropped), uid, serial — config order preserved.
        assert_eq!(
            attrs,
            vec!["cn".to_string(), "uid".to_string(), "serial".to_string()]
        );
    }

    #[test]
    fn render_node_label_uses_first_matching_rule() {
        let rules = vec![rule(&["inetOrgPerson"], "{cn} ({uid})")];
        let attrs = attr_map(&[("cn", &["Bob Baker"]), ("uid", &["bob"])]);
        let ocs = vec!["inetOrgPerson".to_string(), "posixAccount".to_string()];
        assert_eq!(
            render_node_label(&rules, &ocs, &attrs, "fallback"),
            "Bob Baker (bob)"
        );
    }

    #[test]
    fn render_node_label_matches_object_class_case_insensitively() {
        let rules = vec![rule(&["inetOrgPerson"], "{uid}")];
        let attrs = attr_map(&[("uid", &["bob"])]);
        let ocs = vec!["INETORGPERSON".to_string()];
        assert_eq!(render_node_label(&rules, &ocs, &attrs, "fb"), "bob");
    }

    #[test]
    fn render_node_label_falls_back_when_no_rule_matches() {
        let rules = vec![rule(&["device"], "{cn}")];
        let attrs = attr_map(&[("cn", &["Bob"])]);
        let ocs = vec!["inetOrgPerson".to_string()];
        assert_eq!(render_node_label(&rules, &ocs, &attrs, "RDN"), "RDN");
    }

    #[test]
    fn render_node_label_partial_render_shows_empty_segment_not_fallback() {
        // `uid` missing → "Bob ()" (non-empty) is kept; only an all-empty render falls back.
        let rules = vec![rule(&["inetOrgPerson"], "{cn} ({uid})")];
        let attrs = attr_map(&[("cn", &["Bob"])]);
        let ocs = vec!["inetOrgPerson".to_string()];
        assert_eq!(render_node_label(&rules, &ocs, &attrs, "RDN"), "Bob ()");
    }

    #[test]
    fn render_node_label_blank_render_falls_back() {
        // Template is a single missing field → "" → fallback.
        let rules = vec![rule(&["inetOrgPerson"], "{uid}")];
        let attrs = attr_map(&[]); // no uid
        let ocs = vec!["inetOrgPerson".to_string()];
        assert_eq!(render_node_label(&rules, &ocs, &attrs, "RDN"), "RDN");
    }

    #[test]
    fn render_node_label_rule_order_is_respected() {
        let rules = vec![
            rule(&["inetOrgPerson"], "first:{uid}"),
            rule(&["inetOrgPerson"], "second:{uid}"),
        ];
        let attrs = attr_map(&[("uid", &["bob"])]);
        let ocs = vec!["inetOrgPerson".to_string()];
        assert_eq!(render_node_label(&rules, &ocs, &attrs, "fb"), "first:bob");
    }

    #[test]
    fn render_node_label_requires_all_object_classes_present() {
        let rules = vec![rule(&["inetOrgPerson", "posixAccount"], "{uid}")];
        let attrs = attr_map(&[("uid", &["bob"])]);
        // Only one of the two required OCs present → no match → fallback.
        let ocs = vec!["inetOrgPerson".to_string()];
        assert_eq!(render_node_label(&rules, &ocs, &attrs, "RDN"), "RDN");
        // Both present → match.
        let ocs = vec!["inetOrgPerson".to_string(), "posixAccount".to_string()];
        assert_eq!(render_node_label(&rules, &ocs, &attrs, "RDN"), "bob");
    }
}
