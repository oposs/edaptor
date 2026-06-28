//! The password field widget + TLS-gated New/Confirm editor. M3 Phase 2b:
//! `PasswordWidget` presents the masked ‹set›/‹unset› cell and opens a modal.
//! `PasswordEditor::into_view` builds a TLS-gated New + Confirm dialog that
//! keeps `staged_commit = StageSecret { attrs, cleartext }` live on match.

use tvision_rs::{
    delegate, ButtonFlags, ButtonRowAlign, Command, Context, Dialog, Event, FieldValue, InputLine,
    Key, Rect, StaticText, View, ViewId,
};

use crate::config::widget::WidgetKind;
use crate::schema::SchemaModel;
use crate::ui::widget::{Activation, Capability, CommitOutcome, FieldEditor, FieldWidget};
use crate::ui::Shared;
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

/// Carries the field's context into the editor. `attrs` is the list of LDAP
/// attributes to stage (primary from the binding; samba-derived added by fold).
pub(crate) struct PasswordEditor {
    label: String,
    attrs: Vec<String>,
}

impl PasswordEditor {
    /// Extract the primary attr from `WidgetKind::Password` binding.
    /// Intentionally does NOT borrow_mut shared during construction (2a lesson).
    pub(crate) fn for_field(field: &EditField) -> Self {
        let attrs = match &field.widget_binding {
            Some(WidgetKind::Password(pw)) => vec![pw.primary.clone()],
            _ => Vec::new(),
        };
        PasswordEditor {
            label: field.label.clone(),
            attrs,
        }
    }
}

impl FieldEditor for PasswordEditor {
    fn into_view(
        self: Box<Self>,
        _schema: &SchemaModel,
        shared: Shared,
    ) -> (Box<dyn View>, ViewId) {
        // Immutable borrow to read the flag; dropped immediately. Multiple
        // immutable RefCell borrows are allowed, so this is safe even if
        // dispatch holds one.
        let encrypted = shared.borrow().connection_encrypted;
        if !encrypted {
            refusal_dialog()
        } else {
            let pd = PasswordDialog::new(self.label, self.attrs, shared);
            let focus = pd.new_display_id;
            (Box::new(pd), focus)
        }
    }
}

/// A disabled (read-only, skip-focus) `InputLine` for displaying bullet text.
fn ro_cell(bounds: Rect) -> InputLine {
    let mut il = InputLine::with_limit(bounds, 1024);
    il.state.state.disabled = true;
    il
}

