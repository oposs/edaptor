# Test Data & ansible-Feature Parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the local podman LDAP test server the schemas/overlays/ppolicy the `oposs.openldap` ansible role deploys, plus a deterministic generator that seeds ~600 users across 5 departments and ~25 groups, and a demo edaptor config pointing at it.

**Architecture:** Keep the Bitnami image and `dc=example,dc=org` (existing live tests untouched). After startup, `scripts/test-ldap.sh` replays the role's config via the `cn=config` admin (schemas + overlays) then loads vendored data LDIFs as the data admin. A Rust `[[bin]]` (`gen-testdata`) reuses the crate's own `samba::{nthash,sid,account}` to emit a committed, regenerable `testdata.ldif`.

**Tech Stack:** Rust (edaptor crate, `clap`, `anyhow`, `md4`), OpenLDAP (`olcSchemaConfig`/`cn=config`), Bitnami `openldap:2.6.9`, podman, bash.

**Spec:** `docs/superpowers/specs/2026-06-03-test-data-and-features-design.md`

**Validated facts (from a spike — do not re-derive):**
- Enable cn=config writes with env `LDAP_CONFIG_ADMIN_ENABLED=yes` + `LDAP_CONFIG_ADMIN_USERNAME=admin` + `LDAP_CONFIG_ADMIN_PASSWORD=configpassword`; bind `-x -D cn=admin,cn=config -w configpassword`.
- Overlay `.so`s live in `/opt/bitnami/openldap/lib/openldap` (the default `cn=module{0}` path is wrong); add a fresh `cn=module{1}`.
- Data backend DN is `olcDatabase={2}mdb,cn=config`.
- `samba.ldif` (role's `files/samba.ldif`) is already `olcSchemaConfig` and loads as-is.
- `memberOf` auto-populates on users when a `groupOfNames` is added after the overlay is configured.

---

## File Structure

Created:
- `scripts/ldap-provision/README.md` — what each asset is, provenance, regen command, shared password.
- `scripts/ldap-provision/schema/samba.ldif` — verbatim copy of role's `files/samba.ldif`.
- `scripts/ldap-provision/schema/mail.ldif` — hand-written `olcSchemaConfig` form of role's `files/schemas/mail.schema`.
- `scripts/ldap-provision/config/overlays.ldif` — module{1} + memberof/refint/ppolicy overlays on `{2}mdb`.
- `scripts/ldap-provision/data/base.ldif` — `ou=people/groups/services`, `sambaDomain`, service accounts.
- `scripts/ldap-provision/data/ppolicy.ldif` — `ou=policies` + default/serviceaccounts policies.
- `scripts/ldap-provision/data/testdata.ldif` — GENERATED, committed.
- `src/testdata.rs` — pure deterministic generator (lib module).
- `src/bin/gen-testdata.rs` — CLI wrapper that writes the LDIF.
- `examples/demo-config.toml` — edaptor config for this server.
- `tests/live_seed.rs` — gated live assertions on the seeded directory.

Modified:
- `src/lib.rs` — add `pub mod testdata;`.
- `Cargo.toml` — add `[[bin]] gen-testdata`.
- `scripts/test-ldap.sh` — config-admin env + post-start provisioning.
- `README.md` — mention the rich test server + demo config.

**Shared test password (used everywhere a `userPassword` is needed):** `test123`, whose `{SSHA}` literal is `{SSHA}aFhC+kK5zaFpP9mFw8+o2zC0ir486wTl` (taken from the role's molecule test data; verified by the role's own auth tests).

---

## Task 1: Generator core (`src/testdata.rs`) — pure data model + `generate`

**Files:**
- Create: `src/testdata.rs`
- Modify: `src/lib.rs` (add `pub mod testdata;` after the existing `pub mod samba;` line)

- [ ] **Step 1: Register the module**

In `src/lib.rs`, add the line (keep alphabetical-ish with the others):

```rust
pub mod testdata;
```

- [ ] **Step 2: Write `src/testdata.rs` with the model, generation, and unit tests**

```rust
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
```

- [ ] **Step 3: Run the generator tests**

Run: `cargo test --lib testdata:: -- --nocapture`
Expected: 5 tests pass (`default_counts_match_spec`, `uid_numbers_and_samba_sids_are_unique`, `every_user_gid_matches_a_posix_group`, `membership_group_members_all_exist`, `ldif_renders_and_is_deterministic`).

- [ ] **Step 4: Commit**

```bash
git add src/lib.rs src/testdata.rs
git commit -m "feat: deterministic test-directory generator (testdata module)"
```

---

## Task 2: Generator CLI (`src/bin/gen-testdata.rs`) + commit generated LDIF

**Files:**
- Create: `src/bin/gen-testdata.rs`
- Modify: `Cargo.toml` (add a second `[[bin]]`)
- Create: `scripts/ldap-provision/data/testdata.ldif` (generated output)

- [ ] **Step 1: Add the bin target to `Cargo.toml`**

After the existing `[[bin]] name = "edaptor"` block, add:

```toml
[[bin]]
name = "gen-testdata"
path = "src/bin/gen-testdata.rs"
```

- [ ] **Step 2: Write the CLI**

```rust
//! Generate the edaptor test-directory LDIF (deterministic).
//! Default output: scripts/ldap-provision/data/testdata.ldif

use std::io::Write;

use clap::Parser;
use edaptor::testdata::{generate, to_ldif, GenOpts};

#[derive(Parser)]
#[command(about = "Generate the edaptor test-directory LDIF (deterministic).")]
struct Cli {
    /// Number of users to generate.
    #[arg(long, default_value_t = 600)]
    users: usize,
    /// Output path, or '-' for stdout.
    #[arg(long, default_value = "scripts/ldap-provision/data/testdata.ldif")]
    out: String,
    /// Base DN for all entries.
    #[arg(long, default_value = "dc=example,dc=org")]
    base_dn: String,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let opts = GenOpts {
        users: cli.users,
        base_dn: cli.base_dn,
        ..Default::default()
    };
    let ldif = to_ldif(&generate(&opts), &opts);
    if cli.out == "-" {
        std::io::stdout().write_all(ldif.as_bytes())?;
    } else {
        std::fs::write(&cli.out, &ldif)?;
        eprintln!("wrote {} ({} bytes)", cli.out, ldif.len());
    }
    Ok(())
}
```

- [ ] **Step 3: Build the bin**

Run: `cargo build --bin gen-testdata`
Expected: compiles clean.

- [ ] **Step 4: Generate the committed LDIF**

Run (the data dir is created in Task 3; create it now so the write succeeds):
```bash
mkdir -p scripts/ldap-provision/data
cargo run --bin gen-testdata
```
Expected: `wrote scripts/ldap-provision/data/testdata.ldif (… bytes)`.

- [ ] **Step 5: Sanity-check the output**

Run:
```bash
grep -c '^dn: uid=' scripts/ldap-provision/data/testdata.ldif   # expect 600
grep -c '^dn: cn=' scripts/ldap-provision/data/testdata.ldif    # expect 25
grep -c 'objectClass: posixGroup' scripts/ldap-provision/data/testdata.ldif  # expect 5
```
Expected: `600`, `25`, `5`.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml src/bin/gen-testdata.rs scripts/ldap-provision/data/testdata.ldif
git commit -m "feat: gen-testdata CLI + committed testdata.ldif (600 users, 25 groups)"
```

---

## Task 3: Vendored schema + config + static-data LDIFs

**Files:**
- Create: `scripts/ldap-provision/schema/samba.ldif`
- Create: `scripts/ldap-provision/schema/mail.ldif`
- Create: `scripts/ldap-provision/config/overlays.ldif`
- Create: `scripts/ldap-provision/data/base.ldif`
- Create: `scripts/ldap-provision/data/ppolicy.ldif`

- [ ] **Step 1: Copy the Samba schema verbatim**

Run:
```bash
mkdir -p scripts/ldap-provision/schema scripts/ldap-provision/config
cp ~/checkouts/oep-ansible/playbooks/config.d/roles/oposs.openldap/files/samba.ldif \
   scripts/ldap-provision/schema/samba.ldif
```
Then prepend a provenance header as the first lines of `scripts/ldap-provision/schema/samba.ldif` (use an editor; do not alter the `dn:`/`olc*` lines):
```
# Samba schema (olcSchemaConfig). Provenance: oposs.openldap role
# files/samba.ldif. Loaded via the cn=config admin. DO NOT reformat the
# folded olcAttributeTypes continuation lines.
```

- [ ] **Step 2: Write `scripts/ldap-provision/schema/mail.ldif`**

```
# Mail schema (olcSchemaConfig). Provenance: oposs.openldap role
# files/schemas/mail.schema. Loaded via the cn=config admin.
dn: cn=mail,cn=schema,cn=config
objectClass: olcSchemaConfig
cn: mail
olcAttributeTypes: ( 1.1.2.1.1.1 NAME 'mailQuota' DESC 'Mail quota in bytes' EQUALITY integerMatch SYNTAX 1.3.6.1.4.1.1466.115.121.1.27 SINGLE-VALUE )
olcAttributeTypes: ( 1.1.2.1.1.2 NAME 'mailAlias' DESC 'Mail alias addresses' EQUALITY caseIgnoreIA5Match SUBSTR caseIgnoreIA5SubstringsMatch SYNTAX 1.3.6.1.4.1.1466.115.121.1.26{256} )
olcAttributeTypes: ( 1.1.2.1.1.3 NAME 'mailEnabled' DESC 'Mail account enabled flag' EQUALITY booleanMatch SYNTAX 1.3.6.1.4.1.1466.115.121.1.7 SINGLE-VALUE )
olcObjectClasses: ( 1.1.2.1.2.1 NAME 'mailAccount' DESC 'Mail account object class' SUP top AUXILIARY MUST ( mail ) MAY ( mailQuota $ mailAlias $ mailEnabled ) )
```

- [ ] **Step 3: Write `scripts/ldap-provision/config/overlays.ldif`**

```
# Overlays for the edaptor test server (mirrors the oposs.openldap role).
# NOTE: the overlay .so files are in /opt/bitnami/openldap/lib/openldap — the
# default cn=module{0} olcModulePath points at libexec/ and FAILS to load them,
# so we add a fresh module{1} with the correct path. Applied via cn=config admin.

dn: cn=module{1},cn=config
objectClass: olcModuleList
cn: module{1}
olcModulePath: /opt/bitnami/openldap/lib/openldap
olcModuleLoad: memberof.so
olcModuleLoad: refint.so
olcModuleLoad: ppolicy.so

dn: olcOverlay=memberof,olcDatabase={2}mdb,cn=config
objectClass: olcOverlayConfig
objectClass: olcMemberOf
olcOverlay: memberof
olcMemberOfRefInt: TRUE
olcMemberOfDangling: ignore
olcMemberOfGroupOC: groupOfNames
olcMemberOfMemberAD: member
olcMemberOfMemberOfAD: memberOf

dn: olcOverlay=refint,olcDatabase={2}mdb,cn=config
objectClass: olcOverlayConfig
objectClass: olcRefintConfig
olcOverlay: refint
olcRefintAttribute: member memberOf owner manager

dn: olcOverlay=ppolicy,olcDatabase={2}mdb,cn=config
objectClass: olcOverlayConfig
objectClass: olcPPolicyConfig
olcOverlay: ppolicy
olcPPolicyDefault: cn=default,ou=policies,dc=example,dc=org
olcPPolicyHashCleartext: TRUE
olcPPolicyUseLockout: TRUE
```

- [ ] **Step 4: Write `scripts/ldap-provision/data/ppolicy.ldif`**

```
# Password policies (mirrors the role's ppolicy_default.ldif.j2). Loaded as the
# data admin. Must load before group/user data is irrelevant — only the ppolicy
# overlay's olcPPolicyDefault references cn=default here.
dn: ou=policies,dc=example,dc=org
objectClass: organizationalUnit
ou: policies

dn: cn=default,ou=policies,dc=example,dc=org
objectClass: pwdPolicy
objectClass: person
objectClass: top
cn: default
sn: default
pwdAttribute: userPassword
pwdMinLength: 8
pwdMaxAge: 0
pwdInHistory: 5
pwdCheckQuality: 1
pwdMinAge: 0
pwdMaxFailure: 5
pwdLockout: TRUE
pwdLockoutDuration: 1800
pwdGraceAuthNLimit: 3
pwdExpireWarning: 604800
pwdAllowUserChange: TRUE
pwdSafeModify: FALSE

dn: cn=serviceaccounts,ou=policies,dc=example,dc=org
objectClass: pwdPolicy
objectClass: person
objectClass: top
cn: serviceaccounts
sn: serviceaccounts
pwdAttribute: userPassword
pwdMinLength: 8
pwdMaxAge: 0
pwdInHistory: 5
pwdCheckQuality: 1
pwdMinAge: 0
pwdMaxFailure: 5
pwdLockout: TRUE
pwdLockoutDuration: 1800
pwdGraceAuthNLimit: 0
pwdExpireWarning: 0
pwdAllowUserChange: FALSE
pwdSafeModify: FALSE
```

- [ ] **Step 5: Write `scripts/ldap-provision/data/base.ldif`**

```
# Base OUs, the sambaDomain entry, and representative service accounts.
# Loaded as the data admin AFTER the samba schema is in cn=config.
dn: ou=people,dc=example,dc=org
objectClass: organizationalUnit
ou: people
description: User accounts

dn: ou=groups,dc=example,dc=org
objectClass: organizationalUnit
ou: groups
description: Groups

dn: ou=services,dc=example,dc=org
objectClass: organizationalUnit
ou: services
description: Service accounts and LDAP bind accounts

# Samba domain — edaptor discovers the domain SID + RID base from this entry.
dn: sambaDomainName=EXAMPLE,dc=example,dc=org
objectClass: sambaDomain
objectClass: top
sambaDomainName: EXAMPLE
sambaSID: S-1-5-21-1234567890-987654321-1122334455
sambaAlgorithmicRidBase: 1000

# Representative service accounts (illustrative; the functional rootDN is
# cn=admin, which the demo config binds as). Password: test123
dn: cn=sambamanager,ou=services,dc=example,dc=org
objectClass: simpleSecurityObject
objectClass: organizationalRole
cn: sambamanager
description: Samba service account
userPassword: {SSHA}aFhC+kK5zaFpP9mFw8+o2zC0ir486wTl

dn: cn=nssclient,ou=services,dc=example,dc=org
objectClass: simpleSecurityObject
objectClass: organizationalRole
cn: nssclient
description: NSS/PAM POSIX lookup account
userPassword: {SSHA}aFhC+kK5zaFpP9mFw8+o2zC0ir486wTl

dn: cn=ldapmanager,ou=services,dc=example,dc=org
objectClass: simpleSecurityObject
objectClass: organizationalRole
cn: ldapmanager
description: LDAP management account placeholder
userPassword: {SSHA}aFhC+kK5zaFpP9mFw8+o2zC0ir486wTl
```

- [ ] **Step 6: Commit (verification of these LDIFs happens in Task 6)**

```bash
git add scripts/ldap-provision/schema scripts/ldap-provision/config scripts/ldap-provision/data/base.ldif scripts/ldap-provision/data/ppolicy.ldif
git commit -m "feat: vendored schema/overlay/ppolicy/base LDIFs for the test server"
```

---

## Task 4: Wire provisioning into `scripts/test-ldap.sh`

**Files:**
- Modify: `scripts/test-ldap.sh`

- [ ] **Step 1: Add the config-admin env to the `podman run`**

In the `start)` case, inside the `podman run -d --rm ...` invocation, add three `-e` flags right after the `LDAP_ADMIN_PASSWORD` line:

```bash
      -e LDAP_ADMIN_PASSWORD="adminpassword" \
      -e LDAP_CONFIG_ADMIN_ENABLED="yes" \
      -e LDAP_CONFIG_ADMIN_USERNAME="admin" \
      -e LDAP_CONFIG_ADMIN_PASSWORD="configpassword" \
