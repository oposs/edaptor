//! The password field widget + TLS-gated New/Confirm editor. M3 Phase 2b:
//! `PasswordWidget` presents the masked ‹set›/‹unset› cell and opens a modal.
//! `PasswordEditor::into_view` builds a TLS-gated New + Confirm dialog that
//! keeps `staged_commit = StageSecret { attrs, cleartext }` live on match.

use tvision_rs::{
    delegate, ButtonFlags, ButtonRowAlign, Command, Context, Dialog, Event, FieldValue, InputLine,
    Key, KeyEvent, KeyModifiers, Rect, StaticText, View, ViewId,
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
        // The inner field stores 3-byte bullets, so its byte cap admits ~limit/3
        // characters; 8 KiB leaves headroom for any realistic passphrase. The
        // accept-gate in `handle_event` keeps `real` in sync even at the cap.
        let mut inner = InputLine::with_limit(bounds, 8192);
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

    /// Insert one real character `c` at the caret, mirroring it as a bullet in
    /// the inner field. Any active selection is drained first, and the character
    /// is only recorded in `real` when the inner field actually grew (so its byte
    /// cap governs acceptance exactly as for typed input). Shared by the typed
    /// `Key::Char` arm and the bracketed-paste arm.
    fn insert_char_masked(&mut self, c: char, ctx: &mut Context) {
        // Capture caret/selection BEFORE feeding the inner field (feeding moves
        // the caret and clears the selection).
        let sel = self.selection_chars();
        let caret = sel.map(|(a, _)| a).unwrap_or_else(|| self.caret_char());
        let sel_len = sel.map(|(a, b)| b - a).unwrap_or(0);
        let pre = self.inner.data.chars().count();
        self.feed_inner(Key::Char(BULLET), ctx);
        // The inner field deletes any selection, then inserts the bullet UNLESS
        // its byte cap rejects it. Mirror exactly that: always drain the
        // selection, insert `c` only when the inner field grew.
        let accepted = self.inner.data.chars().count() > pre - sel_len;
        let mut chars: Vec<char> = self.real.chars().collect();
        if let Some((a, b)) = sel {
            chars.drain(a..b);
        }
        if accepted {
            chars.insert(caret.min(chars.len()), c);
        }
        self.real = chars.into_iter().collect();
    }

    /// Apply a positional edit to `real`, mirroring the inner field's own
    /// selection semantics: any active selection is drained first, then `op` runs
    /// with the resulting caret char index and whether a selection was drained
    /// (so a no-selection delete knows to remove the neighbouring char).
    fn mutate_real(&mut self, op: impl FnOnce(&mut Vec<char>, usize, bool)) {
        let sel = self.selection_chars();
        let caret = sel.map(|(a, _)| a).unwrap_or_else(|| self.caret_char());
        let mut chars: Vec<char> = self.real.chars().collect();
        if let Some((a, b)) = sel {
            chars.drain(a..b);
        }
        op(&mut chars, caret, sel.is_some());
        self.real = chars.into_iter().collect();
    }
}

#[delegate(to = inner)]
impl View for MaskedInputLine {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        // Extract the key + modifiers up front (all Copy) so the `ev` borrow ends
        // and the arms below can `ev.clear()` / forward `ev` freely.
        let (key, ctrl, alt) = match ev {
            Event::KeyDown(k) => (k.key, k.modifiers.ctrl, k.modifiers.alt),
            // Swallow clipboard commands: cut/paste would desync the mirror and copy
            // would leak the cleartext — a password field exposes none of them.
            // (This is the INTERNAL clipboard, Ctrl+V / Shift+Insert. The terminal's
            // bracketed paste arrives as `Event::Paste` and is masked below.)
            Event::Command(cmd)
                if matches!(*cmd, Command::CUT | Command::COPY | Command::PASTE) =>
            {
                ev.clear();
                return;
            }
            // Bracketed paste: mask it like typed input instead of letting the
            // cleartext reach the inner field (which would render it and leave
            // `real` empty — visible password AND nothing staged). Control chars
            // (a trailing newline, tabs) are dropped: a single-line password holds
            // none.
            Event::Paste(text) => {
                let text = std::mem::take(text);
                ev.clear();
                for c in text.chars().filter(|c| !c.is_control()) {
                    self.insert_char_masked(c, ctx);
                }
                return;
            }
            _ => {
                self.inner.handle_event(ev, ctx);
                return;
            }
        };
        match key {
            // Plain printable char (Shift is fine; Ctrl/Alt are not — the inner
            // field rejects those, so mirroring them would inject a stray char).
            Key::Char(c) if !ctrl && !alt => {
                self.insert_char_masked(c, ctx);
                ev.clear();
            }
            Key::Backspace if !ctrl && !alt => {
                self.mutate_real(|chars, caret, had_sel| {
                    if !had_sel && caret > 0 {
                        chars.remove(caret - 1);
                    }
                });
                self.feed_inner(Key::Backspace, ctx);
                ev.clear();
            }
            Key::Delete if !ctrl && !alt => {
                self.mutate_real(|chars, caret, had_sel| {
                    if !had_sel && caret < chars.len() {
                        chars.remove(caret);
                    }
                });
                self.feed_inner(Key::Delete, ctx);
                ev.clear();
            }
            // Never enter overwrite mode: the inner field would delete-then-insert,
            // keeping the bullet count constant while `real` grows — a silent
            // desync. Swallow Insert so the field stays insert-only.
            Key::Insert => ev.clear(),
            // Ctrl/Alt-modified edit keys (shortcuts, word-delete) would desync the
            // mirror — swallow them rather than half-applying.
            Key::Char(_) | Key::Backspace | Key::Delete => ev.clear(),
            // Navigation / selection / Tab / Enter: operate on the bullet field
            // (positions stay 1:1 with `real`); Tab & Enter fall through unhandled
            // so the dialog can move focus / fire the default button.
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

    /// Borrow a child masked field by id (the shared downcast chain).
    fn masked_mut(&mut self, id: ViewId) -> Option<&mut MaskedInputLine> {
        self.dlg
            .child_mut(id)
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<MaskedInputLine>())
    }

