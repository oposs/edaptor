# tvision-rs Migration M2 — Edit + Write Spine (Design, 2026-06-25)

Second milestone of the tvision-rs UI migration (umbrella:
`docs/superpowers/specs/2026-06-23-tvision-ui-migration-umbrella-design.md`,
§6 "M2"). M1 shipped the read-only three-pane core (`edaptor-tv`); this milestone
turns the form pane into an **editable walking skeleton**: inline edit of plain
single-value fields, the typed `CommitOutcome` path into `form::{changeset,validate}`,
Confirm / Error / Guard dialogs, an async save wired through the worker + pump, and
dirty tracking with nav/quit guards.

This spec fixes the M2 design only. It then gets its own implementation plan
(writing-plans) and a task-by-task implement cycle.

---

## 1. Goal & scope

**Goal.** Edit and persist one real entry end-to-end from the tvision UI, with the
same write-path safety the ratatui UI has (client-side validation, secret-masked
LDIF preview, dirty guards), built on the neutral domain write-path that already
exists (`form::changeset`, `form::validate`, `workflows::save`).

**In scope.**
- `activate()` → `Activation::Inline` editing for **plain single-value** fields.
- A UI-neutral editable form model in `workflows::edit_form`.
- The `FieldWidget` registry seam (keyed by `WidgetKind`) with live `present()` for
  all kinds and live `activate()` for plain only.
- Async save through the worker + `PumpView`, with post-write re-read refresh.
- **MODIFY and MODRDN** execution from `SavePlan` (rename is reachable because the
  RDN attribute — typically `cn` — is a plain single-value editable field).
- Confirm (LDIF preview) / Error / Guard (save/discard/stay) dialogs.
- Dirty tracking + dirty-nav / dirty-focus-switch / dirty-quit guards, with
  quit deferred until the write completes.
- The umlaut/grapheme edit regression test, folded from the spike.

**Out of scope (later milestones).**
- Editing of multi-value, choice, password, picker, membership, objectClass fields
  — those fields render read-only (`present()`) and stay **disabled** (skip-focus)
  in M2 (M3 = objectClass + create; M4 = the rich widgets).
- Membership fan-out on save (no membership widget exists yet → `SavePlan` carries
  no fan-out in M2).
- Create-mode / profile chooser (M3).
- Config-discovery startup dialog (M5).
- Deduplicating the ratatui `ui::edit_form` against the new neutral model — that
  happens at the **M5 cutover** when the ratatui tree is deleted (see §3).

**Non-goals.** No domain-layer feature changes; no config-format changes; keys are
tvision idioms, not a 1:1 port of ratatui keybindings.

---

## 2. What already exists (build on, do not rebuild)

The write-path domain logic is built and unit-tested upstream of the UI:

- `form::changeset::diff(original, edited) -> ChangeSet` (`ModOp` / `ModRdn`;
  RDN-attribute change becomes a MODRDN, not a MODIFY).
- `form::validate::{validate, plan_save}` → `Vec<ValidationError>` / `SavePlan`.
- `workflows::save::prepare_save(...) -> PrepareSave` with variants
  `Invalid(Vec<ValidationError>)`, `DiffError(String)`, `NoChanges`, and
  `Ready { plan: SavePlan, dn: String, ldif: String }`. The `ldif` is already
  secret-masked (`mask_changeset_secrets`).
- `ldap::worker::WorkerHandle::{submit, request, poll}` — async submit + id-keyed
  poll is exactly the pattern M1's read path uses.

M1 UI seams reused: `Rc<RefCell<UiState>>` (`Shared`), the `PumpView` timer drain,
the `REFRESH` broadcast, `workflows::read_flow::ReadFlow`, `workflows::form_model`.

---

## 3. Editable form-state model — `workflows::edit_form` (new, neutral)

A UI-framework-agnostic editable model, ported from the **logic** of
`src/ui/edit_form.rs` but with **no `tui_prompts::TextState`** — the grapheme
editor lives in the tvision pane (§4), not the model.

