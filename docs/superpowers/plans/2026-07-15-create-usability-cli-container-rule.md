# Create-usability (`tui-create` + container rule) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the wrong-container hazard in the TUI create flow and add an
`edaptor tui-create [<profile>] [--container <DN>]` subcommand that launches the TUI
straight into a profile's create form.

**Architecture:** Two independent code paths that reuse existing machinery. Part 1 adds
a pure `resolve_create_container` decision plus a small two-choice modal that fires only
when the operator hits New while standing *above* a profile's home OU. Part 2 threads an
optional `StartupAction` from `main` through `ui::run` onto `UiState`; the pump posts a
one-shot `STARTUP` command that `app::dispatch` executes by opening the existing
`open_create` flow (or a profile chooser first). No new write path or headless planner.

**Tech Stack:** Rust, tvision-rs 0.12, clap (CLI), anyhow. Tests are plain `#[test]`
units run via `cargo test -j4`.

## Global Constraints

- **Cap build/test parallelism at 4 cores** (shared box): `cargo test -j4`,
  `cargo clippy --all-targets -- -D warnings`. The gate is `make check` (fmt + clippy
  `-D warnings` + tests) — it must stay green after every task.
- **Facade boundary:** only `src/ui/**` may `use tvision_rs`. The new pure helpers live
  in `src/workflows/create.rs` and must not reference any tvision type.
- **English** for all identifiers, comments, and doc-comments. User-facing docs may be
  English only here (no localized UI strings added).
- **Docs one-home:** config/behaviour detail → mdBook (`docs/src/`); `CHANGES.md` for
  every user-visible change; README stays orientation-only.
- **Commit trailer** on every commit:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

---

## File Structure

- `src/workflows/create.rs` — **modify.** Add two pure helpers + their unit tests:
  `resolve_create_container` / `CreateContainer` (Part 1) and `resolve_profile_arg`
  (Part 2). Pure, tvision-free.
- `src/ui/dialog/container_chooser.rs` — **create.** Two-row modal mirroring
  `profile_chooser.rs`; writes the highlighted row to `UiState::chosen_container`.
- `src/ui/dialog/mod.rs` — **modify.** `pub mod container_chooser;` + a build smoke test.
- `src/ui/state.rs` — **modify.** Add `chosen_container` and `pending_startup` fields,
  initialised in both `new_for_test` and `bootstrap`.
- `src/ui/mod.rs` — **modify.** Define `StartupAction` enum + `STARTUP` command; change
  `run` to accept `Option<StartupAction>` and stash it on state.
- `src/ui/pump.rs` — **modify.** One-shot `apply_startup_once` that posts `STARTUP` when
  a startup action is pending; a unit test.
- `src/ui/app.rs` — **modify.** Funnel both CREATE arms through a container-rule helper;
  handle the `STARTUP` command in `dispatch`.
- `src/main.rs` — **modify.** Add the `tui-create` subcommand + `build_startup_action`
  helper; update `run_tui`/`ui::run` call sites for the new signature.
- `CHANGES.md`, `docs/src/usage/crud.md` — **modify.** User-visible docs.

---

## Task 1: Part 1 pure core — `resolve_create_container`

