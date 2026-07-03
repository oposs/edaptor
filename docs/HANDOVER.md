# edaptor — Session Handover

Carries the **current concern** into the next session. Not a project history — for
that see `git log`, the specs under `docs/superpowers/specs/`, the SDD ledger
(`.superpowers/sdd/progress.md`), and project memory (`…/memory/MEMORY.md`).

**Date:** 2026-07-03 · **Current concern: finish the Shuttle buttons + resizable
dialog work** on branch `feat/shuttle-widget`. Tasks 1–4 of the plan are
**implemented and committed** (`make check` green); Task 4 is **not yet
peer-reviewed**, the interactive resize has **not been manually verified**, and
**Task 5 (docs/changelog) is not started**. Pick up exactly there.

---

## CURRENT CONCERN — execute the remaining plan steps

**Plan:** [`docs/superpowers/plans/2026-07-03-shuttle-buttons-resize.md`](superpowers/plans/2026-07-03-shuttle-buttons-resize.md)
**Spec:** [`docs/superpowers/specs/2026-07-03-shuttle-buttons-resize-design.md`](superpowers/specs/2026-07-03-shuttle-buttons-resize-design.md)
**SDD ledger (source of truth for progress):** `.superpowers/sdd/progress.md`

We are executing this plan with the **superpowers:subagent-driven-development**
skill: fresh implementer subagent per task → generate a review package →
task-reviewer subagent → fix loop → mark complete. Continue that loop.

### What the work delivers (user request, 2026-07-03)

The two picker dialogs embed a shared `Shuttle` two-list transfer widget. The user
reported: Add/Remove felt reversed between the Object Class and Edit Member dialogs;
Tab landed on the move buttons; and they wanted wide buttons under each list plus a
resizable dialog. The plan:

1. **Task 1 (done, commit `3fc128e`, reviewed clean):** dropped the
   `selected_on_left` flip — both dialogs now render **Available LEFT / Selected
   RIGHT** (conventional). `Shuttle::new(area, left_title, right_title, find_mode)`
   — the `selected_on_left` param is gone.
2. **Task 2 (done, commit `80316ae`, reviewed clean):** extracted geometry into a
   pure `Shuttle::layout(area) -> ShuttleLayout` (min-size clamp `MIN_W=60`,
   `MIN_H=20`); replaced the narrow bottom-left buttons with **wide Add spanning the
   Available (left) column** and **wide Remove spanning the Selected (right)
   column**; both buttons are **non-selectable** (`options.selectable = false`) so
   Tab skips them (still fire via click, Alt-A/Alt-R, Insert/Delete/Enter). Both
   dialogs grew from height **22 → 25**. The Shuttle's group carries
   `grow_mode.hi_x/hi_y` (inert until Task 4; reviewer flagged the scope but it's
   plan-mandated and harmless — KEPT).
3. **Task 3 (done, commit `71a9153`, reviewed clean):** focus-driven graying via
   `sync_move_commands(ctx)` — Add enabled ⇔ Available list focused, Remove enabled
   ⇔ Selected list focused; called from `reset_current` and the end of
   `handle_event`.
4. **Task 4 (done, commit `fc1ce0e`, `make check` green) — ⚠ NOT PEER-REVIEWED:**
   resizable dialogs. Hand-written `Shuttle::change_bounds(bounds)` (added to the
   `#[delegate(... skip(...))]` list) sets the group bounds and repositions every
   child via `layout(bounds)`; the `grow` window flag is set on both dialogs
   (auto-enables drag_grow); OK/Cancel anchored bottom-right via `grow_mode`
   (`lo_x/hi_x/lo_y/hi_y`). All six previously-`#[allow(dead_code)]` ids are now live
   in `change_bounds`. Used `self.group.state_mut().set_bounds(bounds)` (no fallback
   needed).

### DO THIS NEXT, in order

