//! The field-widget plugin contract. M1 implements the read-only `present()`
//! surface; M2 adds `activate`/`inline_editable`.

use crate::workflows::edit_form::EditField;
use crate::workflows::form_model::WidgetSpec;

/// What data a widget's editor needs (used by M2 dispatch; declared now).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    Static,
    NeedsSchema,
    NeedsWorkerSearch,
}

/// Typed result an editor returns to the form (consumed in M2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitOutcome {
    SetValues(Vec<String>),
    StageSecret {
        attrs: Vec<String>,
        cleartext: String,
    },
    SetValuesThenResyncSchema(Vec<String>),
    Cancelled,
}

/// How a field is edited (M2 adds `Modal`/`Immediate`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Activation {
    Inline,
}

/// One plugin per widget kind. M1 uses only `present`; M2 adds `activate`.
pub trait FieldWidget {
    fn capability(&self) -> Capability;
    /// The read-only value-cell text for `field`.
    fn present(&self, field: &EditField) -> String;
    /// How `field` is edited. M2: plain fields return `Inline`.
    fn activate(&self, field: &EditField) -> Activation;
}

/// The default plain presenter: schema/value-driven read-only rendering.
pub struct PlainWidget;

impl FieldWidget for PlainWidget {
    fn capability(&self) -> Capability {
        Capability::Static
    }

    fn present(&self, field: &EditField) -> String {
        present_field(field)
    }

    fn activate(&self, _field: &EditField) -> Activation {
        Activation::Inline
    }
}

/// Registry entry point M1 uses for read-only display. Renders a field's value
/// cell from its `WidgetSpec` and value cardinality. (M2 swaps this for a
/// registry keyed by `WidgetKind` that also dispatches `activate`.)
pub fn present_field(field: &EditField) -> String {
    // Multi-value summary takes precedence over per-value formatting.
    if field.values.len() > 1 {
        return format!("‹{} values›", field.values.len());
    }
    let first = field.values.first().map(String::as_str).unwrap_or("");
    match &field.widget {
        WidgetSpec::DisabledCheckBox(b) => (if *b { "[x]" } else { "[ ]" }).to_string(),
        WidgetSpec::BinaryNote(bytes) => format!("<{bytes} bytes>"),
        WidgetSpec::ReadOnlyText
        | WidgetSpec::ReadOnlyInt
        | WidgetSpec::ReadOnlyDn
        | WidgetSpec::ReadOnlyTime => first.to_string(),
    }
}

/// Whether a field is inline-editable in M2: a free-text plain single-value field
/// that is writable and not orphaned and not bound to a rich widget (choice /
/// picker / membership / objectClass — those land in M3/M4).
pub fn inline_editable(field: &EditField) -> bool {
    field.editable && !field.multi && !field.orphaned && field.widget_binding.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::FieldKind;
    use crate::workflows::edit_form::EditField;
    use crate::workflows::form_model::WidgetSpec;

    fn field(values: &[&str], widget: WidgetSpec) -> EditField {
        EditField {
            label: "attr".into(),
            must: false,
            editable: true,
            multi: false,
            secret: false,
            ordered: false,
            orphaned: false,
            kind: FieldKind::Text,
            widget,
            widget_binding: None,
            values: values.iter().map(|s| s.to_string()).collect(),
            baseline: values.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn test_present_single_text() {
        assert_eq!(
            present_field(&field(&["hello"], WidgetSpec::ReadOnlyText)),
            "hello"
        );
    }

    #[test]
    fn test_present_multi_summarizes_count() {
        assert_eq!(
            present_field(&field(&["a", "b", "c"], WidgetSpec::ReadOnlyText)),
            "‹3 values›"
        );
    }

    #[test]
    fn test_present_checkbox() {
        assert_eq!(
            present_field(&field(&["TRUE"], WidgetSpec::DisabledCheckBox(true))),
            "[x]"
        );
    }

    #[test]
    fn test_plain_activate_is_inline() {
        assert_eq!(
            PlainWidget.activate(&field(&["x"], WidgetSpec::ReadOnlyText)),
            Activation::Inline
        );
    }

    #[test]
    fn test_inline_editable_plain_single_true() {
        assert!(inline_editable(&field(&["x"], WidgetSpec::ReadOnlyText)));
    }

    #[test]
    fn test_inline_editable_multi_false() {
        let mut f = field(&["x"], WidgetSpec::ReadOnlyText);
        f.multi = true;
        assert!(!inline_editable(&f));
    }

    #[test]
    fn test_inline_editable_binary_false() {
        let mut f = field(&[], WidgetSpec::BinaryNote(8));
        f.editable = false;
        assert!(!inline_editable(&f));
    }

    #[test]
    fn test_inline_editable_orphaned_false() {
        let mut f = field(&["x"], WidgetSpec::ReadOnlyText);
        f.orphaned = true;
        assert!(!inline_editable(&f));
    }
}
