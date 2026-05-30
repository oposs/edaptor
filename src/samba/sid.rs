//! SID/RID algebra + `sambaDomain` discovery (spec §9, lines 306-310).
//!
//! Algorithmic RID base default = 1000. Users get **even** RIDs, groups **odd**:
//! - user RID  = `uidNumber * 2 + rid_base`
//! - group RID = `gidNumber * 2 + rid_base + 1`
//! - `sambaSID = "{domain_sid}-{rid}"`

use std::collections::BTreeMap;

use super::SambaDomainInfo;

/// Algorithmic user RID: `uid * 2 + base` (always even when base is even).
pub fn user_rid(uid: u64, base: u32) -> u64 {
    uid * 2 + base as u64
}

/// Algorithmic group RID: `gid * 2 + base + 1` (one more than the user RID).
pub fn group_rid(gid: u64, base: u32) -> u64 {
    gid * 2 + base as u64 + 1
}

/// Full user SID: `{domain_sid}-{user_rid(uid, base)}`.
pub fn user_sid(domain_sid: &str, uid: u64, base: u32) -> String {
    format!("{domain_sid}-{}", user_rid(uid, base))
}

/// Full group SID: `{domain_sid}-{group_rid(gid, base)}`.
pub fn group_sid(domain_sid: &str, gid: u64, base: u32) -> String {
    format!("{domain_sid}-{}", group_rid(gid, base))
}

/// Parse a discovered `sambaDomain` entry's attribute map into a
/// [`SambaDomainInfo`]. Reads `sambaSID` (required — the domain SID) and
/// `sambaAlgorithmicRidBase` (optional, defaults to 1000 when absent or
/// unparseable). Returns `None` when no `sambaSID` value is present.
pub fn parse_samba_domain(attrs: &BTreeMap<String, Vec<String>>) -> Option<SambaDomainInfo> {
    let domain_sid = attrs.get("sambaSID")?.first()?.clone();
    let algorithmic_rid_base = attrs
        .get("sambaAlgorithmicRidBase")
        .and_then(|v| v.first())
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(1000);
    Some(SambaDomainInfo {
        domain_sid,
        algorithmic_rid_base,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOMAIN: &str = "S-1-5-21-1-2-3";

    #[test]
    fn user_rid_is_even_and_uses_base() {
        assert_eq!(user_rid(0, 1000), 1000);
        assert_eq!(user_rid(1000, 1000), 3000);
    }

    #[test]
    fn group_rid_is_odd_and_one_above_user() {
        assert_eq!(group_rid(0, 1000), 1001);
        assert_eq!(group_rid(1000, 1000), 3001);
    }

    #[test]
    fn user_sid_golden() {
        assert_eq!(user_sid(DOMAIN, 1000, 1000), "S-1-5-21-1-2-3-3000");
    }

    #[test]
    fn group_sid_golden() {
        assert_eq!(group_sid(DOMAIN, 1000, 1000), "S-1-5-21-1-2-3-3001");
    }

    #[test]
    fn parse_samba_domain_round_trips() {
        let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        attrs.insert("sambaSID".into(), vec![DOMAIN.into()]);
        attrs.insert("sambaAlgorithmicRidBase".into(), vec!["1000".into()]);
        let info = parse_samba_domain(&attrs).expect("should parse");
        assert_eq!(info.domain_sid, DOMAIN);
        assert_eq!(info.algorithmic_rid_base, 1000);
    }

    #[test]
    fn parse_samba_domain_defaults_base_when_absent() {
        let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        attrs.insert("sambaSID".into(), vec![DOMAIN.into()]);
        let info = parse_samba_domain(&attrs).expect("should parse");
        assert_eq!(info.algorithmic_rid_base, 1000);
    }

    #[test]
    fn parse_samba_domain_none_without_sid() {
        let attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        assert!(parse_samba_domain(&attrs).is_none());
    }
}
