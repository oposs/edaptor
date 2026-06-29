# TUI polish: light theme + interaction rough edges — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the dark `classic_blue` TUI palette with a centralized light (Solarized Light) theme and fix ~14 interaction/affordance rough edges (panel uniformity, active-panel indication, focus/selection contrast, cursor visibility, Tab containment, click-to-focus labels, focus-gated scrollbars, object-types dual-list, multivalue buttons, search prompt, dialog button widths).

**Architecture:** All color choices funnel through one new `edaptor_theme()` builder (`src/ui/theme.rs`) that clones `Theme::classic_blue()` and overrides roles — no TOML config surface. Active-panel tint comes for free from tvision's active/inactive list roles where available, and from a focus-keyed background fill in `draw()` for the tree and form panes. A new shared `DualList` widget (`src/ui/dual_list.rs`), extracted from `membership.rs`, backs both membership and object-types. The rest are localized edits to panes, dialogs, and widgets.

**Tech Stack:** Rust, tvision-rs 0.3 (Turbo Vision port). Build with `cargo`/`make`, cap parallelism at 4 cores.

## Global Constraints

- **Parallelism:** never exceed 4 cores — `cargo test -j4`, `cargo clippy -j4`.
- **Containers:** podman, not docker (only relevant for the demo server).
- **Done = green `make check`:** fmt + `clippy --all-targets -- -D warnings` + tests.
- **Comments/identifiers in English;** any user-facing copy may be localized but here stays English.
- **Docs are part of done:** update `CHANGES.md` (every user-visible change) and the relevant `docs/src/` page; keep `examples/config.toml` ⇄ `docs/src/configuration/full-example.md` consistent (no config-format change here, so likely untouched).
- **tvision-rs imports** (verified): `use tvision_rs::{Role, Theme, Color, Style, Modifiers};`. `Style::new(fg: Color, bg: Color) -> Style`; `Color::Rgb(u8,u8,u8)`; `Theme::classic_blue() -> Theme`; `Theme::set_style(&mut self, role: Role, style: Style)`.
- **No data-logic changes:** this is presentation/interaction only.
- **Solarized Light reference hexes:** base3 `#fdf6e3`, base2 `#eee8d5`, base1 `#93a1a1`, base01 `#586e75`, base00 `#657b83`, blue `#268bd2`, cyan `#2aa198`, red `#dc322f`. Active panel = base3 (brightest); inactive = base2; body text = base01; accent = blue; editable field bg = `#fffdf3`.

---

## File Structure

| File | Responsibility | Tasks |
|------|----------------|-------|
| `src/ui/theme.rs` (new) | `edaptor_theme()` — the single palette definition | 1 |
| `src/ui/app.rs` | wire `edaptor_theme()` at the program build site | 1 |
| `src/ui/panes/tree.rs` | focus-keyed bg fill; focus-gated scrollbar | 3, 7 |
| `src/ui/panes/leaf.rs` | `Filter:` prompt; focus-gated scrollbar | 6, 9 |
| `src/ui/panes/form.rs` | Tab containment; click-to-focus label; focus bg | 2, 4, 5 |
| `src/ui/scroll_group.rs` | focus-keyed backdrop role | 4 |
| `src/ui/dual_list.rs` (new) | shared two-column move-list widget | 10 |
| `src/ui/membership.rs` | consume `DualList` | 11 |
| `src/ui/oc_picker.rs` | object-types via `DualList` | 12 |
| `src/ui/multivalue.rs` | `[+ Add]` / `[- Del]` buttons | 13 |
| `src/ui/dialog/guard.rs` | widen dialog / pad buttons | 8 |

**Suggested execution order / phases:**
1. Theme (Task 1) — keystone, immediately visible.
2. Active-panel tint + focus fills (Tasks 2–4).
3. Navigation (Tasks 5–6 … wait, see numbering below).

Tasks are numbered in dependency order below. Each ends with green `make check` (or a stated manual-verify for purely visual changes) and a commit.

---

### Task 1: Centralized light theme

**Files:**
- Create: `src/ui/theme.rs`
- Modify: `src/ui/app.rs:731` (and module declaration in `src/ui/mod.rs`)
- Test: inline `#[cfg(test)]` in `src/ui/theme.rs`

**Interfaces:**
- Produces: `pub(crate) fn edaptor_theme() -> tvision_rs::Theme`

This is the keystone. All later tint/contrast tasks assume these role values. The blue-vs-cyan panel difference disappears because we set the *background* of the list/outline/input/normal roles to the shared panel surfaces.

- [ ] **Step 1: Write the failing test**

In a new file `src/ui/theme.rs`:

