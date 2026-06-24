# tvision-rs Migration M1 — Three-Pane Read Core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a runnable `edaptor-tv` dev binary showing a read-only three-pane tvision-rs UI (DIT tree / leaf list+search / read-only entry form) driven by the real domain layer, plus the foundational `FieldWidget` plugin trait + registry with read-only presenters — while the existing ratatui `edaptor` binary keeps building and running.

**Architecture:** New tvision UI under `src/tui/`, run via `src/bin/edaptor-tv.rs`. Shared mutable app state is an `Rc<RefCell<UiState>>` cloned into each pane factory closure. An off-thread LDAP `WorkerHandle` is drained by a zero-area `PumpView` on a periodic timer; results flow through `workflows::read_flow::ReadFlow` into `UiState` and a `REFRESH` broadcast re-renders the panes. The read-only form model (`FormModel`) is relocated from `src/ui/` into `workflows::form_model` so both UIs share it.

**Tech Stack:** Rust 2021, `tvision-rs = "0.1"` (resolves to 0.1.2), the existing domain layer (`config`, `ldap::worker`, `schema`, `workflows`), `anyhow`.

## Global Constraints

- **Cap build/test parallelism at 4 cores:** always `-j4` (shared 128-core box). Cargo target dir is `/home/oetiker/scratch/cargo-target`.
- **tvision-rs version:** published `tvision-rs = "0.1"` (0.1.2). No path/git dependency. Alias as `tv` where convenient.
- **0.1.1+ API facts (do NOT reintroduce 0.1.0 workarounds):** `Outline` auto-seeds on first display — call `ov_update` ONLY after mutating the tree. Read tree selection via `Outline::value() -> Some(FieldValue::Int(foc))`. `Deferred` is at the crate root (`tvision_rs::Deferred`).
- **Facade boundary:** only `src/tui/**` and `src/bin/edaptor-tv.rs` may `use tvision_rs`. Only `src/ui/**` may `use ratatui` / `use tui_*`. The domain layer imports neither.
- **Borrow discipline:** never hold a `RefCell` borrow across `ctx.broadcast`, `ListBox::new_list`, `Group::child_mut`, `InputLine::set_value`, or `worker.submit`/`request_entry`. Collect into locals → drop the borrow → call.
- **Strict TDD; atomic commits; crate compiles after every commit; `cargo fmt` before every commit; clippy clean (`--all-targets -D warnings`).**
- **Commit trailer:** `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- **Live tests** gated by env `EDAPTOR_TEST_LDAP_URI` (skip when unset). Interactive acceptance needs a human at a terminal (agent sessions have no TTY → `CrosstermBackend::new()` returns ENXIO).

## Verification commands (used throughout)

```bash
cargo build -j4
cargo build -j4 --bin edaptor-tv
cargo test  -j4
cargo clippy -j4 --all-targets -- -D warnings
cargo fmt --check
# facade guard (must print nothing):
! grep -rl "use tvision_rs" src | grep -vE "^src/(tui/|bin/edaptor-tv.rs)"
! grep -rl "use ratatui\|use tui_" src | grep -vE "^src/ui/"
```

---

## File Structure

| File | Responsibility | Task |
|---|---|---|
| `src/workflows/form_model.rs` | Relocated read-only form model (`FormModel`/`FormField`/`WidgetSpec`/`build_form_model`) | 1 |
| `Cargo.toml` | Add `tvision-rs` dep + `edaptor-tv` bin | 2 |
| `src/tui/mod.rs` | Facade: `run()`, `UiState`, `Shared`, `REFRESH`, bootstrap | 2,3 |
| `src/bin/edaptor-tv.rs` | Dev binary entry (config + password → `tui::run`) | 2 |
| `src/tui/state.rs` | `UiState` struct, `pump_worker`, helpers (`profile_for`, `leaf_rows`) | 3,6,7 |
| `src/tui/widget.rs` | `FieldWidget` trait, `Activation`, `CommitOutcome`, `Capability`, registry, read-only presenters | 4 |
| `src/tui/pump.rs` | `PumpView` (timer-driven worker drain) | 7 |
| `src/tui/panes/tree.rs` | DIT `Outline` pane + `build_branch_nodes` | 5 |
| `src/tui/panes/leaf.rs` | Leaf `ListBox` + search `InputLine` pane | 6 |
| `src/tui/panes/form.rs` | Read-only form `Group` pane | 8 |
| `src/tui/app.rs` | `Program` assembly: desktop/menu/status, three-pane `Splitter`, wiring | 9 |

---

## Task 1: Relocate the read-only form model into `workflows::form_model`

Pure refactor. Moves `src/ui/form.rs` → `src/workflows/form_model.rs`, fixing a layering violation (`workflows::read_flow` and `workflows::create` currently import `crate::ui::form`). The ratatui UI and all existing tests must stay green.

**Files:**
- Move: `src/ui/form.rs` → `src/workflows/form_model.rs`
- Modify: `src/workflows/mod.rs` (add `pub mod form_model;`, fix doc link)
- Modify: `src/ui/mod.rs:11` (remove `pub mod form;`)
- Modify (import path `crate::ui::form` → `crate::workflows::form_model`): `src/workflows/read_flow.rs:16`, `src/workflows/create.rs:12`, `src/ui/edit_form.rs` (lines 3,19,453,778,837,1039,1088,1161,1371,1432,1684), `src/ui/view.rs:21,761`, `src/ui/app/action.rs:566,608,645,744`, `src/ui/app/input.rs:532,624`, `src/ui/app/test_support.rs:48`, `src/ui/app/value_editor.rs:611,750,1092,1236,1338`, `src/ui/app/password_editor.rs:135`, `src/ui/app/save.rs:562`

**Interfaces:**
- Produces (unchanged signatures, new path `crate::workflows::form_model`):
  - `pub struct FormModel { pub title: String, pub fields: Vec<FormField> }`
  - `pub struct FormField { pub label: String, pub kind: FieldKind, pub is_must: bool, pub values: Vec<String>, pub widget: WidgetSpec }`
  - `pub enum WidgetSpec { ReadOnlyText, ReadOnlyInt, ReadOnlyDn, ReadOnlyTime, DisabledCheckBox(bool), BinaryNote(usize) }`
  - `pub fn build_form_model(schema: &SchemaModel, object_classes: &[&str], entry: &LdapEntry, profile_show: &[String]) -> FormModel`

- [ ] **Step 1: Move the file and register the module**

```bash
cd /home/oetiker/checkouts/edaptor
git mv src/ui/form.rs src/workflows/form_model.rs
```

In `src/workflows/mod.rs` add (next to the other `pub mod` lines):

```rust
pub mod form_model;
```

In `src/ui/mod.rs` delete the line:

```rust
pub mod form;
```

- [ ] **Step 2: Update the moved file's own doc link**

In `src/workflows/form_model.rs`, the module doc references `crate::ui::edit_form::EditForm`. Make it framework-neutral — change that doc line to:

```rust
//! editable form is built from; a UI renders it.
```

(Its `use` lines — `crate::ldap::worker::LdapEntry`, `crate::schema::{FieldKind, SchemaModel}` — are already correct and need no change.)

- [ ] **Step 3: Rewrite every importer's path**

Replace `crate::ui::form` with `crate::workflows::form_model` across the crate:

```bash
cd /home/oetiker/checkouts/edaptor
grep -rl "crate::ui::form\b" src --include=*.rs | grep -v form_state \
  | xargs sed -i 's/crate::ui::form\b/crate::workflows::form_model/g'
