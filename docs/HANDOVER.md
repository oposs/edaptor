# edaptor — Project Handover

**Date:** 2026-06-02
**`main` HEAD:** `54d10be` (working tree clean)
**Active worktree:** `/scratch/oetiker/claude-worktrees/ldapedit-feat-rich-templates` on branch `feat-rich-templates` (**35 commits ahead of `main`**, HEAD `8e970ed` + this doc commit) — the **rich user templates** milestone, **all phases (0–6) complete** (see the execution-progress section below). 263 lib tests + 3 gated live `live_templates` tests green; full end-to-end tmux smoke passed (create a posix user with multi-OC, defaults, autonumber, lookup-gidNumber, and password — verified against a real slapd). Ready for final review + finish-branch.

`edaptor` is a Rust **ratatui** TUI for administering an OpenLDAP directory (users, groups, group memberships). It derives the directory's structure from live schema introspection (`cn=subschema`) and generates edit forms from `objectClass` definitions; a TOML config declares connection settings plus *entry profiles* ("what a user/group means here").

> **Note:** `README.md`'s "Status" / "Turbo Vision" wording is **stale** — the UI was migrated off turbo-vision to ratatui 0.30 (see milestones below). The authoritative design is [`docs/superpowers/specs/2026-05-29-edaptor-design.md`](superpowers/specs/2026-05-29-edaptor-design.md).

---

## Milestone status

| Milestone | State |
|---|---|
| M1 Foundation (config, TLS, worker, bind, subschema) | ✅ on `main` |
| M2 Schema model (typed MUST/MAY, syntax→FieldKind) | ✅ |
| M3 TUI shell + generic read tier | ✅ |
| M4 Generic **write** path (diff→ChangeSet→LDIF→Modify/Add/ModRdn/Delete) | ✅ |
| M5 Samba lifecycle (NT hash, SID/RID, group-map, synced password) | ✅ headless + `edaptor passwd <dn>` CLI; not surfaced in the TUI |
| 3-pane redesign | ✅ merged |
| turbo-vision → **ratatui 0.30** migration | ✅ merged |
| UI polish (white theme, double-border focus, per-pane footers, F7 create, status-line hints, `GuardIntent`/`ResolveGuard` dirty-guard) | ✅ merged |
| **Relation membership picker** | ✅ merged (`54d10be`) |
| M6 leftovers (paged-scale lists, result-code→human table polish, SASL EXTERNAL/GSSAPI auth, packaging) | ⏳ pending |
| **Rich user templates** (branch `feat-rich-templates`) | ✅ complete — all phases 0–6 done; gated live tests + full tmux smoke green; awaiting final review + merge |

---

## Background: the relation membership picker (merged to `main`)

**Symmetric group↔user membership editing as a *picker mode* of the existing multi-value value-editor** — no separate dual-pane screen. Driven by one `[[relation]]` config block.

