# edaptor M3 — TUI shell + generic object tier (READ-ONLY)

> **For agentic workers:** REQUIRED SUB-SKILL: Use **superpowers:subagent-driven-development** (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Strict TDD per task: write a failing test → run it to confirm the failure → implement → run tests + `cargo clippy --all-targets -- -D warnings` + `cargo fmt` → commit. **The crate MUST compile after every task's commit.**

**Goal:** Stand up the turbo-vision TUI shell and the generic (schema-typed) object tier in **read-only** mode. Deliver: a `src/ui/facade.rs` boundary that is the only module importing `turbo_vision`; an `Application` shell with a menu bar (derived from config entry profiles, which M3 introduces), a status line, and a manual event loop that drains a non-blocking LDAP-worker response channel; a one-level/base SEARCH extension to the worker protocol with request-id correlation; a lazy DIT browser (`OutlineViewer`) rooted at `base_dn`; and a schema-driven read-only entry form whose widget per attribute is chosen by `FieldKind`. The write path (diff → ChangeSet → LDIF preview → MODIFY/ADD/MODRDN/DELETE) is **explicitly DEFERRED to M4** — nothing in M3 mutates the directory.

**Architecture:** Three layers (spec §3). (1) The async LDAP **worker** thread owns all network I/O; M3 adds a parameterized `Search` request *and a non-blocking submit/poll path* so the UI never blocks. (2) The **domain** layer (config + new `EntryProfile`/`profiles`, `SchemaModel`, and new pure helpers for menu assembly, the FieldKind→widget mapping, the form model, browser node labels and id-correlation) holds all testable logic. (3) The **TUI** layer sits behind `src/ui/facade.rs` — the single boundary importing `turbo_vision` (spec §8 / §14 risk-mitigation). The UI never blocks on the network: the manual loop (spike §9) drains worker `Response`s via a non-blocking `poll()` each iteration and applies them to views (spec §6). Labels everywhere (spec §7): tree nodes and form fields show human-readable labels, not raw DNs/OIDs.

**Tech stack:** Rust 2021, ldap3 0.12 (already wired), anyhow, plus the new dependency **`turbo-vision = "1.2"`**. All turbo-vision API calls match the compile-verified spike `docs/superpowers/research/2026-05-29-turbo-vision-spike.md` (sections 1–9). The OTHER spike (`2026-05-29-api-spike-findings.md`) is superseded for turbo-vision but its ldap3/paged-results notes still apply.

---

## Context from M1+M2 (real signatures on `main` — quoted from source)

These are the **actual** APIs as read from the worktree source. Where the build brief quoted a different shape, the real one below wins.

**`src/config/mod.rs`** — NOTE: there is currently **no** `EntryProfile` and **no** `profiles` field. The file comment literally says *"(Entry profiles arrive in M4.)"*. M3 brings a minimal profile slice forward (Task 1) because the menu is profile-derived (see Decision D0).
```rust
pub struct Config { pub server: ServerConfig, pub auth: AuthConfig }
pub struct ServerConfig {
    pub uri: String,
    pub base_dn: String,          // <-- base DN lives here: config.server.base_dn
    pub start_tls: bool,
    pub timeout_secs: u64,
    pub tls: TlsConfig,
}
pub struct AuthConfig {
    pub method: AuthMethod,                 // Simple | External | Gssapi
    pub bind_dn: Option<String>,
    pub password_source: PasswordSource,    // .resolve() -> Result<String>
}
impl Config { pub fn load(path: &Path) -> Result<Config>; }   // NOTE: load(path), not load_default()
```

**`src/ldap/worker.rs`** — the worker is **synchronous request/reply today** (one reply channel per call). There is **no** `send`/`recv`/`try_recv` and **no** persistent response channel yet. M3 adds the non-blocking path (Task 3, Decision D3).
```rust
pub enum Request { FetchSubschema, Shutdown }
pub struct RawSubschema { pub object_classes: Vec<String>, pub attribute_types: Vec<String>, pub ldap_syntaxes: Vec<String> }
pub enum Response { Subschema(RawSubschema), Done, Error(String) }
pub struct WorkerHandle { /* tx: Sender<Job>, join */ }
impl WorkerHandle {
    pub fn spawn(config: Config, password: String) -> Result<WorkerHandle>;  // connects+binds synchronously
    pub fn request(&self, req: Request) -> Result<Response>;                  // BLOCKING per-request reply
}
// worker_loop uses ldap3 LdapConn/Scope/SearchEntry. SearchEntry (ldap3 0.12):
//   .dn: String, .attrs: HashMap<String,Vec<String>>, .bin_attrs: HashMap<String,Vec<Vec<u8>>>
//   constructed via SearchEntry::construct(result_entry).
```

**`src/schema/model.rs`** — `effective_attributes` returns a struct of `BTreeSet`s, not a tuple; it takes `&[&str]`.
```rust
pub struct SchemaModel { /* ... */ pub warnings: Vec<String> }
pub struct ResolvedAttributes { pub must: BTreeSet<String>, pub may: BTreeSet<String> }
impl SchemaModel {
    pub fn from_raw(raw: &RawSubschema) -> SchemaModel;
    pub fn object_class(&self, name: &str) -> Option<&ObjectClass>;
    pub fn attribute_type(&self, name: &str) -> Option<&AttributeType>;
    pub fn effective_attributes(&self, object_classes: &[&str]) -> ResolvedAttributes; // {must, may}
    pub fn field_kind(&self, attr_name: &str) -> FieldKind;
}
```

**`src/schema/syntax.rs`** — variant is `DistinguishedName` (not `Dn`):
```rust
pub enum FieldKind { Text, Boolean, Integer, DistinguishedName, GeneralizedTime, Binary }
```

**`src/schema/mod.rs`**: `pub use model::{ResolvedAttributes, SchemaModel}; pub use syntax::{classify_syntax, FieldKind};`

**`src/lib.rs`** declares `pub mod config; pub mod ldap; pub mod schema;` and library fns `run_check`, `run_schema`, `SchemaReport`, plus a private `fetch_raw(config, password)`. M3 adds `pub mod ui; pub mod app; pub mod workflows;`.

**`src/main.rs`** resolves the password before constructing anything:
```rust
let config = Config::load(&config_path)?;
let password = config.auth.password_source.resolve().context("resolving bind password")?;
```
The TUI bootstrap (Task 2) must do the same before `WorkerHandle::spawn(config, password)`.

**Integration harness (M2 idiom, reuse in Task 7):** `scripts/test-ldap.sh start|stop` runs a podman OpenLDAP (bitnami, `ldap://localhost:1389`, base `dc=example,dc=org`, admin `cn=admin,dc=example,dc=org` / `adminpassword`). `tests/integration.rs` has a `test_config(uri)` helper and gates live tests on `EDAPTOR_TEST_LDAP_URI` (skip with an eprintln when unset). Inspect the file first and follow its exact pattern.

---

## Key design decisions (resolved up front)

Decided now so executing agents do not get stuck.

- **D0 — Profiles are introduced by M3 (minimal slice).** The menu is "derived from config profiles", but `EntryProfile`/`Config.profiles` do not exist yet. M3 adds a **minimal** profile struct as Task 1's first step (its own failing test + part of Task 1's commit): `EntryProfile { name: String, object_class: String, rdn_attr: String, search_base: String, show: Vec<String> }` and `Config.profiles: Vec<EntryProfile>` parsed from `[[profile]]` TOML blocks with `#[serde(default)]` so existing configs without profiles still load. This is a deliberate, called-out pull-forward of an M4 config sliver (see Notes/scope). Only the fields M3 actually uses (menu names, ordering of the read form) are added; password/membership/Samba profile metadata stays in M4.
- **D1 — Browser node payload.** `OutlineViewer<BrowserNode>` where `BrowserNode { dn: String, label: String, loaded: bool, object_classes: Vec<String> }`. A bare `String` cannot carry the DN to search on, the "children loaded?" flag, or the objectClasses the form needs. `label` holds the human-readable text (cn/description fallback → RDN) for labels-everywhere (spec §7). The `OutlineViewer::new` render closure maps `&BrowserNode -> String` by returning `node.label.clone()`.
- **D2 — Worker SEARCH shape.** A single parameterized request:
  `Request::Search { id: u64, base: String, scope: SearchScope, filter: String, attrs: Vec<String> }`
  with a domain enum `SearchScope { Base, OneLevel }` (mapped to `ldap3::Scope` inside the worker only). Reply:
  `Response::Entries { id: u64, entries: Vec<LdapEntry> }` and `Response::SearchError { id: u64, msg: String }` where
  `LdapEntry { dn: String, attrs: BTreeMap<String, Vec<String>>, bin_attrs: BTreeMap<String, usize> }`.
  Binary attrs ride along as **byte counts** (`usize` = sum of value lengths) so the read-only form renders `<N bytes>` without copying blobs. `BTreeMap` gives deterministic ordering for tests. The existing `Response::{Subschema,Done,Error}` variants are left untouched (FetchSubschema still works unchanged).
