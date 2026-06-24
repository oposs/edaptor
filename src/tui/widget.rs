//! Field-value presenter for read-only display. `present_field` renders each
//! form row's value cell. M2 will extend this with an activation / commit
//! contract (widget editors, `FieldWidget` trait, `CommitOutcome`).

use crate::workflows::form_model::{FormField, WidgetSpec};

/// Registry entry point M1 uses for read-only display. Renders a field's value
/// cell from its `WidgetSpec` and value cardinality. (M2 swaps this for a
/// registry keyed by `WidgetKind` that also dispatches `activate`.)
pub(crate) fn present_field(field: &FormField) -> String {
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
    use crate::workflows::form_model::FormField;

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
}