**Files:**
- Modify: `src/workflows/create.rs` (add after the `dn_boundary_match` helper, ~line 194)
- Test: `src/workflows/create.rs` (its `#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `pub enum CreateContainer { Unambiguous(String), Ask { here: String, home: String } }`
  and `pub fn resolve_create_container(current_branch: &str, search_base: &str) -> CreateContainer`.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `src/workflows/create.rs`:

```rust
#[test]
fn resolve_container_equal_is_unambiguous() {
    let c = resolve_create_container(
        "ou=people,dc=example,dc=org",
        "ou=people,dc=example,dc=org",
    );
    assert_eq!(
        c,
        CreateContainer::Unambiguous("ou=people,dc=example,dc=org".to_string())
    );
}

#[test]
fn resolve_container_inside_home_is_unambiguous_current() {
    // Standing INSIDE the home OU (deeper): create where we stand.
    let c = resolve_create_container(
        "ou=staff,ou=people,dc=example,dc=org",
        "ou=people,dc=example,dc=org",
    );
    assert_eq!(
        c,
        CreateContainer::Unambiguous("ou=staff,ou=people,dc=example,dc=org".to_string())
    );
}

#[test]
fn resolve_container_above_home_asks() {
    // Standing ABOVE the home OU: ambiguous.
    let c = resolve_create_container("dc=example,dc=org", "ou=people,dc=example,dc=org");
    assert_eq!(
        c,
        CreateContainer::Ask {
            here: "dc=example,dc=org".to_string(),
            home: "ou=people,dc=example,dc=org".to_string(),
        }
    );
}

#[test]
fn resolve_container_is_case_insensitive() {
    let c = resolve_create_container("DC=Example,DC=Org", "ou=people,dc=example,dc=org");
    assert_eq!(
        c,
        CreateContainer::Ask {
            here: "DC=Example,DC=Org".to_string(),
            home: "ou=people,dc=example,dc=org".to_string(),
        }
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -j4 -p edaptor resolve_container 2>&1 | tail -20`
Expected: FAIL — `cannot find function resolve_create_container` / `CreateContainer`.

- [ ] **Step 3: Write the implementation**

Add after `dn_boundary_match` (after ~line 194) in `src/workflows/create.rs`:

```rust
/// Where a create should land, given the operator's current tree branch and the
/// chosen profile's `search_base`. Pure. Callers pass DNs already known to be on the
/// same path (guaranteed by [`profiles_for_container`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateContainer {
    /// Unambiguous — create at this container DN.
    Unambiguous(String),
    /// The current branch is an ancestor of the profile's home OU — ask which target.
    Ask { here: String, home: String },
}

/// Decide the create container (see [`CreateContainer`]). Rules (case-insensitive,
/// DN-boundary): equal, or `current` at/inside `search_base` (`search_base` a proper
/// suffix of `current`) → create at `current`. `current` above `search_base`
/// (`current` a proper suffix of `search_base`) → ask. Any other relationship (should
/// not reach here) → create at `current`, never silently relocating. Pure.
pub fn resolve_create_container(current_branch: &str, search_base: &str) -> CreateContainer {
    let cur = current_branch.trim();
    let base = search_base.trim();
    let cur_l = cur.to_lowercase();
    let base_l = base.to_lowercase();

    // Equal, or current is at/inside the home OU → create where we stand.
    if cur_l == base_l || cur_l.ends_with(&format!(",{base_l}")) {
        return CreateContainer::Unambiguous(cur.to_string());
    }
    // Current is an ancestor of the home OU → ambiguous, ask.
    if !base_l.is_empty() && base_l.ends_with(&format!(",{cur_l}")) {
        return CreateContainer::Ask {
            here: cur.to_string(),
            home: base.to_string(),
        };
    }
    // Not on the same path (unexpected): default to current, never relocate.
    CreateContainer::Unambiguous(cur.to_string())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -j4 -p edaptor resolve_container 2>&1 | tail -20`
Expected: PASS — 4 tests.

- [ ] **Step 5: Commit**

```bash
git add src/workflows/create.rs
git commit -m "$(printf 'feat(create): pure resolve_create_container decision\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 2: Part 1 UI — container-chooser dialog + state field

**Files:**
- Modify: `src/ui/state.rs` (add `chosen_container` field; init in `new_for_test` ~line 181 and `bootstrap` ~line 839)
- Create: `src/ui/dialog/container_chooser.rs`
- Modify: `src/ui/dialog/mod.rs` (declare the module + smoke test)

**Interfaces:**
- Consumes: `crate::ui::Shared`.
- Produces: `container_chooser::build(here: String, home: String, shared: Shared) -> (Box<dyn View>, tv::ViewId)`,
  which writes the highlighted row (`0` = here, `1` = home) to `UiState::chosen_container`.

- [ ] **Step 1: Add the `chosen_container` state field**

In `src/ui/state.rs`, add this field to the `UiState` struct right after `chosen_profile`
(the `pub chosen_profile: Option<usize>,` line, ~113):

```rust
    /// Container chooser → controller: the row the user highlighted (0 = current
    /// branch, 1 = the profile's search_base) when OK was pressed. Set by
    /// `ContainerChooser`; read by `dispatch` in the create container rule.
    pub chosen_container: Option<usize>,
```

Then add `chosen_container: None,` in **both** constructors — in `new_for_test` right
after the `chosen_profile: None,` line (~181) and in `bootstrap`'s returned `UiState`
right after its `chosen_profile: None,` line (~839).

- [ ] **Step 2: Write the failing dialog test (module + build)**

Create `src/ui/dialog/container_chooser.rs` with ONLY the test first so it fails to
compile (RED). Paste the full test module now; the implementation is Step 4:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldap::worker::RawSubschema;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn shared() -> Shared {
        use crate::workflows::structure::Structure;
        let schema = crate::schema::SchemaModel::from_raw(&RawSubschema::default());
        let st = crate::ui::state::UiState::new_for_test(
            Structure::build("dc=example,dc=org", vec![]),
            schema,
            "dc=example,dc=org".into(),
            Vec::new(),
            Vec::new(),
        );
        Rc::new(RefCell::new(st))
    }

    fn make_ctx<'a>(
        out: &'a mut std::collections::VecDeque<tv::Event>,
        timers: &'a mut tv::timer::TimerQueue,
        deferred: &'a mut Vec<tv::Deferred>,
    ) -> Context<'a> {
        Context::new(out, timers, 0, deferred)
    }

    #[test]
    fn reset_current_sets_chosen_container_zero_then_down_updates() {
        use tvision_rs::{Deferred, KeyEvent};
        let sh = shared();
        let (mut view, _id) = build(
            "dc=example,dc=org".into(),
            "ou=people,dc=example,dc=org".into(),
            sh.clone(),
        );
        assert_eq!(sh.borrow().chosen_container, None);

        let mut out = std::collections::VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<Deferred> = Vec::new();
        let mut ctx = make_ctx(&mut out, &mut timers, &mut deferred);
        view.reset_current(&mut ctx);
        assert_eq!(sh.borrow().chosen_container, Some(0));

        let chooser = view
            .as_any_mut()
            .and_then(|a| a.downcast_mut::<ContainerChooser>())
            .expect("downcast");
        let mut ev = Event::KeyDown(KeyEvent::from(Key::Down));
        chooser.handle_event(&mut ev, &mut ctx);
        assert_eq!(sh.borrow().chosen_container, Some(1));
    }
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -j4 -p edaptor container_chooser 2>&1 | tail -20`
Expected: FAIL to compile — `ContainerChooser` / `build` / `Shared` unresolved.

- [ ] **Step 4: Write the implementation (prepend above the test module)**

At the TOP of `src/ui/dialog/container_chooser.rs`, before the test module:

```rust
//! Two-row container chooser: when New is invoked *above* a profile's home OU, ask
//! whether to create at the current branch ("Here") or the profile's search_base
//! ("In <home>"). Mirrors `profile_chooser` — a `Dialog`-wrapping `View` with a
//! `ListBox` seeded in `reset_current` (never `borrow_mut` shared during `new`), the
//! highlighted row written to `shared.chosen_container` (0 = here, 1 = home).

use tvision_rs::{
    self as tv, delegate, ButtonFlags, ButtonRowAlign, Command, Context, Dialog, Event, FieldValue,
    Key, ListBox, Rect, View,
};

use crate::ui::Shared;

/// The container chooser dialog.
pub struct ContainerChooser {
    dlg: Dialog,
    list_id: tv::ViewId,
    shared: Shared,
    rows: Vec<String>,
}

impl ContainerChooser {
    fn new(here: String, home: String, shared: Shared) -> Self {
        let rows = vec![format!("Here — {here}"), format!("In {home}")];
        let list_rows = rows.len() as i32; // 2
        let height = 1 + 1 + list_rows + 1 + 2 + 1; // frame + list + pad + buttons
        let width = 64;
        let mut dlg = Dialog::new(
            Rect::new(0, 0, width, height),
            Some("Create where?".to_string()),
        );
        dlg.state_mut().options.center_x = true;
        dlg.state_mut().options.center_y = true;

        let list = ListBox::new(Rect::new(2, 1, width - 2, 1 + list_rows), 1, None, None);
        let list_id = dlg.insert_child(Box::new(list));

        dlg.button_row(
            &[
                (
                    "~O~K",
                    Command::OK,
                    ButtonFlags {
                        default: true,
                        ..ButtonFlags::new()
                    },
                ),
                ("~C~ancel", Command::CANCEL, ButtonFlags::new()),
            ],
            ButtonRowAlign::Right,
        );

        ContainerChooser {
            dlg,
            list_id,
            shared,
            rows,
        }
    }

    fn current_index(&mut self) -> Option<usize> {
        match self.dlg.child_mut(self.list_id).and_then(|v| v.value()) {
            Some(FieldValue::Int(i)) if i >= 0 => Some(i as usize),
            _ => None,
        }
    }

    fn stage_index(&mut self) {
        if let Some(idx) = self.current_index() {
            self.shared.borrow_mut().chosen_container = Some(idx);
        }
    }
}

#[delegate(to = dlg)]
impl View for ContainerChooser {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    fn reset_current(&mut self, ctx: &mut Context) {
        self.dlg.reset_current(ctx);
        let rows = self.rows.clone();
        if let Some(list) = self.dlg.child_mut(self.list_id) {
            if let Some(lb) = list.as_any_mut().and_then(|a| a.downcast_mut::<ListBox>()) {
                lb.new_list(rows, ctx);
            }
        }
        self.shared.borrow_mut().chosen_container = Some(0);
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        let nav = matches!(
            ev,
            Event::KeyDown(k)
                if matches!(k.key, Key::Up | Key::Down | Key::PageUp | Key::PageDown)
        );
        if nav {
            if let Some(list) = self.dlg.child_mut(self.list_id) {
                list.handle_event(ev, ctx);
            }
            self.stage_index();
        } else {
            self.dlg.handle_event(ev, ctx);
        }
    }
}

/// Build the container chooser. Returns `(view, list_view_id)` — pass the id as the
/// focus target to `exec_view_focused` so nav starts on the list.
pub fn build(here: String, home: String, shared: Shared) -> (Box<dyn View>, tv::ViewId) {
    let chooser = ContainerChooser::new(here, home, shared);
    let list_id = chooser.list_id;
    (Box::new(chooser), list_id)
}
```

- [ ] **Step 5: Register the module + smoke test**

In `src/ui/dialog/mod.rs`, add to the module list (after `pub mod confirm;`):

```rust
pub mod container_chooser;
```

And add this smoke test inside the `#[cfg(test)] mod tests` block (after
`profile_chooser_builds_without_panic`):

