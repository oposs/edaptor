# Shuttle: conventional orientation, column-bound buttons, resizable dialog

Date: 2026-07-03
Status: approved (design)
Branch: `feat/shuttle-widget`

## Problem

The embedded `Shuttle` two-list transfer widget (`src/ui/shuttle.rs`) and its two
consumers have three interacting UI problems reported by the user:

1. **Add/Remove feel reversed between dialogs.** The two consumers deliberately
   render opposite column orders via `Shuttle::new`'s `selected_on_left` flag:
   - Object Class picker (`src/ui/oc_picker.rs`): `selected_on_left = true` →
     Active (the Selected set) on the LEFT, Available on the RIGHT.
   - Edit Member picker (`src/ui/multi_picker.rs`): `selected_on_left = false` →
     Available on the LEFT, Members (the Selected set) on the RIGHT.

   The move *semantics* never change (Add = Available→Selected, Remove =
   Selected→out), but the two Add/Remove buttons are hard-pinned to the bottom-left
   corner regardless of `selected_on_left`. Because the physical position of the
   Available column differs between the two dialogs, "Add" pulls from the right
   column in one dialog and the left column in the other — so the pair *feels*
   reversed even though the code is identical. The user judged Object Class
   "correctly wired" and Edit Member "reversed".

2. **Tab lands on the move buttons.** Tabbing through the dialog focuses the
   `Remove` button (and then OK), which is noise: the move buttons are affordances
   for a keyboard/mouse action, not primary dialog navigation stops.

3. **The dialog is a fixed 72×22.** Users with more terminal space cannot enlarge
   it to see more rows.

## Goals

- One conventional orientation for both dialogs.
- Add/Remove buttons that read correctly in both dialogs, gray out when not
  applicable, and stay out of the Tab chain.
- A resizable dialog with a sensible minimum size.

## Non-goals

- No change to the move/de-dup/lock model (`ShuttleModel`) or the incremental-find
  behaviour of either column.
- No arrow-stack (`>`/`<`/`>>`/`<<`) idiom — we keep worded, hotkeyed buttons
  (the TUI-appropriate Turbo Vision idiom).
- No "move all" operation.

## Design

### A. Orientation — drop the flip

Remove the `selected_on_left` parameter from `Shuttle::new` entirely. Both dialogs
render the conventional transfer-widget layout:

- **Available on the LEFT**, **Selected on the RIGHT**.

This is the near-universal shuttle convention (source left → target right). Edit
Member is unchanged. Object Class's "Active" list moves from the left to the right;
its `oc_picker.rs` comment about `selected_on_left = true` "per the user's request"
is removed. The internal move semantics are untouched — only the rendered side of
the Selected column changes for Object Class.

Consequence: with a fixed Available-left orientation and Add bound to the Available
column, the "reversed in Edit Member" symptom disappears structurally — there is no
longer a per-dialog mirror to disagree with the button labels.

### B. Buttons & layout

```
┌─ Object classes ─────────────────────────────────────┐
│  Available                 Active                     │
│ ┌────────────────┐▓       ┌────────────────┐▓        │
│ │ inetOrgPerson  │        │ * person       │         │
│ │ posixAccount   │        │   top          │         │
│ │ …              │        │                │         │
│ └────────────────┘        └────────────────┘         │
│ [       Add        ]      [      Remove      ]        │
│                                                        │
│                                    [ OK ] [ Cancel ]   │
└────────────────────────────────────────────────────────┘
```

- **Add** — a wide button spanning the **left (Available)** column width, placed on
  the button row directly under the left list. Moves the Available list's
  highlighted row into Selected (`CMD_ADD`, unchanged semantics).
- **Remove** — a wide button spanning the **right (Selected)** column width, under
  the right list. Removes the Selected list's highlighted row (`CMD_REMOVE`).
