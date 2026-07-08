# M3 Phase 2a — objectClass widget + live schema resync

**Date:** 2026-06-27 · **Status:** design (approved, pre-plan) · **Branch:** `feat/tvision-ui`

Phase 2a of the M3 milestone. M3 was first split into Phase 1 (stabilize the
form/nav base — DONE) and Phase 2 (the M3 core). At brainstorming the user split
Phase 2 again into two spec→plan→implement cycles:

- **Phase 2a (this doc):** the first modal-widget infrastructure, the objectClass
  picker, and live schema-driven field regeneration — operating on **existing
  entries** only.
- **Phase 2b (separate spec):** the create flow — Alt+N profile chooser,
  single-profile fast path, `FormMode::Create`, create-mode form, objectClass
  auto-injection, and the `plan_create`/`build_add_entry` submit path.

The split front-loads the riskiest piece (umbrella §6 M3: "ObjectClass picker …
the riskiest part") and gives each cycle a clean, independently-verifiable
acceptance gate. The create flow (2b) then builds on a proven resync.

## Scope

All changes are confined to the tvision UI (`src/tui/**`) and the neutral
`workflows::edit_form` model. **No ratatui (`src/ui/**`) changes** and no
domain-layer behaviour changes (the create helpers in `workflows::create` are
already implemented and are untouched here).

1. **Port `sync_schema_fields`** into the neutral `EditForm`.
2. **Reusable modal-widget seam** — `Activation::Modal(Box<dyn FieldEditor>)` plus
   a generic dispatch that runs any field-editor `Dialog` and applies its typed
   `CommitOutcome`. The objectClass picker is the first implementation.
3. **ObjectClass picker widget** — schema-seeded multi-select over a `ListBox`,
   client-side substring filter, returning `SetValuesThenResyncSchema`.
4. **Live resync on commit** — editing objectClass on an existing entry adds/orphans
   form fields immediately, driven by the typed outcome (no global flag).

### Non-goals (deferred to Phase 2b or M4)

- The create flow: Alt+N profile chooser, `FormMode::Create`, create-mode form,
  objectClass auto-injection, `plan_create`/`build_add_entry` submit path. **(2b)**
- The rich widgets choice / password / picker / membership and the free-text
  multi-value value editor. **(M4)** — they reuse the `FieldEditor` seam built here.
- Guardrails beyond the shipping ratatui UI (no structural-class lock, no
  data-loss warning on orphaning a populated field). Parity: the server validates
  on save. *(user-chosen)*
- Reorganising `widget.rs` into a `widgets/` module directory — deferred to M4 when
  there are five widget impls.

## Background (current state — verified)

- **Neutral `workflows::edit_form`** (M2): `EditForm { dn, mode, object_classes,
  fields }`, `EditField { label, must, editable, multi, secret, ordered, orphaned,
  kind, widget, widget_binding, values, baseline }`, `FormMode::Edit` (only
  variant), and methods `set_value`, `is_dirty`, `to_edit_entry`, `build_edit_form`.
  **No `sync_schema_fields`.** `build_edit_form` seeds `values = baseline` (clean)
  and leaves `object_classes` empty for the caller to fill.
- **`form.object_classes`** is populated by `state.rs:145` from the leaf node's
  cached OCs (`requested_leaf` carries `(dn, object_classes)`), which is a separate
  source from the `objectClass` **field** inside `fields` (built from the read's
  `FormModel`). Both must stay consistent after an objectClass edit.
- **`tui::widget`**: `trait FieldWidget { capability; present; activate }`,
  `PlainWidget` (→ `Inline`), `Capability { Static, NeedsSchema, NeedsWorkerSearch }`,
  `CommitOutcome { SetValues, StageSecret, SetValuesThenResyncSchema, Cancelled }`
  (already complete), `Activation { Inline }` (only variant), `present_field`,
  `inline_editable(field) = editable && !multi && !orphaned && widget_binding.is_none()`.
- **`panes/form.rs`**: per-field `label_ids`/`value_ids` cells inside a `ScrollGroup`;
  value cell enabled iff `inline_editable(field)`. `handle_event` intercepts Up/Down
  for nav and delegates to the group. **No Enter/activation handling.**
- **`app::dispatch`** is the single `&mut Program` site (`run_app`'s closure); it
  already runs guard/confirm/error modals via `prog.exec_view_focused(view, default)`.
  There is **no widget-activation modal path** yet.
- **`schema::SchemaModel`** exposes `object_class_names() -> Vec<String>` (sorted),
  `effective_attributes(&[&str]) -> ResolvedAttributes { must, may }`,
  `field_kind(attr) -> FieldKind`, `is_single_value(attr) -> bool`. Reachable at
  form-build/dispatch time via `state.read_flow.schema()`.
- **Reference impl** to port: `ui/edit_form.rs:394–462` `sync_schema_fields` and the
  `order_fields` helper (`ui/edit_form.rs:741–756`); the objectClass picker UX is
  `ui/edit_form.rs:253` `open_objectclass` (pre-tick current OCs, all schema OC
  names, case-insensitive, substring filter).

## Architecture

Two hard constraints shape the whole design:

- **Only `run_app`'s dispatch closure holds `&mut Program`** → a pane cannot open a
  modal itself; it must post a command that surfaces to `app::dispatch`. This is the
  same controller-owned pattern the guard machinery uses (`GUARD_NAV`).
- **Umbrella mandate: "the typed resync outcome (no global flag) works
  end-to-end"** → the resync is driven by applying the `SetValuesThenResyncSchema`
  `CommitOutcome` inline in dispatch, **not** by a `needs_resync` flag polled by the
  pump.

Rejected alternatives: pane-runs-modal (infeasible — no `&mut Program` outside the
closure); pump-polled resync flag (umbrella anti-goal, replaces the typed outcome).

### Component map

| Unit | File | Responsibility |
|---|---|---|
| `EditForm::sync_schema_fields` | `workflows/edit_form.rs` | Pure: recompute orphaned/must, inject new fields, reorder. Ported from ratatui. |
| `make_field` helper | `workflows/edit_form.rs` | One place that configures an `EditField` (kind/multi/editable/secret/widget_binding) — shared by `build_edit_form` and injection. |
| `Activation::Modal`, `trait FieldEditor` | `tui/widget.rs` | The reusable modal seam. |
| `widget_for(field)` | `tui/widget.rs` | Registry: objectClass label → `ObjectClassWidget`, else `PlainWidget`. |
| `ObjectClassWidget` + picker dialog | `tui/oc_picker.rs` (new) | `NeedsSchema` widget; builds the multi-select `Dialog`; stages `SetValuesThenResyncSchema`. |
| Enter→activate, modal-cell gating | `tui/panes/form.rs` | Detect focused field, post `ACTIVATE`, swallow edit keys on modal rows. |
| `ACTIVATE` dispatch + apply | `tui/app.rs` | Build editor, `exec_view_focused`, apply staged `CommitOutcome`. |
| `apply_commit`, new state fields | `tui/state.rs` | `activate_field`, `staged_commit`; testable apply logic. |

### Data flow (editing objectClass on an existing entry)

```
user focuses objectClass row, presses Enter
  └─ FormPane::handle_event: widget_for(field).activate() == Modal
       → state.activate_field = Some(idx); ctx.post(ACTIVATE); consume key
  └─ pump/run_app surfaces ACTIVATE to app::dispatch
       → take activate_field; field = form.fields[idx]
       → editor = widget_for(field).activate()  (Modal(FieldEditor))
       → (view, default_btn) = editor.into_view(schema, shared)   // seeds candidates, pre-ticks
            · the view maintains the prospective CommitOutcome in
              state.staged_commit as the user toggles (always reflects "what OK
              would commit"); buttons are plain OK / CANCEL
       → cmd = prog.exec_view_focused(view, default_btn)          // single exec_view site
       → if cmd == OK: outcome = state.staged_commit.take()       // CANCEL ⇒ discard it
            → state.apply_commit(idx, outcome, schema):
                 · set fields[oc_idx].values = ocs
                 · form.object_classes = ocs           // keep the two sources consistent
                 · form.sync_schema_fields(schema)      // add/orphan/reorder
                 · form_needs_render = true
  └─ FormPane::render sees form_needs_render → rebuild_cells (new field set)
```

Borrow discipline: every step that calls `exec_view_focused`, `ctx.post`,
`new_list`, `child_mut`, or `broadcast` first collects what it needs into locals and
drops the `UiState` borrow (per the project conventions). The OK-button handler
inside the dialog writes `staged_commit` from its own short-lived borrow.

### `sync_schema_fields` port (neutral)

Faithful to `ui/edit_form.rs:394–462`:

1. Read the current `objectClass` field's `values` (find by case-insensitive label;
   empty if absent — no panic).
2. `allowed = effective_attributes(ocs).must ∪ .may ∪ {objectclass}` (lowercased).
3. For each existing field except objectClass: `orphaned = !allowed.contains(key)`;
   `must = allowed && resolved.must contains label` (orphaned ⇒ `must=false`).
   objectClass itself is never orphaned.
4. Inject an empty field (via `make_field`) for every MUST∪MAY attr not already
   present, with `kind`/`multi` from schema.
5. Reorder (port `order_fields`) so orphaned fields sink to the bottom.

Values on still-allowed fields are preserved (only `orphaned`/`must`/membership
change). This is the single source of the "fields add/orphan live" behaviour.

### Modal seam

```rust
// tui/widget.rs
pub enum Activation {
    Inline,
    Modal(Box<dyn FieldEditor>),
}

pub trait FieldEditor {
    /// Build the modal view (and its default button id). The view captures the
    /// shared UiState and keeps `staged_commit` current as the user interacts,
    /// so it always reflects what an OK would commit. Buttons are plain OK/CANCEL.
    fn into_view(self: Box<Self>, schema: &SchemaModel, shared: &Shared)
        -> (Box<dyn View>, Command);
}
```

`exec_view` consumes the view and a `Command::OK` button is ended by the built-in
modal handler before the view could run commit logic, so the result is surfaced the
same way the guard dialogs surface theirs: the view maintains the prospective
`CommitOutcome` in `UiState::staged_commit` continuously (updated on every toggle),
and dispatch reads it back by the `exec_view` **return code** — apply on `OK`,
discard on `CANCEL`. Registry:

```rust
pub fn widget_for(field: &EditField) -> Box<dyn FieldWidget> {
    if field.label.eq_ignore_ascii_case("objectClass") {
        Box::new(ObjectClassWidget)
    } else {
        Box::new(PlainWidget)
    }
}
```

(M4 extends `widget_for` to dispatch on `field.widget_binding` kinds — no form-core
change, per umbrella §4.3.)

### ObjectClass picker dialog

A tvision `Dialog` containing a search `InputLine` and a `ListBox` of object-class
names (tick-marked), plus OK/Cancel. Seeded from `schema.object_class_names()`;
current OCs pre-ticked (case-insensitive, matching `open_objectclass`); the search
box client-substring-filters the list; Space toggles the highlighted row's tick;
**OK** stages `SetValuesThenResyncSchema(ticked)`, **Cancel** stages nothing.
Buttons use modal-exit commands (`OK`/`CANCEL`) so `exec_view` returns them.

### Form-pane activation & modal-cell gating

The objectClass value cell stays an **enabled** (therefore focusable) `InputLine`,
but for modal-activatable fields the pane gates input: in `handle_event`, when the
focused field's `activate()` is `Modal`, **Enter** posts `ACTIVATE` (after stashing
`activate_field`) and is consumed, and all character/edit keys are swallowed so the
cell is never text-edited (render keeps showing `present()` = `‹N values›`). Up/Down
nav is unchanged; inline (plain single-value) rows keep the M2 inline-edit path
untouched. The pane maps the group's focused child id → `value_ids` index to know
which field is active.