```rust
// workflows::edit_form
pub enum FormMode { Edit }            // New arrives in M3

pub struct EditField {
    pub label: String,                // attribute name; `*` marks MUST on render
    pub must: bool,
    pub editable: bool,               // false for read-only kinds / read-only mode / non-plain
    pub multi: bool,
    pub secret: bool,
    pub ordered: bool,                // X-ORDERED → order matters in the dirty check
    pub orphaned: bool,               // no longer permitted by current objectClasses
    pub kind: FieldKind,
    pub widget: WidgetSpec,
    pub widget_binding: Option<config::widget::WidgetKind>,
    pub values: Vec<String>,          // current edited values (display order)
    pub baseline: Vec<String>,        // load-time snapshot for the dirty check
}

pub struct EditForm {
    pub dn: String,
    pub mode: FormMode,
    pub object_classes: Vec<String>,
    pub fields: Vec<EditField>,
}
```

Operations (pure, unit-tested):
- `build_edit_form(&FormModel, &SchemaModel, read_only: bool) -> EditForm` —
  derive fields from the read FormModel; seed `values` and `baseline` equal; set
  `editable` (plain single-value, writable kind, not read-only mode) and `orphaned`.
- `set_value(idx, String)` — write a committed inline edit into `fields[idx].values`
  (single-value semantics: empty → `vec![]` so the diff emits a delete, matching the
  ratatui `current_values()` rule).
- `current_values(idx) -> Vec<String>` — orphaned → `[]`; else the field's `values`.
- `is_dirty() -> bool` — per field, compare `values` vs `baseline`; **set-wise**
  (order-insensitive) unless `ordered`, then order-sensitive.
- `to_edit_entry() -> form::changeset::EditEntry` — `{ dn, attrs }` from
  `current_values()` over all fields.

**Decision — introduce fresh, dedup at M5.** The ratatui `ui::edit_form` is left
**untouched**; the two editable models coexist (same policy as the two UIs in
umbrella §7). Refactoring the live ratatui edit path to delegate into this neutral
core would destabilize the running `edaptor` binary mid-migration, against the
"one editor per tree / keep the running UI working" discipline. The ratatui copy is
deleted wholesale at the M5 cutover; the neutral model's ported unit tests guard
against behavioural drift until then.

---

## 4. Form pane rewrite — `src/tui/panes/form.rs`

M1 renders each field as one **disabled** `InputLine` holding `"label: value"`. M2
splits the row so the value is independently editable:

- **Row layout:** a left **label column** (width = clamped max label length over the
  form; MUST `*` marker; orphaned styling) + a value **`InputLine`** at
  `x = label_w + 2`. The label is drawn by the pane (or a static cell); the
  `InputLine` carries only the value.
- **Editability gate:** a row's `InputLine` is **enabled iff** the registry's
  `activate()` for that field yields `Inline` **and** the `EditField` is
  `editable && !orphaned`. In M2 that means plain single-value, writable,
  non-read-only fields only. Every other row keeps M1's `disabled` (skip-focus,
  read-only) state.
- **The `InputLine` is the grapheme-correct editor** — inline editing needs no
  extra machinery, and this is where the umlaut/grapheme test exercises the editor.
- **Commit:** on Enter (commit + advance to the next editable row) or focus-leave,
  read the `InputLine` value → `edit_form.set_value(idx, text)` → recompute dirty →
  repaint the status dirty marker. Borrow discipline: collect text into a local,
  drop the `RefCell` borrow, then mutate.
- The pane reads `edit_form` from `UiState` (replacing the read-only `FormModel`
  it renders today). Re-render is driven by the existing `REFRESH` broadcast +
  `form_needs_render` flag (§6).

---

## 5. `FieldWidget` trait + registry — `src/tui/widget.rs`

Extend the M1 trait and introduce the registry seam (umbrella §4.3) so M3/M4 add
widgets with **no form-core changes**:

```rust
pub trait FieldWidget {
    fn capability(&self) -> Capability;
    fn present(&self, field: &EditField) -> String;       // live for all kinds
    fn activate(&self, field: &EditField) -> Activation;  // M2: Plain -> Inline
}
```

