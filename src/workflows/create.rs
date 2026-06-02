//! Pure helpers for the Create (ADD) flow (tty-free, unit-tested): composing a
//! new entry's DN + attribute set from a profile and the edited form, and
//! building an empty schema-driven [`FormModel`] for a profile's object class.

use std::collections::BTreeMap;

use crate::config::EntryProfile;
use crate::form::changeset::EditEntry;
use crate::schema::SchemaModel;
use crate::ui::form::{FormField, FormModel, WidgetSpec};

/// Compose the DN and attribute set for a new entry of `profile`'s object class.
///
/// The DN is `<rdn_attr>=<rdn_value>,<container_dn>`. The attribute set is the
/// edited form's non-empty attributes merged with the fixed objectClass values
/// `["top", profile.object_class]` (Decision D2 — the server fills in inherited
/// superclasses; no objectClass picker in M4) and the RDN attribute (ensuring it
/// carries the RDN value even if the form omitted it). Pure.
pub fn build_add_entry(
    profile: &EntryProfile,
    container_dn: &str,
    rdn_value: &str,
    edited: &EditEntry,
) -> (String, BTreeMap<String, Vec<String>>) {
    let dn = format!("{}={},{}", profile.rdn_attr, rdn_value, container_dn);

    let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    // Edited (non-empty) attributes first.
    for (k, vs) in &edited.attrs {
        let non_empty: Vec<String> = vs
            .iter()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .collect();
        if !non_empty.is_empty() {
            attrs.insert(k.clone(), non_empty);
        }
    }

    // Fixed objectClass set: "top" first, then each profile class, deduped case-insensitively.
    let mut oc: Vec<String> = vec!["top".to_string()];
    for c in &profile.object_classes {
        if !oc.iter().any(|x| x.eq_ignore_ascii_case(c)) {
            oc.push(c.clone());
        }
    }
    attrs.insert("objectClass".to_string(), oc);

    // Ensure the RDN attribute carries the RDN value.
    match attrs
        .iter_mut()
        .find(|(k, _)| k.eq_ignore_ascii_case(&profile.rdn_attr))
    {
        Some((_, vs)) => {
            if !vs.iter().any(|v| v == rdn_value) {
                vs.insert(0, rdn_value.to_string());
            }
        }
        None => {
            attrs.insert(profile.rdn_attr.clone(), vec![rdn_value.to_string()]);
        }
    }

    (dn, attrs)
}

