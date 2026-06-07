# edaptor — Session Handover

Carries the **current session's concerns** into the next session. Not a project
history — for that, see git log, the specs under `docs/superpowers/specs/`, and
project memory (`…/memory/MEMORY.md`).

**Date:** 2026-06-08 · **`main` HEAD:** `de7ad9c` · branch
`fix-secret-fields-readonly` was **fast-forward merged into `main` and deleted**.
`main` is **local-only** (origin `git@github.com:oposs/edaptor.git` exists;
`origin/main` is behind and **not pushed**).

`edaptor` is a Rust **ratatui** TUI for administering an OpenLDAP directory
(users, groups, memberships). It introspects live schema (`cn=subschema`) and
generates edit forms from `objectClass` definitions; a TOML config declares
connection settings plus *entry profiles*. All earlier milestones (M1–M5, the
3-pane/ratatui redesign, the unified `[profile.picker.<attr>]`, rustls, the test
server, the `app.rs` decomposition, the choice widget) are **done and on `main`**
— details in git and memory.

---

## Done this session

### Password widget — DONE, merged to `main` (`e961892`→`de7ad9c`)
Executed the 11-task TDD plan (`docs/superpowers/plans/2026-06-07-password-widget.md`)
subagent-driven, plus a final whole-feature integration review.

Passwords are now a **`[profile.widget.<attr>] kind="password"`** widget (sibling
of `kind="choice"`). Enter on the primary (`userPassword`) **or** any derived
field (`sambaNTPassword`/`sambaPwdLastSet` when `samba=true`) opens
**`Overlay::PasswordEditor`** — a TLS-gated New+Confirm popup that stages cleartext
in `EditForm.pending_password`. Save (`prepare_edit_save` **and** the combined
membership path `plan_combined_save`) and create (`fold_create_password`) derive
the mods via `samba::password::password_add_attrs`, strip primary+derived from the
plain diff, and **mask the preview**. **Hard-refuses non-encrypted connections**
(`Config::is_encrypted` = `ldaps://` or `start_tls`, cached on
`App.connection_encrypted`).

Shared refactor: `WidgetKind { Choice(ChoiceWidget) | Password(PasswordWidget) }`;
`EditField.widget_choice` → `widget_binding: Option<WidgetKind>`;
`EditForm.pending_password`. The old `[profile.password]` / `PasswordSpec` /
`inject_password_fields` / `stage_edit_password` machinery was **deleted outright**
(no userbase → clean break); example configs + docs migrated.

Final integration review caught + fixed one real bug: `revert_form` (Alt+C /
guard Discard→Focus) didn't clear `pending_password`, leaving the form perpetually
dirty and able to apply a discarded password on a later save (`de7ad9c`).

**State:** 376 lib tests green, clippy clean (`-D warnings`), fmt clean. See
spec/plan dated 2026-06-07 and memory `edaptor-password-widget`.

---

## Open gaps (carry forward)

1. **TLS positive-path live smoke is the only deferred test.** The negative path
   was live-verified against the plain-`ldap://` podman server (Enter on
   `userPassword` AND `sambaNTPassword` → "requires an encrypted connection" Error
   overlay). The **positive path** (actually setting a password over TLS and
   confirming `userPassword`+`sambaNTPassword`+`sambaPwdLastSet` update) is covered
   by unit tests only — it needs an encrypted endpoint (enable StartTLS/LDAPS on
   the Bitnami container, or point demo-config at `ldaps://`). The test server is
   plain `ldap://` (`start_tls=false`).
2. **Dead code:** `workflows::create::profile_for_entry` is now unused in
   production (only its own tests call it; kept `pub` so no warning). Remove it (and
   `profile_for_entry_where` if then orphaned) or document it as kept API.
3. **`main` is local-only** — CI/docs/release workflows only take effect once
   `main` is pushed and GitHub Pages is enabled (source = GitHub Actions).
4. **Bogus test data:** the seed user `jsmith` has `sambaNTPassword: myfunnysambapw`
   (cleartext in a hash field). The provisioning data has non-hash samba password
   values; consider regenerating.
5. **Stale design spec:** `specs/2026-06-01-three-pane-layout-design.md` is
   Turbo-Vision-era and does NOT match the shipped ratatui UI — misleading if
   reused.
6. **M6 leftovers:** paged-scale lists, result-code→human polish, SASL
   EXTERNAL/GSSAPI auth.

---

## Build / test / run

**⚠ Cap parallelism at 4 cores** (shared 128-core box): `cargo build -j4`,
`cargo test -j4`, `cargo clippy -j4 …`. (Cargo target dir is
`/home/oetiker/scratch/cargo-target` — the binary is NOT under `./target`.)

```bash
cargo build -j4 --all-targets
cargo test -j4 -p edaptor              # 376 lib tests; live_* SKIP without the env var
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
TUI smoke in tmux: warm up the shell ~2s before `send-keys`; poll
`capture-pane -p | grep -q 'DIT'` for the draw; quit via **Alt+X** — do **NOT**
`pkill -f edaptor` (matches and kills the LDAP container). See memory
`edaptor-tui-debug-gotchas`.

---

## Conventions (follow these)

- **Facade boundary:** only `src/ui/*` may `use ratatui`/`use tui_*`. Verify:
  `! grep -rl "use ratatui\|use tui_" src | grep -v "^src/ui/"`.
- **`form` is the pure domain layer** (`changeset`, `validate`, `is_secret_attr`):
  must NOT import `ui`. `ui` and `workflows` both depend on `form`.
- **Strict TDD**, atomic commits; crate must compile after every commit;
  **`cargo fmt` before every commit**; clippy clean (`--tests`/`--all-targets` too).
- **Live tests gated** by `EDAPTOR_TEST_LDAP_URI` (skip when unset). DN base
  `dc=example,dc=org`.
- **Commit trailer:** `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- **Execution style:** subagent-driven (fresh subagent per task + spec-then-quality
  review); see memory `prefers-agent-fanout`.
