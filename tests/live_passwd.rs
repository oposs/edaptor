//! Live integration test for `passwd` username resolution against a
//! containerized OpenLDAP.
//!
//! Enable by setting EDAPTOR_TEST_LDAP_URI (e.g. ldap://localhost:1389).
//! Start a server with: scripts/test-ldap.sh start
//! When the env var is unset each test prints SKIP and passes (no silent skip).
//!
//! Drives the public resolution surface (`passwd::username_searches` +
//! `passwd::resolve_outcome`) through a real worker, the same way `run_passwd`
//! composes them, to confirm a bare username resolves to the expected DN against
//! live seed data.

use edaptor::config::{
    AuthConfig, AuthMethod, Config, EntryProfile, PasswordSource, ServerConfig, TlsConfig,
};
use edaptor::ldap::worker::{Request, Response, SearchScope, WorkerHandle};
use edaptor::passwd::{resolve_outcome, username_searches, Resolution};

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

/// The seed "user" profile: uid-keyed entries under ou=people.
fn user_profile() -> EntryProfile {
    EntryProfile {
        name: "user".to_string(),
        object_classes: vec!["inetOrgPerson".to_string()],
        rdn_attr: "uid".to_string(),
        search_base: "ou=people,dc=example,dc=org".to_string(),
        ..Default::default()
    }
}

/// Resolve a bare username exactly as `run_passwd` does: run every profile search
/// against the live worker, collect the matching DNs, then decide.
fn resolve(worker: &WorkerHandle, profiles: &[EntryProfile], username: &str) -> Resolution {
    let mut dns = Vec::new();
    for (base, filter) in username_searches(profiles, username) {
        match worker
            .request(Request::Search {
                id: 1,
                base,
                scope: SearchScope::Subtree,
                filter,
                attrs: vec!["objectClass".to_string()],
                size_limit: None,
            })
            .expect("search request")
        {
            Response::Entries { entries, .. } => dns.extend(entries.into_iter().map(|e| e.dn)),
            Response::SearchError { msg, .. } => panic!("search error: {msg}"),
            other => panic!("unexpected response: {other:?}"),
        }
    }
    resolve_outcome(dns)
}

#[test]
fn bare_username_resolves_to_single_dn() {
    let Ok(uri) = std::env::var("EDAPTOR_TEST_LDAP_URI") else {
        eprintln!("SKIP: EDAPTOR_TEST_LDAP_URI unset");
        return;
    };
    let (config, password) = test_config(uri);
    let worker = WorkerHandle::spawn(config, password).expect("spawn worker");
    let profiles = [user_profile()];

    assert_eq!(
        resolve(&worker, &profiles, "bbrown"),
        Resolution::Unique("uid=bbrown,ou=people,dc=example,dc=org".to_string()),
    );
}

#[test]
fn unknown_username_is_not_found() {
    let Ok(uri) = std::env::var("EDAPTOR_TEST_LDAP_URI") else {
        eprintln!("SKIP: EDAPTOR_TEST_LDAP_URI unset");
        return;
    };
    let (config, password) = test_config(uri);
    let worker = WorkerHandle::spawn(config, password).expect("spawn worker");
    let profiles = [user_profile()];

    assert_eq!(
        resolve(&worker, &profiles, "no-such-user-zzz"),
        Resolution::NotFound,
    );
}
