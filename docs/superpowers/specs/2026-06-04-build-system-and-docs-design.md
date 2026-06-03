# edaptor — Build System & Documentation Design

**Date:** 2026-06-04
**Status:** approved (pending written-spec review)
**Reference for inspiration:** the `byonk` project (`../byonk`) — its `Makefile`,
`mise.toml`, mdBook docs, and three GitHub Actions workflows (CI, versioned docs
on GitHub Pages, cross-compiled releases).

## Decisions locked

| Decision | Value |
|---|---|
| Project name | `edaptor` (unchanged — package, binary, repo `oposs/edaptor`, docs `/edaptor/`) |
| License | **MIT** (`LICENSE` file + `license = "MIT"` in `Cargo.toml`) |
| Docs content depth | Full handwritten prose |
| Docs centerpiece | **Configuration reference + examples** (highest priority per user) |
| TUI illustrations | Fenced ` ``` ` ASCII blocks — **no captured screenshots** |
| Docs hosting | Versioned GitHub Pages at `https://oposs.github.io/edaptor/` |
| Release container | **Dropped** (edaptor is a TUI client, not a server) |
| TLS backend | **Migrate to rustls** (`tls-rustls-ring`), drop `native-tls`/OpenSSL |

## Goal

Give `edaptor` the same proven build/docs infrastructure as `byonk`, adapted
for a TUI client tool: a `Makefile` + `mise.toml` developer workflow, a full
mdBook documentation site whose centerpiece is the configuration reference, and
three CI workflows (check/lint/test, versioned docs deploy, cross-compiled
release binaries).

## Non-goals

- No multi-arch container image (byonk ships one because it is a server; edaptor
  is a client TUI). The release workflow stops at archived binaries.
- No automated screenshot/sample-image generation pipeline. TUI layouts are
  hand-written as fenced code blocks.
- No behavioural change to TLS: the rustls migration must preserve the exact
  current semantics (custom CA, `verify=false`, StartTLS, connect timeout). It is
  a backend swap, not a feature change.

## Components

### 1. `Makefile`

Ported from byonk, adapted:

- `all` → `release`
- `release` / `debug` / `build` — run `fmt` + `lint` first, then `cargo build [--release]`
- `run` — `cargo run -- --config examples/demo-config.toml`
- `watch` — `cargo watch -x run` (cargo-watch)
- `fmt` — `cargo fmt`
- `lint` — `cargo clippy -- -D warnings`
- `test` — `cargo test`
- `check` — `fmt lint test`
- `coverage` / `coverage-ci` / `coverage-text` — `cargo llvm-cov` (Homebrew-LLVM
  path shim kept from byonk)
- `docs` / `docs-dev` — `cd docs && mdbook build|serve`
- `clean` — `cargo clean` + `rm -rf docs/book`
- `help` — printed target summary

Dropped from byonk: `docs-samples` (no screenshot pipeline), `run-release`
server invocation semantics (replaced by the TUI `run` above).

### 2. `mise.toml`

```toml
[tools]
rust = "latest"
"ubi:rust-lang/mdBook"        = "0.5.2"
"ubi:badboy/mdbook-mermaid"   = "0.17.0"
```

