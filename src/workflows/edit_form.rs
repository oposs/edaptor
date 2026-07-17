//! UI-neutral editable form model: the M2 editable shape derived from a read-only
//! [`FormModel`]. Carries plain `Vec<String>` values + a load-time `baseline` for
//! the set-wise dirty check; the text editor itself lives in the tvision pane, so
//! there is NO `TextState` here (the ratatui `ui::edit_form` was removed at the
//! M5b cutover; this is the sole edit-form model).

use std::collections::BTreeMap;

use crate::config::widget::WidgetKind;
use crate::form::changeset::EditEntry;
use crate::schema::{FieldKind, SchemaModel};
use crate::workflows::form_model::{FormField, FormModel, WidgetSpec};

/// One editable field.
#[derive(Clone)]
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
            // matching the former ratatui `ui::edit_form` baseline (now the sole model).
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

    /// A freshly schema-injected editable field with no `FormField` backing:
    /// empty values/baseline, free-text widget, `kind`/`multi` resolved from
    /// schema. Used by [`EditForm::sync_schema_fields`] when an objectClass change
    /// brings a new attribute into MUST∪MAY.
    pub fn injected(label: String, must: bool, schema: &SchemaModel) -> EditField {
        let multi = !schema.is_single_value(&label);
        let kind = schema.field_kind(&label);
        EditField {
            label,
            must,
            editable: true,
            multi,
            secret: false,
            ordered: false,
            orphaned: false,
            kind,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: None,
            values: Vec::new(),
            baseline: Vec::new(),
        }
    }
}

