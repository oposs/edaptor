//! The private-group predicate (spec §2B2): posixGroup G is the private group
//! of posixAccount U when G.cn = U.uid and G.gidNumber = U.gidNumber = U.uidNumber.

use std::collections::HashMap;

use crate::detect::model::SampleEntry;

fn num(e: &SampleEntry, attr: &str) -> Option<u64> {
    e.first(attr)?.parse().ok()
}

pub fn is_private_group(group: &SampleEntry, user: &SampleEntry) -> bool {
    if !group.has_class("posixGroup") || !user.has_class("posixAccount") {
        return false;
    }
    let (Some(cn), Some(uid)) = (group.first("cn"), user.first("uid")) else {
        return false;
    };
    if !cn.eq_ignore_ascii_case(uid) {
        return false;
    }
    match (
        num(group, "gidNumber"),
        num(user, "gidNumber"),
        num(user, "uidNumber"),
    ) {
        (Some(g), Some(ug), Some(uu)) => g == ug && ug == uu,
        _ => false,
    }
}

/// Lookup tables for the predicate in both directions.
pub struct PrivateIndex<'a> {
    users_by_uid: HashMap<String, Vec<&'a SampleEntry>>,
    groups_by_cn: HashMap<String, Vec<&'a SampleEntry>>,
}

impl<'a> PrivateIndex<'a> {
    pub fn new(entries: impl IntoIterator<Item = &'a SampleEntry>) -> Self {
        let mut users_by_uid: HashMap<String, Vec<&'a SampleEntry>> = HashMap::new();
        let mut groups_by_cn: HashMap<String, Vec<&'a SampleEntry>> = HashMap::new();
        for e in entries {
            if e.has_class("posixAccount") {
                if let Some(uid) = e.first("uid") {
                    users_by_uid.entry(uid.to_lowercase()).or_default().push(e);
                }
            }
            if e.has_class("posixGroup") {
                if let Some(cn) = e.first("cn") {
                    groups_by_cn.entry(cn.to_lowercase()).or_default().push(e);
                }
            }
        }
        PrivateIndex {
            users_by_uid,
            groups_by_cn,
        }
    }

    pub fn private_group_of(&self, user: &SampleEntry) -> Option<&'a SampleEntry> {
        let uid = user.first("uid")?.to_lowercase();
        self.groups_by_cn
            .get(&uid)?
            .iter()
            .copied()
            .find(|g| is_private_group(g, user))
    }

    pub fn is_private(&self, group: &SampleEntry) -> bool {
        let Some(cn) = group.first("cn") else {
            return false;
        };
        self.users_by_uid
            .get(&cn.to_lowercase())
            .is_some_and(|us| us.iter().any(|u| is_private_group(group, u)))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::fixtures::e;

    fn user(uid: &str, cn: &str, num: &str, gid: &str) -> SampleEntry {
        e(
            &format!("cn={cn},ou=people,dc=x"),
            &[
                ("objectClass", &["posixAccount"]),
                ("uid", &[uid]),
                ("cn", &[cn]),
                ("uidNumber", &[num]),
                ("gidNumber", &[gid]),
            ],
        )
    }
    fn group(cn: &str, gid: &str) -> SampleEntry {
        e(
            &format!("cn={cn},ou=groups,dc=x"),
            &[
                ("objectClass", &["posixGroup"]),
                ("cn", &[cn]),
                ("gidNumber", &[gid]),
            ],
        )
    }

    #[test]
    fn predicate_needs_name_and_all_three_numbers() {
        assert!(is_private_group(
            &group("alice", "7000"),
            &user("alice", "Alice Smith", "7000", "7000")
        ));
        assert!(is_private_group(
            &group("ALICE", "7000"),
            &user("alice", "x", "7000", "7000")
        ));
        assert!(!is_private_group(
            &group("alice", "7000"),
            &user("alice", "x", "7000", "100")
        ));
        assert!(!is_private_group(
            &group("alice", "7001"),
            &user("alice", "x", "7000", "7000")
        ));
        assert!(!is_private_group(
            &group("bob", "7000"),
            &user("alice", "x", "7000", "7000")
        ));
    }

    #[test]
    fn index_answers_both_directions() {
        let all = [
            user("alice", "Alice", "7000", "7000"),
            group("alice", "7000"),
            group("staff", "100"),
        ];
        let idx = PrivateIndex::new(all.iter());
        assert_eq!(
            idx.private_group_of(&all[0]).map(|g| g.dn.as_str()),
            Some("cn=alice,ou=groups,dc=x")
        );
        assert!(idx.is_private(&all[1]));
        assert!(!idx.is_private(&all[2]));
    }
}
