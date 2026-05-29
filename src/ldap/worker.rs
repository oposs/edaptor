//! The background LDAP worker thread. It owns the ldap3 connection and is the
//! only place network I/O happens.
//!
//! Two request paths coexist (D3):
//!  * Synchronous: [`WorkerHandle::request`] sends a job with a fresh per-call
//!    reply channel and blocks for the answer. Used for the startup
//!    `FetchSubschema` fetch.
//!  * Non-blocking: [`WorkerHandle::submit`] fires a `Search` job whose reply
//!    sender is a clone of a long-lived response channel; [`WorkerHandle::poll`]
//!    drains that channel without blocking. Used by the browser/read flow so the
//!    UI thread never blocks on the network.
//!
//! Routing is kept uniform by reusing the existing `Job = (Request, Sender<Response>)`
//! plumbing: `submit` simply pairs the `Search` request with a clone of the
//! long-lived `resp_tx`, so the worker loop sends every result through the same
//! `reply.send(..)` call regardless of path.

use anyhow::{anyhow, Context, Result};
use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};

use ldap3::{LdapConn, Scope, SearchEntry};

use crate::config::{AuthMethod, Config};
use crate::ldap::tls::build_settings;

/// Search scope for a [`Request::Search`]. Mapped to `ldap3::Scope` only inside
/// the worker (see [`scope_to_ldap3`]) so `ldap3` types do not leak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchScope {
    /// The base entry itself (read a single entry).
    Base,
    /// The immediate children of the base (one-level browse).
    OneLevel,
}

/// A flattened LDAP entry for the UI. String attribute values are carried
/// verbatim; binary attributes are reduced to a byte count (sum of value
/// lengths) so the read-only form can render `<N bytes>` without copying blobs.
/// `BTreeMap` gives deterministic ordering for tests and display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LdapEntry {
    /// The entry's distinguished name.
    pub dn: String,
    /// String-valued attributes, attribute name -> values.
    pub attrs: BTreeMap<String, Vec<String>>,
    /// Binary-valued attributes, attribute name -> total byte count.
    pub bin_attrs: BTreeMap<String, usize>,
}

/// A request to the worker. Each is paired with a reply `Sender` in the channel.
pub enum Request {
    /// Fetch the raw (unparsed) subschema description strings.
    FetchSubschema,
    /// Search the directory. `id` is echoed in the reply for correlation (D4).
    Search {
        /// Caller-assigned correlation id, echoed in the reply.
        id: u64,
        /// Base DN to search from.
        base: String,
        /// Scope of the search.
        scope: SearchScope,
        /// LDAP filter string.
        filter: String,
        /// Attributes to request (`"*"` for all user attributes).
        attrs: Vec<String>,
    },
    /// Unbind and stop the worker thread.
    Shutdown,
}

/// Raw subschema: the server's description strings, not yet parsed (that is M2).
#[derive(Debug, Clone, Default)]
pub struct RawSubschema {
    pub object_classes: Vec<String>,
    pub attribute_types: Vec<String>,
    pub ldap_syntaxes: Vec<String>,
}

pub enum Response {
    Subschema(RawSubschema),
    /// Result of a [`Request::Search`]; `id` echoes the request (D4).
    Entries {
        id: u64,
        entries: Vec<LdapEntry>,
    },
    /// A failed [`Request::Search`]; `id` echoes the request (D4).
    SearchError {
        id: u64,
        msg: String,
    },
    Done,
    Error(String),
}

type Job = (Request, Sender<Response>);

pub struct WorkerHandle {
    tx: Sender<Job>,
    /// Long-lived response channel for the non-blocking `submit`/`poll` path.
    resp_tx: Sender<Response>,
    resp_rx: Receiver<Response>,
    join: Option<JoinHandle<()>>,
}

impl WorkerHandle {
    /// Spawn the worker, connecting + binding synchronously so connection or
    /// credential failures surface immediately as an Err from spawn().
    pub fn spawn(config: Config, password: String) -> Result<WorkerHandle> {
        let (tx, rx) = mpsc::channel::<Job>();
        let (resp_tx, resp_rx) = mpsc::channel::<Response>();
        let (startup_tx, startup_rx) = mpsc::channel::<std::result::Result<(), String>>();

        let join = thread::spawn(move || {
            let mut conn = match connect_and_bind(&config, &password) {
                Ok(conn) => {
                    let _ = startup_tx.send(Ok(()));
                    conn
                }
                Err(e) => {
                    let _ = startup_tx.send(Err(e.to_string()));
                    return;
                }
            };
            worker_loop(&mut conn, &config, rx);
        });

        match startup_rx.recv() {
            Ok(Ok(())) => Ok(WorkerHandle {
                tx,
                resp_tx,
                resp_rx,
                join: Some(join),
            }),
            Ok(Err(e)) => {
                let _ = join.join();
                Err(anyhow!(e))
            }
            Err(_) => {
                let _ = join.join();
                Err(anyhow!(
                    "worker thread exited before reporting startup status"
                ))
            }
        }
    }

