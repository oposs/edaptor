# edaptor — Session Handover

Carries the **current concern** into the next session. Not a project history — for
that see `git log`, the specs under `docs/superpowers/specs/`, the SDD ledger
(`.superpowers/sdd/progress.md`), and project memory (`…/memory/MEMORY.md`).

**Date:** 2026-07-21 · **Branch: `feat/optimistic-concurrency`** (worktree at
`/scratch/oetiker/claude-worktrees/edaptor-feat-optimistic-concurrency`, off `main` @
`5580cf7`). `Cargo.toml` version **1.2.1**.

**Current concern: the "real-time consistency" overhaul.** edaptor was designed to work
against live directory reality but had drifted into trusting read-time copies that were
never re-validated. A nine-cache audit found this; the fix is decomposed into **four
specs** (umbrella: `docs/superpowers/specs/2026-07-21-realtime-consistency-design.md`).
**Spec 1 (optimistic concurrency) is CODE-COMPLETE on this branch** and is being
finished/merged. Specs 2–4 are the remaining work. The whole thing started from a user
request for **entry deletion** (now Spec 4, shelved until 1–3 land).

---

## DONE THIS SESSION — Spec 1: optimistic concurrency (merge in progress)

Range `5580cf7..b64abea` (11 commits). Built with SDD (9 tasks + 1 final-review fix),
every task spec+quality reviewed, `make check` green. Plan:
`docs/superpowers/plans/2026-07-21-optimistic-concurrency.md`. Ledger has the full
per-task record.

**What it does.** On read, capture the entry's `entryCSN` (OpenLDAP change-sequence
number, finer-grained than `modifyTimestamp`) onto `EditForm.baseline_csn`. On MODIFY and
DELETE, attach a **critical RFC 4528 Assertion** `(entryCSN=<captured>)` + (MODIFY) an
**RFC 4527 Post-Read** returning the new CSN. The write applies only if unchanged; else
the server returns **rc 122** → `Response::WriteConflict` → `WriteOutcome::Conflict`.

**Conflict handling.** Re-read the entry; if the other client's changes do **not** overlap
the attributes we're editing → **rebase and resubmit silently**; if they **do** overlap →
**prompt** (Reload / Overwrite / Cancel). **No path ever silently overwrites an
overlapping foreign change** — only an explicit Overwrite adopts the fresh CSN (the
overlap path keeps the STALE csn so a naive re-save re-prompts). Verified end-to-end by
the opus whole-branch review.

**Capability fallback.** At connect, probe root DSE `supportedControl` for
`1.3.6.1.1.12`. Absent → blind-write fallback + **one-time** status-line warning. Gating is
essential (the assertion is critical → an ungated assertion on a non-supporting server
fails every write with rc 12). Gated at **every** assertion site (do_save,
do_combined_save own-leg + group-legs, resubmit_save).

**Key files.** `src/ldap/worker.rs` (assertion/post-read on run_modify/run_delete,
`assertion_supported`, `WriteConflict`, `WriteOk.new_csn`); `src/workflows/read_flow.rs`
(request `entryCSN`); `src/workflows/edit_form.rs` (`baseline_csn`);
`src/workflows/write_flow.rs` (thread csn, `fetch_group_csns`, submit_combined legs,
Conflict/Error mapping); `src/ui/state.rs` (`resolve_conflict`, `rebase_baselines`,
`reread_blocking_for_conflict`, `attrs_overlap`, `attrs_changed_since_baseline`, one-time
warning); `src/ui/app.rs` (do_save/do_combined_save gating, conflict dialog dispatch,
`force_overwrite`); `src/ui/dialog/conflict.rs` (Reload/Overwrite/Cancel);
`docs/src/concepts/optimistic-concurrency.md`. Live proof:
`tests/live_write.rs::modify_with_stale_csn_conflicts`.

### Deferred / tracked (NOT blocking Spec 1 merge)
- **Full rebase-for-combined-saves.** A conflict on a membership fan-out **group** leg
  currently aborts the batch and surfaces a clear *"membership changed on the server —
  reload and retry"* error (non-atomic, mirrors the existing partial-application path).
  Doing a true own-leg rebase + batch resubmit is a future enhancement; the error is
  correct and safe for now.
- **rc-match dedup (Minor).** rc0/122/other is inlined 3× (`write_response`, `run_modify`,
  `run_delete`) — extract a helper someday.
- **DELETE assertion is plumbed but has no caller.** `run_delete` supports `assert_csn`
  but every `Request::Delete` today passes `None` (no interactive delete flow exists yet).
  **When Spec 4 wires a real delete flow it MUST gate on `assertion_supported` and pass
  the form's `baseline_csn`.**
- **MODRDN (rename) legs are not asserted** this round (out of scope for Spec 1).

---

## NEXT — remaining specs (umbrella: `2026-07-21-realtime-consistency-design.md`)

Do them in order; each is its own spec → plan → SDD cycle.

### Spec 2 — Cache coherence (implements design commitments that were never built)
The three-pane design (`2026-06-01-three-pane-layout-design.md` §5.9, §7.4) promised an
**after-write local reflow** and a **manual Refresh**, but neither was wired:
- **`Structure::add_child` / `Structure::remove`** (`src/workflows/structure.rs:169,190`,
  doc-commented *"e.g. after a successful create"*) have **no production callers**. The
  entry list is a pure projection of `UiState::structure`, built once at bootstrap and
  never mutated — so **a newly created entry does not appear until restart** (the original
  user bug that kicked this off). Wire `add_child` on `WriteOutcome::Created` (label attrs
  from the post-read entry Spec 1 already fetches) and `remove` on delete/rename; handle
  the parent leaf→branch flip for the tree pane.
