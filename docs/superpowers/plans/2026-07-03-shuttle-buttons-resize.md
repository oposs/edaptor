# Shuttle Buttons & Resizable Dialog Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the `Shuttle` transfer widget one conventional orientation (Available left / Selected right), replace the fixed bottom-left Add/Remove buttons with wide, column-bound buttons that gray by focus and stay out of the Tab chain, and make both picker dialogs resizable.

**Architecture:** All work is on the embedded `Shuttle` view (`src/ui/shuttle.rs`) and its two consumers (`src/ui/oc_picker.rs`, `src/ui/multi_picker.rs`). The move model (`ShuttleModel`) and find behaviour are untouched. Geometry math moves out of `Shuttle::new` into a pure `layout(area)` function so it is reused by a hand-written `change_bounds` reflow. Button graying uses tvision's deferred command-set machinery (`ctx.enable_command`/`disable_command` → `COMMAND_SET_CHANGED` → each `Button` re-grays itself).

**Tech Stack:** Rust, `tvision-rs` 0.9. Build/test via `cargo`/`make`, **capped at 4 cores**.

## Global Constraints

- Cap all cargo/compile/test parallelism at 4 cores: `cargo test -j4`, `cargo clippy -j4`. Shared machine.
- `make check` (fmt + `clippy --all-targets -- -D warnings` + tests) must pass before any task is "done".
- English for all code, comments, identifiers, and test names.
- Containers are **podman**, not docker (only relevant for `make run`/`scripts/test-ldap.sh`).
- Every user-visible change gets a `CHANGES.md` entry (Task 6). Config/behaviour docs live in the mdBook `docs/src/`.
- Do not reintroduce removed config layers (`[profile.picker.*]`, `[profile.password]`).
- Commit after every task (frequent commits).

## File Structure

- `src/ui/shuttle.rs` — the widget. Gains: `selected_on_left` removed from `new`; a private `ShuttleLayout` struct + `layout(area)` fn; new stored child ids (headers, bars, buttons); wide non-selectable Add/Remove buttons bound to the Available/Selected columns; focus-driven command enable/disable; hand-written `change_bounds`; grow_mode so it fills its owner. Tests updated + extended.
- `src/ui/oc_picker.rs` — Object Class consumer. Drop `selected_on_left = true` arg; taller dialog (22 → 25); enable `grow`; anchor OK/Cancel. Comment updates.
- `src/ui/multi_picker.rs` — Edit Member consumer. Drop `selected_on_left = false` arg; taller dialog (22 → 25); enable `grow`; anchor OK/Cancel.
- `CHANGES.md`, `docs/src/configuration/widgets.md`, `README.md` — docs (Task 6).

---

## Task 1: Drop the orientation flip (Available left / Selected right always)

Removes `selected_on_left` so both dialogs render conventionally. This alone fixes the "Add/Remove reversed in Edit Member" report: with Available always on the left and the buttons' semantics fixed, the two dialogs stop being mirror images.

**Files:**
- Modify: `src/ui/shuttle.rs` (`Shuttle::new` signature + body; test helper `shuttle()`)
- Modify: `src/ui/oc_picker.rs:126-132` (call site + comment)
- Modify: `src/ui/multi_picker.rs:162-168` (call site + comment)

**Interfaces:**
- Produces: `Shuttle::new(area: Rect, left_title: &str, right_title: &str, find_mode: FindMode) -> Shuttle` — the `selected_on_left: bool` 5th parameter is removed. `left_title` is always the Available column, `right_title` always the Selected column.

- [ ] **Step 1: Update the `Shuttle::new` signature and body**

In `src/ui/shuttle.rs`, change the signature (currently `src/ui/shuttle.rs:129-135`) to drop the last parameter, and delete the flip. Replace the doc line about `selected_on_left` and the `(avail_col, sel_col)` block:

```rust
    /// Build the two columns (each a list + a right-lane scroll bar) inside an
    /// owned `Group`. `find_mode` enables the Available list's built-in
    /// incremental search ([`FindMode::Off`] for none). The Available column is
    /// always rendered on the LEFT, the Selected column on the RIGHT (the
    /// conventional transfer-widget layout). Geometry: headers at row 1, lists at
    /// rows 2..(height-4), 2-cell margins and a 4-cell gutter.
    pub(crate) fn new(
        area: Rect,
        left_title: &str,
        right_title: &str,
        find_mode: FindMode,
    ) -> Shuttle {
        let (x0, y0, x1, y1) = (area.a.x, area.a.y, area.b.x, area.b.y);
        let mid = (x0 + x1) / 2;
        let left = (x0 + 2, mid - 2);
        let right = (mid + 2, x1 - 2);
        let head_y = y0 + 1;
        let list_y = (y0 + 2, y1 - 4);
        let avail_col = left;
        let sel_col = right;
```

