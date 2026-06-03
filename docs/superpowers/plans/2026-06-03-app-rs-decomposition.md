# app.rs Decomposition Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split the 4703-line `src/ui/app.rs` god-file into a focused `src/ui/app/` submodule directory, then relocate domain-pure logic out of the UI layer — without changing any behaviour.

**Architecture:** Immediate-mode ratatui event loop. State lives in `App`; worker/read_flow/structure/write-maps are loop locals (a deliberate "borrow split"). Phases 1–2 keep all signatures and the loop intact; only file boundaries move. Phase 3 (optional) bundles the co-mutated params into a `Ctx` receiver.

**Tech Stack:** Rust, ratatui, crossterm, tui-tree-widget, anyhow.

---

## The Oracle (verification after EVERY task)

This is a behaviour-preserving refactor. There is no new test to write; the existing suite is the oracle. After every task:

```bash
cargo build 2>&1 | tail -5      # must compile
cargo test  2>&1 | grep -E 'test result:' | awk '{s+=$4} END {print s}'   # must print 334
cargo clippy --all-targets 2>&1 | grep -E '^(warning|error)' | head       # must print nothing
```

**Baseline (recorded 2026-06-03): 334 tests pass, clippy clean.** If any task ends with a count ≠ 334 or a clippy warning, STOP and fix or revert that task before continuing. Tests are *moved*, never added or deleted in Phases 1–3, so the count is invariant.

**Move discipline:** functions and tests are moved *verbatim* (cut from source, paste in destination). Do not rewrite bodies. The only edits permitted are: adding `use` lines, adjusting visibility (`pub(crate)`/`pub(super)`), and the `mod`/`pub use` wiring. If a move forces a logic change, that is a signal the boundary is wrong — reconsider, don't patch.

---

## Phase 1 — Pure module split (zero signature changes)

### Task 1: Convert `app.rs` into a directory module

**Files:**
- Move: `src/ui/app.rs` → `src/ui/app/mod.rs`

- [ ] **Step 1: git-move the file**

```bash
mkdir -p src/ui/app
git mv src/ui/app.rs src/ui/app/mod.rs
```

- [ ] **Step 2: Verify nothing else changed**

Rust treats `app/mod.rs` identically to `app.rs`; `src/ui/mod.rs`'s `pub mod app;` is unchanged. Run the oracle.

Run: `cargo build && cargo test 2>&1 | grep -E 'test result:' | awk '{s+=$4} END {print s}'`
Expected: `334`

- [ ] **Step 3: Commit**

```bash
git add -A && git commit -m "refactor(ui): app.rs -> app/mod.rs (directory module, no code change)"
```

---

### Task 2: Extract `app/overlay.rs` (state enums)

The four UI-state enums are the lowest-coupling cluster: `Overlay`, `GuardIntent`, `PendingAction`, `PostWrite` (mod.rs lines ~54–185 in the pre-split file).

**Files:**
- Create: `src/ui/app/overlay.rs`
- Modify: `src/ui/app/mod.rs`

- [ ] **Step 1: Create `app/overlay.rs`**

Cut the four enum definitions (`Overlay`, `GuardIntent`, `PendingAction`, `PostWrite`) and their doc comments out of `mod.rs` into a new `overlay.rs`. Add at the top the `use` lines they need (determined by the compiler errors): `std::collections::BTreeMap`, `crate::form::changeset::ModOp`, `crate::form::validate::SavePlan`, `crate::ui::edit_form::ValueEditor`, `crate::workflows::structure::StructureInput`, and `super::Pane`.

Make each enum `pub(crate)` if it was `pub` (it must stay reachable from `mod.rs` and `view.rs`). Re-check: `Overlay` and `Pane` are read by `view.rs` — keep their visibility identical to before.

- [ ] **Step 2: Wire the module in `mod.rs`**

At the top of `mod.rs` add `mod overlay;` and `pub(crate) use overlay::{GuardIntent, Overlay, PendingAction};` (plus `PostWrite` as `use overlay::PostWrite;` if it was private). Match the original visibility exactly — if `view.rs` did `use crate::ui::app::Overlay;`, that path must still resolve.

