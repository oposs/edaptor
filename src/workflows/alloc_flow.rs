//! Async next-free-number allocation: scan an attribute under the base, then pick
//! the next free value in [min,max] via [`crate::workflows::save::decide_allocation`].
//! Mirrors `read_flow`/`write_flow`; ids are disjoint by range so the pump can route
//! responses to exactly one flow.

use std::collections::HashMap;

use anyhow::Result;

use crate::detect::model::SampleEntry;
use crate::detect::range::{allocate, RangeSpec, SCAN_ATTRS, SCAN_FILTER};
use crate::ldap::worker::{Request, Response, SearchScope, WorkerHandle};
use crate::workflows::save::{decide_allocation, TRUNCATED_SCAN_MSG};

/// What a pending scan will decide once its response arrives.
enum Pending {
    Range { attr: String, min: u64, max: u64 },
    Detected { attr: String, spec: RangeSpec },
}

impl Pending {
    fn into_attr(self) -> String {
        match self {
            Pending::Range { attr, .. } | Pending::Detected { attr, .. } => attr,
        }
    }
}

/// The result of correlating one scan response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllocOutcome {
    Filled { attr: String, value: String },
    Failed { attr: String, msg: String },
    Ignored,
}

pub struct AllocFlow {
    next_id: u64,
    pending: HashMap<u64, Pending>,
}

impl Default for AllocFlow {
    fn default() -> Self {
        Self::new()
    }
}

impl AllocFlow {
    pub fn new() -> Self {
        // Above ReadFlow (1) and WriteFlow (1_000_000) ranges.
        AllocFlow {
            next_id: 2_000_000,
            pending: HashMap::new(),
        }
    }