```rust
//! The single source of truth for eDAPtor's colors. We clone tvision's
//! `classic_blue` and override roles to a light (Solarized Light) palette.
//! No TOML surface — tune here; could be lifted to config later.

use tvision_rs::{Color, Role, Style, Theme};

/// Solarized Light reference colors used across the theme.
const BASE3: Color = Color::Rgb(0xfd, 0xf6, 0xe3); // brightest surface (active pane)
const BASE2: Color = Color::Rgb(0xee, 0xe8, 0xd5); // inactive pane surface
const BASE1: Color = Color::Rgb(0x93, 0xa1, 0xa1); // secondary / frames / disabled
const BASE01: Color = Color::Rgb(0x58, 0x6e, 0x75); // body text
const BLUE: Color = Color::Rgb(0x26, 0x8b, 0xd2); // accent / current item bg
const INPUT_BG: Color = Color::Rgb(0xff, 0xfd, 0xf3); // editable field bg
const DESKTOP: Color = Color::Rgb(0xe3, 0xdd, 0xc8); // desktop behind panes
const SEL_BG: Color = Color::Rgb(0xb5, 0xcd, 0xd8); // multi-selected (staged) bg

/// Build eDAPtor's light theme.
pub(crate) fn edaptor_theme() -> Theme {
    let mut t = Theme::classic_blue();
    // Panel surfaces: kill the cyan ListBox background; everything shares base2/base3.
    t.set_style(Role::Background, Style::new(BASE01, DESKTOP));
    t.set_style(Role::Normal, Style::new(BASE01, BASE2));
    t.set_style(Role::ListNormalInactive, Style::new(BASE01, BASE2));
    t.set_style(Role::ListNormalActive, Style::new(BASE01, BASE3));
    t.set_style(Role::OutlineNormal, Style::new(BASE01, BASE2));
    // Current item: same accent everywhere (list, outline, form).
    t.set_style(Role::ListFocused, Style::new(BASE3, BLUE));
    t.set_style(Role::OutlineFocused, Style::new(BASE3, BLUE));
    t.set_style(Role::Focused, Style::new(BASE3, BLUE));
    // Multi-selected / staged rows.
    t.set_style(Role::ListSelected, Style::new(BASE01, SEL_BG));
    t.set_style(Role::OutlineSelected, Style::new(BASE01, SEL_BG));
    // Editable fields: brightest, signals "type here".
    t.set_style(Role::InputNormal, Style::new(BASE01, INPUT_BG));
    t.set_style(Role::InputSelected, Style::new(BASE3, BLUE));
    // Secondary chrome.
    t.set_style(Role::Disabled, Style::new(BASE1, BASE2));
    t.set_style(Role::ScrollBarPage, Style::new(BASE1, BASE2));
    t.set_style(Role::ScrollBarControls, Style::new(BASE01, BASE2));
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bg(t: &Theme, role: Role) -> Color {
        // Round-trips through the public draw path: classic_blue stores Style by
        // role; we read it back via a fresh DrawCtx-free accessor.
        t.style(role).bg
    }

    #[test]
    fn panels_share_one_background_family() {
        let t = edaptor_theme();
        // The leaf ListBox no longer paints cyan: inactive list bg == base2.
        assert_eq!(bg(&t, Role::ListNormalInactive), BASE2);
        assert_eq!(bg(&t, Role::OutlineNormal), BASE2);
        // Active pane list is the brightest surface.
        assert_eq!(bg(&t, Role::ListNormalActive), BASE3);
    }

    #[test]
    fn current_item_uses_accent_everywhere() {
        let t = edaptor_theme();
        assert_eq!(bg(&t, Role::ListFocused), BLUE);
        assert_eq!(bg(&t, Role::OutlineFocused), BLUE);
        assert_eq!(bg(&t, Role::Focused), BLUE);
    }
}
```

> **Note on `t.style(role)`:** confirm the public accessor name on `Theme` (the draw path uses `ctx.style(role)`, which delegates to the theme). If `Theme` exposes `style(&self, Role) -> Style` publicly, use it; otherwise add a thin `#[cfg(test)]` getter or assert via a `DrawCtx`. Check `tvision-rs-0.3.0/src/theme.rs` for the accessor before writing the test body.

- [ ] **Step 2: Declare the module.** In `src/ui/mod.rs` add `mod theme;` (or `pub(crate) mod theme;`) alongside the other `mod` lines.

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -j4 ui::theme 2>&1 | tail -20`
Expected: compile error (module/accessor) or assertion context — confirm it builds once the accessor is correct, then fails only if values are wrong (they shouldn't be). The real purpose is to lock the role→color contract.

- [ ] **Step 4: Wire the theme into the program**

In `src/ui/app.rs`, replace line 731:

```rust
        Theme::classic_blue(),
```

with:

```rust
        crate::ui::theme::edaptor_theme(),
```

Keep the existing `use` of `Theme` if still referenced elsewhere; otherwise remove the now-unused import to satisfy `-D warnings`.

- [ ] **Step 5: Verify**

Run: `cargo test -j4 -p edaptor ui::theme 2>&1 | tail -20`
Expected: PASS.
Run: `cargo clippy --all-targets -j4 -- -D warnings 2>&1 | tail -20`
Expected: clean.

- [ ] **Step 6: Manual smoke (visual)**

Run the demo server and app per `CLAUDE.md` (`scripts/test-ldap.sh start`, `EDAPTOR_TEST_ADMIN_PW=adminpassword cargo run -- --config examples/demo-config.toml`). Confirm: light panels, no cyan, dark text, the hardware cursor is visible in input fields, the highlighted row is blue. Note any role that's still dark-blue (menu bar / status line) and add its override to `edaptor_theme()` (likely `Role::MenuNormal`/`MenuSelected`, `Role::StatusNormal` — check the `Role` enum for exact names). Re-run until nothing stays dark.

- [ ] **Step 7: Commit**

```bash
git add src/ui/theme.rs src/ui/mod.rs src/ui/app.rs
git commit -m "feat(ui): centralized light (Solarized) theme via edaptor_theme()"
```

---

### Task 2: Form pane focus background fill

**Files:**
- Modify: `src/ui/panes/form.rs` (struct `FormPane`, `View::draw`)

**Interfaces:**
- Consumes: `Role::ListNormalActive` (base3) / `Role::ListNormalInactive` (base2) from Task 1 — reuse as the focused/unfocused fill so panes match the lists.

The form has no built-in active/inactive role pair, so we fill its background based on whether it is in the active focus chain. The pattern is taken verbatim from tvision's Splitter, which keys on `self.state().state.active` (see `widgets/splitter/mod.rs:212`).

- [ ] **Step 1: Add a draw override that fills before delegating**

`FormPane` currently delegates `draw` to its inner `group` via `#[delegate(to = group)]`. Override `draw` explicitly so it fills the pane background first. Add to the `impl View for FormPane` block (the one with `as_any_mut`):

