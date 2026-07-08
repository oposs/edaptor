# tvision-rs UI Migration — Umbrella Design (2026-06-23)

Replace the ratatui UI (`src/ui/*`, ~10k LOC, 17 files) with a `tvision-rs`-based
UI of **functional parity, using tvision-native idioms**. This is the umbrella
spec: it fixes architecture, module layout, the field-widget plugin contract,
milestone sequence, rollout, and acceptance. Each milestone (M1–M5) then gets its
**own** spec → plan → implement cycle; this document is the map they hang off.

The de-risking spike (`spike/tvision-rs`) is a confirmed **GO** — see the findings
doc (`docs/superpowers/research/2026-06-22-tvision-rs-spike-findings.md`, carried
onto this branch). The original blocker (UTF-8/grapheme panic in the *old*
third-party `turbo-vision` crate) is provably gone in our own `tvision-rs 0.1`
port; three-pane render, DIT→leaf→form navigation, and splitter resize were all
confirmed live against the demo server (2026-06-23).

---

## 1. Goal, scope, non-goals

**Goal.** A `tvision-rs` UI that does everything today's ratatui UI does, built so
the special-field editors (choice, password, picker, membership, objectClass,
multi-value) follow **one coherent plugin contract** rather than the ad-hoc tangle
they are today.

**In scope.** Everything under `src/ui/*` is rewritten against `tvision-rs`.

**Out of scope / non-goals.**
- **No domain-layer changes.** `config`, `form`, `ldap`, `schema`, `samba`,
  `workflows` are untouched — the spike proved they drive cleanly from a separate
  binary. The migration is "rewrite `src/ui` against tvision-rs; everything else
  stays."
- **No new LDAP features**, no config-format changes beyond what the widget
  registry naturally needs.
- **Keybindings are re-derived, not ported 1:1.** tvision-rs ships Menu /
  StatusLine / Dialog buttons / mouse / native scrollbars. We adopt those idioms.
  The behaviour inventory (§9) is a **functional-parity checklist, not a key
  contract** — equivalent function matters, identical keys do not.
- **No external/loadable plugins.** The field-widget plugins are a fixed,
  compile-time set we ship; config selects which applies per attribute. "Plugin"
  means a clean internal extension point, not a dynamically loaded module.

---

## 2. Anti-goal: tech debt NOT to reproduce

The migration is a chance to fix accumulated structure, not transcribe it. The
following current tangles must **not** be carried over:

- **The monolithic `value_editor.rs` (1438 LOC)** mixes choice, objectClass,
  picker, and free-text multi-value editing in one file. Each special field is
  re-implemented inline with no shared contract.
- **Split picker logic** spread across `picker.rs` + `value_editor.rs`.
- **Side effects via global flags.** `objectclass_sync_pending` and
  `pending_password` leak widget commit effects into the app event loop as
  out-of-band booleans. These become explicit, typed return values (§4).
- **Presentation logic scattered** across `view.rs` per widget (`present_summary`,
  bullet-mask, `‹N set›`, auto-gen hints) instead of owned by each widget.

The field-widget plugin contract (§4) is the structural replacement for all of it.

---

## 3. Architecture (productionize the proven spike patterns)

All patterns below were validated in the spike; details and file:line citations
are in the findings doc.

- **Shared state.** `type Shared = Rc<RefCell<UiState>>`, cloned into each pane
  factory closure. `Program::new`'s `init_*` factories accept capturing closures
  (`impl FnOnce(Rect) -> Option<Box<dyn View>>`), so app state reaches the view
  tree with no `thread_local` hack. `UiState` holds: worker handle, `Structure`,
  `SchemaModel`, current branch/leaf DN, the active form model, dirty flags,
  pending request-id correlation, and the DFS branch-DN map for the Outline.

- **Worker → view pumping.** A zero-area `PumpView` arms a recurring
  `Context::set_timer(50ms)` on first event; each `Event::Timer` drains
  `worker.poll()`, correlates responses by id into `UiState`, sets dirty flags,
  and `ctx.broadcast(REFRESH)`. `Event::Timer` is broadcast-class in tvision-rs
  (reaches zero-area children). **No tvision-rs change required.**