    fn alloc(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Post a subtree scan of `attr` under `base`; returns the request id.
    pub fn request(
        &mut self,
        worker: &WorkerHandle,
        base: &str,
        attr: &str,
        min: u64,
        max: u64,
    ) -> Result<u64> {
        let id = self.alloc();
        worker.submit(Request::Search {
            id,
            base: base.to_string(),
            scope: SearchScope::Subtree,
            filter: format!("({attr}=*)"),
            attrs: vec![attr.to_string()],
            size_limit: None,
        })?;
        self.pending.insert(
            id,
            Pending::Range {
                attr: attr.to_string(),
                min,
                max,
            },
        );
        Ok(id)
    }

    /// Post the full number scan for a detected range; returns the request id.
    pub fn request_detected(
        &mut self,
        worker: &WorkerHandle,
        base: &str,
        attr: &str,
        spec: RangeSpec,
    ) -> Result<u64> {
        let id = self.alloc();
        worker.submit(Request::Search {
            id,
            base: base.to_string(),
            scope: SearchScope::Subtree,
            filter: SCAN_FILTER.to_string(),
            attrs: SCAN_ATTRS.iter().map(|s| s.to_string()).collect(),
            size_limit: None,
        })?;
        self.pending.insert(
            id,
            Pending::Detected {
                attr: attr.to_string(),
                spec,
            },
        );
        Ok(id)
    }

    /// Correlate one response. Pure; ignores non-matching ids/variants.
    pub fn on_response(&mut self, resp: &Response) -> AllocOutcome {
        match resp {
            Response::Entries {
                id,
                entries,
                truncated,
            } => {
                let Some(pending) = self.pending.remove(id) else {
                    return AllocOutcome::Ignored;
                };
                let (attr, min, max) = match pending {
                    Pending::Range { attr, min, max } => (attr, min, max),
                    Pending::Detected { attr, spec } => {
                        if *truncated {
                            return AllocOutcome::Failed {
                                attr,
                                msg: TRUNCATED_SCAN_MSG.to_string(),
                            };
                        }
                        let scan: Vec<SampleEntry> =
                            entries.iter().map(SampleEntry::from).collect();
                        return match allocate(&spec, &scan) {
                            Ok((n, _)) => AllocOutcome::Filled {
                                attr,
                                value: n.to_string(),
                            },
                            Err(msg) => AllocOutcome::Failed { attr, msg },
                        };
                    }
                };
                let values: Vec<u64> = entries
                    .iter()
                    .flat_map(|e| {
                        e.attrs
                            .iter()
                            .find(|(k, _)| k.eq_ignore_ascii_case(&attr))
                            .map(|(_, v)| v.clone())
                            .unwrap_or_default()
                    })
                    .filter_map(|s| s.parse::<u64>().ok())
                    .collect();
                match decide_allocation(&values, *truncated, min, max) {
                    Ok(n) => AllocOutcome::Filled {
                        attr,
                        value: n.to_string(),
                    },
                    Err(msg) => AllocOutcome::Failed { attr, msg },
                }
            }
            Response::SearchError { id, msg } => {
                if let Some(pending) = self.pending.remove(id) {
                    AllocOutcome::Failed {
                        attr: pending.into_attr(),
                        msg: msg.clone(),
                    }
                } else {
                    AllocOutcome::Ignored
                }
            }
            _ => AllocOutcome::Ignored,
        }
    }

    #[cfg(test)]
    pub(crate) fn alloc_for_test(&mut self) -> u64 {
        self.alloc()
    }

    #[cfg(test)]
    pub(crate) fn insert_for_test(&mut self, id: u64, attr: String, min: u64, max: u64) {
        self.pending.insert(id, Pending::Range { attr, min, max });
    }

    #[cfg(test)]
    pub(crate) fn insert_detected_for_test(&mut self, id: u64, attr: String, spec: RangeSpec) {
        self.pending.insert(id, Pending::Detected { attr, spec });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_fills_next_free_number() {
        let mut af = AllocFlow::new();
        let id = af.alloc_for_test(); // seam mirroring write_flow
        af.insert_for_test(id, "uidNumber".into(), 10000, 19999);
        let entries = vec![
            crate::ldap::worker::LdapEntry {
                dn: "uid=a,dc=x".into(),
                attrs: [("uidNumber".to_string(), vec!["10000".to_string()])]
                    .into_iter()
                    .collect(),
                bin_attrs: Default::default(),
            },
            crate::ldap::worker::LdapEntry {
                dn: "uid=b,dc=x".into(),
                attrs: [("uidNumber".to_string(), vec!["10005".to_string()])]
                    .into_iter()
                    .collect(),
                bin_attrs: Default::default(),
            },
        ];
        let out = af.on_response(&crate::ldap::worker::Response::Entries {
            id,
            entries,
            truncated: false,
        });
        assert!(matches!(out, AllocOutcome::Filled { value, .. } if value == "10006"));
    }

    #[test]
    fn alloc_refuses_truncated_scan() {
        let mut af = AllocFlow::new();
        let id = af.alloc_for_test();
        af.insert_for_test(id, "uidNumber".into(), 10000, 19999);
        let out = af.on_response(&crate::ldap::worker::Response::Entries {
            id,
            entries: vec![],
            truncated: true,
        });
        // attr must match the requested attribute
        assert!(
            matches!(&out, AllocOutcome::Failed { attr, .. } if attr == "uidNumber"),
            "expected Failed with attr=uidNumber, got {out:?}"
        );
    }

    fn scan_entry(
        dn: &str,
        ocs: &[&str],
        pairs: &[(&str, &str)],
    ) -> crate::ldap::worker::LdapEntry {
        let mut attrs: std::collections::BTreeMap<String, Vec<String>> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), vec![v.to_string()]))
            .collect();
        attrs.insert(
            "objectClass".into(),
            ocs.iter().map(|s| s.to_string()).collect(),
        );
        crate::ldap::worker::LdapEntry {
            dn: dn.into(),
            attrs,
            bin_attrs: Default::default(),
        }
    }

    fn user_spec() -> RangeSpec {
        RangeSpec {
            attr: "uidNumber".into(),
            container: "ou=people,dc=x".into(),
            structural: "inetOrgPerson".into(),
            unified: false,
            exclude_private: false,
        }
    }

    #[test]
    fn detected_range_allocation_fills_from_the_scan() {
        let mut af = AllocFlow::new();
        let id = af.alloc_for_test();
        af.insert_detected_for_test(id, "uidNumber".into(), user_spec());
        let entries = (0..3)
            .map(|i| {
                scan_entry(
                    &format!("uid=u{i},ou=people,dc=x"),
                    &["inetOrgPerson", "posixAccount"],
                    &[
                        ("uid", &format!("u{i}")),
                        ("uidNumber", &(5000 + i).to_string()),
                        ("gidNumber", "100"),
                    ],
                )
            })
            .collect();
        let out = af.on_response(&Response::Entries {
            id,
            entries,
            truncated: false,
        });
        assert_eq!(
            out,
            AllocOutcome::Filled {
                attr: "uidNumber".into(),
                value: "5003".into()
            }
        );
    }

    #[test]
    fn detected_range_refuses_a_truncated_scan_like_today() {
        let mut af = AllocFlow::new();
        let id = af.alloc_for_test();
        af.insert_detected_for_test(id, "uidNumber".into(), user_spec());
        let out = af.on_response(&Response::Entries {
            id,
            entries: vec![],
            truncated: true,
        });
        assert_eq!(
            out,
            AllocOutcome::Failed {
                attr: "uidNumber".into(),
                msg: crate::workflows::save::TRUNCATED_SCAN_MSG.into()
            }
        );
    }

    #[test]
    fn detected_range_search_error_fails_with_the_attr() {
        let mut af = AllocFlow::new();
        let id = af.alloc_for_test();
        af.insert_detected_for_test(id, "uidNumber".into(), user_spec());
        let out = af.on_response(&Response::SearchError {
            id,
            msg: "boom".into(),
        });
        assert_eq!(
            out,
            AllocOutcome::Failed {
                attr: "uidNumber".into(),
                msg: "boom".into()
            }
        );
    }

    #[test]
    fn request_detected_posts_the_number_scan() {
        let (worker, rx) = WorkerHandle::recording();
        let mut af = AllocFlow::new();
        af.request_detected(&worker, "dc=x", "uidNumber", user_spec())
            .unwrap();
        let (req, _) = rx.try_recv().expect("a request was submitted");
        match req {
            Request::Search {
                base,
                scope,
                filter,
                attrs,
                size_limit,
                ..
            } => {
                assert_eq!(base, "dc=x");
                assert_eq!(scope, SearchScope::Subtree);
                assert_eq!(filter, "(|(uidNumber=*)(gidNumber=*))");
                assert!(attrs.iter().any(|a| a == "uid"));
                assert_eq!(size_limit, None);
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
