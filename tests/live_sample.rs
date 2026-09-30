//! Live tests for the detection sample search and the sampler.
//!
//! Enable by setting EDAPTOR_TEST_LDAP_URI (e.g. ldap://localhost:11389).
//! When the env var is unset the tests print SKIP and pass.

use std::time::Duration;

use edaptor::config::{AuthConfig, AuthMethod, Config, PasswordSource, ServerConfig, TlsConfig};
use edaptor::detect::sample::{sample, Budget, WorkerSearcher};
use edaptor::ldap::worker::{Request, Response, SampleParams, SearchScope, WorkerHandle};

fn spawn(bind_dn: &str) -> Option<WorkerHandle> {
    let uri = match std::env::var("EDAPTOR_TEST_LDAP_URI") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("SKIP live_sample: set EDAPTOR_TEST_LDAP_URI to run");
            return None;
        }
    };
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
            password_source: PasswordSource::Env("EDAPTOR_TEST_ADMIN_PW".to_string()),
        },
        profiles: Vec::new(),
        overrides: Vec::new(),
        detect: Default::default(),
        meta: Default::default(),
        samba: Default::default(),
        tree: Default::default(),
    };
    let pw = if bind_dn.is_empty() {
        String::new()
    } else {
        std::env::var("EDAPTOR_TEST_ADMIN_PW").unwrap_or_else(|_| "adminpassword".to_string())
    };
    Some(WorkerHandle::spawn(config, pw).expect("worker spawn"))
}

fn params(base: &str, scope: SearchScope, filter: &str, attrs: &[&str]) -> SampleParams {
    SampleParams {
        base: base.to_string(),
        scope,
        filter: filter.to_string(),
        attrs: attrs.iter().map(|a| a.to_string()).collect(),
        size_limit: None,
        types_only: false,
        time_limit: Duration::from_secs(10),
    }
}

fn run(w: &WorkerHandle, params: SampleParams) -> (Vec<edaptor::ldap::worker::LdapEntry>, bool) {
    match w.request(Request::SampleSearch { id: 1, params }).unwrap() {
        Response::Entries {
            entries, truncated, ..
        } => (entries, truncated),
        other => panic!("unexpected {other:?}"),
    }
}

const ADMIN: &str = "cn=admin,dc=example,dc=org";

#[test]
fn types_only_returns_keys_without_values() {
    let Some(w) = spawn(ADMIN) else { return };
    let mut p = params(
        "ou=people,dc=example,dc=org",
        SearchScope::OneLevel,
        "(objectClass=*)",
        &["*"],
    );
    p.types_only = true;
    let (entries, truncated) = run(&w, p);
    assert!(!entries.is_empty() && !truncated);
    for e in &entries {
        assert!(!e.attrs.is_empty(), "{} has no attribute keys", e.dn);
        assert!(e.attrs.values().all(|v| v.is_empty()), "{}", e.dn);
    }
}

#[test]
fn size_limit_truncates() {
    let Some(w) = spawn(ADMIN) else { return };
    let mut p = params(
        "ou=people,dc=example,dc=org",
        SearchScope::OneLevel,
        "(objectClass=*)",
        &["uid"],
    );
    p.size_limit = Some(5);
    let (entries, truncated) = run(&w, p);
    assert_eq!(entries.len(), 5);
    assert!(truncated);
}

#[test]
fn has_subordinates_finds_containers() {
    let Some(w) = spawn(ADMIN) else { return };
    let p = params(
        "dc=example,dc=org",
        SearchScope::Subtree,
        "(hasSubordinates=TRUE)",
        &["1.1"],
    );
    let (entries, _) = run(&w, p);
    assert!(!entries.is_empty());
}

#[test]
fn sampler_reads_the_demo_directory() {
    let Some(w) = spawn(ADMIN) else { return };
    let s = sample(
        &mut WorkerSearcher(&w),
        "dc=example,dc=org",
        &Budget::new(Duration::from_secs(30)),
    )
    .expect("sample");
    assert!(!s.containers.is_empty());
}

#[test]
fn anonymous_bind_is_never_an_error() {
    let Some(w) = spawn("") else { return };
    let s = sample(
        &mut WorkerSearcher(&w),
        "dc=example,dc=org",
        &Budget::new(Duration::from_secs(30)),
    );
    assert!(s.is_ok(), "anonymous view must not be an error: {s:?}");
}
