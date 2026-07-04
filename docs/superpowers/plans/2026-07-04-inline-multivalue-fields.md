# Inline Multi-Value Fields Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render multi-value LDAP attributes as inline bulleted lists in the entry form, edit free-text/ordered lists in place (no popup), keep shuttle/choice/password on a highlight-then-launch model, and drive a dynamic hint in the footer.

**Architecture:** Replace the form pane's fixed one-`InputLine`-per-field row model with a vertical stack of **variable-height field blocks**. Each field maps to one value-view: `InputLine` (single-value text, unchanged), `LaunchValueView` (read-only bullet/masked block that opens the existing modal on an action key), or `ListValueView` (a new in-place multi-line bullet editor built on a pure `ListModel`). The footer follows the focused view's `help_ctx`, which tvision's `StatusLine` propagates automatically.

**Tech Stack:** Rust, tvision-rs 0.11 (`Group`/`ScrollGroup`, `View`, `DrawCtx`, `HelpCtx`, `StatusLine`, `text::next`/`text::prev` grapheme stepping).

## Global Constraints

- **Cap build/test parallelism at 4 cores** (shared machine): `cargo test -j4`, `cargo clippy --all-targets -j4 -- -D warnings`.
- `make check` (fmt + clippy `-D warnings` + tests) must pass before any task is "done".
- **English** for all code, comments, identifiers.
- tvision-rs pinned at `0.11` — do not bump.
- Every user-visible change updates **`CHANGES.md`**; config/behaviour docs live in the **mdBook** (`docs/src/`), orientation in **`README.md`** (no duplication).
- Do **not** reintroduce the removed `[profile.picker.*]` / `[profile.password]` config layers.
- Follow the existing borrow discipline in `form.rs`: collect field metadata under a short `self.state.borrow()`, drop it, then touch views.

## Key existing anchors (verified against the working tree)

- `EditField` — `src/workflows/edit_form.rs:14-28`: `label, must, editable, multi, secret, ordered, orphaned, kind, widget: WidgetSpec, widget_binding: Option<WidgetKind>, values: Vec<String>, baseline: Vec<String>`.
- Widget routing — `src/ui/widget.rs`: `widget_for` (130-154), `is_modal_field` (159-168), `inline_editable` (124-126), `present_field` (105-119). `Activation` (33-36), `FieldEditor::into_view` (42-48), `CommitOutcome` (19-28).
- Form pane — `src/ui/panes/form.rs`: struct (46-70), `handle_event` (609-729), `render` (289-393), `rebuild_cells` (217-285), `focus_field` (439-464), `focusable_value_ids` (418-433), `focused_field_idx` (506-509), `place_cursor_home` (399-414), `value_id_for_label_hit` (477-503), `sync_into_form` (527-578), `scroll_mut` (116-121), `cell_focusable` (42-44).
- Modal launch — pane posts `ACTIVATE` with `state.activate_field = Some(idx)` (`form.rs:663-671`); controller builds+execs the modal and applies the result (`src/ui/app.rs:123-197`); `apply_commit` writes `edit_form.fields[idx].values` and sets `form_needs_render` (`src/ui/state.rs:449-493`).
- Constants — `src/ui/mod.rs`: `ACTIVATE` (43), `REFRESH` (40), `Shared` (37).
- Ordered helpers — `src/ui/ordered.rs`: `strip_ordering(&str)->&str` (12-27), `reconstruct(&[String])->Vec<String>` (29-35).
- tvision: `HelpCtx::custom(&'static str)` (`help.rs:44`); a leaf carries `ViewState.help_ctx`, and `StatusLine` auto-follows the focused leaf's `get_help_ctx()` (`app/program.rs:2169-2199`, `view/group.rs:1134`). `StatusDef::list().def_one_of([ctx], |d| …).def_all(|d| …).build()` with first-match-wins (`status/mod.rs:207-242`); `StatusLine::with_hint(|ctx| Option<String>)` (`status/status_line.rs:206-211`). Grapheme stepping `text::next(&str)->Option<(usize,usize)>` / `text::prev(&str,usize)->usize` (`text.rs:50,64`). Leaf drawing via `DrawCtx`: `content_surface`, `style`, `fill`, `put_str`, `put_char`, `sub` (`view/context.rs`); cursor via `ViewState::set_cursor`/`show_cursor` + `View::cursor_request`.

---

## STAGE 1 — Variable-height blocks + LaunchValueView

Delivers the new look and the highlight-then-launch behaviour for every modal field. Free-text/ordered fields are shown as read-only bullets that still open the existing modal (inline editing arrives in Stage 2). Single-value text keeps its `InputLine`. Shippable on its own.

### Task 1: Field → value-view classification + block height (pure)

**Files:**
- Modify: `src/ui/panes/form.rs` (add a module-private `value_kind` classifier + `block_height` helper near `cell_focusable`, ~line 42)
- Test: inline `#[cfg(test)]` in `src/ui/panes/form.rs`

**Interfaces:**
- Produces:
  - `enum ValueKind { Text, List { ordered: bool }, Launch }`
  - `fn value_kind(f: &EditField) -> ValueKind`
  - `fn block_height(f: &EditField, kind: ValueKind) -> i32` — display rows the field occupies: `Text` → 1; `Launch`/`List` → number of display lines (`1` when the value set is empty, i.e. the `<not set>` line; otherwise the count of bulleted lines including continuation lines from `\n`-split values).

