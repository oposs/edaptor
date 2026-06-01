# edaptor — Migration from turbo-vision to ratatui (Plan)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax. After each task: `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings` must pass; `cargo fmt` before commit.

Date: 2026-06-01
Status: Planning (ready for implementation)
Branch: `feat-three-pane` (see §8 — branch off this, NOT `main`)

**Goal:** Port edaptor's TUI off the `turbo-vision` crate onto the latest **ratatui** stack, with NO vendored forks, preserving the full three-pane browser/editor UX and adding two features turbo-vision could not do cleanly: a **multi-value popup editor** and **password masking**. The domain layer (LDAP worker, schema, diff/validate/ldif, structure model, workflows) ports **untouched**.

**Why we are leaving turbo-vision:** its `InputLine::draw` byte-slices UTF-8 and **panics** when a multibyte char (German umlaut) straddles the visible-width cut — the app "just quits" on horizontal scroll. We currently carry a vendored fork (`vendor/turbo-vision/` + `[patch.crates-io]`) to work around it. ratatui + tui-prompts are Unicode-correct by construction, so the fork and patch get **deleted** in this migration.

**Tech stack (latest ratatui, no forks):**
- `ratatui = "0.30"`
- `tui-tree-widget = "0.24"` (targets ratatui ^0.30)
- `tui-prompts = "0.6"` (targets ratatui ^0.30; used as the Unicode-correct **edit-state engine**, render done by hand)
- `crossterm = "0.29"` (the version ratatui 0.30 + tui-prompts use, so `KeyEvent` types unify)

> `tui-textarea` was **rejected**: its latest release only supports ratatui ^0.29, which would force us off the latest ratatui. We render values by hand and use `tui_prompts::TextState` purely as the cursor/Unicode edit engine (`handle_key_event`, `value()`, `position()`, `focus()/blur()`), so each pane owns its own background.

**Proven reference:** a working spike at `/scratch/oetiker/ratatui-spike/src/main.rs` (~475 lines, compiles clean, tmux-verified). It is the render/input reference for the new UI. **READ IT before Phase 1.** See §2.4 for exactly what it does and does NOT cover.

---

## 1. Scope & boundary

### 1.1 The keystone fact

`src/ui/facade.rs` (1944 lines) is the **only** module that actually `use turbo_vision`. Verified:

```
$ grep -rln "turbo_vision" src/
src/app.rs            # comment only ("the facade translates raw TV events…")
src/form/mod.rs       # doc comment only
src/ui/mod.rs         # doc comment only
src/ui/facade.rs      # REAL use (the only one)
src/workflows/browser.rs  # comment only
src/main.rs           # comments only ("No turbo_vision type is named…")
```

Only `facade.rs` has `use turbo_vision::…`. Every other hit is a comment. The migration replaces `facade.rs` and the `run_loop` *closure body* in `src/main.rs`; the domain layer is framework-agnostic and ports unchanged.

### 1.2 STAYS UNTOUCHED (ports verbatim)

- `src/ldap/` — `worker.rs`, `ldif.rs`, `result.rs`, `tls.rs` (worker thread, paged subtree scan, write paths, rc→message).
- `src/schema/` — `model.rs`, `syntax.rs` (incl. `is_single_value`, `field_kind`, `effective_attributes`).
- `src/form/changeset.rs`, `src/form/validate.rs` — diff/MODRDN/validate/`plan_save`. **EXCEPTION:** one small additive change in `changeset.rs` for the X-ORDERED case (§4) — this is the only domain edit in the whole migration.
- `src/ui/form.rs` — `FormModel`/`FormField`/`WidgetSpec` (read-only-oriented; reused as the *source* the new editable form model is built from — see §3.3).
- `src/ui/form_state.rs` — `guard_decision`/`GuardChoice`/`GuardOutcome` (pure; reused verbatim).
- `src/workflows/` — `browser.rs`, `read_flow.rs`, `create.rs`, `structure.rs` (structure model, `ReadFlow`, `empty_form_for_profile`, `build_add_entry`).
- `src/samba/`, `src/config/`, `src/app.rs` (`UiAction`/`LoopEvent`/`MenuDef`/`menu_action`/`build_menu_defs` — the `CM_*` constants stay as menu ids; see §2.3).
- **`src/main.rs` app-free helpers port verbatim:** `prepare_save`, `submit_prepared`, `compose_renamed_dn`, `parent_dn`, `navigate_form`, `compute_rows`, `edit_entry_from_model`, `structure_input_from_attrs`, `structure_inputs`, `next_id`, and the `PostWrite` / `PrepareSave` enums. Only the `run_loop` closure body and the `facade::*` calls inside it are rewritten (§2.2).
- All domain tests under `tests/` and inline `#[cfg(test)]` modules (changeset/validate/ldif/structure/form/schema) keep passing throughout.

### 1.3 REPLACED

- `src/ui/facade.rs` → deleted and replaced by new ratatui UI modules (§3.1). The boundary module pattern (facade is the only TV importer) is replaced by: ratatui/crossterm imported only inside `src/ui/` (new modules) and the new event loop in `main.rs`.
- The `run_tui` event loop in `src/main.rs`: the blocking-broadcast `Shell::run_loop(|app, event| …)` is replaced by an explicit ratatui draw/poll loop that owns a single `App` state struct (§2.1, §2.2).

