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
            timeout_secs: 10,
            tls: TlsConfig::default(),
        },
        auth: AuthConfig {
            method: AuthMethod::Simple,
            bind_dn: Some("cn=admin,dc=example,dc=org".to_string()),
            password_source: PasswordSource::Env("EDAPTOR_TEST_ADMIN_PW".to_string()),
        },
        profiles: Vec::new(),
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