/// Build an empty schema-driven [`FormModel`] for creating a new entry of the
/// profile's object class: one (empty) field per effective MUST then MAY
/// attribute (excluding `objectClass`, which is fixed by the profile), ordered by
/// the profile's `show` list first. The title is a placeholder describing the new
/// entry. Pure.
pub fn empty_form_for_profile(schema: &SchemaModel, profile: &EntryProfile) -> FormModel {
    let oc_refs: Vec<&str> = profile.object_classes.iter().map(String::as_str).collect();
    let resolved = schema.effective_attributes(&oc_refs);

    let is_must = |attr: &str| resolved.must.iter().any(|m| m.eq_ignore_ascii_case(attr));
    let already =
        |ordered: &[String], attr: &str| ordered.iter().any(|a| a.eq_ignore_ascii_case(attr));
    let in_effective = |attr: &str| {
        resolved.must.iter().any(|m| m.eq_ignore_ascii_case(attr))
            || resolved.may.iter().any(|m| m.eq_ignore_ascii_case(attr))
    };

    let mut ordered: Vec<String> = Vec::new();
    for attr in &profile.show {
        if in_effective(attr) && !already(&ordered, attr) {
            ordered.push(attr.clone());
        }
    }
    for attr in &resolved.must {
        if !already(&ordered, attr) {
            ordered.push(attr.clone());
        }
    }
    for attr in &resolved.may {
        if !already(&ordered, attr) {
            ordered.push(attr.clone());
        }
    }

    let fields = ordered
        .into_iter()
        .filter(|attr| !attr.eq_ignore_ascii_case("objectClass"))
        .map(|attr| {
            let kind = schema.field_kind(&attr);
            FormField {
                is_must: is_must(&attr),
                kind,
                values: Vec::new(),
                widget: WidgetSpec::ReadOnlyText, // editability is decided by build_edit_form
                label: attr,
            }
        })
        .collect();

    FormModel {
        title: format!("New {}", profile.name),
        fields,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldap::worker::RawSubschema;

    fn profile() -> EntryProfile {
        EntryProfile {
            name: "Users".to_string(),
            object_classes: vec!["inetOrgPerson".to_string()],
            rdn_attr: "uid".to_string(),
            search_base: "ou=people,dc=example,dc=org".to_string(),
            show: vec!["uid".to_string(), "cn".to_string(), "sn".to_string()],
            search_attrs: vec![],
        }
    }

    fn edited() -> EditEntry {
        let mut attrs = BTreeMap::new();
        attrs.insert("uid".to_string(), vec!["alice".to_string()]);
        attrs.insert("cn".to_string(), vec!["Alice".to_string()]);
        attrs.insert("sn".to_string(), vec!["Adams".to_string()]);
        attrs.insert("description".to_string(), vec!["".to_string()]); // empty -> dropped
        EditEntry {
            dn: String::new(),
            attrs,
        }
    }

    #[test]
    fn build_add_composes_dn() {
        let (dn, _) = build_add_entry(
            &profile(),
            "ou=people,dc=example,dc=org",
            "alice",
            &edited(),
        );
        assert_eq!(dn, "uid=alice,ou=people,dc=example,dc=org");
    }

    #[test]
    fn build_add_includes_objectclasses() {
        let (_, attrs) = build_add_entry(
            &profile(),
            "ou=people,dc=example,dc=org",
            "alice",
            &edited(),
        );
        let oc = attrs.get("objectClass").expect("objectClass present");
        assert!(oc.contains(&"top".to_string()));
        assert!(oc.contains(&"inetOrgPerson".to_string()));
    }

    #[test]
    fn build_add_includes_all_object_classes_top_first_deduped() {
        let mut p = profile();
        p.object_classes = vec!["inetOrgPerson".into(), "posixAccount".into(), "top".into()];
        let (_, attrs) = build_add_entry(&p, "ou=people,dc=example,dc=org", "alice", &edited());
        let oc = attrs.get("objectClass").unwrap();
        assert_eq!(oc[0], "top");
        assert!(oc.contains(&"inetOrgPerson".to_string()));
        assert!(oc.contains(&"posixAccount".to_string()));
        assert_eq!(
            oc.iter().filter(|v| v.eq_ignore_ascii_case("top")).count(),
            1
        );
    }

    #[test]
    fn build_add_includes_must_attrs_and_drops_empty() {
        let (_, attrs) = build_add_entry(
            &profile(),
            "ou=people,dc=example,dc=org",
            "alice",
            &edited(),
        );
        assert_eq!(attrs.get("uid"), Some(&vec!["alice".to_string()]));
        assert_eq!(attrs.get("cn"), Some(&vec!["Alice".to_string()]));
        assert_eq!(attrs.get("sn"), Some(&vec!["Adams".to_string()]));
        assert!(!attrs.contains_key("description"));
    }

    #[test]
    fn build_add_supplies_rdn_when_form_omits_it() {
        let mut e = edited();
        e.attrs.remove("uid");
        let (_, attrs) = build_add_entry(&profile(), "ou=people,dc=example,dc=org", "bob", &e);
        assert_eq!(attrs.get("uid"), Some(&vec!["bob".to_string()]));
    }

    fn schema() -> SchemaModel {
        let raw = RawSubschema {
            object_classes: vec![
                "( 2.5.6.0 NAME 'top' ABSTRACT MUST objectClass )".to_string(),
                "( 2.5.6.6 NAME 'person' SUP top STRUCTURAL MUST ( sn $ cn ) MAY description )"
                    .to_string(),
                "( 2.16.840.1.113730.3.2.2 NAME 'inetOrgPerson' SUP person STRUCTURAL \
                  MAY ( mail $ uid ) )"
                    .to_string(),
            ],
            attribute_types: vec![
                "( 2.5.4.3 NAME 'cn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".to_string(),
                "( 2.5.4.4 NAME 'sn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".to_string(),
                "( 0.9.2342.19200300.100.1.1 NAME 'uid' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )"
                    .to_string(),
            ],
            ldap_syntaxes: vec![],
        };
        SchemaModel::from_raw(&raw)
    }

    #[test]
    fn empty_form_has_must_and_may_but_not_objectclass() {
        let model = empty_form_for_profile(&schema(), &profile());
        let labels: Vec<&str> = model.fields.iter().map(|f| f.label.as_str()).collect();
        assert!(labels.iter().any(|l| l.eq_ignore_ascii_case("cn")));
        assert!(labels.iter().any(|l| l.eq_ignore_ascii_case("sn")));
        assert!(labels.iter().any(|l| l.eq_ignore_ascii_case("uid")));
        assert!(!labels.iter().any(|l| l.eq_ignore_ascii_case("objectClass")));
        assert!(model.fields.iter().all(|f| f.values.is_empty()));
        assert!(model
            .fields
            .iter()
            .any(|f| f.label.eq_ignore_ascii_case("sn") && f.is_must));
    }

    #[test]
    fn empty_form_orders_by_profile_show() {
        let model = empty_form_for_profile(&schema(), &profile());
        let labels: Vec<&str> = model.fields.iter().map(|f| f.label.as_str()).collect();
        assert_eq!(&labels[..3], &["uid", "cn", "sn"]);
    }
}
