# edaptor — Session Handover

Carries the **current session's concerns** into the next session. Not a project
history — for that, see git log, the specs under `docs/superpowers/specs/`, and
project memory (`…/memory/MEMORY.md`).

**Date:** 2026-06-07 · **`main` HEAD:** `0d16757` ·
**active branch:** `fix-secret-fields-readonly` @ `dd0f9d5` (3 commits ahead of
`main`, **NOT merged**) · `main` is **local-only** (origin
`git@github.com:oposs/edaptor.git` exists but is not pushed).

`edaptor` is a Rust **ratatui** TUI for administering an OpenLDAP directory
(users, groups, memberships). It introspects live schema (`cn=subschema`) and
generates edit forms from `objectClass` definitions; a TOML config declares
connection settings plus *entry profiles*. All earlier milestones (M1–M5, the
3-pane/ratatui redesign, the unified `[profile.picker.<attr>]`, rustls, the test
server, the `app.rs` decomposition) are **done and on `main`** — details in git
and memory.

---

## Done this session

### 1. Password permanently-dirty bug — FIXED, on `main` (`35ab25e`)
Every `ou=people` (password-profile) entry showed dirty the instant it loaded,
popping the Save/Discard/Stay guard on every navigation. Cause: `build_edit_form`
recorded the stored `userPassword` hash in the baseline, then
`inject_password_fields` re-added a blank field → `is_dirty()` saw a delete. Fix:
strip the password attr from the baseline at injection (mirrors
`stage_edit_password`). The user's original report.

### 2. Choice widget feature — DONE, merged to `main` (`…`→`0d16757`)
Generic config-driven **`[profile.widget.<attr>]` `kind="choice"`** widget: pick
from a fixed vocabulary ↔ (de)serialize one attribute string. Wired for
`sambaAcctFlags` (multi, bracketed samba letters, lossless `U`/`W`/`S`/`I`
preservation) and `loginShell` (single, plain). Enum palette (no trait/registry);
mirrors the picker `resolve → App.widgets → tag_widget_fields` pipeline. Pure
token logic in `src/config/widget.rs`; bracketed format in `src/samba/account.rs`
(`parse_bracketed`/`serialize_bracketed`, canonical `N D H T U M W S L X I`).
Presets in `examples/demo-config.toml`. **Live-verified** end-to-end against the
podman LDAP. Built subagent-driven (8 tasks + review). See spec/plan dated
2026-06-05 and memory `edaptor-choice-widget`.

### 3. Password/hash fields read-only + masked preview — on branch (`e961892`)
Security fix: `sambaNTPassword`/`sambaLMPassword` were editable text — a direct
edit was written **verbatim (no NT-hash)** and **leaked in cleartext** in the
save-confirm preview. Now `field_is_editable` returns false for `is_secret_attr`
attrs (display-only), and `mask_changeset_secrets` masks any `is_secret_attr`
(defence in depth). `is_secret_attr` **relocated to `form::changeset`** so
`workflows::save` can use it without a `workflows → ui` import. Live-verified the
field is now inert to typing.

