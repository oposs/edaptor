//! The password field widget + TLS-gated New/Confirm editor. M3 Phase 2b:
//! `PasswordWidget` presents the masked ‹set›/‹unset› cell and opens a modal.
//! `PasswordEditor::into_view` builds a TLS-gated New + Confirm dialog that
//! keeps `staged_commit = StageSecret { attrs, cleartext }` live on match.

use tvision_rs::{
    delegate, ButtonFlags, ButtonRowAlign, Command, Context, Dialog, Event, FieldValue, Key,
    KeyEvent, KeyModifiers, MaskedInput, MessageBoxButtons, MessageBoxKind, Rect, RevealEyeConfig,
    StaticText, View, ViewId,
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
            let focus = pd.new_id;
            (Box::new(pd), focus)
        }
    }
}

/// The character shown in place of every typed password character.
const BULLET: char = '\u{2022}'; // •

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

/// The masked New + Confirm password dialog. The two fields are native
/// tvision-rs [`MaskedInput`]s (masked InputLine + reveal eye), so Tab/caret/
/// focus/paste/clipboard all work natively — no local mirror. Staging is
/// recomputed from the fields' real text after every event.
pub(crate) struct PasswordDialog {
    dlg: Dialog,
    new_id: ViewId,
    confirm_id: ViewId,
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
        let new_id = dlg.insert_child(Box::new(MaskedInput::new(
            Rect::new(2, 2, 54, 3),
            8192,
            BULLET,
            RevealEyeConfig::default(),
        )));

