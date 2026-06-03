# edaptor — Project Handover

**Date:** 2026-06-03
**`main` HEAD:** `e722902` (working tree clean; local-only — not pushed to `origin`)
**In-progress branch:** `feat-unified-picker` (off `main`) — **design only**: the *unified configurable picker* spec is committed; no implementation yet. See "In progress" below.

`edaptor` is a Rust **ratatui** TUI for administering an OpenLDAP directory (users, groups, group memberships). It derives the directory's structure from live schema introspection (`cn=subschema`) and generates edit forms from `objectClass` definitions; a TOML config declares connection settings plus *entry profiles* ("what a user/group means here").

> **Note:** `README.md`'s "Status" / "Turbo Vision" wording is still **stale** — the UI was migrated off turbo-vision to ratatui 0.30. (A `## Local test server` section was added this session and the `## Configuration` example is current; only the top "Status" blurb lags.) The authoritative design is [`docs/superpowers/specs/2026-05-29-edaptor-design.md`](superpowers/specs/2026-05-29-edaptor-design.md).

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
| **Unified configurable picker** (replaces `[[relation]]` + `[profile.lookup]`) | 🚧 **design approved, spec written**; implementation pending on `feat-unified-picker` |
| M6 leftovers (paged-scale lists, result-code→human table polish, SASL EXTERNAL/GSSAPI auth, packaging) | ⏳ pending |

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

## In progress: unified configurable picker (`feat-unified-picker`)

**Design approved; spec committed; NOT implemented.** Collapses the three forked
field-population mechanisms — `[[relation]]` (DN membership + memberOf fan-out)
and `[profile.lookup.<attr>]` (single scalar) — plus the would-be multi-scalar
case (`memberUid` storing `uid`) into **one** `[profile.picker.<attr>]` binding:

- `candidate` (profile name → search scope), `store` (`"dn"` default, or an attr
  name — also the identity key), `select` (`auto`|`single`|`multi`),
  `fanout_attr` (synthetic back-ref like `memberOf` → writes `member` on each
  picked group). One internal `PickerBinding`; `PickerState` keys by store value
  instead of always-DN; one `open`/search/commit path. Clean cut (no back-compat
  for `[[relation]]`/`lookup`); demo config + README rewritten; a `posixgroup`
  profile added so `memberUid` becomes a multi-select user picker.

**Spec:** [`specs/2026-06-03-unified-picker-design.md`](superpowers/specs/2026-06-03-unified-picker-design.md) (on the branch). **Next step:** writing-plans → implementation. **When it lands it supersedes the "Membership picker" architecture below** (`relation.rs`, `LookupSpec`).

---

## Architecture: the picker today (current `main`; to be unified)

> The relation membership picker and value-lookup are the **current** `main`
> code; the unified-picker branch will replace them. Kept here until that lands.

**Relation membership picker** — symmetric group↔user editing as a *picker mode*
of the multi-value value-editor, driven by one `[[relation]]` block. Forward
(group `member`): Enter opens a searchable user picker; commit writes the group.
Reverse (user `memberOf`): Enter opens a group picker; save **fans out** `member`
MODIFYs across affected groups (last-member pre-validation; partial-failure
report); `memberOf` itself is never written (overlay-maintained).

**Value-lookup** (`[profile.lookup.<attr>]`, e.g. gidNumber-from-group):
single-select picker; Enter writes the chosen entry's `value_attr` **scalar**
(not a DN) into the field.

### Where the code lives
- `src/config/relation.rs` — `Relation`, `ResolvedRelation`/`CandidateScope`/`RelationRole`, `resolve_relations`, `holder_lookup`/`backref_lookup`. Pure. (`CandidateScope` + label machinery survive the unification.)
- `src/config/mod.rs` — `Config.relations`; `EntryProfile.{search_attrs, lookups, label}` + `search_attributes()`; `LookupSpec`.
- `src/ui/picker.rs` — `PickerState` (selection always visible, toggle, cursor, `scroll`, `search_active`, `truncated`), `build_member_filter`/`escape_filter` (RFC 4515), `candidate_label`, `pick_value`, `Candidate{dn,label,value}`. Pure.
- `src/ldap/worker.rs` — `Request::Search.size_limit`; `run_search` returns partial entries on a size/time limit (`is_limit_rc`).
- `src/ui/edit_form.rs` — `FieldRelation` + `EditField.{relation,lookup}`; `build_edit_form(…, relations)` tags fields; `tag_lookup_fields`; `ValueEditor` (`open`/`open_picker`/`open_lookup`, `scroll`); `to_edit_entry` excludes BackRef fields.
- `src/ui/app.rs` — `App.{relations,picker_search_id,picker_last_query}`; `picker_editor_key`; `service_picker_search`; the `Response::Entries` picker intercept; `plan_combined_save`/`combined_save_overlay`/`apply_combined_save`/`membership_fanout`/`would_empty`/`read_group_members`.
- `src/ui/view.rs` — picker + multi-value-editor branches in `render_value_editor` (both scroll via `clamp_scroll`).
- `src/ldap/ldif.rs` — `render_changesets` (multi-entry LDIF preview).
- `tests/live_membership.rs`, `tests/live_templates.rs`, `tests/live_seed.rs` — gated live tests.

---

## Open items / known gaps

1. **`main` is local-only** — not pushed to `origin` (`git@github.com:oposs/edaptor.git`). `feat-picker-polish` (an earlier branch) is fully contained in `main` and can be deleted.
2. **README "Status" blurb** still says Turbo Vision / early development — needs a one-paragraph refresh.
3. **M5 Samba** has no standalone in-TUI "Set Password"/Samba-enable action on arbitrary entries (only inline on create/edit of password-profile entries, plus the `edaptor passwd <dn>` CLI).
4. **M6 leftovers** pending (paged-scale lists, result-code polish, SASL auth, packaging).

---

## How to build / test / run

```bash
# Build + checks (must be green before any commit)
cargo build --all-targets
cargo test -p edaptor                       # 308 lib tests; live_* tests SKIP without the env var
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
