# M5c — The Three Reconciliations Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make X-ORDERED multi-valued attributes editable in the tvision UI, add schema-aware client-side last-member pre-validation for membership saves, and restore live `sambaDomain` discovery at startup.

**Architecture:** Three independent reconciliations carried forward from the tvision migration. Each reuses an existing seam: (1) a new dedicated modal editor `src/ui/ordered.rs` routed through `widget_for`, owning the `{n}` strip-on-display / reconstruct-on-commit so the neutral diff layer is untouched; (2) a selective, schema-gated populate of the `group_members` map that already feeds the locked `submit_combined` guard, fed by a blocking group fetch; (3) a ported `discover_samba_domain` blocking search wired into `bootstrap` with config fallback.

**Tech Stack:** Rust, tvision-rs 0.3.x, ldap3 (via `crate::ldap::worker`), the neutral `workflows`/`form`/`schema` layers.

## Global Constraints

- **Cap build/test parallelism at 4 cores** (shared machine): `cargo test -j4`, `cargo clippy -j4 --all-targets -- -D warnings`. Target dir is `/home/oetiker/scratch/cargo-target` (the binary is there, NOT `./target`).
- **Facade boundary:** only `src/ui/**` may `use tvision_rs`. The domain layer (`config`/`form`/`ldap`/`schema`/`samba`/`workflows`) imports NEITHER tvision_rs NOR any ratatui/tui_* crate. Guards must print nothing:
  - `! grep -rl "use tvision_rs" src | grep -vE "^src/ui/"`
  - `! grep -rl "use ratatui\|use tui_" src`
- **Borrow discipline:** never hold a `RefCell`/`UiState` borrow across `ctx.broadcast`/`ctx.post`/`Program::exec_view*`/`worker.submit`/`worker.request`/`new_list`/`child_mut`/`set_value`. Collect into locals → drop the borrow → call.
- **Strict TDD**, atomic commits, crate compiles after every commit, `cargo fmt` before each commit, clippy `--all-targets -D warnings` clean.
- **Activation stays `{Inline, Modal}`** — no `Immediate` variant.
- **Commit trailer:** `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. Use `git commit -F <file>` (or heredoc) for messages containing backticks.
- **Docs one-home:** config detail → mdBook (`docs/src/`); `CHANGES.md` for every user-visible change.

---

## File Structure

- **Create** `src/ui/ordered.rs` — the dedicated X-ORDERED modal editor (`OrderedWidget` / `OrderedEditor` / `OrderedDialog`) plus the two pure helpers `strip_ordering` / `reconstruct`. Owns the entire `{n}` concern. (Part A)
- **Modify** `src/ui/mod.rs` — declare the new `ordered` module. (Part A)
- **Modify** `src/ui/widget.rs` — add the `XOrdered` arm to `widget_for` and `is_modal_field`. (Part A)
- **Modify** `examples/demo-config.toml` + `docs/src/configuration/widgets.md` — a demo x_ordered binding + flip the "editable" claim. (Part A)
- **Modify** `src/workflows/save.rs` — add pure `membership_attr_is_must` and `last_member_block`. (Part B)
- **Modify** `src/workflows/write_flow.rs` — add `fetch_group_members_for_must`; refactor `submit_combined` to call `last_member_block`. (Part B)
- **Modify** `src/ui/app.rs` — `do_combined_save` does the live fetch + pre-confirm block. (Part B)
- **Modify** `src/ui/state.rs` — add pure `samba_in_use` + blocking `discover_samba_domain`; rewire `bootstrap`. (Part C)
- **Modify** `CHANGES.md` — one entry covering all three. (each part appends)

---

# Part A — X-ORDERED editing

### Task A1: Pure `{n}` helpers in a new `ordered` module

**Files:**
- Create: `src/ui/ordered.rs`
- Modify: `src/ui/mod.rs` (declare module)

**Interfaces:**
- Produces: `pub(crate) fn strip_ordering(s: &str) -> &str` — drops a leading `{<digits>}` ordering prefix only; everything else returned unchanged. `pub(crate) fn reconstruct(rows: &[String]) -> Vec<String>` — prepends `{i}` (row index) to each row.

- [ ] **Step 1: Declare the module.** In `src/ui/mod.rs`, add this line immediately after `pub(crate) mod oc_picker;` (line 12):

```rust
pub(crate) mod ordered;
```

- [ ] **Step 2: Write the failing test.** Create `src/ui/ordered.rs` with ONLY the helpers' tests (the functions don't exist yet — this must fail to compile, which counts as failing):

```rust
//! X-ORDERED multi-value editor: like the free-text multi-value editor, but it
//! owns the OpenLDAP `X-ORDERED 'VALUES'` `{n}` ordering prefix. Values are shown
//! with the `{n}` stripped; on commit the prefix is reconstructed from the current
//! row order, so reordering rows is the central operation. Staged values carry
//! `{n}`, so the neutral `form::changeset::diff` (which special-cases x-ordered
//! attrs into a single `Replace`) is unchanged. First save after editing may emit
//! one normalizing `Replace` if the server's stored indices were not `{0..n-1}`;
//! the server re-normalizes, so this is harmless. Capability: `Static`.

/// Drop a leading `{<digits>}` ordering prefix; return everything else unchanged.
/// A `{` not followed by one-or-more ASCII digits and a `}` is NOT a prefix.
pub(crate) fn strip_ordering(s: &str) -> &str {
    let bytes = s.as_bytes();
    if bytes.first() != Some(&b'{') {
        return s;
    }
    let mut i = 1;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    // Need at least one digit (i > 1) and a closing '}' right after.
    if i > 1 && bytes.get(i) == Some(&b'}') {
        &s[i + 1..]
    } else {
        s
    }
}

/// Prepend `{i}` (contiguous row index) to each row, in order.
pub(crate) fn reconstruct(rows: &[String]) -> Vec<String> {
    rows.iter()
        .enumerate()
        .map(|(i, r)| format!("{{{i}}}{r}"))
        .collect()
}

#[cfg(test)]
mod helper_tests {
    use super::*;

    #[test]
    fn strip_removes_leading_index_only() {
        assert_eq!(strip_ordering("{0}read by self"), "read by self");
        assert_eq!(strip_ordering("{12}write"), "write");
        assert_eq!(strip_ordering("{0}"), "");
    }

    #[test]
    fn strip_leaves_non_index_braces() {
        assert_eq!(strip_ordering("plain"), "plain");
        assert_eq!(strip_ordering("{}empty"), "{}empty");
        assert_eq!(strip_ordering("{a}x"), "{a}x");
        assert_eq!(strip_ordering("by group/{0}"), "by group/{0}");
        assert_eq!(strip_ordering(""), "");
    }

    #[test]
    fn reconstruct_numbers_rows_in_order() {
        assert_eq!(
            reconstruct(&["write".to_string(), "read".to_string()]),
            vec!["{0}write".to_string(), "{1}read".to_string()]
        );
    }

