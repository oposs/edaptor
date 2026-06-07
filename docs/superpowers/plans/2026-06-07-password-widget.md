# Password Widget Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make passwords a `[profile.widget.<attr>] kind="password"` widget whose editor is a TLS-gated New+Confirm popup that updates `userPassword` + (samba) `sambaNTPassword` + `sambaPwdLastSet` in one save; remove the old `[profile.password]` mechanism entirely.

**Architecture:** Extend the widget palette: `ResolvedWidget` gains a `WidgetKind { Choice | Password }` enum. A password-tagged field (primary or any derived hash field) opens `Overlay::PasswordEditor`; the new cleartext is staged in `EditForm.pending_password` (the fields are read-only, so it can't live in a field editor); the save/create paths derive the mods via the unchanged `samba::password::password_add_attrs`. No userbase → the old `PasswordSpec`/`inject_password_fields` path is deleted outright.

**Tech Stack:** Rust, ratatui, tui-prompts (`TextState`), serde/toml, `cargo test`. **Cap all cargo at 4 cores: `cargo build -j4`, `cargo test -j4`.**

**Spec:** `docs/superpowers/specs/2026-06-07-password-widget-design.md`

**Branch:** `fix-secret-fields-readonly` (this work continues the read-only fix; same branch).

---

## File structure

| File | Responsibility | Change |
|---|---|---|
| `src/config/mod.rs` | `ServerConfig::is_encrypted`; `WidgetSpecCfg::Password{samba}`; later remove `PasswordSpec` + `EntryProfile.password` | Modify |
| `src/config/widget.rs` | `WidgetKind { Choice(ChoiceWidget) \| Password(PasswordWidget) }`, `PasswordWidget`, resolve password, `widget_for -> &WidgetKind` | Modify |
| `src/ui/edit_form.rs` | `EditField.widget_binding: Option<WidgetKind>` (rename from `widget_choice`); `EditForm.pending_password`; `is_dirty` consults it; `tag_widget_fields` Password; delete `inject_password_fields` | Modify |
| `src/ui/app/overlay.rs` | `Overlay::PasswordEditor(PasswordEditor)` | Modify |
| `src/ui/app/password_editor.rs` *(new)* | `PasswordEditor` state + open (TLS gate) + key handling + commit → `pending_password` | New |
| `src/ui/view.rs` | render `PasswordEditor`; choice arm reads `widget_binding`; primary field "pending"/"↵ to set" display | Modify |
| `src/ui/app/input.rs` | route keys to `PasswordEditor`; Enter opens it | Modify |
| `src/ui/app/value_editor.rs` | choice branch reads `widget_binding`'s `Choice` arm | Modify |
| `src/ui/app/mod.rs` | `App.connection_encrypted`; resolve in `run`; wire `password_editor` module | Modify |
| `src/ui/app/save.rs` | `prepare_edit_save`: derive from `pending_password`, strip primary+derived, TLS guard | Modify |
| `src/ui/app/create.rs` | create: fold `pending_password` into the Add; drop injected-field path | Modify |
| `src/workflows/create.rs` | repoint `profile_for_entry`; remove `password_field_labels`/`stage_edit_password`/`stage_password` old form | Modify |
| `examples/demo-config.toml`, `examples/config.toml` | `[profile.password]` → `[profile.widget.userPassword] kind="password"` | Modify |
| `docs/src/configuration/widgets.md` | document `kind="password"`; drop `[profile.password]` | Modify |

> Tasks 1–8 are additive (the old `[profile.password]` path keeps compiling and the new popup is dormant because no test config wires a password *widget* yet). **Task 9 deletes the old path.** Run `cargo build -j4` after each task.

---

## Task 1: `Config::is_encrypted()`

**Files:** Modify `src/config/mod.rs`; Test in its `#[cfg(test)]`.

- [ ] **Step 1: Failing test**
```rust
#[test]
fn is_encrypted_reflects_ldaps_or_starttls() {
    let mk = |uri: &str, start_tls: bool| Config {
        server: ServerConfig {
            uri: uri.into(),
            base_dn: "dc=x".into(),
            start_tls,
            read_only: false,
            timeout_secs: 10,
            tls: Default::default(),
        },
        ..Default::default()
    };
    assert!(mk("ldaps://h:636", false).is_encrypted());
    assert!(mk("ldap://h:389", true).is_encrypted());
    assert!(mk("LDAPS://H", false).is_encrypted()); // case-insensitive
    assert!(!mk("ldap://h:389", false).is_encrypted());
}
```
(If `Config`/`ServerConfig` lack `Default`, build them the way the nearest existing config test does — check `src/config/mod.rs` tests for the constructor pattern and mirror it; the assertions are what matter.)

- [ ] **Step 2: Run, expect FAIL** — `cargo test -j4 --lib config::tests::is_encrypted_reflects 2>&1 | tail -15` → no method `is_encrypted`.

- [ ] **Step 3: Implement** — add to the `impl Config` block (next to `is_read_only`):
```rust
    /// Whether the LDAP connection is encrypted (LDAPS or StartTLS). Password
    /// changes require this — `userPassword` is sent in clear for the server to
    /// hash.
    pub fn is_encrypted(&self) -> bool {
        self.server.start_tls || self.server.uri.to_ascii_lowercase().starts_with("ldaps://")
    }
```

- [ ] **Step 4: Run, expect PASS** — `cargo test -j4 --lib config::tests::is_encrypted_reflects 2>&1 | tail -8`.

- [ ] **Step 5: Commit**
```bash
git add src/config/mod.rs && git commit -m "feat(config): Config::is_encrypted (LDAPS or StartTLS)"
```

---

## Task 2: `WidgetSpecCfg::Password` config variant

**Files:** Modify `src/config/mod.rs`; Test in its `#[cfg(test)]`.

- [ ] **Step 1: Failing test**
```rust
#[test]
fn parses_password_widget() {
    let toml = r#"
[server]
uri = "ldaps://x"
base_dn = "dc=x"
[auth]

[[profile]]
name = "user"
object_classes = ["inetOrgPerson"]

[profile.widget.userPassword]
kind = "password"
samba = true
"#;
    let cfg: Config = toml::from_str(toml).expect("parse");
    let p = &cfg.profiles[0];
    match &p.widgets["userPassword"] {
        WidgetSpecCfg::Password { samba } => assert!(*samba),
        other => panic!("expected password, got {other:?}"),
    }
}
```
(Match the minimal `[server]`/`[auth]` shape used by the existing widget test `parses_profile_widget_table`.)

- [ ] **Step 2: Run, expect FAIL** — variant doesn't exist.

- [ ] **Step 3: Implement** — add the variant to the existing tagged enum:
```rust
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum WidgetSpecCfg {
    Choice { select: String, format: String, options: Vec<ChoiceOption> },
    Password {
        #[serde(default)]
        samba: bool,
    },
}
```

- [ ] **Step 4: Run, expect PASS** — `cargo test -j4 --lib config 2>&1 | tail -8`.

- [ ] **Step 5: Commit**
```bash
git add src/config/mod.rs && git commit -m "feat(config): parse [profile.widget.<attr>] kind=password"
```

---

## Task 3: Widget resolution → `WidgetKind` enum + `PasswordWidget`

**Files:** Modify `src/config/widget.rs` (+ choice call sites). Test in `config::widget` tests.

This refactors the resolved type from `ResolvedWidget{widget: ChoiceWidget}` to `ResolvedWidget{kind: WidgetKind}`. Update the two choice consumers (`src/ui/app/value_editor.rs` open-choice branch, `src/ui/view.rs::field_display_value`) to read the `Choice` arm — but those use `EditField.widget_choice`, which is changed in Task 4; for THIS task only `widget.rs` and its tests change. Keep `widget_for` returning the kind.

- [ ] **Step 1: Failing tests** (add to `config::widget` tests)
```rust
#[test]
fn resolves_password_widget_with_derived() {
    let mut p = EntryProfile::default();
    p.name = "user".into();
    p.object_classes = vec!["inetOrgPerson".into()];
    p.widgets.insert(
        "userPassword".into(),
        WidgetSpecCfg::Password { samba: true },
    );
    let resolved = resolve_widgets(&vec![p]).expect("ok");
    match widget_for(&resolved, &["inetOrgPerson".into()], "userPassword").unwrap() {
        WidgetKind::Password(pw) => {
            assert_eq!(pw.primary, "userPassword");
            assert!(pw.samba);
            assert_eq!(pw.derived, vec!["sambaNTPassword".to_string(), "sambaPwdLastSet".to_string()]);
        }
        _ => panic!("expected password widget"),
    }
}

#[test]
fn resolves_password_widget_without_samba_has_no_derived() {
    let mut p = EntryProfile::default();
    p.name = "u".into();
    p.object_classes = vec!["inetOrgPerson".into()];
    p.widgets.insert("userPassword".into(), WidgetSpecCfg::Password { samba: false });
    let resolved = resolve_widgets(&vec![p]).unwrap();
    match widget_for(&resolved, &["inetOrgPerson".into()], "userPassword").unwrap() {
        WidgetKind::Password(pw) => assert!(pw.derived.is_empty()),
        _ => panic!(),
    }
}
```
Also update the EXISTING choice tests: they call `widget_for(...).unwrap()` and read `.select`/`.format` directly; wrap with `match … { WidgetKind::Choice(w) => { … } _ => panic!() }`.

- [ ] **Step 2: Run, expect FAIL** — `WidgetKind` undefined.

- [ ] **Step 3: Implement** — in `src/config/widget.rs`:
```rust
/// A resolved password widget: the primary cleartext attr plus the derived
/// attrs written alongside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordWidget {
    pub primary: String,
    pub derived: Vec<String>,
    pub samba: bool,
}

/// A resolved widget of any palette kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WidgetKind {
    Choice(ChoiceWidget),
    Password(PasswordWidget),
}
```
Change `ResolvedWidget`:
```rust
pub struct ResolvedWidget {
    pub owner_object_classes: Vec<String>,
    pub attr: String,
    pub kind: WidgetKind,
}
```
In `resolve_widgets`, wrap the existing choice build in `WidgetKind::Choice(ChoiceWidget{…})` and add the password arm:
```rust
            let kind = match spec {
                WidgetSpecCfg::Choice { select, format, options } => {
                    // ... existing validation producing ChoiceWidget ...
                    WidgetKind::Choice(ChoiceWidget { select, format, options: options.clone() })
                }
                WidgetSpecCfg::Password { samba } => {
                    let derived = if *samba {
                        vec!["sambaNTPassword".to_string(), "sambaPwdLastSet".to_string()]
                    } else {
                        Vec::new()
                    };
                    WidgetKind::Password(PasswordWidget {
                        primary: attr.clone(),
                        derived,
                        samba: *samba,
                    })
                }
            };
            out.push(ResolvedWidget { owner_object_classes: owner.object_classes.clone(), attr: attr.clone(), kind });
```
Change `widget_for` to `-> Option<&WidgetKind>` returning `&w.kind`.

- [ ] **Step 4: Run, expect PASS** — `cargo test -j4 --lib config::widget 2>&1 | tail -20`. (`cargo build -j4` will fail in `ui` until Task 4 — that's expected; for THIS task verify only the `config::widget` tests + `cargo build -j4 -p`… no: the crate is one package. So expect `cargo build` to break in ui. To keep this task self-contained, ALSO do the trivial Task-4 rename in the same commit IF the build won't compile otherwise. PREFERRED: merge Task 3 and Task 4 into one commit since the enum rename forces the `widget_choice` consumers to change together.)

> **Note:** Tasks 3 and 4 are mutually dependent (the enum change forces the `EditField`/consumer change). Execute them as ONE unit: do Task 3's type changes, then immediately Task 4's rename + consumer updates, build clean, then commit once with both messages folded.

- [ ] **Step 5: Commit** (after Task 4 compiles) — see Task 4.

---

## Task 4: `EditField.widget_binding` + `EditForm.pending_password`

**Files:** Modify `src/ui/edit_form.rs`, `src/ui/app/value_editor.rs`, `src/ui/view.rs`, and every `EditField`/`EditForm` literal. Test in `edit_form` tests.

- [ ] **Step 1: Rename the field** — in `EditField`, `pub widget_choice: Option<crate::config::widget::ChoiceWidget>` → `pub widget_binding: Option<crate::config::widget::WidgetKind>`. Add to `EditForm`:
```rust
    /// A password change staged by the PasswordEditor popup (cleartext), pending
    /// the next save. The password fields are read-only, so the new value cannot
    /// live in a field editor; it lives here. Cleared on save/revert.
    pub pending_password: Option<String>,
```

- [ ] **Step 2: Fix all literals + consumers (cargo-build-driven)** — `cargo build -j4 2>&1 | grep -E "error|widget_choice|pending_password"`:
  - Every `EditField { … widget_choice: … }` → `widget_binding: …`. Where it was `Some(choice_widget)` it becomes `Some(WidgetKind::Choice(choice_widget))`; `None` stays `None`.
  - Every `EditForm { … }` literal gains `pending_password: None`.
  - `value_editor.rs` open-choice branch: it reads `field.widget_choice` → now `field.widget_binding`; match `Some(WidgetKind::Choice(w))` to get the `ChoiceWidget`. The `ValueEditor.choice: Option<ChoiceWidget>` field is unchanged (still holds a `ChoiceWidget`).
  - `view.rs::field_display_value`: the choice summary branch reads `field.widget_choice` → `if let Some(WidgetKind::Choice(w)) = &field.widget_binding { … }`.
  - `tag_widget_fields`: currently sets `field.widget_choice = Some(w.clone())` from `widget_for` (which returned `&ChoiceWidget`); now `widget_for` returns `&WidgetKind`. For this task, set `field.widget_binding = Some(kind.clone())` only for the `WidgetKind::Choice` arm (Password handling is Task 5).
  - `input.rs` inline-edit guard: `field.widget_choice.is_none()` → `field.widget_binding.is_none()`.

- [ ] **Step 3: Failing test for pending_password dirty**
```rust
#[test]
fn pending_password_makes_form_dirty() {
    let mut form = writable_form();
    assert!(!form.is_dirty());
    form.pending_password = Some("hunter2".into());
    assert!(form.is_dirty(), "a staged password change is dirty");
}
```

- [ ] **Step 4: Run, expect FAIL** — `is_dirty` ignores `pending_password`.

- [ ] **Step 5: Implement** — in `is_dirty`, add at the top:
```rust
        if self.pending_password.is_some() {
            return true;
        }
```

- [ ] **Step 6: Run, expect PASS + full build** — `cargo test -j4 --lib 2>&1 | tail -6` and `cargo build -j4 2>&1 | tail -3`.

- [ ] **Step 7: Commit (Tasks 3+4)**
```bash
git add -A && git commit -m "refactor(widget): WidgetKind enum + PasswordWidget; EditField.widget_binding; EditForm.pending_password"
```

---

## Task 5: `tag_widget_fields` handles Password (primary + derived)

**Files:** Modify `src/ui/edit_form.rs`. Test in `edit_form` tests.

- [ ] **Step 1: Failing test**
```rust
#[test]
fn tag_widget_fields_tags_primary_and_derived_for_password() {
    use crate::config::widget::{PasswordWidget, ResolvedWidget, WidgetKind};
    let mut form = writable_form(); // has userPassword field
    // add a sambaNTPassword field to the form
    form.fields.push(EditField {
        label: "sambaNTPassword".into(), must: false, editable: false, multi: false,
        secret: true, ordered: false, values: vec!["DEAD".into()],
        kind: crate::schema::FieldKind::Text, widget: crate::ui::form::WidgetSpec::ReadOnlyText,
        editor: TextState::new(), picker: None, widget_binding: None,
    });
    let widgets = vec![ResolvedWidget {
        owner_object_classes: vec!["demoPerson".into()],
        attr: "userPassword".into(),
        kind: WidgetKind::Password(PasswordWidget {
            primary: "userPassword".into(),
            derived: vec!["sambaNTPassword".into(), "sambaPwdLastSet".into()],
            samba: true,
        }),
    }];
    tag_widget_fields(&mut form, &widgets, &["demoPerson".to_string()], false);
    let tagged = |n: &str| matches!(
        form.fields.iter().find(|f| f.label == n).unwrap().widget_binding,
        Some(WidgetKind::Password(_))
    );
    assert!(tagged("userPassword"));
    assert!(tagged("sambaNTPassword"));
}
```

- [ ] **Step 2: Run, expect FAIL** — only the primary (or neither) is tagged.

- [ ] **Step 3: Implement** — rewrite `tag_widget_fields` to iterate resolved widgets and match by kind:
```rust
pub fn tag_widget_fields(
    form: &mut EditForm,
    widgets: &[crate::config::widget::ResolvedWidget],
    object_classes: &[String],
    read_only: bool,
) {
    use crate::config::widget::WidgetKind;
    if read_only {
        return;
    }
    let has_oc = |ocs: &[String]| {
        ocs.iter()
            .any(|oc| object_classes.iter().any(|e| e.eq_ignore_ascii_case(oc)))
    };
    for rw in widgets {
        if !has_oc(&rw.owner_object_classes) {
            continue;
        }
        match &rw.kind {
            WidgetKind::Choice(_) => {
                if let Some(f) = form.fields.iter_mut().find(|f| f.label.eq_ignore_ascii_case(&rw.attr)) {
                    f.widget_binding = Some(rw.kind.clone());
                    f.editable = true;
                }
            }
            WidgetKind::Password(pw) => {
                let mut targets = vec![pw.primary.clone()];
                targets.extend(pw.derived.iter().cloned());
                for f in form.fields.iter_mut() {
                    if targets.iter().any(|t| t.eq_ignore_ascii_case(&f.label)) {
                        f.widget_binding = Some(rw.kind.clone());
                        // password fields stay read-only inline; Enter opens the popup
                    }
                }
            }
        }
    }
}
```

- [ ] **Step 4: Run, expect PASS** — `cargo test -j4 --lib edit_form 2>&1 | tail -10`.

- [ ] **Step 5: Commit**
```bash
git add src/ui/edit_form.rs && git commit -m "feat(ui): tag_widget_fields tags password primary + derived fields"
```

---

## Task 6: `PasswordEditor` overlay (open / keys / render / commit)

**Files:** Create `src/ui/app/password_editor.rs`; Modify `src/ui/app/overlay.rs`, `src/ui/app/mod.rs` (mod decl + `App.connection_encrypted`), `src/ui/app/input.rs`, `src/ui/app/value_editor.rs` (open trigger), `src/ui/view.rs` (render). Test in `password_editor` tests.

- [ ] **Step 1: Overlay variant + App field + module**
  - `overlay.rs`: add `PasswordEditor(crate::ui::app::password_editor::PasswordEditor)` to `Overlay`.
  - `mod.rs`: `pub(crate) mod password_editor;` ; add `pub connection_encrypted: bool` to `App`; in `run`, `let connection_encrypted = config.is_encrypted();` and set it in the `App { … }` literal; add `connection_encrypted: false` (or a sensible default) to test `App` literals (`test_support.rs`, view.rs, value_editor.rs) — cargo-build-driven.

- [ ] **Step 2: PasswordEditor state + open + commit (TDD the open gate + commit)**

Create `src/ui/app/password_editor.rs`:
```rust
//! The set-password popup: a TLS-gated New + Confirm editor that stages a
//! cleartext password into `EditForm.pending_password` (the password fields are
//! read-only; the new value cannot live in a field editor). The actual derive +
//! write happens in the save path.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tui_prompts::{State, TextState};

use super::overlay::Overlay;
use super::App;

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum PwField {
    New,
    Confirm,
}

pub struct PasswordEditor {
    pub new: TextState<'static>,
    pub confirm: TextState<'static>,
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
    let Some(form) = app.form.as_ref() else { return };
    let Some(field) = form.fields.get(focus) else { return };
    let Some(WidgetKind::Password(pw)) = field.widget_binding.clone() else { return };
    if !app.connection_encrypted {
        app.overlay = Some(Overlay::Error {
            text: "Changing a password requires an encrypted connection (ldaps:// or start_tls)."
                .to_string(),
        });
        return;
    }
    let mut affected = vec![pw.primary.clone()];
    affected.extend(pw.derived.iter().cloned());
    app.overlay = Some(Overlay::PasswordEditor(PasswordEditor::new_for(affected)));
}

/// Key handling for the password popup.
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
            let Some(Overlay::PasswordEditor(ed)) = app.overlay.as_ref() else { return };
            let new = ed.new.value().to_string();
            let confirm = ed.confirm.value().to_string();
            if new.is_empty() {
                app.overlay = None; // empty == cancel, no change
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
```

Tests (in `password_editor.rs` `#[cfg(test)]`, mirror `value_editor.rs` test helpers — build a `bare_app`, set `app.focus = Pane::Form`, a form whose focused field has `widget_binding: Some(WidgetKind::Password(..))`; helpers `key`/`alt`):
```rust
#[test]
fn open_refuses_when_not_encrypted() {
    let mut app = app_with_password_field(false); // connection_encrypted = false
    open_password_editor(&mut app);
    assert!(matches!(app.overlay, Some(Overlay::Error { .. })));
}

#[test]
fn open_then_matching_commit_stages_pending_password() {
    let mut app = app_with_password_field(true);
    open_password_editor(&mut app);
    assert!(matches!(app.overlay, Some(Overlay::PasswordEditor(_))));
    type_str(&mut app, "hunter2");          // into New
    password_editor_key(&mut app, key(KeyCode::Tab));
    type_str(&mut app, "hunter2");          // into Confirm
    password_editor_key(&mut app, alt(KeyCode::Char('s')));
    assert!(app.overlay.is_none());
    assert_eq!(app.form.as_ref().unwrap().pending_password.as_deref(), Some("hunter2"));
}

#[test]
fn mismatch_does_not_commit() {
    let mut app = app_with_password_field(true);
    open_password_editor(&mut app);
    type_str(&mut app, "aaa");
    password_editor_key(&mut app, key(KeyCode::Tab));
    type_str(&mut app, "bbb");
    password_editor_key(&mut app, alt(KeyCode::Char('s')));
    assert!(matches!(app.overlay, Some(Overlay::PasswordEditor(_))), "stays open on mismatch");
    assert!(app.form.as_ref().unwrap().pending_password.is_none());
}
```
Write `app_with_password_field(encrypted: bool)` and `type_str` helpers next to these (a one-field form, the field `widget_binding: Some(WidgetKind::Password(PasswordWidget{primary:"userPassword",derived:vec![],samba:false}))`, `editable:false`, `secret:true`; set `app.connection_encrypted = encrypted`, `app.focus = Pane::Form`, `app.form_focus = 0`).

- [ ] **Step 3: Run, expect FAIL** then implement (the code above) then PASS — `cargo test -j4 --lib password_editor 2>&1 | tail -20`.

- [ ] **Step 4: Wire Enter + key routing**
  - `value_editor.rs::open_value_editor`: at the TOP, before the choice/picker branches, add:
    ```rust
    if matches!(field.widget_binding, Some(crate::config::widget::WidgetKind::Password(_))) {
        super::password_editor::open_password_editor(app);
        return;
    }
    ```
    (Adjust: `open_value_editor` borrows `form`/`field`; call `open_password_editor(app)` after dropping the borrow, mirroring the existing structure — it re-reads `app.form_focus`.)
  - `input.rs::overlay_key`: add an arm `Some(Overlay::PasswordEditor(_)) => { super::password_editor::password_editor_key(app, key); None }` alongside the `ValueEditor` arm.

- [ ] **Step 5: Render the popup** — `view.rs::render_overlay`: add a `Overlay::PasswordEditor(ed)` arm calling a new `render_password_editor(f, ed, area)` that draws a centered box: title `Set password`, two rows `New password: ` + masked bullets (`"•".repeat(ed.new.value().len())`) and `Confirm: ` likewise, the selected row cursor, a dim `Updates: {ed.affected.join(", ")}` line, the `ed.message` in red if non-empty, and hint `Alt+S set · Alt+C cancel`. Add a render test asserting the buffer contains "Set password" and "Updates:" and no cleartext (mirror `render_value_editor_*` tests).

- [ ] **Step 6: Build + full suite** — `cargo build -j4 2>&1 | tail -3 && cargo test -j4 --lib 2>&1 | tail -5`.

- [ ] **Step 7: Commit**
```bash
git add -A && git commit -m "feat(ui): PasswordEditor popup (TLS-gated) staging EditForm.pending_password"
```

---

## Task 7: Save path derives from `pending_password`

**Files:** Modify `src/ui/app/save.rs` (`prepare_edit_save`); reuse `samba::password::password_add_attrs`. Test in `save.rs`/`workflows::save` tests as appropriate.

Replace the `stage_edit_password`-from-injected-fields logic with `pending_password`-driven derivation. The password widget for the entry gives `primary`+`derived`+`samba`.

- [ ] **Step 1: Failing test** — a unit test on a small helper. Add a pure helper `stage_pending_password` in `src/workflows/save.rs`:
```rust
/// If `pending` is Some, derive the password mods and strip `primary`+`derived`
/// from BOTH sides so the plain diff never double-writes them. Returns
/// (mods, mask_attrs). `now_secs` injected for testability.
pub fn stage_pending_password(
    pending: Option<&str>,
    primary: &str,
    derived: &[String],
    samba: bool,
    now_secs: u64,
    original: &mut std::collections::BTreeMap<String, Vec<String>>,
    edited: &mut std::collections::BTreeMap<String, Vec<String>>,
) -> (Vec<ModOp>, Vec<String>) {
    let strip = |m: &mut std::collections::BTreeMap<String, Vec<String>>| {
        m.retain(|k, _| {
            !k.eq_ignore_ascii_case(primary) && !derived.iter().any(|d| d.eq_ignore_ascii_case(k))
        });
    };
    strip(original);
    strip(edited);
    let Some(pw) = pending else { return (Vec::new(), Vec::new()) };
    let mods: Vec<ModOp> = crate::samba::password::password_add_attrs(pw, primary, samba, now_secs)
        .into_iter()
        .map(|(attr, values)| ModOp::Replace { attr, values })
        .collect();
    let mut mask = vec![primary.to_string()];
    mask.extend(derived.iter().cloned());
    (mods, mask)
}
```
Test:
```rust
#[test]
fn stage_pending_password_derives_and_strips() {
    let mut orig = BTreeMap::from([
        ("userPassword".to_string(), vec!["{SSHA}old".to_string()]),
        ("sambaNTPassword".to_string(), vec!["OLD".to_string()]),
        ("cn".to_string(), vec!["A".to_string()]),
    ]);
    let mut edited = orig.clone();
    let derived = vec!["sambaNTPassword".to_string(), "sambaPwdLastSet".to_string()];
    let (mods, mask) = stage_pending_password(
        Some("hunter2"), "userPassword", &derived, true, 1_700_000_000, &mut orig, &mut edited);
    assert!(mods.iter().any(|m| matches!(m, ModOp::Replace { attr, .. } if attr == "userPassword")));
    assert!(mods.iter().any(|m| matches!(m, ModOp::Replace { attr, .. } if attr == "sambaNTPassword")));
    assert!(!orig.contains_key("userPassword") && !edited.contains_key("sambaNTPassword"));
    assert!(orig.contains_key("cn"));
    assert!(mask.contains(&"userPassword".to_string()));
}

#[test]
fn stage_pending_password_none_only_strips() {
    let mut orig = BTreeMap::from([("userPassword".to_string(), vec!["x".to_string()])]);
    let mut edited = orig.clone();
    let (mods, mask) = stage_pending_password(None, "userPassword", &[], false, 0, &mut orig, &mut edited);
    assert!(mods.is_empty() && mask.is_empty());
    assert!(!orig.contains_key("userPassword"));
}
```

- [ ] **Step 2: Run FAIL → implement the helper → PASS** — `cargo test -j4 --lib stage_pending_password 2>&1 | tail -12`.

- [ ] **Step 3: Wire into `prepare_edit_save`** — replace the `stage_edit_password` call. `prepare_edit_save` needs the entry's password widget (primary/derived/samba) and the `pending_password`. Thread `widgets: &[ResolvedWidget]` and `pending: Option<&str>` (or pass the whole form — it already has `form.pending_password`) into `prepare_edit_save`. Find the password widget via `widget_for(widgets, &object_classes, <primary>)` — but you don't know the primary; instead scan `widgets` for a `WidgetKind::Password` whose `owner_object_classes` overlap `object_classes`. Add a small `password_widget_for(widgets, ocs) -> Option<&PasswordWidget>` in `config::widget`. Then:
```rust
let (password_mods, mask_attrs) = match password_widget_for(widgets, &object_classes) {
    Some(pw) => stage_pending_password(
        form.pending_password.as_deref(), &pw.primary, &pw.derived, pw.samba,
        now_secs, &mut original.attrs, &mut edited.attrs),
    None => (Vec::new(), Vec::new()),
};
```
Update `prepare_edit_save`'s signature + callers (`&app.widgets`). **TLS guard:** if `form.pending_password.is_some()` and the connection is not encrypted, return `Err("password change requires an encrypted connection")` — thread an `encrypted: bool` arg (pass `app.connection_encrypted`).

- [ ] **Step 4: Build + suite** — `cargo build -j4 2>&1 | tail -3 && cargo test -j4 --lib 2>&1 | tail -5`. (The old `stage_edit_password` is now unused by edit; leave it until Task 9 or delete now if trivial.)

- [ ] **Step 5: Commit**
```bash
git add -A && git commit -m "feat(save): derive password mods from EditForm.pending_password (TLS-guarded)"
```

---

## Task 8: Create flow folds `pending_password`

**Files:** Modify `src/ui/app/create.rs`, `src/workflows/create.rs`. Test in `workflows::create` / `ui::app::create` tests.

- [ ] **Step 1: Repoint `profile_for_entry`** — `src/workflows/create.rs:154` currently uses `|p| p.password.is_some()`. After `EntryProfile.password` is removed (Task 9) this won't compile, so repoint it now to "profile has a password widget": `|p| p.widgets.values().any(|w| matches!(w, WidgetSpecCfg::Password { .. }))`. Update its tests accordingly.

- [ ] **Step 2: Fold pending_password into the Add** — in `prepare_create` (the create-confirm planner), after composing the new entry attrs, if `form.pending_password` is `Some(pw)`: look up the profile's password widget (samba?), insert `password_add_attrs(pw, primary, samba, now)` into the attrs. TLS guard: refuse if not encrypted. Add a test that a create with a staged password includes `userPassword` (+ samba attrs) in the planned Add, and without one omits them. (Mirror the existing create tests' structure.)

