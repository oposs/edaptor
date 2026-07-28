//! Schema-driven READ-ONLY entry form model (tty-free, unit-tested).
//!
//! This is the heart of the read flow: given the server [`SchemaModel`], an
//! entry's objectClasses, the fetched [`LdapEntry`], and the active profile's
//! `show` ordering, it produces a [`FormModel`] — an ordered list of
//! [`FormField`]s, each tagged with a read-only widget hint. It is the source the
//! editable form is built from; a UI renders it.

use crate::ldap::worker::LdapEntry;
use crate::schema::{FieldKind, SchemaModel};
use crate::workflows::gtime;

/// The read-only widget the UI renders for a field, chosen by [`FieldKind`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WidgetSpec {
    /// Free text shown read-only.
    ReadOnlyText,
    /// An integer shown read-only.
    ReadOnlyInt,
    /// A distinguished name shown read-only.
    ReadOnlyDn,
    /// A generalized-time value shown read-only.
    ReadOnlyTime,
    /// A disabled checkbox reflecting the parsed boolean.
    DisabledCheckBox(bool),
    /// A note standing in for binary data: `<N bytes>`.
    BinaryNote(usize),
}

/// One field of the read-only form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormField {
    /// The attribute name (the UI appends a `" *"` marker when `is_must`).
    pub label: String,
    /// The classified syntax of the attribute.
    pub kind: FieldKind,
    /// Whether the attribute is in the effective MUST set.
    pub is_must: bool,
    /// The entry's string values for this attribute (empty for binary/absent).
    pub values: Vec<String>,
    /// The widget the UI should render.
    pub widget: WidgetSpec,
}

/// The full read-only form for one entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormModel {
    /// Dialog title (the entry DN).
    pub title: String,
    /// The ordered fields.
    pub fields: Vec<FormField>,
}

/// Map a [`FieldKind`] (plus the first value / byte count) to a [`WidgetSpec`].
///
/// `Boolean` parses the LDAP boolean syntax (the strings `TRUE`/`FALSE`,
/// case-insensitively; anything else is `false`). `Binary` uses `byte_count`
/// (defaulting to `0` when unknown).
pub fn field_widget_spec(
    kind: FieldKind,
    value: Option<&str>,
    byte_count: Option<usize>,
) -> WidgetSpec {
    match kind {
        FieldKind::Text => WidgetSpec::ReadOnlyText,
        FieldKind::Integer => WidgetSpec::ReadOnlyInt,
        FieldKind::DistinguishedName => WidgetSpec::ReadOnlyDn,
        FieldKind::GeneralizedTime => WidgetSpec::ReadOnlyTime,
        FieldKind::Boolean => {
            let b = value
                .map(|v| v.eq_ignore_ascii_case("TRUE"))
                .unwrap_or(false);
            WidgetSpec::DisabledCheckBox(b)
        }
        FieldKind::Binary => WidgetSpec::BinaryNote(byte_count.unwrap_or(0)),
    }
}

/// Case-insensitive lookup of an attribute's string values in the entry.
fn string_values<'a>(entry: &'a LdapEntry, attr: &str) -> Option<&'a Vec<String>> {
    entry
        .attrs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(attr))
        .map(|(_, v)| v)
}

/// Case-insensitive lookup of an attribute's binary byte count in the entry.
fn binary_count(entry: &LdapEntry, attr: &str) -> Option<usize> {
    entry
        .bin_attrs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(attr))
        .map(|(_, n)| *n)
}

/// The server-maintained audit attributes shown, in this order, in the form's
/// meta block. They are *operational*: the server only returns them when they are
/// named in the search's attribute list (see `ReadFlow::request_entry`), and it
/// alone maintains them — edaptor never writes them.
pub const META_ATTRS: [&str; 4] = [
    "createTimestamp",
    "creatorsName",
    "modifyTimestamp",
    "modifiersName",
];

