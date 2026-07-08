//! Async reverse name-resolution for the `lookup` widget. Given a stored scalar
//! (e.g. `gidNumber = 5000`), resolve the friendly name of the candidate whose
//! `store` attribute equals that value, so the form can show `5000 (staff)`.
//!
//! Id range 4_000_000+ keeps responses disjoint from ReadFlow (1) / WriteFlow
//! (1_000_000) / AllocFlow (2_000_000) / SearchFlow (3_000_000). Unlike
//! SearchFlow (which tracks only the latest term), ResolveFlow tracks EVERY
//! in-flight request so many distinct values resolve concurrently.
//!
//! No tvision_rs, no crate::ui — pure domain logic.

use anyhow::Result;
use std::collections::HashMap;

use crate::config::label::{render_label, LabelSeg};
use crate::ldap::worker::{Request, Response, SearchScope, WorkerHandle};
use crate::workflows::pick_state::{candidate_label, escape_filter};

/// Build an exact-match filter `(&(objectClass=<oc>)(<attr>=<value>))` with the
/// value RFC-4515-escaped. Used to find the single candidate whose `store`
/// attribute equals the field's stored value.
pub fn build_equality_filter(oc: &str, attr: &str, value: &str) -> String {
    format!("(&(objectClass={})({}={}))", oc, attr, escape_filter(value))
}

/// Identity of one reverse-lookup: a scope (base|objectClass|store attr) plus the
/// stored value. Keys the `UiState` resolution cache.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LookupKey {
    pub scope_id: String,
    pub value: String,
}

/// The cache key for a DN-keyed reference (a group `member`), resolved by a base
/// read of the DN. The sentinel `scope_id` has no `|`, so it can never collide
/// with a scalar lookup's `base|oc|store_attr` scope id.
pub fn member_key(dn: &str) -> LookupKey {
    LookupKey {
        scope_id: "@dn".to_string(),
        value: dn.to_string(),
    }
}

/// The result of correlating one worker response against the in-flight resolves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveOutcome {
    /// The candidate was found; `name` is the rendered label.
    Resolved { key: LookupKey, name: String },
    /// No candidate matched (empty result or search error): show the bare value.
    NotFound { key: LookupKey },
    /// The response id did not match any in-flight resolve; discard it.
    Ignored,
}

/// How to render a resolved entry into a display label.
enum Render {
    /// Scalar lookup (`gidNumber` → `staff`): render the profile label template.
    Template(Vec<LabelSeg>),
    /// DN-keyed reference (a group `member`): render `cn (uid)` / `cn` / DN via
    /// [`candidate_label`], matching how the same person reads in the leaf list
    /// and the candidate columns.
    CnUid,
}

/// One in-flight resolve: the key it will produce and how to render its label.
struct Pending {
    key: LookupKey,
    render: Render,
}

/// Async reverse name-resolution. Tracks every in-flight request id → its
/// `Pending`, so concurrent resolves for different values all complete.
pub struct ResolveFlow {
    next_id: u64,
    inflight: HashMap<u64, Pending>,
}

impl Default for ResolveFlow {
    fn default() -> Self {
        Self::new()
    }
}

impl ResolveFlow {
    /// First allocated id is 4_000_000 (disjoint from the other flows).
    pub fn new() -> Self {
        ResolveFlow {
            next_id: 4_000_000,
            inflight: HashMap::new(),
        }
    }

