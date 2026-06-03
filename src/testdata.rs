//! Deterministic test-directory generator (see
//! docs/superpowers/specs/2026-06-03-test-data-and-features-design.md).
//!
//! Pure + deterministic: every value derives from a stable index and the fixed
//! name pools below — no RNG, no clock. Reuses the crate's own Samba logic so
//! generated hashes/SIDs exactly match edaptor's create path.

use std::collections::HashSet;

use crate::samba::account::samba_acct_flags;
use crate::samba::nthash::{nt_hash, samba_pwd_last_set};
use crate::samba::sid::user_sid;

/// Well-known password shared by every generated user.
pub const TEST_PASSWORD: &str = "test123";
/// Precomputed `{SSHA}` of [`TEST_PASSWORD`] (avoids needing an SSHA impl here).
pub const SSHA_PASSWORD: &str = "{SSHA}aFhC+kK5zaFpP9mFw8+o2zC0ir486wTl";
/// Fixed `sambaPwdLastSet` so output is byte-stable (2023-11-14T22:13:20Z).
pub const FIXED_PWD_LAST_SET: u64 = 1_700_000_000;

/// The five departments (round-robin assigned to users).
pub const DEPARTMENTS: [&str; 5] =
    ["Engineering", "Sales", "Marketing", "Finance", "Operations"];

const FIRST_NAMES: [&str; 20] = [
    "James", "Mary", "John", "Patricia", "Robert", "Jennifer", "Michael",
    "Linda", "David", "Elizabeth", "William", "Barbara", "Richard", "Susan",
    "Joseph", "Jessica", "Thomas", "Sarah", "Charles", "Karen",
];
const LAST_NAMES: [&str; 20] = [
    "Smith", "Johnson", "Williams", "Brown", "Jones", "Garcia", "Miller",
    "Davis", "Rodriguez", "Martinez", "Hernandez", "Lopez", "Gonzalez",
    "Wilson", "Anderson", "Thomas", "Taylor", "Moore", "Jackson", "Martin",
];

/// Functional/role groups: (cn, description, every-Nth-user stride).
const FUNCTIONAL_GROUPS: [(&str, &str, usize); 15] = [
    ("all-staff", "All staff members", 1),
    ("managers", "Department managers", 30),
    ("vpn-users", "Remote VPN access", 3),
    ("wifi-users", "Corporate WiFi access", 2),
    ("admins", "System administrators", 100),
    ("helpdesk", "Helpdesk / IT support", 25),
    ("project-apollo", "Project Apollo team", 7),
    ("project-zephyr", "Project Zephyr team", 11),
    ("on-call", "On-call rotation", 40),
    ("finance-approvers", "Finance approval authority", 50),
    ("security-team", "Security response team", 35),
    ("release-managers", "Release management", 60),
    ("interns", "Interns", 45),
    ("board", "Board members", 150),
    ("building-access", "Physical building access", 4),
];

/// Generation options.
pub struct GenOpts {
    pub users: usize,
    pub base_dn: String,
    pub domain_sid: String,
    pub rid_base: u32,
}

impl Default for GenOpts {
    fn default() -> Self {
        Self {
            users: 600,
            base_dn: "dc=example,dc=org".to_string(),
            domain_sid: "S-1-5-21-1234567890-987654321-1122334455".to_string(),
            rid_base: 1000,
        }
    }
}

