//! Pure, tty-free model of the eagerly-loaded DIT structure.
//!
//! Built once from the flat paged scan, it answers — without further queries —
//! which entries are **containers** (either classified as such by `objectClass`,
//! e.g. `organizationalUnit`, or simply because they have children), the direct
//! children of a container (both sub-containers and leaf entries) for the entry
//! list, incremental-search filtering, and the local reflow when an entry gains
//! its first child (promote) or loses its last (demote) — a signal the tree
//! pane still needs even though a classified container no longer disappears
//! from the tree when it loses its last child.

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
    /// All returned string attributes (used to render per-profile label templates).
    pub attrs: BTreeMap<String, Vec<String>>,
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
    /// All returned string attributes (used to render per-profile label templates).
    pub attrs: BTreeMap<String, Vec<String>>,
    /// Child DNs, in input order.
    pub children: Vec<String>,
}

/// Well-known objectClasses that mark an entry as a container even with zero
/// children (an empty `organizationalUnit` must still show up in the tree, not
/// vanish into the leaf list). Hardcoded for now; a future config layer could
/// let a deployment extend this for a custom schema (e.g. a proprietary
/// "department" auxiliary class).
const CONTAINER_OBJECT_CLASSES: &[&str] = &[
    "organizationalUnit",
    "organization",
    "dcObject",
    "domain",
    "container",
];

impl StructureNode {
    /// A node is a branch iff it has at least one child.
    ///
    /// This is purely a child-count predicate — it is what the promote/demote
    /// reflow in [`Structure::upsert`]/[`Structure::remove`] cares about ("did
    /// this node just gain its first child, or lose its last") and must stay
    /// child-count-only for that to work. Use [`Self::is_container`] for "is
    /// this a container" (tree membership, leaf-list exclusion) — a container by
    /// objectClass with zero children is a branch=false but container=true node.
    pub fn is_branch(&self) -> bool {
        !self.children.is_empty()
    }