```

- [ ] **Step 2: Add a `provision` helper and call it after readiness**

Immediately after the `NAME=edaptor-test-ldap` / `IMAGE=...` lines (top-level, before `case`), add:

```bash
PROVISION_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/ldap-provision" && pwd)"

apply_ldif() {  # <bind-dn> <password> <file>
  local bind_dn=$1 pw=$2 file=$3
  podman cp "$file" "$NAME:/tmp/$(basename "$file")"
  # -c: continue past entries that already exist (e.g. on a warm DB)
  podman exec "$NAME" ldapadd -c -x -H ldap://localhost:1389 \
    -D "$bind_dn" -w "$pw" -f "/tmp/$(basename "$file")"
}

provision() {
  echo "Provisioning schemas + overlays (cn=config admin)..."
  apply_ldif "cn=admin,cn=config" "configpassword" "$PROVISION_DIR/schema/samba.ldif"
  apply_ldif "cn=admin,cn=config" "configpassword" "$PROVISION_DIR/schema/mail.ldif"
  apply_ldif "cn=admin,cn=config" "configpassword" "$PROVISION_DIR/config/overlays.ldif"
  echo "Loading directory data (data admin)..."
  apply_ldif "cn=admin,dc=example,dc=org" "adminpassword" "$PROVISION_DIR/data/ppolicy.ldif"
  apply_ldif "cn=admin,dc=example,dc=org" "adminpassword" "$PROVISION_DIR/data/base.ldif"
  apply_ldif "cn=admin,dc=example,dc=org" "adminpassword" "$PROVISION_DIR/data/testdata.ldif"
}
```

- [ ] **Step 3: Invoke `provision` on the ready path**

In the readiness loop, replace the success block so it provisions before printing the hints. Change:

```bash
        echo "Ready."
        echo "  export EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389"
        echo "  export EDAPTOR_TEST_ADMIN_PW=adminpassword"
        exit 0
