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

/// True if a WriteError message looks like a missing-objectClass / schema problem
/// (the test image lacks the relevant schema). Used to SKIP instead of fail.
fn is_schema_missing(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("objectclass")
        || m.contains("object class")
        || m.contains("no such attribute")
        || m.contains("undefined attribute")
        || m.contains("not allowed by")
        || m.contains("schema")
}

/// Task 3.4 — autonumber allocation + multi-objectClass create (gated).
///
/// `allocate_number`/`decide_allocation` are private to app.rs, so we exercise the
/// REAL pieces they compose: the worker subtree scan over `(uidNumber=*)` plus the
/// public `edaptor::config::defaults::next_in_range`. We also prove the
/// allocate-then-create flow end to end by feeding the allocated number into a
/// multi-objectClass posixAccount ADD that supplies every MUST attribute.
///
/// Requires the posix (nis) schema on the server (posixAccount). If the image
/// lacks it, the first ADD fails with a schema/objectClass error and the test
/// prints a SKIP and returns so the suite stays green.
#[test]
fn autonumber_allocation_and_multi_oc_create() {
    let uri = match std::env::var("EDAPTOR_TEST_LDAP_URI") {
        Ok(uri) => uri,
        Err(_) => {
            eprintln!(
                "SKIP autonumber_allocation_and_multi_oc_create: set EDAPTOR_TEST_LDAP_URI to run"
            );
            return;
        }
    };
    let (config, password) = admin_config(uri.clone());
    let worker = WorkerHandle::spawn(config, password).expect("spawn admin worker");

    let container = "ou=users,dc=example,dc=org";
    let main_dn = format!("uid=edaptor-it-autonum,{container}");
    let throwaway_a = format!("uid=edaptor-it-autonum-a,{container}");
    let throwaway_b = format!("uid=edaptor-it-autonum-b,{container}");

    // Idempotent cleanup of all three DNs so the initial scan is deterministic.
    // id range 200-299 for this test.
    for (i, dn) in [&main_dn, &throwaway_a, &throwaway_b].iter().enumerate() {
        let id = 200 + i as u64;
        let _ = worker.submit(Request::Delete {
            id,
            dn: (*dn).clone(),
        });
        let _ = poll_for_id(&worker, id, Duration::from_secs(5));
    }

    // --- Initial scan: collect existing uidNumbers across the whole base. ---
    let scan_uidnumbers = |scan_id: u64| -> Vec<u64> {
        worker
            .submit(Request::Search {
                id: scan_id,
                base: "dc=example,dc=org".to_string(),
                scope: SearchScope::Subtree,
                filter: "(uidNumber=*)".to_string(),
                attrs: vec!["uidNumber".to_string()],
                size_limit: None,
            })
            .expect("submit uidNumber scan");
        match poll_for_id(&worker, scan_id, Duration::from_secs(10)) {
            Some(Response::Entries { entries, .. }) => entries
                .iter()
                .filter_map(|e| e.attrs.get("uidNumber"))
                .flatten()
                .filter_map(|s| s.trim().parse::<u64>().ok())
                .collect(),
            other => panic!("uidNumber scan failed: {}", describe(&other)),
        }
    };

    let existing = scan_uidnumbers(210);
    // Choose a window above any seeded/leftover value so the window contains ONLY
    // our two throwaways, making the allocation assertion robust.
    let base = existing.iter().copied().max().unwrap_or(0) + 10;
    let min = base;
    let max = base + 1000;

    // --- ADD throwaway A at `base` and B at `base+2`, leaving a gap at base+1. ---
    // Throwaway A is also the schema probe: if posixAccount is unknown we SKIP.
    let add_posix_user = |id: u64, dn: &str, uid: &str, uidnum: u64| -> Response {
        let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        attrs.insert(
            "objectClass".to_string(),
            vec![
                "top".to_string(),
                "account".to_string(),
                "posixAccount".to_string(),
            ],
        );
        // posixAccount MUST: cn, uid, uidNumber, gidNumber, homeDirectory.
        attrs.insert("cn".to_string(), vec![uid.to_string()]);
        attrs.insert("uid".to_string(), vec![uid.to_string()]);
        attrs.insert("uidNumber".to_string(), vec![uidnum.to_string()]);
        attrs.insert("gidNumber".to_string(), vec![uidnum.to_string()]);
        attrs.insert("homeDirectory".to_string(), vec![format!("/home/{uid}")]);
        worker
            .submit(Request::Add {
                id,
                dn: dn.to_string(),
                attrs,
            })
            .expect("submit posixAccount add");
        poll_for_id(&worker, id, Duration::from_secs(10))
            .unwrap_or_else(|| panic!("add timed out for {dn}"))
    };

    match add_posix_user(220, &throwaway_a, "edaptor-it-autonum-a", base) {
        Response::WriteOk { .. } => {}
        Response::WriteError { msg, .. } if is_schema_missing(&msg) => {
            eprintln!(
                "SKIP autonumber_allocation_and_multi_oc_create: posix schema absent on image ({msg})"
            );
            return;
        }
        other => panic!("throwaway A ADD failed: {}", describe(&Some(other))),
    }
    match add_posix_user(221, &throwaway_b, "edaptor-it-autonum-b", base + 2) {
        Response::WriteOk { .. } => {}
        other => panic!("throwaway B ADD failed: {}", describe(&Some(other))),
    }

    // --- Re-scan AFTER both throwaway ADDs completed (each polled to WriteOk
    //     above), then allocate. ---
    let rescanned = scan_uidnumbers(230);
    let allocated = edaptor::config::defaults::next_in_range(&rescanned, min, max)
        .expect("window not exhausted");

    // `next_in_range` is max-seen-in-window + 1 (NOT lowest-free): the gap at
    // base+1 is deliberately NOT reused. Max seen in [base,max] is base+2.
    assert_eq!(
        allocated,
        base + 3,
        "allocation must be max_seen+1 (base+2 -> base+3); gap at base+1 is not reused"
    );
    assert_ne!(
        allocated,
        base + 1,
        "the free gap at base+1 must not be reused by next_in_range"
    );

    // --- Multi-OC create using the ALLOCATED number as the autonumber-supplied
    //     MUST attribute. ---
    match add_posix_user(240, &main_dn, "edaptor-it-autonum", allocated) {
        Response::WriteOk { .. } => {}
        other => panic!("multi-OC create ADD failed: {}", describe(&Some(other))),
    }

    // --- Cleanup all three (idempotent), assert each delete WriteOk. ---
    for (i, dn) in [&throwaway_a, &throwaway_b, &main_dn].iter().enumerate() {
        let id = 290 + i as u64;
        worker
            .submit(Request::Delete {
                id,
                dn: (*dn).clone(),
            })
            .expect("submit cleanup delete");
        match poll_for_id(&worker, id, Duration::from_secs(10)) {
            Some(Response::WriteOk { .. }) => {}
            other => panic!("cleanup DELETE of {dn} failed: {}", describe(&other)),
        }
    }
}

