# edaptor M4 — generic-tier WRITE path + wiring the read flow into the live UI

> **For agentic workers:** REQUIRED SUB-SKILL: Use **superpowers:subagent-driven-development** (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Strict TDD per task: write a failing test → run it to confirm the failure → implement → run `cargo test` + `cargo clippy --all-targets -- -D warnings` + `cargo fmt` → commit. **The crate MUST compile after every task's commit.** Commit with:
> ```
> git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -m "$(printf '<subject>\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
> ```

**Goal:** Turn edaptor from a read-only browser into a directory editor for the **generic (schema-typed) object tier**. Two halves: (A) make the M3 read flow actually reachable in the live TUI — the M3 work built the browser + read form but `main.rs::run_tui` never mounts the outline on the desktop and binds no keys, so the tree/form are unreachable interactively (the known M3 gap). (B) add the write path: a pure attribute **diff → `ChangeSet`** (with RDN-change detection → MODRDN, not MODIFY, per spec §8), an RFC 2849 **LDIF renderer** for the preview (spec §12 F1), worker **`Modify`/`Add`/`ModRdn`/`Delete`** requests that map LDAP result codes to human messages (spec §10), an **editable entry form** with client-side MUST/single-value/syntax validation and an LDIF-preview keystroke, and **Create/Delete** through the browser. After every mutation the entry/tree is **re-read** — no silent success (spec §10).

**Architecture (spec §3 three layers):**
1. **Worker thread** (`src/ldap/worker.rs`) — owns all ldap3 network I/O. M4 adds four write requests mapping to ldap3 `modify`/`add`/`modifydn`/`delete`; each maps the ldap3 result code to a human message before it crosses back to the UI. No ldap3 type leaks past the worker (the `LdapEntry`/`SearchScope` rule).
2. **Domain layer** (pure, fully unit-/golden-tested — the heart of M4): the new `form::changeset` diff and the new `ldap::ldif` renderer, plus client-side validation helpers. No terminal, no network.
3. **TUI layer behind `src/ui/facade.rs`** — the ONLY module that may `use turbo_vision` (boundary rule, spec §8/§14). M4 adds: mounting the outline on the desktop, expand/select keybindings, an editable dialog of `InputLine`s bound to `Rc<RefCell<String>>`, the LDIF-preview modal, confirmations, and result/error message boxes. These are tty-only and **not unit-tested**; their inputs/outputs are pure types produced/consumed by the tested domain layer.

**Tech stack:** Rust 2021, ldap3 0.12 (already wired), anyhow, `turbo-vision = "1.2"` (already a dependency since M3). **No new crate dependency is needed for M4:** the turbo-vision write widgets (editable `InputLine` + `.data()` binding, `Dialog::execute`, `message_box`, validators) were already compile-verified in the M3 spike, and the `ChangeSet`/LDIF work is pure Rust (base64 we hand-roll or via an already-present transitive dep — see Decision D5). All turbo-vision calls MUST match the compile-verified spike `docs/superpowers/research/2026-05-29-turbo-vision-spike.md`.

---

## Context from M1–M3 (real signatures on `feat-m4-write-path` — quoted from source)

These are the **actual** APIs read from the worktree. Where the build brief implies a different shape, the source below wins. Items marked **[verify-at-task-start]** were read from module docs / call-sites rather than the full definition during planning (an intermittent shell-output glitch blocked a few full reads); the implementing worker MUST open the cited file and confirm the exact field list before coding, then correct this plan in-place if it differs.

### `src/main.rs::run_tui` — the wiring to fix (fully read)
The current loop already drains the worker and routes responses, but the **outline is built and dropped** and **no keys are bound**:
```rust
let root = facade::new_node(BrowserNode { dn: base_dn.clone(), label: base_dn, loaded: false, object_classes: Vec::new() });
browser.request_children(&worker, &root)?;
let _outline = facade::build_outline(root);          // <-- built, never mounted on the desktop
let mut shell = Shell::new(&menu_defs)?;
shell.run_loop(|app| {
    while let Some(resp) = worker.poll() {            // non-blocking drain each idle tick
        if let Some((node, kids)) = browser.on_response(&resp) { facade::attach_children(&node, kids); continue; }
        match read_flow.on_response(&resp) {
            ReadOutcome::Form(model) => facade::show_entry_dialog(app, &model),
            ReadOutcome::Error(msg) => facade::confirm_error(app, &msg),
            ReadOutcome::Ignored => {}
        }
    }
});
```
There is no path that calls `browser.request_children` on user expansion, and no path that issues a base `Search` on Enter to feed `read_flow`. Task 1 closes both gaps.

### `src/ui/facade.rs` (fully read — the boundary)
```rust
pub fn tv_available() -> bool;
pub fn tv_cm_quit() -> u16;                                   // mirrors crate::app::CM_QUIT
pub fn build_menu_bar(size_w: i16, defs: &[MenuDef]) -> MenuBar;
pub fn build_status_line(size_w: i16, size_h: i16) -> StatusLine;
pub struct Shell { /* app: Application */ }
impl Shell {
    pub fn new(defs: &[MenuDef]) -> anyhow::Result<Shell>;    // requires tty
    pub fn run_loop(&mut self, mut on_idle: impl FnMut(&mut Application)); // CM_QUIT ends loop
}
pub type BrowserNodeRef = Rc<RefCell<Node<BrowserNode>>>;
impl ExpandableNode for BrowserNodeRef { fn dn(&self)->String; fn mark_loaded(&self); }
pub fn new_node(payload: BrowserNode) -> BrowserNodeRef;
pub fn build_outline(root: BrowserNodeRef) -> OutlineViewer<BrowserNode>;
pub fn attach_children(parent: &BrowserNodeRef, kids: Vec<BrowserNode>);
pub fn build_entry_dialog(model: &FormModel) -> Dialog;       // read-only StaticText rows
pub fn show_entry_dialog(app: &mut Application, model: &FormModel); // dialog.execute(app)
pub fn confirm_error(app: &mut Application, msg: &str);       // message_box(MF_ERROR|MF_OK_BUTTON)
```
Imports already present and reusable in M4: `turbo_vision::views::dialog::Dialog`, `views::button::Button`, `views::static_text::StaticText`, `views::outline::{Node, OutlineViewer}`, `helpers::msgbox::{message_box, MF_ERROR, MF_OK_BUTTON}`, `core::command::{CM_CANCEL, CM_QUIT}`, `core::geometry::Rect`, `core::event::{EventType, KB_ALT_X, KB_F10}`. M4 ADD imports (all spike-verified): `views::input_line::InputLine`, `core::command::CM_OK`, `helpers::msgbox::{MF_CONFIRMATION, MF_YES_BUTTON, MF_NO_BUTTON, MF_INFORMATION}`, `core::command::{CM_YES, CM_NO}`, and (Task 5 validation, optional) `core::validator::{Validator, FilterValidator, RangeValidator, PictureValidator}`.

### `src/ui/form.rs` (FULLY read — exact signatures verified)
```rust
pub enum WidgetSpec { ReadOnlyText, ReadOnlyInt, ReadOnlyDn, ReadOnlyTime, DisabledCheckBox(bool), BinaryNote(usize) }
pub struct FormField {
    pub label: String,     // <-- this IS the attribute name (the facade appends " *" when is_must); there is NO separate attr_name
    pub kind: FieldKind,
    pub is_must: bool,
    pub values: Vec<String>,
    pub widget: WidgetSpec,
}
pub struct FormModel { pub title: String, pub fields: Vec<FormField> }   // title is the DN; there is NO separate `dn` field
pub fn field_widget_spec(kind: FieldKind, value: Option<&str>, byte_count: Option<usize>) -> WidgetSpec;
pub fn build_form_model(schema: &SchemaModel, object_classes: &[&str], entry: &LdapEntry, profile_show: &[String]) -> FormModel;
```
**Key facts the write path must account for:**
- `FormField.label` already holds the attribute name — the changeset/edit form key off `label`. No `attr_name` field exists; do NOT invent one.
- `FormModel.title` is the DN (no `dn` field). The edit/save flow gets the DN from `title` (or, cleaner, pass the DN alongside — see Task 5 Step 0).
- **`single_value` is NOT carried** by `FormField`, and `SchemaModel` does not yet expose it. The M2 `AttributeType` (from `ldap_types::schema`) does carry single-value info; **Task 5 Step 0** adds a `SchemaModel::is_single_value(&self, attr: &str) -> bool` accessor (reading `AttributeType.single_value`, following SUP like `field_kind` does) so validation can enforce single-value. Verify the `AttributeType` field name (`single_value`) when implementing.

### `src/workflows/read_flow.rs` (FULLY read — `request_entry` already exists)
```rust
pub struct ReadFlow { schema: SchemaModel, pending: HashMap<u64, Vec<String>>, next_id: u64 }
pub enum ReadOutcome { Form(FormModel), Error(String), Ignored }
impl ReadFlow {
    pub fn new(schema: SchemaModel) -> Self;
    pub fn request_entry(&mut self, worker: &WorkerHandle, dn: &str, profile: Option<&EntryProfile>) -> Result<u64>;
        // already submits Request::Search { scope: Base, filter "(objectClass=*)", attrs ["*"] } and returns the id
    pub fn on_response(&mut self, resp: &Response) -> ReadOutcome;   // matches Response::Entries / SearchError by id
    pub fn form_for(&self, entry: &LdapEntry, profile_show: &[String]) -> FormModel;  // exposed for tests
}
```
`request_entry` exists with this exact signature — Task 1 just CALLS it on Enter (passing the active profile or `None` for the generic tier). The re-read after a write (Task 5/6) also calls `request_entry`. main.rs already constructs `ReadFlow::new(schema)` and routes `on_response` in the idle loop; Task 1 only needs to drive `request_entry` from a selection event.

### `src/workflows/browser.rs` (FULLY read — exact signatures verified)
```rust
pub struct BrowserNode { pub dn: String, pub label: String, pub loaded: bool, pub object_classes: Vec<String> }
pub trait ExpandableNode { fn dn(&self) -> String; fn mark_loaded(&self); }
pub fn node_label(entry: &LdapEntry, rdn_fallback: &str) -> String;     // cn -> description -> RDN
pub fn entries_to_nodes(entries: &[LdapEntry]) -> Vec<BrowserNode>;     // unloaded children w/ labels + objectClasses
pub struct BrowserState<N: ExpandableNode + Clone> { pending: HashMap<u64, N>, next_id: u64, pub base_dn: String }
impl<N: ExpandableNode + Clone> BrowserState<N> {
    pub fn new(base_dn: impl Into<String>) -> Self;
    pub fn request_children(&mut self, worker: &WorkerHandle, node: &N) -> Result<u64>;  // one-level submit; returns id
    pub fn on_response(&mut self, resp: &Response) -> Option<(N, Vec<BrowserNode>)>;      // marks parent loaded; (parent, children)
}
```
**Verified facts:** the bound is `N: ExpandableNode + Clone`. `request_children` does **NOT** check `loaded` — it submits unconditionally every time it's called. Therefore Task 1's `SelectAction` classifier (not `BrowserState`) is responsible for only expanding unloaded nodes, and Task 6's refresh works by `clear_children` (reset `loaded=false`, empty children) then calling `request_children` again. `BrowserState` has no `invalidate` method and (given `request_children` ignores `loaded`) does not strictly need one — Task 6 Step 0 may add a tiny `BrowserState::reset(&mut self)` only if pending-map hygiene requires it; otherwise the facade-side `clear_children` plus a fresh `request_children` suffices.

### `src/ldap/worker.rs` (FULLY read — exact signatures verified)
```rust
pub enum SearchScope { Base, OneLevel }                       // mapped to ldap3::Scope only inside the worker
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LdapEntry {
    pub dn: String,
    pub attrs: BTreeMap<String, Vec<String>>,                 // string-valued
    pub bin_attrs: BTreeMap<String, usize>,                   // binary -> total byte count (blobs NOT copied)
}
pub enum Request {
    FetchSubschema,
    Search { id: u64, base: String, scope: SearchScope, filter: String, attrs: Vec<String> },
    Shutdown,
    // M4 ADDS (follow the same `id` correlation + non-blocking submit/poll convention):
    //   Modify { id: u64, dn: String, changes: Vec<ModOp> }
    //   Add    { id: u64, dn: String, attrs: BTreeMap<String, Vec<String>> }
    //   ModRdn { id: u64, dn: String, new_rdn: String, delete_old: bool, new_superior: Option<String> }
    //   Delete { id: u64, dn: String }
}
pub enum Response {
    Subschema(RawSubschema),
    Entries { id: u64, entries: Vec<LdapEntry> },             // NOTE: plural "Entries", carries a Vec
    SearchError { id: u64, msg: String },                     // NOTE: field is `msg`, not `message`
    Done,
    Error(String),
    // M4 ADDS:  WriteOk { id: u64, dn: String }  and  WriteError { id: u64, msg: String }
    //           (msg already human-mapped from the ldap3 result code by result_code_message)
}
type Job = (Request, Sender<Response>);                       // private
pub struct WorkerHandle { /* tx: Sender<Job>, resp_tx, resp_rx, join */ }
impl WorkerHandle {
    pub fn spawn(config: Config, password: String) -> Result<WorkerHandle>;  // connects + binds synchronously
    pub fn request(&self, req: Request) -> Result<Response>;   // synchronous (fresh reply channel) — used for FetchSubschema
    pub fn submit(&self, req: Request) -> Result<()>;          // non-blocking (clone of long-lived resp_tx) — used for Search; use for writes
    pub fn poll(&self) -> Option<Response>;                    // non-blocking drain (try_recv)
}
// worker_loop matches on Request; each Search arm calls run_search and replies Entries/SearchError.
// Task 4 adds Modify/Add/ModRdn/Delete arms that call ldap3 modify/add/modifydn/delete and reply WriteOk/WriteError.
```
Write requests MUST go through `submit`/`poll` (non-blocking) so the UI never blocks, and MUST echo the request `id` in the reply (same correlation convention as the read path). The existing `submit_then_poll_roundtrip` test pattern (constructs a `WorkerHandle` over hand-made channels) is the template for a unit test that pushes a `WriteOk`/`WriteError` and polls it.

### `src/app.rs`, `src/config/mod.rs`, `src/schema/model.rs` (FULLY read — exact signatures verified)
```rust
// app.rs
pub const CM_QUIT: u16 = 24;          // mirror of turbo_vision CM_QUIT (asserted equal by facade test)
pub const CM_BROWSER: u16 = 1000;
pub const CM_PROFILE_BASE: u16 = 1100;   // profile i -> CM_PROFILE_BASE + i
pub struct MenuDef { pub label: String, pub command: u16 }
pub fn build_menu_defs(profiles: &[EntryProfile]) -> Vec<MenuDef>;  // one per profile, then "Browser", then "Quit"

// config/mod.rs
pub struct Config { pub server: ServerConfig, pub auth: AuthConfig, pub profiles: Vec<EntryProfile> }
// ServerConfig { uri, base_dn: String, start_tls, timeout_secs, tls } ; AuthConfig.password_source.resolve() -> Result<String>
#[derive(Default, Clone)]
pub struct EntryProfile {
    pub name: String,            // menu/display name
    pub object_class: String,    // SINGLE structural objectClass string (NOT a Vec)
    pub rdn_attr: String,        // the RDN attribute for ADD (e.g. "uid"); may be empty
    pub search_base: String,     // container DN; may be empty
    pub show: Vec<String>,       // field ordering for the form
}

// schema/model.rs
pub struct SchemaModel { /* parsed OCs/ATs, name indexes, pub warnings */ }
pub struct ResolvedAttributes { pub must: BTreeSet<String>, pub may: BTreeSet<String> }
impl SchemaModel {
    pub fn from_raw(raw: &RawSubschema) -> SchemaModel;
    pub fn object_class(&self, name: &str) -> Option<&ObjectClass>;
    pub fn attribute_type(&self, name: &str) -> Option<&AttributeType>;
    pub fn effective_attributes(&self, object_classes: &[&str]) -> ResolvedAttributes;  // walks SUP; MUST wins over MAY
    pub fn field_kind(&self, attr_name: &str) -> FieldKind;   // follows SUP to first SYNTAX; default Text
    // Task 5 Step 0 ADDS: pub fn is_single_value(&self, attr_name: &str) -> bool  (reads AttributeType single-value, SUP-followed)
}
```
**Verified facts driving Decision D2/D3:** `EntryProfile.object_class` is a **single** `String` (one structural class) — so ADD's objectClass set is `["top", &profile.object_class]` plus whatever superclasses the server infers; no Vec, no picker needed in M4 (D2 = fixed). `EntryProfile.rdn_attr` gives ADD its RDN attribute directly. `effective_attributes` returns `BTreeSet`s of canonical names — reuse for both the ADD form's MUST/MAY and Task 5 validation. `is_single_value` does not exist yet (added in Task 5 Step 0).

---

## Decisions to confirm before execution (lead: please resolve D1–D6)

- **D1 — where the `form/` module lives.** The brief says NEW `src/form/changeset.rs`. But the existing form code is `src/ui/form.rs` (under `ui`), and `ReadFlow` imports `crate::ui::form`. Two options: **(a)** put the write-path pure modules under `src/ui/` next to `form.rs` (e.g. `src/ui/changeset.rs`); **(b)** create a new top-level `src/form/` dir (`mod.rs` + `changeset.rs` + `validate.rs`). **This plan assumes (b)** because the changeset/validate logic is pure domain logic that should not sit under `ui` (which is conceptually the presentation boundary). Note `EditEntry` (the shared pure edit type) lives in `form/changeset.rs` and is imported by the facade + worker + read/save flow. If the lead prefers (a), only the module-path strings change; the type names and tests are identical.
- **D2 — ADD objectClass strategy in M4.** RESOLVED by source: `EntryProfile.object_class` is a single structural-class `String`. Create builds the new entry's objectClass values as `["top", profile.object_class]` (the server fills in inherited superclasses), pulling MUST/MAY from `schema.effective_attributes(&[&profile.object_class])`. **No objectClass picker in M4** (deferred). Lead only needs to confirm the `["top", structural]` pairing is acceptable for the target directory (OpenLDAP accepts it; auxiliary classes are M5+).
- **D3 — RDN-change ↔ edit dialog interaction.** When the user edits the RDN attribute's value, the changeset must emit a **MODRDN** (new_rdn from the new value), and must NOT also emit a MODIFY replace for that attribute (OpenLDAP updates the RDN attribute as part of MODRDN). This plan: `diff()` detects RDN change by comparing the leftmost RDN component of `original.dn` vs. the edited value of the RDN attribute, emits a single `ChangeSet { modrdn: Some(..), mods: [.. excluding the RDN attr ..] }`. Confirm `delete_old_rdn = true` is the desired default (it is, per spec §8 rename=MODRDN). Multi-valued RDNs (e.g. `cn=x+uid=y`) are **out of M4** — detect and refuse with a clear message.
- **D4 — re-read after write granularity.** Spec §10 "no silent success / re-read after write": after MODIFY, re-read the **entry** (base Search) and rebuild the form. After ADD/DELETE/MODRDN, the **tree** changes — re-read the affected **container's children** (Task 6 invalidate+reload). Confirm we re-read rather than optimistically patching local state (this plan re-reads).
- **D5 — base64 for LDIF.** RFC 2849 requires base64 for values that aren't "safe" (leading space/`:`/`<`, non-ASCII, contains NUL/CR/LF) and for binary attrs. Do we add a tiny dependency (`base64` crate) or hand-roll a ~20-line encoder? This plan **hand-rolls** `fn b64(bytes: &[u8]) -> String` in `ldap::ldif` to avoid a new dependency (brief says "no new crate dependency"). Note: M4's `LdapEntry` only carries binary attrs as **byte counts**, not bytes — so the LDIF preview renders binary values as a `:: <N bytes, not shown>` placeholder comment, NOT real base64. Real binary write is out of M4 scope (flag in Notes).
- **D6 — keybindings for expand/select.** turbo-vision `OutlineViewer` selection + Enter handling: confirm the spike's outline section covers focus/Enter. This plan binds **Enter on a leaf entry → base read → form**, and **expand (Right-arrow / Enter on a container) → one-level Search**. If `OutlineViewer` already emits an expand event we hook that; otherwise we intercept the key in `run_loop`/facade. tty-only; verify against the spike's outline + event sections at Task 1.

---

## Task 1 — Mount the read flow in the live UI (make M3 reachable)

**Why first:** the read flow is built but unreachable (the M3 gap). Wiring it now makes the app usable and gives every later write task a live surface to hang off. No new dependency. Most of this task is tty-only facade work; the *testable* part is the pure routing/selection helpers we extract.

- [ ] **Step 1 (failing test):** In `src/workflows/browser.rs` (or a new pure helper), add a unit test `selecting_container_requests_children_once` asserting that a helper `on_select(node) -> SelectAction` returns `SelectAction::Expand(dn)` for an unloaded container and `SelectAction::None` for an already-loaded one; and `selecting_entry_requests_base_read` asserting a leaf returns `SelectAction::Read(dn)`. Introduce a pure `enum SelectAction { Expand(String), Read(String), None }` and a pure classifier (containers vs. leaves: by `object_classes`, or "has children / structural class" — **[verify]** what data distinguishes a container from a leaf in `BrowserNode`; if insufficient, treat "unloaded" as expandable and "loaded with no children" as leaf, OR always allow both expand and read). Run `cargo test` → confirm it fails.
- [ ] **Step 2 (implement pure logic):** Implement `SelectAction` + classifier in `src/workflows/browser.rs`. Keep it terminal-free. `cargo test` passes for the new tests.
- [ ] **Step 3 (facade wiring, tty-only):** In `src/ui/facade.rs`: (a) add `pub fn mount_outline(shell or app, root: BrowserNodeRef) -> ...` that inserts the `OutlineViewer` into the `Application` desktop (`app.desktop.insert(..)` per spike §6/§1 — **verify exact insert API**), sizing it as the left pane; (b) expose a way for `run_loop`'s idle/event hook to learn the **currently selected node** and the **key pressed** so `main.rs` can call the `SelectAction` classifier. Because `run_loop` currently only forwards `on_idle(&mut Application)`, EITHER extend it with an `on_event(&mut Application, &Event) -> ()` hook OR have the facade own a small `BrowserNodeRef` + emit selection via a returned enum. Document the chosen shape; keep ALL `turbo_vision` use inside facade. Mark these functions "Not tty-testable" in their doc comments (matching the existing convention).
- [ ] **Step 4 (rewire `main.rs::run_tui`):** Replace `let _outline = build_outline(root)` with a real mount; on a select/expand event, call the classifier and then `browser.request_children(&worker, &node)` (Expand) or `read_flow.request_entry(&worker, &dn)` (Read). Keep the existing poll-drain loop. Crate compiles; manual smoke (operator runs `cargo run`) shows: tree expands lazily, Enter on an entry opens the read-only form.
- [ ] **Step 5 (headless smoke test):** Add a `#[test]` that constructs the browser + read_flow + a fake/echo worker (reuse the M3 test scaffolding pattern) and drives a one-level + base round-trip through `on_response`, asserting the form model is produced — proving the pure path below the facade still works. `cargo test` + `cargo clippy --all-targets -- -D warnings` + `cargo fmt`.
- [ ] **Step 6 (commit):**
  `git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -m "$(printf 'M4: mount DIT outline on desktop + wire expand/select keybindings\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"`

**Honesty note:** Steps 3–4 are **tty-only** (real terminal; not unit-tested) — exactly like the existing `Shell`/`build_outline`. The testable contract is the `SelectAction` classifier (Step 1–2) and the headless round-trip (Step 5).

---

## Task 2 — `form::changeset`: pure diff → `ChangeSet` (with RDN-change detection)

**The testable heart of M4.** Pure Rust, strict TDD, no terminal, no network.

- [ ] **Step 0:** Create `src/form/mod.rs` (declares `pub mod changeset;`) and register `pub mod form;` in `src/lib.rs` (**[verify]** lib.rs module list). Crate still compiles (empty module).
- [ ] **Step 1 (types):** In `src/form/changeset.rs` define:
  ```rust
  pub enum ModOp { Add { attr: String, values: Vec<String> }, Delete { attr: String, values: Vec<String> /* empty = delete whole attr */ }, Replace { attr: String, values: Vec<String> } }
  pub struct ModRdn { pub new_rdn: String, pub delete_old: bool /* default true */, pub new_superior: Option<String> /* None in M4 */ }
  pub struct ChangeSet { pub dn: String, pub modrdn: Option<ModRdn>, pub mods: Vec<ModOp> }
  impl ChangeSet { pub fn is_empty(&self) -> bool; }
  pub fn diff(original: &EditEntry, edited: &EditEntry) -> ChangeSet;
  ```
  where `EditEntry { dn: String, attrs: BTreeMap<String, Vec<String>> }` is a small pure type defined in `form::changeset`. **`form::changeset` MUST NOT import anything from `ldap::worker`** (do NOT reuse `LdapEntry`). This is non-negotiable: today `ldap::worker` is imported BY `form`/`workflows`, and Task 4 makes `ldap::worker` import `ModOp` FROM `form::changeset` — if `changeset` also imported `LdapEntry` from the worker, that closes a module cycle. Keeping `EditEntry` self-contained makes the dependency strictly one-directional (`worker → changeset`). The caller (read/save flow) converts an `LdapEntry` into an `EditEntry` at the boundary. Decide the RDN-attribute source: parse the leftmost RDN component of `original.dn` (e.g. `cn=Alice` → attr `cn`, value `Alice`). Add `pub fn rdn_component(dn: &str) -> Option<(String, String)>`.
- [ ] **Step 2 (failing tests — write ALL, run, confirm fail):**
  - `diff_no_change_is_empty` — identical entries ⇒ `is_empty()`.
  - `diff_added_value_emits_add` — new value on an existing attr ⇒ `ModOp::Add`.
  - `diff_removed_value_emits_delete` — value removed ⇒ `ModOp::Delete` with that value.
  - `diff_new_attr_emits_add` / `diff_cleared_attr_emits_delete_whole` (empty values).
  - `diff_changed_single_value_emits_replace` — single-valued attr changed ⇒ `Replace`.
  - `rdn_component_parses_simple` — `cn=Alice,ou=people,dc=x` ⇒ `("cn","Alice")`.
  - `diff_rdn_change_emits_modrdn_not_modify` — edited `cn` value differs from RDN ⇒ `modrdn = Some{ new_rdn: "cn=Bob", delete_old: true, new_superior: None }` AND no `Replace`/`Add`/`Delete` for `cn`.
  - `diff_rdn_unchanged_no_modrdn` — `cn` edited elsewhere but RDN value identical ⇒ no modrdn.
  - `diff_multivalued_rdn_is_refused` — `cn=x+uid=y` ⇒ documented refusal (return an error or a sentinel; choose `diff` returning `Result<ChangeSet, ChangeSetError>` with `MultiValuedRdnUnsupported`).
  Run `cargo test` → confirm all fail.
- [ ] **Step 3 (implement):** Implement `diff`/`rdn_component`. Case-insensitive attribute-name matching is NOT required here (operate on the names as given by the form), but RDN attribute matching IS case-insensitive (LDAP). Re-run → all green.
- [ ] **Step 4 (gate + commit):** `cargo test` + `cargo clippy --all-targets -- -D warnings` + `cargo fmt`.
  `git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -m "$(printf 'M4: form::changeset diff with RDN-change detection (MODRDN vs MODIFY)\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"`

---

## Task 3 — `ldap::ldif`: render `ChangeSet` + full ADD as RFC 2849 LDIF (golden-file tests)

Pure Rust, **golden-file tests** (spec §11). No terminal, no network.

- [ ] **Step 1 (module):** Create `src/ldap/ldif.rs`; add `pub mod ldif;` to `src/ldap/mod.rs` (**[verify]**). Define:
  ```rust
  pub fn render_changeset(cs: &ChangeSet) -> String;     // changetype: modify / modrdn
  pub fn render_add(dn: &str, attrs: &BTreeMap<String, Vec<String>>) -> String; // changetype: add
  fn ldif_line(attr: &str, value: &str) -> String;       // base64-encodes unsafe values: "attr:: <b64>"
  fn is_safe_value(v: &str) -> bool;                     // RFC 2849 safe-string rules
  fn b64(bytes: &[u8]) -> String;                        // hand-rolled, no new dep (Decision D5)
  ```
  LDIF shape: `dn: <dn>` then `changetype: modify`, then per `ModOp` a stanza (`add: attr` / `delete: attr` / `replace: attr` + value lines + `-` separator); for MODRDN: `changetype: modrdn` / `newrdn: <rdn>` / `deleteoldrdn: 1` / optional `newsuperior:`. Wrap long lines per RFC 2849 (76 cols, continuation = leading space) — **or** explicitly choose NOT to wrap for preview readability and document it (this plan: **do not wrap** in the preview; note it). Binary attrs render as `attr:: <N bytes, not shown>` comment placeholder (Decision D5).
- [ ] **Step 2 (golden fixtures + failing tests):** Create `tests/golden/ldif/` (or `src/ldap/testdata/`) with expected `.ldif` files: `modify_simple.ldif`, `modify_add_delete_replace.ldif`, `modrdn.ldif`, `add_entry.ldif`, `base64_value.ldif` (a value with a leading space and a UTF-8 value). Write tests `golden_modify_simple`, `golden_mixed_ops`, `golden_modrdn`, `golden_add`, `golden_base64` that build the `ChangeSet`/attrs in-code and `assert_eq!(render_*(..), include_str!("golden/...").trim_end())`. Run → fail (files exist, renderer is a stub).
  - Sub-test `is_safe_value_*` and `b64_known_vectors` (RFC 4648 vectors: `""→""`, `"f"→"Zg=="`, `"fo"→"Zm8="`, `"foo"→"Zm9v"`, `"foob"→"Zm9vYg=="`, `"fooba"→"Zm9vYmE="`, `"foobar"→"Zm9vYmFy"`).
- [ ] **Step 3 (implement):** Implement renderer + safe-string + base64. Iterate until goldens match. (If a golden is wrong, fix the FIXTURE only when the renderer output is demonstrably RFC-correct.)
- [ ] **Step 4 (gate + commit):** `cargo test` + `cargo clippy --all-targets -- -D warnings` + `cargo fmt`.
  `git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -m "$(printf 'M4: ldap::ldif RFC 2849 renderer for ChangeSet + ADD (golden-file tested)\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"`

---

## Task 4 — Worker write requests (`Modify`/`Add`/`ModRdn`/`Delete`) + result-code → human message

All I/O stays on the worker thread. Non-blocking (`submit`/`poll`) with id correlation, matching the read path.

> **Layering note:** `Request::Modify` carries `changes: Vec<ModOp>`, and `ModOp` is defined in `form::changeset` (Task 2). This means `ldap::worker` imports `ModOp` from `form::changeset` — **this is the one allowed domain→worker type dependency**; do NOT duplicate the `ModOp` enum inside the worker. The facade-boundary rule (only `ui::facade` imports `turbo_vision`) is unaffected: `ModOp` is a pure domain type with no TV or ldap3 content. The worker translates `ModOp` → ldap3 `Mod` privately inside its match arms.

- [ ] **Step 0 (verify):** Read `src/ldap/worker.rs` fully; confirm the exact `Request`/`Response` variants and the `Job`/reply plumbing. Confirm the ldap3 method shapes on `LdapConn`: `modify(dn, mods)`, `add(dn, attrs)`, `modifydn(dn, new_rdn, delete_old, new_superior)`, `delete(dn)` and that each returns an `LdapResult` with an `rc` (result code) + `text`.
- [ ] **Step 1 (failing test — pure mapper):** Add a pure `pub fn result_code_message(rc: u32, text: &str) -> String` (in `worker.rs` or a small `src/ldap/result.rs`) that maps the LDAP result codes edaptor cares about (spec §10) to human messages, e.g. `0 → Ok`(handled as success upstream, but mapper covers it), `32 No such object`, `68 Entry already exists`, `19 Constraint violation`, `50 Insufficient access rights`, `65 Object class violation`, `64 Naming violation`, `16 No such attribute`, `20 Attribute or value exists`, `66 Not allowed on non-leaf (entry has children)`, fallback `"LDAP error <rc>: <text>"`. Tests: `maps_no_such_object`, `maps_not_allowed_on_non_leaf`, `maps_insufficient_access`, `maps_objectclass_violation`, `unknown_code_falls_back_to_text`. Run → fail.
- [ ] **Step 2 (implement mapper):** Implement `result_code_message`; tests green. This is the only **unit-testable** part of Task 4 (the network calls aren't, but are covered by the live test in Task 7).
- [ ] **Step 3 (extend protocol):** Add to `Request`: `Modify { id, dn, changes: Vec<ModOp> }`, `Add { id, dn, attrs: BTreeMap<String, Vec<String>> }`, `ModRdn { id, dn, new_rdn, delete_old: bool, new_superior: Option<String> }`, `Delete { id, dn }`. Add to `Response`: `WriteOk { id, dn }` and `WriteError { id, msg }` (field named `msg` for consistency with `SearchError`; already run through `result_code_message`). Convert `ModOp` (from `form::changeset`) to ldap3's `Mod` **inside the worker only** (so `ldap3` doesn't leak). Map an ldap3 success-but-nonzero `rc` to `WriteError`; map transport/`io` errors to `WriteError` too. Crate compiles.
- [ ] **Step 4 (gate + commit):** `cargo test` + `cargo clippy --all-targets -- -D warnings` + `cargo fmt`.
  `git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -m "$(printf 'M4: worker Modify/Add/ModRdn/Delete requests + result-code human messages\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"`

---

## Task 5 — Editable entry form + save flow (validate → LDIF preview → MODIFY/MODRDN → re-read)

The interactive write surface. Pure parts (validation, building the `EditEntry` from form data, deciding the worker request from the `ChangeSet`) are TDD-tested; the editable dialog itself is tty-only.

- [ ] **Step 0 (schema single-value accessor + DN carrying):** `FormField.label` already IS the attribute name and `FormModel.title` is the DN — do NOT add `attr_name`/`dn` fields. Instead: (a) add `pub fn is_single_value(&self, attr_name: &str) -> bool` to `SchemaModel` (read `AttributeType.single_value`, SUP-followed exactly like `field_kind`; verify the `ldap_types` field name). **Fallback if the `ldap_types` `AttributeType` does not expose single-value cleanly:** scan the raw definition string for the `SINGLE-VALUE` token (the M2 raw strings are still available via `RawSubschema`, or keep a side index at `from_raw`) — do NOT block Task 5 on the parsed-field shape. Unit tests `single_value_attr_is_flagged` / `multi_value_attr_is_not`; (b) the save flow takes the DN from `FormModel.title` (the RDN attribute is found via `changeset::rdn_component(&title)`). `cargo test` still green.
- [ ] **Step 1 (failing tests — pure validation):** Add `src/form/validate.rs` with:
  ```rust
  pub enum ValidationError { MissingMust(String), MultiValueOnSingle(String), SyntaxInvalid { attr: String, reason: String } }
  // Validation needs THREE things the `ResolvedAttributes` set alone does not carry — single-value flags
  // and FieldKind both live on `SchemaModel`. So pass the schema + objectClasses, not a synthetic struct:
  pub fn validate(edited: &EditEntry, schema: &SchemaModel, object_classes: &[&str]) -> Vec<ValidationError>;
  ```
  Internally `validate` calls `schema.effective_attributes(object_classes)` (the verified `ResolvedAttributes { must, may }`) for the MUST check, `schema.is_single_value(attr)` (added in Step 0) for the single-value check, and `schema.field_kind(attr)` (verified) for the per-kind syntax check. Do NOT invent an `EffectiveAttrs` type — it would duplicate `ResolvedAttributes` and still lack single-value/kind. Tests: `missing_must_attr_flagged`, `empty_must_attr_flagged`, `second_value_on_single_valued_flagged`, `integer_syntax_rejects_non_numeric`, `dn_syntax_rejects_garbage` (syntax checks limited to the FieldKinds M2 classifies — Int/Dn/Time/Text; Text always valid), `valid_entry_has_no_errors`. Run → fail.
- [ ] **Step 2 (implement validation):** Implement `validate` reusing the M2 schema lookup. Green.
- [ ] **Step 3 (failing test — request selection):** Add a pure `pub fn plan_save(cs: ChangeSet) -> SavePlan` where `enum SavePlan { Nothing, Modify(Vec<ModOp>), Rename { modrdn: ModRdn, then_mods: Vec<ModOp> }, RenameOnly(ModRdn) }`. Tests: `empty_changeset_is_nothing`, `mods_only_is_modify`, `rdn_only_is_rename_only`, `rdn_plus_mods_is_rename_then_modify`. (This encodes Decision D3: when both apply, MODRDN first, then MODIFY the rest.) Run → fail → implement → green.
- [ ] **Step 4 (editable dialog — tty-only):** In `src/ui/facade.rs` add `pub fn edit_entry_dialog(app, model: &FormModel) -> Option<EditEntry>`: build a `Dialog` of editable `InputLine`s, each bound to an `Rc<RefCell<String>>` via `.data(..)` (spike §3), seeded from `field.values`. Buttons: `~S~ave` (CM_OK), `~C~ancel` (CM_CANCEL), and `~P~review LDIF` (a custom command id `CM_PREVIEW`). On `execute(app)`: if returns `CM_OK`, read each binding back via `data.borrow().clone()`, assemble an `EditEntry`, return `Some`; if `CM_CANCEL`, `None`; if `CM_PREVIEW`, show the LDIF modal then re-loop the dialog (see Step 5). DisabledCheckBox booleans become an editable checkbox cluster (spike) or a `[y/n]` InputLine — choose one and document. Read-only/binary fields render as StaticText (not editable). Mark "Not tty-testable".
- [ ] **Step 5 (LDIF preview modal — tty-only):** `pub fn show_ldif_preview(app, ldif: &str)` shows the rendered LDIF (from `ldap::ldif::render_changeset`) in a scrollable modal (StaticText or a read-only memo; spike §2/§8) with an OK button. Wire `CM_PREVIEW` in `edit_entry_dialog` to: build the current `EditEntry` from the live bindings → `changeset::diff` → `ldif::render_changeset` → `show_ldif_preview`. (Spec §12 F1: "show exactly what will be sent".)
- [ ] **Step 6 (save orchestration in `main.rs`/read_flow):** On dialog `Save`: run `validate`; if errors, show them via `confirm_error` (or a dedicated message box listing each) and re-open the dialog without sending. If valid: `diff` → `plan_save` → `message_box(MF_CONFIRMATION|MF_YES_BUTTON|MF_NO_BUTTON)` confirm; on `CM_YES` `submit` the `Modify`/`ModRdn` request(s) with a fresh correlation id. On `poll` receiving `WriteOk { id, dn }` for that id → **re-read** the entry (base `Search`, refresh the form) and surface a success `message_box(MF_INFORMATION|MF_OK_BUTTON)`; on `WriteError { msg }` → `confirm_error`. (Spec §10: no silent success.) **Rename (MODRDN) DN handling:** after a MODRDN the entry's DN changes, so the new DN is `new_rdn + "," + container` (container = everything after the leftmost comma of the old DN). The re-read MUST use this **new** DN, and the visible tree node must be updated too. The facade currently exposes only `mark_loaded` on a node — **Task 5 Step 0a (add to facade): `pub fn rename_node(node: &BrowserNodeRef, new_dn: String, new_label: String)`** that sets `node.borrow_mut().data.dn`/`.label` (tty-adjacent, not unit-tested). Without it the rename refresh cannot update the displayed label. Simpler alternative if a node handle isn't in scope at save time: `clear_children` + `request_children` on the **parent** container so the renamed child reappears with its new DN/label (this also covers it, at the cost of re-listing siblings).
- [ ] **Step 7 (gate + commit):** `cargo test` + `cargo clippy --all-targets -- -D warnings` + `cargo fmt`.
  `git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -m "$(printf 'M4: editable entry form with validation, LDIF preview, save + re-read\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"`

**Honesty note:** Steps 4–5 (and the dialog/message-box parts of 6) are **tty-only**. The decision logic (validate, diff, plan_save, request selection) is fully unit-tested in Steps 1–3.

---

## Task 6 — Create (ADD) + Delete (with confirm) through the browser

Both mutate the tree; both **re-read** the affected container after success (Decision D4).

- [ ] **Step 0 (tree-refresh infra):** Since `BrowserState::request_children` already submits unconditionally (it does NOT check `loaded`), refresh only needs to clear the facade-side children. Add a facade `pub fn clear_children(parent: &BrowserNodeRef)` (empties `parent.borrow_mut().children` and resets `data.loaded=false`) — used after add/delete/rename to force a fresh one-level `Search` via a subsequent `request_children`. This is tty-adjacent (touches the concrete `Node`), so it lives in the facade and is not unit-tested; the pure refresh contract is covered by the live test in Task 7. (No `BrowserState::invalidate` is needed; add one only if pending-map hygiene proves necessary during implementation.)
- [ ] **Step 1 (failing test — build ADD entry):** Pure `pub fn build_add_entry(profile: &EntryProfile, container_dn: &str, rdn_value: &str, edited: &EditEntry) -> (String /*dn*/, BTreeMap<String, Vec<String>>)` that composes the new DN (`<rdnattr>=<rdn_value>,<container_dn>`) and merges the profile's fixed objectClass set (Decision D2) with the edited attrs. Tests: `build_add_composes_dn`, `build_add_includes_objectclasses`, `build_add_includes_must_attrs`. Run → fail → implement → green.
- [ ] **Step 2 (ADD flow — tty-driven):** Menu/keybinding "New <profile>" (the menu is already profile-derived via `build_menu_defs`) opens the **schema-driven editable form** (reuse Task 5's `edit_entry_dialog`) seeded empty from the profile's effective MUST/MAY. On Save: `validate` → confirm → preview available (reuse `render_add`) → `submit` `Request::Add`. On `WriteOk`: `clear_children` the chosen container node + `browser.request_children` (re-read), surface success. Container chosen = the currently selected container node (or base_dn). Document tty-only parts.
- [ ] **Step 3 (DELETE flow — tty-driven, spec §12 F2):** Keybinding/menu "Delete" on the selected entry → `message_box(MF_CONFIRMATION|MF_YES_BUTTON|MF_NO_BUTTON)` showing the DN; on `CM_YES` `submit` `Request::Delete { id, dn }`. On `WriteOk`: `clear_children` + reload the **parent** container; surface success. On `WriteError` (e.g. rc 66 "Not allowed on non-leaf") → `confirm_error` with the human message from Task 4. (refint/cascade is server-side per spec §8 — edaptor does not cascade; it surfaces the server's result.)
- [ ] **Step 4 (gate + commit):** `cargo test` + `cargo clippy --all-targets -- -D warnings` + `cargo fmt`.
  `git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -m "$(printf 'M4: Create (ADD) + Delete (confirm) through browser with tree re-read\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"`

---

## Task 7 — Live integration test against containerized OpenLDAP

Env-gated; **SKIP cleanly** without `EDAPTOR_TEST_LDAP_URI`. Exercises the real worker + changeset + ldif path end to end.

- [ ] **Step 1 (harness check):** Confirm `scripts/test-ldap.sh` brings up OpenLDAP via **podman** (not docker) at `ldap://localhost:1389`, base `dc=example,dc=org`, admin `cn=admin,dc=example,dc=org` / `adminpassword`. (**[verify]** the script exists from M1/M2; if it's docker-only, add a podman path.) Reuse the existing M2/M3 live-test gating pattern (the `EDAPTOR_TEST_LDAP_URI` env guard).
- [ ] **Step 2 (failing test):** Add `tests/live_write.rs` with `#[test] fn add_modify_modrdn_delete_round_trip()` gated on `std::env::var("EDAPTOR_TEST_LDAP_URI")` (return early/skip if unset, printing a skip note). Steps inside: spawn `WorkerHandle`; **ADD** `cn=edaptor-it,ou=people,dc=example,dc=org` (objectClass inetOrgPerson + sn) via `Request::Add`; assert `WriteOk`; **base Search** asserts it exists; **MODIFY** (build `EditEntry`, `diff`, `submit Modify`) change `description`; re-read asserts new value; **MODRDN** rename `cn` → `edaptor-it2`, assert `WriteOk` + base Search on the new DN succeeds and old DN is gone; **DELETE** the entry, assert `WriteOk` + base Search now returns no-such-object. Cleanup in a teardown (delete by either DN) so reruns are idempotent. Run → fails (or skips if no env).
- [ ] **Step 3 (make it pass):** With the container up and `EDAPTOR_TEST_LDAP_URI` set, run the test; fix any protocol mismatches. Confirm the **mapped human messages** appear on expected failures (e.g. delete a non-leaf to see rc 66) in an additional `delete_non_leaf_reports_human_error` test.
- [ ] **Step 4 (gate + commit):** `cargo test` (with and without the env var, to prove clean skip) + `cargo clippy --all-targets -- -D warnings` + `cargo fmt`.
  `git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -m "$(printf 'M4: live integration test add/modify/modrdn/delete round-trip (env-gated)\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"`

---

## Definition of Done

- The live TUI mounts the DIT outline; expanding a container lazily one-level-searches; Enter on an entry opens the read-only form (M3 gap closed).
- `form::changeset::diff` produces correct per-attribute add/delete/replace ops AND detects an RDN-attribute change as a **MODRDN** (not a MODIFY), refusing multi-valued RDNs with a clear message. Fully unit-tested.
- `ldap::ldif` renders `ChangeSet` and full ADD as RFC 2849 LDIF (modify/modrdn/add changetypes, safe-string + base64 handling), proven by **golden-file tests**.
- The worker handles `Modify`/`Add`/`ModRdn`/`Delete` (non-blocking, id-correlated, all I/O on the worker thread) and maps LDAP result codes to human messages (unit-tested mapper).
- The editable entry form validates MUST/single-value/syntax **client-side before send**, offers an **LDIF-preview keystroke** showing exactly what will be sent, confirms, sends MODIFY (or MODRDN+MODIFY for renames), and **re-reads** the entry on success — no silent success. Decision logic unit-tested; dialog tty-only.
- Create (ADD, schema-driven form in a chosen container) and Delete (with confirmation) work through the browser and **re-read** the affected container.
- A live, env-gated integration test round-trips add→modify→modrdn→delete and **SKIPs cleanly** without `EDAPTOR_TEST_LDAP_URI`.
- After **every** task commit: `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` all pass. Only `src/ui/facade.rs` imports `turbo_vision`.

## Notes / scope

- **Out of M4 (do not implement):** Samba (M5); the users/groups DOMAIN tier — password action, membership dual-pane, Samba-enable (M5/M6); richer entry-profile metadata beyond what the write path needs.
- **Deferred but noted:** paged-results / large-list incremental filter (the one-level Search is unpaged in M4 — fine for typical containers; flag large containers as an M5+ concern). LDIF **line-wrapping** is intentionally omitted from the preview for readability (re-add per RFC 2849 if a real LDIF *export* is ever needed). **Real binary attribute writes** are out of M4: `LdapEntry` carries binary attrs as byte counts only, so the editable form treats them as read-only and the LDIF preview shows a `<N bytes, not shown>` placeholder (Decision D5).
- **`memberOf` is read-only** (spec §8): the editable form must NOT expose `memberOf` as writable — render it read-only even in edit mode. **`groupOfNames` needs ≥1 `member`**: deleting the last member is a server-side constraint that will surface as a mapped result code; M4 does not pre-validate group membership (that's the M6 membership pane).
- **userPassword over TLS** (spec §8): generic-tier edits of `userPassword` go through the normal MODIFY path; the dedicated password action is M5. The worker connection's TLS posture is already established at bind (M1) — no extra work in M4, but do NOT add a plaintext password fallback.
- **Facade boundary** is load-bearing: every TV widget added in Tasks 1/5/6 lives in `src/ui/facade.rs`; non-facade modules see only `EditEntry`, `ChangeSet`, `FormModel`, `String` LDIF, and result enums.
- **Re-read, never optimistic-patch** (Decision D4): all post-write refreshes go back to the directory.
