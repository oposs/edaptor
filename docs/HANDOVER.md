# edaptor — Session Handover

Carries the **current concerns** into the next session. Not a project history —
for that see git log, the specs under `docs/superpowers/specs/`, the SDD ledger
(`.superpowers/sdd/progress.md`), and project memory (`…/memory/MEMORY.md`).

**Date:** 2026-06-29 · **Where we are: the tvision-rs UI migration is COMPLETE
through M5b.** M1–M5b are DONE and committed on `feat/tvision-ui`. edaptor
depends on released `tvision-rs` 0.3.0 (no pin); the main window runs frameless
full-screen (`Fullscreen::Desktop`). **M5b (the cutover) is DONE** — the ratatui
UI (`src/ui/`, ~9.4k LOC), the `edaptor-tv` dev binary, and the
`ratatui`/`tui-tree-widget`/`tui-prompts`/`crossterm` deps are deleted; `src/tui/`
was renamed to `src/ui/`; `crate::tui` → `crate::ui`. The `edaptor` binary now
runs the tvision UI end-to-end. Gate green: 499 lib tests, clippy `-D warnings` +
`fmt --check` clean, both facade guards empty, gated live tests pass.

> **▶ NEXT ACTION — M5c (the three reconciliations; own brainstorm → plan →
> implement cycle):**
>
> **(1) X-ORDERED editing** — the `{n}` diff/save plumbing ALREADY exists in the
> neutral layer (`form::changeset::diff` takes an `x_ordered_attrs` set;
> `write_flow.rs:145` builds + passes it), so M5c is the tvision **display** side
> only: a `widget_for` `XOrdered` arm → an ordered multivalue editor that strips
> `{n}` on display and reconstructs it from row order on commit.
>
> **(2) Schema-aware last-member pre-validation** — block last-member removal ONLY
> when the membership attr is MUST (`groupOfNames`/`groupOfUniqueNames`);
> `posixGroup.memberUid` is MAY so an empty posixGroup is LEGAL and must NOT be
> blocked. Needs a live group-member fetch before the confirm dialog (today
> `submit_combined` gets an EMPTY map → never blocks; server enforces).
>
> **(3) Live `sambaDomain` LDAP discovery** — port the former ratatui
> `discover_samba_domain` logic into `ui::state::bootstrap`
> (`src/ui/state/bootstrap.rs` — `src/ui/` is now the tvision tree since M5b):
> search `(objectClass=sambaDomain)`, parse `sambaSID` via the existing
> `samba::sid::parse_samba_domain`.
>
> M5b's spec §4 deliberately left `widgets.md`'s X-ORDERED-editable claim as-is;
> M5c makes it true again.
>
> _M4 load-bearing facts / divergences (for M5c):_
> - **`Activation::Immediate` was NOT added** (the M4 spec/plan proposed it for
>   sambaSID). It would be never-constructed `dead_code` (the widget's
>   `activate(&field)` can't see the sibling `uidNumber`/samba ctx), failing clippy
>   `-D warnings`. sambaSID is a **dispatch special-case** in `app.rs` ACTIVATE
>   calling neutral `workflows::samba_compute::samba_sid_for_form`. `Activation`
>   stays `{Inline, Modal}`.
> - **M4 dialog key convention:** the search-as-you-type pickers free **Space**
>   (types into the search box) and **Enter** (confirms OK); the list action is
>   **Insert** (picker toggle; membership move-in, also `→`). Choice toggles on
>   Space (no search box). Keep this consistent for any M5 dialogs.
> - **Former parity copies** (all deduped at M5b cutover — the ratatui tree is gone):
>   `workflows::pick_state` is now the sole picker-state implementation;
>   `workflows::save::plan_combined_save` and `workflows::edit_form` are the sole
>   save/form models. The `write_flow` copy is likewise the only version.
> - **Combined membership save caller contract (LOCKED, doc-comment on
>   `plan_combined_save` + test `prepare_combined_no_pending_password_keeps_baseline_hash`):**
>   callers MUST pass the password primary+derived in `mask_attrs`
>   **unconditionally**, else a baseline password hash diffs to a spurious Delete.
>   `prepare_combined` (write_flow) honors this.
> - **Last-member pre-validation is best-effort in M4:** `submit_combined` is
>   passed an EMPTY `group_members` map from the dispatch (no async group-member
>   fetch), so `would_empty` never blocks; the **LDAP server enforces** the
>   `groupOfNames` ≥1 rule (surfaced as `WriteOutcome::Error`). Full client-side
>   pre-validation is an M5 item.
> - **`SearchFlow`** (`workflows/search_flow.rs`) id range **3_000_000+**
>   (disjoint from Read=1, Write=1M, Alloc=2M); latest-id debounce; reuses
>   `pick_state::build_member_filter`. `UiState::submit_search` does the disjoint
>   `worker`+`search_flow` borrow; results land in `UiState.search_results` and
>   the dialog rebuilds on the `REFRESH` broadcast.
>
> _M3 load-bearing facts (so the next session doesn't rediscover them):_
> - **`reset_current` is THE modal-open init hook** in tvision-rs 0.3.0
>   (`program.rs:1710`, runs before first draw/event), NOT `on_bounds_changed`
>   (dead for modal inserts — `Group::insert` calls `set_bounds` directly). Any M4
>   modal widget that must seed a list / stage state on open uses `reset_current`.
> - **Borrow trap (the pattern recurs):** a `FieldEditor` must NOT `borrow_mut()`
>   the shared state during construction/`into_view`, because `dispatch` holds
>   `state.borrow()` to pass the schema in. Stage in `reset_current` / on events
>   (borrow-safe), not in `new()`. (Two real panics this milestone came from this.)
> - **The modal seam M4 reuses:** `widget_for(field)` routes objectClass→picker,
>   `widget_binding==Password`→PasswordWidget, else Plain — extend it for
>   choice/picker/membership. `is_modal_field` makes a field focusable + edit-key-
>   swallowing in the form pane (so it's read-only-but-activatable). The editor
>   stages a typed `CommitOutcome` live into `UiState.staged_commit`; `app::dispatch`
>   ACTIVATE applies it on the modal's `OK` return (`take()` on OK, `=None` on CANCEL).
> - **Async data flows** (picker/membership live search) mirror `AllocFlow`
>   (`workflows/alloc_flow.rs`): a dedicated flow with a disjoint id range, posted by
>   the controller, correlated in `pump_worker`, applied via an `apply_*` method.
> - **Password masking:** tvision `InputLine` has NO built-in masking; the editor
>   owns cleartext buffers + renders bullets in disabled cells. The staged sentinel
>   is `write_flow::PW_SENTINEL` (•••••• ); it is stripped before submit in BOTH
>   create (do_create) and edit (WriteFlow::prepare) and must NEVER reach the server.
> - The objectClass **field's values are authoritative** for `sync_schema_fields`;
>   `EditForm.object_classes` is a mirror kept by `apply_commit` for the save path.
> - **RESOLVED in M4:** `UiState.samba_domain` is now threaded from config and both
>   `WidgetResolver::new` sites pass `samba_domain.is_some()` for `samba_enabled`
>   (live `sambaDomain` LDAP discovery is still deferred to M5).
> - **X-ORDERED — M5 reconciliation item (final-review finding #6):**
>   `apply_widget_bindings` sets `field.ordered=true` for `XOrdered`, but `widget_for`
>   has **no `XOrdered` arm**, so X-ORDERED multi-valued fields route to `PlainWidget`
>   and are **read-only** in tvision (deliberate — editing them needs the deferred
>   `{n}` strip/reconstruct, else data corruption). The `ordered` flag + multivalue
>   order-awareness are dead forward-prep until then. **Resolution (decided):** M5b
>   (cutover) deliberately leaves X-ORDERED read-only AND leaves `widgets.md`'s
>   editable claim as-is; **M5c implements X-ORDERED editing** (the display-side
>   `{n}` strip/reconstruct + the `widget_for` arm), which makes `widgets.md` true
>   again. See the banner's AFTER-M5b section. (The neutral `{n}` diff/save
>   plumbing already exists — `form::changeset::diff` + `write_flow.rs:145`.)
>
> _Closed P1 items, for the record:_
> 1. ✅ **RESOLVED — bottom `░` strip (`bc64274`).** Root cause (verified vs
>    tvision-rs 0.3.0 source): tvision refits nested views via `grow_mode`
>    (`calc_bounds`) only — it calls `on_bounds_changed` solely on the window's
>    direct body, NEVER on splitter-nested panes. The tree filled because `Outline`
>    self-sets `grow_mode {hi_x,hi_y}`; the leaf `ListBox`/search and form
>    header/`ScrollGroup` set none → didn't track the pane → strip. The earlier
>    `FormPane`/`LeafPane::on_bounds_changed` (M3 P1 Tasks 6–7) were the wrong
>    mechanism — DEAD CODE for nested panes. Fix: set `grow_mode` on the pane
>    children (search/header `hi_x`; list/ScrollGroup `hi_x+hi_y`), delete the dead
>    overrides, and move `ScrollGroup`'s resize-recompute into a `change_bounds`
>    override (the hook the framework actually calls). NOT a tvision bug.
>    Live-verified: no strip at launch or after shrink/grow resizes. Minor residual
>    (cosmetic, logged): `ScrollGroup` content-cell *widths* only re-fit on a DN
>    change, so a width-only resize leaves value cells their old width until you
>    pick another entry.
> 2. ✅ **RESOLVED — interactive guard-edge sign-off (agent-driven tmux PTY).** All
>    three deferred flows live-verified; demo data intact afterward (no write ever
>    submitted). **A — keyboard scroll-to-focused:** PASS (viewport scrolls to keep
>    focused editable field visible; pinned DN header stays). **B — guard #2
>    (cancelled-confirm snap-back):** PASS (Save→confirm→cancel leaves form pinned +
>    leaf highlight snaps back to the current entry). **C — guard #3
>    (branch-change-while-dirty):** PASS (Stay reverts the tree highlight to the
>    current branch). Bonus: Alt-X quit-guard→Discard exits clean. **NEW FINDING:** a
>    single left-click on the `Outline` (tree) only FOCUSES the pane — it does NOT
>    move the branch selection, so the branch guard is reached via keyboard arrows,
>    not a click (the leaf pane's `first_click` DOES select). Reliable PTY focus
>    probes (for the next driver): `tmux display-message -p '#{cursor_x}'` locates
>    the focused widget by column; `tmux capture-pane -e` renders the focused element
>    bg bright-green `(0,170,0)`. Still only unit-reasoned (not exercised live):
>    branch Save-Submitted / branch Discard dispatch routing.
>
> Full per-task + review ledger: `.superpowers/sdd/progress.md` (M3 P1 section).
> The original plan + spec (now executed):
> [`plans/2026-06-27-tvision-m3-phase1-stabilize.md`](superpowers/plans/2026-06-27-tvision-m3-phase1-stabilize.md),
> [`specs/2026-06-26-tvision-m3-phase1-stabilize-design.md`](superpowers/specs/2026-06-26-tvision-m3-phase1-stabilize-design.md).

`edaptor` is a Rust TUI for administering an OpenLDAP directory. It introspects
live schema (`cn=subschema`) and generates edit forms from `objectClass`
definitions; a TOML config declares connection settings plus *entry profiles* and
a **widget palette** (`[profile.widget.<attr>]` kinds: `choice` / `password` /
`picker` / `membership`). The UI is **tvision-rs** (`src/ui/`); the `edaptor`
binary is the sole binary. (The ratatui UI and `edaptor-tv` dev binary were
removed at the M5b cutover.)

---

## Git topology (read before you branch)

- **Branch `feat/tvision-ui`** (HEAD: M5b cutover docs, closing the milestone) —
  the long-lived migration branch. ALL tvision work lives here. M5b cutover is
  DONE; the branch is ready to merge to `main` after M5c (or earlier if the owner
  decides).
- `main` is behind/unpushed (origin at v0.4.0). Pushing / CI / release are a
  separate concern, not gated by M5c.
- `Cargo.toml` version is `0.4.0`. Dependency: **`tvision-rs = "0.3"`** from
  crates.io (a plain release dep — no git pin, no patch; the `exec_view_focused`
  pin was dropped once 0.3.0 shipped).

The migration is governed by the **umbrella design**:
[`docs/superpowers/specs/2026-06-23-tvision-ui-migration-umbrella-design.md`](superpowers/specs/2026-06-23-tvision-ui-migration-umbrella-design.md)
— read §6 (milestone sequence) and §4 (the FieldWidget plugin contract) first.
Each milestone gets its own spec → plan → implement cycle.

---

## What's done

**M5b — tvision cutover (DONE; closes the migration)** (spec
`specs/2026-06-28-tvision-m5b-cutover-design.md`, plan
`plans/2026-06-28-tvision-m5b-cutover.md`). 4 staged tasks: (1) rewired `main.rs`
to the tvision UI; (2) deleted the ratatui tree (`src/ui/` ~9.4k LOC), `edaptor-tv`
bin, and `ratatui`/`tui-tree-widget`/`tui-prompts`/`crossterm` deps; (3) renamed
`src/tui/` → `src/ui/` + `crate::tui` → `crate::ui`; (4) updated facade guards,
docs, and mdBook. Gate green: 499 lib tests, clippy `-D warnings` + `fmt --check`
clean, both single-UI facade guards empty, gated live tests (`tv_membership`,
`tv_picker`) pass.

**M5a — startup flow (DONE @ `3344b77`)** (spec
`specs/2026-06-28-tvision-m5a-startup-flow-design.md`, plan
`plans/2026-06-28-tvision-m5a-startup-flow.md`). 4 subagent-driven TDD tasks, each
reviewed clean + a whole-branch review (verdict READY). Replaced the ratatui
config-picker with a tvision one and wired the pre-TUI config-path resolution.
Delivered: `src/ui/dialog/config_picker.rs` (ListBox + detail pane),
`src/ui/startup.rs` (`SHOW_PICKER` + one-shot `PickerTrigger`; `run_config_picker`
short-lived Program; pure `decide_config_path` + `pub resolve_config_path`).
Gate green: 644 lib tests, clippy `-D warnings` + `fmt --check` clean, facade guards
clean; live tmux acceptance (two-config picker nav/detail/Enter/Esc; single-config +
`--config` skip).

**M4 — rich widgets (DONE @ `a08008d`; final review READY)** (spec
`specs/2026-06-28-tvision-m4-rich-widgets-design.md`, plan
`plans/2026-06-28-tvision-m4-rich-widgets.md`). 19 subagent-driven TDD tasks, 5
parts, each reviewed clean (5 fix passes) + a whole-milestone review (verdict
READY). Delivered, all via the M3 `widget_for`→`Activation::Modal`→`staged_commit`
seam with NO form-core changes:
- `src/ui/multivalue.rs` — free-text multi-value editor (add/del/reorder); the
  X-ORDERED `ordered` flag is set but those fields stay **read-only** (see the
  banner's X-ORDERED M5c item).
- `src/ui/choice.rs` — radio/checkbox over the neutral `config::widget::ChoiceWidget`.
- `src/ui/picker.rs` — live-search picker (single/multi, DN/scalar store); the new
  async `workflows::search_flow::SearchFlow` (id 3M+) + neutral `workflows::pick_state`
  (the sole picker-state implementation since M5b) + `UiState::{submit_search,search_results}`.
- `src/ui/membership.rs` — two-column mover; the multi-entry fan-out write
  (`workflows::save::plan_combined_save` + `WriteFlow::submit_combined`
  + `WriteOutcome::CombinedSaved`); confirm dialog renders the combined LDIF.
- sambaSID immediate auto-gen (dispatch special-case → `workflows::samba_compute`),
  `UiState.samba_domain` wired from config.
Full per-task + review ledger: `.superpowers/sdd/progress.md` (M4 section). Gate
green: 637 lib tests + all gated live tests (`tv_picker`, `tv_membership` round-trip
vs the real demo server), clippy `-D warnings` + `fmt --check` clean, facade guards
clean. **Read the top banner's "M4 load-bearing facts / divergences" before M5c.**

**M1 — three-pane read core** (plan `plans/2026-06-24-tvision-m1-read-core.md`).
DIT `Outline` (tree) | leaf `ListBox`+search | read-only form `Group`, driven by
the unchanged domain layer via `workflows::read_flow`/`form_model`. The
`FieldWidget` trait + registry skeleton (`src/ui/widget.rs`), `present()` only.

**M2 — edit + write spine** (spec `specs/2026-06-25-tvision-m2-edit-write-design.md`,
plan `plans/2026-06-25-tvision-m2-edit-write.md`). 9 tasks, subagent-driven,
reviewed clean (whole-branch review verdict READY). Delivered:
- `workflows::edit_form` — neutral editable model (values + baseline + set-wise
  dirty + `to_edit_entry`). The sole edit-form model (the ratatui copy was removed
  at the M5b cutover).
- `workflows::write_flow` — `WriteFlow::{prepare,submit,on_response,submit_followup}`:
  validate+diff via `workflows::save::prepare_save`, submit to the worker, correlate
  `WriteOk`/`WriteError`; **MODIFY + MODRDN** (incl. rename-then-modify two-step).
  `prepare`/`on_response` are pure; submit is a thin worker wrapper.
- `ui::widget` — `present(&EditField)`, `activate()→Inline`, `inline_editable` gate.
- `ui::state` — `UiState` holds `edit_form`/`write_flow`; `pump_worker` routes
  reads then writes → `PumpResult{changed,quit,error}`.
- `ui::panes::form` — editable pane; `ui::dialog::{confirm,error,guard}` +
  `guard_decision`; `ui::app` — the single `run_app` dispatch closure (the only
  `exec_view` site) wiring Save/Exit, dirty-nav guard, deferred-quit.
- Gated live test `tests/tv_edit_write.rs` (skips unless `EDAPTOR_TEST_LDAP_URI`).

**Post-M2 navigation fixes** (found in the first live tmux acceptance pass — M1's
navigation had never been driven interactively):
- DIT `branch_dns` were built in reversed sibling order → wrong leaves shown. Fixed.
- Leaf pane gated the read on a key the `ListBox` had already consumed → selecting
  a leaf never loaded the form. Now detects the change via `value()`. Fixed.
- Form pane indexed past the 32-cell pool for entries with >32 attrs (panic). Now
  `take(FORM_ROWS)`-bounded (graceful truncation). Fixed.
- The tvision UI now accepts `--config <path>` (was positional-only; implemented during M2).
- **Intra-pane keyboard nav**: arrows navigate within a pane (leaf list while the
  search box keeps focus; form fields). Tab switches between panes/widgets.

**tvision-rs upstream improvement** (oetiker's repo; edaptor is its first real
consumer). The pane-focus gap (panes nested in a Splitter were unreachable by Tab)
was fixed UPSTREAM as **hierarchical Tab focus traversal** — Tab/Shift-Tab walk the
focusable-leaf tree across nested groups. Shipped in **tvision-rs 0.2.0**; edaptor
depends on the release. PR #5 (merged). The Splitter is now transparent to focus.

---

## M2 SAVE + GUARDS — live-accepted (the gate is now CLOSED)

The save round-trip and guard dialogs were driven live in tmux and **a real bug
was found and fixed before it could reach M3**:

- **Symptom:** edit a plain attr → Alt-S → confirm dialog → Enter → dialog closed
  but nothing persisted, form stayed dirty, no error.
- **Root cause:** on modal open, tvision's `first_match_visible_selectable`
  focuses the *last-inserted* selectable child — the **Cancel/Stay** button. A
  focused non-default button becomes the acting default, so Enter fired Cancel and
  `do_save` early-returned. Faithful Turbo Vision behaviour; `message_box` dodges
  it via `initial_focus`, but edaptor's bare `exec_view` passed `None` and the
  public API had no way to set focus.
- **Fix (upstream, user-chosen):** tvision-rs **PR #6** adds
  `Program::exec_view_focused(view, focus)`. edaptor's `confirm`/`guard`/`error`
  builders now return `(view, default_btn_id)` and `app::dispatch` calls
  `exec_view_focused`, so dialogs open with Save/OK focused. edaptor is git-pinned
  to the PR commit until it releases (see the banner at the top).
- **Verified live:** save persists to LDAP + dirty `*` clears + re-read; confirm
  Esc keeps editing; quit guard (Enter=Save→confirm, Stay, Discard=quit-no-save);
  dirty-nav guard fires and Discard navigates + drops the edit. Demo data restored.

Useful detail discovered while driving: **only single-valued attributes are
inline-editable** in the form (uidNumber, gidNumber, homeDirectory, sambaSID,
displayName, employeeNumber, gecos, loginShell for a posix/samba user) — arrow nav
cycles exactly those; multi-valued-capable attrs (cn, sn, mail, description, …) are
intentionally skipped (the `present_field` multi-value short-circuit). Also: a
focused `InputLine` selects-all, so typing replaces the whole value.

---

## ⭐ Live-driving the TUI from an agent session (tmux)

**The old handover said interactive checks need a human — that is SUPERSEDED.** You
can drive the real TUI yourself over a PTY with tmux (no human needed):

```bash
scripts/test-ldap.sh start                       # podman demo server (idempotent if up)
tmux kill-session -t edtv 2>/dev/null
tmux new-session -d -s edtv -x 210 -y 50         # wide enough for 3 panes
tmux send-keys -t edtv 'export EDAPTOR_TEST_ADMIN_PW=adminpassword' Enter
tmux send-keys -t edtv '/home/oetiker/scratch/cargo-target/debug/edaptor --config examples/demo-config.toml' Enter
sleep 4
tmux send-keys -t edtv Down       # keys: Down/Up/Tab/Enter, or a literal like 7 / 'User2'
sleep 0.4
tmux capture-pane -t edtv -p | sed -n '2,14p'   # read the screen
tmux kill-session -t edtv         # clean up (the run holds an LDAP bind)
```
Notes: build the binary first (`cargo build -j4 --bin edaptor`) so the run is
fast; the binary is at `/home/oetiker/scratch/cargo-target/debug/edaptor` (NOT
`./target`). For modals (Confirm/Guard) send the button hotkey or arrows+Enter.
Do NOT trigger destructive saves carelessly against the demo data; prefer a temp
entry, or edit-then-Discard. Insert `sleep` between keystrokes (async reads land
via the 50ms pump).

---

## M3 is split into two cycles (each its own spec → plan → implement)

The user approved splitting M3 so the carried-forward problems are fixed first,
because the create flow reuses the form pane and the tree-guard machinery.

### ✅ Phase 1 — stabilize the base (DONE — signed off, strip fixed + guard edges live-verified)

Spec [`specs/2026-06-26-tvision-m3-phase1-stabilize-design.md`](superpowers/specs/2026-06-26-tvision-m3-phase1-stabilize-design.md),
plan [`plans/2026-06-27-tvision-m3-phase1-stabilize.md`](superpowers/plans/2026-06-27-tvision-m3-phase1-stabilize.md).
**12 TDD tasks**, foundation-first:
- **Tasks 1–4 — `ScrollGroup`** (NEW, domain-free, `src/ui/scroll_group.rs`): a
  generic vertical scroll-container of child views. tvision-rs has NO scroll
  container for child widgets (Group has no child offset; Scroller is self-drawn) —
  so we build one: it holds child views at logical positions and repositions them
  by `-top` on scroll (the framework clips offscreen children via `DrawCtx::sub`),
  driving a linked `ScrollBar` through the `ScrollSync` broker, with scroll-to-
  focused. Feasibility was spiked at the source level (PASS). **It is built for
  extraction → a follow-up upstream tvision-rs PR once proven (the user wants this
  contributed); edaptor then switches to the published widget.**
- **Tasks 5–6 — `FormPane` on `ScrollGroup`:** one persistent `Label`+`InputLine`
  per field, rebuilt per entry (`Group::remove`/`insert`), **drops `FORM_ROWS=32`**;
  `on_bounds_changed` refit. Editing reuses the M2 inline path unchanged (each
  field owns its cell → no scroll-time edit smear).
- **Task 7 — `LeafPane` fill** (`on_bounds_changed`) → kills the `▒` strip.
- **Task 8 — guard edge #2** (cancelled confirm snaps back like Stay).
- **Tasks 9–11 — guard edge #3** (branch nav controller mirroring the leaf path:
  `requested_branch`/`reconcile_branch`/`GuardTarget` enum, `TreePane` pure-selector,
  pump + dispatch with `set_tree_row` snap-back on Stay).
- **Task 12** — `CHANGES.md`, facade guards, `make check`, live tmux acceptance.

Execute **subagent-driven** (fresh subagent per task + spec-then-quality review;
final whole-branch review). The plan has complete per-task code; verified 0.3.0 API
facts are listed in its self-review.

### ✅ Phase 2a — objectClass widget + live resync (DONE — live-accepted @ `1e1263d`)

The riskiest half of the M3 core, on EXISTING entries. Delivered: the first reusable
modal-widget seam (`Activation::Modal(Box<dyn FieldEditor>)` + generic `app::dispatch`
ACTIVATE path); the objectClass picker (`src/ui/oc_picker.rs` — schema-seeded
multi-select `ListBox`, client substring filter, pre-tick, staged
`SetValuesThenResyncSchema`); the neutral `EditForm::sync_schema_fields` port (+
`EditField::injected`, `order_fields`); `UiState::{activate_field, staged_commit,
apply_commit}`; form-pane modal-row focus/Enter→ACTIVATE; gated `tests/tv_objectclass.rs`.
**Accept (met):** editing objectClass on an existing entry adds/orphans fields live via
the typed outcome (no global flag). See the spec/plan dated 2026-06-27 and the NEXT
ACTION banner's load-bearing facts (`reset_current` hook; the construction-time
borrow trap; objectClass-field-is-authoritative).

### ✅ Phase 2b — create flow + autonumber + password widget (DONE — live-accepted @ `772b8b9`)

The full create story (user chose the full scope, pulling the M4 password widget
forward), delivered in three blocks: **A core create** — `FormMode::Create`,
`build_create_form` (objectClass auto-injected + editable, MUST/MAY via
`sync_schema_fields`, static defaults), Alt+N (`CREATE`) → profile chooser /
single-profile fast path → create form, `write_flow::submit_create` (`Request::Add`)
+ `WriteOutcome::Created` → navigate, `do_create`. **B autonumber** —
`workflows::alloc_flow::AllocFlow` (async next-free-number scan, ids 2M+) + pump
correlation + `‹allocating…›` placeholder. **C password widget** —
`connection_encrypted`, neutral `apply_widget_bindings` (widget_binding + secret,
wired into edit + create), `PasswordWidget`/`PasswordEditor` (TLS-gated New+Confirm
masked → `StageSecret`), create-fold + edit-fold, sentinel-never-submitted guard.
**Accept (met):** new entry created from a profile end-to-end (live-verified:
fast-path, objectClass auto-inject, autonumber, live DN; real ADD/read/delete by the
gated `tests/tv_create.rs`); password edits masked + TLS-gated. The create-form
wrinkle (`empty_form_for_profile` excludes objectClass) was resolved in
`build_create_form` by injecting an editable objectClass field seeded with
`["top"]+profile.object_classes` then running `sync_schema_fields`.

---

## Nav/guard model (redesigned — read before touching the panes)

Entry-switching was reworked into a **controller-owned transition** (the old
per-pane `submit_selected` poll that posted `GUARD_NAV` only worked for keyboard —
mouse selection runs inside a tvision mouse-track capture, and `program.rs:2267`
skips the app handler when a capture consumes the event, so the pane's posted
command was swallowed). The model now (user-chosen "B"):

- **The form follows the highlight.** Clean form → moving the highlight loads that
  entry. Dirty form → it is **pinned**: no other entry is shown until the guard
  (Save / Discard / Stay) is resolved; **Stay** snaps the highlight back to the form.
- **Panes are pure selectors.** `LeafPane::report_selection` only records
  `UiState::requested_leaf`; it never reads, guards, or posts. The **pump** calls
  `UiState::reconcile_selection` each tick (clean → load; dirty → stash
  `guard_target`, post `GUARD_NAV` from its clean, capture-free context) and
  `app::dispatch` opens the modal; Stay sets `set_leaf_row` (= `current_leaf_row`)
  so the pane snaps the highlight back. Any future trigger (M3 create-flow, tree)
  should funnel through `requested_leaf` → `reconcile_selection`, NOT re-poll.

**Known edges — both now SCHEDULED in the Phase 1 plan (no longer loose TODOs):**

- ✅ FIXED: TV first-click on an unfocused pane only focused it. The leaf/form
  panes now set `options.first_click = true` (the tree got it free via the
  Outline), so a single click both focuses the pane and lands on the row/field.
- 📋 **#2 (Phase 1 Task 8):** guard→Save then **cancelling the confirm** leaves the
  list highlight on the would-be target while the form stays pinned (mismatch).
  Fix: treat a cancelled confirm like "Stay" (`set_leaf_row = current_leaf_row()`);
  `do_save` returns a `SaveOutcome`, dispatch snaps back on `NotSubmitted`.
- 📋 **#3 (Phase 1 Tasks 9–11):** changing **branch** while dirty guards but Stay
  can't snap back. Fix: extend the controller-owned model to the tree
  (`requested_branch`/`reconcile_branch`/`GuardTarget::{Leaf,Branch}`), `TreePane`
  becomes a pure selector, Stay reverts via `set_tree_row = current_branch_row()`.

## Deferred to M3 / cleanup (logged from M2 reviews)

- **Scrollable form + pane fill** — DONE in the Phase 1 plan (Tasks 1–7): the
  `▒` strip and the `FORM_ROWS = 32` cap both go away via the new `ScrollGroup`
  (form) and `on_bounds_changed` (leaf). The full-screen flip exposed the `▒`
  strip; root cause is the panes not re-fitting children on resize, not the flip.
- Minor/cosmetic: `value_set_eq` duplicate-value false-positive; `EditForm::set_value`
  has no `!multi` guard (only single-value callers in M2); read-error shows a stale
  form (status-only); a few `let _ = ctx/REFRESH` import/param silencers in `form.rs`;
  dialog module/builders are `pub` (could tighten to `pub(crate)` now that
  `app::dispatch` consumes them).

---

## Build / test / run

**⚠ Cap parallelism at 4 cores** (shared box). Target dir `/home/oetiker/scratch/cargo-target`.

```bash
cargo build -j4                 # the edaptor binary (tvision UI)
cargo test  -j4                 # lib tests + gated integration (skip w/o env)
cargo clippy -j4 --all-targets -- -D warnings
cargo fmt --check
make check                      # fmt + clippy + tests

# Live LDAP demo (podman): ~600 users / ~25 groups, ldap://localhost:1389
scripts/test-ldap.sh start
export EDAPTOR_TEST_ADMIN_PW=adminpassword
cargo run -- --config examples/demo-config.toml
# gated live tests:
EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389 EDAPTOR_TEST_ADMIN_PW=adminpassword \
    cargo test -j4 --test tv_membership --test tv_picker
```

Facade guards (must print nothing — single-UI reality):
```bash
! grep -rl "use tvision_rs" src | grep -vE "^src/ui/"
! grep -rl "use ratatui\|use tui_" src
```

---

## Conventions (follow these)

- **Facade boundary:** only `src/ui/**` may `use tvision_rs`. The domain layer
  (`config`, `form`, `ldap`, `schema`, `samba`, `workflows`) imports NEITHER
  tvision_rs NOR any former ratatui/tui_* crate, and stays UI-agnostic. There is
  no ratatui tree anymore; the single-UI guards confirm this on every gate run.
- **Widget palette** is the one config-driven "rich field" home: a
  `[profile.widget.<attr>]` `kind` → `config::widget::WidgetKind`. The form honours
  `EditField.widget_binding`. M4 added the rich widgets (choice/password/picker/
  membership/multi-value/sambaSID) as `FieldWidget` impls with NO form-core changes.
- **Borrow discipline:** never hold a `RefCell`/`UiState` borrow across
  `ctx.broadcast`/`ctx.post`/`Program::exec_view`/`worker.submit`/`new_list`/
  `child_mut`/`set_value`. Collect into locals → drop the borrow → call.
- **Strict TDD**, atomic commits, crate compiles after every commit, `cargo fmt`
  before each commit, clippy `--all-targets -D warnings` clean.
- **Live tests gated** by `EDAPTOR_TEST_LDAP_URI` (skip when unset). Base
  `dc=example,dc=org`, password env `EDAPTOR_TEST_ADMIN_PW=adminpassword`.
- **Docs one-home:** config detail → mdBook (`docs/src/`); README orientation only;
  `CHANGES.md` for every user-visible change (the tvision preview already has
  entries). Process/design → `docs/superpowers/`.
- **Commit trailer:** `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
  ⚠ Use `git commit -F` (a file/heredoc) for messages with backticks — `-m "...`...`"`
  triggers shell command-substitution and mangles the message.
- **Execution style:** subagent-driven (fresh subagent per task + two-stage review).
  One editor per working tree at a time.

---

## Load-bearing tvision-rs facts (0.3.0 — so you don't rediscover them)

**New in 0.3.0 / discovered this session (Phase 1 planning):**
- **Frameless full-screen:** `Window::set_fullscreen(Fullscreen::{Off,Desktop,Screen})`
  composes maximize + border-drop, and `Command::FULLSCREEN` cycles it. edaptor's
  pump posts `Command::FULLSCREEN` once on its first tick (the pane can't downcast to
  `Window`; the command routes to the desktop's only window). `Desktop` keeps the
  menu bar + status line. The window's **drop shadow is disabled** (it would paint a
  one-cell strip over the desktop along the right/bottom edges).
- **No scroll-container for child views** (this is WHY Phase 1 builds `ScrollGroup`):
  `Group` has no child scroll offset; `Scroller` is for self-drawn content only. But
  **repositioning children works**: `Group::draw` draws each child through
  `ctx.sub(child_bounds)` and `DrawCtx::sub` clips to `parent_clip ∩ child_bounds`
  (context.rs:910), so a child moved to negative-y / past the bottom clips at the
  edge; mouse routing is `bounds.contains` + local translate (sign-agnostic).
- **Scroll broker:** a content view holds only its bar's `ViewId`. Publish range/value
  with `ctx.request_scroll_bar_params(bar, value,min,max,page,arrow)`; on the bar's
  `SCROLL_BAR_CHANGED { source }` broadcast call `ctx.request_scroll_sync(self_id, h, v)`;
  the pump resolves and calls your overridden `View::apply_scroll_sync(h,v,ctx)`
  (defaulted no-op). `ScrollBar::new(rect)` infers vertical when width==1.
- **`Group::remove(id, ctx)`** exists (alongside `insert`) — so dynamic per-entry
  child rebuild is fine (the Phase 1 FormPane uses it).
- **Headless DRAW tests:** `Buffer::new(w,h)` + `DrawCtx::new(&mut buf, &theme, clip,
  origin)` + `buf.get(x,y).symbol()`; `Theme::classic_blue()`. The crate's own
  `fill_clips_to_clip_rect` / `sub_narrows_clip_and_shifts_origin` are the templates.
- Crate-root re-exports incl. `Buffer, DrawCtx, Point, ScrollBar, GrowMode, StaticText,
  Label, Outline` (lib.rs:124–154). Hide a view by `state_mut().state.visible = false`.

**Carried from 0.2.0:**

- **Hierarchical Tab (new in 0.2.0):** Tab/Shift-Tab walk the focusable-leaf tree
  across nested groups; the Splitter is transparent to focus. Consequence for a
  pane-heavy UI: EVERY focusable widget is a Tab stop (e.g. leaf search box AND
  list are separate stops) — more granular than "Tab between panes". edaptor adds
  arrow-key intra-pane nav to mitigate. A widget that owns Tab (multi-line editor)
  still consumes it.
- **App skeleton:** `Program::new(backend, clock, theme, init_desktop,
  init_status_line, init_menu_bar)` then `program.run_app(|prog, cmd| …)`. The
  `init_*` factories take capturing closures, so `Rc<RefCell<UiState>>` reaches the
  view tree (no thread_local). **`run_app`'s `(prog, cmd)` closure is the ONLY
  place with `&mut Program`** → the single `exec_view` site (`tui::app::dispatch`).
- **`Program` has NO `post`/`broadcast`.** End the app from the dispatch closure
  via `prog.end_modal(Command::QUIT)`. Panes/pump request modals by POSTING a
  custom command (`ctx.post(cmd)`) that surfaces to `run_app`'s closure;
  `Command::QUIT` is consumed by the built-in handler before the closure sees it
  (hence the custom `REQUEST_QUIT`). `exec_view(view) -> Command` blocks but the
  pump timer keeps firing inside it (so async writes finish with a dialog open).
- **Worker → views:** zero-area `PumpView` + `Context::set_timer(50ms)` periodic
  `Event::Timer` → drain `worker.poll()` → correlate via `read_flow`/`write_flow`
  → `ctx.broadcast(REFRESH)`. Broadcasts reach every view incl. zero-area/disabled.
- **`Outline` (0.1.1+):** auto-seeds scrollbar/focus on first display; call
  `tv::ov_update` only after MUTATING the tree. Read selection via
  `Outline::value() -> Some(FieldValue::Int(foc))` (parity with `ListBox`).
- **`ListBox` consumes (clears) Up/Down/PageUp/PageDown.** Detect a selection
  change via `value()` vs a saved index — do NOT gate on `ev` still being a
  KeyDown after `group.handle_event` (the tree pane does this right; the leaf pane
  bug was exactly this).
- **`StaticText` has no `set_value`** (only `new`/`text`/`set_text`). For a cell
  whose text updates at render, use a disabled `InputLine` (the `ro_cell` idiom).
  `StaticText` is fine for static dialog content.
- **Dialogs:** `Dialog::new(rect, Some(title))`; `dlg.state_mut().options.center_x/
  center_y`; `dlg.insert_child(Box::new(StaticText::new(...)))`;
  `dlg.button_row(&[(label, Command, ButtonFlags)], ButtonRowAlign)`. Buttons MUST
  use modal-exit commands (`OK`/`CANCEL`/`YES`/`NO`) so `exec_view` returns them.
- **Headless view tests:** `Context::new(&mut out, &mut timers, 0, &mut deferred)`
  with `tv::timer::TimerQueue::new()` and `Vec<tv::Deferred>` (crate-root
  `tv::Deferred` since 0.1.1). A standalone `InputLine` needs
  `state.state.selected = true`. tvision events are `Event::KeyDown` (NOT `Key`).

## Upstream tvision-rs (oetiker's repo) — working with it

edaptor is tvision-rs's first real consumer; improve it as you go. Directive: work
in a SEPARATE clone of `https://github.com/oetiker/tvision-rs`, one focused PR per
change; edaptor depends on the PUBLISHED crate (a git pin is the only fallback, and
only until a release ships — then bump the version and drop the pin). The
`#[delegate(to=group)]` macro forwards `View` methods via a manual list in
`tvision-rs-macros/src/specs.rs` — a NEW `View` method must be added there to be
forwarded through wrapper views. ⚠ `gh pr edit` fails on that repo (deprecated
Projects-classic GraphQL) — edit PR title/body via REST: `gh api -X PATCH
repos/oetiker/tvision-rs/pulls/N -f title=… -F body=@file`.
