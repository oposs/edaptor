# M3 Phase 1 — stabilize the base (panes fill + scroll, guard edges)

**Date:** 2026-06-26 · **Status:** design (approved, pre-plan) · **Branch:** `feat/tvision-ui`

Phase 1 of the M3 milestone. M3 is split into two spec→plan→implement cycles:

- **Phase 1 (this doc):** harden the form/nav surface the create flow will build on —
  panes fill their cell and scroll, plus the two outstanding guard edges.
- **Phase 2 (separate spec):** the M3 core — ObjectClass widget, schema resync, and
  the create flow (Alt+N profile chooser, create-mode form, objectClass injection).

This split keeps each plan a manageable size and fixes the carried-forward problems
first, because the create flow reuses the form pane and the tree-guard machinery.

## Scope

Four changes, all confined to the tvision UI (`src/tui/**`) and its neutral state
helpers; no domain-layer or ratatui (`src/ui/**`) changes.

1. **Panes fill their cell** — kill the one-cell desktop-background (`▒`) strip the
   frameless full-screen window exposed.
2. **Form pane scrolling** — windowed view over the fields with a vertical
   scrollbar; removes the `FORM_ROWS = 32` display cap.
3. **Guard edge #2** — a cancelled save-confirm on the guard path snaps the list
   highlight back (behaves like *Stay*).
4. **Guard edge #3** — changing branch in the tree while the form is dirty guards,
   and *Stay* reverts the tree selection.

### Non-goals (Phase 2, not here)

- ObjectClass widget / `Activation::Modal` / `FieldEditor`.
- `sync_schema_fields` port, `FormMode::New`, `SetValuesThenResyncSchema` handling.
- Alt+N profile chooser, create-mode form, objectClass auto-injection.
- Horizontal scrolling of any pane (vertical only; values already clip).

## Background (current state)

- **`FormPane`** (`src/tui/panes/form.rs`): a `Group` with `FORM_ROWS = 32`
  pre-allocated row cells — each row a `label_ids[i]` (`ro_cell`, disabled
  `InputLine`) + `value_ids[i]` (`InputLine`, enabled iff inline-editable), plus a
  header cell at y=0. Children are positioned **once at construction** in local
  coords; `render()` pushes `EditForm.fields` text into the cells `.take(32)`. There
  is **no `on_bounds_changed`**, so when the splitter enlarges the pane the children
  keep their construction bounds → undrawn rows/columns show desktop `▒`.
- **`LeafPane`** (`src/tui/panes/leaf.rs`): a `Group` of a search `InputLine` (row 0)
  + a `ListBox` (rows 1..h, created with no scrollbars). Same defect: no
  `on_bounds_changed`, so the inner views don't grow with the pane.
- **`TreePane`**: an `Outline` (built-in) — self-fits, already correct. Left as-is.
- **Nav/guard controller** (`src/tui/state.rs`, `src/tui/app.rs`, `src/tui/pump.rs`):
  the controller-owned transition. Panes are pure selectors: `LeafPane` records
  `UiState::requested_leaf`; the pump's `reconcile_selection()` loads it (clean) or
  stashes `guard_target` + posts `GUARD_NAV` (dirty); `app::dispatch` runs the guard
  modal; *Stay* sets `set_leaf_row = current_leaf_row()` to snap the highlight back.
  **Today this model covers leaf selection only** — branch (tree) changes are not
  funnelled through it, which is edge #3.
- **tvision-rs 0.3.0 scroll primitives:** `ScrollBar::new(bounds)` (orientation
  inferred: width==1 ⇒ vertical) with `set_params/set_value/set_range/set_step`,
  broadcasting `SCROLL_BAR_CHANGED { source = bar id }`; the pump's `ScrollSync`
  broker resolves bar values for a content view via `apply_scroll_sync`. A scroll bar
  for a pane is created manually and inserted into the pane's parent group and linked
  by id — **not** `Window::standard_scroll_bar` (that is window-frame chrome).

## Design

### 1. Panes fill their cell

Give `LeafPane` and `FormPane` an `on_bounds_changed(&mut self, ctx)` that re-bounds
their children to the pane's current size, then forwards to the embedded group so the
inner widgets re-fit:

- `LeafPane`: search box → `Rect(0,0,w,1)`; list → `Rect(0,1,w,h)`; call the
  `ListBox`/list-viewer bounds-changed path so its scroll range republishes.
