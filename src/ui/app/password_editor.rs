//! The set-password popup: a TLS-gated New + Confirm editor that stages a
//! cleartext password into `EditForm.pending_password` (the password fields are
//! read-only; the new value cannot live in a field editor). The actual derive +
//! write happens in the save path.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tui_prompts::{State, TextState};

use super::overlay::Overlay;
use super::App;

/// Which of the popup's two text rows currently has focus.
#[derive(PartialEq, Eq, Clone, Copy)]
pub enum PwField {
    /// The "New password" row.
    New,
    /// The "Confirm" row.
    Confirm,
}

/// The set-password popup state: two masked text rows plus a transient note.
pub struct PasswordEditor {
    /// The new-password row.
    pub new: TextState<'static>,
    /// The confirm-password row.
    pub confirm: TextState<'static>,
    /// Which row is focused.
    pub focus: PwField,
    /// Attributes this change will update, for the popup's note.
    pub affected: Vec<String>,
    /// A transient validation message (e.g. "passwords do not match").
    pub message: String,
}

impl PasswordEditor {
    fn new_for(affected: Vec<String>) -> Self {
        PasswordEditor {
            new: TextState::new(),
            confirm: TextState::new(),
            focus: PwField::New,
            affected,
            message: String::new(),
        }
    }
}

/// Open the set-password popup for the focused field IF it is password-bound.
/// Refuses (Error overlay) when the connection is not encrypted.
pub(crate) fn open_password_editor(app: &mut App) {
    use crate::config::widget::WidgetKind;
    let focus = app.form_focus;
    let Some(form) = app.form.as_ref() else {
        return;
    };
    let Some(field) = form.fields.get(focus) else {
        return;
    };
    let Some(WidgetKind::Password(pw)) = field.widget_binding.clone() else {
        return;
    };
    if !app.connection_encrypted {
        app.overlay = Some(Overlay::Error {
            text: "Changing a password requires an encrypted connection (ldaps://, ldapi://, or start_tls)."
                .to_string(),
        });
        return;
    }
    let mut affected = vec![pw.primary.clone()];
    affected.extend(pw.derived.iter().cloned());
    app.overlay = Some(Overlay::PasswordEditor(PasswordEditor::new_for(affected)));
}

/// Key handling for the set-password popup. Esc / Alt+C cancel; Alt+S validates
/// (match + non-empty) and stages the cleartext into `EditForm.pending_password`;
/// Tab / ↑ / ↓ swap rows; any other key edits the focused row.
pub(crate) fn password_editor_key(app: &mut App, key: KeyEvent) {
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
        KeyCode::Esc => {
            app.overlay = None;
        }
        KeyCode::Char('c') | KeyCode::Char('C') if alt => {
            app.overlay = None;
        }
        KeyCode::Char('s') | KeyCode::Char('S') if alt => {
            let Some(Overlay::PasswordEditor(ed)) = app.overlay.as_ref() else {
                return;
            };
            let new = ed.new.value().to_string();
            let confirm = ed.confirm.value().to_string();
            if new.is_empty() {
                // An empty new password is treated as cancel (no staging).
                app.overlay = None;
                return;
            }
            if new != confirm {
                if let Some(Overlay::PasswordEditor(ed)) = app.overlay.as_mut() {
                    ed.message = "passwords do not match".to_string();
                }
                return;
            }
            if let Some(form) = app.form.as_mut() {
                form.pending_password = Some(new);
            }
            app.overlay = None;
            app.status = "Password staged — Alt+S to save.".to_string();
        }
        KeyCode::Tab | KeyCode::Up | KeyCode::Down => {
            if let Some(Overlay::PasswordEditor(ed)) = app.overlay.as_mut() {
                ed.focus = match ed.focus {
                    PwField::New => PwField::Confirm,
                    PwField::Confirm => PwField::New,
                };
            }
        }
        _ => {
            if let Some(Overlay::PasswordEditor(ed)) = app.overlay.as_mut() {
                ed.message.clear();
                match ed.focus {
                    PwField::New => ed.new.handle_key_event(key),
                    PwField::Confirm => ed.confirm.handle_key_event(key),
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::widget::{PasswordWidget, WidgetKind};
    use crate::ui::app::test_support::*;
    use crate::ui::app::Pane;
    use crate::ui::edit_form::{EditField, EditForm, FormMode};
    use crate::ui::form::WidgetSpec;

    /// An App with one focused, password-bound, read-only/secret field; the
    /// connection encryption flag is set from `encrypted`.
    fn app_with_password_field(encrypted: bool) -> App {
        use crate::schema::FieldKind;
        let field = EditField {
            label: "userPassword".into(),
            must: false,
            editable: false,
            multi: false,
            secret: true,
            ordered: false,
            values: vec![],
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            editor: TextState::new(),
            widget_binding: Some(WidgetKind::Password(PasswordWidget {
                primary: "userPassword".into(),
                derived: vec![],
                samba: false,
            })),
            orphaned: false,
        };
        let mut app = bare_app(false);
        app.connection_encrypted = encrypted;
        app.focus = Pane::Form;
        app.form_focus = 0;
        app.form = Some(EditForm {
            dn: "uid=alice,ou=people,dc=test".into(),
            fields: vec![field],
            baseline: Default::default(),
            mode: FormMode::Edit,
            pending_password: None,
        });
        app
    }

    /// Feed each char of `s` through the popup key handler (edits the focused row).
    fn type_str(app: &mut App, s: &str) {
        for c in s.chars() {
            password_editor_key(app, key(KeyCode::Char(c)));
        }
    }

    #[test]
    fn open_refuses_when_not_encrypted() {
        let mut app = app_with_password_field(false);
        open_password_editor(&mut app);
        assert!(matches!(app.overlay, Some(Overlay::Error { .. })));
    }

    #[test]
    fn open_then_matching_commit_stages_pending_password() {
        let mut app = app_with_password_field(true);
        open_password_editor(&mut app);
        assert!(matches!(app.overlay, Some(Overlay::PasswordEditor(_))));
        type_str(&mut app, "hunter2");
        password_editor_key(&mut app, key(KeyCode::Tab));
        type_str(&mut app, "hunter2");
        password_editor_key(&mut app, alt(KeyCode::Char('s')));
        assert!(app.overlay.is_none());
        assert_eq!(
            app.form.as_ref().unwrap().pending_password.as_deref(),
            Some("hunter2")
        );
    }

    #[test]
    fn mismatch_does_not_commit() {
        let mut app = app_with_password_field(true);
        open_password_editor(&mut app);
        type_str(&mut app, "aaa");
        password_editor_key(&mut app, key(KeyCode::Tab));
        type_str(&mut app, "bbb");
        password_editor_key(&mut app, alt(KeyCode::Char('s')));
        assert!(
            matches!(app.overlay, Some(Overlay::PasswordEditor(_))),
            "stays open on mismatch"
        );
        assert!(app.form.as_ref().unwrap().pending_password.is_none());
    }

    #[test]
    fn empty_new_password_cancels() {
        let mut app = app_with_password_field(true);
        open_password_editor(&mut app);
        password_editor_key(&mut app, alt(KeyCode::Char('s')));
        assert!(app.overlay.is_none(), "empty new == cancel, closes popup");
        assert!(app.form.as_ref().unwrap().pending_password.is_none());
    }
}
