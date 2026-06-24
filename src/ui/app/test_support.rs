//! Shared `#[cfg(test)]` fixtures for the `app` submodule tests.
#![cfg(test)]

use super::*;
use crate::ui::edit_form::FormMode;
use crate::workflows::structure::StructureInput;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

// Domain-pure fixtures now live in the shared module; re-export them so existing
// ui tests that `use crate::ui::app::test_support::*` keep compiling unchanged.
pub(crate) use crate::workflows::test_fixtures::{
    attr_map, bare_profile, create_user_profile, user_schema,
};

/// A bare App (no form) with the given read-only flag, for dispatch tests.
pub(crate) fn bare_app(read_only: bool) -> App {
    App {
        focus: Pane::Tree,
        should_quit: false,
        read_only,
        connection_encrypted: false,
        tree_state: TreeState::default(),
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
        widgets: vec![],
        label_rules: vec![],
        tree_rules: Vec::new(),
        picker_search_id: None,
        picker_last_query: String::new(),
        objectclass_sync_pending: false,
        samba: None,
    }
}

/// Install a one-field form carrying `dn` so Alt+D/delete has a target.
pub(crate) fn with_form(mut app: App, dn: &str) -> App {
    use crate::schema::FieldKind;
    use crate::ui::edit_form::EditField;
    use crate::workflows::form_model::WidgetSpec;
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
            widget_binding: None,
            orphaned: false,
        }],
        baseline: Default::default(),
        mode: FormMode::Edit,
        pending_password: None,
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

pub(crate) fn rule(ocs: &[&str], tmpl: &str) -> LabelRule {
    LabelRule {
        object_classes: ocs.iter().map(|s| s.to_string()).collect(),
        template: crate::config::label::parse_label_template(tmpl),
    }
}

pub(crate) fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}
