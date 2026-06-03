//! Shared `#[cfg(test)]` fixtures for the `app` submodule tests.
#![cfg(test)]

use super::*;
use crate::ldap::worker::RawSubschema;

/// A bare App (no form) with the given read-only flag, for dispatch tests.
pub(crate) fn bare_app(read_only: bool) -> App {
    App {
        focus: Pane::Tree,
        should_quit: false,
        read_only,
        tree_state: TreeState::default(),
        tree_items: vec![],
        current_branch: String::new(),
        last_search: String::new(),
        rows: vec![],
        leaf_sel: 0,
        search: TextState::new(),
        last_seen_leaf: None,
        form: None,
        form_focus: 0,
        form_scroll: 0,
        overlay: None,
        status: String::new(),
        pickers: vec![],
        label_rules: vec![],
        picker_search_id: None,
        picker_last_query: String::new(),
    }
}

/// Install a one-field form carrying `dn` so Alt+D/delete has a target.
pub(crate) fn with_form(mut app: App, dn: &str) -> App {
    use crate::schema::FieldKind;
    use crate::ui::edit_form::EditField;
    use crate::ui::form::WidgetSpec;
    app.form = Some(EditForm {
        dn: dn.to_string(),
        fields: vec![EditField {
            label: "cn".to_string(),
            must: true,
            editable: true,
            multi: false,
            secret: false,
            ordered: false,
            values: vec!["x".to_string()],
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            editor: TextState::new().with_value("x".to_string()),
            picker: None,
        }],
        baseline: Default::default(),
        mode: FormMode::Edit,
    });
    app
}

pub(crate) fn alt(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::ALT)
}

/// A minimal empty structure for tests that call `dispatch_key` (structure
/// is only used when Enter opens the picker; these tests don't exercise that).
pub(crate) fn empty_structure() -> Structure {
    Structure::build("dc=test", vec![])
}

pub(crate) fn structure() -> Structure {
    Structure::build(
        "dc=example,dc=org",
        vec![
            StructureInput {
                dn: "dc=example,dc=org".into(),
                cn: None,
                description: Some("Example".into()),
                object_classes: vec![],
                attrs: Default::default(),
            },
            StructureInput {
                dn: "ou=users,dc=example,dc=org".into(),
                cn: None,
                description: None,
                object_classes: vec![],
                attrs: Default::default(),
            },
            StructureInput {
                dn: "uid=jane,ou=users,dc=example,dc=org".into(),
                cn: Some("Jane".into()),
                description: None,
                object_classes: vec!["inetOrgPerson".into()],
                attrs: [
                    ("cn".to_string(), vec!["Jane".to_string()]),
                    ("uid".to_string(), vec!["jane".to_string()]),
                ]
                .into_iter()
                .collect(),
            },
        ],
    )
}

pub(crate) fn bare_profile(name: &str) -> EntryProfile {
    EntryProfile {
        name: name.into(),
        object_classes: vec![],
        rdn_attr: String::new(),
        search_base: String::new(),
        show: vec![],
        search_attrs: vec![],
        defaults: Default::default(),
        password: None,
        pickers: Default::default(),
        label: None,
    }
}

pub(crate) fn rule(ocs: &[&str], tmpl: &str) -> LabelRule {
    LabelRule {
        object_classes: ocs.iter().map(|s| s.to_string()).collect(),
        template: crate::config::label::parse_label_template(tmpl),
    }
}

pub(crate) fn attr_map(pairs: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
    pairs
        .iter()
        .map(|(k, vs)| (k.to_string(), vs.iter().map(|s| s.to_string()).collect()))
        .collect()
}

pub(crate) fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// Minimal schema for user (inetOrgPerson-like) with uid, description, memberOf.
pub(crate) fn user_schema() -> SchemaModel {
    let raw = RawSubschema {
        object_classes: vec![
            // No SUP top so validate does not require objectClass in the entry.
            "( 1.2.3.4 NAME 'testUser' STRUCTURAL MUST uid MAY ( description $ memberOf ) )".to_string(),
        ],
        attribute_types: vec![
            "( 0.9.2342.19200300.100.1.1 NAME 'uid' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 SINGLE-VALUE )".to_string(),
            "( 2.5.4.13 NAME 'description' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".to_string(),
            "( 1.2.840.113556.1.2.102 NAME 'memberOf' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".to_string(),
        ],
        ldap_syntaxes: vec![],
    };
    SchemaModel::from_raw(&raw)
}

pub(crate) fn create_user_profile() -> EntryProfile {
    EntryProfile {
        name: "User".into(),
        object_classes: vec!["testUser".into()],
        rdn_attr: "uid".into(),
        search_base: "ou=people,dc=example,dc=org".into(),
        show: vec!["uid".into()],
        search_attrs: vec![],
        defaults: Default::default(),
        password: None,
        pickers: Default::default(),
        label: None,
    }
}

pub(crate) fn pw_spec(samba: bool) -> crate::config::PasswordSpec {
    crate::config::PasswordSpec {
        ldap_attribute: "userPassword".into(),
        samba,
    }
}