- **Forward (group's `member`):** Enter on the `member` field opens a live, size-capped (≤20) searchable **user** picker; current members stay pinned; commit writes the group entry via the existing single-entry save.
- **Reverse (user's `memberOf`):** Enter on `memberOf` opens a **group** picker; on save it **fans out** `member` MODIFYs across the affected groups (synchronously, with last-member pre-validation and a partial-failure report). `memberOf` itself is never written (overlay-maintained).

**Spec:** [`docs/superpowers/specs/2026-06-01-relation-membership-picker-design.md`](superpowers/specs/2026-06-01-relation-membership-picker-design.md)
**Plan:** [`docs/superpowers/plans/2026-06-01-relation-membership-picker.md`](superpowers/plans/2026-06-01-relation-membership-picker.md)

### Where the code lives
- `src/config/relation.rs` — `Relation` (`[[relation]]` TOML), `ResolvedRelation`/`CandidateScope`/`RelationRole`, `resolve_relations`, `holder_lookup`/`backref_lookup`. Pure.
- `src/ui/picker.rs` — `PickerState` (selection always visible, toggle, cursor, `truncated`), `build_member_filter`/`escape_filter` (RFC 4515), `candidate_label`. Pure.
- `src/config/mod.rs` — `Config.relations`; `EntryProfile.search_attrs` + `search_attributes()`.
- `src/ldap/worker.rs` — `Request::Search.size_limit`; `run_search` returns partial entries on a size/time limit (`is_limit_rc`) instead of erroring.
- `src/ui/edit_form.rs` — `FieldRelation` on `EditField`; `build_edit_form(…, relations)` tags fields; `ValueEditor` picker fields + `open_picker`; `EditForm::backref_labels`; `to_edit_entry` excludes BackRef fields.
- `src/ui/app.rs` — `App.{relations,picker_search_id,picker_last_query}`; `picker_editor_key`; `service_picker_search`; the `Response::Entries` picker intercept; `plan_combined_save`/`combined_save_overlay`/`apply_combined_save`/`reload_form_sync`/`membership_fanout`/`would_empty`/`read_group_members`.
- `src/ui/view.rs` — picker branch in `render_value_editor`.
- `src/ldap/ldif.rs` — `render_changesets` (multi-entry LDIF preview).
- `tests/live_membership.rs` — gated forward/reverse/last-member/size-cap tests.

### Live wiring (it IS reachable in the running app)
Enter on a field → `dispatch_key` → `open_value_editor(app, structure)` → picker mode for relation fields. `service_picker_search` runs each tick when a picker is open; results route via the `picker_search_id` intercept in `handle_worker_response`. Save: `FormSave`/`ResolveGuard{save:true}` → `combined_save_overlay` → Confirm → `apply_combined_save`. **Gated on config:** a field only becomes a picker if a `[[relation]]` is declared *and* the entry's objectClass matches; otherwise `member` opens the plain free-text editor. Example config block: `README.md` `## Configuration`.

### Merge note (why this was non-trivial)
The branch was cut at `41f90a1`; `main` then absorbed the UI-polish refactor, which **removed the `menu_defs` menu system** (profile-create is now **F7**) and **replaced `PendingAction::{Navigate,SaveThenNavigate}` with `GuardIntent` + `ResolveGuard{intent,save}`**. The merge (`54d10be`) re-integrated the picker onto that model: dropped `menu_defs`/`profile_count`; threaded an optional `then_intent: GuardIntent` through `PendingAction::CombinedSave` so a dirty-`memberOf` form that trips the focus/quit guard **saves, then resumes the pending intent on clean success**. Reviewed (opus) and approved; 207 tests green.

---

## Open items / known gaps

1. **Membership apply-seam:** the gated `tests/live_membership.rs` (fan-out / last-member / size-cap, 4 tests) **now run green against a real podman slapd** (run this session). The manual tmux smoke of the live apply is still not done.
2. **README status section is stale** (says Turbo Vision / early development) — the `## Status` blurb still needs a refresh. The `## Configuration` example is now current (rewritten in Task 6.3 as a rich multi-profile example with `object_classes`, defaults, password, and lookup).
3. **M5 Samba** is headless-only as a *CLI* (`edaptor passwd <dn>`); password setting is now surfaced in the TUI for create- and edit-of password-profile entries (Phase 4), but there is still no standalone in-TUI "Set Password"/Samba-enable action on arbitrary entries.

---

## Current milestone: rich user templates — execution progress

**Spec:** [`docs/superpowers/specs/2026-06-02-rich-user-templates-design.md`](superpowers/specs/2026-06-02-rich-user-templates-design.md) · **Plan:** [`docs/superpowers/plans/2026-06-02-rich-user-templates.md`](superpowers/plans/2026-06-02-rich-user-templates.md). Branch `feat-rich-templates`, 27 commits, HEAD `39a33cd`.

**Done (all phases):**
- **Phase 0** — create-host unification: NEW renders in the pane-3 `FormMode::Create` form (Save→Add); the modal `Overlay::CreateForm` is deleted. Key fns: `build_new_entry_form`, `plan_create`/`prepare_create`, `should_install_form`.
- **Phase 1** — `EntryProfile.object_class: String` → `object_classes: Vec<String>` (BREAKING, no alias). `build_add_entry` emits `top`+all classes deduped; `build_member_filter` ANDs classes; create resolves MUST/MAY over all classes.
- **Phase 2** — `src/config/defaults.rs` (pure): `parse_default_value` (literal / `{attr}` template / `{next:MIN-MAX}`), `next_in_range`, `plan_defaults`; `[profile.defaults]` parsed onto `EntryProfile`.
- **Phase 3** — `Response::Entries.truncated`; `decide_allocation`/`allocate_number` (sync subtree scan, REFUSES on truncation); defaults + autonumber applied in `prepare_create`.
- **Phase 4 (FULL: 4.1–4.6 + combined-path fix)** — `[profile.password]` (`PasswordSpec`); `password_add_attrs` (create) / `password_replace_mods` (edit, honors `ldap_attribute`+samba); `inject_password_fields` (masked attr + `(confirm)` fields, schema password suppressed); `stage_password` (create) / `stage_edit_password` (edit, strips pseudo-fields from baseline+edited so a blank field never clobbers); `prepare_save` folds password mods into the ChangeSet + masks the preview; `prepare_edit_save` shared by plain-F2 and guard-resume. **All 5 save entry points stage the password** — create, single-edit ×2, **combined membership ×2** (the combined path was a real clobber bug, found in review and fixed in `39a33cd`). `tests/live_templates.rs` (gated) verifies create-time password Add→re-bind green against real slapd.
- **Phase 6** — context-filtered profile chooser on F7 (`profiles_for_container` DN-boundary match; 0→all, 1→direct, >1→`Overlay::ChooseProfile`).
- **Phase 5** — value-lookup picker (gidNumber-from-group). A field declared in `[profile.lookup.<attr>]` opens a **single-select** picker; Enter writes the chosen entry's `value_attr` **scalar** (not a DN) into the field. Key pieces: `config::LookupSpec` + `EntryProfile.lookups`; pure `picker::pick_value` + `Candidate.value: Option<String>`; `edit_form::tag_lookup_fields` + `EditField.lookup` (wired at create **and** edit form-build sites; edit uses `lookups_profile_for_entry`); `ValueEditor` carries `lookup`; `open_value_editor` opens the picker for a **single-value** field (no `multi` requirement — the key trap); `service_picker_search` requests `[value_attr, label, …search_attrs]`; the `Response::Entries` intercept fills `Candidate.value` via `pick_value`; `picker_editor_key` binds **Enter** to commit the scalar to `field.editor`. Both `profile_for_entry`/`lookups_profile_for_entry` now share `profile_for_entry_where`.
- **Task 3.4 + 5.3 + 4.6** — gated `tests/live_templates.rs`: create-time password Unix round-trip, autonumber+multi-OC create, and lookup→gidNumber. All 3 pass against the podman slapd (run this session).
- **Task 6.3** — README `## Configuration` rewritten as a rich multi-profile example (multi-OC user + `[profile.defaults]`/`[profile.password]`/`[profile.lookup.gidNumber]` + group + relation). Full tmux smoke passed end-to-end.

**Remaining:**
- Final branch review + finish-branch (merge/PR). Nothing functional outstanding.

---

## How to build / test / run

```bash
# Build + checks (must be green before any commit)
cargo build --all-targets
cargo test -p edaptor                       # 249 pass; live_* tests SKIP without the env var
cargo clippy --all-targets -- -D warnings
cargo fmt --check

# Live / integration tests against a throwaway OpenLDAP (podman)
scripts/test-ldap.sh start                  # prints the two env vars to export
export EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389
export EDAPTOR_TEST_ADMIN_PW=adminpassword
cargo test -p edaptor                        # now the live_* tests actually run
scripts/test-ldap.sh stop

# Run the TUI
cargo run -- --config <path>                 # default config: ~/.config/edaptor/config.toml
```

For a manual membership smoke: point a config (with a `[[relation]]` block) at the test LDAP, open a group → Enter on `member` → type/Space/F2 → Save; then a user → Enter on `memberOf` → toggle a group → Save; try removing a group's last member (expect a clear block).

---

## Conventions (follow these)

- **Facade boundary:** only `src/ui/*` may `use ratatui`/`use tui_*`. Verify: `! grep -rl "use ratatui\|use tui_" src | grep -v "^src/ui/"`.
- **Strict TDD**, atomic commits; crate must compile after every commit; `cargo fmt` before commit.
- **Live tests gated** by `EDAPTOR_TEST_LDAP_URI` (skip when unset) — mirror `tests/live_write.rs` / `tests/live_membership.rs`. DN base in tests is `dc=example,dc=org`.
- **Worktrees** live under `/scratch/oetiker/claude-worktrees/` as `<project>-<branch>`.
- **Commit trailer:** `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- **Execution style:** subagent-driven (fresh subagent per task + spec-then-quality review); see project memory `prefers-agent-fanout`. App.rs-heavy tasks can exhaust a subagent's context — scope tightly or resolve in-session.