- `Activation::Inline` is the only live variant in M2 (`Modal`/`Immediate` land in
  M3/M4 and are defined but unused here, or added then — keep M2 minimal).
- A registry maps `config::widget::WidgetKind` (+ the implicit plain/objectClass/
  sambaSid/nextNumber kinds) → plugin instance. The form pane asks the registry for
  the plugin, calls `present()` to render and `activate()` to decide editability.
- M2 ships `PlainWidget` with a live `activate() -> Inline`; all other kinds keep a
  `present()`-only plugin whose `activate()` is never reached (their rows are
  disabled). Note: `present()` now takes `&EditField` (was `&FormField` in M1) —
  the read-only presenters port across unchanged in body.

---

## 6. Save plumbing — `workflows::write_flow` (new) + pump extension

Mirror the read path. A `WriteFlow` owns the validate → diff → submit → correlate
cycle; `PumpView` drains write responses alongside read responses.

- **Submit.** `WriteFlow::prepare(&EditForm, &SchemaModel) -> PrepareSave` (thin
  wrapper over `workflows::save::prepare_save`). On `Ready { plan, dn, ldif }`, after
  the user confirms (§7), submit the `SavePlan`'s operations to the worker and record
  the request id.
- **SavePlan execution.** M2 handles the operations `SavePlan` actually yields for
  plain edits: a **MODIFY** changeset and, when the RDN attribute changed, a
  **MODRDN**. No membership fan-out (no membership widget yet). If `SavePlan`
  sequences modrdn-then-modify, submit and correlate them in order.
- **Correlate.** `UiState` gains `pending_write: Option<PendingWrite>` where
  `PendingWrite { id, quit_after: bool }`. The pump matches the response id, produces
  a `WriteOutcome::{ Ok, Err(String) }`, and clears `pending_write`.
- **Post-write.**
  - `Ok` → trigger a **re-read of the (possibly renamed) DN** through `ReadFlow`;
    the fresh `FormModel` rebuilds `EditForm` (baseline = new values → dirty clears);
    transient success in the status line. If `quit_after`, quit now.
  - `Err` → Error dialog (§7); edits and dirty state are preserved.
- **`UiState` flags:** `form_needs_render: bool` (re-render trigger; the M1
  `form_dirty` renamed to remove the clash) and the derived **unsaved-edits**
  predicate = `edit_form.is_dirty()`. A read-only `read_only: bool` gates the whole
  save path.

---

## 7. Dialogs — `src/tui/dialog/{confirm,error,guard}.rs`

tvision `Dialog`s. Modal-result plumbing uses `Program::exec_view` (the dialog is
modal only for the **decision**; the write itself stays async via the pump).

- **Confirm** (`confirm.rs`) — renders `PrepareSave::Ready.ldif` (already
  secret-masked); buttons **Save / Cancel**. Save → submit the write (async). Cancel
  → return to the form, edits intact.
- **Error** (`error.rs`) — dismissible; shows a `Vec<ValidationError>` (from
  `PrepareSave::Invalid`) or a worker `WriteOutcome::Err` string.
- **Guard** (`guard.rs`) — buttons **Save / Discard / Stay** over an
  `Intent { Nav(target) | Focus | Quit }`:
  - **Save** → run the save flow; on `Quit` intent set `quit_after = true` so the
    pump **defers the quit until the write returns `Ok`** (umbrella M2 requirement);
    on `Nav`/`Focus`, perform the pending transition after the write succeeds.
  - **Discard** → rebuild `EditForm` from baseline (drop edits), then perform the
    intent immediately.
  - **Stay** → cancel the intent; keep editing.

---

## 8. Dirty tracking + guards

- **Dirty** = `edit_form.is_dirty()` (set-wise per field vs `baseline`,
  order-sensitive only for `ordered`). The status line shows the current DN + a
  dirty marker.
- **Guard triggers** (only when the form is dirty): reselecting a leaf, switching
  focus away from the form pane, and quitting. Each raises an `Intent`; the Guard
  dialog (§7) resolves it.