```rust
    fn draw(&mut self, ctx: &mut DrawCtx) {
        // Active pane = brightest (base3); inactive = base2. Mirrors the list panes
        // which get this from ListNormalActive/Inactive automatically.
        let role = if self.group.state().state.active {
            Role::ListNormalActive
        } else {
            Role::ListNormalInactive
        };
        let style = ctx.style(role);
        let extent = self.group.state().get_extent();
        ctx.fill(extent, ' ', style);
        self.group.draw(ctx);
    }
```

Ensure `Role` and `DrawCtx` are imported in `form.rs` (add to the existing `use tvision_rs::{...}`).

- [ ] **Step 2: Build & clippy**

Run: `cargo build -j4 2>&1 | tail -20` then `cargo clippy --all-targets -j4 -- -D warnings 2>&1 | tail -20`
Expected: clean.

- [ ] **Step 3: Manual verify (visual)**

Run the app. Tab to the form pane: its background brightens (cream) vs base2 when another pane is active. The empty area below the last field shows the fill, not the desktop.

- [ ] **Step 4: Commit**

```bash
git add src/ui/panes/form.rs
git commit -m "feat(ui): brighten the form pane background when it is the active pane"
```

---

### Task 3: Tree pane focus background fill

**Files:**
- Modify: `src/ui/panes/tree.rs` (`impl View for TreePane`)

**Interfaces:**
- Consumes: same `ListNormalActive`/`Inactive` roles as Task 2.

`TreePane` delegates everything to `outline`. Add a `draw` override that fills based on active state, then draws the outline. The outline paints its own rows; the fill shows behind/around them and in empty rows, matching the list panes.

- [ ] **Step 1: Add the draw override**

In the `#[delegate(to = outline)] impl View for TreePane` block (which already overrides `as_any_mut` and `handle_event`), add:

```rust
    fn draw(&mut self, ctx: &mut DrawCtx) {
        let role = if self.outline.state().state.active {
            Role::ListNormalActive
        } else {
            Role::ListNormalInactive
        };
        let style = ctx.style(role);
        let extent = self.outline.state().get_extent();
        ctx.fill(extent, ' ', style);
        self.outline.draw(ctx);
    }
```

Add `Role`, `DrawCtx` to the `use tvision_rs::{...}` line in `tree.rs` if missing.

- [ ] **Step 2: Build & clippy**

Run: `cargo clippy --all-targets -j4 -- -D warnings 2>&1 | tail -20`
Expected: clean.

- [ ] **Step 3: Manual verify (visual)**

Tab to the tree pane → it brightens; tree rows readable on the cream surface; the selected branch shows the blue accent.

- [ ] **Step 4: Commit**

```bash
git add src/ui/panes/tree.rs
git commit -m "feat(ui): brighten the tree pane background when it is the active pane"
```

---

### Task 4: ScrollGroup focus-keyed backdrop

**Files:**
- Modify: `src/ui/scroll_group.rs:169-174` (`draw`)

**Interfaces:**
- Consumes: `ListNormalActive`/`Inactive` from Task 1.

`ScrollGroup::draw` currently fills with `Role::Background` (now the *desktop* tan, which would look wrong inside the form). Change it to the active/inactive panel surface so the scrollable form area matches the rest of the pane.

- [ ] **Step 1: Update the existing test for the new backdrop role**

`backdrop_fill_covers_uncovered_rows` (scroll_group.rs:382) asserts the uncovered row is a blank space — still true. No change needed to that test. Add an assertion-free note; the role change is visual.

- [ ] **Step 2: Change the fill role**

Replace, in `draw` (scroll_group.rs:170):

```rust
        let style = ctx.style(Role::Background);
```

with:

```rust
        // Match the owning pane: brightest when the pane is the active one.
        let role = if self.group.state().state.active {
            Role::ListNormalActive
        } else {
            Role::ListNormalInactive
        };
        let style = ctx.style(role);
```

- [ ] **Step 3: Verify**

Run: `cargo test -j4 scroll_group 2>&1 | tail -20`
Expected: PASS (all existing ScrollGroup tests still green).
Run: `cargo clippy --all-targets -j4 -- -D warnings 2>&1 | tail -20`

- [ ] **Step 4: Commit**

```bash
git add src/ui/scroll_group.rs
git commit -m "fix(ui): scroll group backdrop tracks active-pane surface, not desktop"
```

---

### Task 5: Reserve Tab for switching panes

**Files:**
- Modify: `src/ui/panes/form.rs` (`handle_event`, ~lines 412-457)

**Interfaces:** none new.

