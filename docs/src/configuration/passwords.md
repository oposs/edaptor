# Passwords

Password handling is configured via the `[profile.widget.<attr>]` table with
`kind = "password"`. This turns the named attribute into a **masked,
set-password field** in the create/edit form.

```toml
[profile.widget.userPassword]
kind  = "password"
samba = false
```

- **Table key** (`userPassword` above) — the directory attribute the password
  is written to. Use whatever LDAP attribute holds the cleartext password in your
  directory (typically `userPassword`).
- **`samba`** — when `true`, also maintain the Samba password attributes (see
  below).

For the full list of options and a worked example see [Widgets](widgets.md#the-password-kind).

## The inline password field

When a profile has a `[profile.widget.<attr>]` table with `kind = "password"`,
the create/edit form shows a **set-password popup** for that attribute (and any
derived Samba attributes). Press Enter on the field to open the popup; type the
password twice to confirm. The schema-generated raw field for that attribute is
**suppressed**, so there is one clear place to set the password.

On save, the **cleartext password goes to the directory** (OpenLDAP hashes it
per its password policy / `pwdPolicy` configuration). The
[LDIF preview](../concepts/change-flow.md) of the change shows `********` in
place of the value, so the actual password is never displayed.

Password changes require an encrypted connection (`ldaps://` or
`start_tls = true`).

## The Samba lifecycle (`samba = true`)

Setting `samba = true` keeps a Samba (NT) password in sync with the Unix
password. On save, in addition to the primary attribute, eDAPtor writes:

- **`sambaNTPassword`** — the NT hash of the password, computed **client-side**.
- **`sambaPwdLastSet`** — the timestamp of the change.

Requirements and details:

- The entry must carry the **`sambaSamAccount`** object class (add it to the
  profile's [`object_classes`](entry-profiles.md)) for these attributes to be
  valid.
- The account **SID** is derived from the directory's **`sambaDomain`** entry.
- Because the NT hash is computed on the client, eDAPtor can write both the Unix
  and Samba passwords from a single masked field — they stay synchronized.

## The `edaptor passwd` CLI

To set a password outside the TUI, use the command-line subcommand:

```bash
edaptor passwd <dn>
```

This prompts for a new password and applies it to the entry at `<dn>`, honouring
the same Samba behaviour when configured.
