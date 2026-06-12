//! Three-layer widget resolver: schema introspection < baked-in bundles <
//! explicit profile config. Constructed at form-build time; cheap to throw away.

use crate::config::builtin::builtin_schema;
use crate::config::relation::{scope_of, Cardinality, PickerBinding, StoreKey};
use crate::config::widget::{ChoiceFormat, ChoiceWidget, PasswordWidget, WidgetKind};
use crate::config::{ChoiceOption, EntryProfile, WidgetSpecCfg};
use crate::schema::model::SchemaModel;
use crate::schema::syntax::FieldKind;

/// Resolves the effective widget kind for an attribute in an entry, merging the
/// three layers. Construct once per form-open; the resolver borrows its inputs.
pub struct WidgetResolver<'a> {
    schema: &'a SchemaModel,
    profiles: &'a [EntryProfile],
    profile_widgets: &'a [crate::config::widget::ResolvedWidget],
    samba_enabled: bool,
}

impl<'a> WidgetResolver<'a> {
    pub fn new(
        schema: &'a SchemaModel,
        profiles: &'a [EntryProfile],
        profile_widgets: &'a [crate::config::widget::ResolvedWidget],
        samba_enabled: bool,
    ) -> Self {
        WidgetResolver {
            schema,
            profiles,
            profile_widgets,
            samba_enabled,
        }
    }

    /// Resolve the effective widget kind for `attr` given the entry's object classes.
    /// Returns `None` for plain-text fields with no special widget.
    pub fn resolve_kind(&self, attr: &str, entry_ocs: &[String]) -> Option<WidgetKind> {
        // Layer 1: schema introspection hints (weakest).
        let mut result: Option<WidgetKind> = None;
        if self.schema.is_readonly_attr(attr) {
            result = Some(WidgetKind::Readonly);
        } else if self.schema.field_kind(attr) == FieldKind::Boolean {
            result = Some(WidgetKind::Choice(ChoiceWidget {
                select: Cardinality::Single,
                format: ChoiceFormat::Plain,
                options: vec![
                    ChoiceOption {
                        value: "TRUE".into(),
                        label: "TRUE".into(),
                    },
                    ChoiceOption {
                        value: "FALSE".into(),
                        label: "FALSE".into(),
                    },
                ],
            }));
        }

        // Layer 2: baked-in objectClass bundles.
        // Walk alphabetically sorted objectClasses for determinism; last match wins.
        let bs = builtin_schema();
        let mut sorted_ocs: Vec<&str> = entry_ocs.iter().map(String::as_str).collect();
        sorted_ocs.sort_unstable();
        for oc in &sorted_ocs {
            if let Some(attr_map) = bs.get(&oc.to_lowercase()) {
                if let Some(spec) = attr_map.get(&attr.to_lowercase()) {
                    if let Some(kind) = self.spec_to_kind(spec, attr) {
                        result = Some(kind);
                    }
                }
            }
        }

        // Layer 3: explicit profile config (strongest).
        if let Some(kind) = crate::config::widget::widget_for(self.profile_widgets, entry_ocs, attr)
        {
            result = Some(kind.clone());
        }

        result
    }

    /// Convert a `WidgetSpecCfg` from the baked-in bundle into a live `WidgetKind`.
    /// Returns `None` when a sentinel candidate cannot be resolved (degrades to text)
    /// or when samba is disabled for `SambaSid`.
    fn spec_to_kind(&self, spec: &WidgetSpecCfg, attr: &str) -> Option<WidgetKind> {
        match spec {
            WidgetSpecCfg::Readonly => Some(WidgetKind::Readonly),
            WidgetSpecCfg::XOrdered => Some(WidgetKind::XOrdered),
            WidgetSpecCfg::SambaSid => {
                if self.samba_enabled {
                    Some(WidgetKind::SambaSid)
                } else {
                    None
                }
            }
            WidgetSpecCfg::Password { samba } => {
                let derived = if *samba {
                    vec![
                        "sambaNTPassword".to_string(),
                        "sambaPwdLastSet".to_string(),
                    ]
                } else {
                    Vec::new()
                };
                Some(WidgetKind::Password(PasswordWidget {
                    primary: attr.to_string(),
                    derived,
                    samba: *samba,
                }))
            }
            WidgetSpecCfg::Choice {
                select,
                format,
                options,
            } => {
                let card = match select.as_str() {
                    "multi" => Cardinality::Multi,
                    _ => Cardinality::Single,
                };
                let fmt = match format.as_str() {
                    "bracketed" => ChoiceFormat::Bracketed,
                    _ => ChoiceFormat::Plain,
                };
                Some(WidgetKind::Choice(ChoiceWidget {
                    select: card,
                    format: fmt,
                    options: options.clone(),
                }))
            }
            WidgetSpecCfg::Picker {
                candidate,
                store,
                select,
            } => {
                let scope = self.resolve_candidate(candidate)?;
                let store_key = if store.eq_ignore_ascii_case("dn") {
                    StoreKey::Dn
                } else {
                    StoreKey::Attr(store.clone())
                };
                let card = match select.as_str() {
                    "multi" => Some(Cardinality::Multi),
                    "single" => Some(Cardinality::Single),
                    _ => None, // "auto" — let form derive from schema arity
                };
                Some(WidgetKind::Picker(PickerBinding {
                    attr: attr.to_string(),
                    scope,
                    store: store_key,
                    select: card,
                    fanout_attr: None,
                }))
            }
            WidgetSpecCfg::Membership { candidate, via } => {
                let scope = self.resolve_candidate(candidate)?;
                Some(WidgetKind::Picker(PickerBinding {
                    attr: attr.to_string(),
                    scope,
                    store: StoreKey::Dn,
                    select: Some(Cardinality::Multi),
                    fanout_attr: Some(via.clone()),
                }))
            }
        }
    }