- [ ] **Step 3: Drop the injected-field create path** — in `src/ui/app/create.rs`, remove the `stage_password`/`profile.password`/`inject_password_fields` usage (lines ~56, ~75, ~143). The create form now relies on `tag_widget_fields` (already called) to tag the password field; Enter opens the popup.

- [ ] **Step 4: Build + suite** — `cargo build -j4 2>&1 | tail -5 && cargo test -j4 --lib 2>&1 | tail -5`.

- [ ] **Step 5: Commit**
```bash
git add -A && git commit -m "feat(create): set new-entry password via the popup/pending_password"
```

---

## Task 9: Remove the old `[profile.password]` mechanism

**Files:** `src/config/mod.rs`, `src/ui/edit_form.rs`, `src/workflows/create.rs`, `src/ui/app/action.rs`, and any remaining call sites. cargo-build-driven.

- [ ] **Step 1: Delete** — remove `PasswordSpec` struct, `EntryProfile.password` field, `password_field_labels`, `inject_password_fields`, and `stage_edit_password`/`stage_password` (now unused). Remove the `inject_password_fields` call in `build_loaded_form` (`action.rs`) and any in `create.rs`.

- [ ] **Step 2: Fix fallout** — `cargo build -j4 2>&1 | tail -30`: delete/upgrade tests that referenced the removed items (`inject_password_replaces_…`, `injected_blank_password_is_not_dirty`, `stage_password_*`, `stage_edit_password_*`, `profile_for_entry_requires_…password_spec`). For behaviours still relevant, rewrite against the new path; for ones testing deleted code, remove them. Note in the commit which tests were removed and why.

