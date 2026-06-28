# tvision M4 — Rich widgets (2026-06-28)

The final widget milestone of the tvision-rs UI migration (umbrella
[`2026-06-23-tvision-ui-migration-umbrella-design.md`](2026-06-23-tvision-ui-migration-umbrella-design.md)
§6 M4). M1–M3 are complete and live-accepted; the password widget (an M4 item) was
already pulled forward into M3 Phase 2b. This spec covers the **remaining rich
widgets** as one combined milestone:

> free-text multi-value editor · choice · picker · membership (two-column) ·
> sambaSID immediate · samba-context wiring · X-ORDERED `ordered` side-effect.

It is a single spec → plan → implement cycle (user-chosen over phasing).

---

## 1. Framing & invariants

**Everything is tvision-side UI plus one async flow plus porting two neutral
helpers.** The config model is already complete and the encode/diff logic is
already neutral domain code we just call:

- `config::widget::WidgetKind` already has `Choice`, `Password`, `Picker`,
  `Membership`, `ObjectClassPicker`, `SambaSid`, `NextNumber`, `Readonly`,
  `XOrdered`. **No config-format change.**
- Choice encode/decode is pure & neutral in `config::widget`
  (`parse` / `seed_checked` / `commit_value` / `present_summary`) — lossless
  seed-from-original merge. We **call** it; we do not reimplement it.
- Membership fan-out diff is pure & neutral in `workflows::save`
  (`membership_fanout`, `would_empty`). We **call** them.

**Hard invariants (carried from the umbrella + M3):**

- **No form-core changes.** Every widget plugs into the existing M3 seam:
  `widget_for(field)` routing → `Activation::Modal(Box<dyn FieldEditor>)` (or
  `Immediate`) → editor stages a typed `CommitOutcome` into
  `UiState.staged_commit` → `app::dispatch` ACTIVATE applies it via `apply_commit`
  on the modal's `OK` return. Adding a widget = implement the trait + register it.
- **Facade boundary.** Only `src/tui/**` + `src/bin/edaptor-tv.rs` may
  `use tvision_rs`; the domain layer (incl. the new `workflows::search_flow` and
  the ported combined-save) imports neither UI framework.
- **Don't touch `src/ui/**` (ratatui).** Neutral logic is introduced fresh in
  `workflows::*` as parity copies (the `edit_form` precedent); dedup at M5.
- **Borrow discipline.** Never hold a `UiState`/`RefCell` borrow across
  `broadcast`/`post`/`exec_view`/`worker.submit`/`new_list`/`child_mut`/`set_value`.
  Stage modal state in `reset_current` / on events, **not** in `new()`/`into_view`
  (the construction-time borrow trap — `dispatch` holds `state.borrow()` to pass
  the schema in). Two real panics in M3 came from this.

---

## 2. Build order (user-chosen: static-first, risk last)

A usable, demoable app at every step; the riskiest piece (membership multi-entry
write) lands last on proven infrastructure.

1. **Free-text multi-value editor** + the **X-ORDERED `ordered` side-effect**.
2. **Choice** widget.
3. **sambaSID** immediate widget + **samba-context wiring** into `UiState`.
4. **`SearchFlow`** (shared async LDAP search) + **picker** widget.
5. **Membership** two-column mover + **neutral combined-save port** + **multi-entry
   write** in `write_flow` + **combined LDIF preview** in the confirm dialog.

Each step is independently testable and leaves the tree green.

---

## 3. Shared infrastructure (steps 4–5 depend on this)

### 3.1 `workflows::search_flow::SearchFlow` (async LDAP search)

Mirrors `workflows::alloc_flow::AllocFlow` exactly in shape:

- A dedicated flow with a **disjoint request-id range** (pick a range not used by
  `read_flow` / `write_flow` / `alloc_flow`).
- **Debounce by id, not timer:** each keystroke submits a fresh `Request::Search`;
  responses for superseded ids are discarded (proven ratatui behaviour, no timer
  needed). `SearchFlow` tracks the latest submitted id.