The form's inner `Group` consumes Tab to cycle its own children, so Tab "goes into the panel." Intercept Tab in `FormPane::handle_event` and let it bubble to the `Splitter` (do **not** forward to the inner group, do **not** clear it), so the splitter moves to the next pane. Within-pane field movement already uses Up/Down (form.rs:451).

- [ ] **Step 1: Intercept Tab before the group sees it**

In `handle_event`, just before the `let nav = ...` block (form.rs:451), add:

```rust
        // Tab is reserved for switching panes. Do not let the inner group consume
        // it for intra-pane focus cycling — return without clearing so the parent
        // Splitter receives it and moves to the next pane.
        if matches!(ev, Event::KeyDown(k) if k.key == Key::Tab) {
            self.sync_into_form();
            return;
        }
```

(`Key` is already imported — it's used for `Key::Enter`/`Key::Up` in this file.)

- [ ] **Step 2: Audit leaf & tree for the same**

Check `LeafPane::handle_event` and `TreePane::handle_event`: if either forwards Tab to a child that cycles focus internally, add the same early-return-without-clear. The leaf's `Group` (search + list) may cycle search↔list on Tab — if so, intercept Tab there too so it bubbles. Tree delegates to `Outline`; verify Tab is not swallowed (Outline typically uses arrows). Only add interceptors where a manual test shows Tab failing to leave the pane.

- [ ] **Step 3: Manual verify**

Run the app. From each pane, Tab cycles tree → leaf → form → tree (and Shift-Tab back), never landing "inside" a pane's fields. Up/Down still moves between form fields.

- [ ] **Step 4: Build, clippy, commit**

Run: `cargo clippy --all-targets -j4 -- -D warnings 2>&1 | tail -20`

```bash
git add src/ui/panes/form.rs src/ui/panes/leaf.rs src/ui/panes/tree.rs
git commit -m "fix(ui): Tab switches panes only, never descends into a pane"
```

---

### Task 6: Click a form label to focus its field

**Files:**
- Modify: `src/ui/panes/form.rs` (`handle_event` + a label→value id lookup)

**Interfaces:**
- Consumes: `self.label_ids` / `self.value_ids` (parallel vecs, form.rs:44-47).

A click on a field's read-only **label** cell should focus that field's value editor. Map a mouse-down whose position is inside a label cell to a `focus_child` on the paired value id.

- [ ] **Step 1: Add a helper that maps a screen point to a value id**

Add to `impl FormPane`:

```rust
    /// If `pt` (screen coords) falls inside a label cell, return the paired value
    /// editor's id so a click on the label focuses the field.
    fn value_id_for_label_hit(&self, pt: tv::Point) -> Option<tv::ViewId> {
        for (label_id, value_id) in self.label_ids.iter().zip(self.value_ids.iter()) {
            if let Some(sg) = self.scroll_ref() {
                if let Some(child) = sg.logical_screen_bounds(*label_id) {
                    if child.contains(pt) {
                        return Some(*value_id);
                    }
                }
            }
        }
        None
    }
```

> The exact accessor for a child's on-screen bounds depends on `ScrollGroup`. The label cells live inside the `ScrollGroup`; their screen position = group origin + logical rect − scroll top. Add a small helper `ScrollGroup::logical_screen_bounds(id) -> Option<Rect>` that returns the child's current (repositioned) bounds translated to screen space (it already stores logical rects and `top`; the group's own bounds give the origin). If a simpler path exists (e.g. the framework already routes the click to the label child), prefer intercepting at the child and posting focus — verify against `form.rs` mouse handling before implementing.

- [ ] **Step 2: Handle the label click in `handle_event`**

Near the top of `handle_event`, before delegating to the group:

```rust
        if let Event::MouseDown { pos, .. } = ev {
            if let Some(vid) = self.value_id_for_label_hit(*pos) {
                if let Some(sg) = self.scroll_mut() {
                    sg.focus_child(vid, ctx);
                }
                ev.clear();
                return;
            }
        }
```

> Confirm the mouse event variant/field names against tvision-rs (`Event::MouseDown { pos, .. }` vs `MouseEvent`). Grep `tvision-rs-0.3.0/src/event` for the exact shape and adapt.

- [ ] **Step 3: Manual verify**

Run the app, open an entry, click a field's label text → that field's editor takes focus (cursor appears in it).

- [ ] **Step 4: Build, clippy, commit**

```bash
git add src/ui/panes/form.rs src/ui/scroll_group.rs
git commit -m "feat(ui): clicking a form label focuses its value field"
```

---

### Task 7: Focus-gated scrollbar for the leaf pane

**Files:**
- Modify: `src/ui/panes/leaf.rs` (struct, `new`, `draw`/`handle_event`)

**Interfaces:**
- Produces: a `v_bar: ViewId` field on `LeafPane`.

`ListBox::new(rect, step, h_bar, v_bar)` accepts an optional vertical-bar id. Add a `ScrollBar` sibling, pass its id to the `ListBox`, and toggle bar+list-width on focus: when the pane is active *and* content overflows, show the bar in the right column and shrink the list by one column; otherwise hide the bar and let the list reclaim the column.

- [ ] **Step 1: Add the bar in `new()`**

Replace the `ListBox` construction in `LeafPane::new` (leaf.rs:38-42) so the bar is created first and wired in:

```rust
        // Vertical scroll bar in the right column (width 1 ⇒ vertical). Hidden until
        // the pane is focused and the list overflows (Task 7 toggles visibility).
        let h = bounds.b.y - bounds.a.y;
        let mut v_bar = ScrollBar::new(Rect::new(w - 1, 1, w, h));
        v_bar.state_mut().state.visible = false;
        v_bar.state_mut().grow_mode.hi_x = true; // pin to right column on resize
        v_bar.state_mut().grow_mode.hi_y = true;
        let v_bar = group.insert(Box::new(v_bar));
        // List fills remaining width/height; reserve the bar lane only while shown.
        let mut list = ListBox::new(Rect::new(0, 1, w - 1, h), 1, None, Some(v_bar));
        list.state_mut().grow_mode.hi_x = true;
        list.state_mut().grow_mode.hi_y = true;
        let list_id = group.insert(Box::new(list));
```

Add `ScrollBar` to the `use tvision_rs::{...}` line. Add `v_bar` to the struct and the constructor's struct literal.

- [ ] **Step 2: Toggle visibility on focus + overflow**

In `LeafPane::handle_event` (after the group handles the event, so list length/position is current), add a helper call. Implement:

```rust
    fn sync_scrollbar(&mut self, ctx: &mut Context) {
        let active = self.group.state().state.active;
        let (len, page) = self.list_extent(); // rows total, visible rows
        let overflow = len > page;
        if let Some(bar) = self.group.child_mut(self.v_bar) {
            bar.state_mut().state.visible = active && overflow;
        }
        // Reclaim/return the bar lane: list width = pane width minus (bar ? 1 : 0).
        let w = self.group.state().get_extent().b.x;
        let h = self.group.state().get_extent().b.y;
        let list_w = if active && overflow { w - 1 } else { w };
        if let Some(list) = self.group.child_mut(self.list_id) {
            list.change_bounds(Rect::new(0, 1, list_w, h));
        }
    }
```

> `list_extent()` returns `(total_rows, visible_rows)`. Derive total from the rows you populate (`repopulate`) and visible from the list height (`h - 1`). If `ListBox` exposes range/page accessors, prefer those. Call `self.sync_scrollbar(ctx)` at the end of `handle_event` and after `repopulate`.

- [ ] **Step 3: Manual verify (visual)**

Run the app against the demo server (≈600 users → leaf list overflows). When the leaf pane is **not** focused: no bar, list uses full width. Tab to it: a scrollbar appears in the right column and the list narrows by one column; scrolling works. Accept the one-column shift on focus change (chosen behavior).

- [ ] **Step 4: Build, clippy, commit**

```bash
git add src/ui/panes/leaf.rs
git commit -m "feat(ui): leaf pane shows a scrollbar only while focused and overflowing"
```

---

### Task 8: Focus-gated scrollbar for the tree pane

**Files:**
- Modify: `src/ui/panes/tree.rs` (struct, `new`, `handle_event`)

**Interfaces:** mirror Task 7.

`Outline::new(bounds, h_bar, v_bar, root)` takes optional bar ids. The tree currently passes `None, None`. Because `TreePane` holds the `Outline` directly (not inside a `Group`), wrap it: convert `TreePane` to own a `Group` containing the `Outline` + a `ScrollBar`, OR add the bar as a sibling managed by the pane. Simplest: keep `Outline` but add a sibling `ScrollBar` and a small owning `Group`.

- [ ] **Step 1: Give TreePane a group + bar**

Change the struct to hold a `group: tv::Group`, the `outline` child id, and `v_bar: ViewId`. In `new()`:

```rust
    pub(crate) fn new(bounds: Rect, root: Option<Box<tv::Node>>, state: Shared) -> Self {
        let w = bounds.b.x - bounds.a.x;
        let h = bounds.b.y - bounds.a.y;
        let mut group = tv::Group::new(bounds);
        group.state_mut().options.first_click = true;
        let mut v_bar = ScrollBar::new(Rect::new(w - 1, 0, w, h));
        v_bar.state_mut().state.visible = false;
        v_bar.state_mut().grow_mode.hi_x = true;
        v_bar.state_mut().grow_mode.hi_y = true;
        let v_bar = group.insert(Box::new(v_bar));
        let mut outline = tv::Outline::new(Rect::new(0, 0, w - 1, h), None, Some(v_bar), root);
        outline.state_mut().grow_mode.hi_x = true;
        outline.state_mut().grow_mode.hi_y = true;
        let outline_id = group.insert(Box::new(outline));
        TreePane { group, outline_id, v_bar, state, last_sel: -1 }
    }
```

> This restructures `TreePane` from `#[delegate(to = outline)]` to `#[delegate(to = group)]`. Update `handle_event` to fetch the outline via `self.group.child_mut(self.outline_id)` and downcast, OR keep the outline reachable. Update `select_row_for_test` accordingly. Keep the Task 3 `draw` fill (now keyed on `self.group.state().state.active`, filling `self.group` extent before `self.group.draw`).

- [ ] **Step 2: Toggle visibility on focus + overflow**

Add a `sync_scrollbar` mirroring Task 7, using the outline's node count vs visible rows for overflow, resizing the outline child width to reclaim/return the lane. Call it at the end of `handle_event`.

- [ ] **Step 3: Verify**

Run: `cargo test -j4 tree 2>&1 | tail -20` (fix the `select_row_for_test` seam if the restructure broke it).
Manual: the tree (deep DIT) shows a scrollbar only when focused and overflowing.

- [ ] **Step 4: Build, clippy, commit**

```bash
git add src/ui/panes/tree.rs
git commit -m "feat(ui): tree pane shows a scrollbar only while focused and overflowing"
```

---

### Task 9: Search-filter prompt

**Files:**
- Modify: `src/ui/panes/leaf.rs` (`new`)

**Interfaces:** none new.

The leaf search box (leaf.rs:35) is a bare full-width `InputLine`. Prefix it with a visible `Filter:` label so its purpose is obvious; shrink the input by the label width.

- [ ] **Step 1: Add the prompt label and shift the input**

In `LeafPane::new`, before constructing `search`, insert a label and offset the input:

```rust
        const PROMPT: &str = "Filter:";
        let px = PROMPT.chars().count() as i32 + 1; // label width + 1 space
        group.insert(Box::new(tv::StaticText::new(
            Rect::new(0, 0, px, 1),
            PROMPT.to_string(),
        )));
        let mut search = InputLine::with_limit(Rect::new(px, 0, w, 1), 256);
        search.state.grow_mode.hi_x = true;
        let search_id = group.insert(Box::new(search));
```

Add `StaticText` to the `use tvision_rs::{...}` line if not present.

- [ ] **Step 2: Manual verify**

Run the app: the second pane's top row reads `Filter:` followed by the input; typing filters as before.

- [ ] **Step 3: Build, clippy, commit**

```bash
git add src/ui/panes/leaf.rs
git commit -m "feat(ui): label the leaf search box with a Filter: prompt"
```

---

### Task 10: Extract the shared `DualList` widget

**Files:**
- Create: `src/ui/dual_list.rs`
- Modify: `src/ui/mod.rs` (`mod dual_list;`)
- Test: inline `#[cfg(test)]` in `dual_list.rs`

**Interfaces:**
- Produces:
  ```rust
  pub(crate) struct DualList { /* two ListBoxes + search + headers */ }
  impl DualList {
      pub(crate) fn new(dlg: &mut Dialog, area: Rect,
                        left_title: &str, right_title: &str, with_search: bool) -> DualList;
      pub(crate) fn set_available(&mut self, rows: Vec<DualRow>, ctx: &mut Context);
      pub(crate) fn set_selected(&mut self, rows: Vec<DualRow>, ctx: &mut Context);
      pub(crate) fn selected(&self) -> &[DualRow];
      pub(crate) fn search_text(&self) -> String;
      /// Returns true if the event was a move/flip the host should react to.
      pub(crate) fn handle_event(&mut self, ev: &mut Event, dlg: &mut Dialog, ctx: &mut Context) -> DualEvent;
  }
  pub(crate) struct DualRow { pub key: String, pub label: String, pub removable: bool }
  pub(crate) enum DualEvent { None, MovedIn(String), MovedOut(String), FlippedFocus, SearchChanged(String) }
  ```

`DualList` owns the geometry and move/flip logic that `membership.rs` (lines 115-447) implements today; the host (membership / oc_picker) owns the *data* (async candidates vs static OC list) and decides what `set_available` shows. Keep `DualList` domain-free.

- [ ] **Step 1: Write failing tests for the move logic**

In `src/ui/dual_list.rs` `#[cfg(test)]`:

```rust
#[test]
fn move_in_appends_to_selected_and_reports() {
    let mut dl = DualList::headless_for_test(); // ctor seam that builds without a Dialog
    dl.set_available_rows(vec![row("a"), row("b")]);
    dl.set_selected_rows(vec![]);
    let ev = dl.move_in_highlighted_for_test(0);
    assert!(matches!(ev, DualEvent::MovedIn(ref k) if k == "a"));
    assert_eq!(dl.selected().iter().map(|r| r.key.as_str()).collect::<Vec<_>>(), ["a"]);
}

#[test]
fn move_out_respects_removable_flag() {
    let mut dl = DualList::headless_for_test();
    dl.set_selected_rows(vec![DualRow { key: "top".into(), label: "top".into(), removable: false }]);
    let ev = dl.move_out_highlighted_for_test(0);
    assert!(matches!(ev, DualEvent::None)); // non-removable stays
    assert_eq!(dl.selected().len(), 1);
}
```

(`row(k)` = helper building a removable `DualRow`. Provide `headless_for_test`/`*_for_test` seams that exercise the pure list logic without a `Dialog`/`Context`, mirroring `ScrollGroup`'s test seams.)

- [ ] **Step 2: Implement `DualList`** by lifting the two-column layout and key handling from `membership.rs`:
  - Headers: `Label::new(Rect::new(2,1,38,2), left_title)` and `Rect::new(42,1,78,2), right_title`.
  - Left search `InputLine` at `Rect::new(2,2,38,3)` when `with_search`.
  - `avail` `ListBox` `Rect::new(2,4,38,18)`, `members`/selected `ListBox` `Rect::new(42,4,78,18)`.
  - `focus_members: bool`; Tab flips it (`DualEvent::FlippedFocus`); Up/Down/PageUp/PageDown route to the focused column.
  - `Insert`/`Right` → move highlighted available → selected (`MovedIn`); `Delete`/`Left` → remove highlighted selected if `removable` (`MovedOut`), else `None`.
  - Rebuild helpers mirror `rebuild_avail`/`rebuild_members` (mark available rows already in selected with `✓`; non-removable selected rows get a lock marker e.g. `🔒`/`*`).
  - `handle_event` returns `DualEvent` and reports `SearchChanged` when the search text changes.

- [ ] **Step 3: Run tests**

Run: `cargo test -j4 dual_list 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 4: Clippy, commit**

```bash
git add src/ui/dual_list.rs src/ui/mod.rs
git commit -m "feat(ui): extract reusable DualList two-column move widget"
```

---

### Task 11: Membership consumes `DualList`

**Files:**
- Modify: `src/ui/membership.rs`

**Interfaces:**
- Consumes: `DualList` (Task 10).

Replace `MembershipDialog`'s hand-rolled two columns with a `DualList`. The pump-driven candidate search feeds `dl.set_available(...)`; staged members come from `dl.selected()`. Keep the binding/scope/search-submit logic; delegate layout + move/flip to `DualList`.

- [ ] **Step 1:** In `MembershipDialog::new`, build the `DualList` instead of the headers/search/two ListBoxes (remove the now-duplicated geometry, lines 128-150). Store `dual: DualList`.

- [ ] **Step 2:** In `sync_results`/`rebuild_*`, map `available` candidates → `Vec<DualRow>` (all removable) and call `self.dual.set_available(rows, ctx)`. Seed members → `set_selected`.

- [ ] **Step 3:** In `handle_event`, after the seed/REFRESH handling, route the event through `self.dual.handle_event(ev, &mut self.dlg, ctx)`; on `MovedIn`/`MovedOut` update any host bookkeeping, on `SearchChanged(s)` call `self.submit_search(&s)`. Drop the now-duplicated Tab/Insert/Delete/nav block (lines 405-440).

- [ ] **Step 4:** On OK, read staged members from `self.dual.selected()` and write back as before.

- [ ] **Step 5: Verify**

Run: `cargo test -j4 membership 2>&1 | tail -20`
Manual: open a `memberOf`/membership field, search, move candidates in/out, Tab flips columns, OK persists. Behavior matches pre-refactor.

- [ ] **Step 6: Clippy, commit**

```bash
git add src/ui/membership.rs
git commit -m "refactor(ui): membership dialog uses the shared DualList"
```

---

### Task 12: Object-types as a `DualList`

**Files:**
- Modify: `src/ui/oc_picker.rs`

**Interfaces:**
- Consumes: `DualList` (Task 10).

Replace the single ticked list with a `DualList`: ticked object classes on the **left? no — selected/right**. Convention: **selected (active) classes on the LEFT, available on the RIGHT** per the user's request ("active items on the left, inactive on the right"). Adjust `DualList` titles/orientation so the *selected* column is left. Structural/required classes are non-removable (`removable: false`).

> **Orientation note:** Task 10's `DualList` puts available-left / selected-right (membership convention). For object-types the user wants selected-left. Add a `selected_on_left: bool` flag to `DualList::new` (default false) and have oc_picker pass `true`; the move semantics stay "move toward selected / away from selected" regardless of side.

- [ ] **Step 1:** In `ObjectClassPicker::new`, build a `DualList` (`selected_on_left = true`, `with_search = true`, titles "Active" / "Available") instead of the single `ListBox` (remove lines 67-71 list/search geometry; keep search box ownership via DualList).

- [ ] **Step 2:** Map state: `ticked` (active OC names) → selected rows; `candidates \ ticked` → available rows. Mark structural/required classes `removable: false`. (Determine structural/required from the schema the picker already has access to — reuse whatever `update_staged`/seed logic identifies them; if the current picker has no such notion, treat the entry's STRUCTURAL class and any MUST-providing classes as non-removable. Verify against the schema model before finalizing.)

- [ ] **Step 3:** Replace the Space-toggle handler (lines 215-247) with `self.dual.handle_event(...)`: `MovedIn(name)` ticks, `MovedOut(name)` unticks (rejected for non-removable), `SearchChanged` refilters available. On OK, the ticked set = `dual.selected()` keys.

- [ ] **Step 4: Verify**

Run: `cargo test -j4 oc_picker 2>&1 | tail -20`
Manual: open the object-classes editor → two columns, active on the left; move a class right to remove it, left to add; structural class cannot be removed (feedback/no-op); OK applies, downstream objectClass resync still works.

- [ ] **Step 5: Clippy, commit**

```bash
git add src/ui/oc_picker.rs src/ui/dual_list.rs
git commit -m "feat(ui): object-classes editor uses a dual-list (active left / available right)"
```

---

### Task 13: Multivalue add/remove buttons

**Files:**
- Modify: `src/ui/multivalue.rs` (`MultiValueDialog::new`, `handle_event`)

**Interfaces:** none new.

Make add/remove discoverable with visible `[+ Add]` / `[- Del]` buttons, alongside the existing keys.

- [ ] **Step 1: Add the buttons in `new()`**

The dialog is `Rect::new(0,0,60,20)`, list rows 1..15, edit line row 16. Add a button row above OK/Cancel, or two extra buttons. Insert before the `button_row(OK/Cancel)` call:

```rust
        // Add/Del buttons so the affordance is visible (keys Ins/Del still work).
        dlg.insert_child(Box::new(tv::Button::new(
            Rect::new(2, 17, 13, 19),
            "~+~ Add",
            Command::from(CMD_MV_ADD),
            ButtonFlags::new(),
        )));
        dlg.insert_child(Box::new(tv::Button::new(
            Rect::new(14, 17, 26, 19),
            "~-~ Del",
            Command::from(CMD_MV_DEL),
            ButtonFlags::new(),
        )));
```

Define two private command constants near the top of the file (pick free command ids — check existing `Command` usage to avoid collisions):

```rust
const CMD_MV_ADD: u16 = 1801;
const CMD_MV_DEL: u16 = 1802;
```

> Confirm `tv::Button::new` signature and `Command::from`/command-id construction against tvision-rs (`widgets/button`). The captions use `~x~` hotkey markup as the existing buttons do.

- [ ] **Step 2: Handle the button commands**

In `handle_event`, before the key `match`, intercept the broadcasts:

```rust
        if let Event::Broadcast { command, .. } = ev {
            if *command == Command::from(CMD_MV_ADD) {
                self.add_row(ctx);
                ev.clear();
                return;
            }
            if *command == Command::from(CMD_MV_DEL) {
                self.delete_row(ctx);
                ev.clear();
                return;
            }
        }
```

- [ ] **Step 3: Verify**

Run: `cargo test -j4 multivalue 2>&1 | tail -20`
Manual: open a multi-value field → `[+ Add]` / `[- Del]` buttons visible; clicking adds/removes rows; `Ins`/`Del` still work.

- [ ] **Step 4: Clippy, commit**

```bash
git add src/ui/multivalue.rs
git commit -m "feat(ui): multivalue editor gains visible Add/Del buttons"
```

---

### Task 14: Widen the guard dialog buttons

**Files:**
- Modify: `src/ui/dialog/guard.rs`

**Interfaces:** none.

"Discard" touches the dialog edge. Widen the dialog and/or pad captions so the right-aligned button row has breathing room.

- [ ] **Step 1: Widen and pad**

In `guard.rs`, change the dialog width from 56 to 64 and pad the captions with surrounding spaces so the right-aligned row clears the frame:

```rust
    let mut dlg = Dialog::new(Rect::new(0, 0, 64, 9), Some("Unsaved changes".to_string()));
```

and the button captions:

```rust
            ("~S~ave", Command::YES, ButtonFlags { default: true, ..ButtonFlags::new() }),
            (" ~D~iscard ", Command::NO, ButtonFlags::new()),
            (" S~t~ay ", Command::CANCEL, ButtonFlags::new()),
```

Also widen the `StaticText` bound to match (`Rect::new(2, 2, 62, 4)`).

- [ ] **Step 2: Audit confirm.rs** — its dialog is already 70 wide; verify its right-aligned OK/Cancel row doesn't touch the edge after the theme change. Pad if needed.

- [ ] **Step 3: Manual verify**

Trigger the guard (edit an entry, navigate away): the three buttons are evenly spaced and "Discard" no longer touches the frame.

- [ ] **Step 4: Build, clippy, commit**

```bash
git add src/ui/dialog/guard.rs
git commit -m "fix(ui): widen the unsaved-changes dialog so buttons don't touch the edge"
```

---

### Task 15: Docs & changelog

**Files:**
- Modify: `CHANGES.md`, `docs/src/configuration/widgets.md` (object-types dual-list, multivalue buttons), any theming note.

- [ ] **Step 1: `CHANGES.md`** — under the current unreleased section, add user-visible entries: new light theme; uniform panels + active-panel highlight; Tab switches panes only; click-to-focus form labels; focus-gated scrollbars (panes + dialogs); object-classes dual-list; multivalue Add/Del buttons; `Filter:` prompt; widened guard dialog.

- [ ] **Step 2: `docs/src/`** — update `widgets.md` for the object-classes dual-list interaction (active left / available right, structural classes non-removable) and the multivalue Add/Del buttons. Add a short note about the light theme if a theming page exists; do not introduce a config section (theme is code-level).

- [ ] **Step 3: Build docs**

Run: `make docs 2>&1 | tail -20`
Expected: builds clean.

- [ ] **Step 4: Commit**

```bash
git add CHANGES.md docs/
git commit -m "docs: light theme + TUI interaction polish"
```

---

### Final verification

- [ ] **Run the full gate**

Run: `make check 2>&1 | tail -30`
Expected: fmt clean, `clippy -D warnings` clean, all tests pass.

- [ ] **Full manual pass** against the demo server, walking the rough-edge inventory in the spec (all 14 items) and confirming each is fixed.

---

## Self-review notes

- **Spec coverage:** all 14 inventory items map to tasks — #1→1, #2→5, #3→2/3/4, #4→1, #5→1, #6→6, #7→1, #8→7/8, #9→10-13 (dialog lists already carry `ListBox` bars; verify during 11-13), #10→14, #11→10/12, #12→9, #13→1, #14→13. ✔
- **Risk hotspots:** Tasks 7-8 (focus-gated scrollbar + lane reclaim) and 10-12 (DualList extraction) are the highest-uncertainty; they are visual/framework-heavy and rely on `ListBox`/`Outline`/`ScrollBar`/`Button` signatures that the implementer must confirm against tvision-rs 0.3 before finalizing each code block. Flagged inline.
- **Verification style:** color/layout changes are verified by build + clippy + manual demo-server inspection (they aren't unit-testable); pure logic (theme role contract, DualList move semantics, multivalue rows) gets unit tests.
- **Accessor caveats:** `Theme::style()`, `Event::MouseDown` shape, `ScrollGroup` child-screen-bounds helper, `Button::new`/`Command` construction, and `ListBox` overflow accessors are explicitly called out as "confirm against the crate" rather than assumed.
