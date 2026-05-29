//! DIT browser controller and pure helpers (tty-free, unit-tested).
//!
//! The browser lazily expands one level at a time. On an explicit expand of an
//! unloaded node, [`BrowserState::request_children`] submits a one-level search
//! and records the in-flight id → tree-node handle in `pending`. The manual
//! event loop's idle hook polls the worker and feeds each [`Response`] to
//! [`BrowserState::on_response`], which correlates by id (D4) and returns the
//! parent handle plus its freshly built child payloads for the facade to attach.
//!
//! Facade boundary (Definition of Done): this module must NOT import
//! `turbo_vision`. The concrete tree-node type lives behind the facade, so
//! [`BrowserState`] is generic over an opaque node handle `N` (the facade
//! instantiates it with `Rc<RefCell<Node<BrowserNode>>>`). A handle must expose
//! the node's DN and a way to mark it loaded; that contract is the
//! [`ExpandableNode`] trait, implemented in the facade.

use std::collections::HashMap;

use anyhow::Result;

pub use crate::ldap::worker::Response;
use crate::ldap::worker::{LdapEntry, Request, SearchScope, WorkerHandle};

/// The payload carried by each outline tree node (D1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserNode {
    /// Full DN of this entry; the base for searching its children.
    pub dn: String,
    /// Human-readable display label (labels-everywhere, spec §7).
    pub label: String,
    /// Whether this node's children have been fetched yet.
    pub loaded: bool,
    /// objectClass values, used later to drive the read-only form.
    pub object_classes: Vec<String>,
}

/// Behaviour the browser needs from a tree-node handle without naming the
/// concrete (turbo-vision) node type. Implemented by the facade for its real
/// `Rc<RefCell<Node<BrowserNode>>>`, and by a plain wrapper in this module's
/// tests.
pub trait ExpandableNode {
    /// The DN to search children under.
    fn dn(&self) -> String;
    /// Mark the node as having had its children loaded.
    fn mark_loaded(&self);
}

/// Case-insensitive lookup of the first value of an attribute.
fn first_value<'a>(entry: &'a LdapEntry, attr: &str) -> Option<&'a str> {
    entry
        .attrs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(attr))
        .and_then(|(_, v)| v.first())
        .map(|s| s.as_str())
}

/// All values of an attribute (case-insensitive name match), cloned.
fn all_values(entry: &LdapEntry, attr: &str) -> Vec<String> {
    entry
        .attrs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(attr))
        .map(|(_, v)| v.clone())
        .unwrap_or_default()
}

/// The leftmost RDN component of a DN (e.g. `cn=alice` from
/// `cn=alice,dc=example,dc=org`).
fn rdn_of(dn: &str) -> &str {
    dn.split(',').next().unwrap_or(dn).trim()
}

/// Pick a human-readable label for an entry: prefer `cn`, then `description`,
/// else the supplied RDN fallback (labels-everywhere, spec §7). Lookups are
/// case-insensitive.
pub fn node_label(entry: &LdapEntry, rdn_fallback: &str) -> String {
    if let Some(cn) = first_value(entry, "cn") {
        return cn.to_string();
    }
    if let Some(desc) = first_value(entry, "description") {
        return desc.to_string();
    }
    rdn_fallback.to_string()
}

/// Convert search-result entries into unloaded child payloads, pulling
/// objectClass values and computing a display label from cn/description/RDN.
pub fn entries_to_nodes(entries: &[LdapEntry]) -> Vec<BrowserNode> {
    entries
        .iter()
        .map(|e| {
            let fallback = rdn_of(&e.dn);
            BrowserNode {
                dn: e.dn.clone(),
                label: node_label(e, fallback),
                loaded: false,
                object_classes: all_values(e, "objectClass"),
            }
        })
        .collect()
}

/// Drives lazy expansion: assigns correlation ids, tracks in-flight requests,
/// and resolves polled responses back to the awaiting tree-node handle.
///
/// Generic over the opaque node handle `N` so this module stays free of
/// `turbo_vision`. The facade instantiates `BrowserState<facade::NodeRef>`.
pub struct BrowserState<N: ExpandableNode + Clone> {
    pending: HashMap<u64, N>,
    next_id: u64,
    /// The base DN the browser is rooted at (kept for the root node / re-rooting).
    pub base_dn: String,
}

impl<N: ExpandableNode + Clone> BrowserState<N> {
    /// Create a browser rooted at `base_dn`.
    pub fn new(base_dn: impl Into<String>) -> Self {
        BrowserState {
            pending: HashMap::new(),
            next_id: 1,
            base_dn: base_dn.into(),
        }
    }