- `FormPane`: header → `Rect(0,0,w,1)`; each visible row cell → its `y=row+1` line
  spanning the new width (label `0..LABEL_W`, value `LABEL_W..w`). Row count derives
  from `h` (see §2), so this hook and scrolling share the same relayout routine.

A shared private `relayout(&mut self, ctx)` does the bounds math from the live view
bounds; both `new()` and `on_bounds_changed()` call it. This removes the `▒` strip
(verified live) and is the prerequisite for meaningful scrolling.

### 2. Form pane scrolling

**Idiomatic basis (verified against the tvision-rs 0.3.0 source).** The framework
has **no scroll-container for child views** — `Group` carries no scroll/`delta`
offset for its children, and `Scroller` is strictly for *self-drawn* content (the
`Editor`/`Terminal`/`Outline` subclass it and draw their own rows shifted by
`delta.y`). Every scrollable view in the framework — `ListBox`/`ListViewer`,
`ThemeEditorBody`, the `FilePane` example — hand-rolls the **same windowing pattern**:
a `top`/`scroll_top` index, draw only the visible range, mirror it into a linked
`ScrollBar`. There is no reusable widget to drop in; the choice is which idiomatic
hand-rolled pattern. Two options were evaluated:

- **(a) Windowed pool of `InputLine`/label cells** — pool sized to the visible rows,
  `top` maps `fields → cells` (row virtualization, exactly how `ListBox` reuses rows),
  a real linked `ScrollBar`. Keeps `InputLine`'s text editing for free.
- **(b) Self-drawn `Scroller` subclass** (like `Editor`) — more native for *scrolling*
  but throws away `InputLine` and means **reimplementing inline text editing** by hand.

**We take (a)** — it reuses `InputLine` (no reinvented editing) on top of the
framework's own windowing pattern (no reinvented scrolling). Notably the current form
pane is already pattern (a) minus the windowing, so Phase 1 *completes* it rather than
replacing it. (The leaf pane is `ListBox` and the tree is `Outline` — already idiomatic
built-ins; the form is the one necessarily-custom pane.)

Concretely, make the form a **windowed view** over `EditForm.fields`:

- The cell pool sizes to the **visible row count** `visible = max(0, h - 1)` (header
  takes row 0), recomputed in `relayout`. (Keep a generous fixed pool and only
  *use*/show `visible` of it, or grow the pool on demand — a plan-time decision;
  behaviour is "exactly the rows that fit are live".)
- The pane holds a `top: usize` scroll offset. `render()` maps
  `fields[top .. top+visible]` into the visible cells (no `.take(32)`).
- **Edit-state commit on reassign.** Because a cell is reused for different fields as
  you scroll (virtualization), the focused cell's pending edit must be committed back
  to its current field *before* `top` changes and the cell is repointed at another
  field — otherwise a scroll would smear the edit onto the wrong attribute. The plan
  pins where this commit hooks in (it reuses the existing focus-change/Enter commit
  path; a scroll is just another reassignment trigger).
- Arrow navigation: moving the focused field above `top` or below `top+visible-1`
  scrolls by one (adjust `top`), keeping the focused field on screen. Page-up/down
  optional (plan-time; not required for acceptance).
- The header/dirty marker stays pinned at row 0 (does not scroll).