1. **Peer-review Task 4** (the SDD gate was skipped only to preserve context here):
   - `SKILL=~/.claude/plugins/cache/claude-plugins-official/superpowers/6.0.3/skills/subagent-driven-development`
   - `"$SKILL/scripts/review-package" 71a9153 fc1ce0e` → dispatch a task-reviewer
     subagent (template: `$SKILL/task-reviewer-prompt.md`) with the printed diff
     path, the brief `.superpowers/sdd/task-4-brief.md`, and the report
     `.superpowers/sdd/task-4-report.md`. Fix Critical/Important findings via a fix
     subagent, then mark Task 4 complete in the ledger.
2. **Manual resize verification** (Task 4 Step 8 — a human/tmux job the implementer
   could not do). Drive the real TUI (see the tmux recipe below). Confirm, in **both**
   the Object Class editor and a membership editor: Available LEFT / Selected RIGHT;
   Tab cycles the two lists → OK → Cancel and never the move buttons; Add grays
   unless the Available list is focused and Remove grays unless the Selected list is
   focused; Add/Remove still fire by click, Alt-A/Alt-R, Insert/Delete/Enter;
   dragging the lower-right corner (or Shift+Arrow) enlarges the dialog and columns +
   scrollbars + button rows + OK/Cancel reflow, with shrink stopping at the minimum.
   **Known limitation to sanity-check (documented in the plan):** the resize cascade
   calls `change_bounds` without a `Context`, so list scrollbar *page steps* are not
   refreshed during an interactive resize — cosmetic (PageUp/PageDown distance / thumb
   size), not a correctness issue. If it looks wrong in practice, the plan's Task 4
   Step 3 note describes the `on_bounds_changed` follow-up.
   ⚠ **Membership may not be live-drivable in the demo config** (see the note at the
   bottom of the old handover history / prior sessions: `memberOf` is
   overlay-maintained and not a reachable form field). If so, the objectClass editor
   exercises the identical shared Shuttle/resize code — verify there and note the
   membership limitation, don't chase a demo profile change unless asked.
3. **Task 5 — docs/changelog** (`"$SKILL/scripts/task-brief"
   docs/superpowers/plans/2026-07-03-shuttle-buttons-resize.md 5`): `CHANGES.md`
   entry, refresh `docs/src/configuration/widgets.md` (column sides + button layout +
   resizable), verify `README.md` doesn't contradict. Run `make docs`.
4. **Final whole-branch review** (most capable model) over the merge-base:
   `"$SKILL/scripts/review-package" $(git merge-base main HEAD) HEAD` → dispatch the
   final reviewer (`superpowers:requesting-code-review` template). Feed it the Minor
   roll-up below.
5. **Finish the branch** via **superpowers:finishing-a-development-branch** (present
   merge/PR/cleanup options to the user).

### Minor findings roll-up (carry to the final review; none block a task)

- Task 1: `shuttle()` test helper passes `left_title="Active"` / `right_title="Available"` — inverted vs the new semantics (plan-mandated; cosmetic).
- Task 1: `multi_picker` lost the comment explaining *why* it uses `FindMode::Highlight` (server-backed candidates re-queried on `LIST_FIND_CHANGED`). Restore that rationale when touching `multi_picker` for Task 5 if convenient.
- Task 3: no unit test for the "neither list focused → both buttons disabled" arm of `sync_move_commands` (behavior is implemented; just uncovered).

---

## How the SDD loop works here (so you can resume it)

- **Ledger first:** `cat .superpowers/sdd/progress.md`. Tasks marked complete are
  DONE — do **not** re-dispatch them. Resume at the first not-complete task (Task 4
  review, then Task 5).
- Per task: `scripts/task-brief PLAN N` → dispatch implementer (template
  `$SKILL/implementer-prompt.md`) with the brief path + a report path
  (`.superpowers/sdd/task-N-report.md`) + scene-setting context + global constraints
  → on DONE, `scripts/review-package BASE HEAD` (BASE = the commit before the task,
  from the ledger — never `HEAD~1`) → dispatch task-reviewer → fix loop → append one
  `Task N: complete (commit …, review clean)` line to the ledger.