- **D3 — Non-blocking worker path (the riskiest part; do not gloss it).** Today `request()` is synchronous (one reply channel per call) — fine for the startup `FetchSubschema`. The browser's lazy-expand-in-idle-loop needs a **non-blocking submit + poll**. M3 adds, alongside the existing synchronous `request()`:
  - a long-lived response channel created in `spawn()` (`resp_tx` kept by the worker loop, `resp_rx: Receiver<Response>` stored on `WorkerHandle`);
  - `pub fn submit(&self, req: Request) -> Result<()>` — fire-and-forget; the worker processes it and pushes the `Response` onto the long-lived channel;
  - `pub fn poll(&self) -> Option<Response>` — wraps `resp_rx.try_recv()` (returns `None` on `Empty`/`Disconnected`).
  `request()` stays as-is for the synchronous startup schema fetch. `Search` is delivered via `submit`/`poll`. This is the spike §9 "manual loop + try_recv" pattern, adapted to the real worker. Unit-test `submit`+`poll` plumbing where possible (a fake/local channel round-trip); the live `conn.search` round-trip is the Task 7 integration test.
- **D4 — Request/response correlation.** Every `Search` carries a monotonic `id: u64`; the matching `Response::Entries`/`SearchError` echoes it. The browser keeps `pending: HashMap<u64, NodeRef>` (in-flight id → the tree node awaiting children). A polled response attaches to the correct node. This de-risks M4.
- **D5 — Headless cut.** All logic testable without a tty lives in **pure functions returning domain types**, never turbo-vision widgets:
  - `app::build_menu_defs(profiles: &[EntryProfile]) -> Vec<MenuDef>` (domain `MenuDef { label, command }`).
  - `ui::form::field_widget_spec(kind: FieldKind, value: Option<&str>, byte_count: Option<usize>) -> WidgetSpec` returning a domain enum `WidgetSpec { ReadOnlyText, ReadOnlyInt, ReadOnlyDn, ReadOnlyTime, DisabledCheckBox(bool), BinaryNote(usize) }`.
  - `ui::form::build_form_model(schema, object_classes, entry, profile_show) -> FormModel` (ordered `Vec<FormField>` with label, kind, must-flag, values, widget spec).
  - `workflows::browser::{node_label, entries_to_nodes, BrowserState::on_response}`.
  The facade (`facade.rs`) turns `MenuDef`/`WidgetSpec`/`FormModel`/`BrowserNode` into real turbo-vision widgets in a deliberately **thin, untested** layer.