        dlg.insert_child(Box::new(StaticText::new(
            Rect::new(2, 4, 30, 5),
            "Confirm password:".to_string(),
        )));
        let confirm_id = dlg.insert_child(Box::new(MaskedInput::new(
            Rect::new(2, 5, 54, 6),
            8192,
            BULLET,
            RevealEyeConfig::default(),
        )));

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
            new_id,
            confirm_id,
            attrs,
            shared,
        }
    }

    /// Borrow a child masked field by id (the shared downcast chain).
    fn masked_mut(&mut self, id: ViewId) -> Option<&mut MaskedInput> {
        self.dlg
            .child_mut(id)
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<MaskedInput>())
    }

    /// The cleartext currently held by a masked field.
    fn real_of(&mut self, id: ViewId) -> String {
        match self.masked_mut(id).and_then(|m| m.value()) {
            Some(FieldValue::Text(s)) => s,
            _ => String::new(),
        }
    }

    /// Clear a masked field's content.
    fn clear_field(&mut self, id: ViewId) {
        if let Some(m) = self.masked_mut(id) {
            m.set_value(FieldValue::Text(String::new()));
        }
    }

    /// Write the prospective commit into shared state. Short borrow, dropped here.
    fn update_staged(&mut self) {
        let new = self.real_of(self.new_id);
        let confirm = self.real_of(self.confirm_id);
        let outcome = if !new.is_empty() && new == confirm {
            Some(CommitOutcome::StageSecret {
                attrs: self.attrs.clone(),
                cleartext: new,
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

    /// Called by `exec_view` before the first draw. Clears both fields + staged.
    fn reset_current(&mut self, ctx: &mut Context) {
        self.dlg.reset_current(ctx);
        self.clear_field(self.new_id);
        self.clear_field(self.confirm_id);
        self.update_staged();
    }

    /// Gate the modal close on OK: refuse (and say why) unless a non-empty New
    /// password matches Confirm. The framework's `validate_modal_close` calls this
    /// before ending the modal — returning `false` keeps the dialog open with the
    /// fields intact, and the queued error box is driven inline. Without this the
    /// default OK button closed the dialog regardless of what the two fields held,
    /// staging nothing on a mismatch ("happy either way").
    fn valid(&mut self, cmd: Command, ctx: &mut Context) -> bool {
        // Cancel / Esc can never be vetoed.
        if cmd == Command::CANCEL {
            return true;
        }
        // Only the OK close is gated; anything else defers to the group.
        if cmd != Command::OK {
            return self.dlg.valid(cmd, ctx);
        }
        let new = self.real_of(self.new_id);
        let confirm = self.real_of(self.confirm_id);
        let problem = if new.is_empty() {
            Some("Enter a password.")
        } else if new != confirm {
            Some("The two passwords do not match.")
        } else {
            None
        };
        match problem {
            Some(msg) => {
                ctx.request_message_box(
                    msg.to_string(),
                    MessageBoxKind::Error,
                    MessageBoxButtons::ok(),
                    None,
                    None,
                );
                false
            }
            None => true,
        }
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        // The masked fields are single-line, so Up/Down would be dead keys. Map
        // them to Tab / Shift+Tab so arrows move between the fields and buttons,
        // matching the pre-rewrite dialog where Up/Down switched New/Confirm.
        if let Event::KeyDown(k) = ev {
            let as_tab = match k.key {
                Key::Down => Some(KeyModifiers::default()),
                Key::Up => Some(KeyModifiers {
                    shift: true,
                    ..KeyModifiers::default()
                }),
                _ => None,
            };
            if let Some(mods) = as_tab {
                *ev = Event::KeyDown(KeyEvent::new(Key::Tab, mods));
            }
        }
        // Native routing: the dialog delivers the event to the focused masked
        // field (which masks its own edits), moves focus on Tab, and fires OK /
        // Cancel. Afterwards, recompute the staged commit from the fields' real
        // text.
        self.dlg.handle_event(ev, ctx);
        self.update_staged();
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

    fn ctx_deps() -> (std::collections::VecDeque<Event>, TimerQueue, Vec<Deferred>) {
        (
            std::collections::VecDeque::new(),
            TimerQueue::new(),
            Vec::new(),
        )
    }

    /// Borrow a dialog's masked field mutably (test-only downcast).
    fn cell(pd: &mut PasswordDialog, id: ViewId) -> &mut MaskedInput {
        pd.dlg
            .child_mut(id)
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<MaskedInput>())
            .expect("masked cell")
    }

    /// Seed a masked field's real text directly. `MaskedInput`'s internals
    /// (its inner InputLine + reveal eye) are private, so simulating keystrokes
    /// isn't possible from outside the crate; `set_value` is the field's own
    /// real API for writing its content, and is exactly what a dialog's
    /// scatter pass would use.
    fn set_text(cell: &mut MaskedInput, s: &str) {
        cell.set_value(FieldValue::Text(s.to_string()));
    }

    /// `handle_event` maps Up/Down onto Tab / Shift+Tab so the arrows move between
    /// the fields (the dialog's native focus routing is exercised live — headless
    /// groups don't deliver keys to a focused child, so this asserts the rewrite).
    #[test]
    fn dialog_maps_up_down_to_tab() {
        let sh = shared_with(true);
        let schema = make_schema();
        let ed = test_editor(vec!["userPassword".into()]);
        let (mut view, _focus) = ed.into_view(&schema, sh.clone());
        let (mut out, mut timers, mut deferred) = ctx_deps();
        let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);
        view.reset_current(&mut ctx);

        // The handler rewrites Down→Tab before delegating, so the event is never
        // left as Down (it is either consumed as focus movement, or a Tab).
        let mut down = Event::KeyDown(KeyEvent::from(Key::Down));
        view.handle_event(&mut down, &mut ctx);
        assert!(
            !matches!(down, Event::KeyDown(k) if k.key == Key::Down),
            "Down must be remapped (to Tab) for field navigation"
        );
    }

    /// Encrypted connection: matching New + Confirm → StageSecret; mismatch → None.
    #[test]
    fn stages_when_match() {
        let sh = shared_with(true);
        let schema = make_schema();

        let ed = test_editor(vec!["userPassword".into()]);
        let (mut view, _focus_id) = ed.into_view(&schema, sh.clone());

        let (mut out, mut timers, mut deferred) = ctx_deps();
        let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);
        view.reset_current(&mut ctx);

        let pd = view
            .as_any_mut()
            .and_then(|a| a.downcast_mut::<PasswordDialog>())
            .expect("PasswordDialog");
        let (new_id, confirm_id) = (pd.new_id, pd.confirm_id);

        // New = "abc", Confirm still empty → mismatch → None.
        set_text(cell(pd, new_id), "abc");
        pd.update_staged();
        assert!(
            sh.borrow().staged_commit.is_none(),
            "new set but confirm empty → None"
        );

        // Confirm = "abc" → match → StageSecret.
        set_text(cell(pd, confirm_id), "abc");
        pd.update_staged();
        match sh.borrow().staged_commit.clone() {
            Some(CommitOutcome::StageSecret { attrs, cleartext }) => {
                assert_eq!(attrs, vec!["userPassword".to_string()]);
                assert_eq!(cleartext, "abc");
            }
            other => panic!("expected StageSecret on match, got {other:?}"),
        }

        // Shorten confirm → "ab" ≠ "abc" → None.
        set_text(cell(pd, confirm_id), "ab");
        pd.update_staged();
        assert!(
            sh.borrow().staged_commit.is_none(),
            "mismatch (confirm != new) → None"
        );

        // Re-set confirm → match again.
        set_text(cell(pd, confirm_id), "abc");
        pd.update_staged();
        assert!(
            matches!(sh.borrow().staged_commit, Some(CommitOutcome::StageSecret { ref cleartext, .. }) if cleartext == "abc"),
            "re-match → Some(StageSecret)"
        );
    }

    /// OK must be VETOED when New and Confirm differ: `valid(OK)` returns false and
    /// queues an error box. This is the "happy either way" fix — the default OK
    /// button used to close the dialog regardless.
    #[test]
    fn ok_is_vetoed_on_mismatch_and_accepted_on_match() {
        let sh = shared_with(true);
        let schema = make_schema();
        let ed = test_editor(vec!["userPassword".into()]);
        let (mut view, _focus) = ed.into_view(&schema, sh.clone());
        let (mut out, mut timers, mut deferred) = ctx_deps();

        // Scope 1: an empty then a mismatched New/Confirm — both veto OK.
        {
            let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);
            view.reset_current(&mut ctx);
            let pd = view
                .as_any_mut()
                .and_then(|a| a.downcast_mut::<PasswordDialog>())
                .expect("PasswordDialog");
            let (new_id, confirm_id) = (pd.new_id, pd.confirm_id);
            assert!(
                !pd.valid(Command::OK, &mut ctx),
                "OK must be vetoed while both fields are empty"
            );
            set_text(cell(pd, new_id), "abc");
            set_text(cell(pd, confirm_id), "abd");
            assert!(
                !pd.valid(Command::OK, &mut ctx),
                "OK must be vetoed when New != Confirm"
            );
        } // ctx dropped → free to inspect `deferred`.
        assert!(
            deferred
                .iter()
                .any(|d| matches!(d, Deferred::OpenMessageBox { .. })),
            "a mismatch veto must queue an error message box"
        );

        // Scope 2: fix Confirm to match → OK is accepted; Cancel never vetoed.
        {
            let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);
            let pd = view
                .as_any_mut()
                .and_then(|a| a.downcast_mut::<PasswordDialog>())
                .expect("PasswordDialog");
            let confirm_id = pd.confirm_id;
            set_text(cell(pd, confirm_id), "abc"); // "abd" → "abc", matches New
            assert!(
                pd.valid(Command::OK, &mut ctx),
                "OK must be accepted once New == Confirm"
            );
            assert!(
                pd.valid(Command::CANCEL, &mut ctx),
                "Cancel is never vetoed"
            );
        }
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