- **Borrow discipline (hard rule).** Never hold a `RefCell` borrow across a call
  that can re-enter (`broadcast`, `new_list`, `child_mut`, `set_value`,
  `worker.submit`). Always: collect into locals → drop borrow → call. `REFRESH`
  broadcasts are deferred (queued, delivered next loop pass), but the rule stands.

- **Outline (0.1.1+ behaviour).** `Outline` now **auto-seeds** its scrollbar
  limits and focus on first display/interaction, so no manual seed-on-first-event
  is needed after construction. `tv::ov_update(&mut outline, ctx)` is still
  required **after mutating the tree** (swapping the `root` field, or programmatic
  expand/collapse). Read selection via `Outline::value() -> FieldValue::Int(foc)`,
  consistent with `ListBox` (implemented in 0.1.1; the old `OutlineViewer::ov().foc`
  workaround is no longer needed).

- **Facade boundary (enforced, the migration's core rule).** Only the tvision UI
  module may `use tvision_rs`; only the ratatui UI module may `use ratatui` /
  `use tui_*`. The domain layer stays UI-framework-agnostic. During the transition
  the new UI lives at **`src/tui/`** and the old ratatui UI stays at `src/ui/` (see
  §7); CI guard:
  `! grep -rl "use tvision_rs" src | grep -vE "^src/(tui|bin)/"` and
  `! grep -rl "use ratatui\|use tui_" src | grep -vE "^src/ui/"`.
- **Relocate the neutral form model out of `src/ui/`.** The read-only form model
  is already framework-agnostic but misplaced under `src/ui/` today
  (`FormModel`/`FormField`/`WidgetSpec`/`build_form_model` in `src/ui/form.rs`).
  It moves to **`src/workflows/form_model.rs`** (module `workflows::form_model`) —
  *not* `src/form/`, because `build_form_model` takes `&LdapEntry` and `src/form/`
  forbids importing `ldap::worker` by design. workflows is the orchestration layer
  already allowed to touch ldap+schema, and both current importers (`read_flow.rs`,
  `create.rs`) live there — fixing a real layering violation (domain importing
  `crate::ui`). Both UIs then consume `workflows::form_model` and share *no* UI
  module. The editable `EditForm` pipeline (`src/ui/edit_form.rs`) is relocated
  later, in **M2**, when the tvision UI needs editable fields (only its `TextState`
  editor field + rendering are ratatui-coupled). The label engines
  (`config/label.rs`, `config/tree_label.rs`) are **already** framework-neutral in
  `config/` and need no move.

- **Dependency.** Published `tvision-rs = "0.1"` from crates.io (resolves to
  **0.1.2**, the current release); alias as `tv` (`tv = { package = "tvision-rs" }`).
  We rely on **0.1.1+** APIs (`Outline::value()`, crate-root `Deferred`, Outline
  auto-seed, the `external_state` example pattern). No path/git dependency. If a
  release ever lacks a needed API, the fallback is a git pin on the **upstream**
  `https://github.com/oetiker/tvision-rs`, never a local path; fixes go via a
  separate clone + upstream PR. tvision-rs is edition 2024, edaptor 2021 — fine.

---

## 4. The field-widget plugin contract (foundational)

Every field — plain single-value, plain multi-value, and each special editor —
is a **plugin** implementing one trait. The form core knows nothing about specific
widgets; it talks only to the trait and a registry.

### 4.1 Responsibilities (same for every plugin)

1. **Present** — produce the form value-cell rendering (and any column-2 / tree
   label contribution) from the field's current value(s). Pure, no I/O. Subsumes
   today's `present_summary`, bullet-masking, `‹N set›`, and auto-generate hints.

2. **Activate / edit** — given the field and a narrow capability handle, return an
   `Activation`:
   - `Inline` — plain text edited in place in the form row (grapheme-correct
     `InputLine`).
   - `Modal(Box<dyn FieldEditor>)` — a tvision `Dialog` view that runs the
     interaction and, on commit, yields a `CommitOutcome`.
   - `Immediate(CommitOutcome)` — no UI (e.g. sambaSID auto-generate).

3. **Commit outcome** — a **typed** result the form applies uniformly:
   - `SetValues(Vec<String>)` — normal write back to the field.
   - `StageSecret { attrs, cleartext }` — password staging (does not write the
     field; stages for the save path), replacing the `pending_password` flag.
   - `SetValuesThenResyncSchema(Vec<String>)` — objectClass commit; triggers
     schema-driven field regeneration, replacing the `objectclass_sync_pending`
     flag.
   - `Cancelled`.

### 4.2 Declared capabilities

Each plugin declares what it needs so dispatch injects only that (never the whole
app state):
- `Static` — fixed candidate list (choice).
- `NeedsSchema` — reads `SchemaModel` (objectClass picker; client-side filter).
- `NeedsWorkerSearch` — issues live LDAP searches via the worker (picker,
  membership).

### 4.3 Registry & dispatch

A single registry maps `config::widget::WidgetKind`
(`choice` / `password` / `picker` / `membership` / objectClass / sambaSid /
nextNumber / plain) → plugin instance. The form `Group` builds each field row by
asking the registry for the plugin and calling `present()`; Enter calls
`activate()`. Adding a widget = implement the trait + register it; **no form-core
changes**. The form must honour `EditField.widget_binding` from `config::widget`.

### 4.4 Plugin inventory (parity targets)

| Plugin | Activation | Capability | Commit outcome | Notes |
|---|---|---|---|---|
| Plain single-value | `Inline` | `Static` | `SetValues` | grapheme-correct edit |
| Plain multi-value | `Modal` (value editor: add/del/reorder) | `Static` | `SetValues` | ordered vs set semantics |
| Choice | `Modal` (radio/checkbox over `ListBox`) | `Static` | `SetValues` | encoded summary (e.g. Samba flags) |
| Password | `Modal` (new+confirm, masked) | `Static` | `StageSecret` | TLS-gated; never stored inline |
| Picker | `Modal` (search + select dialog) | `NeedsWorkerSearch` | `SetValues` | single or multi per binding |
| Membership | `Modal` (group membership editor) | `NeedsWorkerSearch` | `SetValues` | two-column move idiom |
| ObjectClass | `Modal` (schema-seeded multi-select, client filter) | `NeedsSchema` | `SetValuesThenResyncSchema` | **riskiest**; create-form auto-inject |
| sambaSID | `Immediate` | `NeedsSchema`/compute | `SetValues` | auto-generate, no popup |
| nextNumber | `Immediate` | — | `SetValues` | allocate next id |

---

## 5. Target module layout for the new tvision UI

Built under `src/tui/` during the transition; renamed to `src/ui/` at the M5
cutover when the ratatui tree is deleted (see §7).

```
tui/mod.rs            facade: run() entry, Shared, UiState, REFRESH command
tui/app.rs            Program assembly: desktop, menu bar, status line, pump wiring
tui/state.rs          UiState shape, dirty tracking, selection model, id correlation
tui/pump.rs           PumpView (timer-driven worker drain)
tui/panes/tree.rs     Outline (DIT) — branch nav, tree-label rules, ov_update on tree swap
tui/panes/leaf.rs     ListBox + search box — column-2 label rules, leaf→form trigger
tui/panes/form.rs     Group of per-attribute field rows; plugin present()/activate()
tui/widget/mod.rs     FieldWidget trait, Activation, CommitOutcome, capabilities, registry
tui/widget/{plain,choice,password,picker,membership,objectclass,sambasid,nextnumber}.rs
tui/dialog/{confirm,error,guard,profile_chooser,value_editor}.rs   (tvision Dialogs)
tui/labels.rs         thin adapters over the neutral config/label + config/tree_label engines
tui/startup.rs        config-discovery / config-picker (tvision Dialog before run_app)
```

The form model + field derivation are **not** in this tree — they live in
`workflows::form_model` (relocated in M1) and are consumed by both UIs.

Files stay focused and small; when a module grows past a couple hundred lines it is
a signal it is doing too much (the opposite of today's `value_editor.rs`).

---

## 6. Milestone sequence (each is its own spec → plan → implement)

Risk-first but incrementally usable: front-load the two real unknowns — the
write-path integration spine and the ObjectClass picker — while keeping a working,
demonstrable app at each step.

### M1 — Three-pane read core + widget framework skeleton
Relocate the neutral form model (`FormModel`/derivation) into
`src/workflows/form_model.rs`. Add the
`tvision-rs` dep + `src/tui/` module + a dev binary `src/bin/edaptor-tv.rs` (the
spike's role; ratatui stays the `edaptor` binary). Build Splitter + Outline +
ListBox/search + form `Group`, driven by `ReadFlow` / `FormModel` (not raw
`LdapEntry`). Schema-driven field derivation; tree-label and column-2 label rules
via thin `tui/labels.rs` adapters over the existing neutral engines. Define the
`FieldWidget` trait + registry and implement **`present()`** for read-only display
(plain / multi / secret-mask presenters).
**Accept:** `cargo run --bin edaptor-tv` navigates DIT → select leaf → reads a real
entry; every field renders per schema and profile; labels match config rules. The
`edaptor` (ratatui) binary still builds and runs. Headless view tests for tree/leaf
navigation and presenters.

### M2 — Edit + write spine (walking skeleton)
`activate()` with `Inline` editing for plain single-value fields; `CommitOutcome`
plumbing into `form::{changeset,validate}`. Confirm (LDIF preview) / Error / Guard
(save/discard/stay) dialogs. Save path wired to the worker + post-write refresh.
Dirty tracking + dirty-nav/quit guards.
**Accept:** edit and persist one real entry end-to-end; guard fires on dirty
navigation and quit; LDIF preview correct; validation errors surface. Umlaut /
grapheme edit test lives here (folded from the spike's umlaut test).

### M3 — ObjectClass picker + create flow (riskiest, early)
ObjectClass plugin: schema-seeded multi-select over `ListBox`, client-side
substring filter, `SetValuesThenResyncSchema` driving schema-based field
regeneration. Alt+N profile chooser dialog; single-profile fast path; create-mode
form; objectClass auto-injection on create.
**Accept:** create a new entry from a profile; editing objectClass adds/orphans
fields live; the typed resync outcome (no global flag) works end-to-end.

### M4 — Rich widgets
Choice, Password (TLS-gated, `StageSecret`), Picker, Membership, free-text
multi-value value editor — each a pure `FieldWidget` impl registered into the
registry, no form-core changes. Live LDAP search for picker/membership via the
worker capability + pump.
**Accept:** each widget reaches functional parity with §9; password requires an
encrypted connection; picker/membership search and select against real LDAP.

### M5 — Startup flow + cutover
Config-discovery / config-picker as a tvision `Dialog` before `run_app`. Final
polish, status-line/menu wiring, mouse. **Cutover:** point `main.rs` at the tvision
UI, rename `src/tui/` → `src/ui/`, delete the old ratatui `src/ui` tree, delete the
`src/bin/edaptor-tv.rs` dev binary, and remove the ratatui/tui-* deps
(`ratatui`, `tui-tree-widget`, `tui-prompts`, `crossterm` if unused by tvision —
note tvision pulls its own `crossterm`). Delete `src/bin/spike-tv.rs`,
`tests/spike_tv_umlaut.rs` (umlaut asserts already folded into M2), and the
`spike-tv` Cargo feature on the spike branch if merged.
**Accept:** `make check` green; no ratatui/tui-* deps remain; spike + dev-binary
artifacts gone; docs (README/CHANGES/mdBook) updated to current behaviour.

---

## 7. Rollout

- **Long-lived branch off `main`** — `feat/tvision-ui` (this branch). **Hard swap
  at cutover, transitional source coexistence during the build.** The new UI is
  built under `src/tui/` and run via a dev binary (`edaptor-tv`); the ratatui UI
  stays at `src/ui/` and remains the `edaptor` binary throughout M1–M4. At M5 the
  ratatui tree and the dev binary are deleted in one cutover (§6 M5). No runtime
  dual-UI, no feature flag.
- **Coexistence is proven and clean, not theoretical.** The spike already compiled
  ratatui + tvision-rs together in this crate and ran live. They share no global
  state; the only shared resource is the terminal, owned by one UI per process
  (separate binaries, never concurrent). Both resolve to the **same `crossterm
  0.29`** — a single backend copy in the tree, no duplication.
- The umbrella spec + each milestone spec/plan live on this branch. The findings
  doc and handover are carried here so they survive independent of the spike
  branch (resolves handover open-gap #1).
- `main` is itself unpushed (origin behind at v0.4.0); pushing, CI, docs deploy,
  and release are a separate concern, not gated by this migration.

---

## 8. Testing & risk

- **Headless view tests (spike Path A).** Build a `Context` directly
  (`tvision_rs::view::Context::new(&mut out, &mut timers, 0, &mut deferred)` with
  `TimerQueue::new()` and crate-root `tvision_rs::Deferred`) to unit-test widget
  key handling without a TTY. A standalone `InputLine` needs
  `il.state.state.selected = true`. See upstream `examples/external_state.rs` for
  the shared-state/pump pattern. The umlaut/grapheme regression becomes a permanent
  form-field test.
- **Live tests** gated by `EDAPTOR_TEST_LDAP_URI` (skip when unset); demo base
  `dc=example,dc=org`, `EDAPTOR_TEST_ADMIN_PW=adminpassword`, podman demo server.
- **Interactive confirmation** per milestone needs a human at a terminal — agent
  sessions have no TTY (`CrosstermBackend::new()` → ENXIO).
- **Top risk: ObjectClass picker** (multi-select + schema field-sync). Mitigated by
  the typed `SetValuesThenResyncSchema` outcome and M3 placement (early).
- **Second risk: tvision API gaps.** Surface early (M1–M3); fallback is the
  upstream git pin + separate-clone PR. Known minor gaps feed the upstream
  improvement side-stream (§10), not blockers.
- **Execution style.** Subagent-driven (fresh subagent per task + spec-then-quality
  review). **One editor per working tree at a time** — never run a side agent on
  the same tree concurrently (caused a commit-bundling collision during the spike).
- **Discipline.** Strict TDD; atomic commits; crate compiles after every commit;
  `cargo fmt` + clippy `--all-targets -D warnings` clean before declaring done.
  Cap parallelism at 4 cores. Commit trailer
  `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.

---

## 9. Functional-parity checklist (what the migrated UI must do)

Derived from the current ratatui UI. This is a **functional** checklist; keys may
change to tvision idioms.

**Shell.** Three-pane layout (DIT tree / leaf list+search / form). Quit (with
dirty-guard). Focus switching between panes (with dirty-guard on leaving a dirty
form). Refresh structure. Status line showing read-only tag, transient status,
focused-pane hints, current DN + dirty marker. Read-only mode disables edit/new/
delete/save paths.

**DIT tree pane.** Up/down navigation; expand/collapse/toggle; selection drives the
leaf list. Config-driven tree labels (`[config.tree]` rules; first matching OC rule
wins; structural RDN fallback).

**Leaf list pane.** Always-visible incremental search box (matches the rendered
label). Up/down + page navigation; selection drives the form load. First row is the
branch entry itself (`‹self›`). Config-driven column-2 labels
(`config.profiles[].label`; first matching OC rule wins; structural fallback). New
entry (Alt+N) and delete entry in writable mode.

**Form pane.** One row per field: label (MUST marker, orphaned styling) + value
cell. Field navigation (up/down + page). Activate field to edit (inline / modal /
immediate per plugin). Multi-value, choice summary, masked secret, auto-fill hints.
Dirty detection vs a load-time baseline (set-wise for unordered multi-value). Save
/ cancel-revert. Create-mode vs edit-mode titles.

**Widgets** (per §4.4): choice (radio/checkbox, encoded summary), password (new +
confirm, masked, TLS-gated, staged), picker (searchable single/multi, selected-first
ordering, 100-result cap hint), membership (group membership editor), objectClass
(schema-seeded multi-select, client filter, field resync), free-text multi-value
(add/del/reorder, ordered vs set), sambaSID & nextNumber (immediate auto-generate).

**Dialogs.** Confirm (LDIF preview for save/create; DN for delete). Error
(dismissible). Guard (save/discard/stay; intent = nav/focus/quit; quit defers until
write completes). Profile chooser (Alt+N when multiple profiles match; single-profile
fast path). Value editor (multi-value) and picker/membership editors per §4.4.

**Startup.** Config-discovery / config-picker when multiple config candidates are
found (select, or cancel to exit), before the main TUI.

---

## 10. Upstream tvision-rs improvements (mostly already landed)

The migration is the best driver tvision-rs gets — edaptor is its first
real-application consumer, so every rough edge is a contribution opportunity. We
**improve tvision-rs as we go** rather than working around gaps silently.

**Status: the spike's findings have already been addressed upstream and released
in 0.1.1 / 0.1.2.** Confirmed against the published 0.1.2 source (2026-06-24):

| # | Kind | Improvement | Status |
|---|---|---|---|
| U1 | behaviour | `Outline` auto-seeds scrollbar/focus on first display; no manual seed needed after construction (`ov_update` still required after a tree mutation) | ✅ **0.1.1** |
| U2 | example | `examples/external_state.rs` — the canonical `Rc<RefCell<T>>` + `broadcast` + timer-pump (`PumpView`) recipe | ✅ **0.1.1** |
| U4 | code | `tvision_rs::Deferred` re-exported at the crate root | ✅ **0.1.1** |
| U5 | code | `Outline::value() -> Some(FieldValue::Int(foc))`, parity with `ListBox` | ✅ **0.1.1** |
| U3 | docs | Note that a standalone `InputLine` needs `state.state.selected = true` for headless tests | ⚠️ still a doc gap (trivial; we set it in test setup) |
| U6 | code | `Outline::set_root(root, ctx)` convenience wrapper (mirrors `ListBox::new_list`) | ❌ still absent (we use `outline.root = …; ov_update(ctx)`) |

So the migration **adopts 0.1.2 directly** — the seed-on-first-event,
`ov().foc`, and two-segment `view::Deferred` workarounds the spike used are all
obsolete and must NOT be carried into the new code.

**Remaining contribution candidates (small, opportunistic, non-blocking):**
- **U6** — an `Outline::set_root(root, ctx)` wrapper, if M1/M3 tree-swap code makes
  the field-write+`ov_update` idiom feel rough. Otherwise leave it.
- **U3** — a one-line doc note for headless `InputLine`.
- Any **new** gap a milestone uncovers.

**Directive when we do contribute.** Work in a **separate clone** of upstream
`https://github.com/oetiker/tvision-rs`, one focused **PR per change**; upstream
stays unmodified by edaptor, never a local fork. edaptor keeps depending on the
**published** crate; a git pin is the only fallback, and only if a release lacks an
API a milestone strictly needs. Never block a milestone on an upstream
merge/release — keep the documented workaround and adopt the improvement once it
ships.

## 11. Open items deferred to milestone specs

- Exact `UiState` field set and the `ReadFlow`/`FormModel` ⇄ form-row mapping (M1).
- The precise `FieldWidget`/`FieldEditor` trait signatures and capability handle
  type (M1/M2 — settle on first real implementations, refine in M3/M4).
- tvision `Dialog` modal-result plumbing (`Program::exec_view` vs broadcast) (M2).
- Mouse and native-scrollbar adoption per pane (M5 polish).
