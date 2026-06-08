# edaptor — Session Handover

Carries the **current session's concerns** into the next session. Not a project
history — for that, see git log, the specs under `docs/superpowers/specs/`, and
project memory (`…/memory/MEMORY.md`).

**Date:** 2026-06-08 · **`main` HEAD:** `78218f3` · **0.2.0 is released**
(tagged `b31fd65`; 0.2.1 not yet cut). `main` is **local-only** (origin
`git@github.com:oposs/edaptor.git` exists; `origin/main` is behind and **not
pushed**).

`edaptor` is a Rust **ratatui** TUI for administering an OpenLDAP directory. It
introspects live schema (`cn=subschema`) and generates edit forms from
`objectClass` definitions; a TOML config declares connection settings plus
*entry profiles*. Earlier milestones (M1–M5, the 3-pane/ratatui redesign,
rustls, the test server, the `app.rs` decomposition) and the **widget palette**
(`[profile.widget.<attr>]` `kind = "choice"`, `"password"`, `"picker"`,
`"membership"`) are **done and on `main`** — details in git, the specs, and memory.

The project has reached **one configurable-fields concept: the widget palette.**
All four kinds are in; the picker/membership fold-in completed this session.

---

## Done this session: picker/membership widgets (implemented + merged)

Folded the old `[profile.picker.<attr>]` system into the widget palette and
**merged to `main`** (merge `78218f3`). Plan-driven, subagent-driven, 9 commits.

1. **Two new palette kinds** — `kind = "picker"` (store picked value(s) in this
   entry: `candidate`, `store`, `select`; covers `gidNumber`/`member`/`memberUid`)
   and `kind = "membership"` (fan this entry's DN into back-ref attr `via` on each
   picked candidate; always multi; covers `memberOf`). `candidate` is a
   `[[profile]]` name **or** an inline `{ base, object_classes, search_attrs?,
   label? }` table (`CandidateRef`, `#[serde(untagged)]`).
2. **Engine unchanged.** Both kinds resolve (in `config::widget::resolve_widgets`)
   into the existing `PickerBinding`/`CandidateScope` as `WidgetKind::Picker(_)`;
   live search / fan-out / combined-save are behavior-preserved.
3. **Clean removal** (no back-compat): `[profile.picker]`, `PickerSpec`,
   `EntryProfile.pickers`, `resolve_pickers`, `picker_for`, `tag_picker_fields`,
   `ResolvedPicker`, `App.pickers`, `EditField.picker`. `EditField` carries only
   `widget_binding`; every read site reads the `Picker` arm. `examples/*.toml`
   migrated; `configuration/pickers.md` folded into `widgets.md`.
4. **Verified.** 376 lib + 7 live tests green; clippy/fmt clean. Live TUI smoke:
   `gidNumber` picker opens from `widget_binding`, candidate search returns
   posixGroups with the current one radio-marked. `CHANGES.md` Unreleased updated.
5. Spec `docs/superpowers/specs/2026-06-08-picker-widget-design.md`; plan
   `docs/superpowers/plans/2026-06-08-picker-widget.md`. Memory:
   `edaptor-picker-widget-merged`.

**NEXT STEP:** the configurable-field palette is complete. Candidate next work:
**cut 0.2.1** (the Unreleased changelog now covers choice/password/picker/
membership + security fixes); or pick up M6 leftovers / the remaining hardcoded
attribute handlers as future palette kinds (see Open gaps).

---

## Open gaps (carry forward)

1. **0.2.1 not yet cut.** The `CHANGES.md` Unreleased section now covers the full
   widget palette (choice/password/picker/membership), the `[profile.password]` /
   `[profile.picker]` removals, and the security fixes — ready to tag whenever.
2. **TLS positive-path live smoke (password widget) still deferred.** Negative
   path was live-verified on plain `ldap://` (password fields → "requires an
   encrypted connection"). The positive path (set a password over TLS, confirm
   `userPassword`+`sambaNTPassword`+`sambaPwdLastSet` update) is unit-tested only;
   it needs an encrypted endpoint (StartTLS/LDAPS on the Bitnami container, or
   point demo-config at `ldaps://`).
3. **Dead code:** `workflows::create::profile_for_entry` is unused in production
   (only its tests call it; kept `pub`). The picker work will revisit this area —
   remove it then, or document as kept API.
4. **Bogus test data:** seed user `jsmith` has `sambaNTPassword: myfunnysambapw`
   (cleartext in a hash field). Provisioning has non-hash samba password values;
   consider regenerating.
5. **`main` is local-only** — CI/docs/release workflows take effect only once
   `main` is pushed and GitHub Pages is enabled (source = GitHub Actions).
6. **Stale design spec:** `specs/2026-06-01-three-pane-layout-design.md` is
   Turbo-Vision-era and does NOT match the shipped ratatui UI.
7. **M6 leftovers:** paged-scale lists, result-code→human polish, SASL
   EXTERNAL/GSSAPI auth.
8. **Remaining hardcoded attribute handlers** (not yet palette kinds): boolean
   checkbox (read-only today — not even editable), binary `<N bytes>`,
   GeneralizedTime, X-ORDERED. Future palette kinds (`boolean`, `date`, …); the
   choice spec reserved `bitmask`/`delimited` formats.

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

# Explore in the TUI (binary is `edaptor`; container often already up)
EDAPTOR_TEST_ADMIN_PW=adminpassword \
  cargo run -j4 --bin edaptor -- --config examples/demo-config.toml
scripts/test-ldap.sh stop

# Docs site (mdbook via mise; book/ is gitignored)
( cd docs && mdbook build )            # clean build = no broken links
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
- **Widget palette = the one config-driven "rich field" home.** New per-attribute
  field behavior should be a `[profile.widget.<attr>]` `kind`, resolved in
  `config::widget` into a `WidgetKind`, tagged onto `EditField.widget_binding`.
- **Strict TDD**, atomic commits; crate must compile after every commit;
  **`cargo fmt` before every commit**; clippy clean (`--all-targets`).
- **Live tests gated** by `EDAPTOR_TEST_LDAP_URI` (skip when unset). DN base
  `dc=example,dc=org`.
- **Docs are one-home:** a config feature is documented as a section of
  `configuration/widgets.md`, linked from the `overview.md` orientation map; no
  separate per-feature config page.
- **No back-compat constraints** — there is no userbase; remove/replace cleanly,
  no deprecation aliases.
- **Commit trailer:** `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- **Execution style:** subagent-driven (fresh subagent per task + spec-then-quality
  review); see memory `prefers-agent-fanout`.
