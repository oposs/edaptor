//! Async one-level search backing the entry list's incremental find.
//!
//! Pane 2 lists the leaf children of the selected container. Its find used to
//! filter the cached [`crate::workflows::structure::Structure`] projection, so an
//! entry another client created was invisible until restart. This flow answers the
//! find from the directory instead: one `SearchScope::OneLevel` query under the
//! selected branch per keystroke, superseded by the next.
//!
//! Id range 5_000_000+ keeps responses disjoint from ReadFlow (1) / WriteFlow
//! (1_000_000) / AllocFlow (2_000_000) / SearchFlow (3_000_000) / ResolveFlow
//! (4_000_000). Only the *latest* id is tracked; a superseded response is dropped,
//! so the list always shows the newest query's answer.
//!
//! No tvision_rs, no crate::ui — pure domain logic.

use anyhow::Result;

use crate::ldap::worker::{LdapEntry, Request, Response, SearchScope, WorkerHandle};
use crate::workflows::pick_state::escape_filter;

/// Result cap for one find. Generous compared with `PICKER_SEARCH_CAP` because
/// this list is the operator's primary navigation surface, not a picker popup.
pub const LEAF_SEARCH_CAP: i32 = 500;

/// Build the RFC-4515 filter for a find over `attrs`.
///
/// - Empty `term` → `(objectClass=*)` (everything in the container).
/// - One attribute → `(cn=*term*)`.
/// - Several → `(|(cn=*term*)(uid=*term*))`.
/// - No attributes configured → falls back to `cn` + `uid`, so the filter can
///   never degenerate into an invalid empty `(|)`.
///
/// `term` is RFC-4515-escaped, so `*`, `(`, `)`, `\` and NUL are literal.
pub fn build_leaf_filter(attrs: &[String], term: &str) -> String {
    if term.is_empty() {
        return "(objectClass=*)".to_string();
    }
    let fallback = ["cn".to_string(), "uid".to_string()];
    let dims: &[String] = if attrs.is_empty() { &fallback } else { attrs };
    let esc = escape_filter(term);
    let parts: Vec<String> = dims.iter().map(|a| format!("({a}=*{esc}*)")).collect();
    if parts.len() == 1 {
        parts.into_iter().next().unwrap_or_default()
    } else {
        format!("(|{})", parts.join(""))
    }
}

/// The result of correlating one response against the latest find.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeafSearchOutcome {
    /// The latest find returned these entries (capped at [`LEAF_SEARCH_CAP`]).
    Results {
        entries: Vec<LdapEntry>,
        truncated: bool,
    },
    /// The latest find failed; the caller falls back to the cached projection.
    Failed(String),
    /// The response belongs to a superseded find (or another flow).
    Ignored,
}

/// One-level container search, superseded on every keystroke.
pub struct LeafSearchFlow {
    next_id: u64,
    latest: Option<u64>,
}

impl Default for LeafSearchFlow {
    fn default() -> Self {
        Self::new()
    }
}

impl LeafSearchFlow {
    /// Create a new flow. The first allocated id is 5_000_000.
    pub fn new() -> Self {
        LeafSearchFlow {
            next_id: 5_000_000,
            latest: None,
        }
    }

