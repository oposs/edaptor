//! Gated live integration test for membership editing.
//!
//! Enable by setting EDAPTOR_TEST_LDAP_URI (e.g. ldap://localhost:1389).
//! Start a server with: scripts/test-ldap.sh start
//! When the env var is unset each test prints SKIP and returns early (no silent skip).
//!
//! Exercises the forward (group.member) edit path end-to-end through the worker:
//!   seed group + two users -> Replace member = [userA, userB] -> Base-search the
//!   group and assert both DNs are present -> clean up.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use edaptor::config::{AuthConfig, AuthMethod, Config, PasswordSource, ServerConfig, TlsConfig};
use edaptor::form::changeset::ModOp;
use edaptor::ldap::worker::{Request, Response, SearchScope, WorkerHandle};

// ---------------------------------------------------------------------------
// Helpers (mirrored from tests/live_write.rs)
// ---------------------------------------------------------------------------

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

/// Base-read a DN, returning the entry's string attrs (None if not found).
fn read_entry(
    worker: &WorkerHandle,
    dn: &str,
    id: u64,
    attrs: &[&str],
) -> Option<BTreeMap<String, Vec<String>>> {
    worker
        .submit(Request::Search {
            id,
            base: dn.to_string(),
            scope: SearchScope::Base,
            filter: "(objectClass=*)".to_string(),
            attrs: attrs.iter().map(|s| s.to_string()).collect(),
            size_limit: None,
        })
        .expect("submit base search");
    match poll_for_id(worker, id, Duration::from_secs(10)) {
        Some(Response::Entries { entries, .. }) => entries.into_iter().next().map(|e| e.attrs),
        _ => None,
    }
}

/// Delete a DN, ignoring errors (cleanup helper).
fn cleanup(worker: &WorkerHandle, id: u64, dn: &str) {
    let _ = worker.submit(Request::Delete {
        id,
        dn: dn.to_string(),
        assert_csn: None,
    });
    let _ = poll_for_id(worker, id, Duration::from_secs(5));
}

// ---------------------------------------------------------------------------
// DN constants for the seeded entries
// ---------------------------------------------------------------------------

const CONTAINER: &str = "ou=users,dc=example,dc=org";
const GROUP_DN: &str = "cn=lm-test-group,ou=users,dc=example,dc=org";
const USER_A_DN: &str = "uid=lm-user-a,ou=users,dc=example,dc=org";
const USER_B_DN: &str = "uid=lm-user-b,ou=users,dc=example,dc=org";

// DNs for the reverse fan-out test (distinct to avoid parallel-run races).
const REV_GROUP_DN: &str = "cn=lm-rev-group,ou=users,dc=example,dc=org";
const REV_USER_A_DN: &str = "uid=lm-rev-user-a,ou=users,dc=example,dc=org";
const REV_OTHER_DN: &str = "uid=lm-rev-other,ou=users,dc=example,dc=org";

