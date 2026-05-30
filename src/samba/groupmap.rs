//! Samba group mapping attributes (`sambaGroupMapping`, spec §9).
//!
//! Maps a POSIX group onto a Samba domain group: `sambaSID` (= the group SID),
//! `gidNumber`, `sambaGroupType = "2"` (SID_NAME_DOM_GRP / domain group),
//! `displayName` (the group cn), plus the `objectClass` value
//! `sambaGroupMapping`.

use std::collections::BTreeMap;

use super::sid::group_sid;
use super::SambaDomainInfo;

/// `sambaGroupType` for a domain group (SID_NAME_DOM_GRP).
const SAMBA_GROUP_TYPE_DOMAIN: &str = "2";

/// Build the `sambaGroupMapping` attribute map for a POSIX group: `sambaSID`
/// (derived from the domain SID + algorithmic RID base), `gidNumber`,
/// `sambaGroupType`, `displayName`, and the `objectClass` value
/// `sambaGroupMapping`.
pub fn build_group_mapping_attrs(
    domain: &SambaDomainInfo,
    gid: u64,
    display_name: &str,
) -> BTreeMap<String, Vec<String>> {
    let base = domain.algorithmic_rid_base;
    let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    attrs.insert(
        "sambaSID".into(),
        vec![group_sid(&domain.domain_sid, gid, base)],
    );
    attrs.insert("gidNumber".into(), vec![gid.to_string()]);
    attrs.insert(
        "sambaGroupType".into(),
        vec![SAMBA_GROUP_TYPE_DOMAIN.into()],
    );
    attrs.insert("displayName".into(), vec![display_name.to_string()]);
    attrs.insert("objectClass".into(), vec!["sambaGroupMapping".into()]);
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
    fn group_mapping_pins_every_value() {
        let attrs = build_group_mapping_attrs(&domain(), 1000, "admins");
        assert_eq!(attrs["sambaSID"], vec![group_sid(DOMAIN_SID, 1000, 1000)]);
        assert_eq!(attrs["sambaSID"], vec!["S-1-5-21-1-2-3-3001".to_string()]);
        assert_eq!(attrs["sambaGroupType"], vec!["2".to_string()]);
        assert_eq!(attrs["gidNumber"], vec!["1000".to_string()]);
        assert_eq!(attrs["displayName"], vec!["admins".to_string()]);
        assert_eq!(attrs["objectClass"], vec!["sambaGroupMapping".to_string()]);
    }
}
