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
        meta: Default::default(),
        samba: Default::default(),
        tree: Default::default(),
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
                | Response::WriteConflict { id, .. }
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
            size_limit: None,
        })
        .expect("submit base search");
    match poll_for_id(worker, id, Duration::from_secs(10)) {
        Some(Response::Entries { entries, .. }) => entries.into_iter().next().map(|e| e.attrs),
        _ => None,
    }
}

/// Read the entry's `entryCSN` (a server-maintained operational attribute, not
/// returned by a plain `"*"` search — it must be requested by name).
fn read_entry_csn(worker: &WorkerHandle, dn: &str, id: u64) -> Option<String> {
    worker
        .submit(Request::Search {
            id,
            base: dn.to_string(),
            scope: SearchScope::Base,
            filter: "(objectClass=*)".to_string(),
            attrs: vec!["entryCSN".to_string()],
            size_limit: None,
        })
        .expect("submit entryCSN search");
    match poll_for_id(worker, id, Duration::from_secs(10)) {
        Some(Response::Entries { entries, .. }) => entries
            .into_iter()
            .next()
            .and_then(|e| e.attrs.get("entryCSN").and_then(|v| v.first().cloned())),
        _ => None,
    }
}

fn describe(resp: &Option<Response>) -> String {
    match resp {
        Some(Response::WriteOk { dn, .. }) => format!("WriteOk({dn})"),
        Some(Response::WriteConflict { dn, .. }) => format!("WriteConflict({dn})"),
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
    let cs = diff(&original, &edited, &Default::default()).expect("diff");
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
            assert_csn: None,
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

/// Discovery path for the sambaSID auto-generate widget: a subtree search for
/// `(objectClass=sambaDomain)` must return the seeded domain entry, and the pure
/// `parse_samba_domain` must extract its SID + RID base. Mirrors the exact filter
/// and attrs used by `ui::app::discover_samba_domain` (which is private), so it
/// guards the real end-to-end path against the seed data.
#[test]
fn discovers_samba_domain_sid() {
    let uri = match std::env::var("EDAPTOR_TEST_LDAP_URI") {
        Ok(uri) => uri,
        Err(_) => {
            eprintln!("SKIP discovers_samba_domain_sid: set EDAPTOR_TEST_LDAP_URI to run");
            return;
        }
    };
    let (config, password) = test_config(uri);
    let worker = WorkerHandle::spawn(config, password).expect("spawn worker");

    worker
        .submit(Request::Search {
            id: 70,
            base: "dc=example,dc=org".to_string(),
            scope: SearchScope::Subtree,
            filter: "(objectClass=sambaDomain)".to_string(),
            attrs: vec![
                "sambaSID".to_string(),
                "sambaAlgorithmicRidBase".to_string(),
            ],
            size_limit: Some(5),
        })
        .expect("submit search");

    match poll_for_id(&worker, 70, Duration::from_secs(10)) {
        Some(Response::Entries { entries, .. }) => {
            let info = entries
                .iter()
                .find_map(|e| edaptor::samba::sid::parse_samba_domain(&e.attrs))
                .expect("a sambaDomain entry must parse");
            assert_eq!(info.domain_sid, "S-1-5-21-1234567890-987654321-1122334455");
            assert_eq!(info.algorithmic_rid_base, 1000);
        }
        other => panic!("expected Entries, got {}", describe(&other)),
    }
}

/// Optimistic concurrency: a MODIFY carrying a stale `assert_csn` must be refused
/// with `Response::WriteConflict` (RFC 4528 Assertion control, rc 122), and a
/// MODIFY carrying the current CSN must succeed and hand back a fresh `new_csn`
/// via the RFC 4527 Post-Read control.
#[test]
fn modify_with_stale_csn_conflicts() {
    let uri = match std::env::var("EDAPTOR_TEST_LDAP_URI") {
        Ok(uri) => uri,
        Err(_) => {
            eprintln!("SKIP modify_with_stale_csn_conflicts: EDAPTOR_TEST_LDAP_URI unset");
            return;
        }
    };
    let (config, password) = test_config(uri);
    let worker = WorkerHandle::spawn(config, password).expect("spawn worker");

    let container = "ou=users,dc=example,dc=org";
    let dn = format!("cn=edaptor-csn-it,{container}");

    // Idempotent cleanup from any prior aborted run.
    let _ = worker.submit(Request::Delete {
        id: 100,
        dn: dn.clone(),
    });
    let _ = poll_for_id(&worker, 100, Duration::from_secs(5));

    // --- ADD a scratch entry ---
    let mut attrs = BTreeMap::new();
    attrs.insert(
        "objectClass".to_string(),
        vec!["top".to_string(), "inetOrgPerson".to_string()],
    );
    attrs.insert("cn".to_string(), vec!["edaptor-csn-it".to_string()]);
    attrs.insert("sn".to_string(), vec!["CSN-IT".to_string()]);
    worker
        .submit(Request::Add {
            id: 110,
            dn: dn.clone(),
            attrs,
        })
        .expect("submit add");
    match poll_for_id(&worker, 110, Duration::from_secs(10)) {
        Some(Response::WriteOk { .. }) => {}
        other => panic!("ADD failed: {}", describe(&other)),
    }

    // --- Read the entry's current entryCSN ---
    let current_csn = read_entry_csn(&worker, &dn, 111)
        .expect("entry should carry entryCSN (server-maintained operational attr)");

    // --- MODIFY with a deliberately wrong assert_csn: expect WriteConflict ---
    let mods = vec![edaptor::form::changeset::ModOp::Replace {
        attr: "description".to_string(),
        values: vec!["should not apply".to_string()],
    }];
    worker
        .submit(Request::Modify {
            id: 120,
            dn: dn.clone(),
            changes: mods,
            assert_csn: Some("19700101000000.000000Z#000000#000#000000".to_string()),
        })
        .expect("submit modify with stale csn");
    match poll_for_id(&worker, 120, Duration::from_secs(10)) {
        Some(Response::WriteConflict { .. }) => {}
        other => panic!(
            "expected WriteConflict for stale assert_csn, got {}",
            describe(&other)
        ),
    }

    // Cross-check: the description must NOT have been applied.
    let unchanged = read_entry(&worker, &dn, 121).expect("entry still exists after conflict");
    assert!(
        !unchanged.contains_key("description"),
        "conflicting write must not have applied"
    );

    // --- MODIFY again with the correct current CSN: expect WriteOk + new_csn ---
    let mods = vec![edaptor::form::changeset::ModOp::Replace {
        attr: "description".to_string(),
        values: vec!["hello from edaptor csn test".to_string()],
    }];
    worker
        .submit(Request::Modify {
            id: 130,
            dn: dn.clone(),
            changes: mods,
            assert_csn: Some(current_csn),
        })
        .expect("submit modify with current csn");
    match poll_for_id(&worker, 130, Duration::from_secs(10)) {
        Some(Response::WriteOk { new_csn, .. }) => {
            assert!(
                new_csn.is_some(),
                "expected a fresh entryCSN from the Post-Read control"
            );
        }
        other => panic!(
            "expected WriteOk with new_csn for the correct assert_csn, got {}",
            describe(&other)
        ),
    }

    // --- Cleanup ---
    worker
        .submit(Request::Delete {
            id: 140,
            dn: dn.clone(),
        })
        .expect("submit cleanup delete");
    let _ = poll_for_id(&worker, 140, Duration::from_secs(10));
}
