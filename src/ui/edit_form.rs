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

use std::collections::BTreeMap;

use tui_prompts::{State, TextState};

use crate::config::relation::{
    backref_lookup, holder_lookup, CandidateScope, RelationRole, ResolvedRelation,
};
use crate::form::changeset::{is_x_ordered, EditEntry};
use crate::schema::{FieldKind, SchemaModel};
use crate::ui::form::{FormField, FormModel, WidgetSpec};
use crate::ui::picker::{Candidate, PickerState};

/// Relation metadata attached to a picker-enabled field.
#[derive(Clone)]
pub struct FieldRelation {
    pub role: RelationRole,
    /// Scope for the candidate search opened from THIS field.
    pub scope: CandidateScope,
}

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
    /// `Some` when this field is a membership relation (opens the picker).
    pub relation: Option<FieldRelation>,
}

impl EditField {
    /// The field's value set as currently edited.
    ///
    /// - multi field → `values` (the multi-value popup writes edits back there);
    /// - single + editable → the live editor, trimmed; an emptied field yields no
    ///   values so the diff emits a delete (not an empty value);
    /// - single + not editable → the original `values` (read-only kinds are kept).
    pub fn current_values(&self) -> Vec<String> {
        if self.multi {
            self.values.clone()
        } else if self.editable {
            let v = self.editor.value().trim();
            if v.is_empty() {
                vec![]
            } else {
                vec![v.to_string()]
            }
        } else {
            self.values.clone()
        }
    }
}

/// The pop-up editor for a multi-valued attribute: one inline edit row per
/// value, with insert / delete / reorder. Lives as an overlay over pane 3; on
/// commit its [`committed_values`](ValueEditor::committed_values) replace the
/// field's `values`. In **picker mode** (`picker.is_some()`) the rows are
/// replaced by a searchable candidate list. (Spike `ValueEditor`.)
pub struct ValueEditor {
    /// Index of the field being edited (into [`EditForm::fields`]).
    pub field: usize,
    /// The attribute name (shown in the popup title).
    pub label: String,
    /// Whether the attribute is X-ORDERED (order matters → shown in the hint).
    pub ordered: bool,
    /// Whether the attribute is secret (rows render masked).
    pub secret: bool,
    /// One edit state per value row (free-text mode only; empty in picker mode).
    pub rows: Vec<TextState<'static>>,
    /// The selected row index (free-text mode only).
    pub sel: usize,
    /// `Some` in picker mode (relation fields); `None` for the free-text editor.
    pub picker: Option<PickerState>,
    /// The picker's incremental-search box (Unicode-correct edit engine).
    pub search: TextState<'static>,
    /// Candidate search scope (picker mode only).
    pub scope: Option<CandidateScope>,
    /// The relation role being edited (picker mode only).
    pub role: Option<RelationRole>,
}

impl ValueEditor {
    /// Open an editor over `field` (at `field_idx`), seeding one row per value.
    pub fn open(field_idx: usize, field: &EditField) -> Self {
        let rows = field
            .values
            .iter()
            .map(|v| TextState::new().with_value(v.clone()))
            .collect();
        ValueEditor {
            field: field_idx,
            label: field.label.clone(),
            ordered: field.ordered,
            secret: field.secret,
            rows,
            sel: 0,
            picker: None,
            search: TextState::new(),
            scope: None,
            role: None,
        }
    }

    /// Open in PICKER mode over a relation `field`. `label_of` resolves a DN to a
    /// display label (caller passes a lookup over the loaded structure).
    pub fn open_picker(
        field_idx: usize,
        field: &EditField,
        label_of: impl Fn(&str) -> String,
    ) -> Self {
        let rel = field
            .relation
            .as_ref()
            .expect("open_picker on a relation field");
        let selected: Vec<Candidate> = field
            .values
            .iter()
            .map(|dn| Candidate {
                dn: dn.clone(),
                label: label_of(dn),
            })
            .collect();
        ValueEditor {
            field: field_idx,
            label: field.label.clone(),
            ordered: field.ordered,
            secret: field.secret,
            rows: Vec::new(),
            sel: 0,
            picker: Some(PickerState::new(selected)),
            search: TextState::new(),
            scope: Some(rel.scope.clone()),
            role: Some(rel.role),
        }
    }

    /// The values to write back on commit: each row trimmed, blank rows dropped.
    pub fn committed_values(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|r| r.value().trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }
}

