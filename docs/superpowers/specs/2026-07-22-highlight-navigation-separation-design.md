# Design: separating the highlight from the navigation

**Date:** 2026-07-22 · **Umbrella:** `2026-07-21-realtime-consistency-design.md`
**Depends on:** Spec 2 (cache coherence, this branch)
**Closes follow-ups:** #1 (guard-Stay snap), #2 (missing status clears),
#3 (`leaf_search_truncated`), #7 (status loss when `current_leaf_row()` is `None`)

## Problem

The list panes report a navigation whenever their widget's focus index differs
from `last_sel`. Nothing distinguishes *why* the focus moved. A rebuild moving it
is indistinguishable from the operator moving it, and the controller pushes the
correction back as a **row index**:

```rust
pub set_leaf_row: Option<i32>,
pub set_tree_row: Option<i32>,
```

Both panes rebuild *before* consuming that index (`tree.rs:269-281`,
`leaf.rs:240-292`), but the index was computed by the controller in an **earlier**
turn — `apply_branch_guard_stay` reads `st.current_branch_row()` against the
then-current `branch_dns`. The rebuild rewrites `branch_dns` and renumbers the
rows, so the pane faithfully applies an index describing the old world.

Every defect in this cluster is a consequence:

- **Spec 2 finding I1** — hits were upserted before `leaf_search_rows` was
  installed, so the snap index was computed against the previous row source. Fixed
  by recomputing after installation (`state.rs:1268`) — one instance, index
  representation retained, class intact.
- **Spec 2 finding I4** — rebuilt panes reported row 0 (the `‹self›` row),
  dragging the form onto the container and wiping the status. Fixed by snapping
  back at every rebuild path — again per-site.
- **Spec 2 finding 2** — `ov_update` re-clamps the outline's focus internally, so
  a vanished branch left `last_sel` stale and the pane reported a branch the
  operator never selected. Fixed by resyncing `last_sel` unconditionally — for the
  tree only.
- **Follow-up #1** — `apply_branch_guard_stay` resolves against pre-rebuild
  `branch_dns`. `tree_dirty` now fires far more often than when this was judged
  rare.
- **Follow-up #7** — when `current_leaf_row()` is `None` there is no index to
  push, so no snap happens and the pane's row-0 refocus is reported as a fresh
  selection.
- **Unreported, found while designing this** — because a find-driven repopulate
  reports a new row, and `reconcile_selection` (`state.rs:1109`) raises the dirty
  guard on *any* selection change with unsaved edits, **typing a find query while
  the form is dirty pops the "discard changes?" modal mid-keystroke**.

## Goal

The highlight and the form are separately owned. A rebuild never fabricates a
navigation; only an operator action does. The stale-index state becomes
unrepresentable rather than detected and corrected per site.

## Non-goals

