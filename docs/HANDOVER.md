# edaptor — Session Handover

Carries the **current session's concerns** into the next session. Not a project
history — for that, see git log, the specs under `docs/superpowers/specs/`, and
project memory (`…/memory/MEMORY.md`).

**Date:** 2026-06-08 · **`main` HEAD:** `25a86b1` · **0.2.0 is released**
(tagged `b31fd65`). `main` is **local-only** (origin
`git@github.com:oposs/edaptor.git` exists; `origin/main` is behind and **not
pushed**).

`edaptor` is a Rust **ratatui** TUI for administering an OpenLDAP directory. It
introspects live schema (`cn=subschema`) and generates edit forms from
`objectClass` definitions; a TOML config declares connection settings plus
*entry profiles*. Earlier milestones (M1–M5, the 3-pane/ratatui redesign,
rustls, the test server, the `app.rs` decomposition) and the **widget palette**
(`[profile.widget.<attr>]` `kind = "choice"` and `"password"`) are **done and on
`main`** — details in git, the specs, and memory.

The project is converging on **one configurable-fields concept: the widget
palette.** `choice` and `password` are in; the next step folds the *picker*
system in too (spec written — see below).

---

## Done this session: docs + the picker-widget design

No code shipped this session — documentation cleanup plus the next design.

1. **Widget palette → one doc home** (`e139e4f`). `configuration/widgets.md` now
   opens with a *concept* section (the `[profile.widget.<attr>]` palette + the
   `kind` discriminator + a kinds table) and hosts both `choice` and `password`.
   Removed the stale "the only implemented kind is choice" line; **deleted
   `configuration/passwords.md`** (folded into widgets.md) and repointed every
   link (SUMMARY, overview orientation map, object-model, usage). Reworded
   "inline password field" → "set-password popup" throughout. `mdbook build`
   clean, no broken links.
2. **`CHANGES.md` Unreleased backfilled** (`e139e4f`) — 0.2.0 shipped the choice
   + password widgets and the security fixes but its changelog only listed DIT
   tree labels; the **Unreleased** section now documents the widget palette, the
   `[profile.password]` removal / read-only hash fields / TLS requirement, and
   the permanently-dirty + cleartext-preview fixes. **User plans to cut 0.2.1**
   with these.
3. **Picker-widget SPEC written** (`25a86b1`), **NOT implemented**. Fold
   `[profile.picker.<attr>]` into the palette as two kinds:
   - **`kind = "picker"`** — store picked value(s) in *this* entry (`candidate`,
     `store`, `select`). Covers `gidNumber`, `member`, `memberUid`.
   - **`kind = "membership"`** — fan *this* entry's DN into a back-ref attr on
     each picked candidate (`candidate`, `via`; always multi). Covers `memberOf`.
   - `candidate` may be a profile-name string **or** an inline scope table
     (`{ base, object_classes, search_attrs, label }`).
   - Engine (live search, fan-out, combined-save) **unchanged** — both kinds
     resolve into the existing `PickerBinding`/`CandidateScope`; only the config
     front-end + storage location change. Clean removal of `[profile.picker]` /
     `PickerSpec` / `App.pickers` / `EditField.picker` / `resolve_pickers` /
     `picker_for` / `tag_picker_fields` (no back-compat).
   - Spec: `docs/superpowers/specs/2026-06-08-picker-widget-design.md`.

**NEXT STEP:** write the implementation plan for the picker widget
(writing-plans) and run it subagent-driven, on a branch.

---

## Open gaps (carry forward)

1. **Picker-widget plan + implementation pending** (spec done, approved). It
   touches the working membership/picker engine's *callers* (binding now lives in
   `EditField.widget_binding`'s `Picker` arm, not `EditField.picker`) — engine
   behavior is preserved, but the rewiring is broad; review carefully.
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
