//! Samba account attributes (`sambaSamAccount`, spec §9).
//!
//! `samba_acct_flags` renders the fixed-width 13-char flag string. The interior
//! is an 11-wide left-justified field (`U` for a normal user, `UD` when
//! disabled), wrapped in brackets — so the width is structural, never a
//! hand-counted run of spaces.

use std::collections::BTreeMap;

use super::nthash::samba_pwd_last_set;
use super::sid::{group_sid, user_sid};
use super::SambaDomainInfo;

/// Render `sambaAcctFlags`: 13 chars total — `[` + an 11-wide interior + `]`.
/// Enabled normal user → `"[U          ]"` (`U` then 10 spaces); disabled →
/// `"[UD         ]"` (`U`,`D` then 9 spaces). The interior is built by
/// left-justifying the flag letters in an 11-wide field so the width is
/// structural, not a hand-counted literal.
pub fn samba_acct_flags(disabled: bool) -> String {
    let letters = if disabled { "UD" } else { "U" };
    format!("[{letters:<11}]")
}

/// Build the `sambaSamAccount` attribute map for a user: `sambaSID`,
/// `sambaPrimaryGroupSID`, `sambaAcctFlags`, `sambaPwdLastSet`, and the
/// `objectClass` value `sambaSamAccount`. SIDs are derived from the domain SID
/// and algorithmic RID base in `domain`.
pub fn build_samba_account_attrs(
    domain: &SambaDomainInfo,
    uid: u64,
    primary_gid: u64,
    disabled: bool,
    now_unix_secs: u64,
) -> BTreeMap<String, Vec<String>> {
    let base = domain.algorithmic_rid_base;
    let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    attrs.insert(
        "sambaSID".into(),
        vec![user_sid(&domain.domain_sid, uid, base)],
    );
    attrs.insert(
        "sambaPrimaryGroupSID".into(),
        vec![group_sid(&domain.domain_sid, primary_gid, base)],
    );
    attrs.insert("sambaAcctFlags".into(), vec![samba_acct_flags(disabled)]);
    attrs.insert(
        "sambaPwdLastSet".into(),
        vec![samba_pwd_last_set(now_unix_secs)],
    );
    attrs.insert("objectClass".into(), vec!["sambaSamAccount".into()]);
    attrs
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOMAIN_SID: &str = "S-1-5-21-1-2-3";

    fn domain() -> SambaDomainInfo {
        SambaDomainInfo {
            domain_sid: DOMAIN_SID.into(),
            algorithmic_rid_base: 1000,
        }
    }

    #[test]
    fn acct_flags_enabled_is_u_padded_to_13() {
        let flags = samba_acct_flags(false);
        assert_eq!(flags, "[U          ]");
        assert_eq!(flags.len(), 13);
    }

    #[test]
    fn acct_flags_disabled_has_ud_padded_to_13() {
        let flags = samba_acct_flags(true);
        assert_eq!(flags, "[UD         ]");
        assert_eq!(flags.len(), 13);
    }

    #[test]
    fn account_attrs_pin_every_value() {
        let attrs = build_samba_account_attrs(&domain(), 1000, 1000, false, 1_700_000_000);
        assert_eq!(attrs["sambaSID"], vec!["S-1-5-21-1-2-3-3000".to_string()]);
        assert_eq!(
            attrs["sambaPrimaryGroupSID"],
            vec!["S-1-5-21-1-2-3-3001".to_string()]
        );
        assert_eq!(attrs["sambaAcctFlags"], vec!["[U          ]".to_string()]);
        assert_eq!(attrs["sambaPwdLastSet"], vec!["1700000000".to_string()]);
        assert_eq!(attrs["objectClass"], vec!["sambaSamAccount".to_string()]);
    }

    #[test]
    fn account_attrs_disabled_sets_flags() {
        let attrs = build_samba_account_attrs(&domain(), 1000, 1000, true, 1_700_000_000);
        assert_eq!(attrs["sambaAcctFlags"], vec!["[UD         ]".to_string()]);
    }
}
