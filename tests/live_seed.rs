//! Live test (gated by EDAPTOR_TEST_LDAP_URI): the rich seed data loaded by
//! scripts/test-ldap.sh is present and well-formed. SKIPS cleanly when unset.

use edaptor::config::{AuthConfig, AuthMethod, Config, PasswordSource, ServerConfig, TlsConfig};
use edaptor::ldap::worker::{Request, Response, SearchScope, WorkerHandle};

const BASE: &str = "dc=example,dc=org";

fn test_config(uri: String) -> (Config, String) {
    let config = Config {
        server: ServerConfig {
            uri,
            base_dn: BASE.to_string(),
            start_tls: false,
            read_only: false,
            timeout_secs: 10,
            tls: TlsConfig::default(),
        },
        auth: AuthConfig {
            method: AuthMethod::Simple,
            bind_dn: Some(format!("cn=admin,{BASE}")),
            password_source: PasswordSource::Env("EDAPTOR_TEST_ADMIN_PW".to_string()),
        },
        profiles: Vec::new(),
        samba: Default::default(),
    };
    let password =
        std::env::var("EDAPTOR_TEST_ADMIN_PW").unwrap_or_else(|_| "adminpassword".to_string());
    (config, password)
}

fn search(
    worker: &WorkerHandle,
    base: &str,
    filter: &str,
    attrs: Vec<String>,
) -> Vec<edaptor::ldap::worker::LdapEntry> {
    let resp = worker
        .request(Request::Search {
            id: 1,
            base: base.to_string(),
            scope: SearchScope::Subtree,
            filter: filter.to_string(),
            attrs,
            size_limit: None,
        })
        .expect("search should reply");
    match resp {
        Response::Entries { entries, .. } => entries,
        other => panic!("expected Entries, got {other:?}"),
    }
}

#[test]
fn seed_people_count_exceeds_one_hundred() {
    let Ok(uri) = std::env::var("EDAPTOR_TEST_LDAP_URI") else {
        eprintln!("SKIP: EDAPTOR_TEST_LDAP_URI not set");
        return;
    };
    let (cfg, password) = test_config(uri);
    let worker = WorkerHandle::spawn(cfg, password).expect("connect+bind");
    // Paged subtree scan returns the full set past the 500 size limit.
    let resp = worker
        .request(Request::LoadStructure {
            id: 1,
            base: format!("ou=people,{BASE}"),
            page_size: 200,
            attrs: vec![],
        })
        .expect("structure scan should reply");
    let count = match resp {
        Response::StructureEntries { nodes, .. } => nodes.len(),
        other => panic!("expected StructureEntries, got {other:?}"),
    };
    assert!(count > 100, "expected >100 people, got {count}");
}

#[test]
fn seed_has_posix_and_membership_groups() {
    let Ok(uri) = std::env::var("EDAPTOR_TEST_LDAP_URI") else {
        eprintln!("SKIP: EDAPTOR_TEST_LDAP_URI not set");
        return;
    };
    let (cfg, password) = test_config(uri);
    let worker = WorkerHandle::spawn(cfg, password).expect("connect+bind");

    let posix = search(
        &worker,
        &format!("ou=groups,{BASE}"),
        "(objectClass=posixGroup)",
        vec!["gidNumber".to_string()],
    );
    assert!(
        posix.len() >= 5,
        "expected >=5 posixGroups, got {}",
        posix.len()
    );
    assert!(
        posix.iter().all(|e| e.attrs.contains_key("gidNumber")),
        "every posixGroup must expose gidNumber"
    );

    let gon = search(
        &worker,
        &format!("ou=groups,{BASE}"),
        "(objectClass=groupOfNames)",
        vec!["member".to_string()],
    );
    assert!(
        gon.len() >= 5,
        "expected >=5 groupOfNames, got {}",
        gon.len()
    );
    assert!(
        gon.iter().all(|e| e.attrs.contains_key("member")),
        "every groupOfNames must expose member"
    );
}

#[test]
fn seed_samba_domain_is_discoverable() {
    let Ok(uri) = std::env::var("EDAPTOR_TEST_LDAP_URI") else {
        eprintln!("SKIP: EDAPTOR_TEST_LDAP_URI not set");
        return;
    };
    let (cfg, password) = test_config(uri);
    let worker = WorkerHandle::spawn(cfg, password).expect("connect+bind");
    let domains = search(
        &worker,
        BASE,
        "(objectClass=sambaDomain)",
        vec!["sambaSID".to_string()],
    );
    assert_eq!(domains.len(), 1, "exactly one sambaDomain");
    assert!(
        domains[0]
            .attrs
            .get("sambaSID")
            .is_some_and(|v| !v.is_empty()),
        "sambaDomain must yield a sambaSID"
    );
}