- **Filter** built from the candidate scope (profile name or inline scope table on
  the binding): empty term → `(objectClass=<oc>)`; with term →
  `(&(objectClass=<oc>)(|(cn=*term*)(uid=*term*)))`, **RFC-4515 escaped**.
- **Cap** at 100 candidates (`PICKER_SEARCH_CAP` parity).
- **Attrs requested:** label-template attrs + the scalar store attr + `cn` fallback.
- Correlated in `tui::state::pump_worker`; applied via a new
  `UiState::apply_search_results` (borrow-safe: collect → drop borrow → mutate).

`SearchFlow` is **pure plumbing**: `prepare` (build the request) and `on_response`
(parse entries → candidate rows) are pure; `submit` is a thin worker wrapper.

### 3.2 `workflows::pick_state` (neutral selection state)

A **fresh parity copy** of the pure logic in `src/ui/picker.rs` (the ratatui file
stays untouched; dedup at M5):

- `selected` / `results` / `saved` sets and **selected-first ordering** (selected
  lead when no search; matches lead when searching; a selected entry that no longer
  matches stays visible; saved-but-removed synthesized at the end with a marker).
- **Key comparison:** DN store → case-insensitive; scalar store → exact.
- Single vs multi cardinality (`config::widget` `select`, `auto` from schema arity).

Consumed by both the picker and the membership editors.

### 3.3 Neutral combined-save (port from `src/ui/app/save.rs`)

Port `plan_combined_save` into `workflows::save` as a neutral function returning a
`CombinedSave { own_mods, fanout: Vec<(group_dn, ModOp)> }`:

- Own-entry diff with the **back-ref label stripped from BOTH sides** (the user's
  `memberOf` is overlay-maintained and never written).
- Per-holder fan-out via the existing `membership_fanout(entry_dn, baseline,
  selected, holder_attr)`.
- **v1 simplification (parity):** reject rename combined with a membership change in
  one save (`src/ui/app/save.rs` §6.3). Surfaces as a clear error.
- Reuses `prepare_save` for the own-entry leg.

### 3.4 Multi-entry write in `workflows::write_flow` (TOP RISK)

Extend `WriteFlow` to handle a combined save:

- Submit the own-entry `Request::Modify` (if any own mods) **plus** one
  `Request::Modify` per touched group; **track all outstanding ids** (today it
  correlates one). A new `WriteOutcome` variant (or an extended `Saved`) reports
  "all legs landed" / partial failure.
- **Pre-validate the last-member rule** (`would_empty`) for every removal **before**
  submitting any leg; abort the whole batch with a clear error if any group would be
  emptied (groupOfNames requires ≥1 member). Matches `apply_combined_save`.
- On completion, re-read the current entry (and, for correctness of the leaf/tree
  labels, the structure is unaffected — only `member` on groups changed; a current-
  entry re-read suffices, matching ratatui).
- `prepare`/`on_response` stay pure; `submit` remains the thin worker wrapper.

This is the one genuinely new orchestration in M4 and the primary risk. It is built
**last**, on the proven SearchFlow + pick_state + neutral combined-save.

---

## 4. The widgets

Each is a `FieldWidget`/`FieldEditor` impl under `src/tui/widget/` + (where modal) a
`src/tui/dialog/` view, registered by extending `widget_for`. **No form-core change.**

### 4.1 Free-text multi-value editor (`tui/widget/multivalue.rs`)

Unblocks editing **all** multi-valued attributes (today only single-valued attrs are
inline-editable — the biggest parity gap).

- **Dialog:** a `ListBox` of current values + an inline `InputLine` to edit the
  selected row + buttons/keys: Add, Delete, Move-Up (Alt-Up), Move-Down (Alt-Down).
- **Commit:** trim each row, drop empties → `SetValues`.
- **Ordered vs set:** honours `field.ordered`. The editor itself is order-aware
  (reorder is meaningful); set-wise vs order-sensitive **dirty** detection is already
  in `workflows::edit_form` (`value_set_eq` vs ordered compare) and consumes
  `field.ordered`.

