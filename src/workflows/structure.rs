//! Pure, tty-free model of the eagerly-loaded DIT structure.
//!
//! Built once from the flat paged scan, it answers — without further queries —
//! which entries are branches (have children), the leaves directly under a branch,
//! incremental-search filtering of those leaves, and the local reflow when an entry
//! gains its first child (promote) or loses its last (demote).

use std::collections::BTreeMap;

/// Input row for building the structure (mapped from the worker's `StructureNodeRaw`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructureInput {
    /// Distinguished name.
    pub dn: String,
    /// `cn` first value, if any.
    pub cn: Option<String>,
    /// `description` first value, if any.
    pub description: Option<String>,
    /// objectClass values.
    pub object_classes: Vec<String>,
}

/// One node in the structure model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructureNode {
    /// Distinguished name (the key).
    pub dn: String,
    /// Display label (cn → description → RDN).
    pub label: String,
    /// objectClass values.
    pub object_classes: Vec<String>,
    /// Child DNs, in input order.
    pub children: Vec<String>,
}

impl StructureNode {
    /// A node is a branch iff it has at least one child.
    pub fn is_branch(&self) -> bool {
        !self.children.is_empty()
    }
}

/// The whole eager structure: a DN→node map plus the root DN.
#[derive(Debug, Clone)]
pub struct Structure {
    root_dn: String,
    nodes: BTreeMap<String, StructureNode>,
}

/// The leftmost RDN of a DN (`uid=jane` from `uid=jane,ou=users,...`).
fn rdn_of(dn: &str) -> &str {
    dn.split(',').next().unwrap_or(dn).trim()
}

/// The parent DN (everything after the first comma), or `None` at the top.
fn parent_of(dn: &str) -> Option<&str> {
    dn.split_once(',').map(|(_, rest)| rest.trim())
}

/// Choose a label: cn → description → RDN.
fn label_for(input: &StructureInput) -> String {
    if let Some(cn) = input.cn.as_ref().filter(|s| !s.is_empty()) {
        return cn.clone();
    }
    if let Some(d) = input.description.as_ref().filter(|s| !s.is_empty()) {
        return d.clone();
    }
    rdn_of(&input.dn).to_string()
}

impl Structure {
    /// Build the model from the flat scan. Parent/child links are derived from DN
    /// suffixes (case-insensitive). The `root` DN is always present even if the
    /// scan returned nothing for it, so a first child can be created.
    pub fn build(root: &str, inputs: Vec<StructureInput>) -> Structure {
        let mut nodes: BTreeMap<String, StructureNode> = BTreeMap::new();

        // First pass: create every node.
        for inp in &inputs {
            nodes.insert(
                inp.dn.clone(),
                StructureNode {
                    dn: inp.dn.clone(),
                    label: label_for(inp),
                    object_classes: inp.object_classes.clone(),
                    children: Vec::new(),
                },
            );
        }
        // Ensure the root exists.
        nodes
            .entry(root.to_string())
            .or_insert_with(|| StructureNode {
                dn: root.to_string(),
                label: rdn_of(root).to_string(),
                object_classes: Vec::new(),
                children: Vec::new(),
            });

        // Second pass: link each node to its parent (in input order).
        for inp in &inputs {
            if let Some(parent) = parent_of(&inp.dn) {
                // Only link if the parent is a known node (within the loaded base).
                if let Some(p) = nodes.get_mut(parent) {
                    if !p.children.iter().any(|c| c.eq_ignore_ascii_case(&inp.dn)) {
                        p.children.push(inp.dn.clone());
                    }
                }
            }
        }

        Structure {
            root_dn: root.to_string(),
            nodes,
        }
    }

    /// The root (base) DN.
    pub fn root_dn(&self) -> &str {
        &self.root_dn
    }

    /// Look up a node by exact DN.
    pub fn get(&self, dn: &str) -> Option<&StructureNode> {
        self.nodes.get(dn)
    }

    /// All branch DNs (have ≥1 child); the root is included if it is a branch.
    pub fn branch_dns(&self) -> Vec<String> {
        self.nodes
            .values()
            .filter(|n| n.is_branch())
            .map(|n| n.dn.clone())
            .collect()
    }

    /// The leaf children (no children of their own) directly under `branch_dn`,
    /// in input order.
    pub fn leaves_of(&self, branch_dn: &str) -> Vec<&StructureNode> {
        let Some(branch) = self.nodes.get(branch_dn) else {
            return Vec::new();
        };
        branch
            .children
            .iter()
            .filter_map(|c| self.nodes.get(c))
            .filter(|c| !c.is_branch())
            .collect()
    }

    /// `leaves_of` filtered by a case-insensitive substring over the label. An
    /// empty `query` returns all leaves.
    pub fn filter_leaves(&self, branch_dn: &str, query: &str) -> Vec<&StructureNode> {
        let q = query.to_lowercase();
        self.leaves_of(branch_dn)
            .into_iter()
            .filter(|n| q.is_empty() || n.label.to_lowercase().contains(&q))
            .collect()
    }

    /// Add a child node (e.g. after a successful create). Returns true if the
    /// parent changed leaf→branch (a reflow the UI must reflect).
    pub fn add_child(&mut self, parent_dn: &str, child: StructureInput) -> bool {
        let was_branch = self.get(parent_dn).map(|n| n.is_branch()).unwrap_or(false);
        let node = StructureNode {
            dn: child.dn.clone(),
            label: label_for(&child),
            object_classes: child.object_classes.clone(),
            children: Vec::new(),
        };
        self.nodes.insert(child.dn.clone(), node);
        if let Some(p) = self.nodes.get_mut(parent_dn) {
            if !p.children.iter().any(|c| c.eq_ignore_ascii_case(&child.dn)) {
                p.children.push(child.dn);
            }
        }
        let is_branch = self.get(parent_dn).map(|n| n.is_branch()).unwrap_or(false);
        !was_branch && is_branch
    }