// DNs for the last-member rejection test.
const LAST_GROUP_DN: &str = "cn=lm-last-group,ou=users,dc=example,dc=org";
const LAST_USER_A_DN: &str = "uid=lm-last-user-a,ou=users,dc=example,dc=org";

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Seed a minimal group + two users, set member=[userA, userB] via a Replace
/// MODIFY, then Base-search the group and assert both DNs are present.
#[test]
fn forward_member_edit_round_trips() {
    let uri = match std::env::var("EDAPTOR_TEST_LDAP_URI") {
        Ok(uri) => uri,
        Err(_) => {
            eprintln!("SKIP forward_member_edit_round_trips: set EDAPTOR_TEST_LDAP_URI to run");
            return;
        }
    };

    let (config, password) = test_config(uri);
    let worker = WorkerHandle::spawn(config, password).expect("spawn worker");

    // --- Idempotent cleanup from any prior aborted run ---
    cleanup(&worker, 1, GROUP_DN);
    cleanup(&worker, 2, USER_A_DN);
    cleanup(&worker, 3, USER_B_DN);

    // --- Seed user A ---
    {
        let mut attrs = BTreeMap::new();
        attrs.insert(
            "objectClass".to_string(),
            vec!["top".to_string(), "inetOrgPerson".to_string()],
        );
        attrs.insert("uid".to_string(), vec!["lm-user-a".to_string()]);
        attrs.insert("cn".to_string(), vec!["LM User A".to_string()]);
        attrs.insert("sn".to_string(), vec!["A".to_string()]);
        worker
            .submit(Request::Add {
                id: 10,
                dn: USER_A_DN.to_string(),
                attrs,
            })
            .expect("submit add user A");
        match poll_for_id(&worker, 10, Duration::from_secs(10)) {
            Some(Response::WriteOk { .. }) => {}
            other => panic!("ADD user A failed: {}", describe(&other)),
        }
    }

    // --- Seed user B ---
    {
        let mut attrs = BTreeMap::new();
        attrs.insert(
            "objectClass".to_string(),
            vec!["top".to_string(), "inetOrgPerson".to_string()],
        );
        attrs.insert("uid".to_string(), vec!["lm-user-b".to_string()]);
        attrs.insert("cn".to_string(), vec!["LM User B".to_string()]);
        attrs.insert("sn".to_string(), vec!["B".to_string()]);
        worker
            .submit(Request::Add {
                id: 20,
                dn: USER_B_DN.to_string(),
                attrs,
            })
            .expect("submit add user B");
        match poll_for_id(&worker, 20, Duration::from_secs(10)) {
            Some(Response::WriteOk { .. }) => {}
            other => panic!("ADD user B failed: {}", describe(&other)),
        }
    }

    // --- Seed group (groupOfNames requires at least one member) ---
    {
        let mut attrs = BTreeMap::new();
        attrs.insert(
            "objectClass".to_string(),
            vec!["top".to_string(), "groupOfNames".to_string()],
        );
        attrs.insert("cn".to_string(), vec!["lm-test-group".to_string()]);
        // Seed with only userA initially; the test will Replace to [userA, userB].
        attrs.insert("member".to_string(), vec![USER_A_DN.to_string()]);
        worker
            .submit(Request::Add {
                id: 30,
                dn: GROUP_DN.to_string(),
                attrs,
            })
            .expect("submit add group");
        match poll_for_id(&worker, 30, Duration::from_secs(10)) {
            Some(Response::WriteOk { .. }) => {}
            other => {
                // Clean up users before panicking.
                cleanup(&worker, 31, USER_A_DN);
                cleanup(&worker, 32, USER_B_DN);
                panic!("ADD group failed: {}", describe(&other));
            }
        }
    }

    // --- MODIFY: Replace member = [userA, userB] ---
    worker
        .submit(Request::Modify {
            id: 40,
            dn: GROUP_DN.to_string(),
            changes: vec![ModOp::Replace {
                attr: "member".to_string(),
                values: vec![USER_A_DN.to_string(), USER_B_DN.to_string()],
            }],
            assert_csn: None,
        })
        .expect("submit modify group member");
    match poll_for_id(&worker, 40, Duration::from_secs(10)) {
        Some(Response::WriteOk { .. }) => {}
        other => {
            cleanup(&worker, 41, GROUP_DN);
            cleanup(&worker, 42, USER_A_DN);
            cleanup(&worker, 43, USER_B_DN);
            panic!("MODIFY group.member failed: {}", describe(&other));
        }
    }

    // --- Verify: Base-search the group and assert both DNs appear in member ---
    let group_attrs =
        read_entry(&worker, GROUP_DN, 50, &["member"]).expect("group should exist after modify");
    let members = group_attrs.get("member").cloned().unwrap_or_default();

    // Normalise to lower-case for comparison (some servers canonicalise DNs).
    let members_lc: Vec<String> = members.iter().map(|m| m.to_lowercase()).collect();
    assert!(
        members_lc.contains(&USER_A_DN.to_lowercase()),
        "member list should contain userA ({USER_A_DN}), got: {members:?}"
    );
    assert!(
        members_lc.contains(&USER_B_DN.to_lowercase()),
        "member list should contain userB ({USER_B_DN}), got: {members:?}"
    );
    assert_eq!(
        members.len(),
        2,
        "group should have exactly 2 members after Replace, got: {members:?}"
    );

    // --- Teardown ---
    cleanup(&worker, 60, GROUP_DN);
    cleanup(&worker, 61, USER_A_DN);
    cleanup(&worker, 62, USER_B_DN);

    // Verify cleanup
    assert!(
        read_entry(&worker, GROUP_DN, 70, &["*"]).is_none(),
        "group should be gone after cleanup"
    );
    _ = CONTAINER; // used for documentation; the OU itself is not created/deleted
}