- **Model choice:** implementers/reviewers on a mid-tier model (sonnet) have been
  fine for these mechanical, fully-specified tasks; the **final whole-branch review
  should use the most capable model**.
- Scratch lives under `.superpowers/sdd/` (git-ignored: briefs, reports, review
  diffs, ledger). `git clean -fdx` destroys it — recover the ledger from `git log`.

---

## Project state (history is elsewhere)

edaptor is a Rust TUI for administering OpenLDAP: it introspects live schema
(`cn=subschema`) and generates edit forms from `objectClass` definitions; a TOML
config declares connection settings plus *entry profiles* and a **widget palette**
(`[profile.widget.<attr>]` kinds: `choice` / `password` / `picker` / `membership`).
The UI is **tvision-rs** (`src/ui/`); `edaptor` is the sole binary. The tvision-rs
UI migration (M1–M5c) is complete and merged to `main`. `Cargo.toml` version
`0.4.0`.

**tvision-rs is now `0.9`** (not the 0.3 the older handover mentioned — that section
is stale). The `Shuttle` and both pickers are built against 0.9's three-surface
focus model and command-set graying.

### The Shuttle and its two consumers

- `src/ui/shuttle.rs` — the generic two-list transfer `View` (domain-free; a
  `Group` + two lists + two scrollbars + Add/Remove buttons, `#[delegate(to =
  group, skip(...))]`). Notifies owners by broadcast `CMD_SHUTTLE_CHANGED` (source =
  its own `ViewId`); the Available list's incremental find broadcasts
  `Command::LIST_FIND_CHANGED`. `ShuttleRow { key, label, locked }`. Pure move
  model = `ShuttleModel` (unit-tested without a Dialog).
- `src/ui/oc_picker.rs` — Object Class editor. Available list uses
  `FindMode::Filter` (static local set); reacts to `CMD_SHUTTLE_CHANGED` →
  `refresh_available` + `update_staged`.
- `src/ui/multi_picker.rs` — Edit Member editor. Available list uses
  `FindMode::Highlight` (server-backed; re-queries via the async worker on
  `LIST_FIND_CHANGED`).

### Still-open larger concern (NOT this task): upstream the Shuttle

