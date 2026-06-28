//! Async LDAP candidate-search flow for picker and membership widgets.
//!
//! Mirrors [`crate::workflows::alloc_flow::AllocFlow`] in shape. Id range
//! 3_000_000+ keeps responses disjoint from ReadFlow (1) / WriteFlow
//! (1_000_000) / AllocFlow (2_000_000). Only the *latest* request id is
//! tracked; stale responses from superseded search terms are silently ignored
//! so the UI always sees the most-recent result set.
//!
//! **Design choice — `Candidate` reuse:** `SearchOutcome::Results` carries
//! `Vec<pick_state::Candidate>` rather than a new `SearchRow` type, because
//! [`Candidate`] already holds the three fields (`dn`, `store_value`, `label`)
//! that the picker and membership widgets need. Reusing it avoids duplication
//! and keeps the pick-state/search-flow boundary thin.
//!
//! **`store_value` default:** `entry_to_candidate` sets `store_value = dn`.
//! This covers the DN-store case (the most common picker binding). Callers
//! that use a scalar store extract the value via `pick_state::pick_value` on
//! the returned `Candidate`'s `dn` or by re-querying the attrs — the flow
//! itself remains agnostic of the binding configuration.
//!
//! No ratatui, no tvision_rs, no crate::tui, no crate::ui — pure domain logic.

use anyhow::Result;

use crate::ldap::worker::{LdapEntry, Request, Response, SearchScope, WorkerHandle};
use crate::workflows::pick_state::{
    build_member_filter, candidate_label, Candidate, PICKER_SEARCH_CAP,
};

/// Build an RFC-4515 LDAP candidate-search filter for a single object class.
///
/// Delegates to `pick_state::build_member_filter` with default search
/// attributes `["cn", "uid"]`.
///
/// - Empty `term` → `(objectClass=<oc>)` (bare, no outer `(&...)` for single class).
/// - With `term` → `(&(objectClass=<oc>)(|(cn=*term*)(uid=*term*)))`.
///
/// Special bytes `*()\NUL` are RFC-4515-escaped before insertion.
pub fn build_search_filter(oc: &str, term: &str) -> String {
    build_member_filter(
        &[oc.to_string()],
        &["cn".to_string(), "uid".to_string()],
        term,
    )
}

/// The result of correlating one search response against the latest request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchOutcome {
    /// The latest search returned results (may be truncated by `PICKER_SEARCH_CAP`).
    Results {
        rows: Vec<Candidate>,
        truncated: bool,
    },
    /// The latest search failed with an LDAP error.
    Failed(String),
    /// The response id did not match `latest`; caller should discard it.
    Ignored,
}

/// Async LDAP candidate search: tracks only the latest in-flight request id
/// so stale responses from superseded search terms are silently discarded.
///
/// Callers typically call [`request`][Self::request] on every keystroke and
/// [`on_response`][Self::on_response] on every worker-poll tick.
pub struct SearchFlow {
    next_id: u64,
    /// The id of the most-recently submitted request. Responses for any other
    /// id are returned as [`SearchOutcome::Ignored`].
    latest: Option<u64>,
}

impl Default for SearchFlow {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchFlow {
    /// Create a new flow. The first allocated id is 3_000_000 (disjoint from
    /// ReadFlow / WriteFlow / AllocFlow id ranges).
    pub fn new() -> Self {
        SearchFlow {
            next_id: 3_000_000,
            latest: None,
        }
    }

