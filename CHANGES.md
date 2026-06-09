# Changelog

All notable changes to eDAPtor are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/), and the project aims to follow
[Semantic Versioning](https://semver.org/).

## Unreleased

### New

- `examples/oposs-openldap.toml` — ready-to-use config template for directories
  managed by the [oposs.openldap](https://github.com/oposs/oposs.openldap)
  Ansible role (POSIX users, groupOfNames + posixGroup, Samba and mailAccount
  optional blocks).
- **`auth.method = "external"`** — SASL EXTERNAL bind over a Unix domain socket
  (`ldapi:///`). Allows password-free root access when running on the slapd
  host; no TLS required. `ldapi://` connections are also treated as secure for
  password-change operations.

### Changed

### Fixed

## 0.3.0 - 2026-06-09

### New

- **Configurable widget palette** via `[profile.widget.<attr>]` — bind an
  attribute to a richer editor than a plain text box:
  - `kind = "choice"` — pick from a fixed vocabulary and (de)serialize a single
    value. Shipped for `sambaAcctFlags` (multi-select account flags, losslessly
    preserving flags the UI does not surface) and `loginShell` (single-select
    from a configured shell list). Read-only fields show a human-readable summary;
    Enter opens a checklist/radio popup.
  - `kind = "password"` — turns the attribute into a masked **set-password
    field**. Enter on it (or on any derived Samba password attribute) opens a
    New+Confirm popup that updates `userPassword` and, when `samba = true`,
    `sambaNTPassword` + `sambaPwdLastSet` in one atomic change.
  - `kind = "picker"` — populate an attribute from a live candidate search and
    store the picked value(s) in this entry (value lookup like `gidNumber`, or a
    DN/scalar list like `member`/`memberUid`). `candidate` is a `[[profile]]`
    name or an inline `{ base, object_classes, … }` scope.
  - `kind = "membership"` — fan this entry's DN into a back-reference attribute
    (`via`) on each picked candidate (e.g. `memberOf` writes `member` on each
    chosen group).
### Changed

- **Pickers are now configured with `[profile.widget.<attr>] kind = "picker"` /
  `"membership"`** instead of `[profile.picker.<attr>]`, which has been removed.
- **Passwords are now configured with `[profile.widget.<attr>] kind = "password"`**
  instead of `[profile.password]`, which has been removed.
- **Password and hash attributes (`userPassword`, `sambaNTPassword`,
  `sambaLMPassword`) are read-only inline** and changed only through the
  set-password popup — preventing a typed cleartext password from being written
  verbatim into a hash attribute.
- **Password changes require an encrypted connection** (`ldaps://` or
  `start_tls = true`); the popup refuses to open on a plaintext connection.

### Fixed

- Password-profile entries no longer appear permanently "dirty" (which popped a
  spurious Save/Discard/Stay guard on every navigation between entries).
- Secret values (passwords, NT hashes) are no longer shown in clear in the change
  (LDIF) preview — they are masked as `********` regardless of how they enter the
  change.

## 0.2.0 - 2026-06-08

### New

- Configurable, presence-keyed, width-aware DIT tree labels via `[[tree.label]]`
  rules. The structural RDN is now always shown by default (e.g.
  `ou=people (People)`), and narrow panes degrade gracefully while keeping the RDN.
### Changed

- DIT tree branch labels now always include the RDN by default (previously a
  node with a `description` showed only the description).

### Fixed

## 0.1.0 - 2026-06-04

### New

- Schema-driven TUI for administering an OpenLDAP directory: browse, create,
  edit, rename, and delete users and groups with forms generated from live
  `objectClass` definitions (`cn=subschema`).
- TOML configuration with entry profiles, defaults (literal / template /
  `{next:MIN-MAX}` auto-number), inline passwords, the full Samba lifecycle, and
  unified `[profile.picker.<attr>]` candidate pickers.
- Three-pane ratatui interface with symmetric membership editing and on-demand
  LDIF preview of the exact change before it is applied.
- rustls TLS backend (custom CA, optional StartTLS, connect timeout).
- Provisioned podman test server (`scripts/test-ldap.sh`) and `edaptor passwd <dn>` CLI.
### Changed

### Fixed
