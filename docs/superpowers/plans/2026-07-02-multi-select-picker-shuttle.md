# Multi-Select Picker → Shuttle Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every multi-select `picker` field (`memberUid`, `member`) use the two-column `Shuttle` transfer widget, leaving only single-select pickers on the old radio-list dialog.

**Architecture:** The Shuttle dialog already exists as `MembershipDialog` (`src/ui/membership.rs`) and already handles DN *and* scalar stores, server-backed search, seeding, and staging `SetValues` — the only fanout-specific behaviour lives in the save path, not the dialog. So we generalize that dialog to serve *all* multi-select pickers (rename it `MultiPickerDialog` in `multi_picker.rs`), route pickers by **cardinality** instead of by fanout, and strip the multi-select code out of the old `PickerDialog` so it becomes single-select-only.

**Tech Stack:** Rust, tvision-rs 0.4 (TUI), the crate's `FieldWidget`/`FieldEditor` plugin architecture.

## Global Constraints

- **Cap build/test parallelism at 4 cores** (shared machine): `cargo test -j4`, `cargo clippy --all-targets -- -D warnings`.
- **`make check`** (fmt + clippy `-D warnings` + tests) must be green before the work is declared done.
- **Do NOT rename `WidgetSpecCfg::Membership`** — that is the config-level `kind = "membership"` (fanout) variant in `src/config/mod.rs` / `resolver.rs` / `config/widget.rs`, a *separate* concept from the UI `MembershipWidget`. Only the UI module and its `MembershipWidget/Editor/Dialog` types are renamed.
- **Comments/identifiers in English** (per project convention).
- **Keep `CHANGES.md` and `docs/src/` in sync** — part of "done".
- The dialog Views (`MultiPickerDialog`, `Shuttle`) are `pub(crate)` and therefore **not reachable from `tests/`** (a separate crate). In-crate unit tests cover dialog behaviour; `tests/` integration coverage sits at the public `SearchFlow` API.

---

## File Structure

- `src/config/relation.rs` — add `PickerBinding::cardinality(field_multi)` helper (single source of the "resolve select vs schema arity" rule).
- `src/ui/membership.rs` → **rename to** `src/ui/multi_picker.rs` — the generalized Shuttle multi-select dialog (`MultiPickerWidget/Editor/Dialog`). Serves fanout *and* non-fanout multi pickers.
- `src/ui/picker.rs` — reduced to single-select only (radio list); drop `Cardinality::Multi` paths.
- `src/ui/widget.rs` — route `Picker` by cardinality; update `widget_for` / `is_modal_field`.
- `src/ui/mod.rs` — module declaration rename.
- `tests/tv_member_uid.rs` — **new** live `SearchFlow`-level integration test for the scalar (`uid`) store.
- `CHANGES.md`, `docs/src/configuration/widgets.md` — changelog + reference doc.

---

## Task 1: `PickerBinding::cardinality` helper

**Files:**
- Modify: `src/config/relation.rs` (add method near `PickerBinding`, ~line 48)
- Modify: `src/ui/picker.rs:90-94` (use the helper in `PickerEditor::into_view`)
- Test: inline `#[cfg(test)]` in `src/config/relation.rs`

**Interfaces:**
- Produces: `impl PickerBinding { pub fn cardinality(&self, field_multi: bool) -> Cardinality }` — returns `self.select` when set, else `Multi` if `field_multi` else `Single`.

- [ ] **Step 1: Write the failing test** — append to (or create) the `#[cfg(test)] mod tests` in `src/config/relation.rs`:

```rust
#[cfg(test)]
mod cardinality_tests {
    use super::*;

    fn binding(select: Option<Cardinality>) -> PickerBinding {
        PickerBinding {
            attr: "member".into(),
            scope: CandidateScope {
                base: "ou=people,dc=example,dc=org".into(),
                object_classes: vec!["inetOrgPerson".into()],
                search_attrs: vec!["cn".into()],
                label_template: None,
            },
            store: StoreKey::Dn,
            select,
            fanout_attr: None,
        }
    }

    #[test]
    fn cardinality_prefers_explicit_select() {
        assert_eq!(binding(Some(Cardinality::Single)).cardinality(true), Cardinality::Single);
        assert_eq!(binding(Some(Cardinality::Multi)).cardinality(false), Cardinality::Multi);
    }

    #[test]
    fn cardinality_falls_back_to_field_arity() {
        assert_eq!(binding(None).cardinality(true), Cardinality::Multi);
        assert_eq!(binding(None).cardinality(false), Cardinality::Single);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -j4 -p edaptor cardinality_ 2>&1 | tail -20`
Expected: FAIL — `no method named cardinality found for struct PickerBinding`.