    /// Remove a node (e.g. after delete). Returns true if its parent changed
    /// branch→leaf (a reflow the UI must reflect).
    pub fn remove(&mut self, dn: &str) -> bool {
        let parent = parent_of(dn).map(str::to_string);
        self.nodes.remove(dn);
        if let Some(parent) = parent {
            let was_branch = self.get(&parent).map(|n| n.is_branch()).unwrap_or(false);
            if let Some(p) = self.nodes.get_mut(&parent) {
                p.children.retain(|c| !c.eq_ignore_ascii_case(dn));
            }
            let is_branch = self.get(&parent).map(|n| n.is_branch()).unwrap_or(false);
            return was_branch && !is_branch;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(dn: &str, cn: Option<&str>, desc: Option<&str>) -> StructureInput {
        StructureInput {
            dn: dn.to_string(),
            cn: cn.map(str::to_string),
            description: desc.map(str::to_string),
            object_classes: vec![],
        }
    }

    fn fixture() -> Structure {
        // dc=example,dc=org
        //   ou=users        (branch: has jane)
        //     uid=jane
        //   ou=empty        (leaf: no children)
        Structure::build(
            "dc=example,dc=org",
            vec![
                input("dc=example,dc=org", None, Some("Example")),
                input("ou=users,dc=example,dc=org", None, None),
                input(
                    "uid=jane,ou=users,dc=example,dc=org",
                    Some("Jane Doe"),
                    None,
                ),
                input("ou=empty,dc=example,dc=org", None, None),
            ],
        )
    }

    #[test]
    fn label_prefers_cn_then_description_then_rdn() {
        let s = fixture();
        assert_eq!(
            s.get("uid=jane,ou=users,dc=example,dc=org").unwrap().label,
            "Jane Doe"
        );
        assert_eq!(s.get("dc=example,dc=org").unwrap().label, "Example");
        assert_eq!(
            s.get("ou=users,dc=example,dc=org").unwrap().label,
            "ou=users"
        );
    }

    #[test]
    fn branch_is_has_children() {
        let s = fixture();
        assert!(s.get("ou=users,dc=example,dc=org").unwrap().is_branch());
        assert!(!s.get("ou=empty,dc=example,dc=org").unwrap().is_branch());
        assert!(!s
            .get("uid=jane,ou=users,dc=example,dc=org")
            .unwrap()
            .is_branch());
        assert!(s.get("dc=example,dc=org").unwrap().is_branch());
    }

    #[test]
    fn root_is_always_present_even_if_childless() {
        let s = Structure::build(
            "dc=example,dc=org",
            vec![input("dc=example,dc=org", None, None)],
        );
        assert!(s.get("dc=example,dc=org").is_some());
        assert_eq!(s.root_dn(), "dc=example,dc=org");
    }

    #[test]
    fn branch_dns_lists_only_branches_plus_root() {
        let s = fixture();
        let mut branches = s.branch_dns();
        branches.sort();
        assert_eq!(
            branches,
            vec!["dc=example,dc=org", "ou=users,dc=example,dc=org"]
        );
    }

    #[test]
    fn leaves_of_lists_only_leaf_children() {
        let s = fixture();
        // Under root: ou=users is a branch (excluded), ou=empty is a leaf (included).
        let leaves: Vec<&str> = s
            .leaves_of("dc=example,dc=org")
            .iter()
            .map(|n| n.dn.as_str())
            .collect();
        assert_eq!(leaves, vec!["ou=empty,dc=example,dc=org"]);
        // Under ou=users: uid=jane is a leaf.
        let leaves: Vec<&str> = s
            .leaves_of("ou=users,dc=example,dc=org")
            .iter()
            .map(|n| n.dn.as_str())
            .collect();
        assert_eq!(leaves, vec!["uid=jane,ou=users,dc=example,dc=org"]);
    }

    #[test]
    fn filter_leaves_is_case_insensitive_substring_on_label() {
        let s = fixture();
        let hits: Vec<&str> = s
            .filter_leaves("ou=users,dc=example,dc=org", "jane")
            .iter()
            .map(|n| n.dn.as_str())
            .collect();
        assert_eq!(hits, vec!["uid=jane,ou=users,dc=example,dc=org"]);
        assert!(s
            .filter_leaves("ou=users,dc=example,dc=org", "zzz")
            .is_empty());
        // Empty filter returns all leaves.
        assert_eq!(s.filter_leaves("ou=users,dc=example,dc=org", "").len(), 1);
    }

    #[test]
    fn promote_marks_parent_as_branch_on_first_child() {
        let mut s = fixture();
        // ou=empty is a leaf; add a child under it.
        let changed = s.add_child(
            "ou=empty,dc=example,dc=org",
            input("cn=x,ou=empty,dc=example,dc=org", Some("X"), None),
        );
        assert!(changed, "leaf->branch is a reflow");
        assert!(s.get("ou=empty,dc=example,dc=org").unwrap().is_branch());
    }

    #[test]
    fn demote_marks_parent_as_leaf_on_last_child_removed() {
        let mut s = fixture();
        let changed = s.remove("uid=jane,ou=users,dc=example,dc=org");
        assert!(changed, "branch->leaf is a reflow");
        assert!(!s.get("ou=users,dc=example,dc=org").unwrap().is_branch());
    }
}