- **Enable state (focus-driven, the user's rule):**
  - Add enabled **iff the Available (left) list is focused**.
  - Remove enabled **iff the Selected (right) list is focused**.
  - The disabled button auto-grays. Mechanism: the `Shuttle`, after delegating an
    event to its group (so `group.current()` reflects the post-Tab focus), calls
    `ctx.enable_command`/`ctx.disable_command` on `CMD_ADD`/`CMD_REMOVE` to match
    the focused list. tvision's deferred command-set machinery flips
    `command_set_changed`; the next idle pump broadcasts `COMMAND_SET_CHANGED`, and
    each `Button` already re-grays itself from `ctx.command_enabled(command)` on
    that broadcast (verified in `tvision-rs-0.9.0` `program.rs` and `button.rs`).
    We seed the initial state in `reset_current` (Available list is the open-time
    focus → Add enabled, Remove disabled).
- **Tab chain:** the Add/Remove buttons are made **non-selectable**
  (`state_mut().options.selectable = false`) so Tab skips them. Tab then cycles
  Available list → Selected list → OK → Cancel. The buttons stay operable via:
  - mouse click,
  - Alt-A / Alt-R hotkeys (the `~A~dd` / `~R~emove` mnemonics — hotkeys still fire
    for a non-selectable button),
  - Insert / Delete / Enter on the focused list (unchanged direct handling in
    `Shuttle::handle_event`).

  A non-selectable button still dispatches its command on click and still grays via
  `COMMAND_SET_CHANGED`, so the enable-state rule and the click/hotkey affordances
  are unaffected.
- **OK / Cancel** remain the *consumer* dialog's `button_row` (right-aligned), moved
  to their own row two rows below the Add/Remove row. They are normal tab stops.

### C. Resizable dialog

- Enable interactive resize on both consumer dialogs: set the dialog's `grow`
  window flag and ensure `drag_grow` drag-mode so the lower-right corner is a resize
  grab. (Both are exposed on `tvision_rs::Dialog` / `Window` in 0.9.)
- The `Shuttle` reflows its own children on resize. Factor the geometry math
  currently inline in `Shuttle::new` into a private `fn layout(area: Rect) ->
  ShuttleLayout` returning every child rect (two headers, two lists, two
  scrollbars, two buttons). `new` uses it for initial placement; a hand-written
  `change_bounds` (added to the `#[delegate]` skip list) sets the group's bounds,
  recomputes rects via `layout`, and repositions each child by id.
- **Minimum size:** clamp the effective layout to at least the current 72×22 so the
  two columns and the button rows never collapse. If the dialog is dragged smaller,
  the layout uses the minimum (the framework's drag limits are the first line of
  defence; the clamp in `layout` is the backstop).
- OK/Cancel (dialog-owned, not Shuttle children) keep their bottom-right position on
  resize via an appropriate `grow_mode` (track the bottom/right edges) applied where
  `button_row` inserts them, or by re-anchoring in the consumer.

## Affected code

- `src/ui/shuttle.rs`
  - Remove `selected_on_left` from `new`; always Available-left / Selected-right.
  - Extract `layout(area)`; wide Add (left column) + Remove (right column) buttons;
    mark both non-selectable.
  - Focus-driven `enable_command`/`disable_command` for `CMD_ADD`/`CMD_REMOVE` after
    group delegation and in `reset_current`.
  - Hand-written `change_bounds` reflow (skip in `#[delegate]`).
  - Update unit tests: drop the `selected_on_left` argument in the `shuttle()`
    helper; add coverage for (a) Add/Remove enable-state tracking focus, (b) buttons
    absent from Tab traversal, (c) reflow on `change_bounds`.
- `src/ui/oc_picker.rs`
  - Drop `selected_on_left = true` (now Available-left/Active-right); update the
    module/inline comments. Enable dialog `grow`. Adjust any tests/comments that
    assume Active-on-left column positions (tests here address columns by *label*,
    so they should be robust, but re-verify).
  - Taller dialog to fit the new button row + OK/Cancel row (see geometry note).
- `src/ui/multi_picker.rs`
  - Already Available-left; drop the now-removed `selected_on_left` argument. Enable
    dialog `grow`. Taller dialog; re-verify the label-addressed tests.
- Dialog height: both consumers currently build a 72×22 dialog and a 72×22 Shuttle.
  The Shuttle grows ~3 rows taller to hold the wide-button row plus a spacer, and
  the consumer dialog grows to keep OK/Cancel on their own row two rows below. Exact
  final height settled during implementation against the real frame; the minimum is
  the taller-by-~3 figure.

## Verification

- `make check` (fmt + clippy -D warnings + tests), `cargo test -j4`.
- Manual (`make run` against the podman demo server):
  - Object Class dialog: Active list on the right; Tab cycles the two lists + OK +
    Cancel only (never the move buttons); Add grays unless the Available list is
    focused, Remove grays unless the Active list is focused; Add/Remove still work by
    click, Alt-A/Alt-R, and Insert/Delete/Enter.
  - Edit Member dialog: same behaviour; Add/Remove no longer feel reversed relative
    to Object Class.
  - Drag the lower-right corner to enlarge either dialog: both columns, scrollbars,
    button rows, and OK/Cancel reflow correctly; shrinking stops at the minimum.

## Docs / changelog

- `CHANGES.md`: entry under the unreleased section — conventional
  Available-left/Selected-right shuttle orientation, column-bound Add/Remove buttons
  that gray by focus and are out of the Tab chain, and resizable picker dialogs.
- mdBook `docs/src/configuration/widgets.md`: refresh any membership/objectClass
  widget description or screenshot that states column sides or button layout.
- README: no change expected (it points at the mdBook for widget detail); verify.
