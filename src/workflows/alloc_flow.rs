//! Async next-free-number allocation: scan an attribute under the base, then pick
//! the next free value in [min,max] via [`crate::workflows::save::decide_allocation`].
//! Mirrors `read_flow`/`write_flow`; ids are disjoint by range so the pump can route
//! responses to exactly one flow.

use std::collections::HashMap;

use anyhow::Result;

use crate::ldap::worker::{Request, Response, SearchScope, WorkerHandle};
use crate::workflows::save::decide_allocation;

/// The result of correlating one scan response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllocOutcome {
    Filled { attr: String, value: String },
    Failed(String),
    Ignored,
}

pub struct AllocFlow {
    next_id: u64,
    pending: HashMap<u64, (String, u64, u64)>, // id -> (attr, min, max)
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
        self.pending.insert(id, (attr.to_string(), min, max));
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
                let Some((attr, min, max)) = self.pending.remove(id) else {
                    return AllocOutcome::Ignored;
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
                    Err(e) => AllocOutcome::Failed(e),
                }
            }
            Response::SearchError { id, msg } => {
                if self.pending.remove(id).is_some() {
                    AllocOutcome::Failed(msg.clone())
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
        self.pending.insert(id, (attr, min, max));
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
        assert!(matches!(out, AllocOutcome::Failed(_)));
    }
}
