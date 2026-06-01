//! End-to-end test against a live OpenLDAP server.
//!
//! Enable by setting EDAPTOR_TEST_LDAP_URI (e.g. ldap://localhost:1389).
//! Start a server with: scripts/test-ldap.sh start
//! When the env var is unset the test prints SKIP and passes (no silent skip).

use edaptor::config::{AuthConfig, AuthMethod, Config, PasswordSource, ServerConfig, TlsConfig};
use edaptor::run_check;

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

#[test]
fn connects_binds_and_fetches_subschema() {
    let uri = match std::env::var("EDAPTOR_TEST_LDAP_URI") {
        Ok(uri) => uri,
        Err(_) => {
            eprintln!(
                "SKIP connects_binds_and_fetches_subschema: set EDAPTOR_TEST_LDAP_URI to run"
            );
            return;
        }
    };

    let (config, password) = test_config(uri);
    let summary = run_check(config, password).expect("run_check should succeed against the server");

    assert!(
        summary.object_class_count > 0,
        "expected objectClasses in subschema"
    );
    assert!(
        summary.attribute_type_count > 0,
        "expected attributeTypes in subschema"
    );
    assert!(
        summary.ldap_syntax_count > 0,
        "expected ldapSyntaxes in subschema"
    );
}

#[test]
fn wrong_password_is_rejected() {
    let uri = match std::env::var("EDAPTOR_TEST_LDAP_URI") {
        Ok(uri) => uri,
        Err(_) => {
            eprintln!("SKIP wrong_password_is_rejected: set EDAPTOR_TEST_LDAP_URI to run");
            return;
        }
    };

    let (config, _password) = test_config(uri);
    // CheckSummary does not derive Debug, so use match instead of unwrap_err.
    let err = match run_check(config, "definitely-wrong".to_string()) {
        Err(e) => e,
        Ok(_) => panic!("expected run_check to fail with a wrong password"),
    };
    assert!(
        err.to_string().contains("rejected the bind credentials")
            || err.to_string().to_lowercase().contains("invalid"),
        "expected a bind rejection, got: {err}"
    );
}

#[test]
fn resolves_inetorgperson_schema() {
    let uri = match std::env::var("EDAPTOR_TEST_LDAP_URI") {
        Ok(uri) => uri,
        Err(_) => {
            eprintln!("SKIP resolves_inetorgperson_schema: set EDAPTOR_TEST_LDAP_URI to run");
            return;
        }
    };

    let (config, password) = test_config(uri);
    let report = edaptor::run_schema(config, password, "inetOrgPerson")
        .expect("run_schema should resolve inetOrgPerson");

    let names: Vec<&str> = report.attributes.iter().map(|a| a.name.as_str()).collect();
    // cn and sn are MUST (inherited from person); mail is MAY.
    assert!(
        names.iter().any(|n| n.eq_ignore_ascii_case("cn")),
        "attrs={names:?}"
    );
    assert!(
        names.iter().any(|n| n.eq_ignore_ascii_case("sn")),
        "attrs={names:?}"
    );
    assert!(report
        .attributes
        .iter()
        .any(|a| a.name.eq_ignore_ascii_case("sn") && a.required));
    assert!(
        names.iter().any(|n| n.eq_ignore_ascii_case("mail")),
        "attrs={names:?}"
    );
}

// ---------------------------------------------------------------------------
// M3: worker SEARCH (base / one-level) + the read flow end-to-end (no tty).
// Same EDAPTOR_TEST_LDAP_URI gate; SKIP-with-eprintln when unset.
// ---------------------------------------------------------------------------

use edaptor::ldap::worker::{Request, Response, SearchScope, WorkerHandle};
use edaptor::schema::SchemaModel;
use edaptor::workflows::read_flow::ReadFlow;
use std::time::{Duration, Instant};

/// Short variant label for diagnostics.
fn variant_name(resp: &Response) -> &'static str {
    match resp {
        Response::Subschema(_) => "Subschema",
        Response::Entries { .. } => "Entries",
        Response::SearchError { .. } => "SearchError",
        Response::StructureEntries { .. } => "StructureEntries",
        Response::StructureError { .. } => "StructureError",
        Response::WriteOk { .. } => "WriteOk",
        Response::WriteError { .. } => "WriteError",
        Response::Done => "Done",
        Response::Error(_) => "Error",
    }
}

