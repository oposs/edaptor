# edaptor — Session Handover

Carries the **current session's concerns** into the next session. Not a project
history — for that, see git log, the specs under `docs/superpowers/specs/`, and
project memory (`…/memory/MEMORY.md`).

**Date:** 2026-06-04 · **`main` HEAD:** `0d4c935` · working tree clean ·
`main` is **local-only** (not pushed to `git@github.com:oposs/edaptor.git`).

`edaptor` is a Rust **ratatui** TUI for administering an OpenLDAP directory
(users, groups, memberships). It introspects live schema (`cn=subschema`) and
generates edit forms from `objectClass` definitions; a TOML config declares
connection settings plus *entry profiles* ("what a user/group means here").
All core milestones (M1–M5, the 3-pane/ratatui redesign, the unified
`[profile.picker.<attr>]`, the test server, and the `src/ui/app.rs`
decomposition) are **done and on `main`** — details in git and memory.

---

## In flight: build system & documentation

The active work. **Design spec committed**
([`specs/2026-06-04-build-system-and-docs-design.md`](superpowers/specs/2026-06-04-build-system-and-docs-design.md));
**nothing implemented yet** — next step is `writing-plans` → implementation.

Locked decisions: name stays `edaptor`, **MIT license**, config reference is the
docs centerpiece, TUI layouts as fenced ` ``` ` blocks (no screenshots),
versioned GitHub Pages at `oposs.github.io/edaptor`, no release container.

Planned deliverables: `Makefile` + `mise.toml`; mdBook docs (`docs/book.toml`,
`docs/src/**`) + ported version-selector theme; `examples/config.toml` (annotated
reference); `.github/workflows/{ci,docs,release}.yml` +
`.github/scripts/manage-doc-versions.sh`; `CHANGES.md`; `LICENSE`; README
status/license refresh.

**Bundled prerequisite — TLS migration to rustls:** swap `native-tls`/OpenSSL →
rustls (`ldap3` `tls-rustls-ring`; add `rustls` + `rustls-pemfile`; drop
`native-tls`). Rewrite `src/ldap/tls.rs` `build_settings` to build a
`rustls::ClientConfig` — custom CA via `RootCertStore` + `set_config`;
`verify=false` via `set_no_tls_verify`, self-installing a `NoCertVerification`
only when a custom CA *and* `verify=false` coexist (confirmed against ldap3
source). Backend swap, identical semantics; removes the static-musl OpenSSL
problem (no vendoring).

---

## Open gaps (carry forward)

1. **`main` is local-only.** The CI/docs/release workflows only take effect once
   `main` is pushed and GitHub Pages is enabled.
2. **README "Status" / "Turbo Vision" blurb is stale** — the UI is ratatui 0.30,
   not turbo-vision. Refresh is folded into the build-system work.
3. **M5 Samba**: no standalone in-TUI "Set Password"/Samba-enable action on
   arbitrary entries (only inline on create/edit of password-profile entries +
   the `edaptor passwd <dn>` CLI).
4. **M6 leftovers**: paged-scale lists, result-code→human polish, SASL
   EXTERNAL/GSSAPI auth.

---

## Build / test / run

```bash
cargo build --all-targets
cargo test -p edaptor                 # ~309 lib tests; live_* tests SKIP without the env var
cargo clippy --all-targets -- -D warnings
cargo fmt --check

# Live tests against the provisioned OpenLDAP (podman)
scripts/test-ldap.sh start            # schemas/overlays + ~600 users / ~25 groups
export EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389
export EDAPTOR_TEST_ADMIN_PW=adminpassword
cargo test -p edaptor                 # now live_membership/templates/seed/structure/write run
scripts/test-ldap.sh stop

# Explore the seed data in the TUI (shared user password: test123)
cargo run -- --config examples/demo-config.toml
```

---

## Conventions (follow these)

- **Facade boundary:** only `src/ui/*` may `use ratatui`/`use tui_*`. Verify:
  `! grep -rl "use ratatui\|use tui_" src | grep -v "^src/ui/"`.
- **Strict TDD**, atomic commits; crate must compile after every commit;
  **`cargo fmt` before every commit**.
- **Live tests gated** by `EDAPTOR_TEST_LDAP_URI` (skip when unset) — mirror
  `tests/live_write.rs` / `tests/live_membership.rs`. DN base is `dc=example,dc=org`.
- **Worktrees** under `/scratch/oetiker/claude-worktrees/` as `<project>-<branch>`.
- **Commit trailer:** `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **Execution style:** subagent-driven (fresh subagent per task + spec-then-quality
  review); see memory `prefers-agent-fanout`.