/// One generated user.
pub struct User {
    pub uid: String,
    pub given: String,
    pub sn: String,
    pub uid_number: u64,
    pub gid_number: u64,
    pub dept_index: usize,
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum GroupKind {
    GroupOfNames,
    PosixGroup,
}

/// One generated group.
pub struct Group {
    pub cn: String,
    pub kind: GroupKind,
    /// member DNs (groupOfNames); empty for posix groups.
    pub members: Vec<String>,
    /// memberUid values (posixGroup); empty for groupOfNames.
    pub member_uids: Vec<String>,
    pub gid_number: Option<u64>,
    pub description: String,
}

/// A fully-resolved dataset, ready to render as LDIF.
pub struct Dataset {
    pub users: Vec<User>,
    pub groups: Vec<Group>,
}

/// gidNumber of a department's posix group: 5000 + index.
pub fn dept_gid(dept_index: usize) -> u64 {
    5000 + dept_index as u64
}

fn user_dn(uid: &str, base_dn: &str) -> String {
    format!("uid={uid},ou=people,{base_dn}")
}

/// Generate the full dataset deterministically.
pub fn generate(opts: &GenOpts) -> Dataset {
    let n_dept = DEPARTMENTS.len();
    let mut users = Vec::with_capacity(opts.users);
    let mut seen: HashSet<String> = HashSet::new();
    for i in 0..opts.users {
        let given = FIRST_NAMES[i % FIRST_NAMES.len()];
        let sn = LAST_NAMES[(i / FIRST_NAMES.len()) % LAST_NAMES.len()];
        let base_uid = format!("{}{}", &given[..1], sn).to_lowercase();
        let mut uid = base_uid.clone();
        let mut k = 1;
        while !seen.insert(uid.clone()) {
            k += 1;
            uid = format!("{base_uid}{k}");
        }
        let dept_index = i % n_dept;
        users.push(User {
            uid,
            given: given.to_string(),
            sn: sn.to_string(),
            uid_number: 10_000 + i as u64,
            gid_number: dept_gid(dept_index),
            dept_index,
        });
    }
    let groups = build_groups(&users, opts);
    Dataset { users, groups }
}

fn build_groups(users: &[User], opts: &GenOpts) -> Vec<Group> {
    let mut groups = Vec::new();
    for (d, dept) in DEPARTMENTS.iter().enumerate() {
        let members: Vec<String> = users
            .iter()
            .filter(|u| u.dept_index == d)
            .map(|u| user_dn(&u.uid, &opts.base_dn))
            .collect();
        let member_uids: Vec<String> = users
            .iter()
            .filter(|u| u.dept_index == d)
            .map(|u| u.uid.clone())
            .collect();
        groups.push(Group {
            cn: dept.to_string(),
            kind: GroupKind::GroupOfNames,
            members,
            member_uids: Vec::new(),
            gid_number: None,
            description: format!("{dept} department members"),
        });
        groups.push(Group {
            cn: format!("{}-unix", dept.to_lowercase()),
            kind: GroupKind::PosixGroup,
            members: Vec::new(),
            member_uids,
            gid_number: Some(dept_gid(d)),
            description: format!("POSIX primary group for {dept}"),
        });
    }
    for (name, desc, stride) in FUNCTIONAL_GROUPS {
        let mut members: Vec<String> = users
            .iter()
            .enumerate()
            .filter(|(idx, _)| idx % stride == 0)
            .map(|(_, u)| user_dn(&u.uid, &opts.base_dn))
            .collect();
        if members.is_empty() && !users.is_empty() {
            members.push(user_dn(&users[0].uid, &opts.base_dn));
        }
        groups.push(Group {
            cn: name.to_string(),
            kind: GroupKind::GroupOfNames,
            members,
            member_uids: Vec::new(),
            gid_number: None,
            description: desc.to_string(),
        });
    }
    groups
}

/// Render the dataset as an LDIF string (users first, then groups, so group
/// `member` DNs resolve and `memberOf` populates on load).
pub fn to_ldif(ds: &Dataset, opts: &GenOpts) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# Generated by `cargo run --bin gen-testdata` — DO NOT EDIT BY HAND.\n\
         # Shared user password: {TEST_PASSWORD}\n\n"
    ));
    for u in &ds.users {
        let dept = DEPARTMENTS[u.dept_index];
        out.push_str(&format!("dn: {}\n", user_dn(&u.uid, &opts.base_dn)));
        out.push_str("objectClass: inetOrgPerson\n");
        out.push_str("objectClass: posixAccount\n");
        out.push_str("objectClass: shadowAccount\n");
        out.push_str("objectClass: sambaSamAccount\n");
        out.push_str(&format!("uid: {}\n", u.uid));
        out.push_str(&format!("cn: {} {}\n", u.given, u.sn));
        out.push_str(&format!("sn: {}\n", u.sn));
        out.push_str(&format!("givenName: {}\n", u.given));
        out.push_str(&format!("mail: {}@example.org\n", u.uid));
        out.push_str(&format!("ou: {dept}\n"));
        out.push_str(&format!("departmentNumber: {}\n", u.dept_index + 1));
        out.push_str(&format!("uidNumber: {}\n", u.uid_number));
        out.push_str(&format!("gidNumber: {}\n", u.gid_number));
        out.push_str(&format!("homeDirectory: /home/{}\n", u.uid));
        out.push_str("loginShell: /bin/bash\n");
        out.push_str(&format!("userPassword: {SSHA_PASSWORD}\n"));
        out.push_str(&format!(
            "sambaSID: {}\n",
            user_sid(&opts.domain_sid, u.uid_number, opts.rid_base)
        ));
        out.push_str(&format!("sambaNTPassword: {}\n", nt_hash(TEST_PASSWORD)));
        out.push_str(&format!(
            "sambaPwdLastSet: {}\n",
            samba_pwd_last_set(FIXED_PWD_LAST_SET)
        ));
        out.push_str(&format!("sambaAcctFlags: {}\n", samba_acct_flags(false)));
        out.push('\n');
    }
    for g in &ds.groups {
        out.push_str(&format!("dn: cn={},ou=groups,{}\n", g.cn, opts.base_dn));
        match g.kind {
            GroupKind::GroupOfNames => {
                out.push_str("objectClass: groupOfNames\n");
                out.push_str(&format!("cn: {}\n", g.cn));
                out.push_str(&format!("description: {}\n", g.description));
                for m in &g.members {
                    out.push_str(&format!("member: {m}\n"));
                }
            }
            GroupKind::PosixGroup => {
                out.push_str("objectClass: posixGroup\n");
                out.push_str(&format!("cn: {}\n", g.cn));
                out.push_str(&format!(
                    "gidNumber: {}\n",
                    g.gid_number.expect("posix group has gid")
                ));
                out.push_str(&format!("description: {}\n", g.description));
                for mu in &g.member_uids {
                    out.push_str(&format!("memberUid: {mu}\n"));
                }
            }
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_counts_match_spec() {
        let ds = generate(&GenOpts::default());
        assert_eq!(ds.users.len(), 600, "600 users");
        // 5 membership + 5 posix + 15 functional = 25
        assert_eq!(ds.groups.len(), 25, "25 groups");
    }

    #[test]
    fn uid_numbers_and_samba_sids_are_unique() {
        let opts = GenOpts::default();
        let ds = generate(&opts);
        let uidnums: HashSet<u64> = ds.users.iter().map(|u| u.uid_number).collect();
        assert_eq!(uidnums.len(), ds.users.len(), "uidNumber unique");
        let sids: HashSet<String> = ds
            .users
            .iter()
            .map(|u| user_sid(&opts.domain_sid, u.uid_number, opts.rid_base))
            .collect();
        assert_eq!(sids.len(), ds.users.len(), "sambaSID unique");
        let uids: HashSet<&str> = ds.users.iter().map(|u| u.uid.as_str()).collect();
        assert_eq!(uids.len(), ds.users.len(), "uid unique");
    }

    #[test]
    fn every_user_gid_matches_a_posix_group() {
        let ds = generate(&GenOpts::default());
        let posix_gids: HashSet<u64> = ds
            .groups
            .iter()
            .filter(|g| g.kind == GroupKind::PosixGroup)
            .map(|g| g.gid_number.unwrap())
            .collect();
        for u in &ds.users {
            assert!(
                posix_gids.contains(&u.gid_number),
                "user {} gid {} has no posix group",
                u.uid,
                u.gid_number
            );
        }
    }

    #[test]
    fn membership_group_members_all_exist() {
        let opts = GenOpts::default();
        let ds = generate(&opts);
        let user_dns: HashSet<String> = ds
            .users
            .iter()
            .map(|u| format!("uid={},ou=people,{}", u.uid, opts.base_dn))
            .collect();
        for g in ds.groups.iter().filter(|g| g.kind == GroupKind::GroupOfNames) {
            assert!(!g.members.is_empty(), "group {} has members", g.cn);
            for m in &g.members {
                assert!(user_dns.contains(m), "group {} member {m} missing", g.cn);
            }
        }
    }

    #[test]
    fn ldif_renders_and_is_deterministic() {
        let opts = GenOpts::default();
        let a = to_ldif(&generate(&opts), &opts);
        let b = to_ldif(&generate(&opts), &opts);
        assert_eq!(a, b, "generation is deterministic");
        assert!(a.contains("objectClass: sambaSamAccount"));
        assert!(a.contains("objectClass: posixGroup"));
    }
}
