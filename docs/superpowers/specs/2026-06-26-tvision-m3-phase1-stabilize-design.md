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
2. **Form pane scrolling** — a new, self-contained `ScrollGroup` (a generic
   scroll-container of child views) holding one cell per field with a real linked
   vertical scrollbar; removes the `FORM_ROWS = 32` display cap. Built for
   extraction — a candidate to contribute upstream to tvision-rs afterward.
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

### 1. Panes fill their cell (the `▒` strip)

The custom panes lay out their children **once at construction** and never re-fit
when the splitter resizes them (no `on_bounds_changed`); the `Outline` tree self-fits
and is fine. Fix:

- `LeafPane`: add `on_bounds_changed(&mut self, ctx)` (a shared `relayout`) that
  re-bounds the search box (`Rect(0,0,w,1)`) and the `ListBox` (`Rect(0,1,w,h)`) to
  the live pane size and drives the list's bounds-changed path so its scroll range
  republishes.
- `FormPane`: its fill is handled by the `ScrollGroup` rework in §2 — the scroll
  container fills the pane and repositions/clips its children — so it needs no
  separate pool-relayout hack.

Removes the `▒` strip (the artifact carried from the full-screen change).

### 2. Form pane scrolling — an extractable `ScrollGroup`

**Idiomatic basis (verified against the tvision-rs 0.3.0 source).** The framework
has **no scroll-container for child views** — `Group` carries no scroll offset for its
children, and `Scroller` is strictly for *self-drawn* content (the `Editor`/`Terminal`
/`Outline` subclass it). So scrolling a *form of editable child widgets* is the one
case the framework doesn't already cover. Three options were evaluated:

- **(a) Virtualized cell pool** — a fixed pool sized to the visible rows, reused
  across fields as you scroll (the `ListBox` row-reuse trick). Edaptor-specific, has an
  edit-commit-on-reassign trap (a scroll mid-edit could smear onto the wrong attr),
  and is not worth upstreaming.
- **(b) Self-drawn `Scroller` subclass** (like `Editor`) — throws away `InputLine`
  and means reimplementing inline text editing by hand. Rejected.
- **(c) `ScrollGroup`** — a small **generic scroll-container of child views**: it owns
  the real per-field `Label`+`InputLine` children, keeps a `top` offset, and on scroll
  repositions every child by `-top` (offscreen children clip automatically — see the
  spike below) while driving a linked `ScrollBar`. Persistent per-field cells ⇒ editing
  just works (focus, cursor, and edit buffer travel *with* the field — no reassign,
  no smear). It is also the **missing reusable primitive**: built self-contained in
  `src/tui/` now, and a candidate to contribute upstream to tvision-rs as a focused
  follow-up PR once proven against this consumer (edaptor then switches to the
  published widget and deletes its copy).

**We take (c).** It gives the cleaner form *and* the contribution. The leaf pane
(`ListBox`) and tree (`Outline`) are already idiomatic built-ins; the form is the one
necessarily-custom pane, and `ScrollGroup` is the right shape for it.

