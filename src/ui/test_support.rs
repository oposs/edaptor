//! Shared test fixtures for `src/ui/**` test modules. Rust test modules are
//! private to their file they're declared in, so a fixture more than one
//! file's tests need lives here instead of being duplicated per file.

use crate::schema::FieldKind;
use crate::workflows::edit_form::{EditField, EditForm, FormMode};
use crate::workflows::form_model::WidgetSpec;

/// An edit form on `dn` with one field whose current value differs from its
/// baseline, so `is_dirty()` is true.
pub(crate) fn dirty_form(dn: &str) -> EditForm {
    let field = EditField {
        label: "cn".into(),
        must: false,
        editable: true,
        multi: false,
        secret: false,
        ordered: false,
        orphaned: false,
        kind: FieldKind::Text,
        widget: WidgetSpec::ReadOnlyText,
        widget_binding: None,
        values: vec!["edited".into()],
        baseline: vec!["old".into()],
    };
    EditForm {
        dn: dn.to_string(),
        mode: FormMode::Edit,
        object_classes: vec![],
        fields: vec![field],
        baseline_csn: None,
    }
}