### 1.4 DELETED

- `vendor/turbo-vision/` (the entire vendored fork).
- The `[patch.crates-io] turbo-vision = { path = "vendor/turbo-vision" }` block and the `turbo-vision = "1.2"` dependency in `Cargo.toml`.
- `tests/utf8_inputline_repro.rs` (a TV-`InputLine`-specific panic repro — moot once `InputLine` is gone; its *lesson* is re-borne as a ratatui umlaut render test in P1, see §6).
- The focus-gated **dimming palette** added in today's uncommitted facade diff (`dark_window_palette`/`pane_palette`) — a turbo-vision palette-chain mechanism with no ratatui analogue. The *intent* (active pane = solid white bg, inactive = dim) carries forward via the spike's focus-gated `Block` style (§3.2).

### 1.5 CARRIES FORWARD CONCEPTUALLY (not code)

- `center_origin`/`centered_rect` **math** from facade.rs (lines ~1411–1424) → re-expressed as the spike's `centered(w, h, area) -> Rect` helper (spike lines 471–475). Reserve-chrome behavior and the `center_origin_centres_and_reserves_chrome` test idea carry over as a pure unit test.
- The **umlaut-crash lesson** → a ratatui render test that draws a German value wider than the value cell and asserts no panic + correct truncation (§6).

---

## 2. New architecture

### 2.1 Single owned `App` state (replaces `Rc<RefCell>` handles)

turbo-vision drove panes through shared `Rc<RefCell<…>>` handles (`LeafHandles`, `FormHandles`) plus `CM_*` broadcast refreshes, because TV owns the view tree and the loop cannot borrow into it. ratatui is **immediate-mode**: the loop owns all state as plain data and re-renders every frame. So the handle plumbing collapses into one struct:

```rust
struct App {
    // focus / layout
    focus: Pane,                 // Tree | Leaf | Form
    split: [u16; 2],             // two divider fractions (if divider drag kept; §7)
    read_only: bool,

    // pane 1 — branch tree
    tree_state: TreeState<String>,           // tui-tree-widget
    tree_items: Vec<TreeItem<'static, String>>,

    // pane 2 — leaf list + incremental search
    current_branch: String,
    rows: Vec<(String, String)>,             // (label, dn) from compute_rows()
    leaf_sel: usize,
    search: TextState<'static>,              // the incremental-search edit box
    last_seen_leaf: Option<String>,

    // pane 3 — live edit form
    form: Option<EditForm>,                  // None = nothing selected (§3.3)
    form_focus: usize,
    form_scroll: usize,

    // overlays (modal state-flags; §3.4)
    overlay: Option<Overlay>,                // Confirm | Error | Guard | Ldif | ValueEditor | CreateForm

    // status line
    status: String,
}
```

`Pane`, `EditForm`, `Overlay`, `ValueEditor` are defined in §3. `compute_rows`, `EditEntry`, `FormModel`, `Structure`, `ReadFlow`, `WorkerHandle` are the **unchanged** domain types. The `pending_followups` / `post: HashMap<u64, PostWrite>` write-tracking maps and `current_form: Option<(FormModel, Vec<String>)>` baseline carry over from today's `run_tui` and live next to `App` (or inside it).

### 2.2 Explicit event loop (replaces `Shell::run_loop` + broadcasts)

The current loop is a callback fed `LoopEvent::{Idle, Action}`; idle drains the worker, actions come from TV commands. The new loop is an explicit ratatui loop. **Critical (non-obvious):** the spike uses **blocking** `event::read()`, but the real app must drain `worker.poll()` every tick (async write/read results arrive on the worker channel). A blocking read would starve the worker drain. So:

```rust
fn run(terminal, app, worker, …) -> Result<()> {
    loop {
        terminal.draw(|f| ui(f, app))?;

        // 1) Drain ALL pending worker responses (the old LoopEvent::Idle body —
        //    read-flow → form, WriteOk/WriteError → post/pending_followups/error).
        while let Some(resp) = worker.poll() { handle_worker_response(app, &resp, …); }

        // 2) Poll input with a timeout so the worker drain keeps ticking when idle.
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Release { continue; }
                match dispatch_key(app, key) {            // §2.3
                    Some(UiAction::…) => handle_action(app, action, &worker, …),
                    None => {}
                }
            }
            // Event::Mouse(_) => handle_mouse(app, …)    // §7, if kept
            // Event::Resize(_, _) => relayout (next draw handles it)
        }

        // 3) post-input housekeeping that the old Idle body did each tick:
        //    - search changed → recompute rows (compute_rows)
        //    - leaf selection changed → dirty guard → navigate_form
        reconcile(app, &worker, …);

        if app.should_quit { return Ok(()); }
    }
}
```