    /// Resolve a `CandidateRef` — sentinel names (`_posix_group_`, `_posix_account_`,
    /// `_any_`) or a regular profile name — to a `CandidateScope`.
    /// Returns `None` when no matching profile exists (field degrades to plain text).
    fn resolve_candidate(
        &self,
        candidate: &crate::config::CandidateRef,
    ) -> Option<crate::config::relation::CandidateScope> {
        use crate::config::CandidateRef;
        match candidate {
            CandidateRef::Inline(inline) => {
                let label_template = inline
                    .label
                    .as_ref()
                    .map(|s| crate::config::label::parse_label_template(s));
                Some(crate::config::relation::CandidateScope {
                    base: inline.base.clone(),
                    object_classes: inline.object_classes.clone(),
                    search_attrs: inline.search_attrs.clone(),
                    label_template,
                })
            }
            CandidateRef::Profile(name) => {
                let target_oc: Option<&str> = match name.as_str() {
                    "_posix_group_" => Some("posixGroup"),
                    "_posix_account_" => Some("posixAccount"),
                    "_any_" => None,
                    other => {
                        return self
                            .profiles
                            .iter()
                            .find(|p| p.name == other)
                            .map(scope_of);
                    }
                };
                match target_oc {
                    None => self.profiles.first().map(scope_of),
                    Some(oc) => self
                        .profiles
                        .iter()
                        .find(|p| {
                            p.object_classes
                                .iter()
                                .any(|o| o.eq_ignore_ascii_case(oc))
                        })
                        .map(scope_of),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::widget::ResolvedWidget;
    use crate::ldap::worker::RawSubschema;
    use crate::schema::model::SchemaModel;

    fn empty_schema() -> SchemaModel {
        SchemaModel::from_raw(&RawSubschema {
            object_classes: vec![],
            attribute_types: vec![],
            ldap_syntaxes: vec![],
        })
    }

    fn no_widgets() -> Vec<ResolvedWidget> {
        vec![]
    }
    fn no_profiles() -> Vec<EntryProfile> {
        vec![]
    }

    #[test]
    fn builtin_loginshell_is_choice_for_posixaccount() {
        let schema = empty_schema();
        let profiles = no_profiles();
        let widgets = no_widgets();
        let resolver = WidgetResolver::new(&schema, &profiles, &widgets, false);
        let kind = resolver.resolve_kind("loginShell", &["posixAccount".into()]);
        assert!(matches!(kind, Some(WidgetKind::Choice(_))), "got {kind:?}");
    }

    #[test]
    fn schema_no_user_modification_wins_over_nothing() {
        let raw = RawSubschema {
            object_classes: vec![],
            attribute_types: vec![
                "( 1.1 NAME 'opAttr' SYNTAX 1.3.6.1.4.1.1466.115.121.1.24 \
                 SINGLE-VALUE NO-USER-MODIFICATION USAGE directoryOperation )"
                    .into(),
            ],
            ldap_syntaxes: vec![],
        };
        let schema = SchemaModel::from_raw(&raw);
        let profiles = no_profiles();
        let widgets = no_widgets();
        let resolver = WidgetResolver::new(&schema, &profiles, &widgets, false);
        let kind = resolver.resolve_kind("opAttr", &["top".into()]);
        assert!(matches!(kind, Some(WidgetKind::Readonly)), "got {kind:?}");
    }

    #[test]
    fn explicit_profile_widget_overrides_builtin() {
        use crate::config::relation::Cardinality;
        use crate::config::widget::{ChoiceFormat, ChoiceWidget};
        let schema = empty_schema();
        let profiles = no_profiles();
        // Explicit profile widget for loginShell with different options.
        let explicit = vec![ResolvedWidget {
            owner_object_classes: vec!["posixAccount".into()],
            attr: "loginShell".into(),
            kind: WidgetKind::Choice(ChoiceWidget {
                select: Cardinality::Single,
                format: ChoiceFormat::Plain,
                options: vec![crate::config::ChoiceOption {
                    value: "/bin/custom".into(),
                    label: "Custom shell".into(),
                }],
            }),
        }];
        let resolver = WidgetResolver::new(&schema, &profiles, &explicit, false);
        let kind = resolver.resolve_kind("loginShell", &["posixAccount".into()]);
        if let Some(WidgetKind::Choice(w)) = kind {
            assert_eq!(w.options[0].value, "/bin/custom");
        } else {
            panic!("expected Choice, got {kind:?}");
        }
    }

    #[test]
    fn samba_sid_requires_samba_enabled() {
        let schema = empty_schema();
        let profiles = no_profiles();
        let widgets = no_widgets();
        let disabled = WidgetResolver::new(&schema, &profiles, &widgets, false);
        assert!(
            disabled
                .resolve_kind("sambaSID", &["sambaSamAccount".into()])
                .is_none(),
            "expected None when samba disabled"
        );

        let profiles2 = no_profiles();
        let widgets2 = no_widgets();
        let enabled = WidgetResolver::new(&schema, &profiles2, &widgets2, true);
        assert!(
            matches!(
                enabled.resolve_kind("sambaSID", &["sambaSamAccount".into()]),
                Some(WidgetKind::SambaSid)
            ),
            "expected SambaSid when samba enabled"
        );
    }
}
