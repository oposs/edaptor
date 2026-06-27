# edaptor — Session Handover

Carries the **current concerns** into the next session. Not a project history —
for that see git log, the specs under `docs/superpowers/specs/`, the SDD ledger
(`.superpowers/sdd/progress.md`), and project memory (`…/memory/MEMORY.md`).

**Date:** 2026-06-27 · **Where we are: the tvision-rs UI migration is mid-flight.
M1 (read core) and M2 (edit + write spine) are COMPLETE, reviewed, and
LIVE-ACCEPTED. edaptor depends on the released `tvision-rs` 0.3.0 (no pin) and the
main window runs frameless full-screen (`Fullscreen::Desktop`). M3 was split into
Phase 1 (stabilize) + Phase 2 (the M3 core); Phase 2 was further split into
2a (objectClass widget + live resync) + 2b (create flow). **M3 Phase 1 and
Phase 2a are both COMPLETE and LIVE-ACCEPTED.** Phase 2a executed subagent-driven
(8 TDD tasks, each reviewed clean), two fix passes (reset_current list seed; a
borrow-panic on picker open found in live acceptance), a whole-branch review
(COMPLETE & SOUND with fixes, now applied), and a full live tmux acceptance.
**M3 Phase 2b (the create flow) is next.**

> **▶ NEXT ACTION — start M3 Phase 2b (the create flow).** Phase 2a landed on
> `feat/tvision-ui` @ `1e1263d` (527 lib tests, clippy/fmt clean, `make check`
> green, both facade guards clean). Begin 2b with its own brainstorming →
> writing-plans cycle: Alt+N profile chooser, single-profile fast path,
> `FormMode::Create`, create-mode form, objectClass auto-injection, and the
> `plan_create`/`build_add_entry` submit path. The neutral `workflows::create`
> helpers are already implemented + tested; 2a's modal seam + `sync_schema_fields`
> + `apply_commit` are the machinery 2b reuses. Spec+plan for 2a:
> [`specs/2026-06-27-tvision-m3-phase2a-objectclass-resync-design.md`](superpowers/specs/2026-06-27-tvision-m3-phase2a-objectclass-resync-design.md),
> [`plans/2026-06-27-tvision-m3-phase2a-objectclass-resync.md`](superpowers/plans/2026-06-27-tvision-m3-phase2a-objectclass-resync.md).
>
> _Phase 2a load-bearing facts (so the next session doesn't rediscover them):_
> - **`reset_current` is THE modal-open init hook** in tvision-rs 0.3.0
>   (`program.rs:1710`, runs before first draw/event), NOT `on_bounds_changed`
>   (dead for modal inserts — `Group::insert` calls `set_bounds` directly). Any M4
>   modal widget that must seed a list / stage state on open uses `reset_current`.
> - **Borrow trap (fixed, but the pattern recurs):** a `FieldEditor` must NOT
>   `borrow_mut()` the shared state during construction/`into_view`, because
>   `dispatch` holds `state.borrow()` to pass the schema in. Stage in
>   `reset_current` (borrow-safe, post-dispatch-borrow), not in `new()`.
> - The objectClass **field's values are authoritative** for `sync_schema_fields`;
>   `EditForm.object_classes` is a mirror kept by `apply_commit` for the save path.
> - `widget_for(field)` routes objectClass→picker else plain; M4 extends it to
>   dispatch on `field.widget_binding` with no form-core change.
>
> _Closed P1 items, for the record:_
> 1. ✅ **RESOLVED — bottom `░` strip (`bc64274`).** Root cause (verified vs
>    tvision-rs 0.3.0 source): tvision refits nested views via `grow_mode`
>    (`calc_bounds`) only — it calls `on_bounds_changed` solely on the window's
>    direct body, NEVER on splitter-nested panes. The tree filled because `Outline`
>    self-sets `grow_mode {hi_x,hi_y}`; the leaf `ListBox`/search and form
>    header/`ScrollGroup` set none → didn't track the pane → strip. The earlier
>    `FormPane`/`LeafPane::on_bounds_changed` (M3 P1 Tasks 6–7) were the wrong
>    mechanism — DEAD CODE for nested panes. Fix: set `grow_mode` on the pane
>    children (search/header `hi_x`; list/ScrollGroup `hi_x+hi_y`), delete the dead
>    overrides, and move `ScrollGroup`'s resize-recompute into a `change_bounds`
>    override (the hook the framework actually calls). NOT a tvision bug.
>    Live-verified: no strip at launch or after shrink/grow resizes. Minor residual
>    (cosmetic, logged): `ScrollGroup` content-cell *widths* only re-fit on a DN
>    change, so a width-only resize leaves value cells their old width until you
>    pick another entry.
> 2. ✅ **RESOLVED — interactive guard-edge sign-off (agent-driven tmux PTY).** All
>    three deferred flows live-verified; demo data intact afterward (no write ever
>    submitted). **A — keyboard scroll-to-focused:** PASS (viewport scrolls to keep
>    focused editable field visible; pinned DN header stays). **B — guard #2
>    (cancelled-confirm snap-back):** PASS (Save→confirm→cancel leaves form pinned +
>    leaf highlight snaps back to the current entry). **C — guard #3
>    (branch-change-while-dirty):** PASS (Stay reverts the tree highlight to the
>    current branch). Bonus: Alt-X quit-guard→Discard exits clean. **NEW FINDING:** a
>    single left-click on the `Outline` (tree) only FOCUSES the pane — it does NOT
>    move the branch selection, so the branch guard is reached via keyboard arrows,
>    not a click (the leaf pane's `first_click` DOES select). Reliable PTY focus
>    probes (for the next driver): `tmux display-message -p '#{cursor_x}'` locates
>    the focused widget by column; `tmux capture-pane -e` renders the focused element
>    bg bright-green `(0,170,0)`. Still only unit-reasoned (not exercised live):
>    branch Save-Submitted / branch Discard dispatch routing.
>
> Full per-task + review ledger: `.superpowers/sdd/progress.md` (M3 P1 section).
> The original plan + spec (now executed):
> [`plans/2026-06-27-tvision-m3-phase1-stabilize.md`](superpowers/plans/2026-06-27-tvision-m3-phase1-stabilize.md),
> [`specs/2026-06-26-tvision-m3-phase1-stabilize-design.md`](superpowers/specs/2026-06-26-tvision-m3-phase1-stabilize-design.md).

`edaptor` is a Rust TUI for administering an OpenLDAP directory. It introspects
live schema (`cn=subschema`) and generates edit forms from `objectClass`
definitions; a TOML config declares connection settings plus *entry profiles* and
a **widget palette** (`[profile.widget.<attr>]` kinds: `choice` / `password` /
`picker` / `membership`). Two UIs coexist during the migration: the shipping
**ratatui** UI (`src/ui/*`, ~10k LOC, the `edaptor` binary) and the new
**tvision-rs** UI (`src/tui/*`, the `edaptor-tv` dev binary).

---

## Git topology (read before you branch)

- **Branch `feat/tvision-ui`** (HEAD `93b84cc`: full-screen + 0.3.0 shipped, M3
  Phase 1 spec+plan committed) — the long-lived migration branch. ALL migration work
  lives here. **It does NOT merge to `main` until the M5 cutover** (umbrella §7) —
  the ratatui UI stays the shipping `edaptor` binary through M1–M4.
- `main` is behind/unpushed (origin at v0.4.0). Pushing / CI / release are a
  separate concern, not gated by the migration.
- `Cargo.toml` version is `0.4.0`. Dependency: **`tvision-rs = "0.3"`** from
  crates.io (a plain release dep — no git pin, no patch; the `exec_view_focused`
  pin was dropped once 0.3.0 shipped).

The migration is governed by the **umbrella design**:
[`docs/superpowers/specs/2026-06-23-tvision-ui-migration-umbrella-design.md`](superpowers/specs/2026-06-23-tvision-ui-migration-umbrella-design.md)
— read §6 (milestone sequence) and §4 (the FieldWidget plugin contract) first.
Each milestone gets its own spec → plan → implement cycle.

---

## What's done

**M1 — three-pane read core** (plan `plans/2026-06-24-tvision-m1-read-core.md`).
DIT `Outline` (tree) | leaf `ListBox`+search | read-only form `Group`, driven by
the unchanged domain layer via `workflows::read_flow`/`form_model`. The
`FieldWidget` trait + registry skeleton (`src/tui/widget.rs`), `present()` only.

**M2 — edit + write spine** (spec `specs/2026-06-25-tvision-m2-edit-write-design.md`,
plan `plans/2026-06-25-tvision-m2-edit-write.md`). 9 tasks, subagent-driven,
reviewed clean (whole-branch review verdict READY). Delivered:
- `workflows::edit_form` — neutral editable model (values + baseline + set-wise
  dirty + `to_edit_entry`). A FRESH parity copy of `ui::edit_form`; **dedup at M5**
  (the duplication is intentional — do not "fix" it).
- `workflows::write_flow` — `WriteFlow::{prepare,submit,on_response,submit_followup}`:
  validate+diff via `workflows::save::prepare_save`, submit to the worker, correlate
  `WriteOk`/`WriteError`; **MODIFY + MODRDN** (incl. rename-then-modify two-step).
  `prepare`/`on_response` are pure; submit is a thin worker wrapper.
- `tui::widget` — `present(&EditField)`, `activate()→Inline`, `inline_editable` gate.
- `tui::state` — `UiState` holds `edit_form`/`write_flow`; `pump_worker` routes
  reads then writes → `PumpResult{changed,quit,error}`.
- `tui::panes::form` — editable pane; `tui::dialog::{confirm,error,guard}` +
  `guard_decision`; `tui::app` — the single `run_app` dispatch closure (the only
  `exec_view` site) wiring Save/Exit, dirty-nav guard, deferred-quit.
- Gated live test `tests/tv_edit_write.rs` (skips unless `EDAPTOR_TEST_LDAP_URI`).

**Post-M2 navigation fixes** (found in the first live tmux acceptance pass — M1's
navigation had never been driven interactively):
- DIT `branch_dns` were built in reversed sibling order → wrong leaves shown. Fixed.
- Leaf pane gated the read on a key the `ListBox` had already consumed → selecting
  a leaf never loaded the form. Now detects the change via `value()`. Fixed.
- Form pane indexed past the 32-cell pool for entries with >32 attrs (panic). Now
  `take(FORM_ROWS)`-bounded (graceful truncation). Fixed.
- `edaptor-tv` now accepts `--config <path>` (was positional-only).
- **Intra-pane keyboard nav**: arrows navigate within a pane (leaf list while the
  search box keeps focus; form fields). Tab switches between panes/widgets.

**tvision-rs upstream improvement** (oetiker's repo; edaptor is its first real
consumer). The pane-focus gap (panes nested in a Splitter were unreachable by Tab)
was fixed UPSTREAM as **hierarchical Tab focus traversal** — Tab/Shift-Tab walk the
focusable-leaf tree across nested groups. Shipped in **tvision-rs 0.2.0**; edaptor
depends on the release. PR #5 (merged). The Splitter is now transparent to focus.

---

## M2 SAVE + GUARDS — live-accepted (the gate is now CLOSED)

The save round-trip and guard dialogs were driven live in tmux and **a real bug
was found and fixed before it could reach M3**:

- **Symptom:** edit a plain attr → Alt-S → confirm dialog → Enter → dialog closed
  but nothing persisted, form stayed dirty, no error.
- **Root cause:** on modal open, tvision's `first_match_visible_selectable`
  focuses the *last-inserted* selectable child — the **Cancel/Stay** button. A
  focused non-default button becomes the acting default, so Enter fired Cancel and
  `do_save` early-returned. Faithful Turbo Vision behaviour; `message_box` dodges
  it via `initial_focus`, but edaptor's bare `exec_view` passed `None` and the
  public API had no way to set focus.
- **Fix (upstream, user-chosen):** tvision-rs **PR #6** adds
  `Program::exec_view_focused(view, focus)`. edaptor's `confirm`/`guard`/`error`
  builders now return `(view, default_btn_id)` and `app::dispatch` calls
  `exec_view_focused`, so dialogs open with Save/OK focused. edaptor is git-pinned
  to the PR commit until it releases (see the banner at the top).
- **Verified live:** save persists to LDAP + dirty `*` clears + re-read; confirm
  Esc keeps editing; quit guard (Enter=Save→confirm, Stay, Discard=quit-no-save);
  dirty-nav guard fires and Discard navigates + drops the edit. Demo data restored.

Useful detail discovered while driving: **only single-valued attributes are
inline-editable** in the form (uidNumber, gidNumber, homeDirectory, sambaSID,
displayName, employeeNumber, gecos, loginShell for a posix/samba user) — arrow nav
cycles exactly those; multi-valued-capable attrs (cn, sn, mail, description, …) are
intentionally skipped (the `present_field` multi-value short-circuit). Also: a
focused `InputLine` selects-all, so typing replaces the whole value.

---

## ⭐ Live-driving the TUI from an agent session (tmux)

**The old handover said interactive checks need a human — that is SUPERSEDED.** You
can drive the real TUI yourself over a PTY with tmux (no human needed):

```bash
scripts/test-ldap.sh start                       # podman demo server (idempotent if up)
tmux kill-session -t edtv 2>/dev/null
tmux new-session -d -s edtv -x 210 -y 50         # wide enough for 3 panes
tmux send-keys -t edtv 'export EDAPTOR_TEST_ADMIN_PW=adminpassword' Enter
tmux send-keys -t edtv '/home/oetiker/scratch/cargo-target/debug/edaptor-tv --config examples/demo-config.toml' Enter
sleep 4
tmux send-keys -t edtv Down       # keys: Down/Up/Tab/Enter, or a literal like 7 / 'User2'
sleep 0.4
tmux capture-pane -t edtv -p | sed -n '2,14p'   # read the screen
tmux kill-session -t edtv         # clean up (the run holds an LDAP bind)
```
Notes: build the binary first (`cargo build -j4 --bin edaptor-tv`) so the run is
fast; the binary is at `/home/oetiker/scratch/cargo-target/debug/edaptor-tv` (NOT
`./target`). For modals (Confirm/Guard) send the button hotkey or arrows+Enter.
Do NOT trigger destructive saves carelessly against the demo data; prefer a temp
entry, or edit-then-Discard. Insert `sleep` between keystrokes (async reads land
via the 50ms pump).

---

## M3 is split into two cycles (each its own spec → plan → implement)

The user approved splitting M3 so the carried-forward problems are fixed first,
because the create flow reuses the form pane and the tree-guard machinery.

### ✅ Phase 1 — stabilize the base (DONE — signed off, strip fixed + guard edges live-verified)

Spec [`specs/2026-06-26-tvision-m3-phase1-stabilize-design.md`](superpowers/specs/2026-06-26-tvision-m3-phase1-stabilize-design.md),
plan [`plans/2026-06-27-tvision-m3-phase1-stabilize.md`](superpowers/plans/2026-06-27-tvision-m3-phase1-stabilize.md).
**12 TDD tasks**, foundation-first:
- **Tasks 1–4 — `ScrollGroup`** (NEW, domain-free, `src/tui/scroll_group.rs`): a
  generic vertical scroll-container of child views. tvision-rs has NO scroll
  container for child widgets (Group has no child offset; Scroller is self-drawn) —
  so we build one: it holds child views at logical positions and repositions them
  by `-top` on scroll (the framework clips offscreen children via `DrawCtx::sub`),
  driving a linked `ScrollBar` through the `ScrollSync` broker, with scroll-to-
  focused. Feasibility was spiked at the source level (PASS). **It is built for
  extraction → a follow-up upstream tvision-rs PR once proven (the user wants this
  contributed); edaptor then switches to the published widget.**
- **Tasks 5–6 — `FormPane` on `ScrollGroup`:** one persistent `Label`+`InputLine`
  per field, rebuilt per entry (`Group::remove`/`insert`), **drops `FORM_ROWS=32`**;
  `on_bounds_changed` refit. Editing reuses the M2 inline path unchanged (each
  field owns its cell → no scroll-time edit smear).
- **Task 7 — `LeafPane` fill** (`on_bounds_changed`) → kills the `▒` strip.
- **Task 8 — guard edge #2** (cancelled confirm snaps back like Stay).
- **Tasks 9–11 — guard edge #3** (branch nav controller mirroring the leaf path:
  `requested_branch`/`reconcile_branch`/`GuardTarget` enum, `TreePane` pure-selector,
  pump + dispatch with `set_tree_row` snap-back on Stay).
- **Task 12** — `CHANGES.md`, facade guards, `make check`, live tmux acceptance.

Execute **subagent-driven** (fresh subagent per task + spec-then-quality review;
final whole-branch review). The plan has complete per-task code; verified 0.3.0 API
facts are listed in its self-review.

### ✅ Phase 2a — objectClass widget + live resync (DONE — live-accepted @ `1e1263d`)

The riskiest half of the M3 core, on EXISTING entries. Delivered: the first reusable
modal-widget seam (`Activation::Modal(Box<dyn FieldEditor>)` + generic `app::dispatch`
ACTIVATE path); the objectClass picker (`src/tui/oc_picker.rs` — schema-seeded
multi-select `ListBox`, client substring filter, pre-tick, staged
`SetValuesThenResyncSchema`); the neutral `EditForm::sync_schema_fields` port (+
`EditField::injected`, `order_fields`); `UiState::{activate_field, staged_commit,
apply_commit}`; form-pane modal-row focus/Enter→ACTIVATE; gated `tests/tv_objectclass.rs`.
**Accept (met):** editing objectClass on an existing entry adds/orphans fields live via
the typed outcome (no global flag). See the spec/plan dated 2026-06-27 and the NEXT
ACTION banner's load-bearing facts (`reset_current` hook; the construction-time
borrow trap; objectClass-field-is-authoritative).

