# edaptor — Project Handover

**Date:** 2026-06-04
**`main` HEAD:** `0d4c935` (working tree clean; local-only — not pushed to `origin`)
**Latest landed:** the **`src/ui/app.rs` decomposition** is **done and merged to `main`** (`37385a0…75cfdc8`) — the ~2850-line god-file is now split into focused modules under `src/ui/app/` plus a domain layer (see project memory `edaptor-app-rs-decomposition`).
**Current focus (in progress):** a **build system + documentation** effort, modelled on the `byonk` project. The design spec is **committed** (`0d4c935`) and awaiting the implementation plan; it includes a **TLS migration from `native-tls`/OpenSSL to rustls**. Nothing implemented yet — see "Build system & documentation" below.

`edaptor` is a Rust **ratatui** TUI for administering an OpenLDAP directory (users, groups, group memberships). It derives the directory's structure from live schema introspection (`cn=subschema`) and generates edit forms from `objectClass` definitions; a TOML config declares connection settings plus *entry profiles* ("what a user/group means here").

> **Note:** `README.md`'s "Status" / "Turbo Vision" wording is still **stale** — the UI was migrated off turbo-vision to ratatui 0.30. (A `## Local test server` section was added and the `## Configuration` example is current; only the top "Status" blurb lags.) This refresh is **slated as part of the build-system & docs work** (it includes README touch-ups). The authoritative design is [`docs/superpowers/specs/2026-05-29-edaptor-design.md`](superpowers/specs/2026-05-29-edaptor-design.md).

---

## Milestone status

| Milestone | State |
|---|---|
| M1 Foundation (config, TLS, worker, bind, subschema) | ✅ on `main` |
| M2 Schema model (typed MUST/MAY, syntax→FieldKind) | ✅ |
| M3 TUI shell + generic read tier | ✅ |
| M4 Generic **write** path (diff→ChangeSet→LDIF→Modify/Add/ModRdn/Delete) | ✅ |
| M5 Samba lifecycle (NT hash, SID/RID, group-map, synced password) | ✅ headless + `edaptor passwd <dn>` CLI; password also set inline on create/edit of password-profile entries; no standalone in-TUI "Set Password" action |
| 3-pane redesign · turbo-vision → **ratatui 0.30** · UI polish | ✅ merged |
| **Relation membership picker** (`[[relation]]`, member↔memberOf) | ✅ merged |
| **Rich user templates** (multi-OC profiles, defaults, autonumber, password, `[profile.lookup]` value-lookup, F7 chooser) | ✅ **merged to `main`** (`d98305b`) |
| **Rich provisioned test server + seed data** | ✅ merged to `main` (`ba20f39`…`33cf887`) |
| **Picker UX fixes** (scroll, search-matches-first, multi-value-editor scroll) | ✅ merged to `main` (`cfe6563`, `87e5533`) |
| **Unified configurable picker** (replaces `[[relation]]` + `[profile.lookup]`) | ✅ **implemented + merged to `main`** (`010f37c`); one `[profile.picker.<attr>]` binding, gated live tests for all 4 shapes |
| **`src/ui/app.rs` decomposition** (god-file → `src/ui/app/` modules + domain layer + `Ctx`) | ✅ **merged to `main`** (`37385a0…75cfdc8`) |
| **Build system + documentation** (Makefile, mise, mdBook docs, CI/docs/release workflows, **rustls migration**, MIT license) | 🟡 **specced** (`0d4c935`), plan + implementation pending — see below |
| M6 leftovers (paged-scale lists, result-code→human table polish, SASL EXTERNAL/GSSAPI auth) | ⏳ pending (packaging now covered by the build-system effort) |

---

## Local test server & seed data (this session — on `main`)

`scripts/test-ldap.sh start` now provisions a Bitnami OpenLDAP to match the
`oposs.openldap` ansible role and seeds it with realistic data, so the TUI and
the gated live tests run against a representative directory.

- **Provisioning assets** — `scripts/ldap-provision/`: `schema/{samba,mail}.ldif`
  (loaded via the `cn=config` admin), `config/overlays.ldif`
  (memberof/refint/ppolicy on `{2}mdb`), `data/{ppolicy,base}.ldif`
  (OUs, `sambaDomain`, service accounts), `data/testdata.ldif` (generated,
  committed). `README.md` in that dir documents each file.
- **Generator** — `src/testdata.rs` (pure, deterministic; reuses
  `samba::{nthash,sid,account}`) + `src/bin/gen-testdata.rs`. Produces ~600
  users / 5 departments / ~25 groups. A unit drift-guard
  (`committed_ldif_matches_generator`) fails if the generator changes without
  regenerating the committed LDIF (`cargo run --bin gen-testdata`).