/// The editable form for one entry.
pub struct EditForm {
    /// The entry's distinguished name.
    pub dn: String,
    /// The ordered fields.
    pub fields: Vec<EditField>,
    /// Immutable snapshot of the original server values (label → values), the
    /// reference the dirty check compares the current edits against.
    pub baseline: BTreeMap<String, Vec<String>>,
}

impl EditForm {
    /// The entry as currently edited, in the shape the save path's
    /// [`crate::form::changeset::diff`] consumes.
    ///
    /// Every field is included — even those whose [`EditField::current_values`]
    /// is empty — so the attribute key set matches the original snapshot built
    /// from the same labels and a cleared field diffs to a delete.
    pub fn to_edit_entry(&self) -> EditEntry {
        let attrs = self
            .fields
            .iter()
            .map(|f| (f.label.clone(), f.current_values()))
            .collect();
        EditEntry {
            dn: self.dn.clone(),
            attrs,
        }
    }

    /// Whether any field's current value SET differs from its baseline SET.
    ///
    /// Set-wise / order-insensitive, matching `changeset::diff`'s `value_set_eq`
    /// semantics, so a pure reorder of a multi-valued attribute is NOT dirty. A
    /// missing baseline key is treated as an empty set.
    pub fn is_dirty(&self) -> bool {
        const EMPTY: &Vec<String> = &Vec::new();
        self.fields.iter().any(|f| {
            let current = f.current_values();
            let baseline = self.baseline.get(&f.label).unwrap_or(EMPTY);
            !value_set_eq(&current, baseline)
        })
    }
}

