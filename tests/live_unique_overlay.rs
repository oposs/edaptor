//! Live integration test for directories running `slapo-unique` (the
//! `oposs.openldap` role configures it).
//!
//! Enable by setting EDAPTOR_TEST_LDAP_URI (e.g. ldap://localhost:1389).
//! Start a server with: scripts/test-ldap.sh start
//! When the env var is unset each test prints SKIP and passes (no silent skip).
//!
//! # Why this exists
//!
//! A `unique` overlay configured as `ldap:///?gidNumber?sub` — uniqueness across
//! the WHOLE subtree — makes user-private groups impossible: the user entry and
//! its companion group share a gidNumber by design, so the companion-create
//! transaction is rejected with a constraint violation. Worse, an overlay
//! rejection inside an RFC 5805 transaction is deferred to commit, and slapd's
//! TXN END result carries the code with an EMPTY diagnostic message, so the user
//! sees a bare "Constraint violation" with no clue what is wrong.
//!
//! The fix is in the server config: scope each URI with a filter so gidNumber
//! uniqueness applies among posixGroups only. These tests pin BOTH halves —
//! the companion pair must be allowed, and real duplicates must still be
//! rejected — so nobody "fixes" the first by disabling the second.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use edaptor::config::{AuthConfig, AuthMethod, Config, PasswordSource, ServerConfig, TlsConfig};
use edaptor::ldap::worker::{Request, Response, WorkerHandle};

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
                Response::WriteOk { id, .. } | Response::WriteError { id, .. }
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
        Some(_) => "other".to_string(),
        None => "timeout".to_string(),
    }
}

fn is_constraint_violation(resp: &Option<Response>) -> bool {
    matches!(resp, Some(Response::WriteError { msg, .. }) if msg.starts_with("Constraint violation"))
}

fn posix_account(uid: &str, uid_number: &str, gid_number: &str) -> BTreeMap<String, Vec<String>> {
    let mut m = BTreeMap::new();
    m.insert(
        "objectClass".to_string(),
        vec![
            "top".to_string(),
            "inetOrgPerson".to_string(),
            "posixAccount".to_string(),
        ],
    );
    m.insert("uid".to_string(), vec![uid.to_string()]);
    m.insert("cn".to_string(), vec![uid.to_string()]);
    m.insert("sn".to_string(), vec![uid.to_string()]);
    m.insert("uidNumber".to_string(), vec![uid_number.to_string()]);
    m.insert("gidNumber".to_string(), vec![gid_number.to_string()]);
    m.insert("homeDirectory".to_string(), vec![format!("/home/{uid}")]);
    m
}

fn posix_group(cn: &str, gid_number: &str) -> BTreeMap<String, Vec<String>> {
    let mut m = BTreeMap::new();
    m.insert(
        "objectClass".to_string(),
        vec!["top".to_string(), "posixGroup".to_string()],
    );
    m.insert("cn".to_string(), vec![cn.to_string()]);
    m.insert("gidNumber".to_string(), vec![gid_number.to_string()]);
    m
}

fn user_dn(uid: &str) -> String {
    format!("uid={uid},ou=people,dc=example,dc=org")
}

fn group_dn(cn: &str) -> String {
    format!("cn={cn},ou=groups,dc=example,dc=org")
}

/// Best-effort delete (ignores result) so a prior run's leftovers don't fail us.
fn cleanup(worker: &WorkerHandle, dn: &str, id: u64) {
    let _ = worker.submit(Request::Delete {
        id,
        dn: dn.to_string(),
        assert_csn: None,
    });
    let _ = poll_for_id(worker, id, Duration::from_secs(5));
}

fn add_one(
    worker: &WorkerHandle,
    id: u64,
    dn: &str,
    attrs: BTreeMap<String, Vec<String>>,
) -> Option<Response> {
    worker
        .submit(Request::Add {
            id,
            dn: dn.to_string(),
            attrs,
        })
        .expect("submit Add");
    poll_for_id(worker, id, Duration::from_secs(10))
}

fn uri_or_skip(test: &str) -> Option<String> {
    match std::env::var("EDAPTOR_TEST_LDAP_URI") {
        Ok(u) => Some(u),
        Err(_) => {
            eprintln!("SKIP {test}: set EDAPTOR_TEST_LDAP_URI to run");
            None
        }
    }
}

/// The companion-create shape: a user entry and its private group share a
/// gidNumber, submitted in one transaction. The unique overlay must allow it.
#[test]
fn companion_pair_sharing_gidnumber_is_allowed() {
    let Some(uri) = uri_or_skip("companion_pair_sharing_gidnumber_is_allowed") else {
        return;
    };
    let (config, password) = test_config(uri);
    let worker = WorkerHandle::spawn(config, password).expect("spawn worker");

    let udn = user_dn("edaptor-uniq-pair");
    let gdn = group_dn("edaptor-uniq-pair");
    cleanup(&worker, &udn, 1);
    cleanup(&worker, &gdn, 2);

    worker
        .submit(Request::AddAtomic {
            id: 10,
            entries: vec![
                (gdn.clone(), posix_group("edaptor-uniq-pair", "59401")),
                (
                    udn.clone(),
                    posix_account("edaptor-uniq-pair", "59401", "59401"),
                ),
            ],
        })
        .expect("submit AddAtomic");
    let resp = poll_for_id(&worker, 10, Duration::from_secs(10));
    assert!(
        matches!(resp, Some(Response::WriteOk { .. })),
        "a user and its private group share a gidNumber by design; the unique \
         overlay must be scoped so this is allowed. Got {}. If this is a \
         constraint violation, the server's olcUniqueURI for gidNumber is not \
         filtered to (objectClass=posixGroup).",
        describe(&resp)
    );

    cleanup(&worker, &udn, 20);
    cleanup(&worker, &gdn, 21);
}