```

Then fix the doc comment in `src/workflows/mod.rs` that mentions `crate::ui::form::FormModel`:

```bash
sed -i 's#crate::ui::form::FormModel#crate::workflows::form_model::FormModel#g' src/workflows/mod.rs
```

- [ ] **Step 4: Verify it builds and all existing tests pass**

Run:
```bash
cargo build -j4
cargo test -j4
```
Expected: builds clean; the full existing test suite passes (this is a pure move — no behaviour change). If any `crate::ui::form` reference remains, fix it (`grep -rn "crate::ui::form\b" src | grep -v form_state` must be empty).

- [ ] **Step 5: Verify the facade/layering guard improves**

Run:
```bash
grep -rn "crate::ui::" src/workflows/   # expected: no matches (workflows no longer imports ui)
cargo clippy -j4 --all-targets -- -D warnings
cargo fmt
```
Expected: no `crate::ui::` in `src/workflows/`; clippy clean.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor: relocate FormModel to workflows::form_model

Moves the framework-agnostic read-only form model out of src/ui into the
workflows orchestration layer, fixing the layering violation where
workflows::{read_flow,create} imported crate::ui::form. Pure move; no
behaviour change. Prepares the model to be shared by both UIs.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 2: Add tvision-rs + `src/tui/` skeleton + `edaptor-tv` dev binary

A minimal tvision `Program` that opens a titled window with a menu bar and status line and quits on Alt+X. Proves the dep resolves and both binaries build.

**Files:**
- Modify: `Cargo.toml`
- Create: `src/tui/mod.rs`
- Create: `src/bin/edaptor-tv.rs`
- Modify: `src/lib.rs` (add `pub mod tui;`)

**Interfaces:**
- Produces: `pub fn edaptor::tui::run(config: crate::config::Config, password: String) -> anyhow::Result<()>`

- [ ] **Step 1: Add the dependency and binary to `Cargo.toml`**

In `[dependencies]` (after `unicode-width`):

```toml
# tvision-rs UI (migration target). During M1-M4 it ships only via the
# edaptor-tv dev binary; the default edaptor binary stays ratatui until M5.
tvision-rs = "0.1"
```

After the existing `[[bin]]` blocks add:

```toml
[[bin]]
name = "edaptor-tv"
path = "src/bin/edaptor-tv.rs"
```

- [ ] **Step 2: Add the module to `src/lib.rs`**

After `pub mod schema;` (keep alphabetical-ish with the others) add:

```rust
pub mod tui;
```

- [ ] **Step 3: Write the minimal `src/tui/mod.rs`**

```rust
//! tvision-rs UI (migration target). Built under `src/tui/` during M1-M4 and
//! run via the `edaptor-tv` dev binary; renamed to `src/ui/` at the M5 cutover.
//! Only this module tree (and `src/bin/edaptor-tv.rs`) may `use tvision_rs`.

use anyhow::Result;
use tvision_rs::{
    self as tv, alt, Command, CrosstermBackend, Desktop, Program, Rect, StatusDef, StatusLine,
    SystemClock, Theme, View, Window,
};

use crate::config::Config;

/// Build the desktop with a single placeholder window (Task 9 fills it in).
fn init_desktop(r: Rect) -> Option<Box<dyn View>> {
    let mut r = r;
    r.a.y += 1; // below menu bar
    r.b.y -= 1; // above status line
    let mut desktop = Desktop::new(r, |br| Some(Desktop::init_background(br)));
    let win_rect = Rect::new(r.a.x + 2, r.a.y + 1, r.b.x - 2, r.b.y - 1);
    let win = Window::new(win_rect, Some("edaptor (tvision)".to_string()), 1);
    desktop.insert_view(Box::new(win));
    Some(Box::new(desktop))
}

fn init_status_line(r: Rect) -> Option<Box<dyn View>> {
    let mut r = r;
    r.a.y = r.b.y - 1;
    let defs = StatusDef::list()
        .def_all(|d| d.item("~Alt-X~ Exit", alt('x'), Command::QUIT))
        .build();
    Some(Box::new(StatusLine::new(r, defs)))
}

fn init_menu_bar(r: Rect) -> Option<Box<dyn View>> {
    let mut r = r;
    r.b.y = r.a.y + 1;
    let menu = tv::Menu::builder()
        .submenu("~F~ile", alt('f'), |m| {
            m.command_key("E~x~it", Command::QUIT, alt('x'), "Alt-X")
        })
        .build();
    Some(Box::new(tv::MenuBar::new(r, menu)))
}

/// Spawn the worker, fetch schema + structure, then run the TUI.
/// (M1: bootstrap is added in Task 3; here it only opens an empty program.)
pub fn run(_config: Config, _password: String) -> Result<()> {
    let backend = Box::new(CrosstermBackend::new()?);
    let mut program = Program::new(
        backend,
        Box::new(SystemClock::new()),
        Theme::classic_blue(),
        init_desktop,
        init_status_line,
        init_menu_bar,
    );
    program.run_app(|_prog, _cmd| {});
    Ok(())
}
```

- [ ] **Step 4: Write `src/bin/edaptor-tv.rs`**

```rust
//! Dev binary for the in-progress tvision UI (M1-M4). Deleted at the M5 cutover.
//! Usage: `cargo run -j4 --bin edaptor-tv -- [config.toml]`
//! Config path defaults to examples/demo-config.toml; password from
//! EDAPTOR_TEST_ADMIN_PW (demo: adminpassword).

use std::path::PathBuf;

use anyhow::{anyhow, Result};
use edaptor::config::Config;

fn main() -> Result<()> {
    let path: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("examples/demo-config.toml"));
    let config = Config::load(&path)?;
    let password = std::env::var("EDAPTOR_TEST_ADMIN_PW")
        .map_err(|_| anyhow!("set EDAPTOR_TEST_ADMIN_PW (demo: adminpassword)"))?;
    edaptor::tui::run(config, password)
}
```

- [ ] **Step 5: Verify both binaries build and the facade guard holds**

Run:
```bash
cargo build -j4                      # default edaptor (ratatui) still builds
cargo build -j4 --bin edaptor-tv     # new binary builds
cargo clippy -j4 --all-targets -- -D warnings
! grep -rl "use tvision_rs" src | grep -vE "^src/(tui/|bin/edaptor-tv.rs)"
cargo fmt
```
Expected: all build; clippy clean; facade guard prints nothing. (No automated run test — `run()` needs a TTY. Manual acceptance: `cargo run -j4 --bin edaptor-tv` opens a blue window; Alt+X quits.)

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(tui): tvision-rs skeleton + edaptor-tv dev binary

Adds the tvision-rs dependency and a minimal src/tui::run that opens a
titled window with a menu bar and status line (Alt-X quits), runnable via
the edaptor-tv dev binary. The default edaptor (ratatui) binary is
untouched. Facade boundary enforced: only src/tui + the dev binary use
tvision_rs.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 3: Bootstrap `UiState` and shared state in `tui::run`

Port the spike's blocking bootstrap (config → worker → schema → structure → ReadFlow) into a real `UiState` held in an `Rc<RefCell<…>>`, with a worker-less test constructor.

**Files:**
- Create: `src/tui/state.rs`
- Modify: `src/tui/mod.rs` (`mod state;`, `Shared`, `REFRESH`, bootstrap in `run`)

**Interfaces:**
- Consumes: `WorkerHandle::{spawn,request,poll}`, `Request::{FetchSubschema,LoadStructure}`, `Response::{Subschema,StructureEntries}`, `SchemaModel::from_raw`, `Structure::build`, `StructureInput`, `ReadFlow::new`, label/tree rule compilers.
- Produces:
  - `pub type Shared = std::rc::Rc<std::cell::RefCell<UiState>>;`
  - `pub const REFRESH: tvision_rs::Command = tvision_rs::Command::custom("edaptor.refresh");`
  - `pub struct UiState { … }` with `pub fn current_leaf_dn(&self) -> Option<&str>` and (test) `new_for_test`.
  - `pub(crate) fn bootstrap(config: Config, password: String) -> Result<UiState>`

- [ ] **Step 1: Write `src/tui/state.rs`**

```rust
//! Shared application state for the tvision UI and the blocking bootstrap that
//! builds it. State is held in `Rc<RefCell<UiState>>` (alias `Shared` in mod.rs).

use anyhow::{anyhow, Result};

use crate::config::tree_label::CompiledTreeRule;
use crate::config::{Config, EntryProfile};
use crate::ldap::worker::{Request, Response, WorkerHandle};
use crate::schema::SchemaModel;
use crate::ui::app::structure_view::LabelRule; // pure label rule (no ratatui)
use crate::workflows::read_flow::ReadFlow;
use crate::workflows::form_model::FormModel;
use crate::workflows::structure::{Structure, StructureInput};

