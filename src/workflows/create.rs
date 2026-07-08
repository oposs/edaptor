//! Pure helpers for the Create (ADD) flow (tty-free, unit-tested): composing a
//! new entry's DN + attribute set from a profile and the edited form, and
//! building an empty schema-driven [`FormModel`] for a profile's object class.

use std::collections::BTreeMap;

use crate::config::EntryProfile;
use crate::form::changeset::EditEntry;
use crate::form::validate::{format_validation_errors, validate};
use crate::ldap::ldif::render_add;
use crate::schema::SchemaModel;
use crate::workflows::form_model::{FormField, FormModel, WidgetSpec};

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
    let errors = validate(&full, schema, &oc_refs, &[]);
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

/// Fold a staged create-form password into a new entry's attribute set (pure;
/// clock injected). When a cleartext `pending` is set AND the new entry's object
/// classes match a password widget, inserts `password_add_attrs` (primary +
/// optional Samba secrets) into `attrs` and returns the masked LDIF preview body.
/// Otherwise leaves `attrs` untouched and returns `None` (the caller keeps its
/// plain preview). The Samba contribution is driven solely by the widget's
/// `samba` flag, matching the edit path's [`stage_pending_password`].
pub fn fold_create_password(
    dn: &str,
    attrs: &mut BTreeMap<String, Vec<String>>,
    pending: Option<&str>,
    widgets: &[crate::config::widget::ResolvedWidget],
    now_secs: u64,
) -> Option<String> {
    let ocs: Vec<String> = attrs.get("objectClass").cloned().unwrap_or_default();
    match (
        pending,
        crate::config::widget::password_widget_for(widgets, &ocs),
    ) {
        (Some(clear), Some(pw)) => {
            for (k, v) in
                crate::samba::password::password_add_attrs(clear, &pw.primary, pw.samba, now_secs)
            {
                attrs.insert(k, v);
            }
            Some(render_add(dn, &mask_password_attrs(attrs, &pw.primary)))
        }
        _ => None,
    }
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

/// The first configured profile that declares a password widget
/// (`[profile.widget.<attr>] kind = "password"`) and whose object classes all
/// match `entry_ocs`. `None` when no password-profile matches. Thin wrapper over
/// [`profile_for_entry_where`]. Pure.
pub fn profile_for_entry<'a>(
    profiles: &'a [EntryProfile],
    entry_ocs: &[String],
) -> Option<&'a EntryProfile> {
    profile_for_entry_where(profiles, entry_ocs, |p| {
        p.widgets
            .values()
            .any(|w| matches!(w, crate::config::WidgetSpecCfg::Password { .. }))
    })
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

