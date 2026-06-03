//! Membership relations: one `[[relation]]` declares both ends of a symmetric
//! holder↔candidate link (e.g. group.member ↔ user.memberOf). Pure; resolved
//! against the configured [`EntryProfile`]s into directional [`ResolvedRelation`]s.

use crate::config::EntryProfile;
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

/// Which side of a relation a field plays. Consumed in Phase 4 by
/// `src/ui/edit_form.rs` (`FieldRelation` variant of `EditField`).
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
    pub object_classes: Vec<String>,
    pub search_attrs: Vec<String>,
    /// Parsed display-label template for entries in this scope, ready to render.
    /// `None` when the underlying profile declares no `label`.
    pub label_template: Option<Vec<crate::config::label::LabelSeg>>,
}

/// Picker cardinality: how many candidates may be selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cardinality {
    Single,
    Multi,
}

/// What a pick stores into the field — and the identity key for dedupe/toggle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreKey {
    /// Store the candidate's DN; key compared case-insensitively.
    Dn,
    /// Store this scalar attribute of the candidate; key compared exactly.
    Attr(String),
}

/// A `[profile.picker.<attr>]` binding resolved against the profile list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerBinding {
    /// The attribute this binds (e.g. `memberUid`).
    pub attr: String,
    /// Resolved candidate search scope (from the `candidate` profile).
    pub scope: CandidateScope,
    /// What each pick contributes, and the identity key.
    pub store: StoreKey,
    /// Cardinality; `None` = derive from the field's schema arity (`select = "auto"`).
    pub select: Option<Cardinality>,
    /// `Some` ⇒ synthetic back-ref: write this attr on each picked candidate's
    /// entry (this entry's DN), and do not write the field to the server.
    pub fanout_attr: Option<String>,
}

/// A resolved picker bound to its owning profile's object classes (for entry
/// matching) — the picker analogue of `ResolvedRelation`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPicker {
    /// Object classes of the profile that DECLARES this picker (the field owner).
    pub owner_object_classes: Vec<String>,
    pub binding: PickerBinding,
}

/// Resolve every `[profile.picker.*]` across all profiles. A picker whose
/// `candidate` names an unknown profile is dropped (caller may warn).
pub fn resolve_pickers(profiles: &[EntryProfile]) -> Vec<ResolvedPicker> {
    let find = |name: &str| profiles.iter().find(|p| p.name == name);
    let mut out = Vec::new();
    for owner in profiles {
        for (attr, spec) in &owner.pickers {
            let Some(cand) = find(&spec.candidate) else {
                continue; // unknown candidate profile → drop
            };
            let store = if spec.store.eq_ignore_ascii_case("dn") {
                StoreKey::Dn
            } else {
                StoreKey::Attr(spec.store.clone())
            };
            let select = match spec.select.to_ascii_lowercase().as_str() {
                "single" => Some(Cardinality::Single),
                "multi" => Some(Cardinality::Multi),
                _ => None, // "auto" (or anything else) → derive from schema arity
            };
            out.push(ResolvedPicker {
                owner_object_classes: owner.object_classes.clone(),
                binding: PickerBinding {
                    attr: attr.clone(),
                    scope: scope_of(cand),
                    store,
                    select,
                    fanout_attr: spec.fanout_attr.clone(),
                },
            });
        }
    }
    out
}

/// The picker binding for `(entry object classes, attr)`, if any: the entry must
/// carry one of the picker's owner object classes and the attr must match.
pub fn picker_for<'a>(
    pickers: &'a [ResolvedPicker],
    ocs: &[String],
    attr: &str,
) -> Option<&'a PickerBinding> {
    pickers
        .iter()
        .find(|p| {
            p.binding.attr.eq_ignore_ascii_case(attr)
                && p.owner_object_classes.iter().any(|oc| has_oc(ocs, oc))
        })
        .map(|p| &p.binding)
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
    let template = p
        .label
        .as_ref()
        .map(|s| crate::config::label::parse_label_template(s));
    // The picker's substring search matches on `search_attrs` AND every attribute
    // shown in the label template, so a search covers all properties the operator
    // can see in the candidate row.
    let mut search_attrs = p.search_attributes();
    if let Some(segs) = template.as_ref() {
        for a in crate::config::label::template_attrs(segs) {
            if !search_attrs.iter().any(|x| x.eq_ignore_ascii_case(&a)) {
                search_attrs.push(a);
            }
        }
    }
    CandidateScope {
        base: p.search_base.clone(),
        object_classes: p.object_classes.clone(),
        search_attrs,
        label_template: template,
    }
}

