//! Membership relations: one `[[relation]]` declares both ends of a symmetric
//! holder↔candidate link (e.g. group.member ↔ user.memberOf). Pure; resolved
//! against the configured [`EntryProfile`]s into directional [`ResolvedRelation`]s.

use serde::Deserialize;

/// A symmetric membership relation as declared in `[[relation]]`. Template names
/// (`holder`, `candidate`) reference `[[profile]]` `name`s.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
pub struct Relation {
    pub name: String,
    /// Template whose entry OWNS the link attribute.
    pub holder: String,
    /// The real, writable attribute on the holder (e.g. `member`).
    pub holder_attr: String,
    /// Template that scopes the picker's candidate search (e.g. `user`).
    pub candidate: String,
    /// Virtual back-reference field shown on the candidate side (e.g. `memberOf`).
    pub back_attr: String,
}

use crate::config::EntryProfile;

/// Which side of a relation a field plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationRole {
    /// The entry owns the link attribute (e.g. group.member) — written directly.
    Holder,
    /// A virtual back-reference (e.g. user.memberOf) — writes fan out to holders.
    BackRef,
}

/// The scope for a live candidate search: where to look and what to match on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateScope {
    pub base: String,
    pub object_class: String,
    pub search_attrs: Vec<String>,
}

/// A relation resolved against the configured profiles: the concrete objectClass
/// for each end plus the search scope used from each direction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRelation {
    pub name: String,
    pub holder_oc: String,
    pub holder_attr: String,
    pub candidate_oc: String,
    pub back_attr: String,
    /// Used on the HOLDER form (editing `holder_attr`) — searches candidates.
    pub candidate_scope: CandidateScope,
    /// Used on the CANDIDATE form (editing `back_attr`) — searches holders.
    pub holder_scope: CandidateScope,
}

fn scope_of(p: &EntryProfile) -> CandidateScope {
    CandidateScope {
        base: p.search_base.clone(),
        object_class: p.object_class.clone(),
        search_attrs: p.search_attributes(),
    }
}

/// Resolve each `[[relation]]` against `profiles`. Relations referencing an
/// unknown template are dropped (caller may warn).
pub fn resolve_relations(
    profiles: &[EntryProfile],
    relations: &[Relation],
) -> Vec<ResolvedRelation> {
    let find = |name: &str| profiles.iter().find(|p| p.name == name);
    relations
        .iter()
        .filter_map(|r| {
            let holder = find(&r.holder)?;
            let candidate = find(&r.candidate)?;
            Some(ResolvedRelation {
                name: r.name.clone(),
                holder_oc: holder.object_class.clone(),
                holder_attr: r.holder_attr.clone(),
                candidate_oc: candidate.object_class.clone(),
                back_attr: r.back_attr.clone(),
                candidate_scope: scope_of(candidate),
                holder_scope: scope_of(holder),
            })
        })
        .collect()
}

fn has_oc(ocs: &[String], oc: &str) -> bool {
    ocs.iter().any(|o| o.eq_ignore_ascii_case(oc))
}

/// The relation where `(ocs, attr)` is the HOLDER side (e.g. group.member).
pub fn holder_lookup<'a>(
    rels: &'a [ResolvedRelation],
    ocs: &[String],
    attr: &str,
) -> Option<&'a ResolvedRelation> {
    rels.iter()
        .find(|r| has_oc(ocs, &r.holder_oc) && r.holder_attr.eq_ignore_ascii_case(attr))
}

/// The relation where `(ocs, attr)` is the BACK-REF side (e.g. user.memberOf).
pub fn backref_lookup<'a>(
    rels: &'a [ResolvedRelation],
    ocs: &[String],
    attr: &str,
) -> Option<&'a ResolvedRelation> {
    rels.iter()
        .find(|r| has_oc(ocs, &r.candidate_oc) && r.back_attr.eq_ignore_ascii_case(attr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn parses_relation_block() {
        let cfg: Config = toml::from_str(
            r#"
            [server]
            uri = "ldaps://x"
            base_dn = "dc=x"
            [auth]
            [[relation]]
            name = "group-membership"
            holder = "group"
            holder_attr = "member"
            candidate = "user"
            back_attr = "memberOf"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.relations.len(), 1);
        assert_eq!(cfg.relations[0].holder_attr, "member");
        assert_eq!(cfg.relations[0].back_attr, "memberOf");
    }

    fn profile(name: &str, oc: &str, base: &str, search: &[&str]) -> crate::config::EntryProfile {
        crate::config::EntryProfile {
            name: name.into(),
            object_class: oc.into(),
            rdn_attr: "x".into(),
            search_base: base.into(),
            show: vec![],
            search_attrs: search.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn fixture() -> Vec<ResolvedRelation> {
        let profiles = vec![
            profile("group", "groupOfNames", "ou=groups,dc=x", &["cn"]),
            profile("user", "inetOrgPerson", "ou=people,dc=x", &["uid", "cn"]),
        ];
        let rels = vec![Relation {
            name: "m".into(),
            holder: "group".into(),
            holder_attr: "member".into(),
            candidate: "user".into(),
            back_attr: "memberOf".into(),
        }];
        resolve_relations(&profiles, &rels)
    }

    #[test]
    fn resolves_both_directions_with_correct_scopes() {
        let r = fixture();
        assert_eq!(r.len(), 1);
        // Holder side (editing group.member) searches CANDIDATES = users.
        assert_eq!(r[0].candidate_scope.base, "ou=people,dc=x");
        assert_eq!(r[0].candidate_scope.object_class, "inetOrgPerson");
        // Back-ref side (editing user.memberOf) searches HOLDERS = groups.
        assert_eq!(r[0].holder_scope.base, "ou=groups,dc=x");
        assert_eq!(r[0].holder_scope.object_class, "groupOfNames");
    }

    #[test]
    fn holder_lookup_matches_holder_oc_and_attr() {
        let r = fixture();
        let ocs = vec!["top".to_string(), "groupOfNames".to_string()];
        // group's `member` → Holder, candidate scope = users.
        let h = holder_lookup(&r, &ocs, "member").unwrap();
        assert_eq!(h.candidate_scope.object_class, "inetOrgPerson");
        // a user's `member` is NOT a holder match (wrong objectClass).
        assert!(holder_lookup(&r, &["inetOrgPerson".to_string()], "member").is_none());
    }

    #[test]
    fn backref_lookup_matches_candidate_oc_and_back_attr() {
        let r = fixture();
        let ocs = vec!["inetOrgPerson".to_string()];
        let b = backref_lookup(&r, &ocs, "memberOf").unwrap();
        assert_eq!(b.holder_scope.object_class, "groupOfNames"); // searches groups
        assert!(backref_lookup(&r, &["groupOfNames".to_string()], "memberOf").is_none());
    }

    #[test]
    fn unknown_template_is_dropped() {
        let profiles = vec![profile("user", "inetOrgPerson", "ou=people", &["uid"])];
        let rels = vec![Relation {
            name: "m".into(),
            holder: "group".into(),
            holder_attr: "member".into(),
            candidate: "user".into(),
            back_attr: "memberOf".into(),
        }];
        assert!(resolve_relations(&profiles, &rels).is_empty()); // `group` profile missing
    }
}