```

to:

```bash
        echo "Ready."
        provision
        echo "Provisioned. Connection hints:"
        echo "  export EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389"
        echo "  export EDAPTOR_TEST_ADMIN_PW=adminpassword"
        echo "  edaptor --config examples/demo-config.toml   # explore the seed data"
        exit 0
```

- [ ] **Step 4: Lint the script**

Run: `bash -n scripts/test-ldap.sh`
Expected: no output (syntax OK).

- [ ] **Step 5: Commit**

```bash
git add scripts/test-ldap.sh
git commit -m "feat: provision schemas/overlays/seed data in test-ldap.sh start"
```

---

## Task 5: Demo config (`examples/demo-config.toml`)

**Files:**
- Create: `examples/demo-config.toml`

- [ ] **Step 1: Write the demo config**

```toml
# edaptor demo config — points at the rich podman test server started by
# `scripts/test-ldap.sh start`. Bind password is `adminpassword`, exposed via
# the EDAPTOR_TEST_ADMIN_PW env var the script tells you to export.

[server]
uri          = "ldap://localhost:1389"
base_dn      = "dc=example,dc=org"
start_tls    = false
timeout_secs = 10

[auth]
method          = "simple"
bind_dn         = "cn=admin,dc=example,dc=org"
password_source = "env:EDAPTOR_TEST_ADMIN_PW"

