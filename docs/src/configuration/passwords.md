# Passwords

The optional `[profile.password]` table turns on an **inline password field**
for a profile's create and edit forms.

```toml
[profile.password]
ldap_attribute = "userPassword"               # default; omit to use userPassword
samba          = false
```

- **`ldap_attribute`** — the directory attribute the password is written to.
  Defaults to `userPassword`; omit the key to use that default.
- **`samba`** — when `true`, also maintain the Samba password attributes (see
  below).

## The inline password field

When a profile has a `[profile.password]` table, the create/edit form shows a
**masked, confirm-twice field** for `ldap_attribute` (you type the password
twice and it is rendered as dots, not echoed). The schema-generated field for
that same attribute is **suppressed**, so you do not see a raw `userPassword`
input alongside the masked one.

On save, the **cleartext password goes to the directory** (OpenLDAP hashes it
per its password policy / `pwdPolicy` configuration). The
[LDIF preview](../concepts/change-flow.md) of the change shows `********` in
place of the value, so the actual password is never displayed.

## The Samba lifecycle (`samba = true`)

Setting `samba = true` keeps a Samba (NT) password in sync with the Unix
password. On save, in addition to `ldap_attribute`, edaptor writes:

- **`sambaNTPassword`** — the NT hash of the password, computed **client-side**.
- **`sambaPwdLastSet`** — the timestamp of the change.

Requirements and details:

- The entry must carry the **`sambaSamAccount`** object class (add it to the
  profile's [`object_classes`](entry-profiles.md)) for these attributes to be
  valid.
- The account **SID** is derived from the directory's **`sambaDomain`** entry.
- Because the NT hash is computed on the client, edaptor can write both the Unix
  and Samba passwords from a single masked field — they stay synchronized.

## The `edaptor passwd` CLI

To set a password outside the TUI, use the command-line subcommand:

```bash
edaptor passwd <dn>
```

This prompts for a new password and applies it to the entry at `<dn>`, honouring
the same Samba behaviour when configured.