Pins the same mdBook/mermaid versions CI uses, so `make docs` works locally with
no manual install (an improvement over byonk's rust-only file).

### 3. mdBook documentation (`docs/book.toml`, `docs/src/**`)

`book.toml` mirrors byonk: mermaid preprocessor, version-selector JS/CSS,
`default-theme = "light"`, `git-repository-url`/`edit-url-template` →
`oposs/edaptor`, `site-url = "/edaptor/"`.

`SUMMARY.md` structure (config reference is the largest section):

```
[Introduction](README.md)

# Getting Started
- Installation
- Quick Start (podman test server)

# Configuration            <-- centerpiece
- Overview
- Server & Authentication  ([server], [auth], password_source, [samba])
- Entry Profiles           (object_classes, rdn_attr, search_base, show, search_attrs, label)
- Defaults                 (literal / "{template}" / "{next:MIN-MAX}")
- Passwords                ([profile.password], Samba lifecycle)
- Pickers                  ([profile.picker.<attr>]: candidate/store/select/fanout_attr)
- Full Example             (annotated examples/config.toml walkthrough)

# Concepts
- Architecture             (schema-driven forms, background LDAP worker)
- Object Model             (two-tier: generic entry engine + users/groups understanding)
- LDAP Constraints         (no has-children flag, size limits, RFC 4533, overlay-maintained memberOf)
- Change Flow              (diff -> ChangeSet -> LDIF preview -> Modify/Add/ModRdn/Delete)

# Usage
- The Three-Pane TUI
- Creating, Editing, Renaming, Deleting
- Membership Editing       (symmetric member <-> memberOf)
- Passwords & Samba

# Reference
- Test Server
```

Content is derived from `README.md`, `docs/HANDOVER.md`, and the design specs in
`docs/superpowers/specs/`. The Introduction corrects the **stale** README wording
(the "Turbo Vision" reference and "design complete, implementation being planned"
status — the UI was migrated to ratatui 0.30 and the milestones are largely done).
Concepts pages use mermaid diagrams. TUI layouts are fenced ` ``` ` blocks.

### 4. `examples/`

- `demo-config.toml` — **already exists**; points at the podman test server
  (`ldap://localhost:1389`, `dc=example,dc=org`, `env:EDAPTOR_TEST_ADMIN_PW`).
  Used by `make run` and the Quick Start docs.
- `config.toml` — **new**: a fully annotated reference config exercising every
  profile/default/password/picker option, mirrored by the docs "Full Example"
  page. This is the copy-pasteable starting point for real deployments.

### 5. `.github/workflows/ci.yml`

Three cached jobs (byonk shape): **check & lint** (`cargo fmt --check`,
`cargo clippy -- -D warnings`), **test** (`cargo test`), **build**
(`cargo build`). Safe without an LDAP server: every `tests/live_*.rs` and
`integration.rs` gates on `EDAPTOR_TEST_LDAP_URI` and prints `SKIP` + returns
early when it is unset.

### 6. `.github/workflows/docs.yml` + versioning machinery

Ported from byonk with `byonk` → `edaptor` and the Pages URL set to
`https://oposs.github.io/edaptor`:

- `.github/scripts/manage-doc-versions.sh` — `cull` / `update-json` /
  `generate-redirect` (keeps last 4 minor versions, builds `versions.json`, emits
  the root redirect). Hardcoded `/byonk/` paths rewritten to `/edaptor/`.
- `docs/theme/version-selector.{js,css}` — the dropdown; regex
  `/edaptor/(v[\d.]+|dev)/`.
- Workflow builds the `dev` docs on push to `main` (paths: `docs/**`, `src/**`,
  the workflow, the script), installs mdBook + mdbook-mermaid via the pinned
  release tarballs, runs `mdbook-mermaid install docs` + `mdbook build`, then
  deploys into `site/edaptor/dev`. **No sample-generation step** (no screenshots).

### 7. `.github/workflows/release.yml`

`workflow_dispatch` with `release_type` (bugfix/feature/major). Ported from byonk
**minus the container job**:

1. **version** — compute next semver from the latest `v*` tag, bump
   `Cargo.toml`, roll `CHANGES.md` `Unreleased` into a dated section, commit + tag.
2. **build-binaries** — matrix: `x86_64`/`aarch64-unknown-linux-musl` (via
   `cross`), `x86_64`/`aarch64-apple-darwin`, `x86_64-pc-windows-msvc`. Plain
   `cargo`/`cross build --release` — rustls links cleanly on musl with no OpenSSL,
   so **no special build flags**. Each archive bundles the binary + `README.md` +
   `LICENSE` + `examples/`.
3. **create-release** — extract the version's `CHANGES.md` section as notes,
   attach all archives.
4. **build-docs / deploy-docs** — build the tagged docs and publish them under
   `site/edaptor/<tag>`, then re-run cull/update-json/generate-redirect.

### 8. TLS migration to rustls (`Cargo.toml` + `src/ldap/tls.rs`)

Replace the `native-tls`/OpenSSL backend with rustls so static musl release
builds need no OpenSSL and no vendoring.

`Cargo.toml`:

- Add `license = "MIT"`.
- `ldap3 = { version = "0.12", default-features = false, features = ["sync", "tls-rustls-ring"] }`
- Remove the `native-tls` dependency.
- Add `rustls = "0.23"` and `rustls-pemfile = "2"` (version-aligned with the
  rustls `ldap3 0.12` re-exports, so the `ClientConfig` types match).

`src/ldap/tls.rs` — rewrite `build_settings` to produce the same
`LdapConnSettings`, preserving every current behaviour:

- **Connect timeout** and **StartTLS** — unchanged (`set_conn_timeout`,
  `set_starttls`).
- **Custom CA** (`tls.ca_cert`): parse the PEM with `rustls-pemfile` into
  `CertificateDer`s, load them into a `RootCertStore`, build a
  `ClientConfig::builder().with_root_certificates(store).with_no_client_auth()`,
  and attach via `settings.set_config(Arc::new(config))`.
- **`verify = false`**: in the common (no custom CA) case, call
  `settings.set_no_tls_verify(true)` — ldap3 then installs its own
  `NoCertVerification` on the default config. **Edge case confirmed in ldap3
  source:** ldap3 only applies that shortcut on its *default* config path, so
  when a custom CA *and* `verify = false` are both set, we must install the
  no-cert verifier into our own `ClientConfig` (`config.dangerous()
  .set_certificate_verifier(...)`). ~5 extra lines, branch documented in code.
- **Tests**: the 4 existing `build_settings` unit tests port directly (they only
  assert Ok/Err on missing/garbage CA files and StartTLS+no-verify); the
  garbage-CA test's error message changes from "parsing CA cert" to the
  rustls-pemfile parse failure — assertion updated accordingly.

This is a backend swap with identical externally-visible semantics; the module
doc comment is updated from "native-tls backend" to "rustls backend".

### 9. `CHANGES.md`

New, Keep-a-Changelog format, seeded with an `Unreleased` block (New/Changed/
Fixed) and a `0.1.0` entry. The release workflow's `perl` rewrite expects exactly
this layout.

### 10. `LICENSE` + README touch-ups

- `LICENSE` — MIT, copyright Tobias Oetiker.
- `README.md` — fix the stale "Turbo Vision" / "design complete" status text,
  set the License section to MIT, and add a link to the documentation site and a
  build/docs badge row.

## Verification

- `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test` all green
  locally (live tests SKIP without a server).
- `make docs` builds the book locally (mdBook + mermaid from mise).
- After the rustls migration: `cargo build` + `cargo test` green (no `native-tls`
  / OpenSSL in the dependency tree — verify with `cargo tree -i openssl-sys`
  returning nothing). A gated live TLS smoke test against the podman server (or
  manual `ldaps://` connect) confirms the custom-CA path still works.
- Workflows are validated by structure review against the working byonk
  originals; first real deploy/release happens when the user pushes/dispatches.

## Risks & mitigations

- **musl + OpenSSL**: eliminated by migrating to rustls — there is no OpenSSL in
  the tree, so static musl builds need no vendoring or system libraries.
- **rustls behaviour drift**: the migration is verified against the documented
  semantics (custom CA, `verify=false`, StartTLS) plus a live `ldaps://` smoke
  check, so the backend swap cannot silently change TLS behaviour.
- **No git tags yet**: the release version step defaults to `v0.0.0` → first
  release computes `0.0.1`/`0.1.0`/`1.0.0` from the chosen bump type. `Cargo.toml`
  currently says `0.1.0`; the workflow overwrites it from the tag math, so the
  first dispatched release should use the bump type that yields the intended
  number (documented in the release section of the docs).
- **First Pages deploy**: byonk's "mirror existing site" step tolerates a missing
  site (first deploy) — ported as-is.
```