# Samba domain SID fallback; edaptor also discovers it from the
# sambaDomainName=EXAMPLE entry in the directory.
[samba]
domain_sid           = "S-1-5-21-1234567890-987654321-1122334455"
algorithmic_rid_base = 1000

[[profile]]
name           = "user"
object_classes = ["inetOrgPerson", "posixAccount", "shadowAccount", "sambaSamAccount"]
rdn_attr       = "uid"
search_base    = "ou=people,dc=example,dc=org"
show           = ["uid", "cn", "sn", "givenName", "mail", "ou", "uidNumber", "gidNumber", "homeDirectory"]
search_attrs   = ["cn", "uid", "mail"]
label          = "{cn} ({uid})"

[profile.defaults]
loginShell    = "/bin/bash"
homeDirectory = "/home/{uid}"
uidNumber     = "{next:10000-60000}"

[profile.password]
ldap_attribute = "userPassword"
samba          = true

[profile.lookup.gidNumber]
object_class = "posixGroup"
search_base  = "ou=groups,dc=example,dc=org"
value_attr   = "gidNumber"
label        = "cn"
search_attrs = ["cn"]

[[profile]]
name           = "group"
object_classes = ["groupOfNames"]
rdn_attr       = "cn"
search_base    = "ou=groups,dc=example,dc=org"
show           = ["cn", "description"]
label          = "{cn}"