- [ ] **Step 3: Run the oracle**

Run: `cargo build && cargo test 2>&1 | grep -E 'test result:' | awk '{s+=$4} END {print s}' && cargo clippy --all-targets 2>&1 | grep -E '^(warning|error)'`
Expected: `334`, no clippy output.

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "refactor(ui): extract app/overlay.rs (state enums)"
```

---

### Task 3: Extract `app/test_support.rs` (shared test fixtures)

Before distributing tests, move the **shared** `#[cfg(test)]` fixtures so per-module test blocks can reuse them. Shared fixtures (used by tests in more than one future module): `bare_app`, `with_form`, `alt`, `key`, `empty_structure`, `structure`, `bare_profile`, `rule`, `attr_map`, `user_schema`, `create_user_profile`, `pw_spec`, and any `*_binding` builders referenced across clusters.

**Files:**
- Create: `src/ui/app/test_support.rs`
- Modify: `src/ui/app/mod.rs`

- [ ] **Step 1: Create the support module**

```rust
//! Shared `#[cfg(test)]` fixtures for the `app` submodule tests.
#![cfg(test)]
// (paste the shared fixture fns here, verbatim, with their `use` lines)
```

Add `#[cfg(test)] mod test_support;` to `mod.rs`. Each per-module test block will do `use super::super::test_support::*;` (or `use crate::ui::app::test_support::*;`). Make the fixtures `pub(crate)`.

- [ ] **Step 2: Point the existing `mod tests` at the support module**

In the still-monolithic `mod tests` block in `mod.rs`, replace the moved fixture definitions with `use super::test_support::*;`. The remaining test fns are unchanged.

- [ ] **Step 3: Run the oracle**

Expected: `334`, clippy clean. (Fixtures only moved; tests still reference them.)

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "refactor(ui): hoist shared app test fixtures to app/test_support.rs"
```

---

### Task 4: Extract `app/structure_view.rs` (UI-typed structure/label helpers)

Pure-but-UI-typed helpers that build pane content. Functions: `label_rules`, `label_rule_attrs`, `render_node_label`, `compute_rows`, `structure_input_from_attrs`, `structure_inputs`, `build_tree_items`, the `LabelRule` struct, and `membership_candidate_label`/`dedupe_ci` if only used here (else leave in mod.rs).

**Files:**
- Create: `src/ui/app/structure_view.rs`
- Modify: `src/ui/app/mod.rs`
- Move tests: `compute_rows_*`, `tree_items_contain_only_branches`, `label_rules_*`, `label_rule_attrs_*`, `render_node_label_*`, `dedupe_ci_drops_empties*`, `membership_candidate_label_*`

- [ ] **Step 1: Move the functions + `LabelRule` to `structure_view.rs`**

Cut listed functions verbatim. Add `use` lines (compiler-driven): `BTreeMap`, `TreeItem`, `crate::workflows::structure::{Structure, StructureInput, StructureNodeRaw}`, `crate::config::EntryProfile`, label-template helpers from `crate::config::label`. Make functions called from `mod.rs`/`action.rs` `pub(crate)`.

- [ ] **Step 2: Move the matching tests**

Cut the test fns listed above into a `#[cfg(test)] mod tests { use super::*; use crate::ui::app::test_support::*; ... }` block at the bottom of `structure_view.rs`.

- [ ] **Step 3: Wire `mod structure_view;` + `pub(crate) use` in `mod.rs`**