/// Task 5.3 — value-lookup picker yields the right scalar (gated).
///
/// Proves `edaptor::ui::picker::pick_value` (the lookup picker's commit path)
/// reads the correct scalar attribute (`gidNumber`) from a REAL directory entry,
/// and returns `None` for an absent attribute. Requires the posix schema
/// (posixGroup); SKIPs with a clear message if it is absent.
#[test]
fn lookup_pick_value_yields_gidnumber() {
    let uri = match std::env::var("EDAPTOR_TEST_LDAP_URI") {
        Ok(uri) => uri,
        Err(_) => {
            eprintln!("SKIP lookup_pick_value_yields_gidnumber: set EDAPTOR_TEST_LDAP_URI to run");
            return;
        }
    };
    let (config, password) = admin_config(uri.clone());
    let worker = WorkerHandle::spawn(config, password).expect("spawn admin worker");

    let container = "ou=users,dc=example,dc=org";
    let dn = format!("cn=edaptor-it-grp,{container}");

    // Idempotent cleanup from any prior aborted run. id range 300-399.
    let _ = worker.submit(Request::Delete {
        id: 300,
        dn: dn.clone(),
    });
    let _ = poll_for_id(&worker, 300, Duration::from_secs(5));

    // --- ADD a posixGroup with a known gidNumber. The ADD doubles as the
    //     schema probe: SKIP if posixGroup is unknown. ---
    let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    attrs.insert(
        "objectClass".to_string(),
        vec!["top".to_string(), "posixGroup".to_string()],
    );
    attrs.insert("cn".to_string(), vec!["edaptor-it-grp".to_string()]);
    attrs.insert("gidNumber".to_string(), vec!["54321".to_string()]);
    worker
        .submit(Request::Add {
            id: 310,
            dn: dn.clone(),
            attrs,
        })
        .expect("submit posixGroup add");
    match poll_for_id(&worker, 310, Duration::from_secs(10)) {
        Some(Response::WriteOk { .. }) => {}
        Some(Response::WriteError { msg, .. }) if is_schema_missing(&msg) => {
            eprintln!(
                "SKIP lookup_pick_value_yields_gidnumber: posix schema absent on image ({msg})"
            );
            return;
        }
        other => panic!("posixGroup ADD failed: {}", describe(&other)),
    }

    // --- Base-scope read of the group requesting gidNumber + cn. ---
    worker
        .submit(Request::Search {
            id: 320,
            base: dn.clone(),
            scope: SearchScope::Base,
            filter: "(objectClass=*)".to_string(),
            attrs: vec!["gidNumber".to_string(), "cn".to_string()],
            size_limit: None,
        })
        .expect("submit group read");
    let entry = match poll_for_id(&worker, 320, Duration::from_secs(10)) {
        Some(Response::Entries { entries, .. }) => {
            assert_eq!(entries.len(), 1, "expected exactly one group entry");
            entries.into_iter().next().unwrap()
        }
        other => panic!("group read failed: {}", describe(&other)),
    };

    // --- The picker commit path reads the right scalar from the real entry. ---
    assert_eq!(
        edaptor::ui::picker::pick_value(&entry.attrs, "gidNumber"),
        Some("54321".to_string()),
        "pick_value must read the gidNumber scalar from the real entry"
    );
    assert_eq!(
        edaptor::ui::picker::pick_value(&entry.attrs, "noSuchAttr"),
        None,
        "pick_value must return None for an absent attribute"
    );

    // --- Cleanup ---
    worker
        .submit(Request::Delete {
            id: 390,
            dn: dn.clone(),
        })
        .expect("submit cleanup delete");
    match poll_for_id(&worker, 390, Duration::from_secs(10)) {
        Some(Response::WriteOk { .. }) => {}
        other => panic!("cleanup DELETE failed: {}", describe(&other)),
    }
}
