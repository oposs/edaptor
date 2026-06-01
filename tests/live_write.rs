//! Live write-path integration test against a containerized OpenLDAP.
//!
//! Enable by setting EDAPTOR_TEST_LDAP_URI (e.g. ldap://localhost:1389).
//! Start a server with: scripts/test-ldap.sh start
//! When the env var is unset each test prints SKIP and passes (no silent skip).
//!
//! Exercises the real worker write path end-to-end:
//!   ADD -> base read -> MODIFY (via changeset diff) -> MODRDN -> DELETE,
//! plus a non-leaf delete that must surface the mapped human error.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use edaptor::config::{AuthConfig, AuthMethod, Config, PasswordSource, ServerConfig, TlsConfig};
use edaptor::form::changeset::{diff, EditEntry};
use edaptor::form::validate::{plan_save, SavePlan};
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
        samba: Default::default(),
        relations: Vec::new(),
    };
    let password =
        std::env::var("EDAPTOR_TEST_ADMIN_PW").unwrap_or_else(|_| "adminpassword".to_string());
    (config, password)
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

/// Base-read a DN, returning the entry's string attrs (None if not found).
fn read_entry(worker: &WorkerHandle, dn: &str, id: u64) -> Option<BTreeMap<String, Vec<String>>> {
    worker
        .submit(Request::Search {
            id,
            base: dn.to_string(),
            scope: SearchScope::Base,
            filter: "(objectClass=*)".to_string(),
            attrs: vec!["*".to_string()],
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

#[test]
fn add_modify_modrdn_delete_round_trip() {
    let uri = match std::env::var("EDAPTOR_TEST_LDAP_URI") {
        Ok(uri) => uri,
        Err(_) => {
            eprintln!("SKIP add_modify_modrdn_delete_round_trip: set EDAPTOR_TEST_LDAP_URI to run");
            return;
        }
    };
    let (config, password) = test_config(uri);
    let worker = WorkerHandle::spawn(config, password).expect("spawn worker");

    let container = "ou=users,dc=example,dc=org";
    let dn = format!("cn=edaptor-it,{container}");
    let dn2 = format!("cn=edaptor-it2,{container}");

    // Idempotent cleanup from any prior aborted run.
    for (id, d) in [(1u64, &dn), (2u64, &dn2)] {
        let _ = worker.submit(Request::Delete { id, dn: d.clone() });
        let _ = poll_for_id(&worker, id, Duration::from_secs(5));
    }

    // --- ADD ---
    let mut attrs = BTreeMap::new();
    attrs.insert(
        "objectClass".to_string(),
        vec!["top".to_string(), "inetOrgPerson".to_string()],
    );
    attrs.insert("cn".to_string(), vec!["edaptor-it".to_string()]);
    attrs.insert("sn".to_string(), vec!["IT".to_string()]);
    worker
        .submit(Request::Add {
            id: 10,
            dn: dn.clone(),
            attrs,
        })
        .expect("submit add");
    match poll_for_id(&worker, 10, Duration::from_secs(10)) {
        Some(Response::WriteOk { .. }) => {}
        other => panic!("ADD failed: {}", describe(&other)),
    }

    let entry = read_entry(&worker, &dn, 11).expect("added entry should exist");
    assert!(entry.contains_key("cn"));

    // --- MODIFY (via changeset diff): set description ---
    let original = EditEntry {
        dn: dn.clone(),
        attrs: entry.clone(),
    };
    let mut edited_attrs = entry.clone();
    edited_attrs.insert(
        "description".to_string(),
        vec!["hello from edaptor".to_string()],
    );
    let edited = EditEntry {
        dn: dn.clone(),
        attrs: edited_attrs,
    };
    let cs = diff(&original, &edited).expect("diff");
    assert!(cs.modrdn.is_none(), "description change is not a rename");
    let mods = match plan_save(cs) {
        SavePlan::Modify(mods) => mods,
        other => panic!("expected a Modify plan, got {other:?}"),
    };
    worker
        .submit(Request::Modify {
            id: 20,
            dn: dn.clone(),
            changes: mods,
        })
        .expect("submit modify");
    match poll_for_id(&worker, 20, Duration::from_secs(10)) {
        Some(Response::WriteOk { .. }) => {}
        other => panic!("MODIFY failed: {}", describe(&other)),
    }
    let after = read_entry(&worker, &dn, 21).expect("entry exists after modify");
    assert_eq!(
        after.get("description"),
        Some(&vec!["hello from edaptor".to_string()]),
        "description should be updated"
    );

    // --- MODRDN: rename cn -> edaptor-it2 ---
    worker
        .submit(Request::ModRdn {
            id: 30,
            dn: dn.clone(),
            new_rdn: "cn=edaptor-it2".to_string(),
            delete_old: true,
            new_superior: None,
        })
        .expect("submit modrdn");
    match poll_for_id(&worker, 30, Duration::from_secs(10)) {
        Some(Response::WriteOk { .. }) => {}
        other => panic!("MODRDN failed: {}", describe(&other)),
    }
    assert!(
        read_entry(&worker, &dn2, 31).is_some(),
        "renamed entry should exist at the new DN"
    );
    assert!(
        read_entry(&worker, &dn, 32).is_none(),
        "old DN should no longer resolve"
    );

    // --- DELETE ---
    worker
        .submit(Request::Delete {
            id: 40,
            dn: dn2.clone(),
        })
        .expect("submit delete");
    match poll_for_id(&worker, 40, Duration::from_secs(10)) {
        Some(Response::WriteOk { .. }) => {}
        other => panic!("DELETE failed: {}", describe(&other)),
    }
    assert!(
        read_entry(&worker, &dn2, 41).is_none(),
        "deleted entry should be gone"
    );
}

#[test]
fn delete_non_leaf_reports_human_error() {
    let uri = match std::env::var("EDAPTOR_TEST_LDAP_URI") {
        Ok(uri) => uri,
        Err(_) => {
            eprintln!("SKIP delete_non_leaf_reports_human_error: set EDAPTOR_TEST_LDAP_URI to run");
            return;
        }
    };
    let (config, password) = test_config(uri);
    let worker = WorkerHandle::spawn(config, password).expect("spawn worker");

    // ou=users has children, so deleting it must fail with rc 66 -> human message.
    worker
        .submit(Request::Delete {
            id: 50,
            dn: "ou=users,dc=example,dc=org".to_string(),
        })
        .expect("submit delete");
    match poll_for_id(&worker, 50, Duration::from_secs(10)) {
        Some(Response::WriteError { msg, .. }) => {
            let low = msg.to_lowercase();
            assert!(
                low.contains("non-leaf") || low.contains("children"),
                "expected a mapped non-leaf message, got: {msg}"
            );
        }
        other => panic!("expected a WriteError, got {}", describe(&other)),
    }
}