    /// The cleartext currently held by a masked field.
    fn real_of(&mut self, id: ViewId) -> String {
        self.masked_mut(id)
            .map(|m| m.real.clone())
            .unwrap_or_default()
    }

    /// Clear a masked field's content.
    fn clear_field(&mut self, id: ViewId) {
        if let Some(m) = self.masked_mut(id) {
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

    /// A bracketed paste (Event::Paste) must be masked exactly like typed
    /// characters: the cleartext mirrors into `real`, the display shows only
    /// bullets, and the event is consumed. RED before the fix: the paste fell
    /// through to the inner InputLine, which rendered the cleartext and left
    /// `real` empty (so nothing was staged either).
    #[test]
    fn masked_field_masks_bracketed_paste() {
        let (mut out, mut timers, mut deferred) = ctx_deps();
        let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);
        let mut m = MaskedInputLine::new(Rect::new(0, 0, 40, 1));
        m.inner.state.state.selected = true;
        let mut ev = Event::Paste("s3cr3t".to_string());
        m.handle_event(&mut ev, &mut ctx);
        assert!(ev.is_nothing(), "the paste event must be consumed");
        assert_eq!(m.real, "s3cr3t", "paste mirrors the cleartext into `real`");
        assert_eq!(m.inner.data.chars().count(), 6, "six bullets shown");
        assert!(
            m.inner.data.chars().all(|c| c == '\u{2022}'),
            "the display is all bullets, never the pasted cleartext"
        );
    }

    /// A pasted string with control characters (a password manager's trailing
    /// newline, embedded tabs/CR) drops them — a single-line password field can
    /// hold none of them, and forwarding them would desync the bullet mirror.
    #[test]
    fn masked_field_paste_strips_control_chars() {
        let (mut out, mut timers, mut deferred) = ctx_deps();
        let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);
        let mut m = MaskedInputLine::new(Rect::new(0, 0, 40, 1));
        m.inner.state.state.selected = true;
        let mut ev = Event::Paste("ab\tcd\r\n".to_string());
        m.handle_event(&mut ev, &mut ctx);
        assert_eq!(
            m.real, "abcd",
            "control characters are stripped from the paste"
        );
        assert_eq!(
            m.inner.data.chars().count(),
            4,
            "four bullets, mirror in sync"
        );
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

    /// A Ctrl/Alt-modified Char (a shortcut delivered as a Char event) must NOT be
    /// mirrored into the password — the inner field rejects it, so mirroring would
    /// inject a stray character the operator never typed.
    #[test]
    fn masked_field_ignores_modified_chars() {
        let (mut out, mut timers, mut deferred) = ctx_deps();
        let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);
        let mut m = MaskedInputLine::new(Rect::new(0, 0, 40, 1));
        typ(&mut m, "a", &mut ctx);
        m.inner.state.state.selected = true;
        let mut ev = Event::KeyDown(KeyEvent::new(
            Key::Char('b'),
            KeyModifiers {
                ctrl: true,
                ..KeyModifiers::default()
            },
        ));
        m.handle_event(&mut ev, &mut ctx);
        assert_eq!(
            m.real, "a",
            "ctrl-modified char must not enter the password"
        );
        assert_eq!(m.inner.data.chars().count(), 1, "bullets stay in sync");
    }

    /// The Insert key must not switch the field into overwrite mode — that would
    /// keep the bullet count constant while `real` grows, desyncing the mirror.
    #[test]
    fn masked_field_insert_key_never_overwrites() {
        let (mut out, mut timers, mut deferred) = ctx_deps();
        let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);
        let mut m = MaskedInputLine::new(Rect::new(0, 0, 40, 1));
        typ(&mut m, "ab", &mut ctx);
        press(&mut m, Key::Home, &mut ctx);
        press(&mut m, Key::Insert, &mut ctx); // must be swallowed
        typ(&mut m, "X", &mut ctx);
        assert_eq!(m.real, "Xab", "Insert must not engage overwrite mode");
        assert_eq!(m.inner.data.chars().count(), 3, "bullets track real 1:1");
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
