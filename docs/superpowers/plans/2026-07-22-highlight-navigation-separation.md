# Highlight / Navigation Separation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a pane rebuild incapable of fabricating a navigation, by replacing the index-based highlight snap with a DN-based plan resolved after the rebuild.

**Architecture:** The panes gain two distinct paths. The *rebuild* path resolves a `HighlightPlan` from the controller, sets the widget focus and resyncs `last_sel` silently. The *event* path delegates to the widget and reports a focus change as an operator navigation. `set_leaf_row`/`set_tree_row` (row indices computed against a pre-rebuild row source) are deleted. A separate signal, `note_entry_vanished`, handles entries that genuinely disappeared.

**Tech Stack:** Rust 2021, tvision-rs 0.12.1, `ldap3` (confined to `src/ldap/**`).

## Global Constraints

- **Spec:** `docs/superpowers/specs/2026-07-22-highlight-navigation-separation-design.md`.
- **Facade boundary:** only `src/ui/**` may `use tvision_rs`; `ldap3` only in `src/ldap/**`.
- **Cap parallelism at 4 cores** on every cargo invocation (`-j4`); shared machine.
- **The gate is `make check`** (fmt + clippy `-D warnings` + tests). Clippy warnings are errors.
- **The I1, I4 and Finding-2 regression tests must keep passing.** They encode real shipped bugs. Their assertions may move from row indices to DNs only where the index ceases to exist; their *scenarios* must not be weakened.
- **Commit trailer:** every commit ends with
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- **Docs are part of done:** `CHANGES.md` for user-visible behaviour; `docs/src/` for concepts.

---

## File Structure

| File | Responsibility |
|---|---|
| `src/ui/state.rs` | `HighlightPlan`, `leaf_highlight_plan`, `branch_highlight_plan`, `begin_operator_action`, `note_entry_vanished`, `GuardTarget::Vanished`. Deletes `set_leaf_row`, `set_tree_row`, `leaf_search_truncated`. |
| `src/ui/panes/leaf.rs` | Rebuild path resolves the plan; `apply_set_row` deleted. |
| `src/ui/panes/tree.rs` | Same for the outline; consumes `branch_highlight_plan`. |
| `src/ui/dialog/vanished.rs` | **New.** Keep editing / Discard / Re-create dialog. |
| `src/ui/dialog/mod.rs` | `VanishedDecision` + `vanished_decision` mapping. |
| `src/ui/app.rs` | `GUARD_NAV` dispatch for the new target; `apply_branch_guard_stay` / `apply_cancelled_guard_save` deleted. |

Tasks 1–2 are pure controller logic and land first so the panes have something to consume. Tasks 3–4 are the pane rewrites. Task 5 is the status policy. Tasks 6–7 are the vanished path. Task 8 is the dead-field removal.

---

### Task 1: `HighlightPlan` and `leaf_highlight_plan`