    fn alloc(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Submit a one-level find for `term` under `branch_dn`.
    ///
    /// `filter_attrs` are the dimensions matched (the column-2 label attributes —
    /// what the operator actually sees); `fetch_attrs` are the attributes returned
    /// (the wider label+tree scan set, so an upserted node carries what the tree
    /// pane needs). Records the id as `latest` and returns it.
    pub fn request(
        &mut self,
        worker: &WorkerHandle,
        branch_dn: &str,
        term: &str,
        filter_attrs: &[String],
        fetch_attrs: &[String],
    ) -> Result<u64> {
        let id = self.alloc();
        let mut attrs = fetch_attrs.to_vec();
        for want in ["cn", "description", "objectClass"] {
            if !attrs.iter().any(|a| a.eq_ignore_ascii_case(want)) {
                attrs.push(want.to_string());
            }
        }
        worker.submit(Request::Search {
            id,
            base: branch_dn.to_string(),
            scope: SearchScope::OneLevel,
            filter: build_leaf_filter(filter_attrs, term),
            attrs,
            size_limit: Some(LEAF_SEARCH_CAP),
        })?;
        self.latest = Some(id);
        Ok(id)
    }

    /// Correlate one worker response. Pure; a non-latest id yields `Ignored`.
    pub fn on_response(&mut self, resp: &Response) -> LeafSearchOutcome {
        match resp {
            Response::Entries {
                id,
                entries,
                truncated,
            } => {
                if Some(*id) != self.latest {
                    return LeafSearchOutcome::Ignored;
                }
                LeafSearchOutcome::Results {
                    entries: entries.clone(),
                    truncated: *truncated,
                }
            }
            Response::SearchError { id, msg } => {
                if Some(*id) != self.latest {
                    return LeafSearchOutcome::Ignored;
                }
                LeafSearchOutcome::Failed(msg.clone())
            }
            _ => LeafSearchOutcome::Ignored,
        }
    }

    /// Test-only: set `latest` without submitting, so `on_response` can be driven
    /// with hand-built responses.
    #[cfg(test)]
    pub(crate) fn force_latest(&mut self, id: u64) {
        self.latest = Some(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldap::worker::{LdapEntry, Response};
    use std::collections::BTreeMap;

    #[test]
    fn filter_single_attr_is_a_bare_substring_match() {
        assert_eq!(build_leaf_filter(&["cn".to_string()], "ann"), "(cn=*ann*)");
    }

    #[test]
    fn filter_multiple_attrs_are_ored() {
        assert_eq!(
            build_leaf_filter(&["cn".to_string(), "uid".to_string()], "ann"),
            "(|(cn=*ann*)(uid=*ann*))"
        );
    }

    #[test]
    fn filter_escapes_rfc4515_specials() {
        assert_eq!(
            build_leaf_filter(&["cn".to_string()], "a*b"),
            "(cn=*a\\2ab*)"
        );
    }

    #[test]
    fn filter_falls_back_to_cn_uid_without_configured_attrs() {
        // No label rules configured → never emit an empty "(|)" (invalid filter).
        assert_eq!(build_leaf_filter(&[], "ann"), "(|(cn=*ann*)(uid=*ann*))");
    }

    #[test]
    fn filter_empty_term_matches_everything() {
        assert_eq!(
            build_leaf_filter(&["cn".to_string()], ""),
            "(objectClass=*)"
        );
    }

    #[test]
    fn stale_response_is_ignored() {
        let mut f = LeafSearchFlow::new();
        f.force_latest(5_000_001);
        let resp = Response::Entries {
            id: 5_000_000,
            entries: vec![],
            truncated: false,
        };
        assert!(matches!(f.on_response(&resp), LeafSearchOutcome::Ignored));
    }

    #[test]
    fn latest_response_yields_entries_and_truncation() {
        let mut f = LeafSearchFlow::new();
        f.force_latest(5_000_007);
        let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        attrs.insert("cn".to_string(), vec!["Ann".to_string()]);
        let resp = Response::Entries {
            id: 5_000_007,
            entries: vec![LdapEntry {
                dn: "uid=ann,ou=p,dc=x".to_string(),
                attrs,
                bin_attrs: Default::default(),
            }],
            truncated: true,
        };
        match f.on_response(&resp) {
            LeafSearchOutcome::Results { entries, truncated } => {
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].dn, "uid=ann,ou=p,dc=x");
                assert!(truncated);
            }
            other => panic!("expected Results, got {other:?}"),
        }
    }

    #[test]
    fn search_error_for_latest_is_failed() {
        let mut f = LeafSearchFlow::new();
        f.force_latest(5_000_009);
        let resp = Response::SearchError {
            id: 5_000_009,
            msg: "Operations error".to_string(),
        };
        assert!(
            matches!(f.on_response(&resp), LeafSearchOutcome::Failed(m) if m == "Operations error")
        );
    }

    #[test]
    fn fresh_flow_ignores_everything() {
        let mut f = LeafSearchFlow::new();
        let resp = Response::Entries {
            id: 5_000_000,
            entries: vec![],
            truncated: false,
        };
        assert!(matches!(f.on_response(&resp), LeafSearchOutcome::Ignored));
    }
}
