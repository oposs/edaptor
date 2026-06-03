//! Pure helpers for the Create (ADD) flow (tty-free, unit-tested): composing a
//! new entry's DN + attribute set from a profile and the edited form, and
//! building an empty schema-driven [`FormModel`] for a profile's object class.

use std::collections::BTreeMap;

use anyhow::Result;

use crate::config::{EntryProfile, PasswordSpec};
use crate::form::changeset::{EditEntry, ModOp};
use crate::form::validate::{format_validation_errors, validate};
use crate::ldap::ldif::render_add;
use crate::schema::SchemaModel;
use crate::ui::form::{FormField, FormModel, WidgetSpec};

/// The two synthetic form-field labels for a password spec: the primary (the
/// configured LDAP attribute) and the confirmation field.
pub fn password_field_labels(spec: &PasswordSpec) -> (String, String) {
    (
        spec.ldap_attribute.clone(),
        format!("{} (confirm)", spec.ldap_attribute),
    )
}

/// Outcome of planning a create from a Create-mode form (pure).
pub enum CreatePrep {
    /// Ready to confirm: composed DN, attribute set, container, and LDIF preview.
    Confirm {
        dn: String,
        attrs: BTreeMap<String, Vec<String>>,
        container: String,
        ldif: String,
    },
    /// A blocking problem (RDN missing, schema validation failure).
    Error(String),
}

/// Pure: validate a Create-mode form's edited entry and produce the confirm data.
pub fn plan_create(
    schema: &SchemaModel,
    profile: &EntryProfile,
    container: &str,
    edited: &EditEntry,
) -> CreatePrep {
    let rdn_value = edited
        .attrs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(&profile.rdn_attr))
        .and_then(|(_, v)| v.first().cloned())
        .unwrap_or_default();
    if rdn_value.trim().is_empty() {
        return CreatePrep::Error("The RDN attribute must have a value.".to_string());
    }
    let (dn, attrs) = build_add_entry(profile, container, rdn_value.trim(), edited);
    let oc_refs: Vec<&str> = profile.object_classes.iter().map(String::as_str).collect();
    let full = EditEntry {
        dn: dn.clone(),
        attrs: attrs.clone(),
    };
    let errors = validate(&full, schema, &oc_refs);
    if !errors.is_empty() {
        return CreatePrep::Error(format_validation_errors(&errors));
    }
    let ldif = render_add(&dn, &attrs);
    CreatePrep::Confirm {
        dn,
        attrs,
        container: container.to_string(),
        ldif,
    }
}

/// Extract + validate the password from edited create/edit attrs. Removes BOTH
/// the primary and confirm pseudo-attributes from `attrs` (confirm is never a real
/// attribute). Returns `Ok(None)` when no password was entered, `Ok(Some(pw))` for
/// a confirmed password, `Err` when the two entries disagree. Pure.
pub fn stage_password(
    spec: &PasswordSpec,
    attrs: &mut BTreeMap<String, Vec<String>>,
) -> Result<Option<String>, String> {
    let (primary, confirm) = password_field_labels(spec);
    let take = |attrs: &BTreeMap<String, Vec<String>>, label: &str| {
        attrs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(label))
            .and_then(|(_, v)| v.first().cloned())
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    let pw = take(attrs, &primary);
    let cf = take(attrs, &confirm);
    attrs.retain(|k, _| !k.eq_ignore_ascii_case(&primary) && !k.eq_ignore_ascii_case(&confirm));
    if pw.is_empty() {
        return Ok(None);
    }
    if pw != cf {
        return Err("Passwords do not match.".to_string());
    }
    Ok(Some(pw))
}

/// A copy of `attrs` with the password-related attribute values masked, for the
/// LDIF confirm preview (never show the cleartext or the NT hash). Pure.
pub fn mask_password_attrs(
    attrs: &BTreeMap<String, Vec<String>>,
    ldap_attribute: &str,
) -> BTreeMap<String, Vec<String>> {
    let mut out = attrs.clone();
    for key in [ldap_attribute, "sambaNTPassword", "sambaPwdLastSet"] {
        if let Some(k) = out.keys().find(|k| k.eq_ignore_ascii_case(key)).cloned() {
            out.insert(k, vec!["********".to_string()]);
        }
    }
    out
}