Classification rule (mirror `widget_for`/`is_modal_field` at `widget.rs:130-168`):
- `Text` when `inline_editable(f)` (single-value plain, editable) **or** the field is non-editable/read-only single-valued (falls through `widget_for` to `PlainWidget`) — anything that is one line of plain text today.
- `List { ordered }` when the field routes to the inline editors: `f.editable && !f.orphaned && ((f.multi && f.widget_binding.is_none()) )` for free-text (`ordered=false`), or `matches!(f.widget_binding, Some(WidgetKind::XOrdered))` for ordered (`ordered=true`).
- `Launch` for the remaining modal fields: objectClass (label eq_ignore_ascii_case "objectClass"), `Password`, `Choice`, `Picker`, `SambaSid`.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod value_kind_tests {
    use super::*;
    use crate::workflows::edit_form::EditField;
    use crate::config::widget::WidgetKind;

    // Reuse the existing ef(...) test builder in this file (form.rs:799) where possible,
    // extended locally for widget_binding/multi/ordered.
    fn field(label: &str, multi: bool, binding: Option<WidgetKind>) -> EditField {
        let mut f = ef(label, "", true); // ef sets editable=true, multi=false
        f.multi = multi;
        f.widget_binding = binding;
        f
    }

    #[test]
    fn single_value_text_is_text_kind() {
        let f = field("cn", false, None);
        assert_eq!(value_kind(&f), ValueKind::Text);
        assert_eq!(block_height(&f, ValueKind::Text), 1);
    }

    #[test]
    fn plain_multi_is_list_unordered() {
        let f = field("mail", true, None);
        assert_eq!(value_kind(&f), ValueKind::List { ordered: false });
    }

    #[test]
    fn xordered_is_list_ordered() {
        let f = field("olcAccess", true, Some(WidgetKind::XOrdered));
        assert_eq!(value_kind(&f), ValueKind::List { ordered: true });
    }

    #[test]
    fn objectclass_is_launch() {
        let f = field("objectClass", true, None);
        assert_eq!(value_kind(&f), ValueKind::Launch);
    }

    #[test]
    fn empty_multi_block_is_one_line() {
        let f = field("mail", true, None); // values empty
        assert_eq!(block_height(&f, ValueKind::List { ordered: false }), 1);
    }

    #[test]
    fn three_values_one_with_newline_is_four_lines() {
        let mut f = field("mail", true, None);
        f.values = vec!["a".into(), "b\ncont".into(), "c".into()];
        assert_eq!(block_height(&f, ValueKind::List { ordered: false }), 4);
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -j4 --lib value_kind_tests 2>&1 | tail -20`
Expected: FAIL — `value_kind` / `ValueKind` / `block_height` not found.

- [ ] **Step 3: Implement the classifier and height helper**

```rust
/// Which value-view a field renders as in the form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueKind {
    Text,
    List { ordered: bool },
    Launch,
}

fn value_kind(f: &crate::workflows::edit_form::EditField) -> ValueKind {
    use crate::config::widget::WidgetKind;
    if matches!(f.widget_binding, Some(WidgetKind::XOrdered)) {
        return ValueKind::List { ordered: true };
    }
    if f.editable && f.multi && !f.orphaned && f.widget_binding.is_none() {
        return ValueKind::List { ordered: false };
    }
    if crate::ui::widget::is_modal_field(f) {
        // Remaining modal fields (objectClass/Password/Choice/Picker/SambaSid) launch.
        return ValueKind::Launch;
    }
    ValueKind::Text
}

/// Display rows a field occupies. `Text` is always one row; list/launch blocks
/// grow with their values, and an empty value set collapses to the single
/// `<not set>` row.
fn block_height(f: &crate::workflows::edit_form::EditField, kind: ValueKind) -> i32 {
    match kind {
        ValueKind::Text => 1,
        ValueKind::List { .. } | ValueKind::Launch => {
            let non_empty: Vec<&String> =
                f.values.iter().filter(|v| !v.trim().is_empty()).collect();
            if non_empty.is_empty() {
                return 1; // the `<not set>` line
            }
            non_empty
                .iter()
                .map(|v| v.split('\n').count() as i32)
                .sum()
        }
    }
}
```

Note: `is_modal_field` is already imported in `form.rs:19`. `value_kind` must be evaluated **before** `is_modal_field` for the ordered/plain-multi cases, hence the early returns above.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -j4 --lib value_kind_tests 2>&1 | tail -20`
Expected: PASS (6 tests).

- [ ] **Step 5: Commit**

```bash
git add src/ui/panes/form.rs
git commit -m "feat(ui): classify form fields into Text/List/Launch value kinds"
```

---

### Task 2: `LaunchValueView` widget

A focusable leaf that renders a read-only bullet list (multi) / single line / `*****` (password) / `<not set>` (empty), highlights the **whole block** when focused, carries a `help_ctx`, and on an *action* key posts nothing itself but reports "activate me" to the pane. It never edits.

**Files:**
- Create: `src/ui/panes/launch_view.rs`
- Modify: `src/ui/panes/mod.rs` (add `mod launch_view;`)
- Test: inline `#[cfg(test)]` in `launch_view.rs`

**Interfaces:**
- Produces:
  - `pub(crate) struct LaunchValueView` implementing `tv::View`.
  - `pub(crate) fn new(bounds: Rect, help_ctx: HelpCtx) -> Self`
  - `pub(crate) fn set_lines(&mut self, lines: Vec<String>)` — the already-formatted display lines (bullets or `*****`/`<not set>`), one per row. The pane computes these.
  - `pub(crate) fn take_activate(&mut self) -> bool` — true once if the last event was an action key (pane then posts `ACTIVATE`).
  - Consumes: nav keys (Up/Down/Home/End/PgUp/PgDn/Left/Right) are left **unconsumed** (`ev` untouched) so the pane moves focus between fields; every other `KeyDown` sets the internal activate flag and is cleared.

- [ ] **Step 1: Write the failing tests** (pure model behaviour — no ctx)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tvision_rs::{Event, Key, KeyDown, Rect};
    use tvision_rs::HelpCtx;

    fn view() -> LaunchValueView {
        LaunchValueView::new(Rect::new(0, 0, 20, 1), HelpCtx::custom("edaptor.field.launch"))
    }

    #[test]
    fn printable_key_requests_activation_and_is_consumed() {
        let mut v = view();
        let mut ev = Event::KeyDown(KeyDown::from(Key::Char('x')));
        v.on_key(&mut ev);
        assert!(ev.is_nothing(), "action key consumed");
        assert!(v.take_activate());
        assert!(!v.take_activate(), "flag clears after one take");
    }

    #[test]
    fn enter_requests_activation() {
        let mut v = view();
        let mut ev = Event::KeyDown(KeyDown::from(Key::Enter));
        v.on_key(&mut ev);
        assert!(v.take_activate());
    }

    #[test]
    fn arrow_keys_pass_through_for_field_nav() {
        let mut v = view();
        let mut ev = Event::KeyDown(KeyDown::from(Key::Down));
        v.on_key(&mut ev);
        assert!(!ev.is_nothing(), "nav key left for the pane");
        assert!(!v.take_activate());
    }
}
```

> `on_key(&mut self, ev: &mut Event)` is the pure key classifier used by `handle_event`; test it directly so no `Context` is needed. Confirm the exact `KeyDown` constructor against `src/ui/multivalue.rs` / `input_line.rs` usage (`KeyDown::from(Key::…)`); adjust if the crate uses a different shape.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -j4 --lib ui::panes::launch_view 2>&1 | tail -20`
Expected: FAIL — module/type not found.

- [ ] **Step 3: Implement `LaunchValueView`**

