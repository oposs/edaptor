# edaptor — Session Handover

Carries the **current concern** into the next session. Not a project history — for
that see `git log`, the specs under `docs/superpowers/specs/`, the SDD ledger
(`.superpowers/sdd/progress.md`), and project memory (`…/memory/MEMORY.md`).

**Date:** 2026-07-22 · **Branch: `feat/cache-coherence`** (worktree at
`/scratch/oetiker/claude-worktrees/edaptor-feat-cache-coherence`, off `main` @
`4231dcd`). 29 commits, 22 files, ~+2990/-228. `Cargo.toml` version **1.2.1**.

**Current concern: Spec 2 of the "real-time consistency" overhaul is CODE-COMPLETE
and reviewed READY TO MERGE — it is waiting on the merge decision.** Specs 3 and 4
are the remaining work (umbrella:
`docs/superpowers/specs/2026-07-21-realtime-consistency-design.md`).

---

## DONE — Spec 2: cache coherence (ready to merge)

Spec `docs/superpowers/specs/2026-07-21-cache-coherence-design.md`, plan
`docs/superpowers/plans/2026-07-21-cache-coherence.md`, built with SDD (9 tasks,
each spec+quality reviewed) plus a whole-branch opus review and two fix rounds.
`make check` green with the live demo server up (838 lib tests + every live suite).

**What it does.** edaptor's in-memory model of the directory was built once at
startup and never updated. Now:

- **`Structure::upsert` is the single mutation point**, fed by *every* entry read
  (navigation and the post-write re-read alike), so create, rename and
  label-changing edits all reflow through one path and any entry you visit
  self-heals. `add_child` is gone; `remove` finally has callers.
- **Writes reflow the model**: a rename drops the stale node (a renamed
  *container* re-runs the scan, since every descendant DN changed), a create
  clears a stale find query and selects the new row, and every write clears
  `lookup_cache` (which cached negatives, so a label edaptor itself changed
  stayed wrong all session).
- **The tree pane can rebuild** (`tree_dirty` → `build_branch_nodes` → swap
  `Outline::root` → `ov_update`), restoring the highlight **by DN**.
- **Every incremental find asks the server.** New `LeafSearchFlow` (ids
  5_000_000+, supersede-per-keystroke, `SearchScope::OneLevel`, cap 500) backs
  the entry list; the `lookup` combobox re-queries instead of narrowing a
  one-shot load capped at 100. Hits are upserted, so they persist.
- **Alt+R (File → Reload)** re-runs the eager scan, keeps your place by DN, and
  raises an error dialog on failure. The dead `UiAction` enum (`src/app.rs`) was
  deleted — it documented an Alt+R that never existed.
- **`UiState::status` is finally rendered** (see below).

**Key files.** `src/workflows/structure.rs` (`upsert`), `src/workflows/leaf_search.rs`
(new), `src/workflows/labels.rs` (`compute_rows_for_dns`), `src/ui/state.rs`
(`upsert_from_read`, `set_leaf_search`, `apply_leaf_search_outcome`,
`adopt_structure`, `reload_structure`, the status clearing policy),
`src/ui/panes/tree.rs` (`rebuild`), `src/ui/panes/leaf.rs`, `src/ui/lookup.rs`,
`src/ui/app.rs` (RELOAD + status-line footer), `src/ui/help_ctx.rs`
(`status_or_hint`), `docs/src/concepts/live-data.md`, `tests/live_search.rs`.

### Defects the review loop caught (all fixed — worth knowing they existed)

1. **Rename detection was unsound.** The plan inferred "this was a rename" from
   `current_leaf != reread_dn`. Save and navigation are independent async paths,
   so saving A then discarding/navigating to B deleted **B** — the entry on
   screen — from the model. Fixed by threading `renamed_from` from the SavePlan
   through `WriteIntent`/`WriteOutcome`. Nothing infers a rename from UI state now.
2. **The tree rebuild could invent a navigation.** `ov_update` re-clamps the
   outline's focus internally, so a *vanished* branch left `last_sel` stale and
   the pane reported a branch the operator never selected. `last_sel` is now
   resynced unconditionally from the outline's actual focus.
3. **A rebuilt entry list reported row 0** (the `‹self›` row), dragging the form
   onto the container and wiping the status. Every rebuild path now snaps
   `set_leaf_row` back.
4. **Container rename orphaned its subtree** — `remove` + upsert-at-a-new-key
   left the children stranded. Now re-scans.
5. Cross-container leakage of in-flight find results (fixed with
   `LeafSearchFlow::cancel()` on container switch, reload and create).

### Verified live (not just unit-tested)
An entry created out-of-band by `ldapadd` while the TUI ran was found by typing,
with no restart — the original user-reported bug. Alt+R shows "Reloaded 640
entries.", navigating clears it, and field hints still render.

### Deliberate decisions (do not "fix" these)
- **While a find is in flight the previous query's rows stay on screen** (no
  flicker). Product decision, taken explicitly. Cross-*container* leakage is a
  bug and was fixed; within-container staleness is intended.
- The spec's promised lookup truncation notice was **dropped** (commit `a601594`)
  — per-keystroke re-querying reaches candidates past the cap, and the docs state
  the cap in prose.

---

## The status line, and what it opened up