- **D6 — Manual loop, not idle hook.** Use the manual loop (spike §9: `app.idle()` → `app.draw()` → flush → `poll_event(50ms)` → drain `worker.poll()` → check quit). Justification: the manual loop is the spike's sanctioned pattern for interleaving I/O, keeps channel draining explicit and in one place, avoids an extra `IdleView` type, and matches spec §6/§8.

---

## Task 1 — turbo-vision dep + `ui` facade skeleton + minimal `EntryProfile`/`profiles` (crate still compiles)

- [ ] Add to `Cargo.toml` `[dependencies]`: `turbo-vision = "1.2"`.
- [ ] **Profiles (D0).** In `src/config/mod.rs` add:
  ```rust
  #[derive(Debug, Deserialize, Default, Clone)]
  pub struct EntryProfile {
      pub name: String,
      pub object_class: String,
      #[serde(default)] pub rdn_attr: String,
      #[serde(default)] pub search_base: String,
      #[serde(default)] pub show: Vec<String>,
  }
  ```
  and on `Config`: `#[serde(default, rename = "profile")] pub profiles: Vec<EntryProfile>,` so `[[profile]]` blocks parse and absent profiles default to empty.
- [ ] Write failing test `parses_profiles` in `src/config/mod.rs`: a TOML string with two `[[profile]]` blocks (Users/inetOrgPerson, Groups/groupOfNames) yields `cfg.profiles.len() == 2` with the right `name`/`object_class`. Also a test `config_without_profiles_still_parses` (existing minimal config → `profiles` empty). Run → confirm fail → implement → pass. Verify the existing 3 config tests still pass.
- [ ] Create `src/ui/mod.rs` declaring `pub mod facade;` (add `pub mod form;` in Task 5).
- [ ] Create `src/ui/facade.rs`: the ONLY module with `use turbo_vision::prelude::*;` (plus any specific `turbo_vision::...` paths). For Task 1 it holds a module doc-comment stating the boundary rule (no other module may import `turbo_vision`) and a single compile-checking fn `pub fn tv_available() -> bool { true }`.
- [ ] In `src/lib.rs` add `pub mod ui;`.
- [ ] Write failing test `facade_boundary_compiles` in `src/ui/facade.rs` (`#[cfg(test)]`): `assert!(tv_available());`. Confirm it fails to compile first (fn absent), then passes.
- [ ] Verify: `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`.
- [ ] Commit:
```bash
git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -am "$(printf 'M3: turbo-vision dep, ui::facade boundary, minimal config profiles\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 2 — App shell, menu assembly, status line, manual event loop (do-nothing shell)

- [ ] Create `src/app.rs`; add `pub mod app;` to `src/lib.rs`.
- [ ] Domain type `pub struct MenuDef { pub label: String, pub command: u16 }` in `src/app.rs`. Define command id constants for the generic Browser and Quit (Quit = turbo-vision's `CM_QUIT`; Browser = an app-local id, e.g. `const CM_BROWSER: u16 = 1000;`).
- [ ] Write failing test `menu_defs_from_profiles` in `src/app.rs`: given two `EntryProfile`s (Users, Groups) `build_menu_defs` returns defs labelled `[Users, Groups, Browser, Quit]` in that order, the Quit def carrying `CM_QUIT`. Run → confirm fail.
- [ ] Implement `pub fn build_menu_defs(profiles: &[EntryProfile]) -> Vec<MenuDef>` (one entry per profile by `profile.name`, then a generic "Browser" entry, then "Quit"). Pure, tty-free. Make it pass.
- [ ] In `src/ui/facade.rs` add (thin, untested) wrappers matching the spike §1/§7 EXACTLY:
  - `build_menu_bar(size_w: i16, defs: &[MenuDef]) -> MenuBar`:
    ```rust
    let mut mb = MenuBar::new(Rect::new(0, 0, size_w, 1));
    let mut builder = MenuBuilder::new();
    for d in defs { builder = builder.item(&d.label, d.command, 0); }  // key code 0 = no hotkey
    mb.add_submenu(SubMenu::new("~E~daptor", builder.build()));
    mb
    ```
    (Spike confirms `MenuBuilder::new().item(text, cmd, key).build()`, `MenuBar::new(Rect)`, `SubMenu::new(title, Menu)`, `add_submenu`. Key `0` is the no-shortcut sentinel used throughout the spike examples.)
  - `build_status_line(size_w: i16, size_h: i16) -> StatusLine` via spike §1:
    ```rust
    StatusLine::new(Rect::new(0, size_h - 1, size_w, size_h),
        vec![StatusItem::new("~Alt+X~ Quit", KB_ALT_X, CM_QUIT),
             StatusItem::new("~F10~ Menu", KB_F10, 0)])
    ```
  - `pub struct Shell { app: Application }` with `pub fn new(defs: &[MenuDef]) -> anyhow::Result<Shell>`: `Application::new()?`, `let (w,h) = app.terminal.size();`, `app.set_menu_bar(build_menu_bar(w, defs)); app.set_status_line(build_status_line(w, h));`.
  - `pub fn run_loop(&mut self, mut on_idle: impl FnMut(&mut Application))` implementing the spike §1/§9 manual loop:
    ```rust
    self.app.running = true;
    while self.app.running {
        self.app.idle();
        on_idle(&mut self.app);                    // Task 4 drains worker.poll() here
        self.app.draw();
        let _ = self.app.terminal.flush();
        if let Ok(Some(mut ev)) = self.app.terminal.poll_event(Duration::from_millis(50)) {
            self.app.handle_event(&mut ev);
            if ev.what == EventType::Command && ev.command == CM_QUIT { self.app.running = false; }
        }
    }
    ```
- [ ] Wire `src/main.rs`: in the `None` (no-subcommand) arm, load config + resolve password (as today), `build_menu_defs(&config.profiles)`, build the `Shell`, and `run_loop(|_app| {})` (empty idle for now). CM_QUIT / Alt-X exits cleanly. Keep `check` / `schema` subcommands working.
- [ ] Headless honesty: `Shell::new` / `run_loop` / `build_menu_bar` / `build_status_line` need a tty and are **not** unit-tested; only `build_menu_defs` is. Add a `// not tty-testable` comment in the facade.
- [ ] Verify: `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`. (Optional manual: `cargo run` in a real terminal — shell opens, Alt-X quits.)
- [ ] Commit:
```bash
git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -am "$(printf 'M3: app shell with profile-derived menu, status line, manual event loop\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 3 — Worker: parameterized SEARCH (base + one-level) + non-blocking submit/poll + id correlation

- [ ] In `src/ldap/worker.rs` add `pub enum SearchScope { Base, OneLevel }` and
  `pub struct LdapEntry { pub dn: String, pub attrs: std::collections::BTreeMap<String, Vec<String>>, pub bin_attrs: std::collections::BTreeMap<String, usize> }`.
- [ ] Extend `Request` with `Search { id: u64, base: String, scope: SearchScope, filter: String, attrs: Vec<String> }`.
- [ ] Extend `Response` with `Entries { id: u64, entries: Vec<LdapEntry> }` and `SearchError { id: u64, msg: String }` (leave `Subschema`/`Done`/`Error(String)` unchanged — D2).
- [ ] **Non-blocking path (D3).** Modify `WorkerHandle`/`spawn`/`worker_loop`:
  - create a long-lived `(resp_tx, resp_rx) = mpsc::channel::<Response>()` in `spawn`; move `resp_tx` into the worker loop, store `resp_rx` on `WorkerHandle`.
  - add `pub fn submit(&self, req: Request) -> Result<()>` that sends a `Search` job whose worker handling pushes its `Response` onto `resp_tx` (NOT the per-call reply channel).
  - add `pub fn poll(&self) -> Option<Response>` wrapping `resp_rx.try_recv()` (→ `None` on Empty/Disconnected).
  - keep `request()` for `FetchSubschema` (synchronous startup fetch). Decide and document how the existing `Job = (Request, Sender<Response>)` plumbing coexists with the long-lived channel — simplest: `Search` jobs carry a no-op reply sender and the worker routes results to `resp_tx`; or widen `Job` so search results always go to `resp_tx`. Pick one, comment it.
- [ ] Write failing unit tests in `src/ldap/worker.rs`:
  - `search_entry_conversion`: pure helper `fn to_ldap_entry(se: SearchEntry) -> LdapEntry` maps `se.dn`/`se.attrs` into the `BTreeMap` and converts `se.bin_attrs` into per-attr byte counts (sum of each value's `len()`). Build a `SearchEntry` fixture (or feed `(dn, attrs, bin_attrs)` via a thin shim if `SearchEntry` is awkward to construct directly) and assert the mapping, including byte-count summing.
  - `scope_maps_to_ldap3`: `fn scope_to_ldap3(SearchScope) -> ldap3::Scope` maps `Base→Scope::Base`, `OneLevel→Scope::OneLevel`.
  - `submit_then_poll_roundtrip`: a channel-level test that pushing a `Response::Entries{id,..}` onto the long-lived channel makes `poll()` return it once then `None` (test the `poll` wrapper over a constructed handle/channel; if `WorkerHandle` is hard to build without a live conn, extract the `poll` logic onto the `resp_rx` directly and test that).
  Run → confirm fail.
- [ ] Implement `to_ldap_entry` + `scope_to_ldap3` + the `worker_loop` `Search` arm: `conn.search(&base, scope_to_ldap3(scope), &filter, attrs)`, map each `SearchEntry::construct(...)` via `to_ldap_entry`, push `Response::Entries { id, entries }` to `resp_tx`; on error push `Response::SearchError { id, msg }`. Network I/O stays on the worker thread (spec §3). Make tests pass.
- [ ] Verify: `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`.
- [ ] Commit:
```bash
git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -am "$(printf 'M3: worker SEARCH (base/one-level) with non-blocking submit/poll and id correlation\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 4 — DIT browser: OutlineViewer with lazy one-level expansion

