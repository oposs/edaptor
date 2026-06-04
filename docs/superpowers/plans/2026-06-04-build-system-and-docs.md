# Build System & Documentation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give `edaptor` the same proven build/docs infrastructure as the `byonk` project — a `Makefile` + `mise.toml` developer workflow, an mdBook documentation site whose centerpiece is the configuration reference, an annotated example config, and three CI workflows (check/lint/test, versioned docs deploy, cross-compiled release binaries) — adapted for a TUI client (no container image, no screenshot pipeline).

**Architecture:** Most artifacts are **ported verbatim or near-verbatim** from `../byonk` (`/home/oetiker/checkouts/byonk`) with a fixed find/replace from byonk's identifiers to edaptor's. The documentation prose is **handwritten** from `README.md`, the design specs under `docs/superpowers/specs/`, and project memory. The release workflow drops byonk's container job; the docs workflows drop byonk's screenshot/sample generation and historical-version backfill.

**Tech Stack:** GNU Make, mise, mdBook 0.5.2 + mdbook-mermaid 0.17.0, GitHub Actions, `cross`, TOML.

**PREREQUISITE:** The **rustls TLS migration** (`2026-06-04-rustls-tls-migration.md`) should land first — `release.yml`'s musl cross-builds assume no OpenSSL. This plan does not re-touch TLS code.

---

## Naming facts (the find/replace this whole plan depends on)

byonk's repo is `oetiker/byonk` and its Pages live at `oetiker.github.io/byonk`. **edaptor's are different** (per the design spec and project memory): repo **`oposs/edaptor`**, Pages **`https://oposs.github.io/edaptor`**, docs path segment **`/edaptor/`**, binary **`edaptor`**. Whenever a ported file contains `byonk`, the replacement target is the edaptor equivalent — usually `edaptor`, and for the repo owner `oposs` (NOT `oetiker`). GitHub Actions never hardcode the repo (they use `${{ github.repository }}`), so only `PAGES_URL`, `/byonk/` path segments, the binary name, and display strings change.

byonk-specific assets that **do not exist** in edaptor and must be dropped wherever a ported file references them: `docs/generate-samples.sh`, the `screens/` directory, the `fonts/` directory, `config.yaml` (edaptor uses `examples/*.toml`), `Dockerfile.release`, and byonk's historical release tags.

---

## File Structure

Created by this plan:

| Path | Responsibility |
|---|---|
| `LICENSE` | MIT license text (copyright Tobias Oetiker, 2026) |
| `mise.toml` | Pin Rust + mdBook + mdbook-mermaid for local `make docs` |
| `Makefile` | Developer workflow (build/run/test/lint/docs) |
| `examples/config.toml` | Annotated reference config exercising every option (docs centerpiece) |
| `CHANGES.md` | Keep-a-Changelog history; release workflow rewrites it |
| `docs/book.toml` | mdBook config (mermaid, version selector, light theme, edaptor URLs) |
| `docs/theme/version-selector.js` | Version dropdown logic |
| `docs/theme/version-selector.css` | Version dropdown + dev-banner styling |
| `docs/src/SUMMARY.md` | Book table of contents |
| `docs/src/**/*.md` | Handwritten documentation pages |
| `.github/workflows/ci.yml` | check & lint, test, build (verbatim from byonk) |
| `.github/workflows/docs.yml` | Build dev docs on push to main, deploy to Pages |
| `.github/workflows/release.yml` | Versioned cross-compiled binary releases + tagged docs |
| `.github/scripts/manage-doc-versions.sh` | cull / update-json / generate-redirect |
| `Cross.toml` | musl cross-compile images |

Modified: `Cargo.toml` (add `license = "MIT"`), `README.md` (status/license/docs-link refresh).

---

## Task 1: LICENSE + Cargo.toml license field

**Files:**
- Create: `LICENSE`
- Modify: `Cargo.toml:5` (add `license` line under `description`)

- [ ] **Step 1: Create `LICENSE`**

Copy `../byonk/LICENSE` verbatim, changing only the copyright year from `2025` to `2026`. Full content:

```
MIT License

Copyright (c) 2026 Tobias Oetiker

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

- [ ] **Step 2: Add the `license` field to `Cargo.toml`**

In `Cargo.toml`, after line 5 (`description = "..."`), add a `license` line so the `[package]` table reads:

```toml
[package]
name = "edaptor"
version = "0.1.0"
edition = "2021"
description = "TUI for editing OpenLDAP directories (users, groups, memberships)"
license = "MIT"
```

- [ ] **Step 3: Verify it parses**

Run: `cargo metadata --no-deps --format-version 1 >/dev/null && echo OK`
Expected: `OK` (no TOML parse error).

- [ ] **Step 4: Commit**

```bash
git add LICENSE Cargo.toml
git commit -m "$(cat <<'EOF'
chore: add MIT LICENSE and license metadata

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: mise.toml