```rust
#[test]
fn container_chooser_builds_without_panic() {
    use crate::ldap::worker::RawSubschema;
    use crate::workflows::structure::Structure;
    use std::cell::RefCell;
    use std::rc::Rc;
    let schema = crate::schema::SchemaModel::from_raw(&RawSubschema::default());
    let st = crate::ui::state::UiState::new_for_test(
        Structure::build("dc=example,dc=org", vec![]),
        schema,
        "dc=example,dc=org".into(),
        Vec::new(),
        Vec::new(),
    );
    let shared = Rc::new(RefCell::new(st));
    let _v = container_chooser::build(
        "dc=example,dc=org".into(),
        "ou=people,dc=example,dc=org".into(),
        shared,
    );
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -j4 -p edaptor container_chooser 2>&1 | tail -20`
Expected: PASS — the reset/down test and the smoke test.

- [ ] **Step 7: Commit**

```bash
git add src/ui/dialog/container_chooser.rs src/ui/dialog/mod.rs src/ui/state.rs
git commit -m "$(printf 'feat(ui): container-chooser dialog + chosen_container state\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 3: Part 1 wiring — apply the container rule in the CREATE handler

**Files:**
- Modify: `src/ui/app.rs` (imports ~line 19; the `CREATE` command arm ~lines 265-299;
  add a helper near `open_create` ~line 312)

**Interfaces:**
- Consumes: `crate::workflows::create::{resolve_create_container, CreateContainer}`,
  `crate::ui::dialog::container_chooser`, existing `open_create`.

This task is verified by `make check` (compile + existing tests) and a manual TUI check;
the decision logic it calls is already unit-tested (Task 1).

- [ ] **Step 1: Import the new items**

In `src/ui/app.rs`, add to the existing `use crate::workflows...` imports (near line 20)
a line:

```rust
use crate::workflows::create::{resolve_create_container, CreateContainer};
```

- [ ] **Step 2: Add the container-rule helper**

Add this function immediately above `fn open_create(` (~line 312) in `src/ui/app.rs`:

```rust
/// Resolve the create container for `profile_idx` under `current_branch`, asking via
/// a modal when the branch sits above the profile's home OU, then open the create
/// form. Cancelling the container prompt aborts the create.
fn open_create_with_container_rule(
    prog: &mut Program,
    state: &Shared,
    profile_idx: usize,
    current_branch: &str,
) {
    let search_base = state.borrow().profiles[profile_idx].search_base.clone();
    match resolve_create_container(current_branch, &search_base) {
        CreateContainer::Unambiguous(dn) => open_create(state, profile_idx, &dn),
        CreateContainer::Ask { here, home } => {
            let (view, focus) =
                crate::ui::dialog::container_chooser::build(here.clone(), home.clone(), state.clone());
            if prog.exec_view_focused(view, focus) == Command::OK {
                let choice = state.borrow_mut().chosen_container.take();
                match choice {
                    Some(0) => open_create(state, profile_idx, &here),
                    Some(1) => open_create(state, profile_idx, &home),
                    _ => {}
                }
            } else {
                state.borrow_mut().chosen_container = None;
            }
        }
    }
}
```

- [ ] **Step 3: Route both CREATE arms through the helper**

In the `else if cmd == CREATE {` block (~lines 276-298), replace the single-match arm
and the post-chooser `open_create` call so both go through the helper. The two edits:

Replace:
```rust
            [only] => open_create(state, *only, &container),
```
with:
```rust
            [only] => open_create_with_container_rule(prog, state, *only, &container),
```

And replace (inside the `_ =>` chooser branch):
```rust
                    if let Some(rel) = chosen {
                        if let Some(idx) = idxs.get(rel) {
                            open_create(state, *idx, &container);
                        }
                    }
```
with:
```rust
                    if let Some(rel) = chosen {
                        if let Some(idx) = idxs.get(rel) {
                            open_create_with_container_rule(prog, state, *idx, &container);
                        }
                    }
```

- [ ] **Step 4: Verify it builds and all tests pass**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5 && cargo test -j4 2>&1 | tail -15`
Expected: clippy clean; all tests PASS.

- [ ] **Step 5: Manual check (record the result)**

```bash
scripts/test-ldap.sh start
export EDAPTOR_TEST_ADMIN_PW=adminpassword
cargo run -- --config examples/demo-config.toml
```
Navigate to the **tree root** (above `ou=people`), press **Alt-N**. Expected: the
"Create where?" modal offers `Here — <root>` and `In ou=people,…`; picking "In …" opens
the new-user form and the confirm LDIF shows the DN under `ou=people`. Navigate *into*
`ou=people` and press Alt-N again: no modal, form opens directly.

- [ ] **Step 6: Commit**

```bash
git add src/ui/app.rs
git commit -m "$(printf 'feat(ui): ask which container when creating above the home OU\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 4: Part 2 pure core — `resolve_profile_arg`

**Files:**
- Modify: `src/workflows/create.rs` (add after `resolve_create_container`)
- Test: `src/workflows/create.rs` (`tests` module)

**Interfaces:**
- Produces: `pub fn resolve_profile_arg(profiles: &[EntryProfile], name: Option<&str>) -> Result<Option<usize>, String>`.
  `Some(name)` → matching index (case-insensitive) or an error listing valid names;
  `None` → `Ok(None)` (caller shows the chooser).

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `src/workflows/create.rs` (the module already has a
`prof(base)` helper; add a small `named` helper for name-based cases):

```rust
fn named(name: &str) -> EntryProfile {
    EntryProfile {
        name: name.to_string(),
        ..Default::default()
    }
}

#[test]
fn resolve_profile_arg_none_yields_none() {
    let ps = vec![named("Users"), named("Groups")];
    assert_eq!(resolve_profile_arg(&ps, None), Ok(None));
}

#[test]
fn resolve_profile_arg_matches_case_insensitively() {
    let ps = vec![named("Users"), named("Groups")];
    assert_eq!(resolve_profile_arg(&ps, Some("users")), Ok(Some(0)));
    assert_eq!(resolve_profile_arg(&ps, Some("GROUPS")), Ok(Some(1)));
}

#[test]
fn resolve_profile_arg_unknown_lists_valid_names() {
    let ps = vec![named("Users"), named("Groups")];
    let err = resolve_profile_arg(&ps, Some("Admins")).unwrap_err();
    assert!(err.contains("Admins"));
    assert!(err.contains("Users"));
    assert!(err.contains("Groups"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -j4 -p edaptor resolve_profile_arg 2>&1 | tail -20`
Expected: FAIL — `cannot find function resolve_profile_arg`.

- [ ] **Step 3: Write the implementation**

Add after `resolve_create_container` in `src/workflows/create.rs`:

```rust
/// Resolve the optional `<profile>` argument of `tui-create` against the configured
/// profiles. `Some(name)` → the matching index (case-insensitive), or an error listing
/// the valid names when unknown. `None` → `Ok(None)` (the caller shows the chooser).
/// Pure.
pub fn resolve_profile_arg(
    profiles: &[EntryProfile],
    name: Option<&str>,
) -> Result<Option<usize>, String> {
    let Some(name) = name else {
        return Ok(None);
    };
    if let Some(idx) = profiles.iter().position(|p| p.name.eq_ignore_ascii_case(name)) {
        return Ok(Some(idx));
    }
    let valid: Vec<&str> = profiles.iter().map(|p| p.name.as_str()).collect();
    Err(format!(
        "unknown profile '{name}'. Configured profiles: {}",
        if valid.is_empty() {
            "(none)".to_string()
        } else {
            valid.join(", ")
        }
    ))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -j4 -p edaptor resolve_profile_arg 2>&1 | tail -20`
Expected: PASS — 3 tests.

- [ ] **Step 5: Commit**

```bash
git add src/workflows/create.rs
git commit -m "$(printf 'feat(create): pure resolve_profile_arg for tui-create\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 5: Part 2 plumbing — `StartupAction`, state field, pump post, dispatch handler

**Files:**
- Modify: `src/ui/mod.rs` (add `StartupAction` enum, `STARTUP` command, change `run`)
- Modify: `src/ui/state.rs` (add `pending_startup` field to struct + both constructors)
- Modify: `src/ui/pump.rs` (one-shot `apply_startup_once` + call + test)
- Modify: `src/ui/app.rs` (imports + `STARTUP` handler in `dispatch`)
- Modify: `src/main.rs` (only: update `run_tui` signature + the `None` arm to compile)

**Interfaces:**
- Produces: `pub enum StartupAction { Create { profile_idx: usize, container: String }, ChooseThenCreate { container: Option<String> } }`,
  `pub const STARTUP: tv::Command`, and `pub fn run(config: Config, password: String, startup: Option<StartupAction>) -> Result<()>`.
- Consumes (Task 6): `main` builds a `StartupAction` and passes it to `ui::run`.

- [ ] **Step 1: Define `StartupAction` + `STARTUP` and change `run`**

In `src/ui/mod.rs`, add near the other command consts (after the
`pub const SHOW_ERROR` line):

```rust
pub const STARTUP: tv::Command = tv::Command::custom("edaptor.startup");

/// A one-shot action to run once the TUI has started (schema is already loaded by
/// `bootstrap`). Carried on `UiState::pending_startup`, posted by the pump as the
/// `STARTUP` command, and executed once in `app::dispatch`.
#[derive(Debug, Clone)]
pub enum StartupAction {
    /// Open a create form for `profile_idx` under `container`.
    Create { profile_idx: usize, container: String },
    /// Show the all-profiles chooser, then open a create form for the pick under
    /// `container` (the pick's `search_base` when `None`).
    ChooseThenCreate { container: Option<String> },
}
```

Change the `run` function to accept and stash the action:

```rust
/// Spawn the worker, fetch schema + structure, then run the TUI. `startup` runs a
/// one-shot action (e.g. open a create form) once the loop starts; `None` = normal browse.
pub fn run(config: Config, password: String, startup: Option<StartupAction>) -> Result<()> {
    let mut booted = state::bootstrap(config, password)?;
    booted.pending_startup = startup;
    let state: Shared = Rc::new(RefCell::new(booted));
    let backend = Box::new(CrosstermBackend::new()?);
    let mut program = app::build_program(backend, state.clone());
    let dispatch_state = state.clone();
    program.run_app(move |prog, cmd| app::dispatch(prog, cmd, &dispatch_state));
    Ok(())
}
```

- [ ] **Step 2: Add the `pending_startup` state field**

In `src/ui/state.rs`, add to the `UiState` struct after `chosen_container` (added in
Task 2):

```rust
    /// One-shot action to run after the TUI starts (set by `tui-create` via
    /// `ui::run`; `None` for a normal launch). Posted once by the pump as `STARTUP`,
    /// taken by `dispatch`. See [`crate::ui::StartupAction`].
    pub pending_startup: Option<crate::ui::StartupAction>,
```

Initialise `pending_startup: None,` in **both** constructors — after the
`chosen_container: None,` line in `new_for_test` and in `bootstrap`'s returned struct.

- [ ] **Step 3: Update the existing `main.rs` call site so the crate keeps compiling**

Changing `ui::run` to 3 args breaks its only caller. Fix it now so later steps compile.
In `src/main.rs`, replace:

```rust
        None => run_tui(config, password)?,
```
with:
```rust
        None => run_tui(config, password, None)?,
```

And replace the `run_tui` function:

```rust
fn run_tui(
    config: Config,
    password: String,
    startup: Option<edaptor::ui::StartupAction>,
) -> Result<()> {
    edaptor::ui::run(config, password, startup)
}
```

- [ ] **Step 4: Handle `STARTUP` in `dispatch`**

In `src/ui/app.rs`, extend the imports (line ~19) to include `STARTUP` and
`StartupAction`:

```rust
use crate::ui::{Shared, ACTIVATE, CREATE, GUARD_NAV, REQUEST_QUIT, SAVE, SHOW_ERROR, STARTUP};
use crate::ui::StartupAction;
```

Add a new arm at the end of the `dispatch` `if/else if` chain (after the
`else if cmd == SHOW_ERROR { ... }` block, ~line 306):

```rust
    } else if cmd == STARTUP {
        let action = state.borrow_mut().pending_startup.take();
        match action {
            Some(StartupAction::Create {
                profile_idx,
                container,
            }) => {
                open_create(state, profile_idx, &container);
            }
            Some(StartupAction::ChooseThenCreate { container }) => {
                let names: Vec<String> =
                    state.borrow().profiles.iter().map(|p| p.name.clone()).collect();
                if names.is_empty() {
                    state.borrow_mut().status = "No profiles configured.".into();
                    return;
                }
                let (view, focus) =
                    crate::ui::dialog::profile_chooser::build(names, state.clone());
                if prog.exec_view_focused(view, focus) == Command::OK {
                    let chosen = state.borrow_mut().chosen_profile.take();
                    if let Some(idx) = chosen {
                        let dn = container.clone().unwrap_or_else(|| {
                            state.borrow().profiles[idx].search_base.clone()
                        });
                        if dn.trim().is_empty() {
                            state.borrow_mut().status =
                                "Profile has no search_base; pass --container.".into();
                            return;
                        }
                        open_create(state, idx, &dn);
                    }
                } else {
                    state.borrow_mut().chosen_profile = None;
                }
            }
            None => {}
        }
    }
```

- [ ] **Step 5: Confirm the crate compiles green before the pump TDD**

Run: `cargo build 2>&1 | tail -5`
Expected: builds. (`STARTUP` is defined and handled, but nothing posts it yet — inert.)

- [ ] **Step 6: Write the failing pump test**

Add to `src/ui/pump.rs`'s `#[cfg(test)] mod tests` (mirrors `posts_fullscreen_command_once`):

```rust
/// The pump posts `STARTUP` exactly once when a startup action is pending, and never
/// when none is set.
#[test]
fn posts_startup_command_once_when_pending() {
    let make_state = |pending: bool| {
        let structure = Structure::build("dc=x", Vec::new());
        let schema = SchemaModel::from_raw(&crate::ldap::worker::RawSubschema::default());
        let mut st = crate::ui::state::UiState::new_for_test(
            structure,
            schema,
            "dc=x".into(),
            Vec::new(),
            Vec::new(),
        );
        if pending {
            st.pending_startup = Some(crate::ui::StartupAction::ChooseThenCreate { container: None });
        }
        Rc::new(RefCell::new(st))
    };
    let count_startup = |out: &VecDeque<Event>| {
        out.iter()
            .filter(|e| matches!(e, Event::Command(c) if *c == crate::ui::STARTUP))
            .count()
    };

    // Pending → posts once, then idempotent.
    let mut pump = PumpView::new(make_state(true));
    {
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<tv::Deferred> = Vec::new();
        let mut ctx = headless(&mut out, &mut timers, &mut deferred);
        pump.apply_startup_once(&mut ctx);
        assert_eq!(count_startup(&out), 1);
    }
    {
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<tv::Deferred> = Vec::new();
        let mut ctx = headless(&mut out, &mut timers, &mut deferred);
        pump.apply_startup_once(&mut ctx);
        assert_eq!(count_startup(&out), 0, "startup is posted at most once");
    }

    // Not pending → never posts.
    let mut pump2 = PumpView::new(make_state(false));
    let mut out = VecDeque::new();
    let mut timers = tv::timer::TimerQueue::new();
    let mut deferred: Vec<tv::Deferred> = Vec::new();
    let mut ctx = headless(&mut out, &mut timers, &mut deferred);
    pump2.apply_startup_once(&mut ctx);
    assert_eq!(count_startup(&out), 0);
}
```

- [ ] **Step 7: Run to verify it fails**

Run: `cargo test -j4 -p edaptor posts_startup 2>&1 | tail -20`
Expected: FAIL to compile — no `apply_startup_once`, no `startup_posted`.

- [ ] **Step 8: Implement the pump one-shot**

In `src/ui/pump.rs`, add a `startup_posted: bool` field to `PumpView` (after
`fullscreen_applied: bool,`), initialise it `false` in `new`, and add the method
(mirroring `apply_fullscreen_once`):

```rust
    /// One-shot: if a startup action is pending, post `STARTUP` so `dispatch` runs it
    /// from the main loop (safe re-entry point, like `GUARD_NAV`). Posted at most once.
    fn apply_startup_once(&mut self, ctx: &mut Context) {
        if self.startup_posted {
            return;
        }
        if self.state.borrow().pending_startup.is_some() {
            ctx.post(crate::ui::STARTUP);
        }
        self.startup_posted = true;
    }
```

Call it in the `Event::Timer` branch, right after `self.apply_fullscreen_once(ctx);`:

```rust
            self.apply_startup_once(ctx);
```

- [ ] **Step 9: Run to verify the pump test passes**

Run: `cargo test -j4 -p edaptor posts_startup 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 10: Verify the whole workspace builds and tests pass**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5 && cargo test -j4 2>&1 | tail -15`
Expected: clippy clean; all tests PASS.

- [ ] **Step 11: Commit**

```bash
git add src/ui/mod.rs src/ui/state.rs src/ui/pump.rs src/ui/app.rs src/main.rs
git commit -m "$(printf 'feat(ui): StartupAction + one-shot STARTUP dispatch\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 6: Part 2 CLI — `edaptor tui-create` subcommand

**Files:**
- Modify: `src/main.rs` (add `Command::TuiCreate`, dispatch arm, `build_startup_action` + test)

**Interfaces:**
- Consumes: `edaptor::workflows::create::resolve_profile_arg`, `edaptor::ui::StartupAction`,
  `run_tui` (Task 5).

- [ ] **Step 1: Write the failing test for `build_startup_action`**

Add a `#[cfg(test)] mod tests` block at the bottom of `src/main.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use edaptor::config::EntryProfile;
    use edaptor::ui::StartupAction;

    fn profiles() -> Vec<EntryProfile> {
        vec![
            EntryProfile {
                name: "Users".into(),
                search_base: "ou=people,dc=example,dc=org".into(),
                ..Default::default()
            },
            EntryProfile {
                name: "Groups".into(),
                search_base: "ou=groups,dc=example,dc=org".into(),
                ..Default::default()
            },
        ]
    }

    #[test]
    fn named_profile_defaults_container_to_search_base() {
        let a =
            build_startup_action(&profiles(), Some("users"), None).expect("ok");
        match a {
            StartupAction::Create {
                profile_idx,
                container,
            } => {
                assert_eq!(profile_idx, 0);
                assert_eq!(container, "ou=people,dc=example,dc=org");
            }
            _ => panic!("expected Create"),
        }
    }

    #[test]
    fn container_override_wins() {
        let a = build_startup_action(
            &profiles(),
            Some("Users"),
            Some("ou=staff,ou=people,dc=example,dc=org".into()),
        )
        .expect("ok");
        match a {
            StartupAction::Create { container, .. } => {
                assert_eq!(container, "ou=staff,ou=people,dc=example,dc=org")
            }
            _ => panic!("expected Create"),
        }
    }

    #[test]
    fn no_profile_yields_choose_then_create() {
        let a = build_startup_action(&profiles(), None, None).expect("ok");
        assert!(matches!(
            a,
            StartupAction::ChooseThenCreate { container: None }
        ));
    }

    #[test]
    fn unknown_profile_errors() {
        let e = build_startup_action(&profiles(), Some("Admins"), None).unwrap_err();
        assert!(e.to_string().contains("Admins"));
    }

    #[test]
    fn blank_container_errors() {
        let e =
            build_startup_action(&profiles(), Some("Users"), Some("   ".into())).unwrap_err();
        assert!(e.to_string().contains("container"));
    }

    #[test]
    fn empty_search_base_without_container_errors() {
        let ps = vec![EntryProfile {
            name: "NoBase".into(),
            search_base: String::new(),
            ..Default::default()
        }];
        let e = build_startup_action(&ps, Some("NoBase"), None).unwrap_err();
        assert!(e.to_string().contains("search_base"));
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -j4 -p edaptor --bin edaptor build_startup 2>&1 | tail -20`
Expected: FAIL to compile — `build_startup_action` not found.

- [ ] **Step 3: Implement `build_startup_action`**

Add this free function to `src/main.rs` (above `run_tui`):

```rust
/// Turn the `tui-create` arguments into a [`edaptor::ui::StartupAction`], resolving the
/// profile name and container *before* the TUI launches so errors surface on the
/// terminal (never after a screen takeover). A blank `--container`, an unknown profile,
/// or a profile with no `search_base` and no `--container` are all errors.
fn build_startup_action(
    profiles: &[edaptor::config::EntryProfile],
    profile: Option<&str>,
    container: Option<String>,
) -> Result<edaptor::ui::StartupAction> {
    use edaptor::ui::StartupAction;

    if let Some(c) = &container {
        if c.trim().is_empty() {
            return Err(anyhow::anyhow!("--container must not be empty"));
        }
    }
    match edaptor::workflows::create::resolve_profile_arg(profiles, profile)
        .map_err(|e| anyhow::anyhow!(e))?
    {
        Some(idx) => {
            let dn = container.unwrap_or_else(|| profiles[idx].search_base.clone());
            if dn.trim().is_empty() {
                return Err(anyhow::anyhow!(
                    "profile '{}' has no search_base; pass --container",
                    profiles[idx].name
                ));
            }
            Ok(StartupAction::Create {
                profile_idx: idx,
                container: dn,
            })
        }
        None => Ok(StartupAction::ChooseThenCreate { container }),
    }
}
```

- [ ] **Step 4: Run to verify the tests pass**

Run: `cargo test -j4 -p edaptor --bin edaptor build_startup 2>&1 | tail -20`
Expected: PASS — 6 tests.

- [ ] **Step 5: Add the `TuiCreate` subcommand + dispatch arm**

In `src/main.rs`, add a variant to the `Command` enum (after `Passwd { .. }`):

```rust
    /// Launch the TUI straight into a profile's create form. With no `<profile>` a
    /// chooser is shown first. `--container` defaults to the profile's `search_base`.
    TuiCreate {
        /// Profile name to create (case-insensitive). Omit to pick from a chooser.
        profile: Option<String>,
        /// Container DN for the new object. Defaults to the profile's search_base.
        #[arg(long, value_name = "DN")]
        container: Option<String>,
    },
```

Add the dispatch arm in `main`'s `match command` (after the `Passwd` arm):

```rust
        Some(Command::TuiCreate { profile, container }) => {
            let action = build_startup_action(&config.profiles, profile.as_deref(), container)?;
            run_tui(config, password, Some(action))?;
        }
```

- [ ] **Step 6: Verify build, tests, and the help text**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5 && cargo test -j4 2>&1 | tail -15`
Expected: clippy clean; all tests PASS.

Then confirm the subcommand is wired:
Run: `cargo run -- tui-create --help 2>&1 | tail -20`
Expected: usage shows `[PROFILE]` positional and `--container <DN>`.

- [ ] **Step 7: Manual check (record the result)**

```bash
scripts/test-ldap.sh start
export EDAPTOR_TEST_ADMIN_PW=adminpassword
cargo run -- --config examples/demo-config.toml tui-create Users
```
Expected: the TUI opens directly into the new-`Users` create form; the confirm LDIF
shows the DN under the profile's `search_base`. Also try without a name
(`… tui-create`) → the profile chooser appears first; and an unknown name
(`… tui-create Nope`) → an error prints and the TUI never launches.

- [ ] **Step 8: Commit**

```bash
git add src/main.rs
git commit -m "$(printf 'feat(cli): edaptor tui-create <profile> launcher\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 7: Docs + CHANGES

**Files:**
- Modify: `CHANGES.md` (Unreleased)
- Modify: `docs/src/usage/crud.md` (container-choice behaviour + `tui-create`)

- [ ] **Step 1: CHANGES.md — add entries under Unreleased**

Under `### New`, after the live-autofill bullet, add:

```markdown
- **`edaptor tui-create [<profile>] [--container <DN>]`.** Launch straight into a
  profile's create form. With no `<profile>` a chooser is shown first; `--container`
  defaults to the profile's `search_base`. An unknown profile name errors before the
  TUI starts.
```

Under `### Changed`, add:

```markdown
- **Creating above a profile's home OU now asks where to put the object.** Pressing
  New while standing above a profile's `search_base` prompts "Create where?" —
  the current branch or the profile's home OU — instead of silently composing the
  entry at the current (wrong) location.
```

- [ ] **Step 2: docs/src/usage/crud.md — document both**

Read the file to find the section that covers creating a new entry (New / Alt-N). Add
these two subsections there — (a) the "Create where?" prompt, then (b) the CLI launcher:

```markdown
### Where a new entry is created

New entries land in the container that matches the chosen profile. If you press **New**
while the tree is focused on a branch *above* that profile's `search_base` (for example
at the directory root), eDAPtor cannot tell whether you meant "here" or "in the
profile's home OU", so it asks with a **Create where?** prompt:

- **Here — `<current branch>`** creates the entry where the tree is focused.
- **In `<search_base>`** creates it in the profile's home OU.

Standing on the profile's `search_base` (or inside it) skips the prompt and creates the
entry there directly.

### Launching into a create form from the command line

`edaptor tui-create <profile>` opens the TUI directly on a new-entry form for the named
profile, skipping the browse-and-navigate step:

    edaptor tui-create Users

- `<profile>` is matched case-insensitively against the configured profile names. Omit
  it to be shown a profile chooser at launch. An unknown name prints the list of valid
  names and exits before the TUI starts.
- `--container <DN>` overrides where the new object is created; by default it lands in
  the profile's `search_base`.

The form, defaults, autonumber, password entry, confirmation and write are exactly the
interactive create flow — `tui-create` only chooses which form opens.
```

- [ ] **Step 3: Build the docs**

Run: `make docs 2>&1 | tail -15`
Expected: mdBook builds without error.

- [ ] **Step 4: Commit**

```bash
git add CHANGES.md docs/src/usage/crud.md
git commit -m "$(printf 'docs: tui-create subcommand + create-container prompt\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Final verification

- [ ] **Run the full gate**

Run: `make check 2>&1 | tail -20`
Expected: fmt clean, clippy `-D warnings` clean, all tests PASS.

- [ ] **Confirm the manual checks from Task 3 Step 5 and Task 6 Step 7 were performed**
  and their outcomes recorded (container prompt fires only above the home OU;
  `tui-create` opens the right form; unknown name errors pre-launch).
