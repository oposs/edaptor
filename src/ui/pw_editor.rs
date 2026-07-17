//! The password field widget + TLS-gated New/Confirm editor. M3 Phase 2b:
//! `PasswordWidget` presents the masked ‹set›/‹unset› cell and opens a modal.
//! `PasswordEditor::into_view` builds a TLS-gated New + Confirm dialog that
//! keeps `staged_commit = StageSecret { attrs, cleartext }` live on match.

use tvision_rs::{
    delegate, ButtonFlags, ButtonRowAlign, Command, Context, Dialog, Event, FieldValue, InputLine,
    Key, KeyEvent, Rect, StaticText, View, ViewId,
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

/// A real, focusable `InputLine` that masks its content: the inner field only ever
/// holds bullets (so its caret/selection/scroll logic works normally and nothing
/// secret is ever rendered), while `real` mirrors the actual characters 1:1 with
/// those bullets. Editing keys are mirrored onto `real` from the inner field's
/// pre-event caret/selection, then a bulletised equivalent is fed to the inner
/// field. `select_all_on_focus` is off, so focusing the field never selects-all.
pub(crate) struct MaskedInputLine {
    inner: InputLine,
    real: String,
}

impl MaskedInputLine {
    fn new(bounds: Rect) -> Self {
        let mut inner = InputLine::with_limit(bounds, 1024);
        inner.set_select_all_on_focus(false);
        MaskedInputLine {
            inner,
            real: String::new(),
        }
    }

    /// Clear both the mirror and the visible bullets.
    fn clear(&mut self) {
        self.real.clear();
        self.inner.set_value(FieldValue::Text(String::new()));
    }

    /// The caret position as a char index into `real` (== bullet index in `inner`).
    fn caret_char(&self) -> usize {
        self.inner.data[..self.inner.cur_pos as usize]
            .chars()
            .count()
    }

    /// The active selection as a `[start, end)` char range, or `None` when empty.
    fn selection_chars(&self) -> Option<(usize, usize)> {
        let (s, e) = (self.inner.sel_start, self.inner.sel_end);
        if e > s {
            let a = self.inner.data[..s as usize].chars().count();
            let b = self.inner.data[..e as usize].chars().count();
            Some((a, b))
        } else {
            None
        }
    }

    /// Feed a synthetic single-key event to the inner (bullet) field.
    fn feed_inner(&mut self, key: Key, ctx: &mut Context) {
        let mut synth = Event::KeyDown(KeyEvent::from(key));
        self.inner.handle_event(&mut synth, ctx);
    }
}

#[delegate(to = inner)]
impl View for MaskedInputLine {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        match ev {
            Event::KeyDown(k) => match k.key {
                Key::Char(c) => {
                    // Delete any selection, then insert `c` at the caret — mirroring
                    // exactly what the inner field will do with the bullet below.
                    let mut chars: Vec<char> = self.real.chars().collect();
                    let at = match self.selection_chars() {
                        Some((a, b)) => {
                            chars.drain(a..b);
                            a
                        }
                        None => self.caret_char(),
                    };
                    let at = at.min(chars.len());
                    chars.insert(at, c);
                    self.real = chars.into_iter().collect();
                    self.feed_inner(Key::Char(BULLET), ctx);
                    ev.clear();
                }
                Key::Backspace => {
                    let mut chars: Vec<char> = self.real.chars().collect();
                    match self.selection_chars() {
                        Some((a, b)) => {
                            chars.drain(a..b);
                        }
                        None => {
                            let idx = self.caret_char();
                            if idx > 0 {
                                chars.remove(idx - 1);
                            }
                        }
                    }
                    self.real = chars.into_iter().collect();
                    self.feed_inner(Key::Backspace, ctx);
                    ev.clear();
                }
                Key::Delete => {
                    let mut chars: Vec<char> = self.real.chars().collect();
                    match self.selection_chars() {
                        Some((a, b)) => {
                            chars.drain(a..b);
                        }
                        None => {
                            let idx = self.caret_char();
                            if idx < chars.len() {
                                chars.remove(idx);
                            }
                        }
                    }
                    self.real = chars.into_iter().collect();
                    self.feed_inner(Key::Delete, ctx);
                    ev.clear();
                }
                // Navigation / selection / Tab / Enter: operate on the bullet field
                // (positions stay 1:1 with `real`); Tab & Enter fall through unhandled
                // so the dialog can move focus / fire the default button.
                _ => self.inner.handle_event(ev, ctx),
            },
            // Swallow clipboard commands: cut/paste would desync the mirror and copy
            // would leak the cleartext — a password field exposes none of them.
            Event::Command(cmd)
                if matches!(*cmd, Command::CUT | Command::COPY | Command::PASTE) =>
            {
                ev.clear();
            }
            _ => self.inner.handle_event(ev, ctx),
        }
    }
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

/// The masked New + Confirm password dialog. The two fields are real, focusable
/// [`MaskedInputLine`]s, so Tab/caret/focus all work natively (no phantom
/// select-all block, no invisible "active field" flag). Staging is recomputed
/// from the fields' mirrors after every event.
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
        let new_id = dlg.insert_child(Box::new(MaskedInputLine::new(Rect::new(2, 2, 54, 3))));

        dlg.insert_child(Box::new(StaticText::new(
            Rect::new(2, 4, 30, 5),
            "Confirm password:".to_string(),
        )));
        let confirm_id = dlg.insert_child(Box::new(MaskedInputLine::new(Rect::new(2, 5, 54, 6))));

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

    /// The cleartext currently held by a masked field.
    fn real_of(&mut self, id: ViewId) -> String {
        self.dlg
            .child_mut(id)
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<MaskedInputLine>())
            .map(|m| m.real.clone())
            .unwrap_or_default()
    }

    /// Clear a masked field's content.
    fn clear_field(&mut self, id: ViewId) {
        if let Some(m) = self
            .dlg
            .child_mut(id)
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<MaskedInputLine>())
        {
            m.clear();
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

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        // Native routing: the dialog delivers the event to the focused masked
        // field (which masks its own edits), moves focus on Tab, and fires OK /
        // Cancel. Afterwards, recompute the staged commit from the field mirrors.
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
    fn cell(pd: &mut PasswordDialog, id: ViewId) -> &mut MaskedInputLine {
        pd.dlg
            .child_mut(id)
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<MaskedInputLine>())
            .expect("masked cell")
    }

    /// Type a string into a masked cell (marking it selected so its inner
    /// InputLine accepts keys, as a real focused child would).
    fn typ(cell: &mut MaskedInputLine, s: &str, ctx: &mut Context) {
        cell.inner.state.state.selected = true;
        for c in s.chars() {
            let mut ev = Event::KeyDown(KeyEvent::from(Key::Char(c)));
            cell.handle_event(&mut ev, ctx);
        }
    }

    fn press(cell: &mut MaskedInputLine, key: Key, ctx: &mut Context) {
        cell.inner.state.state.selected = true;
        let mut ev = Event::KeyDown(KeyEvent::from(key));
        cell.handle_event(&mut ev, ctx);
    }

    /// A masked field mirrors typed chars 1:1 while only ever showing bullets, and
    /// edits (backspace / mid-caret insert / delete) keep the mirror in sync.
    #[test]
    fn masked_field_mirrors_real_and_masks_display() {
        let (mut out, mut timers, mut deferred) = ctx_deps();
        let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);
        let mut m = MaskedInputLine::new(Rect::new(0, 0, 40, 1));

        typ(&mut m, "abc", &mut ctx);
        assert_eq!(m.real, "abc");
        assert_eq!(
            m.inner.data, "\u{2022}\u{2022}\u{2022}",
            "display is all bullets"
        );

        press(&mut m, Key::Backspace, &mut ctx);
        assert_eq!(m.real, "ab");
        assert_eq!(m.inner.data, "\u{2022}\u{2022}");

        // Move caret home, insert 'X' at the front.
        press(&mut m, Key::Home, &mut ctx);
        typ(&mut m, "X", &mut ctx);
        assert_eq!(
            m.real, "Xab",
            "mid-caret insert lands at the caret, not the end"
        );

        // Delete-forward at the front removes 'X'.
        press(&mut m, Key::Home, &mut ctx);
        press(&mut m, Key::Delete, &mut ctx);
        assert_eq!(m.real, "ab");
    }

    /// Multibyte characters mirror correctly (bullet count == char count, no byte
    /// desync).
    #[test]
    fn masked_field_handles_multibyte() {
        let (mut out, mut timers, mut deferred) = ctx_deps();
        let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);
        let mut m = MaskedInputLine::new(Rect::new(0, 0, 40, 1));
        typ(&mut m, "pä€", &mut ctx);
        assert_eq!(m.real, "pä€");
        assert_eq!(m.inner.data.chars().count(), 3);
        assert!(m.inner.data.chars().all(|c| c == '\u{2022}'));
        press(&mut m, Key::Backspace, &mut ctx);
        assert_eq!(m.real, "pä");
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
        typ(cell(pd, new_id), "abc", &mut ctx);
        pd.update_staged();
        assert!(
            sh.borrow().staged_commit.is_none(),
            "new typed but confirm empty → None"
        );

        // Confirm = "abc" → match → StageSecret.
        typ(cell(pd, confirm_id), "abc", &mut ctx);
        pd.update_staged();
        match sh.borrow().staged_commit.clone() {
            Some(CommitOutcome::StageSecret { attrs, cleartext }) => {
                assert_eq!(attrs, vec!["userPassword".to_string()]);
                assert_eq!(cleartext, "abc");
            }
            other => panic!("expected StageSecret on match, got {other:?}"),
        }

        // Backspace confirm → "ab" ≠ "abc" → None.
        press(cell(pd, confirm_id), Key::Backspace, &mut ctx);
        pd.update_staged();
        assert!(
            sh.borrow().staged_commit.is_none(),
            "mismatch (confirm != new) → None"
        );

        // Re-type 'c' → match again.
        typ(cell(pd, confirm_id), "c", &mut ctx);
        pd.update_staged();
        assert!(
            matches!(sh.borrow().staged_commit, Some(CommitOutcome::StageSecret { ref cleartext, .. }) if cleartext == "abc"),
            "re-match → Some(StageSecret)"
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