The two-leg rename partial failure (follow-up #5 — its own brainstorm); entry
reads requesting `scan_attrs` (#4); the `lookup` search term after a pick (#6);
the status wording pass (#8); Spec 3 (autonumber) and Spec 4 (delete), though
this design deliberately builds the vanished-entry machinery Spec 4 needs.

## Components

### 1. Two paths through the panes

- **Rebuild path** — repopulate/rebuild, resolve the desired highlight from the
  controller, set the widget focus, resync `last_sel` **silently**. Never reports.
- **Event path** — delegate the event to the widget; a focus change afterwards is
  an operator move and is reported via `request_leaf`/`request_branch`.

This is the Finding-2 fix generalised from the tree to both panes.

### 2. `HighlightPlan` — the controller's one policy

```rust
/// What the pane should do with its highlight after a rebuild.
pub enum HighlightPlan {
    /// Highlight this DN; the form does not move.
    Pin(String),
    /// Highlight this DN and let the form follow it.
    Follow(String),
    /// Nothing to highlight.
    Clear,
}
```

Resolved by a pure method on `UiState` against the freshly-built row source:

| situation | plan |
|---|---|
| no rows | `Clear` |
| open entry is in the rows | `Pin(current_leaf)` |
| open entry absent, form **clean** | `Follow(first_row)` |
| open entry absent, form **dirty** | `Pin(first_row)` |
| no form open yet | `Pin(first_row)` |

**`first_row` means the first row that is not the `‹self›` row.** `leaf_rows`
puts the branch's own entry at row 0 whenever no filter is active, so a literal
`rows.first()` would resolve to the *container* — which is exactly the I4 defect
this design exists to remove (a rebuild dragging the form onto the container and
wiping the status). A container with no children therefore yields `Clear`, not a
`Pin` on itself. The `Pin(current_leaf)` check still searches the **full** row
set, so an operator who deliberately opened the container's own entry keeps it.

`Pin` sets the widget focus and resyncs `last_sel` only. `Follow` does that *and*
emits `request_leaf`, so the form moves through the existing
`reconcile_selection` path — which cannot raise the guard here, because `Follow`
is only produced when the form is clean.

`Follow` is the deliberate form of today's accidental find-follow: typing a find
is navigation, so with a clean form the highlight and form move to the first hit.
With a **dirty** form the highlight still moves but the form stays pinned and the
guard is **not** raised — the modal-mid-keystroke bug disappears, and the guard
returns to firing only on deliberate navigation (Enter, arrows, click).

Guard "Stay" ceases to be a special push: it is "re-resolve the highlight", which
returns `Pin(current_leaf)` by construction.

**The tree pane shares the enum but never produces `Follow`.** It resolves
`Pin(current_branch)` when that branch survives the rebuild and `Clear` when it
does not; a branch change is always operator-driven or an explicit
`commit_branch`, so the tree must never navigate the form on its own.

### 3. Vanishing is a separate signal

Absence from the rebuilt rows is **not** evidence that an entry is gone — a find
excludes rows routinely. `note_entry_vanished(dn)` fires only on hard evidence:
`current_leaf` absent from `structure` after an Alt+R reload or a rename rescan,
or a re-read returning no-such-object.

**Detection lives inside `adopt_structure`**, the single funnel every reload and
rescan passes through. It already nulled `current_leaf` silently for a vanished
leaf, with a doc comment calling that deliberate ("no dirty-form guard is
needed"); this design supersedes that decision, so the silent null becomes a
`note_entry_vanished` call. Detecting *after* the reload cannot work —
`adopt_structure` has by then already discarded the DN. The rename-rescan path
runs right after a successful save, so its form is never dirty and it only ever
takes the clean arm.

For a dirty form the guard is **drained from the `RELOAD` dispatch arm**, not the
navigation guard: a reload is not a navigation, so the pump never posts
`GUARD_NAV` for it. The arm runs the dialog after a successful reload.

- **form clean** → clear the form, status reports the entry is gone.
- **form dirty** → `GuardTarget::Vanished(dn)`, raising a three-button dialog:
  **Keep editing / Discard / Re-create**.
  - *Keep editing* — the form and its edits stay; the status carries the notice.
  - *Discard* — clears the form **without** the re-read the normal Discard path
    does (`app.rs:257`), which would fail against a deleted DN.
  - *Re-create* — submits the form's values as an ADD at that DN, **behind the
    same LDIF confirm preview every other write in edaptor uses**
    (`confirm::build`, as in `do_save` `app.rs:579` and `do_create` `app.rs:916`),
    rendered with `crate::ldap::ldif::render_add`. Resurrecting an entry another
    admin deliberately deleted is the most consequential write of the three and
    must not be the only unconfirmed one. If another client re-created the DN
    meanwhile, LDAP's rc 68 (`entryAlreadyExists`) rejects it, so the action is
    safe by construction.

Unsaved work is never destroyed without asking.

### 3a. A successful create must say so

`WriteOutcome::Created` (`state.rs:793`) sets **no** status, unlike `Saved`
(`:650`). The comment at `state.rs:815` justifies using the non-clearing re-read
*"so a status set for this create survives to be seen"* — but nothing ever sets
one. A create has been silent since it was written, invisibly so until Spec 2
made the status line render at all.

`Created` now sets `status = format!("Created {dn}.")`. This fixes the ordinary
create path as much as the re-creation path.

The message is deliberately **not** specialised to "Re-created": distinguishing
the two would mean threading a flag through `WriteIntent`/`WriteOutcome`, and
that async correlation surface is exactly where Spec 2's worst defect came from
(inferring a rename from UI state). "Created X." is accurate for both.

### 4. One named status-clearing policy

`begin_operator_action()` clears the status at every operator-action site: the
four that already clear (`commit_branch`, `reconcile_selection`,
`set_leaf_search`, `apply_commit`) plus `open_create`, modal cancel and guard
Stay. This keeps the policy established by `c016f2a` — clear at the call site,
never inside a shared helper such as `reread`, where a rename looks like a
navigation and eats its own "Saved." — while making it named and greppable
instead of seven bare `status.clear()` calls.

## Removed

`set_leaf_row`, `set_tree_row`, the snapping use of `current_leaf_row()` and
`current_branch_row()`, `apply_branch_guard_stay`, and `leaf_search_truncated`
(follow-up #3 — set in six places, read only by two tests; the truncation notice
travels via `status`).

`apply_cancelled_guard_save` is **reduced, not removed**: it still clears
`guard_target` and `pending_nav`, but its highlight push becomes a rebuild
request, leaving the plan to resolve where the highlight belongs.

## Testing

TDD throughout. The I1, I4 and Finding-2 regression tests must keep passing
**unchanged** — they encode real bugs and are the safety net for refactoring the
branch's most-reviewed code. Their assertions move from row indices to DNs only
where the index no longer exists.

New coverage:

- the `HighlightPlan` truth table, all five rows;
- a rebuild that moves the pinned DN to a different index — the highlight follows
  the DN and no `requested_leaf` is produced;
- a find repopulate with a **dirty** form — no `requested_leaf`, no guard target;
- a find repopulate with a **clean** form — `requested_leaf` is the first hit;
- a reload dropping `current_leaf`, clean — the form is cleared, status set;
- the same, dirty — `GuardTarget::Vanished`, form and edits intact;
- `Discard` on a Vanished target clears the form and issues **no** read;
- `Re-create` submits an ADD carrying the form's values.

## Risks

This refactors the most heavily reviewed code on the branch, and it is wider than
the three follow-ups that prompted it. The mitigation is that it removes the
possibility of the bug rather than adding a fourth per-site guard against it, and
Spec 4's delete flow needs the vanished-entry machinery regardless — building it
now avoids writing delete against a model that cannot express "the entry you were
editing is gone".