Leave the rest of `new` (headers, lists, bars, buttons, struct build) unchanged for this task — the button rework is Task 2.

- [ ] **Step 2: Update the test helper**

In `src/ui/shuttle.rs` `mod tests`, the `shuttle()` helper (`src/ui/shuttle.rs:748-756`) passes `false` as the 5th arg. Remove it:

```rust
    fn shuttle() -> Shuttle {
        Shuttle::new(
            Rect::new(0, 0, 72, 22),
            "Active",
            "Available",
            FindMode::Filter,
        )
    }
```

- [ ] **Step 3: Update both call sites**

In `src/ui/oc_picker.rs` (call at `src/ui/oc_picker.rs:126-132`), remove the `selected_on_left` argument and fix the comment. Note titles swap so the LEFT title is Available and the RIGHT is the Selected set ("Active"):

```rust
        // Conventional transfer layout: Available on the LEFT, Active (the
        // Selected set) on the RIGHT. Insert the Shuttle FIRST so it is the
        // dialog's first selectable child: the modal's open-time `reset_current`
        // then makes it current, so key events route into it (and reach the
        // Available list inside it).
        let shuttle = Shuttle::new(
            Rect::new(0, 0, 72, 22),
            "Available",
            "Active",
            /* find */ FindMode::Filter,
        );
```

Also update the module doc at `src/ui/oc_picker.rs:1-18`: the active classes now sit in the **Active** column on the **right**, remaining classes in **Available** on the **left**; delete the sentence about `selected_on_left = true` "per the user's request".

In `src/ui/multi_picker.rs` (call at `src/ui/multi_picker.rs:162-168`), remove the argument (titles already Available-left / Members-right, so only the arg + comment change):

```rust
        // Available on the left, Members (the Selected set) on the right — the
        // conventional transfer layout. Insert the Shuttle FIRST so it is the
        // dialog's first selectable child (the modal's open-time reset_current
        // then makes it current, and focus reaches the Available list inside it).
        let shuttle = Shuttle::new(
            Rect::new(0, 0, 80, 22),
            "Available",
            "Members",
            /* find */ FindMode::Highlight,
        );
```

Also update the module doc at `src/ui/multi_picker.rs:7-17` if it names a side that changed (Available-left/Members-right is unchanged here, so likely only the removed argument matters).

- [ ] **Step 4: Build and run the full test suite**

Run: `cargo test -j4 --lib ui::shuttle ui::oc_picker ui::multi_picker`
Expected: PASS. The oc_picker/multi_picker tests address columns by *label* (`highlight_active_by_label`, `highlight_avail_by_label`, etc.), so they are orientation-agnostic and should stay green. If any test hard-codes a physical column index, fix it to address by label.

- [ ] **Step 5: Full check**

Run: `make check`
Expected: PASS (fmt clean, no clippy warnings, all tests pass).

- [ ] **Step 6: Commit**

```bash
git add src/ui/shuttle.rs src/ui/oc_picker.rs src/ui/multi_picker.rs
git commit -m "fix(ui): shuttle always renders Available-left/Selected-right

Drop the selected_on_left flip. Both pickers now use the conventional
transfer layout, so Add/Remove no longer feel reversed between the
Object Class and Edit Member dialogs.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 2: Extract `layout()`, add wide column-bound Add/Remove buttons, taller dialogs

Move geometry into a pure function, reserve rows for a wide-button row two rows above OK/Cancel, and make Add span the Available (left) column and Remove span the Selected (right) column. Mark both buttons non-selectable so Tab skips them.

**Files:**
- Modify: `src/ui/shuttle.rs` (add `ShuttleLayout` + `layout`; rewrite `new` to use it; store new child ids; wide non-selectable buttons)
- Modify: `src/ui/oc_picker.rs` (dialog + shuttle height 22 → 25)
- Modify: `src/ui/multi_picker.rs` (dialog + shuttle height 22 → 25)
- Test: `src/ui/shuttle.rs` `mod tests`

**Interfaces:**
- Consumes: `Shuttle::new(area, left_title, right_title, find_mode)` from Task 1.
- Produces:
  - `struct ShuttleLayout { left_header: Rect, right_header: Rect, avail_list: Rect, avail_bar: Rect, sel_list: Rect, sel_bar: Rect, add_btn: Rect, remove_btn: Rect }`
  - `fn layout(area: Rect) -> ShuttleLayout` (private, pure; clamps `area` to at least `Shuttle::MIN_W` × `Shuttle::MIN_H` for the internal math).
  - `const MIN_W: i32 = 60; const MIN_H: i32 = 20;` on `impl Shuttle`.
  - New fields on `struct Shuttle`: `left_header_id, right_header_id, avail_bar_id, sel_bar_id, add_id, remove_id: ViewId` (in addition to the existing `avail_id`, `selected_id`).

- [ ] **Step 1: Write a failing test for the layout geometry**

Add to `src/ui/shuttle.rs` `mod tests`:

```rust
    #[test]
    fn layout_splits_columns_and_places_wide_buttons() {
        let l = Shuttle::layout(Rect::new(0, 0, 72, 25));
        // Two columns split at the midpoint (36), 2-cell margins, 4-cell gutter.
        assert_eq!(l.avail_list.a.x, 2, "Available list starts at left margin");
        assert!(l.avail_list.b.x <= 34, "Available list ends before the gutter");
        assert!(l.sel_list.a.x >= 38, "Selected list starts after the gutter");
        assert_eq!(l.sel_list.b.x, 69, "Selected list ends before its scrollbar");
        // Add spans the Available (left) column; Remove spans the Selected (right).
        assert_eq!(l.add_btn.a.x, l.avail_list.a.x, "Add left edge aligns Available column");
        assert_eq!(l.remove_btn.a.x, l.sel_list.a.x, "Remove left edge aligns Selected column");
        assert!(l.add_btn.width() >= 20, "Add is a wide button, got {}", l.add_btn.width());
        assert!(l.remove_btn.width() >= 20, "Remove is a wide button");
        // The button row sits above where the dialog's OK/Cancel row lands (y-3),
        // with a spacer: buttons top at height-6.
        assert_eq!(l.add_btn.a.y, 25 - 6, "button row two rows above OK/Cancel");
        assert_eq!(l.remove_btn.a.y, 25 - 6);
        // Lists end above the button row.
        assert!(l.avail_list.b.y <= l.add_btn.a.y, "lists clear the button row");
    }