Pins the same mdBook/mermaid versions CI uses, so `make docs` works locally with no manual install (an improvement over byonk's rust-only file).

**Files:**
- Create: `mise.toml`

- [ ] **Step 1: Create `mise.toml`**

```toml
[tools]
rust = "latest"
# mdBook's release archive ships a lowercase `mdbook` binary, but the repo is
# `mdBook`; ubi needs the explicit exe name or it looks for `mdBook*` and fails.
"ubi:rust-lang/mdBook"        = { version = "0.5.2", exe = "mdbook" }
"ubi:badboy/mdbook-mermaid"   = "0.17.0"
```

> **Note:** the `exe = "mdbook"` is required — without it `mise install` fails with
> "could not find any files matching [mdBook*]" (ubi derives the binary name from
> the repo name `mdBook`, but the shipped binary is lowercase `mdbook`).

- [ ] **Step 2: Verify mise accepts it (if mise is installed)**

Run: `mise ls 2>/dev/null | head || echo "mise not installed — skip"`
Expected: either a tool listing including mdBook/mdbook-mermaid, or the skip message. A TOML syntax error in `mise.toml` would make `mise` complain — none expected.

- [ ] **Step 3: Commit**

```bash
git add mise.toml
git commit -m "$(cat <<'EOF'
build: pin rust + mdBook + mdbook-mermaid via mise

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Makefile

Ported from `../byonk/Makefile`, adapted: `run` launches the TUI against the demo config; the server-only `run-release`, the screenshot `docs-samples` target, and the `screens`/coverage `lcov.info` clean of byonk are dropped/adjusted. Coverage targets and the Homebrew-LLVM shim are kept.

**Files:**
- Create: `Makefile`

- [ ] **Step 1: Create `Makefile`**

```makefile
# edaptor Makefile
# Build software and documentation

export PATH := $(HOME)/.cargo/bin:$(PATH)

.PHONY: all build release debug run watch clean docs docs-dev check fmt lint test \
        coverage coverage-ci coverage-text help

# Default target
all: release

# =============================================================================
# Software Build
# =============================================================================

# Build release binary (runs fmt and clippy first)
release: fmt lint
	cargo build --release

# Build debug binary (runs fmt and clippy first)
debug: fmt lint
	cargo build

# Alias for debug build
build: debug

# Run the TUI against the podman demo server (debug mode)
run: fmt lint
	cargo run -- --config examples/demo-config.toml

# Run with auto-reload (requires cargo-watch)
watch:
	cargo watch -x 'run -- --config examples/demo-config.toml'

# Format code
fmt:
	cargo fmt

# Run clippy linter
lint:
	cargo clippy --all-targets -- -D warnings

# Run tests
test:
	cargo test

# Coverage configuration for Homebrew Rust (set LLVM paths)
# For rustup users, these variables are not needed
LLVM_PREFIX ?= $(shell brew --prefix llvm 2>/dev/null || echo "")
ifneq ($(LLVM_PREFIX),)
  export LLVM_COV := $(LLVM_PREFIX)/bin/llvm-cov
  export LLVM_PROFDATA := $(LLVM_PREFIX)/bin/llvm-profdata
endif

# Run tests with coverage (requires cargo-llvm-cov)
coverage:
	cargo llvm-cov --html --open

# Generate coverage report for CI (lcov format)
coverage-ci:
	cargo llvm-cov --lcov --output-path lcov.info

# Generate coverage report (text summary)
coverage-text:
	cargo llvm-cov --summary-only

# Clean build artifacts
clean:
	cargo clean
	rm -rf docs/book
	rm -f lcov.info

# =============================================================================
# Documentation (mdBook)
# =============================================================================

# Build documentation
docs:
	cd docs && mdbook build

# Start documentation dev server
docs-dev:
	cd docs && mdbook serve

# =============================================================================
# Development Helpers
# =============================================================================

# Check everything before commit
check: fmt lint test
	@echo "All checks passed!"

# =============================================================================
# Help
# =============================================================================

help:
	@echo "edaptor Makefile"
	@echo ""
	@echo "Software:"
	@echo "  make build        Build debug binary (runs fmt + clippy)"
	@echo "  make release      Build release binary (runs fmt + clippy)"
	@echo "  make run          Run the TUI against examples/demo-config.toml"
	@echo "  make watch        Run with auto-reload (needs cargo-watch)"
	@echo "  make fmt          Format code"
	@echo "  make lint         Run clippy"
	@echo "  make test         Run tests"
	@echo "  make check        Format, lint, and test"
	@echo "  make clean        Clean all build artifacts"
	@echo ""
	@echo "Coverage (requires cargo-llvm-cov):"
	@echo "  make coverage      Generate HTML coverage report and open in browser"
	@echo "  make coverage-ci   Generate lcov.info for CI integration"
	@echo "  make coverage-text Print coverage summary to terminal"
	@echo ""
	@echo "Documentation:"
	@echo "  make docs         Build documentation"
	@echo "  make docs-dev     Start docs dev server"
	@echo ""
	@echo "  make help         Show this help"
```

- [ ] **Step 2: Verify the build targets work**

Run: `make fmt lint`
Expected: `cargo fmt` then `cargo clippy --all-targets -- -D warnings` both succeed.

Run: `make help`
Expected: prints the help text above. (Do **not** run `make docs` yet — the book does not exist until Task 7.)

- [ ] **Step 3: Commit**

```bash
git add Makefile
git commit -m "$(cat <<'EOF'
build: add Makefile (build/run/test/lint/docs/coverage)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: examples/config.toml (annotated reference)

A fully annotated reference config exercising every profile/default/password/picker option — the copy-pasteable starting point for real deployments, mirrored by the docs "Full Example" page. Unlike `examples/demo-config.toml` (which targets the podman test server), this one is a generic `dc=example,dc=com` template. The content is adapted from the heavily-annotated block in `README.md:63-164`.

**Files:**
- Create: `examples/config.toml`

- [ ] **Step 1: Create `examples/config.toml`**

```toml
# edaptor configuration reference
# ================================
# A single TOML file declares the LDAP connection, how to authenticate, and a
# set of "entry profiles" describing what a user / group means in your directory.
# Pass it with `edaptor --config <path>` (default: ~/.config/edaptor/config.toml).
#
# This file exercises every supported option and is safe to copy as a starting
# point. Replace the dc=example,dc=com base and the object classes with whatever
# your directory actually uses (edaptor introspects cn=subschema, so the forms
# adapt to your schema automatically).

[server]
uri          = "ldaps://ldap.example.com"   # ldap:// or ldaps://
base_dn      = "dc=example,dc=com"
start_tls    = false                          # true upgrades an ldap:// connection; do NOT combine with ldaps://
read_only    = false                          # true disables all write actions in the TUI
timeout_secs = 10                             # bound the TCP connect so an unreachable server cannot hang

# Optional TLS trust settings. Omit the whole table to use the system trust store
# with full verification.
[server.tls]
# ca_cert = "/etc/ssl/certs/my-ca.pem"        # trust a custom CA (PEM)
verify    = true                              # set false ONLY for testing — accepts any certificate

[auth]
method          = "simple"                    # simple bind (SASL EXTERNAL/GSSAPI are a later milestone)
bind_dn         = "cn=ldapmanager,dc=example,dc=com"
# The password is NEVER stored in this file. Choose a source:
#   "prompt"            -> ask interactively at startup (no echo)
#   "env:VAR"           -> read environment variable VAR
#   "command:some cmd"  -> run a command and read its stdout
password_source = "prompt"

# ---------------------------------------------------------------------------
# Entry profiles: what a "user", "group", and "posixgroup" mean here.
# ---------------------------------------------------------------------------
# `search_attrs` sets which attributes the picker substring-search matches;
# it falls back to `show`, then to ["cn"] when omitted.
#
# This "user" is a full posix (+optional Samba) account template: multiple object
# classes, defaulted/templated/auto-numbered fields, an inline password field,
# and picker bindings that pull values from (or fan out to) other profiles.
[[profile]]
name           = "user"
object_classes = ["inetOrgPerson", "posixAccount", "shadowAccount"]
rdn_attr       = "uid"
search_base    = "ou=people,dc=example,dc=com"
show           = ["uid", "cn", "sn", "givenName", "mail", "uidNumber", "gidNumber", "homeDirectory"]
search_attrs   = ["cn", "uid", "mail"]        # picker searches these attributes
# How an entry of this profile is labelled in the membership picker. `{attr}` is
# substituted by that attribute's value; literal text is kept. Defaults to cn.
label          = "{cn} ({uid})"               # e.g. "Bob Baker (bob)"

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

# Inline password field: the create/edit form shows a masked, confirm-twice field
# for `ldap_attribute` (the schema-generated field is suppressed). The cleartext
# goes to the directory; the LDIF preview shows ********.
#   samba = true -> also write sambaNTPassword/sambaPwdLastSet (needs sambaSamAccount).
[profile.password]
ldap_attribute = "userPassword"               # default; omit to use userPassword
samba          = false

# Picker bindings: `[profile.picker.<attr>]` declares how an attribute's field is
# populated from a live candidate search. Four knobs:
#   candidate   (required) — a [[profile]] `name` supplying the candidate search scope.
#   store       (default "dn") — "dn" stores the candidate's DN; any other value is
#                 an attribute name whose scalar is stored.
#   select      (default "auto") — cardinality: "auto" derives from the attribute's
#                 schema arity; "single" or "multi" override it.
#   fanout_attr (optional) — when set, the field is NOT written to the server;
#                 instead this entry's DN is added/removed in `fanout_attr` on each
#                 picked candidate (e.g. a user's memberOf fan-out writes `member`
#                 on each picked group).

# gidNumber: single-select picker over posixGroups; stores the chosen group's
# gidNumber scalar into the field (not its DN).
[profile.picker.gidNumber]
candidate = "posixgroup"
store     = "gidNumber"
select    = "single"

# memberOf: synthetic back-ref — ticking a group writes `member` on it. The
# memberOf attribute itself is overlay-maintained; edaptor never writes it directly.
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

- [ ] **Step 2: Verify it parses with edaptor's own config loader**

Run: `cargo run --quiet -- --config examples/config.toml --help 2>/dev/null || true`
Then assert the file is valid TOML at minimum:
Run: `python3 -c "import tomllib,sys; tomllib.load(open('examples/config.toml','rb')); print('valid TOML')"`
Expected: `valid TOML`.

If edaptor exposes a config-check/dry-run flag, prefer that; otherwise the TOML parse + the live docs example below are the check. (Do **not** require a server connection here.)

- [ ] **Step 3: Commit**

```bash
git add examples/config.toml
git commit -m "$(cat <<'EOF'
docs: add annotated examples/config.toml reference

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: CHANGES.md

Keep-a-Changelog format seeded with an `Unreleased` block and a `0.1.0` entry. **The exact layout matters** — `release.yml`'s `perl -i -0777` rewrite expects `## Unreleased` followed by `### New` / `### Changed` / `### Fixed` sections, then version sections as `## X.Y.Z - DATE`.

**Files:**
- Create: `CHANGES.md`

- [ ] **Step 1: Create `CHANGES.md`**

```markdown
# Changelog

All notable changes to edaptor are documented here. The format follows
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
```

> **Note:** the `## 0.1.0` line uses today's date (`2026-06-04`). If the first dispatched release is computed by `release.yml` from the bump type, that workflow appends a *new* dated section above this one and leaves `Unreleased` empty — so keep the `Unreleased` block's three empty subsections exactly as shown.

- [ ] **Step 2: Sanity-check the structure the release perl expects**

Run: `grep -nE '^## Unreleased|^### New|^### Changed|^### Fixed|^## 0\.1\.0' CHANGES.md`
Expected: prints the `## Unreleased`, the three `###` headers, and the `## 0.1.0 - 2026-06-04` line in that order.

- [ ] **Step 3: Commit**

```bash
git add CHANGES.md
git commit -m "$(cat <<'EOF'
docs: add CHANGES.md (Keep-a-Changelog, seeded 0.1.0)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: README refresh

Fix the stale "Turbo Vision" / "design complete, being planned" wording (the UI is ratatui 0.30 and the milestones are largely done), set the License section to MIT, and add a documentation-site link + a status line that matches reality.

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Replace the opening description (lines 1-9)**

Replace:

```markdown
# edaptor

A terminal UI (TUI) for administering an OpenLDAP directory — adding, modifying
and removing **users** and **groups**, and managing **group memberships** — built
in Rust on a [Turbo Vision](https://crates.io/crates/turbo-vision) port.

> **eDAPtor** — the *DAP* (Directory Access Protocol, the P in LDAP) baked into
> an *editor* / *adaptor*.
```

with:

```markdown
# edaptor

A terminal UI (TUI) for administering an OpenLDAP directory — adding, modifying
and removing **users** and **groups**, and managing **group memberships** — built
in Rust with [ratatui](https://ratatui.rs/).

> **eDAPtor** — the *DAP* (Directory Access Protocol, the P in LDAP) baked into
> an *editor* / *adaptor*.

📖 **Documentation:** <https://oposs.github.io/edaptor>
```

- [ ] **Step 2: Replace the Status section (lines 24-28)**

Replace:

```markdown
## Status

🚧 **Early development.** The design is complete; implementation is being planned
and executed in milestones.

- 📄 Design specification: [`docs/superpowers/specs/2026-05-29-edaptor-design.md`](docs/superpowers/specs/2026-05-29-edaptor-design.md)
```

with:

```markdown
## Status

**Working.** The core milestones are implemented on a three-pane ratatui
interface: schema-driven create/edit/rename/delete, defaults and auto-numbering,
inline passwords with the Samba lifecycle, unified candidate pickers, and
symmetric membership editing. See the [documentation](https://oposs.github.io/edaptor)
for usage, and `docs/superpowers/specs/` for the design specifications.
```

- [ ] **Step 3: Replace the License section (lines 166-168)**

Replace:

```markdown
## License

To be determined.
```

with:

```markdown
## License

[MIT](LICENSE) © Tobias Oetiker
```

- [ ] **Step 4: Verify no stale references remain**

Run: `grep -ni "turbo vision\|to be determined\|design is complete" README.md`
Expected: no matches.

- [ ] **Step 5: Commit**

```bash
git add README.md
git commit -m "$(cat <<'EOF'
docs: refresh README (ratatui not Turbo Vision, MIT, docs link)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: mdBook scaffold (config, theme, TOC) and a buildable empty book

Stand up the book so `make docs` builds, before writing prose. This task creates `book.toml`, the version-selector theme files, `SUMMARY.md`, and a stub for every page it references (so mdBook does not error on missing files). Later tasks fill the stubs with real content.

**Files:**
- Create: `docs/book.toml`
- Create: `docs/theme/version-selector.js`
- Create: `docs/theme/version-selector.css`
- Create: `docs/src/SUMMARY.md`
- Create: `docs/src/README.md` and one stub `.md` per `SUMMARY.md` entry

- [ ] **Step 1: Create `docs/book.toml`**

Ported from `../byonk/docs/book.toml` with edaptor identifiers and the `oposs` owner:

```toml
[book]
title = "edaptor"
description = "A terminal UI for administering an OpenLDAP directory"
authors = ["Tobias Oetiker"]
language = "en"
src = "src"

[build]
build-dir = "book"

[preprocessor.mermaid]
command = "mdbook-mermaid"

[output.html]
additional-js = ["mermaid.min.js", "mermaid-init.js", "theme/version-selector.js"]
additional-css = ["theme/version-selector.css"]
default-theme = "light"
preferred-dark-theme = "navy"
git-repository-url = "https://github.com/oposs/edaptor"
edit-url-template = "https://github.com/oposs/edaptor/edit/main/docs/{path}"
site-url = "/edaptor/"

[output.html.playground]
editable = false

[output.html.fold]
enable = true
```

- [ ] **Step 2: Create `docs/theme/version-selector.js`**

Ported from `../byonk/docs/theme/version-selector.js` with every `byonk` → `edaptor` (comment line 1, the two path regexes, and the `fetch` URL). Full content:

```javascript
// Version selector for edaptor documentation
(function() {
    'use strict';

    // Detect current version from URL path
    function getCurrentVersion() {
        const path = window.location.pathname;
        const match = path.match(/\/edaptor\/(v[\d.]+|dev)\//);
        return match ? match[1] : null;
    }

    // Create version selector dropdown
    function createVersionSelector(versions, currentVersion) {
        const container = document.createElement('div');
        container.className = 'version-selector';

        const select = document.createElement('select');
        select.id = 'version-select';
        select.setAttribute('aria-label', 'Select documentation version');

        versions.forEach(v => {
            const option = document.createElement('option');
            option.value = v.path;
            option.textContent = v.version + (v.prerelease ? ' (dev)' : '');
            if (v.version === currentVersion) {
                option.selected = true;
            }
            select.appendChild(option);
        });

        select.addEventListener('change', function() {
            const newPath = this.value;
            // Try to preserve the current page path
            const currentPath = window.location.pathname;
            const pageMatch = currentPath.match(/\/edaptor\/(?:v[\d.]+|dev)\/(.*)$/);
            const page = pageMatch ? pageMatch[1] : '';
            window.location.href = newPath + page;
        });

        const label = document.createElement('span');
        label.className = 'version-label';
        label.textContent = 'Version: ';

        container.appendChild(label);
        container.appendChild(select);

        return container;
    }

    // Create dev warning banner
    function createDevBanner() {
        const banner = document.createElement('div');
        banner.className = 'dev-warning-banner';
        banner.innerHTML = `
            <strong>Development Version</strong>
            <span>You are viewing documentation for the development version.
            This may include unreleased features and changes.</span>
        `;
        return banner;
    }

    // Initialize version selector
    function init() {
        const currentVersion = getCurrentVersion();
        if (!currentVersion) return;

        // Fetch versions.json
        fetch('/edaptor/versions.json')
            .then(response => response.json())
            .then(data => {
                // Insert version selector into the menu bar
                const menuBar = document.querySelector('.menu-bar');
                if (menuBar) {
                    const rightButtons = menuBar.querySelector('.right-buttons');
                    if (rightButtons) {
                        const selector = createVersionSelector(data.versions, currentVersion);
                        rightButtons.insertBefore(selector, rightButtons.firstChild);
                    }
                }

                // Show dev warning banner if on dev version
                if (currentVersion === 'dev') {
                    const main = document.querySelector('main') || document.querySelector('#content');
                    if (main) {
                        const banner = createDevBanner();
                        main.insertBefore(banner, main.firstChild);
                    }
                }
            })
            .catch(err => {
                console.warn('Could not load versions.json:', err);
            });
    }

    // Run on DOM ready
    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', init);
    } else {
        init();
    }
})();
```

- [ ] **Step 3: Create `docs/theme/version-selector.css`**

Copy `../byonk/docs/theme/version-selector.css` **verbatim** (it contains no byonk strings — only mdBook CSS variables and generic class names). The full content:

```css
/* Version selector dropdown */
.version-selector {
    display: flex;
    align-items: center;
    margin-right: 1rem;
    font-size: 0.9em;
}

.version-selector .version-label {
    margin-right: 0.3rem;
    color: var(--icons);
    white-space: nowrap;
}

.version-selector select {
    padding: 0.25rem 0.5rem;
    border: 1px solid var(--icons);
    border-radius: 4px;
    background-color: var(--bg);
    color: var(--fg);
    font-size: 0.9em;
    cursor: pointer;
    min-width: 80px;
}

.version-selector select:hover {
    border-color: var(--links);
}

.version-selector select:focus {
    outline: none;
    border-color: var(--links);
    box-shadow: 0 0 0 2px rgba(var(--links-rgb, 0, 0, 0), 0.2);
}

/* Development warning banner */
.dev-warning-banner {
    background: linear-gradient(135deg, #ff9800 0%, #f57c00 100%);
    color: #000;
    padding: 0.75rem 1rem;
    margin: -0.5rem -0.5rem 1rem -0.5rem;
    border-radius: 4px;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.5rem;
    box-shadow: 0 2px 4px rgba(0, 0, 0, 0.1);
}

.dev-warning-banner strong {
    background: rgba(0, 0, 0, 0.15);
    padding: 0.2rem 0.5rem;
    border-radius: 3px;
    font-size: 0.85em;
    text-transform: uppercase;
    letter-spacing: 0.5px;
}

.dev-warning-banner span {
    flex: 1;
    min-width: 200px;
}

/* Dark theme adjustments */
.navy .dev-warning-banner,
.coal .dev-warning-banner,
.ayu .dev-warning-banner {
    background: linear-gradient(135deg, #e65100 0%, #bf360c 100%);
    color: #fff;
}

.navy .dev-warning-banner strong,
.coal .dev-warning-banner strong,
.ayu .dev-warning-banner strong {
    background: rgba(255, 255, 255, 0.15);
}

/* Responsive adjustments */
@media (max-width: 600px) {
    .version-selector .version-label {
        display: none;
    }

    .dev-warning-banner {
        flex-direction: column;
        text-align: center;
    }
}
```

- [ ] **Step 4: Create `docs/src/SUMMARY.md`**

```markdown
# Summary

[Introduction](README.md)

# Getting Started

- [Installation](getting-started/installation.md)
- [Quick Start](getting-started/quick-start.md)

# Configuration

- [Overview](configuration/overview.md)
- [Server & Authentication](configuration/server-auth.md)
- [Entry Profiles](configuration/entry-profiles.md)
- [Defaults](configuration/defaults.md)
- [Passwords](configuration/passwords.md)
- [Pickers](configuration/pickers.md)
- [Full Example](configuration/full-example.md)

# Concepts

- [Architecture](concepts/architecture.md)
- [Object Model](concepts/object-model.md)
- [LDAP Constraints](concepts/ldap-constraints.md)
- [Change Flow](concepts/change-flow.md)

# Usage

- [The Three-Pane TUI](usage/three-pane.md)
- [Creating, Editing, Renaming, Deleting](usage/crud.md)
- [Membership Editing](usage/membership.md)
- [Passwords & Samba](usage/passwords.md)

# Reference

- [Test Server](reference/test-server.md)
```

- [ ] **Step 5: Create a stub for every referenced page**

For each of the 19 `.md` files referenced above (`README.md`, the 2 getting-started, 7 configuration, 4 concepts, 4 usage, 1 reference), create the file with a single H1 placeholder so mdBook builds. Use the page's title as the H1. Example for `docs/src/getting-started/installation.md`:

```markdown
# Installation

<!-- filled in by a later task -->
```

Create the directory tree: `docs/src/{getting-started,configuration,concepts,usage,reference}/`. The H1 for each file:
- `docs/src/README.md` → `# edaptor`
- `getting-started/installation.md` → `# Installation`
- `getting-started/quick-start.md` → `# Quick Start`
- `configuration/overview.md` → `# Configuration Overview`
- `configuration/server-auth.md` → `# Server & Authentication`
- `configuration/entry-profiles.md` → `# Entry Profiles`
- `configuration/defaults.md` → `# Defaults`
- `configuration/passwords.md` → `# Passwords`
- `configuration/pickers.md` → `# Pickers`
- `configuration/full-example.md` → `# Full Example`
- `concepts/architecture.md` → `# Architecture`
- `concepts/object-model.md` → `# Object Model`
- `concepts/ldap-constraints.md` → `# LDAP Constraints`
- `concepts/change-flow.md` → `# Change Flow`
- `usage/three-pane.md` → `# The Three-Pane TUI`
- `usage/crud.md` → `# Creating, Editing, Renaming, Deleting`
- `usage/membership.md` → `# Membership Editing`
- `usage/passwords.md` → `# Passwords & Samba`
- `reference/test-server.md` → `# Test Server`

- [ ] **Step 6: Install mermaid assets and build the book**

mdBook + mdbook-mermaid must be available (via `mise install` or already on PATH). The `additional-js` in `book.toml` references `mermaid.min.js` / `mermaid-init.js`, which are produced by `mdbook-mermaid install`:

Run:
```bash
mdbook-mermaid install docs
cd docs && mdbook build && cd ..
```
Expected: `mdbook-mermaid install` writes `docs/mermaid.min.js` and `docs/mermaid-init.js`; `mdbook build` reports `... has been deleted` / build success and produces `docs/book/index.html`. No "Summary parsing failed" / missing-file errors.

- [ ] **Step 7: Decide gitignore for build output**

Add `docs/book/` to `.gitignore` (do not commit built HTML). Leave `docs/mermaid.min.js` and `docs/mermaid-init.js` committed (CI also regenerates them, but committing keeps a local `mdbook build` working without the install step).

Run: `grep -q '^docs/book/$' .gitignore || printf 'docs/book/\n' >> .gitignore`

- [ ] **Step 8: Commit**

```bash
git add docs/book.toml docs/theme docs/src docs/mermaid.min.js docs/mermaid-init.js .gitignore
git commit -m "$(cat <<'EOF'
docs: scaffold mdBook (book.toml, version selector, TOC, stubs)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Documentation content — Introduction + Getting Started

Fill three stubs with real prose. **Sources:** `README.md`, `docs/HANDOVER.md`, and the run/test commands in the handover. TUI layouts are fenced ` ``` ` blocks (no screenshots).

**Files:**
- Modify: `docs/src/README.md`, `docs/src/getting-started/installation.md`, `docs/src/getting-started/quick-start.md`

- [ ] **Step 1: Write `docs/src/README.md` (Introduction)**

Required sections/content (correct the stale README wording — the UI is ratatui 0.30, milestones are largely done):
- One-paragraph intro: terminal UI for administering an OpenLDAP directory (users, groups, memberships), built in Rust with ratatui.
- "What makes it different" — schema introspection (`cn=subschema`), forms generated from `objectClass` definitions, a TOML config declaring connection + entry profiles. (Adapt `README.md:11-22`.)
- "Highlights" bullet list — two-tier object model, responsive background LDAP worker, immediate-apply with on-demand LDIF preview, cn-based labels, full Samba lifecycle. (Adapt `README.md:30-42`.)
- A short "Where to go next" linking to Installation, Quick Start, and the Configuration section.
- Do **not** mention Turbo Vision or "design being planned".

- [ ] **Step 2: Write `docs/src/getting-started/installation.md`**

Required content:
- Building from source: `cargo build --release`, binary at `target/release/edaptor`. Prereq: a recent stable Rust toolchain (mention `mise install` picks up the pinned tools).
- Note the TLS backend is rustls (no OpenSSL needed) — static musl release binaries will be published via GitHub Releases once the project is pushed.
- Config file location: `--config <path>`, default `~/.config/edaptor/config.toml`. Link to the Configuration section.

- [ ] **Step 3: Write `docs/src/getting-started/quick-start.md`**

Required content, drawn from `docs/HANDOVER.md:62-78` and `README.md:46-57`:
- The fastest path is the bundled podman test server. Show the exact commands:
  ```bash
  scripts/test-ldap.sh start
  export EDAPTOR_TEST_ADMIN_PW=adminpassword
  cargo run -- --config examples/demo-config.toml
  ```
- Note the seed data: ~600 users / ~25 groups, shared user password `test123`, base `dc=example,dc=org`.
- One fenced ` ``` ` ASCII sketch of the three-pane layout the user will see on launch (tree | list | detail) — keep it schematic, label the three panes and the footer hint line. (Derive the exact pane arrangement from `usage/three-pane.md`, Task 11; keep them consistent.)
- Stopping the server: `scripts/test-ldap.sh stop`.

- [ ] **Step 4: Build and verify**

Run: `cd docs && mdbook build && cd ..`
Expected: success, no warnings about broken links. Spot-check `docs/book/getting-started/quick-start.html` exists.

- [ ] **Step 5: Commit**

```bash
git add docs/src/README.md docs/src/getting-started
git commit -m "$(cat <<'EOF'
docs: write Introduction + Getting Started pages

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: Documentation content — Configuration (the centerpiece)

Seven pages. **Primary source:** the annotated `examples/config.toml` (Task 4) and `README.md:63-164`. Each option's prose must match the example file's comments. This is the highest-priority section per the design spec — be thorough and use real TOML snippets pulled from `examples/config.toml`.

**Files:**
- Modify: the 7 files under `docs/src/configuration/`

- [ ] **Step 1: `configuration/overview.md`**

- What the config file is (single TOML, `--config`, default path), and the top-level shape: `[server]`, `[server.tls]`, `[auth]`, repeated `[[profile]]` tables.
- A one-paragraph orientation map linking to each subsection.
- Note: edaptor introspects the live schema, so forms adapt automatically; the config declares *intent* (what a user/group means), not field layouts.

- [ ] **Step 2: `configuration/server-auth.md`**

- `[server]`: `uri` (ldap:// vs ldaps://), `base_dn`, `start_tls` (and the "don't combine with ldaps://" rule), `read_only`, `timeout_secs`. Show the `[server]` TOML block.
- `[server.tls]`: `ca_cert` (custom CA, PEM), `verify` (and the explicit "testing only" warning that `verify=false` accepts any certificate). Mention the rustls backend.
- `[auth]`: `method = "simple"` (note SASL EXTERNAL/GSSAPI are a later milestone), `bind_dn`, and `password_source` with its three forms (`prompt`, `env:VAR`, `command:cmd`) and the "password is never stored in the file" rule.

- [ ] **Step 3: `configuration/entry-profiles.md`**

- What a `[[profile]]` is: `name`, `object_classes`, `rdn_attr`, `search_base`, `show`, `search_attrs` (with the fallback chain: `search_attrs` → `show` → `["cn"]`), `label` (with `{attr}` substitution, defaults to cn).
- Use the `user`, `group`, `posixgroup` profiles from `examples/config.toml` as worked examples.
- Cross-link to Defaults, Passwords, Pickers for the sub-tables.

- [ ] **Step 4: `configuration/defaults.md`**

- `[profile.defaults]`: fills EMPTY fields on create only (never overwrites operator input). The three value kinds with examples: literal (`loginShell = "/bin/bash"`), template (`homeDirectory = "/home/{uid}"`), auto-number (`uidNumber = "{next:10000-60000}"`).
- Explain the auto-number scan semantics + the size-limit caveat (refuses if the directory scan is truncated — bind with a high-limit identity). Source: `README.md:96-104` and project memory `edaptor-ldap-constraints`.

- [ ] **Step 5: `configuration/passwords.md`**

- `[profile.password]`: `ldap_attribute` (default `userPassword`), `samba` flag. Describe the inline masked confirm-twice field, suppression of the schema-generated field, cleartext to directory, `********` in LDIF preview.
- `samba = true` lifecycle: also writes `sambaNTPassword`/`sambaPwdLastSet`, needs `sambaSamAccount`, NT-hash computed client-side, SID from the directory's `sambaDomain`. Source: `README.md:106-112`, `README.md:42`, memory `edaptor-next-milestone-user-templates`.
- Mention the `edaptor passwd <dn>` CLI for setting a password outside the TUI.

- [ ] **Step 6: `configuration/pickers.md`**

- `[profile.picker.<attr>]`: the four knobs — `candidate` (required), `store` (default `dn`; else an attribute name whose scalar is stored), `select` (`auto`/`single`/`multi`), `fanout_attr` (writes this entry's DN into `fanout_attr` on each picked candidate instead of writing the field).
- Three worked examples straight from `examples/config.toml`: `gidNumber` (single, stores scalar), `memberOf` (dn + fanout to `member`, the overlay-maintained back-ref edaptor never writes directly), `member`/`memberUid` (multi, dn vs uid scalar).

- [ ] **Step 7: `configuration/full-example.md`**

- Embed the complete `examples/config.toml` in one fenced ```toml block, then walk through it table-by-table with short prose between blocks, linking back to the subsection pages. This is the "copy-pasteable starting point" page. Keep it byte-identical to `examples/config.toml` (if they drift, the example file is the source of truth).

- [ ] **Step 8: Build and verify**

Run: `cd docs && mdbook build && cd ..`
Expected: success; spot-check each `docs/book/configuration/*.html`. Verify no broken intra-doc links (mdbook prints warnings for these).

- [ ] **Step 9: Commit**

```bash
git add docs/src/configuration
git commit -m "$(cat <<'EOF'
docs: write Configuration reference (centerpiece, 7 pages)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: Documentation content — Concepts (with mermaid diagrams)

Four pages. **Sources:** the design specs under `docs/superpowers/specs/`, project memory (`edaptor-ldap-constraints`, `edaptor-ratatui-migration`, `edaptor-app-rs-decomposition`), and `README.md`. Each page uses at least one mermaid diagram (fenced ```mermaid block).

**Files:**
- Modify: the 4 files under `docs/src/concepts/`

- [ ] **Step 1: `concepts/architecture.md`**

- Schema-driven forms: introspect `cn=subschema`, generate edit forms from `objectClass` definitions at runtime.
- Background LDAP worker: all LDAP I/O on a worker thread; the UI never blocks on the network. Source: `README.md:36-38`, memory `edaptor-ratatui-migration`.
- Include a mermaid `flowchart` of: TUI (ratatui) ⇄ channel ⇄ LDAP worker ⇄ OpenLDAP. Concrete starter:
  ```mermaid
  flowchart LR
      UI["Three-pane TUI (ratatui)"] -- requests --> W["LDAP worker thread"]
      W -- results --> UI
      W <--> S[("OpenLDAP\ncn=subschema, entries")]
  ```

- [ ] **Step 2: `concepts/object-model.md`**

- The two-tier model: a generic schema-driven entry engine, with a *users & groups* understanding layered over it (passwords, memberships, Samba) acting across view/create/edit/delete/rename. Source: `README.md:32-35`.
- A mermaid diagram showing the generic entry engine at the base and the user/group understanding on top.

- [ ] **Step 3: `concepts/ldap-constraints.md`**

- The hard OpenLDAP limits that shape the UI, straight from memory `edaptor-ldap-constraints`: no has-children flag (can't cheaply know if a node has children), server size limits (and how they gate the `{next}` auto-number scan), RFC 4533 sync constraints, overlay-maintained `memberOf` (edaptor writes `member`, never `memberOf`), no per-entry rights introspection.
- Explain how each constraint maps to a design choice.

- [ ] **Step 4: `concepts/change-flow.md`**

- The diff → ChangeSet → LDIF preview → Modify/Add/ModRdn/Delete pipeline. Immediate apply with on-demand LDIF preview of the exact change. Source: `README.md:38-39`, design spec.
- A mermaid `flowchart` of the change pipeline. Concrete starter:
  ```mermaid
  flowchart TD
      E[Edit form] --> D[Diff vs. original]
      D --> C[ChangeSet]
      C --> P[LDIF preview]
      P -->|confirm| O{Operation}
      O --> A[Add]
      O --> M[Modify]
      O --> R[ModRdn]
      O --> X[Delete]
  ```

- [ ] **Step 5: Build and verify mermaid renders**

Run: `cd docs && mdbook build && cd ..`
Expected: success. Open `docs/book/concepts/change-flow.html` and confirm the page contains a `<pre class="mermaid">` (or rendered SVG) block — i.e. the mermaid preprocessor ran. If the diagrams appear as raw text, `mdbook-mermaid install docs` (Task 7 Step 6) was not run.

- [ ] **Step 6: Commit**

```bash
git add docs/src/concepts
git commit -m "$(cat <<'EOF'
docs: write Concepts pages with mermaid diagrams

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: Documentation content — Usage

Four pages. **Sources:** memory `edaptor-three-pane-progress`, `edaptor-ui-polish`, `README.md`. TUI layouts are fenced ` ``` ` ASCII blocks. **Keep the three-pane sketch consistent with the Quick Start sketch (Task 8 Step 3).**

**Files:**
- Modify: the 4 files under `docs/src/usage/`

- [ ] **Step 1: `usage/three-pane.md`**

- Describe the three panes (navigation tree | entry list | detail/edit), focus model (double-border focus, Tab/Shift-Tab to move, per-pane footers), and the white theme. Source: memory `edaptor-ui-polish`, `edaptor-three-pane-progress`.
- One canonical fenced ASCII layout of the full screen with the three panes and footer. This sketch is the reference the Quick Start page points at — make it the authoritative one.

- [ ] **Step 2: `usage/crud.md`**

- Walk the create / edit / rename / delete flows: how forms are generated from the entry's profile, defaults filling on create, the dirty-guard before discarding edits, rename = ModRdn, delete with confirmation. Each ends in the LDIF-preview → apply flow (link to `concepts/change-flow.md`).

- [ ] **Step 3: `usage/membership.md`**

- Symmetric membership editing: editing a group's `member` and a user's `memberOf` are two views of the same relationship; incremental search on both panes; the fan-out write model (link to `configuration/pickers.md`). Source: `README.md:40`, memory `edaptor-unified-picker-merged`.

- [ ] **Step 4: `usage/passwords.md`**

- Setting passwords in the TUI (inline field on create/edit of password-profile entries), the Samba-enable behaviour, and the `edaptor passwd <dn>` CLI. Note the M5 gap honestly: there is no standalone in-TUI "set password" on arbitrary non-password-profile entries (only inline + the CLI). Source: `docs/HANDOVER.md:53-55`.

- [ ] **Step 5: Build and verify**

Run: `cd docs && mdbook build && cd ..`
Expected: success, no broken links.

- [ ] **Step 6: Commit**

```bash
git add docs/src/usage
git commit -m "$(cat <<'EOF'
docs: write Usage pages (three-pane, CRUD, membership, passwords)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 12: Documentation content — Reference (Test Server)

One page. **Sources:** `docs/HANDOVER.md:62-78`, `README.md:46-57`, memory `edaptor-test-server`, and `scripts/test-ldap.sh`.

**Files:**
- Modify: `docs/src/reference/test-server.md`

- [ ] **Step 1: Write `reference/test-server.md`**

- What `scripts/test-ldap.sh start` provisions: a podman OpenLDAP mirroring the `oposs.openldap` role — Samba + mail schemas, memberOf/refint/ppolicy overlays, password policies — seeded with ~600 users across 5 departments and ~25 groups.
- The exact lifecycle commands (`start` / `stop`), the env vars (`EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389`, `EDAPTOR_TEST_ADMIN_PW=adminpassword`), the base `dc=example,dc=org`, the shared user password `test123`.
- How the live test suite uses it (gated by `EDAPTOR_TEST_LDAP_URI`): the `live_*` tests run only when it is set.
- Note this is podman, not docker.

- [ ] **Step 2: Build the full book one final time**

Run: `make docs`
Expected: builds clean. Then verify no stub markers remain:
Run: `grep -rl "filled in by a later task" docs/src && echo "STUBS REMAIN" || echo "no stubs"`
Expected: `no stubs`.

- [ ] **Step 3: Commit**

```bash
git add docs/src/reference
git commit -m "$(cat <<'EOF'
docs: write Test Server reference page

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 13: CI workflow (ci.yml)

Copy `../byonk/.github/workflows/ci.yml` **verbatim** — it contains zero byonk-specific strings and its three jobs (check & lint, test, build) are exactly what edaptor needs. Live tests gate on `EDAPTOR_TEST_LDAP_URI` and SKIP when unset, so `cargo test` is safe with no server.

**Files:**
- Create: `.github/workflows/ci.yml`

- [ ] **Step 1: Create `.github/workflows/ci.yml` from byonk verbatim**

Run:
```bash
mkdir -p .github/workflows
cp ../byonk/.github/workflows/ci.yml .github/workflows/ci.yml
```
Then confirm no byonk string slipped in (there should be none):
Run: `grep -ni byonk .github/workflows/ci.yml || echo "clean"`
Expected: `clean`.

- [ ] **Step 2: Lint the YAML**

Run: `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml')); print('valid yaml')"`
Expected: `valid yaml`.

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "$(cat <<'EOF'
ci: add check/lint, test, build workflow

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 14: Doc-versioning script (manage-doc-versions.sh)

Copy `../byonk/.github/scripts/manage-doc-versions.sh` and rewrite every hardcoded `/byonk/` path segment (and the HTML `<title>`) to `/edaptor/`. All seven `/byonk/` occurrences are inside generated output (versions.json paths + the redirect HTML); the `<site-dir>` is a positional arg, so the workflows pass `site/edaptor`.

**Files:**
- Create: `.github/scripts/manage-doc-versions.sh`

- [ ] **Step 1: Copy and rewrite**

Run:
```bash
mkdir -p .github/scripts
sed -e 's#/byonk/#/edaptor/#g' -e 's/Byonk Documentation/edaptor Documentation/' \
    ../byonk/.github/scripts/manage-doc-versions.sh > .github/scripts/manage-doc-versions.sh
chmod +x .github/scripts/manage-doc-versions.sh
```

- [ ] **Step 2: Verify no byonk path remains and the script is sound**

Run: `grep -n "byonk\|Byonk" .github/scripts/manage-doc-versions.sh || echo "clean"`
Expected: `clean`.

Run: `bash -n .github/scripts/manage-doc-versions.sh && echo "syntax ok"`
Expected: `syntax ok`.

- [ ] **Step 3: Smoke-test the generators in a temp dir**

Run:
```bash
tmp=$(mktemp -d); mkdir -p "$tmp/edaptor/dev" "$tmp/edaptor/v0.1.0"
.github/scripts/manage-doc-versions.sh "$tmp/edaptor" update-json
.github/scripts/manage-doc-versions.sh "$tmp/edaptor" generate-redirect
grep -q '/edaptor/' "$tmp/edaptor/versions.json" && grep -q '/edaptor/' "$tmp/edaptor/index.html" && echo "OK"
rm -rf "$tmp"
```
Expected: `OK` (versions.json lists `dev` + `v0.1.0` with `/edaptor/` paths; the redirect HTML points at `/edaptor/`).

- [ ] **Step 4: Commit**

```bash
git add .github/scripts/manage-doc-versions.sh
git commit -m "$(cat <<'EOF'
ci: add doc-version management script (cull/json/redirect)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 15: Docs deploy workflow (docs.yml)

Port `../byonk/.github/workflows/docs.yml` and adapt: set `PAGES_URL` to edaptor's; rewrite `/byonk/`→`/edaptor/` paths; **remove** the screenshot-generation steps (edaptor has no `docs/generate-samples.sh`); **remove** the `screens/**` path filter; and **remove** the historical-version `backfill-releases` / `build-dev-for-backfill` / `deploy-backfill` jobs (edaptor has no prior tags). The result is the dev-docs build + Pages deploy only.

**Files:**
- Create: `.github/workflows/docs.yml`

- [ ] **Step 1: Create the trimmed `docs.yml`**

Write `.github/workflows/docs.yml` with exactly this content (this is byonk's `build-dev` + `deploy` jobs, minus screenshots/screens, with edaptor identifiers):

```yaml
name: Documentation

on:
  push:
    branches: [main]
    paths:
      - 'docs/**'
      - 'src/**'
      - '.github/workflows/docs.yml'
      - '.github/scripts/manage-doc-versions.sh'
  workflow_dispatch:

permissions:
  contents: read
  pages: write
  id-token: write

concurrency:
  group: pages
  cancel-in-progress: false

env:
  PAGES_URL: "https://oposs.github.io/edaptor"

jobs:
  build-dev:
    name: Build Dev Documentation
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install mdBook and plugins
        run: |
          mkdir -p ~/bin
          curl -sSL https://github.com/rust-lang/mdBook/releases/download/v0.5.2/mdbook-v0.5.2-x86_64-unknown-linux-gnu.tar.gz | tar -xz -C ~/bin
          curl -sSL https://github.com/badboy/mdbook-mermaid/releases/download/v0.17.0/mdbook-mermaid-v0.17.0-x86_64-unknown-linux-gnu.tar.gz | tar -xz -C ~/bin
          echo "$HOME/bin" >> $GITHUB_PATH

      - name: Setup mermaid assets
        run: mdbook-mermaid install docs

      - name: Build documentation
        working-directory: docs
        run: mdbook build

      - name: Upload dev docs artifact
        uses: actions/upload-artifact@v4
        with:
          name: docs-dev
          path: docs/book
          retention-days: 1

  deploy:
    name: Deploy to GitHub Pages
    runs-on: ubuntu-latest
    needs: [build-dev]
    if: ${{ always() && needs.build-dev.result == 'success' }}
    environment:
      name: github-pages
      url: ${{ steps.deployment.outputs.page_url }}
    steps:
      - uses: actions/checkout@v4
        with:
          sparse-checkout: .github/scripts

      - name: Download dev docs
        uses: actions/download-artifact@v4
        with:
          name: docs-dev
          path: new-docs

      - name: Mirror existing GitHub Pages
        run: |
          mkdir -p site
          wget --mirror --no-parent --no-host-directories \
               --directory-prefix=site \
               --reject="index.html?*" \
               --execute robots=off \
               --quiet \
               "$PAGES_URL/" 2>/dev/null || echo "No existing site to mirror (first deployment?)"
          find site -name "*.tmp" -delete 2>/dev/null || true
          find site -name "index.html\?*" -delete 2>/dev/null || true
          echo "Existing site contents:"
          ls -la site/ || echo "(empty)"

      - name: Update site with dev docs
        run: |
          rm -rf site/edaptor/dev
          mkdir -p site/edaptor/dev
          cp -r new-docs/* site/edaptor/dev/

      - name: Update versions and redirect
        run: |
          chmod +x .github/scripts/manage-doc-versions.sh
          .github/scripts/manage-doc-versions.sh site/edaptor cull
          .github/scripts/manage-doc-versions.sh site/edaptor update-json
          .github/scripts/manage-doc-versions.sh site/edaptor generate-redirect

      - name: Show final site structure
        run: |
          echo "Final site structure:"
          find site -type f -name "*.html" | head -20
          echo ""
          echo "versions.json:"
          cat site/edaptor/versions.json

      - name: Setup Pages
        uses: actions/configure-pages@v4

      - name: Upload artifact
        uses: actions/upload-pages-artifact@v3
        with:
          path: site/edaptor

      - name: Deploy to GitHub Pages
        id: deployment
        uses: actions/deploy-pages@v4
```

- [ ] **Step 2: Verify YAML + no byonk/screenshot leftovers**

Run: `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/docs.yml')); print('valid yaml')"`
Expected: `valid yaml`.

Run: `grep -niE "byonk|generate-samples|screens|backfill" .github/workflows/docs.yml || echo "clean"`
Expected: `clean`.

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/docs.yml
git commit -m "$(cat <<'EOF'
ci: add docs deploy workflow (dev docs -> versioned Pages)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 16: Release workflow (release.yml) + Cross.toml

Port `../byonk/.github/workflows/release.yml` and adapt: edaptor identifiers; bundle `README.md` + `LICENSE` + `examples/` (not byonk's `fonts`/`screens`/`config.yaml`); **remove the entire `build-container` job and its `Dockerfile.release`**; drop `build-container` from `create-release`'s `needs`; **remove the screenshot-generation step** from `build-docs`. Keep the five-target binary matrix (two musl via `cross`, two darwin, one windows) and the tagged-docs deploy. Add `Cross.toml` for the musl images.

> **Watch-note (first release dispatch):** the rustls `ring` provider compiles a small C/asm core. On `x86_64-unknown-linux-musl` via `cross` this is routine, but `aarch64-unknown-linux-musl` can need extra target C-toolchain setup inside the cross image and occasionally fails where x86_64 succeeds. This is **not verifiable from CI** (the release workflow is `workflow_dispatch`-only), so it does not block this plan — but the first dispatched release should watch the `aarch64-unknown-linux-musl` job. If `ring` won't build there, the fallback is to drop that one matrix entry rather than switch providers (`ring` is already the more musl-portable choice vs aws-lc-rs).

**Files:**
- Create: `.github/workflows/release.yml`
- Create: `Cross.toml`

- [ ] **Step 1: Create `Cross.toml` verbatim from byonk**

Run: `cp ../byonk/Cross.toml Cross.toml`
Content (for reference — it has no byonk strings):
```toml
# Cross-compilation configuration for fully static musl binaries
# See https://github.com/cross-rs/cross

[target.x86_64-unknown-linux-musl]
image = "ghcr.io/cross-rs/x86_64-unknown-linux-musl:main"

[target.aarch64-unknown-linux-musl]
image = "ghcr.io/cross-rs/aarch64-unknown-linux-musl:main"

[build.env]
passthrough = ["RUSTFLAGS"]
```

- [ ] **Step 2: Create `.github/workflows/release.yml`**

Write the file with this content. It is byonk's release workflow with: `byonk`→`edaptor` binary/archive/display names; `PAGES_URL` set to edaptor's; the archive staging copying `README.md`+`LICENSE`+`examples/` (no fonts/screens/config.yaml); **no `build-container` job**; `create-release` `needs: [version, build-binaries]`; and the `build-docs` job's mdBook build with **no** screenshot step:

```yaml
name: Release

on:
  workflow_dispatch:
    inputs:
      release_type:
        description: 'Release type'
        required: true
        type: choice
        options:
          - bugfix
          - feature
          - major

env:
  CARGO_TERM_COLOR: always
  PAGES_URL: "https://oposs.github.io/edaptor"

jobs:
  version:
    name: Bump Version
    runs-on: ubuntu-latest
    permissions:
      contents: write
    outputs:
      version: ${{ steps.version.outputs.version }}
      tag: ${{ steps.version.outputs.tag }}
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0

      - name: Verify main branch
        run: |
          if [ "${{ github.ref }}" != "refs/heads/main" ]; then
            echo "::error::Releases must be created from the main branch"
            exit 1
          fi

      - name: Calculate new version
        id: version
        run: |
          LATEST=$(git tag -l 'v[0-9]*.[0-9]*.[0-9]*' | sort -V | tail -1 || echo "v0.0.0")
          if [ -z "$LATEST" ]; then
            LATEST="v0.0.0"
          fi
          MAJOR=$(echo "$LATEST" | sed 's/v\([0-9]*\)\.\([0-9]*\)\.\([0-9]*\)/\1/')
          MINOR=$(echo "$LATEST" | sed 's/v\([0-9]*\)\.\([0-9]*\)\.\([0-9]*\)/\2/')
          PATCH=$(echo "$LATEST" | sed 's/v\([0-9]*\)\.\([0-9]*\)\.\([0-9]*\)/\3/')
          case "${{ inputs.release_type }}" in
            major)
              NEW_VERSION="$((MAJOR+1)).0.0"
              ;;
            feature)
              NEW_VERSION="${MAJOR}.$((MINOR+1)).0"
              ;;
            bugfix)
              NEW_VERSION="${MAJOR}.${MINOR}.$((PATCH+1))"
              ;;
          esac
          echo "version=${NEW_VERSION}" >> $GITHUB_OUTPUT
          echo "tag=v${NEW_VERSION}" >> $GITHUB_OUTPUT
          echo "New version: ${NEW_VERSION}"

      - name: Update Cargo.toml version
        run: |
          sed -i 's/^version = ".*"/version = "${{ steps.version.outputs.version }}"/' Cargo.toml

      - name: Update CHANGES.md
        run: |
          DATE=$(date +%Y-%m-%d)
          VERSION="${{ steps.version.outputs.version }}"
          perl -i -0777 -pe '
            s/## Unreleased\n+(### New\n(.*?))?(\n### Changed\n(.*?))?(\n### Fixed\n(.*?))?\n+(?=##|\z)/
              "## Unreleased\n\n### New\n\n### Changed\n\n### Fixed\n\n" .
              "## '"$VERSION"' - '"$DATE"'\n" .
              ($2 ? "\n### New\n$2" : "") .
              ($4 ? "\n### Changed\n$4" : "") .
              ($6 ? "\n### Fixed\n$6" : "") .
              "\n"
            /se' CHANGES.md

      - name: Commit and tag
        run: |
          git config user.name "github-actions[bot]"
          git config user.email "github-actions[bot]@users.noreply.github.com"
          git add Cargo.toml CHANGES.md
          git commit -m "Release ${{ steps.version.outputs.tag }}"
          git tag -a "${{ steps.version.outputs.tag }}" -m "Release ${{ steps.version.outputs.tag }}"
          git push origin main --tags

  build-binaries:
    name: Build ${{ matrix.target }}
    needs: version
    runs-on: ${{ matrix.os }}
    strategy:
      matrix:
        include:
          - target: x86_64-unknown-linux-musl
            os: ubuntu-latest
            archive: tar.gz
            cross: true
          - target: aarch64-unknown-linux-musl
            os: ubuntu-latest
            archive: tar.gz
            cross: true
          - target: x86_64-apple-darwin
            os: macos-latest
            archive: tar.gz
          - target: aarch64-apple-darwin
            os: macos-latest
            archive: tar.gz
          - target: x86_64-pc-windows-msvc
            os: windows-latest
            archive: zip
    steps:
      - uses: actions/checkout@v4
        with:
          ref: ${{ needs.version.outputs.tag }}

      - name: Install Rust toolchain
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}

      - name: Install cross (Linux musl)
        if: matrix.cross
        run: cargo install cross --git https://github.com/cross-rs/cross

      - name: Build binary
        run: |
          if [ "${{ matrix.cross }}" = "true" ]; then
            RUSTFLAGS="-C target-feature=+crt-static" cross build --release --target ${{ matrix.target }}
          else
            cargo build --release --target ${{ matrix.target }}
          fi
        shell: bash

      - name: Create archive (Unix)
        if: matrix.archive == 'tar.gz'
        run: |
          mkdir -p dist
          BINARY="target/${{ matrix.target }}/release/edaptor"
          ARCHIVE="edaptor-${{ needs.version.outputs.version }}-${{ matrix.target }}.tar.gz"
          mkdir -p staging/edaptor
          cp "$BINARY" staging/edaptor/
          cp -r examples staging/edaptor/ 2>/dev/null || true
          cp README.md staging/edaptor/
          cp LICENSE staging/edaptor/
          tar -czvf "dist/${ARCHIVE}" -C staging edaptor
          echo "ARCHIVE=${ARCHIVE}" >> $GITHUB_ENV
        shell: bash

      - name: Create archive (Windows)
        if: matrix.archive == 'zip'
        run: |
          mkdir dist
          $BINARY = "target/${{ matrix.target }}/release/edaptor.exe"
          $ARCHIVE = "edaptor-${{ needs.version.outputs.version }}-${{ matrix.target }}.zip"
          mkdir staging/edaptor
          Copy-Item $BINARY staging/edaptor/
          Copy-Item -Recurse examples staging/edaptor/ -ErrorAction SilentlyContinue
          Copy-Item README.md staging/edaptor/
          Copy-Item LICENSE staging/edaptor/
          Compress-Archive -Path staging/edaptor -DestinationPath "dist/$ARCHIVE"
          echo "ARCHIVE=$ARCHIVE" >> $env:GITHUB_ENV
        shell: pwsh

      - name: Upload artifact
        uses: actions/upload-artifact@v4
        with:
          name: edaptor-${{ matrix.target }}
          path: dist/*

  create-release:
    name: Create GitHub Release
    needs: [version, build-binaries]
    runs-on: ubuntu-latest
    permissions:
      contents: write
    steps:
      - uses: actions/checkout@v4
        with:
          ref: ${{ needs.version.outputs.tag }}

      - name: Download all artifacts
        uses: actions/download-artifact@v4
        with:
          path: artifacts
          pattern: edaptor-*
          merge-multiple: true

      - name: Extract release notes
        id: changelog
        run: |
          VERSION="${{ needs.version.outputs.version }}"
          NOTES=$(sed -n "/^## ${VERSION}/,/^## [0-9]/p" CHANGES.md | sed '$d')
          echo "$NOTES" > release-notes.md
          echo "Release notes:"
          cat release-notes.md

      - name: List artifacts
        run: ls -la artifacts/

      - name: Create Release
        uses: softprops/action-gh-release@v2
        with:
          tag_name: ${{ needs.version.outputs.tag }}
          name: edaptor ${{ needs.version.outputs.tag }}
          body_path: release-notes.md
          files: artifacts/*
          fail_on_unmatched_files: true
          draft: false
          prerelease: false

  build-docs:
    name: Build Release Documentation
    needs: version
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          ref: ${{ needs.version.outputs.tag }}

      - name: Install mdBook and plugins
        run: |
          mkdir -p ~/bin
          curl -sSL https://github.com/rust-lang/mdBook/releases/download/v0.5.2/mdbook-v0.5.2-x86_64-unknown-linux-gnu.tar.gz | tar -xz -C ~/bin
          curl -sSL https://github.com/badboy/mdbook-mermaid/releases/download/v0.17.0/mdbook-mermaid-v0.17.0-x86_64-unknown-linux-gnu.tar.gz | tar -xz -C ~/bin
          echo "$HOME/bin" >> $GITHUB_PATH

      - name: Setup mermaid assets
        run: mdbook-mermaid install docs

      - name: Build documentation
        working-directory: docs
        run: mdbook build

      - name: Upload docs artifact
        uses: actions/upload-artifact@v4
        with:
          name: docs-release
          path: docs/book
          retention-days: 1

  deploy-docs:
    name: Deploy Release Documentation
    needs: [version, build-docs, create-release]
    runs-on: ubuntu-latest
    permissions:
      contents: read
      pages: write
      id-token: write
    environment:
      name: github-pages
      url: ${{ steps.deployment.outputs.page_url }}
    steps:
      - uses: actions/checkout@v4
        with:
          sparse-checkout: .github/scripts

      - name: Download docs artifact
        uses: actions/download-artifact@v4
        with:
          name: docs-release
          path: new-docs

      - name: Mirror existing GitHub Pages
        run: |
          mkdir -p site
          wget --mirror --no-parent --no-host-directories \
               --directory-prefix=site \
               --reject="index.html?*" \
               --execute robots=off \
               --quiet \
               "$PAGES_URL/" 2>/dev/null || echo "No existing site to mirror"
          find site -name "*.tmp" -delete 2>/dev/null || true
          find site -name "index.html\?*" -delete 2>/dev/null || true
          echo "Existing site contents:"
          ls -la site/edaptor/ 2>/dev/null || echo "(empty or no edaptor dir)"

      - name: Add new version to site
        run: |
          VERSION="${{ needs.version.outputs.tag }}"
          mkdir -p "site/edaptor/$VERSION"
          cp -r new-docs/* "site/edaptor/$VERSION/"
          echo "Added version $VERSION"

      - name: Update versions and redirect
        run: |
          chmod +x .github/scripts/manage-doc-versions.sh
          .github/scripts/manage-doc-versions.sh site/edaptor cull
          .github/scripts/manage-doc-versions.sh site/edaptor update-json
          .github/scripts/manage-doc-versions.sh site/edaptor generate-redirect

      - name: Show final site structure
        run: |
          echo "Final site structure:"
          ls -la site/edaptor/
          echo ""
          echo "versions.json:"
          cat site/edaptor/versions.json

      - name: Setup Pages
        uses: actions/configure-pages@v4

      - name: Upload artifact
        uses: actions/upload-pages-artifact@v3
        with:
          path: site/edaptor

      - name: Deploy to GitHub Pages
        id: deployment
        uses: actions/deploy-pages@v4
```

- [ ] **Step 3: Verify YAML, no container leftovers, correct needs**

Run: `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release.yml')); print('valid yaml')"`
Expected: `valid yaml`.

Run: `grep -niE "byonk|build-container|Dockerfile|docker/|ghcr.io/\\$\\{\\{|fonts|screens|generate-samples" .github/workflows/release.yml || echo "clean"`
Expected: `clean` (no container job, no byonk, no screenshot/fonts/screens). Note `ghcr.io/cross-rs` does NOT appear in this file — that's only in `Cross.toml`, which is fine.

Run: `grep -n "needs: \[version, build-binaries\]" .github/workflows/release.yml`
Expected: matches the `create-release` job (proves `build-container` was dropped from its `needs`).

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/release.yml Cross.toml
git commit -m "$(cat <<'EOF'
ci: add release workflow (cross-compiled binaries + tagged docs)

Ported from byonk minus the container image job; bundles README + LICENSE
+ examples and relies on the rustls backend for clean static musl builds.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 17: Final whole-system verification

**Files:** none (verification only)

- [ ] **Step 1: Full local check matches CI**

Run:
```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test -p edaptor
cargo build
```
Expected: all green; `live_*` tests SKIP without the env var. (If the rustls plan has not landed yet, `cargo test` still passes — these workflows do not depend on it to compile, only the release musl builds benefit.)

- [ ] **Step 2: Docs build clean with no stubs**

Run: `make docs && (grep -rl "filled in by a later task" docs/src && echo "STUBS REMAIN" || echo "no stubs")`
Expected: book builds; prints `no stubs`.

- [ ] **Step 3: All workflow YAML is valid**

Run: `for f in .github/workflows/*.yml; do python3 -c "import yaml,sys; yaml.safe_load(open('$f')); print('$f ok')"; done`
Expected: `ci.yml ok`, `docs.yml ok`, `release.yml ok`.

- [ ] **Step 4: No leftover byonk identifiers anywhere we authored**

Run: `grep -rniE "byonk" .github Makefile mise.toml docs/book.toml docs/theme CHANGES.md examples/config.toml || echo "clean"`
Expected: `clean`.

- [ ] **Step 5: Record handover note**

Note in the handover that the build/docs infra is complete but **inert until `main` is pushed to `oposs/edaptor` and GitHub Pages is enabled** (Pages source = GitHub Actions). The first `release.yml` dispatch should pick the bump type that yields the intended first version number (the workflow overwrites `Cargo.toml`'s version from the tag math).

---

## Self-review (done while writing)

- **Spec coverage:** component 1 Makefile → Task 3; 2 mise.toml → Task 2; 3 mdBook (book.toml, theme, SUMMARY, content) → Tasks 7-12; 4 examples (`config.toml`; `demo-config.toml` already exists) → Task 4; 5 ci.yml → Task 13; 6 docs.yml + `manage-doc-versions.sh` + version-selector theme → Tasks 14, 15, 7; 7 release.yml (minus container) + Cross.toml → Task 16; 8 TLS migration → **separate plan** (`2026-06-04-rustls-tls-migration.md`); 9 CHANGES.md → Task 5; 10 LICENSE + README → Tasks 1, 6; `license = "MIT"` owned here (Task 1) not by the TLS plan. All non-goals respected: no container image (Task 16 removes it), no screenshot pipeline (removed from Makefile/docs.yml/release.yml).
- **Placeholder scan:** no "TBD"/"add X" — mechanical files have exact content or exact `cp`+`sed` commands with grep verification; doc-content tasks specify each page's required sections, the source file/memory to draw facts from, and the concrete mermaid/ASCII to embed. The H1-stub step lists every file and its exact H1.
- **Consistency:** edaptor identifiers (`oposs/edaptor`, `/edaptor/`, `https://oposs.github.io/edaptor`, binary `edaptor`) are uniform across book.toml, version-selector.js, manage-doc-versions.sh, docs.yml, release.yml. The three-pane ASCII sketch is declared authoritative in Task 11 and referenced (not duplicated divergently) by Task 8. `versions.json`/`/edaptor/` path layout is consistent between the script (Task 14) and both deploy jobs (Tasks 15-16). The `## Unreleased` + `### New/Changed/Fixed` shape in CHANGES.md (Task 5) matches the `perl` rewrite in release.yml (Task 16).