- [ ] **Step 4: Run the oracle** — Expected: `334`, clippy clean.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "refactor(ui): extract app/structure_view.rs (label/row/tree builders)"
```

---

### Task 5: Extract `app/input.rs` (key dispatch + editors)

Functions: `dispatch_key`, `edit_focused_field`, `open_value_editor`, `picker_editor_key`, `value_editor_key`, `service_picker_search`, `overlay_key`, `choose_profile_key`, `guard_key`, and pane helpers `next_pane`/`prev_pane`/`next_index` (move with input; they are navigation).

**Files:**
- Create: `src/ui/app/input.rs`
- Modify: `src/ui/app/mod.rs`
- Move tests: `focus_cycles_*`, `value_editor_*`, `alt_n_opens_*`, `choose_profile_key_*`, `tab_off_a_dirty_form_*`, `quit_while_dirty_*`, `guard_key_maps_*`, `next_index_clamps_*`, `picker_enter_*`, `open_value_editor_*`, `lookup_*`, `select_*`, `single_select_*`, `esc_cancels_a_create_form_*`, `alt_d_deletes_*`, `alt_r_refreshes_*`

- [ ] **Step 1: Move functions verbatim into `input.rs`** with compiler-driven `use` lines (`crossterm::event::*`, `tui_prompts`, `super::{App, Pane}`, `super::overlay::*`, `crate::app::UiAction`, picker/edit_form imports). Functions invoked by `dispatch_key`/`overlay_key` from `mod.rs` become `pub(crate)`.

- [ ] **Step 2: Move the matching test fns** into a `#[cfg(test)] mod tests` block in `input.rs` (`use super::*; use crate::ui::app::test_support::*;`).

- [ ] **Step 3: Wire `mod input;` + `pub(crate) use input::{dispatch_key, overlay_key, ...};`** in `mod.rs`.

- [ ] **Step 4: Run the oracle** — Expected: `334`, clippy clean.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "refactor(ui): extract app/input.rs (key dispatch + value/picker editors)"
```

---

### Task 6: Extract `app/action.rs` (action handling + nav/guard glue)

Functions: `handle_action`, `refresh_structure`, `should_install_form`, `revert_form`, `rebind_selection`, `execute_pending`, `perform_guard_intent`, `navigate_to`, `reconcile`, `guard_if_dirty`, `object_classes_of`, `build_loaded_form`.

**Files:**
- Create: `src/ui/app/action.rs`
- Modify: `src/ui/app/mod.rs`
- Move tests: `should_install_blocks_*`, `should_install_allows_*`, `revert_discards_*`

- [ ] **Step 1: Move functions verbatim** into `action.rs` with `use` lines for `super::*`, `super::overlay::*`, `super::save::*`, `super::create::*`, worker/read_flow/structure types. Mark `pub(crate)` what `mod.rs` (event loop) and `input.rs` call: `handle_action`, `execute_pending`, `reconcile`, `guard_if_dirty`.

- [ ] **Step 2: Move matching tests** into `#[cfg(test)] mod tests` in `action.rs`.

- [ ] **Step 3: Wire `mod action;` + `pub(crate) use`** in `mod.rs`.

- [ ] **Step 4: Run the oracle** — Expected: `334`, clippy clean.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "refactor(ui): extract app/action.rs (UiAction handling + nav/guard)"
```

---

### Task 7: Extract `app/save.rs` (save + combined-save orchestration)

Functions (orchestration that touches worker/App): `submit_prepared`, `reload_form_sync`, `apply_combined_save`, `combined_save_overlay`, `read_group_members`, `apply_one_modify`, `allocate_number`, `mask_changeset_secrets`, `prepare_save`, `prepare_edit_save`, `plan_combined_save`, `membership_fanout`, `would_empty`, `decide_allocation`, `compose_renamed_dn`, `parent_dn`, `format_validation_errors`, the `PrepareSave`/`CombinedPlan` enums.

> Note: many of these are pure and will leave `ui/` entirely in Phase 2. For Phase 1 they land in `save.rs` so the move stays mechanical; Phase 2 relocates the pure subset.

**Files:**
- Create: `src/ui/app/save.rs`
- Modify: `src/ui/app/mod.rs`
- Move tests: `compose_renamed_dn_*`, `validation_errors_format_*`, `fanout_*`, `would_empty_*`, `plan_combined_save_*`, `rename_plus_membership_*`, `combined_save_*`, `prepare_save_*`, `mask_password_attrs_*` (if save-side), `allocation_refuses_*`

- [ ] **Step 1: Move functions + enums verbatim** into `save.rs`, compiler-driven `use` lines.
- [ ] **Step 2: Move matching tests** into `#[cfg(test)] mod tests`.
- [ ] **Step 3: Wire `mod save;` + `pub(crate) use`** in `mod.rs`.
- [ ] **Step 4: Run the oracle** — Expected: `334`, clippy clean.
- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "refactor(ui): extract app/save.rs (save + combined-save flow)"
```

---

### Task 8: Extract `app/create.rs` (create flow + password staging)

Functions: `plan_create`, `stage_password`, `mask_password_attrs`, `now_unix_secs_or_zero`, `profile_for_entry_where`, `profile_for_entry`, `password_replace_mods`, `stage_edit_password`, `apply_static_defaults`, `prepare_create`, `build_new_entry_form`, `open_create_form`, the `CreatePrep` enum.

**Files:**
- Create: `src/ui/app/create.rs`
- Modify: `src/ui/app/mod.rs`
- Move tests: `build_new_entry_form_*`, `plan_create_*`, `stage_password_*`, `stage_edit_password_*`, `apply_static_defaults_*`, `profile_for_entry_*`, `mask_password_attrs_*` (if create-side)

- [ ] **Step 1: Move functions + enum verbatim** into `create.rs`, compiler-driven `use` lines.
- [ ] **Step 2: Move matching tests** into `#[cfg(test)] mod tests`.
- [ ] **Step 3: Wire `mod create;` + `pub(crate) use`** in `mod.rs`.
- [ ] **Step 4: Run the oracle** — Expected: `334`, clippy clean.
- [ ] **Step 5: Verify `mod.rs` is now lean**