- **Demo config** — `examples/demo-config.toml` (base `dc=example,dc=org`, bind
  `cn=admin`/`adminpassword` via `EDAPTOR_TEST_ADMIN_PW`). Shared user password
  is `test123`.
- **Gated assertions** — `tests/live_seed.rs` (people count, posixGroup
  gidNumbers, sambaDomain discoverable).

**Bitnami gotchas worth knowing (validated):** cn=config writes need
`LDAP_CONFIG_ADMIN_*` (bind `cn=admin,cn=config`); overlay `.so`s live in
`/opt/bitnami/openldap/lib/openldap` (NOT the default module path); Bitnami
auto-creates `ou=groups`, so `base.ldif` deliberately omits it; `apply_ldif`
tolerates only "Already exists (68)" and fails loud on anything else.

**Spec/plan:** [`specs/2026-06-03-test-data-and-features-design.md`](superpowers/specs/2026-06-03-test-data-and-features-design.md) · [`plans/2026-06-03-test-data-and-features.md`](superpowers/plans/2026-06-03-test-data-and-features.md)

---

## Picker UX fixes (this session — on `main`)

The membership/lookup picker popup and the multi-value free-text editor both
rendered rows from index 0 with no scroll offset, so the cursor went off-screen
past the fold. Both now use a sticky `scroll` offset synced to the cursor via
`clamp_scroll` (`PickerState.scroll`, `ValueEditor.scroll`). The picker also now
puts **search matches first** when a term is active (`PickerState.search_active`
flips `visible()` ordering); with no term, selected members lead. Covered by
unit + `TestBackend` render tests in `src/ui/picker.rs` and `src/ui/view.rs`.

---

## Unified configurable picker (merged to `main`, `010f37c`)

Collapsed the three forked field-population mechanisms — `[[relation]]` (DN
membership + `memberOf` fan-out), `[profile.lookup.<attr>]` (single scalar), and
the previously-unbuilt multi-scalar case (`memberUid` storing `uid`) — into
**one** `[profile.picker.<attr>]` binding consumed by one engine. Implemented
TDD/subagent-driven in 10 commits (`55649df…8b56cfd`); spec
[`specs/2026-06-03-unified-picker-design.md`](superpowers/specs/2026-06-03-unified-picker-design.md),
plan [`plans/2026-06-03-unified-picker.md`](superpowers/plans/2026-06-03-unified-picker.md).

**Config surface** — `[profile.picker.<attr>]` on the profile owning the field:
`candidate` (a `[[profile]]` name → search scope), `store` (`"dn"` default, or an
attr name — also the identity key), `select` (`auto`|`single`|`multi`),
`fanout_attr` (synthetic back-ref: field not written; this entry's DN
added/removed in `fanout_attr` on each picked candidate). The demo config
declares four: `member` (group, DN, multi), `gidNumber` (user, scalar, single),
`memberUid` (posixgroup, `uid`, multi), `memberOf` (user, fanout → `member`).

**Where the code lives**
- `src/config/relation.rs` (filename kept; now picker-only) — `PickerSpec`-resolved `PickerBinding`, `StoreKey::{Dn,Attr}`, `Cardinality`, `ResolvedPicker`, `resolve_pickers`, `picker_for`; `CandidateScope` + `scope_of` retained. (`Relation`/`ResolvedRelation`/`LookupSpec` and friends are **deleted**.)
- `src/config/mod.rs` — `EntryProfile.pickers: BTreeMap<String, PickerSpec>`; `PickerSpec`. (`Config.relations`, `EntryProfile.lookups`, `LookupSpec` removed.)
- `src/ui/picker.rs` — `Candidate{dn,label,store_value}`; `PickerState` keyed by **store value** (`key_ci` flag), `selected_values()`/`selected_dns()`.
- `src/ui/edit_form.rs` — `EditField.picker: Option<PickerBinding>`; unified `ValueEditor::open(field, binding)` + `open_plain`; `tag_picker_fields(form, pickers, ocs, read_only)` (fan-out fields force-editable, honoring global read-only); `to_edit_entry` excludes fan-out fields; `fanout_labels()`.
- `src/ui/app.rs` — `App.pickers`; `tag_picker_fields` wired into both form seams (`build_loaded_form`, `build_new_entry_form`); one `open_value_editor` dispatch, one `service_picker_search` (binding-driven), the `Response::Entries` store-value mapping (labels/DNs upgraded by store value), one Alt+S commit (single keeps the cursor fallback), `plan_combined_save` fan-out keyed on `fanout_attr`.
- `src/ui/view.rs` — `render_value_editor` single-vs-multi markers from binding cardinality (passed as `single: bool`).
- `tests/live_membership.rs`, `tests/live_templates.rs` — gated live tests; `live_templates` adds the 4-shape picker coverage (one un-gated config-resolution check).