```

(`Rect::width()` exists in tvision-rs; if not, compute `l.add_btn.b.x - l.add_btn.a.x`.)

- [ ] **Step 2: Run it to confirm it fails**

Run: `cargo test -j4 --lib ui::shuttle::tests::layout_splits_columns_and_places_wide_buttons`
Expected: FAIL — `layout` / `MIN_W` do not exist yet.

- [ ] **Step 3: Add the `ShuttleLayout` struct and `layout()` function**

Add near the top of `impl Shuttle` in `src/ui/shuttle.rs`:

```rust
    /// Minimum interior the two columns + button rows need before they overlap.
    const MIN_W: i32 = 60;
    const MIN_H: i32 = 20;

    /// Every child rect derived purely from the widget's `area`. Extracted from
    /// `new` so a resize (`change_bounds`) can recompute the same geometry.
    fn layout(area: Rect) -> ShuttleLayout {
        // Clamp the working extent so a too-small area never yields overlapping
        // or inverted rects (the window's drag limit is the first defence; this
        // is the backstop).
        let x0 = area.a.x;
        let y0 = area.a.y;
        let x1 = x0 + (area.b.x - x0).max(Self::MIN_W);
        let y1 = y0 + (area.b.y - y0).max(Self::MIN_H);

        let mid = (x0 + x1) / 2;
        let left = (x0 + 2, mid - 2);
        let right = (mid + 2, x1 - 2);
        let head_y = y0 + 1;
        // Reserve the bottom: OK/Cancel land at y1-3 (dialog button_row); the
        // wide Add/Remove row sits two rows above at y1-6..y1-4, so the lists end
        // at y1-7.
        let list_top = y0 + 2;
        let list_bot = y1 - 7;
        let btn_top = y1 - 6;
        let btn_bot = y1 - 4;

        ShuttleLayout {
            left_header: Rect::new(left.0, head_y, left.1, head_y + 1),
            right_header: Rect::new(right.0, head_y, right.1, head_y + 1),
            avail_list: Rect::new(left.0, list_top, left.1 - 1, list_bot),
            avail_bar: Rect::new(left.1 - 1, list_top, left.1, list_bot),
            sel_list: Rect::new(right.0, list_top, right.1 - 1, list_bot),
            sel_bar: Rect::new(right.1 - 1, list_top, right.1, list_bot),
            add_btn: Rect::new(left.0, btn_top, left.1, btn_bot),
            remove_btn: Rect::new(right.0, btn_top, right.1, btn_bot),
        }
    }