    /// A node is a **container** — shown in the DIT tree (panel 1) and excluded
    /// from the plain leaf listing — iff it has ≥1 child (whatever its class) OR
    /// its `objectClass` matches one of [`CONTAINER_OBJECT_CLASSES`]
    /// case-insensitively. The objectClass check is what makes an *empty* OU
    /// still show up in the tree instead of masquerading as a leaf entry.
    pub fn is_container(&self) -> bool {
        self.is_branch()
            || self.object_classes.iter().any(|oc| {
                CONTAINER_OBJECT_CLASSES
                    .iter()
                    .any(|c| oc.eq_ignore_ascii_case(c))
            })
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
                    attrs: inp.attrs.clone(),
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
                attrs: BTreeMap::new(),
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

    /// All container DNs (see [`StructureNode::is_container`]); the root is
    /// included if it is one. These are the DNs the DIT tree (panel 1) shows —
    /// an empty `organizationalUnit` is included even though it has no children.
    pub fn branch_dns(&self) -> Vec<String> {
        self.nodes
            .values()
            .filter(|n| n.is_container())
            .map(|n| n.dn.clone())
            .collect()
    }

    /// All direct children of `branch_dn` — both sub-containers and leaf
    /// entries — in input order. This is the panel-2 (entry list) row source:
    /// the operator sees everything directly under the selected container, not
    /// just its leaves.
    pub fn children_of(&self, branch_dn: &str) -> Vec<&StructureNode> {
        let Some(branch) = self.nodes.get(branch_dn) else {
            return Vec::new();
        };
        branch
            .children
            .iter()
            .filter_map(|c| self.nodes.get(c))
            .collect()
    }

    /// The leaf children (not containers — see [`StructureNode::is_container`])
    /// directly under `branch_dn`, in input order. Kept for consumers that
    /// genuinely want leaves only; panel 2 uses [`Self::children_of`] instead so
    /// sub-containers are shown too.
    pub fn leaves_of(&self, branch_dn: &str) -> Vec<&StructureNode> {
        self.children_of(branch_dn)
            .into_iter()
            .filter(|c| !c.is_container())
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

    /// Insert or update the node for `input.dn`, preserving any children the node
    /// already has, and link it under its parent when that parent is a known node.
    ///
    /// This is the single production mutation point for the structure: it is fed by
    /// every entry read (navigation and post-write re-read alike), so a create, a
    /// rename's new DN, and a label-changing edit all reflow through one path. An
    /// entry whose parent lies outside the loaded base is inserted but stays
    /// unlinked — hence invisible — exactly as [`Structure::build`] treats it.
    ///
    /// Returns `true` when the **tree pane** must rebuild: either the parent flipped
    /// leaf→branch, or an existing branch's attributes changed (the tree renders
    /// branch labels from `attrs`).
    pub fn upsert(&mut self, input: StructureInput) -> bool {
        let dn = input.dn.clone();
        let parent = parent_of(&dn).map(str::to_string);
        let parent_was_branch = parent
            .as_ref()
            .and_then(|p| self.nodes.get(p))
            .map(|n| n.is_branch())
            .unwrap_or(false);

        // Preserve the existing subtree links; note whether this node is itself a
        // branch whose rendered attributes change.
        let (children, was_branch, attrs_changed) = match self.nodes.get(&dn) {
            Some(n) => (n.children.clone(), n.is_branch(), n.attrs != input.attrs),
            None => (Vec::new(), false, false),
        };

        self.nodes.insert(
            dn.clone(),
            StructureNode {
                dn: dn.clone(),
                label: label_for(&input),
                object_classes: input.object_classes,
                attrs: input.attrs,
                children,
            },
        );
        if let Some(p) = parent.as_ref().and_then(|p| self.nodes.get_mut(p)) {
            if !p.children.iter().any(|c| c.eq_ignore_ascii_case(&dn)) {
                p.children.push(dn.clone());
            }
        }

        let parent_is_branch = parent
            .as_ref()
            .and_then(|p| self.nodes.get(p))
            .map(|n| n.is_branch())
            .unwrap_or(false);
        (!parent_was_branch && parent_is_branch) || (was_branch && attrs_changed)
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
        input_oc(dn, cn, desc, &[])
    }

    fn input_oc(dn: &str, cn: Option<&str>, desc: Option<&str>, ocs: &[&str]) -> StructureInput {
        StructureInput {
            dn: dn.to_string(),
            cn: cn.map(str::to_string),
            description: desc.map(str::to_string),
            object_classes: ocs.iter().map(|s| s.to_string()).collect(),
            attrs: Default::default(),
        }
    }

    fn fixture() -> Structure {
        // dc=example,dc=org           (domain/dcObject; children: admin, users, empty)
        //   cn=admin                  (inetOrgPerson leaf, directly under root)
        //   ou=users                  (organizationalUnit; child: jane)
        //     uid=jane                (inetOrgPerson leaf)
        //   ou=empty                  (organizationalUnit, NO children — a container by
        //                              objectClass alone; this is the "empty OU" bug case)
        Structure::build(
            "dc=example,dc=org",
            vec![
                input_oc(
                    "dc=example,dc=org",
                    None,
                    Some("Example"),
                    &["dcObject", "organization"],
                ),
                input_oc(
                    "cn=admin,dc=example,dc=org",
                    Some("Admin"),
                    None,
                    &["inetOrgPerson"],
                ),
                input_oc(
                    "ou=users,dc=example,dc=org",
                    None,
                    None,
                    &["organizationalUnit"],
                ),
                input_oc(
                    "uid=jane,ou=users,dc=example,dc=org",
                    Some("Jane Doe"),
                    None,
                    &["inetOrgPerson"],
                ),
                input_oc(
                    "ou=empty,dc=example,dc=org",
                    None,
                    None,
                    &["organizationalUnit"],
                ),
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
        // ou=empty has NO children — is_branch is purely child-count, so this
        // stays false even though it IS a container by objectClass (see
        // `is_container_true_for_empty_organizational_unit` below).
        assert!(!s.get("ou=empty,dc=example,dc=org").unwrap().is_branch());
        assert!(!s
            .get("uid=jane,ou=users,dc=example,dc=org")
            .unwrap()
            .is_branch());
        assert!(!s.get("cn=admin,dc=example,dc=org").unwrap().is_branch());
        assert!(s.get("dc=example,dc=org").unwrap().is_branch());
    }

    #[test]
    fn is_container_true_for_empty_organizational_unit() {
        let s = fixture();
        // The core bug fix: a childless OU is still a container, by objectClass.
        assert!(s.get("ou=empty,dc=example,dc=org").unwrap().is_container());
    }

    #[test]
    fn is_container_false_for_leaf_with_no_children() {
        let s = fixture();
        assert!(!s
            .get("uid=jane,ou=users,dc=example,dc=org")
            .unwrap()
            .is_container());
        assert!(!s.get("cn=admin,dc=example,dc=org").unwrap().is_container());
    }

    #[test]
    fn is_container_true_for_any_node_with_children_regardless_of_class() {
        // A node classed as a plain leaf class that nonetheless has a child
        // (unusual, but structurally possible) is still a container — the
        // has-children fallback in `is_container` covers it.
        let s = Structure::build(
            "dc=x",
            vec![
                input_oc("dc=x", None, None, &[]),
                input_oc("cn=weird,dc=x", None, None, &["inetOrgPerson"]),
                input_oc("cn=kid,cn=weird,dc=x", None, None, &["inetOrgPerson"]),
            ],
        );
        assert!(
            s.get("cn=weird,dc=x").unwrap().is_container(),
            "has children ⇒ container regardless of objectClass"
        );
    }

    #[test]
    fn is_container_true_for_each_known_container_class_case_insensitively() {
        for oc in [
            "organizationalUnit",
            "ORGANIZATION",
            "dcObject",
            "Domain",
            "CONTAINER",
        ] {
            let s = Structure::build("dc=x", vec![input_oc("dc=x", None, None, &[oc])]);
            assert!(
                s.get("dc=x").unwrap().is_container(),
                "{oc} must classify as a container"
            );
        }
        // A plain leaf class is not a container.
        let s = Structure::build(
            "dc=x",
            vec![input_oc("dc=x", None, None, &["inetOrgPerson"])],
        );
        assert!(!s.get("dc=x").unwrap().is_container());
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
    fn branch_dns_includes_containers_even_with_no_children() {
        let s = fixture();
        let mut branches = s.branch_dns();
        branches.sort();
        // ou=empty (a childless OU) now appears in the tree — the fix for bug (a).
        assert_eq!(
            branches,
            vec![
                "dc=example,dc=org",
                "ou=empty,dc=example,dc=org",
                "ou=users,dc=example,dc=org",
            ]
        );
    }

    #[test]
    fn children_of_returns_all_direct_children_containers_and_leaves() {
        let s = fixture();
        // Root's children: a leaf (admin) and two containers (users, empty) — all
        // three must be present; this is the panel-2 row source.
        let dns: Vec<&str> = s
            .children_of("dc=example,dc=org")
            .iter()
            .map(|n| n.dn.as_str())
            .collect();
        assert_eq!(
            dns,
            vec![
                "cn=admin,dc=example,dc=org",
                "ou=users,dc=example,dc=org",
                "ou=empty,dc=example,dc=org",
            ]
        );
    }

    #[test]
    fn leaves_of_excludes_containers_even_when_childless() {
        let s = fixture();
        // Under root: ou=users and ou=empty are BOTH containers now (one by
        // children, one by objectClass) — only the leaf (admin) remains.
        let leaves: Vec<&str> = s
            .leaves_of("dc=example,dc=org")
            .iter()
            .map(|n| n.dn.as_str())
            .collect();
        assert_eq!(leaves, vec!["cn=admin,dc=example,dc=org"]);
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
    fn upsert_preserves_existing_children() {
        // Upserting a CONTAINER must never orphan its subtree.
        let mut s = fixture();
        let changed = s.upsert(input("ou=users,dc=example,dc=org", Some("Users"), None));
        assert!(
            !changed,
            "updating an existing branch's label is not a flip"
        );
        let node = s.get("ou=users,dc=example,dc=org").unwrap();
        assert_eq!(node.label, "Users", "label refreshed from the new input");
        assert_eq!(
            node.children,
            vec!["uid=jane,ou=users,dc=example,dc=org".to_string()],
            "children survive the upsert"
        );
    }

    #[test]
    fn upsert_links_new_node_under_known_parent() {
        let mut s = fixture();
        let changed = s.upsert(input(
            "uid=bob,ou=users,dc=example,dc=org",
            Some("Bob"),
            None,
        ));
        assert!(!changed, "parent was already a branch — no tree flip");
        let leaves: Vec<&str> = s
            .leaves_of("ou=users,dc=example,dc=org")
            .iter()
            .map(|n| n.dn.as_str())
            .collect();
        assert!(leaves.contains(&"uid=bob,ou=users,dc=example,dc=org"));
    }

    #[test]
    fn upsert_promoting_parent_leaf_to_branch_returns_true() {
        let mut s = fixture();
        // ou=empty has no children yet: the first child flips is_branch false->true
        // (it was already is_container==true via objectClass, so this is purely
        // the has-children reflow signal the tree rebuild depends on).
        let changed = s.upsert(input("cn=x,ou=empty,dc=example,dc=org", Some("X"), None));
        assert!(changed, "leaf->branch flip must request a tree rebuild");
        assert!(s.get("ou=empty,dc=example,dc=org").unwrap().is_branch());
    }

    #[test]
    fn upsert_changing_a_branch_attr_returns_true() {
        // The tree pane renders branch labels from `attrs`, so an attr change on a
        // BRANCH must request a rebuild.
        let mut s = fixture();
        let mut inp = input("ou=users,dc=example,dc=org", None, None);
        inp.attrs
            .insert("description".to_string(), vec!["Staff".to_string()]);
        assert!(s.upsert(inp), "branch attrs changed → rebuild");
    }

    #[test]
    fn upsert_unchanged_leaf_returns_false() {
        let mut s = fixture();
        assert!(
            !s.upsert(input(
                "uid=jane,ou=users,dc=example,dc=org",
                Some("Jane Doe"),
                None
            )),
            "re-upserting an unchanged leaf is not a tree change"
        );
    }

    #[test]
    fn upsert_with_unknown_parent_inserts_unlinked() {
        // An entry outside the loaded base: inserted, but not reachable as a leaf.
        let mut s = fixture();
        s.upsert(input(
            "uid=zoe,ou=other,dc=elsewhere,dc=org",
            Some("Zoe"),
            None,
        ));
        assert!(s.get("uid=zoe,ou=other,dc=elsewhere,dc=org").is_some());
        assert!(s.leaves_of("ou=other,dc=elsewhere,dc=org").is_empty());
    }

    #[test]
    fn rename_modelled_as_remove_then_upsert_leaves_no_stale_node() {
        let mut s = fixture();
        s.remove("uid=jane,ou=users,dc=example,dc=org");
        s.upsert(input(
            "uid=jane2,ou=users,dc=example,dc=org",
            Some("Jane Doe"),
            None,
        ));
        assert!(s.get("uid=jane,ou=users,dc=example,dc=org").is_none());
        let leaves: Vec<&str> = s
            .leaves_of("ou=users,dc=example,dc=org")
            .iter()
            .map(|n| n.dn.as_str())
            .collect();
        assert_eq!(leaves, vec!["uid=jane2,ou=users,dc=example,dc=org"]);
    }

    #[test]
    fn demote_marks_parent_as_leaf_on_last_child_removed() {
        let mut s = fixture();
        let changed = s.remove("uid=jane,ou=users,dc=example,dc=org");
        assert!(changed, "branch->leaf is a reflow");
        assert!(!s.get("ou=users,dc=example,dc=org").unwrap().is_branch());
    }
}