```rust
//! Read-only value block for modal ("launch") fields: renders a bulleted list
//! (or `*****` / `<not set>`), highlights as a whole when focused, and reports an
//! activation request when the user presses any action key. Editing happens in
//! the modal the pane opens — this view never mutates values.

use tvision_rs::{
    self as tv, DrawCtx, Event, HelpCtx, Key, Point, Rect, Role, SurfaceRoles, View, ViewState,
};

pub(crate) struct LaunchValueView {
    state: ViewState,
    lines: Vec<String>,
    activate: bool,
}

const SURFACE_ROLES: SurfaceRoles = SurfaceRoles {
    normal: Role::InputNormal,
    surface: Role::InputSurface,
    inactive: Role::InputInactive,
};

impl LaunchValueView {
    pub(crate) fn new(bounds: Rect, help_ctx: HelpCtx) -> Self {
        let mut state = ViewState::new(bounds);
        state.options.selectable = true;
        state.help_ctx = help_ctx;
        Self { state, lines: vec!["<not set>".to_string()], activate: false }
    }

    pub(crate) fn set_lines(&mut self, lines: Vec<String>) {
        self.lines = if lines.is_empty() { vec!["<not set>".to_string()] } else { lines };
    }

    pub(crate) fn take_activate(&mut self) -> bool {
        std::mem::take(&mut self.activate)
    }

    /// Classify a key: nav keys pass through (leave `ev`); any other key marks an
    /// activation request and consumes the event.
    fn on_key(&mut self, ev: &mut Event) {
        let Event::KeyDown(k) = ev else { return };
        let is_nav = matches!(
            k.key,
            Key::Up | Key::Down | Key::Left | Key::Right | Key::Home | Key::End | Key::PageUp
                | Key::PageDown | Key::Tab
        );
        if is_nav {
            return; // pane handles field navigation
        }
        self.activate = true;
        ev.clear();
    }
}

impl View for LaunchValueView {
    fn state(&self) -> &ViewState { &self.state }
    fn state_mut(&mut self) -> &mut ViewState { &mut self.state }

    fn draw(&mut self, ctx: &mut DrawCtx) {
        let size = self.state.size;
        let color = ctx.content_surface(SURFACE_ROLES, self.state.state.focused, true);
        ctx.fill(Rect::new(0, 0, size.x, size.y), ' ', color);
        for (row, line) in self.lines.iter().enumerate() {
            if (row as i32) < size.y {
                ctx.put_str(0, row as i32, line, color);
            }
        }
    }

    fn handle_event(&mut self, ev: &mut Event, _ctx: &mut tv::Context) {
        if matches!(ev, Event::KeyDown(_)) {
            self.on_key(ev);
        }
    }

    // No text cursor: the whole block is the selection cue via the focused surface.
    fn cursor_request(&self) -> Option<Point> { None }

    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> { Some(self) }
}
```

> Verify the `content_surface(SURFACE_ROLES, focused, selectable)` signature and that `SurfaceRoles`/`Role::Input*` are re-exported from `tvision_rs` (they are used by `input_line.rs`). If `content_surface`'s "focused" branch is not visually strong enough for a whole-block highlight, fill with `ctx.style(Role::InputSelected)` when `self.state.state.focused`.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -j4 --lib ui::panes::launch_view 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/ui/panes/launch_view.rs src/ui/panes/mod.rs
git commit -m "feat(ui): LaunchValueView read-only value block with activate-on-keypress"
```

---

### Task 3: Present helpers — bullet lines & `<not set>`

Centralise how a field's values become display lines, so both `LaunchValueView` (Stage 1) and `ListValueView` (Stage 2) format identically.

**Files:**
- Create: `src/ui/panes/value_lines.rs`
- Modify: `src/ui/panes/mod.rs` (`mod value_lines;`)
- Test: inline `#[cfg(test)]`