**Files:**
- Modify: `src/ui/state.rs` (add the enum near `GuardTarget` at line 26; add the method in the `impl UiState` block that contains `leaf_rows`, ~line 1298)
- Test: `src/ui/state.rs` (inline `#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `pub enum HighlightPlan { Pin(String), Follow(String), Clear }` and
  `pub fn leaf_highlight_plan(&self) -> HighlightPlan` on `UiState`.
- Consumed by: Task 3.

- [ ] **Step 1: Write the failing tests** — the full truth table, one test per row.

```rust
    /// The truth table from the design. `Pin` moves the highlight only; `Follow`
    /// additionally lets the form follow. A dirty form is never followed, so a
    /// find-driven rebuild cannot raise the guard.
    #[test]
    fn highlight_plan_pins_the_open_entry_when_it_is_still_listed() {
        let mut st = st_with_rows(&["cn=a,ou=p,dc=x", "cn=b,ou=p,dc=x"]);
        st.current_leaf = Some("cn=b,ou=p,dc=x".to_string());
        assert_eq!(
            st.leaf_highlight_plan(),
            HighlightPlan::Pin("cn=b,ou=p,dc=x".to_string())
        );
    }

    #[test]
    fn highlight_plan_follows_the_first_row_when_the_open_entry_is_absent_and_clean() {
        let mut st = st_with_rows(&["cn=a,ou=p,dc=x", "cn=b,ou=p,dc=x"]);
        st.current_leaf = Some("cn=gone,ou=p,dc=x".to_string());
        // No edit_form at all == clean.
        assert_eq!(
            st.leaf_highlight_plan(),
            HighlightPlan::Follow("cn=a,ou=p,dc=x".to_string())
        );
    }

    /// The modal-mid-keystroke bug: a find that excludes the open entry must move
    /// the highlight but NOT the form, so `reconcile_selection` is never reached
    /// and the dirty guard never fires.
    #[test]
    fn highlight_plan_only_pins_when_the_form_is_dirty() {
        let mut st = st_with_rows(&["cn=a,ou=p,dc=x", "cn=b,ou=p,dc=x"]);
        st.current_leaf = Some("cn=gone,ou=p,dc=x".to_string());
        st.edit_form = Some(dirty_form("cn=gone,ou=p,dc=x"));
        assert_eq!(
            st.leaf_highlight_plan(),
            HighlightPlan::Pin("cn=a,ou=p,dc=x".to_string())
        );
    }

    #[test]
    fn highlight_plan_pins_the_first_row_when_no_entry_is_open() {
        let st = st_with_rows(&["cn=a,ou=p,dc=x"]);
        assert_eq!(
            st.leaf_highlight_plan(),
            HighlightPlan::Pin("cn=a,ou=p,dc=x".to_string())
        );
    }

    #[test]
    fn highlight_plan_clears_when_there_are_no_rows() {
        let mut st = st_with_rows(&[]);
        st.current_leaf = Some("cn=gone,ou=p,dc=x".to_string());
        assert_eq!(st.leaf_highlight_plan(), HighlightPlan::Clear);
    }
```

`st_with_rows` goes in this test module. **`dirty_form` goes in a new shared
fixture module** — Task 3's pane tests need it too, and Rust test modules are
private to their file. Create `src/ui/test_support.rs`, declare it from
`src/ui/mod.rs` as `#[cfg(test)] pub(crate) mod test_support;`, and `use
crate::ui::test_support::dirty_form;` from both call sites.

`dirty_form` mirrors the existing form fixtures around `state.rs:1733`; check that
file region and copy the field construction verbatim so it stays in step with
`EditForm`'s real shape.

```rust
    /// A `UiState` whose current branch is `ou=p,dc=x` and whose structure holds
    /// `dns` as its children, so `leaf_rows()` returns them in order.
    fn st_with_rows(dns: &[&str]) -> UiState {
        let mut inputs = vec![si("dc=x"), si("ou=p,dc=x")];
        inputs.extend(dns.iter().map(|d| si(d)));
        let structure = Structure::build("dc=x", inputs);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st = UiState::new_for_test(
            structure,
            schema,
            "dc=x".into(),
            Vec::new(),
            compile_tree_rules(&TreeConfig::default()),
        );
        st.current_branch = Some("ou=p,dc=x".to_string());
        st
    }

    /// An edit form on `dn` with one field whose current value differs from its
    /// baseline, so `is_dirty()` is true.
    fn dirty_form(dn: &str) -> crate::workflows::edit_form::EditForm {
        use crate::workflows::edit_form::{EditField, EditForm, FormMode};
        let mut f = EditField::new_for_test("cn", vec!["old".to_string()]);
        f.baseline = vec!["old".to_string()];
        f.values = vec!["edited".to_string()];
        EditForm {
            dn: dn.to_string(),
            fields: vec![f],
            mode: FormMode::Edit,
            ..EditForm::empty_for_test()
        }
    }
```

If `EditField::new_for_test` / `EditForm::empty_for_test` do not exist, build the
structs literally the way the fixture at `state.rs:1733` does — do **not** add new
test constructors just for this.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -j4 --lib highlight_plan`
Expected: FAIL — `cannot find type HighlightPlan in this scope`.

- [ ] **Step 3: Implement**

Add next to `GuardTarget` in `src/ui/state.rs`:

```rust
/// What a pane should do with its highlight after rebuilding its row source.
///
/// A rebuild must never look like an operator navigation, so the controller
/// answers with a **DN** — resolved against the freshly-built rows by the pane —
/// rather than a row index computed against the rows the rebuild just replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HighlightPlan {
    /// Highlight this DN. The form does not move.
    Pin(String),
    /// Highlight this DN and let the form follow it.
    Follow(String),
    /// Nothing to highlight.
    Clear,
}
```

Add to the `impl UiState` block containing `leaf_rows`:

```rust
    /// Where the entry list's highlight belongs after a rebuild, and whether the
    /// form should follow it. See the truth table in the design doc.
    ///
    /// `Follow` is produced only for a **clean** form: typing a find is
    /// navigation, so the form tracks the first hit — but never at the cost of
    /// unsaved edits, and never by raising the dirty guard mid-keystroke.
    pub fn leaf_highlight_plan(&self) -> HighlightPlan {
        let rows = self.leaf_rows();
        // NOT `rows.first()`: `leaf_rows` puts the branch's own `‹self›` row at
        // index 0 whenever no filter is active, so the literal first row is the
        // CONTAINER — the I4 trap this design exists to remove. A childless
        // container therefore yields `Clear`. The `Pin(current_leaf)` check below
        // still searches the full row set, so an operator who deliberately opened
        // the container's own entry keeps it.
        let branch = self.current_branch.as_deref();
        let is_self_row = |dn: &str| branch.is_some_and(|b| dn.eq_ignore_ascii_case(b));
        let Some((_, first_dn)) = rows.iter().find(|(_, dn)| !is_self_row(dn)) else {
            return HighlightPlan::Clear;
        };
        if let Some(cur) = self.current_leaf.as_deref() {
            if rows.iter().any(|(_, dn)| dn.eq_ignore_ascii_case(cur)) {
                return HighlightPlan::Pin(cur.to_string());
            }
            let dirty = self
                .edit_form
                .as_ref()
                .map(|f| f.is_dirty())
                .unwrap_or(false);
            if dirty {
                return HighlightPlan::Pin(first_dn.clone());
            }
            return HighlightPlan::Follow(first_dn.clone());
        }
        HighlightPlan::Pin(first_dn.clone())
    }
```

DN comparison is `eq_ignore_ascii_case` to match how `state.rs` compares DNs
elsewhere.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -j4 --lib highlight_plan`
Expected: PASS, 5 tests.

- [ ] **Step 5: Commit**

```bash
git add src/ui/state.rs
git commit -m "feat(state): HighlightPlan — the controller answers with a DN, not a row

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: `branch_highlight_plan` for the tree

**Files:**
- Modify: `src/ui/state.rs` (same impl block as Task 1)
- Test: `src/ui/state.rs`

**Interfaces:**
- Consumes: `HighlightPlan` from Task 1.
- Produces: `pub fn branch_highlight_plan(&self) -> HighlightPlan` — returns only
  `Pin` or `Clear`, **never** `Follow`.
- Consumed by: Task 4.

- [ ] **Step 1: Write the failing tests**

```rust
    /// The tree shares the enum but must never navigate the form on its own:
    /// a branch change is always operator-driven or an explicit `commit_branch`.
    #[test]
    fn branch_highlight_plan_pins_the_current_branch() {
        let mut st = st_with_rows(&[]);
        st.branch_dns = vec!["dc=x".to_string(), "ou=p,dc=x".to_string()];
        st.current_branch = Some("ou=p,dc=x".to_string());
        assert_eq!(
            st.branch_highlight_plan(),
            HighlightPlan::Pin("ou=p,dc=x".to_string())
        );
    }

    #[test]
    fn branch_highlight_plan_clears_when_the_branch_vanished() {
        let mut st = st_with_rows(&[]);
        st.branch_dns = vec!["dc=x".to_string()];
        st.current_branch = Some("ou=gone,dc=x".to_string());
        assert_eq!(st.branch_highlight_plan(), HighlightPlan::Clear);
    }

    #[test]
    fn branch_highlight_plan_clears_rather_than_falling_back_to_a_first_row() {
        let mut st = st_with_rows(&[]);
        st.branch_dns = vec!["dc=x".to_string(), "ou=p,dc=x".to_string()];
        st.current_branch = Some("ou=gone,dc=x".to_string());
        // Assert the FULL value, not merely "not Follow": the likely wrong
        // implementation is a copy-paste of the leaf policy, which falls back to
        // the first row and would return Pin("dc=x") — and a !matches!(Follow)
        // check would wave that straight through. This is the case where the two
        // policies genuinely diverge, so it is the one worth pinning down.
        assert_eq!(
            st.branch_highlight_plan(),
            HighlightPlan::Clear,
            "a tree rebuild must never navigate the form, nor fall back to a row"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -j4 --lib branch_highlight_plan`
Expected: FAIL — `no method named branch_highlight_plan`.

- [ ] **Step 3: Implement**

```rust
    /// Where the tree's highlight belongs after a rebuild. Only ever `Pin` or
    /// `Clear`: unlike the entry list, the tree never moves the form by itself.
    pub fn branch_highlight_plan(&self) -> HighlightPlan {
        let Some(cur) = self.current_branch.as_deref() else {
            return HighlightPlan::Clear;
        };
        if self.branch_dns.iter().any(|d| d.eq_ignore_ascii_case(cur)) {
            HighlightPlan::Pin(cur.to_string())
        } else {
            HighlightPlan::Clear
        }
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -j4 --lib branch_highlight_plan`
Expected: PASS, 3 tests.

- [ ] **Step 5: Commit**

```bash
git add src/ui/state.rs
git commit -m "feat(state): branch_highlight_plan — the tree pins or clears, never follows

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Leaf pane consumes the plan; `set_leaf_row` deleted

**Files:**
- Modify: `src/ui/panes/leaf.rs:73-99` (`repopulate`), `:211-222` (`apply_set_row` — delete), `:292` (call site)
- Modify: `src/ui/state.rs:186` (delete `set_leaf_row`), plus its writes at `:1268`, `:1278`, `:1381`, `:1420` and initialisers at `:291`, `:1634`
- Modify: `src/ui/app.rs:81` (`apply_cancelled_guard_save`), `:271` (guard Stay)
- Test: `src/ui/panes/leaf.rs`

**Interfaces:**
- Consumes: `UiState::leaf_highlight_plan` (Task 1).
- Produces: `LeafPane::apply_highlight_plan(&mut self, ctx: &mut Context)`, called
  at the end of `repopulate`. No controller-side push field remains.

- [ ] **Step 1: Write the failing tests**

```rust
    /// A rebuild that moves the pinned entry to a different index must follow the
    /// DN — and must NOT look like a navigation. This is the general form of the
    /// I1 fix: the index is never computed against the pre-rebuild row source.
    ///
    /// The shift is produced by REMOVING a row that precedes the pinned one
    /// (another admin deleting an entry). Do not try to produce it by upserting a
    /// new entry: `leaf_rows` is `‹self›` followed by `leaves_of`, which returns
    /// children in INSERTION order with no sort, so an upsert appends last and the
    /// pinned row's index would not move at all — the test would pass against a
    /// stale-index implementation and prove nothing.
    #[test]
    fn rebuild_keeps_the_highlight_on_the_pinned_dn_without_reporting() {
        let shared = shared_with_rows(&["cn=a,ou=p,dc=x", "cn=b,ou=p,dc=x"]);
        shared.borrow_mut().current_leaf = Some("cn=b,ou=p,dc=x".to_string());
        let mut pane = LeafPane::new(Rect::new(0, 0, 30, 10), shared.clone());
        refresh(&mut pane);
        shared.borrow_mut().requested_leaf = None;
        // Precondition: rows are [‹self› ou=p, cn=a, cn=b], so cn=b is row 2.
        assert_eq!(
            pane.selected_row_for_test(),
            2,
            "test premise: the pinned entry starts at row 2 (row 0 is ‹self›)"
        );

        // cn=a disappears → rows become [‹self› ou=p, cn=b] → cn=b moves to row 1.
        {
            let mut st = shared.borrow_mut();
            st.structure.remove("cn=a,ou=p,dc=x");
            st.list_dirty = true;
        }
        refresh(&mut pane);

        let st = shared.borrow();
        assert_eq!(
            pane.selected_row_for_test(),
            1,
            "the highlight follows the DN across the renumbering"
        );
        assert_eq!(
            st.requested_leaf, None,
            "a rebuild must never be reported as an operator navigation"
        );
    }

    /// The modal-mid-keystroke bug, at pane level: a find that excludes the open
    /// entry moves the highlight but leaves the form pinned, so the controller is
    /// never asked to navigate and the dirty guard cannot fire.
    #[test]
    fn find_rebuild_does_not_move_a_dirty_form() {
        let shared = shared_with_rows(&["cn=a,ou=p,dc=x", "cn=b,ou=p,dc=x"]);
        {
            let mut st = shared.borrow_mut();
            st.current_leaf = Some("cn=b,ou=p,dc=x".to_string());
            st.edit_form = Some(dirty_form("cn=b,ou=p,dc=x"));
            st.leaf_search_rows = Some(vec!["cn=a,ou=p,dc=x".to_string()]);
            st.search = "a".to_string();
            st.list_dirty = true;
        }
        let mut pane = LeafPane::new(Rect::new(0, 0, 30, 10), shared.clone());
        refresh(&mut pane);

        assert_eq!(
            shared.borrow().requested_leaf,
            None,
            "a dirty form is never dragged along by a find"
        );
    }

    /// The clean counterpart: find-follow is deliberate, so the form tracks the
    /// first hit.
    #[test]
    fn find_rebuild_follows_the_first_hit_when_clean() {
        let shared = shared_with_rows(&["cn=a,ou=p,dc=x", "cn=b,ou=p,dc=x"]);
        {
            let mut st = shared.borrow_mut();
            st.current_leaf = Some("cn=b,ou=p,dc=x".to_string());
            st.leaf_search_rows = Some(vec!["cn=a,ou=p,dc=x".to_string()]);
            st.search = "a".to_string();
            st.list_dirty = true;
        }
        let mut pane = LeafPane::new(Rect::new(0, 0, 30, 10), shared.clone());
        refresh(&mut pane);

        assert_eq!(
            shared.borrow().requested_leaf.as_ref().map(|(dn, _)| dn.as_str()),
            Some("cn=a,ou=p,dc=x"),
            "a clean form follows the find to the first hit"
        );
    }
```

**`leaf.rs`'s test module has no helpers** — `shared_with_rows`, `refresh` and
`si` do not exist there, and the existing tests build `StructureInput` literally
and construct a `Context` by hand (see the I4 test at `leaf.rs:861`). Write these
three small helpers at the top of that test module and use them for the new tests:

```rust
    /// A `StructureInput` for `dn`, with the RDN value as its `cn` so the row
    /// renders a label. Note: `state.rs`'s test module has a two-argument `si`;
    /// this one is local to this file.
    fn si(dn: &str) -> StructureInput {
        let cn = dn.split('=').nth(1).and_then(|s| s.split(',').next());
        StructureInput {
            dn: dn.into(),
            cn: cn.map(str::to_string),
            description: None,
            object_classes: vec![],
            attrs: BTreeMap::new(),
        }
    }

    /// Shared state on branch `ou=p,dc=x` holding `dns` as its leaves.
    fn shared_with_rows(dns: &[&str]) -> Shared {
        let mut inputs = vec![si("dc=x"), si("ou=p,dc=x")];
        inputs.extend(dns.iter().map(|d| si(d)));
        let structure = Structure::build("dc=x", inputs);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.current_branch = Some("ou=p,dc=x".into());
        st.list_dirty = true;
        Rc::new(RefCell::new(st))
    }

    /// Drive one `REFRESH` broadcast through the pane.
    fn refresh(pane: &mut LeafPane) {
        let mut out: VecDeque<Event> = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<tv::Deferred> = Vec::new();
        let mut ctx = tv::Context::new(&mut out, &mut timers, 0, &mut deferred);
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut ev, &mut ctx);
    }