/// Seed a group that already has one other member, then apply the reverse fan-out
/// MODIFY (Add member=userA), and assert userA now appears in the group's member list.
/// This is the same MODIFY the back-ref fan-out produces in the combined save.
#[test]
fn reverse_memberof_edit_writes_group_member() {
    let uri = match std::env::var("EDAPTOR_TEST_LDAP_URI") {
        Ok(uri) => uri,
        Err(_) => {
            eprintln!(
                "SKIP reverse_memberof_edit_writes_group_member: set EDAPTOR_TEST_LDAP_URI to run"
            );
            return;
        }
    };

    let (config, password) = test_config(uri);
    let worker = WorkerHandle::spawn(config, password).expect("spawn worker");

    // Idempotent cleanup from any prior aborted run.
    cleanup(&worker, 1, REV_GROUP_DN);
    cleanup(&worker, 2, REV_USER_A_DN);
    cleanup(&worker, 3, REV_OTHER_DN);

    // Seed "other" user (the placeholder member so the group is never empty).
    {
        let mut attrs = BTreeMap::new();
        attrs.insert(
            "objectClass".to_string(),
            vec!["top".to_string(), "inetOrgPerson".to_string()],
        );
        attrs.insert("uid".to_string(), vec!["lm-rev-other".to_string()]);
        attrs.insert("cn".to_string(), vec!["LM Rev Other".to_string()]);
        attrs.insert("sn".to_string(), vec!["Other".to_string()]);
        worker
            .submit(Request::Add {
                id: 10,
                dn: REV_OTHER_DN.to_string(),
                attrs,
            })
            .expect("submit add other user");
        match poll_for_id(&worker, 10, Duration::from_secs(10)) {
            Some(Response::WriteOk { .. }) => {}
            other => panic!("ADD other user failed: {}", describe(&other)),
        }
    }

    // Seed userA.
    {
        let mut attrs = BTreeMap::new();
        attrs.insert(
            "objectClass".to_string(),
            vec!["top".to_string(), "inetOrgPerson".to_string()],
        );
        attrs.insert("uid".to_string(), vec!["lm-rev-user-a".to_string()]);
        attrs.insert("cn".to_string(), vec!["LM Rev User A".to_string()]);
        attrs.insert("sn".to_string(), vec!["RevA".to_string()]);
        worker
            .submit(Request::Add {
                id: 20,
                dn: REV_USER_A_DN.to_string(),
                attrs,
            })
            .expect("submit add rev user A");
        match poll_for_id(&worker, 20, Duration::from_secs(10)) {
            Some(Response::WriteOk { .. }) => {}
            other => {
                cleanup(&worker, 21, REV_OTHER_DN);
                panic!("ADD rev user A failed: {}", describe(&other));
            }
        }
    }

    // Seed group with only REV_OTHER_DN as member (so it is never empty).
    {
        let mut attrs = BTreeMap::new();
        attrs.insert(
            "objectClass".to_string(),
            vec!["top".to_string(), "groupOfNames".to_string()],
        );
        attrs.insert("cn".to_string(), vec!["lm-rev-group".to_string()]);
        attrs.insert("member".to_string(), vec![REV_OTHER_DN.to_string()]);
        worker
            .submit(Request::Add {
                id: 30,
                dn: REV_GROUP_DN.to_string(),
                attrs,
            })
            .expect("submit add rev group");
        match poll_for_id(&worker, 30, Duration::from_secs(10)) {
            Some(Response::WriteOk { .. }) => {}
            other => {
                cleanup(&worker, 31, REV_USER_A_DN);
                cleanup(&worker, 32, REV_OTHER_DN);
                panic!("ADD rev group failed: {}", describe(&other));
            }
        }
    }

    // Apply the reverse fan-out: Add member=userA to the group.
    // This is the same MODIFY the back-ref fan-out emits.
    worker
        .submit(Request::Modify {
            id: 40,
            dn: REV_GROUP_DN.to_string(),
            changes: vec![edaptor::form::changeset::ModOp::Add {
                attr: "member".to_string(),
                values: vec![REV_USER_A_DN.to_string()],
            }],
            assert_csn: None,
        })
        .expect("submit Add member fan-out modify");
    match poll_for_id(&worker, 40, Duration::from_secs(10)) {
        Some(Response::WriteOk { .. }) => {}
        other => {
            cleanup(&worker, 41, REV_GROUP_DN);
            cleanup(&worker, 42, REV_USER_A_DN);
            cleanup(&worker, 43, REV_OTHER_DN);
            panic!("fan-out Add member failed: {}", describe(&other));
        }
    }

    // Verify: userA is now in the group's member list.
    let group_attrs = read_entry(&worker, REV_GROUP_DN, 50, &["member"])
        .expect("rev group should exist after modify");
    let members = group_attrs.get("member").cloned().unwrap_or_default();
    let members_lc: Vec<String> = members.iter().map(|m| m.to_lowercase()).collect();
    assert!(
        members_lc.contains(&REV_USER_A_DN.to_lowercase()),
        "group member list should contain userA after fan-out Add, got: {members:?}"
    );

    // Teardown.
    cleanup(&worker, 60, REV_GROUP_DN);
    cleanup(&worker, 61, REV_USER_A_DN);
    cleanup(&worker, 62, REV_OTHER_DN);
}

