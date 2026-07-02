//! Picker engine types: `PickerBinding`, `CandidateScope`, and `scope_of`.
//! These are the core types used by the widget palette (`[profile.widget.<attr>]`)
//! to drive the unified candidate search UI.

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

/// A `[profile.widget.<attr>]` picker/membership binding resolved against the profile list.
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

impl PickerBinding {
    /// Resolve the effective cardinality: an explicit `select` wins; otherwise
    /// derive it from the field's schema arity (`select = "auto"`). This is the
    /// single source of the rule shared by `widget_for` routing and the editors.
    pub fn cardinality(&self, field_multi: bool) -> Cardinality {
        self.select.unwrap_or(if field_multi {
            Cardinality::Multi
        } else {
            Cardinality::Single
        })
    }
}

pub(crate) fn scope_of(p: &EntryProfile) -> CandidateScope {
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
            widgets: Default::default(),
            label: None,
        }
    }

    #[test]
    fn candidate_scope_carries_parsed_label_template() {
        use crate::config::label::{parse_label_template, LabelSeg};
        // The candidate profile carries the label; `scope_of` must parse it into
        // the returned CandidateScope.
        let mut user = profile("user", "inetOrgPerson", "ou=people,dc=x", &["uid", "cn"]);
        user.label = Some("{cn} ({uid})".to_string());
        let scope = scope_of(&user);
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
        // search_attrs = [cn]; label adds displayName → the picker search now
        // matches displayName too, even though it was not in search_attrs.
        let mut user = profile("user", "inetOrgPerson", "ou=people,dc=x", &["cn"]);
        user.label = Some("{cn} — {displayName}".to_string());
        let sa = scope_of(&user).search_attrs;
        assert!(sa.iter().any(|a| a == "cn"));
        assert!(
            sa.iter().any(|a| a == "displayName"),
            "label-template attr joins the search: {sa:?}"
        );
    }
}

#[cfg(test)]
mod cardinality_tests {
    use super::*;

    fn binding(select: Option<Cardinality>) -> PickerBinding {
        PickerBinding {
            attr: "member".into(),
            scope: CandidateScope {
                base: "ou=people,dc=example,dc=org".into(),
                object_classes: vec!["inetOrgPerson".into()],
                search_attrs: vec!["cn".into()],
                label_template: None,
            },
            store: StoreKey::Dn,
            select,
            fanout_attr: None,
        }
    }

    #[test]
    fn cardinality_prefers_explicit_select() {
        assert_eq!(binding(Some(Cardinality::Single)).cardinality(true), Cardinality::Single);
        assert_eq!(binding(Some(Cardinality::Multi)).cardinality(false), Cardinality::Multi);
    }

    #[test]
    fn cardinality_falls_back_to_field_arity() {
        assert_eq!(binding(None).cardinality(true), Cardinality::Multi);
        assert_eq!(binding(None).cardinality(false), Cardinality::Single);
    }
}