[[relation]]
name        = "group-membership"
holder      = "group"
holder_attr = "member"
candidate   = "user"
back_attr   = "memberOf"
```

- [ ] **Step 2: Verify it parses**

Add a temporary throwaway check that the crate's config loader accepts the file. Run:
```bash
cargo test --lib config:: 2>&1 | tail -5
```
Then confirm the file is valid TOML:
```bash
python3 -c "import tomllib,sys; tomllib.load(open('examples/demo-config.toml','rb')); print('toml ok')"
```
Expected: existing config tests pass; `toml ok`.

- [ ] **Step 3: Commit**

```bash
git add examples/demo-config.toml
git commit -m "feat: demo edaptor config for the rich test server"
```

---

## Task 6: End-to-end provisioning smoke test (manual, real container)

This task verifies the vendored LDIFs and the script actually load against a real Bitnami container. No code; it gates correctness of Tasks 3–5.

**Files:** none.

- [ ] **Step 1: Start the provisioned server**

Run: `scripts/test-ldap.sh start`
Expected: ends with `Ready.` → `Provisioning…` → `Loading directory data…` → `Provisioned.` with no `ldap_add`/`ldap_modify` errors. If `mail.ldif` is rejected, fix the offending `olcAttributeTypes` line and re-run.

- [ ] **Step 2: Verify schemas, overlays, and memberOf**

Run:
```bash
# schemas present
podman exec edaptor-test-ldap ldapsearch -x -H ldap://localhost:1389 \
  -D cn=admin,cn=config -w configpassword -b cn=schema,cn=config cn \
  | grep -iE 'cn: (samba|mail)'
