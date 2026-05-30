//! Samba account attributes (`sambaSamAccount`, spec §9).
//!
//! `samba_acct_flags` renders the fixed-width 13-char flag string. The interior
//! is an 11-wide left-justified field, wrapped in brackets — so the width is
//! structural, never a hand-counted run of spaces. Flag letters are emitted in
//! Samba's canonical `pdb_encode_acct_ctrl` order (`N D H T U M W S L X I`), so
//! a disabled normal account is `[DU         ]` (D before U), matching what a
//! real `sambaSamAccount` entry shows.

use std::collections::BTreeMap;

use super::nthash::samba_pwd_last_set;
use super::sid::{group_sid, user_sid};
use super::SambaDomainInfo;

/// Render `sambaAcctFlags`: 13 chars total — `[` + an 11-wide interior + `]`.
/// Enabled normal user → `"[U          ]"` (`U` then 10 spaces); disabled →
/// `"[DU         ]"` (`D` before `U`, then 9 spaces). The interior letters are
/// emitted in Samba's canonical `pdb_encode_acct_ctrl` order and left-justified
/// in an 11-wide field, so the width is structural and the ordering is correct
/// by construction for any future flag combination.
pub fn samba_acct_flags(disabled: bool) -> String {
    // (ACB bit letter, present?) pairs in Samba's canonical encode order
    // `N D H T U M W S L X I`. Only the flags M5 sets are wired; the rest are
    // placeholders kept in order so adding them later stays canonical.
    let order: [(char, bool); 2] = [('D', disabled), ('U', true)];
    let letters: String = order
        .iter()
        .filter(|(_, on)| *on)
        .map(|(c, _)| *c)
        .collect();
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
    fn acct_flags_disabled_has_du_in_canonical_order_padded_to_13() {
        // Samba's pdb_encode_acct_ctrl emits D (ACB_DISABLED) before U
        // (ACB_NORMAL): a disabled normal account shows `[DU         ]`.
        let flags = samba_acct_flags(true);
        assert_eq!(flags, "[DU         ]");
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
        assert_eq!(attrs["sambaAcctFlags"], vec!["[DU         ]".to_string()]);
    }
}
