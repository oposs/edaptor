# edaptor — Session Handover

Carries the **current concern** into the next session. Not a project history — for
that see `git log`, the specs under `docs/superpowers/specs/`, the SDD ledger
(`.superpowers/sdd/progress.md`), and project memory (`…/memory/MEMORY.md`).

**Date:** 2026-07-15 · **Branch: `feat/usability`** (off `main` @ v1.0.0).
**Current concern: a batch of usability improvements.** Two are **done, committed,
`make check` green, and passed a final opus whole-branch review (ready to merge)**;
three more are **agreed but not yet started**. We keep working on this branch and
**open a single PR at the end** — do not merge to `main` mid-way.

---

## DONE on this branch (committed, reviewed)

Full range `9c8efcf..HEAD` (10 commits). `make check` green throughout.

1. **PgUp/PgDn page the entry form** (`fix(ui)` `c034768`). The form's `ScrollGroup`
   only scrolled arrow-by-arrow; the focused `InputLine` ignores Page keys. Now
   `ScrollGroup::handle_event` intercepts PageUp/PageDown and moves focus one
   viewport (reusing the scrollbar-drag `focus_target_for_row` logic). Verified the
   tree/leaf browse lists *already* paged (tvision `ListViewer`/`Outline`).

2. **Live templated defaults — create-mode autofill** (feature, 3 SDD tasks +
   fmt/test-hardening commits `5c69526`, `689277f`, `c76bfd8`, `585525a`, `582c9b7`,
   `e609348`). In **create mode only**, a `[profile.defaults]` template such as
   `cn = "{givenName} {sn}"` fills **and keeps updating** the target as the operator
   types the sources, until the operator edits the target (clear it to re-arm).
   - Spec: `docs/superpowers/specs/2026-07-14-live-templated-defaults-design.md`
   - Plan: `docs/superpowers/plans/2026-07-14-live-templated-defaults.md`
   - **Architecture:** pure latch/recompute core in `src/config/defaults.rs`
     (`LiveTemplateState{segs,auto,last_written}`, `live_templates()`,
     `recompute_live()`); a `live_templates` map on `UiState` built by `open_create`
     (`src/ui/app.rs`); a create-mode-only hook `FormPane::apply_live_templates`
     called after `sync_into_form()` in `handle_event` (`src/ui/panes/form.rs`).
   - Latch rule: `value != last_written ⇒ auto = value.is_empty()`; while auto,
     `Some(out)` write-if-differs / `None` (source empty) clear-if-nonempty.
     `last_written` distinguishes our writes from operator edits (proven convergent —
     no busy-loop). Literals & `{next:MIN-MAX}` autonumbers stay one-shot; edit mode
     inert (gated on `FormMode::Create` **and** non-empty map — both now tested).
   - Final opus review verdict **READY TO MERGE**; details in the SDD ledger.

**Also investigated (no code change): Esc closes every modal.** All edaptor modals
are tvision `Dialog`s, and `Dialog` maps **Esc → CANCEL → end_modal** natively; the
custom popups delegate their non-nav path to the inner `Dialog`. So Esc already
cancels everywhere. Only nuance: in a list with an active type-to-find filter the
first Esc clears the filter, a second closes (correct). If a specific dialog ever
resurfaces where Esc does nothing, dig there — the user couldn't reproduce one.

---

## NEXT (agreed with the user, not started) — design on THIS branch

These came from the same usability request. Do each as its own brainstorm → spec →
plan → SDD cycle. Recommended order: **(c) then (b)**.

### (c) Create-template concept — polish + CLI + the container rule
Most of it already exists: `New` (the `CREATE` command in `src/ui/app.rs:265`) uses
the current tree branch as container, filters profiles via
`create::profiles_for_container` (search_base matches the container at a DN
boundary — exact, or one is a parent/child of the other), and: **0 → error, 1 →
open directly, >1 → profile-chooser dialog**. Real gaps to close:
- **No `edaptor create` CLI subcommand** (only `check`/`schema`/`passwd` exist in
  `src/main.rs`). The user wants a commandline `create` that asks what to make.
- **Wrong-container hazard:** if you're *above* a profile's home OU (e.g. at the
  tree root), the profile is still offered but the object is created at your
  *current* location, not the profile's `search_base`. The user's instinct: make
  them navigate to the right OU first (or create at `search_base`). Needs a decided
  rule — brainstorm it before coding.

### (b) Companion group entry on user create (user private group)
When creating a user that has its own `gidNumber`, also emit a matching `posixGroup`
in the groups OU (cn = uid, same gidNumber). **Biggest of the three:** today
`create::plan_create` builds exactly ONE add; this needs a way to *declare* a
companion entry in the profile config **and** a multi-add write path (with
confirm-preview of both stanzas). Look at `src/workflows/save.rs` /
`write_flow.rs` for how multi-entry writes are already done for membership fan-out.

---

## Working agreement / how to resume

- **Pull first** (`git pull --ff-only`); this repo lands work across machines.
- **SDD ledger is the source of truth for progress:** `.superpowers/sdd/progress.md`
  (git-ignored scratch under `.superpowers/sdd/`: briefs, reports, review diffs).
  The current ledger covers the live-templated-defaults feature (all complete).
- **SDD scripts:** `SKILL=~/.claude/plugins/cache/claude-plugins-official/superpowers/6.1.1/skills/subagent-driven-development`
  → `scripts/task-brief PLAN N`, `scripts/review-package BASE HEAD`. Fresh
  implementer subagent per task → review package → task-reviewer → fix loop → mark
  complete → final whole-branch review (most capable model).
- **Build/test (cap parallelism at 4 cores — shared box):**
  ```bash
  make check          # fmt + clippy -D warnings + tests — the gate
  cargo test -j4
  make docs           # build the mdBook
  scripts/test-ldap.sh start   # podman demo LDAP (~600 users/~25 groups)
  export EDAPTOR_TEST_ADMIN_PW=adminpassword
  cargo run -- --config examples/demo-config.toml
  ```
- **Docs one-home:** config detail → mdBook (`docs/src/`); README orientation only;
  `CHANGES.md` for every user-visible change (Unreleased section is populated);
  process/design → `docs/superpowers/`.
- **Facade boundary:** only `src/ui/**` may `use tvision_rs`.
- **Commit trailer:** `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **Finish:** when the batch is done, open **one PR** for `feat/usability` (remote
  `origin` = `git@github.com:oposs/edaptor.git`).

## Project state

edaptor is a Rust TUI (tvision-rs **0.12**) for administering OpenLDAP: introspects
live schema (`cn=subschema`), generates edit forms from `objectClass` defs; TOML
config declares connection + *entry profiles* + a **widget palette**
(`[profile.widget.<attr>]` kinds: `choice`/`password`/`picker`/`membership`/`lookup`)
and `[profile.defaults]` (literal / `{attr}` template / `{next:MIN-MAX}` autonumber —
templates now live in create mode). `Cargo.toml` version **1.0.0**. `edaptor` is the
sole binary; UI lives in `src/ui/`.