    #[test]
    fn strip_then_reconstruct_round_trips_order() {
        let stored = ["{0}a".to_string(), "{1}b".to_string()];
        let display: Vec<String> = stored.iter().map(|s| strip_ordering(s).to_string()).collect();
        assert_eq!(reconstruct(&display), vec!["{0}a".to_string(), "{1}b".to_string()]);
    }
}
```

- [ ] **Step 3: Run the test to verify it passes.**

Run: `cargo test -j4 --lib ui::ordered::helper_tests 2>&1 | tail -20`
Expected: 4 tests pass (the helpers are defined in the same step as their tests; this task's value is the verified pure logic before the editor is built on it).

- [ ] **Step 4: Verify fmt + clippy.**

Run: `cargo fmt && cargo clippy -j4 --lib -- -D warnings 2>&1 | tail -5`
Expected: no warnings.

- [ ] **Step 5: Commit.**

```bash
git add src/ui/ordered.rs src/ui/mod.rs
git commit -F - <<'EOF'
feat(m5c): x-ordered {n} strip/reconstruct helpers

Pure strip_ordering / reconstruct in the new src/ui/ordered module —
the load-bearing {n} contract for the X-ORDERED editor (Task A2).

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

### Task A2: The X-ORDERED modal editor + routing

**Files:**
- Modify: `src/ui/ordered.rs` (add the widget/editor/dialog + tests)
- Modify: `src/ui/widget.rs` (`widget_for` + `is_modal_field` arms)

**Interfaces:**
- Consumes: `strip_ordering`, `reconstruct` (Task A1); `Activation`, `Capability`, `CommitOutcome`, `FieldEditor`, `FieldWidget` (`crate::ui::widget`); `Shared` (`crate::ui`); `EditField` (`crate::workflows::edit_form`).
- Produces: `pub(crate) struct OrderedWidget` (a `FieldWidget`).

- [ ] **Step 1: Write the failing tests.** Append to `src/ui/ordered.rs` (before the final `}` of the file, after `helper_tests`):