/// Everything the panes read/write, behind a single RefCell.
pub struct UiState {
    /// `None` only in headless unit tests.
    pub worker: Option<WorkerHandle>,
    pub read_flow: ReadFlow,
    pub structure: Structure,
    pub base_dn: String,
    pub profiles: Vec<EntryProfile>,
    pub label_rules: Vec<LabelRule>,
    pub tree_rules: Vec<CompiledTreeRule>,
    /// DFS pre-order index → branch DN, matching `Outline`'s `foc` numbering.
    pub branch_dns: Vec<String>,
    pub current_branch: Option<String>,
    pub current_leaf: Option<String>,
    pub search: String,
    /// The loaded read-only form (None until a leaf is read).
    pub form: Option<FormModel>,
    pub list_dirty: bool,
    pub form_dirty: bool,
}

impl UiState {
    pub fn current_leaf_dn(&self) -> Option<&str> {
        self.current_leaf.as_deref()
    }

    /// Test-only constructor: a worker-less state over a pre-built Structure and
    /// schema. `pump_worker` returns false (no worker). Added to in later tasks.
    #[cfg(test)]
    pub fn new_for_test(
        structure: Structure,
        schema: SchemaModel,
        base_dn: String,
        label_rules: Vec<LabelRule>,
        tree_rules: Vec<CompiledTreeRule>,
    ) -> Self {
        UiState {
            worker: None,
            read_flow: ReadFlow::new(schema),
            structure,
            base_dn,
            profiles: Vec::new(),
            label_rules,
            tree_rules,
            branch_dns: Vec::new(),
            current_branch: None,
            current_leaf: None,
            search: String::new(),
            form: None,
            list_dirty: false,
            form_dirty: false,
        }
    }
}

/// `StructureNodeRaw` (worker) → `StructureInput` (structure model).
fn to_input(n: crate::ldap::worker::StructureNodeRaw) -> StructureInput {
    StructureInput {
        dn: n.dn,
        cn: n.cn,
        description: n.description,
        object_classes: n.object_classes,
        attrs: n.attrs,
    }
}

/// Blocking startup: spawn the worker, fetch schema + eager structure, build the
/// compiled label rules and the ReadFlow. Mirrors `ui::app::run`'s bootstrap.
pub(crate) fn bootstrap(config: Config, password: String) -> Result<UiState> {
    let base_dn = config.server.base_dn.clone();
    let profiles = config.profiles.clone();
    let label_rules = crate::ui::app::structure_view::label_rules(&profiles);
    let tree_rules = crate::config::tree_label::compile_tree_rules(&config.tree);

    let worker = WorkerHandle::spawn(config, password)?;

    let raw = match worker.request(Request::FetchSubschema)? {
        Response::Subschema(raw) => raw,
        other => return Err(anyhow!("FetchSubschema: unexpected {other:?}")),
    };
    let schema = SchemaModel::from_raw(&raw);

    let nodes = match worker.request(Request::LoadStructure {
        id: 0,
        base: base_dn.clone(),
        page_size: 500,
        attrs: vec![],
    })? {
        Response::StructureEntries { nodes, .. } => nodes,
        other => return Err(anyhow!("LoadStructure: unexpected {other:?}")),
    };
    let structure = Structure::build(&base_dn, nodes.into_iter().map(to_input).collect());

    Ok(UiState {
        worker: Some(worker),
        read_flow: ReadFlow::new(schema),
        structure,
        base_dn,
        profiles,
        label_rules,
        tree_rules,
        branch_dns: Vec::new(),
        current_branch: None,
        current_leaf: None,
        search: String::new(),
        form: None,
        list_dirty: false,
        form_dirty: false,
    })
}
```

> NOTE on reuse: `LabelRule` and `label_rules()` live in `src/ui/app/structure_view.rs` and are pure (no ratatui). M1 imports them from there. They will move into `src/tui/labels.rs` when the ratatui tree is deleted at M5; for now reusing them avoids duplicating logic.

- [ ] **Step 2: Wire `state` into `src/tui/mod.rs`**

Add near the top of `src/tui/mod.rs`:

```rust
mod state;

use std::cell::RefCell;
use std::rc::Rc;

pub use state::UiState;

/// Shared mutable app state, cloned into each pane factory closure.
pub type Shared = Rc<RefCell<UiState>>;

/// Broadcast command: re-render all panes from current `UiState`.
pub const REFRESH: tv::Command = tv::Command::custom("edaptor.refresh");
```

Change `run` to bootstrap and hold the state (it is consumed by the desktop factory in Task 9; for now bind it so it lives for the program):

```rust
pub fn run(config: Config, password: String) -> Result<()> {
    let state: Shared = Rc::new(RefCell::new(state::bootstrap(config, password)?));
    let _ = &state; // used by init_desktop in Task 9
    let backend = Box::new(CrosstermBackend::new()?);
    let mut program = Program::new(
        backend,
        Box::new(SystemClock::new()),
        Theme::classic_blue(),
        init_desktop,
        init_status_line,
        init_menu_bar,
    );
    program.run_app(|_prog, _cmd| {});
    Ok(())
}
```

- [ ] **Step 3: Write the failing test (bootstrap state shape via the test constructor)**

Append to `src/tui/state.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldap::worker::RawSubschema;
    use std::collections::BTreeMap;

    fn si(dn: &str, child_hint: Option<&str>) -> StructureInput {
        StructureInput {
            dn: dn.into(),
            cn: child_hint.map(Into::into),
            description: None,
            object_classes: vec![],
            attrs: BTreeMap::new(),
        }
    }

    #[test]
    fn test_state_starts_empty() {
        let structure = Structure::build("dc=x", vec![si("dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let st = UiState::new_for_test(
            structure,
            schema,
            "dc=x".into(),
            Vec::new(),
            Vec::new(),
        );
        assert!(st.current_leaf_dn().is_none());
        assert!(st.form.is_none());
        assert!(!st.list_dirty);
    }
}
```

- [ ] **Step 4: Run the test to verify it passes (compile gate)**

Run: `cargo test -j4 --bin edaptor-tv test_state_starts_empty` — wait: unit tests in a library module run via the lib test target. Run instead:
```bash
cargo test -j4 tui::state::tests::test_state_starts_empty
```
Expected: PASS. (The test exercises construction; the real value is that it forces the whole `state.rs` to compile against the live domain APIs.)

- [ ] **Step 5: Verify build + lints**

Run:
```bash
cargo build -j4 && cargo build -j4 --bin edaptor-tv
cargo clippy -j4 --all-targets -- -D warnings
cargo fmt
```
Expected: clean. (`label_rules`/`LabelRule` must be `pub` in `structure_view.rs` — the explorer confirmed `pub fn label_rules` and `pub struct LabelRule`. If `structure_view` itself is not `pub`, make the needed items reachable: `src/ui/app/mod.rs` already exposes `structure_view`; if not, add `pub use` — verify with the build.)

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(tui): UiState + blocking bootstrap (worker, schema, structure)

Ports the spike bootstrap into a real Rc<RefCell<UiState>>: spawn worker,
fetch subschema -> SchemaModel, eager LoadStructure -> Structure, build
ReadFlow and the compiled label/tree rules. Adds a worker-less test
constructor. Reuses the pure LabelRule/label_rules from structure_view
(moves to src/tui/labels.rs at M5).

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 4: `FieldWidget` trait + registry + read-only presenters

The foundational plugin contract. M1 implements only the read-only `present()` surface (the editing `activate()`/`CommitOutcome` arrive in M2). Pure logic — fully unit-tested, no tvision needed.

**Files:**
- Create: `src/tui/widget.rs`
- Modify: `src/tui/mod.rs` (`pub mod widget;`)

**Interfaces:**
- Consumes: `crate::workflows::form_model::{FormField, WidgetSpec}`.
- Produces:
  - `pub enum Capability { Static, NeedsSchema, NeedsWorkerSearch }`
  - `pub enum CommitOutcome { SetValues(Vec<String>), StageSecret { attrs: Vec<String>, cleartext: String }, SetValuesThenResyncSchema(Vec<String>), Cancelled }` (defined now; consumed in M2)
  - `pub enum Activation { Inline, /* Modal/Immediate added in M2 */ }`
  - `pub trait FieldWidget { fn capability(&self) -> Capability; fn present(&self, field: &FormField) -> String; }`
  - `pub fn present_field(field: &FormField) -> String` (registry entry point M1 uses)

- [ ] **Step 1: Write the failing presenter tests**

Create `src/tui/widget.rs`:

```rust
//! The field-widget plugin contract. M1 implements the read-only `present()`
//! surface; editing (`activate`/`CommitOutcome`) lands in M2.

use crate::workflows::form_model::{FormField, WidgetSpec};

/// What data a widget's editor needs (used by M2 dispatch; declared now).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    Static,
    NeedsSchema,
    NeedsWorkerSearch,
}

