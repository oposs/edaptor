# Passwords & Samba

edaptor sets passwords two ways: an **inline password field** in the create/edit
form (for entries whose profile declares a password), and the **`edaptor passwd`
CLI** for setting a password from the command line. The configuration that turns
on the inline field is described under
[Passwords](../configuration/passwords.md); this page covers using it.

## The inline password field (in the TUI)

When an entry's [profile](../configuration/entry-profiles.md) declares a
`[profile.password]` table, the create/edit form shows a dedicated **masked,
confirm-twice** password field for that attribute (`userPassword` by default).
The schema-generated raw field for that attribute is suppressed, so there is one
clear place to set the password.

- Type the password once, then again to confirm — they must match.
- The cleartext is sent to the directory; the **LDIF preview masks it as
  `********`**, so the actual password never appears on screen.
- Like any other field, the password is applied as part of the form's normal
  save → LDIF-preview → apply flow.

### Samba-enabled passwords

If the profile sets `samba = true`, saving the password also keeps the entry's
Samba credentials in sync in the same atomic change:

- edaptor computes the **NT hash client-side** and writes `sambaNTPassword`
  alongside `userPassword`, and updates `sambaPwdLastSet`.
- This requires the entry to be a `sambaSamAccount`; the Samba SID is derived
  from the directory's `sambaDomain` entry.

The result is a single password change that updates both the Unix
(`userPassword`) and Samba (`sambaNTPassword`) credentials together.

## The `edaptor passwd <dn>` CLI

To set a password without entering the TUI, use the `passwd` subcommand:

```bash
edaptor passwd uid=alice,ou=people,dc=example,dc=org
```

It prompts for the new password twice (no echo), and on a match performs a
single atomic MODIFY that updates `userPassword` and — when the target is a
`sambaSamAccount` — `sambaNTPassword` and `sambaPwdLastSet`. This command is
**TLS-only** (it refuses to send a cleartext password over an unencrypted
connection), so the configured server must be reachable over `ldaps://` or with
StartTLS.

## Known gap: no standalone in-TUI "Set Password"

There is currently **no standalone "Set Password" action inside the TUI** for an
arbitrary entry. Passwords can be set in the TUI only through the **inline field**
on the create/edit form of entries whose profile declares `[profile.password]`.
For any other entry — or to (re)set a password outside that form — use the
**`edaptor passwd <dn>`** CLI described above.