/// Order-insensitive value-set equality: same length and every element of each
/// side appears in the other (symmetric check). This is the dirty-check sibling
/// of `changeset::diff`'s value comparison; note the changeset version is
/// currently only a one-directional subset check (a latent asymmetry masked by
/// LDAP per-attribute value uniqueness — to be made symmetric in P5).
fn value_set_eq(a: &[String], b: &[String]) -> bool {
    a.len() == b.len()
        && a.iter().all(|v| b.iter().any(|w| w == v))
        && b.iter().all(|v| a.iter().any(|w| w == v))
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
pub fn build_edit_form(
    model: &FormModel,
    schema: &SchemaModel,
    read_only: bool,
    relations: &[ResolvedRelation],
) -> EditForm {
    // Derive the entry's objectClasses from the `objectClass` field values.
    // These drive `holder_lookup` / `backref_lookup` to attach picker metadata.
    let object_classes: Vec<String> = model
        .fields
        .iter()
        .find(|f| f.label.eq_ignore_ascii_case("objectClass"))
        .map(|f| f.values.clone())
        .unwrap_or_default();

    let fields: Vec<EditField> = model
        .fields
        .iter()
        .map(|f| {
            let relation = holder_lookup(relations, &object_classes, &f.label)
                .map(|r| FieldRelation {
                    role: RelationRole::Holder,
                    scope: r.candidate_scope.clone(),
                })
                .or_else(|| {
                    backref_lookup(relations, &object_classes, &f.label).map(|r| FieldRelation {
                        role: RelationRole::BackRef,
                        scope: r.holder_scope.clone(),
                    })
                });
            // BackRef fields (e.g. memberOf) are normally non-editable; the picker
            // makes them editable. (P5 wires the fan-out save.)
            let editable = match &relation {
                Some(FieldRelation {
                    role: RelationRole::BackRef,
                    ..
                }) => !read_only,
                _ => !read_only && field_is_editable(f),
            };
            let seed = f.values.first().cloned().unwrap_or_default();
            EditField {
                label: f.label.clone(),
                must: f.is_must,
                editable,
                multi: !schema.is_single_value(&f.label),
                secret: is_secret_attr(&f.label),
                ordered: is_x_ordered(&f.label),
                values: f.values.clone(),
                kind: f.kind,
                widget: f.widget.clone(),
                editor: TextState::new().with_value(seed),
                relation,
            }
        })
        .collect();

    // The immutable snapshot of original server values the dirty check compares
    // against: every field's original `values`, keyed by label.
    let baseline = fields
        .iter()
        .map(|f| (f.label.clone(), f.values.clone()))
        .collect();

    EditForm {
        dn: model.title.clone(),
        fields,
        baseline,
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

    /// A `FormModel` for a group entry: objectClass=groupOfNames, with a
    /// multi-valued `member` field. The objectClass field must carry the value
    /// so `build_edit_form`'s objectClass lookup works.
    fn group_model_with_member() -> crate::ui::form::FormModel {
        use crate::schema::FieldKind;
        use crate::ui::form::{FormField, FormModel, WidgetSpec};
        FormModel {
            title: "cn=testgroup,ou=groups,dc=example,dc=org".to_string(),
            fields: vec![
                FormField {
                    label: "objectClass".to_string(),
                    kind: FieldKind::Text,
                    is_must: true,
                    values: vec!["top".to_string(), "groupOfNames".to_string()],
                    widget: WidgetSpec::ReadOnlyText,
                },
                FormField {
                    label: "cn".to_string(),
                    kind: FieldKind::Text,
                    is_must: true,
                    values: vec!["testgroup".to_string()],
                    widget: WidgetSpec::ReadOnlyText,
                },
                FormField {
                    label: "member".to_string(),
                    kind: FieldKind::DistinguishedName,
                    is_must: false,
                    values: vec![],
                    widget: WidgetSpec::ReadOnlyDn,
                },
            ],
        }
    }

    /// A minimal `SchemaModel` that knows `objectClass` (single), `cn` (single),
    /// and `member` (multi-valued — no SINGLE-VALUE → picker-ready).
    fn schema_with_member() -> SchemaModel {
        let raw = RawSubschema {
            object_classes: vec![
                "( 2.5.6.0 NAME 'top' ABSTRACT MUST objectClass )".to_string(),
                "( 2.5.6.9 NAME 'groupOfNames' SUP top STRUCTURAL \
                  MUST ( cn $ member ) )"
                    .to_string(),
            ],
            attribute_types: vec![
                "( 2.5.4.0 NAME 'objectClass' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".to_string(),
                "( 2.5.4.3 NAME 'cn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 SINGLE-VALUE )"
                    .to_string(),
                "( 2.5.4.31 NAME 'member' SUP distinguishedName )".to_string(),
            ],
            ldap_syntaxes: vec![],
        };
        SchemaModel::from_raw(&raw)
    }

    #[test]
    fn member_field_on_group_gets_holder_relation() {
        use crate::config::relation::{resolve_relations, Relation};
        use crate::config::EntryProfile;
        let profiles = vec![
            EntryProfile {
                name: "group".into(),
                object_class: "groupOfNames".into(),
                rdn_attr: "cn".into(),
                search_base: "ou=groups".into(),
                show: vec![],
                search_attrs: vec!["cn".into()],
            },
            EntryProfile {
                name: "user".into(),
                object_class: "inetOrgPerson".into(),
                rdn_attr: "uid".into(),
                search_base: "ou=people".into(),
                show: vec![],
                search_attrs: vec!["uid".into()],
            },
        ];
        let rels = resolve_relations(
            &profiles,
            &[Relation {
                name: "m".into(),
                holder: "group".into(),
                holder_attr: "member".into(),
                candidate: "user".into(),
                back_attr: "memberOf".into(),
            }],
        );
        // A form for a group entry: objectClass=groupOfNames, fields include `member`.
        let model = group_model_with_member();
        let form = build_edit_form(&model, &schema_with_member(), false, &rels);
        let f = form.fields.iter().find(|f| f.label == "member").unwrap();
        let rel = f.relation.as_ref().expect("member is a relation field");
        assert!(matches!(
            rel.role,
            crate::config::relation::RelationRole::Holder
        ));
        assert_eq!(rel.scope.object_class, "inetOrgPerson"); // searches users
    }

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
        let form = build_edit_form(&model, &schema(), false, &[]);
        let field = |name: &str| form.fields.iter().find(|f| f.label == name).unwrap();

        assert!(!field("cn").multi, "cn is single-valued");
        assert!(field("mail").multi, "mail is multi-valued");
        assert!(field("userPassword").secret, "userPassword is secret");
        assert!(!field("cn").secret);
        assert!(field("cn").editable, "cn edits in writable mode");
    }

    #[test]
    fn value_editor_open_and_commit_drops_empties() {
        let model = build_form_model(&schema(), &["demoPerson"], &entry(), &[]);
        let form = build_edit_form(&model, &schema(), false, &[]);
        let mail_idx = form.fields.iter().position(|f| f.label == "mail").unwrap();
        let mut ve = ValueEditor::open(mail_idx, &form.fields[mail_idx]);
        assert_eq!(ve.rows.len(), 2); // a@x.org, a@y.org
        assert_eq!(ve.label, "mail");
        // Add a blank row and a whitespace row; commit drops both.
        ve.rows.push(TextState::new());
        ve.rows.push(TextState::new().with_value("   ".to_string()));
        ve.rows
            .push(TextState::new().with_value(" c@z.org ".to_string()));
        let committed = ve.committed_values();
        assert_eq!(committed, vec!["a@x.org", "a@y.org", "c@z.org"]);
    }

    #[test]
    fn read_only_mode_disables_all_editing() {
        let model = build_form_model(&schema(), &["demoPerson"], &entry(), &[]);
        let form = build_edit_form(&model, &schema(), true, &[]);
        assert!(form.fields.iter().all(|f| !f.editable));
        assert_eq!(form.dn, "cn=Alice,dc=example,dc=org");
    }

    /// Build a writable form over the standard demo entry.
    fn writable_form() -> EditForm {
        let model = build_form_model(&schema(), &["demoPerson"], &entry(), &[]);
        build_edit_form(&model, &schema(), false, &[])
    }

    fn field_index(form: &EditForm, name: &str) -> usize {
        form.fields.iter().position(|f| f.label == name).unwrap()
    }

    #[test]
    fn fresh_form_is_not_dirty() {
        let form = writable_form();
        assert!(!form.is_dirty());
    }

    #[test]
    fn editing_a_single_field_makes_it_dirty() {
        let mut form = writable_form();
        let i = field_index(&form, "cn");
        form.fields[i].editor = TextState::new().with_value("changed");

        assert!(form.is_dirty());
        assert_eq!(
            form.to_edit_entry().attrs["cn"],
            vec!["changed".to_string()]
        );
    }

    #[test]
    fn reorder_of_multi_value_is_not_dirty() {
        let mut form = writable_form();
        let i = field_index(&form, "mail");
        form.fields[i].values.reverse();

        assert!(!form.is_dirty(), "a pure reorder is set-wise equal");
    }

    #[test]
    fn adding_a_multi_value_is_dirty() {
        let mut form = writable_form();
        let i = field_index(&form, "mail");
        form.fields[i].values.push("a@z.org".to_string());

        assert!(form.is_dirty());
    }

    #[test]
    fn emptying_a_single_field_drops_the_value() {
        let mut form = writable_form();
        let i = field_index(&form, "cn");
        form.fields[i].editor = TextState::new().with_value("");

        assert!(form.fields[i].current_values().is_empty());
        assert!(form.to_edit_entry().attrs["cn"].is_empty());
    }

    #[test]
    fn reverting_an_edit_clears_dirty() {
        let mut form = writable_form();
        let i = field_index(&form, "cn");
        form.fields[i].editor = TextState::new().with_value("changed");
        assert!(form.is_dirty());

        // Restore the original value: back to clean.
        form.fields[i].editor = TextState::new().with_value("Alice");
        assert!(!form.is_dirty());
    }

    #[test]
    fn open_picker_seeds_selection_from_field_values() {
        use crate::config::relation::{CandidateScope, RelationRole};
        let scope = CandidateScope {
            base: "ou=people".into(),
            object_class: "inetOrgPerson".into(),
            search_attrs: vec!["uid".into()],
        };
        let field = EditField {
            label: "member".into(),
            must: false,
            editable: true,
            multi: true,
            secret: false,
            ordered: false,
            values: vec!["uid=a,ou=people".into(), "uid=b,ou=people".into()],
            kind: crate::schema::FieldKind::Text,
            widget: crate::ui::form::WidgetSpec::ReadOnlyText,
            editor: TextState::new(),
            relation: Some(FieldRelation {
                role: RelationRole::Holder,
                scope: scope.clone(),
            }),
        };
        // labels resolved via a closure (DN→label); here identity.
        let ve = ValueEditor::open_picker(0, &field, |dn| dn.to_string());
        let picker = ve.picker.expect("picker mode");
        assert_eq!(
            picker.selected_dns(),
            vec!["uid=a,ou=people".to_string(), "uid=b,ou=people".to_string()]
        );
        assert_eq!(ve.scope.unwrap().object_class, "inetOrgPerson");
    }
}