**Interfaces:**
- Produces:
  - `pub(crate) const NOT_SET: &str = "<not set>";`
  - `pub(crate) fn bullet_lines(values: &[String], strip_ordering: bool) -> Vec<String>` — for each non-empty value, first display line `"- "` + first text line, continuation lines (`\n`-split) indented two spaces; empty set → `vec![NOT_SET]`. When `strip_ordering`, apply `crate::ui::ordered::strip_ordering` to each value first.
  - `pub(crate) fn masked_line() -> Vec<String>` → `vec!["*****".to_string()]`.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_not_set() {
        assert_eq!(bullet_lines(&[], false), vec![NOT_SET.to_string()]);
        assert_eq!(bullet_lines(&["   ".into()], false), vec![NOT_SET.to_string()]);
    }

    #[test]
    fn values_render_as_bullets() {
        let v = vec!["a".to_string(), "b".to_string()];
        assert_eq!(bullet_lines(&v, false), vec!["- a".to_string(), "- b".to_string()]);
    }

    #[test]
    fn newline_becomes_indented_continuation() {
        let v = vec!["b\ncont".to_string()];
        assert_eq!(bullet_lines(&v, false), vec!["- b".to_string(), "  cont".to_string()]);
    }

    #[test]
    fn ordering_prefix_stripped_when_requested() {
        let v = vec!["{0}read".to_string(), "{1}write".to_string()];
        assert_eq!(bullet_lines(&v, true), vec!["- read".to_string(), "- write".to_string()]);
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -j4 --lib ui::panes::value_lines 2>&1 | tail -20`
Expected: FAIL — module not found.

- [ ] **Step 3: Implement**

```rust
//! Shared formatting of a field's values into display rows (bulleted list,
//! `<not set>`, `*****`). Used by the read-only launch block and the inline list
//! editor so both look identical.

pub(crate) const NOT_SET: &str = "<not set>";

pub(crate) fn bullet_lines(values: &[String], strip_ordering: bool) -> Vec<String> {
    let cleaned: Vec<String> = values
        .iter()
        .map(|v| {
            if strip_ordering {
                crate::ui::ordered::strip_ordering(v).to_string()
            } else {
                v.clone()
            }
        })
        .filter(|v| !v.trim().is_empty())
        .collect();
    if cleaned.is_empty() {
        return vec![NOT_SET.to_string()];
    }
    let mut out = Vec::new();
    for v in &cleaned {
        for (i, line) in v.split('\n').enumerate() {
            out.push(if i == 0 { format!("- {line}") } else { format!("  {line}") });
        }
    }
    out
}

pub(crate) fn masked_line() -> Vec<String> {
    vec!["*****".to_string()]
}
```

> `strip_ordering` is currently `pub(crate)` in `src/ui/ordered.rs:12` — confirm visibility reaches here (same crate). If it is not `pub(crate)`, widen it.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -j4 --lib ui::panes::value_lines 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/ui/panes/value_lines.rs src/ui/panes/mod.rs
git commit -m "feat(ui): shared bullet-line / not-set / masked value formatting"
```

---

### Task 4: Form pane — variable-height block layout

Rework `rebuild_cells`/`render`/navigation so each field is one composite block. Single-value → `InputLine` (as today); everything else → `LaunchValueView` for now (Stage 2 swaps `List` fields to the inline editor). This is the biggest structural task.

**Files:**
- Modify: `src/ui/panes/form.rs` (struct + `rebuild_cells` 217-285, `render` 289-393, `handle_event` nav 719-726, `focus_field`/`value_id_for_label_hit`, add `layout_blocks`)
- Test: extend inline form tests

**Interfaces:**
- Consumes: `value_kind`, `block_height` (Task 1); `LaunchValueView` (Task 2); `bullet_lines`/`masked_line` (Task 3).
- Produces (form-pane internal): `kinds: Vec<ValueKind>` field (parallel to `value_ids`); `block_tops: Vec<i32>`; `fn layout_blocks(&mut self, heights: &[i32])` positioning label at each block's first row and the value view over the whole block; a `help_ctx_for(kind, field)` returning the per-field `HelpCtx`.

- [ ] **Step 1: Add the parallel state and layout helper (structural, compile-first)**

Add to the struct (`form.rs:46-70`):

```rust
    kinds: Vec<ValueKind>,      // value-view kind per field, parallel to value_ids
    block_tops: Vec<i32>,       // y of each field block's first row, parallel to value_ids
```

Add the layout helper (positions labels + value views from a heights slice):

```rust
    /// Position every field block: label on the block's first row (right-aligned
    /// column), value view spanning the whole block height. Returns total content
    /// height. Callers pass per-field heights computed from `block_height`.
    fn layout_blocks(&mut self, label_w: i32, inner_w: i32, heights: &[i32]) -> i32 {
        let mut y = 0;
        let mut tops = Vec::with_capacity(heights.len());
        let (lids, vids) = (self.label_ids.clone(), self.value_ids.clone());
        if let Some(sg) = self.scroll_mut() {
            for (i, &h) in heights.iter().enumerate() {
                tops.push(y);
                if let Some(&lid) = lids.get(i) {
                    if let Some(l) = sg.child_mut(lid) {
                        l.change_bounds(Rect::new(0, y, label_w, y + 1));
                    }
                }
                if let Some(&vid) = vids.get(i) {
                    if let Some(v) = sg.child_mut(vid) {
                        v.change_bounds(Rect::new(label_w, y, inner_w, y + h));
                    }
                }
                y += h;
            }
        }
        self.block_tops = tops;
        y
    }
```

- [ ] **Step 2: Rewrite `rebuild_cells` to insert the right view per field**

Replace the per-field loop (`form.rs:253-269`) so it builds one label + one value view per field, chosen by `value_kind`, at heights from `block_height`:

```rust
        // Per field: classify, compute height, insert label + the right value view.
        let kinds: Vec<ValueKind> = fields.iter().map(|(f, _)| value_kind(f)).collect();
        let heights: Vec<i32> =
            fields.iter().zip(&kinds).map(|((f, _), k)| block_height(f, *k)).collect();
        {
            let Some(sg) = self.scroll_mut() else { return };
            sg.clear_content(ctx);
            let w = sg.inner_width();
            inner_w = w;
            let longest = /* unchanged longest-label computation */;
            label_w = label_col_width(longest, w);
            for (i, ((f, editable), kind)) in fields.iter().zip(&kinds).enumerate() {
                let lid = sg.add_content(
                    Box::new(FieldLabel::label(Rect::new(0, 0, label_w, 1))),
                    Rect::new(0, 0, label_w, 1),
                );
                let hctx = help_ctx_for(*kind, f);
                let vid = match kind {
                    ValueKind::Text => {
                        let mut il = InputLine::with_limit(Rect::new(0, 0, w, 1), 1024);
                        il.state.state.disabled = !editable;
                        il.state_mut().help_ctx = hctx; // footer follows single-value fields too
                        sg.add_content(Box::new(il), Rect::new(0, 0, w, 1))
                    }
                    ValueKind::List { .. } | ValueKind::Launch => {
                        // Stage 1: List fields also use LaunchValueView (read-only bullets
                        // + existing modal). Stage 2 replaces the List arm with ListValueView.
                        let v = LaunchValueView::new(Rect::new(0, 0, w, 1), hctx);
                        sg.add_content(Box::new(v), Rect::new(0, 0, w, 1))
                    }
                };
                new_lids.push(lid);
                new_vids.push(vid);
            }
        }
        self.label_ids = new_lids;
        self.value_ids = new_vids;
        self.kinds = kinds;
        self.label_w = label_w;
        self.built_w = inner_w;
        let total_h = self.layout_blocks(label_w, inner_w, &heights);
        // Ensure the ScrollGroup content extent covers the stacked blocks.
        if let Some(sg) = self.scroll_mut() { sg.set_content_height(total_h); } // confirm method name
```

> The metadata collection (`form.rs:219-232`) must now yield `(EditField, bool)` (clone the field or the fields it needs) so `value_kind`/`block_height`/`help_ctx_for` can run outside the borrow. Confirm `ScrollGroup` has a set-content-height / extent API (`scroll_group.rs`); if content height is derived from children, dropping this call is fine.

- [ ] **Step 3: Rewrite `render` to feed each view and relayout on height change**

In `render` (`form.rs:320-367`), replace the single value push with a per-kind push, recompute heights, and relayout if any changed:

```rust
        // Push content into each value view by kind; collect fresh heights.
        let (kinds, label_w, inner_w) = (self.kinds.clone(), self.label_w, self.built_w);
        let mut heights = Vec::with_capacity(kinds.len());
        {
            let st = self.state.borrow();
            let form = st.edit_form.as_ref();
            let (label_ids, value_ids) = (self.label_ids.clone(), self.value_ids.clone());
            if let Some(sg) = self.scroll_mut() {
                for (i, kind) in kinds.iter().enumerate() {
                    let field = form.and_then(|f| f.fields.get(i));
                    let Some(field) = field else { continue };
                    heights.push(block_height(field, *kind));
                    if let Some(&lid) = label_ids.get(i) {
                        if let Some(l) = sg.child_mut(lid) {
                            let marker = if field.must { "*" } else { "" };
                            l.set_value(FieldValue::Text(format!("{}{}", field.label, marker)));
                        }
                    }
                    if let Some(&vid) = value_ids.get(i) {
                        if let Some(v) = sg.child_mut(vid) {
                            match kind {
                                ValueKind::Text => {
                                    let s = field.values.first().cloned().unwrap_or_default();
                                    v.set_value(FieldValue::Text(s));
                                    v.state_mut().state.disabled = !cell_focusable(field);
                                }
                                ValueKind::Launch => {
                                    if let Some(lv) = v.as_any_mut()
                                        .and_then(|a| a.downcast_mut::<LaunchValueView>())
                                    {
                                        lv.set_lines(launch_lines(field));
                                    }
                                }
                                ValueKind::List { ordered } => {
                                    // Stage 1: still a LaunchValueView showing bullets.
                                    if let Some(lv) = v.as_any_mut()
                                        .and_then(|a| a.downcast_mut::<LaunchValueView>())
                                    {
                                        lv.set_lines(bullet_lines(&field.values, *ordered));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        } // borrow dropped
        if heights != self.block_tops_to_heights() /* or track prev heights */ {
            self.layout_blocks(label_w, inner_w, &heights);
        }
```

Add a `launch_lines(field)` free fn: masked for password (`field.secret`), single line for single-value launch (`present_field` or first value), bullets for multi. Keep it small and unit-tested alongside Task 3 if convenient.

> Height-change detection: store `block_heights: Vec<i32>` on the struct alongside `block_tops` and compare, rather than reconstructing from tops. Adjust the struct accordingly.

- [ ] **Step 4: Navigation — delegate-then-bubble for Up/Down**

Change the nav arm (`form.rs:719-726`). For `Text`/`Launch` focused kinds, keep the current behaviour (Up/Down → `focus_field(±1)`). Foundation for Stage 2: route the raw event to the focused view first only for `List` kinds (none exist yet in Stage 1, so behaviour is unchanged):

```rust
        let nav_updown = matches!(ev, Event::KeyDown(k) if matches!(k.key, Key::Up | Key::Down));
        if nav_updown {
            let down = matches!(ev, Event::KeyDown(k) if k.key == Key::Down);
            match self.focused_kind() {
                Some(ValueKind::List { .. }) => {
                    // Stage 2 wires intra-list movement + boundary bubble here.
                    self.focus_field(if down { 1 } else { -1 }, ctx);
                    ev.clear();
                }
                _ => {
                    self.focus_field(if down { 1 } else { -1 }, ctx);
                    ev.clear();
                }
            }
        } else if /* action key on a Launch field */ self.focused_kind() == Some(ValueKind::Launch)
            && matches!(ev, Event::KeyDown(_))
        {
            self.group.handle_event(ev, ctx); // LaunchValueView sets its activate flag
            if let Some(true) = self.focused_launch_take_activate() {
                if let Some(idx) = self.focused_field_idx() {
                    self.state.borrow_mut().activate_field = Some(idx);
                    ctx.post(ACTIVATE);
                }
            }
        } else {
            self.group.handle_event(ev, ctx);
        }
```

Add helpers `focused_kind() -> Option<ValueKind>` (from `focused_field_idx` + `self.kinds`) and `focused_launch_take_activate() -> Option<bool>` (downcast the focused child to `LaunchValueView` and `take_activate`). The old Enter/typing modal-launch arms (`form.rs:663-681`) are **replaced** by this LaunchValueView path — remove them.

- [ ] **Step 5: Fix `value_id_for_label_hit` for multi-row labels**

Labels now sit on the block's first row; `local_bounds_of(label_id)` already reflects the new bounds, so `value_id_for_label_hit` (`form.rs:477-503`) needs no change beyond confirming it maps a click to the correct paired `value_id`. Clicking a value block still focuses that block. Verify by test.

- [ ] **Step 6: Update/extend tests**

Adjust existing form tests that assumed one row per field (they index `value_ids` by field, which still holds — one value view per field — so most survive). Add:

```rust
#[test]
fn multi_value_field_block_is_multiple_rows_tall() {
    // Build a form with a 3-value plain multi field; assert block_tops increments
    // by the field's block_height and the next field starts below it.
}

#[test]
fn action_key_on_launch_field_posts_activate() {
    // Focus an objectClass (Launch) field, send Char('x'), assert
    // state.activate_field == Some(idx).
}
```

- [ ] **Step 7: Run the form pane tests + clippy**

Run: `cargo test -j4 --lib ui::panes::form 2>&1 | tail -25` → PASS
Run: `cargo clippy --all-targets -j4 -- -D warnings 2>&1 | tail -5` → clean

- [ ] **Step 8: Manual smoke check**

Run the demo (`scripts/test-ldap.sh start`; `EDAPTOR_TEST_ADMIN_PW=adminpassword cargo run -- --config examples/demo-config.toml`). Confirm: multi-value fields show bullets; empty fields show `<not set>`; objectClass/membership/password show read-only blocks that open their modal on a keypress; single-value fields edit in place; Up/Down move between fields.

- [ ] **Step 9: Commit**

```bash
git add src/ui/panes/form.rs
git commit -m "feat(ui): variable-height field blocks; modal fields render as read-only bullet blocks"
```

---

### Task 5: CHANGES + docs for Stage 1

**Files:**
- Modify: `CHANGES.md`, `docs/src/configuration/widgets.md` (or the entry-editing page)

- [ ] **Step 1:** Add a `CHANGES.md` entry under Unreleased → Changed describing the inline bulleted rendering and highlight-then-launch behaviour; **remove** the interim `<press ENTER to add Value(s)>` line added in `88b8032` (empty state is now `<not set>`).
- [ ] **Step 2:** Update the mdBook page describing how multi-value fields display/edit.
- [ ] **Step 3:** `make docs` builds cleanly.
- [ ] **Step 4:** Commit `docs: describe inline multi-value rendering (stage 1)`.

---

## STAGE 2 — Inline `ListValueView` editor

Replaces the `List` fields' read-only block + modal with true in-place editing.

### Task 6: `ListModel` — pure list-of-values editor core

The heart of the feature: a `Vec<String>` of values (each may contain `\n`), a cursor, and every edit operation, with zero `Context`/`View` coupling.

**Files:**
- Create: `src/ui/panes/list_model.rs`
- Modify: `src/ui/panes/mod.rs` (`mod list_model;`)
- Test: inline `#[cfg(test)]` (the bulk of the coverage)

**Interfaces:**
- Produces:
  - `pub(crate) struct ListModel { items: Vec<String>, item: usize, off: usize /* byte offset into items[item] */, on_handle: bool }`
  - `pub(crate) fn from_values(values: &[String], strip_ordering: bool) -> Self`
  - `pub(crate) fn to_values(&self, reconstruct_ordering: bool) -> Vec<String>` — trim each item, drop empties; when `reconstruct_ordering`, pass survivors through `crate::ui::ordered::reconstruct`.
  - `pub(crate) fn is_empty(&self) -> bool` (no non-blank items)
  - Cursor moves: `left`, `right`, `up`, `down` each returning `Move` = `Moved | Boundary` so the view knows when to bubble field navigation.
  - Edits: `insert_char(char)`, `enter()`, `newline()` (Ctrl-Enter), `backspace()`, `delete()`.
  - Reorder (ordered only): `move_item(dir: i32)`, and handle mode: `enter_handle()`, `leave_handle()`, `on_handle()`.
  - Display: `display_lines(&self) -> Vec<String>` (reuses `value_lines::bullet_lines`-style formatting, but with the live handle marker), and `cursor_xy(&self) -> (i32, i32)` mapping the cursor to a display column/row.

- [ ] **Step 1: Write failing tests** — cover every operation. (Representative subset; write all.)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn m(vals: &[&str]) -> ListModel {
        ListModel::from_values(&vals.iter().map(|s| s.to_string()).collect::<Vec<_>>(), false)
    }

    #[test]
    fn empty_model_is_not_set() {
        let m = ListModel::from_values(&[], false);
        assert!(m.is_empty());
        assert_eq!(m.display_lines(), vec!["<not set>".to_string()]);
    }

    #[test]
    fn typing_into_empty_creates_first_item() {
        let mut m = ListModel::from_values(&[], false);
        m.insert_char('a');
        assert_eq!(m.to_values(false), vec!["a".to_string()]);
        assert_eq!(m.display_lines(), vec!["- a".to_string()]);
    }

    #[test]
    fn enter_splits_current_item() {
        let mut m = m(&["ab"]);
        m.right(); // cursor after 'a'
        m.enter();
        assert_eq!(m.to_values(false), vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn ctrl_enter_inserts_newline_within_item() {
        let mut m = m(&["ab"]);
        m.right();
        m.newline();
        assert_eq!(m.to_values(false), vec!["a\nb".to_string()]);
        assert_eq!(m.display_lines(), vec!["- a".to_string(), "  b".to_string()]);
    }

    #[test]
    fn backspace_at_item_start_merges_into_previous() {
        let mut m = m(&["a", "b"]);
        m.down(); // to item 1, offset 0 (home)
        m.backspace();
        assert_eq!(m.to_values(false), vec!["ab".to_string()]);
    }

    #[test]
    fn emptying_item_then_backspace_removes_marker() {
        let mut m = m(&["a", "x", "c"]);
        m.down(); // item 1
        m.backspace(); // delete 'x' -> item 1 empty
        m.backspace(); // remove empty item, merge
        assert_eq!(m.to_values(false), vec!["a".to_string(), "c".to_string()]);
    }

    #[test]
    fn removing_last_item_reverts_to_not_set() {
        let mut m = m(&["only"]);
        for _ in 0..4 { m.backspace(); } // delete 'ynlo'... reach empty
        // one more backspace on empty item 0 keeps it empty (no prev)
        assert!(m.is_empty());
        assert_eq!(m.display_lines(), vec!["<not set>".to_string()]);
    }

    #[test]
    fn up_down_report_boundary_at_edges() {
        let mut m = m(&["a", "b"]);
        assert_eq!(m.up(), Move::Boundary);   // already at top
        assert_eq!(m.down(), Move::Moved);
        assert_eq!(m.down(), Move::Boundary);  // at bottom
    }

    #[test]
    fn move_item_reorders() {
        let mut m = m(&["a", "b", "c"]);
        m.down(); // item 1 = "b"
        m.move_item(1); // b down
        assert_eq!(m.to_values(false), vec!["a".to_string(), "c".to_string(), "b".to_string()]);
    }

    #[test]
    fn to_values_reconstructs_ordering_prefixes() {
        let mut m = ListModel::from_values(
            &["{0}read".to_string(), "{1}write".to_string()], true);
        m.move_item(1); // pointless at item0 top? adjust: assert renumber on reorder
        assert_eq!(m.to_values(true), vec!["{0}read".to_string(), "{1}write".to_string()]);
    }

    #[test]
    fn left_at_start_enters_handle_then_right_leaves() {
        let mut m = m(&["a"]);
        assert_eq!(m.left(), Move::Moved); // onto handle
        assert!(m.on_handle());
        m.right();
        assert!(!m.on_handle());
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -j4 --lib ui::panes::list_model 2>&1 | tail -25`
Expected: FAIL — module not found.

- [ ] **Step 3: Implement `ListModel`**

Implement the struct and ops. Use `crate::text`-equivalent grapheme stepping via tvision's `text::next`/`text::prev` for `left`/`right`/`backspace`/`delete` on the current item's bytes (mirror `input_line.rs:635-649`). Key rules:
- `from_values`: strip ordering prefixes into `items` when `strip_ordering`; drop nothing here (keep raw for round-trip), cursor at item 0 offset 0.
- `insert_char`: if `is_empty()`, create `items = [String::new()]`, item 0; insert the char at `off`, advance `off`.
- `enter`: split `items[item]` at `off` into two items; cursor to start of the new second item.
- `newline`: insert `'\n'` at `off`; advance `off`.
- `backspace`: if `off > 0`, delete the previous grapheme; else if `item > 0`, set `off = len(items[item-1])`, append `items[item]` to `items[item-1]`, remove `items[item]`, `item -= 1`. If `item == 0 && off == 0`, no-op.
- `delete`: if `off < len`, delete the next grapheme; else if `item < last`, append `items[item+1]` and remove it.
- `up`/`down`: move between **display lines** (continuation-aware): compute the current display row; if a previous/next display row exists **within** the model, move the cursor there (map column back to a byte offset, clamped) and return `Moved`; at the very top/bottom display row return `Boundary`.
- `move_item(dir)`: swap `items[item]` with neighbour; move `item` with it; clamp.
- handle mode (`on_handle`): `left()` at `off==0` sets `on_handle=true` (returns `Moved`); while `on_handle`, `up`/`down` call `move_item(-1|+1)` and return `Moved` (or `Boundary` at ends); `right()` or any edit clears `on_handle`.
- `display_lines`: like `value_lines::bullet_lines` on `items` (already stripped), but when `on_handle`, render the current item's bullet as the **wide hamburger `≡`** instead of `-`.
- `cursor_xy`: return `(col, row)` in display space; when `on_handle`, col = 0 (the marker cell).

> Keep `Move` a small `#[derive(PartialEq)] enum { Moved, Boundary }`. Reuse `value_lines` formatting where possible to avoid divergence (extract a shared `format_item(idx, text, is_handle)` if cleaner).

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -j4 --lib ui::panes::list_model 2>&1 | tail -25`
Expected: PASS (all ops). Fix until green.

- [ ] **Step 5: Commit**

```bash
git add src/ui/panes/list_model.rs src/ui/panes/mod.rs
git commit -m "feat(ui): pure ListModel — inline multi-value editing core"
```

---

### Task 7: `ListValueView` widget

Wrap `ListModel` in a `View`: draw the bullets + cursor, handle keys, expose height + a boundary-exit signal + `to_values` for the pane to sync.

**Files:**
- Create: `src/ui/panes/list_view.rs`
- Modify: `src/ui/panes/mod.rs`
- Test: inline `#[cfg(test)]` for the key-classification/height/boundary logic (pure `on_key` like Task 2)

**Interfaces:**
- Produces:
  - `pub(crate) struct ListValueView` (holds `ListModel`, `ViewState`, `ordered: bool`, `boundary_exit: Option<i32>`)
  - `pub(crate) fn new(bounds: Rect, values: &[String], ordered: bool, help_ctx_body: HelpCtx, help_ctx_handle: HelpCtx) -> Self`
  - `pub(crate) fn line_count(&self) -> i32`
  - `pub(crate) fn to_values(&self) -> Vec<String>` (delegates to `ListModel::to_values(self.ordered)`)
  - `pub(crate) fn take_boundary_exit(&mut self) -> Option<i32>` (−1 up / +1 down when an Up/Down hit the model edge)
  - `pub(crate) fn resync(&mut self, values: &[String])` (rebuild model from external values; used by `render`)
  - Sets `state.help_ctx` to the body/handle context depending on `model.on_handle()` each event so the footer switches when the cursor is on the handle.

- [ ] **Step 1: Write failing tests** (pure key handling)

```rust
#[test]
fn down_at_bottom_sets_boundary_exit() {
    let mut v = ListValueView::new(Rect::new(0,0,20,2), &["a".into(),"b".into()], false, body(), handle());
    // move to last line then down
    v.on_key(&mut key(Key::Down));
    v.on_key(&mut key(Key::Down));
    assert_eq!(v.take_boundary_exit(), Some(1));
}

#[test]
fn enter_adds_item_and_grows_line_count() {
    let mut v = ListValueView::new(Rect::new(0,0,20,1), &["a".into()], false, body(), handle());
    assert_eq!(v.line_count(), 1);
    v.on_key(&mut key(Key::End));
    v.on_key(&mut key(Key::Enter));
    assert_eq!(v.line_count(), 2);
    assert_eq!(v.to_values(), vec!["a".to_string()]); // trailing empty dropped by to_values
}
```

(Provide `body()`/`handle()`/`key()` helpers; confirm the `Ctrl+Enter` event shape — check how the crate encodes modified Enter, likely `KeyDown{ key: Key::Enter, ctrl: true }` or a control code; mirror how `multivalue.rs` reads keys.)

- [ ] **Step 2: Run → FAIL.** `cargo test -j4 --lib ui::panes::list_view 2>&1 | tail -20`

- [ ] **Step 3: Implement `ListValueView`.** Model on `LaunchValueView` (Task 2) for the `View` skeleton, plus:
  - `on_key`: map keys to `ListModel` ops. Printable→`insert_char`; Enter→`enter`; Ctrl+Enter→`newline`; Backspace→`backspace`; Delete→`delete`; Left/Right→`left`/`right`; Home/End→jump within item; Up/Down→`up`/`down`, and when the result is `Move::Boundary` set `self.boundary_exit = Some(-1|1)` and **leave `ev` unconsumed**; Ctrl+Up/Down (ordered only)→`move_item`. Consume every edit/handled key with `ev.clear()`.
  - After each event, set `state.help_ctx` = handle-context if `model.on_handle()` else body-context.
  - `draw`: fill focused surface; draw `model.display_lines()`; set the text cursor via `state.set_cursor(col, row)` + `show_cursor()` from `model.cursor_xy()`; `cursor_request` returns `Some(cursor)` when focused.
  - `line_count` = `model.display_lines().len()`.

- [ ] **Step 4: Run → PASS.**

- [ ] **Step 5: Commit** `feat(ui): ListValueView — in-place bullet list editor view`.

---

### Task 8: Wire `List` fields to `ListValueView` + dynamic relayout + sync

**Files:**
- Modify: `src/ui/panes/form.rs`

**Interfaces:**
- Consumes: `ListValueView` (Task 7).

- [ ] **Step 1:** In `rebuild_cells` (Task 4 loop), replace the Stage-1 `List` arm (which built a `LaunchValueView`) with a `ListValueView::new(bounds, &field.values, ordered, help_ctx_body, help_ctx_handle)`.
- [ ] **Step 2:** In `render`, replace the `List` push: call `downcast_mut::<ListValueView>()` → `resync(&field.values)` **only** when the form was externally reset (guard so live edits are not clobbered — resync on `form_needs_render` ticks is safe because those fire only on external change, never mid-typing; matches the existing InputLine contract at `form.rs:388-391`).
- [ ] **Step 3:** In `handle_event` nav arm (Task 4 Step 4), implement the `List` branch: route the raw Up/Down to `self.group.handle_event(ev, ctx)`; then `if let Some(dir) = self.focused_list_take_boundary_exit() { self.focus_field(dir, ctx); }`. For all other keys on a focused `List` field, route to the group (the view edits) and then, **if `line_count` changed**, recompute heights and `layout_blocks` so the block grows/shrinks live and following blocks shift.
- [ ] **Step 4:** In `sync_into_form` (`form.rs:527-578`), extend the pull: for `List` fields, `downcast_ref::<ListValueView>()` → `to_values()` and write `form.fields[i].values = vals` (do **not** set `form_needs_render`). Keep the existing `InputLine` pull for `Text` fields.
- [ ] **Step 5:** Add helper `focused_list_take_boundary_exit(&mut self) -> Option<i32>`.
- [ ] **Step 6:** Tests: `list_field_grows_block_when_item_added` (focus a List field, send End+Enter, assert the next field's `block_tops` moved down by 1); `down_past_list_bottom_moves_to_next_field`.
- [ ] **Step 7:** `cargo test -j4 --lib ui::panes::form` → PASS; clippy clean.
- [ ] **Step 8:** Manual smoke: edit `mail` inline — Enter adds a value, Ctrl+Enter adds a wrapped line, Backspace on an emptied item removes it, last removal → `<not set>`; on `olcAccess` test Ctrl+↑/↓ and the ← -to-`≡`-handle reorder.
- [ ] **Step 9:** Commit `feat(ui): inline editing for free-text and ordered multi-value fields`.

---

### Task 9: Retire the free-text/ordered modals

**Files:**
- Modify: `src/ui/multivalue.rs`, `src/ui/ordered.rs`, `src/ui/widget.rs`, `src/ui/app.rs`

- [ ] **Step 1:** `widget_for`/`is_modal_field`: `MultiValueWidget` and `OrderedWidget` no longer `activate` into a modal — their fields are `ValueKind::List` and never post `ACTIVATE`. Keep the widgets' `present` (still used by `present_field` fallbacks?) or delete if unused after Stage 1/2. Verify no caller of `MultiValueEditor`/`OrderedEditor` remains.
- [ ] **Step 2:** Delete `MultiValueEditor`/`MultiValueDialog` and `OrderedEditor`/`OrderedDialog` (the dialog + Add/Del buttons `CMD_MV_ADD`/`CMD_MV_DEL`), keeping `strip_ordering`/`reconstruct` (now used by `ListModel`/`value_lines`). Move those two helpers to a small `src/ui/ordered.rs` core or leave the file with just them + tests.
- [ ] **Step 3:** Remove now-dead tests referencing the deleted dialogs; keep `strip_ordering`/`reconstruct` tests.
- [ ] **Step 4:** `cargo test -j4` and `cargo clippy --all-targets -j4 -- -D warnings` clean (no dead-code warnings).
- [ ] **Step 5:** Commit `refactor(ui): remove free-text/ordered modal editors, superseded by inline editing`.

---

## STAGE 3 — Dynamic footer hints

### Task 10: Per-field help contexts + StatusLine hints

**Files:**
- Modify: `src/ui/app.rs` (`init_status_line` 21-32), `src/ui/panes/form.rs` (`help_ctx_for`), a new `src/ui/help_ctx.rs` for the `HelpCtx` constants.

**Interfaces:**
- Produces: `HelpCtx` constants — `FIELD_TEXT`, `FIELD_LIST`, `FIELD_LIST_ORDERED`, `FIELD_LIST_HANDLE`, `FIELD_LAUNCH_PICKER`, `FIELD_LAUNCH_PASSWORD` (each `HelpCtx::custom("edaptor.field.…")`).

- [ ] **Step 1:** Define the constants; make `help_ctx_for(kind, field)` return the right one (password → `FIELD_LAUNCH_PASSWORD` when `field.secret`; ordered list → `FIELD_LIST_ORDERED`; etc.). `ListValueView` swaps between body/handle contexts on `on_handle` (Task 7 Step 3).
- [ ] **Step 2:** Extend `init_status_line` to add context defs. Keep the global `Alt-N/Alt-S/Alt-X` via `def_all`; add per-context hint text via `StatusLine::with_hint(|ctx| match ctx { … })` returning the hint strings from the spec table. (Use the hint tail rather than swapping the whole item row, so the global actions stay visible.)

```rust
let hint = |ctx: HelpCtx| -> Option<String> {
    Some(match ctx.name() {
        "edaptor.field.text" => "↑↓ move · Enter next field",
        "edaptor.field.list" => "Enter add · Ctrl-Enter newline · Backspace empties→removes · ↑↓ move",
        "edaptor.field.list.ordered" => "Enter add · Ctrl-Enter newline · Ctrl-↑↓ or ← handle to reorder",
        "edaptor.field.list.handle" => "↑↓ reorder · → back to text",
        "edaptor.field.launch.picker" => "any key: open picker · ↑↓ move",
        "edaptor.field.launch.password" => "any key: edit password",
        _ => return None,
    }.to_string())
};
Some(Box::new(StatusLine::new(r, defs).with_hint(hint)))
```

- [ ] **Step 3:** Confirm auto-propagation end-to-end: the footer text changes as focus moves between fields and when the cursor lands on the `≡` handle (no manual `set_help_ctx` call needed — the Program idle loop reads the focused leaf's `help_ctx`).
- [ ] **Step 4:** Test: a focused-field helper asserting the pane's focused value view returns the expected `help_ctx` for each kind (unit-level, since the Program loop is integration-only).
- [ ] **Step 5:** Manual smoke: tab through fields and watch the footer hint change; move onto an ordered handle and see the reorder hint.
- [ ] **Step 6:** Commit `feat(ui): dynamic footer hints driven by the focused field's help context`.

---

### Task 11: Final docs + full verification

**Files:**
- Modify: `CHANGES.md`, `README.md`, `docs/src/…`

- [ ] **Step 1:** `CHANGES.md`: consolidate the Stage 1–3 entries into a clear user-facing description (inline bulleted editing, reorder handle, footer hints).
- [ ] **Step 2:** README: ensure the short overview mentions inline multi-value editing and points to the mdBook; do not restate detail.
- [ ] **Step 3:** mdBook: a page/section documenting the inline editor keys (Enter/Ctrl-Enter/Backspace/reorder) and the launch fields, with the footer-hint reference.
- [ ] **Step 4:** `make check` (fmt + clippy + tests) → all green. `make docs` → builds.
- [ ] **Step 5:** Commit `docs: document inline multi-value editing, reorder handle, footer hints`.

---

## Self-Review

**Spec coverage:**
- Bulleted rendering + variable height → Tasks 1, 3, 4. ✓
- Single-value unchanged → Task 1 (`Text`), Task 4 (InputLine arm). ✓
- Inline free-text editing (Enter/Ctrl-Enter/Backspace-removes/`<not set>`) → Tasks 6, 7, 8. ✓
- Ordered reorder (Ctrl-↑/↓ **and** ← -to-`≡`-handle) → Task 6 (`move_item`, handle mode), Task 7, Task 8. ✓
- Launch fields (objectClass/membership/choice/picker/password) highlight + action-key launch → Tasks 2, 4. ✓
- Password `*****` → Task 3 (`masked_line`), Task 4 (`launch_lines`). ✓
- Commit path unchanged / feeds `EditField.values` → Task 8 Step 4 (`sync_into_form`), Task 4 (Launch still uses `apply_commit`). ✓
- Retire free-text/ordered modals → Task 9. ✓
- Dynamic footer hints via help-context → Task 10. ✓
- `<not set>` supersedes `<press ENTER…>` → Task 5 Step 1. ✓

**Placeholder scan:** Two `render`/`rebuild_cells` snippets use `/* unchanged … */` markers for the longest-label computation and note "confirm method name" for `set_content_height` — these point at *existing* code to preserve and one API to verify, not unwritten logic. All new logic (classification, model ops, view key-handling, hint mapping) has complete code or exhaustively enumerated rules.

**Type consistency:** `ValueKind` used identically across Tasks 1/4/8/10. `ListModel::to_values(reconstruct: bool)` and `ListValueView::to_values()` (which passes `self.ordered`) are consistent. `take_boundary_exit`/`take_activate` naming consistent between Tasks 2 and 7. `bullet_lines(values, strip_ordering)` signature stable across Tasks 3/4/6.

**Open verification items for the implementer (flagged, not gaps):** exact `KeyDown`/`Ctrl+Enter` event construction in this tvision version; `ScrollGroup` content-height API; whether `content_surface`'s focused branch reads strongly enough as a whole-block highlight (fallback: `Role::InputSelected` fill). Each has a stated fallback.