    fn alloc(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Submit a candidate search under `base` for entries of object class `oc`
    /// matching `term`. `attrs` are the LDAP attributes to return per entry.
    ///
    /// Builds the filter via [`build_search_filter`], submits a
    /// [`Request::Search`] with [`SearchScope::Subtree`] and
    /// `size_limit = PICKER_SEARCH_CAP`, and records the assigned id as
    /// `latest`. Returns the assigned id.
    pub fn request(
        &mut self,
        worker: &WorkerHandle,
        base: &str,
        oc: &str,
        term: &str,
        attrs: &[String],
    ) -> Result<u64> {
        let id = self.alloc();
        let filter = build_search_filter(oc, term);
        worker.submit(Request::Search {
            id,
            base: base.to_string(),
            scope: SearchScope::Subtree,
            filter,
            attrs: attrs.to_vec(),
            size_limit: Some(PICKER_SEARCH_CAP),
        })?;
        self.latest = Some(id);
        Ok(id)
    }

    /// Correlate one worker response. Pure (no I/O); ignores responses whose
    /// id does not match `latest`.
    ///
    /// | Response | id == latest | Outcome |
    /// |---|---|---|
    /// | `Entries` | ✓ | `Results { rows, truncated }` |
    /// | `Entries` | ✗ | `Ignored` |
    /// | `SearchError` | ✓ | `Failed(msg)` |
    /// | `SearchError` | ✗ | `Ignored` |
    /// | anything else | — | `Ignored` |
    pub fn on_response(&mut self, resp: &Response) -> SearchOutcome {
        match resp {
            Response::Entries {
                id,
                entries,
                truncated,
            } => {
                if Some(*id) != self.latest {
                    return SearchOutcome::Ignored;
                }
                let rows = entries.iter().map(entry_to_candidate).collect();
                SearchOutcome::Results {
                    rows,
                    truncated: *truncated,
                }
            }
            Response::SearchError { id, msg } => {
                if Some(*id) != self.latest {
                    return SearchOutcome::Ignored;
                }
                SearchOutcome::Failed(msg.clone())
            }
            _ => SearchOutcome::Ignored,
        }
    }

    /// Test-only: forcefully set `latest` without submitting a request.
    ///
    /// Simulates the effect of a prior [`request`][Self::request] call when a
    /// live [`WorkerHandle`] is unavailable. Lets `on_response` be driven
    /// directly with hand-built [`Response`] values.
    #[cfg(test)]
    pub(crate) fn force_latest(&mut self, id: u64) {
        self.latest = Some(id);
    }
}

/// Map an [`LdapEntry`] to a [`Candidate`].
///
/// `store_value` defaults to the entry DN (covers the DN-store case).
/// `label` is derived from the `cn` attribute via `pick_state::candidate_label`
/// (falls back to the raw DN when `cn` is absent).
fn entry_to_candidate(entry: &LdapEntry) -> Candidate {
    Candidate {
        dn: entry.dn.clone(),
        store_value: entry.dn.clone(),
        label: candidate_label(&entry.dn, &entry.attrs),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldap::worker::LdapEntry;
    use std::collections::BTreeMap;

    // --- filter delegation tests ---

    #[test]
    fn filter_empty_term_is_objectclass_only() {
        assert_eq!(
            build_search_filter("posixGroup", ""),
            "(objectClass=posixGroup)"
        );
    }

    #[test]
    fn filter_escapes_and_substrings() {
        // '*' in the term must be RFC-4515-escaped to '\2a'; the filter wraps
        // cn and uid as the default search dimensions.
        assert_eq!(
            build_search_filter("posixGroup", "a*b"),
            "(&(objectClass=posixGroup)(|(cn=*a\\2ab*)(uid=*a\\2ab*)))"
        );
    }

    // --- stale / latest id routing ---

    #[test]
    fn stale_response_is_ignored() {
        let mut sf = SearchFlow::new();
        let old = 3_000_000u64;
        // Advance latest past `old` without a live worker.
        sf.force_latest(3_000_001);
        let resp = Response::Entries {
            id: old,
            entries: vec![],
            truncated: false,
        };
        assert!(
            matches!(sf.on_response(&resp), SearchOutcome::Ignored),
            "response for old id must be Ignored"
        );
    }

    #[test]
    fn latest_entries_response_produces_results() {
        let mut sf = SearchFlow::new();
        sf.force_latest(3_000_007);
        let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        attrs.insert("cn".to_string(), vec!["Alice".to_string()]);
        let entry = LdapEntry {
            dn: "uid=alice,ou=people,dc=example,dc=com".to_string(),
            attrs,
            bin_attrs: Default::default(),
        };
        let resp = Response::Entries {
            id: 3_000_007,
            entries: vec![entry],
            truncated: true,
        };
        match sf.on_response(&resp) {
            SearchOutcome::Results { rows, truncated } => {
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].dn, "uid=alice,ou=people,dc=example,dc=com");
                assert_eq!(rows[0].store_value, "uid=alice,ou=people,dc=example,dc=com");
                assert_eq!(rows[0].label, "Alice", "label must come from cn");
                assert!(truncated, "truncated flag must be forwarded");
            }
            other => panic!("expected Results, got {other:?}"),
        }
    }

