//! Turn a ServerConfig into ldap3 LdapConnSettings (native-tls backend).
//!
//! M1 wires the configured CA, the verify flag, and the connect timeout.
//! Client-certificate identity (for SASL EXTERNAL) is added in the auth
//! milestone (M6); per-operation timeouts are tracked for a later milestone.

use std::time::Duration;

use anyhow::{Context, Result};
use ldap3::LdapConnSettings;
use native_tls::{Certificate, TlsConnector};

use crate::config::ServerConfig;

pub fn build_settings(server: &ServerConfig) -> Result<LdapConnSettings> {
    // Bound the TCP connect so an unreachable/black-hole server cannot hang the
    // worker thread indefinitely. (Per-operation timeouts come in a later milestone.)
    let mut settings =
        LdapConnSettings::new().set_conn_timeout(Duration::from_secs(server.timeout_secs));

    // StartTLS upgrades an ldap:// connection (do NOT combine with ldaps://).
    if server.start_tls {
        settings = settings.set_starttls(true);
    }

    // Trust a custom CA if configured.
    if let Some(ca_path) = &server.tls.ca_cert {
        let pem = std::fs::read(ca_path)
            .with_context(|| format!("reading CA cert {}", ca_path.display()))?;
        let ca = Certificate::from_pem(&pem)
            .with_context(|| format!("parsing CA cert {}", ca_path.display()))?;
        let connector = TlsConnector::builder()
            .add_root_certificate(ca)
            .build()
            .context("building TLS connector")?;
        settings = settings.set_connector(connector);
    }

    // Disable verification only when explicitly configured (testing).
    if !server.tls.verify {
        settings = settings.set_no_tls_verify(true);
    }

    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ServerConfig, TlsConfig};
    use std::io::Write;

    fn server_with_tls(tls: TlsConfig, start_tls: bool) -> ServerConfig {
        ServerConfig {
            uri: "ldaps://ldap.example.com:636".to_string(),
            base_dn: "dc=example,dc=com".to_string(),
            start_tls,
            read_only: false,
            timeout_secs: 10,
            tls,
        }
    }

    #[test]
    fn builds_settings_with_no_custom_ca() {
        let server = server_with_tls(TlsConfig::default(), false);
        assert!(build_settings(&server).is_ok());
    }

    #[test]
    fn builds_settings_with_starttls_and_no_verify() {
        let tls = TlsConfig {
            verify: false,
            ..TlsConfig::default()
        };
        let server = server_with_tls(tls, true);
        assert!(build_settings(&server).is_ok());
    }

    #[test]
    fn missing_ca_file_is_an_error() {
        let tls = TlsConfig {
            ca_cert: Some("/no/such/ca.pem".into()),
            ..TlsConfig::default()
        };
        let server = server_with_tls(tls, false);
        let err = build_settings(&server).err().unwrap();
        assert!(err.to_string().contains("reading CA cert"), "got: {err}");
    }

    #[test]
    fn garbage_ca_file_is_a_parse_error() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"this is not a certificate").unwrap();
        let tls = TlsConfig {
            ca_cert: Some(f.path().to_path_buf()),
            ..TlsConfig::default()
        };
        let server = server_with_tls(tls, false);
        let err = build_settings(&server).err().unwrap();
        assert!(err.to_string().contains("parsing CA cert"), "got: {err}");
    }
}