/// Resolve each `[[relation]]` against `profiles`. Relations referencing an
/// unknown template are dropped (caller may warn).
pub fn resolve_relations(
    profiles: &[EntryProfile],
    relations: &[Relation],
) -> Vec<ResolvedRelation> {
    // Case-sensitive: profile names are config-key identifiers, not LDAP naming.
    let find = |name: &str| profiles.iter().find(|p| p.name == name);
    relations
        .iter()
        .filter_map(|r| {
            let holder = find(&r.holder)?;
            let candidate = find(&r.candidate)?;
            Some(ResolvedRelation {
                name: r.name.clone(),
                holder_oc: holder.object_classes.first().cloned().unwrap_or_default(),
                holder_attr: r.holder_attr.clone(),
                candidate_oc: candidate
                    .object_classes
                    .first()
                    .cloned()
                    .unwrap_or_default(),
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
        assert_eq!(cfg.relations[0].name, "group-membership");
        assert_eq!(cfg.relations[0].holder, "group");
        assert_eq!(cfg.relations[0].candidate, "user");
        assert_eq!(cfg.relations[0].holder_attr, "member");
        assert_eq!(cfg.relations[0].back_attr, "memberOf");
    }

    fn profile(name: &str, oc: &str, base: &str, search: &[&str]) -> crate::config::EntryProfile {
        crate::config::EntryProfile {
            name: name.into(),
            object_classes: vec![oc.into()],
            rdn_attr: "x".into(),
            search_base: base.into(),
            show: vec![],
            search_attrs: search.iter().map(|s| s.to_string()).collect(),
            defaults: Default::default(),
            password: None,
            lookups: Default::default(),
            pickers: Default::default(),
            label: None,
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
        assert_eq!(
            r[0].candidate_scope.object_classes,
            vec!["inetOrgPerson".to_string()]
        );
        // Back-ref side (editing user.memberOf) searches HOLDERS = groups.
        assert_eq!(r[0].holder_scope.base, "ou=groups,dc=x");
        assert_eq!(
            r[0].holder_scope.object_classes,
            vec!["groupOfNames".to_string()]
        );
    }

    #[test]
    fn holder_lookup_matches_holder_oc_and_attr() {
        let r = fixture();
        let ocs = vec!["top".to_string(), "groupOfNames".to_string()];
        // group's `member` → Holder, candidate scope = users.
        let h = holder_lookup(&r, &ocs, "member").unwrap();
        assert_eq!(
            h.candidate_scope.object_classes,
            vec!["inetOrgPerson".to_string()]
        );
        // a user's `member` is NOT a holder match (wrong objectClass).
        assert!(holder_lookup(&r, &["inetOrgPerson".to_string()], "member").is_none());
    }

    #[test]
    fn backref_lookup_matches_candidate_oc_and_back_attr() {
        let r = fixture();
        let ocs = vec!["inetOrgPerson".to_string()];
        let b = backref_lookup(&r, &ocs, "memberOf").unwrap();
        assert_eq!(
            b.holder_scope.object_classes,
            vec!["groupOfNames".to_string()]
        ); // searches groups
        assert!(backref_lookup(&r, &["groupOfNames".to_string()], "memberOf").is_none());
    }

    #[test]
    fn candidate_scope_carries_parsed_label_template() {
        use crate::config::label::{parse_label_template, LabelSeg};
        let mut user = profile("user", "inetOrgPerson", "ou=people,dc=x", &["uid", "cn"]);
        user.label = Some("{cn} ({uid})".to_string());
        let profiles = vec![
            profile("group", "groupOfNames", "ou=groups,dc=x", &["cn"]),
            user,
        ];
        let rels = vec![Relation {
            name: "m".into(),
            holder: "group".into(),
            holder_attr: "member".into(),
            candidate: "user".into(),
            back_attr: "memberOf".into(),
        }];
        let r = resolve_relations(&profiles, &rels);
        assert_eq!(
            r[0].candidate_scope.label_template,
            Some(vec![
                LabelSeg::Field("cn".into()),
                LabelSeg::Lit(" (".into()),
                LabelSeg::Field("uid".into()),
                LabelSeg::Lit(")".into()),
            ])
        );
        assert_eq!(
            r[0].candidate_scope.label_template,
            Some(parse_label_template("{cn} ({uid})"))
        );
        // The candidate search now also covers the label-template attributes
        // (search_attrs `uid`/`cn` plus the template's `cn`/`uid`, deduped).
        assert!(r[0].candidate_scope.search_attrs.iter().any(|a| a == "uid"));
        assert!(r[0].candidate_scope.search_attrs.iter().any(|a| a == "cn"));
        // Holder profile has no label → None.
        assert!(r[0].holder_scope.label_template.is_none());
    }

    #[test]
    fn scope_search_attrs_gain_label_template_attrs_not_already_listed() {
        // search_attrs = [cn]; label adds displayName → the picker search now
        // matches displayName too, even though it was not in search_attrs.
        let mut user = profile("user", "inetOrgPerson", "ou=people,dc=x", &["cn"]);
        user.label = Some("{cn} — {displayName}".to_string());
        let profiles = vec![
            profile("group", "groupOfNames", "ou=groups,dc=x", &["cn"]),
            user,
        ];
        let rels = vec![Relation {
            name: "m".into(),
            holder: "group".into(),
            holder_attr: "member".into(),
            candidate: "user".into(),
            back_attr: "memberOf".into(),
        }];
        let r = resolve_relations(&profiles, &rels);
        let sa = &r[0].candidate_scope.search_attrs;
        assert!(sa.iter().any(|a| a == "cn"));
        assert!(
            sa.iter().any(|a| a == "displayName"),
            "label-template attr joins the search: {sa:?}"
        );
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

    #[test]
    fn resolves_picker_dn_store_defaults() {
        use crate::config::PickerSpec;
        let mut group = profile("group", "groupOfNames", "ou=groups,dc=x", &["cn"]);
        group.pickers.insert(
            "member".to_string(),
            PickerSpec {
                candidate: "user".into(),
                store: "dn".into(),
                select: "auto".into(),
                fanout_attr: None,
            },
        );
        let user = profile("user", "inetOrgPerson", "ou=people,dc=x", &["uid", "cn"]);
        let resolved = resolve_pickers(&[group, user]);
        assert_eq!(resolved.len(), 1);
        let b = &resolved[0].binding;
        assert_eq!(b.attr, "member");
        assert_eq!(b.scope.base, "ou=people,dc=x"); // candidate = user
        assert_eq!(b.store, StoreKey::Dn);
        assert_eq!(b.select, None); // "auto"
        assert_eq!(b.fanout_attr, None);
        assert_eq!(
            resolved[0].owner_object_classes,
            vec!["groupOfNames".to_string()]
        );
    }

    #[test]
    fn resolves_picker_scalar_store_and_select() {
        use crate::config::PickerSpec;
        let mut user = profile("user", "inetOrgPerson", "ou=people,dc=x", &["uid"]);
        user.pickers.insert(
            "gidNumber".to_string(),
            PickerSpec {
                candidate: "posixgroup".into(),
                store: "gidNumber".into(),
                select: "single".into(),
                fanout_attr: None,
            },
        );
        let pg = profile("posixgroup", "posixGroup", "ou=groups,dc=x", &["cn"]);
        let resolved = resolve_pickers(&[user, pg]);
        let b = &resolved[0].binding;
        assert_eq!(b.store, StoreKey::Attr("gidNumber".to_string()));
        assert_eq!(b.select, Some(Cardinality::Single));
    }

    #[test]
    fn resolves_picker_fanout() {
        use crate::config::PickerSpec;
        let mut user = profile("user", "inetOrgPerson", "ou=people,dc=x", &["uid"]);
        user.pickers.insert(
            "memberOf".to_string(),
            PickerSpec {
                candidate: "group".into(),
                store: "dn".into(),
                select: "multi".into(),
                fanout_attr: Some("member".into()),
            },
        );
        let group = profile("group", "groupOfNames", "ou=groups,dc=x", &["cn"]);
        let resolved = resolve_pickers(&[user, group]);
        let b = &resolved[0].binding;
        assert_eq!(b.fanout_attr.as_deref(), Some("member"));
        assert_eq!(b.select, Some(Cardinality::Multi));
        assert_eq!(b.scope.base, "ou=groups,dc=x"); // candidate = group
    }

    #[test]
    fn unknown_picker_candidate_is_dropped() {
        use crate::config::PickerSpec;
        let mut group = profile("group", "groupOfNames", "ou=groups,dc=x", &["cn"]);
        group.pickers.insert(
            "member".to_string(),
            PickerSpec {
                candidate: "nope".into(),
                store: "dn".into(),
                select: "auto".into(),
                fanout_attr: None,
            },
        );
        assert!(resolve_pickers(&[group]).is_empty());
    }

    #[test]
    fn picker_for_matches_owner_oc_and_attr() {
        use crate::config::PickerSpec;
        let mut group = profile("group", "groupOfNames", "ou=groups,dc=x", &["cn"]);
        group.pickers.insert(
            "member".to_string(),
            PickerSpec {
                candidate: "user".into(),
                store: "dn".into(),
                select: "auto".into(),
                fanout_attr: None,
            },
        );
        let user = profile("user", "inetOrgPerson", "ou=people,dc=x", &["uid"]);
        let resolved = resolve_pickers(&[group, user]);
        let ocs = vec!["top".to_string(), "groupOfNames".to_string()];
        assert!(picker_for(&resolved, &ocs, "member").is_some());
        assert!(picker_for(&resolved, &["inetOrgPerson".to_string()], "member").is_none());
        assert!(picker_for(&resolved, &ocs, "owner").is_none());
        // attr + object-class matching are case-insensitive.
        assert!(picker_for(&resolved, &ocs, "MEMBER").is_some());
        assert!(picker_for(&resolved, &["GROUPOFNAMES".to_string()], "member").is_some());
    }

    #[test]
    fn resolve_pickers_store_and_select_are_case_insensitive() {
        use crate::config::PickerSpec;
        let mut group = profile("group", "groupOfNames", "ou=groups,dc=x", &["cn"]);
        group.pickers.insert(
            "member".to_string(),
            PickerSpec {
                candidate: "user".into(),
                store: "DN".into(),      // upper-case sentinel
                select: "Single".into(), // capitalized cardinality
                fanout_attr: None,
            },
        );
        let user = profile("user", "inetOrgPerson", "ou=people,dc=x", &["uid"]);
        let resolved = resolve_pickers(&[group, user]);
        assert_eq!(resolved[0].binding.store, StoreKey::Dn);
        assert_eq!(resolved[0].binding.select, Some(Cardinality::Single));
    }
}