    /// Send a request and block for its response (synchronous path).
    pub fn request(&self, req: Request) -> Result<Response> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send((req, reply_tx))
            .map_err(|_| anyhow!("worker thread is gone"))?;
        reply_rx
            .recv()
            .map_err(|_| anyhow!("worker dropped the reply channel"))
    }

    /// Fire a request without waiting; its [`Response`] arrives on the long-lived
    /// channel and is retrieved via [`poll`](Self::poll). Intended for `Search`.
    pub fn submit(&self, req: Request) -> Result<()> {
        self.tx
            .send((req, self.resp_tx.clone()))
            .map_err(|_| anyhow!("worker thread is gone"))
    }

    /// Non-blocking drain of one pending [`Response`] from the long-lived
    /// channel. Returns `None` when empty or the worker has disconnected.
    pub fn poll(&self) -> Option<Response> {
        match self.resp_rx.try_recv() {
            Ok(resp) => Some(resp),
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => None,
        }
    }
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        let (reply_tx, _reply_rx) = mpsc::channel();
        let _ = self.tx.send((Request::Shutdown, reply_tx));
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn connect_and_bind(config: &Config, password: &str) -> Result<LdapConn> {
    let settings = build_settings(&config.server)?;
    let mut conn = LdapConn::with_settings(settings, &config.server.uri)
        .with_context(|| format!("connecting to {}", config.server.uri))?;

    match config.auth.method {
        AuthMethod::Simple => {
            let bind_dn = config
                .auth
                .bind_dn
                .as_deref()
                .ok_or_else(|| anyhow!("auth.method = simple requires auth.bind_dn"))?;
            conn.simple_bind(bind_dn, password)
                .context("sending simple bind")?
                .success()
                .context("LDAP rejected the bind credentials")?;
        }
        AuthMethod::External => {
            return Err(anyhow!(
                "auth.method = external is not implemented until M6"
            ));
        }
        AuthMethod::Gssapi => {
            return Err(anyhow!("auth.method = gssapi is not implemented until M6"));
        }
    }
    Ok(conn)
}

/// Map the domain [`SearchScope`] to `ldap3::Scope` (worker-private).
fn scope_to_ldap3(scope: SearchScope) -> Scope {
    match scope {
        SearchScope::Base => Scope::Base,
        SearchScope::OneLevel => Scope::OneLevel,
    }
}

/// Flatten an ldap3 [`SearchEntry`] into the UI's [`LdapEntry`]: string attrs
/// copied verbatim, binary attrs reduced to a per-attribute byte count (sum of
/// each value's length).
fn to_ldap_entry(se: SearchEntry) -> LdapEntry {
    let attrs: BTreeMap<String, Vec<String>> = se.attrs.into_iter().collect();
    let bin_attrs: BTreeMap<String, usize> = se
        .bin_attrs
        .into_iter()
        .map(|(k, vals)| (k, vals.iter().map(|v| v.len()).sum()))
        .collect();
    LdapEntry {
        dn: se.dn,
        attrs,
        bin_attrs,
    }
}

fn worker_loop(conn: &mut LdapConn, config: &Config, rx: Receiver<Job>) {
    while let Ok((req, reply)) = rx.recv() {
        match req {
            Request::FetchSubschema => {
                let resp = match fetch_subschema(conn, &config.server.base_dn) {
                    Ok(raw) => Response::Subschema(raw),
                    Err(e) => Response::Error(e.to_string()),
                };
                let _ = reply.send(resp);
            }
            Request::Search {
                id,
                base,
                scope,
                filter,
                attrs,
            } => {
                let resp = match run_search(conn, &base, scope, &filter, attrs) {
                    Ok(entries) => Response::Entries { id, entries },
                    Err(e) => Response::SearchError {
                        id,
                        msg: format!("{e:#}"),
                    },
                };
                let _ = reply.send(resp);
            }
            Request::Shutdown => {
                let _ = conn.unbind();
                let _ = reply.send(Response::Done);
                break;
            }
        }
    }
}