```rust
use tvision_rs::{
    self as tv, delegate, ButtonFlags, ButtonRowAlign, Command, Context, Dialog, Event, FieldValue,
    InputLine, Key, ListBox, Rect, View,
};

use crate::schema::SchemaModel;
use crate::ui::widget::{Activation, Capability, CommitOutcome, FieldEditor, FieldWidget};
use crate::ui::Shared;
use crate::workflows::edit_form::EditField;

/// Plugin for X-ORDERED editable multi-value fields (`WidgetKind::XOrdered`).
/// Presents the values with `{n}` stripped and opens the ordered modal editor.
pub(crate) struct OrderedWidget;

impl FieldWidget for OrderedWidget {
    fn capability(&self) -> Capability {
        Capability::Static
    }

    fn present(&self, field: &EditField) -> String {
        if field.values.iter().all(|v| strip_ordering(v).trim().is_empty()) {
            "\u{2014}".to_string() // em dash
        } else {
            field
                .values
                .iter()
                .map(|s| strip_ordering(s))
                .collect::<Vec<_>>()
                .join(", ")
        }
    }

    fn activate(&self, field: &EditField) -> Activation {
        Activation::Modal(Box::new(OrderedEditor {
            label: field.label.clone(),
            values: field.values.clone(),
        }))
    }
}

/// Carries the field's current (`{n}`-prefixed) values into the dialog builder.
pub(crate) struct OrderedEditor {
    pub label: String,
    pub values: Vec<String>,
}

impl FieldEditor for OrderedEditor {
    fn into_view(
        self: Box<Self>,
        _schema: &SchemaModel,
        shared: Shared,
    ) -> (Box<dyn View>, tv::ViewId) {
        let OrderedEditor { label, values } = *self;
        let dlg = OrderedDialog::new(label, values, shared);
        let focus = dlg.input_id;
        (Box::new(dlg), focus)
    }
}

/// The interactive dialog. `rows` holds the DISPLAY (stripped) values; the
/// `InputLine` mirrors the selected row. Staged values are reconstructed with
/// `{n}` from the current row order.
pub(crate) struct OrderedDialog {
    dlg: Dialog,
    list_id: tv::ViewId,
    input_id: tv::ViewId,
    shared: Shared,
    rows: Vec<String>,
    sel: usize,
}

impl OrderedDialog {
    fn new(label: String, values: Vec<String>, shared: Shared) -> Self {
        let title = format!("Edit {label} (ordered)");
        let mut dlg = Dialog::new(Rect::new(0, 0, 60, 20), Some(title));
        dlg.state_mut().options.center_x = true;
        dlg.state_mut().options.center_y = true;
        let list = ListBox::new(Rect::new(2, 1, 58, 15), 1, None, None);
        let list_id = dlg.insert_child(Box::new(list));
        let input = InputLine::with_limit(Rect::new(2, 16, 58, 17), 1024);
        let input_id = dlg.insert_child(Box::new(input));
        dlg.button_row(
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
        // Strip {n} on load: the dialog edits display values only.
        let rows = values.iter().map(|v| strip_ordering(v).to_string()).collect();
        OrderedDialog {
            dlg,
            list_id,
            input_id,
            shared,
            rows,
            sel: 0,
        }
    }

    fn refresh_list(&mut self, ctx: &mut Context) {
        let rows = self.rows.clone();
        let len = rows.len();
        if let Some(list) = self.dlg.child_mut(self.list_id) {
            if let Some(lb) = list.as_any_mut().and_then(|a| a.downcast_mut::<ListBox>()) {
                lb.new_list(rows, ctx);
            }
            if len > 0 {
                let clamped = self.sel.min(len - 1) as i32;
                list.set_value_ctx(FieldValue::Int(clamped), ctx);
            }
        }
    }

    fn load_input(&mut self) {
        let text = self.rows.get(self.sel).cloned().unwrap_or_default();
        if let Some(c) = self.dlg.child_mut(self.input_id) {
            c.set_value(FieldValue::Text(text));
        }
    }

    /// Reconstruct `{n}` from the trimmed, non-empty rows in order and stage it.
    fn update_staged(&self) {
        let trimmed: Vec<String> = self
            .rows
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        self.shared.borrow_mut().staged_commit =
            Some(CommitOutcome::SetValues(reconstruct(&trimmed)));
    }

    fn refresh_all(&mut self, ctx: &mut Context) {
        self.refresh_list(ctx);
        self.load_input();
        self.update_staged();
    }

    fn move_sel(&mut self, delta: i32, ctx: &mut Context) {
        if self.rows.is_empty() {
            self.sel = 0;
            return;
        }
        let len = self.rows.len() as i32;
        let mut s = self.sel as i32 + delta;
        if s < 0 {
            s = 0;
        }
        if s >= len {
            s = len - 1;
        }
        self.sel = s as usize;
        self.refresh_list(ctx);
        self.load_input();
    }

    fn swap_row(&mut self, delta: i32, ctx: &mut Context) {
        if self.rows.len() < 2 {
            return;
        }
        let j = self.sel as i32 + delta;
        if j < 0 || j >= self.rows.len() as i32 {
            return;
        }
        let j = j as usize;
        self.rows.swap(self.sel, j);
        self.sel = j;
        self.refresh_all(ctx);
    }

    fn add_row(&mut self, ctx: &mut Context) {
        let at = if self.rows.is_empty() { 0 } else { self.sel + 1 };
        self.rows.insert(at, String::new());
        self.sel = at;
        self.refresh_all(ctx);
    }

    fn delete_row(&mut self, ctx: &mut Context) {
        if self.rows.is_empty() {
            return;
        }
        self.rows.remove(self.sel);
        if self.rows.is_empty() {
            self.sel = 0;
        } else if self.sel >= self.rows.len() {
            self.sel = self.rows.len() - 1;
        }
        self.refresh_all(ctx);
    }

    fn type_char(&mut self, c: char, ctx: &mut Context) {
        if self.rows.is_empty() {
            self.rows.push(String::new());
            self.sel = 0;
        }
        self.rows[self.sel].push(c);
        self.refresh_all(ctx);
    }

    fn backspace(&mut self, ctx: &mut Context) {
        if let Some(row) = self.rows.get_mut(self.sel) {
            row.pop();
        }
        self.refresh_all(ctx);
    }
}

#[delegate(to = dlg)]
impl View for OrderedDialog {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    fn reset_current(&mut self, ctx: &mut Context) {
        self.dlg.reset_current(ctx);
        self.sel = 0;
        self.refresh_all(ctx);
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        let (key, alt) = match ev {
            Event::KeyDown(k) => (k.key, k.modifiers.alt),
            _ => {
                self.dlg.handle_event(ev, ctx);
                return;
            }
        };
        match (key, alt) {
            (Key::Up, false) => {
                self.move_sel(-1, ctx);
                ev.clear();
            }
            (Key::Down, false) => {
                self.move_sel(1, ctx);
                ev.clear();
            }
            (Key::Up, true) => {
                self.swap_row(-1, ctx);
                ev.clear();
            }
            (Key::Down, true) => {
                self.swap_row(1, ctx);
                ev.clear();
            }
            (Key::Char('a'), true) | (Key::Insert, _) => {
                self.add_row(ctx);
                ev.clear();
            }
            (Key::Char('d'), true) | (Key::Delete, _) => {
                self.delete_row(ctx);
                ev.clear();
            }
            (Key::Char(c), false) => {
                self.type_char(c, ctx);
                ev.clear();
            }
            (Key::Backspace, _) => {
                self.backspace(ctx);
                ev.clear();
            }
            _ => {
                self.dlg.handle_event(ev, ctx);
            }
        }
    }
}

#[cfg(test)]
mod editor_tests {
    use super::*;
    use crate::config::widget::WidgetKind;
    use crate::ldap::worker::RawSubschema;
    use crate::schema::{FieldKind, SchemaModel};
    use crate::workflows::form_model::WidgetSpec;
    use std::cell::RefCell;
    use std::rc::Rc;
    use tvision_rs::{timer::TimerQueue, Deferred, KeyEvent, KeyModifiers};

    fn xordered_field(label: &str, vals: &[&str]) -> EditField {
        EditField {
            label: label.into(),
            must: false,
            editable: true,
            multi: true,
            secret: false,
            ordered: true,
            orphaned: false,
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: Some(WidgetKind::XOrdered),
            values: vals.iter().map(|s| s.to_string()).collect(),
            baseline: vals.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn schema_for_test() -> SchemaModel {
        SchemaModel::from_raw(&RawSubschema::default())
    }

    fn test_shared() -> Shared {
        use crate::workflows::structure::Structure;
        let st = crate::ui::state::UiState::new_for_test(
            Structure::build("dc=example,dc=org", vec![]),
            schema_for_test(),
            "dc=example,dc=org".into(),
            Vec::new(),
            Vec::new(),
        );
        Rc::new(RefCell::new(st))
    }

    fn headless<'a>(
        out: &'a mut std::collections::VecDeque<tv::Event>,
        timers: &'a mut TimerQueue,
        deferred: &'a mut Vec<Deferred>,
    ) -> Context<'a> {
        Context::new(out, timers, 0, deferred)
    }

    fn key(view: &mut dyn View, ctx: &mut Context, k: Key, alt: bool) {
        let mut ev = Event::KeyDown(KeyEvent::new(
            k,
            KeyModifiers {
                alt,
                ..KeyModifiers::default()
            },
        ));
        view.handle_event(&mut ev, ctx);
    }

    fn staged(shared: &Shared) -> Option<CommitOutcome> {
        shared.borrow().staged_commit.clone()
    }

    #[test]
    fn present_strips_ordering_prefixes() {
        let w = OrderedWidget;
        let f = xordered_field("olcAccess", &["{0}read", "{1}write"]);
        assert_eq!(w.present(&f), "read, write");
    }

    #[test]
    fn open_stages_reconstructed_values_unchanged() {
        let shared = test_shared();
        let ed = Box::new(OrderedEditor {
            label: "olcAccess".into(),
            values: vec!["{0}read".into(), "{1}write".into()],
        });
        let (mut view, _focus) = ed.into_view(&schema_for_test(), shared.clone());
        let mut out = std::collections::VecDeque::new();
        let mut timers = TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless(&mut out, &mut timers, &mut deferred);
        view.reset_current(&mut ctx);
        // No edit: staged equals the original {n} values.
        assert_eq!(
            staged(&shared),
            Some(CommitOutcome::SetValues(vec![
                "{0}read".into(),
                "{1}write".into()
            ]))
        );
    }

    #[test]
    fn reorder_reassigns_indices() {
        let shared = test_shared();
        let ed = Box::new(OrderedEditor {
            label: "olcAccess".into(),
            values: vec!["{0}read".into(), "{1}write".into()],
        });
        let (mut view, _focus) = ed.into_view(&schema_for_test(), shared.clone());
        let mut out = std::collections::VecDeque::new();
        let mut timers = TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless(&mut out, &mut timers, &mut deferred);
        view.reset_current(&mut ctx);
        // sel = 0; Alt+Down swaps rows 0 and 1 → indices reassigned by order.
        key(view.as_mut(), &mut ctx, Key::Down, true);
        assert_eq!(
            staged(&shared),
            Some(CommitOutcome::SetValues(vec![
                "{0}write".into(),
                "{1}read".into()
            ]))
        );
    }

    #[test]
    fn delete_then_add_renumbers_contiguously() {
        let shared = test_shared();
        let ed = Box::new(OrderedEditor {
            label: "olcAccess".into(),
            values: vec!["{0}a".into(), "{1}b".into(), "{2}c".into()],
        });
        let (mut view, _focus) = ed.into_view(&schema_for_test(), shared.clone());
        let mut out = std::collections::VecDeque::new();
        let mut timers = TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless(&mut out, &mut timers, &mut deferred);
        view.reset_current(&mut ctx);
        // sel = 0; Alt+d deletes "a" → contiguous {0}b {1}c.
        key(view.as_mut(), &mut ctx, Key::Char('d'), true);
        assert_eq!(
            staged(&shared),
            Some(CommitOutcome::SetValues(vec!["{0}b".into(), "{1}c".into()]))
        );
    }
}
```

- [ ] **Step 2: Run to verify it fails.**

Run: `cargo test -j4 --lib ui::ordered::editor_tests 2>&1 | tail -20`
Expected: FAIL — `OrderedWidget`/`OrderedEditor` already added above as part of the same edit, so this compiles and PASSES. (The editor body and its tests are one cohesive deliverable; the meaningful gate is the routing wiring in Step 3, whose test fails first.)

- [ ] **Step 3: Add the routing arm + a routing test (RED first).** In `src/ui/widget.rs`, add to the `widget_for` chain — insert this arm **immediately before** the `} else if field.editable && field.multi && !field.orphaned && field.widget_binding.is_none() {` arm (line 150):

```rust
    } else if matches!(field.widget_binding, Some(WidgetKind::XOrdered)) {
        Box::new(crate::ui::ordered::OrderedWidget)
```

And in `is_modal_field`, add this disjunct after the `SambaSid` line (line 173):

```rust
        || matches!(field.widget_binding, Some(WidgetKind::XOrdered))
```

