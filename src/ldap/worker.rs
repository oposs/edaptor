//! The background LDAP worker thread. It owns the ldap3 connection and is the
//! only place network I/O happens.
//!
//! Two request paths coexist (D3):
//!  * Synchronous: [`WorkerHandle::request`] sends a job with a fresh per-call
//!    reply channel and blocks for the answer. Used for the startup
//!    `FetchSubschema` fetch.
//!  * Non-blocking: [`WorkerHandle::submit`] fires a `Search` job whose reply
//!    sender is a clone of a long-lived response channel; [`WorkerHandle::poll`]
//!    drains that channel without blocking. Used by the read flow so the
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

use std::collections::HashSet;

use ldap3::adapters::{Adapter, EntriesOnly, PagedResults};
use ldap3::{LdapConn, Mod, Scope, SearchEntry, SearchOptions, SearchResult};

use crate::config::{AuthMethod, Config};
use crate::form::changeset::ModOp;
use crate::ldap::result::result_code_message;
use crate::ldap::tls::build_settings;

/// Search scope for a [`Request::Search`]. Mapped to `ldap3::Scope` only inside
/// the worker (see [`scope_to_ldap3`]) so `ldap3` types do not leak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchScope {
    /// The base entry itself (read a single entry).
    Base,
    /// The immediate children of the base (one-level browse).
    OneLevel,
    /// The entire subtree under the base (used for the eager structure scan).
    Subtree,
}

/// A flattened LDAP entry for the UI. String attribute values are carried
/// verbatim; binary attributes are reduced to a byte count (sum of value
/// lengths) so the read-only form can render `<N bytes>` without copying blobs.
/// `BTreeMap` gives deterministic ordering for tests and display.
/// One entry from the eager structure scan: DN + display label inputs + objectClass.
/// Deliberately minimal (no full attributes) so a 100k-entry directory stays cheap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructureNodeRaw {
    /// Distinguished name (the structural key).
    pub dn: String,
    /// `cn` first value, if present (label preference 1).
    pub cn: Option<String>,
    /// `description` first value, if present (label preference 2).
    pub description: Option<String>,
    /// objectClass values (kept for future domain classification).
    pub object_classes: Vec<String>,
}

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
        /// Optional server-side size limit (picker type-ahead caps at ~20).
        size_limit: Option<i32>,
    },
    /// Eagerly load the entire subtree structure under `base` (paged). `id` is
    /// echoed in the reply for correlation.
    LoadStructure {
        /// Correlation id.
        id: u64,
        /// Base DN to scan (the whole subtree below + including it).
        base: String,
        /// Paged-results page size (e.g. 500).
        page_size: i32,
    },
    /// Modify an entry's attributes. `id` is echoed in the reply (D4).
    Modify {
        /// Correlation id.
        id: u64,
        /// Target DN.
        dn: String,
        /// The attribute modifications (pure domain type from `form::changeset`).
        changes: Vec<ModOp>,
    },
    /// Add a new entry. `id` is echoed in the reply.
    Add {
        /// Correlation id.
        id: u64,
        /// New entry's DN.
        dn: String,
        /// Attribute values for the new entry.
        attrs: BTreeMap<String, Vec<String>>,
    },
    /// Rename an entry (MODRDN). `id` is echoed in the reply.
    ModRdn {
        /// Correlation id.
        id: u64,
        /// Current DN.
        dn: String,
        /// The new RDN, e.g. `cn=Bob`.
        new_rdn: String,
        /// Whether to delete the old RDN attribute value.
        delete_old: bool,
        /// Optional new superior (parent) DN. `None` in M4.
        new_superior: Option<String>,
    },
    /// Delete an entry. `id` is echoed in the reply.
    Delete {
        /// Correlation id.
        id: u64,
        /// DN to delete.
        dn: String,
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

#[derive(Debug, Clone)]
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
    /// Result of a [`Request::LoadStructure`] eager scan; `id` echoes the request.
    StructureEntries {
        /// Correlation id.
        id: u64,
        /// Every entry under the base (paged), minimal payload.
        nodes: Vec<StructureNodeRaw>,
    },
    /// A failed [`Request::LoadStructure`]; `id` echoes the request. `truncated`
    /// is true when the server refused to page (rc 3/4/11) so the UI can fall back
    /// to lazy one-level browsing.
    StructureError {
        /// Correlation id.
        id: u64,
        /// Human-readable error message.
        msg: String,
        /// True if the failure was a size/time/admin limit (fallback signal).
        truncated: bool,
    },
    /// A successful write (Modify/Add/ModRdn/Delete); `id` echoes the request.
    WriteOk {
        /// Correlation id.
        id: u64,
        /// The affected DN (post-rename DN for ModRdn is computed by the caller).
        dn: String,
    },
    /// A failed write; `id` echoes the request. `msg` is already human-mapped
    /// from the LDAP result code by [`result_code_message`].
    WriteError {
        /// Correlation id.
        id: u64,
        /// Human-readable error message.
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
        SearchScope::Subtree => Scope::Subtree,
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
                size_limit,
            } => {
                let resp = match run_search(conn, &base, scope, &filter, attrs, size_limit) {
                    Ok(entries) => Response::Entries { id, entries },
                    Err(e) => Response::SearchError {
                        id,
                        msg: format!("{e:#}"),
                    },
                };
                let _ = reply.send(resp);
            }
            Request::LoadStructure {
                id,
                base,
                page_size,
            } => {
                let resp = match run_load_structure(conn, &base, page_size) {
                    Ok(nodes) => Response::StructureEntries { id, nodes },
                    Err((msg, truncated)) => Response::StructureError { id, msg, truncated },
                };
                let _ = reply.send(resp);
            }
            Request::Modify { id, dn, changes } => {
                let _ = reply.send(run_modify(conn, id, &dn, &changes));
            }
            Request::Add { id, dn, attrs } => {
                let _ = reply.send(run_add(conn, id, &dn, &attrs));
            }
            Request::ModRdn {
                id,
                dn,
                new_rdn,
                delete_old,
                new_superior,
            } => {
                let _ = reply.send(run_modrdn(
                    conn,
                    id,
                    &dn,
                    &new_rdn,
                    delete_old,
                    new_superior.as_deref(),
                ));
            }
            Request::Delete { id, dn } => {
                let _ = reply.send(run_delete(conn, id, &dn));
            }
            Request::Shutdown => {
                let _ = conn.unbind();
                let _ = reply.send(Response::Done);
                break;
            }
        }
    }
}