- [ ] Create `src/workflows/mod.rs` (`pub mod browser;`) and `src/workflows/browser.rs`; add `pub mod workflows;` to `src/lib.rs`.
- [ ] Domain payload (D1): `pub struct BrowserNode { pub dn: String, pub label: String, pub loaded: bool, pub object_classes: Vec<String> }`.
- [ ] Pure helpers (tty-free, unit-tested):
  - `pub fn node_label(entry: &LdapEntry, rdn_fallback: &str) -> String` — prefer `cn`, then `description`, else the RDN fallback (labels-everywhere, spec §7). (Attribute lookups are case-insensitive over `entry.attrs` keys.)
  - `pub fn entries_to_nodes(entries: &[LdapEntry]) -> Vec<BrowserNode>` — build child payloads (`loaded:false`), pulling `objectClass` values into `object_classes`, computing `label` via `node_label` using the entry's leftmost RDN component as fallback.
- [ ] Write failing tests `label_prefers_cn`, `label_falls_back_to_rdn`, `entries_become_unloaded_nodes`. Run → confirm fail → implement → pass.
- [ ] Browser controller `pub struct BrowserState { pending: HashMap<u64, Rc<RefCell<Node<BrowserNode>>>>, next_id: u64, base_dn: String }` with:
  - `pub fn request_children(&mut self, worker: &WorkerHandle, node: &Rc<RefCell<Node<BrowserNode>>>) -> Result<u64>` — allocate `next_id`, `worker.submit(Request::Search { id, base: node.borrow().payload().dn.clone(), scope: OneLevel, filter: "(objectClass=*)".into(), attrs: vec!["cn".into(),"description".into(),"objectClass".into()] })?`, record `id → node` in `pending`, return id.
  - `pub fn on_response(&mut self, resp: &Response) -> Option<(Rc<RefCell<Node<BrowserNode>>>, Vec<BrowserNode>)>` — if `Response::Entries{id,..}` matches a `pending` id, remove it, convert children via `entries_to_nodes`, mark the parent node `loaded:true`, return `(node, children)`; else `None`. Unit-test correlation: insert a fake pending id mapped to a node, feed a matching `Response::Entries`, assert it resolves and marks loaded; feed a non-matching id, assert `None`.