## Error handling & invariants

- **No objectClass field / no schema:** activation and `sync_schema_fields` are
  find-guarded no-ops; never panic.
- **Empty or structural-class results:** accepted (parity); the LDAP server rejects
  illegal objectClass sets at save time via the existing write-error dialog.
- **Borrow discipline:** no `RefCell`/`UiState` borrow held across
  `exec_view`/`post`/`broadcast`/`new_list`/`child_mut`.
- **Idempotent render:** `form_needs_render` is set on apply; the pane clears it on
  rebuild (same contract as M2/Phase 1).
- **Facade boundary:** the neutral `sync_schema_fields` imports no UI crate; only
  `src/tui/**` touches `tvision_rs`.

## Testing

**Neutral (headless, no tvision):**
- `sync_schema_fields`: add OC → fields injected with right kind/multi; remove OC →
  affected fields `orphaned=true` and sorted to the bottom; objectClass never
  orphaned; must/may flags recomputed; values preserved on still-allowed fields;
  absent objectClass field → no-op.
- `make_field` parity: an injected field equals one built by `build_edit_form` for
  the same attr.

**Widget / registry:**
- `widget_for`: objectClass label (any case) → `ObjectClassWidget`; other labels →
  `PlainWidget`. `ObjectClassWidget::activate()` is `Modal`; `present()` unchanged;
  `capability() == NeedsSchema`.