**Scrollbar — a real linked `ScrollBar`.** The form is the rightmost splitter pane.
A vertical `ScrollBar::new` (width-1 ⇒ vertical) is created in the splitter cell and
passed to the form **by id**, mirroring the `ListBox`+bar idiom; the form's value
cells shrink by one column while the bar is shown (`fields.len() > visible`). The bar
and the form stay in sync through the framework's `ScrollSync` broker, exactly as
`ListViewer` does (it holds only the bar's id, never the bar itself):
- the form publishes range/value with `ctx.request_scroll_bar_params(bar, value=top,
  min=0, max=fields.len()-visible, page/arrow steps, …)` whenever the field set or
  `top` changes;
- on a user drag the bar broadcasts `SCROLL_BAR_CHANGED { source = bar }`; the form
  sees it and calls `ctx.request_scroll_sync(self_id, h, v)`; the pump resolves the
  bar's value and calls the form's overridden `View::apply_scroll_sync`, which sets
  `top` and re-renders.

No self-drawn indicator — the bar is a first-class linked view, wired through the
same broker every built-in scroller uses.

### 3. Guard edge #2 — cancelled confirm snaps back

`app::dispatch`'s `GUARD_NAV` → `GuardDecision::Save` calls
`do_save(prog, state, target, false)`. Today, if the LDIF confirm is cancelled,
`do_save` returns early and the highlight is left on the target while the form stays
pinned to the original.

Change `do_save` to report its outcome (e.g. return a small enum / bool:
`Submitted` vs `NotSubmitted`). In the `GUARD_NAV` Save arm, if `do_save` did **not**
submit, snap the highlight back: `set_leaf_row = current_leaf_row()` and clear any
stashed `guard_target`/`pending_nav` — i.e. a cancelled confirm on the guard path is
treated exactly like *Stay*. The plain Alt-S path (no guard target) keeps today's
behaviour (no snap-back needed; nothing moved).

### 4. Guard edge #3 — branch change while dirty

Extend the controller-owned transition to the tree, mirroring the leaf path:

- `UiState`: add `requested_branch: Option<String>` (pane→controller intent) and
  `set_tree_row: Option<i32>` (controller→pane snap-back), plus
  `current_branch_row()` (the tree row of `current_branch`).
- `TreePane`: on selection change record `requested_branch` only — no read, no
  guard, no post (it currently triggers the branch load directly; that moves into the
  reconcile step). Honour `set_tree_row` on its next event to snap the highlight back.
- Pump `reconcile_selection()` (or a sibling `reconcile_branch()`): clean form →
  switch branch (repopulate leaves) as today; dirty form → stash a branch guard
  target and post `GUARD_NAV`.
- `app::dispatch` `GUARD_NAV`: Save/Discard act on the branch target (Discard
  switches branch + drops the edit; Save persists then switches); **Stay** sets
  `set_tree_row = current_branch_row()` to revert the tree selection to the current
  branch.

The leaf and branch guards share one `GUARD_NAV` command and one guard modal; the
controller disambiguates by which target is stashed (`guard_target` for a leaf vs a
new branch target — represented so the dispatch knows which to act on).

## Testing

Strict TDD, atomic commits, `cargo fmt` + `clippy --all-targets -D warnings` clean
after each commit, facade guards clean.

- **Headless view tests** (the established `Context::new(&mut out, &mut timers, 0,
  &mut deferred)` pattern in `panes/leaf.rs`/`form.rs` tests):
  - `relayout`/`on_bounds_changed`: after a bounds grow, child cell bounds cover the
    full pane (no gap at the right/bottom edge); pure-ish bounds-math assertions.
  - Form windowing: with `fields.len() > visible`, `top` advances on arrow past the
    edge; rendered cells map `fields[top..]`; scrollbar shown/hidden by overflow.
  - Guard #2: a unit test that the `GUARD_NAV` Save arm, given a cancelled confirm
    (do_save → NotSubmitted), sets `set_leaf_row = current_leaf_row()` and clears the
    targets. Keep the dialog out of the unit by testing the decision routing (as M2's
    `save_flow_action` tests do).
  - Guard #3: `reconcile`-level tests mirroring the existing 4 leaf reconcile tests —
    clean branch change loads; dirty branch change stashes target + signals guard;
    Stay sets `set_tree_row`.
- **Live tmux acceptance** (the documented PTY method):
  - No `▒` strip anywhere; resizing keeps panes filled.
  - Form with >32 attributes: scrollbar appears, arrows scroll, all attrs reachable;
    header/dirty marker stays at top.
  - Cancel the guard-Save confirm → highlight snaps back to the pinned form.
  - Dirty form + change branch in the tree → guard fires; Stay reverts the tree;
    Discard switches branch and drops the edit; Save persists then switches.

## Acceptance criteria

1. No desktop-background (`▒`) strip in any pane at launch or after a resize.
2. The form pane shows a vertical scrollbar when fields exceed the visible height,
   scrolls via arrows and the bar, and reaches every attribute (no 32-row cap).
3. Cancelling a guard-triggered Save snaps the list highlight back to the pinned
   form (no highlight/form mismatch).
4. Changing branch while the form is dirty raises the guard; Stay reverts the tree to
   the current branch; Discard/Save behave consistently with the leaf guard.
5. `make check` green (fmt + clippy `-D warnings` + tests); facade guards print
   nothing; the new headless tests pass.

## Conventions

Facade boundary (only `src/tui/**` may `use tvision_rs`); borrow discipline (no
`RefCell`/`UiState` borrow held across `ctx.*`/`child_mut`/`set_value`); live tests
gated by `EDAPTOR_TEST_LDAP_URI`; commit trailer
`Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. `CHANGES.md` updated for
the user-visible scrolling/fill change; the M3 page in the mdBook is a Phase 2
concern (no config-format change here).