/// Seed a group whose ONLY member is userA, then attempt to delete that member.
/// The server enforces groupOfNames ≥1 member via objectClassViolation and should
/// return a WriteError — confirming the server-side guard independent of the
/// client pre-check.
#[test]
fn removing_last_member_is_rejected_by_server() {
    let uri = match std::env::var("EDAPTOR_TEST_LDAP_URI") {
        Ok(uri) => uri,
        Err(_) => {
            eprintln!(
                "SKIP removing_last_member_is_rejected_by_server: set EDAPTOR_TEST_LDAP_URI to run"
            );
            return;
        }
    };

    let (config, password) = test_config(uri);
    let worker = WorkerHandle::spawn(config, password).expect("spawn worker");

    // Idempotent cleanup from any prior aborted run.
    cleanup(&worker, 1, LAST_GROUP_DN);
    cleanup(&worker, 2, LAST_USER_A_DN);

    // Seed userA.
    {
        let mut attrs = BTreeMap::new();
        attrs.insert(
            "objectClass".to_string(),
            vec!["top".to_string(), "inetOrgPerson".to_string()],
        );
        attrs.insert("uid".to_string(), vec!["lm-last-user-a".to_string()]);
        attrs.insert("cn".to_string(), vec!["LM Last User A".to_string()]);
        attrs.insert("sn".to_string(), vec!["LastA".to_string()]);
        worker
            .submit(Request::Add {
                id: 10,
                dn: LAST_USER_A_DN.to_string(),
                attrs,
            })
            .expect("submit add last user A");
        match poll_for_id(&worker, 10, Duration::from_secs(10)) {
            Some(Response::WriteOk { .. }) => {}
            other => panic!("ADD last user A failed: {}", describe(&other)),
        }
    }

    // Seed group with exactly one member: userA.
    {
        let mut attrs = BTreeMap::new();
        attrs.insert(
            "objectClass".to_string(),
            vec!["top".to_string(), "groupOfNames".to_string()],
        );
        attrs.insert("cn".to_string(), vec!["lm-last-group".to_string()]);
        attrs.insert("member".to_string(), vec![LAST_USER_A_DN.to_string()]);
        worker
            .submit(Request::Add {
                id: 20,
                dn: LAST_GROUP_DN.to_string(),
                attrs,
            })
            .expect("submit add last group");
        match poll_for_id(&worker, 20, Duration::from_secs(10)) {
            Some(Response::WriteOk { .. }) => {}
            other => {
                cleanup(&worker, 21, LAST_USER_A_DN);
                panic!("ADD last group failed: {}", describe(&other));
            }
        }
    }

    // Attempt to delete the sole member — server must reject this.
    worker
        .submit(Request::Modify {
            id: 30,
            dn: LAST_GROUP_DN.to_string(),
            changes: vec![edaptor::form::changeset::ModOp::Delete {
                attr: "member".to_string(),
                values: vec![LAST_USER_A_DN.to_string()],
            }],
            assert_csn: None,
        })
        .expect("submit Delete last member modify");
    let resp = poll_for_id(&worker, 30, Duration::from_secs(10));
    assert!(
        matches!(resp, Some(Response::WriteError { .. })),
        "server should reject deleting the last member of a groupOfNames, got: {}",
        describe(&resp)
    );

    // Teardown (group still has userA; delete group first, then user).
    cleanup(&worker, 40, LAST_GROUP_DN);
    cleanup(&worker, 41, LAST_USER_A_DN);
}

