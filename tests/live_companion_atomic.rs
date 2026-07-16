//! Live integration test for the RFC 5805 atomic multi-add (`Request::AddAtomic`)
//! against a containerized OpenLDAP.
//!
//! Enable by setting EDAPTOR_TEST_LDAP_URI (e.g. ldap://localhost:1389).
//! Start a server with: scripts/test-ldap.sh start
//! When the env var is unset each test prints SKIP and passes (no silent skip).
//!
//! Proves the two properties the companion-create feature relies on:
//!   1. two valid entries submitted in one transaction both land (commit);
//!   2. when the second entry is invalid, the transaction rolls back and the
//!      first entry is NOT created (atomicity).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use edaptor::config::{AuthConfig, AuthMethod, Config, PasswordSource, ServerConfig, TlsConfig};
use edaptor::ldap::worker::{Request, Response, SearchScope, WorkerHandle};

fn test_config(uri: String) -> (Config, String) {
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
        meta: Default::default(),
        samba: Default::default(),
        tree: Default::default(),
    };
    let password =
        std::env::var("EDAPTOR_TEST_ADMIN_PW").unwrap_or_else(|_| "adminpassword".to_string());
    (config, password)
}

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

fn read_entry(worker: &WorkerHandle, dn: &str, id: u64) -> Option<BTreeMap<String, Vec<String>>> {
    worker
        .submit(Request::Search {
            id,
            base: dn.to_string(),
            scope: SearchScope::Base,
            filter: "(objectClass=*)".to_string(),
            attrs: vec!["*".to_string()],
            size_limit: None,
        })
        .expect("submit base search");
    match poll_for_id(worker, id, Duration::from_secs(10)) {
        Some(Response::Entries { entries, .. }) => entries.into_iter().next().map(|e| e.attrs),
        _ => None,
    }
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

fn posix_group(cn: &str, gid: &str) -> BTreeMap<String, Vec<String>> {
    let mut m = BTreeMap::new();
    m.insert(
        "objectClass".to_string(),
        vec!["top".to_string(), "posixGroup".to_string()],
    );
    m.insert("cn".to_string(), vec![cn.to_string()]);
    m.insert("gidNumber".to_string(), vec![gid.to_string()]);
    m
}

/// A posixGroup missing its MUST `gidNumber` — the server rejects it.
fn posix_group_missing_gid(cn: &str) -> BTreeMap<String, Vec<String>> {
    let mut m = BTreeMap::new();
    m.insert(
        "objectClass".to_string(),
        vec!["top".to_string(), "posixGroup".to_string()],
    );
    m.insert("cn".to_string(), vec![cn.to_string()]);
    m
}

/// Best-effort delete (ignores result) so a prior run's leftovers don't fail us.
fn cleanup(worker: &WorkerHandle, dn: &str, id: u64) {
    let _ = worker.submit(Request::Delete {
        id,
        dn: dn.to_string(),
    });
    let _ = poll_for_id(worker, id, Duration::from_secs(5));
}

#[test]
fn add_atomic_commits_both_entries() {
    let uri = match std::env::var("EDAPTOR_TEST_LDAP_URI") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("SKIP add_atomic_commits_both_entries: set EDAPTOR_TEST_LDAP_URI to run");
            return;
        }
    };
    let (config, password) = test_config(uri);
    let worker = WorkerHandle::spawn(config, password).expect("spawn worker");

    let a = "cn=edaptor-atomic-a,ou=groups,dc=example,dc=org";
    let b = "cn=edaptor-atomic-b,ou=groups,dc=example,dc=org";
    cleanup(&worker, a, 1);
    cleanup(&worker, b, 2);

    worker
        .submit(Request::AddAtomic {
            id: 10,
            entries: vec![
                (a.to_string(), posix_group("edaptor-atomic-a", "59001")),
                (b.to_string(), posix_group("edaptor-atomic-b", "59002")),
            ],
        })
        .expect("submit AddAtomic");
    let resp = poll_for_id(&worker, 10, Duration::from_secs(10));
    assert!(
        matches!(resp, Some(Response::WriteOk { .. })),
        "atomic add of two valid entries must commit; got {}",
        describe(&resp)
    );

    assert!(
        read_entry(&worker, a, 11).is_some(),
        "first entry must exist after commit"
    );
    assert!(
        read_entry(&worker, b, 12).is_some(),
        "second entry must exist after commit"
    );

    cleanup(&worker, a, 20);
    cleanup(&worker, b, 21);
}

#[test]
fn add_atomic_rolls_back_on_second_failure() {
    let uri = match std::env::var("EDAPTOR_TEST_LDAP_URI") {
        Ok(u) => u,
        Err(_) => {
            eprintln!(
                "SKIP add_atomic_rolls_back_on_second_failure: set EDAPTOR_TEST_LDAP_URI to run"
            );
            return;
        }
    };
    let (config, password) = test_config(uri);
    let worker = WorkerHandle::spawn(config, password).expect("spawn worker");

    let good = "cn=edaptor-atomic-rb,ou=groups,dc=example,dc=org";
    let bad = "cn=edaptor-atomic-rb2,ou=groups,dc=example,dc=org";
    cleanup(&worker, good, 1);
    cleanup(&worker, bad, 2);

    // First entry valid, second invalid (posixGroup without MUST gidNumber).
    worker
        .submit(Request::AddAtomic {
            id: 30,
            entries: vec![
                (good.to_string(), posix_group("edaptor-atomic-rb", "59010")),
                (
                    bad.to_string(),
                    posix_group_missing_gid("edaptor-atomic-rb2"),
                ),
            ],
        })
        .expect("submit AddAtomic");
    let resp = poll_for_id(&worker, 30, Duration::from_secs(10));
    assert!(
        matches!(resp, Some(Response::WriteError { .. })),
        "an invalid second entry must fail the transaction; got {}",
        describe(&resp)
    );

    // The transaction rolled back: the (valid) first entry must NOT exist.
    assert!(
        read_entry(&worker, good, 31).is_none(),
        "atomic add must roll back the first entry when the second fails"
    );

    // Belt-and-suspenders cleanup in case the server did not roll back.
    cleanup(&worker, good, 40);
    cleanup(&worker, bad, 41);
}