Run: `wc -l src/ui/app/*.rs`
Expected: `mod.rs` < ~600 lines (App/Pane/run/event_loop/handle_worker_response + wiring); no file > ~900 lines.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "refactor(ui): extract app/create.rs (create flow + password staging)"
```

---

### Task 9: Phase 1 review checkpoint

- [ ] Run `cargo build && cargo test && cargo clippy --all-targets` — full output, confirm 334 + clean.
- [ ] Run `wc -l src/ui/app/*.rs` — confirm decomposition target met.
- [ ] Skim each new file's top: are `use` lists minimal (no dead imports — clippy catches these)?
- [ ] Confirm `git log --oneline` shows one commit per task (clean history).

---

## Phase 2 — Honour the boundary rule (relocate domain-pure logic out of `ui/`)

The `ui/` module's documented rule: only ratatui/crossterm-touching code belongs here. Move pure domain functions to their proper home. Each relocation is its own task + commit + oracle run.

### Task 10: Create `workflows/save.rs` and move pure save logic

**Files:**
- Create: `src/workflows/save.rs`
- Modify: `src/workflows/mod.rs` (add `pub mod save;`), `src/ui/app/save.rs`

- [ ] **Step 1:** Move these *pure* functions (no `App`, no ratatui type) from `ui/app/save.rs` to `workflows/save.rs`: `prepare_save`, `prepare_edit_save`, `plan_combined_save`, `membership_fanout`, `would_empty`, `decide_allocation`, `compose_renamed_dn`, `parent_dn`, `mask_changeset_secrets`, `format_validation_errors`, plus `PrepareSave`/`CombinedPlan` enums. Their tests move with them (into `workflows/save.rs`'s `#[cfg(test)] mod tests`).
- [ ] **Step 2:** Leave in `ui/app/save.rs` only orchestration that touches `App`/worker/`Overlay`: `submit_prepared`, `reload_form_sync`, `apply_combined_save`, `combined_save_overlay`, `read_group_members`, `apply_one_modify`, `allocate_number`. Update their `use` to `crate::workflows::save::{...}`.
- [ ] **Step 3:** Run the oracle — `334`, clippy clean.
- [ ] **Step 4:** Commit: `refactor: move pure save/diff/validation logic to workflows/save.rs`

### Task 11: Move pure create/password logic to `workflows/create.rs`

**Files:**
- Modify: `src/workflows/create.rs` (extend), `src/ui/app/create.rs`

- [ ] **Step 1:** Move these *pure* functions to the existing `workflows/create.rs`: `plan_create`, `stage_password`, `stage_edit_password`, `password_replace_mods`, `apply_static_defaults`, `mask_password_attrs`, `profile_for_entry`, `profile_for_entry_where`, `now_unix_secs_or_zero`. Tests move with them.
- [ ] **Step 2:** Leave in `ui/app/create.rs` only `App`/UI-touching orchestration: `prepare_create`, `open_create_form`, `build_new_entry_form` (returns `EditForm`). Update `use` paths.
- [ ] **Step 3:** Run the oracle — `334`, clippy clean.
- [ ] **Step 4:** Commit: `refactor: move pure create/password staging to workflows/create.rs`

### Task 12: Phase 2 review checkpoint

- [ ] Confirm `ui/app/` now contains **only** ratatui/`App`/`Overlay`/`EditForm`/`TreeItem`-touching code. Grep `src/ui/app` for `ModOp`/`SavePlan`/`ChangeSet` pure manipulation that slipped through.
- [ ] Note any real duplication the moves exposed; dedupe only if genuinely repeated (do not manufacture abstractions). One commit per dedupe.
- [ ] Oracle: `334`, clippy clean.

---

## Phase 3 — `Ctx` struct (OPTIONAL — evaluate after Phase 2, stop if it fights)

Removes the param-threading and the `#[allow(clippy::too_many_arguments)]`.

### Task 13: Introduce `Ctx` over the co-mutated set only

**Files:**
- Modify: `src/ui/app/mod.rs` (define `Ctx`, rewire `event_loop`), `action.rs`, `input.rs`, `save.rs`, `create.rs`

- [ ] **Step 1:** Define in `mod.rs`:

```rust
/// Per-tick orchestration receiver. Bundles ONLY the co-mutated state so
/// `&mut self` methods don't conflict on disjoint reads. Read-only/shared
/// resources (`structure`, `profiles`, `base_dn`) stay explicit method params.
pub(crate) struct Ctx<'a> {
    pub app: &'a mut App,
    pub worker: &'a WorkerHandle,
    pub read_flow: &'a mut ReadFlow,
    pub post: &'a mut HashMap<u64, PostWrite>,
    pub pending_followups: &'a mut HashMap<u64, (String, Vec<ModOp>, Option<String>)>,
}
```

- [ ] **Step 2:** In `event_loop`, construct per tick with a reborrow so the `terminal.draw(|f| view::ui(f, app))` borrow split is preserved:

```rust
let mut cx = Ctx { app: &mut *app, worker, read_flow: &mut *read_flow,
                   post: &mut post, pending_followups: &mut pending_followups };
```

- [ ] **Step 3:** Convert orchestration free functions to `impl Ctx` methods taking `&mut self` + the read-only params (`structure: &Structure`/`&mut Structure`, `profiles`, `base_dn`). Convert one cluster (e.g. `action.rs`) at a time, running `cargo build` after each to localise borrow errors. **If the borrow checker resists a specific method, leave that one as a free function — partial adoption is fine.**
- [ ] **Step 4:** Remove `#[allow(clippy::too_many_arguments)]` where it no longer applies.
- [ ] **Step 5:** Run the oracle — `334`, clippy clean.
- [ ] **Step 6:** Commit: `refactor(ui): bundle co-mutated orchestration state into Ctx`

### Task 14: Final review

- [ ] Oracle: `334`, clippy clean.
- [ ] `wc -l src/ui/app/*.rs src/workflows/*.rs` — confirm no oversized file remains.
- [ ] `grep -rn 'too_many_arguments' src/ui/app` — expect none (or a documented, justified remainder).
- [ ] Update `MEMORY.md` pointer for the decomposition outcome.

---

## Self-Review notes

- **Spec coverage:** Phase 1 (Tasks 1–9) = module split; Phase 2 (Tasks 10–12) = boundary relocation; Phase 3 (Tasks 13–14, optional) = Ctx. All three spec phases covered.
- **Test invariant:** every task asserts `334`. Tests are moved, never authored/deleted.
- **Function-name consistency:** function names in tasks match the current `app.rs` symbols (verified against the source grep on 2026-06-03).
- **No pasted bodies:** deliberate — this is verbatim code movement; pasting 2800 lines into the plan would invite accidental edits. The discipline rule (move verbatim, only `use`/visibility/wiring may change) is the safeguard.
