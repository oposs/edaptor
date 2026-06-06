//! Samba account attributes (`sambaSamAccount`, spec §9).
//!
//! `samba_acct_flags` renders the fixed-width 13-char flag string. The interior
//! is an 11-wide left-justified field, wrapped in brackets — so the width is
//! structural, never a hand-counted run of spaces. Flag letters are emitted in
//! Samba's canonical `pdb_encode_acct_ctrl` order (`N D H T U M W S L X I`), so
//! a disabled normal account is `[DU         ]` (D before U), matching what a
//! real `sambaSamAccount` entry shows.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use super::nthash::samba_pwd_last_set;
use super::sid::{group_sid, user_sid};
use super::SambaDomainInfo;

/// The 11 Samba ACB flag letters in canonical `pdb_encode_acct_ctrl` order.
/// The interior of `sambaAcctFlags` is exactly this wide (11), which is why a
/// fully-flagged account is `[NDHTUMWSLXI]`.
const ACB_ORDER: [char; 11] = ['N', 'D', 'H', 'T', 'U', 'M', 'W', 'S', 'L', 'X', 'I'];

/// Parse a `sambaAcctFlags` value into the set of present letters. Tolerant of
/// missing brackets; padding spaces are dropped; unknown letters are kept
/// (lossless). Case-sensitive (Samba letters are uppercase).
pub fn parse_bracketed(s: &str) -> BTreeSet<char> {
    let inner = s.trim().strip_prefix('[').unwrap_or(s.trim());
    let inner = inner.strip_suffix(']').unwrap_or(inner);
    inner.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Serialise a letter set to the canonical bracketed form: known letters in
/// `ACB_ORDER`, then any unknown letters (sorted) for losslessness, left-
/// justified to width 11 inside `[`...`]`.
pub fn serialize_bracketed(set: &BTreeSet<char>) -> String {
    let mut letters: String = ACB_ORDER.iter().filter(|c| set.contains(c)).collect();
    let mut unknown: Vec<char> = set
        .iter()
        .copied()
        .filter(|c| !ACB_ORDER.contains(c))
        .collect();
    unknown.sort_unstable();
    letters.extend(unknown);
    format!("[{letters:<11}]")
}

/// Render `sambaAcctFlags`: 13 chars total — `[` + an 11-wide interior + `]`.
/// Enabled normal user → `"[U          ]"` (`U` then 10 spaces); disabled →
/// `"[DU         ]"` (`D` before `U`, then 9 spaces). The interior letters are
/// emitted in Samba's canonical `pdb_encode_acct_ctrl` order and left-justified
/// in an 11-wide field, so the width is structural and the ordering is correct
/// by construction for any future flag combination.
pub fn samba_acct_flags(disabled: bool) -> String {
    let mut set = BTreeSet::new();
    set.insert('U'); // normal user account is always set on create
    if disabled {
        set.insert('D');
    }
    serialize_bracketed(&set)
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

    #[test]
    fn bracketed_round_trips_and_is_canonical() {
        let set = parse_bracketed("[DU         ]");
        assert!(set.contains(&'D') && set.contains(&'U') && set.len() == 2);
        assert_eq!(serialize_bracketed(&set), "[DU         ]");
        let mut s = std::collections::BTreeSet::new();
        s.insert('U');
        s.insert('D');
        assert_eq!(serialize_bracketed(&s), "[DU         ]");
        assert_eq!(serialize_bracketed(&s).len(), 13);
    }

    #[test]
    fn bracketed_is_lossless_for_unmanaged_letters() {
        let set = parse_bracketed("[UXW        ]");
        assert!(set.contains(&'W'));
        assert_eq!(serialize_bracketed(&set), "[UWX        ]"); // canonical: U,W,X
    }

    #[test]
    fn bracketed_tolerates_missing_brackets_and_empty() {
        assert!(parse_bracketed("U").contains(&'U'));
        assert_eq!(
            serialize_bracketed(&std::collections::BTreeSet::new()),
            "[           ]"
        );
    }

    #[test]
    fn samba_acct_flags_golden_unchanged() {
        assert_eq!(samba_acct_flags(false), "[U          ]");
        assert_eq!(samba_acct_flags(true), "[DU         ]");
    }
}