```

Add the struct just above `struct Shuttle` (module-private):

```rust
/// The rect of every `Shuttle` child, computed from the widget area by
/// [`Shuttle::layout`]. Kept as a plain record so `new` and `change_bounds`
/// derive identical geometry.
struct ShuttleLayout {
    left_header: Rect,
    right_header: Rect,
    avail_list: Rect,
    avail_bar: Rect,
    sel_list: Rect,
    sel_bar: Rect,
    add_btn: Rect,
    remove_btn: Rect,
}
```

- [ ] **Step 4: Run the layout test — it passes**

Run: `cargo test -j4 --lib ui::shuttle::tests::layout_splits_columns_and_places_wide_buttons`
Expected: PASS.

- [ ] **Step 5: Rewrite `new` to build children from `layout()` and store all ids**

Replace the body of `Shuttle::new` (the geometry + inserts, currently `src/ui/shuttle.rs:136-227`) so it uses `layout()`, builds wide non-selectable buttons, and stores every id. Add the new fields to `struct Shuttle` first:

```rust
pub(crate) struct Shuttle {
    group: Group,
    model: ShuttleModel,
    avail_id: ViewId,
    selected_id: ViewId,
    left_header_id: ViewId,
    right_header_id: ViewId,
    avail_bar_id: ViewId,
    sel_bar_id: ViewId,
    add_id: ViewId,
    remove_id: ViewId,
}
```

`new` body:

```rust
    pub(crate) fn new(
        area: Rect,
        left_title: &str,
        right_title: &str,
        find_mode: FindMode,
    ) -> Shuttle {
        let l = Self::layout(area);
        let mut group = Group::new(area);
        // Fill the owner on resize: the dialog's change_bounds cascade resizes
        // this widget, and our own change_bounds reflows the children.
        group.state_mut().grow_mode.hi_x = true;
        group.state_mut().grow_mode.hi_y = true;

        let left_header_id =
            group.insert(Box::new(Label::new(l.left_header, left_title, None)));
        let right_header_id =
            group.insert(Box::new(Label::new(l.right_header, right_title, None)));

        // Available column (left): SortedListBox + scroll bar.
        let avail_bar_id = group.insert(Box::new(ScrollBar::new(l.avail_bar)));
        let avail_id = group.insert(Box::new(
            SortedListBox::new(l.avail_list, 1, None, Some(avail_bar_id)).with_find(find_mode),
        ));

        // Selected column (right): plain ListBox (insertion order) + scroll bar.
        // FindMode::Filter narrows the local staged set as the user types (so
        // letters never leak to the Add/Remove hotkeys).
        let sel_bar_id = group.insert(Box::new(ScrollBar::new(l.sel_bar)));
        let selected_id = group.insert(Box::new(
            ListBox::new(l.sel_list, 1, None, Some(sel_bar_id)).with_find(FindMode::Filter),
        ));

        // Wide move buttons, each spanning the column it acts on: Add under the
        // Available (left) column, Remove under the Selected (right). Both are
        // marked non-selectable so Tab skips them (they stay operable by click,
        // Alt-A / Alt-R, and Insert/Delete/Enter on the focused list). Non-
        // selectable does not disable pre/post-process, so the Alt hotkey still
        // fires.
        let mut add = Button::new(l.add_btn, "~A~dd", CMD_ADD, ButtonFlags::new());
        add.state_mut().options.selectable = false;
        let add_id = group.insert(Box::new(add));

        let mut remove = Button::new(l.remove_btn, "~R~emove", CMD_REMOVE, ButtonFlags::new());
        remove.state_mut().options.selectable = false;
        let remove_id = group.insert(Box::new(remove));

        Shuttle {
            group,
            model: ShuttleModel::default(),
            avail_id,
            selected_id,
            left_header_id,
            right_header_id,
            avail_bar_id,
            sel_bar_id,
            add_id,
            remove_id,
        }
    }
```

(Confirm `Button` has a public `state_mut()`; if the field is `state`, use `add.state.options.selectable = false`. The `Button` struct in tvision-rs exposes `state: ViewState` — use whichever is accessible from the consumer crate. `ViewState` has `options: Options` public.)

- [ ] **Step 6: Bump both dialogs (and the embedded Shuttle) to height 25**

In `src/ui/oc_picker.rs`, the dialog (`src/ui/oc_picker.rs:117`) and the Shuttle rect (updated in Task 1) both use height 22. Change both to 25:

```rust
        let mut dlg = Dialog::new(Rect::new(0, 0, 72, 25), Some("Object classes".to_string()));
```
```rust
        let shuttle = Shuttle::new(
            Rect::new(0, 0, 72, 25),
            "Available",
            "Active",
            /* find */ FindMode::Filter,
        );
```

In `src/ui/multi_picker.rs`, the dialog (`src/ui/multi_picker.rs:150`) and Shuttle rect use 80×22. Change both to 80×25:

```rust
        let mut dlg = Dialog::new(Rect::new(0, 0, 80, 25), Some(title));
```
```rust
        let shuttle = Shuttle::new(
            Rect::new(0, 0, 80, 25),
            "Available",
            "Members",
            /* find */ FindMode::Highlight,
        );