    fn alloc(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Submit an exact-match search for the candidate whose `store_attr == value`.
    /// `attrs` are the label-template attributes to fetch; `template` is rendered
    /// against the first matching entry in `on_response`. Records the id as pending.
    #[allow(clippy::too_many_arguments)]
    pub fn request(
        &mut self,
        worker: &WorkerHandle,
        base: &str,
        oc: &str,
        store_attr: &str,
        value: &str,
        attrs: &[String],
        template: Vec<LabelSeg>,
    ) -> Result<u64> {
        let id = self.alloc();
        worker.submit(Request::Search {
            id,
            base: base.to_string(),
            scope: SearchScope::Subtree,
            filter: build_equality_filter(oc, store_attr, value),
            attrs: attrs.to_vec(),
            size_limit: Some(2),
        })?;
        let key = LookupKey {
            scope_id: format!("{base}|{oc}|{store_attr}"),
            value: value.to_string(),
        };
        self.inflight.insert(
            id,
            Pending {
                key,
                render: Render::Template(template),
            },
        );
        Ok(id)
    }

    /// Submit a base-scoped read of `dn` to resolve a DN-keyed reference (a group
    /// `member`). `attrs` are fetched and rendered as `cn (uid)` via
    /// [`candidate_label`]. The resulting [`LookupKey`] uses the shared member
    /// scope (see [`member_key`]). Records the id as pending.
    pub fn request_by_dn(
        &mut self,
        worker: &WorkerHandle,
        dn: &str,
        attrs: &[String],
    ) -> Result<u64> {
        let id = self.alloc();
        worker.submit(Request::Search {
            id,
            base: dn.to_string(),
            scope: SearchScope::Base,
            filter: "(objectClass=*)".to_string(),
            attrs: attrs.to_vec(),
            size_limit: Some(2),
        })?;
        self.inflight.insert(
            id,
            Pending {
                key: member_key(dn),
                render: Render::CnUid,
            },
        );
        Ok(id)
    }

    /// Whether a resolve for `key` is currently in flight.
    pub fn is_pending(&self, key: &LookupKey) -> bool {
        self.inflight.values().any(|p| &p.key == key)
    }

    /// Correlate one worker response. Removes the matched id from `inflight`.
    pub fn on_response(&mut self, resp: &Response) -> ResolveOutcome {
        match resp {
            Response::Entries { id, entries, .. } => {
                let Some(p) = self.inflight.remove(id) else {
                    return ResolveOutcome::Ignored;
                };
                match entries.first() {
                    Some(e) => {
                        let name = match &p.render {
                            Render::Template(t) => render_label(t, &e.attrs),
                            Render::CnUid => candidate_label(&p.key.value, &e.attrs),
                        };
                        ResolveOutcome::Resolved { key: p.key, name }
                    }
                    None => ResolveOutcome::NotFound { key: p.key },
                }
            }
            Response::SearchError { id, .. } => match self.inflight.remove(id) {
                Some(p) => ResolveOutcome::NotFound { key: p.key },
                None => ResolveOutcome::Ignored,
            },
            _ => ResolveOutcome::Ignored,
        }
    }

    /// Test-only: register an in-flight resolve without a live worker.
    #[cfg(test)]
    pub(crate) fn force_pending(&mut self, id: u64, key: LookupKey, template: Vec<LabelSeg>) {
        self.inflight.insert(
            id,
            Pending {
                key,
                render: Render::Template(template),
            },
        );
    }

    /// Test-only: register an in-flight DN resolve (renders via `cn (uid)`).
    #[cfg(test)]
    pub(crate) fn force_pending_dn(&mut self, id: u64, dn: &str) {
        self.inflight.insert(
            id,
            Pending {
                key: member_key(dn),
                render: Render::CnUid,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::label::parse_label_template;
    use crate::ldap::worker::LdapEntry;
    use std::collections::BTreeMap;

    fn key(v: &str) -> LookupKey {
        LookupKey {
            scope_id: "ou=groups,dc=x|posixGroup|gidNumber".into(),
            value: v.into(),
        }
    }

    #[test]
    fn equality_filter_escapes_value() {
        assert_eq!(
            build_equality_filter("posixGroup", "gidNumber", "5000"),
            "(&(objectClass=posixGroup)(gidNumber=5000))"
        );
        // RFC-4515 metacharacters in the value are escaped.
        assert_eq!(
            build_equality_filter("posixGroup", "cn", "a*b"),
            "(&(objectClass=posixGroup)(cn=a\\2ab))"
        );
    }

    #[test]
    fn dn_resolve_renders_cn_uid() {
        // A DN-keyed member resolve renders `cn (uid)` from the base-read entry
        // (matching the leaf list / candidate columns), keyed under the shared
        // member scope.
        let dn = "cn=user01,ou=users,dc=x";
        let mut rf = ResolveFlow::new();
        rf.force_pending_dn(4_000_000, dn);
        let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        attrs.insert("cn".into(), vec!["User1".into()]);
        attrs.insert("uid".into(), vec!["user01".into()]);
        let resp = Response::Entries {
            id: 4_000_000,
            entries: vec![LdapEntry {
                dn: dn.into(),
                attrs,
                bin_attrs: Default::default(),
            }],
            truncated: false,
        };
        match rf.on_response(&resp) {
            ResolveOutcome::Resolved { key, name } => {
                assert_eq!(key, member_key(dn));
                assert_eq!(name, "User1 (user01)");
            }
            other => panic!("expected Resolved, got {other:?}"),
        }
    }

    #[test]
    fn resolved_renders_label_from_first_entry() {
        let mut rf = ResolveFlow::new();
        rf.force_pending(4_000_000, key("5000"), parse_label_template("{cn}"));
        let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        attrs.insert("cn".into(), vec!["staff".into()]);
        let resp = Response::Entries {
            id: 4_000_000,
            entries: vec![LdapEntry {
                dn: "cn=staff,ou=groups,dc=x".into(),
                attrs,
                bin_attrs: Default::default(),
            }],
            truncated: false,
        };
        assert_eq!(
            rf.on_response(&resp),
            ResolveOutcome::Resolved {
                key: key("5000"),
                name: "staff".into()
            }
        );
        // The id is consumed: a second identical response is Ignored.
        assert_eq!(rf.on_response(&resp), ResolveOutcome::Ignored);
    }

    #[test]
    fn no_entries_is_not_found() {
        let mut rf = ResolveFlow::new();
        rf.force_pending(4_000_001, key("9999"), parse_label_template("{cn}"));
        let resp = Response::Entries {
            id: 4_000_001,
            entries: vec![],
            truncated: false,
        };
        assert_eq!(
            rf.on_response(&resp),
            ResolveOutcome::NotFound { key: key("9999") }
        );
    }

    #[test]
    fn search_error_is_not_found() {
        let mut rf = ResolveFlow::new();
        rf.force_pending(4_000_002, key("1"), parse_label_template("{cn}"));
        let resp = Response::SearchError {
            id: 4_000_002,
            msg: "boom".into(),
        };
        assert_eq!(
            rf.on_response(&resp),
            ResolveOutcome::NotFound { key: key("1") }
        );
    }

    #[test]
    fn unknown_id_is_ignored_and_is_pending_tracks_keys() {
        let mut rf = ResolveFlow::new();
        rf.force_pending(4_000_003, key("42"), parse_label_template("{cn}"));
        assert!(rf.is_pending(&key("42")));
        assert!(!rf.is_pending(&key("43")));
        let resp = Response::Entries {
            id: 999,
            entries: vec![],
            truncated: false,
        };
        assert_eq!(rf.on_response(&resp), ResolveOutcome::Ignored);
        assert!(
            rf.is_pending(&key("42")),
            "unrelated response must not clear pending"
        );
    }
}