/// Convert a domain [`ModOp`] into an ldap3 [`Mod`] (worker-private so `ldap3`
/// does not leak past the worker). Values become a `HashSet` (ldap3's shape).
fn mod_op_to_ldap3(op: &ModOp) -> Mod<String> {
    match op {
        ModOp::Add { attr, values } => Mod::Add(attr.clone(), values.iter().cloned().collect()),
        ModOp::Delete { attr, values } => {
            Mod::Delete(attr.clone(), values.iter().cloned().collect())
        }
        ModOp::Replace { attr, values } => {
            Mod::Replace(attr.clone(), values.iter().cloned().collect())
        }
    }
}

/// Turn an ldap3 write call's `Result<LdapResult>` into a [`Response`]: a zero
/// result code is `WriteOk`; a non-zero code or transport error is `WriteError`
/// with the human-mapped message (spec §10).
fn write_response(id: u64, dn: &str, res: ldap3::result::Result<ldap3::LdapResult>) -> Response {
    match res {
        Ok(r) if r.rc == 0 => Response::WriteOk {
            id,
            dn: dn.to_string(),
        },
        Ok(r) => Response::WriteError {
            id,
            msg: result_code_message(r.rc, &r.text),
        },
        Err(e) => Response::WriteError {
            id,
            msg: format!("{e}"),
        },
    }
}