`UiState::status` was written in ~8 places and **rendered nowhere** — "Saved.",
and Spec 1's one-time "server does not support optimistic concurrency" warning,
had never been visible to anyone. It is now rendered in the status line's footer
(`status_or_hint`, `try_borrow` in the draw path), with a clearing policy so a
message cannot outlive its action and suppress the per-field key hints.

**This is worth auditing further:** every `st.status = …` in the codebase is now
user-visible for the first time. Some of those strings were written by people who
knew nobody would read them. A pass over their wording is a cheap win.

---

## NEXT — remaining specs

### Spec 3 — Autonumber allocation via counter entry
The one **data-corrupting** cache: `open_create` scans for the next free number
at form-open and never re-checks at submit, so two admins creating in the same
window get the same `uidNumber`. The assertion control cannot detect this (the
collision is on a *different* entry). Fix: a **counter entry** allocated by
compare-and-swap (read N, modify to N+1 asserting `(value=N)`, retry on rc 122 —
the primitive Spec 1 proved). Falls back to the current scan when no counter
entry is configured.

### Spec 4 — Delete entries (the original request)
Spec drafted: `docs/superpowers/specs/2026-07-20-delete-entries-design.md`. Much
smaller now: `Request::Delete`/`run_delete`/rc-66 mapping exist, `Structure::remove`
has callers and a rebuild path, and `lookup_cache` invalidation is handled. **When
wiring the delete flow it MUST gate on `assertion_supported` and pass the form's
`baseline_csn`** (`run_delete` supports `assert_csn` but every caller passes `None`
today).

---

## Follow-ups left open (none blocking the merge)

- `apply_branch_guard_stay` sets `set_tree_row` from the pre-rebuild `branch_dns`;
  resolving it by DN (as `rebuild` does) would delete the bug class. `tree_dirty`
  now fires far more often than when this was judged rare.
- No status clear on `open_create` / modal cancel / guard Stay.
- `leaf_search_truncated` is a dead field (the notice goes via `status`).
- Per-entry reads request `*`, which cannot return an *operational* attribute
  named by a label/tree template; appending `scan_attrs` would fix it.
- Two-leg rename where leg 1 (MODRDN) succeeds and leg 2 (MODIFY) fails: the
  model never learns the rename happened. `WriteConflict` drops `renamed_from` too.
- After a pick, editing a `lookup` input sends the whole `"5000 (staff)"` string
  as the search term.
- Residual status losses when `current_leaf_row()` is `None` (Alt+R after the
  selected entry vanished; a container rename reached via guard→Save).

---

## Working agreement / how to resume
- **Pull first** (`git pull --ff-only`); this repo lands work across machines.
- **Ask before any `ssh`/remote command**; **never** run destructive commands
  without explicit confirmation (`rm`, `git reset --hard`, `git push --force`).
- **SDD:** `SKILL=~/.claude/plugins/cache/claude-plugins-official/superpowers/6.1.1/skills/subagent-driven-development`
  → `scripts/task-brief PLAN N`, `scripts/review-package BASE HEAD`. Ledger:
  `.superpowers/sdd/progress.md` — it records every finding and its disposition.
- **Build/test (cap parallelism at 4 cores):**
  ```bash
  make check          # fmt + clippy -D warnings + tests — the gate
  scripts/test-ldap.sh start ; export EDAPTOR_TEST_ADMIN_PW=adminpassword
  export EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389   # runs the gated live tests
  ```
- **tmux harness** (drives the TUI headlessly — how the live verification above
  was done; the binary lands in `/home/oetiker/scratch/cargo-target/debug/edaptor`):
  ```bash
  tmux new-session -d -s ed -x 200 -y 50
  tmux send-keys -t ed "EDAPTOR_TEST_ADMIN_PW=adminpassword <bin> --config examples/demo-config.toml" Enter
  tmux capture-pane -t ed -p       # -p strips colour; -e keeps escapes
  ```
- **Docs one-home:** config detail → mdBook (`docs/src/`); README orientation only;
  `CHANGES.md` every user-visible change; process/design → `docs/superpowers/`.
- **Facade boundary:** only `src/ui/**` may `use tvision_rs`; `ldap3` only in `src/ldap/**`.
- **Commit trailer:** `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **Remote:** `origin` = `git@github.com:oposs/edaptor.git`.

## Project state
edaptor is a Rust TUI (**tvision-rs 0.12.1**) for administering OpenLDAP: introspects
live schema, generates edit forms from `objectClass` defs; TOML config declares
connection + *entry profiles* + a **widget palette** (`[profile.widget.<attr>]` kinds
`choice`/`password`/`picker`/`membership`/`lookup`/`readonly`/`x_ordered`),
`[profile.defaults]` and `[profile.companion]`. Writes carry optimistic-concurrency
assertions (Spec 1); the directory view now tracks reality (Spec 2). Sole binary
`edaptor`; UI in `src/ui/`.

### Note on the pre-existing live-test failure
`HANDOVER` previously flagged
`tests/live_templates.rs::picker_gidnumber_scalar_store_resolves_group_gidnumber`
as failing on `main`. It **passes** against a freshly seeded demo server
(`scripts/test-ldap.sh start`), so it looks like stale container state rather than
a code defect.