/// Compose a create-mode [`EditForm`] for `profile` under `container`: a schema-driven
/// empty form (`empty_form_for_profile`), with an editable `objectClass` field seeded
/// with `["top"] + profile.object_classes` (deduped, case-insensitive) so the picker
/// can edit it and `sync_schema_fields` injects the effective MUST/MAY fields; then
/// static defaults are applied. Returns the form plus the autonumber requests
/// `(attr, min, max)` that still need a directory scan (Block B fills them). Pure.
pub fn build_create_form(
    schema: &SchemaModel,
    profile: &EntryProfile,
    profile_idx: usize,
    container: &str,
) -> (
    crate::workflows::edit_form::EditForm,
    Vec<(String, u64, u64)>,
) {
    use crate::schema::FieldKind;
    use crate::workflows::edit_form::{build_edit_form, EditField, FormMode};
    use crate::workflows::form_model::WidgetSpec;

    let model = empty_form_for_profile(schema, profile);
    let mut form = build_edit_form(&model, schema, false);
    form.mode = FormMode::Create {
        profile_idx,
        container: container.to_string(),
    };

    // Canonical objectClass set: ["top"] + profile classes, deduped case-insensitively.
    let mut ocs: Vec<String> = vec!["top".to_string()];
    for oc in &profile.object_classes {
        if !ocs.iter().any(|x| x.eq_ignore_ascii_case(oc)) {
            ocs.push(oc.clone());
        }
    }
    form.object_classes = ocs.clone();

    // Ensure an editable objectClass field carrying that set (auto-injection).
    if let Some(f) = form
        .fields
        .iter_mut()
        .find(|f| f.label.eq_ignore_ascii_case("objectClass"))
    {
        f.editable = true;
        f.values = ocs.clone();
    } else {
        form.fields.push(EditField {
            label: "objectClass".to_string(),
            must: true,
            editable: true,
            multi: true,
            secret: false,
            ordered: false,
            orphaned: false,
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: None,
            values: ocs.clone(),
            baseline: Vec::new(),
        });
    }

    // Regenerate fields for the seeded objectClass set.
    form.sync_schema_fields(schema);

    // Apply static defaults; collect autonumber requests. Work on an attrs map, then
    // write filled values back into the (still-empty) fields.
    let mut attrs: std::collections::BTreeMap<String, Vec<String>> = form
        .fields
        .iter()
        .map(|f| (f.label.clone(), f.values.clone()))
        .collect();
    let autonum = apply_static_defaults(&profile.defaults, &mut attrs);
    for f in &mut form.fields {
        if f.values.is_empty() {
            if let Some(v) = attrs.get(&f.label) {
                if !v.is_empty() {
                    f.values = v.clone();
                }
            }
        }
    }

    (form, autonum)
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
            widgets: Default::default(),
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
    fn profile_for_entry_requires_oc_subset_and_password_widget() {
        use crate::config::WidgetSpecCfg;
        let mut pw_user = create_user_profile();
        pw_user.object_classes = vec!["inetOrgPerson".into(), "posixAccount".into()];
        pw_user.widgets.insert(
            "userPassword".into(),
            WidgetSpecCfg::Password { samba: false },
        );
        // A profile with no password widget must never match.
        let mut plain = create_user_profile();
        plain.object_classes = vec!["inetOrgPerson".into()];
        plain.widgets.clear();
        let profiles = vec![plain, pw_user];

        let ocs = vec![
            "top".to_string(),
            "inetOrgPerson".to_string(),
            "posixAccount".to_string(),
        ];
        let m = profile_for_entry(&profiles, &ocs).expect("password profile matches");
        assert!(m
            .widgets
            .values()
            .any(|w| matches!(w, WidgetSpecCfg::Password { .. })));
        assert_eq!(m.object_classes.len(), 2);
        // Entry missing posixAccount: the 2-OC profile no longer matches, and the
        // plain profile has no password widget → None.
        assert!(profile_for_entry(&profiles, &["inetOrgPerson".to_string()]).is_none());
    }

    fn pw_widget(samba: bool) -> crate::config::widget::ResolvedWidget {
        crate::config::widget::ResolvedWidget {
            owner_object_classes: vec!["inetOrgPerson".into()],
            attr: "userPassword".into(),
            kind: crate::config::widget::WidgetKind::Password(
                crate::config::widget::PasswordWidget {
                    primary: "userPassword".into(),
                    derived: if samba {
                        vec!["sambaNTPassword".into(), "sambaPwdLastSet".into()]
                    } else {
                        Vec::new()
                    },
                    samba,
                },
            ),
        }
    }

    fn add_attrs() -> BTreeMap<String, Vec<String>> {
        let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        attrs.insert(
            "objectClass".into(),
            vec!["top".into(), "inetOrgPerson".into()],
        );
        attrs.insert("uid".into(), vec!["alice".into()]);
        attrs
    }

    #[test]
    fn fold_create_password_inserts_userpassword_when_staged() {
        let widgets = vec![pw_widget(false)];
        let mut attrs = add_attrs();
        let body = fold_create_password(
            "uid=alice,ou=people,dc=example,dc=org",
            &mut attrs,
            Some("hunter2"),
            &widgets,
            1_700_000_000,
        )
        .expect("staged password yields a masked preview");
        assert_eq!(
            attrs.get("userPassword"),
            Some(&vec!["hunter2".to_string()])
        );
        assert!(!attrs.contains_key("sambaNTPassword"));
        // The preview body masks the cleartext.
        assert!(body.contains("********"));
        assert!(!body.contains("hunter2"));
    }

    #[test]
    fn fold_create_password_includes_samba_attrs_when_samba_widget() {
        let widgets = vec![pw_widget(true)];
        let mut attrs = add_attrs();
        let body = fold_create_password(
            "uid=alice,ou=people,dc=example,dc=org",
            &mut attrs,
            Some("hunter2"),
            &widgets,
            1_700_000_000,
        )
        .expect("staged samba password yields a masked preview");
        assert_eq!(
            attrs.get("userPassword"),
            Some(&vec!["hunter2".to_string()])
        );
        assert_eq!(
            attrs.get("sambaNTPassword"),
            Some(&vec![crate::samba::nthash::nt_hash("hunter2")])
        );
        assert_eq!(
            attrs.get("sambaPwdLastSet"),
            Some(&vec!["1700000000".to_string()])
        );
        assert!(!body.contains("hunter2"));
    }

    #[test]
    fn fold_create_password_omits_when_no_pending() {
        let widgets = vec![pw_widget(false)];
        let mut attrs = add_attrs();
        let body = fold_create_password(
            "uid=alice,ou=people,dc=example,dc=org",
            &mut attrs,
            None,
            &widgets,
            1_700_000_000,
        );
        assert!(body.is_none(), "no staged password keeps the plain preview");
        assert!(!attrs.contains_key("userPassword"));
        assert!(!attrs.contains_key("sambaNTPassword"));
    }

    #[test]
    fn fold_create_password_omits_when_no_password_widget() {
        let mut attrs = add_attrs();
        let body = fold_create_password(
            "uid=alice,ou=people,dc=example,dc=org",
            &mut attrs,
            Some("hunter2"),
            &[],
            1_700_000_000,
        );
        assert!(body.is_none());
        assert!(!attrs.contains_key("userPassword"));
    }

    #[test]
    fn build_create_form_injects_objectclass_and_resolves_fields() {
        // schema with person (MUST sn,cn MAY description) + organizationalPerson.
        let raw = crate::ldap::worker::RawSubschema {
            object_classes: vec![
                "( 2.5.6.0 NAME 'top' ABSTRACT MUST objectClass )".into(),
                "( 2.5.6.6 NAME 'person' SUP top STRUCTURAL MUST ( sn $ cn ) MAY description )"
                    .into(),
            ],
            attribute_types: vec![
                "( 2.5.4.3 NAME 'cn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 SINGLE-VALUE )".into(),
                "( 2.5.4.4 NAME 'sn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".into(),
                "( 2.5.4.13 NAME 'description' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".into(),
            ],
            ldap_syntaxes: vec![],
        };
        let schema = crate::schema::SchemaModel::from_raw(&raw);
        let profile = EntryProfile {
            name: "People".into(),
            object_classes: vec!["person".into()],
            rdn_attr: "cn".into(),
            search_base: "ou=people,dc=example,dc=org".into(),
            show: vec![],
            search_attrs: vec![],
            defaults: Default::default(),
            widgets: Default::default(),
            label: None,
        };
        let (form, autonum) =
            build_create_form(&schema, &profile, 0, "ou=people,dc=example,dc=org");
        assert!(matches!(
            form.mode,
            crate::workflows::edit_form::FormMode::Create { profile_idx: 0, .. }
        ));
        let oc = form
            .fields
            .iter()
            .find(|f| f.label.eq_ignore_ascii_case("objectClass"))
            .unwrap();
        assert!(oc.editable, "objectClass field must be editable");
        assert!(oc.values.iter().any(|v| v.eq_ignore_ascii_case("top")));
        assert!(oc.values.iter().any(|v| v.eq_ignore_ascii_case("person")));
        assert!(form.fields.iter().any(|f| f.label == "sn")); // MUST injected by resync
        assert!(form.object_classes.iter().any(|v| v == "person"));
        assert!(autonum.is_empty()); // no {next:…} default in this profile
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
