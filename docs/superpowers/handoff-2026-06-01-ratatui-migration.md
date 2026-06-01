# Handoff — ratatui migration (P0–P3 done, P4–P6 remain)

Date: 2026-06-01. Branch: **`feat-ratatui-migration`** in worktree
`/scratch/oetiker/claude-worktrees/ldapedit-feat-ratatui-migration` (branched off
`feat-three-pane` HEAD `74fcf50`; the uncommitted TV vendor fork / patch / utf8
repro were intentionally dropped per plan §8).

**Plan:** [docs/superpowers/plans/2026-06-01-ratatui-migration.md](file:///scratch/oetiker/claude-worktrees/ldapedit-feat-ratatui-migration/docs/superpowers/plans/2026-06-01-ratatui-migration.md)
**Spike (render reference):** `/scratch/oetiker/ratatui-spike/src/main.rs`

## How this is being executed
`superpowers:subagent-driven-development`, but **hybrid** (per advisor): the
orchestration **spine is implemented in-session** (P0 skeleton, P1 wiring/event
loop, P2 save flow, P3 popup, and P4 next); **independent unit-testable tasks are
fanned out to subagents** (P2-T1 build_edit_form, T1.4 umlaut test); and **each
phase ends with a code-review subagent gate** (spec+quality), whose findings are
verified (not blindly applied) and fixed. Commit/verify is calibrated to the
plan's **green checkpoints**, not every checkbox.

## DONE (committed, reviewed, verified live against OpenLDAP)
- **P0** — deps swapped (ratatui 0.30 / tui-tree-widget 0.24 / tui-prompts 0.6 /
  crossterm 0.29), `facade.rs` deleted, empty 3-pane shell. Commit `6bb9ace`.
- **P1** — read-only browser. New `src/ui/{app.rs, view.rs, edit_form.rs}`.
  Umlaut render test re-bears the deleted `utf8_inputline_repro`; verified live
  German-on-scroll with no panic. Commits `6c76be0`, `800d26c` (review fix).
- **P2** — inline editing, `Overlay::{Confirm,Error}` + `PendingAction`, F2 save
  → LDIF confirm → Modify/ModRdn → re-read (persisted, ldapsearch-confirmed), F3
  revert, password masking. ALL write-path orchestration moved `main.rs`→
  `ui/app.rs`; `main.rs` is now just the CLI. Commits `d6037f2`, `69661fe`
  (review fix).
- **P3** — multi-value popup editor (`Overlay::ValueEditor`): Alt+↑↓ reorder,
  Alt+a/Insert add, Alt+d delete, F2 commit (drops empties), Esc cancel; secret
  rows masked; ordered/set hint. Verified live: reorder+save → "No changes"
  (set-wise §4 tie); add+save → real `add: cn`. Commit `27494c4`.
  **P3 review status: <PENDING — fill in after the background reviewer returns;
  apply + commit any fixes before starting P4>.**

State at handoff: crate is **`cargo clippy --all-targets` clean (no dead code)**,
**~159 lib tests pass**. Confirm with `cargo test` + `cargo clippy --all-targets -- -D warnings`.

## Architecture (where things live now)
- `src/ui/app.rs` — `App` state, `Overlay`/`PendingAction`/`PostWrite` enums,
  `run(config,password)`, `event_loop` (draw → drain `worker.poll()` → polled
  input → `reconcile`), `dispatch_key` (focus-gated, returns `Option<UiAction>`),
  `handle_action` (save/cancel), `handle_worker_response`, `reconcile`,
  `overlay_key`/`value_editor_key`/`execute_pending`, and the write-path helpers
  (`prepare_save`, `submit_prepared`, `compose_renamed_dn`, `next_id`, …).
  Worker/read_flow/structure/post maps are **`run()`/`event_loop` locals**; `App`
  holds only UI state — `draw`'s `&mut App` closure borrow can't collide.
- `src/ui/view.rs` — `ui`, `pane_block`, `render_tree/leaf/form`, `render_overlay`
  + `render_value_editor`, `clamp_scroll` (two-directional, pure, tested),
  `centered`, `field_display_value` (editor-aware + secret masking). **No
  byte-slicing of values anywhere** (the migration's whole point).
- `src/ui/edit_form.rs` — `EditField`/`EditForm`/`ValueEditor`, `build_edit_form`,
  `is_secret_attr`; `EditForm::{to_edit_entry, is_dirty (set-wise), baseline}`.
- `src/form/changeset.rs` — domain. `is_x_ordered` predicate added (NOT yet wired
  into `diff()` — that's P5).
- Domain layer (`ldap/`, `schema/`, `form/`, `workflows/`) ported **untouched**.

## Key implementation notes (carry these into P4+)
- **Event loop must stay `event::poll(50ms)` + drain every tick** (plan §2.2) —
  never blocking `read()`, or the LDAP worker starves.
- **Stale-read DN gate:** `handle_worker_response` installs a base-read form only
  when `last_seen_leaf.eq_ignore_ascii_case(model.title)` — drops overlapping
  stale reads. After a save, `rebind_selection(app, reread_dn)` sets both
  `last_seen_leaf` and `rows[leaf_sel].dn` so the re-read lands and `reconcile`
  doesn't fire a competing read of the old DN. (Full tree/structure reflow is P4.)
- **Overlays capture ALL keys:** `event_loop` routes to `overlay_key` first when
  `overlay.is_some()`; `reconcile` is skipped while an overlay is open.
- **Borrow pattern for commit-from-overlay:** `app.overlay.take()` to move the
  payload out, THEN mutate `app.form` (see `value_editor_key` F2 commit).
- Bare `q` quits only in the Tree pane (search/form need the key); Alt+X/Ctrl+C
  everywhere.

## NEXT — P4 (create / delete / guard / refresh / read-only) — HEAVY spine, in-session
Inputs already read & ready (all pure, ported untouched):
- `src/ui/form_state.rs` — `guard_decision(dirty, Option<GuardChoice>) ->
  GuardOutcome {Proceed, SaveThenProceed, Cancel}`.
- `src/workflows/create.rs` — `empty_form_for_profile(schema, profile) ->
  FormModel`; `build_add_entry(profile, container, rdn_value, edited) -> (dn,
  attrs)`. RDN value comes from the edited form's `profile.rdn_attr` field.
- `src/workflows/structure.rs` — `Structure::{add_child(parent, StructureInput)
  -> bool /*leaf→branch reflow*/, remove(dn) -> bool /*branch→leaf reflow*/}`.
- `src/app.rs` — `UiAction {Activate, FormSave, FormCancel, NewEntry(usize),
  DeleteEntry(String), Refresh, None}`, `build_menu_defs(&profiles)`,
  `menu_action(cm, profile_count, selected_dn) -> UiAction`, `CM_*` ids.
- `src/ldap/ldif.rs` — `render_add(&dn, &attrs)`; `render_changeset`.

P4 tasks (re-host the OLD `run_tui` arms — see git `74fcf50:src/main.rs` for the
exact bodies of the `UiAction::{NewEntry, DeleteEntry, Refresh}` + guard branches):
- **P4-T1 dirty-guard on nav:** in `reconcile` step 3, before navigating away
  while `app.form.is_dirty()`, open `Overlay::Guard` → map `GuardChoice` via
  `guard_decision` → Proceed / SaveThenProceed (run the save flow, defer nav) /
  Cancel (stay). Will need a `Overlay::Guard` variant + a pending "nav target".
- **P4-T2 create:** `NewEntry(i)` → `empty_form_for_profile` → a `CreateForm`
  overlay reusing the **same EditForm widget** (one editable-form impl, two
  hosts) → validate → LDIF confirm (`render_add`) → `Add` → on WriteOk
  `structure.add_child` + rebuild `tree_items`/`rows` (`PostWrite::Created`).
- **P4-T3 delete + refresh:** `DeleteEntry(dn)` → confirm → `Delete` → on WriteOk
  `structure.remove` + reflow (`PostWrite::Deleted`); `Refresh` → re-run
  `LoadStructure` → rebuild `tree_items`/`rows`. (Add `Created`/`Deleted` arms to
  `PostWrite`, currently only `Save`.)
- **P4-T4 read-only mode:** already partly wired (`app.read_only` disables
  editors via `build_edit_form`); ensure F2/F3/NewEntry/Delete are suppressed and
  the (P5) status line reflects it. A **menu bar** drives NewEntry/Delete/Refresh
  — likely a top line of `menu_action` hotkeys (menu bar itself is P5-T2).
- Structure reflow here also fixes the P2/P3 leaf-label/tree staleness after a
  rename.

## Then P5, P6
- **P5-T1 (FAN-OUT, changeset.rs only — safe to run in parallel with P4):** wire
  `is_x_ordered` into `diff()` — when ordered & both sides non-empty, compare
  order-sensitively and emit `Replace` with the full new ordered list on any
  diff; else keep set-wise. **ALSO** make `changeset.rs` `value_set_eq` symmetric
  (add the reverse subset check; latent asymmetry masked by LDAP per-attribute
  value uniqueness). 3 regression tests: `diff_pure_reorder_of_unordered_is_no_change`,
  `diff_reorder_of_x_ordered_emits_replace`, `diff_x_ordered_unchanged_is_no_change`.
- **P5-T2:** status line (read-only / dirty `*` / DN — pure `status_line(app)`),
  menu bar from `build_menu_defs`, cursor polish, error overlays for every worker
  error path (`format_validation_errors` already ported into app.rs).
- **P5-T3:** parity sweep vs plan §6 checklist.
- **P6:** confirm `grep -rn turbo_vision src/ Cargo.toml` empty (clean the
  remaining COMMENT mentions in `app.rs`/`workflows/browser.rs`/`form/mod.rs`/
  `workflows/mod.rs` — deferred from P0); `cargo clippy --all-targets -- -D
  warnings`, `cargo fmt`, full `cargo test`, final tmux smoke; then
  `superpowers:finishing-a-development-branch`.

## Run recipe (recreate each session — /tmp + podman don't persist)
```
cd /scratch/oetiker/claude-worktrees/ldapedit-feat-ratatui-migration
bash scripts/test-ldap.sh start          # OpenLDAP on ldap://localhost:1389
cargo build                              # binary: /home/oetiker/scratch/cargo-target/debug/edaptor
tmux new-session -d -s ed -x 120 -y 30 \
  "EDAPTOR_PW=adminpassword /home/oetiker/scratch/cargo-target/debug/edaptor --config /tmp/edaptor-try.toml"
tmux send-keys -t ed Down Enter ; tmux capture-pane -t ed -p ; tmux kill-session -t ed
```
Config `/tmp/edaptor-try.toml` (writable; bind cn=admin,dc=example,dc=org / pw
`adminpassword`); `/tmp/edaptor-try-ro.toml` adds `read_only=true`. Seed: ou=users
{user01,user02}, ou=groups{readers}. Capture colour with `tmux capture-pane -e`.
Quit the app with Alt-X. **Stop the container when done** (`scripts/test-ldap.sh`).
There is no on-screen status line yet (P5-T2), so verify "No changes" by the
ABSENCE of the confirm overlay.
```