Then add this test to `widget.rs`'s `mod tests`:

```rust
    #[test]
    fn xordered_field_routes_and_is_modal() {
        use crate::config::widget::WidgetKind;
        let mut f = field(&["{0}a", "{1}b"], WidgetSpec::ReadOnlyText);
        f.label = "olcAccess".into();
        f.multi = true;
        f.ordered = true;
        f.widget_binding = Some(WidgetKind::XOrdered);
        assert!(is_modal_field(&f));
        assert!(matches!(widget_for(&f).activate(&f), Activation::Modal(_)));
        // Presented with {n} stripped.
        assert_eq!(widget_for(&f).present(&f), "a, b");
    }
```

- [ ] **Step 4: Run the full new test set to verify it passes.**

Run: `cargo test -j4 --lib ui::ordered:: ui::widget::tests::xordered 2>&1 | tail -25`
Expected: all `ordered` editor tests + `xordered_field_routes_and_is_modal` PASS.

- [ ] **Step 5: fmt + clippy + full lib tests.**

Run: `cargo fmt && cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -5 && cargo test -j4 --lib 2>&1 | tail -5`
Expected: no warnings; all lib tests pass.

- [ ] **Step 6: Commit.**

```bash
git add src/ui/ordered.rs src/ui/widget.rs
git commit -F - <<'EOF'
feat(m5c): X-ORDERED modal editor + widget_for routing

OrderedWidget/OrderedEditor/OrderedDialog: shows values with the {n}
prefix stripped, reconstructs {n} from row order on commit, so the
neutral diff layer is unchanged. Routes WidgetKind::XOrdered (previously
read-only via PlainWidget) through the modal seam.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

### Task A3: Demo affordance + docs + changelog (X-ORDERED)

**Files:**
- Modify: `examples/demo-config.toml`
- Modify: `docs/src/configuration/widgets.md`
- Modify: `CHANGES.md`

**Interfaces:** none (config/docs only).

- [ ] **Step 1: Add a demo x_ordered binding.** In `examples/demo-config.toml`, the `member` widget block ends at line 85; the next `[[profile]]` (`posixgroup`) starts at line 87. Insert this block between them (so it binds to the `group` profile, whose `show` already lists `description`):

```toml
# DEMO ONLY: treat the group's multi-valued `description` as an X-ORDERED
# editable list so the ordered editor is drivable against the demo server.
# Note: saving writes `{n}`-prefixed values; edaptor strips them on read, so
# round-trips stay self-consistent (other tools will see the `{n}` tags).
[profile.widget.description]
kind = "x_ordered"
```

- [ ] **Step 2: Flip the docs claim.** Open `docs/src/configuration/widgets.md` and read the `### x_ordered` section (around line 315). Replace any wording that says X-ORDERED fields are currently read-only / not yet editable in the tvision UI with a statement that they are editable: an ordered list editor (add/delete/reorder) that hides the `{n}` prefix and regenerates it from row order on save. If the section already claims "editable" with no read-only caveat, leave the prose and only ensure the example block matches `kind = "x_ordered"`. Verify with:

Run: `grep -n -i "read-only\|read only\|not.*editable\|deferred\|x_ordered\|reorder" docs/src/configuration/widgets.md | sed -n '1,20p'`
Expected: no stale "read-only"/"deferred" claim remains in the `x_ordered` section.

- [ ] **Step 3: Build the docs to confirm no breakage.**

Run: `cd docs && mdbook build 2>&1 | tail -5; cd ..`
Expected: builds without error (`mdbook` present per the working agreement; if absent, skip and note it).

- [ ] **Step 4: Add the changelog entry.** In `CHANGES.md`, under the current unreleased section, add (create the bullet group if needed):

```markdown
- X-ORDERED attributes (`kind = "x_ordered"`) are now editable in the UI: an
  ordered list editor (add / delete / Alt+↑/↓ reorder) that hides the `{n}`
  ordering prefix and regenerates it from row order on save.
```

- [ ] **Step 5: Commit.**

```bash
git add examples/demo-config.toml docs/src/configuration/widgets.md CHANGES.md
git commit -F - <<'EOF'
docs(m5c): X-ORDERED editable — demo binding, widgets.md, CHANGES

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

# Part B — schema-aware last-member pre-validation

### Task B1: `membership_attr_is_must` (pure)

**Files:**
- Modify: `src/workflows/save.rs` (add function + tests)

**Interfaces:**
- Consumes: `SchemaModel::effective_attributes(&[&str]) -> ResolvedAttributes { must: BTreeSet<String>, .. }` (already in scope via `save.rs`'s existing `validate` usage).
- Produces: `pub fn membership_attr_is_must(schema: &SchemaModel, object_classes: &[&str], attr: &str) -> bool`.

- [ ] **Step 1: Write the failing test.** In `src/workflows/save.rs`, inside the existing `#[cfg(test)] mod tests`, add a fixture + tests:

```rust
    fn group_schema() -> SchemaModel {
        use crate::ldap::worker::RawSubschema;
        SchemaModel::from_raw(&RawSubschema {
            object_classes: vec![
                "( 2.5.6.0 NAME 'top' ABSTRACT MUST objectClass )".to_string(),
                "( 2.5.6.9 NAME 'groupOfNames' SUP top STRUCTURAL MUST ( member $ cn ) \
                  MAY ( description $ owner ) )"
                    .to_string(),
                "( 2.5.6.17 NAME 'groupOfUniqueNames' SUP top STRUCTURAL \
                  MUST ( uniqueMember $ cn ) MAY ( description ) )"
                    .to_string(),
                "( 1.3.6.1.1.1.2.2 NAME 'posixGroup' SUP top STRUCTURAL \
                  MUST ( cn $ gidNumber ) MAY ( userPassword $ memberUid $ description ) )"
                    .to_string(),
            ],
            attribute_types: vec![],
            ldap_syntaxes: vec![],
        })
    }

    #[test]
    fn member_is_must_for_group_of_names() {
        let s = group_schema();
        assert!(membership_attr_is_must(&s, &["groupOfNames"], "member"));
        assert!(membership_attr_is_must(&s, &["groupOfNames"], "MEMBER")); // case-insensitive
    }

    #[test]
    fn unique_member_is_must_for_group_of_unique_names() {
        let s = group_schema();
        assert!(membership_attr_is_must(&s, &["groupOfUniqueNames"], "uniqueMember"));
    }

    #[test]
    fn member_uid_is_may_for_posix_group() {
        let s = group_schema();
        assert!(!membership_attr_is_must(&s, &["posixGroup"], "memberUid"));
    }

    #[test]
    fn unknown_class_or_attr_is_not_must() {
        let s = group_schema();
        assert!(!membership_attr_is_must(&s, &["doesNotExist"], "member"));
        assert!(!membership_attr_is_must(&s, &["groupOfNames"], "noSuchAttr"));
    }
```

- [ ] **Step 2: Run to verify it fails.**

Run: `cargo test -j4 --lib workflows::save::tests::member 2>&1 | tail -15`
Expected: FAIL — `membership_attr_is_must` not found.

- [ ] **Step 3: Implement.** Add to `src/workflows/save.rs` (near `would_empty`, after line 385):