- **Manual Refresh action** — make `LoadStructure` callable outside bootstrap (its only
  caller is `state.rs:~932`) and bind a command; the escape hatch for genuine multi-client
  structure staleness.
- **`lookup_cache` invalidation** (`state.rs:~80`) — grep shows only `insert`/`contains_key`,
  never `remove`/`clear`; it caches negatives too, so a label (incl. after edaptor's OWN
  rename) stays wrong all session. Clear affected keys on our own writes.
- Clear `state.search` on create (stale incremental-find query hides the new row); make
  `current_leaf_row()` find the new entry so the highlight snaps to it.

### Spec 3 — Autonumber allocation via counter entry
The **one data-corrupting** cache: `open_create` scans for the next free number at
form-open (`app.rs:~449`, `alloc_flow.rs:~57`) and never re-checks at submit → two admins
creating in the same window both get the same `uidNumber`. Assertion control on the *new*
entry can't detect this (different DN). Fix = a **counter entry** allocated by CAS (read N,
`modify` to N+1 asserting `(value=N)`, retry on rc 122 — the exact primitive Spec 1
proved). Falls back to the current scan (keep the truncation-refusal guard) when no
counter entry is configured.

### Spec 4 — Delete entries (the original request; shelved until 1–3 land)
Spec already drafted: `docs/superpowers/specs/2026-07-20-delete-entries-design.md`.
Opt-in per profile (`[profile.delete]`), non-recursive (server enforces rc 66), typed
confirm dialog, removes the companion user-private group (probe: only when it has no other
members — else "left in place"), strips the user from referencing groups
(`(|(member=X)(uniqueMember=X)(memberUid=Y))`), **blocks** when the user is the sole member
of a MUST-membership group (reuses `would_empty`/`last_member_block`). Non-atomic, surfaced
honestly. Much smaller once Specs 1–2 exist (DELETE assertion + structure reflow already
there). `Request::Delete`/`run_delete`/rc-66 mapping + `Structure::remove` all exist.

---

## OTHER OPEN ITEMS

### Pre-existing live-test failure (unrelated to Spec 1)
`tests/live_templates.rs::picker_gidnumber_scalar_store_resolves_group_gidnumber` fails
against the live demo server — and fails **identically on `main` before this branch**, so
it is NOT caused by Spec 1. `make check` stays green without a server (it SKIPs). Being
investigated at handover time; see the ledger / this session's tail for status. Root-cause
it against `scripts/test-ldap.sh` demo data before trusting it as a gate.

### Enablers already in place
- The RFC 4528/4527 controls are first-class in **ldap3 0.12.1** (`Assertion`,
  `PostRead`, `PostReadResp`, `ldap_escape`) — no BER hand-encoding. Critical flag needs
  the struct-literal route (`Assertion { filter }.critical().into()`, not `::new`).
- The demo OpenLDAP **advertises** `1.3.6.1.1.12` / `1.3.6.1.1.13.1/.2` and honours the
  assertion (verified: right csn → rc 0, wrong → rc 122). `entryCSN` present, µs-precision.

---

## Working agreement / how to resume
- **Pull first** (`git pull --ff-only`); this repo lands work across machines.
- **Ask before any `ssh`/remote command**; **never** run destructive commands without
  explicit confirmation (`rm`, `git reset --hard`, `git push --force`, etc.).
- **SDD:** `SKILL=~/.claude/plugins/cache/claude-plugins-official/superpowers/6.1.1/skills/subagent-driven-development`
  → `scripts/task-brief PLAN N`, `scripts/review-package BASE HEAD`. Fresh implementer
  subagent per task → review package → task-reviewer → fix loop → final whole-branch
  review (most capable model). Ledger: `.superpowers/sdd/progress.md`.
- **Build/test (cap parallelism at 4 cores):**
  ```bash
  make check          # fmt + clippy -D warnings + tests — the gate
  cargo test -j4 ; make docs
  scripts/test-ldap.sh start ; export EDAPTOR_TEST_ADMIN_PW=adminpassword
  export EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389   # to run the gated live tests
  ```
- **tmux harness** (drives the TUI headlessly — used to smoke-test the conflict flow):
  ```bash
  tmux new-session -d -s ed -x 200 -y 50
  tmux send-keys -t ed "EDAPTOR_TEST_ADMIN_PW=adminpassword <bin> --config <cfg>" Enter
  tmux capture-pane -t ed -p       # -p strips colour (focus highlight invisible); -e keeps escapes
  ```
  For a concurrency smoke: open an entry in edaptor, `ldapmodify` the same entry from a
  shell (the "other client"), then edit + save — a disjoint attr → silent "Saved.", an
  overlapping attr → the "Entry changed" dialog.
- **Docs one-home:** config detail → mdBook (`docs/src/`); README orientation only;
  `CHANGES.md` every user-visible change; process/design → `docs/superpowers/`.
- **Facade boundary:** only `src/ui/**` may `use tvision_rs`; `ldap3` only in `src/ldap/**`.
- **Commit trailer:** `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **Remote:** `origin` = `git@github.com:oposs/edaptor.git`.

## Project state
edaptor is a Rust TUI (**tvision-rs 0.12.1**) for administering OpenLDAP: introspects live
schema, generates edit forms from `objectClass` defs; TOML config declares connection +
*entry profiles* + a **widget palette** (`[profile.widget.<attr>]` kinds
`choice`/`password`/`picker`/`membership`/`lookup`/`readonly`/`x_ordered`),
`[profile.defaults]` (literal / `{attr}` template / `{next:MIN-MAX}` autonumber / live in
create mode), and `[profile.companion]`. Writes now carry optimistic-concurrency
assertions (Spec 1). Sole binary `edaptor`; UI in `src/ui/`.