### 4. Password widget — SPEC + PLAN written, on branch (`0b87689`, `dd0f9d5`) — NOT implemented
Reaction to the read-only change ("editing the samba pw field does nothing —
odd"). Design: passwords become a **`[profile.widget.<attr>] kind="password"`**
widget; Enter on the primary or any derived hash field opens **one** TLS-gated
New+Confirm popup (`Overlay::PasswordEditor`) that updates `userPassword` +
(samba) `sambaNTPassword` + `sambaPwdLastSet` in one save. New value staged in
`EditForm.pending_password`. `[profile.password]`/`PasswordSpec`/
`inject_password_fields` **removed outright** (no userbase → no back-compat).
Hard refuse on non-TLS connections. Shared refactor: `ResolvedWidget` gains a
`WidgetKind { Choice | Password }` enum; `EditField.widget_choice` →
`widget_binding: Option<WidgetKind>`.
- Spec: `docs/superpowers/specs/2026-06-07-password-widget-design.md`
- Plan: `docs/superpowers/plans/2026-06-07-password-widget.md` (11 TDD tasks)

**NEXT STEP:** execute that plan (subagent-driven). Then decide how to integrate
the whole `fix-secret-fields-readonly` branch (security fix + password widget)
into `main`.

---

## Open gaps (carry forward)

1. **`fix-secret-fields-readonly` is unmerged** — it carries the read-only
   security fix (#3, green: **362 lib tests**) plus the unimplemented password-
   widget design (#4). Implement the password widget on this branch, then merge.
2. **Password-widget plan pending implementation** (Task list above). The
   `WidgetKind` enum refactor touches the working choice code (Tasks 3+4 are one
   commit); Task 9 removal touches many call sites/tests — review carefully.
3. **TLS for the live password smoke.** The password widget **hard-refuses**
   non-encrypted connections; the podman test server is plain `ldap://`
   (`start_tls=false`), so the popup's *positive* path needs an encrypted
   endpoint (enable StartTLS/LDAPS on the container, or point demo-config at
   `ldaps://`). The *negative* path (Enter → "requires encrypted connection"
   error) is always testable on plain ldap.
4. **M5 "set password on arbitrary entry"** — being addressed by the password
   widget: Enter on any password field of a password-profile entry opens the
   popup (edit + create). A standalone action on non-profile entries is still
   only the `edaptor passwd <dn>` CLI.
5. **Bogus test data:** the seed user `jsmith` has `sambaNTPassword: myfunnysambapw`
   (cleartext in a hash field — a live example of the footgun #3 prevents). The
   provisioning data has non-hash samba password values; consider regenerating.
6. **`main` is local-only** — CI/docs/release workflows only take effect once
   `main` is pushed and GitHub Pages is enabled (source = GitHub Actions).
7. **Stale design spec:** `specs/2026-06-01-three-pane-layout-design.md` is
   Turbo-Vision-era and does NOT match the shipped ratatui UI — misleading if
   reused.
8. **M6 leftovers:** paged-scale lists, result-code→human polish, SASL
   EXTERNAL/GSSAPI auth.

---

## Build / test / run

**⚠ Cap parallelism at 4 cores** (shared 128-core box): `cargo build -j4`,
`cargo test -j4`, `cargo clippy -j4 …`.

```bash
cargo build -j4 --all-targets
cargo test -j4 -p edaptor              # 362 lib tests on the branch; live_* SKIP without the env var
cargo clippy -j4 --all-targets -- -D warnings
cargo fmt --check

# Live tests / TUI against the provisioned OpenLDAP (podman)
scripts/test-ldap.sh start             # schemas/overlays + ~600 users / ~25 groups
export EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389
export EDAPTOR_TEST_ADMIN_PW=adminpassword
cargo test -j4 -p edaptor              # live_* now run

# Explore in the TUI (the binary is `edaptor`; container often already up)
EDAPTOR_TEST_ADMIN_PW=adminpassword \
  cargo run -j4 --bin edaptor -- --config examples/demo-config.toml
scripts/test-ldap.sh stop
```
TUI smoke in tmux: warm up the shell + `mise trust` before sending the launch
keys; do **not** `pkill -f edaptor` (matches and kills the LDAP container) — quit
via Alt+X / `tmux kill-session`. See memory `edaptor-tui-debug-gotchas`.

---

## Conventions (follow these)

- **Facade boundary:** only `src/ui/*` may `use ratatui`/`use tui_*`. Verify:
  `! grep -rl "use ratatui\|use tui_" src | grep -v "^src/ui/"`.
- **`form` is the pure domain layer** (`changeset`, `validate`, now also
  `is_secret_attr`/`is_x_ordered`): must NOT import `ui`. `ui` and `workflows`
  both depend on `form`.
- **Strict TDD**, atomic commits; crate must compile after every commit;
  **`cargo fmt` before every commit**; clippy clean (`--tests` too).
- **Live tests gated** by `EDAPTOR_TEST_LDAP_URI` (skip when unset). DN base
  `dc=example,dc=org`.
- **Worktrees** under `/scratch/oetiker/claude-worktrees/` as `<project>-<branch>`,
  entered via the native `EnterWorktree` tool (`path` param).
- **Commit trailer:** `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- **Execution style:** subagent-driven (fresh subagent per task + spec-then-quality
  review); see memory `prefers-agent-fanout`.
