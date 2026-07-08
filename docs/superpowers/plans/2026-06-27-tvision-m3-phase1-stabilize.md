# M3 Phase 1 — Stabilize the Base Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the tvision panes fill their cell and the form pane scroll (via a new reusable `ScrollGroup`), and close the two outstanding dirty-form guard edges (#2 cancelled-confirm, #3 branch-change).

**Architecture:** Build a domain-free `ScrollGroup` widget (a `Group` that holds child views at logical positions, repositions them by `-top` on scroll — the framework clips offscreen children automatically — and drives a linked `ScrollBar` through the `ScrollSync` broker). Rebuild `FormPane` on top of it with one persistent `Label`+`InputLine` per field (no 32-row cap). Extend the controller-owned nav model (already used for leaf selection) to the tree so a dirty-form branch change guards and snaps back.

**Tech Stack:** Rust, tvision-rs 0.3.0 (`Group`, `InputLine`, `ScrollBar`, `Outline`, `Context`/`DrawCtx`, the `ScrollSync` broker), the edaptor `src/tui/**` facade.

## Global Constraints

- **Facade boundary:** only `src/tui/**` and `src/bin/edaptor-tv.rs` may `use tvision_rs`. No `src/ui/**` (ratatui) or domain-layer changes.
- **Cap build/test parallelism at 4 cores.** Target dir `/home/oetiker/scratch/cargo-target` (export `CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target`).
- **Borrow discipline:** never hold a `RefCell`/`UiState` borrow across `ctx.*` / `child_mut` / `change_bounds` / `set_value` / `focus_child` / `remove`. Collect into locals → drop the borrow → call.
- **Strict TDD**, atomic commits, crate compiles after every commit, `cargo fmt` before each commit, `cargo clippy --all-targets -- -D warnings` clean.
- **Headless view tests** use `Context::new(&mut out, &mut timers, 0, &mut deferred)` with `tv::timer::TimerQueue::new()` and `Vec<tv::Deferred>`; tvision events are `Event::KeyDown` (not `Key`).
- **Commit trailer:** `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>` (use `git commit -F <file>` for messages with backticks).
- Run after each task: `CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target cargo test -j4 <scope>` then `cargo fmt` + `cargo clippy -j4 --all-targets -- -D warnings`.
- Facade guards must print nothing:
  `! grep -rl "use tvision_rs" src | grep -vE "^src/(tui/|bin/edaptor-tv.rs)"`

---

## File Structure