```

Set `list_dirty = true` before each `refresh` where a rebuild is expected.
**Do not refactor the existing I4 test onto these helpers** — it is a regression
test for a shipped bug and stays as it is.

**`dirty_form` is defined in `state.rs`'s test module (Task 1) and is not visible
here** — Rust test modules are private to their file. Hoist it instead: put it in
a `#[cfg(test)] pub(crate) mod test_support;` (new file
`src/ui/test_support.rs`, declared from `src/ui/mod.rs`) and have both Task 1's and
this task's tests `use crate::ui::test_support::dirty_form;`. Do not copy-paste the
fixture into a second file.

Add `selected_row_for_test(&mut self) -> i32` beside the other `#[cfg(test)]`
accessors at `leaf.rs:151-177`:

```rust
    #[cfg(test)]
    pub(crate) fn selected_row_for_test(&mut self) -> i32 {
        match self.group.child_mut(self.list_id).and_then(|v| v.value()) {
            Some(FieldValue::Int(i)) => i,
            _ => -1,
        }
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -j4 --lib leaf::tests`
Expected: FAIL — `no method named selected_row_for_test`, then the highlight
assertions fail because `repopulate` still resets `last_sel = -1` and reports row 0.

- [ ] **Step 3: Implement**

Replace `repopulate`'s tail (`leaf.rs:96-98`) and delete `apply_set_row`:

