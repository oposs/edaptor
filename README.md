# eDAPtor

*the tui LDAP editor* — a terminal UI for administering an OpenLDAP directory —
adding, modifying and removing **users** and **groups**, and managing **group
memberships** — built in Rust with [ratatui](https://ratatui.rs/).

> The name is **e**ditor and L**DAP**, creatively merged.

📖 **Documentation:** <https://oposs.github.io/edaptor>

## What makes it different

edaptor **derives the directory's structure from the LDAP server itself** via
full schema introspection (`cn=subschema`) and generates its edit forms
dynamically from `objectClass` definitions. A config file holds all connection
properties plus a small set of *entry profiles* describing what a "user" and a
"group" mean in your directory.

It is designed against the
[`oposs.openldap`](https://github.com/oposs) server configuration (OUs
`people`/`groups`/…, `groupOfNames` membership, the memberOf / refint / ppolicy
overlays, and the Samba schema).

## Status

**Working.** The core milestones are implemented on a three-pane ratatui
interface: schema-driven create/edit/rename/delete, defaults and auto-numbering,
inline passwords with the Samba lifecycle, unified candidate pickers, and
symmetric membership editing. See the [documentation](https://oposs.github.io/edaptor)
for usage, and `docs/superpowers/specs/` for the design specifications.

## Highlights of the design

- **Two-tier object model:** a generic schema-driven entry engine, with a
  pervasive *users & groups* understanding layered over it (passwords,
  memberships, Samba) that acts naturally across view/create/edit/delete/rename.
- **Responsive:** all LDAP I/O runs on a background worker thread; the UI never
  freezes on the network.
- **Safe & transparent:** immediate apply with an on-demand LDIF preview of the
  exact change.
- **Human-friendly:** `cn`-based labels everywhere (raw DNs only on demand);
  symmetric membership editing with incremental search on both panes.
- **Full Samba lifecycle:** client-side NT-hash, synced Unix+Samba passwords,
  SID discovered from the directory's `sambaDomain` entry.

## Local test server

`scripts/test-ldap.sh start` launches a podman OpenLDAP that mirrors the
`oposs.openldap` role — Samba + mail schemas, the memberOf/refint/ppolicy
overlays, password policies — and seeds it with ~600 users across 5 departments
and ~25 groups (see `scripts/ldap-provision/`). Point edaptor at it with:

```bash
scripts/test-ldap.sh start
export EDAPTOR_TEST_ADMIN_PW=adminpassword
edaptor --config examples/demo-config.toml
```

All generated users share the password `test123`.

## Configuration

A single TOML file (`--config <path>`, default `~/.config/edaptor/config.toml`).

```toml
[server]
uri          = "ldaps://ldap.example.com"
base_dn      = "dc=example,dc=com"
start_tls    = false
timeout_secs = 10

[auth]
method          = "simple"
bind_dn         = "cn=ldapmanager,dc=example,dc=com"
# Password is NEVER stored here. Supported sources: "prompt", "env:VAR", "command:cmd"
password_source = "prompt"

# Entry profiles: what a "user", "group", and "posixgroup" mean in this directory.
# `search_attrs` sets which attributes the picker substring-search matches.
# Falls back to `show`, then to `["cn"]` when omitted.
#
# This "user" is a full posix (+optional Samba) account template: multiple
# object classes, defaulted/templated/auto-numbered fields, an inline password
# field, and picker bindings that pull values from or fan out to other profiles.
[[profile]]
name           = "user"
object_classes = ["inetOrgPerson", "posixAccount", "shadowAccount"]
rdn_attr       = "uid"
search_base    = "ou=people,dc=example,dc=com"
show           = ["uid", "cn", "sn", "givenName", "mail", "uidNumber", "gidNumber", "homeDirectory"]
search_attrs   = ["cn", "uid", "mail"]   # picker searches these attributes
# How an entry of this profile is labelled in the membership picker. `{attr}` is
# substituted by that attribute's value; literal text is kept. Defaults to cn.
label          = "{cn} ({uid})"          # e.g. "Bob Baker (bob)"

# Defaults fill EMPTY fields on create (operator-entered values are never
# overwritten). Three value kinds:
#   literal             -> a fixed string
#   "/home/{uid}"       -> template; {attr} is substituted from another field
#   "{next:MIN-MAX}"    -> auto-number; the next free value in [MIN,MAX] across
#                          the whole directory (refuses if the scan is truncated
#                          by a server size limit — bind with a high-limit identity)
[profile.defaults]
loginShell    = "/bin/bash"
homeDirectory = "/home/{uid}"
uidNumber     = "{next:10000-60000}"

# Inline password field: the create/edit form shows a masked, confirm-twice
# field for `ldap_attribute` (the schema-generated field is suppressed). The
# cleartext goes to the directory; the LDIF preview shows `********`.
#   samba = true  -> also write sambaNTPassword/sambaPwdLastSet (needs sambaSamAccount).
[profile.password]
ldap_attribute = "userPassword"   # default; omit to use userPassword
samba          = false

# Picker bindings: `[profile.picker.<attr>]` declares how an attribute's field
# is populated from a live candidate search. The four configuration knobs are:
#
#   candidate   (required) — a [[profile]] `name` supplying the candidate search scope.
#   store       (default "dn") — what to write per pick: "dn" stores the candidate's DN;
#                 any other value is treated as an attribute name whose scalar is stored.
#   select      (default "auto") — cardinality: "auto" derives from the attribute's schema
#                 arity; "single" or "multi" override it.
#   fanout_attr (optional) — when set, the field is NOT written to the server; instead,
#                 this entry's DN is added/removed in `fanout_attr` on each picked candidate
#                 (e.g. a user's `memberOf` fan-out writes `member` on each picked group).

# gidNumber: single-select picker over posixGroups; stores the chosen group's gidNumber
# scalar into the field (not its DN).
[profile.picker.gidNumber]
candidate = "posixgroup"
store     = "gidNumber"
select    = "single"

# memberOf: synthetic back-ref — ticking a group writes `member` on it.
# The memberOf attribute itself is overlay-maintained; edaptor never writes it directly.
[profile.picker.memberOf]
candidate   = "group"
store       = "dn"
fanout_attr = "member"

[[profile]]
name           = "group"
object_classes = ["groupOfNames"]
rdn_attr       = "cn"
search_base    = "ou=groups,dc=example,dc=com"
show           = ["cn", "description"]
label          = "{cn}"

# member: multi-select DN picker over users (cardinality from schema, typically multi).
[profile.picker.member]
candidate = "user"

[[profile]]
name           = "posixgroup"
object_classes = ["posixGroup"]
rdn_attr       = "cn"
search_base    = "ou=groups,dc=example,dc=com"
show           = ["cn", "gidNumber", "memberUid", "description"]
label          = "{cn}"

# memberUid: multi-select picker; stores each picked user's `uid` scalar (not DN).
[profile.picker.memberUid]
candidate = "user"
store     = "uid"
```

## License

[MIT](LICENSE) © Tobias Oetiker
