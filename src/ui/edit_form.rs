//! The editable form model.
//!
//! `FormModel`/`FormField` (`crate::ui::form`) are read-only-oriented and carry
//! no edit state, so the editable shape is net-new here: [`EditField`] adds the
//! `multi` / `secret` / `ordered` / `editable` flags and a `TextState` edit
//! engine, and [`EditForm`] groups them under a DN.
//!
//! P1 builds this for *display* (read-only). P2-T1 adds the `baseline`, the
//! set-wise dirty check, and `to_edit_entry()` — deliberately left out here so
//! that work stays a self-contained, unit-tested task.

use tui_prompts::TextState;

use crate::form::changeset::is_x_ordered;
use crate::schema::{FieldKind, SchemaModel};
use crate::ui::form::{FormField, FormModel, WidgetSpec};

/// One field of the editable form.
pub struct EditField {
    /// Attribute name (a `*` suffix marks MUST on render).
    pub label: String,
    /// Whether the attribute is in the effective MUST set.
    pub must: bool,
    /// Whether this field accepts edits (read-only kinds and global read-only
    /// mode force this false).
    pub editable: bool,
    /// Whether the attribute is multi-valued (edited via the value-editor popup).
    pub multi: bool,
    /// Whether the attribute holds a secret (rendered masked, never in clear).
    pub secret: bool,
    /// Whether the attribute is X-ORDERED (the `{n}` prefix makes order matter).
    pub ordered: bool,
    /// The attribute's current string values (display order).
    pub values: Vec<String>,
    /// The classified syntax (drives read-only display formatting).
    pub kind: FieldKind,
    /// The read-only widget choice (checkbox / binary note formatting).
    pub widget: WidgetSpec,
    /// Inline single-value edit state, seeded from `values[0]`. The Unicode-correct
    /// edit engine (tui-prompts); rendering is done by hand so the pane owns its bg.
    pub editor: TextState<'static>,
}

/// The editable form for one entry.
pub struct EditForm {
    /// The entry's distinguished name.
    pub dn: String,
    /// The ordered fields.
    pub fields: Vec<EditField>,
}

/// Build an [`EditForm`] from a read-only [`FormModel`] plus the server schema.
///
/// - `multi`    = the attribute is not single-valued in the schema;
/// - `editable` = not global-read-only AND the field kind is editable
///   (binary / boolean-checkbox / `memberOf` stay static — [`field_is_editable`]);
/// - `secret`   = a password attribute ([`is_secret_attr`]);
/// - `ordered`  = an X-ORDERED config attribute ([`is_x_ordered`]).
///
/// P1 uses the result purely for display. The single-value `editor` is seeded
/// from `values[0]` so P2's editing has its starting point.
pub fn build_edit_form(model: &FormModel, schema: &SchemaModel, read_only: bool) -> EditForm {
    let fields = model
        .fields
        .iter()
        .map(|f| {
            let seed = f.values.first().cloned().unwrap_or_default();
            EditField {
                label: f.label.clone(),
                must: f.is_must,
                editable: !read_only && field_is_editable(f),
                multi: !schema.is_single_value(&f.label),
                secret: is_secret_attr(&f.label),
                ordered: is_x_ordered(&f.label),
                values: f.values.clone(),
                kind: f.kind,
                widget: f.widget.clone(),
                editor: TextState::new().with_value(seed),
            }
        })
        .collect();

    EditForm {
        dn: model.title.clone(),
        fields,
    }
}

/// Port of the facade's editability rule: `memberOf` is server-maintained and
/// binary / boolean-checkbox kinds are not free-text, so none of them edit.
fn field_is_editable(field: &FormField) -> bool {
    if field.label.eq_ignore_ascii_case("memberOf") {
        return false;
    }
    !matches!(
        field.widget,
        WidgetSpec::BinaryNote(_) | WidgetSpec::DisabledCheckBox(_)
    )
}

/// Whether `attr` holds a secret that must be masked on screen. Conservative
/// minimal set (extend as needed); case-insensitive.
pub fn is_secret_attr(attr: &str) -> bool {
    const SECRET: &[&str] = &["userPassword", "sambaNTPassword", "sambaLMPassword"];
    SECRET.iter().any(|a| a.eq_ignore_ascii_case(attr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldap::worker::{LdapEntry, RawSubschema};
    use crate::ui::form::build_form_model;
    use std::collections::BTreeMap;

    fn schema() -> SchemaModel {
        let raw = RawSubschema {
            object_classes: vec![
                "( 2.5.6.0 NAME 'top' ABSTRACT MUST objectClass )".to_string(),
                "( 2.5.6.6 NAME 'person' SUP top STRUCTURAL MUST ( sn $ cn ) \
                  MAY ( userPassword $ description ) )"
                    .to_string(),
                "( 1.2.3 NAME 'demoPerson' SUP person STRUCTURAL MAY mail )".to_string(),
            ],
            attribute_types: vec![
                // cn single-valued; mail multi-valued (no SINGLE-VALUE).
                "( 2.5.4.3 NAME 'cn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 SINGLE-VALUE )"
                    .to_string(),
                "( 2.5.4.4 NAME 'sn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 SINGLE-VALUE )"
                    .to_string(),
                "( 1.1.1 NAME 'mail' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".to_string(),
                "( 1.1.9 NAME 'userPassword' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".to_string(),
            ],
            ldap_syntaxes: vec![],
        };
        SchemaModel::from_raw(&raw)
    }

    fn entry() -> LdapEntry {
        let mut attrs = BTreeMap::new();
        attrs.insert("cn".to_string(), vec!["Alice".to_string()]);
        attrs.insert("sn".to_string(), vec!["Adams".to_string()]);
        attrs.insert(
            "mail".to_string(),
            vec!["a@x.org".to_string(), "a@y.org".to_string()],
        );
        attrs.insert("userPassword".to_string(), vec!["secret".to_string()]);
        LdapEntry {
            dn: "cn=Alice,dc=example,dc=org".to_string(),
            attrs,
            bin_attrs: BTreeMap::new(),
        }
    }

    #[test]
    fn flags_are_set_from_schema_and_rules() {
        let model = build_form_model(&schema(), &["demoPerson"], &entry(), &[]);
        let form = build_edit_form(&model, &schema(), false);
        let field = |name: &str| form.fields.iter().find(|f| f.label == name).unwrap();

        assert!(!field("cn").multi, "cn is single-valued");
        assert!(field("mail").multi, "mail is multi-valued");
        assert!(field("userPassword").secret, "userPassword is secret");
        assert!(!field("cn").secret);
        assert!(field("cn").editable, "cn edits in writable mode");
    }

    #[test]
    fn read_only_mode_disables_all_editing() {
        let model = build_form_model(&schema(), &["demoPerson"], &entry(), &[]);
        let form = build_edit_form(&model, &schema(), true);
        assert!(form.fields.iter().all(|f| !f.editable));
        assert_eq!(form.dn, "cn=Alice,dc=example,dc=org");
    }
}