```rust
        self.last_search = search;
        self.apply_highlight_plan(ctx);
        self.sync_scrollbar(ctx);
    }

    /// Resolve the controller's [`HighlightPlan`] against the rows just installed,
    /// set the widget focus, and resync `last_sel` **silently** so the move is not
    /// re-reported as an operator navigation. `Follow` additionally asks the
    /// controller to move the form, which it only ever does for a clean form.
    fn apply_highlight_plan(&mut self, ctx: &mut Context) {
        let (plan, rows) = {
            let st = self.state.borrow();
            (st.leaf_highlight_plan(), st.leaf_rows())
        };
        let dn = match &plan {
            HighlightPlan::Pin(dn) | HighlightPlan::Follow(dn) => dn.clone(),
            HighlightPlan::Clear => {
                self.last_sel = -1;
                return;
            }
        };
        let row = rows
            .iter()
            .position(|(_, d)| d.eq_ignore_ascii_case(&dn))
            .map(|i| i as i32)
            .unwrap_or(-1);
        if row >= 0 {
            if let Some(list) = self.group.child_mut(self.list_id) {
                list.set_value_ctx(FieldValue::Int(row), ctx);
            }
        }
        // Silently: `report_selection` compares against `last_sel`, so syncing it
        // here is what makes the rebuild invisible to the controller.
        self.last_sel = row;
        if let HighlightPlan::Follow(dn) = plan {
            let ocs = {
                let st = self.state.borrow();
                st.structure
                    .get(&dn)
                    .map(|n| n.object_classes.clone())
                    .unwrap_or_default()
            };
            self.state.borrow_mut().request_leaf(dn, ocs);
        }
    }
```

Delete the `self.apply_set_row(ctx);` call at `leaf.rs:292`, leaving
`self.report_selection();`.

Delete `pub set_leaf_row: Option<i32>` (`state.rs:186`), its two initialisers and
its four assignments. At `state.rs:1268` and `:1278` (the I1/I4 sites) the
assignment goes away entirely — `list_dirty = true` already makes the pane rebuild,
and the rebuild now resolves the highlight itself. Do the same at `:1381`, `:1420`.

In `src/ui/app.rs`, `apply_cancelled_guard_save` loses its first line and
`apply_branch_guard_stay`'s leaf counterpart at `:271` becomes a rebuild request:

```rust
pub(crate) fn apply_cancelled_guard_save(st: &mut crate::ui::state::UiState) {
    // The highlight is re-resolved from `leaf_highlight_plan` on the next
    // rebuild, which returns `Pin(current_leaf)` while the form is pinned.
    st.list_dirty = true;
    st.guard_target = None;
    st.pending_nav = None;
}
```

and at the `GuardDecision::Stay` arm, replace `st.set_leaf_row = st.current_leaf_row();`
with `st.list_dirty = true;`.

- [ ] **Step 4: Run the full suite**

Run: `cargo test -j4 --lib`
Expected: PASS. The I1 and I4 regression tests must still pass; if an assertion
referenced `set_leaf_row`, re-express it as the pane's selected row or the absence
of `requested_leaf`. Do not delete either test.

- [ ] **Step 5: Commit**

