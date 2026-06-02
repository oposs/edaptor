//! Live rich-templates integration tests against a containerized OpenLDAP.
//!
//! Enable by setting EDAPTOR_TEST_LDAP_URI (e.g. ldap://localhost:1389).
//! Start a server with: scripts/test-ldap.sh start
//! When the env var is unset each test prints SKIP and passes (no silent skip).
//!
//! Covers the **create-time** password path (Task 4.6): unlike `live_samba.rs`
//! (which sets the password via a post-ADD MODIFY), this proves the password is
//! carried in the initial `Add` itself — exactly what `prepare_create` does via
//! `password_add_attrs`:
//!   ADD user with `password_add_attrs(...)` folded in -> re-bind as that user DN
//!   with the cleartext (a successful bind = `userPassword` took at create time)
//!   -> DELETE.
//!
//! The Samba side of the create-time password (`sambaNTPassword` == nt_hash) is
//! pinned by the pure unit tests on `password_add_attrs` (src/samba/password.rs)
//! and the live MODIFY round-trip in `live_samba.rs`; the bitnami test image does
//! not ship the Samba schema, so a create-time `sambaSamAccount` ADD cannot be
//! exercised here.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use edaptor::config::{AuthConfig, AuthMethod, Config, PasswordSource, ServerConfig, TlsConfig};
use edaptor::ldap::worker::{Request, Response, SearchScope, WorkerHandle};
use edaptor::samba::password::password_add_attrs;

/// Admin config + bind password for the test directory.
fn admin_config(uri: String) -> (Config, String) {
    let config = Config {
        server: ServerConfig {
            uri,
            base_dn: "dc=example,dc=org".to_string(),
            start_tls: false,
            read_only: false,
            timeout_secs: 10,
            tls: TlsConfig::default(),
        },
        auth: AuthConfig {
            method: AuthMethod::Simple,
            bind_dn: Some("cn=admin,dc=example,dc=org".to_string()),
            password_source: PasswordSource::Env("EDAPTOR_TEST_ADMIN_PW".to_string()),
        },
        profiles: Vec::new(),
        samba: Default::default(),
        relations: Vec::new(),
    };
    let password =
        std::env::var("EDAPTOR_TEST_ADMIN_PW").unwrap_or_else(|_| "adminpassword".to_string());
    (config, password)
}

/// A config that binds as `bind_dn` with `password` (used to prove the password
/// took by re-binding as the freshly created user).
fn user_config(uri: String, bind_dn: &str, password: &str) -> (Config, String) {
    let config = Config {
        server: ServerConfig {
            uri,
            base_dn: "dc=example,dc=org".to_string(),
            start_tls: false,
            read_only: false,
            timeout_secs: 10,
            tls: TlsConfig::default(),
        },
        auth: AuthConfig {
            method: AuthMethod::Simple,
            bind_dn: Some(bind_dn.to_string()),
            password_source: PasswordSource::Prompt,
        },
        profiles: Vec::new(),
        samba: Default::default(),
        relations: Vec::new(),
    };
    (config, password.to_string())
}

/// Poll the worker channel until a reply correlated to `want_id` arrives.
fn poll_for_id(worker: &WorkerHandle, want_id: u64, timeout: Duration) -> Option<Response> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match worker.poll() {
            Some(resp) => match &resp {
                Response::Entries { id, .. }
                | Response::SearchError { id, .. }
                | Response::WriteOk { id, .. }
                | Response::WriteError { id, .. }
                    if *id == want_id =>
                {
                    return Some(resp);
                }
                _ => continue,
            },
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    None
}

fn describe(resp: &Option<Response>) -> String {
    match resp {
        Some(Response::WriteOk { dn, .. }) => format!("WriteOk({dn})"),
        Some(Response::WriteError { msg, .. }) => format!("WriteError({msg})"),
        Some(Response::Entries { entries, .. }) => format!("Entries({})", entries.len()),
        Some(Response::SearchError { msg, .. }) => format!("SearchError({msg})"),
        Some(_) => "other".to_string(),
        None => "timeout".to_string(),
    }
}

#[test]
fn create_time_password_unix_round_trip() {
    let uri = match std::env::var("EDAPTOR_TEST_LDAP_URI") {
        Ok(uri) => uri,
        Err(_) => {
            eprintln!(
                "SKIP create_time_password_unix_round_trip: set EDAPTOR_TEST_LDAP_URI to run"
            );
            return;
        }
    };
    let (config, password) = admin_config(uri.clone());
    let worker = WorkerHandle::spawn(config, password).expect("spawn admin worker");

    let container = "ou=users,dc=example,dc=org";
    let dn = format!("cn=edaptor-create-pw-it,{container}");
    let new_password = "Cr3ate-Time-Pw!";

    // Idempotent cleanup from any prior aborted run.
    let _ = worker.submit(Request::Delete {
        id: 1,
        dn: dn.clone(),
    });
    let _ = poll_for_id(&worker, 1, Duration::from_secs(5));

    // --- ADD a throwaway user, folding the password into the Add exactly as
    //     prepare_create does (non-Samba: userPassword only). ---
    let mut attrs = BTreeMap::new();
    attrs.insert(
        "objectClass".to_string(),
        vec!["top".to_string(), "inetOrgPerson".to_string()],
    );
    attrs.insert("cn".to_string(), vec!["edaptor-create-pw-it".to_string()]);
    attrs.insert("sn".to_string(), vec!["IT".to_string()]);
    for (k, v) in password_add_attrs(new_password, "userPassword", false, 1_700_000_000) {
        attrs.insert(k, v);
    }
    assert_eq!(
        attrs.get("userPassword"),
        Some(&vec![new_password.to_string()]),
        "create-time Add carries the cleartext userPassword"
    );

    worker
        .submit(Request::Add {
            id: 10,
            dn: dn.clone(),
            attrs,
        })
        .expect("submit add");
    match poll_for_id(&worker, 10, Duration::from_secs(10)) {
        Some(Response::WriteOk { .. }) => {}
        other => panic!("create ADD failed: {}", describe(&other)),
    }

    // --- Prove the password took: re-bind as the new user DN with the cleartext.
    //     A successful synchronous bind in spawn() means create-time userPassword
    //     was accepted by the server. ---
    let (user_cfg, user_pw) = user_config(uri.clone(), &dn, new_password);
    let user_worker =
        WorkerHandle::spawn(user_cfg, user_pw).expect("re-bind as the freshly created user");
    user_worker
        .submit(Request::Search {
            id: 30,
            base: dn.clone(),
            scope: SearchScope::Base,
            filter: "(objectClass=*)".to_string(),
            attrs: vec!["cn".to_string()],
            size_limit: None,
        })
        .expect("submit self-read as user");
    match poll_for_id(&user_worker, 30, Duration::from_secs(10)) {
        Some(Response::Entries { entries, .. }) => assert_eq!(entries.len(), 1),
        other => panic!("self-read after re-bind failed: {}", describe(&other)),
    }
    drop(user_worker);

    // --- Cleanup ---
    worker
        .submit(Request::Delete {
            id: 99,
            dn: dn.clone(),
        })
        .expect("submit cleanup delete");
    match poll_for_id(&worker, 99, Duration::from_secs(10)) {
        Some(Response::WriteOk { .. }) => {}
        other => panic!("cleanup DELETE failed: {}", describe(&other)),
    }
}