# overlays present
podman exec edaptor-test-ldap ldapsearch -x -H ldap://localhost:1389 \
  -D cn=admin,cn=config -w configpassword -b 'olcDatabase={2}mdb,cn=config' \
  '(objectClass=olcOverlayConfig)' olcOverlay
# memberOf populated on a seeded user (pick any uid from testdata.ldif)
FIRST_UID=$(grep -m1 '^dn: uid=' scripts/ldap-provision/data/testdata.ldif | sed 's/^dn: //')
podman exec edaptor-test-ldap ldapsearch -x -H ldap://localhost:1389 \
  -D cn=admin,dc=example,dc=org -w adminpassword -b "$FIRST_UID" memberOf
```
Expected: `cn: samba` + `cn: mail`; three overlays (`memberof`, `refint`, `ppolicy`); at least one `memberOf:` line on the user (they are in their dept group + `all-staff`).

- [ ] **Step 3: Verify counts + the gidNumber-lookup source**

Run:
```bash
podman exec edaptor-test-ldap ldapsearch -x -LLL -H ldap://localhost:1389 \
  -D cn=admin,dc=example,dc=org -w adminpassword \
  -b ou=people,dc=example,dc=org '(objectClass=inetOrgPerson)' dn \
  | grep -c '^dn:'
podman exec edaptor-test-ldap ldapsearch -x -LLL -H ldap://localhost:1389 \
  -D cn=admin,dc=example,dc=org -w adminpassword \
  -b ou=groups,dc=example,dc=org '(objectClass=posixGroup)' gidNumber
```
Expected: user count is 600 (or the paged total; if the default size limit truncates at 500, that is expected and the live test in Task 7 uses the paged scan); five posixGroups each with a `gidNumber` (5000–5004).

- [ ] **Step 4: Leave the server running for Task 7, then note status**

No commit. If anything failed, fix the relevant LDIF/script from Tasks 3–4, `scripts/test-ldap.sh stop`, and re-run this task.

---

## Task 7: Gated live seed test (`tests/live_seed.rs`)

**Files:**
- Create: `tests/live_seed.rs`

- [ ] **Step 1: Write the test**

```rust
//! Live test (gated by EDAPTOR_TEST_LDAP_URI): the rich seed data loaded by
//! scripts/test-ldap.sh is present and well-formed. SKIPS cleanly when unset.

use edaptor::config::{
    AuthConfig, AuthMethod, Config, PasswordSource, ServerConfig, TlsConfig,
};
use edaptor::ldap::worker::{Request, Response, SearchScope, WorkerHandle};

const BASE: &str = "dc=example,dc=org";

fn test_config(uri: String) -> (Config, String) {
    let config = Config {
        server: ServerConfig {
            uri,
            base_dn: BASE.to_string(),
            start_tls: false,
            read_only: false,
            timeout_secs: 10,
            tls: TlsConfig::default(),
        },
        auth: AuthConfig {
            method: AuthMethod::Simple,
            bind_dn: Some(format!("cn=admin,{BASE}")),
            password_source: PasswordSource::Env("EDAPTOR_TEST_ADMIN_PW".to_string()),
        },
        profiles: Vec::new(),
        samba: Default::default(),
        relations: Vec::new(),
    };
    let password =
        std::env::var("EDAPTOR_TEST_ADMIN_PW").unwrap_or_else(|_| "adminpassword".to_string());
    (config, password)
}

fn search(worker: &WorkerHandle, base: &str, filter: &str, attrs: Vec<String>) -> Vec<edaptor::ldap::worker::LdapEntry> {
    let resp = worker
        .request(Request::Search {
            id: 1,
            base: base.to_string(),
            scope: SearchScope::Subtree,
            filter: filter.to_string(),
            attrs,
            size_limit: None,
        })
        .expect("search should reply");
    match resp {
        Response::Entries { entries, .. } => entries,
        other => panic!("expected Entries, got {other:?}"),
    }
}

