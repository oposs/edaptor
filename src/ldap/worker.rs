//! The background LDAP worker thread. It owns the ldap3 connection and is the
//! only place network I/O happens. Callers send a Request and block for a
//! Response over a per-request reply channel.

use anyhow::{anyhow, Context, Result};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use ldap3::{LdapConn, Scope, SearchEntry};

use crate::config::{AuthMethod, Config};
use crate::ldap::tls::build_settings;

/// A request to the worker. Each is paired with a reply Sender in the channel.
pub enum Request {
    /// Fetch the raw (unparsed) subschema description strings.
    FetchSubschema,
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
    Done,
    Error(String),
}

type Job = (Request, Sender<Response>);

pub struct WorkerHandle {
    tx: Sender<Job>,
    join: Option<JoinHandle<()>>,
}

impl WorkerHandle {
    /// Spawn the worker, connecting + binding synchronously so connection or
    /// credential failures surface immediately as an Err from spawn().
    pub fn spawn(config: Config, password: String) -> Result<WorkerHandle> {
        let (tx, rx) = mpsc::channel::<Job>();
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

    /// Send a request and block for its response.
    pub fn request(&self, req: Request) -> Result<Response> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.tx
            .send((req, reply_tx))
            .map_err(|_| anyhow!("worker thread is gone"))?;
        reply_rx
            .recv()
            .map_err(|_| anyhow!("worker dropped the reply channel"))
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
            Request::Shutdown => {
                let _ = conn.unbind();
                let _ = reply.send(Response::Done);
                break;
            }
        }
    }
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