### 4.2 X-ORDERED `ordered` side-effect (with 4.1)

`workflows::widget_bind::apply_widget_bindings` already sets
`widget_binding = Some(XOrdered)` but **not** `field.ordered = true`. Set it, so the
multi-value editor + dirty detection treat X-ORDERED attrs as order-sensitive. (The
`{n}` prefix strip/reconstruct on save is **out of scope** — it is incomplete in
ratatui too and belongs to a later save-path task; M4 only wires the `ordered` flag.)

### 4.3 Choice (`tui/widget/choice.rs`)

- **Dialog:** a `ListBox` with radio markers `(•)/( )` (single) or checkbox `[x]/[ ]`
  (multi), seeded by `ChoiceWidget::seed_checked(current)`.
- **Toggle:** Space (radio replaces; checkbox toggles). Commit → `commit_value`
  (lossless merge preserving unlisted tokens, canonical re-serialize) → `SetValues`.
- **Present** (read-only cell): `present_summary` (joined option labels, or raw value
  when off-list).
- Covers Plain (token *is* value, e.g. `loginShell`) and Bracketed (Samba
  `[DU         ]` flag encoding) purely via the existing `config::widget` functions.

### 4.4 Picker (`tui/widget/picker.rs`)

- **Dialog:** a searchable list (search `InputLine` on top + `ListBox`), single =
  radio (Enter picks, replaces), multi = checkbox (toggle). Selected-first ordering.
  100-result cap hint when truncated.
- **Data:** `SearchFlow` (3.1) + `pick_state` (3.2).
- **Commit:** the picked `store_value`(s) (DN by default, or the configured scalar
  attr) → `SetValues`. Single → one value; multi → the set.

### 4.5 Membership — two-column mover (`tui/widget/membership.rs`)

User-chosen **genuine two-column** idiom (no ratatui parity to copy — designed
fresh):