fn run_search(
    conn: &mut LdapConn,
    base: &str,
    scope: SearchScope,
    filter: &str,
    attrs: Vec<String>,
) -> Result<Vec<LdapEntry>> {
    let (entries, _res) = conn
        .search(base, scope_to_ldap3(scope), filter, attrs)?
        .success()
        .with_context(|| format!("searching {base}"))?;
    Ok(entries
        .into_iter()
        .map(SearchEntry::construct)
        .map(to_ldap_entry)
        .collect())
}

fn fetch_subschema(conn: &mut LdapConn, base_dn: &str) -> Result<RawSubschema> {
    // 1. Find the subschema subentry DN (operational attribute on the base entry).
    let (entries, _res) = conn
        .search(
            base_dn,
            Scope::Base,
            "(objectClass=*)",
            vec!["subschemaSubentry"],
        )?
        .success()
        .context("reading subschemaSubentry")?;
    let subschema_dn = entries
        .into_iter()
        .map(SearchEntry::construct)
        .find_map(|e| {
            e.attrs
                .get("subschemaSubentry")
                .and_then(|v| v.first().cloned())
        })
        .ok_or_else(|| anyhow!("server did not expose subschemaSubentry on {base_dn}"))?;

    // 2. Read the schema definition strings from that entry.
    let (entries, _res) = conn
        .search(
            &subschema_dn,
            Scope::Base,
            "(objectClass=subschema)",
            vec!["objectClasses", "attributeTypes", "ldapSyntaxes"],
        )?
        .success()
        .context("reading subschema definitions")?;
    let entry = entries
        .into_iter()
        .map(SearchEntry::construct)
        .next()
        .ok_or_else(|| anyhow!("subschema entry {subschema_dn} not found"))?;

    Ok(RawSubschema {
        object_classes: entry
            .attrs
            .get("objectClasses")
            .cloned()
            .unwrap_or_default(),
        attribute_types: entry
            .attrs
            .get("attributeTypes")
            .cloned()
            .unwrap_or_default(),
        ldap_syntaxes: entry.attrs.get("ldapSyntaxes").cloned().unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_maps_to_ldap3() {
        assert!(matches!(scope_to_ldap3(SearchScope::Base), Scope::Base));
        assert!(matches!(
            scope_to_ldap3(SearchScope::OneLevel),
            Scope::OneLevel
        ));
    }

    #[test]
    fn search_entry_conversion() {
        // Build a SearchEntry fixture directly (its fields are public).
        let mut attrs = std::collections::HashMap::new();
        attrs.insert("cn".to_string(), vec!["alice".to_string()]);
        attrs.insert(
            "objectClass".to_string(),
            vec!["top".to_string(), "person".to_string()],
        );
        let mut bin_attrs = std::collections::HashMap::new();
        // Two binary values of 3 and 5 bytes -> byte count 8.
        bin_attrs.insert("jpegPhoto".to_string(), vec![vec![0u8; 3], vec![0u8; 5]]);
        let se = SearchEntry {
            dn: "cn=alice,dc=example,dc=org".to_string(),
            attrs,
            bin_attrs,
        };

        let entry = to_ldap_entry(se);
        assert_eq!(entry.dn, "cn=alice,dc=example,dc=org");
        assert_eq!(entry.attrs.get("cn"), Some(&vec!["alice".to_string()]));
        assert_eq!(
            entry.attrs.get("objectClass"),
            Some(&vec!["top".to_string(), "person".to_string()])
        );
        // Byte counts summed across values.
        assert_eq!(entry.bin_attrs.get("jpegPhoto"), Some(&8usize));
    }

    #[test]
    fn submit_then_poll_roundtrip() {
        // Test the poll() wrapper's semantics over the long-lived channel without
        // a live connection: push a Response, poll once (Some), poll again (None).
        let (resp_tx, resp_rx) = mpsc::channel::<Response>();
        let (tx, _rx) = mpsc::channel::<Job>();
        let handle = WorkerHandle {
            tx,
            resp_tx: resp_tx.clone(),
            resp_rx,
            join: None,
        };
        resp_tx
            .send(Response::Entries {
                id: 7,
                entries: vec![],
            })
            .unwrap();
        match handle.poll() {
            Some(Response::Entries { id, .. }) => assert_eq!(id, 7),
            _ => panic!("expected Entries with id 7"),
        }
        assert!(handle.poll().is_none(), "second poll should be empty");
    }
}