/// Wall-clock seconds since the Unix epoch (0 on a pre-epoch clock). The one
/// impure call in the password paths; isolated so the planners stay pure.
pub fn now_unix_secs_or_zero() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The first profile that satisfies `pred` AND whose (non-empty) object classes
/// are all present (case-insensitive) in `entry_ocs` — i.e. the loaded entry is
/// an instance of that profile. Tie-break: config order (declare the more
/// specific profile first). Shared core of the password/lookup resolvers below;
/// only the `pred` differs, so the object-class subset check stays identical.
/// Pure.
fn profile_for_entry_where<'a>(
    profiles: &'a [EntryProfile],
    entry_ocs: &[String],
    pred: impl Fn(&EntryProfile) -> bool,
) -> Option<&'a EntryProfile> {
    profiles.iter().find(|p| {
        pred(p)
            && !p.object_classes.is_empty()
            && p.object_classes
                .iter()
                .all(|oc| entry_ocs.iter().any(|e| e.eq_ignore_ascii_case(oc)))
    })
}

/// The first configured profile that declares a `[profile.password]` block and
/// whose object classes all match `entry_ocs`. `None` when no password-profile
/// matches. Thin wrapper over [`profile_for_entry_where`]. Pure.
pub fn profile_for_entry<'a>(
    profiles: &'a [EntryProfile],
    entry_ocs: &[String],
) -> Option<&'a EntryProfile> {
    profile_for_entry_where(profiles, entry_ocs, |p| p.password.is_some())
}

/// Edit-path password mods: the same `(attr, values)` pairs as create
/// (`password_add_attrs`), mapped to REPLACE ops so the new credential overwrites
/// the old within one atomic MODIFY. Honors `ldap_attribute` and Samba. Pure.
fn password_replace_mods(
    clear: &str,
    ldap_attribute: &str,
    samba: bool,
    now_secs: u64,
) -> Vec<ModOp> {
    crate::samba::password::password_add_attrs(clear, ldap_attribute, samba, now_secs)
        .into_iter()
        .map(|(attr, values)| ModOp::Replace { attr, values })
        .collect()
}

/// Compute the password contribution to an edit save. Always strips the password
/// pseudo-fields (primary + confirm) from BOTH `baseline` and `edited`, so the
/// injected masked field never appears as an attribute diff — an un-stripped
/// baseline still carrying the directory's stored hash would otherwise diff to a
/// spurious Delete. When a confirmed new password was entered, also strips the
/// Samba secret attrs from both sides (the REPLACE mods are then their sole
/// source) and returns those mods plus the attrs to mask in the preview. Returns
/// empty vecs when the field was left blank. Pure (clock injected as `now_secs`).
pub fn stage_edit_password(
    spec: &PasswordSpec,
    object_classes: &[String],
    baseline: &mut BTreeMap<String, Vec<String>>,
    edited: &mut BTreeMap<String, Vec<String>>,
    now_secs: u64,
) -> Result<(Vec<ModOp>, Vec<String>), String> {
    let (primary, confirm) = password_field_labels(spec);
    let strip = |m: &mut BTreeMap<String, Vec<String>>, labels: &[&str]| {
        m.retain(|k, _| !labels.iter().any(|l| k.eq_ignore_ascii_case(l)));
    };
    // `primary` == spec.ldap_attribute; drop both pseudo-fields from the baseline
    // so the stored value never diffs against the (blank) form field.
    strip(baseline, &[primary.as_str(), confirm.as_str()]);
    // stage_password validates the confirm match and removes both pseudo-fields
    // from `edited`, returning the cleartext (or None when left blank).
    let clear = match stage_password(spec, edited)? {
        Some(pw) => pw,
        None => return Ok((Vec::new(), Vec::new())),
    };
    let samba = spec.samba
        && object_classes
            .iter()
            .any(|o| o.eq_ignore_ascii_case("sambaSamAccount"));
    if samba {
        strip(baseline, &["sambaNTPassword", "sambaPwdLastSet"]);
        strip(edited, &["sambaNTPassword", "sambaPwdLastSet"]);
    }
    let mods = password_replace_mods(&clear, &spec.ldap_attribute, samba, now_secs);
    let mut mask = vec![spec.ldap_attribute.clone()];
    if samba {
        mask.push("sambaNTPassword".to_string());
    }
    Ok((mods, mask))
}

/// Apply literal/template defaults to still-empty fields (pure); return the
/// autonumber requests `(attr, min, max)` that still need a directory scan.
pub fn apply_static_defaults(
    defaults: &crate::config::defaults::ProfileDefaults,
    attrs: &mut BTreeMap<String, Vec<String>>,
) -> Vec<(String, u64, u64)> {
    use crate::config::defaults::{plan_defaults, Resolution};
    let mut autonum = Vec::new();
    for res in plan_defaults(defaults, attrs) {
        match res {
            Resolution::Fill { attr, value } => {
                attrs.insert(attr, vec![value]);
            }
            Resolution::NeedsAutonumber { attr, min, max } => autonum.push((attr, min, max)),
        }
    }
    autonum
}

