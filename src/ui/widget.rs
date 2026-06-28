//! The field-widget plugin contract. M1 implements the read-only `present()`
//! surface; M2 adds `activate`/`inline_editable`.

use crate::schema::SchemaModel;
use crate::ui::Shared;
use crate::workflows::edit_form::EditField;
use crate::workflows::form_model::WidgetSpec;
use tvision_rs::{self as tv, View};

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

/// How a field is edited. `Inline` = grapheme edit in place (M2). `Modal` = a
/// dialog editor that yields a typed `CommitOutcome` (M3+: the first impl is the
/// objectClass picker). Not `PartialEq`/`Clone`: it carries a trait object.
pub enum Activation {
    Inline,
    Modal(Box<dyn FieldEditor>),
}

/// A modal field editor: builds its tvision dialog and keeps the prospective
/// `CommitOutcome` in `shared.borrow_mut().staged_commit` as the user interacts.
/// `dispatch` reads it back by the `exec_view` return code (apply on OK, discard
/// on CANCEL). Returns the view plus the `ViewId` to focus initially.
pub trait FieldEditor {
    fn into_view(
        self: Box<Self>,
        schema: &SchemaModel,
        shared: Shared,
    ) -> (Box<dyn View>, tv::ViewId);
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

/// The `sambaSID` presenter/activation marker. The actual SID computation is a
/// dispatch special-case in `app.rs` (it needs the sibling `uidNumber` value and
/// the `UiState.samba_domain` — context this trait can't see), so `activate`
/// here is never reached: the ACTIVATE handler intercepts `SambaSid`-bound
/// fields before building an editor. This widget exists for read-only
/// presentation and to keep `widget_for` total.
pub struct SambaSidWidget;

impl FieldWidget for SambaSidWidget {
    fn capability(&self) -> Capability {
        Capability::Static
    }

    fn present(&self, field: &EditField) -> String {
        match field.values.first() {
            Some(v) if !v.is_empty() => v.clone(),
            _ => "‹unset›".to_string(),
        }
    }

    fn activate(&self, _field: &EditField) -> Activation {
        // Unreachable in practice: the ACTIVATE dispatch special-cases SambaSid.
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

/// The widget plugin for a field. objectClass → ObjectClassWidget; password-bound
/// fields → PasswordWidget; everything else → PlainWidget.
pub fn widget_for(field: &EditField) -> Box<dyn FieldWidget> {
    use crate::config::widget::WidgetKind;
    if field.label.eq_ignore_ascii_case("objectClass") {
        Box::new(crate::ui::oc_picker::ObjectClassWidget)
    } else if matches!(field.widget_binding, Some(WidgetKind::Password(_))) {
        Box::new(crate::ui::pw_editor::PasswordWidget)
    } else if matches!(field.widget_binding, Some(WidgetKind::Choice(_))) {
        Box::new(crate::ui::choice::ChoiceWidget)
    } else if matches!(
        &field.widget_binding,
        Some(WidgetKind::Picker(b)) if b.fanout_attr.is_none()
    ) {
        Box::new(crate::ui::picker::PickerWidget)
    } else if matches!(
        &field.widget_binding,
        Some(WidgetKind::Picker(b)) if b.fanout_attr.is_some()
    ) {
        Box::new(crate::ui::membership::MembershipWidget)
    } else if matches!(field.widget_binding, Some(WidgetKind::SambaSid)) {
        Box::new(SambaSidWidget)
    } else if field.editable && field.multi && !field.orphaned && field.widget_binding.is_none() {
        Box::new(crate::ui::multivalue::MultiValueWidget)
    } else {
        Box::new(PlainWidget)
    }
}

/// Whether a field opens a modal editor on activation (vs inline edit). Cheap
/// check used by the form pane for focus/nav/Enter without building an editor.
/// Mirrors the `widget_for` routing.
pub fn is_modal_field(field: &EditField) -> bool {
    use crate::config::widget::WidgetKind;
    field.label.eq_ignore_ascii_case("objectClass")
        || matches!(field.widget_binding, Some(WidgetKind::Password(_)))
        || matches!(field.widget_binding, Some(WidgetKind::Choice(_)))
        || matches!(
            &field.widget_binding,
            Some(WidgetKind::Picker(b)) if b.fanout_attr.is_none()
        )
        || matches!(
            &field.widget_binding,
            Some(WidgetKind::Picker(b)) if b.fanout_attr.is_some()
        )
        || matches!(field.widget_binding, Some(WidgetKind::SambaSid))
        || (field.editable && field.multi && !field.orphaned && field.widget_binding.is_none())
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
        assert!(matches!(
            PlainWidget.activate(&field(&["x"], WidgetSpec::ReadOnlyText)),
            Activation::Inline
        ));
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

    #[test]
    fn objectclass_is_modal_field() {
        let mut f = field(&["top"], WidgetSpec::ReadOnlyText);
        f.label = "objectClass".into();
        assert!(is_modal_field(&f));
        assert!(matches!(widget_for(&f).activate(&f), Activation::Modal(_)));
    }

    #[test]
    fn plain_field_is_not_modal() {
        let f = field(&["x"], WidgetSpec::ReadOnlyText);
        assert!(!is_modal_field(&f));
        assert!(matches!(widget_for(&f).activate(&f), Activation::Inline));
    }

    #[test]
    fn sambasid_field_routes_and_is_modal() {
        use crate::config::widget::WidgetKind;
        let mut f = field(&[], WidgetSpec::ReadOnlyText);
        f.label = "sambaSID".into();
        f.widget_binding = Some(WidgetKind::SambaSid);
        assert!(is_modal_field(&f));
        // Empty value presents as the unset marker.
        assert_eq!(widget_for(&f).present(&f), "‹unset›");
        // A populated value presents verbatim.
        f.values = vec!["S-1-5-21-1-2-3-3000".into()];
        assert_eq!(widget_for(&f).present(&f), "S-1-5-21-1-2-3-3000");
    }

    #[test]
    fn password_field_routes_to_password_widget_and_is_modal() {
        use crate::config::widget::{PasswordWidget as PwCfg, WidgetKind};
        let mut f = field(&[], WidgetSpec::ReadOnlyText);
        f.label = "userPassword".into();
        f.widget_binding = Some(WidgetKind::Password(PwCfg {
            primary: "userPassword".into(),
            derived: vec![],
            samba: false,
        }));
        assert!(is_modal_field(&f));
        assert!(matches!(widget_for(&f).activate(&f), Activation::Modal(_)));
    }
}
