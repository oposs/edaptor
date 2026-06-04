# edaptor — Session Handover

Carries the **current session's concerns** into the next session. Not a project
history — for that, see git log, the specs under `docs/superpowers/specs/`, and
project memory (`…/memory/MEMORY.md`).

**Date:** 2026-06-04 · **`main` HEAD:** `1529b83` · working tree clean ·
`main` is **local-only** (not pushed to `git@github.com:oposs/edaptor.git`).

`edaptor` is a Rust **ratatui** TUI for administering an OpenLDAP directory
(users, groups, memberships). It introspects live schema (`cn=subschema`) and
generates edit forms from `objectClass` definitions; a TOML config declares
connection settings plus *entry profiles* ("what a user/group means here").
All core milestones (M1–M5, the 3-pane/ratatui redesign, the unified
`[profile.picker.<attr>]`, the test server, and the `src/ui/app.rs`
decomposition) are **done and on `main`** — details in git and memory.

---

## Done this session: build system & documentation + rustls

Implemented from two plans (`plans/2026-06-04-rustls-tls-migration.md`,
`plans/2026-06-04-build-system-and-docs.md`), subagent-driven with per-task +
final review, **merged to `main`** (20 commits, `c0afb42`…`1529b83`). All green:
`cargo fmt`/`clippy`, **335 tests**, `mdbook build` clean (no stubs), 3 workflow
YAMLs valid, no `byonk` leftovers.

- **rustls migration** (`src/ldap/tls.rs`, `Cargo.toml`): `native-tls`/OpenSSL →
  `ldap3` `tls-rustls-ring` + `rustls` (`default-features = false`) +
  `rustls-pemfile`. Custom CA → `RootCertStore` + `set_config`; `verify=false` →
  `set_no_tls_verify` (authoritatively subsumes any CA — simpler than the spec's
  verifier-injection, identical observable semantics). **OpenSSL is gone from the
  tree** (`cargo tree -i openssl-sys` empty). Crypto is now `ring` (see note below).
- **Build/docs:** `Makefile`, `mise.toml` (needs `exe = "mdbook"`), `LICENSE`
  (MIT), `CHANGES.md`, `examples/config.toml`, mdBook site under `docs/` (config
  reference is the centerpiece; 19 pages; version-selector theme), README refresh,
  and `.github/workflows/{ci,docs,release}.yml` + `manage-doc-versions.sh` +
  `Cross.toml`. Release workflow drops byonk's container job; docs workflow drops
  the screenshot/backfill steps. Identifiers use `oposs/edaptor` + `/edaptor/`.

**`ring` brings a small bundled C/asm core** (the `cc` build processes): edaptor's
own code is pure Rust, and there is **no external/system OpenSSL and no vendoring**
(the whole point), but `rustls`'s `ring` provider compiles its bundled C internally
during `cargo build`. ldap3 0.12 only offers `ring` or `aws-lc-rs` (both have C) —
there is no pure-Rust crypto option through ldap3's features. `ring` is the more
musl-portable of the two; the `aarch64-unknown-linux-musl` release build is the one
to watch on the first `release.yml` dispatch.

Deferred: rustls plan **Task 3 (live `ldaps://` smoke)** — the podman test server
is mostly plaintext `ldap://`, so its TLS value is limited; the custom-CA path is
covered by the `builds_settings_with_valid_custom_ca` unit test.

---

## Open gaps (carry forward)

1. **`main` is local-only.** The CI/docs/release workflows only take effect once
   `main` is pushed and GitHub Pages is enabled (Pages source = GitHub Actions).
2. **Stale design spec:** `specs/2026-06-01-three-pane-layout-design.md` is
   Turbo-Vision-era (F-keys, frameless/draggable panes) and does **not** match the
   shipped ratatui UI (Alt-keys Alt+R/N/D/S/C/X + Tab/Shift-Tab, bordered panes,
   fixed 26/28/46% dividers, single status line). The new docs were written from
   `src/ui/` code, not that spec — but the spec itself is misleading if reused.
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