`src/ui/shuttle.rs` is explicitly built to graduate **upstream to tvision-rs**
(oetiker's repo). Directive: separate clone, one focused PR; edaptor depends on the
published crate (git pin only as fallback). When it lands, delete `shuttle.rs` and
re-point the two consumers. Do this **after** the current buttons/resize branch is
finished and merged. See project memory `shuttle-widget-incubation`.

---

## Build / test / run

**⚠ Cap parallelism at 4 cores** (shared box).

```bash
make check                                        # fmt + clippy -D warnings + tests — the gate
cargo test -j4 --lib ui::shuttle ui::oc_picker ui::multi_picker
cargo clippy -j4 --all-targets -- -D warnings
make docs                                         # build the mdBook (Task 5)

# Live LDAP demo (podman): ~600 users / ~25 groups, ldap://localhost:1389
scripts/test-ldap.sh start
export EDAPTOR_TEST_ADMIN_PW=adminpassword
cargo run -- --config examples/demo-config.toml
```

Facade guards (must print nothing — only `src/ui/**` may use tvision_rs):
```bash
! grep -rl "use tvision_rs" src | grep -vE "^src/ui/"
! grep -rl "use ratatui\|use tui_" src
```

## ⭐ Live-driving the TUI from an agent session (tmux) — for the manual resize check

```bash
scripts/test-ldap.sh start                       # podman demo server (idempotent)
cargo build -j4 --bin edaptor
tmux kill-session -t edtv 2>/dev/null
tmux new-session -d -s edtv -x 210 -y 50
tmux send-keys -t edtv 'export EDAPTOR_TEST_ADMIN_PW=adminpassword' Enter
tmux send-keys -t edtv 'cargo run -j4 --bin edaptor -- --config examples/demo-config.toml' Enter
sleep 5
tmux send-keys -t edtv Down; sleep 0.4
tmux capture-pane -t edtv -p | sed -n '2,40p'    # read the screen
# Resize the dialog: focus it, then Shift+Arrow grows it (drag_grow). E.g.
# tmux send-keys -t edtv S-Right S-Down ; capture again to see the reflow.
tmux kill-session -t edtv                         # clean up (holds an LDAP bind)
```

Insert `sleep` between keystrokes (async reads land via the 50ms pump). Focus
probes: `tmux capture-pane -e` renders the focused element bg bright-green
`(0,170,0)`; `tmux display-message -p '#{cursor_x}'` locates the focused column.
Prefer edit-then-Discard; do not trigger destructive saves against demo data.

---

## Conventions (follow these)

- **Pull first** each session (`git pull --ff-only`); if not a clean fast-forward,
  stop and surface it. (This branch `feat/shuttle-widget` has no upstream tracking —
  that is expected; work commits directly on it.)
- **Facade boundary:** only `src/ui/**` may `use tvision_rs`. Domain layer stays
  UI-agnostic.
- **Borrow discipline:** never hold a `RefCell`/`UiState` borrow across
  `ctx.broadcast`/`ctx.post`/`exec_view`/`worker.submit`/`new_list`/`child_mut`/
  `set_value`. Collect into locals → drop the borrow → call.
- **Strict TDD**, atomic commits, crate compiles after every commit, `cargo fmt` +
  clippy `--all-targets -D warnings` clean before each commit.
- **Docs one-home:** config detail → mdBook (`docs/src/`); README orientation only;
  `CHANGES.md` for every user-visible change; process/design → `docs/superpowers/`.
- **Commit trailer:** `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
  Use `git commit -F` (file/heredoc) for messages containing backticks.

---

## Load-bearing tvision-rs 0.9 facts (so you don't rediscover them)

- **Command-set graying:** `ctx.enable_command(cmd)` / `disable_command(cmd)` push
  `Deferred::EnableCommand`/`DisableCommand`; the loop flips `COMMAND_SET_CHANGED`
  on the next idle pump, and each `Button` re-grays itself from
  `ctx.command_enabled(command)`. This is how Task 3's graying works.
- **Non-selectable buttons:** `state.options.selectable = false` removes a view from
  Tab traversal but does NOT disable pre/post-process — so Alt-hotkeys and clicks
  still fire. (Task 2 uses this for Add/Remove.)
- **Resize:** setting the dialog `grow` `WindowFlags` auto-enables `drag_grow`
  (`Window` derives `mode.drag_grow = flags.grow`). The window-resize cascade is
  `Group::change_bounds` → each child's `calc_bounds` + `change_bounds` — **no
  `Context`, no `on_bounds_changed`** for children. Per-child `grow_mode`
  (`lo_x/hi_x/lo_y/hi_y`, `rel`, `fixed`) drives the built-in reflow; a two-column
  split can't be expressed by simple grow_mode deltas, so the Shuttle overrides
  `change_bounds` and repositions children explicitly from `layout(bounds)`.
- **`#[delegate(to = group, skip(...))]`** forwards the `View` impl to the embedded
  `Group`; hand-written methods go in `skip(...)`. Shuttle now skips
  `handle_event, as_any_mut, reset_current, value, set_value, set_value_ctx,
  change_bounds`.
- **`reset_current` is THE modal-open init hook** (before first draw/event). Seed
  lists / stage state / set initial focus + command graying there.
- **Headless view tests:** `Context::new(&mut out, &mut timers, 0, &mut deferred)`
  with `tv::timer::TimerQueue::new()` and `Vec<tv::Deferred>`. The `shuttle.rs` test
  module `Harness` is the template (`broadcast_seen`, `command_disabled`).
- **`WindowFlags`** is re-exported at the crate root: `tv::WindowFlags` (fields
  `r#move`, `close`, `grow`, `zoom`). `Dialog::set_flags`, `Dialog::child_mut`,
  `Button::state_mut`, `ViewState::set_bounds` are all public.
