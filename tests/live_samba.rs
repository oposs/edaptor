//! Live synced-password round-trip against a containerized OpenLDAP.
//!
//! Enable by setting EDAPTOR_TEST_LDAP_URI (e.g. ldap://localhost:1389).
//! Start a server with: scripts/test-ldap.sh start
//! When the env var is unset the test prints SKIP and passes (no silent skip).
//!
//! It proves the **Unix** side of the synced password end-to-end:
//!   ADD user -> apply build_password_mods -> re-bind as that user DN with the
//!   new password (a successful bind = `userPassword` took) -> DELETE.
//!
//! The bitnami `openldap:2.6.9` test image does NOT ship the Samba schema, so the
//! Samba attributes (`sambaNTPassword`/`sambaPwdLastSet`) cannot be written there.
//! The test probes for the schema by attempting the samba mod-set and detecting
//! `undefinedAttributeType` (rc 17); when absent it `eprintln!`-logs that the
//! Samba assertions were skipped (no silent skip) and asserts only the Unix path.
//! The Samba correctness itself is pinned by the pure unit tests in `src/samba`.
//!
//! NOTE: the production `edaptor::run_passwd` enforces a TLS gate (refusing plain
//! `ldap://`). The plain test container is unencrypted, so this live test drives
//! the same logic via `build_password_mods` + the worker `Modify` path directly —
//! the TLS gate itself is unit-tested in `src/samba/password.rs`.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use edaptor::config::{AuthConfig, AuthMethod, Config, PasswordSource, ServerConfig, TlsConfig};
use edaptor::ldap::worker::{Request, Response, SearchScope, WorkerHandle};
use edaptor::samba::password::build_password_mods;

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
        meta: Default::default(),
        samba: Default::default(),
        tree: Default::default(),
    };
    let password =
        std::env::var("EDAPTOR_TEST_ADMIN_PW").unwrap_or_else(|_| "adminpassword".to_string());
    (config, password)
}

/// A config that binds as `bind_dn` with `password` (used to prove the Unix
/// password took by re-binding as the target user).
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
        meta: Default::default(),
        samba: Default::default(),
        tree: Default::default(),
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
fn synced_password_unix_round_trip() {
    let uri = match std::env::var("EDAPTOR_TEST_LDAP_URI") {
        Ok(uri) => uri,
        Err(_) => {
            eprintln!("SKIP synced_password_unix_round_trip: set EDAPTOR_TEST_LDAP_URI to run");
            return;
        }
    };
    let (config, password) = admin_config(uri.clone());
    let worker = WorkerHandle::spawn(config, password).expect("spawn admin worker");

    let container = "ou=users,dc=example,dc=org";
    let dn = format!("cn=edaptor-passwd-it,{container}");
    let new_password = "S3cr3t-Synced-Pw!";

    // Idempotent cleanup from any prior aborted run.
    let _ = worker.submit(Request::Delete {
        id: 1,
        dn: dn.clone(),
        assert_csn: None,
    });
    let _ = poll_for_id(&worker, 1, Duration::from_secs(5));

    // --- ADD a throwaway user with an initial password ---
    let mut attrs = BTreeMap::new();
    attrs.insert(
        "objectClass".to_string(),
        vec!["top".to_string(), "inetOrgPerson".to_string()],
    );
    attrs.insert("cn".to_string(), vec!["edaptor-passwd-it".to_string()]);
    attrs.insert("sn".to_string(), vec!["IT".to_string()]);
    attrs.insert("userPassword".to_string(), vec!["initial-pw".to_string()]);
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

    // --- Probe for the Samba schema by attempting the samba mod-set ---
    // (userPassword + sambaNTPassword + sambaPwdLastSet). If the schema is absent,
    // OpenLDAP rejects with undefinedAttributeType (rc 17).
    let samba_mods = build_password_mods(new_password, true, 1_700_000_000);
    worker
        .submit(Request::Modify {
            id: 20,
            dn: dn.clone(),
            changes: samba_mods,
            assert_csn: None,
        })
        .expect("submit samba modify");
    let samba_present = match poll_for_id(&worker, 20, Duration::from_secs(10)) {
        Some(Response::WriteOk { .. }) => true,
        Some(Response::WriteError { msg, .. }) => {
            let low = msg.to_lowercase();
            assert!(
                low.contains("undefined") || low.contains("attribute"),
                "expected an undefinedAttributeType-style error when the Samba \
                 schema is absent, got: {msg}"
            );
            eprintln!("skipping Samba assertions: sambaSamAccount schema not present ({msg})");
            false
        }
        other => panic!("Samba probe MODIFY returned: {}", describe(&other)),
    };

    // --- Ensure the Unix password is set to `new_password` ---
    // When the Samba schema was present the combined MODIFY already set
    // userPassword; otherwise apply the Unix-only mod-set now.
    if !samba_present {
        let unix_mods = build_password_mods(new_password, false, 1_700_000_000);
        worker
            .submit(Request::Modify {
                id: 21,
                dn: dn.clone(),
                changes: unix_mods,
                assert_csn: None,
            })
            .expect("submit unix modify");
        match poll_for_id(&worker, 21, Duration::from_secs(10)) {
            Some(Response::WriteOk { .. }) => {}
            other => panic!("Unix-only MODIFY failed: {}", describe(&other)),
        }
    }

    // --- Prove the Unix side: re-bind as the user DN with the NEW password ---
    // A successful synchronous bind in spawn() means userPassword took.
    let (user_cfg, user_pw) = user_config(uri.clone(), &dn, new_password);
    let user_worker =
        WorkerHandle::spawn(user_cfg, user_pw).expect("re-bind as the user with the new password");
    // A read of self proves the bound session is usable too.
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

    // --- If the Samba schema WAS present, assert the samba attrs were written ---
    if samba_present {
        worker
            .submit(Request::Search {
                id: 40,
                base: dn.clone(),
                scope: SearchScope::Base,
                filter: "(objectClass=*)".to_string(),
                attrs: vec!["sambaNTPassword".to_string(), "sambaPwdLastSet".to_string()],
                size_limit: None,
            })
            .expect("submit samba attr read");
        match poll_for_id(&worker, 40, Duration::from_secs(10)) {
            Some(Response::Entries { entries, .. }) => {
                let e = entries.into_iter().next().expect("entry exists");
                // The NT hash of `new_password`: uppercase hex of MD4(UTF-16LE).
                let nt = e
                    .attrs
                    .get("sambaNTPassword")
                    .and_then(|v| v.first())
                    .expect("sambaNTPassword should be present");
                assert_eq!(nt.len(), 32, "NT hash is 32 hex chars");
                assert_eq!(nt.to_ascii_uppercase(), *nt, "NT hash is uppercase");
                assert_eq!(
                    e.attrs.get("sambaPwdLastSet"),
                    Some(&vec!["1700000000".to_string()]),
                    "sambaPwdLastSet should be the injected timestamp"
                );
            }
            other => panic!("samba attr read failed: {}", describe(&other)),
        }
    }

    // --- Cleanup ---
    worker
        .submit(Request::Delete {
            id: 99,
            dn: dn.clone(),
            assert_csn: None,
        })
        .expect("submit cleanup delete");
    match poll_for_id(&worker, 99, Duration::from_secs(10)) {
        Some(Response::WriteOk { .. }) => {}
        other => panic!("cleanup DELETE failed: {}", describe(&other)),
    }
}
