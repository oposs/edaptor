# M3 Phase 2b — the create flow (+ autonumber + password widget)

**Date:** 2026-06-28 · **Status:** design (approved, pre-plan) · **Branch:** `feat/tvision-ui`

Phase 2b of the M3 milestone — the second half of the M3 core, building on the
Phase 2a modal-widget seam and live schema resync. M3 was split Phase 1 (stabilize)
→ Phase 2 (the core), and Phase 2 was split 2a (objectClass widget + resync, DONE)
→ 2b (this doc: the create flow).

The user chose the **full** create scope: not just the core create flow, but also
**autonumber** (next-free uidNumber/gidNumber) and the **password widget** (which the
umbrella scheduled for M4). Pulling the password widget forward effectively completes
the M4 password item. This is ~3 cohesive sub-features; it stays one spec but the
plan sequences three independently-testable blocks (A core create, B autonumber,
C password) — see §9.

## Scope

Confined to the tvision UI (`src/tui/**`) and the neutral `workflows::*`/`config`
layers. No ratatui (`src/ui/**`) changes. The neutral create helpers
(`workflows::create`) and the worker `Request::Add` already exist and are reused.

1. **Create entry point** — Alt+N from the current DIT branch; single-profile fast
   path, profile chooser dialog when >1 profile matches, status when 0.
2. **Create-mode form** — `FormMode::Create`, schema-driven empty form, objectClass
   auto-injection (editable via the 2a picker), live DN composition from the RDN
   field, static defaults applied.
3. **Autonumber** — auto-allocate next-free numeric fields on form open via an async
   worker scan correlated by the pump.
4. **Password widget** — a general (create + edit) TLS-gated `FieldWidget`: a
   New+Confirm masked modal returning `StageSecret`; refuses on an unencrypted
   connection.
5. **Submit (ADD)** — Alt-S in create mode validates + diffs via `plan_create`,
   confirms the LDIF, submits `Request::Add`, and navigates to the new entry.

### Non-goals

- No changes to the shipping ratatui UI.
- No new LDAP primitives — the worker `Request::Add` is reused as-is.
- Delete/move of entries (not part of M3).
- The other M4 rich widgets (choice / picker / membership / free-text multi-value
  editor) — only the password widget is pulled forward here, because create needs it.

## Background (current state — verified)