/// The other half: two real groups may NOT share a gidNumber. Guards against
/// "fixing" the test above by disabling gidNumber uniqueness altogether.
#[test]
fn two_groups_may_not_share_a_gidnumber() {
    let Some(uri) = uri_or_skip("two_groups_may_not_share_a_gidnumber") else {
        return;
    };
    let (config, password) = test_config(uri);
    let worker = WorkerHandle::spawn(config, password).expect("spawn worker");

    let a = group_dn("edaptor-uniq-ga");
    let b = group_dn("edaptor-uniq-gb");
    cleanup(&worker, &a, 1);
    cleanup(&worker, &b, 2);

    let first = add_one(&worker, 30, &a, posix_group("edaptor-uniq-ga", "59402"));
    assert!(
        matches!(first, Some(Response::WriteOk { .. })),
        "first group must be created; got {}",
        describe(&first)
    );

    let second = add_one(&worker, 31, &b, posix_group("edaptor-uniq-gb", "59402"));
    assert!(
        is_constraint_violation(&second),
        "a second group with the same gidNumber must be rejected by the unique \
         overlay; got {}",
        describe(&second)
    );

    cleanup(&worker, &a, 40);
    cleanup(&worker, &b, 41);
}

/// An overlay rejection inside an RFC 5805 transaction is deferred to commit,
/// and slapd's TXN END result carries the code with an EMPTY diagnostic
/// message. edaptor must replay the adds outside the transaction to recover the
/// reason, and put it in front of the user.
///
/// Two entries sharing a `mail` value trip the overlay's `ldap:///?mail?sub`
/// rule — a genuine error, so this needs no misconfigured server.
#[test]
fn transaction_failure_recovers_the_reason_the_server_withheld() {
    let Some(uri) = uri_or_skip("transaction_failure_recovers_the_reason_the_server_withheld")
    else {
        return;
    };
    let (config, password) = test_config(uri);
    let worker = WorkerHandle::spawn(config, password).expect("spawn worker");

    let a = user_dn("edaptor-uniq-ma");
    let b = user_dn("edaptor-uniq-mb");
    cleanup(&worker, &a, 1);
    cleanup(&worker, &b, 2);

    let mut first = posix_account("edaptor-uniq-ma", "59405", "59405");
    first.insert(
        "mail".to_string(),
        vec!["edaptor-uniq-clash@example.org".to_string()],
    );
    let mut second = posix_account("edaptor-uniq-mb", "59406", "59406");
    second.insert(
        "mail".to_string(),
        vec!["edaptor-uniq-clash@example.org".to_string()],
    );

    worker
        .submit(Request::AddAtomic {
            id: 70,
            entries: vec![(a.clone(), first), (b.clone(), second)],
        })
        .expect("submit AddAtomic");
    let resp = poll_for_id(&worker, 70, Duration::from_secs(10));
    let Some(Response::WriteError { msg, .. }) = &resp else {
        panic!(
            "two entries sharing a mail must be rejected; got {}",
            describe(&resp)
        );
    };

    assert!(
        msg.contains("wrote nothing"),
        "the user must be told the change rolled back; msg={msg}"
    );
    assert!(
        msg.contains(&b),
        "the recovered diagnosis must name the entry that failed; msg={msg}"
    );
    assert!(
        msg.contains("mail"),
        "the server's reason names the offending attribute and must survive the \
         replay; msg={msg}"
    );

    // The replay creates the first entry to reproduce the clash and must remove
    // it again: a failed save may leave nothing behind.
    assert!(
        !msg.contains("WARNING"),
        "the replay must clean up after itself; msg={msg}"
    );
    let leftover = add_one(
        &worker,
        71,
        &a,
        posix_account("edaptor-uniq-ma", "59405", "59405"),
    );
    assert!(
        matches!(leftover, Some(Response::WriteOk { .. })),
        "the replay left {a} behind — adding it again should have been possible, \
         got {}",
        describe(&leftover)
    );

    cleanup(&worker, &a, 80);
    cleanup(&worker, &b, 81);
}

/// Two accounts may NOT share a uidNumber.
#[test]
fn two_accounts_may_not_share_a_uidnumber() {
    let Some(uri) = uri_or_skip("two_accounts_may_not_share_a_uidnumber") else {
        return;
    };
    let (config, password) = test_config(uri);
    let worker = WorkerHandle::spawn(config, password).expect("spawn worker");

    let a = user_dn("edaptor-uniq-ua");
    let b = user_dn("edaptor-uniq-ub");
    cleanup(&worker, &a, 1);
    cleanup(&worker, &b, 2);

    let first = add_one(
        &worker,
        50,
        &a,
        posix_account("edaptor-uniq-ua", "59403", "59403"),
    );
    assert!(
        matches!(first, Some(Response::WriteOk { .. })),
        "first account must be created; got {}",
        describe(&first)
    );

    let second = add_one(
        &worker,
        51,
        &b,
        posix_account("edaptor-uniq-ub", "59403", "59404"),
    );
    assert!(
        is_constraint_violation(&second),
        "a second account with the same uidNumber must be rejected by the unique \
         overlay; got {}",
        describe(&second)
    );

    cleanup(&worker, &a, 60);
    cleanup(&worker, &b, 61);
}