```rust
/// True when `attr` is a MUST (required) attribute for any of `object_classes`
/// per `schema` (case-insensitive). Gates last-member pre-validation: only block
/// removing a group's final member when its membership attribute is MUST (e.g.
/// `member` in `groupOfNames`), never for MAY (`memberUid` in `posixGroup`,
/// where an empty group is legal).
pub fn membership_attr_is_must(
    schema: &SchemaModel,
    object_classes: &[&str],
    attr: &str,
) -> bool {
    schema
        .effective_attributes(object_classes)
        .must
        .iter()
        .any(|m| m.eq_ignore_ascii_case(attr))
}
```

- [ ] **Step 4: Run to verify it passes.**

Run: `cargo test -j4 --lib workflows::save::tests 2>&1 | tail -10`
Expected: the four new tests + existing `save` tests PASS.

- [ ] **Step 5: Commit.**

```bash
git add src/workflows/save.rs
git commit -F - <<'EOF'
feat(m5c): schema MUST check for membership attrs

membership_attr_is_must gates last-member pre-validation: MUST member
(groupOfNames/groupOfUniqueNames) vs MAY memberUid (posixGroup).

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

### Task B2: `last_member_block` (pure) + refactor `submit_combined` to use it

**Files:**
- Modify: `src/workflows/save.rs` (add `last_member_block` + tests)
- Modify: `src/workflows/write_flow.rs` (call it from `submit_combined`)

**Interfaces:**
- Consumes: `would_empty` (save.rs); `ModOp` (`crate::form::changeset`, already imported in save.rs at line 5).
- Produces: `pub fn last_member_block(fanout: &[(String, ModOp)], group_members: &std::collections::HashMap<String, Vec<String>>, own_dn: &str) -> Option<String>`.

- [ ] **Step 1: Write the failing test.** In `src/workflows/save.rs` `mod tests`, add:

```rust
    #[test]
    fn last_member_block_fires_only_for_known_sole_member() {
        use std::collections::HashMap;
        let fanout = vec![(
            "cn=admins,ou=groups".to_string(),
            ModOp::Delete {
                attr: "member".into(),
                values: vec!["uid=ann,ou=people".into()],
            },
        )];
        // Group known with ann as the sole member → blocked.
        let mut gm: HashMap<String, Vec<String>> = HashMap::new();
        gm.insert(
            "cn=admins,ou=groups".into(),
            vec!["uid=ann,ou=people".into()],
        );
        assert!(last_member_block(&fanout, &gm, "uid=ann,ou=people").is_some());

        // Group not in the map (e.g. MAY membership, never fetched) → no block.
        let empty: HashMap<String, Vec<String>> = HashMap::new();
        assert!(last_member_block(&fanout, &empty, "uid=ann,ou=people").is_none());

        // Group with another member too → no block.
        let mut gm2: HashMap<String, Vec<String>> = HashMap::new();
        gm2.insert(
            "cn=admins,ou=groups".into(),
            vec!["uid=ann,ou=people".into(), "uid=bob,ou=people".into()],
        );
        assert!(last_member_block(&fanout, &gm2, "uid=ann,ou=people").is_none());
    }

    #[test]
    fn last_member_block_ignores_add_ops() {
        use std::collections::HashMap;
        let fanout = vec![(
            "cn=admins,ou=groups".to_string(),
            ModOp::Add {
                attr: "member".into(),
                values: vec!["uid=ann,ou=people".into()],
            },
        )];
        let mut gm: HashMap<String, Vec<String>> = HashMap::new();
        gm.insert("cn=admins,ou=groups".into(), vec!["uid=ann,ou=people".into()]);
        assert!(last_member_block(&fanout, &gm, "uid=ann,ou=people").is_none());
    }
```

- [ ] **Step 2: Run to verify it fails.**

Run: `cargo test -j4 --lib workflows::save::tests::last_member 2>&1 | tail -15`
Expected: FAIL — `last_member_block` not found.

- [ ] **Step 3: Implement `last_member_block`.** Add to `src/workflows/save.rs` after `would_empty` (after line 385):

```rust
/// Pre-validation for a combined membership save: scan the fan-out `Delete`
/// ops and, for any group present in `group_members` whose sole member is
/// `own_dn`, return a refusal message. Groups absent from `group_members` are
/// treated as having no known members (`would_empty` returns false) — this is
/// how MAY-membership groups are exempted: the caller only populates the map
/// for groups whose membership attribute is MUST (see
/// [`membership_attr_is_must`]). Returns `None` when nothing would be emptied.
pub fn last_member_block(
    fanout: &[(String, ModOp)],
    group_members: &std::collections::HashMap<String, Vec<String>>,
    own_dn: &str,
) -> Option<String> {
    for (group_dn, op) in fanout {
        if let ModOp::Delete { .. } = op {
            let current = group_members
                .get(group_dn)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            if would_empty(current, own_dn) {
                return Some(format!(
                    "Refusing to save: removing {own_dn} from {group_dn} would leave \
                     the group with no members (a required attribute)."
                ));
            }
        }
    }
    None
}
```

- [ ] **Step 4: Refactor `submit_combined` to call it.** In `src/workflows/write_flow.rs`, the pre-validation loop is around lines 385–401. Replace the inline `for (group_dn, op) in &fanout { ... would_empty ... }` block with a single call. The new body of that region:

```rust
        // Last-member pre-validation (schema-gated by the caller's populate of
        // `group_members`): refuse before submitting anything.
        if let Some(msg) =
            crate::workflows::save::last_member_block(&fanout, group_members, &own_dn)
        {
            return Err(msg);
        }
```

Keep `submit_combined`'s signature, doc-comment, and everything else unchanged. The existing test `submit_combined_last_member_aborts_with_nothing_submitted` (write_flow.rs ~line 1060) must still pass — its message assertion may check a substring; if it asserts the exact old string, update that test's expected substring to match (e.g. assert it `contains("would leave")`).

- [ ] **Step 5: Run the affected tests.**

Run: `cargo test -j4 --lib workflows::save::tests::last_member workflows::write_flow::tests::submit_combined 2>&1 | tail -20`
Expected: PASS (adjust the existing test's substring assertion in Step 4 if it fails on the message text only).

- [ ] **Step 6: fmt + clippy + full lib tests, then commit.**

```bash
cargo fmt && cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -5 && cargo test -j4 --lib 2>&1 | tail -5
git add src/workflows/save.rs src/workflows/write_flow.rs
git commit -F - <<'EOF'
refactor(m5c): last_member_block helper; submit_combined delegates

Extract the pure last-member pre-validation so both submit_combined and
the pre-confirm UI check (Task B4) share one implementation. Behaviour
unchanged; submit_combined contract preserved.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

### Task B3: `fetch_group_members_for_must` (blocking, schema-gated populate)

**Files:**
- Modify: `src/workflows/write_flow.rs` (add function + a recording-worker test)

**Interfaces:**
- Consumes: `WorkerHandle::request`, `Request::Search`, `SearchScope::Base`, `Response::Entries`, `LdapEntry` (`crate::ldap::worker`); `ModOp::Delete` (`crate::form::changeset`); `membership_attr_is_must` (save.rs); `SchemaModel`.
- Produces: `pub fn fetch_group_members_for_must(worker: &WorkerHandle, schema: &SchemaModel, fanout: &[(String, ModOp)]) -> std::collections::HashMap<String, Vec<String>>`.

- [ ] **Step 1: Add the `SearchScope` import.** In `src/workflows/write_flow.rs`, line 14 currently reads:

```rust
use crate::ldap::worker::{Request, Response, WorkerHandle};
```