- [ ] **Step 3: Add the method** — insert after the `PickerBinding` struct (after line 48) in `src/config/relation.rs`:

```rust
impl PickerBinding {
    /// Resolve the effective cardinality: an explicit `select` wins; otherwise
    /// derive it from the field's schema arity (`select = "auto"`). This is the
    /// single source of the rule shared by `widget_for` routing and the editors.
    pub fn cardinality(&self, field_multi: bool) -> Cardinality {
        self.select.unwrap_or(if field_multi {
            Cardinality::Multi
        } else {
            Cardinality::Single
        })
    }
}
```

- [ ] **Step 4: Use it in `PickerEditor::into_view`** — in `src/ui/picker.rs`, replace lines 90-94:

```rust
        let cardinality = binding.select.unwrap_or(if multi {
            Cardinality::Multi
        } else {
            Cardinality::Single
        });
```

with:

```rust
        let cardinality = binding.cardinality(multi);
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -j4 -p edaptor cardinality_ 2>&1 | tail -20` then `cargo test -j4 -p edaptor picker:: 2>&1 | tail -20`
Expected: PASS for both.

- [ ] **Step 6: Commit**

```bash
git add src/config/relation.rs src/ui/picker.rs
git commit -m "refactor(config): add PickerBinding::cardinality helper"
```

---

## Task 2: Rename `membership.rs` → `multi_picker.rs` (behaviour unchanged)

Pure rename — the module still activates only for the fanout case at this point. This isolates the mechanical churn from the behaviour change in Task 3.

**Files:**
- Rename: `src/ui/membership.rs` → `src/ui/multi_picker.rs`
- Modify: `src/ui/mod.rs:11`
- Modify: `src/ui/widget.rs:147`

**Interfaces:**
- Produces: `pub(crate) struct MultiPickerWidget` (was `MembershipWidget`), `MultiPickerEditor` (was `MembershipEditor`), `MultiPickerDialog` (was `MembershipDialog`) in `crate::ui::multi_picker`.

- [ ] **Step 1: Move the file with git**

Run:
```bash
git mv src/ui/membership.rs src/ui/multi_picker.rs
```

- [ ] **Step 2: Rename the types inside the moved file**

Run (in-place, exact identifiers only — `Membership` UI types → `MultiPicker`):
```bash
sed -i \
  -e 's/MembershipWidget/MultiPickerWidget/g' \
  -e 's/MembershipEditor/MultiPickerEditor/g' \
  -e 's/MembershipDialog/MultiPickerDialog/g' \
  src/ui/multi_picker.rs
```

- [ ] **Step 3: Update the module's doc header** — the top-of-file `//!` comment in `src/ui/multi_picker.rs` opens with "Membership (fan-out) picker". Replace the first doc paragraph so it describes the general role. Change the first line:

```rust
//! Membership (fan-out) picker — a two-column "mover" dialog. A
```

to:

```rust
//! Multi-select picker — a two-column "mover" dialog backed by [`Shuttle`].
//! Serves every multi-select `WidgetKind::Picker` binding: a plain multi picker
//! (e.g. `memberUid`, `member`) writes the picked store values onto this entry;
//! a fan-out binding (`fanout_attr.is_some()`, e.g. `memberOf`) instead writes
//! this entry's DN onto each picked candidate at save time (the combined-save
//! path handles that expansion — the dialog itself is identical either way). A
```

- [ ] **Step 4: Update `mod.rs`** — in `src/ui/mod.rs` replace line 11:

```rust
pub(crate) mod membership;
```

with:

```rust
pub(crate) mod multi_picker;
```

Keep the file's alphabetical ordering acceptable (place it where `membership` was, or move it between `mod dialog;` region and others — ordering is not enforced, leave it adjacent to the other picker modules if trivial).

- [ ] **Step 5: Update the dispatch reference** — in `src/ui/widget.rs` replace line 147:

```rust
        Box::new(crate::ui::membership::MembershipWidget)
```

with:

```rust
        Box::new(crate::ui::multi_picker::MultiPickerWidget)
```

- [ ] **Step 6: Verify the rename compiles and all tests pass**

Run: `cargo test -j4 -p edaptor 2>&1 | tail -25`
Expected: PASS (no behaviour change; the renamed module tests run under their new names).

Then confirm no stale references remain:

