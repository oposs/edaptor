//! Apply resolved `[profile.widget.<attr>]` bindings onto a neutral `EditForm`'s
//! fields: set `secret` for password fields and attach `widget_binding` where unset.
//! Neutral port of `ui::edit_form::inject_resolver_kinds`.

use std::collections::BTreeSet;

use crate::config::resolver::WidgetResolver;
use crate::config::widget::WidgetKind;
use crate::workflows::edit_form::EditForm;

/// Affordance shown in place of an empty value for a password-derived field (the
/// Samba hashes). They are written on save and only become visible on re-read.
pub const PW_DERIVED_NOTE: &str = "\u{27E8}updated automatically when you set the password\u{27E9}";

/// Apply profile-driven widget bindings to `form`'s fields.
///
/// For every field:
/// - `secret` is set to `true` iff the resolver resolves the field to `Password`.
/// - If `widget_binding` is already set (e.g. `ObjectClassPicker` from 2a's label
///   routing), it is left untouched.
/// - Otherwise, for every field whose label is NOT `objectClass`, the resolved kind
///   (if any) is attached as `widget_binding`.
///
/// Neutral port of `ui::edit_form::inject_resolver_kinds`.
pub fn apply_widget_bindings(
    form: &mut EditForm,
    resolver: &WidgetResolver<'_>,
    object_classes: &[String],
) {
    // Attributes maintained by a password widget (the Samba hashes), collected so
    // the second pass can mark their sibling fields read-only with an affordance.
    let mut pw_derived: BTreeSet<String> = BTreeSet::new();

    for f in &mut form.fields {
        let kind = resolver.resolve_kind(&f.label, object_classes);
        if let Some(WidgetKind::Password(pw)) = &kind {
            pw_derived.extend(pw.derived.iter().map(|d| d.to_lowercase()));
        }
        // Set secret regardless of whether a binding is already present —
        // `tag_widget_fields` may have already attached a Password binding
        // (e.g. via a profile widget list), but `secret` must still be set
        // for masking / ordering / save paths to work correctly.
        f.secret = matches!(&kind, Some(WidgetKind::Password(_)));
        if f.widget_binding.is_some() {
            continue;
        }
        // Attach config-driven bindings (Password / Choice / Picker / …).
        // objectClass routing stays label-based (2a is_modal_field / widget_for),
        // so do not set a binding for it here; leave the label-driven path intact.
        if !f.label.eq_ignore_ascii_case("objectClass") {
            f.widget_binding = kind;
        }
        // X-ORDERED attrs are order-sensitive: drive the dirty check + editor.
        if matches!(f.widget_binding, Some(WidgetKind::XOrdered)) {
            f.ordered = true;
        }
    }

    // Second pass: a password-derived field (e.g. `sambaNTPassword`) is written by
    // the password fold, never edited by hand. Make it read-only and carry the
    // "updated automatically…" affordance, unless it already resolved to a richer
    // widget. Skip the password field itself (it drives the derivation).
    if !pw_derived.is_empty() {
        for f in &mut form.fields {
            if !pw_derived.contains(&f.label.to_lowercase()) {
                continue;
            }
            let plain = matches!(f.widget_binding, None | Some(WidgetKind::Readonly { .. }));
            if plain {
                f.widget_binding = Some(WidgetKind::Readonly {
                    note: Some(PW_DERIVED_NOTE.to_string()),
                });
                f.editable = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::resolver::WidgetResolver;
    use crate::config::widget::{resolve_widgets, WidgetKind};
    use crate::config::{EntryProfile, WidgetSpecCfg};
    use crate::ldap::worker::RawSubschema;
    use crate::schema::{FieldKind, SchemaModel};
    use crate::workflows::edit_form::{EditField, EditForm, FormMode};
    use crate::workflows::form_model::WidgetSpec;

    fn empty_schema() -> SchemaModel {
        SchemaModel::from_raw(&RawSubschema {
            object_classes: vec![],
            attribute_types: vec![],
            ldap_syntaxes: vec![],
        })
    }

    /// TDD RED → GREEN: a profile with `[profile.widget.userPassword] kind="password"`
    /// → after `apply_widget_bindings`, the field's `secret == true` and
    /// `widget_binding` is `Some(WidgetKind::Password(_))`.
    #[test]
    fn password_profile_widget_sets_secret_and_binding() {
        // Profile with a password widget for userPassword.
        let mut profile = EntryProfile {
            name: "user".into(),
            object_classes: vec!["inetOrgPerson".into()],
            ..Default::default()
        };
        profile.widgets.insert(
            "userPassword".into(),
            WidgetSpecCfg::Password { samba: false },
        );
        let profiles = vec![profile];
        let resolved_widgets = resolve_widgets(&profiles).expect("resolve ok");

        let schema = empty_schema();
        let resolver = WidgetResolver::new(&schema, &profiles, &resolved_widgets, false);

        // EditForm with a userPassword field and no pre-existing binding.
        let up_field = EditField {
            label: "userPassword".into(),
            must: false,
            editable: true,
            multi: false,
            secret: false,
            ordered: false,
            orphaned: false,
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: None,
            values: vec![],
            baseline: vec![],
        };
        let mut form = EditForm {
            dn: "cn=test,dc=example,dc=org".into(),
            mode: FormMode::Edit,
            object_classes: vec!["inetOrgPerson".into()],
            fields: vec![up_field],
        };

        let object_classes = vec!["inetOrgPerson".into()];
        apply_widget_bindings(&mut form, &resolver, &object_classes);

        let f = &form.fields[0];
        assert!(f.secret, "userPassword field must be marked secret");
        assert!(
            matches!(f.widget_binding, Some(WidgetKind::Password(_))),
            "userPassword must have a Password widget binding, got {:?}",
            f.widget_binding
        );
    }

    /// A password widget's derived attrs (Samba hashes) must be marked read-only
    /// with the "updated automatically…" note, so the empty field shows an
    /// affordance instead of looking broken (fix #5).
    #[test]
    fn password_derived_fields_become_readonly_with_note() {
        let mut profile = EntryProfile {
            name: "user".into(),
            object_classes: vec!["sambaSamAccount".into()],
            ..Default::default()
        };
        profile.widgets.insert(
            "userPassword".into(),
            WidgetSpecCfg::Password { samba: true },
        );
        let profiles = vec![profile];
        let resolved_widgets = resolve_widgets(&profiles).expect("resolve ok");
        let schema = empty_schema();
        let resolver = WidgetResolver::new(&schema, &profiles, &resolved_widgets, true);

        let mk = |label: &str| EditField {
            label: label.into(),
            must: false,
            editable: true,
            multi: false,
            secret: false,
            ordered: false,
            orphaned: false,
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: None,
            values: vec![],
            baseline: vec![],
        };
        let mut form = EditForm {
            dn: String::new(),
            mode: FormMode::Edit,
            object_classes: vec!["sambaSamAccount".into()],
            fields: vec![mk("userPassword"), mk("sambaNTPassword")],
        };
        apply_widget_bindings(&mut form, &resolver, &["sambaSamAccount".into()]);

        let nt = form
            .fields
            .iter()
            .find(|f| f.label == "sambaNTPassword")
            .unwrap();
        assert!(!nt.editable, "derived field must be read-only");
        match &nt.widget_binding {
            Some(WidgetKind::Readonly { note: Some(n) }) => {
                assert_eq!(n, PW_DERIVED_NOTE)
            }
            other => panic!("expected Readonly with note, got {other:?}"),
        }
        // The password field itself is untouched (still a Password widget).
        let up = form
            .fields
            .iter()
            .find(|f| f.label == "userPassword")
            .unwrap();
        assert!(matches!(up.widget_binding, Some(WidgetKind::Password(_))));
    }

    /// objectClass must NOT get a `widget_binding` set here — its routing is
    /// label-based (2a `is_modal_field` / `widget_for`).
    #[test]
    fn objectclass_never_gets_widget_binding() {
        let profiles: Vec<EntryProfile> = vec![];
        let resolved_widgets = resolve_widgets(&profiles).expect("resolve ok");
        let schema = empty_schema();
        let resolver = WidgetResolver::new(&schema, &profiles, &resolved_widgets, false);

        let oc_field = EditField {
            label: "objectClass".into(),
            must: true,
            editable: false,
            multi: true,
            secret: false,
            ordered: false,
            orphaned: false,
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: None,
            values: vec!["inetOrgPerson".into()],
            baseline: vec!["inetOrgPerson".into()],
        };
        let mut form = EditForm {
            dn: "cn=test,dc=example,dc=org".into(),
            mode: FormMode::Edit,
            object_classes: vec!["inetOrgPerson".into()],
            fields: vec![oc_field],
        };

        apply_widget_bindings(&mut form, &resolver, &["inetOrgPerson".into()]);

        assert!(
            form.fields[0].widget_binding.is_none(),
            "objectClass must not receive a widget_binding from apply_widget_bindings"
        );
    }

    /// TDD RED → GREEN: a field bound to `XOrdered` must have `ordered = true`
    /// so the dirty check and multi-value editor treat the values as order-sensitive.
    #[test]
    fn xordered_binding_sets_ordered_flag() {
        use crate::config::widget::WidgetKind;
        let mut profile = EntryProfile {
            name: "posixgroup".into(),
            object_classes: vec!["posixGroup".into()],
            ..Default::default()
        };
        profile
            .widgets
            .insert("memberUid".into(), WidgetSpecCfg::XOrdered);
        let profiles = vec![profile];
        let resolved_widgets = resolve_widgets(&profiles).expect("resolve ok");
        let schema = empty_schema();
        let resolver = WidgetResolver::new(&schema, &profiles, &resolved_widgets, false);

        let member_field = EditField {
            label: "memberUid".into(),
            must: false,
            editable: true,
            multi: true,
            secret: false,
            ordered: false,
            orphaned: false,
            kind: crate::schema::FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: None,
            values: vec![],
            baseline: vec![],
        };
        let mut form = EditForm {
            dn: "cn=testgroup,dc=example,dc=org".into(),
            mode: FormMode::Edit,
            object_classes: vec!["posixGroup".into()],
            fields: vec![member_field],
        };

        apply_widget_bindings(&mut form, &resolver, &["posixGroup".to_string()]);

        let f = form.fields.iter().find(|f| f.label == "memberUid").unwrap();
        assert!(
            matches!(f.widget_binding, Some(WidgetKind::XOrdered)),
            "memberUid must have XOrdered widget binding, got {:?}",
            f.widget_binding
        );
        assert!(f.ordered, "XOrdered binding must set field.ordered = true");
    }

    /// A field with an existing `widget_binding` must not be overwritten.
    #[test]
    fn existing_binding_is_preserved() {
        let profiles: Vec<EntryProfile> = vec![];
        let resolved_widgets = resolve_widgets(&profiles).expect("resolve ok");
        let schema = empty_schema();
        let resolver = WidgetResolver::new(&schema, &profiles, &resolved_widgets, false);

        let field = EditField {
            label: "someAttr".into(),
            must: false,
            editable: false,
            multi: false,
            secret: false,
            ordered: false,
            orphaned: false,
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: Some(WidgetKind::Readonly { note: None }),
            values: vec![],
            baseline: vec![],
        };
        let mut form = EditForm {
            dn: "cn=test,dc=example,dc=org".into(),
            mode: FormMode::Edit,
            object_classes: vec![],
            fields: vec![field],
        };

        apply_widget_bindings(&mut form, &resolver, &[]);

        assert!(
            matches!(
                form.fields[0].widget_binding,
                Some(WidgetKind::Readonly { .. })
            ),
            "pre-existing binding must not be overwritten"
        );
    }
}
