//! Live test for the entry list's server-backed incremental find.
//!
//! Enable by setting EDAPTOR_TEST_LDAP_URI (e.g. ldap://localhost:1389).
//! Start a server with: scripts/test-ldap.sh start
//! When the env var is unset the test prints SKIP and passes.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use edaptor::config::{AuthConfig, AuthMethod, Config, PasswordSource, ServerConfig, TlsConfig};
use edaptor::ldap::worker::{Request, Response, WorkerHandle};
use edaptor::workflows::leaf_search::{LeafSearchFlow, LeafSearchOutcome};

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

#[test]
fn find_sees_an_entry_created_out_of_band() {
    let Ok(uri) = std::env::var("EDAPTOR_TEST_LDAP_URI") else {
        println!("SKIP: EDAPTOR_TEST_LDAP_URI unset");
        return;
    };
    let (config, password) = test_config(uri);
    let worker = WorkerHandle::spawn(config, password).expect("worker");

    let dn = "uid=coherence-probe,ou=people,dc=example,dc=org";

    // Idempotent cleanup from any prior aborted run.
    let _ = worker.submit(Request::Delete {
        id: 900_000,
        dn: dn.to_string(),
        assert_csn: None,
    });
    let _ = poll_for_id(&worker, 900_000, Duration::from_secs(5));

    // The "other client": add an entry the running edaptor never saw.
    let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    attrs.insert(
        "objectClass".into(),
        vec![
            "inetOrgPerson".into(),
            "organizationalPerson".into(),
            "person".into(),
            "top".into(),
        ],
    );
    attrs.insert("cn".into(), vec!["Coherence Probe".into()]);
    attrs.insert("sn".into(), vec!["Probe".into()]);
    attrs.insert("uid".into(), vec!["coherence-probe".into()]);
    let add_id = 900_001;
    worker
        .submit(Request::Add {
            id: add_id,
            dn: dn.to_string(),
            attrs,
        })
        .expect("submit add");
    let resp = poll_for_id(&worker, add_id, Duration::from_secs(10)).expect("add reply");
    assert!(
        matches!(resp, Response::WriteOk { .. }),
        "add failed: {resp:?}"
    );

    // The find must see it without any structure rescan.
    let mut flow = LeafSearchFlow::new();
    let id = flow
        .request(
            &worker,
            "ou=people,dc=example,dc=org",
            "coherence-probe",
            &["cn".to_string(), "uid".to_string()],
            &["cn".to_string(), "uid".to_string()],
        )
        .expect("submit find");
    let resp = poll_for_id(&worker, id, Duration::from_secs(10)).expect("find reply");
    match flow.on_response(&resp) {
        LeafSearchOutcome::Results { entries, .. } => {
            assert!(
                entries.iter().any(|e| e.dn.eq_ignore_ascii_case(dn)),
                "the out-of-band entry must be findable: {entries:?}"
            );
        }
        other => panic!("expected Results, got {other:?}"),
    }

    // Clean up so repeated runs stay idempotent.
    let del_id = 900_002;
    worker
        .submit(Request::Delete {
            id: del_id,
            dn: dn.to_string(),
            assert_csn: None,
        })
        .expect("submit delete");
    let _ = poll_for_id(&worker, del_id, Duration::from_secs(10));
}
