# edaptor

A terminal UI (TUI) for administering an OpenLDAP directory — adding, modifying
and removing **users** and **groups**, and managing **group memberships** — built
in Rust on a [Turbo Vision](https://crates.io/crates/turbo-vision) port.

> **eDAPtor** — the *DAP* (Directory Access Protocol, the P in LDAP) baked into
> an *editor* / *adaptor*.

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

🚧 **Early development.** The design is complete; implementation is being planned
and executed in milestones.

- 📄 Design specification: [`docs/superpowers/specs/2026-05-29-edaptor-design.md`](docs/superpowers/specs/2026-05-29-edaptor-design.md)

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

# Entry profiles: what a "user" and a "group" mean in this directory.
# `search_attrs` sets which attributes the picker substring-search matches.
# Falls back to `show`, then to `["cn"]` when omitted.
[[profile]]
name           = "user"
object_classes = ["inetOrgPerson"]   # a list; add posixAccount/shadowAccount for posix users
rdn_attr       = "uid"
search_base    = "ou=people,dc=example,dc=com"
show           = ["uid", "cn", "sn", "givenName", "mail"]
search_attrs   = ["cn", "uid", "mail"]   # picker searches these attributes

[[profile]]
name           = "group"
object_classes = ["groupOfNames"]
rdn_attr       = "cn"
search_base    = "ou=groups,dc=example,dc=com"
show           = ["cn", "description"]

# Membership relation: enables the symmetric group↔user membership picker.
# Opening a group's `member` field shows a live searchable user picker;
# opening a user's `memberOf` field fans out the changes to each affected group.
# `holder` and `candidate` reference [[profile]] `name`s above.
[[relation]]
name        = "group-membership"
holder      = "group"       # profile whose entry owns the link attribute
holder_attr = "member"      # the writable attribute on the holder (e.g. groupOfNames.member)
candidate   = "user"        # profile that scopes the picker's candidate search
back_attr   = "memberOf"    # virtual back-reference field shown on the candidate side
```

## License

To be determined.
