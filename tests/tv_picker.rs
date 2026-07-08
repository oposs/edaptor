//! Live picker integration test for the M4 tvision layer (Task 14).
//!
//! Enable by setting EDAPTOR_TEST_LDAP_URI (e.g. ldap://localhost:1389).
//! Start a server with: scripts/test-ldap.sh start
//! When the env var is unset the test prints SKIP and passes (no silent skip).
//!
//! Read-only: it drives a real candidate search via `SearchFlow` against the
//! demo data and simulates a pick through `PickState`, asserting that:
//!   * a scalar-store picker (gidNumber over posixGroup) yields candidates whose
//!     `store_value` is the numeric gidNumber (not a DN), and that picking one
//!     stages exactly that scalar; and
//!   * a DN-store picker (users under ou=people) yields candidates whose
//!     `store_value` is the entry DN, and that picking one stages that DN.
//!
//! No writes are made to the demo data — there is nothing to clean up.

use std::time::{Duration, Instant};

use edaptor::config::{AuthConfig, AuthMethod, Config, PasswordSource, ServerConfig, TlsConfig};
use edaptor::ldap::worker::{Response, WorkerHandle};
use edaptor::workflows::pick_state::PickState;
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

/// Drive one `SearchFlow.request`/`on_response` round-trip and return its outcome.
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
                // Only the response correlated to `want_id` is non-Ignored.
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
fn picker_live_search_and_pick() {
    let Ok(uri) = std::env::var("EDAPTOR_TEST_LDAP_URI") else {
        eprintln!("SKIP picker_live_search_and_pick: set EDAPTOR_TEST_LDAP_URI to run");
        return;
    };

    let (config, password) = test_config(uri);
    let worker = WorkerHandle::spawn(config, password).expect("spawn worker");

    // -----------------------------------------------------------------------
    // Scalar store: gidNumber over posixGroup (mirrors demo-config's gidNumber
    // picker — candidate = posixgroup, store = gidNumber, select = single).
    // -----------------------------------------------------------------------
    {
        let mut flow = SearchFlow::new();
        let attrs = vec!["cn".to_string(), "gidNumber".to_string()];
        let outcome = run_search(
            &worker,
            &mut flow,
            "ou=groups,dc=example,dc=org",
            "posixGroup",
            "", // empty term → objectClass-only, returns up to the cap
            &attrs,
            Some("gidNumber"),
            Duration::from_secs(10),
        );
        let rows = match outcome {
            SearchOutcome::Results { rows, .. } => rows,
            other => panic!("expected scalar-store Results, got {other:?}"),
        };
        assert!(
            !rows.is_empty(),
            "the demo server must return posixGroup candidates"
        );
        // store_value must be the numeric gidNumber, NOT a DN.
        let first = &rows[0];
        assert!(
            first.store_value.chars().all(|c| c.is_ascii_digit()),
            "scalar store_value must be a numeric gidNumber, got {:?}",
            first.store_value
        );
        assert!(
            first.dn.contains("ou=groups"),
            "candidate dn should be a group DN, got {:?}",
            first.dn
        );

        // Simulate a single-select pick (exact-key store): toggle row 0.
        let mut pick = PickState::new(vec![], false);
        pick.set_results(rows.clone());
        pick.cursor = 0;
        pick.toggle_cursor();
        assert_eq!(
            pick.selected_values(),
            vec![first.store_value.clone()],
            "picking the first group must stage exactly its gidNumber"
        );
    }

    // -----------------------------------------------------------------------
    // DN store: users under ou=people (mirrors the `member` / `memberUid`
    // pickers' candidate = user; here we assert the DN-store default).
    // -----------------------------------------------------------------------
    {
        let mut flow = SearchFlow::new();
        let attrs = vec!["cn".to_string(), "uid".to_string()];
        let outcome = run_search(
            &worker,
            &mut flow,
            "ou=people,dc=example,dc=org",
            "inetOrgPerson",
            "",
            &attrs,
            None, // DN store
            Duration::from_secs(10),
        );
        let rows = match outcome {
            SearchOutcome::Results { rows, .. } => rows,
            other => panic!("expected DN-store Results, got {other:?}"),
        };
        assert!(
            !rows.is_empty(),
            "the demo server must return user candidates"
        );
        let first = &rows[0];
        assert_eq!(
            first.store_value, first.dn,
            "DN store: store_value must equal the candidate DN"
        );
        assert!(
            first.dn.contains("ou=people"),
            "candidate dn should be a person DN, got {:?}",
            first.dn
        );

        // Simulate a multi-select DN pick (case-insensitive key): toggle row 0.
        let mut pick = PickState::new(vec![], true);
        pick.set_results(rows.clone());
        pick.cursor = 0;
        pick.toggle_cursor();
        assert_eq!(
            pick.selected_values(),
            vec![first.dn.clone()],
            "picking the first user must stage exactly its DN"
        );
    }
}