    #[test]
    fn search_error_for_latest_produces_failed() {
        let mut sf = SearchFlow::new();
        sf.force_latest(3_000_099);
        let resp = Response::SearchError {
            id: 3_000_099,
            msg: "Size limit exceeded".to_string(),
        };
        assert!(
            matches!(sf.on_response(&resp), SearchOutcome::Failed(msg) if msg == "Size limit exceeded"),
            "SearchError for latest must yield Failed"
        );
    }

    #[test]
    fn search_error_for_stale_id_is_ignored() {
        let mut sf = SearchFlow::new();
        sf.force_latest(3_000_100);
        let resp = Response::SearchError {
            id: 3_000_000,
            msg: "old error".to_string(),
        };
        assert!(
            matches!(sf.on_response(&resp), SearchOutcome::Ignored),
            "SearchError for old id must be Ignored"
        );
    }

    #[test]
    fn no_latest_means_all_responses_ignored() {
        // Fresh SearchFlow has no latest; every response is stale by definition.
        let mut sf = SearchFlow::new();
        let resp = Response::Entries {
            id: 3_000_000,
            entries: vec![],
            truncated: false,
        };
        assert!(
            matches!(sf.on_response(&resp), SearchOutcome::Ignored),
            "with latest=None every response must be Ignored"
        );
    }

    #[test]
    fn label_falls_back_to_dn_when_no_cn() {
        let mut sf = SearchFlow::new();
        sf.force_latest(3_000_042);
        let entry = LdapEntry {
            dn: "uid=nobody,dc=x".to_string(),
            attrs: BTreeMap::new(),
            bin_attrs: Default::default(),
        };
        let resp = Response::Entries {
            id: 3_000_042,
            entries: vec![entry],
            truncated: false,
        };
        match sf.on_response(&resp) {
            SearchOutcome::Results { rows, .. } => {
                assert_eq!(rows[0].label, "uid=nobody,dc=x", "label must fall back to DN");
            }
            other => panic!("expected Results, got {other:?}"),
        }
    }

    #[test]
    fn multiple_entries_all_mapped() {
        let mut sf = SearchFlow::new();
        sf.force_latest(3_000_010);
        let entries = vec![
            {
                let mut attrs = BTreeMap::new();
                attrs.insert("cn".to_string(), vec!["Alice".to_string()]);
                LdapEntry {
                    dn: "uid=alice,dc=x".to_string(),
                    attrs,
                    bin_attrs: Default::default(),
                }
            },
            LdapEntry {
                dn: "uid=bob,dc=x".to_string(),
                attrs: BTreeMap::new(),
                bin_attrs: Default::default(),
            },
        ];
        let resp = Response::Entries {
            id: 3_000_010,
            entries,
            truncated: false,
        };
        match sf.on_response(&resp) {
            SearchOutcome::Results { rows, truncated } => {
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0].label, "Alice");
                assert_eq!(rows[1].label, "uid=bob,dc=x");
                assert!(!truncated);
            }
            other => panic!("expected Results, got {other:?}"),
        }
    }
}