### ▶ Phase 2b — the create flow (NEXT, not yet specced)

Per umbrella §6 M3, the remaining half: Alt+N profile chooser dialog; single-profile
fast path; `FormMode::Create`; create-mode form; objectClass auto-injection on create;
the `plan_create`/`build_add_entry` submit path. **Accept:** create a new entry from a
profile end-to-end. Start with its own **brainstorming → writing-plans** cycle. The
neutral layer is well-prepared: `workflows::create` has `profiles_for_container`,
`empty_form_for_profile`, `build_add_entry`, `plan_create`, defaults/autonumber/
password-fold (all tested); Phase 2a's modal seam, `sync_schema_fields`, and
`apply_commit` are the machinery 2b reuses. The create form has a wrinkle to resolve
in design: `empty_form_for_profile` excludes objectClass from fields, but the
objectClass widget needs a row to activate the picker — decide how create-mode seeds
the objectClass field (and pre-injects the profile's classes).

---

## Nav/guard model (redesigned — read before touching the panes)

Entry-switching was reworked into a **controller-owned transition** (the old
per-pane `submit_selected` poll that posted `GUARD_NAV` only worked for keyboard —
mouse selection runs inside a tvision mouse-track capture, and `program.rs:2267`
skips the app handler when a capture consumes the event, so the pane's posted
command was swallowed). The model now (user-chosen "B"):

- **The form follows the highlight.** Clean form → moving the highlight loads that
  entry. Dirty form → it is **pinned**: no other entry is shown until the guard
  (Save / Discard / Stay) is resolved; **Stay** snaps the highlight back to the form.
- **Panes are pure selectors.** `LeafPane::report_selection` only records
  `UiState::requested_leaf`; it never reads, guards, or posts. The **pump** calls
  `UiState::reconcile_selection` each tick (clean → load; dirty → stash
  `guard_target`, post `GUARD_NAV` from its clean, capture-free context) and
  `app::dispatch` opens the modal; Stay sets `set_leaf_row` (= `current_leaf_row`)
  so the pane snaps the highlight back. Any future trigger (M3 create-flow, tree)
  should funnel through `requested_leaf` → `reconcile_selection`, NOT re-poll.

**Known edges — both now SCHEDULED in the Phase 1 plan (no longer loose TODOs):**

- ✅ FIXED: TV first-click on an unfocused pane only focused it. The leaf/form
  panes now set `options.first_click = true` (the tree got it free via the
  Outline), so a single click both focuses the pane and lands on the row/field.
- 📋 **#2 (Phase 1 Task 8):** guard→Save then **cancelling the confirm** leaves the
  list highlight on the would-be target while the form stays pinned (mismatch).
  Fix: treat a cancelled confirm like "Stay" (`set_leaf_row = current_leaf_row()`);
  `do_save` returns a `SaveOutcome`, dispatch snaps back on `NotSubmitted`.
- 📋 **#3 (Phase 1 Tasks 9–11):** changing **branch** while dirty guards but Stay
  can't snap back. Fix: extend the controller-owned model to the tree
  (`requested_branch`/`reconcile_branch`/`GuardTarget::{Leaf,Branch}`), `TreePane`
  becomes a pure selector, Stay reverts via `set_tree_row = current_branch_row()`.

## Deferred to M3 / cleanup (logged from M2 reviews)

- **Scrollable form + pane fill** — DONE in the Phase 1 plan (Tasks 1–7): the
  `▒` strip and the `FORM_ROWS = 32` cap both go away via the new `ScrollGroup`
  (form) and `on_bounds_changed` (leaf). The full-screen flip exposed the `▒`
  strip; root cause is the panes not re-fitting children on resize, not the flip.
- Minor/cosmetic: `value_set_eq` duplicate-value false-positive (shared with
  ratatui; fix at M5 dedup); `EditForm::set_value` has no `!multi` guard (only
  single-value callers in M2); read-error shows a stale form (status-only); a few
  `let _ = ctx/REFRESH` import/param silencers in `form.rs`; dialog module/builders
  are `pub` (could tighten to `pub(crate)` now that `app::dispatch` consumes them).

---

## Build / test / run

**⚠ Cap parallelism at 4 cores** (shared box). Target dir `/home/oetiker/scratch/cargo-target`.

```bash
cargo build -j4                 # both bins (edaptor ratatui + edaptor-tv tvision)
cargo build -j4 --bin edaptor-tv
cargo test  -j4                 # ~495 lib tests + gated integration (skip w/o env)
cargo clippy -j4 --all-targets -- -D warnings
cargo fmt --check
make check                      # fmt + clippy + tests

# Live LDAP demo (podman): ~600 users / ~25 groups, ldap://localhost:1389
scripts/test-ldap.sh start
export EDAPTOR_TEST_ADMIN_PW=adminpassword
# gated live tests:
EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389 cargo test -j4 --test tv_edit_write
```

Facade guards (must print nothing):
```bash
! grep -rl "use tvision_rs" src | grep -vE "^src/(tui/|bin/edaptor-tv.rs)"
! grep -rl "use ratatui\|use tui_" src | grep -vE "^src/ui/"
```

---

## Conventions (follow these)

- **Facade boundary (core migration rule):** only `src/tui/**` and
  `src/bin/edaptor-tv.rs` may `use tvision_rs`; only `src/ui/**` may
  `use ratatui`/`use tui_*`. The domain layer (`config`, `form`, `ldap`, `schema`,
  `samba`, `workflows`) imports NEITHER and stays UI-agnostic.
- **Don't touch `src/ui/**` (ratatui).** It's the running binary; the neutral
  models are introduced fresh in `workflows::*` and the ratatui tree is deleted
  wholesale at the M5 cutover. Editing the live UI mid-migration is the wrong risk.
- **Widget palette** is the one config-driven "rich field" home: a
  `[profile.widget.<attr>]` `kind` → `config::widget::WidgetKind`. The form honours
  `EditField.widget_binding`. M4 adds the rich widgets (choice/password/picker/
  membership) as `FieldWidget` impls with NO form-core changes.
- **Borrow discipline:** never hold a `RefCell`/`UiState` borrow across
  `ctx.broadcast`/`ctx.post`/`Program::exec_view`/`worker.submit`/`new_list`/
  `child_mut`/`set_value`. Collect into locals → drop the borrow → call.
- **Strict TDD**, atomic commits, crate compiles after every commit, `cargo fmt`
  before each commit, clippy `--all-targets -D warnings` clean.
- **Live tests gated** by `EDAPTOR_TEST_LDAP_URI` (skip when unset). Base
  `dc=example,dc=org`, password env `EDAPTOR_TEST_ADMIN_PW=adminpassword`.
- **Docs one-home:** config detail → mdBook (`docs/src/`); README orientation only;
  `CHANGES.md` for every user-visible change (the tvision preview already has
  entries). Process/design → `docs/superpowers/`.
- **Commit trailer:** `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
  ⚠ Use `git commit -F` (a file/heredoc) for messages with backticks — `-m "...`...`"`
  triggers shell command-substitution and mangles the message.
- **Execution style:** subagent-driven (fresh subagent per task + two-stage review).
  One editor per working tree at a time.

---

## Load-bearing tvision-rs facts (0.3.0 — so you don't rediscover them)

**New in 0.3.0 / discovered this session (Phase 1 planning):**
- **Frameless full-screen:** `Window::set_fullscreen(Fullscreen::{Off,Desktop,Screen})`
  composes maximize + border-drop, and `Command::FULLSCREEN` cycles it. edaptor's
  pump posts `Command::FULLSCREEN` once on its first tick (the pane can't downcast to
  `Window`; the command routes to the desktop's only window). `Desktop` keeps the
  menu bar + status line. The window's **drop shadow is disabled** (it would paint a
  one-cell strip over the desktop along the right/bottom edges).
- **No scroll-container for child views** (this is WHY Phase 1 builds `ScrollGroup`):
  `Group` has no child scroll offset; `Scroller` is for self-drawn content only. But
  **repositioning children works**: `Group::draw` draws each child through
  `ctx.sub(child_bounds)` and `DrawCtx::sub` clips to `parent_clip ∩ child_bounds`
  (context.rs:910), so a child moved to negative-y / past the bottom clips at the
  edge; mouse routing is `bounds.contains` + local translate (sign-agnostic).
- **Scroll broker:** a content view holds only its bar's `ViewId`. Publish range/value
  with `ctx.request_scroll_bar_params(bar, value,min,max,page,arrow)`; on the bar's
  `SCROLL_BAR_CHANGED { source }` broadcast call `ctx.request_scroll_sync(self_id, h, v)`;
  the pump resolves and calls your overridden `View::apply_scroll_sync(h,v,ctx)`
  (defaulted no-op). `ScrollBar::new(rect)` infers vertical when width==1.
- **`Group::remove(id, ctx)`** exists (alongside `insert`) — so dynamic per-entry
  child rebuild is fine (the Phase 1 FormPane uses it).
- **Headless DRAW tests:** `Buffer::new(w,h)` + `DrawCtx::new(&mut buf, &theme, clip,
  origin)` + `buf.get(x,y).symbol()`; `Theme::classic_blue()`. The crate's own
  `fill_clips_to_clip_rect` / `sub_narrows_clip_and_shifts_origin` are the templates.
- Crate-root re-exports incl. `Buffer, DrawCtx, Point, ScrollBar, GrowMode, StaticText,
  Label, Outline` (lib.rs:124–154). Hide a view by `state_mut().state.visible = false`.

**Carried from 0.2.0:**

- **Hierarchical Tab (new in 0.2.0):** Tab/Shift-Tab walk the focusable-leaf tree
  across nested groups; the Splitter is transparent to focus. Consequence for a
  pane-heavy UI: EVERY focusable widget is a Tab stop (e.g. leaf search box AND
  list are separate stops) — more granular than "Tab between panes". edaptor adds
  arrow-key intra-pane nav to mitigate. A widget that owns Tab (multi-line editor)
  still consumes it.
- **App skeleton:** `Program::new(backend, clock, theme, init_desktop,
  init_status_line, init_menu_bar)` then `program.run_app(|prog, cmd| …)`. The
  `init_*` factories take capturing closures, so `Rc<RefCell<UiState>>` reaches the
  view tree (no thread_local). **`run_app`'s `(prog, cmd)` closure is the ONLY
  place with `&mut Program`** → the single `exec_view` site (`tui::app::dispatch`).
- **`Program` has NO `post`/`broadcast`.** End the app from the dispatch closure
  via `prog.end_modal(Command::QUIT)`. Panes/pump request modals by POSTING a
  custom command (`ctx.post(cmd)`) that surfaces to `run_app`'s closure;
  `Command::QUIT` is consumed by the built-in handler before the closure sees it
  (hence the custom `REQUEST_QUIT`). `exec_view(view) -> Command` blocks but the
  pump timer keeps firing inside it (so async writes finish with a dialog open).
- **Worker → views:** zero-area `PumpView` + `Context::set_timer(50ms)` periodic
  `Event::Timer` → drain `worker.poll()` → correlate via `read_flow`/`write_flow`
  → `ctx.broadcast(REFRESH)`. Broadcasts reach every view incl. zero-area/disabled.
- **`Outline` (0.1.1+):** auto-seeds scrollbar/focus on first display; call
  `tv::ov_update` only after MUTATING the tree. Read selection via
  `Outline::value() -> Some(FieldValue::Int(foc))` (parity with `ListBox`).
- **`ListBox` consumes (clears) Up/Down/PageUp/PageDown.** Detect a selection
  change via `value()` vs a saved index — do NOT gate on `ev` still being a
  KeyDown after `group.handle_event` (the tree pane does this right; the leaf pane
  bug was exactly this).
- **`StaticText` has no `set_value`** (only `new`/`text`/`set_text`). For a cell
  whose text updates at render, use a disabled `InputLine` (the `ro_cell` idiom).
  `StaticText` is fine for static dialog content.
- **Dialogs:** `Dialog::new(rect, Some(title))`; `dlg.state_mut().options.center_x/
  center_y`; `dlg.insert_child(Box::new(StaticText::new(...)))`;
  `dlg.button_row(&[(label, Command, ButtonFlags)], ButtonRowAlign)`. Buttons MUST
  use modal-exit commands (`OK`/`CANCEL`/`YES`/`NO`) so `exec_view` returns them.
- **Headless view tests:** `Context::new(&mut out, &mut timers, 0, &mut deferred)`
  with `tv::timer::TimerQueue::new()` and `Vec<tv::Deferred>` (crate-root
  `tv::Deferred` since 0.1.1). A standalone `InputLine` needs
  `state.state.selected = true`. tvision events are `Event::KeyDown` (NOT `Key`).

## Upstream tvision-rs (oetiker's repo) — working with it

edaptor is tvision-rs's first real consumer; improve it as you go. Directive: work
in a SEPARATE clone of `https://github.com/oetiker/tvision-rs`, one focused PR per
change; edaptor depends on the PUBLISHED crate (a git pin is the only fallback, and
only until a release ships — then bump the version and drop the pin). The
`#[delegate(to=group)]` macro forwards `View` methods via a manual list in
`tvision-rs-macros/src/specs.rs` — a NEW `View` method must be added there to be
forwarded through wrapper views. ⚠ `gh pr edit` fails on that repo (deprecated
Projects-classic GraphQL) — edit PR title/body via REST: `gh api -X PATCH
repos/oetiker/tvision-rs/pulls/N -f title=… -F body=@file`.
