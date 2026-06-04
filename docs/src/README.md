# eDAPtor

*the tui LDAP editor*

eDAPtor is a terminal UI for administering an OpenLDAP directory — adding,
modifying and removing **users** and **groups**, and managing **group
memberships** — built in Rust with [ratatui](https://ratatui.rs/).

> The name is **e**ditor and L**DAP**, creatively merged.

## What makes it different

edaptor **derives the directory's structure from the LDAP server itself.** It
introspects the live schema via `cn=subschema` and generates its edit forms
dynamically from the relevant `objectClass` definitions, so the fields it shows
always match what the server will actually accept. A single TOML config holds
all connection properties plus a small set of *entry profiles* that describe
what a "user" and a "group" mean in your particular directory — which object
classes, RDN attribute, search base, and labels to use.

## Highlights

- **Two-tier object model:** a generic schema-driven entry engine, with a
  pervasive *users & groups* understanding layered over it (passwords,
  memberships, Samba) that acts naturally across view, create, edit, delete and
  rename.
- **Responsive:** all LDAP I/O runs on a background worker thread, so the UI
  never freezes while waiting on the network.
- **Safe and transparent:** changes apply immediately, with an on-demand LDIF
  preview of the exact modification before you commit.
- **Human-friendly:** `cn`-based labels everywhere (raw DNs only on demand), and
  symmetric membership editing with incremental search on both sides of a
  relationship.
- **Full Samba lifecycle:** client-side NT-hash, synced Unix and Samba
  passwords, with the SID discovered from the directory's `sambaDomain` entry.

## Where to go next

- [Installation](getting-started/installation.md) — build from source and where
  the binary lands.
- [Quick Start](getting-started/quick-start.md) — spin up the bundled test
  server and explore in minutes.
- [Configuration](configuration/overview.md) — the TOML config file, connection
  settings, and entry profiles in full.
