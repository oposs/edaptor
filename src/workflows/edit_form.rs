//! UI-neutral editable form model: the M2 editable shape derived from a read-only
//! [`FormModel`]. Carries plain `Vec<String>` values + a load-time `baseline` for
//! the set-wise dirty check; the text editor itself lives in the tvision pane, so
//! there is NO `TextState` here (cf. the ratatui `ui::edit_form`, deleted at M5).

use std::collections::BTreeMap;

use crate::config::widget::WidgetKind;
use crate::form::changeset::EditEntry;
use crate::schema::{FieldKind, SchemaModel};
use crate::workflows::form_model::{FormField, FormModel, WidgetSpec};

/// One editable field.
pub struct EditField {
    pub label: String,
    pub must: bool,
    pub editable: bool,
    pub multi: bool,
    pub secret: bool,
    pub ordered: bool,
    pub orphaned: bool,
    pub kind: FieldKind,
    pub widget: WidgetSpec,
    pub widget_binding: Option<WidgetKind>,
    pub values: Vec<String>,
    pub baseline: Vec<String>,
}

impl EditField {
    /// The field's value set as currently edited.
    ///
    /// - orphaned → `[]` (the diff emits a Delete regardless);
    /// - single + editable → the trimmed `values[0]`; emptied → `[]` so the diff
    ///   emits a delete, not an empty value;
    /// - otherwise → `values` unchanged.
    pub fn current_values(&self) -> Vec<String> {
        if self.orphaned {
            return vec![];
        }
        if !self.multi && self.editable {
            // Single-value inline edit: trim and drop if blank, so an emptied
            // field yields no values and the diff emits a delete (not an empty
            // value). Multi-valued and read-only fields keep `values` verbatim,
            // matching the ratatui `ui::edit_form` baseline (dedup at M5).
            let v = self.values.first().map(|s| s.trim()).unwrap_or("");
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

/// Create vs edit; only `Edit` exists in M2 (`New` is M3's create flow).
pub enum FormMode {
    Edit,
}

/// An editable entry: its DN, objectClasses, and fields.
pub struct EditForm {
    pub dn: String,
    pub mode: FormMode,
    pub object_classes: Vec<String>,
    pub fields: Vec<EditField>,
}

impl EditForm {
    /// Write a committed single-value inline edit into `fields[idx]`.
    pub fn set_value(&mut self, idx: usize, text: String) {
        if let Some(f) = self.fields.get_mut(idx) {
            f.values = vec![text];
        }
    }

    /// Whether any field's current value differs from its baseline. Set-wise
    /// (order-insensitive) unless the field is `ordered`, matching
    /// `changeset::diff` semantics so a pure reorder of an unordered attribute is
    /// NOT dirty.
    pub fn is_dirty(&self) -> bool {
        self.fields.iter().any(|f| {
            let current = f.current_values();
            if f.ordered {
                current != f.baseline
            } else {
                !value_set_eq(&current, &f.baseline)
            }
        })
    }

    /// A pure [`EditEntry`] of every field's current values, keyed by label.
    pub fn to_edit_entry(&self) -> EditEntry {
        let attrs: BTreeMap<String, Vec<String>> = self
            .fields
            .iter()
            .map(|f| (f.label.clone(), f.current_values()))
            .collect();
        EditEntry {
            dn: self.dn.clone(),
            attrs,
        }
    }
}

/// Order-insensitive value-set equality (same length, each element of each side
/// present in the other). The dirty-check sibling of `changeset::diff`.
pub fn value_set_eq(a: &[String], b: &[String]) -> bool {
    a.len() == b.len()
        && a.iter().all(|v| b.iter().any(|w| w == v))
        && b.iter().all(|v| a.iter().any(|w| w == v))
}

/// True when a field is a free-text editor (not binary / boolean-checkbox).
fn field_is_editable(f: &FormField) -> bool {
    !matches!(
        f.widget,
        WidgetSpec::BinaryNote(_) | WidgetSpec::DisabledCheckBox(_)
    )
}

/// Build an [`EditForm`] from a read-only [`FormModel`] + schema. `values` and
/// `baseline` are seeded equal (clean). `editable = !read_only && free-text kind`;
/// `multi` from the schema; `secret`/`ordered`/`orphaned` start `false`
/// (M4/M3 passes refine them). `object_classes` come from the model's `cn=`?—no:
/// they are not on `FormModel`, so the caller passes them via the read path; here
/// we leave them empty and the caller fills `object_classes` (see Task 4 wiring).
pub fn build_edit_form(model: &FormModel, schema: &SchemaModel, read_only: bool) -> EditForm {
    let fields: Vec<EditField> = model
        .fields
        .iter()
        .map(|f| {
            let editable = !read_only && field_is_editable(f);
            EditField {
                label: f.label.clone(),
                must: f.is_must,
                editable,
                multi: !schema.is_single_value(&f.label),
                secret: false,
                ordered: false,
                orphaned: false,
                kind: f.kind,
                widget: f.widget.clone(),
                widget_binding: None,
                values: f.values.clone(),
                baseline: f.values.clone(),
            }
        })
        .collect();

    EditForm {
        dn: model.title.clone(),
        mode: FormMode::Edit,
        object_classes: Vec::new(),
        fields,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldap::worker::RawSubschema;
    use crate::schema::{FieldKind, SchemaModel};
    use crate::workflows::form_model::{FormField, FormModel, WidgetSpec};

    fn schema() -> SchemaModel {
        let raw = RawSubschema {
            object_classes: vec![
                "( 2.5.6.0 NAME 'top' ABSTRACT MUST objectClass )".to_string(),
                "( 2.5.6.6 NAME 'person' SUP top STRUCTURAL MUST ( sn $ cn ) MAY description )"
                    .to_string(),
            ],
            attribute_types: vec![
                "( 2.5.4.3 NAME 'cn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 SINGLE-VALUE )"
                    .to_string(),
                "( 2.5.4.4 NAME 'sn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".to_string(),
            ],
            ldap_syntaxes: vec![],
        };
        SchemaModel::from_raw(&raw)
    }

    fn model() -> FormModel {
        FormModel {
            title: "cn=Alice,dc=example,dc=org".to_string(),
            fields: vec![
                FormField {
                    label: "cn".into(),
                    kind: FieldKind::Text,
                    is_must: true,
                    values: vec!["Alice".into()],
                    widget: WidgetSpec::ReadOnlyText,
                },
                FormField {
                    label: "sn".into(),
                    kind: FieldKind::Text,
                    is_must: true,
                    values: vec!["Adams".into()],
                    widget: WidgetSpec::ReadOnlyText,
                },
            ],
        }
    }

    #[test]
    fn build_seeds_values_and_baseline_equal() {
        let f = build_edit_form(&model(), &schema(), false);
        assert_eq!(f.dn, "cn=Alice,dc=example,dc=org");
        assert!(!f.is_dirty());
        assert_eq!(f.fields[0].values, vec!["Alice".to_string()]);
        assert_eq!(f.fields[0].baseline, vec!["Alice".to_string()]);
        assert!(f.fields[0].editable);
    }

    #[test]
    fn read_only_forces_non_editable() {
        let f = build_edit_form(&model(), &schema(), true);
        assert!(f.fields.iter().all(|x| !x.editable));
    }

    #[test]
    fn set_value_marks_dirty_and_to_edit_entry_reflects_it() {
        let mut f = build_edit_form(&model(), &schema(), false);
        f.set_value(0, "Alicia".to_string());
        assert!(f.is_dirty());
        let e = f.to_edit_entry();
        assert_eq!(e.dn, "cn=Alice,dc=example,dc=org");
        assert_eq!(e.attrs.get("cn"), Some(&vec!["Alicia".to_string()]));
    }

    #[test]
    fn emptied_single_field_yields_no_values() {
        let mut f = build_edit_form(&model(), &schema(), false);
        f.set_value(0, "   ".to_string());
        assert_eq!(f.fields[0].current_values(), Vec::<String>::new());
    }

    #[test]
    fn reorder_only_is_not_dirty_setwise() {
        assert!(value_set_eq(
            &["a".into(), "b".into()],
            &["b".into(), "a".into()]
        ));
    }

    #[test]
    fn ordered_field_reorder_is_dirty() {
        let mut f = build_edit_form(&model(), &schema(), false);
        f.fields[0].ordered = true;
        f.fields[0].values = vec!["b".into(), "a".into()];
        f.fields[0].baseline = vec!["a".into(), "b".into()];
        assert!(f.is_dirty());
    }
}