- [ ] In `src/ui/facade.rs` (thin) add, matching spike §6 EXACTLY:
  - `pub fn new_node(p: BrowserNode) -> Rc<RefCell<Node<BrowserNode>>> { Rc::new(RefCell::new(Node::new(p))) }` — **`Node::new` takes ONE arg (the payload)**; there is no expanded flag.
  - `pub fn build_outline(root: Rc<RefCell<Node<BrowserNode>>>) -> OutlineViewer<BrowserNode>`:
    ```rust
    let mut v = OutlineViewer::new(Rect::new(1, 1, 40, 20), |n: &BrowserNode| n.label.clone());
    v.add_root(root);
    v
    ```
    (The render closure `&BrowserNode -> String` is where labels-everywhere plugs in.)
  - `attach_children(parent: &Rc<RefCell<Node<BrowserNode>>>, kids: Vec<BrowserNode>)` — for each child `parent.borrow_mut().add_child(new_node(child))` (spike §6: `Node::add_child(Rc<RefCell<Node<T>>>)`).
- [ ] **Expansion trigger — explicit action is the PRIMARY plan.** The spike confirms `OutlineViewer`/`Node` construction but does **NOT** verify any on-expand callback API (spike §6 lists no expand hook). Therefore: bind an **explicit "expand/open" action** (Enter, or an app command id) on the selected/highlighted node; when fired and the node is unloaded, call `BrowserState::request_children`. The polled `Response::Entries` is applied in the manual loop's idle hook (`on_idle` from Task 2 calls `worker.poll()` → `BrowserState::on_response` → `attach_children`). **Stretch (optional, ≤5 min):** if a genuine on-expand hook turns out to exist on `OutlineViewer` in 1.2, wire it; otherwise keep the explicit action. Document which path shipped in a code comment.
- [ ] Root node = `BrowserNode { dn: base_dn, label: base_dn, loaded:false, .. }`; request its children once at open.
- [ ] Headless honesty: `node_label`, `entries_to_nodes`, `BrowserState::on_response` are unit-tested; `OutlineViewer`/`Node`/`build_outline`/`attach_children` and the live expand wiring are NOT tty-testable.
- [ ] Verify: `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`.
- [ ] Commit:
```bash
git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -am "$(printf 'M3: lazy DIT browser (OutlineViewer) with one-level expand and id correlation\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 5 — Schema-driven READ-ONLY entry form (FieldKind → widget spec)

- [ ] Create `src/ui/form.rs`; add `pub mod form;` to `src/ui/mod.rs`.
- [ ] Domain types (tty-free):
  - `pub enum WidgetSpec { ReadOnlyText, ReadOnlyInt, ReadOnlyDn, ReadOnlyTime, DisabledCheckBox(bool), BinaryNote(usize) }`
  - `pub struct FormField { pub label: String, pub kind: FieldKind, pub is_must: bool, pub values: Vec<String>, pub widget: WidgetSpec }`
  - `pub struct FormModel { pub title: String, pub fields: Vec<FormField> }`
- [ ] Write failing tests in `src/ui/form.rs`:
  - `field_widget_spec_maps_kinds`: `Text→ReadOnlyText`, `Integer→ReadOnlyInt`, `DistinguishedName→ReadOnlyDn`, `GeneralizedTime→ReadOnlyTime`, `Boolean→DisabledCheckBox(parsed)`, `Binary→BinaryNote(byte_count)`.
  - `boolean_parses_true_false`: value `"TRUE"`→`DisabledCheckBox(true)`, `"FALSE"`→`false` (LDAP Boolean syntax is the strings TRUE/FALSE).
  - `form_model_orders_by_profile_show`: with `profile_show = ["uid","cn","mail"]`, those fields appear first in that order, remaining effective attrs after; MUST attrs flagged.
  - `form_model_marks_must`: an attr in the effective MUST set has `is_must:true`.
  Run → confirm fail.
- [ ] Implement:
  - `pub fn field_widget_spec(kind: FieldKind, value: Option<&str>, byte_count: Option<usize>) -> WidgetSpec`.
  - `pub fn build_form_model(schema: &SchemaModel, object_classes: &[&str], entry: &LdapEntry, profile_show: &[String]) -> FormModel` — call `schema.effective_attributes(object_classes)` → `ResolvedAttributes { must, may }`; build the ordered attr list = `profile_show` entries that are in (must ∪ may), then the remaining must, then the remaining may; for each attr: `schema.field_kind(attr)`, pull `entry.attrs`/`entry.bin_attrs` values, compute `field_widget_spec`, set `is_must = must.contains(attr)` (case-insensitive), label = attr name + `" *"` marker carried via `is_must` (the facade renders the marker). Title = entry DN. Make tests pass.
  - Note the `&[&str]` signature on `effective_attributes`: build a `Vec<&str>` of the entry's objectClasses to pass in.
- [ ] In `src/ui/facade.rs` (thin, untested) add `build_entry_dialog(model: &FormModel) -> Dialog` using spike §2 builders:
  - `DialogBuilder::new().bounds(Rect::new(0,0,60,h)).title(&model.title).build()`.
  - per field a `StaticText` label (append `" *"` when `is_must`) plus, per `WidgetSpec`:
    - `ReadOnlyText/Int/Dn/Time` → a `StaticText` showing the joined values (read-only, no edit affordance — preferred over a disabled `InputLine` to avoid any write path). NO data binding, NO write-back.
    - `DisabledCheckBox(b)` → a `CheckBox` rendered disabled reflecting `b`.
    - `BinaryNote(n)` → a `StaticText` reading `<N bytes>`.
  - one `ButtonBuilder::new().title("~C~lose").command(CM_CANCEL).default(true).build()`; shown via `dialog.execute(&mut app) -> u16` (spike §2).
- [ ] Headless honesty: `field_widget_spec` + `build_form_model` are fully unit-tested (the heart of the schema-driven form); `build_entry_dialog` is thin and tty-only.
- [ ] Verify: `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`.
- [ ] Commit:
```bash
git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -am "$(printf 'M3: schema-driven read-only entry form (FieldKind to widget-spec mapping)\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 6 — Wire selection → read-only form (the M3 read flow) + headless smoke tests

