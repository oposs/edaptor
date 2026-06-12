//! Resolving the `edaptor passwd <user-or-dn>` target.
//!
//! The CLI argument is either a full DN (`uid=andy,ou=people,dc=example,dc=org`)
//! or a bare username (`andy`). A username is resolved to a DN by searching every
//! configured profile's `search_base` for `(<rdn_attr>=<username>)`. This module
//! holds the pure decision logic; the LDAP round-trips live in `run_passwd`.

use crate::config::EntryProfile;
use crate::ui::picker::escape_filter;

/// True when `arg` looks like a DN rather than a bare username: a DN always
/// contains at least one `=` (the RDN assertion), a username never does.
pub fn looks_like_dn(arg: &str) -> bool {
    arg.contains('=')
}

/// The per-profile searches that resolve `username` to a DN: one `(base, filter)`
/// per profile with a non-empty `search_base`, matching the profile's `rdn_attr`
/// against the (filter-escaped) username. Run each with subtree scope. Profiles
/// with an empty `search_base` or empty `rdn_attr` are skipped.
pub fn username_searches(profiles: &[EntryProfile], username: &str) -> Vec<(String, String)> {
    let value = escape_filter(username);
    profiles
        .iter()
        .filter(|p| !p.search_base.is_empty() && !p.rdn_attr.is_empty())
        .map(|p| (p.search_base.clone(), format!("({}={})", p.rdn_attr, value)))
        .collect()
}

/// The outcome of resolving a username against the collected candidate DNs.
#[derive(Debug, PartialEq, Eq)]
pub enum Resolution {
    /// Exactly one entry matched — its DN.
    Unique(String),
    /// No entry matched.
    NotFound,
    /// More than one distinct entry matched — the candidate DNs (sorted).
    Ambiguous(Vec<String>),
}

/// Decide the resolution from the candidate DNs collected across all searches.
/// Duplicates (the same DN matched under overlapping bases) are removed
/// case-insensitively before deciding.
pub fn resolve_outcome(dns: Vec<String>) -> Resolution {
    // Dedup case-insensitively, keeping the first spelling seen for each entry.
    let mut unique: Vec<String> = Vec::new();
    for dn in dns {
        if !unique.iter().any(|d| d.eq_ignore_ascii_case(&dn)) {
            unique.push(dn);
        }
    }
    match unique.len() {
        0 => Resolution::NotFound,
        1 => Resolution::Unique(unique.into_iter().next().unwrap()),
        _ => {
            unique.sort();
            Resolution::Ambiguous(unique)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(name: &str, rdn: &str, base: &str) -> EntryProfile {
        EntryProfile {
            name: name.to_string(),
            rdn_attr: rdn.to_string(),
            search_base: base.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn bare_username_is_not_a_dn() {
        assert!(!looks_like_dn("andy"));
        assert!(!looks_like_dn(""));
    }

    #[test]
    fn dn_is_recognised_by_its_rdn_assertion() {
        assert!(looks_like_dn("uid=andy,ou=people,dc=example,dc=org"));
        assert!(looks_like_dn("cn=andy"));
    }

    #[test]
    fn username_searches_one_per_profile_with_base() {
        let profiles = [
            profile("user", "uid", "ou=people,dc=example,dc=org"),
            profile("group", "cn", "ou=groups,dc=example,dc=org"),
        ];
        assert_eq!(
            username_searches(&profiles, "andy"),
            vec![
                (
                    "ou=people,dc=example,dc=org".to_string(),
                    "(uid=andy)".to_string()
                ),
                (
                    "ou=groups,dc=example,dc=org".to_string(),
                    "(cn=andy)".to_string()
                ),
            ]
        );
    }

    #[test]
    fn username_searches_skip_profiles_without_base_or_rdn() {
        let profiles = [
            profile("user", "uid", "ou=people,dc=example,dc=org"),
            profile("no_base", "uid", ""),
            profile("no_rdn", "", "ou=x,dc=example,dc=org"),
        ];
        assert_eq!(
            username_searches(&profiles, "andy"),
            vec![(
                "ou=people,dc=example,dc=org".to_string(),
                "(uid=andy)".to_string()
            )]
        );
    }

    #[test]
    fn username_searches_escape_the_filter_value() {
        let profiles = [profile("user", "uid", "ou=people,dc=example,dc=org")];
        assert_eq!(
            username_searches(&profiles, "a)b*"),
            vec![(
                "ou=people,dc=example,dc=org".to_string(),
                r"(uid=a\29b\2a)".to_string()
            )]
        );
    }

    #[test]
    fn no_matches_is_not_found() {
        assert_eq!(resolve_outcome(vec![]), Resolution::NotFound);
    }

    #[test]
    fn single_match_is_unique() {
        assert_eq!(
            resolve_outcome(vec!["uid=andy,ou=people,dc=example,dc=org".to_string()]),
            Resolution::Unique("uid=andy,ou=people,dc=example,dc=org".to_string())
        );
    }

    #[test]
    fn duplicate_dn_across_bases_collapses_to_unique() {
        assert_eq!(
            resolve_outcome(vec![
                "uid=andy,ou=people,dc=example,dc=org".to_string(),
                "UID=andy,OU=people,DC=example,DC=org".to_string(),
            ]),
            Resolution::Unique("uid=andy,ou=people,dc=example,dc=org".to_string())
        );
    }

    #[test]
    fn distinct_matches_are_ambiguous() {
        assert_eq!(
            resolve_outcome(vec![
                "uid=andy,ou=people,dc=example,dc=org".to_string(),
                "cn=andy,ou=groups,dc=example,dc=org".to_string(),
            ]),
            Resolution::Ambiguous(vec![
                "cn=andy,ou=groups,dc=example,dc=org".to_string(),
                "uid=andy,ou=people,dc=example,dc=org".to_string(),
            ])
        );
    }
}