- [ ] **Step 3: Build + full suite green** — `cargo build -j4 2>&1 | tail -3 && cargo test -j4 --lib 2>&1 | tail -5`. `cargo clippy -j4 --lib --tests 2>&1 | tail -20` clean; `cargo fmt`.

- [ ] **Step 4: Commit**
```bash
git add -A && git commit -m "refactor: remove [profile.password]/PasswordSpec/inject_password_fields (replaced by password widget)"
```

---

## Task 10: Migrate example configs + docs

**Files:** `examples/demo-config.toml`, `examples/config.toml`, `docs/src/configuration/widgets.md`.

- [ ] **Step 1: Migrate examples** — replace each:
```toml
[profile.password]
ldap_attribute = "userPassword"
samba          = true
```
with:
```toml
[profile.widget.userPassword]
kind  = "password"
samba = true
```
(In `examples/config.toml`, set `samba` to match whatever its profile used; if it had no samba, `samba = false` or omit.)

- [ ] **Step 2: Verify examples parse + resolve** — extend the existing `demo_config_widgets_resolve` test (or add one) to assert the demo config resolves a `WidgetKind::Password` for `userPassword`. Run `cargo test -j4 --lib config 2>&1 | tail -8`.

- [ ] **Step 3: Docs** — in `docs/src/configuration/widgets.md` add a `kind = "password"` section (samba flag, what it updates, the TLS requirement, Enter-on-any-password-field → popup) and remove any `[profile.password]` reference from the config docs.