fn run_modify(conn: &mut LdapConn, id: u64, dn: &str, changes: &[ModOp]) -> Response {
    let mods: Vec<Mod<String>> = changes.iter().map(mod_op_to_ldap3).collect();
    write_response(id, dn, conn.modify(dn, mods))
}

fn run_add(
    conn: &mut LdapConn,
    id: u64,
    dn: &str,
    attrs: &BTreeMap<String, Vec<String>>,
) -> Response {
    let entry: Vec<(String, HashSet<String>)> = attrs
        .iter()
        .map(|(k, vs)| (k.clone(), vs.iter().cloned().collect::<HashSet<String>>()))
        .collect();
    write_response(id, dn, conn.add(dn, entry))
}

fn run_modrdn(
    conn: &mut LdapConn,
    id: u64,
    dn: &str,
    new_rdn: &str,
    delete_old: bool,
    new_superior: Option<&str>,
) -> Response {
    write_response(id, dn, conn.modifydn(dn, new_rdn, delete_old, new_superior))
}

fn run_delete(conn: &mut LdapConn, id: u64, dn: &str) -> Response {
    write_response(id, dn, conn.delete(dn))
}

/// True for the LDAP result codes that mean "the server capped the result set"
/// (time/size/admin limit). Used to decide whether to fall back to lazy browsing.
fn is_limit_rc(rc: u32) -> bool {
    matches!(rc, 3 | 4 | 11)
}

/// Page through the entire subtree under `base` (RFC 2696) and return minimal
/// per-entry structure data. Bypasses the server's per-request size limit. On a
/// time/size/admin limit it returns the entries gathered so far paired with a
/// `truncated` flag so the caller can fall back to lazy browsing.
fn run_load_structure(
    conn: &mut LdapConn,
    base: &str,
    page_size: i32,
) -> std::result::Result<Vec<StructureNodeRaw>, (String, bool)> {
    let adapters: Vec<Box<dyn Adapter<_, _>>> = vec![
        Box::new(EntriesOnly::new()),
        Box::new(PagedResults::new(page_size)),
    ];
    let attrs = vec![
        "cn".to_string(),
        "description".to_string(),
        "objectClass".to_string(),
    ];
    let mut stream = conn
        .streaming_search_with(adapters, base, Scope::Subtree, "(objectClass=*)", attrs)
        .map_err(|e| (format!("{e}"), false))?;

    let mut out = Vec::new();
    loop {
        match stream.next() {
            Ok(Some(re)) => {
                let se = SearchEntry::construct(re);
                out.push(structure_node_from(se));
            }
            Ok(None) => break,
            Err(e) => return Err((format!("{e}"), false)),
        }
    }

    match stream.result().success() {
        Ok(_) => Ok(out),
        Err(ldap3::LdapError::LdapResult { result }) if is_limit_rc(result.rc) => {
            Err((result_code_message(result.rc, &result.text), true))
        }
        Err(e) => Err((format!("{e}"), false)),
    }
}

/// First value of a (case-sensitive ldap3 key) attribute from a SearchEntry.
fn first_attr(se: &SearchEntry, attr: &str) -> Option<String> {
    se.attrs.get(attr).and_then(|v| v.first().cloned())
}

/// Flatten a SearchEntry into the minimal structure payload.
fn structure_node_from(se: SearchEntry) -> StructureNodeRaw {
    let cn = first_attr(&se, "cn");
    let description = first_attr(&se, "description");
    let object_classes = se.attrs.get("objectClass").cloned().unwrap_or_default();
    StructureNodeRaw {
        dn: se.dn,
        cn,
        description,
        object_classes,
    }
}