```bash
git add src/ui/panes/leaf.rs src/ui/state.rs src/ui/app.rs
git commit -m "refactor(leaf): resolve the highlight by DN after the rebuild

Deletes set_leaf_row: an index computed against the row source the rebuild
then replaced. The pane now resolves the controller's HighlightPlan against
the rows it just installed and resyncs last_sel silently, so a rebuild can
no longer be reported as an operator navigation.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Tree pane consumes the plan; `set_tree_row` deleted

**Files:**
- Modify: `src/ui/panes/tree.rs:257-301` (`handle_event`), `rebuild` (~`:521-560`)
- Modify: `src/ui/state.rs:192` (delete `set_tree_row`) and its initialisers `:293`, `:1636`
- Modify: `src/ui/app.rs:88-91` (delete `apply_branch_guard_stay`), call sites `:243`, `:269`
- Test: `src/ui/panes/tree.rs`

**Interfaces:**
- Consumes: `UiState::branch_highlight_plan` (Task 2).
- Produces: `TreePane::apply_highlight_plan(&mut self, ctx: &mut Context)`.

- [ ] **Step 1: Write the failing test**

```rust
    /// Follow-up #1: the guard "Stay" snap used to be an index resolved against
    /// the PRE-rebuild `branch_dns`. With a rebuild pending, that index described
    /// the old numbering. Resolving by DN after the rebuild removes the class.
    #[test]
    fn guard_stay_snaps_by_dn_across_a_pending_rebuild() {
        let inputs = vec![si("dc=x"), si("ou=b,dc=x"), si("cn=1,ou=b,dc=x")];
        let structure = Structure::build("dc=x", inputs);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st = UiState::new_for_test(
            structure,
            schema,
            "dc=x".into(),
            Vec::new(),
            compile_tree_rules(&TreeConfig::default()),
        );
        let (root, dns) = build_branch_nodes(&st, 40);
        st.branch_dns = dns; // ["dc=x", "ou=b,dc=x"]
        st.current_branch = Some("ou=b,dc=x".into());
        let shared: std::rc::Rc<std::cell::RefCell<UiState>> =
            std::rc::Rc::new(std::cell::RefCell::new(st));
        let mut pane = TreePane::new(Rect::new(0, 0, 30, 10), root, shared.clone());

        // A new container sorts ahead of ou=b, so ou=b moves from row 1 to row 2 —
        // and the rebuild has NOT happened yet when the guard resolves.
        {
            let mut st = shared.borrow_mut();
            st.structure.upsert(si("cn=1,ou=a,dc=x"));
            st.tree_dirty = true;
        }
        refresh_tree(&mut pane);

        let st = shared.borrow();
        assert_eq!(
            st.branch_dns,
            vec![
                "dc=x".to_string(),
                "ou=a,dc=x".to_string(),
                "ou=b,dc=x".to_string()
            ],
        );
        assert_eq!(
            pane.selected_row_for_test(),
            2,
            "the highlight resolves to ou=b's NEW row, not its pre-rebuild index"
        );
        assert_eq!(
            st.requested_branch, None,
            "restoring the highlight must not look like a navigation"
        );
    }
```

Reuse the `refresh_tree` helper pattern already used by
`refresh_with_tree_dirty_rebuilds_and_keeps_the_selected_dn` (`tree.rs:528`), and
add a `selected_row_for_test` accessor reading the outline's `value()`.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -j4 --lib tree::tests::guard_stay_snaps_by_dn`
Expected: FAIL — `no method named selected_row_for_test`.

- [ ] **Step 3: Implement**

In `handle_event`, delete the `set_tree_row` block (`tree.rs:274-281`) and call the
plan resolver right after the rebuild:

```rust
        if needs_rebuild {
            self.rebuild(ctx);
            self.state.borrow_mut().tree_dirty = false;
        }
        self.apply_highlight_plan(ctx);
```

```rust
    /// Resolve the controller's [`HighlightPlan`] against the DFS index just
    /// rebuilt and resync `last_sel` silently. The tree never produces `Follow`,
    /// so this only ever moves the highlight — never the form.
    fn apply_highlight_plan(&mut self, ctx: &mut Context) {
        let (plan, dns) = {
            let st = self.state.borrow();
            (st.branch_highlight_plan(), st.branch_dns.clone())
        };
        let row = match plan {
            HighlightPlan::Pin(dn) | HighlightPlan::Follow(dn) => dns
                .iter()
                .position(|d| d.eq_ignore_ascii_case(&dn))
                .map(|i| i as i32)
                .unwrap_or(-1),
            HighlightPlan::Clear => -1,
        };
        if row >= 0 {
            if let Some(outline) = self.outline_mut() {
                tv::widgets::outline::adjust_focus(outline, row, ctx);
            }
        }
        // Finding 2: `ov_update` re-clamps `foc` internally, so `last_sel` must be
        // resynced unconditionally — including when the branch vanished (row -1),
        // or the pane reports a branch the operator never selected.
        self.last_sel = if row >= 0 {
            row
        } else {
            match self.outline_mut().and_then(|o| o.value()) {
                Some(FieldValue::Int(i)) => i,
                _ => 0,
            }
        };
    }
```

Delete `apply_branch_guard_stay` from `app.rs` and replace both call sites
(`:243`, `:269`) with `st.tree_dirty = true;` so the highlight is re-resolved on
the next rebuild. Delete `pub set_tree_row: Option<i32>` and its initialisers.

- [ ] **Step 4: Run the suite**

Run: `cargo test -j4 --lib`
Expected: PASS, including
`refresh_with_tree_dirty_rebuilds_and_keeps_the_selected_dn` and the Finding-2
vanished-branch test, both unchanged.

- [ ] **Step 5: Commit**

```bash
git add src/ui/panes/tree.rs src/ui/state.rs src/ui/app.rs
git commit -m "refactor(tree): resolve the highlight by DN after the rebuild

Deletes set_tree_row and apply_branch_guard_stay. Guard 'Stay' is now just
're-resolve the highlight', which returns Pin(current_branch) by
construction — no index survives a rebuild it was not computed against.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: `begin_operator_action` — one named status policy

**Files:**
- Modify: `src/ui/state.rs` (new method; call sites `commit_branch` ~`:1191`, `reconcile_selection` `:1127`, `set_leaf_search` `:1214`, `apply_commit`)
- Modify: `src/ui/app.rs:442` (`open_create`), the modal-cancel path, the guard `Stay` arm `:265-275`
- Test: `src/ui/state.rs`, `src/ui/app.rs`

**Interfaces:**
- Produces: `pub fn begin_operator_action(&mut self)` on `UiState`.

- [ ] **Step 1: Write the failing tests**

```rust
    /// Follow-up #2: a status message must not outlive the action it describes.
    /// Opening the create form is a new operator action, so "Saved." goes.
    #[test]
    fn open_create_clears_a_stale_status() {
        let mut st = st_with_rows(&["cn=a,ou=p,dc=x"]);
        st.status = "Saved.".to_string();
        st.begin_operator_action();
        assert!(st.status.is_empty());
    }

```

Write only that one test. The policy's other half — that clearing happens at the
call site and never inside `reread`, where a rename looks like a navigation and
would eat its own "Saved." — is already covered by
`a_rename_keeps_its_saved_confirmation` (added by `c016f2a`). Locate that test and
confirm it still passes; do not duplicate or modify it.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -j4 --lib open_create_clears_a_stale_status`
Expected: FAIL — `no method named begin_operator_action`.

- [ ] **Step 3: Implement**

```rust
    /// The operator started a new action: whatever the status line was reporting
    /// described the previous one.
    ///
    /// Call this at the **call site** of each operator action, never inside a
    /// shared helper — `reread` is reached both by a navigation and by a rename's
    /// post-write re-read, and clearing there made every rename eat its own
    /// "Saved." (fixed in `c016f2a`).
    pub fn begin_operator_action(&mut self) {
        self.status.clear();
    }
```