Run: `grep -rn "ui::membership\|MembershipWidget\|MembershipEditor\|MembershipDialog" src/`
Expected: no output.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "refactor(ui): rename membership.rs to multi_picker.rs (no behaviour change)"
```

---

## Task 3: Route multi-select pickers to the Shuttle

Broaden `MultiPickerWidget` and the dispatch so **every** multi-select picker (fanout *or* non-fanout) opens the Shuttle dialog; single-select non-fanout still opens `PickerWidget`.

**Files:**
- Modify: `src/ui/multi_picker.rs` (`MultiPickerWidget::activate`, and `MultiPickerEditor` carries no cardinality — it is always multi)
- Modify: `src/ui/picker.rs` (`PickerWidget::activate` narrows to single)
- Modify: `src/ui/widget.rs` (`widget_for`, `is_modal_field`)
- Test: routing test in `src/ui/widget.rs`; new scalar-store dialog test in `src/ui/multi_picker.rs`

**Interfaces:**
- Consumes: `PickerBinding::cardinality(field_multi)` (Task 1).
- Produces: `widget_for` returns `MultiPickerWidget` for any `Picker` where `fanout_attr.is_some() || cardinality == Multi`; `PickerWidget` otherwise.

- [ ] **Step 1: Write the failing routing test** — append to the `#[cfg(test)] mod tests` in `src/ui/widget.rs`:

```rust
#[test]
fn multi_nonfanout_picker_routes_to_multi_picker() {
    use crate::config::relation::{CandidateScope, Cardinality, PickerBinding, StoreKey};
    use crate::config::widget::WidgetKind;
    let binding = PickerBinding {
        attr: "memberUid".into(),
        scope: CandidateScope {
            base: "ou=people,dc=example,dc=org".into(),
            object_classes: vec!["inetOrgPerson".into()],
            search_attrs: vec!["uid".into()],
            label_template: None,
        },
        store: StoreKey::Attr("uid".into()),
        select: Some(Cardinality::Multi),
        fanout_attr: None,
    };
    let mut f = field(&[], WidgetSpec::ReadOnlyText);
    f.label = "memberUid".into();
    f.multi = true;
    f.widget_binding = Some(WidgetKind::Picker(binding));
    // A multi, non-fanout picker must be modal AND must activate a Modal editor
    // on the multi-picker widget (not Inline).
    assert!(is_modal_field(&f));
    assert!(matches!(
        crate::ui::multi_picker::MultiPickerWidget.activate(&f),
        crate::ui::widget::Activation::Modal(_)
    ));
    assert!(matches!(
        crate::ui::picker::PickerWidget.activate(&f),
        crate::ui::widget::Activation::Inline
    ));
}

#[test]
fn single_nonfanout_picker_routes_to_picker() {
    use crate::config::relation::{CandidateScope, Cardinality, PickerBinding, StoreKey};
    use crate::config::widget::WidgetKind;
    let binding = PickerBinding {
        attr: "gidNumber".into(),
        scope: CandidateScope {
            base: "ou=groups,dc=example,dc=org".into(),
            object_classes: vec!["posixGroup".into()],
            search_attrs: vec!["cn".into()],
            label_template: None,
        },
        store: StoreKey::Attr("gidNumber".into()),
        select: Some(Cardinality::Single),
        fanout_attr: None,
    };
    let mut f = field(&[], WidgetSpec::ReadOnlyText);
    f.label = "gidNumber".into();
    f.multi = false;
    f.widget_binding = Some(WidgetKind::Picker(binding));
    assert!(matches!(
        crate::ui::picker::PickerWidget.activate(&f),
        crate::ui::widget::Activation::Modal(_)
    ));
    assert!(matches!(
        crate::ui::multi_picker::MultiPickerWidget.activate(&f),
        crate::ui::widget::Activation::Inline
    ));
}
```

(If `Activation` is not re-exported at `crate::ui::widget::Activation`, use its real path `crate::ui::widget::Activation` — it is defined in `widget.rs`, so `Activation` is already in scope inside the test module; drop the `crate::ui::widget::` qualifier if the compiler flags it as redundant.)

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -j4 -p edaptor multi_nonfanout_picker_routes 2>&1 | tail -25`
Expected: FAIL — `MultiPickerWidget.activate` returns `Inline` for a non-fanout binding (current guard requires `fanout_attr.is_some()`).

- [ ] **Step 3: Broaden `MultiPickerWidget::activate`** — in `src/ui/multi_picker.rs`, the `activate` match arm currently reads:

```rust
    fn activate(&self, field: &EditField) -> Activation {
        match &field.widget_binding {
            Some(WidgetKind::Picker(b)) if b.fanout_attr.is_some() => {
                Activation::Modal(Box::new(MultiPickerEditor {
                    label: field.label.clone(),
                    binding: b.clone(),
                    current: field.values.clone(),
                }))
            }
            _ => Activation::Inline,
        }
    }
