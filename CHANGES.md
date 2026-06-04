# Changelog

All notable changes to eDAPtor are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/), and the project aims to follow
[Semantic Versioning](https://semver.org/).

## Unreleased

### New

### Changed

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