/// Seed 21 matching users under ou=users, then submit a size-capped search with
/// `size_limit: Some(20)`. Before the fix, OpenLDAP's rc=4 (sizeLimitExceeded)
/// caused `run_search` to error and emit `Response::SearchError`; after the fix
/// it returns `Response::Entries` with exactly 20 entries (the partial set).
///
/// This test directly validates Fix 1 described in the bug report.
#[test]
fn size_capped_search_returns_partial_entries_not_error() {
    let uri = match std::env::var("EDAPTOR_TEST_LDAP_URI") {
        Ok(uri) => uri,
        Err(_) => {
            eprintln!(
                "SKIP size_capped_search_returns_partial_entries_not_error: set EDAPTOR_TEST_LDAP_URI to run"
            );
            return;
        }
    };

    let (config, password) = test_config(uri);
    let worker = WorkerHandle::spawn(config, password).expect("spawn worker");

    // DN constants for the 21 cap-test users.
    const CAP_USER_BASE: &str = "ou=users,dc=example,dc=org";
    const CAP_USER_COUNT: u32 = 21;
    let cap_dn = |n: u32| format!("uid=lm-cap-user{n:02},{CAP_USER_BASE}");

    // Idempotent cleanup from any prior aborted run.
    for n in 0..CAP_USER_COUNT {
        cleanup(&worker, 200 + n as u64, &cap_dn(n));
    }

    // Seed 21 users whose uid matches the search term "lm-cap-user".
    for n in 0..CAP_USER_COUNT {
        let mut attrs = BTreeMap::new();
        attrs.insert(
            "objectClass".to_string(),
            vec!["top".to_string(), "inetOrgPerson".to_string()],
        );
        attrs.insert("uid".to_string(), vec![format!("lm-cap-user{n:02}")]);
        attrs.insert("cn".to_string(), vec![format!("LM Cap User {n:02}")]);
        attrs.insert("sn".to_string(), vec!["Cap".to_string()]);
        let id = 300 + n as u64;
        worker
            .submit(Request::Add {
                id,
                dn: cap_dn(n),
                attrs,
            })
            .expect("submit add cap user");
        match poll_for_id(&worker, id, Duration::from_secs(10)) {
            Some(Response::WriteOk { .. }) => {}
            other => {
                // Cleanup whatever was created, then fail.
                for m in 0..=n {
                    cleanup(&worker, 400 + m as u64, &cap_dn(m));
                }
                panic!("ADD cap user {n} failed: {}", describe(&other));
            }
        }
    }

    // Submit a subtree search capped at 20 matching all 21 seeded users.
    // The filter targets the unique uid prefix so only our entries match.
    let search_id = 500u64;
    worker
        .submit(Request::Search {
            id: search_id,
            base: CAP_USER_BASE.to_string(),
            scope: SearchScope::Subtree,
            filter: "(uid=lm-cap-user*)".to_string(),
            attrs: vec!["uid".to_string()],
            size_limit: Some(20),
        })
        .expect("submit capped search");

    let resp = poll_for_id(&worker, search_id, Duration::from_secs(15));

    // Teardown before asserting so the LDAP server stays clean on failure.
    for n in 0..CAP_USER_COUNT {
        cleanup(&worker, 600 + n as u64, &cap_dn(n));
    }

    // After Fix 1: server returns rc=4 (sizeLimitExceeded) + 20 partial entries.
    // run_search must return those 20 entries instead of converting rc=4 to Err.
    match resp {
        Some(Response::Entries { ref entries, .. }) => {
            assert_eq!(
                entries.len(),
                20,
                "expected exactly 20 capped entries, got {}",
                entries.len()
            );
        }
        Some(Response::SearchError { ref msg, .. }) => {
            panic!(
                "Fix 1 regression: got SearchError instead of partial Entries. \
                 run_search is still converting rc=4 to an error. msg={msg}"
            );
        }
        other => panic!("unexpected response: {}", describe(&other)),
    }
}