#[test]
fn seed_people_count_exceeds_one_hundred() {
    let Ok(uri) = std::env::var("EDAPTOR_TEST_LDAP_URI") else {
        eprintln!("SKIP: EDAPTOR_TEST_LDAP_URI not set");
        return;
    };
    let (cfg, password) = test_config(uri);
    let worker = WorkerHandle::spawn(cfg, password).expect("connect+bind");
    // Paged subtree scan returns the full set past the 500 size limit.
    let resp = worker
        .request(Request::LoadStructure {
            id: 1,
            base: format!("ou=people,{BASE}"),
            page_size: 200,
            attrs: vec![],
        })
        .expect("structure scan should reply");
    let count = match resp {
        Response::StructureEntries { nodes, .. } => nodes.len(),
        other => panic!("expected StructureEntries, got {other:?}"),
    };
    assert!(count > 100, "expected >100 people, got {count}");
}

#[test]
fn seed_has_posix_and_membership_groups() {
    let Ok(uri) = std::env::var("EDAPTOR_TEST_LDAP_URI") else {
        eprintln!("SKIP: EDAPTOR_TEST_LDAP_URI not set");
        return;
    };
    let (cfg, password) = test_config(uri);
    let worker = WorkerHandle::spawn(cfg, password).expect("connect+bind");

    let posix = search(
        &worker,
        &format!("ou=groups,{BASE}"),
        "(objectClass=posixGroup)",
        vec!["gidNumber".to_string()],
    );
    assert!(posix.len() >= 5, "expected >=5 posixGroups, got {}", posix.len());
    assert!(
        posix.iter().all(|e| e.attrs.contains_key("gidNumber")),
        "every posixGroup must expose gidNumber"
    );

    let gon = search(
        &worker,
        &format!("ou=groups,{BASE}"),
        "(objectClass=groupOfNames)",
        vec!["member".to_string()],
    );
    assert!(gon.len() >= 5, "expected >=5 groupOfNames, got {}", gon.len());
    assert!(
        gon.iter().all(|e| e.attrs.contains_key("member")),
        "every groupOfNames must expose member"
    );
}

#[test]
fn seed_samba_domain_is_discoverable() {
    let Ok(uri) = std::env::var("EDAPTOR_TEST_LDAP_URI") else {
        eprintln!("SKIP: EDAPTOR_TEST_LDAP_URI not set");
        return;
    };
    let (cfg, password) = test_config(uri);
    let worker = WorkerHandle::spawn(cfg, password).expect("connect+bind");
    let domains = search(
        &worker,
        BASE,
        "(objectClass=sambaDomain)",
        vec!["sambaSID".to_string()],
    );
    assert_eq!(domains.len(), 1, "exactly one sambaDomain");
    assert!(
        domains[0].attrs.get("sambaSID").is_some_and(|v| !v.is_empty()),
        "sambaDomain must yield a sambaSID"
    );
}
```

- [ ] **Step 2: Verify field/type names against the worker API**

The test references `Request::Search { id, base, scope, filter, attrs, size_limit }`, `Request::LoadStructure { id, base, page_size, attrs }`, `Response::Entries { entries, .. }`, `Response::StructureEntries { nodes, .. }`, `SearchScope::Subtree`, `WorkerHandle::spawn`, `WorkerHandle::request`, and `LdapEntry.attrs: BTreeMap<String, Vec<String>>`. If any name differs in `src/ldap/worker.rs`, adjust the test to match (do not change the worker). Confirm with:

Run: `grep -nE "pub fn (spawn|request)|StructureEntries|struct LdapEntry|pub nodes|pub entries" src/ldap/worker.rs`
Expected: the referenced items exist.

- [ ] **Step 3: Run the seed test against the running container**

Run:
```bash
export EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389
export EDAPTOR_TEST_ADMIN_PW=adminpassword
cargo test --test live_seed -- --nocapture
```
Expected: 3 tests pass (not skipped, since the env var is set and the server from Task 6 is running).

- [ ] **Step 4: Commit**

```bash
git add tests/live_seed.rs
git commit -m "test: gated live assertions on the seeded test directory"
```

---

## Task 8: Provisioning README, top-level README note, full regression

**Files:**
- Create: `scripts/ldap-provision/README.md`
- Modify: `README.md`

- [ ] **Step 1: Write `scripts/ldap-provision/README.md`**

```markdown
# Test-server provisioning assets

These files turn the bare Bitnami OpenLDAP started by `../test-ldap.sh` into a
directory that mirrors the `oposs.openldap` ansible role (schemas, overlays,
password policy) and seeds it with realistic data.

`test-ldap.sh start` loads them in this order:

