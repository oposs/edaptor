//! Profile detection (spec 2026-09-29): sample the directory, detect profiles,
//! merge the config over them. `infer`, `patterns`, `range`, `merge` and `dump`
//! are pure; `sample` and `load` talk to the LDAP worker.

pub mod assume;
#[cfg(test)]
pub(crate) mod fixtures;
pub mod infer;
pub mod load;
pub mod merge;
pub mod model;
pub mod names;
pub mod patterns;
pub mod private;
pub mod range;
pub mod sample;

use std::time::Duration;

/// Client size limit of every container sample search.
pub const SAMPLE_SIZE: i32 = 200;
/// At most this many containers are sampled (server order).
pub const MAX_CONTAINERS: usize = 100;
/// Time budget of the whole sampling step.
pub const DETECT_BUDGET: Duration = Duration::from_secs(10);
/// A value-inferring rule needs at least this many entries.
pub const MIN_SAMPLE: usize = 3;
/// Keys per cross-container lookup filter.
pub const LOOKUP_BATCH: usize = 50;
/// The attributes a container sample fetches (values). Never secrets.
pub const SAMPLE_ATTRS: &[&str] = &[
    "objectClass",
    "uid",
    "cn",
    "sn",
    "givenName",
    "displayName",
    "gecos",
    "mail",
    "uidNumber",
    "gidNumber",
    "homeDirectory",
    "loginShell",
    "memberUid",
    "member",
    "uniqueMember",
    "sambaSID",
    "description",
];
/// Structural classes whose detected profiles are hidden from the all-profiles
/// chooser unless the current container is exactly theirs.
pub const INFRASTRUCTURE_CLASSES: &[&str] = &[
    "organizationalUnit",
    "organization",
    "domain",
    "dcObject",
    "sambaDomain",
    "pwdPolicy",
];

/// `matched` is more than half of `of` (and `of` is not zero).
pub fn more_than_half(matched: usize, of: usize) -> bool {
    of > 0 && matched * 2 > of
}

/// A value-inferring rule applies: more than half AND at least `MIN_SAMPLE`.
pub fn rule_applies(matched: usize, sampled: usize) -> bool {
    sampled >= MIN_SAMPLE && more_than_half(matched, sampled)
}

/// Split a DN into its RDN components at unescaped commas (`\,` stays inside
/// its component). Components are returned untrimmed.
pub fn dn_components(dn: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut escaped = false;
    for (i, ch) in dn.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            ',' => {
                out.push(&dn[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if !dn.is_empty() {
        out.push(&dn[start..]);
    }
    out
}

/// The parent DN (everything after the first unescaped comma), trimmed.
pub fn parent_dn(dn: &str) -> Option<&str> {
    let first = dn_components(dn).into_iter().next()?;
    let rest = dn.get(first.len() + 1..)?;
    let rest = rest.trim_start();
    (!rest.is_empty()).then_some(rest)
}

/// Canonical comparison form of a DN: components trimmed, whitespace around `=`
/// removed, lowercased.
pub fn normalize_dn(dn: &str) -> String {
    dn_components(dn)
        .iter()
        .map(|c| match c.split_once('=') {
            Some((a, v)) => format!("{}={}", a.trim(), v.trim()),
            None => c.trim().to_string(),
        })
        .collect::<Vec<_>>()
        .join(",")
        .to_lowercase()
}

/// DN equality after [`normalize_dn`].
pub fn dn_eq(a: &str, b: &str) -> bool {
    normalize_dn(a) == normalize_dn(b)
}

/// The most frequent item (compared case-insensitively; the first spelling seen
/// is kept) and its count. Ties: the lexicographically smallest lowercased item.
pub fn most_common<'a>(items: impl IntoIterator<Item = &'a str>) -> Option<(String, usize)> {
    let mut counts: std::collections::BTreeMap<String, (String, usize)> =
        std::collections::BTreeMap::new();
    for it in items {
        let e = counts
            .entry(it.to_lowercase())
            .or_insert_with(|| (it.to_string(), 0));
        e.1 += 1;
    }
    // BTreeMap iterates in ascending key order; `max_by_key` keeps the LAST max,
    // so iterate in reverse to keep the smallest key among equal counts.
    counts
        .into_iter()
        .rev()
        .max_by_key(|(_, (_, n))| *n)
        .map(|(_, v)| v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_needs_three_and_a_majority() {
        assert!(rule_applies(2, 3));
        assert!(!rule_applies(2, 2));
        assert!(!rule_applies(1, 2));
        assert!(!rule_applies(6, 12));
        assert!(rule_applies(7, 12));
        assert!(more_than_half(2, 2));
        assert!(!more_than_half(0, 0));
    }

    #[test]
    fn dn_components_respect_escaped_commas() {
        assert_eq!(
            dn_components(r"cn=Smith\, John,ou=people,dc=x"),
            vec![r"cn=Smith\, John", "ou=people", "dc=x"]
        );
        assert_eq!(
            parent_dn(r"cn=Smith\, John,ou=people,dc=x"),
            Some("ou=people,dc=x")
        );
        assert_eq!(parent_dn("dc=x"), None);
        assert_eq!(
            parent_dn("cn=a+uid=b, ou=people,dc=x"),
            Some("ou=people,dc=x")
        );
    }

    #[test]
    fn dn_eq_ignores_case_and_spacing() {
        assert!(dn_eq(
            "OU=People, DC=Example,dc=org",
            "ou=people,dc=example,dc=org"
        ));
        assert!(!dn_eq("ou=people2,dc=x", "ou=people,dc=x"));
    }

    #[test]
    fn most_common_is_case_insensitive_and_stable() {
        assert_eq!(
            most_common(["cn", "CN", "uid"]),
            Some(("cn".to_string(), 2))
        );
        assert_eq!(most_common(["b", "a"]), Some(("a".to_string(), 1)));
        assert_eq!(most_common(Vec::<&str>::new()), None);
    }
}
