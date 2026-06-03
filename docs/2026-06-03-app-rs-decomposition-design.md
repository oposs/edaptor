# app.rs decomposition — design

**Date:** 2026-06-03
**Status:** approved, ready for implementation plan
**Scope:** behaviour-preserving restructuring of `src/ui/app.rs`. No feature changes.

## Problem

`src/ui/app.rs` is 4703 lines (~2800 code + ~1900 tests) — by far the largest
file in the crate (next is `view.rs` at 1145). It has accreted ~10 distinct
responsibility clusters into one module. The rest of the crate is well-factored;
this is an isolated god-file.

Three distinct smells, each with its own fix:

1. **Everything-in-one-file.** Ten clusters (event loop, worker-response routing,
   key dispatch, value/picker editors, dirty-guard + navigation, single-entry
   save, combined membership save, create flow, password staging, structure/label
   helpers) share one file.
2. **Domain logic in the UI layer.** `src/ui/mod.rs` documents a boundary rule:
   *"ratatui / crossterm are imported only inside `ui`; every other module deals
   in plain domain types."* Yet `app.rs` holds large amounts of pure
   `ChangeSet`/`ModOp`/`SavePlan`/validation/password logic that never touches
   ratatui.
3. **Param-threading.** `handle_worker_response` carries
   `#[allow(clippy::too_many_arguments)]`. Every orchestration function threads the
   same set: `app, worker, read_flow, structure, profiles, base_dn, post,
   pending_followups`.

## Goals

- Idiomatic, maintainable Rust; DRY where duplication is real.
- **Behaviour-preserving.** The oracle: `cargo test` = **334 passed**, `cargo
  clippy --all-targets` = **clean**. Every phase ends here. The test count must
  not change (tests move between files; none are added or dropped in Phases 1–3).
- No file left materially oversized (target: no `app/` file > ~900 lines).

## Non-goals

- No feature changes, no behaviour changes, no new tests.
- No change to the immediate-mode event-loop architecture or the deliberate
  "borrow split" documented at the top of `app.rs` (worker/read_flow/structure/
  write-maps live as loop locals so `terminal.draw`'s `&mut App` borrow never
  collides with orchestration borrows).
- No unrelated refactoring of other modules beyond receiving relocated functions.

## Design — three independently committable phases

The phases are ordered safest-first so value banks early and the speculative work
can be abandoned without losing the rest.

### Phase 1 — Pure module split (zero signature changes)

Convert `src/ui/app.rs` into a `src/ui/app/` directory module. Functions stay
**free functions with identical signatures** — this is text-movement plus `use`
lines and `pub(crate)`/`pub(super)` visibility tweaks, nothing more. Tests move
with the units they cover.

| New file | Holds |
|---|---|
| `app/mod.rs` | `App`, `Pane`, `run()`, `event_loop()`, `handle_worker_response()`, module decls + re-exports |
| `app/overlay.rs` | `Overlay`, `GuardIntent`, `PendingAction`, `PostWrite` enums |
| `app/input.rs` | `dispatch_key`, `overlay_key`, `choose_profile_key`, `guard_key`, `edit_focused_field`, `open_value_editor`, `value_editor_key`, `picker_editor_key`, `service_picker_search` |
| `app/action.rs` | `handle_action`, `execute_pending`, `perform_guard_intent`, `navigate_to`, `reconcile`, `guard_if_dirty`, `refresh_structure`, `revert_form`, `rebind_selection`, pane/index helpers |
| `app/save.rs` | save + combined-save orchestration: `submit_prepared`, `reload_form_sync`, `apply_combined_save`, `read_group_members`, `apply_one_modify`, `allocate_number` |
| `app/create.rs` | create orchestration: `prepare_create`, `open_create_form` |
| `app/structure_view.rs` | UI-typed structure/label helpers: `build_tree_items`, `compute_rows`, `render_node_label`, `label_rules`, `label_rule_attrs`, `LabelRule`, `structure_inputs`, `structure_input_from_attrs` |

Module-private helpers (`next_id`, `next_pane`, `prev_pane`, `next_index`,
`object_classes_of`, `dedupe_ci`, `membership_candidate_label`) live with their
primary caller or in `mod.rs` if shared.

**Exit:** 334 tests green, clippy clean, no `app/` file > ~900 lines. Commit.

### Phase 2 — Honour the boundary rule

Relocate genuinely domain-pure logic (no ratatui types, no `App`) out of `ui/`:

- **To `workflows/save.rs` (new) / `form/`:** `prepare_save`, `prepare_edit_save`,
  `plan_combined_save`, `membership_fanout`, `would_empty`, `decide_allocation`,
  `compose_renamed_dn`, `parent_dn`, `mask_changeset_secrets`,
  `format_validation_errors`.
- **To `workflows/create.rs` (extend existing):** `plan_create`, `stage_password`,
  `stage_edit_password`, `password_replace_mods`, `apply_static_defaults`,
  `mask_password_attrs`, `profile_for_entry`, `profile_for_entry_where`,
  `now_unix_secs_or_zero`.

**Stays in `ui/`** (produces UI types): anything returning `Overlay`, `EditForm`,
`TreeItem`, `ValueEditor`, or a ratatui widget — e.g. `combined_save_overlay`,
`build_loaded_form`, `build_new_entry_form`, `build_tree_items`.

Real duplication surfaced by the move is deduplicated; no abstraction is
manufactured to chase a DRY target. Tests for relocated pure functions move to
their new home. **Exit:** 334 green, clippy clean. Separate commit.

### Phase 3 — `Ctx` struct (optional; evaluate after Phase 2)

Remove the param-threading and the `too_many_arguments` allow.

**Constraint that keeps this out of borrow-checker hell:** bundle only the
**co-mutated** set into the receiver — `app`, `read_flow`, `post`,
`pending_followups`. Keep **read-only / shared** resources (`structure`,
`profiles`, `base_dn`) as explicit method params. Forcing disjoint reads through
`&mut self` is what creates conflicts the split-param form doesn't have; we avoid
that deliberately.

`Ctx` is reborrowed per tick (`Ctx { app: &mut *app, .. }`) so the documented
`terminal.draw` borrow split is preserved. Orchestration free functions become
`impl Ctx` methods.

**This phase is optional.** If it fights the borrow checker, stop — Phases 1–2
have already delivered the maintainability win. **Exit (if done):** 334 green,
clippy clean, no `too_many_arguments` allow remaining. Separate commit.

## Execution

Orchestrated with subagents, but **sequentially**, not fan-out: each phase (and
each move within a phase) edits overlapping files, so parallel agents on the same
file would conflict (a known failure mode in this repo — subagents stall on the
whole `app.rs`). The orchestrator dispatches one focused, well-bounded step at a
time and runs `cargo build` + `cargo test` between steps, reverting any step that
breaks the oracle. Subagents are given **narrow line ranges**, never the whole
file.

## Risks

- **Borrow checker (Phase 3).** Mitigated by the co-mutated-only `Ctx` rule above;
  phase is optional and isolated.
- **Test drift.** Mitigated by asserting the exact 334 count after every step.
- **Visibility churn.** Moving free functions across module boundaries needs
  `pub(crate)`/`pub(super)`; mechanical, caught immediately by the compiler.
- **`view.rs` coupling.** `view::ui` reads many `App` fields; Phase 1/2 keep `App`
  and its field visibility intact, so `view.rs` is untouched.