Replace the bare `self.status.clear()` at the four existing sites with
`self.begin_operator_action()`, and add the call to `open_create`, the modal-cancel
path, and the guard `Stay` arm.

- [ ] **Step 4: Run the suite**

Run: `cargo test -j4 --lib`
Expected: PASS, including `a_rename_keeps_its_saved_confirmation`.

- [ ] **Step 5: Commit**

```bash
git add src/ui/state.rs src/ui/app.rs
git commit -m "refactor(state): name the status-clearing policy, apply it uniformly

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: `note_entry_vanished` and `GuardTarget::Vanished`

**Files:**
- Modify: `src/ui/state.rs:26-29` (`GuardTarget`), new method, reload/rescan call sites (`reload_structure` ~`:1454`, the rename rescan ~`:694`)
- Test: `src/ui/state.rs`

**Interfaces:**
- Produces: `GuardTarget::Vanished(String)` and
  `pub fn note_entry_vanished(&mut self, dn: &str)`.
- Consumed by: Task 7.

- [ ] **Step 1: Write the failing tests**

```rust
    /// A clean form whose entry disappeared is cleared, and the operator is told.
    #[test]
    fn a_vanished_entry_clears_a_clean_form_and_says_so() {
        let mut st = st_with_rows(&["cn=a,ou=p,dc=x"]);
        st.current_leaf = Some("cn=gone,ou=p,dc=x".to_string());
        st.note_entry_vanished("cn=gone,ou=p,dc=x");
        assert!(st.edit_form.is_none(), "the form is cleared");
        assert!(
            st.status.contains("no longer"),
            "status reports the disappearance, got {:?}",
            st.status
        );
        assert_eq!(st.guard_target, None, "a clean form asks nothing");
    }

    /// Unsaved work is never destroyed without asking: the guard is raised instead.
    #[test]
    fn a_vanished_entry_asks_before_discarding_unsaved_edits() {
        let mut st = st_with_rows(&["cn=a,ou=p,dc=x"]);
        st.current_leaf = Some("cn=gone,ou=p,dc=x".to_string());
        st.edit_form = Some(dirty_form("cn=gone,ou=p,dc=x"));
        st.note_entry_vanished("cn=gone,ou=p,dc=x");
        assert_eq!(
            st.guard_target,
            Some(GuardTarget::Vanished("cn=gone,ou=p,dc=x".to_string()))
        );
        assert!(st.edit_form.is_some(), "the edits survive until answered");
    }

    /// Absence from a find is NOT evidence of vanishing — only a reload or a
    /// no-such-object read is. A find must never trigger this path.
    #[test]
    fn a_reload_that_keeps_the_entry_does_not_report_it_vanished() {
        let mut st = st_with_rows(&["cn=a,ou=p,dc=x"]);
        st.current_leaf = Some("cn=a,ou=p,dc=x".to_string());
        st.note_missing_after_reload();
        assert!(st.status.is_empty());
        assert_eq!(st.guard_target, None);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -j4 --lib vanished`
Expected: FAIL — `no variant named Vanished`.

- [ ] **Step 3: Implement**

```rust
pub enum GuardTarget {
    Leaf(String, Vec<String>),
    Branch(String),
    /// The entry the form is editing is gone from the directory and the form has
    /// unsaved edits. Carries the vanished DN.
    Vanished(String),
}
```

```rust
    /// The entry `dn` is gone from the directory. Called ONLY on hard evidence —
    /// absence from `structure` after a reload or rescan, or a read returning
    /// no-such-object. Absence from a find's hits is not evidence: a find excludes
    /// rows routinely.
    pub fn note_entry_vanished(&mut self, dn: &str) {
        let dirty = self
            .edit_form
            .as_ref()
            .map(|f| f.is_dirty())
            .unwrap_or(false);
        self.status = format!("{dn} is no longer in the directory.");
        if dirty {
            self.guard_target = Some(GuardTarget::Vanished(dn.to_string()));
        } else {
            self.edit_form = None;
            self.current_leaf = None;
            self.form_needs_render = true;
        }
        self.list_dirty = true;
    }

    /// After a reload or rescan rebuilt `structure`: if the entry the form is on
    /// is no longer there, report it vanished.
    pub fn note_missing_after_reload(&mut self) {
        let Some(cur) = self.current_leaf.clone() else {
            return;
        };
        if self.structure.get(&cur).is_none() {
            self.note_entry_vanished(&cur);
        }
    }
```

Call `note_missing_after_reload()` at the end of the successful `reload_structure`
arm and after the container-rename rescan. Verify `form_needs_render` is the
correct field name in this codebase before using it — grep it; if the form pane
uses a different signal, use that one.

- [ ] **Step 4: Run the suite**

Run: `cargo test -j4 --lib`
Expected: PASS. Every `match` on `GuardTarget` must now handle `Vanished`; clippy
will point at any that do not.

- [ ] **Step 5: Commit**

```bash
git add src/ui/state.rs
git commit -m "feat(state): note_entry_vanished — a gone entry is a signal, not an absent row

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: The Vanished dialog — Keep editing / Discard / Re-create

**Blocked on:** tvision 0.13 (`Dialog::button_row` auto-sizing). Verify
`Cargo.toml` pins a version providing it before starting; if it does not, stop and
report rather than hand-laying a button row.

**Files:**
- Create: `src/ui/dialog/vanished.rs`
- Modify: `src/ui/dialog/guard.rs` (drop its `button_row` fork)
- Modify: `src/ui/dialog/mod.rs` (module decl, `VanishedDecision`, `vanished_decision`)
- Modify: `src/ui/app.rs:229-276` (`GUARD_NAV` dispatch)
- Modify: `src/ui/state.rs:793` (`WriteOutcome::Created` sets a status)
- Test: `src/ui/dialog/mod.rs`, `src/ui/app.rs`, `src/ui/state.rs`

**Interfaces:**
- Consumes: `GuardTarget::Vanished` (Task 6).
- Produces: `vanished::build(dn: &str) -> (Box<dyn View>, ViewId)` and
  `pub enum VanishedDecision { Recreate, Discard, KeepEditing }`.

**Note:** Re-create cannot reuse `do_create` — `plan_create` requires
`FormMode::Create { profile_idx, container }` and a vanished entry's form is in
`FormMode::Edit`. Build the attributes from `form.to_edit_entry()` and call
`write_flow.submit_create` at the vanished DN directly.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn vanished_decision_maps_the_three_commands() {
        assert_eq!(vanished_decision(Command::YES), VanishedDecision::Recreate);
        assert_eq!(vanished_decision(Command::NO), VanishedDecision::Discard);
        assert_eq!(
            vanished_decision(Command::CANCEL),
            VanishedDecision::KeepEditing
        );
    }
```

and in `app.rs`'s tests, covering the attribute assembly (the part worth testing —
the modal itself is not unit-testable):

```rust
    /// Re-create submits the form's CURRENT values at the vanished DN. Empty
    /// fields are dropped: LDAP rejects an attribute with no values.
    ///
    /// If `EditField::new_for_test` / `EditForm::empty_for_test` do not exist,
    /// build the structs literally the way the fixture at `state.rs:1733` does —
    /// do not add test constructors just for this.
    #[test]
    fn recreate_attrs_take_the_forms_current_values() {
        use crate::workflows::edit_form::{EditField, EditForm, FormMode};
        let mut cn = EditField::new_for_test("cn", vec!["alice".to_string()]);
        cn.values = vec!["alice".to_string()];
        let empty = EditField::new_for_test("description", vec![]);
        let form = EditForm {
            dn: "cn=alice,ou=p,dc=x".to_string(),
            fields: vec![cn, empty],
            mode: FormMode::Edit,
            ..EditForm::empty_for_test()
        };
        let attrs = recreate_attrs(&form);
        assert_eq!(attrs.get("cn"), Some(&vec!["alice".to_string()]));
        assert!(
            !attrs.contains_key("description"),
            "an empty attribute must not be sent"
        );
    }
```

and in `state.rs`'s tests, for the silent-create bug:

```rust
    /// A create has never reported itself: `Created` set no status, while `Saved`
    /// set "Saved." — invisible until Spec 2 made the status line render at all.
    /// The comment at the re-read call claims a status "set for this create"
    /// survives; nothing ever set one.
    #[test]
    fn a_successful_create_says_so() {
        let mut st = st_with_rows(&["cn=a,ou=p,dc=x"]);
        st.apply_write_outcome(WriteOutcome::Created {
            dn: "cn=new,ou=p,dc=x".into(),
            quit_after: false,
        });
        assert_eq!(st.status, "Created cn=new,ou=p,dc=x.");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -j4 --lib vanished_decision recreate_attrs`
Expected: FAIL — `cannot find function vanished_decision`.

- [ ] **Step 3: Implement**

`src/ui/dialog/vanished.rs` — use `Dialog::button_row`, which as of tvision
0.13 sizes its faces to the widest label. **Do not hand-lay a button row**, and
do not declare width constants: "Keep editing" is 12 columns, so the row sizes
itself to 16.

```rust
//! The entry being edited vanished from the directory: Re-create / Discard /
//! Keep editing over a form whose DN no longer exists.

use tvision_rs::{ButtonFlags, Command, Dialog, Rect, StaticText, View, ViewId};
use tvision_rs::dialog::ButtonRowAlign;

const DLG_W: i32 = 64;
const DLG_H: i32 = 9;

/// Build the vanished-entry dialog. Returns the view and the "Keep editing"
/// button id to focus on open, so Enter takes the only non-destructive choice.
/// The dialog returns `Command::YES` (Re-create), `Command::NO` (Discard), or
/// `Command::CANCEL` (Keep editing).
pub fn build(dn: &str) -> (Box<dyn View>, ViewId) {
    let mut dlg = Dialog::new(
        Rect::new(0, 0, DLG_W, DLG_H),
        Some("Entry removed".to_string()),
    );
    dlg.state_mut().options.center_x = true;
    dlg.state_mut().options.center_y = true;
    dlg.insert_child(Box::new(StaticText::new(
        Rect::new(2, 2, 62, 5),
        format!("{dn}\nis no longer in the directory. You have unsaved changes."),
    )));
    let ids = dlg.button_row(
        &[
            ("~R~e-create", Command::YES, ButtonFlags::new()),
            ("~D~iscard", Command::NO, ButtonFlags::new()),
            (
                "~K~eep editing",
                Command::CANCEL,
                ButtonFlags {
                    default: true,
                    ..ButtonFlags::new()
                },
            ),
        ],
        ButtonRowAlign::Right,
    );
    (Box::new(dlg), ids[2])
}
```

**Also simplify `src/ui/dialog/guard.rs` in this task.** It hand-lays its row
solely because the old `button_row` forced a 10-column face; that reason is gone.
Replace its manual layout and its four local constants (`BTN_W`, `BTN_H`,
`BTN_GAP`, `MARGIN_RIGHT`, `BUTTON_ROW_FROM_BOTTOM`) with one `button_row` call
using the same three specs it has today, keeping Save as the focused/default
button. Its existing tests must pass unchanged.

In `mod.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VanishedDecision {
    Recreate,
    Discard,
    KeepEditing,
}

pub fn vanished_decision(answer: Command) -> VanishedDecision {
    if answer == Command::YES {
        VanishedDecision::Recreate
    } else if answer == Command::NO {
        VanishedDecision::Discard
    } else {
        VanishedDecision::KeepEditing
    }
}
```

In `app.rs`, add a `GuardTarget::Vanished` arm to the `GUARD_NAV` dispatch that
runs the new dialog instead of `run_guard`:

```rust
/// The form's current values, ready for an ADD. Empty attributes are dropped —
/// LDAP rejects an attribute carrying no values.
fn recreate_attrs(
    form: &crate::workflows::edit_form::EditForm,
) -> std::collections::BTreeMap<String, Vec<String>> {
    form.to_edit_entry()
        .attrs
        .into_iter()
        .filter(|(_, v)| !v.is_empty())
        .collect()
}
```

```rust
            Some(GuardTarget::Vanished(dn)) => {
                let (view, keep) = crate::ui::dialog::vanished::build(&dn);
                let answer = prog.exec_view_focused(view, keep);
                match crate::ui::dialog::vanished_decision(answer) {
                    VanishedDecision::KeepEditing => {}
                    VanishedDecision::Discard => {
                        // No re-read: the DN is gone, so the read would fail.
                        let mut st = state.borrow_mut();
                        st.edit_form = None;
                        st.current_leaf = None;
                        st.form_needs_render = true;
                        st.list_dirty = true;
                    }
                    VanishedDecision::Recreate => {
                        let Some(attrs) = state.borrow().edit_form.as_ref().map(recreate_attrs)
                        else {
                            return;
                        };
                        // Behind the same LDIF preview as every other write: this
                        // resurrects an entry someone deliberately deleted, and it
                        // must not be the one unconfirmed write in the app. Borrow
                        // is dropped before exec_view — the draw path borrows too.
                        let ldif = crate::ldap::ldif::render_add(&dn, &attrs);
                        let (view, save) = crate::ui::dialog::confirm::build(&ldif);
                        if prog.exec_view_focused(view, save) != Command::OK {
                            return; // cancel: keep the form and its edits
                        }
                        let mut st = state.borrow_mut();
                        let crate::ui::state::UiState {
                            worker, write_flow, ..
                        } = &mut *st;
                        if let Some(w) = worker.as_ref() {
                            // rc 68 (entryAlreadyExists) rejects this safely if
                            // another client re-created the DN meanwhile.
                            let _ = write_flow.submit_create(w, &dn, attrs, false);
                        }
                    }
                }
                state.borrow_mut().guard_target = None;
            }
```

In `src/ui/state.rs`, the `WriteOutcome::Created` arm (`:793`) gains a status
alongside the existing `current_leaf` / `list_dirty` assignments, before the
`reread` call:

```rust
                self.status = format!("Created {dn}.");
```

The message is deliberately **not** specialised to "Re-created": telling the two
apart means threading a flag through `WriteIntent`/`WriteOutcome`, and that async
correlation surface is where Spec 2's worst defect came from. "Created X." is
accurate for both. The existing comment at `:814-816` — which claims a status set
for this create survives the re-read — becomes true for the first time; leave it.

- [ ] **Step 4: Run the suite**

Run: `cargo test -j4 --lib`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/ui/dialog/vanished.rs src/ui/dialog/mod.rs src/ui/app.rs src/ui/state.rs
git commit -m "feat(ui): ask before losing edits to an entry that vanished

Re-create goes behind the same LDIF preview as every other write, and a
successful create finally reports itself — Created set no status at all,
which nobody could see until the status line began rendering.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 8: Delete `leaf_search_truncated`, docs, and the gate

**Files:**
- Modify: `src/ui/state.rs` (`:129`, `:270`, `:805`, `:1197`, `:1219`, `:1253`, `:1274`, `:1407`, `:1613`, and the two tests at `:2932`, `:3016`)
- Modify: `CHANGES.md`, `docs/src/concepts/live-data.md`

- [ ] **Step 1: Rewrite the two tests to assert the observable behaviour**

The truncation notice reaches the operator through `status`, so assert that:

```rust
        assert!(
            st.status.contains("Showing the first"),
            "the truncation notice reaches the operator via the status line"
        );
```

Run: `cargo test -j4 --lib leaf_search`
Expected: PASS (the field still exists at this point).

- [ ] **Step 2: Delete the field and every assignment**

Remove `pub leaf_search_truncated: bool`, both initialisers and all six
assignments. Nothing reads it.

- [ ] **Step 3: Run the gate**

Run: `make check`
Expected: PASS — fmt clean, clippy silent, all tests green. Clippy is the check
that matters here: a genuinely dead field would have been flagged, so confirm
nothing else referenced it.

- [ ] **Step 4: Update the docs**

`CHANGES.md`, under the current unreleased section:

```markdown
- The entry list and tree no longer move the form on their own. Rebuilding a
  list (after a create, rename, reload or find) restores the highlight by DN
  instead of by row number, so it can no longer land on the wrong entry.
- Typing a find query no longer interrupts you with the "unsaved changes"
  prompt: the list narrows and highlights, but a form with unsaved edits stays
  put until you navigate deliberately.
- When the entry you are editing is deleted by someone else, edaptor now says
  so instead of silently moving you elsewhere — and if you have unsaved edits it
  asks whether to keep them, discard them, or re-create the entry.
```

In `docs/src/concepts/live-data.md`, add this after the paragraph describing what
Alt+R does:

```markdown
If the entry you were editing is gone when the projection is rebuilt — because
another client deleted or renamed it — edaptor tells you rather than quietly
moving you somewhere else. With no unsaved changes the form is cleared and the
status line names the entry. With unsaved changes nothing is thrown away: you
are asked whether to keep editing, discard your changes, or re-create the entry
from the values still on screen.
```

Keep it to orientation prose; the behaviour table belongs in the design doc, not
the mdBook.

- [ ] **Step 5: Commit**

```bash
git add src/ui/state.rs CHANGES.md docs/src/concepts/live-data.md
git commit -m "refactor(state): drop the dead leaf_search_truncated field; docs

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Verification before declaring done

- [ ] `make check` green.
- [ ] Live suites green with the demo server up:
  ```bash
  scripts/test-ldap.sh start
  export EDAPTOR_TEST_ADMIN_PW=adminpassword
  export EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389
  make check
  ```
- [ ] **Live TUI check via the tmux harness** — this refactor is behavioural and
  unit tests cannot see the highlight the operator sees:
  1. Navigate to a container, open an entry, type a find that excludes it →
     highlight moves, form follows (clean form).
  2. Edit a field (do not save), type a find → **no modal**, form stays put.
  3. `ldapdelete` the open entry out-of-band, press Alt+R → status reports it is
     gone; with unsaved edits the Re-create / Discard / Keep editing dialog
     appears.
  4. Rename a container via the form → the tree rebuilds and the highlight stays
     on the renamed container.

---

## Appendix: independent small fixes

These are **not** part of the design above (its non-goals list them). They are
one-commit TDD fixes each, to be done after Task 8.

- **#4 — entry reads must request `scan_attrs`.** `src/workflows/read_flow.rs:78`
  and `src/ui/state.rs:888` request `vec!["*", "entryCSN"]`. `*` never returns an
  *operational* attribute, so a label or tree template naming one renders from the
  eager scan and then goes blank once the entry is visited — visiting an entry
  degrades the model. Append `scan_attrs`. Test: a read whose projected node keeps
  an operational attr named by `scan_attrs`.
- **#6 — the `lookup` search term after a pick.** `src/ui/lookup.rs:409` submits
  the raw input, which after a pick is `"5000 (staff)"`, so the next keystroke
  queries the server for that whole string and the candidate list empties. Add the
  inverse of `input_after_pick` (strip from the first `" ("`) and submit that.
  Test: `search_term_after_pick("5000 (staff)") == "5000"`.
- **#8 — status wording pass.** Every `st.status = …` became operator-visible for
  the first time in Spec 2. Read the ~15 live sites and fix wording written when
  nobody could read it. No behaviour change; no new tests.

**#5 (two-leg rename where MODRDN succeeds and MODIFY fails) is deliberately not
here** — it needs its own brainstorm, and it overlaps Spec 4's delete gating.