/// Typed result an editor returns to the form (consumed in M2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitOutcome {
    SetValues(Vec<String>),
    StageSecret { attrs: Vec<String>, cleartext: String },
    SetValuesThenResyncSchema(Vec<String>),
    Cancelled,
}

/// How a field is edited (M2 adds `Modal`/`Immediate`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Activation {
    Inline,
}

/// One plugin per widget kind. M1 uses only `present`.
pub trait FieldWidget {
    fn capability(&self) -> Capability;
    /// The read-only value-cell text for `field`.
    fn present(&self, field: &FormField) -> String;
}

/// The default plain presenter: schema/value-driven read-only rendering.
pub struct PlainWidget;

impl FieldWidget for PlainWidget {
    fn capability(&self) -> Capability {
        Capability::Static
    }

    fn present(&self, field: &FormField) -> String {
        present_field(field)
    }
}

/// Registry entry point M1 uses for read-only display. Renders a field's value
/// cell from its `WidgetSpec` and value cardinality. (M2 swaps this for a
/// registry keyed by `WidgetKind` that also dispatches `activate`.)
pub fn present_field(field: &FormField) -> String {
    // Multi-value summary takes precedence over per-value formatting.
    if field.values.len() > 1 {
        return format!("‹{} values›", field.values.len());
    }
    let first = field.values.first().map(String::as_str).unwrap_or("");
    match &field.widget {
        WidgetSpec::DisabledCheckBox(b) => (if *b { "[x]" } else { "[ ]" }).to_string(),
        WidgetSpec::BinaryNote(bytes) => format!("<{bytes} bytes>"),
        WidgetSpec::ReadOnlyText
        | WidgetSpec::ReadOnlyInt
        | WidgetSpec::ReadOnlyDn
        | WidgetSpec::ReadOnlyTime => first.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::FieldKind;

    fn field(values: &[&str], widget: WidgetSpec) -> FormField {
        FormField {
            label: "attr".into(),
            kind: FieldKind::Text,
            is_must: false,
            values: values.iter().map(|s| s.to_string()).collect(),
            widget,
        }
    }

    #[test]
    fn test_present_single_text() {
        assert_eq!(present_field(&field(&["hello"], WidgetSpec::ReadOnlyText)), "hello");
    }

    #[test]
    fn test_present_empty_text() {
        assert_eq!(present_field(&field(&[], WidgetSpec::ReadOnlyText)), "");
    }

    #[test]
    fn test_present_multi_summarizes_count() {
        let f = field(&["a", "b", "c"], WidgetSpec::ReadOnlyText);
        assert_eq!(present_field(&f), "‹3 values›");
    }

    #[test]
    fn test_present_checkbox() {
        assert_eq!(present_field(&field(&["TRUE"], WidgetSpec::DisabledCheckBox(true))), "[x]");
        assert_eq!(present_field(&field(&[], WidgetSpec::DisabledCheckBox(false))), "[ ]");
    }

    #[test]
    fn test_present_binary_note() {
        assert_eq!(present_field(&field(&[], WidgetSpec::BinaryNote(2048))), "<2048 bytes>");
    }

    #[test]
    fn test_plain_widget_capability_is_static() {
        assert_eq!(PlainWidget.capability(), Capability::Static);
    }
}
```

- [ ] **Step 2: Register the module**

In `src/tui/mod.rs` add:

```rust
pub mod widget;
```

- [ ] **Step 3: Run the tests to verify they fail then pass**

Run: `cargo test -j4 tui::widget::tests`
Expected: compiles and all six tests PASS. (If `CommitOutcome`/`Activation`/`Capability` trigger dead-code warnings under clippy, they are public API consumed in M2 — that's fine; do NOT add `#[allow(dead_code)]`, public items are not dead. Verify with the clippy step.)

- [ ] **Step 4: Verify lints**

Run:
```bash
cargo clippy -j4 --all-targets -- -D warnings
cargo fmt
```
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(tui): FieldWidget trait + registry + read-only presenters

Defines the plugin contract (Capability/Activation/CommitOutcome/
FieldWidget) and implements the read-only present() surface: plain text,
multi-value count summary, disabled checkbox, binary note. Unit-tested.
Editing (activate/CommitOutcome) is wired in M2.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 5: DIT tree pane (`Outline`) with config-driven labels

A self-seeding `Outline` over the structure's branch hierarchy, labelled by the tree rules, that updates `current_branch` and broadcasts `REFRESH` on selection change. Adapts the spike's `TreePane`/`build_branch_nodes` but reads labels from `eval_tree_label`/`fit_label` and uses 0.1.2's `Outline::value()` (no `ov().foc`, no manual first-event seed).

**Files:**
- Create: `src/tui/panes/tree.rs`
- Create/Modify: `src/tui/mod.rs` (`mod panes;`) and `src/tui/panes/mod.rs` (`pub mod tree;`)

**Interfaces:**
- Consumes: `Shared`, `REFRESH`, `UiState.{structure,tree_rules,branch_dns,current_branch,list_dirty}`, `Structure::{branch_dns,get}`, `crate::config::tree_label::{eval_tree_label,fit_label}`.
- Produces:
  - `pub fn build_branch_nodes(state: &UiState, width: usize) -> (Option<Box<tvision_rs::Node>>, Vec<String>)`
  - `pub struct TreePane { … }` implementing `View`.

- [ ] **Step 1: Create `src/tui/panes/mod.rs`**

```rust
pub mod tree;
```

And in `src/tui/mod.rs` add `mod panes;` (panes are wired by `app.rs` in Task 9; `mod` keeps them compiled and testable now).

- [ ] **Step 2: Write `build_branch_nodes` + the DFS/label test (failing)**

Create `src/tui/panes/tree.rs`:

```rust
//! DIT tree pane: an `Outline` over the structure's branch hierarchy.

use tvision_rs::{
    self as tv, delegate, Context, Event, FieldValue, OutlineViewer, Rect, View,
};

use crate::config::tree_label::{eval_tree_label, fit_label};
use crate::tui::state::UiState;
use crate::tui::{Shared, REFRESH};

/// Build a tvision `Node` tree and a parallel DFS pre-order DN index from the
/// structure's branch hierarchy. Only branches (nodes with ≥1 child) appear;
/// leaves live in pane 2. Labels come from the compiled tree rules, width-fit to
/// `width`. Pre-order matches the `foc` index `Outline` assigns.
pub fn build_branch_nodes(state: &UiState, width: usize) -> (Option<Box<tv::Node>>, Vec<String>) {
    use std::collections::HashSet;
    let branches: HashSet<String> = state.structure.branch_dns().into_iter().collect();
    let mut dns = Vec::new();

    fn rdn_of(dn: &str) -> &str {
        dn.split_once(',').map(|(h, _)| h).unwrap_or(dn)
    }

    fn build(
        dn: &str,
        state: &UiState,
        branches: &std::collections::HashSet<String>,
        width: usize,
        dns: &mut Vec<String>,
    ) -> tv::Node {
        dns.push(dn.to_string());
        let node = state.structure.get(dn);
        let label = match node {
            Some(n) => {
                let segs = eval_tree_label(&state.tree_rules, &n.attrs, rdn_of(dn));
                let fit = fit_label(&segs, width.max(4));
                if fit.is_empty() { rdn_of(dn).to_string() } else { fit }
            }
            None => rdn_of(dn).to_string(),
        };
        let mut tnode = tv::Node::new(&label).with_expanded(true);

        let child_branches: Vec<String> = node
            .map(|n| {
                n.children
                    .iter()
                    .filter(|c| branches.contains(*c))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();

        let mut chain: Option<Box<tv::Node>> = None;
        for cb in child_branches.into_iter().rev() {
            let mut child = build(&cb, state, branches, width, dns);
            if let Some(next) = chain.take() {
                child = child.with_next(next);
            }
            chain = Some(Box::new(child));
        }
        if let Some(children) = chain {
            tnode = tnode.with_children(children);
        }
        tnode
    }

    let root_dn = state.base_dn.clone();
    if branches.contains(&root_dn) || state.structure.get(&root_dn).is_some() {
        let root = build(&root_dn, state, &branches, width, &mut dns);
        (Some(Box::new(root)), dns)
    } else {
        (None, dns)
    }
}

/// Outline pane: updates `current_branch` + `list_dirty` and broadcasts REFRESH
/// when the selected branch changes. (0.1.2 auto-seeds; read selection via
/// `Outline::value()`; call `ov_update` only after a tree mutation — none here.)
pub struct TreePane {
    outline: tv::Outline,
    state: Shared,
    last_sel: i32,
}

impl TreePane {
    pub fn new(bounds: Rect, root: Option<Box<tv::Node>>, state: Shared) -> Self {
        TreePane {
            outline: tv::Outline::new(bounds, None, None, root),
            state,
            last_sel: -1,
        }
    }
}

#[delegate(to = outline)]
impl View for TreePane {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        self.outline.handle_event(ev, ctx);
        let sel = match self.outline.value() {
            Some(FieldValue::Int(i)) => i,
            _ => self.outline.ov().foc, // fallback; 0.1.1+ implements value()
        };
        if sel != self.last_sel {
            self.last_sel = sel;
            let mut updated = false;
            if sel >= 0 {
                let mut st = self.state.borrow_mut();
                if let Some(dn) = st.branch_dns.get(sel as usize).cloned() {
                    st.current_branch = Some(dn);
                    st.list_dirty = true;
                    updated = true;
                }
            } // borrow dropped before broadcast
            if updated {
                ctx.broadcast(REFRESH, None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tree_label::compile_tree_rules;
    use crate::ldap::worker::RawSubschema;
    use crate::schema::SchemaModel;
    use crate::workflows::structure::{Structure, StructureInput};
    use std::collections::BTreeMap;

    fn si(dn: &str) -> StructureInput {
        StructureInput {
            dn: dn.into(),
            cn: None,
            description: None,
            object_classes: vec![],
            attrs: BTreeMap::new(),
        }
    }

    #[test]
    fn test_branch_nodes_dfs_preorder_excludes_leaves() {
        // dc=x (root) -> ou=a (branch, has child) ; ou=b is a childless leaf.
        let inputs = vec![
            si("dc=x"),
            si("ou=a,dc=x"),
            si("ou=b,dc=x"),
            si("cn=1,ou=a,dc=x"),
        ];
        let structure = Structure::build("dc=x", inputs);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let st = UiState::new_for_test(
            structure,
            schema,
            "dc=x".into(),
            Vec::new(),
            compile_tree_rules(&Default::default()),
        );
        let (root, dns) = build_branch_nodes(&st, 40);
        assert!(root.is_some());
        assert_eq!(dns, vec!["dc=x".to_string(), "ou=a,dc=x".to_string()]);
        assert!(!dns.contains(&"ou=b,dc=x".to_string()));
    }
}
```

> If `crate::config::TreeConfig` does not implement `Default`, replace `compile_tree_rules(&Default::default())` in the test with `Vec::new()` (an empty `Vec<CompiledTreeRule>` — `eval_tree_label` falls back to the RDN). Verify which compiles.

- [ ] **Step 3: Register and run the test**

Confirm `src/tui/panes/mod.rs` has `pub mod tree;`. Run:
```bash
cargo test -j4 tui::panes::tree::tests::test_branch_nodes_dfs_preorder_excludes_leaves
```
Expected: PASS.

- [ ] **Step 4: Verify build + facade + lints**

```bash
cargo build -j4 --bin edaptor-tv
cargo clippy -j4 --all-targets -- -D warnings
! grep -rl "use tvision_rs" src | grep -vE "^src/(tui/|bin/edaptor-tv.rs)"
cargo fmt
```
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(tui): DIT tree pane (Outline) with config-driven labels

build_branch_nodes renders the structure's branch hierarchy into a
tvision Node tree (DFS pre-order DN map matching Outline foc), labelled
via eval_tree_label/fit_label. TreePane updates current_branch and
broadcasts REFRESH on selection change, using 0.1.1+ Outline::value()
(no ov().foc, no manual seed). DFS/label logic unit-tested.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 6: Leaf pane (`ListBox` + search) → triggers `ReadFlow::request_entry`

A leaf list populated from `compute_rows` (with config column-2 labels and search filtering), that on selection change submits a base read via `ReadFlow::request_entry`, choosing the profile by the leaf's objectClasses.

**Files:**
- Create: `src/tui/panes/leaf.rs`
- Modify: `src/tui/panes/mod.rs` (`pub mod leaf;`)
- Modify: `src/tui/state.rs` (add `profile_for` + `leaf_rows` helpers)

**Interfaces:**
- Consumes: `Shared`, `REFRESH`, `crate::ui::app::structure_view::compute_rows`, `ReadFlow::request_entry`, `UiState.{structure,current_branch,search,profiles,read_flow,worker,current_leaf,label_rules}`, `Structure::get` (for the selected leaf's objectClasses).
- Produces:
  - `UiState::leaf_rows(&self) -> Vec<(String, String)>` (label, dn)
  - `fn profile_for<'a>(profiles: &'a [EntryProfile], ocs: &[String]) -> Option<&'a EntryProfile>`
  - `pub struct LeafPane { … }` implementing `View`.

- [ ] **Step 1: Add `leaf_rows` + `profile_for` to `state.rs` with tests (failing)**

Append to `src/tui/state.rs` (before the `#[cfg(test)]` block, in `impl UiState`/module scope):

```rust
impl UiState {
    /// (label, dn) rows for the current branch, filtered by `search`, using the
    /// configured column-2 label rules. Empty when no branch is selected.
    pub fn leaf_rows(&self) -> Vec<(String, String)> {
        match &self.current_branch {
            Some(b) => crate::ui::app::structure_view::compute_rows(
                &self.structure,
                b,
                &self.search,
                &self.label_rules,
            ),
            None => Vec::new(),
        }
    }
}

/// First profile whose declared object_classes are all present on the entry.
pub fn profile_for<'a>(
    profiles: &'a [EntryProfile],
    ocs: &[String],
) -> Option<&'a EntryProfile> {
    profiles.iter().find(|p| {
        !p.object_classes.is_empty()
            && p.object_classes
                .iter()
                .all(|need| ocs.iter().any(|have| have.eq_ignore_ascii_case(need)))
    })
}
```

Add to the existing `#[cfg(test)] mod tests` in `state.rs`:

```rust
    #[test]
    fn test_profile_for_matches_all_ocs() {
        let mut p = crate::config::EntryProfile {
            name: "user".into(),
            object_classes: vec!["inetOrgPerson".into()],
            rdn_attr: String::new(),
            search_base: String::new(),
            show: vec![],
            search_attrs: vec![],
            defaults: Default::default(),
            widgets: Default::default(),
            label: None,
        };
        let profiles = vec![p.clone()];
        assert!(profile_for(&profiles, &["inetOrgPerson".into(), "top".into()]).is_some());
        assert!(profile_for(&profiles, &["organizationalUnit".into()]).is_none());
        p.object_classes.clear();
        assert!(profile_for(&[p], &["anything".into()]).is_none());
    }
```

> If `EntryProfile`'s field set differs from the literal above (verify against `src/config/mod.rs:179`), adjust the struct literal to match — the explorer listed: `name, object_classes, rdn_attr, search_base, show, search_attrs, defaults, widgets, label`.

- [ ] **Step 2: Run the helper test**

Run: `cargo test -j4 tui::state::tests::test_profile_for_matches_all_ocs`
Expected: PASS.

- [ ] **Step 3: Write `src/tui/panes/leaf.rs`**

```rust
//! Leaf list pane: a search box over a ListBox of the current branch's leaves.

use tvision_rs::{
    self as tv, delegate, Context, Event, FieldValue, Group, InputLine, Key, ListBox, Rect, View,
};

use crate::tui::state::profile_for;
use crate::tui::{Shared, REFRESH};

/// A search `InputLine` (row 0) above a `ListBox`. Recomputes rows from the
/// shared state on REFRESH and whenever the search text changes; submits a base
/// read via ReadFlow when the selection moves to a new leaf.
pub struct LeafPane {
    group: Group,
    search_id: tv::ViewId,
    list_id: tv::ViewId,
    state: Shared,
    last_sel: i32,
    last_search: String,
    seeded: bool,
}

impl LeafPane {
    pub fn new(bounds: Rect, state: Shared) -> Self {
        let mut group = Group::new(bounds);
        let w = bounds.b.x - bounds.a.x;
        let search = InputLine::with_limit(Rect::new(0, 0, w, 1), 256);
        let search_id = group.insert(Box::new(search));
        let list = ListBox::new(Rect::new(0, 1, w, bounds.b.y - bounds.a.y), 1, None, None);
        let list_id = group.insert(Box::new(list));
        LeafPane {
            group,
            search_id,
            list_id,
            state,
            last_sel: -1,
            last_search: String::new(),
            seeded: false,
        }
    }

    fn repopulate(&mut self, ctx: &mut Context) {
        let rows: Vec<String> = self.state.borrow().leaf_rows().into_iter().map(|(l, _)| l).collect();
        if let Some(list) = self.group.child_mut(self.list_id) {
            if let Some(lb) = list.as_any_mut().and_then(|a| a.downcast_mut::<ListBox>()) {
                lb.new_list(rows, ctx);
            }
        }
        self.last_sel = -1;
    }

    fn submit_selected(&mut self) {
        let sel = match self
            .group
            .child_mut(self.list_id)
            .and_then(|v| v.value())
        {
            Some(FieldValue::Int(i)) => i,
            _ => return,
        };
        if sel == self.last_sel {
            return;
        }
        self.last_sel = sel;

        // Collect dn + objectClasses outside any long-lived borrow.
        let target: Option<(String, Vec<String>)> = {
            let st = self.state.borrow();
            st.leaf_rows().get(sel as usize).map(|(_l, dn)| {
                let ocs = st
                    .structure
                    .get(dn)
                    .map(|n| n.object_classes.clone())
                    .unwrap_or_default();
                (dn.clone(), ocs)
            })
        };
        let Some((dn, ocs)) = target else { return };

        let mut st = self.state.borrow_mut();
        if st.current_leaf.as_deref() == Some(dn.as_str()) {
            return;
        }
        // Disjoint field borrows: worker (read) + read_flow (mut) + profiles (read).
        let crate::tui::state::UiState {
            worker,
            read_flow,
            profiles,
            current_leaf,
            ..
        } = &mut *st;
        if let Some(w) = worker.as_ref() {
            let profile = profile_for(profiles, &ocs);
            if read_flow.request_entry(w, &dn, profile).is_ok() {
                *current_leaf = Some(dn);
            }
        }
    }
}

#[delegate(to = group)]
impl View for LeafPane {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        let is_refresh = matches!(ev, Event::Broadcast { command, .. } if *command == REFRESH);
        if !self.seeded || (is_refresh && self.state.borrow().list_dirty) {
            self.seeded = true;
            self.repopulate(ctx);
            self.state.borrow_mut().list_dirty = false;
        }

        self.group.handle_event(ev, ctx);

        // Sync search text from the InputLine into shared state; recompute on change.
        let cur = match self.group.child_mut(self.search_id).and_then(|v| v.value()) {
            Some(FieldValue::Text(s)) => s,
            _ => String::new(),
        };
        if cur != self.last_search {
            self.last_search = cur.clone();
            self.state.borrow_mut().search = cur;
            self.repopulate(ctx);
        }

        // Submit a read when selection lands on a new leaf.
        if matches!(ev, Event::Key(k) if matches!(k.key, Key::Up | Key::Down)) || is_refresh {
            self.submit_selected();
        }
    }
}
```

> The `downcast_mut::<ListBox>()` requires `ListBox: 'static` and `as_any_mut` returning the inner — `Group::child_mut` yields `&mut dyn View`; `View::as_any_mut` is the documented downcast hook (used in the spike). If `value()`/`new_list` are reachable directly on `&mut dyn View` for the ListBox without downcast in your tvision version, prefer that; otherwise the downcast is the seam. Verify at build time.

- [ ] **Step 4: Headless smoke test (pane constructs + repopulates)**

Add to `src/tui/panes/leaf.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldap::worker::RawSubschema;
    use crate::schema::SchemaModel;
    use crate::tui::state::UiState;
    use crate::workflows::structure::{Structure, StructureInput};
    use std::cell::RefCell;
    use std::collections::{BTreeMap, VecDeque};
    use std::rc::Rc;

    #[test]
    fn test_leaf_pane_lists_rows_for_selected_branch() {
        let inputs = vec![
            StructureInput { dn: "dc=x".into(), cn: None, description: None, object_classes: vec![], attrs: BTreeMap::new() },
            StructureInput { dn: "ou=p,dc=x".into(), cn: None, description: None, object_classes: vec![], attrs: BTreeMap::new() },
            StructureInput { dn: "cn=a,ou=p,dc=x".into(), cn: Some("a".into()), description: None, object_classes: vec![], attrs: BTreeMap::new() },
            StructureInput { dn: "cn=b,ou=p,dc=x".into(), cn: Some("b".into()), description: None, object_classes: vec![], attrs: BTreeMap::new() },
        ];
        let structure = Structure::build("dc=x", inputs);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut state = UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        state.current_branch = Some("ou=p,dc=x".into());
        let shared: Shared = Rc::new(RefCell::new(state));

        // Two leaves + the ‹self› row = 3 rows expected from leaf_rows.
        assert_eq!(shared.borrow().leaf_rows().len(), 3);

        let mut pane = LeafPane::new(Rect::new(0, 0, 30, 10), shared.clone());

        // Drive one timer/refresh-free event through a headless Context to seed.
        let mut out: VecDeque<Event> = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<tv::Deferred> = Vec::new();
        let mut ctx = tv::Context::new(&mut out, &mut timers, 0, &mut deferred);
        let mut ev = Event::Broadcast { command: REFRESH, source: None };
        shared.borrow_mut().list_dirty = true;
        pane.handle_event(&mut ev, &mut ctx);
        // No panic, borrow discipline held; list_dirty cleared.
        assert!(!shared.borrow().list_dirty);
    }
}
```

> Verify the `Event::Broadcast { command, source }` field names against the tvision 0.1.2 source (the spike matched `Event::Broadcast { command, .. }`). If `source` is named differently, adjust the literal.

- [ ] **Step 5: Register, run, lint**

Ensure `src/tui/panes/mod.rs` has `pub mod leaf;`. Run:
```bash
cargo test -j4 tui::panes::leaf::tests
cargo build -j4 --bin edaptor-tv
cargo clippy -j4 --all-targets -- -D warnings
cargo fmt
```
Expected: PASS + clean.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(tui): leaf pane (search + ListBox) drives ReadFlow reads

LeafPane renders the current branch's leaves via compute_rows (column-2
label rules + search filter) and, on selection change, submits a base
read through ReadFlow::request_entry, picking the profile by the leaf's
objectClasses (profile_for). Borrow discipline observed across new_list
and request_entry. Helpers and pane seeding tested headlessly.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 7: `PumpView` — drain the worker into `ReadFlow`, broadcast REFRESH

A zero-area timer view that drains `worker.poll()` each tick, routes responses through `ReadFlow::on_response`, stores the resulting `FormModel` in `UiState`, and broadcasts `REFRESH`.

**Files:**
- Create: `src/tui/pump.rs`
- Modify: `src/tui/mod.rs` (`mod pump;`)
- Modify: `src/tui/state.rs` (add `UiState::pump_worker`)

**Interfaces:**
- Consumes: `Shared`, `REFRESH`, `WorkerHandle::poll`, `ReadFlow::on_response`, `crate::workflows::read_flow::ReadOutcome`.
- Produces:
  - `UiState::pump_worker(&mut self) -> bool`
  - `pub struct PumpView { … }` implementing `View`.

- [ ] **Step 1: Add `pump_worker` to `state.rs` with a test (failing)**

Append to `impl UiState` in `src/tui/state.rs`:

```rust
impl UiState {
    /// Drain ready worker responses through ReadFlow; install a FormModel when a
    /// pending read returns. Returns true if anything changed (caller broadcasts
    /// REFRESH). No-op without a worker (test instances). Borrow-safe: collects
    /// responses before touching read_flow.
    pub fn pump_worker(&mut self) -> bool {
        use crate::workflows::read_flow::ReadOutcome;
        let mut resps = Vec::new();
        if let Some(w) = self.worker.as_ref() {
            while let Some(r) = w.poll() {
                resps.push(r);
            }
        }
        let mut changed = false;
        for resp in &resps {
            match self.read_flow.on_response(resp) {
                ReadOutcome::Form { model, .. } => {
                    self.form = Some(model);
                    self.form_dirty = true;
                    changed = true;
                }
                ReadOutcome::Error(msg) => {
                    self.form = Some(error_form(&msg));
                    self.form_dirty = true;
                    changed = true;
                }
                ReadOutcome::Ignored => {}
            }
        }
        changed
    }
}

/// A one-field FormModel used to surface a read error in the form pane.
fn error_form(msg: &str) -> crate::workflows::form_model::FormModel {
    use crate::schema::FieldKind;
    use crate::workflows::form_model::{FormField, FormModel, WidgetSpec};
    FormModel {
        title: "error".into(),
        fields: vec![FormField {
            label: "error".into(),
            kind: FieldKind::Text,
            is_must: false,
            values: vec![msg.to_string()],
            widget: WidgetSpec::ReadOnlyText,
        }],
    }
}
```

Add to `state.rs` tests:

```rust
    #[test]
    fn test_pump_worker_noop_without_worker() {
        let structure = Structure::build("dc=x", vec![si("dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st = UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        assert!(!st.pump_worker());
        assert!(st.form.is_none());
    }
```

- [ ] **Step 2: Run the test**

Run: `cargo test -j4 tui::state::tests::test_pump_worker_noop_without_worker`
Expected: PASS. (Confirms `ReadOutcome` variant names + `on_response` signature compile against the live API.)

- [ ] **Step 3: Write `src/tui/pump.rs`**

```rust
//! Zero-area timer view that drains the async LDAP worker into shared state.

use tvision_rs::{self as tv, Context, DrawCtx, Event, View};

use crate::tui::{Shared, REFRESH};

/// Arms a ~20Hz periodic timer on its first event, then drains the worker each
/// tick. `Event::Timer` is broadcast-class in tvision-rs, so this zero-area,
/// never-drawn view still receives every tick.
pub struct PumpView {
    vs: tv::ViewState,
    state: Shared,
    armed: bool,
}

impl PumpView {
    pub fn new(state: Shared) -> Self {
        PumpView {
            vs: tv::ViewState::new(tv::Rect::new(0, 0, 0, 0)),
            state,
            armed: false,
        }
    }
}

impl View for PumpView {
    fn state(&self) -> &tv::ViewState {
        &self.vs
    }
    fn state_mut(&mut self) -> &mut tv::ViewState {
        &mut self.vs
    }
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }
    fn draw(&mut self, _ctx: &mut DrawCtx) {}

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        if !self.armed {
            self.armed = true;
            ctx.set_timer(
                std::time::Duration::from_millis(50),
                Some(std::time::Duration::from_millis(50)),
            );
        }
        if matches!(ev, Event::Timer(_)) {
            let changed = self.state.borrow_mut().pump_worker();
            if changed {
                ctx.broadcast(REFRESH, None);
            }
        }
    }
}
```

Add `mod pump;` to `src/tui/mod.rs`.

- [ ] **Step 4: Verify build + facade + lints**

```bash
cargo build -j4 --bin edaptor-tv
cargo clippy -j4 --all-targets -- -D warnings
! grep -rl "use tvision_rs" src | grep -vE "^src/(tui/|bin/edaptor-tv.rs)"
cargo fmt
```
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(tui): PumpView drains the worker into ReadFlow

UiState::pump_worker drains worker.poll(), routes each response through
ReadFlow::on_response, installs the resulting FormModel (or a one-field
error form), and reports change. PumpView arms a 20Hz periodic timer and
broadcasts REFRESH when state changes — the spike's proven pump, now on
the real ReadFlow path. No-op path unit-tested.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 8: Read-only form pane (`Group`) rendering `FormModel` via the registry

A `Group` of read-only `InputLine` rows that fills from the current `FormModel` on REFRESH, each row rendered `label: value` with the value produced by `widget::present_field`.

**Files:**
- Create: `src/tui/panes/form.rs`
- Modify: `src/tui/panes/mod.rs` (`pub mod form;`)

**Interfaces:**
- Consumes: `Shared`, `REFRESH`, `UiState.{form,form_dirty}`, `crate::tui::widget::present_field`, `crate::workflows::form_model::FormModel`.
- Produces: `pub struct FormPane { … }` implementing `View`; `fn render_rows(model: &FormModel) -> Vec<String>`.

- [ ] **Step 1: Write `render_rows` + test (failing), then the pane**

Create `src/tui/panes/form.rs`:

```rust
//! Read-only entry form pane: one InputLine row per field, `label: value`.

use tvision_rs::{self as tv, delegate, Context, Event, FieldValue, Group, InputLine, Rect, View};

use crate::tui::widget::present_field;
use crate::tui::{Shared, REFRESH};
use crate::workflows::form_model::FormModel;

const FORM_ROWS: usize = 32;

/// Render a FormModel into `"label: value"` strings (MUST marked with `*`).
fn render_rows(model: &FormModel) -> Vec<String> {
    model
        .fields
        .iter()
        .map(|f| {
            let marker = if f.is_must { " *" } else { "" };
            format!("{}{}: {}", f.label, marker, present_field(f))
        })
        .collect()
}

pub struct FormPane {
    group: Group,
    rows: Vec<tv::ViewId>,
    state: Shared,
}

impl FormPane {
    pub fn new(bounds: Rect, state: Shared) -> Self {
        let mut group = Group::new(bounds);
        let w = bounds.b.x - bounds.a.x;
        let mut rows = Vec::new();
        for i in 0..FORM_ROWS {
            let y = i as i32;
            let il = InputLine::with_limit(Rect::new(0, y, w, y + 1), 1024);
            rows.push(group.insert(Box::new(il)));
        }
        FormPane { group, rows, state }
    }
}

#[delegate(to = group)]
impl View for FormPane {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        let is_refresh = matches!(ev, Event::Broadcast { command, .. } if *command == REFRESH);
        if is_refresh && self.state.borrow().form_dirty {
            let lines: Vec<String> = {
                let mut st = self.state.borrow_mut();
                st.form_dirty = false;
                st.form.as_ref().map(render_rows).unwrap_or_default()
            }; // borrow dropped before mutating children
            for (i, &id) in self.rows.iter().enumerate() {
                let text = lines.get(i).cloned().unwrap_or_default();
                if let Some(child) = self.group.child_mut(id) {
                    child.set_value(FieldValue::Text(text));
                }
            }
        }
        self.group.handle_event(ev, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::FieldKind;
    use crate::workflows::form_model::{FormField, WidgetSpec};

    #[test]
    fn test_render_rows_labels_and_must_marker() {
        let model = FormModel {
            title: "cn=a,dc=x".into(),
            fields: vec![
                FormField { label: "cn".into(), kind: FieldKind::Text, is_must: true, values: vec!["a".into()], widget: WidgetSpec::ReadOnlyText },
                FormField { label: "mail".into(), kind: FieldKind::Text, is_must: false, values: vec!["a@x".into(), "b@x".into()], widget: WidgetSpec::ReadOnlyText },
            ],
        };
        let rows = render_rows(&model);
        assert_eq!(rows[0], "cn *: a");
        assert_eq!(rows[1], "mail: ‹2 values›");
    }
}
```

Add `pub mod form;` to `src/tui/panes/mod.rs`.

- [ ] **Step 2: Run the test**

Run: `cargo test -j4 tui::panes::form::tests::test_render_rows_labels_and_must_marker`
Expected: PASS.

- [ ] **Step 3: Verify build + lints**

```bash
cargo build -j4 --bin edaptor-tv
cargo clippy -j4 --all-targets -- -D warnings
cargo fmt
```
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(tui): read-only form pane renders FormModel via the registry

FormPane fills a Group of read-only InputLine rows from the current
FormModel on REFRESH, each rendered 'label[ *]: value' with the value
produced by widget::present_field. render_rows unit-tested; borrow
discipline observed across child_mut/set_value.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 9: Assemble the three-pane `Splitter` and wire everything in `app.rs`

Compose the desktop: a window holding a three-column `Splitter` (tree | leaf | form) plus the zero-area `PumpView`, with the DFS branch-DN map recorded into `UiState`. This is the integration task; acceptance is the live `edaptor-tv` run.

**Files:**
- Create: `src/tui/app.rs`
- Modify: `src/tui/mod.rs` (move desktop/menu/status into `app.rs`; `run` delegates to it)

**Interfaces:**
- Consumes: everything from Tasks 3–8.
- Produces: `pub(crate) fn build_program(state: Shared) -> Program` (or inline in `run`).

- [ ] **Step 1: Write `src/tui/app.rs`**

```rust
//! Program assembly: desktop, menu bar, status line, three-pane splitter, pump.

use tvision_rs::{
    self as tv, alt, Command, Constraints, Desktop, Program, Rect, Splitter, StatusDef,
    StatusLine, SystemClock, Theme, View, Window,
};

use crate::tui::panes::{form::FormPane, leaf::LeafPane, tree::{build_branch_nodes, TreePane}};
use crate::tui::pump::PumpView;
use crate::tui::Shared;

fn init_status_line(r: Rect) -> Option<Box<dyn View>> {
    let mut r = r;
    r.a.y = r.b.y - 1;
    let defs = StatusDef::list()
        .def_all(|d| d.item("~Alt-X~ Exit", alt('x'), Command::QUIT))
        .build();
    Some(Box::new(StatusLine::new(r, defs)))
}

fn init_menu_bar(r: Rect) -> Option<Box<dyn View>> {
    let mut r = r;
    r.b.y = r.a.y + 1;
    let menu = tv::Menu::builder()
        .submenu("~F~ile", alt('f'), |m| {
            m.command_key("E~x~it", Command::QUIT, alt('x'), "Alt-X")
        })
        .build();
    Some(Box::new(tv::MenuBar::new(r, menu)))
}

fn init_desktop(r: Rect, state: Shared) -> Option<Box<dyn View>> {
    let mut r = r;
    r.a.y += 1;
    r.b.y -= 1;
    let mut desktop = Desktop::new(r, |br| Some(Desktop::init_background(br)));

    let win_rect = Rect::new(r.a.x + 1, r.a.y, r.b.x - 1, r.b.y);
    let mut win = Window::new(win_rect, Some("edaptor".to_string()), 1);
    let ext = win.state().get_extent();
    let interior = Rect::new(1, 1, ext.b.x - 1, ext.b.y - 1);
    let width = (interior.b.x - interior.a.x).max(8) as usize;

    // Build the branch tree and record the DFS DN map.
    let (root, dn_map) = build_branch_nodes(&state.borrow(), width / 3);
    state.borrow_mut().branch_dns = dn_map;

    let tree: Box<dyn View> = Box::new(TreePane::new(interior, root, state.clone()));
    let leaf: Box<dyn View> = Box::new(LeafPane::new(interior, state.clone()));
    let form: Box<dyn View> = Box::new(FormPane::new(interior, state.clone()));

    let split = Splitter::cols()
        .pane(tree, Constraints::flex().min(16))
        .pane(leaf, Constraints::flex().min(16))
        .pane(form, Constraints::flex().min(20))
        .joined();

    let split_id = win.insert_child(Box::new(split));
    if let Some(v) = win.child_mut(split_id) {
        v.change_bounds(interior);
    }
    win.insert_child(Box::new(PumpView::new(state.clone())));

    desktop.insert_view(Box::new(win));
    Some(Box::new(desktop))
}

pub(crate) fn build_program(backend: Box<dyn tv::Backend>, state: Shared) -> Program {
    let s = state.clone();
    Program::new(
        backend,
        Box::new(SystemClock::new()),
        Theme::classic_blue(),
        move |r| init_desktop(r, s.clone()),
        init_status_line,
        init_menu_bar,
    )
}
```

- [ ] **Step 2: Simplify `src/tui/mod.rs` `run` to delegate**

Replace the body of `run` (and delete the now-duplicated `init_*` helpers from `mod.rs`, which now live in `app.rs`):

```rust
mod app;

pub fn run(config: Config, password: String) -> Result<()> {
    let state: Shared = Rc::new(RefCell::new(state::bootstrap(config, password)?));
    let backend = Box::new(CrosstermBackend::new()?);
    let mut program = app::build_program(backend, state);
    program.run_app(|_prog, _cmd| {});
    Ok(())
}
```

Remove the now-unused imports from `mod.rs` (`Desktop`, `StatusDef`, etc. moved to `app.rs`); keep `CrosstermBackend`, `Program` if still referenced (the `build_program` call removes the need — trim to what compiles).

- [ ] **Step 3: Build, lint, facade**

```bash
cargo build -j4 && cargo build -j4 --bin edaptor-tv
cargo clippy -j4 --all-targets -- -D warnings
cargo fmt --check
! grep -rl "use tvision_rs" src | grep -vE "^src/(tui/|bin/edaptor-tv.rs)"
! grep -rl "use ratatui\|use tui_" src | grep -vE "^src/ui/"
cargo test -j4
```
Expected: everything builds; full test suite green; facade guards print nothing.

- [ ] **Step 4: Live acceptance (manual — needs a human at a terminal)**

```bash
scripts/test-ldap.sh start
export EDAPTOR_TEST_ADMIN_PW=adminpassword
cargo run -j4 --bin edaptor-tv -- examples/demo-config.toml
# Verify: three panes render (tree | leaf+search | form). Arrow through the DIT
# tree → leaf list updates. Arrow/select a leaf → the form fills (~50-100ms) with
# that entry's attributes, MUST marked '*', multi-values shown '‹N values›',
# checkboxes/binary noted. Type in the search box → leaf list filters. Alt-X quits.
scripts/test-ldap.sh stop
```
Expected: all of the above. (If no TTY is available in this session, hand this step to the user and record their confirmation.)

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(tui): assemble three-pane read core (tree | leaf | form)

build_program composes the desktop: a window with a three-column joined
Splitter (DIT Outline | leaf search+ListBox | read-only form Group) plus
the zero-area PumpView, recording the DFS branch-DN map into UiState.
run() now delegates to app::build_program. Completes M1: navigate DIT ->
leaf -> read a real entry, driven by the real worker/ReadFlow/FormModel.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Self-Review (completed during planning)

- **Spec coverage (M1 acceptance):** three-pane Splitter (T9); Outline DIT nav + tree labels (T5); ListBox+search + column-2 labels (T6); read-only form via ReadFlow/FormModel (T6 read trigger, T7 pump, T8 render); FieldWidget trait + registry + read-only present() (T4); relocate FormModel out of ui (T1); tvision dep + dev binary, ratatui still builds (T2); headless view tests (T3–T8); manual live acceptance (T9). All M1 spec bullets map to a task.
- **Placeholder scan:** no TBD/TODO; every code step shows real code; tests have real assertions. Verification commands are exact.
- **Type consistency:** `FormModel`/`FormField`/`WidgetSpec`/`FieldKind`, `ReadFlow::{request_entry,on_response}` → `ReadOutcome::{Form{model,object_classes},Error,Ignored}`, `WorkerHandle::{spawn,request,poll}`, `Structure::{build,branch_dns,get,…}`, `Outline::value()→FieldValue::Int`, `present_field` — all used consistently with the explored signatures.
- **Verify-at-build caveats flagged inline** (downcast seam in T6; `Event::Broadcast` field names; `TreeConfig: Default`; `EntryProfile` literal) — each step says to confirm against the live source, which the implementer has.
