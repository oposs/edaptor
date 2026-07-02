//! Live memberUid picker integration test (scalar `uid` store).
//!
//! Enable by setting EDAPTOR_TEST_LDAP_URI (e.g. ldap://localhost:1389).
//! Start a server with: scripts/test-ldap.sh start
//! When the env var is unset the test prints SKIP and passes (no silent skip).
//!
//! Read-only: it drives a real candidate search via `SearchFlow` for the
//! `memberUid` picker shape (candidate = user under ou=people, store = uid) and
//! asserts the candidates' `store_value` is the user's `uid` scalar (NOT a DN) —
//! exactly the keys the multi-select Shuttle stages into `memberUid`.

use std::time::{Duration, Instant};

use edaptor::config::{AuthConfig, AuthMethod, Config, PasswordSource, ServerConfig, TlsConfig};
use edaptor::ldap::worker::{Response, WorkerHandle};
use edaptor::workflows::search_flow::{SearchFlow, SearchOutcome};

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

#[allow(clippy::too_many_arguments)]
fn run_search(
    worker: &WorkerHandle,
    flow: &mut SearchFlow,
    base: &str,
    oc: &str,
    term: &str,
    attrs: &[String],
    store_attr: Option<&str>,
    timeout: Duration,
) -> SearchOutcome {
    let want_id = flow
        .request(worker, base, oc, term, attrs, store_attr)
        .expect("submit search request");
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match worker.poll() {
            Some(resp) => {
                let matches_id = matches!(
                    &resp,
                    Response::Entries { id, .. } | Response::SearchError { id, .. } if *id == want_id
                );
                if matches_id {
                    return flow.on_response(&resp);
                }
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    panic!("search for term {term:?} timed out");
}

#[test]
fn member_uid_picker_stores_uid_scalar() {
    let Ok(uri) = std::env::var("EDAPTOR_TEST_LDAP_URI") else {
        eprintln!("SKIP member_uid_picker_stores_uid_scalar: set EDAPTOR_TEST_LDAP_URI to run");
        return;
    };

    let (config, password) = test_config(uri);
    let worker = WorkerHandle::spawn(config, password).expect("spawn worker");

    // memberUid picker shape: candidate = user (ou=people, inetOrgPerson),
    // store = uid. The requested attrs must include `uid` (the store attr).
    let mut flow = SearchFlow::new();
    let attrs = vec!["cn".to_string(), "uid".to_string()];
    let outcome = run_search(
        &worker,
        &mut flow,
        "ou=people,dc=example,dc=org",
        "inetOrgPerson",
        "", // empty term → objectClass-only, returns up to the cap
        &attrs,
        Some("uid"), // scalar store
        Duration::from_secs(10),
    );
    let rows = match outcome {
        SearchOutcome::Results { rows, .. } => rows,
        other => panic!("expected uid-store Results, got {other:?}"),
    };
    assert!(!rows.is_empty(), "the demo server must return user candidates");

    let first = &rows[0];
    // store_value must be the uid scalar, NOT the DN.
    assert_ne!(
        first.store_value, first.dn,
        "uid store: store_value must be the uid scalar, not the DN"
    );
    assert!(
        !first.store_value.contains('=') && !first.store_value.contains(','),
        "uid store_value must look like a bare uid, got {:?}",
        first.store_value
    );
    assert!(
        first.dn.contains("ou=people"),
        "candidate dn should be a person DN, got {:?}",
        first.dn
    );
}