`handle_worker_response`, `reconcile`, and the action/guard handlers reuse the **exact** body of today's `run_tui` closure (the `Response::WriteOk` post-handling, `ReadFlow::on_response`, the `guard_decision` branch, `Activate`/`FormSave`/`FormCancel`/`NewEntry`/`DeleteEntry`/`Refresh` arms) — only the `facade::info/confirm/confirm_error/refresh_*` calls are swapped for `app.status = …` / set `app.overlay` / direct `App` mutation. This is the single largest re-host and the bulk of the migration effort (the spike does NOT cover any of it — §2.4).

### 2.3 Event/action mapping (replaces `CM_*` commands + broadcasts)

- **`dispatch_key`** translates a focused-pane `KeyEvent` into either a direct `App` mutation (tree nav, leaf nav, form field nav/edit, focus cycle on F6/Tab) or a `UiAction` (the existing enum in `src/app.rs`: `Activate`, `FormSave`, `FormCancel`, `NewEntry(i)`, `DeleteEntry(dn)`, `Refresh`). **`UiAction` and `menu_action`/`build_menu_defs` are reused as-is** — the `CM_*` ids remain the menu command vocabulary; the menu bar becomes a ratatui top line (§3.5) whose entries map key/click → `menu_action(cm, …) -> UiAction`.
- **Broadcasts disappear.** `refresh_tree`/`refresh_leaf`/`refresh_form` were "tell the TV view to rebuild from the shared handle." In immediate mode the next `terminal.draw` already reflects `App`, so these become no-ops (delete the calls; the data write that preceded each one stays).
- **Key map** (from spike + spec parity): `F6`/`Tab` cycle pane focus; tree `↑↓`/`Enter`/`Space` (expand/select → sets `current_branch`, emits `Activate`); leaf `↑↓` select, typing routes to the search `TextState`; form `↑↓` field nav, `PgUp/PgDn` scroll, `Enter` on a multi-value field opens the ValueEditor popup, text keys edit the focused single-value field; `F2` Save, `F3` Cancel; `Alt+X`/`q` quit (guard if dirty). Overlay-open swallows keys to the overlay (spike `popup_key` pattern).

### 2.4 What the spike proves — and what it does NOT

**The spike proves (render + input plumbing only):** 3-pane `Layout`; focus-gated active-pane **solid white background** via `pane_block` (the thing TV could not do cleanly); `tui-tree-widget` tree; **password masking** (`"•".repeat(chars().count())`); correct **umlaut** render/edit through `TextState`; manual form **scroll** (`form_scroll` + per-row viewport clip); the **multi-value popup** mechanics (per-value `TextState` rows, Alt+a insert / Alt+d delete / Alt+↑↓ reorder, F2 commit dropping empties, Esc cancel); hand-styled value rendering so each pane owns its bg.

**The spike does NOT have (this is the real migration work):**
- dirty/baseline tracking; the set-wise dirty-check (§3.3 — the spike just mutates `values`);
- the Save/Discard/Stay **guard** dialog and `guard_decision` wiring;
- the **save flow**: validate → diff → LDIF confirm → `Modify`/`ModRdn` → re-read;
- **create** (profile form → validate → LDIF confirm → `Add` → splice Structure) and **delete**;
- **incremental search** filtering pane 2 (it has a static leaf list);
- **read-only mode** (no editors, no Save/Cancel, suppress create/delete);
- **status line** and **menu bar**;
- **mouse** (divider drag / click-to-focus);
- **worker** integration and the async response drain.

So: the migration is **re-hosting `run_tui`'s orchestration on the spike's render model**, not "port the spike."

### 2.5 View → ratatui mapping

| turbo-vision (facade.rs) | ratatui replacement |
|---|---|
| `DitOutline` (`OutlineViewer` in a windowed wrapper, `CM_DIT_ACTIVATE`/`CM_DIT_REFRESH`) | `tui_tree_widget::{Tree, TreeState, TreeItem}` rendered with `render_stateful_widget`; selection read from `TreeState`, drives `current_branch`/`Activate` |
| `LeafListPane` (`ListBox` + search `InputLine`, `CM_LEAF_SELECT`/`CM_LEAF_REFRESH`) | hand-styled `List`/`Paragraph` of `rows` with a highlighted `leaf_sel`; a `TextState` search box above it; local filter via `compute_rows(structure, branch, search)` |
| `FormPane` (inner `Group` of rows, manual `delta` scroll, Save/Cancel `Button`s) | `EditForm` rows rendered hand-styled per spike `render_form`; `form_scroll` viewport; `TextState` per single-value field; F2/F3 keys (no on-screen buttons needed, but a hint line) |
| `SplitContainer` (frameless Group + draggable dividers) | `Layout::horizontal` with two ratio constraints; divider drag = mouse handler adjusting the ratios (§7) |
| Modals: `confirm`, `confirm_error`, `confirm_guard`, `show_ldif_preview`, `edit_entry_dialog`/`build_entry_dialog`, `show_entry_dialog`, `info` | `Overlay` enum variants rendered as a centered `Clear` + `Block` over the panes; key-driven (spike `render_popup`/`popup_key` pattern); `info` → `app.status` |
| **(new)** multi-value editor | `ValueEditor` overlay — spike `render_popup`/`popup_key` verbatim as the starting point |
| **(new)** password masking | per-field `secret` flag → render `•`×len, never the cleartext (spike `render_form`) |
| Status line (`build_status_line`, F2/F3/Alt+X items, read-only aware) | bottom `Line` built from `App.read_only` + focus + dirty |
| Menu bar (`build_menu_bar` from `MenuDef`s) | top `Line` of menu labels; hotkey/click → `menu_action` |