```

Change the guard to also accept a non-fanout multi binding:

```rust
    fn activate(&self, field: &EditField) -> Activation {
        use crate::config::relation::Cardinality;
        match &field.widget_binding {
            Some(WidgetKind::Picker(b))
                if b.fanout_attr.is_some()
                    || b.cardinality(field.multi) == Cardinality::Multi =>
            {
                Activation::Modal(Box::new(MultiPickerEditor {
                    label: field.label.clone(),
                    binding: b.clone(),
                    current: field.values.clone(),
                }))
            }
            _ => Activation::Inline,
        }
    }
```

- [ ] **Step 4: Narrow `PickerWidget::activate` to single** — in `src/ui/picker.rs`, the `activate` arm currently reads:

```rust
    fn activate(&self, field: &EditField) -> Activation {
        match &field.widget_binding {
            Some(WidgetKind::Picker(b)) if b.fanout_attr.is_none() => {
                Activation::Modal(Box::new(PickerEditor {
                    label: field.label.clone(),
                    binding: b.clone(),
                    current: field.values.clone(),
                    multi: field.multi,
                }))
            }
            _ => Activation::Inline,
        }
    }
```

Change the guard to single-select non-fanout only:

```rust
    fn activate(&self, field: &EditField) -> Activation {
        use crate::config::relation::Cardinality;
        match &field.widget_binding {
            Some(WidgetKind::Picker(b))
                if b.fanout_attr.is_none()
                    && b.cardinality(field.multi) == Cardinality::Single =>
            {
                Activation::Modal(Box::new(PickerEditor {
                    label: field.label.clone(),
                    binding: b.clone(),
                    current: field.values.clone(),
                    multi: field.multi,
                }))
            }
            _ => Activation::Inline,
        }
    }