/// Create vs edit. `Create` composes a new entry of `profile_idx` under `container`.
pub enum FormMode {
    Edit,
    Create {
        profile_idx: usize,
        container: String,
    },
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
    ///
    /// For `ordered` fields the comparison is order-sensitive but ignores the
    /// `{n}` X-ORDERED prefixes: the editor canonicalises them on load
    /// (`to_values` reconstructs `{0}`, `{1}`, …), so a baseline stored WITHOUT
    /// prefixes — or with non-canonical ones — would otherwise read as dirty the
    /// moment the form syncs, even with no edit. Stripping both sides compares the
    /// meaningful sequence (content + order); a genuine reorder still changes it.
    pub fn is_dirty(&self) -> bool {
        self.fields.iter().any(|f| {
            let current = f.current_values();
            if f.ordered {
                strip_ordering_seq(&current) != strip_ordering_seq(&f.baseline)
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

    /// Recompute the form's fields from the current `objectClass` field values:
    /// flag fields that left MUST∪MAY as `orphaned`, refresh `must`, inject empty
    /// fields for newly-allowed attrs, then reorder. Faithful neutral port of
    /// `ui::edit_form::sync_schema_fields`. Values on still-allowed fields are
    /// preserved; objectClass is never orphaned. No-op-safe if no objectClass field.
    pub fn sync_schema_fields(&mut self, schema: &SchemaModel) {
        let oc_values: Vec<String> = self
            .fields
            .iter()
            .find(|f| f.label.eq_ignore_ascii_case("objectClass"))
            .map(|f| f.values.clone())
            .unwrap_or_default();
        let oc_refs: Vec<&str> = oc_values.iter().map(|s| s.as_str()).collect();

        let resolved = schema.effective_attributes(&oc_refs);
        let allowed: std::collections::BTreeSet<String> = resolved
            .must
            .iter()
            .chain(resolved.may.iter())
            .map(|s| s.to_lowercase())
            .chain(std::iter::once("objectclass".to_string()))
            .collect();

        for field in &mut self.fields {
            let key = field.label.to_lowercase();
            if key == "objectclass" {
                field.orphaned = false;
                continue;
            }
            let in_allowed = allowed.contains(&key);
            field.orphaned = !in_allowed;
            field.must = in_allowed
                && resolved
                    .must
                    .iter()
                    .any(|m| m.eq_ignore_ascii_case(&field.label));
        }

        let existing: std::collections::HashSet<String> =
            self.fields.iter().map(|f| f.label.to_lowercase()).collect();
        for attr in resolved.must.iter().chain(resolved.may.iter()) {
            if existing.contains(&attr.to_lowercase()) {
                continue;
            }
            let is_must = resolved.must.contains(attr);
            self.fields
                .push(EditField::injected(attr.clone(), is_must, schema));
        }

        order_fields(self);
    }
}

/// The fan-out back-ref attr for a field — the holder attribute a membership
/// change writes into each group (e.g. `member`), if the field carries a picker
/// binding whose `fanout_attr` is set. `None` for plain / choice / password
/// fields. Neutral port of `ui::edit_form::fanout_attr_of` (reads
/// [`EditField::widget_binding`] for `WidgetKind::Picker(b)` with `b.fanout_attr`).
pub fn fanout_attr_of(f: &EditField) -> Option<&str> {
    match &f.widget_binding {
        Some(WidgetKind::Picker(b)) => b.fanout_attr.as_deref(),
        _ => None,
    }
}

impl EditForm {
    /// Labels of fields whose picker binding fans out (excluded from the own-entry
    /// diff; their baseline→selection delta drives the per-holder fan-out save).
    pub fn fanout_labels(&self) -> Vec<String> {
        self.fields
            .iter()
            .filter(|f| fanout_attr_of(f).is_some())
            .map(|f| f.label.clone())
            .collect()
    }
}

/// Each value with its `{n}` X-ORDERED prefix stripped, order preserved. Used by
/// the dirty check so an ordered field compares by content+order, not by the
/// exact `{n}` serialization the editor canonicalises on load.
fn strip_ordering_seq(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|v| crate::ui::ordered::strip_ordering(v).to_string())
        .collect()
}

/// Order-insensitive value-set equality (same length, each element of each side
/// present in the other). The dirty-check sibling of `changeset::diff`.
pub fn value_set_eq(a: &[String], b: &[String]) -> bool {
    a.len() == b.len()
        && a.iter().all(|v| b.iter().any(|w| w == v))
        && b.iter().all(|v| a.iter().any(|w| w == v))
}

/// Reorder a built form's fields into: `objectClass` first (pinned to the very
/// top, right under the DN header), then mandatory, then populated-or-special
/// (non-empty current value, secret, or widget-bound), then the rest — each
/// bucket case-insensitive by label. Orphaned fields have empty `current_values`,
/// so they fall into the last bucket. Neutral port of `ui::edit_form::order_fields`
/// (the ratatui picker probe becomes `widget_binding.is_some()`).
pub fn order_fields(form: &mut EditForm) {
    fn is_object_class(f: &EditField) -> bool {
        f.label.eq_ignore_ascii_case("objectClass")
    }
    fn bucket(f: &EditField) -> u8 {
        if f.orphaned {
            2
        } else if f.must {
            0
        } else if !f.current_values().is_empty() || f.secret || f.widget_binding.is_some() {
            1
        } else {
            2
        }
    }
    form.fields.sort_by(|a, b| {
        // objectClass always sorts before every other field.
        (!is_object_class(a))
            .cmp(&!is_object_class(b))
            .then_with(|| bucket(a).cmp(&bucket(b)))
            .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
    });
}

/// Stably pull the profile's `show`-listed fields to the front of the form — right
/// after the pinned `objectClass` — in `show` order, leaving every other field in
/// its existing (bucketed) relative order. Case-insensitive; each `show` entry
/// claims at most one field (the first unclaimed match). A no-op when `show` is
/// empty.
///
/// [`order_fields`] alphabetises within buckets and has no notion of the profile's
/// preferred field order, so it overwrites the `show`-driven order that
/// `empty_form_for_profile` starts with. Callers apply this AFTER `order_fields`
/// (create build + objectClass re-sync) so the config's `show` list drives the top
/// of the form; the bucket logic still governs everything not listed in `show`.
pub fn reorder_by_show(form: &mut EditForm, show: &[String]) {
    if show.is_empty() {
        return;
    }
    let mut claimed = vec![false; form.fields.len()];
    let mut front: Vec<usize> = Vec::new();

    // objectClass stays pinned at the very top (order_fields already put it first).
    if let Some(pos) = form
        .fields
        .iter()
        .position(|f| f.label.eq_ignore_ascii_case("objectClass"))
    {
        claimed[pos] = true;
        front.push(pos);
    }
    // Then the show-listed fields, in show order.
    for attr in show {
        if let Some(pos) = form
            .fields
            .iter()
            .position(|f| f.label.eq_ignore_ascii_case(attr))
            .filter(|&p| !claimed[p])
        {
            claimed[pos] = true;
            front.push(pos);
        }
    }
    // Reassemble: the front fields, then every remaining field in its current order.
    let mut reordered: Vec<EditField> = Vec::with_capacity(form.fields.len());
    for &i in &front {
        reordered.push(form.fields[i].clone());
    }
    for (i, f) in form.fields.iter().enumerate() {
        if !claimed[i] {
            reordered.push(f.clone());
        }
    }
    form.fields = reordered;
}

/// True when a field is a free-text editor (not binary / boolean-checkbox).
fn field_is_editable(f: &FormField) -> bool {
    !matches!(
        f.widget,
        WidgetSpec::BinaryNote(_) | WidgetSpec::DisabledCheckBox(_)
    )
}

/// The DN a create-mode form would produce: `<rdn_attr>=<rdn_value>,<container>`,
/// with the RDN value trimmed. When the value is blank, a `…` placeholder stands in
/// (so the header reads `uid=…,ou=…`). Pure.
pub fn composed_create_dn(rdn_attr: &str, rdn_value: &str, container: &str) -> String {
    let v = rdn_value.trim();
    let shown = if v.is_empty() { "…" } else { v };
    format!("{rdn_attr}={shown},{container}")
}

/// Build an [`EditForm`] from a read-only [`FormModel`] + schema. `values` and
/// `baseline` are seeded equal (clean). `editable = !read_only && free-text kind`;
/// `multi` from the schema; `secret`/`ordered`/`orphaned` start `false`
/// (M4/M3 passes refine them). `object_classes` come from the model's `cn=`?—no:
/// they are not on `FormModel`, so the caller passes them via the read path; here
/// we leave them empty and the caller fills `object_classes` (see Task 4 wiring).
pub fn build_edit_form(model: &FormModel, schema: &SchemaModel, read_only: bool) -> EditForm {
    let mut fields: Vec<EditField> = model
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

    // objectClass always leads the form (right under the DN title), keeping the
    // rest of the profile-driven order intact. `sync_schema_fields`/`order_fields`
    // pin it the same way after an objectClass change.
    if let Some(pos) = fields
        .iter()
        .position(|f| f.label.eq_ignore_ascii_case("objectClass"))
    {
        let oc = fields.remove(pos);
        fields.insert(0, oc);
    }

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
    fn build_pins_object_class_to_the_top() {
        // A model whose objectClass field is NOT first must still yield a form with
        // objectClass leading, right under the DN title.
        let mut m = model();
        m.fields.push(FormField {
            label: "objectClass".into(),
            kind: FieldKind::Text,
            is_must: true,
            values: vec!["top".into(), "person".into()],
            widget: WidgetSpec::ReadOnlyText,
        });
        let f = build_edit_form(&m, &schema(), false);
        assert_eq!(
            f.fields.first().unwrap().label,
            "objectClass",
            "objectClass must lead the built form"
        );
        // The remaining fields keep their original relative order.
        let rest: Vec<&str> = f.fields[1..].iter().map(|x| x.label.as_str()).collect();
        assert_eq!(rest, vec!["cn", "sn"]);
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

    #[test]
    fn injected_field_resolves_kind_and_multi_from_schema() {
        let s = schema();
        // `cn` is SINGLE-VALUE in the fixture; `sn` is multi.
        let cn = EditField::injected("cn".into(), true, &s);
        assert!(!cn.multi);
        assert!(cn.must);
        assert!(cn.editable);
        assert!(cn.values.is_empty() && cn.baseline.is_empty());
        let sn = EditField::injected("sn".into(), false, &s);
        assert!(sn.multi);
        assert!(!sn.must);
    }

    #[test]
    fn order_fields_puts_must_first_then_populated_then_empty() {
        let mut f = build_edit_form(&model(), &schema(), false);
        // model() has cn (must, populated) and sn (must, populated): add an empty
        // optional and a populated optional to exercise all three buckets.
        f.fields
            .push(EditField::injected("description".into(), false, &schema())); // empty optional
        let mut populated_opt = EditField::injected("givenName".into(), false, &schema());
        populated_opt.values = vec!["x".into()];
        f.fields.push(populated_opt);
        order_fields(&mut f);
        let labels: Vec<&str> = f.fields.iter().map(|x| x.label.as_str()).collect();
        // must (cn, sn) first (alphabetical), then populated optional (givenName),
        // then empty optional (description) last.
        assert_eq!(labels, vec!["cn", "sn", "givenName", "description"]);
    }

    #[test]
    fn order_fields_sinks_orphaned_to_bottom() {
        let mut f = build_edit_form(&model(), &schema(), false);
        f.fields[0].orphaned = true; // cn orphaned → current_values() == [] → bucket 2
        order_fields(&mut f);
        assert_eq!(f.fields.last().unwrap().label, "cn");
    }

    #[test]
    fn reorder_by_show_pulls_show_fields_after_object_class() {
        let mut f = build_edit_form(&model(), &schema(), false);
        f.fields
            .push(EditField::injected("objectClass".into(), true, &schema()));
        f.fields
            .push(EditField::injected("description".into(), false, &schema()));
        let mut gn = EditField::injected("givenName".into(), false, &schema());
        gn.values = vec!["x".into()];
        f.fields.push(gn);
        order_fields(&mut f); // alphabetical buckets, objectClass pinned first
        reorder_by_show(
            &mut f,
            &["givenName".to_string(), "sn".to_string(), "cn".to_string()],
        );
        let labels: Vec<&str> = f.fields.iter().map(|x| x.label.as_str()).collect();
        // objectClass pinned, then show order, then the rest (empty `description`).
        assert_eq!(
            labels,
            vec!["objectClass", "givenName", "sn", "cn", "description"]
        );
    }

    #[test]
    fn reorder_by_show_empty_is_noop() {
        let mut f = build_edit_form(&model(), &schema(), false);
        order_fields(&mut f);
        let before: Vec<String> = f.fields.iter().map(|x| x.label.clone()).collect();
        reorder_by_show(&mut f, &[]);
        let after: Vec<String> = f.fields.iter().map(|x| x.label.clone()).collect();
        assert_eq!(before, after);
    }

    #[test]
    fn reorder_by_show_ignores_attrs_not_in_form() {
        let mut f = build_edit_form(&model(), &schema(), false);
        order_fields(&mut f);
        // `mail` isn't a field here; it's silently skipped, `sn` still moves front.
        reorder_by_show(&mut f, &["mail".to_string(), "sn".to_string()]);
        let labels: Vec<&str> = f.fields.iter().map(|x| x.label.as_str()).collect();
        assert_eq!(labels, vec!["sn", "cn"]);
    }

    #[test]
    fn order_fields_pins_object_class_to_the_top() {
        let mut f = build_edit_form(&model(), &schema(), false);
        // objectClass is a MUST field; without the pin it would sort alphabetically
        // among the other MUSTs (after cn). It must instead lead the whole form.
        f.fields
            .push(EditField::injected("objectClass".into(), true, &schema()));
        order_fields(&mut f);
        assert_eq!(
            f.fields.first().unwrap().label,
            "objectClass",
            "objectClass must lead the form, ahead of the other mandatory fields"
        );
    }

    fn schema_oc() -> SchemaModel {
        // top (MUST objectClass); person (MUST sn,cn; MAY description);
        // organizationalPerson SUP person (MAY title, ou);
        // extensibleObject (no extra attrs, used to test removal).
        let raw = RawSubschema {
            object_classes: vec![
                "( 2.5.6.0 NAME 'top' ABSTRACT MUST objectClass )".into(),
                "( 2.5.6.6 NAME 'person' SUP top STRUCTURAL MUST ( sn $ cn ) MAY description )"
                    .into(),
                "( 2.5.6.7 NAME 'organizationalPerson' SUP person STRUCTURAL MAY ( title $ ou ) )"
                    .into(),
            ],
            attribute_types: vec![
                "( 2.5.4.3 NAME 'cn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 SINGLE-VALUE )".into(),
                "( 2.5.4.4 NAME 'sn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".into(),
                "( 2.5.4.13 NAME 'description' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".into(),
                "( 2.5.4.12 NAME 'title' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".into(),
                "( 2.5.4.11 NAME 'ou' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".into(),
            ],
            ldap_syntaxes: vec![],
        };
        SchemaModel::from_raw(&raw)
    }

    /// Build an EditForm with an explicit objectClass field carrying `ocs`.
    fn form_with_ocs(ocs: &[&str]) -> EditForm {
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
            values: ocs.iter().map(|s| s.to_string()).collect(),
            baseline: ocs.iter().map(|s| s.to_string()).collect(),
        };
        EditForm {
            dn: "cn=Bob,dc=example,dc=org".into(),
            mode: FormMode::Edit,
            object_classes: ocs.iter().map(|s| s.to_string()).collect(),
            fields: vec![oc_field],
        }
    }

    #[test]
    fn sync_injects_must_and_may_fields_for_classes() {
        let mut f = form_with_ocs(&["top", "person"]);
        f.sync_schema_fields(&schema_oc());
        let has = |l: &str| f.fields.iter().any(|x| x.label.eq_ignore_ascii_case(l));
        assert!(has("cn") && has("sn") && has("description"));
        let cn = f.fields.iter().find(|x| x.label == "cn").unwrap();
        assert!(cn.must); // person MUST cn
        let desc = f.fields.iter().find(|x| x.label == "description").unwrap();
        assert!(!desc.must); // person MAY description
    }

    #[test]
    fn sync_orphans_fields_when_class_removed() {
        // Start with organizationalPerson (title/ou allowed + populated), then remove it.
        let mut f = form_with_ocs(&["top", "organizationalPerson"]);
        f.sync_schema_fields(&schema_oc()); // title/ou now injected & allowed
        if let Some(t) = f.fields.iter_mut().find(|x| x.label == "title") {
            t.values = vec!["Boss".into()];
        }
        // Now drop down to plain person: title/ou leave MUST∪MAY → orphaned.
        f.fields
            .iter_mut()
            .find(|x| x.label.eq_ignore_ascii_case("objectClass"))
            .unwrap()
            .values = vec!["top".into(), "person".into()];
        f.sync_schema_fields(&schema_oc());
        let title = f.fields.iter().find(|x| x.label == "title").unwrap();
        assert!(title.orphaned);
        assert!(!title.must);
        // title still present but sunk to the bottom region; objectClass never orphaned.
        let oc = f
            .fields
            .iter()
            .find(|x| x.label.eq_ignore_ascii_case("objectClass"))
            .unwrap();
        assert!(!oc.orphaned);
    }

    #[test]
    fn sync_preserves_values_on_still_allowed_fields() {
        let mut f = form_with_ocs(&["top", "person"]);
        f.sync_schema_fields(&schema_oc());
        f.fields
            .iter_mut()
            .find(|x| x.label == "cn")
            .unwrap()
            .values = vec!["Bob".into()];
        // add organizationalPerson; cn stays allowed and keeps its value.
        f.fields
            .iter_mut()
            .find(|x| x.label.eq_ignore_ascii_case("objectClass"))
            .unwrap()
            .values = vec!["top".into(), "person".into(), "organizationalPerson".into()];
        f.sync_schema_fields(&schema_oc());
        let cn = f.fields.iter().find(|x| x.label == "cn").unwrap();
        assert_eq!(cn.values, vec!["Bob".to_string()]);
        assert!(!cn.orphaned);
    }

    #[test]
    fn fanout_labels_come_from_picker_binding_with_fanout_attr() {
        use crate::config::relation::{CandidateScope, PickerBinding, StoreKey};
        use crate::config::widget::WidgetKind;

        let scope = CandidateScope {
            base: "ou=groups,dc=x".into(),
            object_classes: vec!["groupOfNames".into()],
            search_attrs: vec!["cn".into()],
            label_template: None,
        };
        let binding = |fanout: Option<&str>| {
            Some(WidgetKind::Picker(PickerBinding {
                attr: "memberOf".into(),
                scope: scope.clone(),
                store: StoreKey::Dn,
                select: None,
                fanout_attr: fanout.map(|s| s.to_string()),
            }))
        };
        let mut f = build_edit_form(&model(), &schema(), false);
        // A picker with fanout_attr → a fan-out label; without → not.
        f.fields[0].widget_binding = binding(Some("member"));
        f.fields[1].widget_binding = binding(None);
        assert_eq!(fanout_attr_of(&f.fields[0]), Some("member"));
        assert_eq!(fanout_attr_of(&f.fields[1]), None);
        assert_eq!(f.fanout_labels(), vec![f.fields[0].label.clone()]);
    }

    #[test]
    fn composed_create_dn_uses_rdn_and_container() {
        assert_eq!(
            composed_create_dn("uid", "  alice ", "ou=people,dc=example,dc=org"),
            "uid=alice,ou=people,dc=example,dc=org"
        );
    }

    #[test]
    fn composed_create_dn_placeholder_when_rdn_empty() {
        assert_eq!(
            composed_create_dn("uid", "   ", "ou=people,dc=example,dc=org"),
            "uid=…,ou=people,dc=example,dc=org"
        );
    }
}
