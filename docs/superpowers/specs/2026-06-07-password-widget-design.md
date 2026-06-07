# Password Widget — `kind = "password"` + Set-Password Popup

**Date:** 2026-06-07
**Status:** approved (pending written-spec review)
**Area:** `eDAPtor` — the configurable-field widget palette + the edit form's password handling.

## Problem

After making password/hash attributes read-only inline (commit `e961892`), pressing a key on `sambaNTPassword`/`userPassword` does **nothing** — no editor, no error — which is confusing. And the only way to change a password was the injected inline `userPassword` + confirm fields driven by `[profile.password]`, a one-off mechanism separate from the new `[profile.widget.<attr>]` palette.

We want passwords to be a **configurable-field widget** like everything else: a `kind = "password"` widget whose editor is a **popup** (New + Confirm) that updates *all* the password representations (`userPassword` + `sambaNTPassword` + `sambaPwdLastSet`) in one save, opened by Enter on any of those fields.

## Goals

1. A `kind = "password"` widget under `[profile.widget.<attr>]`, sibling of `kind = "choice"`.
2. Enter on the primary or any derived password field opens **one** set-password popup; it states which attributes it will update.
3. Commit derives all values via the existing, proven `samba::password` logic and folds them into the normal save as one MODIFY.
4. **Single mechanism:** delete `[profile.password]` / `PasswordSpec` and the injected inline fields; migrate the example configs.
5. **Encrypted-only:** refuse a password change unless the connection is TLS (LDAPS or StartTLS) — `userPassword` is sent in clear for the server to hash.

## Non-goals

- No change to the cryptography: `userPassword` is still sent cleartext over the (now-required) encrypted connection for the server to hash per its policy; `sambaNTPassword = nt_hash(pw)`; `sambaPwdLastSet = now`. `samba::password` / `samba::nthash` are reused unchanged.
- `sambaLMPassword` remains legacy/dead — never written, not part of the managed set (stays read-only display).
- No password *strength* policy / generation in this change.
- No new widget kinds beyond `password` (the `choice` widget is unchanged except the shared resolution refactor in §3).

## Locked decisions

| Decision | Value |
|---|---|
| Config | `[profile.widget.<attr>]` with `kind = "password"`, `samba = <bool>` (default false) |
| Bound attr | the table key `<attr>` = the primary cleartext attribute (e.g. `userPassword`) |
| Derived attrs | `samba = true` ⇒ `["sambaNTPassword", "sambaPwdLastSet"]`; else none |
| `[profile.password]` | **removed outright** (PasswordSpec, `EntryProfile.password`, `password_field_labels`, `inject_password_fields` all deleted); examples migrated. No userbase yet → **no deprecation, alias, or backward-compat shim** — just delete and replace |
| Trigger | Enter on the primary field OR any derived field opens the popup |
| Editor | new `Overlay::PasswordEditor` — masked New + Confirm rows + "Updates: …" note |
| Staging | `EditForm.pending_password: Option<String>` (the primary/derived fields are read-only, so the new value can NOT live in a field editor) |
| Dirty | `is_dirty()` is also true when `pending_password.is_some()` |
| Save | derive mods from `pending_password` via `password_add_attrs`; strip primary + derived attrs from the normal diff; mask them in the preview (already masked via `is_secret_attr`) |
| TLS | **hard refuse** on a non-encrypted connection — popup will not open; an Error overlay explains why. Also guarded at save (defence in depth) |
| Encrypted test | `ServerConfig::is_encrypted()` = `start_tls || uri (lowercased) starts_with "ldaps://"`, cached on `App` |

## Config schema

```toml
[profile.widget.userPassword]
kind  = "password"
samba = true
```

Serde: extend the existing tagged enum in `src/config/mod.rs`:

```rust
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum WidgetSpecCfg {
    Choice { select: String, format: String, options: Vec<ChoiceOption> },
    Password {
        #[serde(default)]
        samba: bool,
    },
}
```

`PasswordSpec` and `EntryProfile.password` are removed.

## Resolution (§3 — shared refactor)