```

- [ ] **Step 7: Fix the surface/dimming pixel test for the new height**

The test `the_focused_list_is_bright_and_its_sibling_recedes` (`src/ui/shuttle.rs:825-871`) builds `shuttle()` and a `Buffer::new(72, 22)` and reads specific cells. Update the `shuttle()` helper height to 25 (Step 8 below) and bump the buffer + DrawCtx rect here to 25:

```rust
        let mut buf = Buffer::new(72, 25);
        {
            let mut dc = DrawCtx::new(&mut buf, &theme, Rect::new(0, 0, 72, 25), Point::new(0, 0));
```

The sampled rows (`buf.get(4, 3)`, `buf.get(50, 3)`) are still inside the list area (`y 2..18` with height 25), so the coordinates stay valid.

- [ ] **Step 8: Update the `shuttle()` test helper to height 25**

```rust
    fn shuttle() -> Shuttle {
        Shuttle::new(
            Rect::new(0, 0, 72, 25),
            "Active",
            "Available",
            FindMode::Filter,
        )
    }
```

Note the helper's titles are `"Active"` (left) / `"Available"` (right) — only used by tests that address rows by content, so the label text is immaterial; leaving them is fine. (Optionally swap to `"Available"`/`"Active"` for clarity — not required.)

- [ ] **Step 9: Add a test asserting the buttons are non-selectable (out of Tab)**

```rust
    #[test]
    fn move_buttons_are_not_tab_stops() {
        let mut sh = shuttle();
        for id in [sh.add_id, sh.remove_id] {
            let selectable = sh
                .group
                .child_mut(id)
                .map(|c| c.state().options.selectable)
                .expect("button present");
            assert!(!selectable, "move buttons must be skipped by Tab traversal");
        }
    }
```

- [ ] **Step 10: Run the shuttle + consumer tests**

Run: `cargo test -j4 --lib ui::shuttle ui::oc_picker ui::multi_picker`
Expected: PASS (including the two new tests and the updated pixel test).

- [ ] **Step 11: Full check**

Run: `make check`
Expected: PASS.

- [ ] **Step 12: Commit**

```bash
git add src/ui/shuttle.rs src/ui/oc_picker.rs src/ui/multi_picker.rs
git commit -m "feat(ui): wide column-bound Add/Remove buttons, taller picker dialogs

Extract Shuttle geometry into layout(); Add spans the Available column,
Remove spans the Selected column. Both are non-selectable so Tab skips
them (still operable by click, Alt-A/Alt-R, Insert/Delete/Enter). Dialogs
grow to 25 rows to seat the wide-button row above OK/Cancel.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 3: Focus-driven graying of Add/Remove

Add is enabled only while the Available list is focused; Remove only while the Selected list is focused. tvision auto-grays the disabled button.

**Files:**
- Modify: `src/ui/shuttle.rs` (`reset_current`, `handle_event`; small helper `sync_move_commands`)
- Test: `src/ui/shuttle.rs` `mod tests`

**Interfaces:**
- Consumes: `CMD_ADD`, `CMD_REMOVE` (module consts), `self.avail_id`, `self.selected_id`, `self.group.current()`.
- Produces: `fn sync_move_commands(&mut self, ctx: &mut Context)` — enables exactly one of `CMD_ADD`/`CMD_REMOVE` based on which list is `group.current()`, disabling the other (and disabling both when neither list is current).

- [ ] **Step 1: Write a failing test for focus-driven command state**

The harness `Context` records deferred effects. `ctx.enable_command`/`disable_command` push `Deferred::EnableCommand`/`DisableCommand`. Extend the test `Harness` to expose the deferred queue, then assert. Add to `mod tests`:

```rust
    /// Whether the harness saw a deferred enable/disable for `cmd`.
    impl Harness {
        fn command_disabled(&self, cmd: Command) -> bool {
            // Last enable/disable wins.
            self.deferred.iter().rev().find_map(|d| match d {
                Deferred::DisableCommand(c) if *c == cmd => Some(true),
                Deferred::EnableCommand(c) if *c == cmd => Some(false),
                _ => None,
            }).unwrap_or(false)
        }
    }

    #[test]
    fn focus_on_available_enables_add_disables_remove() {
        let mut sh = shuttle();
        let mut h = Harness::new();
        sh.set_available(vec![row("a")], &mut h.ctx());
        sh.set_selected(vec![row("x")], &mut h.ctx());
        let aid = sh.avail_id;
        {
            let mut ctx = h.ctx();
            sh.group.focus_child(aid, &mut ctx);
            sh.sync_move_commands(&mut ctx);
        }
        assert!(!h.command_disabled(CMD_ADD), "Add enabled while Available focused");
        assert!(h.command_disabled(CMD_REMOVE), "Remove disabled while Available focused");
    }

    #[test]
    fn focus_on_selected_enables_remove_disables_add() {
        let mut sh = shuttle();
        let mut h = Harness::new();
        sh.set_available(vec![row("a")], &mut h.ctx());
        sh.set_selected(vec![row("x")], &mut h.ctx());
        let sid = sh.selected_id;
        {
            let mut ctx = h.ctx();
            sh.group.focus_child(sid, &mut ctx);
            sh.sync_move_commands(&mut ctx);
        }
        assert!(h.command_disabled(CMD_ADD), "Add disabled while Selected focused");
        assert!(!h.command_disabled(CMD_REMOVE), "Remove enabled while Selected focused");
    }
```

The `Harness` already owns `deferred: Vec<Deferred>` (`src/ui/shuttle.rs:652`); `Deferred` is imported in the test module (`src/ui/shuttle.rs:644`). If `Deferred::EnableCommand`/`DisableCommand` are not in scope, they are `tvision_rs::Deferred` variants — reference them via the existing `Deferred` import.

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -j4 --lib ui::shuttle::tests::focus_on_available_enables_add_disables_remove`
Expected: FAIL — `sync_move_commands` does not exist.

- [ ] **Step 3: Implement `sync_move_commands` and call it**

Add the helper to `impl Shuttle`:

```rust
    /// Enable exactly the move command whose source list is focused: Add when the
    /// Available list is current, Remove when the Selected list is current,
    /// disabling the other (both disabled when neither list is current, e.g. focus
    /// on OK/Cancel). tvision's deferred command set flips COMMAND_SET_CHANGED on
    /// the next idle pump, and each Button re-grays itself from
    /// `ctx.command_enabled`.
    fn sync_move_commands(&mut self, ctx: &mut Context) {
        let cur = self.group.current();
        if cur == Some(self.avail_id) {
            ctx.enable_command(CMD_ADD);
            ctx.disable_command(CMD_REMOVE);
        } else if cur == Some(self.selected_id) {
            ctx.disable_command(CMD_ADD);
            ctx.enable_command(CMD_REMOVE);
        } else {
            ctx.disable_command(CMD_ADD);
            ctx.disable_command(CMD_REMOVE);
        }
    }
```

Call it from `reset_current` (after the open-time focus is set) — replace the current `reset_current` (`src/ui/shuttle.rs:451-454`):

```rust
    fn reset_current(&mut self, ctx: &mut Context) {
        self.group.reset_current(ctx);
        self.group.focus_child(self.avail_id, ctx);
        self.sync_move_commands(ctx);
    }
```

And from `handle_event`, after the delegating `self.group.handle_event(ev, ctx)` at the end (`src/ui/shuttle.rs:551`), so a Tab that moves focus updates the graying:

```rust
        self.group.handle_event(ev, ctx);
        // A Tab/focus change may have moved currency between the two lists (or to
        // a button/OK/Cancel): re-derive which move command is live so the buttons
        // gray correctly.
        self.sync_move_commands(ctx);
```

- [ ] **Step 4: Run the two new tests — they pass**

Run: `cargo test -j4 --lib ui::shuttle::tests::focus_on`
Expected: PASS (both `focus_on_*` tests).

- [ ] **Step 5: Full check**

Run: `make check`
Expected: PASS. Watch for a clippy `if`/`else if`/`else` lint — the three-arm form is intentional; leave as-is.

- [ ] **Step 6: Commit**

```bash
git add src/ui/shuttle.rs
git commit -m "feat(ui): gray shuttle Add/Remove by which list is focused

Add is live only while the Available list is current, Remove only while
the Selected list is current; the other grays out via the command-set
machinery. Re-synced on open and after every focus change.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 4: Resizable dialogs (Shuttle reflow + OK/Cancel anchoring)

Enable the `grow` flag on both dialogs (auto-enables `drag_grow`), reflow the Shuttle's children on `change_bounds`, and anchor OK/Cancel to the bottom-right so they track the resize.

**Files:**
- Modify: `src/ui/shuttle.rs` (`change_bounds` override + `#[delegate]` skip list)
- Modify: `src/ui/oc_picker.rs` (dialog `grow` flag + OK/Cancel grow_mode)
- Modify: `src/ui/multi_picker.rs` (dialog `grow` flag + OK/Cancel grow_mode)
- Test: `src/ui/shuttle.rs` `mod tests`

**Interfaces:**
- Consumes: `Shuttle::layout` (Task 2), the stored child ids (Task 2).
- Produces: `Shuttle::change_bounds(&mut self, bounds: Rect)` — repositions every child via `layout(bounds)`; the group's own bounds are set to `bounds`.

- [ ] **Step 1: Write a failing test for reflow on change_bounds**

```rust
    #[test]
    fn change_bounds_reflows_children_wider() {
        use tvision_rs::View as _;
        let mut sh = shuttle(); // 72 x 25
        let before = Shuttle::layout(Rect::new(0, 0, 72, 25));
        // Grow to 100 x 30.
        View::change_bounds(&mut sh, Rect::new(0, 0, 100, 30));
        let want = Shuttle::layout(Rect::new(0, 0, 100, 30));
        assert_ne!(want.sel_list.b.x, before.sel_list.b.x, "test premise: geometry changes");
        // Each child now sits at the recomputed rect.
        let sel_bounds = sh.group.child_mut(sh.selected_id).unwrap().state().get_bounds();
        assert_eq!(sel_bounds, want.sel_list, "Selected list follows the new width");
        let add_bounds = sh.group.child_mut(sh.add_id).unwrap().state().get_bounds();
        assert_eq!(add_bounds, want.add_btn, "Add button widens with its column");
    }
```

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test -j4 --lib ui::shuttle::tests::change_bounds_reflows_children_wider`
Expected: FAIL — the delegated `change_bounds` does not reposition the inner children by `layout` (they keep their old rects because their grow_mode is default/fixed).

- [ ] **Step 3: Add `change_bounds` to the delegate skip list and implement it**

Change the delegate attribute (`src/ui/shuttle.rs:441`) to also skip `change_bounds`:

```rust
#[delegate(to = group, skip(handle_event, as_any_mut, reset_current, value, set_value, set_value_ctx, change_bounds))]
```

Add the method inside `impl View for Shuttle`:

```rust
    /// Reflow on resize. The dialog's change_bounds cascade calls this with the
    /// widget's new bounds; recompute every child rect from `layout` and apply it.
    /// (The two-column split moves the midpoint by delta/2, which per-child
    /// grow_mode cannot express, so we reposition explicitly rather than delegate
    /// to the group's grow-mode reflow.) Scrollbar page-step refresh is a cosmetic
    /// follow-up handled lazily by the lists on their next draw.
    fn change_bounds(&mut self, bounds: Rect) {
        self.group.state_mut().set_bounds(bounds);
        let l = Self::layout(bounds);
        let places = [
            (self.left_header_id, l.left_header),
            (self.right_header_id, l.right_header),
            (self.avail_bar_id, l.avail_bar),
            (self.avail_id, l.avail_list),
            (self.sel_bar_id, l.sel_bar),
            (self.selected_id, l.sel_list),
            (self.add_id, l.add_btn),
            (self.remove_id, l.remove_btn),
        ];
        for (id, rect) in places {
            if let Some(c) = self.group.child_mut(id) {
                c.change_bounds(rect);
            }
        }
    }
```

(`ViewState::set_bounds` and `View::change_bounds`/`state().get_bounds()` are the tvision-rs seams already used elsewhere in this file. If `set_bounds` is not directly reachable on the group's state, use `self.group.change_bounds(bounds)` first — its own child reflow is a no-op for default-grow_mode children — then apply the `places` loop to override positions.)

- [ ] **Step 4: Run the reflow test — it passes**

Run: `cargo test -j4 --lib ui::shuttle::tests::change_bounds_reflows_children_wider`
Expected: PASS.

- [ ] **Step 5: Enable `grow` and anchor OK/Cancel in `oc_picker`**

In `src/ui/oc_picker.rs`, after `dlg.button_row(...)` returns its ids (currently the return value is discarded), capture and anchor them, and add the `grow` flag. Replace the `button_row` call + the two `center_*` lines region:

```rust
        dlg.state_mut().options.center_x = true;
        dlg.state_mut().options.center_y = true;
        // Allow the user to resize the dialog (grow flag also enables drag_grow).
        dlg.set_flags(tv::WindowFlags {
            r#move: true,
            close: true,
            grow: true,
            ..tv::WindowFlags::default()
        });
```

```rust
        let button_ids = dlg.button_row(
            &[
                (
                    "~O~K",
                    Command::OK,
                    ButtonFlags {
                        default: true,
                        ..ButtonFlags::new()
                    },
                ),
                ("~C~ancel", Command::CANCEL, ButtonFlags::new()),
            ],
            ButtonRowAlign::Right,
        );
        // Keep OK/Cancel pinned to the bottom-right as the dialog grows: both the
        // top and bottom edges track the owner (lo_y + hi_y translate the fixed-
        // height button down), likewise lo_x + hi_x to the right.
        for id in button_ids {
            if let Some(b) = dlg.child_mut(id) {
                let gm = &mut b.state_mut().grow_mode;
                gm.lo_x = true;
                gm.hi_x = true;
                gm.lo_y = true;
                gm.hi_y = true;
            }
        }
```

`WindowFlags` is re-exported at the crate root, so reference it as `tv::WindowFlags` (or add it to the `use tvision_rs::{...}` list). `ButtonRowAlign` is already imported (`src/ui/oc_picker.rs:24`). `set_flags`, `child_mut`, and `state_mut` are public on `Dialog`/`View`.

- [ ] **Step 6: Enable `grow` and anchor OK/Cancel in `multi_picker`**

Apply the identical treatment in `src/ui/multi_picker.rs` around `src/ui/multi_picker.rs:150-184`: add the `set_flags` grow block after the `center_*` lines, capture the `button_row` return into `button_ids`, and run the same grow_mode loop. Use the same `WindowFlags`/import approach as oc_picker.

- [ ] **Step 7: Full check**

Run: `make check`
Expected: PASS.

- [ ] **Step 8: Manual verification against the demo server**

```bash
scripts/test-ldap.sh start
export EDAPTOR_TEST_ADMIN_PW=adminpassword
cargo run -j4 -- --config examples/demo-config.toml
```

Confirm, in both the Object Class editor and a membership (Edit Member) editor:
- Available is on the LEFT, the Selected set on the RIGHT.
- Tab cycles the two lists → OK → Cancel and never lands on Add/Remove.
- Add grays out unless the Available (left) list is focused; Remove grays unless the Selected (right) list is focused.
- Add/Remove still fire via mouse click, Alt-A / Alt-R, and Insert/Delete/Enter on the focused list.
- Dragging the lower-right corner (or Shift+Arrow) enlarges the dialog; both columns, scrollbars, the wide button row, and OK/Cancel reflow correctly; shrinking stops at the minimum without overlapping widgets.

Record the result in the commit message if anything needed adjustment (e.g. scroll-step refresh).

- [ ] **Step 9: Commit**

```bash
git add src/ui/shuttle.rs src/ui/oc_picker.rs src/ui/multi_picker.rs
git commit -m "feat(ui): resizable picker dialogs with reflowing shuttle

Enable the dialog grow flag (drag_grow) on both pickers; the Shuttle
reflows its columns, scrollbars and wide buttons on change_bounds, and
OK/Cancel stay anchored bottom-right via grow_mode.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 5: Changelog & docs

**Files:**
- Modify: `CHANGES.md`
- Modify: `docs/src/configuration/widgets.md`
- Verify: `README.md`

- [ ] **Step 1: Add a CHANGES.md entry**

Under the current unreleased section, add:

```markdown
- Picker dialogs (object classes, membership) now use the conventional transfer
  layout — Available on the left, the selected set on the right — with wide Add
  and Remove buttons under their respective columns. Each button is enabled only
  while its list is focused and is skipped by Tab (still usable via click,
  Alt-A/Alt-R, or Insert/Delete/Enter). The dialogs are resizable.
```

- [ ] **Step 2: Update the widgets mdBook page**

In `docs/src/configuration/widgets.md`, find the `membership` and objectClass widget descriptions. Update any text or ASCII that states column sides or the button layout to match: Available left, selected right, wide Add (left) / Remove (right), Tab skips the move buttons, resizable dialog. Do not restate config reference that lives elsewhere.

Run: `make docs`
Expected: mdBook builds with no broken-link/errors.

- [ ] **Step 3: Verify README**

Read `README.md`; confirm it does not describe the picker column sides or button layout in a way that now contradicts the new behaviour. It should point at the mdBook for widget detail — if it does, no change. If it has a stale sentence, fix it minimally.

- [ ] **Step 4: Commit**

```bash
git add CHANGES.md docs/src/configuration/widgets.md README.md
git commit -m "docs: conventional resizable picker layout with column-bound buttons

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Self-Review Notes (for the executor)

- **Spec coverage:** Orientation (Task 1), wide column-bound buttons + non-selectable + taller dialog (Task 2), focus-driven graying (Task 3), resizable dialog + OK/Cancel anchoring (Task 4), docs/changelog (Task 5) — every spec section maps to a task.
- **Known limitation (documented in Task 4 Step 3):** the resize cascade calls `change_bounds` without a `Context`, so list scrollbar *page steps* are not refreshed via `on_bounds_changed` during an interactive resize. This is cosmetic (PageUp/PageDown distance / thumb size), not a correctness issue — the visible rows and scroll range still track. If manual verification (Task 4 Step 8) shows it matters, add a `Shuttle::on_bounds_changed` override that calls each list's `on_bounds_changed(ctx)`, and confirm whether the drag path delivers it (it is delivered on the deferred `ChangeBounds` path but not the group cascade — may require requesting a bounds change by id).
- **Type consistency:** `layout()` returns `ShuttleLayout` used identically in `new` (Task 2) and `change_bounds` (Task 4). Field names (`avail_list`, `sel_list`, `add_btn`, `remove_btn`, …) and stored ids (`avail_id`, `selected_id`, `add_id`, `remove_id`, `left_header_id`, `right_header_id`, `avail_bar_id`, `sel_bar_id`) are used consistently across tasks.
- **API confirmations (verified in tvision-rs 0.9 during planning):** `Button` has a public `state: ViewState` field and `state_mut()` (both work for `options.selectable = false`); `WindowFlags` is re-exported at the crate root (`tv::WindowFlags`); `Dialog::child_mut`, `Dialog::set_flags`, `Button::state_mut`, and `ViewState::set_bounds` are all public.