This task wires the **end-to-end read flow** — the spine of the milestone — then locks it down with headless smoke tests.

- [ ] In `src/main.rs` (or a small `workflows` glue fn) wire the read flow (primary deliverable, not an afterthought): when a browser node is "opened" (explicit action), issue a **base**-scope SEARCH (`scope: Base`, `attrs: vec!["*".into()]`) for the node's DN via `worker.submit`; in the idle hook, when the matching `Response::Entries` is polled, build a `FormModel` via `build_form_model` (objectClasses from the entry; `profile_show` from the active profile's `show`, or empty for the generic tier) and open the dialog via `facade::build_entry_dialog(...).execute(app)`. All non-blocking in the manual loop.
- [ ] Add `pub fn confirm_error(app: &mut Application, msg: &str)` in `src/ui/facade.rs` over `message_box(app, msg, MF_ERROR | MF_OK_BUTTON)` (spike §8) to surface `Response::Error`/`SearchError` to the user. (Not tty-testable.)
- [ ] Headless smoke tests (no tty):
  - `menu_defs_smoke`: `build_menu_defs` over a realistic profile set returns expected count/labels.
  - `form_model_smoke`: `build_form_model` over a small hand-built `SchemaModel` (`SchemaModel::from_raw` with inline definitions, as the M2 model tests do) + a hand-built `LdapEntry` produces a non-empty ordered `FormModel` with correct kinds/must-flags (e.g. a `cn`→ReadOnlyText, a Boolean attr→DisabledCheckBox, a `member`/DN attr→ReadOnlyDn).
  - `browser_correlation_smoke`: `BrowserState` with two interleaved pending ids resolves each `Response::Entries` to the right node.