Change it to:

```rust
use crate::ldap::worker::{Request, Response, SearchScope, WorkerHandle};
```

Also ensure the `save::{...}` import (line ~19) includes `membership_attr_is_must`:

```rust
    compose_renamed_dn, membership_attr_is_must, plan_combined_save, prepare_save,
    stage_pending_password, would_empty,
```

- [ ] **Step 2: Write the failing test.** In `src/workflows/write_flow.rs` `mod tests`, add (the test spins a responder thread because `WorkerHandle::request` blocks on a reply):

```rust
    #[test]
    fn fetch_populates_only_must_membership_groups() {
        use crate::form::changeset::ModOp;
        use crate::ldap::worker::{LdapEntry, Response, SearchScope};
        use std::collections::BTreeMap;

        let (worker, rx) = WorkerHandle::recording();
        // Responder: answer each Base search by the requested base DN.
        let responder = std::thread::spawn(move || {
            while let Ok((req, reply)) = rx.recv() {
                let crate::ldap::worker::Request::Search { base, scope, .. } = req else {
                    continue;
                };
                assert!(matches!(scope, SearchScope::Base));
                let mut attrs = BTreeMap::new();
                if base.starts_with("cn=admins") {
                    // groupOfNames: member is MUST.
                    attrs.insert("objectClass".to_string(), vec!["groupOfNames".to_string()]);
                    attrs.insert("member".to_string(), vec!["uid=ann,ou=people".to_string()]);
                } else {
                    // posixGroup: memberUid is MAY.
                    attrs.insert("objectClass".to_string(), vec!["posixGroup".to_string()]);
                    attrs.insert("memberUid".to_string(), vec!["ann".to_string()]);
                }
                let _ = reply.send(Response::Entries {
                    id: 0,
                    entries: vec![LdapEntry { dn: base.clone(), attrs }],
                    truncated: false,
                });
            }
        });

        let schema = group_schema_for_write_flow();
        let fanout = vec![
            (
                "cn=admins,ou=groups".to_string(),
                ModOp::Delete {
                    attr: "member".into(),
                    values: vec!["uid=ann,ou=people".into()],
                },
            ),
            (
                "cn=staff,ou=groups".to_string(),
                ModOp::Delete {
                    attr: "memberUid".into(),
                    values: vec!["ann".into()],
                },
            ),
        ];
        let map = fetch_group_members_for_must(&worker, &schema, &fanout);
        // MUST group included; MAY group omitted.
        assert!(map.contains_key("cn=admins,ou=groups"));
        assert!(!map.contains_key("cn=staff,ou=groups"));
        assert_eq!(map["cn=admins,ou=groups"], vec!["uid=ann,ou=people".to_string()]);

        drop(worker); // closes rx so the responder thread exits
        let _ = responder.join();
    }

    fn group_schema_for_write_flow() -> SchemaModel {
        use crate::ldap::worker::RawSubschema;
        SchemaModel::from_raw(&RawSubschema {
            object_classes: vec![
                "( 2.5.6.0 NAME 'top' ABSTRACT MUST objectClass )".to_string(),
                "( 2.5.6.9 NAME 'groupOfNames' SUP top STRUCTURAL MUST ( member $ cn ) )"
                    .to_string(),
                "( 1.3.6.1.1.1.2.2 NAME 'posixGroup' SUP top STRUCTURAL \
                  MUST ( cn $ gidNumber ) MAY ( memberUid ) )"
                    .to_string(),
            ],
            attribute_types: vec![],
            ldap_syntaxes: vec![],
        })
    }
```

