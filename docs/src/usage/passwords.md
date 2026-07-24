# Passwords & Samba

eDAPtor sets passwords two ways: a **set-password popup** in the create/edit
form (for entries whose profile declares a password widget), and the
**`edaptor passwd` CLI** for setting a password from the command line. The
configuration that enables the popup is described under
[Widgets](../configuration/widgets.md); this page covers using it.

## The set-password popup (in the TUI)
When an entry's [profile](../configuration/entry-profiles.md) declares a
`[profile.widget.<attr>]` table with `kind = "password"`, the create/edit form
shows a dedicated **masked, set-password popup** for that attribute (`userPassword`
by default). The schema-generated raw field for that attribute is suppressed, so
there is one clear place to set the password.

- Type the password once, then again to confirm — they must match.
- Each field shows only bullets while you type; a small **reveal eye** sits in
  its last column — hold <kbd>Space</kbd>, or press and hold the eye with the
  mouse, to peek at what you've typed.
- The cleartext is sent to the directory; the **LDIF preview masks it as
  `********`**, so the actual password never appears on screen.
- Like any other field, the password is applied as part of the form's normal
  save → LDIF-preview → apply flow.

### Samba-enabled passwords

If the profile sets `samba = true`, saving the password also keeps the entry's
Samba credentials in sync in the same atomic change:

- eDAPtor computes the **NT hash client-side** and writes `sambaNTPassword`
  alongside `userPassword`, and updates `sambaPwdLastSet`.
- This requires the entry to be a `sambaSamAccount`; the Samba SID is derived
  from the directory's `sambaDomain` entry.

The result is a single password change that updates both the Unix
(`userPassword`) and Samba (`sambaNTPassword`) credentials together.

Because they are maintained from the password, the derived fields
(`sambaNTPassword`, `sambaPwdLastSet`) are shown **read-only** in the form. While
empty they display *⟨updated automatically when you set the password⟩* rather than
looking like blank fields — their value is written on save and becomes visible
once the entry is re-read.

## The `edaptor passwd <user>` CLI

To set a password without entering the TUI, use the `passwd` subcommand. It
accepts either a **bare username** or a **full DN**:

```bash
edaptor passwd alice                                   # resolved via the configured profiles
edaptor passwd uid=alice,ou=people,dc=example,dc=org   # explicit DN
```

A bare username (anything without an `=`) is resolved to a DN by searching every
configured profile's `search_base` for `(<rdn_attr>=<username>)` — e.g.
`(uid=alice)` under `ou=people`. The lookup happens **before** the password
prompt, so an unknown or ambiguous username fails immediately instead of after
you type the password:

- **no match** → `no entry found for username "alice" …`;
- **more than one match** (e.g. the same name under two profiles) → the matching
  DNs are listed and you are asked to pass a full DN instead.

Once the target is resolved, edaptor prints which DN it is about to change, then
prompts for the new password twice (no echo). On a match it performs a single
atomic MODIFY that updates `userPassword` and — when the target is a
`sambaSamAccount` — `sambaNTPassword` and `sambaPwdLastSet`. This command is
**TLS-only** (it refuses to send a cleartext password over an unencrypted
connection), so the configured server must be reachable over `ldaps://`, with
StartTLS, or over `ldapi://`.

## Known gap: no standalone in-TUI "Set Password"

There is currently **no standalone "Set Password" action inside the TUI** for an
arbitrary entry. Passwords can be set in the TUI only through the **set-password popup**
on the create/edit form of entries whose profile declares a password widget.
For any other entry — or to (re)set a password outside that form — use the
**`edaptor passwd <user>`** CLI described above.