```

- [ ] **Step 5: Update `widget_for` dispatch** — in `src/ui/widget.rs`, replace the two Picker arms (lines 138-147):

```rust
    } else if matches!(
        &field.widget_binding,
        Some(WidgetKind::Picker(b)) if b.fanout_attr.is_none()
    ) {
        Box::new(crate::ui::picker::PickerWidget)
    } else if matches!(
        &field.widget_binding,
        Some(WidgetKind::Picker(b)) if b.fanout_attr.is_some()
    ) {
        Box::new(crate::ui::multi_picker::MultiPickerWidget)
```

with a single cardinality-based split:

```rust
    } else if let Some(WidgetKind::Picker(b)) = &field.widget_binding {
        use crate::config::relation::Cardinality;
        if b.fanout_attr.is_some() || b.cardinality(field.multi) == Cardinality::Multi {
            Box::new(crate::ui::multi_picker::MultiPickerWidget)
        } else {
            Box::new(crate::ui::picker::PickerWidget)
        }
```

- [ ] **Step 6: Simplify `is_modal_field`** — in `src/ui/widget.rs`, replace the two Picker `matches!` arms (lines 167-174):

```rust
        || matches!(
            &field.widget_binding,
            Some(WidgetKind::Picker(b)) if b.fanout_attr.is_none()
        )
        || matches!(
            &field.widget_binding,
            Some(WidgetKind::Picker(b)) if b.fanout_attr.is_some()
        )
```

with a single arm (every picker is modal, regardless of cardinality/fanout):

```rust
        || matches!(field.widget_binding, Some(WidgetKind::Picker(_)))
```

- [ ] **Step 7: Run the routing tests to verify they pass**

Run: `cargo test -j4 -p edaptor _picker_routes 2>&1 | tail -25`
Expected: PASS for both `multi_nonfanout_picker_routes_to_multi_picker` and `single_nonfanout_picker_routes_to_picker`.

- [ ] **Step 8: Add a headless dialog test for the scalar (`uid`) store** — append to the `#[cfg(test)] mod tests` in `src/ui/multi_picker.rs`. This proves a non-fanout memberUid binding seeds, fills Available from a scalar-store search, and stages `SetValues([uid])` on a move. Model it on the existing membership dialog tests in the same file (reuse their `Harness`/helpers if present; otherwise mirror the `oc_picker`/`picker` headless-context pattern):

```rust
#[test]
fn nonfanout_scalar_picker_seeds_moves_and_stages_uid() {
    use crate::config::relation::{CandidateScope, Cardinality, PickerBinding, StoreKey};
    use crate::workflows::pick_state::Candidate;

    let shared = test_shared(); // same helper the other tests in this module use
    // A delivered candidate whose scalar store_value is a uid (not a DN).
    shared.borrow_mut().search_results = vec![Candidate {
        dn: "uid=ann,ou=people,dc=example,dc=org".into(),
        label: "Ann Smith".into(),
        store_value: "ann".into(),
    }];

    let binding = PickerBinding {
        attr: "memberUid".into(),
        scope: CandidateScope {
            base: "ou=people,dc=example,dc=org".into(),
            object_classes: vec!["inetOrgPerson".into()],
            search_attrs: vec!["uid".into()],
            label_template: None,
        },
        store: StoreKey::Attr("uid".into()),
        select: Some(Cardinality::Multi),
        fanout_attr: None,
    };
    let ed: Box<dyn FieldEditor> = Box::new(MultiPickerEditor {
        label: "memberUid".into(),
        binding,
        current: vec![],
    });
    let (mut view, _focus) = ed.into_view(&schema(), shared.clone());

    let mut out = std::collections::VecDeque::new();
    let mut timers = TimerQueue::new();
    let mut deferred = Vec::new();
    let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
    view.reset_current(&mut ctx);

    // The scalar candidate is offered in Available (keyed by its uid).
    let dlg = view
        .as_any_mut()
        .and_then(|a| a.downcast_mut::<MultiPickerDialog>())
        .expect("downcast MultiPickerDialog");
    let sh = dlg.shuttle_mut().expect("shuttle present");
    let aid = sh.avail_id_for_test();
    sh.highlight(aid, 0, &mut ctx);

    // Move it in and assert the staged commit is the uid scalar.
    let mut ev = Event::KeyDown(tv::KeyEvent::from(tv::Key::Insert));
    dlg.handle_event(&mut ev, &mut ctx);

    assert_eq!(
        shared.borrow().staged_commit,
        Some(CommitOutcome::SetValues(vec!["ann".to_string()])),
        "moving the scalar candidate in must stage its uid, not its DN"
    );
}
```

Notes for the implementer: match the exact test scaffolding already used by the other tests at the bottom of `multi_picker.rs` — the helper names (`test_shared`, `schema`, `headless_ctx`) and the way they downcast to the dialog and reach the embedded `Shuttle`. `Shuttle::avail_id_for_test` and `Shuttle::highlight` are `#[cfg(test)]` helpers on `Shuttle` (see `src/ui/shuttle.rs`). If `shuttle_mut` is private, drive the move by dispatching the key straight to `dlg.handle_event` after highlighting via whatever test seam the existing membership tests already use (they highlight the Available row by label).

- [ ] **Step 9: Run the new dialog test**

Run: `cargo test -j4 -p edaptor nonfanout_scalar_picker 2>&1 | tail -25`
Expected: PASS. If the exact helper wiring differs, adapt to the sibling tests in the same file (they are the source of truth for the scaffolding).

- [ ] **Step 10: Full test + clippy**

Run: `cargo test -j4 -p edaptor 2>&1 | tail -15 && cargo clippy --all-targets -- -D warnings 2>&1 | tail -15`
Expected: all green.

- [ ] **Step 11: Commit**

```bash
git add src/ui/widget.rs src/ui/picker.rs src/ui/multi_picker.rs
git commit -m "feat(ui): route multi-select pickers to the Shuttle dialog"
```

---

## Task 4: Reduce `PickerDialog` to single-select

Multi-select pickers no longer reach `PickerWidget`, so remove the now-dead `Cardinality::Multi` code from `picker.rs` and its multi-only tests. The radio single-select path stays.

**Files:**
- Modify: `src/ui/picker.rs` (`PickerDialog` struct/methods and tests)

**Interfaces:**
- Produces: `PickerDialog` no longer stores or branches on `Cardinality`; `marker` is radio-only; `pick_at` always replaces the selection.

- [ ] **Step 1: Delete the multi-select tests** — in `src/ui/picker.rs` remove the tests that assert multi behaviour: `multi_toggle_stages_selected_store_value` and `space_does_not_toggle_candidate`. Also delete the `multi_dn_binding()` helper (now unused) — but FIRST do Step 2, which reuses one caller.

- [ ] **Step 2: Repoint the activation tests to single bindings** — `present_joins_values_or_none`, `non_fanout_picker_activates_modal`, and `fanout_picker_does_not_activate_here` build `multi_dn_binding()`. `present` is arity-agnostic (keep), but `non_fanout_picker_activates_modal` must now use a **single** binding (a multi non-fanout picker yields `Inline` on `PickerWidget` after Task 3). Add a single-select helper near the other test helpers:

```rust
fn single_dn_binding() -> PickerBinding {
    PickerBinding {
        attr: "secretary".into(),
        scope: dn_scope(),
        store: StoreKey::Dn,
        select: Some(Cardinality::Single),
        fanout_attr: None,
    }
}
```

Then in `non_fanout_picker_activates_modal` replace the field construction to use `single_dn_binding()` and `multi = false`:

```rust
    #[test]
    fn non_fanout_picker_activates_modal() {
        let f = picker_field("secretary", &[], single_dn_binding(), false);
        assert!(matches!(PickerWidget.activate(&f), Activation::Modal(_)));
    }
```

For `present_joins_values_or_none`, `present` does not depend on cardinality — change its binding to `single_dn_binding()` too (and `multi = false`) so `multi_dn_binding` can be deleted, but keep the value assertions unchanged (they exercise `present`, not activation).

`fanout_picker_does_not_activate_here` sets `b.fanout_attr = Some(...)`; keep it but build from `single_dn_binding()` (a fanout binding still yields `Inline` on `PickerWidget`).

- [ ] **Step 3: Run to verify the trimmed test set fails to compile / fails as expected**

Run: `cargo test -j4 -p edaptor picker:: 2>&1 | tail -25`
Expected: FAIL — `PickerDialog::new` still takes a `cardinality` argument and `marker`/`pick_at` still reference `Cardinality::Multi`; the file may still compile if you have not yet removed the field. This step just confirms the multi tests are gone; proceed to strip the code.

- [ ] **Step 4: Remove the `cardinality` field and constructor param** — in `src/ui/picker.rs`:

Delete the field from the struct (`picker.rs:116`):

```rust
    cardinality: Cardinality,
```

Change `PickerDialog::new`'s signature — drop the `cardinality: Cardinality` parameter (line 132) and its struct-init line (`cardinality,` near line 208). Update the sole caller in `PickerEditor::into_view` (Task 1 left `let cardinality = binding.cardinality(multi);`) — the single-select dialog no longer needs it, so remove that `let` binding and the argument, but keep a debug assertion that we are single:

```rust
        // Multi-select pickers are routed to MultiPickerDialog; this dialog is
        // single-select only.
        debug_assert_eq!(binding.cardinality(multi), Cardinality::Single);
        let dlg = PickerDialog::new(label, binding, current, shared);
```

- [ ] **Step 5: Make `marker` radio-only** — replace `PickerDialog::marker` (lines 217-227):

```rust
    fn marker(&self, selected: bool, saved: bool) -> &'static str {
        match (self.cardinality, selected, saved) {
            (Cardinality::Single, true, _) => "(\u{2022}) ",
            (Cardinality::Single, false, _) => "( ) ",
            (Cardinality::Multi, true, _) => "[x] ",
            (Cardinality::Multi, false, true) => "[-] ",
            (Cardinality::Multi, false, false) => "[ ] ",
        }
    }
```

with:

```rust
    /// Radio marker for the highlighted single-select candidate.
    fn marker(&self, selected: bool, _saved: bool) -> &'static str {
        if selected {
            "(\u{2022}) "
        } else {
            "( ) "
        }
    }
```

- [ ] **Step 6: Make `pick_at` always replace** — replace the `match self.cardinality { ... }` block in `pick_at` (lines 310-320) with the single-select behaviour only:

```rust
        // Single-select radio: the pick replaces the whole selection.
        self.pick.selected = vec![cand];
```

(Delete the `Cardinality::Multi` arm and its `self.pick.cursor = idx; self.pick.toggle_cursor();`.)

- [ ] **Step 7: Drop the now-unused `Cardinality` import if the compiler flags it** — after the edits, `Cardinality` may still be referenced (in the `debug_assert_eq!`). Keep the import if so; remove it only if `cargo build` reports it unused.

- [ ] **Step 8: Update the module doc + present** — `PickerWidget::present` joins values (fine for single). Update the top-of-file `//!` doc first paragraph so it says single-select (it currently says "single / multi select"). Change:

```rust
//! Picker field widget (single / multi select over live LDAP results). A
```

to:

```rust
//! Picker field widget (single-select over live LDAP results). A
//! `WidgetKind::Picker` binding that resolves to single cardinality and is not a
//! fan-out opens a modal with a search `InputLine` over a radio `ListBox`.
//! Multi-select pickers are served by `ui::multi_picker` (the Shuttle dialog).
```

Remove the now-inaccurate sentence about "checkbox for multi" further down the doc comment if present.

- [ ] **Step 9: Run the picker tests + full suite + clippy**

Run: `cargo test -j4 -p edaptor picker:: 2>&1 | tail -20`
Expected: PASS (single-select tests: `single_pick_replaces_selection`, `reset_seeds_current_selection`, `non_fanout_picker_activates_modal`, `fanout_picker_does_not_activate_here`, `present_joins_values_or_none`).

Run: `cargo test -j4 -p edaptor 2>&1 | tail -15 && cargo clippy --all-targets -- -D warnings 2>&1 | tail -15`
Expected: all green, no dead-code or unused-import warnings.

- [ ] **Step 10: Commit**

```bash
git add src/ui/picker.rs
git commit -m "refactor(ui): reduce PickerDialog to single-select only"
```

---

## Task 5: Live integration test for the scalar (`uid`) store

`memberUid`'s picker stores the candidate's `uid` scalar (not its DN). `tv_picker.rs` covers gidNumber (scalar over posixGroup) and users (DN store) but **not** a user + `uid` scalar store — the exact shape `memberUid` uses to feed the Shuttle's Available column. Add a gated `SearchFlow`-level test (public API; the dialog itself is `pub(crate)` and covered by the Task 3 unit test).

**Files:**
- Create: `tests/tv_member_uid.rs`

**Interfaces:**
- Consumes: `edaptor::workflows::search_flow::{SearchFlow, SearchOutcome}`, `edaptor::ldap::worker::WorkerHandle` (public), plus the config builders mirrored from `tv_picker.rs`.

- [ ] **Step 1: Create the gated test file** — write `tests/tv_member_uid.rs`:

```rust
//! Live memberUid picker integration test (scalar `uid` store).
//!
//! Enable by setting EDAPTOR_TEST_LDAP_URI (e.g. ldap://localhost:1389).
//! Start a server with: scripts/test-ldap.sh start
//! When the env var is unset the test prints SKIP and passes (no silent skip).
//!
//! Read-only: it drives a real candidate search via `SearchFlow` for the
//! `memberUid` picker shape (candidate = user under ou=people, store = uid) and
//! asserts the candidates' `store_value` is the user's `uid` scalar (NOT a DN) —
//! exactly the keys the multi-select Shuttle stages into `memberUid`.

use std::time::{Duration, Instant};

use edaptor::config::{AuthConfig, AuthMethod, Config, PasswordSource, ServerConfig, TlsConfig};
use edaptor::ldap::worker::{Response, WorkerHandle};
use edaptor::workflows::search_flow::{SearchFlow, SearchOutcome};

fn test_config(uri: String) -> (Config, String) {
    let config = Config {
        server: ServerConfig {
            uri,
            base_dn: "dc=example,dc=org".to_string(),
            start_tls: false,
            read_only: false,
            timeout_secs: 10,
            tls: TlsConfig::default(),
        },
        auth: AuthConfig {
            method: AuthMethod::Simple,
            bind_dn: Some("cn=admin,dc=example,dc=org".to_string()),
            password_source: PasswordSource::Env("EDAPTOR_TEST_ADMIN_PW".to_string()),
        },
        profiles: Vec::new(),
        meta: Default::default(),
        samba: Default::default(),
        tree: Default::default(),
    };
    let password =
        std::env::var("EDAPTOR_TEST_ADMIN_PW").unwrap_or_else(|_| "adminpassword".to_string());
    (config, password)
}

#[allow(clippy::too_many_arguments)]
fn run_search(
    worker: &WorkerHandle,
    flow: &mut SearchFlow,
    base: &str,
    oc: &str,
    term: &str,
    attrs: &[String],
    store_attr: Option<&str>,
    timeout: Duration,
) -> SearchOutcome {
    let want_id = flow
        .request(worker, base, oc, term, attrs, store_attr)
        .expect("submit search request");
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match worker.poll() {
            Some(resp) => {
                let matches_id = matches!(
                    &resp,
                    Response::Entries { id, .. } | Response::SearchError { id, .. } if *id == want_id
                );
                if matches_id {
                    return flow.on_response(&resp);
                }
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    panic!("search for term {term:?} timed out");
}

#[test]
fn member_uid_picker_stores_uid_scalar() {
    let Ok(uri) = std::env::var("EDAPTOR_TEST_LDAP_URI") else {
        eprintln!("SKIP member_uid_picker_stores_uid_scalar: set EDAPTOR_TEST_LDAP_URI to run");
        return;
    };

    let (config, password) = test_config(uri);
    let worker = WorkerHandle::spawn(config, password).expect("spawn worker");

    // memberUid picker shape: candidate = user (ou=people, inetOrgPerson),
    // store = uid. The requested attrs must include `uid` (the store attr).
    let mut flow = SearchFlow::new();
    let attrs = vec!["cn".to_string(), "uid".to_string()];
    let outcome = run_search(
        &worker,
        &mut flow,
        "ou=people,dc=example,dc=org",
        "inetOrgPerson",
        "", // empty term → objectClass-only, returns up to the cap
        &attrs,
        Some("uid"), // scalar store
        Duration::from_secs(10),
    );
    let rows = match outcome {
        SearchOutcome::Results { rows, .. } => rows,
        other => panic!("expected uid-store Results, got {other:?}"),
    };
    assert!(!rows.is_empty(), "the demo server must return user candidates");

    let first = &rows[0];
    // store_value must be the uid scalar, NOT the DN.
    assert_ne!(
        first.store_value, first.dn,
        "uid store: store_value must be the uid scalar, not the DN"
    );
    assert!(
        !first.store_value.contains('=') && !first.store_value.contains(','),
        "uid store_value must look like a bare uid, got {:?}",
        first.store_value
    );
    assert!(
        first.dn.contains("ou=people"),
        "candidate dn should be a person DN, got {:?}",
        first.dn
    );
}
```

- [ ] **Step 2: Run without the server (SKIP path)**

Run: `cargo test -j4 -p edaptor --test tv_member_uid 2>&1 | tail -15`
Expected: PASS with `SKIP member_uid_picker_stores_uid_scalar` printed (env var unset).

- [ ] **Step 3: Run against the live demo server (best-effort)**

Run:
```bash
scripts/test-ldap.sh start
EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389 EDAPTOR_TEST_ADMIN_PW=adminpassword \
  cargo test -j4 -p edaptor --test tv_member_uid -- --nocapture 2>&1 | tail -20
```
Expected: PASS with real assertions (store_value is a bare uid). If the local server is unavailable, the SKIP path (Step 2) is the gate; note it and move on.

- [ ] **Step 4: Commit**

```bash
git add tests/tv_member_uid.rs
git commit -m "test: live SearchFlow coverage for the memberUid uid-scalar store"
```

---

## Task 6: Changelog + reference docs

**Files:**
- Modify: `CHANGES.md` (unreleased section)
- Modify: `docs/src/configuration/widgets.md` (the `picker` kind section)

- [ ] **Step 1: Add the CHANGES.md entry** — under the current unreleased section, add a bullet (match the file's existing style/heading):

```markdown
- Multi-select `picker` fields (`memberUid`, `member`) now use the two-column
  Shuttle editor (Available | Members) with type-to-find, matching the
  objectClass and membership editors. The single-list checkbox picker is now
  single-select only. Fixed-vocabulary `choice` fields are unchanged.
```

- [ ] **Step 2: Update the picker reference doc** — in `docs/src/configuration/widgets.md`, in the `## The picker kind` section (after the intro paragraph around line 169), add a sentence describing the two UIs. Insert after the paragraph that ends "…writes the right value(s) into this entry's attribute.":

```markdown

A **single-select** picker (`select = "single"`) presents a search box over a
radio list. A **multi-select** picker (`select = "multi"`, the default for
multi-valued attributes such as `member` and `memberUid`) presents the
two-column **Available | Selected** shuttle: type to search, then
Insert/Enter/[Add] to move a candidate in and Delete/[Remove] to move it out.
```

- [ ] **Step 3: Verify the mdBook builds**

Run: `make docs 2>&1 | tail -15`
Expected: builds without error (or `cd docs && mdbook build`).

- [ ] **Step 4: Commit**

```bash
git add CHANGES.md docs/src/configuration/widgets.md
git commit -m "docs: multi-select pickers use the Shuttle editor"
```

---

## Task 7: Final verification

- [ ] **Step 1: Run the full gate**

Run: `make check 2>&1 | tail -25`
Expected: fmt clean, clippy `-D warnings` clean, all tests pass.

- [ ] **Step 2: Manual smoke (optional, if a demo server is handy)**

Run:
```bash
scripts/test-ldap.sh start
export EDAPTOR_TEST_ADMIN_PW=adminpassword
cargo run -- --config examples/demo-config.toml
```
Navigate to a `posixGroup` entry, open `memberUid`, and confirm the two-column Shuttle appears (Available users on the left, members on the right), type-to-find works, and moves stage correctly. Open `gidNumber` on a user and confirm it still shows the single-select radio list.

- [ ] **Step 3: Confirm no stale references**

Run: `grep -rn "MembershipWidget\|MembershipEditor\|MembershipDialog\|ui::membership" src/ tests/`
Expected: no output (the config-level `WidgetSpecCfg::Membership` is untouched and will not match these patterns).

---

## Self-Review notes

- **Spec coverage:** module split (Tasks 2-4), cardinality routing (Tasks 1, 3), single-select reduction (Task 4), non-fanout scalar dialog test (Task 3 Step 8), routing tests (Task 3), live integration (Task 5), CHANGES + widgets doc (Task 6), `choice` untouched (called out in Task 6 copy + Global Constraints). The spec's "drive the real Shuttle live end-to-end" is adjusted to a `SearchFlow`-level integration test + in-crate dialog unit test because the dialog is `pub(crate)` (documented in Global Constraints and the spec update).
- **`[-]` marker removal:** covered by Task 4 Step 5 (marker becomes radio-only) — matches the approved design.
- **No config-kind rename:** enforced in Global Constraints and Task 7 Step 3.