- **Read-only mode** (`read_only`) disables every editable row and the entire
  save path; no guard ever fires (nothing is dirty).

---

## 9. Module layout (M2 delta)

```
workflows/edit_form.rs   NEW  neutral EditField/EditForm + build/dirty/to_edit_entry
workflows/write_flow.rs  NEW  prepare -> submit -> correlate -> WriteOutcome
tui/state.rs             EDIT edit_form, pending_write, form_needs_render, read_only
tui/widget.rs            EDIT activate() + WidgetKind registry; present(&EditField)
tui/pump.rs              EDIT drain write responses -> WriteOutcome; deferred quit
tui/panes/form.rs        EDIT label column + editable value InputLine; commit path
tui/app.rs               EDIT guard wiring on nav/focus/quit; menu/status save action
tui/dialog/mod.rs        NEW  module
tui/dialog/confirm.rs    NEW  LDIF-preview confirm dialog (exec_view)
tui/dialog/error.rs      NEW  dismissible error dialog
tui/dialog/guard.rs      NEW  save/discard/stay guard dialog
```

`src/ui/edit_form.rs` (ratatui) is **not** touched (see §3). Files stay small; a
module pushing past a couple hundred lines is a signal it is doing too much.

---

## 10. Testing & acceptance

**Headless unit (no TTY):**
- `workflows::edit_form` — `build_edit_form`, `set_value` semantics (empty → delete),
  `is_dirty` (set-wise + ordered), `to_edit_entry`. Ported from the ratatui tests.
- `workflows::write_flow` — `prepare` over `PrepareSave` variants; submit↔response
  id correlation and `WriteOutcome` mapping with a fake worker; deferred-quit flag.
- `tui::widget` — registry dispatch; `PlainWidget::activate() == Inline`; non-plain
  rows resolve to disabled.

**Headless pane (`tvision_rs::view::Context`):**
- Form pane: editable row gating (plain enabled, others disabled); InputLine edit →
  `set_value` → `is_dirty()`.
- **Umlaut/grapheme edit test** folded from the spike — a permanent regression test
  on the form value editor (a standalone `InputLine` needs
  `state.state.selected = true` in test setup).

**Live (gated `EDAPTOR_TEST_LDAP_URI`, skip when unset):**
- Edit a plain attribute on a real entry and persist it; re-read confirms the value.
- Rename via the RDN attribute (MODRDN) persists and the form follows the new DN.

**Interactive acceptance (human at a terminal — agent sessions have no TTY):**
- Edit and persist one real entry end-to-end.
- Guard fires on dirty leaf-nav, dirty focus-switch, and dirty quit; quit defers
  until the write completes.
- LDIF preview is correct and secret-masked; validation errors surface in the Error
  dialog.

**Discipline (umbrella §8):** strict TDD; atomic commits; crate compiles after every
commit; `cargo fmt` + clippy `--all-targets -D warnings` clean before done; cap
parallelism at 4 cores; one editor per working tree; facade guard stays green
(`! grep -rl "use tvision_rs" src | grep -vE "^src/(tui/|bin/edaptor-tv.rs)"`).
Commit trailer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.

---

## 11. Risks

- **MODRDN reachability.** Inline-editing the RDN attribute (`cn`) triggers a rename.
  Mitigated: `form::changeset` already classifies it and `prepare_save` produces the
  MODRDN; M2 only has to submit/correlate the op sequence and re-read the new DN.
- **Row-layout regression.** Splitting the M1 single-InputLine row into label +
  value risks read-only display drift. Mitigated by keeping the disabled-row path
  for non-editable fields and headless render assertions.
- **Async-save / deferred-quit ordering.** Quit-while-saving must not lose the
  write. Mitigated by the `quit_after` flag resolved only on `WriteOutcome::Ok`.
- **tvision dialog plumbing.** `exec_view` modal-result handling is first used here;
  surface any gap early and feed the upstream side-stream (umbrella §10), never block.
