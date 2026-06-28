//! The password field widget + a minimal placeholder editor. M3 Phase 2b:
//! `PasswordWidget` presents the masked ‹set›/‹unset› cell and opens a modal.
//! Task 16 replaces `PasswordEditor::into_view` with the real TLS-gated
//! New + Confirm dialog; only the shape is established here.

use tvision_rs::{ButtonFlags, ButtonRowAlign, Command, Dialog, Rect, StaticText, View, ViewId};

use crate::schema::SchemaModel;
use crate::tui::widget::{Activation, Capability, FieldEditor, FieldWidget};
use crate::tui::Shared;
use crate::workflows::edit_form::EditField;

/// The plugin for password fields (bound via `WidgetKind::Password`).
pub(crate) struct PasswordWidget;

impl FieldWidget for PasswordWidget {
    fn capability(&self) -> Capability {
        Capability::Static
    }

    fn present(&self, field: &EditField) -> String {
        if field.values.is_empty() {
            "\u{2039}unset\u{203a}".to_string() // ‹unset›
        } else {
            "\u{2039}set\u{203a}".to_string() // ‹set›
        }
    }

    fn activate(&self, field: &EditField) -> Activation {
        Activation::Modal(Box::new(PasswordEditor::for_field(field)))
    }
}

/// Carries the field's label into the placeholder dialog. Task 16 replaces the
/// dialog body with the real TLS-gated New + Confirm interaction.
pub(crate) struct PasswordEditor {
    label: String,
}

impl PasswordEditor {
    /// Capture the minimal field context needed to build the editor. Intentionally
    /// does not borrow_mut shared during construction (2a lesson).
    pub(crate) fn for_field(field: &EditField) -> Self {
        PasswordEditor {
            label: field.label.clone(),
        }
    }
}

impl FieldEditor for PasswordEditor {
    fn into_view(
        self: Box<Self>,
        _schema: &SchemaModel,
        _shared: Shared,
    ) -> (Box<dyn View>, ViewId) {
        let msg = format!(
            "Password editor for '{}' — not yet implemented.\nPress OK to dismiss.",
            self.label
        );
        let mut dlg = Dialog::new(Rect::new(0, 0, 52, 10), Some("Set Password".to_string()));
        dlg.state_mut().options.center_x = true;
        dlg.state_mut().options.center_y = true;
        dlg.insert_child(Box::new(StaticText::new(Rect::new(2, 2, 50, 6), msg)));
        let ids = dlg.button_row(
            &[(
                "~O~K",
                Command::OK,
                ButtonFlags {
                    default: true,
                    ..ButtonFlags::new()
                },
            )],
            ButtonRowAlign::Right,
        );
        (Box::new(dlg), ids[0])
    }
}
