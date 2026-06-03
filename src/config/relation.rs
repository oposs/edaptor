//! Picker bindings: each `[profile.picker.<attr>]` declares how an attribute's
//! field is populated from a live candidate search. Pure; resolved against the
//! configured [`EntryProfile`]s into directional [`ResolvedPicker`]s.

use crate::config::EntryProfile;

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
/// matching).
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

fn has_oc(ocs: &[String], oc: &str) -> bool {
    ocs.iter().any(|o| o.eq_ignore_ascii_case(oc))
}

#[cfg(test)]
mod tests {
    use super::*;

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
            pickers: Default::default(),
            label: None,
        }
    }

    #[test]
    fn candidate_scope_carries_parsed_label_template() {
        use crate::config::label::{parse_label_template, LabelSeg};
        use crate::config::PickerSpec;
        // The candidate profile carries the label; `scope_of` (via resolve_pickers)
        // must parse it into the binding's scope.
        let mut user = profile("user", "inetOrgPerson", "ou=people,dc=x", &["uid", "cn"]);
        user.label = Some("{cn} ({uid})".to_string());
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
        let resolved = resolve_pickers(&[group, user]);
        let scope = &resolved[0].binding.scope;
        assert_eq!(
            scope.label_template,
            Some(vec![
                LabelSeg::Field("cn".into()),
                LabelSeg::Lit(" (".into()),
                LabelSeg::Field("uid".into()),
                LabelSeg::Lit(")".into()),
            ])
        );
        assert_eq!(
            scope.label_template,
            Some(parse_label_template("{cn} ({uid})"))
        );
        // The candidate search now also covers the label-template attributes
        // (search_attrs `uid`/`cn` plus the template's `cn`/`uid`, deduped).
        assert!(scope.search_attrs.iter().any(|a| a == "uid"));
        assert!(scope.search_attrs.iter().any(|a| a == "cn"));
    }

    #[test]
    fn scope_search_attrs_gain_label_template_attrs_not_already_listed() {
        use crate::config::PickerSpec;
        // search_attrs = [cn]; label adds displayName → the picker search now
        // matches displayName too, even though it was not in search_attrs.
        let mut user = profile("user", "inetOrgPerson", "ou=people,dc=x", &["cn"]);
        user.label = Some("{cn} — {displayName}".to_string());
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
        let resolved = resolve_pickers(&[group, user]);
        let sa = &resolved[0].binding.scope.search_attrs;
        assert!(sa.iter().any(|a| a == "cn"));
        assert!(
            sa.iter().any(|a| a == "displayName"),
            "label-template attr joins the search: {sa:?}"
        );
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