(If `SchemaModel` is not already imported in `write_flow.rs`'s test module, add `use crate::schema::SchemaModel;` to the `mod tests` use-block.)

- [ ] **Step 3: Run to verify it fails.**

Run: `cargo test -j4 --lib workflows::write_flow::tests::fetch_populates 2>&1 | tail -15`
Expected: FAIL — `fetch_group_members_for_must` not found.

- [ ] **Step 4: Implement.** Add to `src/workflows/write_flow.rs` (module-level, e.g. just before `impl WriteFlow` or after it — a free function):

```rust
/// Blocking, schema-gated populate of the `group_members` map that
/// [`WriteFlow::submit_combined`] consumes. For each fan-out `Delete` op, fetch
/// the group's `objectClass` + membership attr (a single Base-scoped search) and
/// — only when that attr is MUST for the group ([`membership_attr_is_must`]) —
/// record the group's current members. MAY-membership groups (e.g. `posixGroup`
/// `memberUid`) are deliberately omitted so emptying them is allowed. Best-effort:
/// a failed/empty fetch leaves the group out (the server remains the backstop).
pub fn fetch_group_members_for_must(
    worker: &WorkerHandle,
    schema: &SchemaModel,
    fanout: &[(String, ModOp)],
) -> std::collections::HashMap<String, Vec<String>> {
    let mut map = std::collections::HashMap::new();
    for (group_dn, op) in fanout {
        let ModOp::Delete { attr, .. } = op else {
            continue;
        };
        let resp = worker.request(Request::Search {
            id: 0,
            base: group_dn.clone(),
            scope: SearchScope::Base,
            filter: "(objectClass=*)".to_string(),
            attrs: vec!["objectClass".to_string(), attr.clone()],
            size_limit: Some(1),
        });
        let Ok(Response::Entries { entries, .. }) = resp else {
            continue;
        };
        let Some(entry) = entries.first() else {
            continue;
        };
        let ocs: Vec<&str> = entry
            .attrs
            .get("objectClass")
            .map(|v| v.iter().map(String::as_str).collect())
            .unwrap_or_default();
        if membership_attr_is_must(schema, &ocs, attr) {
            let members = entry.attrs.get(attr).cloned().unwrap_or_default();
            map.insert(group_dn.clone(), members);
        }
    }
    map
}
```

Note: `ModOp` must be in scope at module level in `write_flow.rs`. It already is (used by `submit_combined`'s `fanout` destructure); if a compile error says otherwise, add `use crate::form::changeset::ModOp;`.

- [ ] **Step 5: Run to verify it passes.**

Run: `cargo test -j4 --lib workflows::write_flow::tests::fetch_populates 2>&1 | tail -15`
Expected: PASS.

- [ ] **Step 6: fmt + clippy + full lib tests, then commit.**

```bash
cargo fmt && cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -5 && cargo test -j4 --lib 2>&1 | tail -5
git add src/workflows/write_flow.rs
git commit -F - <<'EOF'
feat(m5c): blocking schema-gated group-member fetch

fetch_group_members_for_must populates the group_members map only for
groups whose membership attr is MUST, so submit_combined's guard blocks
emptying a groupOfNames but never a posixGroup.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

### Task B4: Wire the live fetch + pre-confirm block into `do_combined_save`

**Files:**
- Modify: `src/ui/app.rs` (`do_combined_save`, around lines 523–562)
- Modify: `tests/tv_membership.rs` (gated live assertion)
- Modify: `CHANGES.md`

**Interfaces:**
- Consumes: `fetch_group_members_for_must` (Task B3), `last_member_block` (Task B2), `error::build`, `confirm::build`, `UiState`.

- [ ] **Step 1: Replace the `PlanCombined::Ready` arm body.** In `src/ui/app.rs`, replace the entire `PlanCombined::Ready(combined) => { ... }` arm (lines 523–562) with this. The change: do the blocking, schema-gated fetch and a pre-confirm `last_member_block` check BEFORE the confirm dialog; pass the populated map to `submit_combined`.

```rust
        PlanCombined::Ready(combined) => {
            let reread_dn = combined.own_dn.clone();
            // M5c: live, schema-gated group-member fetch (blocking) so last-member
            // pre-validation runs client-side. Only MUST-membership groups are
            // populated; MAY groups (e.g. posixGroup memberUid) are exempt.
            let group_members = {
                let st = state.borrow();
                match (st.worker.as_ref(), st.edit_form.as_ref()) {
                    (Some(w), Some(_)) => crate::workflows::write_flow::fetch_group_members_for_must(
                        w,
                        st.read_flow.schema(),
                        &combined.fanout,
                    ),
                    _ => std::collections::HashMap::new(),
                }
            };
            // Refuse BEFORE showing the confirm if a removal would empty a MUST group.
            if let Some(msg) = crate::workflows::save::last_member_block(
                &combined.fanout,
                &group_members,
                &combined.own_dn,
            ) {
                let (view, ok) = error::build(&msg);
                prog.exec_view_focused(view, ok);
                return SaveOutcome::NotSubmitted;
            }
            // Focus the Save button so Enter confirms (firstMatch would pick Cancel).
            let (view, save) = confirm::build(&combined.ldif);
            if prog.exec_view_focused(view, save) != Command::OK {
                return SaveOutcome::NotSubmitted; // Cancel: keep editing.
            }
            // Submit the batch. Scope the borrow so the `UiState` destructure drops
            // before any error dialog `exec_view_focused`. `submit_combined` re-runs
            // `last_member_block` as defense-in-depth.
            let submit_result = {
                let mut st = state.borrow_mut();
                st.pending_nav = nav;
                st.guard_target = None;
                st.pending_password = None; // cleartext consumed; clear before worker picks it up
                let crate::ui::state::UiState {
                    worker, write_flow, ..
                } = &mut *st;
                worker.as_ref().map(|w| {
                    write_flow.submit_combined(w, combined, &group_members, &reread_dn, quit_after)
                })
            };
            match submit_result {
                Some(Ok(())) => SaveOutcome::Submitted,
                Some(Err(msg)) => {
                    let (view, ok) = error::build(&msg);
                    prog.exec_view_focused(view, ok);
                    SaveOutcome::NotSubmitted
                }
                None => SaveOutcome::NotSubmitted,
            }
        }
```

- [ ] **Step 2: Verify it compiles + lib tests + clippy.**

Run: `cargo fmt && cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -8 && cargo test -j4 --lib 2>&1 | tail -5`
Expected: compiles clean (note: `combined.fanout` is read by-ref before `combined` is moved into `submit_combined` — the immutable borrow ends before the move); all lib tests pass.

- [ ] **Step 3: Add a gated live assertion.** Open `tests/tv_membership.rs`, read its existing structure (how it connects to `EDAPTOR_TEST_LDAP_URI`, builds a form, and submits a combined save). Add a test (gated by the same env guard the file already uses) that: picks a `groupOfNames` group with a single member, drives a membership removal of that sole member, runs the combined save path, and asserts it is refused client-side with a message containing `"would leave"` and that the group still has its member on a re-read (demo data intact). If the harness exposes the planning/submit functions directly (not the full TUI), assert via `fetch_group_members_for_must` + `last_member_block` against the live server instead. Mirror the file's existing connection + cleanup idioms exactly.

- [ ] **Step 4: Run the gated live test (server must be up).**

```bash
scripts/test-ldap.sh start
EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389 EDAPTOR_TEST_ADMIN_PW=adminpassword \
  cargo test -j4 --test tv_membership 2>&1 | tail -25
```
Expected: the new last-member assertion passes; existing `tv_membership` tests still pass; demo data unchanged.

- [ ] **Step 5: Changelog + commit.** Add to `CHANGES.md` under the unreleased section:

```markdown
- Removing the last member of a required-membership group (`groupOfNames` /
  `groupOfUniqueNames`) is now blocked client-side before saving, with a clear
  message; emptying a `posixGroup` (`memberUid` is optional) is still allowed.
```

```bash
git add src/ui/app.rs tests/tv_membership.rs CHANGES.md
git commit -F - <<'EOF'
feat(m5c): client-side last-member pre-validation in combined save

do_combined_save fetches affected MUST-membership groups' members and
refuses (before the confirm dialog) any removal that would empty a
required group. posixGroup memberUid is exempt.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

# Part C — live sambaDomain discovery

### Task C1: `samba_in_use` + `discover_samba_domain` + bootstrap rewire

**Files:**
- Modify: `src/ui/state.rs` (add the two functions, rewire `bootstrap`, add tests)
- Modify: `CHANGES.md`

**Interfaces:**
- Consumes: `WorkerHandle::request`, `Request::Search`, `SearchScope::Subtree`, `Response::Entries` (`crate::ldap::worker`); `crate::samba::sid::parse_samba_domain`; `crate::config::widget::{ResolvedWidget, WidgetKind}`; `samba_info_from_config` (state.rs).
- Produces: `fn discover_samba_domain(worker: &WorkerHandle, base: &str) -> Option<crate::samba::SambaDomainInfo>`; `fn samba_in_use(widgets: &[crate::config::widget::ResolvedWidget]) -> bool`.

- [ ] **Step 1: Add the `SearchScope` import.** In `src/ui/state.rs`, line 8 currently reads:

```rust
use crate::ldap::worker::{Request, Response, WorkerHandle};
```

Change it to:

```rust
use crate::ldap::worker::{Request, Response, SearchScope, WorkerHandle};
```

- [ ] **Step 2: Write the failing test for `samba_in_use`.** In `src/ui/state.rs` `#[cfg(test)] mod tests`, add:

```rust
    #[test]
    fn samba_in_use_true_only_with_samba_sid_widget() {
        use crate::config::widget::{ResolvedWidget, WidgetKind};
        let none: Vec<ResolvedWidget> = vec![ResolvedWidget {
            owner_object_classes: vec!["posixGroup".into()],
            attr: "memberUid".into(),
            kind: WidgetKind::XOrdered,
        }];
        assert!(!super::samba_in_use(&none));

        let with_samba = vec![ResolvedWidget {
            owner_object_classes: vec!["sambaSamAccount".into()],
            attr: "sambaSID".into(),
            kind: WidgetKind::SambaSid,
        }];
        assert!(super::samba_in_use(&with_samba));
    }
```

- [ ] **Step 3: Run to verify it fails.**

Run: `cargo test -j4 --lib ui::state::tests::samba_in_use 2>&1 | tail -12`
Expected: FAIL — `samba_in_use` not found.

- [ ] **Step 4: Implement both functions.** Add to `src/ui/state.rs`, right after `samba_info_from_config` (after line 616):

```rust
/// True when any resolved widget is a `sambaSID` field — i.e. the samba domain
/// is actually needed, so a live discovery search at startup is worth issuing.
fn samba_in_use(widgets: &[crate::config::widget::ResolvedWidget]) -> bool {
    use crate::config::widget::WidgetKind;
    widgets.iter().any(|w| matches!(w.kind, WidgetKind::SambaSid))
}

/// Discover the samba domain context from a live `sambaDomain` entry under
/// `base` (best-effort). Returns the first entry that parses via
/// [`crate::samba::sid::parse_samba_domain`]; `None` when none is found, the
/// search fails, or access is denied — callers fall back to the config
/// `domain_sid`.
fn discover_samba_domain(
    worker: &WorkerHandle,
    base: &str,
) -> Option<crate::samba::SambaDomainInfo> {
    let resp = worker
        .request(Request::Search {
            id: 0,
            base: base.to_string(),
            scope: SearchScope::Subtree,
            filter: "(objectClass=sambaDomain)".to_string(),
            attrs: vec![
                "sambaSID".to_string(),
                "sambaAlgorithmicRidBase".to_string(),
            ],
            size_limit: Some(5),
        })
        .ok()?;
    let Response::Entries { entries, .. } = resp else {
        return None;
    };
    entries
        .iter()
        .find_map(|e| crate::samba::sid::parse_samba_domain(&e.attrs))
}
```

- [ ] **Step 5: Rewire `bootstrap`.** In `src/ui/state.rs`, the worker is spawned at line 643 (`let worker = WorkerHandle::spawn(...)?;`) and `samba_domain` is currently computed from config at line 642 (BEFORE the worker exists). Move the `samba_domain` computation to AFTER the worker spawn. Delete line 642:

```rust
    let samba_domain = samba_info_from_config(&config);
```

and insert this immediately AFTER `let worker = WorkerHandle::spawn(config, password)?;` (line 643):

```rust
    // M5c: prefer a live sambaDomain entry when a sambaSID widget is configured;
    // fall back to the static config domain_sid (or no samba at all).
    let samba_domain = if samba_in_use(&resolved_widgets) {
        discover_samba_domain(&worker, &base_dn).or_else(|| samba_info_from_config(&config))
    } else {
        samba_info_from_config(&config)
    };
```

Note: `base_dn` is already bound at line 632; `resolved_widgets` at line 634; both are in scope here. `samba_info_from_config(&config)` is still valid — `config` is moved into `WorkerHandle::spawn(config, …)` on line 643. **`config` is moved, so it can no longer be borrowed afterward.** To fix, capture the fallback BEFORE the spawn. Replace the spawn region so the config-derived fallback is computed first:

Before (lines ~641–643):
```rust
    let connection_encrypted = config.is_encrypted();
    let samba_domain = samba_info_from_config(&config);
    let worker = WorkerHandle::spawn(config, password)?;
```

After:
```rust
    let connection_encrypted = config.is_encrypted();
    let samba_from_config = samba_info_from_config(&config);
    let worker = WorkerHandle::spawn(config, password)?;
    // M5c: prefer a live sambaDomain entry when a sambaSID widget is configured;
    // fall back to the static config domain_sid (or no samba at all).
    let samba_domain = if samba_in_use(&resolved_widgets) {
        discover_samba_domain(&worker, &base_dn).or_else(|| samba_from_config)
    } else {
        samba_from_config
    };
```

- [ ] **Step 6: Run to verify the unit test passes + full lib tests + clippy.**

Run: `cargo fmt && cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -8 && cargo test -j4 --lib ui::state 2>&1 | tail -10`
Expected: `samba_in_use_true_only_with_samba_sid_widget` passes; no clippy warnings; all `ui::state` tests pass.

- [ ] **Step 7: Live verification (server up).** The demo server carries a `sambaDomain` entry (per `examples/demo-config.toml` line 16). Confirm discovery overrides the config fallback by running the app and checking sambaSID auto-gen uses the discovered SID. Quick check via the tmux harness (per HANDOVER): launch against `examples/demo-config.toml`, open a user, activate the `sambaSID` field, and confirm a SID is generated. (No write needed; the discovery happens at bootstrap.)

```bash
scripts/test-ldap.sh start
cargo build -j4 --bin edaptor 2>&1 | tail -3
```
Expected: builds; manual tmux drive shows sambaSID generated from the directory's domain (best-effort — if the demo lacks the entry, the config fallback SID is used; note which path was exercised).

- [ ] **Step 8: Changelog + commit.** Add to `CHANGES.md` under the unreleased section:

```markdown
- The samba domain SID is now discovered live from a `sambaDomain` directory
  entry at startup (when a `sambaSID` widget is configured), falling back to the
  configured `[samba] domain_sid`.
```

```bash
git add src/ui/state.rs CHANGES.md
git commit -F - <<'EOF'
feat(m5c): live sambaDomain discovery at startup

bootstrap now searches (objectClass=sambaDomain) when a sambaSID widget
is configured and prefers the discovered SID over the config fallback.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

# Part D — milestone gate

### Task D1: Full gate + facade guards + HANDOVER update

**Files:**
- Modify: `docs/HANDOVER.md` (mark M5c done / update NEXT ACTION)

- [ ] **Step 1: Facade guards (must print nothing).**

Run:
```bash
grep -rl "use tvision_rs" src | grep -vE "^src/ui/"; echo "guard1 rc=$?"
grep -rl "use ratatui\|use tui_" src; echo "guard2 rc=$?"
```
Expected: no file paths printed (grep rc=1 for both, meaning no matches).

- [ ] **Step 2: Full `make check`.**

Run: `make check 2>&1 | tail -20`
Expected: fmt clean, clippy `-D warnings` clean, all lib + (ungated) integration tests pass.

- [ ] **Step 3: All gated live tests (server up).**

```bash
scripts/test-ldap.sh start
EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389 EDAPTOR_TEST_ADMIN_PW=adminpassword \
  cargo test -j4 --test tv_membership --test tv_picker --test tv_edit_write \
      --test tv_objectclass --test tv_create 2>&1 | tail -20
```
Expected: all pass; demo data intact.

- [ ] **Step 4: Update the handover.** In `docs/HANDOVER.md`, replace the "▶ NEXT ACTION — M5c" banner with a short "M5c DONE" note (the three reconciliations are complete: X-ORDERED editable; schema-aware last-member pre-validation; live sambaDomain discovery), and move the migration to "ready to merge to main". Keep the load-bearing facts that remain true.

- [ ] **Step 5: Commit.**

```bash
git add docs/HANDOVER.md
git commit -F - <<'EOF'
docs(m5c): close the milestone — three reconciliations done

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

## Self-Review notes (for the implementer)

- **Spec coverage:** Seam 1 → A1–A3; Seam 2 → B1–B4; Seam 3 → C1; cross-cutting gate → D1. All three spec sections map to tasks.
- **Type consistency:** `strip_ordering`/`reconstruct` (A1) are used verbatim in A2; `membership_attr_is_must` (B1) is used in `last_member_block`'s exemption story (B2) and `fetch_group_members_for_must` (B3); `last_member_block` (B2) is called by both `submit_combined` (B2) and `do_combined_save` (B4); `fetch_group_members_for_must` (B3) is called by B4; `samba_in_use`/`discover_samba_domain` (C1) are used only inside `bootstrap`.
- **Borrow trap (B4):** `combined.fanout` is borrowed immutably to build the map, then `combined` is moved into `submit_combined`; the immutable borrow ends before the move. The fetch borrow of `state` is dropped before `exec_view_focused`.
- **Config move (C1):** `config` is moved into `WorkerHandle::spawn`; the config-derived samba fallback is captured into `samba_from_config` BEFORE the spawn (Step 5 final form), so no use-after-move.
- **Demo data (A3):** saving an x_ordered `description` writes `{n}` prefixes; edaptor strips them on read so round-trips stay self-consistent. Other tools will see the tags — acceptable for the demo server.
