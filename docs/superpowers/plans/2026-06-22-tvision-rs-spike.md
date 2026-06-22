# tvision-rs Migration Spike — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prove edaptor's core 3-pane navigation+display UX runs on our own `tvision-rs` (v0.1.0) against the real demo LDAP server, and produce a gap-list + effort estimate for a full UI migration.

**Architecture:** A throwaway `src/bin/spike-tv.rs`, gated behind a `spike-tv` cargo feature so the default build/tests and the shipping ratatui UI are untouched. It reuses edaptor's existing domain layer (`config`, `ldap::worker`, `schema`, `workflows::{structure,read_flow}`) verbatim and drives a tvision-rs `Program`: an outer `Splitter` of `Outline` (DIT tree) │ `ListBox` (leaves) │ form `Group`. Cross-pane state lives in a single `Rc<RefCell<SpikeState>>` shared by three custom wrapper views; navigation deltas are detected in `handle_event` and broadcast a `REFRESH` command; the async LDAP worker is drained on a periodic timer.

**Tech Stack:** Rust (edaptor lib, edition 2021), `tvision-rs` aliased as `tv` (edition 2024 — fine as a dependency of a 2021 crate), podman OpenLDAP demo server.

## Global Constraints

- **Cap build/test parallelism at 4 cores** (shared machine): always `cargo … -j4`.
- **English** for all code, comments, identifiers. UI strings may be German where edaptor already is.
- **Containers use podman**, not docker.
- **Do NOT modify anything under `src/ui`.** It remains the shipping UI and the fallback. The spike is additive and isolated behind the `spike-tv` feature.
- **Do NOT import `edaptor::ui::*`** from the spike — those are tty-facing / `pub(crate)`. Reimplement the trivial `StructureNodeRaw → StructureInput` conversion inline.
- **`tvision-rs` is consumed as a path dependency** to a local sibling checkout so findings → tvision-rs edits flow back immediately during co-development.
- **Demo server auth:** `EDAPTOR_TEST_ADMIN_PW=adminpassword`; config at `examples/demo-config.toml`.
- The spike is a **throwaway probe** — no production polish required.
- **`make check` must still pass** with the feature OFF (the default), since the spike code is feature-gated out.

---

### Task 1: Spike binary scaffold + headless domain bootstrap

Stand up the feature-gated binary and prove edaptor's domain layer boots and yields real DIT data — with **no TUI yet**. This isolates "does the domain layer plug in" from any tvision-rs concern.

**Files:**
- Modify: `Cargo.toml` (add `tvision-rs` dep, `spike-tv` feature, `[[bin]]` entry)
- Create: `src/bin/spike-tv.rs`

**Interfaces:**
- Consumes (from edaptor lib, verified signatures):
  - `edaptor::config::Config::load(&Path) -> anyhow::Result<Config>`; `config.server.base_dn: String`, `config.profiles: Vec<EntryProfile>`, `config.auth` (resolve password).
  - `edaptor::ldap::worker::WorkerHandle::spawn(config: Config, password: String) -> Result<WorkerHandle>`; `.request(Request) -> Result<Response>`; `.submit(Request) -> Result<()>`; `.poll() -> Option<Response>`.
  - `Request::FetchSubschema`; `Request::LoadStructure { id, base, page_size, attrs }`; `Request::Search { id, base, scope, filter, attrs, size_limit }`; `SearchScope::Base`.
  - `Response::Subschema(RawSubschema)`, `Response::StructureEntries { id, nodes, .. }`, `Response::Entries { id, entries, truncated }`, `Response::SearchError { id, msg }`.
  - `worker::StructureNodeRaw { dn, cn, description, object_classes, attrs }`, `worker::LdapEntry { dn, attrs: BTreeMap<String,Vec<String>>, bin_attrs }`.
  - `edaptor::schema::SchemaModel::from_raw(&RawSubschema) -> SchemaModel`.
  - `edaptor::workflows::structure::{Structure, StructureInput, StructureNode}`; `Structure::build(root: &str, inputs: Vec<StructureInput>) -> Structure`; `.branch_dns() -> Vec<String>`; `.leaves_of(&str) -> Vec<&StructureNode>`; `StructureNode { dn, label, .. }`.
- Produces (consumed by later tasks):
  - `fn bootstrap() -> anyhow::Result<Boot>` where `struct Boot { worker: WorkerHandle, structure: Structure, base_dn: String, profiles: Vec<EntryProfile> }`.
  - `fn to_input(n: StructureNodeRaw) -> StructureInput` (inline converter — the two structs share field names).

- [ ] **Step 1: Add the dependency, feature, and bin entry to `Cargo.toml`**

Append to `[dependencies]` (path points at a sibling checkout — clone `git@github.com:oetiker/tvision-rs.git` next to `edaptor` first):

```toml
# Spike only: our own Turbo Vision port, aliased `tv` per its house style.
# Pulled in only with --features spike-tv so the default build is unaffected.
tvision-rs = { package = "tvision-rs", path = "../tvision-rs", optional = true }
```

Add a features section (edaptor has none yet) and the bin entry:

```toml
[features]
spike-tv = ["dep:tvision-rs"]

[[bin]]
name = "spike-tv"
path = "src/bin/spike-tv.rs"
required-features = ["spike-tv"]
```

- [ ] **Step 2: Verify the feature is wired and OFF by default**