- **`workflows::create`** (pure, tested): `plan_create(schema, profile, container,
  edited) -> CreatePrep::{Confirm{dn,attrs,container,ldif}, Error}` (reads the RDN
  value from the edited form's `rdn_attr` field; validates the composed entry);
  `build_add_entry` (DN = `<rdn_attr>=<rdn_value>,<container>`; attrs = form attrs +
  `["top"]+profile.object_classes` deduped + RDN attr); `empty_form_for_profile`
  (schema-driven empty `FormModel`, excludes objectClass); `profiles_for_container`
  (DN-boundary match); `apply_static_defaults` (fills literal/template defaults,
  returns `Vec<(attr,min,max)>` autonumber requests still needing a scan);
  `fold_create_password` (folds a staged cleartext into the ADD attrs incl. samba);
  `mask_password_attrs`; `profile_for_entry`.
- **Worker:** `Request::Add { id, dn, attrs }` + `run_add` exist (built for the
  ratatui create flow); `Response::{WriteOk, WriteError}` correlate writes.
- **`write_flow`** handles MODIFY + MODRDN (`submit`, `submit_followup`,
  `on_response`); it has **no ADD path** — 2b adds one.
- **`workflows::edit_form`** has `FormMode::Edit` only; `EditForm::sync_schema_fields`
  + `apply_commit`'s resync (2a) exist. `prepare_save`'s `password_mods`/`mask_attrs`
  are currently `[]` (M4 placeholders).
- **`config`:** `EntryProfile { name, object_classes, rdn_attr, search_base, show,
  search_attrs, defaults, widgets, label }`; `WidgetSpecCfg::Password { samba }` and
  `config::widget::{ResolvedWidget, password_widget_for, password_add_attrs}` exist;
  `defaults` carries `{next:MIN-MAX}` autonumber specs.
- **Reference (ratatui):** `ui/app/create.rs` (create-form build, container,
  allocate-on-open), `ui/app/password_editor.rs` (TLS-gated New+Confirm,
  `app.connection_encrypted`, refusal message), `ui/app/save.rs::allocate_number` +
  `workflows/save.rs` (scan + limit-refusal). These are the behaviours to port into
  the neutral/tvision layers.
- **TLS state:** `ServerConfig` carries `start_tls` + scheme (ldaps/ldapi); the
  tvision `UiState` has **no** `connection_encrypted` flag yet — 2b adds it.

## Architecture

Two constraints (same as 2a) shape the design:
- **Only `run_app`'s dispatch closure holds `&mut Program`** → panes/pump post
  commands; modals (chooser, password editor, confirm) run in `app::dispatch`.
- **Async work goes through the worker + pump** → autonumber scans and the ADD
  submit are posted and correlated in the pump, never blocking.

Borrow discipline is load-bearing (the 2a lesson): a modal editor must NOT
`borrow_mut` shared state during construction/`into_view` (dispatch holds a borrow to
pass schema); stage via `reset_current`/on events.

### Component map

| Unit | File | Responsibility |
|---|---|---|
| `FormMode::Create { profile_idx, container }` | `workflows/edit_form.rs` | Mark a form as composing a new entry. |
| `build_create_form` | `workflows/create.rs` (or `edit_form.rs`) | Neutral: empty form + objectClass field seeded with `["top"]+ocs` + `sync_schema_fields` + static defaults; returns the form + autonumber requests. |
| `composed_create_dn` | `workflows/edit_form.rs` | Neutral: `<rdn_attr>=<rdn_value>,<container>` for the live header. |
| `AllocFlow` | `workflows/alloc_flow.rs` (new) | Async next-number: post scan `Request::Search`, correlate, compute next free via the `save.rs` allocation logic. |
| create-submit | `workflows/write_flow.rs` | `submit_create` (`Request::Add`) + `WriteIntent::Create` + `WriteOutcome::Created{dn}`. |
| `connection_encrypted` | `tui/state.rs` | Bool set at bootstrap from `ServerConfig`. |
| `PasswordWidget` + `PasswordEditor` | `tui/pw_editor.rs` (new) | `FieldWidget` (mask present, Modal activate) + the TLS-gated New+Confirm editor → `StageSecret`. |
| profile chooser | `tui/dialog/profile_chooser.rs` (new) | A `Dialog`+`ListBox` of profile names → chosen index. |
| `CREATE`/Alt+N | `tui/mod.rs`, `tui/app.rs` | Command + menu/status wiring; dispatch entry point + `do_create`. |
| Alt-S branch | `tui/app.rs` | `FormMode::Edit`→`do_save`; `Create`→`do_create`. |
| widget routing | `tui/widget.rs` | `widget_for`: Password binding → PasswordWidget; objectClass → ObjectClassWidget; else plain. |
| `apply_commit` StageSecret | `tui/state.rs` | Stash `pending_password` (used by create fold + edit prepare). |

### Block A — core create

1. **Command + entry.** `CREATE = Command::custom("edaptor.create")`, added to the
   menu ("~N~ew") + status (`~Alt-N~ New`). `dispatch` `CREATE` arm:
   - `container = state.current_branch` (status + return if none).
   - `idxs = profiles_for_container(&profiles, &container)`: `len 0` → status; `len 1`
     → `open_create(profiles[idxs[0]], container)`; `len >1` → run the profile chooser
     modal, on OK `open_create(profiles[chosen], container)`.
2. **`open_create`** (in dispatch, borrow-disciplined): build the create form via the
   neutral `build_create_form`, set `state.edit_form = Some(form)`, post autonumber
   scans (§Block B), `form_needs_render = true`.
3. **`build_create_form(schema, profile) -> (EditForm, Vec<(attr,min,max)>)`**
   (neutral): `empty_form_for_profile` → `build_edit_form` → set
   `mode: FormMode::Create{profile_idx, container}`; **inject an objectClass field**
   (multi, editable, values = `["top"]+profile.object_classes` deduped) so the 2a
   picker can edit it and resync runs; call `sync_schema_fields(schema)` to populate
   MUST/MAY fields; `apply_static_defaults(&profile.defaults, …)` to fill literals and
   collect autonumber requests; return the form + requests.
4. **Create-mode header.** `header_text` composes the DN for `FormMode::Create` from
   the current `rdn_attr` field value + container (`composed_create_dn`), with a
   ` (new)` marker and the dirty `*`. Empty RDN renders `<rdn_attr>=…,<container>`.
5. **Submit.** Alt-S → `dispatch` SAVE arm branches on `mode`: `Create` → `do_create`:
   - `plan_create(schema, profile, container, &form.to_edit_entry())` (borrow-drop
     first). `Error(msg)` → error dialog. `Confirm{dn,attrs,ldif}` → fold the staged
     password (§Block C) into `attrs`/`ldif`, run the confirm dialog
     (`exec_view_focused`, OK-focused); on OK `write_flow.submit_create(worker, &dn,
     attrs, quit_after)`.
   - `write_flow`: `submit_create` allocs an id, `worker.submit(Request::Add{id,dn,
     attrs})`, records `WriteIntent::Create{dn}`. `on_response` WriteOk for a Create →
     `WriteOutcome::Created{dn}`.
   - pump `apply_write_outcome` Created → set `current_leaf = dn`, `list_dirty = true`,
     re-read the new entry (so it reloads in `FormMode::Edit`), clear create state.
6. **Guards.** A dirty create form participates in the existing dirty-nav/quit guard
   (Save/Discard/Stay) unchanged; Discard drops the create form.

### Block B — autonumber (AllocFlow)

- **`AllocFlow`** (new, mirrors `read_flow`/`write_flow`): `request(worker, attr,
  min, max) -> id` posts a `Request::Search` (base = `base_dn`, filter `(attr=*)`,
  attrs `[attr]`) tracking `id → (attr, min, max)`. `on_response(resp)` for a tracked
  Entries id computes the next free value via the neutral allocation logic
  (`workflows::save` scan/limit handling — extract a pure
  `next_free_number(existing, min, max) -> Result<u64, ScanLimited>` if not already
  present) and returns `AllocOutcome::Filled{attr, value}` or `AllocOutcome::Limited`.
- **Pump** routes Entries responses to AllocFlow (after read_flow, before write_flow;
  the ids are disjoint by range like read/write). On `Filled` it fills the field
  (find by label, `set_value` if still empty/`‹allocating…›`) + `form_needs_render`;
  on `Limited` it sets a status and leaves the field empty.
- **Presentation.** Until filled, the field shows `‹allocating…›` (a transient value
  written into the field at `open_create`); the user may type over it. No separate
  nextNumber widget — auto-allocate covers the create case.

### Block C — password widget (general, TLS-gated)

- **`connection_encrypted: bool`** on `UiState`, set at bootstrap from the connection
  (ldaps:// or ldapi:// scheme, or `start_tls`).
- **`PasswordWidget`** (`FieldWidget`): `capability() = Static`; `present()` →
  `‹set›`/`‹unset›` (never the value); `activate()` →
  `Modal(Box::new(PasswordEditor{ attrs, encrypted }))` (the editor captures the
  binding's add-attrs + the encrypted flag from the field/shared at activate time).
- **`widget_for`** extended: `field.widget_binding == Some(WidgetKind::Password{..})`
  → PasswordWidget; objectClass label → ObjectClassWidget; else PlainWidget.
  `is_modal_field` returns true for password fields too (so the form pane makes them
  focusable + Enter-activatable, like objectClass).
- **`PasswordEditor::into_view`** (borrow-safe — no `borrow_mut` in construction):
  if `!encrypted` → build a refusal `Dialog` (message: "Changing a password requires
  an encrypted connection (ldaps://, ldapi://, or start_tls).", OK only) that never
  stages. Else a **New + Confirm** dialog: two masked `InputLine`s (echo-masked) +
  OK/Cancel. Staging follows the **2a model** (a `Command::OK` button ends the modal
  before the view could run commit logic): the editor keeps `staged_commit` **live**
  as the user types — set `CommitOutcome::StageSecret{ attrs, cleartext }` whenever
  both fields are non-empty and equal, else `None` — updated on each `handle_event`.
  `dispatch` applies the staged outcome on an `OK` return code and discards it on
  `CANCEL` (so a mismatch/empty at OK time commits nothing). The list-seed analogue
  is not needed, but the same `reset_current`-vs-construction borrow rule applies:
  never `borrow_mut` shared during `into_view`.
- **`apply_commit` StageSecret arm** (currently no-op): set `state.pending_password =
  Some(cleartext)` (and record which attrs). Mark `form_needs_render` so the field
  shows `‹set›`.
- **Folding the staged secret:**
  - **Create:** `do_create` calls `fold_create_password(&dn, &mut attrs, pending,
    &resolved_widgets, now)` before the confirm preview (so the LDIF shows the masked
    secret) and into the ADD.
  - **Edit:** `prepare_save` gains the staged secret → the neutral `password_mods`
    path (port the ratatui `stage_pending_password` into the neutral save prepare),
    so editing a password on an existing entry produces the MODIFY. This finishes the
    M4 password item for both paths.
- **Masking discipline:** the cleartext lives only in `pending_password` and the
  editor; it is never written into `EditField.values`, never rendered, and is masked
  in the LDIF preview (`mask_password_attrs`).

## Error handling & invariants

- Alt+N with no branch selected, or no matching profile → status, no form change.
- Autonumber scan truncated by a server size limit → refuse to allocate (neutral
  logic), status message, field left empty (the user can type a value).
- Password on an unencrypted connection → refusal dialog, nothing staged.
- Create ADD failure (e.g. server rejects objectClass set or duplicate DN) → the
  worker's mapped `WriteError` surfaces via the existing error dialog; the create form
  stays open (still dirty) for correction.
- Borrow discipline: no `RefCell`/`UiState` borrow across `exec_view`/`post`/
  `broadcast`/`new_list`/`child_mut`; editors stage via shared state on events, not in
  construction.
- Facade boundary: neutral `workflows::*` import no UI crate; only `src/tui/**` uses
  `tvision_rs`.

## Testing

**Neutral (headless):**
- `build_create_form`: objectClass field injected with `["top"]+ocs`; MUST/MAY fields
  present after resync; static defaults filled; autonumber requests surfaced.
- `composed_create_dn` for empty + filled RDN.
- `next_free_number(existing, min, max)`: gap-filling, min/max bounds, limit refusal.
- `fold_create_password` (already tested) + the new `prepare_save` password-mods path
  for edit.
- `apply_commit` `StageSecret` sets `pending_password` + render flag.

**Widget / dialog (headless `Context`):**
- `widget_for` routes Password binding → PasswordWidget; PasswordEditor refuses when
  `encrypted=false` (stages nothing) and stages `StageSecret` when encrypted + match.
- profile chooser returns the selected index; objectClass create-mode unchanged.

**Async (headless):**
- AllocFlow: request → Entries correlation → `Filled{attr,value}` / `Limited`.
- write_flow: `submit_create` records a Create intent; WriteOk → `Created{dn}`.

**Live (gated by `EDAPTOR_TEST_LDAP_URI`):**
- Create a **non-password** entry under `ou=people` end-to-end against the demo:
  Alt+N → (fast path / chooser) → fill RDN + autonumbered uidNumber/gidNumber →
  confirm → assert the entry exists; then clean it up (delete) so the demo seed is
  restored, or create under a disposable RDN.
- Password gate: against the plain `ldap://` demo, assert the password editor refuses
  (the connection is unencrypted) — this is the testable password path on the demo.

**Final tmux acceptance (agent-driven PTY):** Alt+N from `ou=people` → create form
with objectClass pre-injected and uidNumber auto-allocating → fill `uid` → Alt-S →
confirm → the new entry appears in the leaf list and loads in edit mode. Password
field shows the refusal on the plain demo. Demo data left clean (delete the test
entry afterward).

## Acceptance criteria

1. From a profile, a new entry is created end-to-end (Alt+N → form → confirm → ADD →
   it appears and reloads in edit mode).
2. objectClass is auto-injected on create and remains editable (the 2a resync runs).
3. Numeric fields with `{next:…}` defaults auto-allocate the next free value via an
   async scan.
4. The password widget edits passwords in both create and edit, masked, returning
   `StageSecret`, and refuses on an unencrypted connection.
5. `make check` green; both facade guards clean; live create test leaves the demo
   seed clean.

## Execution plan structure (for writing-plans)

One spec, three sequenced blocks, each independently testable:
- **A — core create:** FormMode::Create, build_create_form, CREATE command + chooser
  + dispatch + do_create, write_flow create-submit, navigate. (Autonumber fields show
  `‹allocating…›` but stay manual until B; password fields are plain until C.)
- **B — autonumber:** AllocFlow + pump correlation + open_create posting scans.
- **C — password widget:** connection_encrypted, PasswordWidget/PasswordEditor,
  widget_for routing, apply_commit StageSecret, create-fold + edit prepare folding.

Documentation: CHANGES.md entries per block; the mdBook M3 page gets the create-flow
+ password sections when the milestone lands; HANDOVER + SDD ledger as usual.
