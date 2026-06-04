//! Turn a ServerConfig into ldap3 LdapConnSettings (rustls backend).
//!
//! M1 wires the configured CA, the verify flag, and the connect timeout.
//! Client-certificate identity (for SASL EXTERNAL) is added in the auth
//! milestone (M6); per-operation timeouts are tracked for a later milestone.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use ldap3::LdapConnSettings;
use rustls::pki_types::CertificateDer;
use rustls::{ClientConfig, RootCertStore};

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

    if !server.tls.verify {
        // Verification disabled (testing only): accept any certificate. This
        // subsumes any configured CA — once every certificate is accepted the
        // trust anchor is irrelevant — matching the previous native-tls
        // `danger_accept_invalid_certs(true)` behaviour. ldap3 installs its own
        // no-cert verifier on its default config when this flag is set.
        settings = settings.set_no_tls_verify(true);
    } else if let Some(ca_path) = &server.tls.ca_cert {
        // Trust a custom CA: parse the PEM, load it into a RootCertStore, and
        // hand ldap3 a ClientConfig built around it. ldap3 uses a caller-supplied
        // config verbatim, so this is the only branch that builds one.
        let pem = std::fs::read(ca_path)
            .with_context(|| format!("reading CA cert {}", ca_path.display()))?;
        let certs = rustls_pemfile::certs(&mut &pem[..])
            .collect::<std::result::Result<Vec<CertificateDer<'static>>, _>>()
            .with_context(|| format!("parsing CA cert {}", ca_path.display()))?;
        if certs.is_empty() {
            anyhow::bail!("no certificates found in CA cert {}", ca_path.display());
        }
        let mut store = RootCertStore::empty();
        for cert in certs {
            store
                .add(cert)
                .with_context(|| format!("adding CA cert {}", ca_path.display()))?;
        }
        let config = ClientConfig::builder()
            .with_root_certificates(store)
            .with_no_client_auth();
        settings = settings.set_config(Arc::new(config));
    }

    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ServerConfig, TlsConfig};
    use std::io::Write;

    // A self-signed CA, generated offline with
    //   openssl req -x509 -newkey rsa:2048 -nodes -days 36500 \
    //     -subj "/CN=edaptor-test-ca" -keyout /dev/null -out ca.pem
    // Used only to drive the custom-CA branch (parse -> RootCertStore ->
    // ClientConfig::builder -> set_config), which the other tests skip.
    const VALID_CA_PEM: &str = "-----BEGIN CERTIFICATE-----
MIIDFzCCAf+gAwIBAgIUJr70ZihROr85j8WdByc0RI3obicwDQYJKoZIhvcNAQEL
BQAwGjEYMBYGA1UEAwwPZWRhcHRvci10ZXN0LWNhMCAXDTI2MDYwMzIzNDEwOFoY
DzIxMjYwNTEwMjM0MTA4WjAaMRgwFgYDVQQDDA9lZGFwdG9yLXRlc3QtY2EwggEi
MA0GCSqGSIb3DQEBAQUAA4IBDwAwggEKAoIBAQCvvh8SCvaMEVahBRlwK0CfqMeS
RVJw8PkIuKvUWwjmigpli1y5lmq+pOahTTF20aCHkyq6+L2k1zAkQmqUW8hRWpLd
pCH8j1uNo8uFPZZhFrDTJ/aSQhF+ZTjZEFNrm5XVHbJCTL2MUJ/WoAPFL0rszy5i
8J2EyEpoRe+GiWqYQa7TOQ2jI4Q1OsSxdi7ut7kErNmxhUZLOmC2aQTu8fvjzSgS
e4pyAQnVLrtD4Fn0Nfu9tuMH+u7RXZF3dk5cIOEmIM9KqrAa0V7tsg2KTZxk4c1Q
Nsy8NXSdP6+p+Q8EzZ/aBfOlyQnAdUJTRng9J4BQU5gDk5qV4yUpvgGJxq+vAgMB
AAGjUzBRMB0GA1UdDgQWBBQP4cnSJiU8JtMOlvztpyZzsHRCjzAfBgNVHSMEGDAW
gBQP4cnSJiU8JtMOlvztpyZzsHRCjzAPBgNVHRMBAf8EBTADAQH/MA0GCSqGSIb3
DQEBCwUAA4IBAQBTZRUmC+q2rOoJziMBPzAIcf8yONESlAN2dzYNgJFwEF8xZOYk
dcCBSInwr1bHDVc+t5AXZU+H7Th45kdQIUvlc8UTm+1BIje9zb7/ydThyzZZEkax
40h6V1ihwFfvc8FH2gxbdkkcY2xt7QxWymJGF/UM3oHXTApvjpiOuXfWhyfeGkAo
75OVgwUQTmxthrJc5DJ6LcgCEQ+qE8bp3eqi0NEjQox7uw9vw3FKlmakVEAT1mry
Ql5m5Vy9xP0uzl2aVtUGO6B0FrstTlMUQ0yDKwXzx+5ZL8IxJTSd8Bo+5+78ooey
7CqIroe4B39d5saUMPTPVUEAgMn+Ez4qBUPX
-----END CERTIFICATE-----
";

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
            verify: true,
            ..TlsConfig::default()
        };
        let server = server_with_tls(tls, false);
        let err = build_settings(&server).err().unwrap();
        assert!(err.to_string().contains("reading CA cert"), "got: {err}");
    }

    #[test]
    fn garbage_ca_file_yields_no_certificates() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"this is not a certificate").unwrap();
        let tls = TlsConfig {
            ca_cert: Some(f.path().to_path_buf()),
            verify: true,
            ..TlsConfig::default()
        };
        let server = server_with_tls(tls, false);
        let err = build_settings(&server).err().unwrap();
        assert!(
            err.to_string().contains("no certificates found"),
            "got: {err}"
        );
    }

    #[test]
    fn builds_settings_with_valid_custom_ca() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(VALID_CA_PEM.as_bytes()).unwrap();
        let tls = TlsConfig {
            ca_cert: Some(f.path().to_path_buf()),
            verify: true,
            ..TlsConfig::default()
        };
        let server = server_with_tls(tls, false);
        // Drives the full custom-CA path including ClientConfig::builder().
        assert!(build_settings(&server).is_ok());
    }
}