    /// Submit a one-level search for the children of `node` and record the
    /// in-flight id → node mapping. Returns the assigned correlation id.
    pub fn request_children(&mut self, worker: &WorkerHandle, node: &N) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        worker.submit(Request::Search {
            id,
            base: node.dn(),
            scope: SearchScope::OneLevel,
            filter: "(objectClass=*)".to_string(),
            attrs: vec![
                "cn".to_string(),
                "description".to_string(),
                "objectClass".to_string(),
            ],
        })?;
        self.pending.insert(id, node.clone());
        Ok(id)
    }

    /// Correlate a polled [`Response`] to a pending request. On a matching
    /// `Entries`, mark the parent node loaded and return it with the child
    /// payloads to attach; otherwise (`SearchError`, unknown id, other variant)
    /// return `None`. On a `SearchError` whose id is pending, the entry is
    /// removed from `pending` so it is not retried forever.
    pub fn on_response(&mut self, resp: &Response) -> Option<(N, Vec<BrowserNode>)> {
        match resp {
            Response::Entries { id, entries } => {
                let node = self.pending.remove(id)?;
                node.mark_loaded();
                let children = entries_to_nodes(entries);
                Some((node, children))
            }
            Response::SearchError { id, .. } => {
                self.pending.remove(id);
                None
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::rc::Rc;

    fn entry(dn: &str, attrs: &[(&str, &[&str])]) -> LdapEntry {
        let mut map = BTreeMap::new();
        for (k, vs) in attrs {
            map.insert(
                k.to_string(),
                vs.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            );
        }
        LdapEntry {
            dn: dn.to_string(),
            attrs: map,
            bin_attrs: BTreeMap::new(),
        }
    }

    #[test]
    fn label_prefers_cn() {
        let e = entry(
            "uid=alice,dc=example,dc=org",
            &[("cn", &["Alice Adams"]), ("description", &["ignored"])],
        );
        assert_eq!(node_label(&e, "uid=alice"), "Alice Adams");
    }

    #[test]
    fn label_falls_back_to_description_then_rdn() {
        let with_desc = entry("ou=x,dc=org", &[("description", &["Team X"])]);
        assert_eq!(node_label(&with_desc, "ou=x"), "Team X");
        let bare = entry("ou=x,dc=org", &[]);
        assert_eq!(node_label(&bare, "ou=x"), "ou=x");
    }

    #[test]
    fn entries_become_unloaded_nodes() {
        let entries = vec![
            entry(
                "cn=alice,dc=example,dc=org",
                &[("cn", &["alice"]), ("objectClass", &["top", "person"])],
            ),
            entry("ou=groups,dc=example,dc=org", &[("ou", &["groups"])]),
        ];
        let nodes = entries_to_nodes(&entries);
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].dn, "cn=alice,dc=example,dc=org");
        assert_eq!(nodes[0].label, "alice");
        assert!(!nodes[0].loaded);
        assert_eq!(nodes[0].object_classes, vec!["top", "person"]);
        // No cn/description -> RDN fallback.
        assert_eq!(nodes[1].label, "ou=groups");
    }

    /// A tty-free fake node handle standing in for the facade's real tree node.
    #[derive(Clone)]
    struct FakeNode(Rc<RefCell<BrowserNode>>);

    impl FakeNode {
        fn new(dn: &str) -> Self {
            FakeNode(Rc::new(RefCell::new(BrowserNode {
                dn: dn.to_string(),
                label: dn.to_string(),
                loaded: false,
                object_classes: vec![],
            })))
        }
        fn loaded(&self) -> bool {
            self.0.borrow().loaded
        }
    }

    impl ExpandableNode for FakeNode {
        fn dn(&self) -> String {
            self.0.borrow().dn.clone()
        }
        fn mark_loaded(&self) {
            self.0.borrow_mut().loaded = true;
        }
    }

    #[test]
    fn on_response_correlates_matching_id_and_marks_loaded() {
        let mut state: BrowserState<FakeNode> = BrowserState::new("dc=example,dc=org");
        let parent = FakeNode::new("dc=example,dc=org");
        state.pending.insert(42, parent.clone());

        let resp = Response::Entries {
            id: 42,
            entries: vec![entry("ou=people,dc=example,dc=org", &[("ou", &["people"])])],
        };
        let (returned, children) = state.on_response(&resp).expect("should correlate id 42");
        assert!(Rc::ptr_eq(&returned.0, &parent.0));
        assert!(returned.loaded(), "parent marked loaded");
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].label, "ou=people");
        assert!(state.pending.is_empty());
    }

    #[test]
    fn on_response_ignores_unknown_id() {
        let mut state: BrowserState<FakeNode> = BrowserState::new("dc=example,dc=org");
        state.pending.insert(1, FakeNode::new("dc=example,dc=org"));
        let resp = Response::Entries {
            id: 999,
            entries: vec![],
        };
        assert!(state.on_response(&resp).is_none());
        assert_eq!(state.pending.len(), 1);
    }

    #[test]
    fn on_response_drops_pending_on_search_error() {
        let mut state: BrowserState<FakeNode> = BrowserState::new("dc=example,dc=org");
        state.pending.insert(5, FakeNode::new("dc=example,dc=org"));
        let resp = Response::SearchError {
            id: 5,
            msg: "boom".to_string(),
        };
        assert!(state.on_response(&resp).is_none());
        assert!(state.pending.is_empty(), "errored id removed from pending");
    }

    #[test]
    fn interleaved_pending_ids_resolve_to_their_own_nodes() {
        let mut state: BrowserState<FakeNode> = BrowserState::new("dc=example,dc=org");
        let a = FakeNode::new("ou=a,dc=example,dc=org");
        let b = FakeNode::new("ou=b,dc=example,dc=org");
        state.pending.insert(10, a.clone());
        state.pending.insert(11, b.clone());

        let (n11, _) = state
            .on_response(&Response::Entries {
                id: 11,
                entries: vec![],
            })
            .unwrap();
        assert!(Rc::ptr_eq(&n11.0, &b.0));
        let (n10, _) = state
            .on_response(&Response::Entries {
                id: 10,
                entries: vec![],
            })
            .unwrap();
        assert!(Rc::ptr_eq(&n10.0, &a.0));
    }
}
