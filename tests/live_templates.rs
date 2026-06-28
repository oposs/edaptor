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

use edaptor::config::relation::{PickerBinding, StoreKey};
use edaptor::config::widget::{resolve_widgets, WidgetKind};
use edaptor::config::{
    AuthConfig, AuthMethod, Config, EntryProfile, PasswordSource, ServerConfig, TlsConfig,
};
use edaptor::ldap::worker::{Request, Response, SearchScope, WorkerHandle};
use edaptor::samba::password::password_add_attrs;
use edaptor::workflows::pick_state::{build_member_filter, pick_value};

/// Resolve the `[profile.widget.<attr>]` picker/membership binding for `attr`
/// from the given profiles. Pickers are driven through the widget palette
/// (`WidgetKind::Picker`) and resolved via `resolve_widgets`.
fn picker_binding_for(profiles: &[EntryProfile], attr: &str) -> PickerBinding {
    let widgets = resolve_widgets(profiles).expect("demo-config widgets resolve");
    widgets
        .into_iter()
        .find_map(|w| match w.kind {
            WidgetKind::Picker(b) if b.attr.eq_ignore_ascii_case(attr) => Some(b),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{attr} picker must be resolved from demo config"))
}

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
/// Proves `edaptor::workflows::pick_state::pick_value` (the lookup picker's commit path)
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
        pick_value(&entry.attrs, "gidNumber"),
        Some("54321".to_string()),
        "pick_value must read the gidNumber scalar from the real entry"
    );
    assert_eq!(
        pick_value(&entry.attrs, "noSuchAttr"),
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

// ---------------------------------------------------------------------------
// Task 9 — unified picker store-value shapes (live, gated)
// ---------------------------------------------------------------------------

/// Delete a DN, ignoring errors (idempotent cleanup helper for the picker tests).
fn cleanup_entry(worker: &WorkerHandle, id: u64, dn: &str) {
    let _ = worker.submit(Request::Delete {
        id,
        dn: dn.to_string(),
    });
    let _ = poll_for_id(worker, id, Duration::from_secs(5));
}

/// Test 1 — member binding: candidate search over `ou=people` using the resolved
/// `member` picker scope yields real user DNs as store values.
///
/// The `member` binding (group profile, candidate=user, store=dn) builds an
/// objectClass-AND filter for the four inetOrgPerson/posixAccount/… classes.
/// An empty search term returns objectClass-only filter, so it matches every
/// seeded user. Asserts: hits are non-empty; each hit's DN is its own store value;
/// every DN ends with `ou=people,dc=example,dc=org`.
#[test]
fn picker_member_candidate_search_yields_user_dns() {
    let uri = match std::env::var("EDAPTOR_TEST_LDAP_URI") {
        Ok(u) => u,
        Err(_) => {
            eprintln!(
                "SKIP picker_member_candidate_search_yields_user_dns: set EDAPTOR_TEST_LDAP_URI to run"
            );
            return;
        }
    };

    let (config, password) = admin_config(uri);
    let worker = WorkerHandle::spawn(config, password).expect("spawn admin worker");

    // Load the demo-config pickers so we work from the real binding definitions.
    let cfg: edaptor::config::Config = toml::from_str(include_str!("../examples/demo-config.toml"))
        .expect("demo-config.toml parses");

    // The `member` picker: owner = group profile, candidate = user.
    let binding = picker_binding_for(&cfg.profiles, "member");
    let binding = &binding;

    // Empty search term → objectClass-only filter (no term branch).
    let filter = build_member_filter(
        &binding.scope.object_classes,
        &binding.scope.search_attrs,
        "",
    );

    worker
        .submit(Request::Search {
            id: 410,
            base: binding.scope.base.clone(),
            scope: SearchScope::Subtree,
            filter,
            attrs: vec!["uid".to_string()],
            size_limit: Some(10),
        })
        .expect("submit member candidate search");

    let entries = match poll_for_id(&worker, 410, Duration::from_secs(10)) {
        Some(Response::Entries { entries, .. }) => entries,
        other => panic!("member candidate search failed: {}", describe(&other)),
    };

    assert!(
        !entries.is_empty(),
        "member picker search should return at least one user candidate from ou=people"
    );

    // For store = Dn, the store value is the entry's own DN (assert once, not per-entry).
    assert_eq!(
        binding.store,
        StoreKey::Dn,
        "member binding must have StoreKey::Dn"
    );

    for entry in &entries {
        let dn_lc = entry.dn.to_lowercase();
        assert!(
            dn_lc.contains("ou=people,dc=example,dc=org"),
            "each candidate DN should be under ou=people, got: {}",
            entry.dn
        );
        // The store value for a DN picker is the DN itself.
        assert!(
            dn_lc.starts_with("uid="),
            "seeded user DNs should start with uid=, got: {}",
            entry.dn
        );
    }
}

/// Test 2 — gidNumber binding: searching posixGroups and extracting the scalar
/// `gidNumber` via `pick_value` gives a numeric value (not a DN).
///
/// The `gidNumber` binding (user profile, candidate=posixgroup, store=Attr("gidNumber"))
/// searches `ou=groups` for posixGroup entries and commits their `gidNumber`
/// scalar — not their DN. Asserts: hits are non-empty; `pick_value(&attrs,
/// "gidNumber")` yields Some(numeric string) that parses as an integer.
#[test]
fn picker_gidnumber_scalar_store_resolves_group_gidnumber() {
    let uri = match std::env::var("EDAPTOR_TEST_LDAP_URI") {
        Ok(u) => u,
        Err(_) => {
            eprintln!(
                "SKIP picker_gidnumber_scalar_store_resolves_group_gidnumber: set EDAPTOR_TEST_LDAP_URI to run"
            );
            return;
        }
    };

    let (config, password) = admin_config(uri);
    let worker = WorkerHandle::spawn(config, password).expect("spawn admin worker");

    let cfg: edaptor::config::Config = toml::from_str(include_str!("../examples/demo-config.toml"))
        .expect("demo-config.toml parses");

    let binding = picker_binding_for(&cfg.profiles, "gidNumber");
    let binding = &binding;

    // Confirm the store key is a scalar attribute, not a DN.
    assert_eq!(
        binding.store,
        StoreKey::Attr("gidNumber".to_string()),
        "gidNumber binding must store the gidNumber scalar attribute"
    );

    // Determine the store attribute name for the search attrs list.
    let store_attr = match &binding.store {
        StoreKey::Attr(a) => a.clone(),
        StoreKey::Dn => panic!("expected scalar store"),
    };

    let filter = build_member_filter(
        &binding.scope.object_classes,
        &binding.scope.search_attrs,
        "",
    );

    worker
        .submit(Request::Search {
            id: 420,
            base: binding.scope.base.clone(),
            scope: SearchScope::Subtree,
            filter,
            attrs: vec![store_attr.clone(), "cn".to_string()],
            size_limit: Some(10),
        })
        .expect("submit gidNumber candidate search");

    let entries = match poll_for_id(&worker, 420, Duration::from_secs(10)) {
        Some(Response::Entries { entries, .. }) => entries,
        other => panic!("gidNumber candidate search failed: {}", describe(&other)),
    };

    assert!(
        !entries.is_empty(),
        "gidNumber picker search should return at least one posixGroup candidate"
    );

    for entry in &entries {
        let scalar = pick_value(&entry.attrs, &store_attr)
            .unwrap_or_else(|| panic!("gidNumber must be present on entry {}", entry.dn));

        // The store value must be parseable as an integer (it's a UNIX gid).
        scalar.parse::<i64>().unwrap_or_else(|_| {
            panic!(
                "gidNumber scalar '{scalar}' must be numeric for {}",
                entry.dn
            )
        });

        // The store value must NOT look like a DN (no commas / equals typical in DNs).
        assert!(
            !scalar.contains(',') && !scalar.contains("ou="),
            "gidNumber scalar store must be a plain integer, not a DN: '{scalar}'"
        );
    }
}

/// Test 3 — memberUid multi-scalar round-trip: derive two uid values the way the
/// picker derives them (via `pick_value` on real directory entries), create a
/// throwaway posixGroup carrying those values in `memberUid`, read it back, and
/// assert exactly those two scalars are present (order-insensitive). Then delete
/// the entry.
///
/// This exercises the FULL store-value extraction path: the two uids come from
/// the same `pick_value(&entry.attrs, "uid")` call that the production
/// `Response::Entries` intercept uses for a scalar-store binding. Any regression
/// in that extraction would prevent us from deriving the uids here.
#[test]
fn picker_memberuid_multi_scalar_round_trips_uids() {
    let uri = match std::env::var("EDAPTOR_TEST_LDAP_URI") {
        Ok(u) => u,
        Err(_) => {
            eprintln!(
                "SKIP picker_memberuid_multi_scalar_round_trips_uids: set EDAPTOR_TEST_LDAP_URI to run"
            );
            return;
        }
    };

    let (config, password) = admin_config(uri);
    let worker = WorkerHandle::spawn(config, password).expect("spawn admin worker");

    let group_dn = "cn=ztest-picker-memberuid,ou=groups,dc=example,dc=org";

    // Idempotent cleanup from any prior aborted run.
    cleanup_entry(&worker, 430, group_dn);

    // --- Resolve the memberUid binding from demo-config. ---
    let cfg: edaptor::config::Config = toml::from_str(include_str!("../examples/demo-config.toml"))
        .expect("demo-config.toml parses");

    let binding = picker_binding_for(&cfg.profiles, "memberUid");
    let binding = &binding;

    // Confirm the store key is a scalar uid attribute (not a DN).
    assert_eq!(
        binding.store,
        StoreKey::Attr("uid".to_string()),
        "memberUid binding must store the uid scalar attribute"
    );

    // --- Search candidate users using the picker's own scope, requesting `uid`. ---
    let filter = build_member_filter(
        &binding.scope.object_classes,
        &binding.scope.search_attrs,
        "",
    );

    worker
        .submit(Request::Search {
            id: 435,
            base: binding.scope.base.clone(),
            scope: SearchScope::Subtree,
            filter,
            attrs: vec!["uid".to_string()],
            size_limit: Some(5),
        })
        .expect("submit memberUid candidate search");

    let candidate_entries = match poll_for_id(&worker, 435, Duration::from_secs(10)) {
        Some(Response::Entries { entries, .. }) => entries,
        other => panic!("memberUid candidate search failed: {}", describe(&other)),
    };

    // --- Extract store values exactly as the production picker does. ---
    // Collect two distinct non-empty uid scalars via pick_value.
    let derived_uids: Vec<String> = {
        let mut seen = std::collections::BTreeSet::new();
        let mut out = Vec::new();
        for entry in &candidate_entries {
            if let Some(uid) = pick_value(&entry.attrs, "uid") {
                if seen.insert(uid.clone()) {
                    // Confirm it is a plain uid, not a DN.
                    assert!(
                        !uid.contains(',') && !uid.contains("ou="),
                        "pick_value must return a plain uid scalar, not a DN: '{uid}'"
                    );
                    out.push(uid);
                    if out.len() == 2 {
                        break;
                    }
                }
            }
        }
        out
    };
    assert_eq!(
        derived_uids.len(),
        2,
        "need at least 2 distinct seeded users with a uid attribute under {}",
        binding.scope.base
    );
    let uid_a = &derived_uids[0];
    let uid_b = &derived_uids[1];

    // --- ADD a throwaway posixGroup carrying the two derived uid scalars. ---
    let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    attrs.insert(
        "objectClass".to_string(),
        vec!["top".to_string(), "posixGroup".to_string()],
    );
    attrs.insert("cn".to_string(), vec!["ztest-picker-memberuid".to_string()]);
    // Use a high gidNumber that won't clash with seeded groups (5000–5004).
    attrs.insert("gidNumber".to_string(), vec!["59999".to_string()]);
    attrs.insert("memberUid".to_string(), vec![uid_a.clone(), uid_b.clone()]);

    worker
        .submit(Request::Add {
            id: 440,
            dn: group_dn.to_string(),
            attrs,
        })
        .expect("submit throwaway posixGroup add");

    match poll_for_id(&worker, 440, Duration::from_secs(10)) {
        Some(Response::WriteOk { .. }) => {}
        Some(Response::WriteError { msg, .. }) if is_schema_missing(&msg) => {
            eprintln!(
                "SKIP picker_memberuid_multi_scalar_round_trips_uids: posix schema absent ({msg})"
            );
            return;
        }
        other => panic!("throwaway posixGroup ADD failed: {}", describe(&other)),
    }

    // --- Read back the group. ---
    worker
        .submit(Request::Search {
            id: 450,
            base: group_dn.to_string(),
            scope: SearchScope::Base,
            filter: "(objectClass=*)".to_string(),
            attrs: vec!["memberUid".to_string()],
            size_limit: None,
        })
        .expect("submit throwaway group read");

    let read_back = poll_for_id(&worker, 450, Duration::from_secs(10));

    // Cleanup before asserting so the directory stays clean even on assertion failure.
    cleanup_entry(&worker, 460, group_dn);

    let member_uids = match read_back {
        Some(Response::Entries { entries, .. }) => {
            let entry = entries.into_iter().next().expect("group entry must exist");
            entry.attrs.get("memberUid").cloned().unwrap_or_default()
        }
        other => panic!("throwaway group read failed: {}", describe(&other)),
    };

    // Order-insensitive membership check: exactly the two derived uids must be present.
    assert_eq!(
        member_uids.len(),
        2,
        "memberUid must have exactly 2 scalar values, got: {member_uids:?}"
    );
    assert!(
        member_uids.iter().any(|v| v == uid_a),
        "memberUid must contain derived uid '{uid_a}', got: {member_uids:?}"
    );
    assert!(
        member_uids.iter().any(|v| v == uid_b),
        "memberUid must contain derived uid '{uid_b}', got: {member_uids:?}"
    );

    // Confirm neither value looks like a DN (scalar store, not DN store).
    for uid_val in &member_uids {
        assert!(
            !uid_val.contains(',') && !uid_val.contains("ou="),
            "memberUid value must be a plain uid scalar, not a DN: '{uid_val}'"
        );
    }
}

/// Test 4 — memberOf binding wiring: assert that the resolved `memberOf` picker
/// has `fanout_attr == Some("member")`, `store == StoreKey::Dn`, and searches
/// under `ou=groups` (the group scope). This pins the fan-out wiring from the
/// real demo config without duplicating the write round-trip already covered by
/// `reverse_memberof_edit_writes_group_member` in `live_membership.rs`.
///
/// This test does NO LDAP I/O — it only parses demo-config and asserts the
/// resolved binding — so it runs in every `cargo test` without the live server.
#[test]
fn picker_memberof_binding_resolves_fanout_to_member() {
    let cfg: edaptor::config::Config = toml::from_str(include_str!("../examples/demo-config.toml"))
        .expect("demo-config.toml parses");

    let binding = picker_binding_for(&cfg.profiles, "memberOf");
    let binding = &binding;

    // Fan-out wiring: the synthetic back-ref writes `member` on each picked group.
    assert_eq!(
        binding.fanout_attr.as_deref(),
        Some("member"),
        "memberOf binding must fan out to the `member` attribute on picked groups"
    );

    // Store: picks the group's DN (to be written as `member` on the group entry).
    assert_eq!(
        binding.store,
        StoreKey::Dn,
        "memberOf binding must use StoreKey::Dn (picks group DNs)"
    );

    // Scope: candidate search base must be under ou=groups (the group profile).
    assert!(
        binding.scope.base.to_lowercase().contains("ou=groups"),
        "memberOf candidate scope must search under ou=groups, got: {}",
        binding.scope.base
    );
}