/// True when `attr` is one of the [`META_ATTRS`] (case-insensitive).
///
/// The attribute *name* is the classifier throughout — no per-field flag. These
/// are operational attributes: they are never in an objectClass's MUST∪MAY, so
/// `build_form_model` can never emit them as ordinary editable fields, and a form
/// field carrying one of these labels is therefore always a meta row. Every site
/// that must exclude them (the save diff, `validate`, the orphan sweep, field
/// ordering, the pane's focus/blank-row handling) asks this one question.
pub fn is_meta_attr(attr: &str) -> bool {
    META_ATTRS.iter().any(|m| m.eq_ignore_ascii_case(attr))
}

/// Build the ordered read-only form model.
///
/// Ordering: the `profile_show` attributes present in the effective (must ∪ may)
/// set first, in `show` order; then the remaining MUST attributes; then the
/// remaining MAY attributes. Each field's widget is chosen by
/// [`field_widget_spec`] from the entry's first value / byte count; `is_must` is
/// set case-insensitively from the effective MUST set. The title is the DN.
///
/// Finally, the [`META_ATTRS`] the entry actually carries are appended as `meta`
/// fields, in `META_ATTRS` order, with timestamps rendered in local time. They sit
/// outside the schema-driven ordering because they are not attributes the operator
/// edits — the UI shows them as a separate read-only block.
pub fn build_form_model(
    schema: &SchemaModel,
    object_classes: &[&str],
    entry: &LdapEntry,
    profile_show: &[String],
) -> FormModel {
    let resolved = schema.effective_attributes(object_classes);

    let is_must = |attr: &str| resolved.must.iter().any(|m| m.eq_ignore_ascii_case(attr));
    let in_effective =
        |attr: &str| is_must(attr) || resolved.may.iter().any(|m| m.eq_ignore_ascii_case(attr));
    let already =
        |ordered: &[String], attr: &str| ordered.iter().any(|a| a.eq_ignore_ascii_case(attr));

    let mut ordered: Vec<String> = Vec::new();
    for attr in profile_show {
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

    let mut fields: Vec<FormField> = ordered
        .into_iter()
        .map(|attr| {
            let kind = schema.field_kind(&attr);
            let values = string_values(entry, &attr).cloned().unwrap_or_default();
            let bytes = binary_count(entry, &attr);
            let widget = field_widget_spec(kind, values.first().map(|s| s.as_str()), bytes);
            FormField {
                is_must: is_must(&attr),
                kind,
                values,
                widget,
                label: attr,
            }
        })
        .collect();

    // The audit block. A server that returns none of them (or an entry the bind
    // DN may not see them on) simply produces no meta fields.
    for attr in META_ATTRS {
        let Some(values) = string_values(entry, attr) else {
            continue;
        };
        if values.is_empty() {
            continue;
        }
        // Format by attribute, not by schema kind: the two timestamps are times
        // whatever a given server's subschema says (or fails to say) about them.
        let kind = schema.field_kind(attr);
        let values: Vec<String> = if attr.ends_with("Timestamp") {
            values
                .iter()
                .map(|v| gtime::format_generalized_time(v))
                .collect()
        } else {
            values.clone()
        };
        fields.push(FormField {
            label: attr.to_string(),
            kind,
            is_must: false,
            values,
            widget: WidgetSpec::ReadOnlyText,
        });
    }

    FormModel {
        title: entry.dn.clone(),
        fields,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldap::worker::RawSubschema;
    use std::collections::BTreeMap;

    #[test]
    fn field_widget_spec_maps_kinds() {
        assert_eq!(
            field_widget_spec(FieldKind::Text, Some("x"), None),
            WidgetSpec::ReadOnlyText
        );
        assert_eq!(
            field_widget_spec(FieldKind::Integer, Some("3"), None),
            WidgetSpec::ReadOnlyInt
        );
        assert_eq!(
            field_widget_spec(FieldKind::DistinguishedName, Some("cn=x"), None),
            WidgetSpec::ReadOnlyDn
        );
        assert_eq!(
            field_widget_spec(FieldKind::GeneralizedTime, Some("20240101000000Z"), None),
            WidgetSpec::ReadOnlyTime
        );
        assert_eq!(
            field_widget_spec(FieldKind::Boolean, Some("TRUE"), None),
            WidgetSpec::DisabledCheckBox(true)
        );
        assert_eq!(
            field_widget_spec(FieldKind::Binary, None, Some(42)),
            WidgetSpec::BinaryNote(42)
        );
    }

    #[test]
    fn boolean_parses_true_false() {
        assert_eq!(
            field_widget_spec(FieldKind::Boolean, Some("TRUE"), None),
            WidgetSpec::DisabledCheckBox(true)
        );
        assert_eq!(
            field_widget_spec(FieldKind::Boolean, Some("FALSE"), None),
            WidgetSpec::DisabledCheckBox(false)
        );
        assert_eq!(
            field_widget_spec(FieldKind::Boolean, Some("true"), None),
            WidgetSpec::DisabledCheckBox(true)
        );
        assert_eq!(
            field_widget_spec(FieldKind::Boolean, None, None),
            WidgetSpec::DisabledCheckBox(false)
        );
    }

    fn schema() -> SchemaModel {
        let raw = RawSubschema {
            object_classes: vec![
                "( 2.5.6.0 NAME 'top' ABSTRACT MUST objectClass )".to_string(),
                "( 2.5.6.6 NAME 'person' SUP top STRUCTURAL MUST ( sn $ cn ) \
                  MAY ( userPassword $ description ) )"
                    .to_string(),
                "( 1.2.3 NAME 'demoPerson' SUP person STRUCTURAL \
                  MAY ( mail $ uid $ active $ manager ) )"
                    .to_string(),
            ],
            attribute_types: vec![
                "( 2.5.4.3 NAME 'cn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".to_string(),
                "( 2.5.4.4 NAME 'sn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".to_string(),
                "( 1.1.1 NAME 'mail' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".to_string(),
                "( 1.1.2 NAME 'uid' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".to_string(),
                "( 1.1.3 NAME 'active' SYNTAX 1.3.6.1.4.1.1466.115.121.1.7 )".to_string(),
                "( 1.1.4 NAME 'manager' SYNTAX 1.3.6.1.4.1.1466.115.121.1.12 )".to_string(),
            ],
            ldap_syntaxes: vec![],
        };
        SchemaModel::from_raw(&raw)
    }

    fn entry() -> LdapEntry {
        let mut attrs = BTreeMap::new();
        attrs.insert("cn".to_string(), vec!["Alice".to_string()]);
        attrs.insert("sn".to_string(), vec!["Adams".to_string()]);
        attrs.insert("uid".to_string(), vec!["alice".to_string()]);
        attrs.insert("mail".to_string(), vec!["alice@example.org".to_string()]);
        attrs.insert("active".to_string(), vec!["TRUE".to_string()]);
        attrs.insert(
            "manager".to_string(),
            vec!["cn=boss,dc=example,dc=org".to_string()],
        );
        LdapEntry {
            dn: "uid=alice,dc=example,dc=org".to_string(),
            attrs,
            bin_attrs: BTreeMap::new(),
        }
    }

    /// The same entry plus the operational audit attributes a server returns when
    /// they are named in the search (see `ReadFlow::request_entry`).
    fn entry_with_meta() -> LdapEntry {
        let mut e = entry();
        e.attrs.insert(
            "createTimestamp".to_string(),
            vec!["20260614091244Z".to_string()],
        );
        e.attrs.insert(
            "creatorsName".to_string(),
            vec!["cn=admin,dc=example,dc=org".to_string()],
        );
        e.attrs.insert(
            "modifyTimestamp".to_string(),
            vec!["20260728110322Z".to_string()],
        );
        e.attrs.insert(
            "modifiersName".to_string(),
            vec!["cn=operator,dc=example,dc=org".to_string()],
        );
        e
    }

    #[test]
    fn meta_fields_come_last_in_meta_attrs_order() {
        let model = build_form_model(&schema(), &["demoPerson"], &entry_with_meta(), &[]);
        let labels: Vec<&str> = model.fields.iter().map(|f| f.label.as_str()).collect();
        let tail = &labels[labels.len() - 4..];
        assert_eq!(
            tail,
            &[
                "createTimestamp",
                "creatorsName",
                "modifyTimestamp",
                "modifiersName"
            ],
            "the audit block is appended, in META_ATTRS order"
        );
    }

    #[test]
    fn meta_timestamps_are_formatted_and_names_pass_through() {
        let model = build_form_model(&schema(), &["demoPerson"], &entry_with_meta(), &[]);
        let value = |attr: &str| {
            model
                .fields
                .iter()
                .find(|f| f.label == attr)
                .and_then(|f| f.values.first())
                .cloned()
                .unwrap_or_default()
        };
        // No local offset is captured in tests, so rendering takes the UTC path.
        assert_eq!(value("createTimestamp"), "2026-06-14 09:12:44 UTC");
        assert_eq!(value("modifyTimestamp"), "2026-07-28 11:03:22 UTC");
        assert_eq!(value("creatorsName"), "cn=admin,dc=example,dc=org");
        assert_eq!(value("modifiersName"), "cn=operator,dc=example,dc=org");
    }

    #[test]
    fn absent_meta_attrs_produce_no_rows() {
        // A server that does not return them (or a bind DN that may not read
        // them) yields a form with no meta rows at all.
        let model = build_form_model(&schema(), &["demoPerson"], &entry(), &[]);
        assert!(
            !model.fields.iter().any(|f| is_meta_attr(&f.label)),
            "no audit attributes on the entry → no meta rows"
        );
    }

    #[test]
    fn is_meta_attr_is_case_insensitive() {
        assert!(is_meta_attr("createtimestamp"));
        assert!(is_meta_attr("ModifiersName"));
        assert!(!is_meta_attr("modifyTimestampX"));
        assert!(!is_meta_attr("cn"));
    }

    #[test]
    fn form_model_orders_by_profile_show() {
        let schema = schema();
        let show = vec!["uid".to_string(), "cn".to_string(), "mail".to_string()];
        let model = build_form_model(&schema, &["demoPerson"], &entry(), &show);
        let labels: Vec<&str> = model.fields.iter().map(|f| f.label.as_str()).collect();
        assert_eq!(&labels[..3], &["uid", "cn", "mail"]);
        assert_eq!(model.title, "uid=alice,dc=example,dc=org");
        let sn_pos = labels.iter().position(|l| *l == "sn").unwrap();
        assert!(sn_pos >= 3, "sn should come after the profile_show block");
    }

    #[test]
    fn form_model_marks_must() {
        let schema = schema();
        let model = build_form_model(&schema, &["demoPerson"], &entry(), &[]);
        let cn = model.fields.iter().find(|f| f.label == "cn").unwrap();
        assert!(cn.is_must, "cn is MUST (from person)");
        let mail = model.fields.iter().find(|f| f.label == "mail").unwrap();
        assert!(!mail.is_must, "mail is MAY");
    }

    #[test]
    fn form_model_picks_widgets_by_kind() {
        let schema = schema();
        let model = build_form_model(&schema, &["demoPerson"], &entry(), &[]);
        let by = |name: &str| {
            model
                .fields
                .iter()
                .find(|f| f.label == name)
                .unwrap()
                .widget
                .clone()
        };
        assert_eq!(by("cn"), WidgetSpec::ReadOnlyText);
        assert_eq!(by("active"), WidgetSpec::DisabledCheckBox(true));
        assert_eq!(by("manager"), WidgetSpec::ReadOnlyDn);
    }

    #[test]
    fn binary_attr_becomes_byte_note() {
        let mut e = entry();
        e.bin_attrs.insert("photo".to_string(), 7);
        let raw_with_photo = RawSubschema {
            object_classes: vec![
                "( 2.5.6.0 NAME 'top' ABSTRACT MUST objectClass )".to_string(),
                "( 2.5.6.200 NAME 'withPhoto' SUP top STRUCTURAL MAY photo )".to_string(),
            ],
            // octet-string syntax (.40) classifies as Binary.
            attribute_types: vec![
                "( 2.5.4.200 NAME 'photo' SYNTAX 1.3.6.1.4.1.1466.115.121.1.40 )".to_string(),
            ],
            ldap_syntaxes: vec![],
        };
        let s2 = SchemaModel::from_raw(&raw_with_photo);
        let model = build_form_model(&s2, &["withPhoto"], &e, &[]);
        let photo = model.fields.iter().find(|f| f.label == "photo").unwrap();
        assert_eq!(photo.widget, WidgetSpec::BinaryNote(7));
    }
}