**Known follow-up (now DONE):** the ~2850-line `src/ui/app.rs` god-file was split into focused modules under `src/ui/app/` plus a domain layer, with co-mutated orchestration state bundled into a `Ctx` struct — merged to `main` (`37385a0…75cfdc8`). See project memory `edaptor-app-rs-decomposition`.

---

## Build system & documentation (specced this session — NOT yet implemented)

A `byonk`-modelled build/docs effort. **Spec committed** at
[`specs/2026-06-04-build-system-and-docs-design.md`](superpowers/specs/2026-06-04-build-system-and-docs-design.md);
implementation plan and code still **pending**. Decisions locked: name stays
`edaptor` (repo `oposs/edaptor`, docs `/edaptor/`), **MIT license**, config
reference is the docs centerpiece, TUI layouts shown as fenced ` ``` ` blocks
(no captured screenshots), versioned GitHub Pages at `oposs.github.io/edaptor`,
**no release container** (TUI client).

Planned deliverables:
- `Makefile` + `mise.toml` (rust + pinned mdbook/mdbook-mermaid).
- mdBook docs under `docs/src/**` (Getting Started · **Configuration** · Concepts
  · Usage · Reference); `docs/book.toml` + ported version-selector theme.
- `examples/config.toml` (annotated reference; `demo-config.toml` already exists).
- `.github/workflows/{ci,docs,release}.yml` + `.github/scripts/manage-doc-versions.sh`.
- `CHANGES.md` (Keep-a-Changelog), `LICENSE` (MIT), README status/license refresh.

**TLS migration (prerequisite, part of this effort):** swap `native-tls`/OpenSSL
→ **rustls** (`ldap3` `tls-rustls-ring`; add `rustls` + `rustls-pemfile`; drop
`native-tls`). Rewrite `src/ldap/tls.rs` `build_settings` to build a
`rustls::ClientConfig` (custom-CA via `RootCertStore` + `set_config`;
`verify=false` via `set_no_tls_verify`, with a self-installed `NoCertVerification`
when a custom CA *and* `verify=false` coexist — confirmed against ldap3 source).
Backend swap only — identical externally-visible semantics. This removes the
static-musl OpenSSL problem entirely (no vendoring needed).

---

## Open items / known gaps

1. **`main` is local-only** — not pushed to `origin` (`git@github.com:oposs/edaptor.git`). The build-system effort wires up CI/docs/release workflows that only take effect once `main` is pushed and GitHub Pages is enabled.
2. **Build system & docs not yet implemented** — spec is committed (`0d4c935`); the next step is `writing-plans` → implementation. Includes the rustls migration and the README "Status"/"Turbo Vision" refresh.
3. **M5 Samba** has no standalone in-TUI "Set Password"/Samba-enable action on arbitrary entries (only inline on create/edit of password-profile entries, plus the `edaptor passwd <dn>` CLI).
4. **M6 leftovers** pending (paged-scale lists, result-code polish, SASL auth). Packaging is now folded into the build-system effort.

---

## How to build / test / run

```bash
# Build + checks (must be green before any commit)
cargo build --all-targets
cargo test -p edaptor                       # ~309 lib tests; live_* tests SKIP without the env var
cargo clippy --all-targets -- -D warnings
cargo fmt --check

# Live / integration tests against the provisioned OpenLDAP (podman)
scripts/test-ldap.sh start                  # provisions schemas/overlays + seeds 600 users/25 groups
export EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389
export EDAPTOR_TEST_ADMIN_PW=adminpassword
cargo test -p edaptor                        # now live_membership/templates/seed/structure/write run
scripts/test-ldap.sh stop

# Explore the seed data in the TUI
cargo run -- --config examples/demo-config.toml
```

For a manual membership smoke: open a group → Enter on `member` → type to search → toggle → Alt+S; then a user → Enter on `memberOf` → toggle a group → Alt+S; try removing a group's last member (expect a clear block).

---

## Conventions (follow these)

- **Facade boundary:** only `src/ui/*` may `use ratatui`/`use tui_*`. Verify: `! grep -rl "use ratatui\|use tui_" src | grep -v "^src/ui/"`.
- **Strict TDD**, atomic commits; crate must compile after every commit; **run `cargo fmt` before commit** (a batch of this session's files were committed unformatted and fixed in `e722902` — don't repeat).
- **Live tests gated** by `EDAPTOR_TEST_LDAP_URI` (skip when unset) — mirror `tests/live_write.rs` / `tests/live_membership.rs`. DN base in tests is `dc=example,dc=org`.
- **Worktrees** live under `/scratch/oetiker/claude-worktrees/` as `<project>-<branch>`.
- **Commit trailer:** `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **Execution style:** subagent-driven (fresh subagent per task + spec-then-quality review); see project memory `prefers-agent-fanout`. App.rs-heavy tasks can exhaust a subagent's context — scope tightly or resolve in-session.
