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

## License

To be determined.