- **Create** `src/tui/scroll_group.rs` — the `ScrollGroup` widget (domain-free, extraction candidate).
- **Modify** `src/tui/mod.rs` — declare `mod scroll_group;`.
- **Modify** `src/tui/panes/form.rs` — rebuild `FormPane` on `ScrollGroup`; one cell per field; per-entry rebuild; scroll-to-focused; drop `FORM_ROWS`.
- **Modify** `src/tui/panes/leaf.rs` — `on_bounds_changed` relayout (kills the `▒` strip).
- **Modify** `src/tui/panes/tree.rs` — pure selector: record `requested_branch`, honour `set_tree_row`; no inline load/broadcast.
- **Modify** `src/tui/state.rs` — `requested_branch`, `set_tree_row`, `current_branch_row()`, `reconcile_branch()`, a `GuardTarget` enum to disambiguate leaf vs branch; `do_save`-outcome support.
- **Modify** `src/tui/pump.rs` — call `reconcile_branch()` each tick.
- **Modify** `src/tui/app.rs` — `do_save` returns a `SaveOutcome`; `GUARD_NAV` snap-back on cancelled confirm (#2) and branch-target handling (#3).
- **Modify** `CHANGES.md` — user-visible scroll/fill entry.

Task order is foundation-first: `ScrollGroup` (1–4) → `FormPane` rebuild (5–6) → `LeafPane` fill (7) → guard #2 (8) → guard #3 (9–11) → changelog + live acceptance (12).

---

### Task 1: `ScrollGroup` — struct, construction, reposition math

**Files:**
- Create: `src/tui/scroll_group.rs`
- Modify: `src/tui/mod.rs` (add `mod scroll_group;`)
- Test: in `src/tui/scroll_group.rs` `#[cfg(test)]`

**Interfaces:**
- Produces:
  - `pub(crate) struct ScrollGroup` wrapping a `tv::Group`.
  - `pub(crate) fn new(bounds: tv::Rect) -> ScrollGroup`
  - `pub(crate) fn inner_width(&self) -> i32` — content width (excludes the 1-col bar lane).
  - `pub(crate) fn add_content(&mut self, view: Box<dyn tv::View>, logical: tv::Rect) -> tv::ViewId`
  - `pub(crate) fn clear_content(&mut self, ctx: &mut tv::Context)`
  - `pub(crate) fn child_mut(&mut self, id: tv::ViewId) -> Option<&mut dyn tv::View>`
  - `pub(crate) fn content_height(&self) -> i32` / `fn max_top(&self) -> i32`
  - internal `fn reposition(&mut self)` (sets each content child `bounds.y = logical.y - top`).

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tvision_rs::{self as tv, InputLine, Rect};

    fn cell(y: i32, w: i32) -> Box<dyn tv::View> {
        Box::new(InputLine::with_limit(Rect::new(0, y, w, y + 1), 64))
    }

    #[test]
    fn reposition_shifts_content_by_top() {
        let mut sg = ScrollGroup::new(Rect::new(0, 0, 20, 5)); // viewport height 5
        let w = sg.inner_width();
        // 8 logical rows (0..8), taller than the 5-row viewport.
        let mut ids = Vec::new();
        for y in 0..8 {
            ids.push(sg.add_content(cell(y, w), Rect::new(0, y, w, y + 1)));
        }
        // content height 8, viewport 5 → max_top 3
        assert_eq!(sg.content_height(), 8);
        assert_eq!(sg.max_top(), 3);
        // at top=0 the first child sits on its logical row
        assert_eq!(sg.child_mut(ids[0]).unwrap().state().get_bounds().a.y, 0);
        // scroll math is applied by reposition() through set_top (Task 2 wires ctx);
        // here exercise the pure helper:
        sg.set_top_for_test(2);
        assert_eq!(sg.child_mut(ids[0]).unwrap().state().get_bounds().a.y, -2);
        assert_eq!(sg.child_mut(ids[5]).unwrap().state().get_bounds().a.y, 3);
    }

    #[test]
    fn inner_width_reserves_bar_lane() {
        let sg = ScrollGroup::new(Rect::new(0, 0, 20, 5));
        assert_eq!(sg.inner_width(), 19); // 20 - 1 bar column
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target cargo test -j4 --lib scroll_group`
Expected: FAIL — `ScrollGroup` not found.

- [ ] **Step 3: Write minimal implementation**

In `src/tui/mod.rs` add (next to the other `mod` lines): `pub(crate) mod scroll_group;`

Create `src/tui/scroll_group.rs`:

```rust
//! A reusable vertical scroll container for child views.
//!
//! tvision-rs has no scroll-container for child widgets (Group has no child
//! offset; Scroller is for self-drawn content). `ScrollGroup` fills the gap: it
//! holds child views at stable *logical* positions and, on scroll, repositions
//! each child's bounds by `-top` — the framework clips children that fall outside
//! the group automatically (`DrawCtx::sub` intersects each child's clip with the
//! group region). A linked vertical `ScrollBar` lives in the right column and is
//! driven through the `ScrollSync` broker. Domain-free: candidate for upstreaming
//! to tvision-rs.

use tvision_rs::{self as tv, Context, Rect, ScrollBar, View, ViewId};

pub(crate) struct ScrollGroup {
    group: tv::Group,
    v_bar: ViewId,
    /// (child id, logical rect) for repositionable content (excludes the bar).
    content: Vec<(ViewId, Rect)>,
    top: i32,
    inner_w: i32,
    viewport_h: i32,
}

impl ScrollGroup {
    pub(crate) fn new(bounds: Rect) -> Self {
        let w = bounds.b.x - bounds.a.x;
        let h = bounds.b.y - bounds.a.y;
        let mut group = tv::Group::new(bounds);
        // Vertical bar in the right column (width 1 ⇒ vertical). Local coords.
        let v_bar = group.insert(Box::new(ScrollBar::new(Rect::new(w - 1, 0, w, h))));
        ScrollGroup {
            group,
            v_bar,
            content: Vec::new(),
            top: 0,
            inner_w: (w - 1).max(0),
            viewport_h: h.max(0),
        }
    }

    pub(crate) fn inner_width(&self) -> i32 {
        self.inner_w
    }

    pub(crate) fn add_content(&mut self, view: Box<dyn View>, logical: Rect) -> ViewId {
        let id = self.group.insert(view);
        self.content.push((id, logical));
        self.reposition_one(id, logical);
        id
    }

    pub(crate) fn child_mut(&mut self, id: ViewId) -> Option<&mut dyn View> {
        self.group.child_mut(id)
    }

    pub(crate) fn content_height(&self) -> i32 {
        self.content.iter().map(|(_, r)| r.b.y).max().unwrap_or(0)
    }

    pub(crate) fn max_top(&self) -> i32 {
        (self.content_height() - self.viewport_h).max(0)
    }

    fn reposition_one(&mut self, id: ViewId, logical: Rect) {
        let b = Rect::new(logical.a.x, logical.a.y - self.top, logical.b.x, logical.b.y - self.top);
        if let Some(v) = self.group.child_mut(id) {
            v.change_bounds(b);
        }
    }

    fn reposition(&mut self) {
        let items: Vec<(ViewId, Rect)> = self.content.clone();
        for (id, logical) in items {
            self.reposition_one(id, logical);
        }
    }

    /// Test seam: set `top` and reposition without a Context (no bar republish).
    #[cfg(test)]
    pub(crate) fn set_top_for_test(&mut self, top: i32) {
        self.top = top.clamp(0, self.max_top());
        self.reposition();
    }

    pub(crate) fn clear_content(&mut self, ctx: &mut Context) {
        let ids: Vec<ViewId> = self.content.iter().map(|(id, _)| *id).collect();
        for id in ids {
            self.group.remove(id, ctx);
        }
        self.content.clear();
        self.top = 0;
    }
}
```

(`v_bar` is read in Task 3; allow it now or add `let _ = self.v_bar;` is unnecessary since it is a field, not an unused local.)

- [ ] **Step 4: Run test to verify it passes**

Run: `CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target cargo test -j4 --lib scroll_group`
Expected: PASS (2 tests).

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt
CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target cargo clippy -j4 --all-targets -- -D warnings
git add src/tui/scroll_group.rs src/tui/mod.rs
git commit -m "feat(tui): ScrollGroup skeleton — content reposition math"
```

---

### Task 2: `ScrollGroup` — `View` impl, scroll, and offscreen clipping (headless draw proof)

**Files:**
- Modify: `src/tui/scroll_group.rs`
- Test: same file

**Interfaces:**
- Produces:
  - `impl View for ScrollGroup` (delegates handle_event/draw to the inner group; overrides `as_any_mut`, `on_bounds_changed`).
  - `pub(crate) fn scroll_to(&mut self, top: i32, ctx: &mut Context)` — clamps `top`, repositions, republishes the bar (bar republish is a no-op stub until Task 3).
  - `pub(crate) fn current(&self) -> Option<ViewId>` / `pub(crate) fn focus_child(&mut self, id: ViewId, ctx: &mut Context)` (forward to group).

- [ ] **Step 1: Write the failing test** (headless draw, proving offscreen children clip — mirrors the crate's own `fill_clips_to_clip_rect`)

```rust
#[test]
fn scrolled_child_clips_to_viewport() {
    use tvision_rs::{Buffer, DrawCtx, FieldValue, Point, StaticText, Theme, View};
    // viewport 0..4 rows, content rows 0..8.
    let mut sg = ScrollGroup::new(Rect::new(0, 0, 10, 4));
    let w = sg.inner_width();
    for y in 0..8 {
        // StaticText draws its text at the top-left of its bounds.
        let t = StaticText::new(Rect::new(0, y, w, y + 1), format!("R{y}"));
        sg.add_content(Box::new(t), Rect::new(0, y, w, y + 1));
    }
    // scroll so logical row 2 is at screen row 0; rows 0,1 go to negative y (clip).
    sg.set_top_for_test(2);

    let mut buf = Buffer::new(10, 4);
    let theme = Theme::classic_blue();
    {
        let mut ctx = DrawCtx::new(&mut buf, &theme, Rect::new(0, 0, 10, 4), Point::new(0, 0));
        // Drawing the group draws each child through ctx.sub(child_bounds), which
        // clips to the group region — so the negative-y rows must not appear.
        <ScrollGroup as View>::draw(&mut sg, &mut ctx);
    }
    // Screen row 0 shows logical row 2 ("R2"); rows above it (R0,R1) are clipped out.
    assert_eq!(buf.get(0, 0).symbol(), "R".chars().next().unwrap().to_string());
    // The glyph at (1,0) is '2' (from "R2"), proving row 2 — not row 0 — is on top.
    assert_eq!(buf.get(1, 0).symbol(), "2");
    let _ = FieldValue::Int(0);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target cargo test -j4 --lib scroll_group::tests::scrolled_child_clips_to_viewport`
Expected: FAIL — no `View` impl / `draw` for `ScrollGroup`.

- [ ] **Step 3: Write minimal implementation**

Add to `src/tui/scroll_group.rs` (after the inherent impl). Use the `delegate` macro so the inner group handles drawing/routing, and override the hooks:

```rust
use tvision_rs::{delegate, DrawCtx, Event};

impl ScrollGroup {
    pub(crate) fn current(&self) -> Option<ViewId> {
        self.group.current()
    }

    pub(crate) fn focus_child(&mut self, id: ViewId, ctx: &mut Context) {
        self.group.focus_child(id, ctx);
    }

    pub(crate) fn scroll_to(&mut self, top: i32, ctx: &mut Context) {
        let clamped = top.clamp(0, self.max_top());
        if clamped != self.top {
            self.top = clamped;
            self.reposition();
        }
        self.publish_bar(ctx);
    }

    /// Republish the bar params (stub until Task 3 fills it in).
    fn publish_bar(&mut self, _ctx: &mut Context) {}
}

#[delegate(to = group)]
impl View for ScrollGroup {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    fn draw(&mut self, ctx: &mut DrawCtx) {
        self.group.draw(ctx);
    }

    fn on_bounds_changed(&mut self, ctx: &mut Context) {
        // Recompute the viewport + bar lane from the new size, re-fit the bar, and
        // re-clamp/reposition content so it fills (the §1 fill behaviour).
        let ext = self.group.state().get_extent();
        let w = ext.b.x - ext.a.x;
        let h = ext.b.y - ext.a.y;
        self.inner_w = (w - 1).max(0);
        self.viewport_h = h.max(0);
        if let Some(b) = self.group.child_mut(self.v_bar) {
            b.change_bounds(Rect::new(w - 1, 0, w, h));
        }
        self.scroll_to(self.top, ctx); // re-clamp + reposition + republish
    }
}
```

(`Event` import is used by the delegate-expanded forwarders; keep it.)

- [ ] **Step 4: Run test to verify it passes**

Run: `CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target cargo test -j4 --lib scroll_group`
Expected: PASS (all scroll_group tests).

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt
CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target cargo clippy -j4 --all-targets -- -D warnings
git add src/tui/scroll_group.rs
git commit -m "feat(tui): ScrollGroup View impl + offscreen clipping proof"
```

---

### Task 3: `ScrollGroup` — linked `ScrollBar` via the `ScrollSync` broker

**Files:**
- Modify: `src/tui/scroll_group.rs`
- Test: same file

**Interfaces:**
- Produces:
  - `publish_bar` filled in (`ctx.request_scroll_bar_params`), bar hidden when content fits.
  - `handle_event` override: on `SCROLL_BAR_CHANGED { source == v_bar }` → `ctx.request_scroll_sync(self_id, None, Some(v_bar))`.
  - `apply_scroll_sync(&mut self, _h, v, ctx)` override → `scroll_to(v, ctx)`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn publish_bar_sets_params_and_hides_when_fits() {
    use tvision_rs::{Deferred, FieldValue};
    let mut sg = ScrollGroup::new(Rect::new(0, 0, 10, 5));
    let w = sg.inner_width();
    for y in 0..8 {
        sg.add_content(
            Box::new(tv::InputLine::with_limit(Rect::new(0, y, w, y + 1), 64)),
            Rect::new(0, y, w, y + 1),
        );
    }
    let mut out = std::collections::VecDeque::new();
    let mut timers = tv::timer::TimerQueue::new();
    let mut deferred: Vec<Deferred> = Vec::new();
    let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);
    sg.scroll_to(2, &mut ctx);
    // A ScrollBarSetParams deferred with value=2, max=max_top()=3 was requested.
    let found = deferred.iter().any(|d| matches!(
        d,
        Deferred::ScrollBarSetParams { value: Some(2), max: Some(3), .. }
    ));
    assert!(found, "publish_bar must request value=2 max=3");
    let _ = FieldValue::Int(0);
}

#[test]
fn apply_scroll_sync_sets_top() {
    use tvision_rs::Deferred;
    let mut sg = ScrollGroup::new(Rect::new(0, 0, 10, 5));
    let w = sg.inner_width();
    for y in 0..8 {
        sg.add_content(
            Box::new(tv::InputLine::with_limit(Rect::new(0, y, w, y + 1), 64)),
            Rect::new(0, y, w, y + 1),
        );
    }
    let mut out = std::collections::VecDeque::new();
    let mut timers = tv::timer::TimerQueue::new();
    let mut deferred: Vec<Deferred> = Vec::new();
    let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);
    <ScrollGroup as View>::apply_scroll_sync(&mut sg, None, Some(2), &mut ctx);
    // child for logical row 5 is now at screen row 3 (5 - top=2).
    let id = sg.content_id_for_test(5);
    assert_eq!(sg.child_mut(id).unwrap().state().get_bounds().a.y, 3);
}
```

Add a small test seam near `set_top_for_test`:

```rust
#[cfg(test)]
pub(crate) fn content_id_for_test(&self, idx: usize) -> ViewId {
    self.content[idx].0
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target cargo test -j4 --lib scroll_group`
Expected: FAIL — `publish_bar` is a stub; `apply_scroll_sync` not overridden.

- [ ] **Step 3: Write minimal implementation**

Replace the `publish_bar` stub and add the event/sync overrides. `self_id` comes from `self.group.state().id()`.

```rust
fn publish_bar(&mut self, ctx: &mut Context) {
    let max = self.max_top();
    // Bar visible only when content overflows the viewport.
    if let Some(b) = self.group.child_mut(self.v_bar) {
        b.state_mut().state.visible = max > 0;
    }
    ctx.request_scroll_bar_params(
        self.v_bar,
        Some(self.top),
        Some(0),
        Some(max),
        Some(self.viewport_h.max(1)),
        Some(1),
    );
}
```

Add to the `#[delegate(to = group)] impl View` block:

```rust
fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
    // React to OUR bar's value change by asking the pump to broker the read.
    if let Event::Broadcast { command, source } = ev {
        if *command == tv::Command::SCROLL_BAR_CHANGED && *source == Some(self.v_bar) {
            if let Some(id) = self.group.state().id() {
                ctx.request_scroll_sync(id, None, Some(self.v_bar));
            }
        }
    }
    self.group.handle_event(ev, ctx);
}

fn apply_scroll_sync(&mut self, _h: Option<i32>, v: Option<i32>, ctx: &mut Context) {
    if let Some(v) = v {
        self.scroll_to(v, ctx);
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target cargo test -j4 --lib scroll_group`
Expected: PASS.

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt
CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target cargo clippy -j4 --all-targets -- -D warnings
git add src/tui/scroll_group.rs
git commit -m "feat(tui): ScrollGroup linked ScrollBar via ScrollSync broker"
```

---

### Task 4: `ScrollGroup` — scroll-to-focused (`ensure_visible`)

**Files:**
- Modify: `src/tui/scroll_group.rs`
- Test: same file

**Interfaces:**
- Produces:
  - `pub(crate) fn ensure_visible(&mut self, logical: Rect, ctx: &mut Context)` — scrolls minimally so `logical` is within `[top, top+viewport_h)`.
  - `pub(crate) fn logical_of(&self, id: ViewId) -> Option<Rect>` — the stored logical rect for a content child.
  - `handle_event` also calls `ensure_visible` for the focused content child after delegating (so Tab/arrows into an offscreen field scroll it on screen).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn ensure_visible_scrolls_offscreen_child_into_view() {
    let mut sg = ScrollGroup::new(Rect::new(0, 0, 10, 4)); // viewport rows 0..4
    let w = sg.inner_width();
    for y in 0..8 {
        sg.add_content(
            Box::new(tv::InputLine::with_limit(Rect::new(0, y, w, y + 1), 64)),
            Rect::new(0, y, w, y + 1),
        );
    }
    let mut out = std::collections::VecDeque::new();
    let mut timers = tv::timer::TimerQueue::new();
    let mut deferred = Vec::new();
    let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);
    // Row 6 is below the viewport (top=0 shows 0..4). ensure_visible scrolls so
    // its bottom (7) is the viewport bottom → top = 7 - 4 = 3.
    sg.ensure_visible(Rect::new(0, 6, w, 7), &mut ctx);
    let id = sg.content_id_for_test(6);
    let y = sg.child_mut(id).unwrap().state().get_bounds().a.y;
    assert!((0..4).contains(&y), "row 6 must be inside the viewport, got y={y}");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target cargo test -j4 --lib scroll_group::tests::ensure_visible_scrolls_offscreen_child_into_view`
Expected: FAIL — `ensure_visible` not found.

- [ ] **Step 3: Write minimal implementation**

```rust
impl ScrollGroup {
    pub(crate) fn logical_of(&self, id: ViewId) -> Option<Rect> {
        self.content.iter().find(|(i, _)| *i == id).map(|(_, r)| *r)
    }

    pub(crate) fn ensure_visible(&mut self, logical: Rect, ctx: &mut Context) {
        if logical.a.y < self.top {
            self.scroll_to(logical.a.y, ctx);
        } else if logical.b.y > self.top + self.viewport_h {
            self.scroll_to(logical.b.y - self.viewport_h, ctx);
        }
    }
}
```

Extend `handle_event` (in the `impl View` block) to ensure the focused child is visible after routing:

```rust
fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
    if let Event::Broadcast { command, source } = ev {
        if *command == tv::Command::SCROLL_BAR_CHANGED && *source == Some(self.v_bar) {
            if let Some(id) = self.group.state().id() {
                ctx.request_scroll_sync(id, None, Some(self.v_bar));
            }
        }
    }
    self.group.handle_event(ev, ctx);
    // Scroll-to-focused: keep the focused content child within the viewport.
    if let Some(cur) = self.group.current() {
        if let Some(logical) = self.logical_of(cur) {
            self.ensure_visible(logical, ctx);
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target cargo test -j4 --lib scroll_group`
Expected: PASS.

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt
CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target cargo clippy -j4 --all-targets -- -D warnings
git add src/tui/scroll_group.rs
git commit -m "feat(tui): ScrollGroup ensure_visible (scroll-to-focused)"
```

---

### Task 5: `FormPane` — rebuild on `ScrollGroup` (one cell per field, per-entry rebuild)

**Files:**
- Modify: `src/tui/panes/form.rs`
- Test: `src/tui/panes/form.rs` `#[cfg(test)]` (adapt the existing tests)

**Interfaces:**
- Consumes: `ScrollGroup` (Task 1–4).
- Produces: `FormPane` whose outer `Group` holds a pinned header cell (row 0) + a `ScrollGroup` (rows 1..h). Cells are rebuilt from `EditForm.fields` whenever the shown entry changes; **no `FORM_ROWS` cap**.

**Notes for the implementer:** This replaces the fixed 32-cell pool. Keep `ro_cell`, `header_text`, `inline_editable`/`present_field` usage, the Up/Down focus nav, and `sync_into_form` semantics. The big change: cells are created per field into the `ScrollGroup`, and `render()` rebuilds them when `edit_form.dn` (or field count) differs from what is currently built.

- [ ] **Step 1: Write the failing test**

Replace `more_fields_than_rows_truncates_without_panic` with a test that a large field set is fully built (no cap), and keep the others working against the new structure:

```rust
#[test]
fn builds_a_cell_per_field_no_row_cap() {
    use crate::ldap::worker::RawSubschema;
    let schema = SchemaModel::from_raw(&RawSubschema::default());
    let structure = Structure::build("dc=x", vec![]);
    let mut st = UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
    let fields: Vec<EditField> = (0..40).map(|i| ef(&format!("attr{i}"), "v", true)).collect();
    st.edit_form = Some(EditForm {
        dn: "cn=a,dc=x".into(),
        mode: FormMode::Edit,
        object_classes: vec![],
        fields,
    });
    st.form_needs_render = true;
    let shared: Shared = Rc::new(RefCell::new(st));
    let mut pane = FormPane::new(Rect::new(0, 0, 80, 12), shared.clone()); // small viewport
    let mut out = VecDeque::new();
    let mut timers = tv::timer::TimerQueue::new();
    let mut deferred = Vec::new();
    let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
    let mut ev = Event::Broadcast { command: REFRESH, source: None };
    pane.handle_event(&mut ev, &mut ctx); // must not panic; builds 40 rows
    assert_eq!(pane.field_cell_count(), 40, "one value cell per field, uncapped");
}
```

Add the test seam to `FormPane`:

```rust
#[cfg(test)]
pub(crate) fn field_cell_count(&self) -> usize {
    self.value_ids.len()
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target cargo test -j4 --lib panes::form`
Expected: FAIL (still capped at FORM_ROWS / no `field_cell_count`).

- [ ] **Step 3: Write the implementation**

Rewrite the `FormPane` struct and its construction/render to use `ScrollGroup`. Key shape (full module rewrite of the non-test code):

```rust
use crate::tui::scroll_group::ScrollGroup;

const LABEL_W: i32 = 22;

pub(crate) struct FormPane {
    group: Group,            // outer: header (row 0) + scroll (rows 1..h)
    header_id: tv::ViewId,
    scroll_id: tv::ViewId,
    value_ids: Vec<tv::ViewId>,
    label_ids: Vec<tv::ViewId>,
    built_dn: Option<String>,
    state: Shared,
}

impl FormPane {
    pub(crate) fn new(bounds: Rect, state: Shared) -> Self {
        let mut group = Group::new(bounds);
        group.state_mut().options.first_click = true;
        let w = bounds.b.x - bounds.a.x;
        let h = bounds.b.y - bounds.a.y;
        let header_id = group.insert(Box::new(ro_cell(Rect::new(0, 0, w, 1))));
        let scroll_id = group.insert(Box::new(ScrollGroup::new(Rect::new(0, 1, w, h))));
        FormPane { group, header_id, scroll_id, value_ids: Vec::new(), label_ids: Vec::new(), built_dn: None, state }
    }

    fn scroll_mut(&mut self) -> Option<&mut ScrollGroup> {
        self.group
            .child_mut(self.scroll_id)
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<ScrollGroup>())
    }

    /// Rebuild one label+value cell per field into the ScrollGroup. Called when the
    /// shown entry changes (different `dn`).
    fn rebuild_cells(&mut self, ctx: &mut Context) {
        // Field labels/widths from state (drop borrow before mutating views).
        let fields: Vec<(String, bool)> = {
            let st = self.state.borrow();
            match st.edit_form.as_ref() {
                None => Vec::new(),
                Some(form) => form
                    .fields
                    .iter()
                    .map(|f| {
                        let marker = if f.must { "*" } else { "" };
                        (format!("{}{}", f.label, marker), inline_editable(f))
                    })
                    .collect(),
            }
        };
        self.value_ids.clear();
        self.label_ids.clear();
        let Some(sg) = self.scroll_mut() else { return };
        sg.clear_content(ctx);
        let w = sg.inner_width();
        for (row, (_label, editable)) in fields.iter().enumerate() {
            let y = row as i32;
            let lid = sg.add_content(Box::new(ro_cell(Rect::new(0, y, LABEL_W, y + 1))), Rect::new(0, y, LABEL_W, y + 1));
            let mut il = InputLine::with_limit(Rect::new(LABEL_W, y, w, y + 1), 1024);
            il.state.state.disabled = !editable;
            let vid = sg.add_content(Box::new(il), Rect::new(LABEL_W, y, w, y + 1));
            self.label_ids.push(lid);
            self.value_ids.push(vid);
        }
    }

    /// Repaint header + cell text from `edit_form`; rebuild cells first if the
    /// shown entry changed.
    fn render(&mut self, ctx: &mut Context) {
        let cur_dn = self.state.borrow().edit_form.as_ref().map(|f| f.dn.clone());
        if cur_dn != self.built_dn {
            self.rebuild_cells(ctx);
            self.built_dn = cur_dn;
        }
        let (header, rows): (String, Vec<(String, String, bool)>) = {
            let st = self.state.borrow();
            match st.edit_form.as_ref() {
                None => (String::new(), Vec::new()),
                Some(form) => {
                    let header = header_text(form);
                    let rows = form.fields.iter().map(|f| {
                        let marker = if f.must { "*" } else { "" };
                        (format!("{}{}", f.label, marker), present_field(f), inline_editable(f))
                    }).collect();
                    (header, rows)
                }
            }
        };
        if let Some(h) = self.group.child_mut(self.header_id) {
            h.set_value(FieldValue::Text(header));
        }
        // Update each cell via the scroll group.
        let (label_ids, value_ids) = (self.label_ids.clone(), self.value_ids.clone());
        if let Some(sg) = self.scroll_mut() {
            for (i, (label, value, editable)) in rows.iter().enumerate() {
                if let (Some(&lid), Some(&vid)) = (label_ids.get(i), value_ids.get(i)) {
                    if let Some(l) = sg.child_mut(lid) { l.set_value(FieldValue::Text(label.clone())); }
                    if let Some(v) = sg.child_mut(vid) {
                        v.set_value(FieldValue::Text(value.clone()));
                        v.state_mut().state.disabled = !editable;
                    }
                }
            }
        }
        if let Some(first) = self.editable_value_ids().first().copied() {
            if let Some(sg) = self.scroll_mut() { sg.focus_child(first, ctx); }
        }
    }
```

Update `editable_value_ids`, `focus_field`, and `sync_into_form` to index `self.value_ids` (now full-length, **drop the `.take(FORM_ROWS)`**) and to read/write cells through `self.scroll_mut()` instead of `self.group`. Update `value_disabled`/`set_value_text` test seams to go through `scroll_mut`. The Up/Down `focus_field` calls `sg.focus_child`; the embedded `ScrollGroup::handle_event` already does scroll-to-focused, so the focused field stays visible.

Remove the `FORM_ROWS` constant.

- [ ] **Step 4: Run test to verify it passes**

Run: `CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target cargo test -j4 --lib panes::form`
Expected: PASS (all form tests, incl. the adapted ones).

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt
CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target cargo clippy -j4 --all-targets -- -D warnings
git add src/tui/panes/form.rs
git commit -m "feat(tui): rebuild FormPane on ScrollGroup — per-field cells, no 32-row cap"
```

---

### Task 6: `FormPane` — `on_bounds_changed` forwards to the `ScrollGroup`

**Files:**
- Modify: `src/tui/panes/form.rs`
- Test: same file

**Interfaces:**
- Produces: `FormPane::on_bounds_changed` re-bounds the header (full width, row 0) and the `ScrollGroup` child (rows 1..h) to the live pane size; the `ScrollGroup` then re-fits itself (Task 2).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn on_bounds_changed_refits_header_and_scroll() {
    let shared = state_with_form();
    let mut pane = FormPane::new(Rect::new(0, 0, 40, 6), shared);
    let mut out = VecDeque::new();
    let mut timers = tv::timer::TimerQueue::new();
    let mut deferred = Vec::new();
    let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
    <FormPane as View>::change_bounds(&mut pane, Rect::new(0, 0, 80, 20));
    <FormPane as View>::on_bounds_changed(&mut pane, &mut ctx);
    // Header spans the new full width; scroll child fills rows 1..20.
    assert_eq!(pane.header_bounds_for_test().b.x, 80);
    assert_eq!(pane.scroll_bounds_for_test(), Rect::new(0, 1, 80, 20));
}
```

Add test seams:

```rust
#[cfg(test)]
pub(crate) fn header_bounds_for_test(&mut self) -> Rect {
    self.group.child_mut(self.header_id).unwrap().state().get_bounds()
}
#[cfg(test)]
pub(crate) fn scroll_bounds_for_test(&mut self) -> Rect {
    self.group.child_mut(self.scroll_id).unwrap().state().get_bounds()
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target cargo test -j4 --lib panes::form::tests::on_bounds_changed_refits_header_and_scroll`
Expected: FAIL — `on_bounds_changed` not overridden.

- [ ] **Step 3: Write minimal implementation**

Add to the `#[delegate(to = group)] impl View for FormPane`:

```rust
fn on_bounds_changed(&mut self, ctx: &mut Context) {
    let ext = self.group.state().get_extent();
    let w = ext.b.x - ext.a.x;
    let h = ext.b.y - ext.a.y;
    if let Some(hdr) = self.group.child_mut(self.header_id) {
        hdr.change_bounds(Rect::new(0, 0, w, 1));
    }
    if let Some(sc) = self.group.child_mut(self.scroll_id) {
        sc.change_bounds(Rect::new(0, 1, w, h));
        sc.on_bounds_changed(ctx);
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target cargo test -j4 --lib panes::form`
Expected: PASS.

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt
CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target cargo clippy -j4 --all-targets -- -D warnings
git add src/tui/panes/form.rs
git commit -m "feat(tui): FormPane on_bounds_changed refits header + ScrollGroup"
```

---

### Task 7: `LeafPane` — `on_bounds_changed` relayout (kills the `▒` strip)

**Files:**
- Modify: `src/tui/panes/leaf.rs`
- Test: same file

**Interfaces:**
- Produces: `LeafPane::on_bounds_changed` re-bounds the search box (`0,0,w,1`) and the list (`0,1,w,h`) to the live pane size and drives the list's bounds-changed path.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn on_bounds_changed_refits_search_and_list() {
    let inputs = vec![
        StructureInput { dn: "dc=x".into(), cn: None, description: None, object_classes: vec![], attrs: BTreeMap::new() },
    ];
    let structure = Structure::build("dc=x", inputs);
    let schema = SchemaModel::from_raw(&RawSubschema::default());
    let st = UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
    let shared: Shared = Rc::new(RefCell::new(st));
    let mut pane = LeafPane::new(Rect::new(0, 0, 30, 10), shared);
    let mut out = VecDeque::new();
    let mut timers = tv::timer::TimerQueue::new();
    let mut deferred = Vec::new();
    let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
    <LeafPane as View>::change_bounds(&mut pane, Rect::new(0, 0, 50, 20));
    <LeafPane as View>::on_bounds_changed(&mut pane, &mut ctx);
    assert_eq!(pane.search_bounds_for_test(), Rect::new(0, 0, 50, 1));
    assert_eq!(pane.list_bounds_for_test(), Rect::new(0, 1, 50, 20));
}
```

Add the headless ctx helper to the leaf test module if absent, plus seams:

```rust
#[cfg(test)]
fn headless_ctx<'a>(out: &'a mut VecDeque<Event>, timers: &'a mut tv::timer::TimerQueue, deferred: &'a mut Vec<tv::Deferred>) -> Context<'a> {
    Context::new(out, timers, 0, deferred)
}
// on LeafPane:
#[cfg(test)]
pub(crate) fn search_bounds_for_test(&mut self) -> Rect {
    self.group.child_mut(self.search_id).unwrap().state().get_bounds()
}
#[cfg(test)]
pub(crate) fn list_bounds_for_test(&mut self) -> Rect {
    self.group.child_mut(self.list_id).unwrap().state().get_bounds()
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target cargo test -j4 --lib panes::leaf::tests::on_bounds_changed_refits_search_and_list`
Expected: FAIL — `on_bounds_changed` not overridden.

- [ ] **Step 3: Write minimal implementation**

Add to `#[delegate(to = group)] impl View for LeafPane`:

```rust
fn on_bounds_changed(&mut self, ctx: &mut Context) {
    let ext = self.group.state().get_extent();
    let w = ext.b.x - ext.a.x;
    let h = ext.b.y - ext.a.y;
    if let Some(s) = self.group.child_mut(self.search_id) {
        s.change_bounds(Rect::new(0, 0, w, 1));
    }
    if let Some(l) = self.group.child_mut(self.list_id) {
        l.change_bounds(Rect::new(0, 1, w, h));
        l.on_bounds_changed(ctx);
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target cargo test -j4 --lib panes::leaf`
Expected: PASS.

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt
CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target cargo clippy -j4 --all-targets -- -D warnings
git add src/tui/panes/leaf.rs
git commit -m "fix(tui): LeafPane refits search+list on resize (kills the desktop-bg strip)"
```

---

### Task 8: Guard edge #2 — cancelled confirm snaps back

**Files:**
- Modify: `src/tui/app.rs`
- Test: `src/tui/app.rs` `#[cfg(test)]` (a routing test, dialog kept out)

**Interfaces:**
- Consumes: `do_save`, `dispatch`, `UiState::current_leaf_row`, `set_leaf_row`.
- Produces:
  - `enum SaveOutcome { Submitted, NotSubmitted }`
  - `fn do_save(...) -> SaveOutcome`
  - In `dispatch`'s `GUARD_NAV` → `Save` arm: when `do_save` returns `NotSubmitted`, snap back (`set_leaf_row = current_leaf_row()`, clear `guard_target`/`pending_nav`) — a cancelled confirm behaves like *Stay*.

**Note:** `do_save` opens a modal (`exec_view_focused`), so the snap-back logic is tested via a pure helper, not by driving the dialog. Factor the snap-back into a tiny pure function and unit-test that.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn cancelled_guard_save_snaps_highlight_back() {
    // The guard→Save path that does NOT submit must request a snap-back to the
    // pinned form's row and clear the stashed nav targets (like Stay).
    use crate::tui::state::UiState;
    use crate::workflows::structure::{Structure, StructureInput};
    use crate::ldap::worker::RawSubschema;
    use crate::schema::SchemaModel;
    use std::collections::BTreeMap;

    let inputs = vec![
        StructureInput { dn: "dc=x".into(), cn: None, description: None, object_classes: vec![], attrs: BTreeMap::new() },
        StructureInput { dn: "ou=p,dc=x".into(), cn: None, description: None, object_classes: vec![], attrs: BTreeMap::new() },
        StructureInput { dn: "cn=a,ou=p,dc=x".into(), cn: Some("a".into()), description: None, object_classes: vec![], attrs: BTreeMap::new() },
    ];
    let structure = Structure::build("dc=x", inputs);
    let schema = SchemaModel::from_raw(&RawSubschema::default());
    let mut st = UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
    st.current_branch = Some("ou=p,dc=x".into());
    st.current_leaf = Some("cn=a,ou=p,dc=x".into());
    st.guard_target = Some(("cn=b,ou=p,dc=x".into(), vec![]));
    st.pending_nav = Some(("cn=b,ou=p,dc=x".into(), vec![]));

    apply_cancelled_guard_save(&mut st);

    assert_eq!(st.set_leaf_row, st.current_leaf_row(), "snap back to the pinned form's row");
    assert!(st.guard_target.is_none());
    assert!(st.pending_nav.is_none());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target cargo test -j4 --lib tui::app::tests::cancelled_guard_save_snaps_highlight_back`
Expected: FAIL — `apply_cancelled_guard_save` not found.

- [ ] **Step 3: Write minimal implementation**

In `src/tui/app.rs`:

```rust
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SaveOutcome {
    Submitted,
    NotSubmitted,
}

/// Snap the leaf highlight back to the pinned form and clear nav targets (the
/// cancelled-confirm-on-guard case == Stay). Pure; unit-tested.
pub(crate) fn apply_cancelled_guard_save(st: &mut crate::tui::state::UiState) {
    st.set_leaf_row = st.current_leaf_row();
    st.guard_target = None;
    st.pending_nav = None;
}
```

Change `do_save`'s signature to `-> SaveOutcome`: return `SaveOutcome::NotSubmitted` at the early `None` return, the `Status`/`Error` arms, and the confirm-`Cancel` (`!= Command::OK`) return; return `SaveOutcome::Submitted` after a successful `write_flow.submit`. Then in `dispatch`, the `GUARD_NAV` → `GuardDecision::Save` arm:

```rust
GuardDecision::Save => {
    if do_save(prog, state, target, false) == SaveOutcome::NotSubmitted {
        let mut st = state.borrow_mut();
        apply_cancelled_guard_save(&mut st);
    }
}
```

(The `REQUEST_QUIT` Save arm and plain `SAVE` arm ignore the return value — `let _ = do_save(...)`.)

- [ ] **Step 4: Run test to verify it passes**

Run: `CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target cargo test -j4 --lib tui::app`
Expected: PASS.

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt
CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target cargo clippy -j4 --all-targets -- -D warnings
git add src/tui/app.rs
git commit -m "fix(tui): cancelled guard-Save snaps the highlight back (edge #2)"
```

---

### Task 9: Guard edge #3 — state: `requested_branch`, `set_tree_row`, `reconcile_branch`

**Files:**
- Modify: `src/tui/state.rs`
- Test: `src/tui/state.rs` `#[cfg(test)]`

**Interfaces:**
- Produces on `UiState`:
  - fields `requested_branch: Option<String>`, `set_tree_row: Option<i32>`.
  - `enum GuardTarget { Leaf(String, Vec<String>), Branch(String) }` and change `guard_target` to `Option<GuardTarget>`.
  - `fn request_branch(&mut self, dn: String)`
  - `fn current_branch_row(&self) -> Option<i32>` (index of `current_branch` in `branch_dns`).
  - `fn reconcile_branch(&mut self) -> bool` — clean form → switch branch (set `current_branch`, `list_dirty=true`); dirty → stash `GuardTarget::Branch` and return `true`.

**Note:** `guard_target` currently holds `(String, Vec<String>)` (a leaf). Introduce `GuardTarget` and update `reconcile_selection` to stash `GuardTarget::Leaf`, and `app::dispatch`/`do_save` to read it (done in Task 11). Keep this task's edits compiling by updating the existing `guard_target` readers minimally (the dispatch rewrite lands in Task 11; here, adapt `reconcile_selection` + any direct `guard_target` field uses in `state.rs`).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn reconcile_branch_clean_switches_dirty_guards() {
    // (mirror the existing reconcile_selection tests)
    let inputs = vec![
        si("dc=x", None), si("ou=p,dc=x", None), si("ou=q,dc=x", None),
        si("cn=a,ou=p,dc=x", Some("a")), si("cn=b,ou=q,dc=x", Some("b")),
    ];
    let structure = Structure::build("dc=x", structure_inputs_from(inputs));
    let schema = SchemaModel::from_raw(&RawSubschema::default());
    let mut st = UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
    st.branch_dns = vec!["dc=x".into(), "ou=p,dc=x".into(), "ou=q,dc=x".into()];
    st.current_branch = Some("ou=p,dc=x".into());

    // Clean form → switch immediately.
    st.request_branch("ou=q,dc=x".into());
    assert!(!st.reconcile_branch());
    assert_eq!(st.current_branch.as_deref(), Some("ou=q,dc=x"));
    assert!(st.list_dirty);

    // Dirty form → stash a Branch guard target, signal guard, do not switch.
    st.current_branch = Some("ou=p,dc=x".into());
    st.edit_form = Some(dirty_form("cn=a,ou=p,dc=x"));
    st.request_branch("ou=q,dc=x".into());
    assert!(st.reconcile_branch());
    assert!(matches!(st.guard_target, Some(GuardTarget::Branch(ref b)) if b == "ou=q,dc=x"));
    assert_eq!(st.current_branch.as_deref(), Some("ou=p,dc=x"), "stays until guarded");
}
```

(Use the existing test helpers; add a small `dirty_form(dn)` helper that builds an `EditForm` whose first field's `values != baseline`, and an `si(dn, cn)`/`structure_inputs_from` consistent with the file's existing helpers.)

- [ ] **Step 2: Run test to verify it fails**

Run: `CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target cargo test -j4 --lib tui::state::`
Expected: FAIL — new fields/enum/methods absent.

- [ ] **Step 3: Write minimal implementation**

Add the enum (top of `state.rs`):

```rust
/// A dirty-blocked navigation awaiting the guard's decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardTarget {
    Leaf(String, Vec<String>),
    Branch(String),
}
```

Change the struct field `pub guard_target: Option<(String, Vec<String>)>` → `pub guard_target: Option<GuardTarget>`, and add `pub requested_branch: Option<String>` and `pub set_tree_row: Option<i32>`. Initialise all three to `None` in both constructors (`new_for_test` and the production `Ok(UiState { .. })`). Update `reconcile_selection` to stash `GuardTarget::Leaf(dn, ocs)`.

Add the methods:

```rust
pub fn request_branch(&mut self, dn: String) {
    self.requested_branch = Some(dn);
}

pub fn current_branch_row(&self) -> Option<i32> {
    let cur = self.current_branch.as_deref()?;
    self.branch_dns.iter().position(|d| d == cur).map(|i| i as i32)
}

pub fn reconcile_branch(&mut self) -> bool {
    let Some(dn) = self.requested_branch.take() else { return false; };
    if self.current_branch.as_deref() == Some(dn.as_str()) {
        return false;
    }
    let dirty = self.edit_form.as_ref().map(|f| f.is_dirty()).unwrap_or(false);
    if dirty {
        self.guard_target = Some(GuardTarget::Branch(dn));
        true
    } else {
        self.current_branch = Some(dn);
        self.list_dirty = true;
        false
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target cargo test -j4 --lib tui::state`
Expected: PASS (existing reconcile tests still pass with `GuardTarget::Leaf`).

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt
CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target cargo clippy -j4 --all-targets -- -D warnings
git add src/tui/state.rs
git commit -m "feat(tui): branch nav controller state (requested_branch/reconcile_branch/GuardTarget)"
```

---

### Task 10: Guard edge #3 — `TreePane` becomes a pure selector

**Files:**
- Modify: `src/tui/panes/tree.rs`
- Test: same file

**Interfaces:**
- Consumes: `UiState::request_branch`, `set_tree_row`, `branch_dns`.
- Produces: `TreePane::handle_event` records `requested_branch` on selection change (no inline `current_branch`/`list_dirty`/broadcast), and honours `set_tree_row` (snap the outline selection back on guard *Stay*).

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn tree_records_requested_branch_only() {
    let inputs = vec![ si("dc=x"), si("ou=a,dc=x"), si("ou=b,dc=x"), si("cn=1,ou=a,dc=x"), si("cn=1,ou=b,dc=x") ];
    let structure = Structure::build("dc=x", inputs);
    let schema = SchemaModel::from_raw(&RawSubschema::default());
    let mut st = UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), compile_tree_rules(&TreeConfig::default()));
    let (root, dns) = build_branch_nodes(&st, 40);
    st.branch_dns = dns;
    let shared: std::rc::Rc<std::cell::RefCell<UiState>> = std::rc::Rc::new(std::cell::RefCell::new(st));
    let mut pane = TreePane::new(Rect::new(0, 0, 30, 10), root, shared.clone());

    let mut out = std::collections::VecDeque::new();
    let mut timers = tv::timer::TimerQueue::new();
    let mut deferred = Vec::new();
    let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);
    // Move selection to row 1 (ou=a) and deliver an event.
    pane.select_row_for_test(1, &mut ctx);
    let mut ev = Event::Broadcast { command: REFRESH, source: None };
    pane.handle_event(&mut ev, &mut ctx);

    let st = shared.borrow();
    assert_eq!(st.requested_branch.as_deref(), Some("ou=a,dc=x"));
    assert_eq!(st.current_branch, None, "pure selector: never switches inline");
}
```

Add a `select_row_for_test` seam to `TreePane`:

```rust
#[cfg(test)]
pub(crate) fn select_row_for_test(&mut self, row: i32, ctx: &mut Context) {
    self.outline.set_value_ctx(tv::FieldValue::Int(row), ctx);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target cargo test -j4 --lib panes::tree::tests::tree_records_requested_branch_only`
Expected: FAIL — tree still sets `current_branch` inline.

- [ ] **Step 3: Write minimal implementation**

Rewrite `TreePane::handle_event`:

```rust
fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
    // Controller → pane: snap the selection back (guard "Stay") before reporting.
    let snap = self.state.borrow_mut().set_tree_row.take();
    if let Some(row) = snap {
        self.outline.set_value_ctx(FieldValue::Int(row), ctx);
        self.last_sel = row;
    }

    self.outline.handle_event(ev, ctx);
    let sel = match self.outline.value() {
        Some(FieldValue::Int(i)) => i,
        _ => 0,
    };
    if sel != self.last_sel {
        self.last_sel = sel;
        if sel >= 0 {
            let dn = self.state.borrow().branch_dns.get(sel as usize).cloned();
            if let Some(dn) = dn {
                self.state.borrow_mut().request_branch(dn); // pure selector
            }
        }
    }
}
```

(The `REFRESH` import stays used by the test; the pane no longer broadcasts.)

- [ ] **Step 4: Run test to verify it passes**

Run: `CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target cargo test -j4 --lib panes::tree`
Expected: PASS.

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt
CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target cargo clippy -j4 --all-targets -- -D warnings
git add src/tui/panes/tree.rs
git commit -m "refactor(tui): TreePane is a pure selector (records requested_branch)"
```

---

### Task 11: Guard edge #3 — pump reconcile + dispatch branch guard

**Files:**
- Modify: `src/tui/pump.rs`, `src/tui/app.rs`
- Test: `src/tui/app.rs` `#[cfg(test)]` (the `GuardTarget` dispatch routing helper)

**Interfaces:**
- Consumes: `UiState::reconcile_branch`, `current_branch_row`, `GuardTarget`, `set_tree_row`.
- Produces:
  - pump calls `reconcile_branch()` each tick and posts `GUARD_NAV` when it returns `true`.
  - `dispatch` `GUARD_NAV` reads `guard_target: Option<GuardTarget>` and acts per variant; *Stay* on a `Branch` target sets `set_tree_row = current_branch_row()`.

- [ ] **Step 1: Write the failing test** (pure routing of the guard decision for a Branch target)

```rust
#[test]
fn guard_stay_on_branch_target_reverts_tree() {
    use crate::tui::state::{GuardTarget, UiState};
    use crate::workflows::structure::Structure;
    use crate::ldap::worker::RawSubschema;
    use crate::schema::SchemaModel;
    let structure = Structure::build("dc=x", vec![]);
    let schema = SchemaModel::from_raw(&RawSubschema::default());
    let mut st = UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
    st.branch_dns = vec!["dc=x".into(), "ou=p,dc=x".into(), "ou=q,dc=x".into()];
    st.current_branch = Some("ou=p,dc=x".into());
    st.guard_target = Some(GuardTarget::Branch("ou=q,dc=x".into()));

    apply_branch_guard_stay(&mut st);

    assert_eq!(st.set_tree_row, st.current_branch_row(), "revert tree to current branch");
    assert!(st.guard_target.is_none());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target cargo test -j4 --lib tui::app::tests::guard_stay_on_branch_target_reverts_tree`
Expected: FAIL — `apply_branch_guard_stay` not found.

- [ ] **Step 3: Write minimal implementation**

In `src/tui/pump.rs`, inside the `Event::Timer` block after `reconcile_selection`:

```rust
let need_branch_guard = self.state.borrow_mut().reconcile_branch();
// ... existing: if need_guard { ctx.post(GUARD_NAV); }
if need_branch_guard {
    ctx.post(crate::tui::GUARD_NAV);
}
```

In `src/tui/app.rs` add the pure Stay helper and rewrite the `GUARD_NAV` arm to dispatch on the `GuardTarget` variant:

```rust
pub(crate) fn apply_branch_guard_stay(st: &mut crate::tui::state::UiState) {
    st.set_tree_row = st.current_branch_row();
    st.guard_target = None;
}
```

```rust
} else if cmd == GUARD_NAV {
    let target = state.borrow().guard_target.clone();
    match run_guard(prog) {
        GuardDecision::Save => {
            // Save persists the pinned form; the post-save nav uses the target.
            let nav = match &target {
                Some(GuardTarget::Leaf(dn, ocs)) => Some((dn.clone(), ocs.clone())),
                _ => None, // branch save: just persist, then the tree re-requests
            };
            if do_save(prog, state, nav, false) == SaveOutcome::NotSubmitted {
                let mut st = state.borrow_mut();
                match target {
                    Some(GuardTarget::Branch(_)) => apply_branch_guard_stay(&mut st),
                    _ => apply_cancelled_guard_save(&mut st),
                }
            } else if let Some(GuardTarget::Branch(dn)) = target {
                // Save submitted: switch the branch now (form will reload clean).
                let mut st = state.borrow_mut();
                st.current_branch = Some(dn);
                st.list_dirty = true;
                st.guard_target = None;
            }
        }
        GuardDecision::Discard => {
            discard_edits(state);
            match target {
                Some(GuardTarget::Leaf(dn, ocs)) => state.borrow_mut().reread_public(&dn, &ocs),
                Some(GuardTarget::Branch(dn)) => {
                    let mut st = state.borrow_mut();
                    st.current_branch = Some(dn);
                    st.list_dirty = true;
                }
                None => {}
            }
            state.borrow_mut().guard_target = None;
        }
        GuardDecision::Stay => {
            let mut st = state.borrow_mut();
            match target {
                Some(GuardTarget::Branch(_)) => apply_branch_guard_stay(&mut st),
                _ => { st.set_leaf_row = st.current_leaf_row(); }
            }
            st.guard_target = None;
        }
    }
}
```

(Adjust imports: `use crate::tui::state::GuardTarget;`. The leaf-Stay path keeps today's `set_leaf_row` behaviour. Verify no other reader of `guard_target` assumes the old tuple type — update them to match the enum.)

- [ ] **Step 4: Run the full suite**

Run: `CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target cargo test -j4`
Expected: PASS (whole workspace).

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt
CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target cargo clippy -j4 --all-targets -- -D warnings
git add src/tui/pump.rs src/tui/app.rs
git commit -m "fix(tui): guard branch change while dirty; Stay reverts the tree (edge #3)"
```

---

### Task 12: Changelog + facade guards + live acceptance

**Files:**
- Modify: `CHANGES.md`

- [ ] **Step 1: Update `CHANGES.md`** — under `## Unreleased` → `### New` / `### Fixed`:

```markdown
- tvision UI (preview): the three panes now fill their area and the entry form
  scrolls — a vertical scrollbar appears when an entry has more attributes than
  fit, and every attribute is reachable (the former 32-row display cap is gone).
```
and under `### Fixed`:
```markdown
- tvision UI (preview): cancelling the save-confirm raised by an unsaved-changes
  guard now snaps the list highlight back to the form being edited; and changing
  branch in the tree while the form is dirty now raises the same guard (Stay
  reverts the tree, Discard/Save behave as on a leaf change).
```

- [ ] **Step 2: Facade guards (must print nothing)**

Run:
```bash
! grep -rl "use tvision_rs" src | grep -vE "^src/(tui/|bin/edaptor-tv.rs)"
! grep -rl "use ratatui\|use tui_" src | grep -vE "^src/ui/"
```
Expected: no output.

- [ ] **Step 3: `make check`**

Run: `CARGO_TARGET_DIR=/home/oetiker/scratch/cargo-target make check`
Expected: fmt clean, clippy clean, all tests pass.

- [ ] **Step 4: Live tmux acceptance** (build first: `cargo build -j4 --bin edaptor-tv`; binary at `/home/oetiker/scratch/cargo-target/debug/edaptor-tv`). Drive per the handover's tmux recipe and confirm:
  - No `▒` desktop-background strip in any pane at launch or after a `tmux resize-window`.
  - An entry with many attributes shows a form scrollbar; Up/Down past the visible edge scrolls; every attribute is reachable; the header/dirty row stays pinned at top.
  - Edit a field → change to another leaf → guard fires; Cancel the Save-confirm → highlight snaps back to the pinned form (no mismatch).
  - With a dirty form, change branch in the tree → guard fires; Stay reverts the tree to the current branch; Discard switches branch and drops the edit.
  - Restore any demo data touched.

- [ ] **Step 5: Commit**

```bash
git add CHANGES.md
git commit -m "docs(changes): tvision panes fill+scroll; guard edges #2/#3"
```

---

## Self-Review

**Spec coverage:** §1 panes-fill → Tasks 6 (form), 7 (leaf), and ScrollGroup `on_bounds_changed` (Task 2). §2 form scrolling via ScrollGroup → Tasks 1–6 (struct/clip/bar/ensure-visible/form-rebuild/refit). §3 guard #2 → Task 8. §4 guard #3 → Tasks 9–11. Acceptance criteria 1–5 → Task 12. Upstream-extraction note → satisfied by `ScrollGroup` being domain-free in its own module.

**Placeholder scan:** none — every step has concrete code/commands.

**Type consistency:** `ScrollGroup::{new, inner_width, add_content, clear_content, child_mut, scroll_to, ensure_visible, logical_of, current, focus_child, content_height, max_top}` used consistently across Tasks 1–6. `GuardTarget::{Leaf,Branch}` introduced in Task 9 and consumed in Tasks 10–11. `SaveOutcome::{Submitted,NotSubmitted}` defined and used in Task 8 and reused in Task 11. `set_tree_row`/`requested_branch`/`current_branch_row` defined in Task 9 and used in Tasks 10–11.

**Verified API facts (checked against tvision-rs 0.3.0 source, lib.rs:133–154, view.rs:91):**
- Crate-root re-exports confirmed: `Buffer`, `DrawCtx`, `Context`, `Deferred`, `Point`, `Rect`, `Group`, `GrowMode`, `ScrollBar`, `StaticText`, `Label`, `Outline`, `InputLine`, `ViewId` — all import as `tvision_rs::<Name>`.
- `state_mut().state.visible` is correct: `State.visible: bool` (view.rs:91), reached via `ViewState.state`.

**One validation point (confirm during TDD):**
- The headless-draw assertion in Task 2 asserts a specific glyph from a `StaticText`'s rendering (`Buffer::get(x,y).symbol()`); if `StaticText` pads/aligns differently, adjust the asserted glyph — the behaviour under test (offscreen rows clip; the on-top row is the scrolled one) is the contract, not the literal character.
