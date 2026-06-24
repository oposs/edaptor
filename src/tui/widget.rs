//! The field-widget plugin contract. M1 implements the read-only `present()`
//! surface; editing (`activate`/`CommitOutcome`) lands in M2.

use crate::workflows::form_model::{FormField, WidgetSpec};

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

/// One plugin per widget kind. M1 uses only `present`.
pub trait FieldWidget {
    fn capability(&self) -> Capability;
    /// The read-only value-cell text for `field`.
    fn present(&self, field: &FormField) -> String;
}

/// The default plain presenter: schema/value-driven read-only rendering.
pub struct PlainWidget;

impl FieldWidget for PlainWidget {
    fn capability(&self) -> Capability {
        Capability::Static
    }

    fn present(&self, field: &FormField) -> String {
        present_field(field)
    }
}

/// Registry entry point M1 uses for read-only display. Renders a field's value
/// cell from its `WidgetSpec` and value cardinality. (M2 swaps this for a
/// registry keyed by `WidgetKind` that also dispatches `activate`.)
pub fn present_field(field: &FormField) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::FieldKind;

    fn field(values: &[&str], widget: WidgetSpec) -> FormField {
        FormField {
            label: "attr".into(),
            kind: FieldKind::Text,
            is_must: false,
            values: values.iter().map(|s| s.to_string()).collect(),
            widget,
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
    fn test_present_empty_text() {
        assert_eq!(present_field(&field(&[], WidgetSpec::ReadOnlyText)), "");
    }

    #[test]
    fn test_present_multi_summarizes_count() {
        let f = field(&["a", "b", "c"], WidgetSpec::ReadOnlyText);
        assert_eq!(present_field(&f), "‹3 values›");
    }

    #[test]
    fn test_present_checkbox() {
        assert_eq!(
            present_field(&field(&["TRUE"], WidgetSpec::DisabledCheckBox(true))),
            "[x]"
        );
        assert_eq!(
            present_field(&field(&[], WidgetSpec::DisabledCheckBox(false))),
            "[ ]"
        );
    }

    #[test]
    fn test_present_binary_note() {
        assert_eq!(
            present_field(&field(&[], WidgetSpec::BinaryNote(2048))),
            "<2048 bytes>"
        );
    }

    #[test]
    fn test_plain_widget_capability_is_static() {
        assert_eq!(PlainWidget.capability(), Capability::Static);
    }
}