| File | Bind identity | Purpose |
|------|---------------|---------|
| `schema/samba.ldif` | `cn=admin,cn=config` | Samba schema (verbatim from the role) |
| `schema/mail.ldif`  | `cn=admin,cn=config` | Mail schema (`olcSchemaConfig` form of the role's `mail.schema`) |
| `config/overlays.ldif` | `cn=admin,cn=config` | `memberof` + `refint` + `ppolicy` modules/overlays on `{2}mdb` |
| `data/ppolicy.ldif` | `cn=admin,dc=example,dc=org` | `ou=policies` + default/serviceaccounts policies |
| `data/base.ldif` | `cn=admin,dc=example,dc=org` | `ou=people/groups/services`, `sambaDomain`, service accounts |
| `data/testdata.ldif` | `cn=admin,dc=example,dc=org` | 600 users / 25 groups (generated) |

**Module path note:** on the Bitnami image the overlay `.so`s live in
`/opt/bitnami/openldap/lib/openldap`, not the default `libexec/` path — hence the
fresh `cn=module{1}` in `overlays.ldif`.

**Shared user password:** every generated user (and the service accounts) has
password `test123`.

## Regenerating `testdata.ldif`

```bash
cargo run --bin gen-testdata            # 600 users, dc=example,dc=org
cargo run --bin gen-testdata -- --users 150
cargo run --bin gen-testdata -- --out - # to stdout
```

Output is deterministic (no RNG, fixed timestamp), so regenerating with the same
flags produces an identical file.
```

- [ ] **Step 2: Add a note to the top-level `README.md`**

Under the `## Configuration` section (or a new `## Local test server` section just above it), add:

```markdown
## Local test server

`scripts/test-ldap.sh start` launches a podman OpenLDAP that mirrors the
`oposs.openldap` role — Samba + mail schemas, the memberOf/refint/ppolicy
overlays, password policies — and seeds it with ~600 users across 5 departments
and ~25 groups (see `scripts/ldap-provision/`). Point edaptor at it with:

```bash
scripts/test-ldap.sh start
export EDAPTOR_TEST_ADMIN_PW=adminpassword
edaptor --config examples/demo-config.toml
```

All generated users share the password `test123`.
```

- [ ] **Step 3: Full regression — unit + headless suites**

Run: `cargo test`
Expected: all non-gated tests pass; the `live_*` suites SKIP (unless `EDAPTOR_TEST_LDAP_URI` is exported). Then run the gated suites against the running container to confirm none regressed:
```bash
export EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389
export EDAPTOR_TEST_ADMIN_PW=adminpassword
cargo test --test live_structure --test live_membership --test live_samba \
           --test live_write --test live_templates --test live_seed
```
Expected: all pass (the existing suites self-seed under `ou=users` and are unaffected by the new `ou=people` data).

- [ ] **Step 4: Stop the container**

Run: `scripts/test-ldap.sh stop`
Expected: `Stopped edaptor-test-ldap`.

- [ ] **Step 5: Commit**

```bash
git add scripts/ldap-provision/README.md README.md
git commit -m "docs: document the provisioned test server and demo config"
```

---

## Self-Review (completed against the spec)

- **Spec coverage:** schemas (T3), overlays (T3+T4), ppolicy (T3), service accounts + sambaDomain (T3), generator (T1+T2), 600 users / 5 depts / separate groupOfNames + posixGroup + functional groups (T1), test-ldap.sh provisioning keeping `dc=example,dc=org` and the default `ou=users` tree (T4), demo config (T5), generator unit tests (T1), gated live_seed (T7), README/provenance (T8), no-runtime-dependency-on-oep-ansible (assets vendored in T3). All spec sections map to a task.
- **Group schema correction:** spec + plan both use **separate** `groupOfNames` and `posixGroup` entries (two structural classes can't combine under stock `nis`); plan never emits a dual-class group.
- **Type consistency:** generator names (`GenOpts`, `generate`, `to_ldif`, `GroupKind::{GroupOfNames,PosixGroup}`, `dept_gid`) are used identically across T1/T2; live test uses real worker API names (`Request::Search`/`LoadStructure`, `Response::Entries`/`StructureEntries`, `SearchScope::Subtree`) with a verification step (T7 Step 2) in case any field name differs.
- **Reused APIs verified present and `pub`:** `samba::nthash::nt_hash`, `samba::nthash::samba_pwd_last_set`, `samba::sid::user_sid`, `samba::account::samba_acct_flags`.
- **No placeholders.**