---

## 3. New module layout & components

### 3.1 Files

**Create (new ratatui UI):**
- `src/ui/app.rs` — the `App` state struct (§2.1), `Pane`, `Overlay`, plus `dispatch_key`, `handle_action`, `handle_worker_response`, `reconcile`. (Or split orchestration into `src/ui/run.rs` if `app.rs` grows large.)
- `src/ui/view.rs` — pure render: `ui(frame, app)`, `pane_block`, `render_tree`, `render_leaf`, `render_form`, `render_overlay`, `render_status`, `render_menu`, `centered`. Mirrors the spike's render fns.
- `src/ui/edit_form.rs` — `EditForm` + `EditField` + `ValueEditor` (the editable form model, §3.3) and the `build_edit_form(model, schema)` constructor. Pure where possible (no ratatui in the model itself; only `TextState` for edit state) → **unit-testable**.
- `src/ui/ordered.rs` (or fold into `changeset.rs`) — the X-ORDERED known-attr predicate (§4).

**Delete:** `src/ui/facade.rs`, `vendor/turbo-vision/`, `tests/utf8_inputline_repro.rs`.

**Modify:** `src/ui/mod.rs` (drop `pub mod facade;`, add the new modules), `src/main.rs` (`run_tui` body + imports), `Cargo.toml` (§9), `src/form/changeset.rs` (§4 only).

### 3.2 Focus-gated pane background (spike `pane_block`)

Active pane = solid white bg / black fg via a `Block` whose `.style()` is chosen by `focused`; inactive = dark bg / gray fg, dim border. This replaces the TV palette-chain dimming with a one-line per-pane choice (spike lines 307–322). **Keep the border-on-focus highlight** (yellow bold) so the active pane is obvious even on a mono terminal.

### 3.3 The editable form model (NET-NEW — `FormField` cannot carry it)

`FormModel`/`FormField` (`src/ui/form.rs`) are **read-only-oriented**: `WidgetSpec` is a read-only widget choice and `FormField` has no `multi`/`secret`/`ordered`/edit-state. The spike's `Field { multi, secret, ordered, editor: TextState }` is the editable shape. So we build an editable model from `FormModel` + `SchemaModel`:

```rust
struct EditField {
    label: String,
    must: bool,                  // FormField.is_must
    editable: bool,              // facade's field_is_editable rule (read-only kinds → false)
    multi: bool,                 // !schema.is_single_value(label)
    secret: bool,                // password-attr rule: label ∈ {userPassword, sambaNTPassword, …}
    ordered: bool,               // X-ORDERED predicate (§4)
    values: Vec<String>,         // FormField.values (display order)
    editor: TextState<'static>,  // single-value inline edit state (seeded from values[0])
    kind: FieldKind,             // for read-only display formatting (field_display)
}
struct EditForm { dn: String, fields: Vec<EditField>, baseline: BTreeMap<String, Vec<String>> }
```