**Picker dialog (headless `Context::new` harness):**
- Seeds candidates from a fixture schema; current OCs pre-ticked; substring filter
  narrows; Space toggles. After toggles, `staged_commit` reflects
  `SetValuesThenResyncSchema(expected_set)`; dispatch applies it on an `OK` return
  code and discards it on `CANCEL`.

**Form pane / dispatch:**
- Enter on the objectClass row sets `activate_field` and posts `ACTIVATE`; character
  keys are swallowed on a modal row; Enter on a plain inline row does not activate.
- `apply_commit(SetValuesThenResyncSchema)` updates the objectClass field values,
  sets `form.object_classes`, resyncs, and sets `form_needs_render`; `Cancelled` is
  a no-op.

**Live (gated by `EDAPTOR_TEST_LDAP_URI`):**
- `tests/tv_objectclass.rs` (skips unless env set): on an existing demo entry, apply
  an objectClass change and assert the resulting field set adds/orphans as expected.
- Final **tmux PTY acceptance** (agent-driven, per the handover recipe): focus the
  objectClass row → open the picker → tick/untick → OK → observe fields appear /
  sink live; Cancel leaves the form unchanged. Demo data left intact (edit-then-
  revert / no illegal save).

## Acceptance criteria (umbrella M3, 2a portion)

1. Editing objectClass on an existing entry **adds** newly-allowed fields and
   **orphans** now-disallowed fields **live** (no save round-trip needed).
2. The change is driven by the typed `SetValuesThenResyncSchema` outcome applied in
   dispatch — **no global resync flag**.
3. The reusable `FieldEditor` seam is in place (objectClass is its first impl) so M4
   widgets register with no form-core change.
4. `make check` green (fmt + clippy `-D warnings` + tests); both facade guards
   clean; demo data intact after the live pass.

## Documentation touch-points

- `CHANGES.md`: new entry under the unreleased tvision-preview section (objectClass
  editing + live field regeneration in the tvision UI).
- mdBook M3 page: the objectClass editing behaviour is documented when the M3 core
  lands; 2a contributes the "editing objectClass regenerates fields" paragraph.
- `docs/HANDOVER.md` + `.superpowers/sdd/progress.md`: per-task ledger as usual.