Run: `cargo build -j4` then `cargo build -j4 --features spike-tv 2>&1 | head -20`
Expected: the plain build does NOT pull `tvision-rs` (not in `cargo tree -j4`); the featured build resolves the path dep (it may fail to *compile* the not-yet-written bin — that's fine, we only check resolution here). If the path is wrong, fix it before continuing.

- [ ] **Step 3: Write the bootstrap + headless main in `src/bin/spike-tv.rs`**

```rust
//! Throwaway spike: drive edaptor's domain layer onto tvision-rs.
//! Build/run: `cargo run -j4 --features spike-tv --bin spike-tv`
//! Requires the podman demo server (scripts/test-ldap.sh start) and
//! EDAPTOR_TEST_ADMIN_PW=adminpassword.

use std::path::Path;

use anyhow::{anyhow, Result};
use edaptor::config::Config;
use edaptor::ldap::worker::{Request, Response, StructureNodeRaw, WorkerHandle};
use edaptor::schema::SchemaModel;
use edaptor::workflows::structure::{Structure, StructureInput};

/// Everything the TUI needs, produced by a blocking startup sequence.
struct Boot {
    worker: WorkerHandle,
    structure: Structure,
    base_dn: String,
    profiles: Vec<edaptor::config::EntryProfile>,
}

/// `StructureNodeRaw` (worker) → `StructureInput` (structure model). The two
/// share field names; this is the inline replacement for the `pub(crate)`
/// `ui::app::structure_view::structure_inputs` we must not import.
fn to_input(n: StructureNodeRaw) -> StructureInput {
    StructureInput {
        dn: n.dn,
        cn: n.cn,
        description: n.description,
        object_classes: n.object_classes,
        attrs: n.attrs,
    }
}

fn bootstrap() -> Result<Boot> {
    let config = Config::load(Path::new("examples/demo-config.toml"))?;
    let base_dn = config.server.base_dn.clone();
    let profiles = config.profiles.clone();

    let password = std::env::var("EDAPTOR_TEST_ADMIN_PW")
        .map_err(|_| anyhow!("set EDAPTOR_TEST_ADMIN_PW (demo: adminpassword)"))?;

    let worker = WorkerHandle::spawn(config, password)?;

    // Schema (blocking request).
    let raw = match worker.request(Request::FetchSubschema)? {
        Response::Subschema(raw) => raw,
        other => return Err(anyhow!("FetchSubschema: unexpected {other:?}")),
    };
    let _schema = SchemaModel::from_raw(&raw); // kept for the form task

    // Eager DIT scan (blocking request).
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

    Ok(Boot { worker, structure, base_dn, profiles })
}

fn main() -> Result<()> {
    let boot = bootstrap()?;
    let branches = boot.structure.branch_dns();
    println!("base_dn = {}", boot.base_dn);
    println!("profiles = {}", boot.profiles.len());
    println!("branches = {}", branches.len());
    if let Some(first) = branches.first() {
        let leaves = boot.structure.leaves_of(first);
        println!("first branch {first} has {} leaves", leaves.len());
        for leaf in leaves.iter().take(5) {
            println!("  - {} [{}]", leaf.label, leaf.dn);
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Run against the demo server and verify real data**

```bash
scripts/test-ldap.sh start
export EDAPTOR_TEST_ADMIN_PW=adminpassword
cargo run -j4 --features spike-tv --bin spike-tv
```
Expected: prints the demo base DN, a non-zero branch count (~25+ groups/OUs), and the first branch's leaves with real labels/DNs. If `Config::load`, `to_input`, or any `Response` arm mismatches the real types, fix against the compiler/`cargo doc` before moving on.

- [ ] **Step 5: Confirm default build is unaffected, then commit**

```bash
cargo build -j4 && cargo clippy -j4 --all-targets -- -D warnings
git add Cargo.toml src/bin/spike-tv.rs
git commit -m "spike(tv): feature-gated binary + headless domain bootstrap"
```
Expected: clean build/clippy with the feature off. (Clippy with `--features spike-tv` is exercised in later tasks once the TUI compiles.)

---

### Task 2: Minimal tvision-rs program skeleton

Replace the headless `main` with the smallest live tvision-rs app — desktop, an empty window, a status line, Alt-X to quit. Proves tvision-rs links and its event loop runs in this repo.

**Files:**
- Modify: `src/bin/spike-tv.rs`

**Interfaces:**
- Consumes (tvision-rs, verified): `tv::{Backend, Command, CrosstermBackend, Desktop, Program, Rect, StatusDef, StatusLine, SystemClock, Theme, View, Window, alt}`; `Program::new(backend, clock, theme, init_desktop, init_status_line, init_menu_bar)`; `Program::run_app(|&mut Program, Command| {})`; `Desktop::new(rect, |br| Some(Desktop::init_background(br)))`; `desktop.insert_view(Box<dyn View>)`; `Window::new(rect, Some(title), number)`.
- Produces: `struct SpikeApp { program: Program }` with `fn new(Box<dyn Backend>) -> Self`, `fn run(&mut self) -> Command`, and the three `init_*` factories — the scaffold every later task extends.

- [ ] **Step 1: Add the tvision-rs imports and the `SpikeApp` skeleton**

Add the import (mirrors `examples/splitter.rs:37-41`, trimmed):

```rust
use std::io;
use tvision_rs::{
    self as tv, Backend, Command, CrosstermBackend, Desktop, Program, Rect, StatusDef,
    StatusLine, SystemClock, Theme, View, Window, alt,
};
```

Add the app type and factories (ported from `examples/splitter.rs:181-274`):

```rust
struct SpikeApp {
    program: Program,
}

impl SpikeApp {
    fn new(backend: Box<dyn Backend>) -> Self {
        let program = Program::new(
            backend,
            Box::new(SystemClock::new()),
            Theme::classic_blue(),
            Self::init_desktop,
            Self::init_status_line,
            Self::init_menu_bar,
        );
        SpikeApp { program }
    }

    fn init_desktop(r: Rect) -> Option<Box<dyn View>> {
        let mut r = r;
        r.a.y += 1; // below menu bar
        r.b.y -= 1; // above status line
        let mut desktop = Desktop::new(r, |br| Some(Desktop::init_background(br)));
        let win_rect = Rect::new(r.a.x + 2, r.a.y + 1, r.b.x - 2, r.b.y - 1);
        let win = Window::new(win_rect, Some("edaptor (tvision spike)".to_string()), 1);
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

    fn run(&mut self) -> Command {
        self.program.run_app(|_prog, _cmd| {})
    }
}
```

- [ ] **Step 2: Rewrite `main` to launch the app (keep `bootstrap()` for the next task)**

```rust
fn main() -> io::Result<()> {
    // bootstrap() is wired into the app in Task 4; keep it referenced so it
    // does not warn as dead code.
    let _ = bootstrap; // silence unused until Task 4
    let mut app = SpikeApp::new(Box::new(CrosstermBackend::new()?));
    let _ = app.run();
    Ok(())
}
```

- [ ] **Step 3: Build with the feature and run interactively**

```bash
cargo build -j4 --features spike-tv 2>&1 | tail -20
cargo run -j4 --features spike-tv --bin spike-tv
```
Expected: a Turbo-Vision desktop with a single titled window, a File menu, and a status line. `Alt-X` quits cleanly and restores the terminal. If the screen does not paint, note it as finding-stream-2.3 material (startup-clear behaviour) and check `examples/hello.rs` for any missing init call.

- [ ] **Step 4: Commit**

```bash
git add src/bin/spike-tv.rs
git commit -m "spike(tv): minimal Program/Desktop/StatusLine skeleton"
```

---

### Task 3: Static three-pane splitter

Drop the splitter.rs three-pane layout (placeholder data) into the window. Proves the headline UX: resizable panes (mouse drag + Ctrl-F5 keyboard resize) and Tab focus cycling.

**Files:**
- Modify: `src/bin/spike-tv.rs`

**Interfaces:**
- Consumes (tvision-rs, verified from `examples/splitter.rs`): `tv::{Button, ButtonFlags, Constraints, Context, Event, Group, InputLine, Label, ListBox, Node, Outline, Splitter, delegate}`; `Splitter::cols()/rows()`, `.pane(Box<dyn View>, Constraints)`, `.joined()`, `.insert(view, c) -> ViewId`; `Constraints::flex().min(n)`; `Outline::new(bounds, None, None, Some(root_node))`; `Node::new(&str).with_expanded(bool).with_children(Box<Node>).with_next(Box<Node>)`; `ListBox::new(bounds, 1, None, None)`, `list.new_list(Vec<String>, &mut Context)`; `Group::new(bounds)`, `group.insert(Box<dyn View>) -> ViewId`; `InputLine::with_limit(bounds, limit)`; `Label::new(bounds, "~N~ame", Some(id))`; `win.insert_child(view) -> ViewId`, `win.child_mut(id)`, `win.state().get_extent()`, `v.change_bounds(rect)`; `Ctrl-F5` → `Command::RESIZE`.
- Produces: `struct ListPane { list: ListBox, items: Vec<String>, seeded: bool }` with the `#[delegate(to = list)]` View impl (seed-on-first-event) — the wrapper Task 6 evolves into the live leaf pane; `fn build_tree/build_list/build_form(bounds) -> Box<dyn View>`.

- [ ] **Step 1: Port `ListPane` and the three pane builders verbatim**

Copy `ListPane` (`examples/splitter.rs:64-95`) and `build_tree` / `build_list` / `build_form` (`examples/splitter.rs:102-175`) into `src/bin/spike-tv.rs` unchanged (placeholder Animals/fruit/Name+City data). Add `Ctrl-F5` helper from `examples/splitter.rs:43-51`.

- [ ] **Step 2: Assemble the splitter inside the window in `init_desktop`**

Replace the empty-window body with the nested-splitter assembly from `examples/splitter.rs:206-242`:

```rust
let mut win = Window::new(win_rect, Some("edaptor (tvision spike)".to_string()), 1);
let ext = win.state().get_extent();
let interior = Rect::new(1, 1, ext.b.x - 1, ext.b.y - 1);

let left = Splitter::rows()
    .pane(build_tree(interior), Constraints::flex().min(3))
    .pane(build_list(interior), Constraints::flex().min(3));
let middle = build_form(interior);
let right = Splitter::rows()
    .pane(build_list(interior), Constraints::flex().min(3))
    .pane(build_form(interior), Constraints::flex().min(6));
let split = Splitter::cols()
    .pane(Box::new(left), Constraints::flex().min(16))
    .pane(middle, Constraints::flex().min(16))
    .pane(Box::new(right), Constraints::flex().min(16))
    .joined();
let split_id = win.insert_child(Box::new(split));
if let Some(v) = win.child_mut(split_id) {
    v.change_bounds(interior);
}
desktop.insert_view(Box::new(win));
```

Update the status line to advertise resize: `.item("~Ctrl-F5~ Resize", ctrl_f5(), Command::RESIZE).item("~Alt-X~ Exit", alt('x'), Command::QUIT)`.

- [ ] **Step 3: Run and verify the splitter UX**

```bash
cargo run -j4 --features spike-tv --bin spike-tv
```
Expected: three columns with joined linework; left/right columns each split into two rows; **mouse-drag a divider seam resizes** neighbouring panes; **Ctrl-F5 → Tab → arrows** nudges a divider; **Tab** cycles focus across panes and form fields; the form's Name/City `InputLine`s accept typing. Record any friction (e.g. how non-obvious the resize affordance is) for the findings doc.

- [ ] **Step 4: Commit**

```bash
git add src/bin/spike-tv.rs
git commit -m "spike(tv): static three-pane splitter layout"
```

---

### Task 4: SpikeState shared model

Introduce the single shared state object the panes and worker pump coordinate through, and feed the Task 1 bootstrap into it. No visible behaviour change yet — this locks in the cross-pane architecture.

**Files:**
- Modify: `src/bin/spike-tv.rs`

**Interfaces:**
- Consumes: `Boot` (Task 1); `tv::Command`.
- Produces:
  - `const REFRESH: Command = Command::custom("spike.refresh");`
  - `type Shared = std::rc::Rc<std::cell::RefCell<SpikeState>>;`
  - `struct SpikeState { worker: WorkerHandle, structure: Structure, base_dn: String, current_branch: Option<String>, current_leaf: Option<String>, entry: Vec<(String, String)>, list_dirty: bool, form_dirty: bool, pending_read: Option<u64> }`
  - `impl SpikeState { fn new(boot: Boot) -> Self; fn leaves(&self) -> Vec<(String,String)>; fn pump_worker(&mut self) -> bool }` (`pump_worker` returns `true` if any state changed — used in Task 7).

- [ ] **Step 1: Define `SpikeState`, `Shared`, and `REFRESH`**

```rust
use std::cell::RefCell;
use std::rc::Rc;
use edaptor::ldap::worker::{Response, SearchScope};

const REFRESH: tv::Command = tv::Command::custom("spike.refresh");

type Shared = Rc<RefCell<SpikeState>>;

struct SpikeState {
    worker: WorkerHandle,
    structure: Structure,
    base_dn: String,
    current_branch: Option<String>,
    current_leaf: Option<String>,
    /// (attr, value) lines for the currently displayed entry.
    entry: Vec<(String, String)>,
    list_dirty: bool,
    form_dirty: bool,
    pending_read: Option<u64>,
}

impl SpikeState {
    fn new(boot: Boot) -> Self {
        SpikeState {
            worker: boot.worker,
            structure: boot.structure,
            base_dn: boot.base_dn,
            current_branch: None,
            current_leaf: None,
            entry: Vec::new(),
            list_dirty: false,
            form_dirty: false,
            pending_read: None,
        }
    }

    /// (label, dn) rows for the selected branch — synchronous from the eager model.
    fn leaves(&self) -> Vec<(String, String)> {
        match &self.current_branch {
            Some(b) => self
                .structure
                .leaves_of(b)
                .into_iter()
                .map(|n| (n.label.clone(), n.dn.clone()))
                .collect(),
            None => Vec::new(),
        }
    }

    /// Drain ready worker responses; fill `entry` when our pending read returns.
    /// Returns true if anything changed (so the caller can broadcast REFRESH).
    fn pump_worker(&mut self) -> bool {
        let mut changed = false;
        while let Some(resp) = self.worker.poll() {
            match resp {
                Response::Entries { id, entries, .. } if Some(id) == self.pending_read => {
                    self.pending_read = None;
                    self.entry = entries
                        .first()
                        .map(|e| {
                            e.attrs
                                .iter()
                                .flat_map(|(k, vs)| {
                                    vs.iter().map(move |v| (k.clone(), v.clone()))
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    self.form_dirty = true;
                    changed = true;
                }
                Response::SearchError { id, msg } if Some(id) == self.pending_read => {
                    self.pending_read = None;
                    self.entry = vec![("error".into(), msg)];
                    self.form_dirty = true;
                    changed = true;
                }
                _ => {} // ignore unrelated/stale responses in the spike
            }
        }
        changed
    }
}
```

- [ ] **Step 2: Build the `Shared` in `main` and thread it into `SpikeApp`**

`Program::new`'s `init_*` are `fn` pointers (no captures), so the shared state cannot be closed over there. Change `SpikeApp` to build the program with closures instead — verify tvision-rs accepts closures for the factories (the signature is `impl FnOnce(Rect) -> Option<Box<dyn View>>`, so a move-closure capturing `Shared` clones is allowed). Restructure:

```rust
impl SpikeApp {
    fn new(backend: Box<dyn Backend>, state: Shared) -> Self {
        let s_desk = state.clone();
        let program = Program::new(
            backend,
            Box::new(SystemClock::new()),
            Theme::classic_blue(),
            move |r| Self::init_desktop(r, s_desk.clone()),
            Self::init_status_line,
            Self::init_menu_bar,
        );
        SpikeApp { program }
    }
    fn init_desktop(r: Rect, state: Shared) -> Option<Box<dyn View>> { /* Task 5/6 use state */ }
}

fn main() -> io::Result<()> {
    let boot = bootstrap().expect("bootstrap");
    let state: Shared = Rc::new(RefCell::new(SpikeState::new(boot)));
    let mut app = SpikeApp::new(Box::new(CrosstermBackend::new()?), state);
    let _ = app.run();
    Ok(())
}
```

If `Program::new` rejects a closure (requires a bare `fn`), that is a **finding** (stream 2.3: factories can't capture app state) — fall back to a `thread_local!` holding the `Shared` and read it inside the `fn` factories. Note whichever path was needed.

- [ ] **Step 3: Build and run — verify no regression**

```bash
cargo run -j4 --features spike-tv --bin spike-tv
```
Expected: identical to Task 3 (static panes still render; state is built but not yet consumed). The point is that bootstrap → `Shared` → program construction compiles and runs end-to-end.

- [ ] **Step 4: Commit**

```bash
git add src/bin/spike-tv.rs
git commit -m "spike(tv): shared SpikeState model + worker pump method"
```

---

### Task 5: Tree pane from real DIT branches

Replace the placeholder Animals tree with the real branch hierarchy from `Structure`, plus a DFS-order index→DN map so a selection can be turned back into a branch DN.

**Files:**
- Modify: `src/bin/spike-tv.rs`

**Interfaces:**
- Consumes: `SpikeState.structure.branch_dns()`, `Structure::get(dn) -> Option<&StructureNode>`, `StructureNode { label, children, .. }`; `tv::{Node, Outline}`.
- Produces: `fn build_branch_nodes(state: &SpikeState) -> (Option<Box<Node>>, Vec<String>)` — the root `Node` chain and the parallel `Vec<String>` of branch DNs in pre-order (matching `Outline`'s DFS focus index).

- [ ] **Step 1: Write a failing unit test for the DFS map**

Add a `#[cfg(test)]` module in `src/bin/spike-tv.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dfs_map_is_preorder_and_matches_node_count() {
        // Two branches under root, each with one sub-branch.
        let inputs = vec![
            si("dc=x", None), si("ou=a,dc=x", Some("ou=a,dc=x")),
            si("ou=b,dc=x", Some("ou=b,dc=x")),
            si("cn=1,ou=a,dc=x", None), si("cn=2,ou=a,dc=x", None),
        ];
        let structure = Structure::build("dc=x", inputs);
        let mut st = SpikeState::new_for_test(structure, "dc=x".into());
        let (root, dns) = build_branch_nodes(&st);
        assert!(root.is_some());
        // root + ou=a (a branch: has leaf children) appear; pre-order, root first.
        assert_eq!(dns.first().map(String::as_str), Some("dc=x"));
        assert!(dns.contains(&"ou=a,dc=x".to_string()));
    }

    fn si(dn: &str, cn: Option<&str>) -> StructureInput {
        StructureInput { dn: dn.into(), cn: cn.map(Into::into), description: None,
            object_classes: vec![], attrs: Default::default() }
    }
}
```

Add a test-only constructor `SpikeState::new_for_test(structure: Structure, base_dn: String) -> Self` (worker-less) gated `#[cfg(test)]` — or refactor `new` to split worker from model. (A `#[cfg(test)]` constructor that `panic!`s on `pump_worker` is acceptable for the spike.)

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -j4 --features spike-tv --bin spike-tv dfs_map -- --nocapture`
Expected: FAIL — `build_branch_nodes` / `new_for_test` not defined.

- [ ] **Step 3: Implement `build_branch_nodes`**

```rust
fn build_branch_nodes(state: &SpikeState) -> (Option<Box<tv::Node>>, Vec<String>) {
    let branches: std::collections::HashSet<String> =
        state.structure.branch_dns().into_iter().collect();
    let mut dns = Vec::new();

    // Recursive pre-order build over branch DNs, chaining siblings with with_next.
    fn build(
        dn: &str,
        structure: &Structure,
        branches: &std::collections::HashSet<String>,
        dns: &mut Vec<String>,
    ) -> tv::Node {
        dns.push(dn.to_string());
        let label = structure.get(dn).map(|n| n.label.clone()).unwrap_or_else(|| dn.to_string());
        let mut node = tv::Node::new(&label).with_expanded(true);

        // Child *branches* only (leaves live in pane 2), in input order.
        let child_branches: Vec<String> = structure
            .get(dn)
            .map(|n| n.children.iter().filter(|c| branches.contains(*c)).cloned().collect())
            .unwrap_or_default();

        // Build a with_next-chained sibling list, then attach as children.
        let mut chain: Option<Box<tv::Node>> = None;
        for cb in child_branches.into_iter().rev() {
            let mut child = build(&cb, structure, branches, dns);
            if let Some(next) = chain.take() {
                child = child.with_next(next);
            }
            chain = Some(Box::new(child));
        }
        if let Some(children) = chain {
            node = node.with_children(children);
        }
        node
    }

    let root_dn = state.base_dn.clone();
    if branches.contains(&root_dn) || state.structure.get(&root_dn).is_some() {
        let root = build(&root_dn, &state.structure, &branches, &mut dns);
        (Some(Box::new(root)), dns)
    } else {
        (None, dns)
    }
}
```

NOTE: pre-order push order here (parent, then children) must match `Outline`'s focus DFS. Verify against `src/widgets/outline.rs` ordering in Step 5; if `Outline` counts differently, adjust the push site and re-run the test. Record the ordering contract for the findings doc.

- [ ] **Step 4: Use it in `init_desktop` for the left-column tree**

Replace `build_tree(interior)` with a tree built from state. Store the DFS DN map on `SpikeState` (add field `branch_dns: Vec<String>`, set it here) so Task 6 can map a selection index → DN:

```rust
let (root, dn_map) = build_branch_nodes(&state.borrow());
state.borrow_mut().branch_dns = dn_map;
let tree: Box<dyn View> = Box::new(tv::Outline::new(interior, None, None, root));
```

- [ ] **Step 5: Run test + interactive check**

```bash
cargo test -j4 --features spike-tv --bin spike-tv dfs_map
cargo run -j4 --features spike-tv --bin spike-tv
```
Expected: test PASSES; the left-top pane shows the **real demo DIT branches** (e.g. `dc=…`, `ou=people`, `ou=groups`), expandable/collapsible. Confirm focus index visually tracks the DN map (top branch selected → index 0).

- [ ] **Step 6: Commit**

```bash
git add src/bin/spike-tv.rs
git commit -m "spike(tv): populate Outline from real DIT branches"
```

---

### Task 6: Branch → leaf navigation (synchronous)

Wire tree selection to the leaf list. A custom `TreePane` wrapper detects selection deltas and writes `current_branch` + broadcasts `REFRESH`; a `LeafPane` wrapper rebuilds its `ListBox` from `Structure::leaves_of` on `REFRESH`. No worker needed — `Structure` is eager.

**Files:**
- Modify: `src/bin/spike-tv.rs`

**Interfaces:**
- Consumes: `tv::{Context, Event, ViewId, FieldValue}`, `View::value() -> Option<FieldValue>`, `Context::broadcast(Command, Option<ViewId>)`, the `Shared` state, `SpikeState::{leaves, branch_dns}`; `ListBox::new_list`.
- Produces: `struct TreePane { outline: Outline, state: Shared, last_sel: i32 }` and `struct LeafPane { list: ListBox, state: Shared, seeded: bool }`, both with `#[delegate]` View impls.

- [ ] **Step 1: Implement `TreePane` (delta-detect selection → set branch → broadcast)**

```rust
struct TreePane {
    outline: tv::Outline,
    state: Shared,
    last_sel: i32,
}

#[tv::delegate(to = outline)]
impl View for TreePane {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> { Some(self) }

    fn handle_event(&mut self, ev: &mut tv::Event, ctx: &mut tv::Context) {
        self.outline.handle_event(ev, ctx);
        // After the inner widget processed the key/mouse, read its focus index.
        if let Some(tv::FieldValue::Int(sel)) = self.outline.value() {
            if sel != self.last_sel {
                self.last_sel = sel;
                let mut st = self.state.borrow_mut();
                if let Some(dn) = st.branch_dns.get(sel as usize).cloned() {
                    st.current_branch = Some(dn);
                    st.list_dirty = true;
                }
                drop(st);
                ctx.broadcast(REFRESH, None);
            }
        }
    }
}
```

VERIFY in Step 4: that `Outline::value()` returns the focus index as `FieldValue::Int`. If `Outline` exposes selection differently (e.g. a `foc` accessor via downcast), adapt — and record the discovery cost for the findings doc.

- [ ] **Step 2: Implement `LeafPane` (rebuild list on REFRESH when dirty)**

```rust
struct LeafPane {
    list: tv::ListBox,
    state: Shared,
    seeded: bool,
}

#[tv::delegate(to = list)]
impl View for LeafPane {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> { Some(self) }

    fn handle_event(&mut self, ev: &mut tv::Event, ctx: &mut tv::Context) {
        let refresh = matches!(ev, tv::Event::Broadcast { command, .. } if *command == REFRESH);
        if !self.seeded || refresh {
            let dirty = self.state.borrow().list_dirty;
            if !self.seeded || dirty {
                self.seeded = true;
                let rows: Vec<String> = self.state.borrow().leaves()
                    .into_iter().map(|(label, _dn)| label).collect();
                self.list.new_list(rows, ctx);
                self.state.borrow_mut().list_dirty = false;
            }
        }
        self.list.handle_event(ev, ctx);
    }
}
```

- [ ] **Step 3: Build both panes from state in `init_desktop`**

Replace the left column's `build_tree`/`build_list` with the wrappers (each gets a `state.clone()`):

```rust
let tree = Box::new(TreePane { outline: tv::Outline::new(interior, None, None, root), state: state.clone(), last_sel: -1 });
let leaf = Box::new(LeafPane { list: tv::ListBox::new(interior, 1, None, None), state: state.clone(), seeded: false });
let left = tv::Splitter::rows()
    .pane(tree, tv::Constraints::flex().min(3))
    .pane(leaf, tv::Constraints::flex().min(3));
```

- [ ] **Step 4: Run and verify navigation**

```bash
cargo run -j4 --features spike-tv --bin spike-tv
```
Expected: selecting/expanding a branch in the top-left tree **immediately updates the bottom-left list** with that branch's leaf labels from real data (e.g. select `ou=people` → list shows user labels). Verify with a branch that has many leaves and one with none.

- [ ] **Step 5: Commit**

```bash
git add src/bin/spike-tv.rs
git commit -m "spike(tv): branch->leaf navigation via shared state + broadcast"
```

---

### Task 7: Leaf → form via the async worker pump

The integration centrepiece. Selecting a leaf submits a base-scope read; a periodic timer drains the worker; the form pane shows the entry's attributes. This resolves spec §5 (worker→view pumping) concretely.

**Files:**
- Modify: `src/bin/spike-tv.rs`

**Interfaces:**
- Consumes: `Request::Search { id, base, scope: SearchScope::Base, filter, attrs, size_limit }`, `WorkerHandle::submit`, `SpikeState::pump_worker`; `tv::Context::set_timer(Duration, Option<Duration>) -> TimerId`; `Event::Timer`; `InputLine::set_value(FieldValue::Text)`.
- Produces: `struct FormPane { group: Group, rows: Vec<ViewId>, state: Shared }`; `struct PumpView { state: Shared, armed: bool }` (invisible view that owns the timer + worker drain); leaf-selection handling added to `LeafPane`.

- [ ] **Step 1: Add leaf-selection → submit read in `LeafPane::handle_event`**

After the existing `self.list.handle_event(ev, ctx)` line, append delta detection on the list's selected index:

```rust
        if let Some(tv::FieldValue::Int(sel)) = self.list.value() {
            let mut st = self.state.borrow_mut();
            let leaves = st.leaves();
            if let Some((_label, dn)) = leaves.get(sel as usize).cloned() {
                if st.current_leaf.as_deref() != Some(dn.as_str()) {
                    st.current_leaf = Some(dn.clone());
                    let id = NEXT_ID.with(|c| { let v = c.get(); c.set(v + 1); v });
                    st.pending_read = Some(id);
                    let _ = st.worker.submit(Request::Search {
                        id, base: dn, scope: SearchScope::Base,
                        filter: "(objectClass=*)".into(), attrs: vec![], size_limit: Some(1),
                    });
                }
            }
        }
```

Add a `thread_local! { static NEXT_ID: std::cell::Cell<u64> = const { std::cell::Cell::new(1000) }; }` for correlation ids. (Track `last_leaf_sel` like the tree if the list re-fires on non-changes.)

- [ ] **Step 2: Implement `PumpView` — the worker drain on a periodic timer**

```rust
struct PumpView { state: Shared, armed: bool }

impl View for PumpView {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> { Some(self) }

    fn handle_event(&mut self, ev: &mut tv::Event, ctx: &mut tv::Context) {
        if !self.armed {
            self.armed = true;
            // ~20 Hz periodic timer → drains the LDAP worker between keystrokes.
            ctx.set_timer(std::time::Duration::from_millis(50),
                          Some(std::time::Duration::from_millis(50)));
        }
        if matches!(ev, tv::Event::Timer(_)) {
            let changed = self.state.borrow_mut().pump_worker();
            if changed { ctx.broadcast(REFRESH, None); }
        }
    }
    // draw(): no-op (invisible). Provide minimal required View methods / a 0-area bounds.
}
```

Insert one `PumpView` into the window (e.g. `win.insert_child(Box::new(PumpView { state: state.clone(), armed: false }))`). VERIFY in Step 5: that a 0-area / hidden child still receives `Timer` events. If hidden views are skipped, attach the timer logic to the always-present `TreePane` instead — record which worked (finding stream 2.3: "where does off-thread data get pumped").

PRIMARY/ALTERNATIVE: this uses the **periodic timer**. Also try `Program::set_on_idle(|prog| …)` (fires each event-less pass, gets `&mut Program`) and record in the findings which is the cleaner bridge for an external worker thread.

- [ ] **Step 3: Implement `FormPane` with a fixed pool of InputLines**

```rust
const FORM_ROWS: usize = 24;

struct FormPane { group: tv::Group, rows: Vec<tv::ViewId>, state: Shared }

impl FormPane {
    fn build(bounds: Rect, state: Shared) -> Self {
        let mut group = tv::Group::new(bounds);
        let mut rows = Vec::new();
        for i in 0..FORM_ROWS {
            let y = 1 + i as i32;
            let il = tv::InputLine::with_limit(Rect::new(1, y, bounds.b.x - 1, y + 1), 1024);
            rows.push(group.insert(Box::new(il)));
        }
        FormPane { group, rows, state }
    }
}

#[tv::delegate(to = group)]
impl View for FormPane {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> { Some(self) }

    fn handle_event(&mut self, ev: &mut tv::Event, ctx: &mut tv::Context) {
        let refresh = matches!(ev, tv::Event::Broadcast { command, .. } if *command == REFRESH);
        if refresh && self.state.borrow().form_dirty {
            let entry = self.state.borrow().entry.clone();
            for (i, id) in self.rows.iter().enumerate() {
                let text = entry.get(i).map(|(k, v)| format!("{k}: {v}")).unwrap_or_default();
                if let Some(child) = self.group.child_mut(*id) {
                    child.set_value(tv::FieldValue::Text(text));
                }
            }
            self.state.borrow_mut().form_dirty = false;
        }
        self.group.handle_event(ev, ctx);
    }
}
```

VERIFY: `Group::child_mut(ViewId)` exists (Window has it; confirm Group does — else iterate via the Group's child API or downcast). Use `FormPane::build(interior, state.clone())` as the middle pane.

- [ ] **Step 4: Run and verify the full chain against real data**

```bash
scripts/test-ldap.sh start
export EDAPTOR_TEST_ADMIN_PW=adminpassword
cargo run -j4 --features spike-tv --bin spike-tv
```
Expected: select a branch → leaves list; select a leaf → **the middle form fills with that entry's `attr: value` lines** within ~50 ms. Pick a user whose `cn`/`sn` contains an umlaut (the demo set has German names) and confirm it displays **uncorrupted**. Note the observed latency and the working pump mechanism for the findings.

- [ ] **Step 5: Commit**

```bash
cargo clippy -j4 --features spike-tv --all-targets -- -D warnings
git add src/bin/spike-tv.rs
git commit -m "spike(tv): leaf->form via worker pump (timer-driven drain)"
```

---

### Task 8: UTF-8 / umlaut regression test

The named test guarding the exact failure that drove edaptor *off* the old `turbo-vision` crate: grapheme-correct editing in `InputLine`. Modelled on tvision-rs's own `ctrl_word_nav_over_multibyte_no_panic` and `scroll_follow_wide_glyphs_is_columns_not_bytes` tests.

**Files:**
- Create: `tests/spike_tv_umlaut.rs` (feature-gated integration test)

**Interfaces:**
- Consumes (tvision-rs public API): `tv::InputLine::with_limit`, `View::handle_event`, `View::value() -> FieldValue::Text`, and a headless `Context`. The in-crate tests build a `Context` via `Context::new(&mut out, &mut timers, 0, &mut deferred)` — **first confirm these are reachable from an external crate**; if not, that is finding-stream-2.3 #1 (no public way to drive a view in a consumer's test) and we fall back to `HeadlessBackend` + a `Program`.

- [ ] **Step 1: Confirm the headless drive path available to consumers**

Run: `cargo doc -j4 --features spike-tv -p tvision-rs --no-deps 2>&1 | tail -3` then check whether `tvision_rs::view::Context::new`, `tvision_rs::timer::TimerQueue`, `tvision_rs::view::Deferred` are `pub` (grep the rendered docs or `cargo tree`/source). Decide: **path A** (construct `Context` directly, like the in-crate tests) or **path B** (`HeadlessBackend` + `Program`, feeding `Event::KeyDown`). Write down the answer — it is a findings-doc entry either way.

- [ ] **Step 2: Write the failing test (path A shown; adapt to B if needed)**

```rust
#![cfg(feature = "spike-tv")]
//! Regression: the bug that drove edaptor off the old `turbo-vision` crate was
//! an InputLine that byte-sliced UTF-8 and panicked on an umlaut. Prove our
//! tvision-rs InputLine edits German text by grapheme without panic/corruption.

use tvision_rs::{Event, InputLine, Key, KeyEvent, KeyModifiers, Rect, View};

fn type_str(il: &mut InputLine, s: &str) {
    for ch in s.chars() {
        let mut ev = Event::KeyDown(KeyEvent::new(Key::Char(ch), KeyModifiers::default()));
        drive(il, &mut ev);
    }
}

// `drive` wraps whichever headless Context path Step 1 selected.
fn drive(il: &mut InputLine, ev: &mut Event) { /* path A or B */ }

#[test]
fn inputline_accepts_umlauts_without_panic() {
    let mut il = InputLine::with_limit(Rect::new(0, 0, 20, 1), 256);
    type_str(&mut il, "Müller");
    type_str(&mut il, " Zürich");
    // Backspace once: must delete the trailing 'h' grapheme, not split a byte.
    let mut bs = Event::KeyDown(KeyEvent::new(Key::Backspace, KeyModifiers::default()));
    drive(&mut il, &mut bs);

    let value = match il.value() {
        Some(tvision_rs::FieldValue::Text(s)) => s,
        other => panic!("expected text value, got {other:?}"),
    };
    assert_eq!(value, "Müller Züric", "grapheme-correct edit, no corruption");
}
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -j4 --features spike-tv --test spike_tv_umlaut`
Expected: FAIL — `drive` unimplemented (stub) until Step 4.

- [ ] **Step 4: Implement `drive` per the Step-1 decision**

Path A (if `Context::new` is public — mirrors `input_line.rs:1208-1216`):

```rust
fn drive(il: &mut InputLine, ev: &mut Event) {
    use std::collections::VecDeque;
    use tvision_rs::timer::TimerQueue;
    use tvision_rs::view::{Context, Deferred};
    let mut out: VecDeque<Event> = VecDeque::new();
    let mut timers = TimerQueue::new();
    let mut deferred: Vec<Deferred> = Vec::new();
    let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);
    il.handle_event(ev, &mut ctx);
}
```

Path B (if not public — `HeadlessBackend` + a minimal `Program` hosting the `InputLine`, dispatching the event through the program). Implement whichever Step 1 chose.

- [ ] **Step 5: Run to verify it passes, then commit**

```bash
cargo test -j4 --features spike-tv --test spike_tv_umlaut
git add tests/spike_tv_umlaut.rs
git commit -m "spike(tv): umlaut/grapheme regression test for InputLine"
```
Expected: PASS. If it fails on a real grapheme bug in tvision-rs, that is a **blocking finding** — record it; the full migration is gated on the fix (spec §1).

---

### Task 9: Findings document

The non-code deliverable: the three streams from the spec (§2.1–2.3) plus the migration effort estimate. This is what makes the spike valuable beyond the running binary.

**Files:**
- Create: `docs/superpowers/research/2026-06-22-tvision-rs-spike-findings.md`

- [ ] **Step 1: Write the findings doc with these required sections**

Capture, with concrete file/line refs and verbatim snippets where useful:

1. **Worker → view pumping (spec §5):** which mechanism was used (periodic `Context::set_timer` vs `Program::set_on_idle`), why, observed leaf→form latency, and whether a hidden/0-area view receives `Timer` events. State plainly whether this needed any tvision-rs change (it should not).
2. **tvision-rs documentation gaps (spec §2.2):** what was hard to find and where it actually lived; the missing "bring-your-own-state / external data source" recipe; any doc that existed but didn't surface under the obvious search term; the C++→Rust porting-guide deltas that bit (e.g. `fn`-pointer factories can't capture state → needed closures/`thread_local`).
3. **Framework feature gaps (spec §2.3):** split into **"exists, found under name Y"** vs **"genuinely absent"**. For each candidate gap, record the searches done (source, `docs/`, `PORTING-GUIDE`, `CHANGELOG`, examples) BEFORE declaring absence. Likely entries: cross-view shared-state/messaging ergonomics (we hand-rolled `Rc<RefCell>` + `REFRESH` broadcast — is there a blessed pattern?); dynamic `Outline`/`ListBox` content replacement after insert; reading a widget's selection (did `value() -> Int` suffice, or was a downcast needed?); whether consumers can unit-test a view headlessly (Task 8 Step 1).
4. **Migration effort estimate:** size the remaining full-migration work in the spec's layers — overlays→`Dialog`s (Confirm/Error/Guard/profile-chooser/ValueEditor), each rich widget (Choice/Password/Picker/Membership/ObjectClassPicker), save/validate/changeset wiring, config-driven labels & tree rules, config-picker startup. Flag the riskiest piece.
5. **Go / no-go recommendation** against spec §7 success criteria.

- [ ] **Step 2: Commit**

```bash
git add docs/superpowers/research/2026-06-22-tvision-rs-spike-findings.md
git commit -m "docs: tvision-rs spike findings (pumping, doc & feature gaps, estimate)"
```

---

## Self-Review

**Spec coverage:**
- §2.1 edaptor port → Tasks 1–7. ✔
- §2.2 doc improvements → Task 9 §2. ✔
- §2.3 framework features (search-hard discipline) → Task 9 §3 (explicit "searches done before declaring absent"). ✔
- §3 scope IN (3-pane, real data, splitter resize, typing) → Tasks 3,5,6,7. Scope OUT correctly absent (no save/overlays/rich widgets). ✔
- §4 separate binary, domain reuse, pane mapping → Tasks 1–7. ✔
- §5 worker-pump risk → Task 7 + Task 9 §1. ✔
- §6 deliverables: binary (T1–7), findings doc (T9), umlaut test (T8). ✔
- §7 success criteria → Task 9 §5 go/no-go. ✔

**Placeholder scan:** No "TBD"/"handle errors appropriately". The genuinely-unknown tvision-rs API points (selection accessor, `Group::child_mut`, hidden-view timers, public `Context::new`) are framed as explicit VERIFY-then-adapt steps with concrete check commands and named fallbacks — appropriate for a spike whose purpose is discovery, not hidden guesses.

**Type consistency:** `Shared = Rc<RefCell<SpikeState>>`, `REFRESH`, `SpikeState` fields (`current_branch/current_leaf/entry/list_dirty/form_dirty/pending_read/branch_dns`), `pump_worker() -> bool`, and `to_input`/`build_branch_nodes`/`FORM_ROWS` are used consistently across Tasks 4–7. `StructureInput`/`StructureNodeRaw` field names match the explorer's verified signatures.