/// Build the TLS refusal dialog. Returned directly as a `Box<dyn View>`.
fn refusal_dialog() -> (Box<dyn View>, ViewId) {
    let msg = "Changing a password requires an encrypted connection \
               (ldaps://, ldapi://, or start_tls).";
    let mut dlg = Dialog::new(Rect::new(0, 0, 60, 9), Some("Set Password".to_string()));
    dlg.state_mut().options.center_x = true;
    dlg.state_mut().options.center_y = true;
    dlg.insert_child(Box::new(StaticText::new(
        Rect::new(2, 2, 58, 6),
        msg.to_string(),
    )));
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

/// The masked New + Confirm password dialog. Chars go to the active buffer;
/// display cells show bullets. Staging is updated live on every keystroke.
pub(crate) struct PasswordDialog {
    dlg: Dialog,
    new_display_id: ViewId,
    confirm_display_id: ViewId,
    new_buf: String,
    confirm_buf: String,
    active_field: u8, // 0 = new, 1 = confirm
    attrs: Vec<String>,
    shared: Shared,
}

impl PasswordDialog {
    fn new(label: String, attrs: Vec<String>, shared: Shared) -> Self {
        let title = format!("Set password — {label}");
        let mut dlg = Dialog::new(Rect::new(0, 0, 56, 13), Some(title));
        dlg.state_mut().options.center_x = true;
        dlg.state_mut().options.center_y = true;

        dlg.insert_child(Box::new(StaticText::new(
            Rect::new(2, 1, 30, 2),
            "New password:".to_string(),
        )));
        let new_display_id = dlg.insert_child(Box::new(ro_cell(Rect::new(2, 2, 54, 3))));

        dlg.insert_child(Box::new(StaticText::new(
            Rect::new(2, 4, 30, 5),
            "Confirm password:".to_string(),
        )));
        let confirm_display_id = dlg.insert_child(Box::new(ro_cell(Rect::new(2, 5, 54, 6))));

        dlg.button_row(
            &[
                (
                    "~O~K",
                    Command::OK,
                    ButtonFlags {
                        default: true,
                        ..ButtonFlags::new()
                    },
                ),
                ("~C~ancel", Command::CANCEL, ButtonFlags::new()),
            ],
            ButtonRowAlign::Right,
        );

        PasswordDialog {
            dlg,
            new_display_id,
            confirm_display_id,
            new_buf: String::new(),
            confirm_buf: String::new(),
            active_field: 0,
            attrs,
            shared,
        }
    }

    /// Refresh the two bullet-display cells from the current buffers.
    fn update_display(&mut self) {
        let new_bullets = "\u{2022}".repeat(self.new_buf.chars().count());
        let confirm_bullets = "\u{2022}".repeat(self.confirm_buf.chars().count());
        if let Some(c) = self.dlg.child_mut(self.new_display_id) {
            c.set_value(FieldValue::Text(new_bullets));
        }
        if let Some(c) = self.dlg.child_mut(self.confirm_display_id) {
            c.set_value(FieldValue::Text(confirm_bullets));
        }
    }

    /// Write the prospective commit into shared state. Short borrow, dropped here.
    fn update_staged(&self) {
        let outcome = if !self.new_buf.is_empty() && self.new_buf == self.confirm_buf {
            Some(CommitOutcome::StageSecret {
                attrs: self.attrs.clone(),
                cleartext: self.new_buf.clone(),
            })
        } else {
            None
        };
        self.shared.borrow_mut().staged_commit = outcome;
    }
}

#[delegate(to = dlg)]
impl View for PasswordDialog {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    /// Called by `exec_view` before the first draw. Clears buffers + staged.
    fn reset_current(&mut self, ctx: &mut Context) {
        self.dlg.reset_current(ctx);
        self.new_buf.clear();
        self.confirm_buf.clear();
        self.active_field = 0;
        self.update_display();
        self.update_staged();
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        let key = match ev {
            Event::KeyDown(k) => k.key,
            _ => {
                self.dlg.handle_event(ev, ctx);
                return;
            }
        };

        match key {
            Key::Char(c) => {
                if self.active_field == 0 {
                    self.new_buf.push(c);
                } else {
                    self.confirm_buf.push(c);
                }
                self.update_display();
                self.update_staged();
                ev.clear();
            }
            Key::Backspace => {
                if self.active_field == 0 {
                    self.new_buf.pop();
                } else {
                    self.confirm_buf.pop();
                }
                self.update_display();
                self.update_staged();
                ev.clear();
            }
            Key::Tab | Key::Down | Key::Up => {
                self.active_field = 1 - self.active_field;
                ev.clear();
            }
            _ => {
                self.dlg.handle_event(ev, ctx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldap::worker::RawSubschema;
    use std::cell::RefCell;
    use std::rc::Rc;
    use tvision_rs::{timer::TimerQueue, Deferred, KeyEvent};

    fn shared_with(encrypted: bool) -> Shared {
        use crate::workflows::structure::Structure;
        let raw = RawSubschema::default();
        let schema = crate::schema::SchemaModel::from_raw(&raw);
        let mut st = crate::ui::state::UiState::new_for_test(
            Structure::build("dc=example,dc=org", vec![]),
            schema,
            "dc=example,dc=org".into(),
            Vec::new(),
            Vec::new(),
        );
        st.connection_encrypted = encrypted;
        Rc::new(RefCell::new(st))
    }

    fn test_editor(attrs: Vec<String>) -> Box<PasswordEditor> {
        Box::new(PasswordEditor {
            label: "userPassword".into(),
            attrs,
        })
    }

    fn make_schema() -> SchemaModel {
        let raw = RawSubschema::default();
        SchemaModel::from_raw(&raw)
    }

    /// RED→GREEN: Unencrypted connection must produce a refusal dialog that never
    /// writes anything to staged_commit, even when key events are delivered.
    #[test]
    fn refuses_when_unencrypted() {
        let sh = shared_with(false);
        let schema = make_schema();

        let ed = test_editor(vec!["userPassword".into()]);
        let (mut view, _focus_id) = ed.into_view(&schema, sh.clone());

        let mut out: std::collections::VecDeque<Event> = std::collections::VecDeque::new();
        let mut timers = TimerQueue::new();
        let mut deferred: Vec<Deferred> = Vec::new();
        let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);

        view.reset_current(&mut ctx);

        // Send char events — the refusal dialog must never stage anything.
        for c in "secret".chars() {
            let mut ev = Event::KeyDown(KeyEvent::from(Key::Char(c)));
            view.handle_event(&mut ev, &mut ctx);
        }

        assert!(
            sh.borrow().staged_commit.is_none(),
            "refusal dialog must never stage anything; staged_commit must remain None"
        );
    }

    /// RED→GREEN: Encrypted connection, matching New + Confirm → StageSecret;
    /// mismatch → None.
    #[test]
    fn stages_when_match() {
        let sh = shared_with(true);
        let schema = make_schema();

        let ed = test_editor(vec!["userPassword".into()]);
        let (mut view, _focus_id) = ed.into_view(&schema, sh.clone());

        let mut out: std::collections::VecDeque<Event> = std::collections::VecDeque::new();
        let mut timers = TimerQueue::new();
        let mut deferred: Vec<Deferred> = Vec::new();
        let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);

        view.reset_current(&mut ctx);

        // Type "abc" into New field (active_field = 0).
        for c in "abc".chars() {
            let mut ev = Event::KeyDown(KeyEvent::from(Key::Char(c)));
            view.handle_event(&mut ev, &mut ctx);
        }

        // Confirm is empty → mismatch → None.
        assert!(
            sh.borrow().staged_commit.is_none(),
            "new typed but confirm empty → mismatch → staged_commit must be None"
        );

        // Tab to Confirm field (active_field becomes 1).
        let mut ev = Event::KeyDown(KeyEvent::from(Key::Tab));
        view.handle_event(&mut ev, &mut ctx);

        // Type "abc" into Confirm field → match.
        for c in "abc".chars() {
            let mut ev = Event::KeyDown(KeyEvent::from(Key::Char(c)));
            view.handle_event(&mut ev, &mut ctx);
        }

        let staged = sh.borrow().staged_commit.clone();
        match staged {
            Some(CommitOutcome::StageSecret { attrs, cleartext }) => {
                assert_eq!(attrs, vec!["userPassword".to_string()]);
                assert_eq!(cleartext, "abc");
            }
            other => panic!("expected StageSecret on match, got {other:?}"),
        }

        // Mismatch: backspace + different char in Confirm → None.
        let mut ev = Event::KeyDown(KeyEvent::from(Key::Backspace));
        view.handle_event(&mut ev, &mut ctx);
        // confirm = "ab", new = "abc" → mismatch
        assert!(
            sh.borrow().staged_commit.is_none(),
            "mismatch (confirm != new) → staged_commit must be None"
        );

        // Restore match by typing 'c' back.
        let mut ev = Event::KeyDown(KeyEvent::from(Key::Char('c')));
        view.handle_event(&mut ev, &mut ctx);
        // confirm = "abc", new = "abc" → match again
        let staged2 = sh.borrow().staged_commit.clone();
        assert!(
            matches!(staged2, Some(CommitOutcome::StageSecret { ref cleartext, .. }) if cleartext == "abc"),
            "re-match after fix → staged_commit must be Some(StageSecret)"
        );
    }

    /// Verify that for_field extracts the primary attr from WidgetKind::Password.
    #[test]
    fn for_field_extracts_primary_attr() {
        use crate::config::widget::{PasswordWidget as PwCfg, WidgetKind};
        use crate::schema::FieldKind;
        use crate::workflows::form_model::WidgetSpec;

        let field = EditField {
            label: "userPassword".into(),
            must: false,
            editable: true,
            multi: false,
            secret: true,
            ordered: false,
            orphaned: false,
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: Some(WidgetKind::Password(PwCfg {
                primary: "userPassword".into(),
                derived: vec![],
                samba: false,
            })),
            values: vec![],
            baseline: vec![],
        };
        let ed = PasswordEditor::for_field(&field);
        assert_eq!(ed.attrs, vec!["userPassword".to_string()]);
        assert_eq!(ed.label, "userPassword");
    }
}