**Feasibility spike (source-level, PASS).** Reposition-on-scroll works on tvision-rs
0.3.0 unchanged:
- *Draw clip* — `Group::draw` draws each child through `ctx.sub(child_bounds)`, and
  `DrawCtx::sub` sets the child clip to `parent_clip ∩ child_bounds` (context.rs:910);
  a child at negative-y clips at the top edge, one past the bottom clips there, a
  fully-offscreen one draws nothing. The framework's own `sub_narrows_clip_and_shifts_
  origin` / `fill_clips_to_clip_rect` tests already cover this clip path.
- *Mouse* — routing is `bounds.contains(parent-local pos)` + local translate
  (group.rs:198/928), sign-agnostic, so clicks map correctly to negative-origin
  children.
- *Cursor / Tab* — correct **given scroll-to-focused** (Tab walks all focusable
  leaves, so focusing an offscreen field must scroll it into view). That is the one
  piece of real implementation work, not a feasibility risk.

**`ScrollGroup` design:**

- Owns a content child set + an optional linked vertical `ScrollBar` id, and a
  `top: i32` offset. Each child has a stable *logical* y over the full content height;
  scrolling sets the child's on-screen `bounds.y = logical_y − top` via
  `change_bounds`, so **bounds, draw, mouse, and cursor stay consistent** (no
  draw-only offset that would desync mouse/cursor).
- `ensure_visible(child)` / scroll-to-focused: when focus moves to a child outside the
  visible band, adjust `top` so it shows, then republish the bar. Hooked off the
  focus-change path so Tab/arrows never strand the cursor offscreen.
- Linked `ScrollBar` via the framework `ScrollSync` broker, exactly as `ListViewer`
  does (it holds only the bar's id): publishes range/value with
  `ctx.request_scroll_bar_params(bar, value=top, min=0, max=content−visible, steps…)`;
  on a user drag the bar broadcasts `SCROLL_BAR_CHANGED { source = bar }`, the group
  calls `ctx.request_scroll_sync(self_id, …)`, and the pump calls the group's
  overridden `View::apply_scroll_sync` to set `top`. No self-drawn indicator.

**`FormPane` on top of `ScrollGroup`:**

- One `Label`/`ro_cell` + `InputLine` per field (no `FORM_ROWS = 32` cap); the header
  + dirty marker stay pinned (outside the scrolled band).
- **Per-entry rebuild:** on a new leaf load the field set changes, so the child cells
  are rebuilt from `EditForm.fields` — a view-tree mutation done through the
  established deferred / `handle_event` path, observing borrow discipline (no
  `UiState`/`RefCell` borrow held across `child_mut`/`change_bounds`/`ctx.*`). This is
  user-paced (per navigation, not per frame).
- Editing reuses the **M2 inline-edit/commit path unchanged** (commit on
  focus-change / Enter); because each field owns its cell there is no scroll-time
  smear, so the virtualized-pool's commit-on-reassign problem does not arise.
- The bar occupies the pane's right column; value cells shrink by one column while the
  bar is shown (`content > visible`).

The `ScrollGroup` lives in its own module (`src/tui/scroll_group.rs` or similar) with
no edaptor-domain coupling, so the follow-up upstream extraction is a lift-and-publish,
not a rewrite.

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
  - `LeafPane::on_bounds_changed`: after a bounds grow, the search/list child bounds
    cover the full pane (no right/bottom gap); bounds-math assertions.
  - `ScrollGroup` (its own test module — it has no edaptor coupling): with content
    taller than the viewport, `scroll_to`/`top` shifts each child's `bounds.y` by
    `-top`; a child scrolled above the top or below the bottom clips (assert via the
    headless `Buffer`/`DrawCtx` harness — `Buffer::new(w,h)` + `buf.get(x,y).symbol()`,
    as the crate's own clip tests do); `ensure_visible(child)` adjusts `top` so a
    focused child enters the viewport; the linked-bar params (`request_scroll_bar_params`)
    and `apply_scroll_sync` round-trip update `top`.
  - `FormPane` over `ScrollGroup`: a >viewport field set is fully reachable (every
    field's cell can be brought on-screen); per-entry rebuild yields one cell per
    field; the header/dirty row stays pinned.
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

## Upstream contribution (follow-up, out of Phase 1 scope)

`ScrollGroup` is built self-contained and domain-free in `src/tui/` so it can later be
lifted into tvision-rs as a focused PR (the established workflow: separate clone, one
PR per change, edaptor then depends on the published crate). This is a deliberate
*follow-up* — Phase 1 ships with the component living in edaptor; the extraction does
not block Phase 1 and is not gated on a tvision-rs release.

## Conventions

Facade boundary (only `src/tui/**` may `use tvision_rs`); borrow discipline (no
`RefCell`/`UiState` borrow held across `ctx.*`/`child_mut`/`set_value`); live tests
gated by `EDAPTOR_TEST_LDAP_URI`; commit trailer
`Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. `CHANGES.md` updated for
the user-visible scrolling/fill change; the M3 page in the mdBook is a Phase 2
concern (no config-format change here).