- [ ] **Step 4: Commit**
```bash
git add -A && git commit -m "docs: migrate examples + reference to the password widget"
```

---

## Task 11: Live smoke (manual; needs an encrypted endpoint)

**Files:** none.

- [ ] **Step 1: Enable TLS on the test server OR point at it encrypted.** The bitnami OpenLDAP container can serve StartTLS/LDAPS. Either set `start_tls = true` in a copy of demo-config (if the server presents a cert the client trusts — may need `[server.tls]` config), or run a quick local `ldaps://`. If TLS can't be brought up quickly, VERIFY the negative path instead: with plain `ldap://`, Enter on the password field shows the "requires an encrypted connection" Error overlay (the positive path is covered by unit tests).

- [ ] **Step 2: Negative path (always doable):** launch against plain `ldap://` demo-config, navigate to a user, Enter on `userPassword`/`sambaNTPassword` → expect the encrypted-connection Error overlay (not silence, not a popup).

- [ ] **Step 3: Positive path (if TLS available):** Enter → popup; type New+Confirm; Alt+S → "staged"; Alt+S save → confirm preview shows `userPassword: ********`, `sambaNTPassword: ********`, `sambaPwdLastSet: <n>`; confirm → re-read shows the NT hash changed and `userPassword` updated. Restore the test user afterwards.

- [ ] **Step 4: Final** — `cargo test -j4 2>&1 | tail -5` green.

---

## Self-review notes (author)

- **Spec coverage:** config kind (T2), resolution/WidgetKind (T3), widget_binding+pending_password (T4), tagging primary+derived (T5), popup+TLS-gate (T6), save derivation+strip+TLS guard (T7), create flow + profile_for_entry repoint (T8), clean removal (T9), examples+docs (T10), TLS via is_encrypted (T1), live smoke incl. negative path (T11). Display "pending"/"↵ to set" marker — folded into T6 Step 5 render (primary field via `field_display_value`); if missed, add a one-line branch there.
- **Tasks 3+4 are one commit** (the enum rename forces the consumer change) — flagged in T3.
- **Type consistency:** `WidgetKind`, `PasswordWidget{primary,derived,samba}`, `widget_for -> Option<&WidgetKind>`, `password_widget_for`, `EditField.widget_binding`, `EditForm.pending_password`, `PasswordEditor`, `open_password_editor`/`password_editor_key`, `stage_pending_password`, `Config::is_encrypted`, `App.connection_encrypted` used consistently across tasks.
- **No-userbase / clean break:** T9 deletes outright; no alias.