`ResolvedWidget` currently carries `widget: ChoiceWidget`. Generalise to a kind enum so the palette holds multiple kinds:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WidgetKind {
    Choice(ChoiceWidget),
    Password(PasswordWidget),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordWidget {
    pub primary: String,        // the cleartext attr (table key)
    pub derived: Vec<String>,   // [sambaNTPassword, sambaPwdLastSet] when samba
    pub samba: bool,
}

pub struct ResolvedWidget {
    pub owner_object_classes: Vec<String>,
    pub attr: String,           // primary attr (the bound field)
    pub kind: WidgetKind,
}
```

`resolve_widgets` builds a `WidgetKind::Password { primary: attr, derived, samba }` for a password entry. Existing choice call sites match `WidgetKind::Choice(_)`.

`EditField.widget_choice: Option<ChoiceWidget>` is renamed to **`EditField.widget_binding: Option<WidgetKind>`** — NOT `widget`, which already exists on `EditField` as the read-only display widget (`WidgetSpec`: checkbox / binary note). The inline-edit guard and the `field_display_value` summary match on `widget_binding`'s arm.

## Tagging + trigger

`tag_widget_fields` matches each form field against the resolved widgets for the entry's object classes:
- `Choice` → tag the field whose label equals the widget's `attr` (as today).
- `Password` → tag the field whose label equals `primary` **or** is in `derived`, all with the same `WidgetKind::Password` (so Enter on any opens the same popup). All remain read-only inline (secret).

(For `Choice`, `widget_for(widgets, ocs, attr) -> Option<&WidgetKind>` keeps the single-attr lookup; `Password` matching spans `primary`+`derived`, so the password branch matches directly inside `tag_widget_fields` rather than via the single-attr `widget_for`.)

`open_value_editor` (Enter): if the focused field's `widget` is `Password`:
1. If `!app.connection_encrypted` → set `Overlay::Error("Changing a password requires an encrypted connection (ldaps:// or start_tls).")` and return (this replaces the silent no-reaction).
2. Else open `Overlay::PasswordEditor` seeded empty, carrying the affected attr list (`primary` + `derived`) for the note.

## The popup — `Overlay::PasswordEditor`

```rust
pub struct PasswordEditor {
    pub new: TextState<'static>,
    pub confirm: TextState<'static>,
    pub focus: PwField,          // New | Confirm
    pub affected: Vec<String>,   // for the "Updates: …" note
}
```
Render (centered overlay): masked `New password:` and `Confirm:` rows (bullets), a dim `Updates: userPassword, sambaNTPassword, sambaPwdLastSet` line, hint `Alt+S set · Alt+C cancel`. Tab/↑↓ switch rows; chars edit the focused row.

Keys:
- **Alt+S**: if `new` empty → treat as cancel (no change). If `new != confirm` → keep the popup open and show an inline `passwords do not match` message (no commit). Else set `app.form.pending_password = Some(new)`, close the popup. Status: "Password staged — Alt+S to save."
- **Esc / Alt+C**: discard, close.

## Staging, dirty, save

- `EditForm.pending_password: Option<String>` — set by the popup, displayed as a "pending" marker on the primary field, cleared by revert (Alt+C on the form) and by a successful save.
- `is_dirty()` → `… || self.pending_password.is_some()`.
- Save path (`prepare_edit_save`): replace the old `stage_edit_password` pseudo-field logic with: if `form.pending_password` is `Some(pw)` and the entry has a password widget, compute `mods = password_add_attrs(pw, primary, samba, now)` and **remove** `primary` + `derived` from both `original` and `edited` before `diff` (so they never double-diff), then fold `mods` into the changeset and add them to `mask_attrs`. If `pending_password` is `None`, no password mods.
- `password_add_attrs`, `nt_hash`, `samba_pwd_last_set` unchanged.
- **TLS guard (defence in depth):** `prepare_edit_save` returns an error if `pending_password.is_some()` and the connection is not encrypted.

## Create flow

The create form (`build_new_entry_form` / `open_create_form` / `prepare_create`) currently sets a new entry's password via the same injected fields + `stage_password`. With those removed, create uses the SAME popup: `tag_widget_fields` already runs on the create form, so the password field is tagged and Enter opens the `PasswordEditor` (TLS-gated). The staged `pending_password` is folded into the **Add** at confirm time — `prepare_create` derives `password_add_attrs(pw, primary, samba, now)` and inserts them into the new entry's attribute set (the primary/derived attrs are otherwise absent on a new entry, so no stripping needed). A create with no staged password simply omits them. `profile_for_entry` (which today keys off `password.is_some()`) is repointed to "the entry's object classes match a profile that has a password widget."

## Display

Primary + derived password fields render masked, read-only. The primary shows `•••• (↵ to set)` normally and `•••• (pending)` when `pending_password.is_some()`. (Reuse the masked render; add the parenthetical.)

## Removed / migrated

- Delete: `PasswordSpec`, `EntryProfile.password`, `inject_password_fields`, `password_field_labels`, and `stage_edit_password`/`stage_password`'s pseudo-field handling (replaced by the `pending_password` stage).
- `examples/demo-config.toml`, `examples/config.toml`: `[profile.password]` → `[profile.widget.userPassword] kind="password" samba=…`.
- Docs: `docs/src/configuration/widgets.md` gains a `kind="password"` section; remove `[profile.password]` from the reference.

## Testing

- **config** (`config::widget`): `kind="password"` resolves to `PasswordWidget{primary, derived, samba}`; `samba=false` → empty derived; demo config resolves.
- **dirty/stage** (`edit_form` / `workflows::save`): `pending_password=Some` makes the form dirty; the save derives `userPassword` + (samba) `sambaNTPassword` + `sambaPwdLastSet` mods and strips them from the plain diff; `None` → no password mods; the preview masks them.
- **popup** (`value_editor`): Enter on a password field with an encrypted connection opens `PasswordEditor`; with a non-encrypted connection opens the Error overlay (no popup); mismatched new/confirm does not commit; matching sets `pending_password` and closes.
- **TLS guard**: `prepare_edit_save` errors when `pending_password.is_some()` and not encrypted.
- **`is_encrypted()`** unit: `ldaps://` true; `start_tls=true` true; plain `ldap://` false.
- **live smoke**: requires an encrypted test endpoint — enable StartTLS/LDAPS on the podman server (or point demo-config at `ldaps://`/`start_tls=true`); then set a password via the popup and confirm `userPassword` + `sambaNTPassword` + `sambaPwdLastSet` all update; confirm a plain-`ldap://` config shows the encrypted-connection error.

## Future extension points (named, not built)

- Password generation / strength meter in the popup.
- A `kind="password"` `also = [...]` explicit derived-attr list (instead of the `samba` bool) if non-samba derived attrs ever appear.
- `sambaLMPassword` purge action (it is dead; could offer to delete it).
