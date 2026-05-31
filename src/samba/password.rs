//! Synced password mod-set builder + TLS gate (spec §9/§10).
//!
//! `is_secure` is the policy gate: password actions are refused unless the
//! connection is LDAPS or StartTLS. `build_password_mods` produces ONE atomic
//! `MODIFY replace` set — always `userPassword` (cleartext, the server hashes
//! it per `password-hash`/ppolicy), plus `sambaNTPassword` + `sambaPwdLastSet`
//! when the entry is a `sambaSamAccount`, keeping Unix and Samba in sync in a
//! single operation.

use crate::config::ServerConfig;
use crate::form::changeset::ModOp;

use super::nthash::{nt_hash, samba_pwd_last_set};

/// True when the server connection is encrypted: an `ldaps://` URI or StartTLS.
/// Password actions are refused (by the caller) when this is false.
pub fn is_secure(server: &ServerConfig) -> bool {
    server
        .uri
        .trim_start()
        .to_ascii_lowercase()
        .starts_with("ldaps://")
        || server.start_tls
}

/// Build the synced password modify set. Always replaces `userPassword` with the
/// cleartext (the server hashes it). When `is_samba_account`, also replaces
/// `sambaNTPassword` (NT hash) and `sambaPwdLastSet` (now, secs) so the Unix and
/// Samba credentials stay in lockstep within one atomic MODIFY.
pub fn build_password_mods(
    password: &str,
    is_samba_account: bool,
    now_unix_secs: u64,
) -> Vec<ModOp> {
    let mut mods = vec![ModOp::Replace {
        attr: "userPassword".into(),
        values: vec![password.to_string()],
    }];
    if is_samba_account {
        mods.push(ModOp::Replace {
            attr: "sambaNTPassword".into(),
            values: vec![nt_hash(password)],
        });
        mods.push(ModOp::Replace {
            attr: "sambaPwdLastSet".into(),
            values: vec![samba_pwd_last_set(now_unix_secs)],
        });
    }
    mods
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ServerConfig, TlsConfig};

    fn server(uri: &str, start_tls: bool) -> ServerConfig {
        ServerConfig {
            uri: uri.into(),
            base_dn: "dc=example,dc=com".into(),
            start_tls,
            read_only: false,
            timeout_secs: 10,
            tls: TlsConfig::default(),
        }
    }

    #[test]
    fn is_secure_true_for_ldaps() {
        assert!(is_secure(&server("ldaps://ldap.example.com:636", false)));
    }

    #[test]
    fn is_secure_true_for_start_tls() {
        assert!(is_secure(&server("ldap://ldap.example.com:389", true)));
    }

    #[test]
    fn is_secure_false_for_plain_ldap() {
        assert!(!is_secure(&server("ldap://ldap.example.com:389", false)));
    }

    #[test]
    fn non_samba_emits_only_user_password() {
        let mods = build_password_mods("password", false, 1_700_000_000);
        assert_eq!(
            mods,
            vec![ModOp::Replace {
                attr: "userPassword".into(),
                values: vec!["password".into()],
            }]
        );
    }

    #[test]
    fn samba_emits_three_mods_with_exact_nt_hash() {
        let mods = build_password_mods("password", true, 1_700_000_000);
        assert_eq!(
            mods,
            vec![
                ModOp::Replace {
                    attr: "userPassword".into(),
                    values: vec!["password".into()],
                },
                ModOp::Replace {
                    attr: "sambaNTPassword".into(),
                    values: vec!["8846F7EAEE8FB117AD06BDD830B7586C".into()],
                },
                ModOp::Replace {
                    attr: "sambaPwdLastSet".into(),
                    values: vec!["1700000000".into()],
                },
            ]
        );
    }
}
