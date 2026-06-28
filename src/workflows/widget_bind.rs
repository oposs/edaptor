//! Apply resolved `[profile.widget.<attr>]` bindings onto a neutral `EditForm`'s
//! fields: set `secret` for password fields and attach `widget_binding` where unset.
//! Neutral port of `ui::edit_form::inject_resolver_kinds`.

use crate::config::resolver::WidgetResolver;
use crate::config::widget::WidgetKind;
use crate::workflows::edit_form::EditForm;

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
    for f in &mut form.fields {
        let kind = resolver.resolve_kind(&f.label, object_classes);
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
            widget_binding: Some(WidgetKind::Readonly),
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
            matches!(form.fields[0].widget_binding, Some(WidgetKind::Readonly)),
            "pre-existing binding must not be overwritten"
        );
    }
}