- **Layout:** left **Available** column (a `ListBox` fed by the live `SearchFlow`,
  with a search `InputLine` above it) ‖ right **Members** column (a `ListBox` of the
  staged DN set, seeded from the field's baseline `memberOf`).
- **Move:** Enter or → moves the highlighted Available row into Members (de-dupe by
  DN, case-insensitive); Del or ← removes the highlighted Members row. A row already
  in Members is marked in the Available column.
- **Commit:** the Members DN set → `SetValues` into the field. **Saving** is the
  combined multi-entry write (3.3 + 3.4): the diff vs baseline becomes per-group
  `member` add/delete MODIFYs; the user's own `memberOf` is never written.
- **Confirm:** the existing Confirm dialog renders the **combined LDIF preview** (own
  entry, if any own mods, + each touched group) when fan-out is present
  (user-chosen — extend the one dialog, parity with `combined_save_overlay`).

### 4.6 sambaSID immediate (`tui/widget/sambasid.rs`)

- **Activation: `Immediate`** (no popup). Reads the sibling `uidNumber` from
  `UiState.edit_form` + the samba domain context (4.7); computes
  `domain_sid-(uid*2+rid_base)` via `samba::sid::user_sid`; returns
  `Immediate(SetValues)`.
- **Errors:** missing/non-numeric `uidNumber` or no configured domain SID → error
  dialog explaining what to fix; the field stays manually inline-editable (parity).

### 4.7 Samba-context wiring

`UiState` gains the samba domain info (`domain_sid`, `algorithmic_rid_base`) from
`Config`. Flip the hard-`false` `samba_enabled` at the two `WidgetResolver::new`
sites (M3 deferred this). Needed by 4.6 and by Bracketed-choice Samba flags.

---

## 5. Module layout (additions only)

```
workflows/search_flow.rs     async LDAP search (mirrors alloc_flow); disjoint id range
workflows/pick_state.rs      neutral selection state (parity copy of ui::picker)
workflows/save.rs            + neutral plan_combined_save → CombinedSave{own_mods,fanout}
workflows/write_flow.rs      + multi-entry submit/correlate + last-member pre-validate
workflows/widget_bind.rs     + set field.ordered=true for XOrdered
tui/state.rs                 + samba ctx; apply_search_results; combined-write tracking
tui/widget/multivalue.rs     free-text multi-value editor
tui/widget/choice.rs         radio/checkbox over config::widget
tui/widget/picker.rs         searchable single/multi picker
tui/widget/membership.rs     two-column mover (fan-out)
tui/widget/sambasid.rs       immediate auto-generate
tui/dialog/*                 multivalue / choice / picker / two-column-membership dialogs
tui/widget.rs (widget_for)   + routing for choice/picker/membership/sambasid/xordered
```

Files stay small and focused (the opposite of the old `value_editor.rs` monolith).

---

## 6. Testing & acceptance

- **Headless view tests** per widget: key handling + `present` rendering, built on a
  hand-constructed `Context` (the M1–M3 pattern). Choice (radio replace, checkbox
  merge-lossless, Bracketed Samba), multi-value (add/del/reorder, empty-then-nav no
  panic, ordered dirty), picker (selected-first, single radio vs multi toggle),
  membership (move both directions, de-dupe), sambaSID (compute + error paths).
- **Neutral unit tests:** `plan_combined_save` (back-ref stripped both sides;
  rename+membership rejected); `would_empty` / `membership_fanout` already covered;
  `apply_search_results` stale-id discard.
- **Gated live tests** (`EDAPTOR_TEST_LDAP_URI`, demo server): picker/membership
  live search returns + caps; **membership fan-out write** — create a temp user,
  add/remove a group, assert each group's `member` changed and the user's `memberOf`
  was not written; last-member abort; clean up.
- **tmux acceptance** (the handover's PTY recipe): drive each widget live; verify the
  combined LDIF preview for a membership save; restore demo data.
- **Gate:** `make check` green (fmt + clippy `-D warnings` + tests); both facade
  guards print nothing; CHANGES.md + mdBook widget pages updated.

## 7. Risks (in order)

1. **Multi-entry membership write** (3.4) — multi-id correlation, last-member abort
   *before* any submit, combined preview, partial-failure reporting. Built last on
   proven infra; the neutral diff primitives already exist and are tested.
2. **Two-column mover** (4.5) — net-new UI with no parity to copy; live search feeds
   only the Available column, Members is the staged set. Settle focus/move keys on
   the first real build.
3. **Async search-result application** — borrow discipline in `apply_search_results`
   (collect → drop borrow → mutate); `reset_current` for modal list seeding; the
   construction-time borrow trap (no `borrow_mut` in `into_view`).
4. **sambaSID sibling read** — `activate()` must read `uidNumber` from
   `UiState.edit_form` at activation time; ensure the capability handle exposes it.

## 8. Out of scope

- X-ORDERED `{n}` prefix strip/reconstruct on save (incomplete in ratatui too;
  later save-path task). M4 only wires the `ordered` flag (4.2). **Consequence
  (confirmed in the M4 final review):** because the `{n}` handling is deferred,
  M4 deliberately does **not** add an `XOrdered` arm to `widget_for`/`is_modal_field`,
  so X-ORDERED multi-valued fields stay **read-only** in the tvision UI (routing
  to `PlainWidget`). The `field.ordered` flag (4.2) and the multivalue editor's
  order-awareness are forward-prep for when `{n}` handling lands. **M5 cutover must
  reconcile this**: either implement X-ORDERED editing (`{n}` strip/reconstruct +
  the routing arm) or update `docs/src/configuration/widgets.md` (which currently
  documents X-ORDERED editing for the shipping ratatui UI).
- Dedup of the `pick_state` / `edit_form` parity copies (M5 cutover).
- Startup config-discovery dialog, mouse polish (M5).
- Any new LDAP feature or domain-layer change.