- [ ] **Tty boundary doc-comment** (be explicit, spec §11): `Application::new()`, `Shell::new`, `run_loop`, `build_menu_bar`, `build_status_line`, `build_outline`, `attach_children`, `build_entry_dialog`, `confirm_error`/`message_box` all require a terminal and are **NOT** unit-tested; everything below the facade (menu defs, form model, widget-spec mapping, browser correlation, node labels, entry conversion, scope mapping) **IS** unit-tested headlessly.
- [ ] Verify: `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`.
- [ ] Commit:
```bash
git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -am "$(printf 'M3: wire browser selection to read-only form + headless smoke tests\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 7 — Live integration test: one-level browse + base read against containerized OpenLDAP

- [ ] First **inspect `tests/integration.rs`** and reuse its M2 idiom: `test_config(uri)` helper, `EDAPTOR_TEST_LDAP_URI` env gate (skip-with-eprintln when unset), `scripts/test-ldap.sh start|stop` for the **podman** bitnami OpenLDAP (`ldap://localhost:1389`, base `dc=example,dc=org`, admin `cn=admin,dc=example,dc=org`/`adminpassword`). Add new tests to that file (or a new `tests/m3_browse_read.rs` mirroring it).
- [ ] Test `one_level_search_lists_children`: `WorkerHandle::spawn(test_config(uri), pw)`, `submit(Request::Search { id:1, base: base_dn, scope: OneLevel, filter:"(objectClass=*)".into(), attrs: vec!["cn".into(),"objectClass".into()] })`, then loop `poll()` (with a short bounded retry/sleep) until a `Response::Entries{id:1,..}` arrives; assert id echoes and at least one expected child DN (e.g. `ou=users,dc=example,dc=org`) appears.
- [ ] Test `base_search_reads_entry_then_form_model`: base-scope SEARCH on a known entry DN; convert the entry to a `FormModel` via `build_form_model` with a real `SchemaModel` (fetched via `request(Request::FetchSubschema)` → `SchemaModel::from_raw`); assert MUST attrs present and field kinds sane (`cn`→Text, a DN attr→DistinguishedName). Exercises worker SEARCH + schema + form model end-to-end (no tty).
- [ ] Verify: `cargo test` (default green; live tests SKIP without the env var), and the live run:
```bash
scripts/test-ldap.sh start
export EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389
export EDAPTOR_TEST_ADMIN_PW=adminpassword   # match the M2 test's env var name; confirm in the file
cargo test --test integration -- --nocapture
unset EDAPTOR_TEST_LDAP_URI EDAPTOR_TEST_ADMIN_PW
cargo test --test integration -- --nocapture   # confirm all SKIP
scripts/test-ldap.sh stop
```
  Then `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`. STOP the container even on failure. Report actual output honestly; if a seeded child OU/entry name differs, adjust the assertion to the real seed data.