fn run_search(
    conn: &mut LdapConn,
    base: &str,
    scope: SearchScope,
    filter: &str,
    attrs: Vec<String>,
    size_limit: Option<i32>,
) -> Result<Vec<LdapEntry>> {
    if let Some(n) = size_limit {
        // `with_search_options` applies to the next search on this conn.
        conn.with_search_options(SearchOptions::new().sizelimit(n));
    }
    // Destructure the SearchResult directly so we can inspect the result code:
    // rc==0 → clean success; is_limit_rc(rc) → server capped the set but the
    // partial entries are still valid (spec §7 requires showing them); any other
    // non-zero rc is a real error.
    let SearchResult(raw_entries, res) = conn.search(base, scope_to_ldap3(scope), filter, attrs)?;
    if res.rc != 0 && !is_limit_rc(res.rc) {
        return Err(anyhow!(result_code_message(res.rc, &res.text)))
            .with_context(|| format!("searching {base}"));
    }
    Ok(raw_entries
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
    fn limit_rc_triggers_truncation_fallback() {
        assert!(is_limit_rc(3)); // timeLimitExceeded
        assert!(is_limit_rc(4)); // sizeLimitExceeded
        assert!(is_limit_rc(11)); // adminLimitExceeded
        assert!(!is_limit_rc(0));
        assert!(!is_limit_rc(32)); // noSuchObject
    }

    #[test]
    fn scope_maps_to_ldap3() {
        assert!(matches!(scope_to_ldap3(SearchScope::Base), Scope::Base));
        assert!(matches!(
            scope_to_ldap3(SearchScope::OneLevel),
            Scope::OneLevel
        ));
        assert!(matches!(
            scope_to_ldap3(SearchScope::Subtree),
            Scope::Subtree
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

    #[test]
    fn submit_then_poll_write_ok_roundtrip() {
        // Same pattern as the search roundtrip, but for a write reply.
        let (resp_tx, resp_rx) = mpsc::channel::<Response>();
        let (tx, _rx) = mpsc::channel::<Job>();
        let handle = WorkerHandle {
            tx,
            resp_tx: resp_tx.clone(),
            resp_rx,
            join: None,
        };
        resp_tx
            .send(Response::WriteOk {
                id: 9,
                dn: "cn=x,dc=example,dc=org".to_string(),
            })
            .unwrap();
        match handle.poll() {
            Some(Response::WriteOk { id, dn }) => {
                assert_eq!(id, 9);
                assert_eq!(dn, "cn=x,dc=example,dc=org");
            }
            _ => panic!("expected WriteOk with id 9"),
        }
        assert!(handle.poll().is_none());
    }

    #[test]
    fn write_response_maps_codes() {
        // rc 0 -> WriteOk; non-zero -> WriteError with the human message.
        let ok = write_response(1, "cn=a,dc=x", Ok(make_result(0, "")));
        assert!(matches!(ok, Response::WriteOk { id: 1, .. }));

        let err = write_response(2, "cn=a,dc=x", Ok(make_result(32, "no such object")));
        match err {
            Response::WriteError { id, msg } => {
                assert_eq!(id, 2);
                assert!(msg.starts_with("No such object"), "msg={msg}");
            }
            _ => panic!("expected WriteError"),
        }
    }

    fn make_result(rc: u32, text: &str) -> ldap3::LdapResult {
        ldap3::LdapResult {
            rc,
            matched: String::new(),
            text: text.to_string(),
            refs: Vec::new(),
            ctrls: Vec::new(),
        }
    }

    #[test]
    fn mod_op_converts_to_ldap3() {
        let m = mod_op_to_ldap3(&ModOp::Replace {
            attr: "sn".to_string(),
            values: vec!["Brown".to_string()],
        });
        match m {
            Mod::Replace(attr, vals) => {
                assert_eq!(attr, "sn");
                assert!(vals.contains("Brown"));
            }
            _ => panic!("expected Replace"),
        }
    }

    #[test]
    fn search_request_has_size_limit_field() {
        // Compile-level guarantee that the field exists and defaults are explicit.
        let r = Request::Search {
            id: 1,
            base: "dc=x".into(),
            scope: SearchScope::OneLevel,
            filter: "(objectClass=*)".into(),
            attrs: vec!["cn".into()],
            size_limit: Some(20),
        };
        match r {
            Request::Search { size_limit, .. } => assert_eq!(size_limit, Some(20)),
            _ => panic!(),
        }
    }
}