/// Poll the worker's non-blocking channel until the `Search` reply correlated to
/// `want_id` arrives, or we time out. `request()` (the synchronous schema fetch)
/// uses a separate per-call reply channel, so only `submit`ted searches land
/// here; we still filter by id for robustness (D4). Network I/O is on the worker
/// thread — this only drains the channel.
fn poll_for_id(worker: &WorkerHandle, want_id: u64, timeout: Duration) -> Option<Response> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match worker.poll() {
            Some(Response::Entries { id, entries }) if id == want_id => {
                return Some(Response::Entries { id, entries });
            }
            Some(Response::SearchError { id, msg }) if id == want_id => {
                return Some(Response::SearchError { id, msg });
            }
            Some(_) => continue, // foreign / stale response: ignore
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    None
}

#[test]
fn one_level_search_lists_children() {
    let uri = match std::env::var("EDAPTOR_TEST_LDAP_URI") {
        Ok(uri) => uri,
        Err(_) => {
            eprintln!("SKIP one_level_search_lists_children: set EDAPTOR_TEST_LDAP_URI to run");
            return;
        }
    };

    let (config, password) = test_config(uri);
    let base_dn = config.server.base_dn.clone();
    let worker = WorkerHandle::spawn(config, password).expect("spawn worker");

    worker
        .submit(Request::Search {
            id: 1,
            base: base_dn,
            scope: SearchScope::OneLevel,
            filter: "(objectClass=*)".to_string(),
            attrs: vec!["cn".to_string(), "objectClass".to_string()],
            size_limit: None,
        })
        .expect("submit one-level search");

    match poll_for_id(&worker, 1, Duration::from_secs(10)) {
        Some(Response::Entries { id, entries }) => {
            assert_eq!(id, 1, "reply must echo the request id");
            assert!(!entries.is_empty(), "base should have children");
            let dns: Vec<&str> = entries.iter().map(|e| e.dn.as_str()).collect();
            // bitnami OpenLDAP seeds ou=users,dc=example,dc=org by default.
            assert!(
                dns.iter()
                    .any(|d| d.eq_ignore_ascii_case("ou=users,dc=example,dc=org")),
                "expected ou=users child, got {dns:?}"
            );
        }
        Some(Response::SearchError { id, msg }) => {
            panic!("one-level search errored (id={id}): {msg}")
        }
        Some(other) => panic!("expected Entries, got {}", variant_name(&other)),
        None => panic!("timed out waiting for one-level search response"),
    }
}

#[test]
fn base_search_reads_entry_then_form_model() {
    let uri = match std::env::var("EDAPTOR_TEST_LDAP_URI") {
        Ok(uri) => uri,
        Err(_) => {
            eprintln!(
                "SKIP base_search_reads_entry_then_form_model: set EDAPTOR_TEST_LDAP_URI to run"
            );
            return;
        }
    };

    let (config, password) = test_config(uri);
    let worker = WorkerHandle::spawn(config, password).expect("spawn worker");

    // Fetch the real schema for the form-model typing (synchronous path).
    let raw = match worker
        .request(Request::FetchSubschema)
        .expect("request schema")
    {
        Response::Subschema(raw) => raw,
        other => panic!("expected Subschema, got {}", variant_name(&other)),
    };
    let schema = SchemaModel::from_raw(&raw);
    let read_flow = ReadFlow::new(schema);

    // Base-read a known seeded user, then turn it into a form model.
    let dn = "cn=user01,ou=users,dc=example,dc=org";
    worker
        .submit(Request::Search {
            id: 2,
            base: dn.to_string(),
            scope: SearchScope::Base,
            filter: "(objectClass=*)".to_string(),
            attrs: vec!["*".to_string()],
            size_limit: None,
        })
        .expect("submit base search");

    match poll_for_id(&worker, 2, Duration::from_secs(10)) {
        Some(Response::Entries { id, entries }) => {
            assert_eq!(id, 2);
            let entry = entries.first().expect("user01 should exist");
            let model = read_flow.form_for(entry, &[]);
            assert_eq!(model.title, dn);
            let labels: Vec<&str> = model.fields.iter().map(|f| f.label.as_str()).collect();
            // inetOrgPerson MUST cn/sn should appear and sn be flagged.
            assert!(
                labels.iter().any(|l| l.eq_ignore_ascii_case("cn")),
                "fields={labels:?}"
            );
            assert!(
                model
                    .fields
                    .iter()
                    .any(|f| f.label.eq_ignore_ascii_case("sn") && f.is_must),
                "sn should be a flagged MUST; fields={labels:?}"
            );
        }
        Some(Response::SearchError { id, msg }) => {
            panic!("base search errored (id={id}): {msg}")
        }
        Some(other) => panic!("expected Entries, got {}", variant_name(&other)),
        None => panic!("timed out waiting for base search response"),
    }
}