/// Indices of profiles whose `search_base` matches `container_dn` at a DN-component
/// boundary: equal, or one is a proper suffix of the other (case-insensitive). So
/// `ou=people,…` matches but `ou=people2,…` does not. Profiles with an empty
/// `search_base` never match. Pure.
pub fn profiles_for_container(profiles: &[EntryProfile], container_dn: &str) -> Vec<usize> {
    profiles
        .iter()
        .enumerate()
        .filter(|(_, p)| {
            !p.search_base.is_empty() && dn_boundary_match(&p.search_base, container_dn)
        })
        .map(|(i, _)| i)
        .collect()
}

/// True when `a` == `b` or one ends with `,<other>` (case-insensitive): a match at
/// a DN-component boundary, so `ou=people2,dc=x` does NOT match `ou=people,dc=x`.
fn dn_boundary_match(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim().to_lowercase(), b.trim().to_lowercase());
    if a == b {
        return true;
    }
    a.ends_with(&format!(",{b}")) || b.ends_with(&format!(",{a}"))
}

/// Compose the DN and attribute set for a new entry of `profile`'s object class.
///
/// The DN is `<rdn_attr>=<rdn_value>,<container_dn>`. The attribute set is the
/// edited form's non-empty attributes merged with the canonical objectClass set
/// `["top"] + profile.object_classes` (deduped, case-insensitive; the server fills
/// in inherited superclasses) and the RDN attribute (ensuring it carries the RDN
/// value even if the form omitted it). Pure.
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
    use crate::workflows::test_fixtures::*;

    fn profile() -> EntryProfile {
        EntryProfile {
            name: "Users".to_string(),
            object_classes: vec!["inetOrgPerson".to_string()],
            rdn_attr: "uid".to_string(),
            search_base: "ou=people,dc=example,dc=org".to_string(),
            show: vec!["uid".to_string(), "cn".to_string(), "sn".to_string()],
            search_attrs: vec![],
            defaults: Default::default(),
            password: None,
            pickers: Default::default(),
            label: None,
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

    fn prof(base: &str) -> EntryProfile {
        EntryProfile {
            search_base: base.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn profiles_for_container_matches_exact_and_descendant() {
        let ps = vec![
            prof("ou=people,dc=example,dc=org"),
            prof("ou=groups,dc=example,dc=org"),
        ];
        // Exact container → just that profile.
        assert_eq!(
            profiles_for_container(&ps, "ou=people,dc=example,dc=org"),
            vec![0]
        );
        // A parent container offers all profiles whose search_base is under it.
        assert_eq!(profiles_for_container(&ps, "dc=example,dc=org"), vec![0, 1]);
    }

    #[test]
    fn profiles_for_container_rejects_non_boundary_prefix() {
        let ps = vec![prof("ou=people,dc=example,dc=org")];
        assert!(profiles_for_container(&ps, "ou=people2,dc=example,dc=org").is_empty());
    }

    #[test]
    fn profiles_for_container_is_case_insensitive() {
        let ps = vec![prof("OU=People,DC=Example,DC=Org")];
        assert_eq!(
            profiles_for_container(&ps, "ou=people,dc=example,dc=org"),
            vec![0]
        );
    }

    #[test]
    fn profiles_for_container_empty_search_base_never_matches() {
        let ps = vec![prof("")];
        assert!(profiles_for_container(&ps, "dc=example,dc=org").is_empty());
    }

    #[test]
    fn plan_create_builds_confirm_with_composed_dn() {
        use std::collections::BTreeMap;
        let mut attrs = BTreeMap::new();
        attrs.insert("uid".to_string(), vec!["alice".to_string()]);
        let edited = EditEntry {
            dn: String::new(),
            attrs,
        };
        let prep = plan_create(
            &user_schema(),
            &create_user_profile(),
            "ou=people,dc=example,dc=org",
            &edited,
        );
        match prep {
            CreatePrep::Confirm { dn, .. } => {
                assert_eq!(dn, "uid=alice,ou=people,dc=example,dc=org")
            }
            CreatePrep::Error(e) => panic!("unexpected error: {e}"),
        }
    }

    #[test]
    fn plan_create_errors_when_rdn_missing() {
        use std::collections::BTreeMap;
        let edited = EditEntry {
            dn: String::new(),
            attrs: BTreeMap::new(),
        };
        let prep = plan_create(
            &user_schema(),
            &create_user_profile(),
            "ou=people,dc=example,dc=org",
            &edited,
        );
        assert!(matches!(prep, CreatePrep::Error(_)));
    }

    #[test]
    fn stage_password_strips_fields_validates_match_and_empty() {
        use crate::config::PasswordSpec;
        use std::collections::BTreeMap;
        let spec = PasswordSpec {
            ldap_attribute: "userPassword".into(),
            samba: false,
        };
        // matching pair → Some, both pseudo-fields stripped, other attrs kept
        let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        attrs.insert("userPassword".into(), vec!["hunter2".into()]);
        attrs.insert("userPassword (confirm)".into(), vec!["hunter2".into()]);
        attrs.insert("cn".into(), vec!["Alice".into()]);
        assert_eq!(
            stage_password(&spec, &mut attrs).unwrap(),
            Some("hunter2".to_string())
        );
        assert!(!attrs.contains_key("userPassword"));
        assert!(!attrs.contains_key("userPassword (confirm)"));
        assert!(attrs.contains_key("cn"));
        // mismatch → Err
        let mut a2: BTreeMap<String, Vec<String>> = BTreeMap::new();
        a2.insert("userPassword".into(), vec!["a".into()]);
        a2.insert("userPassword (confirm)".into(), vec!["b".into()]);
        assert!(stage_password(&spec, &mut a2).is_err());
        // empty → None
        let mut a3: BTreeMap<String, Vec<String>> = BTreeMap::new();
        a3.insert("userPassword".into(), vec!["".into()]);
        a3.insert("userPassword (confirm)".into(), vec!["".into()]);
        assert_eq!(stage_password(&spec, &mut a3).unwrap(), None);
    }

    #[test]
    fn mask_password_attrs_masks_secret_values_only() {
        use std::collections::BTreeMap;
        let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        attrs.insert("userPassword".into(), vec!["hunter2".into()]);
        attrs.insert("sambaNTPassword".into(), vec!["DEADBEEF".into()]);
        attrs.insert("cn".into(), vec!["Alice".into()]);
        let m = mask_password_attrs(&attrs, "userPassword");
        assert_eq!(m.get("userPassword"), Some(&vec!["********".to_string()]));
        assert_eq!(
            m.get("sambaNTPassword"),
            Some(&vec!["********".to_string()])
        );
        assert_eq!(m.get("cn"), Some(&vec!["Alice".to_string()]));
    }

    #[test]
    fn profile_for_entry_requires_oc_subset_and_password_spec() {
        use crate::config::PasswordSpec;
        let mut pw_user = create_user_profile();
        pw_user.object_classes = vec!["inetOrgPerson".into(), "posixAccount".into()];
        pw_user.password = Some(PasswordSpec {
            ldap_attribute: "userPassword".into(),
            samba: false,
        });
        // A profile with no password block must never match.
        let mut plain = create_user_profile();
        plain.object_classes = vec!["inetOrgPerson".into()];
        plain.password = None;
        let profiles = vec![plain, pw_user];

        let ocs = vec![
            "top".to_string(),
            "inetOrgPerson".to_string(),
            "posixAccount".to_string(),
        ];
        let m = profile_for_entry(&profiles, &ocs).expect("password profile matches");
        assert!(m.password.is_some());
        assert_eq!(m.object_classes.len(), 2);
        // Entry missing posixAccount: the 2-OC profile no longer matches, and the
        // plain profile has no password → None.
        assert!(profile_for_entry(&profiles, &["inetOrgPerson".to_string()]).is_none());
    }

    #[test]
    fn stage_edit_password_blank_yields_no_mods_and_strips_pseudo_fields() {
        use std::collections::BTreeMap;
        // baseline carries the directory's stored hash; edited carries the blank
        // injected fields. After staging, neither side keeps the password attr.
        let mut baseline: BTreeMap<String, Vec<String>> = BTreeMap::new();
        baseline.insert("userPassword".into(), vec!["{SSHA}deadbeef".into()]);
        baseline.insert("cn".into(), vec!["Alice".into()]);
        let mut edited: BTreeMap<String, Vec<String>> = BTreeMap::new();
        edited.insert("userPassword".into(), vec!["".into()]);
        edited.insert("userPassword (confirm)".into(), vec!["".into()]);
        edited.insert("cn".into(), vec!["Alice".into()]);

        let (mods, mask) = stage_edit_password(
            &pw_spec(false),
            &[],
            &mut baseline,
            &mut edited,
            1_700_000_000,
        )
        .unwrap();
        assert!(mods.is_empty(), "blank password produces no mods");
        assert!(mask.is_empty());
        assert!(
            !baseline.contains_key("userPassword"),
            "baseline hash stripped"
        );
        assert!(!edited.contains_key("userPassword"));
        assert!(!edited.contains_key("userPassword (confirm)"));
        assert!(baseline.contains_key("cn") && edited.contains_key("cn"));
    }

    #[test]
    fn stage_edit_password_set_yields_replace_and_strips_baseline_hash() {
        use std::collections::BTreeMap;
        let mut baseline: BTreeMap<String, Vec<String>> = BTreeMap::new();
        baseline.insert("userPassword".into(), vec!["{SSHA}old".into()]);
        let mut edited: BTreeMap<String, Vec<String>> = BTreeMap::new();
        edited.insert("userPassword".into(), vec!["hunter2".into()]);
        edited.insert("userPassword (confirm)".into(), vec!["hunter2".into()]);

        let (mods, mask) = stage_edit_password(
            &pw_spec(false),
            &[],
            &mut baseline,
            &mut edited,
            1_700_000_000,
        )
        .unwrap();
        assert_eq!(
            mods,
            vec![ModOp::Replace {
                attr: "userPassword".into(),
                values: vec!["hunter2".into()],
            }]
        );
        assert_eq!(mask, vec!["userPassword".to_string()]);
        assert!(!baseline.contains_key("userPassword"), "old hash stripped");
    }

    #[test]
    fn stage_edit_password_samba_includes_nt_hash_and_strips_samba_attrs() {
        use std::collections::BTreeMap;
        let mut baseline: BTreeMap<String, Vec<String>> = BTreeMap::new();
        baseline.insert("sambaNTPassword".into(), vec!["OLDHASH".into()]);
        baseline.insert("sambaPwdLastSet".into(), vec!["1".into()]);
        let mut edited: BTreeMap<String, Vec<String>> = BTreeMap::new();
        edited.insert("userPassword".into(), vec!["hunter2".into()]);
        edited.insert("userPassword (confirm)".into(), vec!["hunter2".into()]);

        let ocs = vec!["sambaSamAccount".to_string()];
        let (mods, mask) = stage_edit_password(
            &pw_spec(true),
            &ocs,
            &mut baseline,
            &mut edited,
            1_700_000_000,
        )
        .unwrap();
        // The NT hash REPLACE is present and equals the M5 nthash of the cleartext.
        assert!(mods.contains(&ModOp::Replace {
            attr: "sambaNTPassword".into(),
            values: vec![crate::samba::nthash::nt_hash("hunter2")],
        }));
        assert!(mask.contains(&"sambaNTPassword".to_string()));
        assert!(
            !baseline.contains_key("sambaNTPassword"),
            "old NT hash stripped"
        );
        assert!(!baseline.contains_key("sambaPwdLastSet"));
    }

    #[test]
    fn stage_edit_password_mismatch_errors() {
        use std::collections::BTreeMap;
        let mut baseline: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut edited: BTreeMap<String, Vec<String>> = BTreeMap::new();
        edited.insert("userPassword".into(), vec!["a".into()]);
        edited.insert("userPassword (confirm)".into(), vec!["b".into()]);
        assert!(
            stage_edit_password(&pw_spec(false), &[], &mut baseline, &mut edited, 0).is_err(),
            "confirm mismatch must error"
        );
    }

    #[test]
    fn apply_static_defaults_fills_literals_templates_and_surfaces_autonumber() {
        use crate::config::defaults::{parse_default_value, DefaultValue, ProfileDefaults};
        use std::collections::BTreeMap;
        let mut d = ProfileDefaults::default();
        d.entries.insert(
            "loginShell".into(),
            DefaultValue::Literal("/bin/bash".into()),
        );
        d.entries.insert(
            "homeDirectory".into(),
            parse_default_value("/home/{uid}").unwrap(),
        );
        d.entries.insert(
            "uidNumber".into(),
            parse_default_value("{next:10000-60000}").unwrap(),
        );
        let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        attrs.insert("uid".into(), vec!["alice".into()]);
        let autonum = apply_static_defaults(&d, &mut attrs);
        assert_eq!(
            attrs.get("loginShell"),
            Some(&vec!["/bin/bash".to_string()])
        );
        assert_eq!(
            attrs.get("homeDirectory"),
            Some(&vec!["/home/alice".to_string()])
        );
        // autonumber is NOT filled here (needs a worker scan); it's surfaced.
        assert_eq!(autonum, vec![("uidNumber".to_string(), 10000, 60000)]);
        assert!(!attrs.contains_key("uidNumber"));
    }
}
