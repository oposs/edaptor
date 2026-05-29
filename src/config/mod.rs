//! Configuration: connection properties + auth. (Entry profiles arrive in M4.)

pub mod password;
pub use password::PasswordSource;

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub auth: AuthConfig,
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    pub uri: String,
    pub base_dn: String,
    #[serde(default)]
    pub start_tls: bool,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub tls: TlsConfig,
}

fn default_timeout() -> u64 {
    10
}

#[derive(Debug, Deserialize)]
pub struct TlsConfig {
    #[serde(default)]
    pub ca_cert: Option<PathBuf>,
    #[serde(default)]
    pub client_cert: Option<PathBuf>,
    #[serde(default)]
    pub client_key: Option<PathBuf>,
    #[serde(default = "default_true")]
    pub verify: bool,
}

fn default_true() -> bool {
    true
}

// Manual Default so an absent [server.tls] table yields verify = true.
impl Default for TlsConfig {
    fn default() -> Self {
        TlsConfig {
            ca_cert: None,
            client_cert: None,
            client_key: None,
            verify: true,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct AuthConfig {
    #[serde(default)]
    pub method: AuthMethod,
    #[serde(default)]
    pub bind_dn: Option<String>,
    #[serde(default)]
    pub password_source: PasswordSource,
}

#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthMethod {
    #[default]
    Simple,
    External,
    Gssapi,
}

impl Config {
    pub fn load(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let config: Config =
            toml::from_str(&text).with_context(|| format!("parsing config {}", path.display()))?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_config() {
        let toml = r#"
            [server]
            uri = "ldaps://ldap.example.com:636"
            base_dn = "dc=example,dc=com"

            [auth]
            method = "simple"
            bind_dn = "cn=ldapmanager,dc=example,dc=com"
            password_source = "prompt"
        "#;
        let cfg: Config = toml::from_str(toml).expect("should parse");
        assert_eq!(cfg.server.uri, "ldaps://ldap.example.com:636");
        assert_eq!(cfg.server.base_dn, "dc=example,dc=com");
        assert_eq!(cfg.server.timeout_secs, 10); // default
        assert!(cfg.server.tls.verify); // default true
        assert_eq!(cfg.auth.method, AuthMethod::Simple);
        assert_eq!(
            cfg.auth.bind_dn.as_deref(),
            Some("cn=ldapmanager,dc=example,dc=com")
        );
    }

    #[test]
    fn tls_defaults_to_verify_true_when_table_absent() {
        let toml = r#"
            [server]
            uri = "ldap://ldap.example.com:389"
            base_dn = "dc=example,dc=com"
            [auth]
            bind_dn = "cn=admin,dc=example,dc=com"
        "#;
        let cfg: Config = toml::from_str(toml).expect("should parse");
        assert!(cfg.server.tls.verify);
        assert!(!cfg.server.start_tls); // default false
        assert_eq!(cfg.auth.method, AuthMethod::Simple); // default
    }

    #[test]
    fn missing_uri_is_an_error() {
        let toml = r#"
            [server]
            base_dn = "dc=example,dc=com"
            [auth]
            bind_dn = "cn=admin,dc=example,dc=com"
        "#;
        assert!(toml::from_str::<Config>(toml).is_err());
    }
}
