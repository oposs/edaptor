# edaptor — Session Handover

Carries the **current concern** into the next session. Not a project history — for
that see `git log`, the specs under `docs/superpowers/specs/`, the SDD ledger
(`.superpowers/sdd/progress.md`), and project memory (`…/memory/MEMORY.md`).

**Date:** 2026-07-16 · **Branch: `feat/usability`** (off `main` @ v1.0.0).
**Current concern: the usability batch is COMPLETE.** All four items are **done,
committed, `make check` green, and each passed a final whole-branch/feature review
(READY TO MERGE)**. Nothing is in flight — the next step is to **open the single
`feat/usability` PR** (remote `origin` = `git@github.com:oposs/edaptor.git`).

**Before the PR, please eyeball these interactive checks** (they need a live terminal
+ demo LDAP and can't be driven headlessly; static review + the live worker tests found
no correctness risk):
- item (c): the **"Create where?"** modal firing when you press New *above* a profile's
  home OU, and **`edaptor tui-create <profile>`** opening the right create form / the
  no-arg chooser fallback.
- item (b): creating a user through a companion-declaring profile and confirming the
  two-stanza preview + that **both** the user and its `posixGroup` land. (The atomic
  commit *and* rollback are already proven by `tests/live_companion_atomic.rs` against
  the demo server; this is just the UI-flow eyeball.)

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

3. **Create-usability — `tui-create` launcher + TUI container rule** (item (c); 7 SDD
   tasks + fmt + mouse-staging fix, range `8bf5d4c..26ae032`). Two parts:
   - **Container rule.** Pressing New *above* a profile's home OU now pops a
     **"Create where?"** modal (current branch vs. the profile's `search_base`)
     instead of silently creating at the wrong location; at/inside the home OU is
     unambiguous (no prompt). Pure `resolve_create_container` in
     `src/workflows/create.rs`; `container_chooser` dialog; both `CREATE` arms funnel
     through `open_create_with_container_rule` in `src/ui/app.rs`.
   - **`edaptor tui-create [<profile>] [--container <DN>]`.** Launches the TUI straight
     into a profile's create form, reusing the whole interactive flow. Mechanism: a
     `StartupAction` on `UiState::pending_startup` (set by `ui::run`), posted once by
     the pump as `STARTUP`, run in `app::dispatch`. `<profile>` optional (chooser
     fallback; unknown name errors *pre-launch*); `--container` defaults to
     `search_base`. Name/container resolved in `main::build_startup_action`.
   - Spec: `docs/superpowers/specs/2026-07-15-create-usability-cli-container-rule-design.md`
   - Plan: `docs/superpowers/plans/2026-07-15-create-usability-cli-container-rule.md`
   - Final opus review **READY TO MERGE** (no Critical/Important); container logic
     proven consistent with `profiles_for_container`; STARTUP timing safe. A
     mouse-staging fix (choosers now honour clicks, not just keyboard) shipped as a
     follow-up. Accepted cosmetic Minors: chooser empty-`search_base` uses a status
     line not a modal; `container_chooser` fixed width truncates long DNs.

4. **Companion entry on create — user-private group** (item (b); 7 SDD tasks + a live
   test + a doc fix, range `45d26ff..06f3941`). A profile can declare
   `[profile.companion]` (object classes, `rdn_attr`, `search_base`, a templated
   `attributes` map) and creating through it emits a second entry — e.g. a `posixGroup`
   mirroring a POSIX user (`cn = {uid}`, same `gidNumber`).
   - **Atomic via LDAP transactions (RFC 5805).** The worker's `Request::AddAtomic`
     wraps both Adds in `StartTxn`/`EndTxn` under a `TxnSpec` control when the server
     advertises txn (detected from the root DSE → `UiState::server_supports_txn`);
     otherwise a **sequential companion-first fallback** (`CompanionThenPrimary` →
     `PrimaryAfterCompanion`, orphan-named on primary failure). `do_create` previews both
     LDIF stanzas and dispatches by capability.
   - **Architecture:** pure `CompanionSpec` + `validate_companions` (`src/config/mod.rs`);
     pure `plan_companion` (`src/workflows/create.rs`, reusing `resolve_template`);
     `AddAtomic` + `txn_supported` + `fetch_root_dse` (`src/ldap/worker.rs`); the two
     write paths in `src/workflows/write_flow.rs`.
   - Spec: `docs/superpowers/specs/2026-07-16-companion-entry-on-create-design.md`;
     Plan: `docs/superpowers/plans/2026-07-16-companion-entry-on-create.md`.
   - Final opus review **READY TO MERGE** (no Critical/Important). Atomic **commit and
     rollback** proven live by `tests/live_companion_atomic.rs` against the demo
     OpenLDAP. Non-blocking follow-ups listed under NEXT.

**Also investigated (no code change): Esc closes every modal.** All edaptor modals
are tvision `Dialog`s, and `Dialog` maps **Esc → CANCEL → end_modal** natively; the
custom popups delegate their non-nav path to the inner `Dialog`. So Esc already
cancels everywhere. Only nuance: in a list with an active type-to-find filter the
first Esc clears the filter, a second closes (correct). If a specific dialog ever
resurfaces where Esc does nothing, dig there — the user couldn't reproduce one.

---

## NEXT

Nothing is queued — the batch is done (all four items above are committed and reviewed
READY TO MERGE). **Open the single `feat/usability` PR** after the interactive eyeball
checks at the top. Non-blocking follow-ups surfaced by the final reviews (own cycle, not
this PR): DN-escape the RDN value in `build_add_entry` **and** `plan_companion` together
(pre-existing, unreachable for uid-keyed entries); `debug_assert!(!entries.is_empty())`
in `run_add_atomic`; refresh the `do_create` doc-comment for the companion-plan borrow.

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