- [ ] Commit:
```bash
git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -am "$(printf 'M3: integration test for one-level browse + base read to form model\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Definition of Done

- [ ] `Cargo.toml` declares `turbo-vision = "1.2"`; `src/ui/facade.rs` is the ONLY module importing `turbo_vision` (prove: `grep -rl turbo_vision src | grep -v 'ui/facade.rs'` is empty).
- [ ] `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` clean after EVERY task's commit (crate compiles at each step).
- [ ] `Config` parses `[[profile]]` blocks into `profiles: Vec<EntryProfile>`; configs without profiles still load (existing config tests still pass).
- [ ] App shell opens in a real terminal; menu reflects config profiles + Browser + Quit; status line present; Alt-X / CM_QUIT exits the manual loop cleanly.
- [ ] Worker handles `Request::Search { Base | OneLevel }` and replies `Response::Entries { id, entries }` / `SearchError { id, msg }` with id correlation; `submit`/`poll` provide a non-blocking path while `request()` still serves the startup `FetchSubschema`; all network I/O stays on the worker thread.
- [ ] DIT browser lazily issues a one-level SEARCH on explicit node expansion, attaches children via id correlation, shows cn/description labels (labels-everywhere).
- [ ] Selecting an entry opens a **read-only** schema-driven form: one field per effective attribute, widget chosen by `FieldKind`, MUST attrs marked, ordering driven by `profile.show`. No editing, no writes — confirmed by inspection (no MODIFY/ADD/MODRDN/DELETE anywhere; read widgets are `StaticText`/disabled, no data binding).
- [ ] Headless unit tests cover: `build_menu_defs`, `field_widget_spec`, `build_form_model`, `BrowserState::on_response` correlation, `node_label`/`entries_to_nodes`, `to_ldap_entry`/`scope_to_ldap3`, `submit`/`poll` roundtrip, profile parsing. Tty-only pieces documented as such.
- [ ] Live integration test passes against the podman OpenLDAP and SKIPs cleanly without the env var.
- [ ] Every turbo-vision call matches the compile-verified spike: `Rect::new(x1,y1,x2,y2)` corners (use `from_coords` only for origin+size); `MenuBar::new(Rect)` + `MenuBuilder::new().item(...).build()` + `SubMenu::new`; `StatusLine::new(Rect, vec![StatusItem::new(...)])`; `OutlineViewer::new(Rect, |&T|->String)` + `add_root` + `Node::new(payload)` (one arg) + `add_child`; `DialogBuilder`/`ButtonBuilder`/`StaticTextBuilder`; `dialog.execute(&mut app) -> u16`; `message_box(app, msg, MF_ERROR | MF_OK_BUTTON)`; manual `idle()/draw()/flush()/poll_event()` loop with `app.running`.

---

## Notes / scope

- **READ-ONLY milestone.** The write path (diff → ChangeSet → LDIF preview → MODIFY/ADD/MODRDN/DELETE), the full curated profile tier, paged results for large containers, and color/theming are **DEFERRED to M4+**. Do not add edit affordances; read widgets must not bind data or write back.
- **Profiles pulled forward (D0):** `config/mod.rs` says profiles "arrive in M4", but M3's menu is profile-derived, so M3 adds a **minimal** `EntryProfile`/`Config.profiles` slice (name/object_class/rdn_attr/search_base/show) with `#[serde(default)]`. The richer profile metadata (password/membership/Samba/label/search_attributes per spec §5) remains M4.
- **Two-tier model (spec §3.1):** M3 ships the **generic tier** (all effective attributes, schema-typed) + the browser. The curated Users/Groups bespoke forms are later; `profile.show` is used here only to order/scope the generic read form.
- **Non-blocking worker (D3) is the highest-risk change** — it adds a long-lived response channel + `submit`/`poll` to a worker that is synchronous today. Keep `request()` intact for the startup schema fetch; route only `Search` through `submit`/`poll`. This unblocks the entire browse/read flow; get it right in Task 3 before building the browser on it.
- **Facade discipline (spec §8/§14):** keep `turbo_vision` imports confined to `src/ui/facade.rs`. New widgets get a thin wrapper there consuming/returning domain types — never leak turbo-vision types into `app.rs`/`workflows/`.
- **Spike gap (carried):** the OutlineViewer expand-callback API is **unverified** (spike §6 confirms construction only). The plan ships the explicit-expand action as PRIMARY; an auto-on-expand hook is a documented stretch, not a dependency.
- **Headless reality (spike §9):** `Application::new()` needs a tty, so shell/dialog/menubar/outline/message_box cannot be unit-tested. The plan pushes all logic below the facade into pure functions so the milestone's substance is covered without a terminal (spec §11). Manual smoke in a real terminal is expected before merge.
- **Environment note:** the worktree shell does not persist `cd` between commands; use absolute paths / `cargo --manifest-path` / `git -C`. Use **podman** for the integration container. Commit identity: `user.name='oetiker'`, `user.email='oetiker@gmail.com'` (matches the repo's existing commits).

### Open questions for the human before execution

1. **Integration seed data:** what child OUs/entries does `scripts/test-ldap.sh` seed under `dc=example,dc=org` (so Task 7's `one_level_search_lists_children` asserts on a real child DN)? Confirm the env var name for the admin password (`EDAPTOR_TEST_ADMIN_PW`?) used by the existing `test_config`.
2. **Profile pull-forward scope:** OK to add the minimal `EntryProfile`/`profiles` (D0) in M3, or would you rather hard-code the menu (Users/Groups/Browser/Quit) and keep ALL profile config strictly in M4? Plan assumes the minimal pull-forward.
3. **Read-only widget choice:** plan uses `StaticText` for value display (clearly non-editable, zero write path) rather than a disabled `InputLine`. Confirm that's the desired look, or prefer field-like disabled inputs.
4. **Generic vs profile tier on select:** when a selected entry matches a configured profile's `object_class`, use that profile's `show` to order-then-append the rest (generic stays complete)? Plan assumes yes.
