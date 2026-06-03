//! Live test (gated by EDAPTOR_TEST_LDAP_URI): the eager subtree paged scan
//! returns the full structure under the base, including entries past the default
//! size limit. SKIPS cleanly when the env var is unset.

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
        samba: Default::default(),
        relations: Vec::new(),
    };
    let password =
        std::env::var("EDAPTOR_TEST_ADMIN_PW").unwrap_or_else(|_| "adminpassword".to_string());
    (config, password)
}

#[test]
fn eager_structure_scan_returns_subtree() {
    let Ok(uri) = std::env::var("EDAPTOR_TEST_LDAP_URI") else {
        eprintln!("SKIP: EDAPTOR_TEST_LDAP_URI not set");
        return;
    };
    let (cfg, password) = test_config(uri);
    let worker = WorkerHandle::spawn(cfg, password).expect("worker should connect+bind");

    let resp = worker
        .request(Request::LoadStructure {
            id: 1,
            base: "dc=example,dc=org".to_string(),
            page_size: 2,
            attrs: vec![],
        })
        .expect("structure scan should reply");

    match resp {
        Response::StructureEntries { id, nodes } => {
            assert_eq!(id, 1);
            assert!(
                nodes.iter().any(|n| n.dn == "dc=example,dc=org"),
                "base present"
            );
            assert!(
                nodes.iter().any(|n| n.dn.starts_with("ou=users")),
                "ou=users present"
            );
            assert!(
                nodes.len() >= 3,
                "expected several entries, got {}",
                nodes.len()
            );
        }
        other => panic!("expected StructureEntries, got {other:?}"),
    }
}