`build_edit_form(model: &FormModel, schema: &SchemaModel, read_only: bool) -> EditForm`:
- `multi  = !schema.is_single_value(&f.label)` (verified: `SchemaModel::is_single_value` exists, model.rs:177);
- `editable = !read_only && field_is_editable_kind(f.kind)` (port the facade's `field_is_editable` rule — read-only kinds like binary/checkbox/DN-of-`memberOf` stay static);
- `secret` from a small password-attr set (port from config/`samba`; at minimum `userPassword`, `sambaNTPassword`, `sambaLMPassword`);
- `ordered` from the X-ORDERED predicate (§4);
- `baseline` = `{label → values}` for the **set-wise dirty check** (below).

**Dirty / `to_edit_entry`** (replaces `FormPane::is_dirty`/`take_edit`): build the current `EditEntry` by overlaying live editor/popup values onto `baseline`, then `is_dirty = ∃ attr where value_set NE baseline value_set` — **set-wise**, matching `changeset::diff` (the old `is_dirty` used order-sensitive `Vec` `!=`, which after the multi-value popup could report a pure reorder as dirty — exactly the bug §4 guards against). `to_edit_entry()` returns the same `EditEntry` shape today's `FormPane::take_edit` produces, so `prepare_save`/`diff`/`validate` consume it unchanged.

This is an **explicit task** (P2-T1) — it is the crux the form-pane phase otherwise hand-waves.

### 3.4 Overlays (replace modal `Dialog::execute` loops)

In TV, each modal ran its own blocking `execute()` loop. In ratatui there is one loop; a modal is an `Overlay` state-flag that (a) is rendered as a centered `Clear`+`Block` on top, and (b) **captures keys** until dismissed (spike: `if app.overlay.is_some() { overlay_key(app, key); continue; }`).

```rust
enum Overlay {
    Confirm  { text: String, on_yes: PendingAction },  // generic yes/no (save-LDIF, create-LDIF, delete)
    Error    { text: String },                         // confirm_error
    Info     { text: String },                         // (or route info → status line)
    Guard,                                             // Save / Discard / Stay  → GuardChoice
    Ldif     { text: String, scroll: u16 },            // show_ldif_preview (scrollable)
    ValueEditor(ValueEditor),                          // multi-value popup (spike)
    CreateForm(EditForm),                              // create flow (was edit_entry_dialog)
}
```

`PendingAction` carries what to do on confirm (e.g. submit the prepared `SavePlan`, submit the `Add`, submit the `Delete`) so the confirm handler stays small. The **create flow** (`edit_entry_dialog` → an empty schema-driven form → validate → LDIF confirm → `Add`) becomes a `CreateForm` overlay reusing the **same `EditForm`** widget as pane 3 — one editable-form implementation, two hosts (pane 3 + create overlay).

### 3.5 Status line & menu bar

- **Status** (bottom): read-only-aware (`build_status_line` logic): show `Alt+X Quit`; when writable add `F2 Save  F3 Cancel`; append a `*` dirty marker and the current DN. Pure function `status_line(app) -> String`, unit-testable.
- **Menu** (top): labels from `build_menu_defs(profiles)` (unchanged). A hotkey or click maps to `menu_action(cm, profile_count, selected_dn) -> UiAction`. The menu can be a single line of `Label (key)` hints; a full dropdown is optional polish.

---

## 4. The X-ORDERED multi-value diff fix (`changeset.rs`)

**Honest framing — the set-wise diff already exists.** `changeset.rs` **already** diffs multi-valued attributes set-wise: `value_set_eq` (lines 267–269) makes a pure reorder produce **zero** mods, and the multi-valued branch (lines 222–252) emits `Add`/`Delete` by set membership. So the migration must **not** claim to "add set-wise diffing" — that would be false. A pure reorder in the new popup already yields no change. ✅

**The two real gaps:**

1. **X-ORDERED is mis-diffed.** OpenLDAP X-ORDERED config attributes (e.g. `olcAccess`, `olcDbIndex`, `olcSuffix`) carry a `{n}` prefix and order **is** significant; for them a reorder *is* a real change and set-wise diff is **wrong**. The current code diffs them set-wise like everything else. This is an **edge case**: such attrs essentially never appear in a user/group directory (they live under `cn=config`), but the plan must handle it honestly.
   - **Detection — verified:** the schema parser does **NOT** expose an X-ORDERED flag (`src/schema/` has `is_single_value` but no ordered flag; X-ORDERED is an OpenLDAP schema extension the current `RawSubschema` parse drops). So detection is a **hardcoded known-attr predicate** plus an optional `{n}`-prefix heuristic on the values:
     ```rust
     fn is_x_ordered(attr: &str) -> bool {
         const ORDERED: &[&str] = &["olcAccess", "olcDbIndex", "olcSuffix",
                                    "olcRootDN", "olcLimits", "olcSyncrepl"];
         ORDERED.iter().any(|a| a.eq_ignore_ascii_case(attr))
     }
     ```
     (List is conservative; extend if a config-editing profile is ever added. Document that a future schema-parser enhancement could replace the list with a real X-ORDERED flag.)
   - **Diff change:** in `diff`, when `is_x_ordered(attr)` and both sides non-empty, compare **order-sensitively** (`orig == new`) and emit a `Replace` with the full new ordered list on any difference (you cannot express a reorder as Add/Delete for an ordered attr). Otherwise keep today's set-wise behavior.

2. **A regression test that locks the popup-reorder = no-change behavior.** Even though the code already passes, add it so the multi-value popup can never silently regress the diff:
   - `diff_pure_reorder_of_unordered_is_no_change` — `mail: [a, b]` → `[b, a]` ⇒ `cs.is_empty()`.
   - `diff_reorder_of_x_ordered_emits_replace` — `olcAccess: [{0}…, {1}…]` → reordered ⇒ a `Replace` with the new order.
   - `diff_x_ordered_unchanged_is_no_change` — identical ordered list ⇒ empty.

This is **P5-T1** (after the popup exists, so the integration is real), with its own tests; it is the only domain-layer edit in the migration.

---

## 5. Phasing

Each phase ends at a checkpoint a human can verify. Build stays green after every task. UI checkpoints use tmux (the UI needs a tty):

```bash
# build & run against live LDAP
scripts/test-ldap.sh start           # podman OpenLDAP
cargo build                          # binary: /home/oetiker/scratch/cargo-target/debug/edaptor
tmux new-session -d -s ed -x 140 -y 34 \
  "/home/oetiker/scratch/cargo-target/debug/edaptor --config /tmp/edaptor-try.toml"
tmux send-keys -t ed Down Enter      # drive it
tmux capture-pane -t ed -p           # observe
tmux kill-session -t ed
```

### P0 — deps + skeleton compiles (no behavior)
- [ ] **T0.1** `Cargo.toml`: add the 4 crates (§9); remove `turbo-vision` dep + `[patch.crates-io]`; `rm -rf vendor/turbo-vision`. `cargo build` will now fail (facade still references TV) — expected; proceed.
- [ ] **T0.2** Add `src/ui/{app,view,edit_form}.rs` skeletons; `src/ui/mod.rs` drops `facade`, adds the new modules. Stub a minimal ratatui app: init terminal, draw three empty `pane_block`s, `q`/`Alt+X` quits, F6 cycles focus. `main.rs::run_tui` calls the new `run`. Delete `facade.rs` and the now-dead `tests/utf8_inputline_repro.rs`.
- **Checkpoint:** `cargo build` + `cargo test` green (domain tests pass; UI is an empty 3-pane shell). `grep -rn "turbo_vision" src/` returns **nothing** (including comments — clean them up). tmux: three empty panes render, F6 moves the highlight, q quits.

### P1 — read-only 3-pane renders against live LDAP
- [ ] **T1.1** Wire the worker + eager `LoadStructure` (verbatim from `run_tui`) into `App`; build `tree_items` from `Structure` (port `build_structure_tree` logic to `TreeItem`s); seed `rows` via `compute_rows`.
- [ ] **T1.2** `render_tree`/`render_leaf`/`render_form` from the spike; tree selection sets `current_branch` (emit `Activate`), leaf nav sets `leaf_sel`, selection → `navigate_form` → `ReadFlow` base-read → `build_edit_form(read_only=true)` → render.
- [ ] **T1.3** Incremental search: typing in pane 2 routes to `search: TextState`; `reconcile` recomputes `rows` via `compute_rows` when it changes.
- [ ] **T1.4** Umlaut render test (the migration's reason-for-being): a unit/render test drawing a German value wider than the value cell asserts no panic + correct grapheme truncation (re-bears the deleted `utf8_inputline_repro` lesson).
- **Checkpoint:** tmux against live LDAP — tree expands, selecting a branch lists leaves, selecting a leaf shows the read-only form with **umlauts intact**, search filters. No editing yet.

### P2 — editing + save
- [ ] **P2-T1** `build_edit_form` for editable fields (§3.3): `TextState` per single-value field, set-wise dirty check, `to_edit_entry`. **Unit-tested** (build from a `FormModel`+schema fixture; dirty toggles; reorder-of-multi is not dirty).
- [ ] **P2-T2** Form key handling: field nav, scroll, text edit into the focused `TextState`, cursor positioning (spike `render_form` `set_cursor_position`).
- [ ] **P2-T3** Save flow: F2 → `prepare_save` (reused) → on `Ready` open `Overlay::Confirm{Ldif…}` → on yes `submit_prepared` (reused) → `WriteOk` re-reads (reused post-handling). F3 cancel reverts to baseline.
- [ ] **P2-T4** Password masking: `secret` fields render `•`×len, never cleartext, in both pane 3 and the value editor.
- **Checkpoint:** tmux — edit a field, F2 shows the real LDIF confirm, apply writes to LDAP and re-reads; `userPassword` masks; F3 reverts.

### P3 — multi-value popup editor
- [ ] **P3-T1** `ValueEditor` overlay (spike `render_popup`/`popup_key` as the base): per-value `TextState` rows, Alt+a insert, Alt+d delete, Alt+↑↓ reorder, F2 commit (drop empties), Esc cancel; writes back into the field's `values`.
- [ ] **P3-T2** Show the ordered-vs-set hint (`{n} ordered` vs `set`) from the field's `ordered` flag; masking honored for secret multi-values.
- **Checkpoint:** tmux — Enter on a multi-value field opens the popup; insert/delete/reorder/commit work with umlauts; a pure reorder of an unordered attr, after save, produces **no** change (ties to §4).

### P4 — create / delete / guard / refresh / read-only
- [ ] **P4-T1** Dirty-guard on navigation: moving the leaf highlight while dirty opens `Overlay::Guard` → `GuardChoice` → `guard_decision` (reused) → Proceed/SaveThenProceed/Cancel (reuse today's branch bodies).
- [ ] **P4-T2** Create: `NewEntry(i)` → `empty_form_for_profile` → `CreateForm` overlay (reuses `EditForm`) → validate → LDIF confirm → `Add` → splice `Structure` (reuse `PostWrite::Created`).
- [ ] **P4-T3** Delete: `DeleteEntry` → confirm → `Delete` → reflow (reuse `PostWrite::Deleted`). Refresh: `Refresh` → re-run eager scan → rebuild `tree_items`/`rows`.
- [ ] **P4-T4** Read-only mode: no editors, no Save/Cancel hints, suppress create/delete; status line reflects it.
- **Checkpoint:** tmux — full parity sweep against the §3 checklist; read-only config hides write affordances.

### P5 — X-ORDERED diff fix + parity polish
- [ ] **P5-T1** The `changeset.rs` X-ORDERED fix + the three regression tests (§4).
- [ ] **P5-T2** Status line, menu bar, cursor positioning, scroll-keeps-focus-visible, error overlays for every worker error path; `format_validation_errors` reused for validation overlays.
- [ ] **P5-T3** Parity sweep vs §3 checklist; fix gaps.
- **Checkpoint:** `cargo test` (incl. new X-ORDERED tests) green; tmux parity sweep clean.

### P6 — cleanup & finalize
- [ ] **T6.1** Confirm `vendor/` gone, no `[patch.crates-io]`, no `turbo_vision` anywhere (`grep -rn turbo_vision src/ Cargo.toml`), `cargo tree | grep turbo` empty.
- [ ] **T6.2** Remove dead facade-era comments in `app.rs`/`form/mod.rs`/`ui/mod.rs`/`browser.rs`/`main.rs` that reference turbo-vision/`CM_*` broadcasts.
- [ ] **T6.3** `cargo clippy --all-targets -- -D warnings`, `cargo fmt`, full `cargo test`, final tmux smoke.
- **Checkpoint:** clean tree, all tests green, app runs on a real tty with full parity + the two new features.

---

## 6. Feature-parity checklist (vs current three-pane app)

Verify each at the P-checkpoint noted:

- [ ] Tree expand/select (P1) → leaf load (P1) → form (P1)
- [ ] Incremental search in leaf pane (P1)
- [ ] Read-only form render with umlauts (P1) — **the migration's whole point**
- [ ] Editable form fields + cursor (P2)
- [ ] F2 Save → LDIF confirm → Modify/ModRdn → re-read (P2)
- [ ] F3 Cancel reverts to baseline (P2)
- [ ] Password masking (P2)
- [ ] **Multi-value popup editor** (P3) — NEW
- [ ] Dirty-guard (Save/Discard/Stay) on navigation (P4)
- [ ] Create-entry flow (profile form → validate → confirm → Add → reflow) (P4)
- [ ] Delete (confirm → Delete → reflow) (P4)
- [ ] Refresh (re-run eager scan) (P4)
- [ ] Read-only global mode (P4)
- [ ] F6 pane cycle (P0)
- [ ] Status line (read-only / F2/F3 / dirty) (P5)
- [ ] Menu bar from profiles (P5)
- [ ] X-ORDERED-correct diff (P5) — NEW
- [ ] Divider drag (P5/§7, if kept) — mouse
- [ ] Click-to-focus pane (P5/§7, if kept) — mouse

Re-bearing the deleted `utf8_inputline_repro.rs`: the P1 umlaut render test is the replacement.

---

## 7. Risks & owned complexity (honest)

turbo-vision gave these for free; ratatui makes us own them.

1. **Form scrolling** — TV's `FormPane` scrolled by translating an inner `Group`. ratatui: manual `form_scroll` + a per-row viewport clip (spike `render_form`). **Risk:** keeping the focused field visible (the spike's `ensure_visible` is coarse). *Mitigation:* clamp `form_scroll` so `form_focus` is always in `[scroll, scroll+viewport)`; unit-test the clamp (pure).
2. **Focus traversal** — TV's `Group` did Tab cycling. Now explicit (F6/Tab between panes; ↑↓/Tab within the form). *Mitigation:* small, with a unit test for the cycle order; the spike already demonstrates it.
3. **Modal loops** — TV ran nested blocking `execute()` loops. Now one loop + `Overlay` state-flag that captures keys (spike pattern). **Risk:** an overlay must swallow *all* keys (no leak to panes) and the guard/confirm must round-trip a result. *Mitigation:* `if app.overlay.is_some() { overlay_key(); continue; }` as the very first branch; `PendingAction` carries the on-confirm effect.
4. **Cursor positioning** — TV placed the caret via `update_cursor`. Now `frame.set_cursor_position` for the focused `TextState` only (spike). **Risk:** wrong cursor when the value scrolls horizontally past the cell. *Mitigation:* clamp `position()` to the cell width (spike does this); accept simple horizontal-scroll-of-value as polish.
5. **Mouse: divider drag + click-to-focus** — TV had `SF_DRAGGING`/`SF_RESIZING`. ratatui gives raw `Event::Mouse`; we own hit-testing. **Risk/effort:** non-trivial. *Mitigation:* **Decide explicitly** — recommend **deferring mouse** (divider drag + click-to-focus) to a post-migration polish task and shipping keyboard-first parity (F6 + fixed/ratio splits). Spec §2 lists draggable dividers as a goal, but they are not load-bearing for the editor UX; note the deferral in the spec. If kept: `Event::Mouse` → if on a divider column, enter drag mode adjusting `App.split`; else click focuses the pane under the cursor.
6. **Async worker drain vs blocking input** — §2.2: must use `event::poll(timeout)`, not blocking `read()`. **Risk:** subtle starvation/lag if done wrong. *Mitigation:* the loop structure in §2.2 is mandatory; a 50 ms poll keeps the worker drain responsive without busy-spinning.
7. **Editable form model is net-new** (§3.3) — `FormField` can't carry edit state; `build_edit_form` is real new code, not a port. *Mitigation:* P2-T1 is dedicated and unit-tested.
8. **Effort estimate (honest):** P0–P1 are the quick wins (skeleton + read-only render — the spike does most of it). P2 and P4 are the heavy phases (save flow + create/delete/guard re-host). P3 is medium (popup is mostly spike code). P5 is small but must not be skipped (the X-ORDERED correctness fix). Net new code is concentrated in `app.rs` orchestration and `edit_form.rs`; everything else is port-or-reuse.

---

## 8. Branch strategy

**Recommendation: branch off `feat-three-pane`, NOT `main`.**

`feat-three-pane` already carries the **framework-agnostic** three-pane domain layer this migration depends on and keeps: the eager structure model (`workflows/structure.rs`), `form_state.rs`, the worker paged subtree scan + `LoadStructure`, read-only-mode derivation, and the `App`-free `run_tui` helpers. Branching off `main` would discard all of that and force a re-port. The three-pane **UX intent** is preserved; only the **implementation** (turbo-vision → ratatui) changes.

Create:
```bash
git switch feat-three-pane
git switch -c feat-ratatui-migration
```

**Today's uncommitted changes on `feat-three-pane`** (`git status`: modified `Cargo.toml`/`Cargo.lock`/`facade.rs`, untracked `vendor/`, `tests/utf8_inputline_repro.rs`) — decide per item:
- **vendored fork (`vendor/turbo-vision/`) + `[patch.crates-io]` + the `Cargo.toml` patch comment** → **drop** (deleted by the migration; do not commit them).
- **focus-gated dimming palette** in `facade.rs` (`dark_window_palette`/`pane_palette`) → **drop** (TV-specific; intent re-expressed via the spike's focus-gated `Block`).
- **`tests/utf8_inputline_repro.rs`** → **drop** (TV-`InputLine`-specific; replaced by the P1 umlaut render test).
- **`center_origin`/`centered_rect` math + its test** → the math **carries forward conceptually** as the spike's `centered()` + a pure unit test; the TV code itself is dropped with `facade.rs`.

Net: **do not commit the uncommitted TV fixes**; start the migration branch from the last committed `feat-three-pane` state (HEAD), then delete `facade.rs`/`vendor/` as part of P0. (If you want the umlaut repro preserved for posterity, cherry-pick its *intent* into the P1 ratatui test rather than carrying the TV file.)

---

## 9. Dependency / Cargo changes

**Add:**
```toml
[dependencies]
ratatui = "0.30"
tui-tree-widget = "0.24"
tui-prompts = "0.6"
crossterm = "0.29"
```

**Remove:**
```toml
# delete from [dependencies]:
turbo-vision = "1.2"

# delete the whole patch block:
[patch.crates-io]
turbo-vision = { path = "vendor/turbo-vision" }
```
…and `rm -rf vendor/turbo-vision`. `cargo update` regenerates `Cargo.lock` (turbo-vision and its deps drop out; the four ratatui-stack crates and their shared `crossterm 0.29` / unicode-width deps come in). Verify: `cargo tree | grep -i turbo` is empty; `cargo tree | grep crossterm` shows a single 0.29.

---

## 10. Testing strategy

**Becomes pure-unit-testable (more than before — state is plain data now):**
- `build_edit_form` (FormModel+schema → EditField flags: multi/secret/editable/ordered) — new.
- Set-wise **dirty check** + `to_edit_entry` (reorder-of-multi is not dirty) — new.
- `status_line(app)` (read-only / dirty / DN) — new.
- Pane focus cycle order; `form_scroll` clamp / keep-focus-visible — new pure helpers.
- `centered()` placement (port of `center_origin` test).
- X-ORDERED predicate + the §4 diff regression tests.
- All existing pure tests (`menu_action`, `guard_decision`, `compute_rows` inputs, `prepare_save`/`compose_renamed_dn`/`parent_dn`/`next_id`) stay.

**Stays tmux-only (needs a tty):** rendering, focus highlight, scrolling, overlay capture, cursor placement, mouse (if kept). Verified via the §5 tmux recipe against live LDAP (`scripts/test-ldap.sh start`, `/tmp/edaptor-try.toml`).

**Must keep passing throughout:** the domain test suites — `changeset`, `validate`, `ldif`, `structure`, `form`, `schema`, and the gated live tests (`tests/live_*`). Run `cargo test` after every task; these are the regression backstop proving the domain layer truly ported untouched.

---

## 11. Self-review (plan author)

- **Grounded:** `grep` confirms only `facade.rs` truly `use turbo_vision`; `changeset.rs:267` confirms set-wise diff already exists (so §4 is reframed to X-ORDERED + a lock test, not "add set-wise"); `schema/model.rs:177` confirms `is_single_value` exists and **no** X-ORDERED flag (so §4 detection is a hardcoded list); `form.rs` confirms `FormField` lacks edit/multi/secret flags (so §3.3 is net-new); `main.rs` confirms the app-free helpers that port verbatim; the spike was read in full and its non-coverage enumerated (§2.4).
- **Honest effort:** the migration is mostly **re-hosting `run_tui` orchestration** on the spike's render model plus one net-new editable-form model; not "port the spike."
- **Open decision for the implementer:** keep or defer **mouse** (divider drag / click-to-focus). Recommendation: defer to post-migration; ship keyboard-first.
