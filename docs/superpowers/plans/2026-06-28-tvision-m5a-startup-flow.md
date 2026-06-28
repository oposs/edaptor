# M5a — Startup Flow Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the ratatui config-picker with a tvision one and wire the full pre-TUI startup sequence (path resolution → `Config::load` → password → bootstrap → main `Program`), testable now via `edaptor-tv` while the ratatui UI still ships.

**Architecture:** A new `src/tui/dialog/config_picker.rs` builds a centered `Dialog` (a `ListBox` of config names + a two-line read-only detail pane showing the highlighted config's description and full path). A new `src/tui/startup.rs` owns the sequence: a pure `decide_config_path` decision function, and `run_config_picker` which drives the dialog in a **short-lived, self-contained tvision `Program`** (a zero-area `PickerTrigger` posts `SHOW_PICKER` on its first timer tick; the `run_app` closure `exec_view_focused`s the dialog, reads the staged index, and ends the program). `edaptor-tv` calls `startup::resolve_config_path`; `main.rs` adopts it at the M5b cutover.

**Tech Stack:** Rust, `tvision-rs` 0.3, the existing `config::discovery` domain module.

**Spec:** [`2026-06-28-tvision-m5a-startup-flow-design.md`](../specs/2026-06-28-tvision-m5a-startup-flow-design.md)

## Global Constraints

- **Facade boundary:** only `src/tui/**` and `src/bin/edaptor-tv.rs` may `use tvision_rs`. The domain layer stays UI-agnostic. `config::discovery` is reused unchanged.
- **Cap parallelism at 4 cores.** Target dir is `/home/oetiker/scratch/cargo-target`. Build the dev binary with `cargo build -j4 --bin edaptor-tv`; run from `/home/oetiker/scratch/cargo-target/debug/edaptor-tv`.
- **Strict TDD**, atomic commits, crate compiles after every commit, `cargo fmt` before each commit, `cargo clippy --all-targets -- -D warnings` clean.
- **Borrow discipline:** never hold a `RefCell` borrow across `ctx.post` / `ctx.broadcast` / `exec_view*` / `new_list` / `child_mut` / `set_value`. Collect into locals, drop the borrow, then call.
- **Do NOT touch `src/ui/**`** (the ratatui tree, incl. `config_picker.rs`) — it dies at the M5b cutover.
- **Commit trailer:** `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. Use `git commit -F` for messages containing backticks.
- **Facade guards (must print nothing):**
  ```bash
  ! grep -rl "use tvision_rs" src | grep -vE "^src/(tui/|bin/edaptor-tv.rs)"
  ! grep -rl "use ratatui\|use tui_" src | grep -vE "^src/ui/"
  ```

---

## File Structure

- **Create `src/tui/dialog/config_picker.rs`** — the picker `Dialog` view. Owns: `PickerItem` (name/description/path), the `ConfigPicker` view (`#[delegate(to = dlg)]`), `build(items, selected) -> (Box<dyn View>, ViewId)`. Stages the highlighted index into a caller-owned `Rc<RefCell<Option<usize>>>`; updates the detail cells on selection-change.
- **Modify `src/tui/dialog/mod.rs`** — declare `pub(crate) mod config_picker;` and add a build smoke test.
- **Create `src/tui/startup.rs`** — `PathDecision` enum + pure `decide_config_path`, `resolve_config_path` (the public entry), `run_config_picker` (the short-lived `Program`), `PickerTrigger` (zero-area one-shot), and the `SHOW_PICKER` command const.
- **Modify `src/tui/mod.rs`** — declare `pub(crate) mod startup;`.
- **Modify `src/bin/edaptor-tv.rs`** — resolve the config path via `edaptor::tui::startup::resolve_config_path` instead of the hardcoded-default parser.
- **Modify `CHANGES.md`** — entry under the unreleased section.

---

## Task 1: Config-picker Dialog

**Files:**
- Create: `src/tui/dialog/config_picker.rs`
- Modify: `src/tui/dialog/mod.rs` (add `pub(crate) mod config_picker;` + smoke test)
- Test: inline `#[cfg(test)]` in `config_picker.rs`

**Interfaces:**
- Produces:
  - `pub(crate) struct PickerItem { pub name: String, pub description: String, pub path: std::path::PathBuf }`
  - `pub(crate) fn build(items: Vec<PickerItem>, selected: std::rc::Rc<std::cell::RefCell<Option<usize>>>) -> (Box<dyn tvision_rs::View>, tvision_rs::ViewId)` — returns the dialog view and the `ListBox`'s `ViewId` (the `exec_view_focused` focus target). On `reset_current` the dialog seeds the list, stages index `Some(0)`, and fills the detail cells for item 0; arrow nav restages the index and refreshes the detail cells.
- Consumes: nothing from other tasks.

- [ ] **Step 1: Write the failing test**

Create `src/tui/dialog/config_picker.rs` with only the test module (the rest is added in Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::rc::Rc;
    use tvision_rs::{self as tv, Context, Event, FieldValue, Key, KeyEvent};

    fn items() -> Vec<PickerItem> {
        vec![
            PickerItem {
                name: "production".into(),
                description: "prod directory".into(),
                path: PathBuf::from("/etc/edaptor/prod.toml"),
            },
            PickerItem {
                name: "lab".into(),
                description: "local lab".into(),
                path: PathBuf::from("/home/me/.config/edaptor/lab.toml"),
            },
        ]
    }

    fn make_ctx<'a>(
        out: &'a mut VecDeque<Event>,
        timers: &'a mut tv::timer::TimerQueue,
        deferred: &'a mut Vec<tv::Deferred>,
    ) -> Context<'a> {
        Context::new(out, timers, 0, deferred)
    }

    /// reset_current seeds the list, stages index 0, and fills the detail cells
    /// for item 0; a Down event restages the index to 1 and refreshes the detail.
    #[test]
    fn reset_seeds_index_zero_and_down_updates_index_and_detail() {
        let selected: Rc<RefCell<Option<usize>>> = Rc::new(RefCell::new(None));
        let (mut view, _focus) = build(items(), selected.clone());

        assert_eq!(*selected.borrow(), None, "None before reset_current");

        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<tv::Deferred> = Vec::new();
        let mut ctx = make_ctx(&mut out, &mut timers, &mut deferred);
        view.reset_current(&mut ctx);
        assert_eq!(*selected.borrow(), Some(0), "Some(0) after reset_current");

        // Detail cells reflect item 0.
        {
            let picker = view
                .as_any_mut()
                .and_then(|a| a.downcast_mut::<ConfigPicker>())
                .expect("downcast ConfigPicker");
            let desc = picker.detail_text(picker.desc_id);
            let path = picker.detail_text(picker.path_id);
            assert_eq!(desc, "prod directory");
            assert_eq!(path, "/etc/edaptor/prod.toml");

            let mut ev = Event::KeyDown(KeyEvent::from(Key::Down));
            picker.handle_event(&mut ev, &mut ctx);
        }
        assert_eq!(*selected.borrow(), Some(1), "Some(1) after Down");

        let picker = view
            .as_any_mut()
            .and_then(|a| a.downcast_mut::<ConfigPicker>())
            .expect("downcast ConfigPicker");
        assert_eq!(picker.detail_text(picker.path_id), "/home/me/.config/edaptor/lab.toml");
        let _ = FieldValue::Int(0); // keep the import used if asserts change
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -j4 --lib tui::dialog::config_picker 2>&1 | tail -20`
Expected: FAIL — `PickerItem`, `build`, `ConfigPicker` not found (and the module isn't declared yet).

- [ ] **Step 3: Write the implementation**

Prepend to `src/tui/dialog/config_picker.rs` (above the test module):

```rust
//! Config-picker dialog: shown by the startup flow when config discovery finds
//! more than one candidate. A `ListBox` of config names plus a two-line
//! read-only detail pane (description + full path) for the highlighted entry.
//!
//! Pattern mirrors `dialog::profile_chooser` / `oc_picker`: a `Dialog`-wrapping
//! `View` with `#[delegate(to = dlg)]`, list seeded in `reset_current` (NOT in
//! `new()`), highlighted index staged into a caller-owned cell. Unlike the
//! in-app dialogs it owns its own `Rc<RefCell<Option<usize>>>` rather than the
//! app `UiState` (which does not exist yet at startup).

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use tvision_rs::{
    self as tv, delegate, ButtonFlags, ButtonRowAlign, Command, Context, Dialog, Event, FieldValue,
    InputLine, Key, ListBox, Rect, View,
};

/// One discovered config, flattened for display (decoupled from `ConfigCandidate`
/// so the dialog is testable without filesystem discovery).
pub(crate) struct PickerItem {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
}

/// A disabled (read-only, skip-focus) `InputLine` whose text we set at runtime.
/// `StaticText` has no `set_value`, so we reuse the form-pane `ro_cell` idiom.
fn ro_cell(bounds: Rect) -> InputLine {
    let mut il = InputLine::with_limit(bounds, 1024);
    il.state.state.disabled = true;
    il
}

pub(crate) struct ConfigPicker {
    dlg: Dialog,
    list_id: tv::ViewId,
    desc_id: tv::ViewId,
    path_id: tv::ViewId,
    items: Vec<PickerItem>,
    selected: Rc<RefCell<Option<usize>>>,
}

impl ConfigPicker {
    fn new(items: Vec<PickerItem>, selected: Rc<RefCell<Option<usize>>>) -> Self {
        let list_rows = items.len().clamp(3, 12) as i32;
        // frame + list + gap + desc + path + gap + buttons + frame
        let height = 1 + list_rows + 1 + 1 + 1 + 1 + 2 + 1;
        let width = 72;
        let mut dlg = Dialog::new(
            Rect::new(0, 0, width, height),
            Some("Select configuration".to_string()),
        );
        dlg.state_mut().options.center_x = true;
        dlg.state_mut().options.center_y = true;

        let list = ListBox::new(Rect::new(2, 1, width - 2, 1 + list_rows), 1, None, None);
        let list_id = dlg.insert_child(Box::new(list));

        let desc_y = 1 + list_rows + 1;
        let desc_id = dlg.insert_child(Box::new(ro_cell(Rect::new(2, desc_y, width - 2, desc_y + 1))));
        let path_y = desc_y + 1;
        let path_id = dlg.insert_child(Box::new(ro_cell(Rect::new(2, path_y, width - 2, path_y + 1))));

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

        ConfigPicker {
            dlg,
            list_id,
            desc_id,
            path_id,
            items,
            selected,
        }
    }

    /// Read the current list-highlight index.
    fn current_index(&mut self) -> Option<usize> {
        match self.dlg.child_mut(self.list_id).and_then(|v| v.value()) {
            Some(FieldValue::Int(i)) if i >= 0 => Some(i as usize),
            _ => None,
        }
    }

    /// Stage the highlight into the caller's cell and refresh the detail cells.
    fn stage_and_show(&mut self) {
        let idx = self.current_index().unwrap_or(0);
        *self.selected.borrow_mut() = Some(idx);
        let (desc, path) = match self.items.get(idx) {
            Some(it) => (it.description.clone(), it.path.to_string_lossy().into_owned()),
            None => (String::new(), String::new()),
        };
        if let Some(c) = self.dlg.child_mut(self.desc_id) {
            c.set_value(FieldValue::Text(desc));
        }
        if let Some(c) = self.dlg.child_mut(self.path_id) {
            c.set_value(FieldValue::Text(path));
        }
    }

    /// Test helper: read a detail cell's current text.
    #[cfg(test)]
    fn detail_text(&mut self, id: tv::ViewId) -> String {
        match self.dlg.child_mut(id).and_then(|v| v.value()) {
            Some(FieldValue::Text(t)) => t,
            _ => String::new(),
        }
    }
}

#[delegate(to = dlg)]
impl View for ConfigPicker {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    fn reset_current(&mut self, ctx: &mut Context) {
        self.dlg.reset_current(ctx);
        let rows: Vec<String> = self.items.iter().map(|it| it.name.clone()).collect();
        if let Some(list) = self.dlg.child_mut(self.list_id) {
            if let Some(lb) = list.as_any_mut().and_then(|a| a.downcast_mut::<ListBox>()) {
                lb.new_list(rows, ctx);
            }
        }
        self.stage_and_show();
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
            self.stage_and_show();
        } else {
            self.dlg.handle_event(ev, ctx);
        }
    }
}

/// Build the config-picker dialog. Returns `(view, list_view_id)`; pass the id as
/// the focus target to `exec_view_focused` so the list is active immediately.
pub(crate) fn build(
    items: Vec<PickerItem>,
    selected: Rc<RefCell<Option<usize>>>,
) -> (Box<dyn View>, tv::ViewId) {
    let picker = ConfigPicker::new(items, selected);
    let list_id = picker.list_id;
    (Box::new(picker), list_id)
}
```

Then declare the module in `src/tui/dialog/mod.rs` (alongside the existing `pub mod profile_chooser;`):

```rust
pub(crate) mod config_picker;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -j4 --lib tui::dialog::config_picker 2>&1 | tail -20`
Expected: PASS (`reset_seeds_index_zero_and_down_updates_index_and_detail`).

- [ ] **Step 5: Add the build smoke test to `dialog/mod.rs`**

In the `#[cfg(test)] mod tests` of `src/tui/dialog/mod.rs`, alongside the other `*_builds_without_panic` tests:

```rust
#[test]
fn config_picker_builds_without_panic() {
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::rc::Rc;
    let items = vec![config_picker::PickerItem {
        name: "a".into(),
        description: "desc".into(),
        path: PathBuf::from("/tmp/a.toml"),
    }];
    let (_v, _id) = config_picker::build(items, Rc::new(RefCell::new(None)));
}
```

Run: `cargo test -j4 --lib tui::dialog 2>&1 | tail -15`
Expected: PASS (both the new picker test and the smoke test).

- [ ] **Step 6: Lint + fmt**

Run: `cargo fmt && cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -15`
Expected: clean (no warnings).

- [ ] **Step 7: Commit**

```bash
git add src/tui/dialog/config_picker.rs src/tui/dialog/mod.rs
git commit -F - <<'EOF'
feat(tui): config-picker dialog (ListBox + detail pane)

Centered Dialog with a ListBox of config names and a two-line read-only
detail pane (description + full path) for the highlighted entry. Stages
the highlight index into a caller-owned cell; nav refreshes the detail.
Mirrors dialog::profile_chooser but owns its own selection cell (no
UiState at startup).

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

## Task 2: Picker Program harness (`PickerTrigger` + `run_config_picker`)

**Files:**
- Create: `src/tui/startup.rs`
- Modify: `src/tui/mod.rs` (add `pub(crate) mod startup;`)
- Test: inline `#[cfg(test)]` in `startup.rs`

**Interfaces:**
- Consumes: `dialog::config_picker::{PickerItem, build}` (Task 1).
- Produces:
  - `pub(crate) const SHOW_PICKER: tv::Command`
  - `pub(crate) fn run_config_picker(items: Vec<PickerItem>) -> anyhow::Result<Option<PathBuf>>` — runs the short-lived `Program`, returns `Some(path)` on OK, `None` on Cancel/close. (Not headless-testable; verified live in Task 4.)
  - `PickerTrigger` (private) — zero-area view that posts `SHOW_PICKER` once on its first timer tick.

- [ ] **Step 1: Write the failing test**

Create `src/tui/startup.rs`:

```rust
//! Pre-TUI startup sequence: resolve the config path (explicit flag / discovery /
//! picker) before the main program connects. The picker runs in its own
//! short-lived tvision `Program` because the main program is built from an
//! already-bootstrapped state — there is no connection yet when the picker shows.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use anyhow::Result;
use tvision_rs::{self as tv, Context, DrawCtx, Event, View};

use crate::tui::dialog::config_picker::{self, PickerItem};

/// Posted once by `PickerTrigger` on the first timer tick; the picker program's
/// `run_app` closure responds by exec-viewing the dialog.
pub(crate) const SHOW_PICKER: tv::Command = tv::Command::custom("edaptor.show_picker");

/// Zero-area view that arms a timer on its first event and posts `SHOW_PICKER`
/// exactly once. Mirrors `pump::PumpView`'s one-shot `FULLSCREEN` post.
struct PickerTrigger {
    vs: tv::ViewState,
    armed: bool,
    posted: bool,
}

impl PickerTrigger {
    fn new() -> Self {
        PickerTrigger {
            vs: tv::ViewState::new(tv::Rect::new(0, 0, 0, 0)),
            armed: false,
            posted: false,
        }
    }

    /// One-shot: post `SHOW_PICKER` the first time this is reached.
    fn post_once(&mut self, ctx: &mut Context) {
        if self.posted {
            return;
        }
        ctx.post(SHOW_PICKER);
        self.posted = true;
    }
}

impl View for PickerTrigger {
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
            self.post_once(ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn headless<'a>(
        out: &'a mut VecDeque<Event>,
        timers: &'a mut tv::timer::TimerQueue,
        deferred: &'a mut Vec<tv::Deferred>,
    ) -> Context<'a> {
        Context::new(out, timers, 0, deferred)
    }

    /// PickerTrigger posts SHOW_PICKER exactly once, then never again.
    #[test]
    fn picker_trigger_posts_show_picker_once() {
        let mut trigger = PickerTrigger::new();
        let count = |out: &VecDeque<Event>| {
            out.iter()
                .filter(|e| matches!(e, Event::Command(c) if *c == SHOW_PICKER))
                .count()
        };

        {
            let mut out = VecDeque::new();
            let mut timers = tv::timer::TimerQueue::new();
            let mut deferred: Vec<tv::Deferred> = Vec::new();
            let mut ctx = headless(&mut out, &mut timers, &mut deferred);
            trigger.post_once(&mut ctx);
            assert_eq!(count(&out), 1, "first post emits exactly one SHOW_PICKER");
        }
        {
            let mut out = VecDeque::new();
            let mut timers = tv::timer::TimerQueue::new();
            let mut deferred: Vec<tv::Deferred> = Vec::new();
            let mut ctx = headless(&mut out, &mut timers, &mut deferred);
            trigger.post_once(&mut ctx);
            assert_eq!(count(&out), 0, "idempotent: no further posts");
        }
    }
}
```

Add to `src/tui/mod.rs` (with the other module declarations):

```rust
pub(crate) mod startup;
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -j4 --lib tui::startup 2>&1 | tail -20`
Expected: FAIL — `run_config_picker` is referenced by neither yet, but the module won't compile until the `Interfaces`-promised `run_config_picker` exists if anything calls it; at this point the test only exercises `PickerTrigger`. If it compiles, the test runs RED only if `post_once` is missing. (It is present in Step 1, so this step's RED is the *module-not-declared* compile error before adding the `mod startup;` line — declare it, then the test passes. To honour TDD, first run with the `post_once` body replaced by `{ let _ = ctx; }` to see the assertion fail.)

Concretely: temporarily change `post_once` to `{ let _ = ctx; }`, run the test, observe:
Expected: FAIL — `assert_eq!(count(&out), 1)` left `0`.

- [ ] **Step 3: Restore `post_once` and add `run_config_picker`**

Restore `post_once` to the real body (Step 1). Then append `run_config_picker` (above the test module):

```rust
/// Run the config picker in its own short-lived `Program`. Returns the chosen
/// path, or `None` if the user cancelled (caller exits cleanly).
///
/// The main program is built from an already-bootstrapped state, so the picker
/// cannot be a modal inside it. This minimal program's desktop holds only a
/// `PickerTrigger`; on its first tick the trigger posts `SHOW_PICKER`, the
/// `run_app` closure exec-views the dialog, reads the staged index, and ends.
pub(crate) fn run_config_picker(items: Vec<PickerItem>) -> Result<Option<PathBuf>> {
    use tvision_rs::{
        Command, CrosstermBackend, Desktop, Program, Rect, SystemClock, Theme, View,
    };

    let paths: Vec<PathBuf> = items.iter().map(|it| it.path.clone()).collect();
    let selected: Rc<RefCell<Option<usize>>> = Rc::new(RefCell::new(None));
    let chosen: Rc<RefCell<Option<usize>>> = Rc::new(RefCell::new(None));

    let backend = Box::new(CrosstermBackend::new()?);
    let mut program = Program::new(
        backend,
        Box::new(SystemClock::new()),
        Theme::classic_blue(),
        |r: Rect| -> Option<Box<dyn View>> {
            let mut desktop = Desktop::new(r, |br| Some(Desktop::init_background(br)));
            desktop.insert_view(Box::new(PickerTrigger::new()));
            Some(Box::new(desktop))
        },
        |_r| None,
        |_r| None,
    );

    // `items`/`selected` move into the closure; `chosen` is cloned out for reading.
    let chosen_w = chosen.clone();
    let cell = selected.clone();
    let mut items_opt = Some(items);
    program.run_app(move |prog, cmd| {
        if cmd == SHOW_PICKER {
            if let Some(items) = items_opt.take() {
                let (view, focus) = config_picker::build(items, cell.clone());
                let answer = prog.exec_view_focused(view, focus);
                if answer == Command::OK {
                    *chosen_w.borrow_mut() = *cell.borrow();
                }
                prog.end_modal(Command::QUIT);
            }
        }
    });

    let idx = *chosen.borrow();
    Ok(idx.and_then(|i| paths.get(i).cloned()))
}
```

- [ ] **Step 4: Run tests + lint**

Run: `cargo test -j4 --lib tui::startup 2>&1 | tail -15`
Expected: PASS (`picker_trigger_posts_show_picker_once`).

Run: `cargo fmt && cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -15`
Expected: clean. (`run_config_picker` is not yet called from non-test code — it becomes reachable in Task 3, which lands before any commit-gate that would flag dead code. If clippy flags it dead here, proceed to Task 3 in the same working session before the `make check` gate; the Task 2 commit below is followed immediately by Task 3.)

- [ ] **Step 5: Commit**

```bash
git add src/tui/startup.rs src/tui/mod.rs
git commit -F - <<'EOF'
feat(tui): picker program harness (PickerTrigger + run_config_picker)

A short-lived tvision Program that drives the config-picker dialog: a
zero-area PickerTrigger posts SHOW_PICKER on its first timer tick, the
run_app closure exec_view_focused's the dialog and reads the staged
index. Returns the chosen path or None on cancel.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

## Task 3: Path decision + `resolve_config_path`

**Files:**
- Modify: `src/tui/startup.rs` (add `PathDecision`, `decide_config_path`, `resolve_config_path`)
- Test: inline `#[cfg(test)]` in `startup.rs`

**Interfaces:**
- Consumes: `config::discovery::{discover_configs, ConfigCandidate}`; `run_config_picker` (Task 2).
- Produces:
  - `pub fn resolve_config_path(cli_config: Option<PathBuf>) -> Result<Option<PathBuf>>` — the public entry. `Ok(Some(path))` to use, `Ok(None)` if the user cancelled the picker (caller exits cleanly), `Err` if no config was found.
  - `PathDecision` + `decide_config_path` (pure, testable without a TTY).

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` in `src/tui/startup.rs`:

```rust
use crate::config::discovery::ConfigCandidate;
use crate::config::MetaConfig;

fn candidate(path: &str, name: Option<&str>) -> ConfigCandidate {
    ConfigCandidate {
        path: PathBuf::from(path),
        meta: MetaConfig {
            name: name.map(|s| s.to_string()),
            description: None,
            ..MetaConfig::default()
        },
    }
}

#[test]
fn explicit_flag_short_circuits_discovery() {
    let d = decide_config_path(Some(PathBuf::from("/x/my.toml")), Vec::new());
    assert!(matches!(d, PathDecision::Explicit(p) if p == PathBuf::from("/x/my.toml")));
}

#[test]
fn zero_candidates_is_none_found() {
    let d = decide_config_path(None, Vec::new());
    assert!(matches!(d, PathDecision::NoneFound));
}

#[test]
fn one_candidate_is_used_directly() {
    let d = decide_config_path(None, vec![candidate("/etc/edaptor/a.toml", Some("a"))]);
    assert!(matches!(d, PathDecision::Single(p) if p == PathBuf::from("/etc/edaptor/a.toml")));
}

#[test]
fn many_candidates_request_the_picker() {
    let d = decide_config_path(
        None,
        vec![
            candidate("/etc/edaptor/a.toml", Some("a")),
            candidate("/etc/edaptor/b.toml", Some("b")),
        ],
    );
    match d {
        PathDecision::Picker(items) => {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0].name, "a");
            assert_eq!(items[1].path, PathBuf::from("/etc/edaptor/b.toml"));
        }
        _ => panic!("expected Picker"),
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -j4 --lib tui::startup 2>&1 | tail -20`
Expected: FAIL — `decide_config_path` / `PathDecision` not found.

- [ ] **Step 3: Write the implementation**

Append to `src/tui/startup.rs` (above the test module). Add the imports at the top of the file: `use crate::config::discovery;`.

```rust
/// The outcome of inspecting the CLI flag + discovered candidates.
enum PathDecision {
    /// `--config <p>` was given; use it verbatim (discovery skipped).
    Explicit(PathBuf),
    /// Exactly one config discovered; use it (no picker).
    Single(PathBuf),
    /// More than one discovered; show the picker over these items.
    Picker(Vec<PickerItem>),
    /// No config found anywhere.
    NoneFound,
}

/// Pure decision: map the CLI flag + discovered candidates to a `PathDecision`.
/// Discovery itself (filesystem) is done by the caller so this stays testable.
fn decide_config_path(
    cli_config: Option<PathBuf>,
    candidates: Vec<discovery::ConfigCandidate>,
) -> PathDecision {
    if let Some(p) = cli_config {
        return PathDecision::Explicit(p);
    }
    match candidates.len() {
        0 => PathDecision::NoneFound,
        1 => PathDecision::Single(candidates.into_iter().next().unwrap().path),
        _ => PathDecision::Picker(
            candidates
                .into_iter()
                .map(|c| PickerItem {
                    name: c.display_name(),
                    description: c.meta.description.clone().unwrap_or_default(),
                    path: c.path,
                })
                .collect(),
        ),
    }
}

/// Resolve the config path for startup. `--config` wins; otherwise discover
/// configs and pick (0 → error, 1 → use it, many → picker dialog).
/// `Ok(None)` means the user cancelled the picker — the caller should exit cleanly.
pub fn resolve_config_path(cli_config: Option<PathBuf>) -> Result<Option<PathBuf>> {
    // Skip discovery entirely when an explicit path is given.
    let candidates = if cli_config.is_some() {
        Vec::new()
    } else {
        discovery::discover_configs()
    };
    match decide_config_path(cli_config, candidates) {
        PathDecision::Explicit(p) | PathDecision::Single(p) => Ok(Some(p)),
        PathDecision::NoneFound => Err(anyhow::anyhow!(
            "no config found in ~/.config/edaptor/ or /etc/edaptor/; \
             use --config to specify one"
        )),
        PathDecision::Picker(items) => run_config_picker(items),
    }
}
```

- [ ] **Step 4: Run tests + lint**

Run: `cargo test -j4 --lib tui::startup 2>&1 | tail -20`
Expected: PASS (all four decision tests + the trigger test).

Run: `cargo fmt && cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -15`
Expected: clean (`run_config_picker` is now reachable via `resolve_config_path`).

- [ ] **Step 5: Commit**

```bash
git add src/tui/startup.rs
git commit -F - <<'EOF'
feat(tui): config-path resolution (decide + resolve_config_path)

Pure decide_config_path maps the --config flag + discovered candidates
to Explicit/Single/Picker/NoneFound; resolve_config_path runs discovery
and dispatches (0 -> error, 1 -> use it, many -> picker dialog).
Returns Ok(None) when the user cancels the picker.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

## Task 4: Wire `edaptor-tv` + docs + live acceptance

**Files:**
- Modify: `src/bin/edaptor-tv.rs`
- Modify: `CHANGES.md`
- Test: update `src/bin/edaptor-tv.rs` arg-parsing tests; live tmux acceptance.

**Interfaces:**
- Consumes: `edaptor::tui::startup::resolve_config_path` (Task 3). **Note:** `resolve_config_path` is `pub` (Task 3) but reached here through the `tui` module; ensure `tui` re-exports it or call it via the full path `edaptor::tui::startup::resolve_config_path`. Add `pub use startup::resolve_config_path;` to `src/tui/mod.rs` if a shorter path is preferred — the plan uses the full path.

- [ ] **Step 1: Rewrite `edaptor-tv.rs` to use the startup flow**

The dev binary keeps its `EDAPTOR_TEST_ADMIN_PW` password shortcut, but now resolves the config path through discovery + the picker (so the flow is live-testable). Replace the body of `src/bin/edaptor-tv.rs` with:

```rust
//! Dev binary for the in-progress tvision UI (M1-M5a). Deleted at the M5b cutover.
//! Usage: `cargo run -j4 --bin edaptor-tv -- [--config <path>]`
//! With no --config, discovers configs in ~/.config/edaptor and /etc/edaptor and
//! shows the picker if more than one is found. Password from EDAPTOR_TEST_ADMIN_PW
//! (demo: adminpassword).

use std::path::PathBuf;

use anyhow::{anyhow, Result};
use edaptor::config::Config;

/// Parse only the `--config <path>` / `--config=<path>` flag; everything else is
/// ignored. Returns `None` when no flag is present (→ discovery + picker).
fn config_flag<I: IntoIterator<Item = String>>(args: I) -> Option<PathBuf> {
    let mut iter = args.into_iter();
    while let Some(a) = iter.next() {
        if a == "--config" {
            return iter.next().map(PathBuf::from);
        } else if let Some(p) = a.strip_prefix("--config=") {
            return Some(PathBuf::from(p));
        }
    }
    None
}

fn main() -> Result<()> {
    let cli_config = config_flag(std::env::args().skip(1));
    let path = match edaptor::tui::startup::resolve_config_path(cli_config)? {
        Some(p) => p,
        None => return Ok(()), // user cancelled the picker
    };
    let config = Config::load(&path)?;
    let password = std::env::var("EDAPTOR_TEST_ADMIN_PW")
        .map_err(|_| anyhow!("set EDAPTOR_TEST_ADMIN_PW (demo: adminpassword)"))?;
    edaptor::tui::run(config, password)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flag(args: &[&str]) -> Option<String> {
        config_flag(args.iter().map(|s| s.to_string())).map(|p| p.to_string_lossy().into_owned())
    }

    #[test]
    fn flag_with_separate_value() {
        assert_eq!(flag(&["--config", "a.toml"]).as_deref(), Some("a.toml"));
    }

    #[test]
    fn flag_with_equals() {
        assert_eq!(flag(&["--config=foo.toml"]).as_deref(), Some("foo.toml"));
    }

    #[test]
    fn no_flag_is_none() {
        assert_eq!(flag(&[]), None);
        assert_eq!(flag(&["something"]), None);
    }
}
```

This makes `tui::startup` reachable: ensure `src/tui/mod.rs` exposes it — `pub(crate) mod startup;` is enough for the `edaptor::tui::startup::...` path **only if** `tui` is `pub` and `startup` is `pub`. Since the binary is outside the crate, change the declaration in `src/tui/mod.rs` to `pub mod startup;` (matching the existing `pub mod widget;` / `pub mod dialog;` convention) and keep `resolve_config_path` `pub`.

- [ ] **Step 2: Run the binary's unit tests**

Run: `cargo test -j4 --bin edaptor-tv 2>&1 | tail -15`
Expected: PASS (the three `config_flag` tests).

- [ ] **Step 3: Full gate**

Run:
```bash
cargo fmt --check
cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -10
cargo test -j4 2>&1 | tail -15
```
Expected: fmt clean; clippy clean; all lib + bin tests pass.

Facade guards (must print nothing):
```bash
! grep -rl "use tvision_rs" src | grep -vE "^src/(tui/|bin/edaptor-tv.rs)"
! grep -rl "use ratatui\|use tui_" src | grep -vE "^src/ui/"
```

- [ ] **Step 4: Live tmux acceptance — picker path**

Build, seed a temp config dir with two configs, point `XDG_CONFIG_HOME` at it, and drive the picker:

```bash
cargo build -j4 --bin edaptor-tv
scripts/test-ldap.sh start
PICKDIR=/tmp/claude-1003/-home-oetiker-checkouts-edaptor/65e6bbae-ad5a-48c1-ad91-59fe1c2a3693/scratchpad/edaptor
mkdir -p "$PICKDIR"
# Two real configs that both point at the demo server (copy the demo, add [meta]).
{ printf '[meta]\nname = "demo-one"\ndescription = "first demo config"\n\n'; cat examples/demo-config.toml; } > "$PICKDIR/one.toml"
{ printf '[meta]\nname = "demo-two"\ndescription = "second demo config"\n\n'; cat examples/demo-config.toml; } > "$PICKDIR/two.toml"

tmux kill-session -t edtv 2>/dev/null
tmux new-session -d -s edtv -x 210 -y 50
tmux send-keys -t edtv "export EDAPTOR_TEST_ADMIN_PW=adminpassword XDG_CONFIG_HOME=$PICKDIR/cfg" Enter
# Put the two configs where discovery looks: $XDG_CONFIG_HOME/edaptor/
tmux send-keys -t edtv "mkdir -p $PICKDIR/cfg/edaptor && cp $PICKDIR/one.toml $PICKDIR/two.toml $PICKDIR/cfg/edaptor/" Enter
tmux send-keys -t edtv '/home/oetiker/scratch/cargo-target/debug/edaptor-tv' Enter
sleep 3
tmux capture-pane -t edtv -p | sed -n '1,24p'   # expect the "Select configuration" dialog + 2 names + detail
```
Expected: a centered **Select configuration** dialog listing `demo-one` / `demo-two`, with the description + path of the highlighted entry shown below.

Drive it:
```bash
tmux send-keys -t edtv Down ; sleep 0.4
tmux capture-pane -t edtv -p | sed -n '1,24p'   # detail pane now shows demo-two's path
tmux send-keys -t edtv Enter ; sleep 4
tmux capture-pane -t edtv -p | sed -n '1,6p'     # main 3-pane TUI loaded from the chosen config
tmux send-keys -t edtv M-x ; sleep 0.5           # Alt-X quit guard (clean form → exits)
tmux capture-pane -t edtv -p | sed -n '1,6p'
tmux kill-session -t edtv 2>/dev/null
```
Expected: Down moves the highlight + updates the detail path; Enter loads the main TUI; Alt-X exits.

Also verify Cancel exits cleanly:
```bash
tmux new-session -d -s edtv -x 210 -y 50
tmux send-keys -t edtv "export EDAPTOR_TEST_ADMIN_PW=adminpassword XDG_CONFIG_HOME=$PICKDIR/cfg" Enter
tmux send-keys -t edtv '/home/oetiker/scratch/cargo-target/debug/edaptor-tv' Enter
sleep 3
tmux send-keys -t edtv Escape ; sleep 1
tmux capture-pane -t edtv -p | sed -n '1,4p'     # back at the shell prompt (clean exit, no panic)
tmux kill-session -t edtv 2>/dev/null
```
Expected: Esc closes the picker and the process exits cleanly (shell prompt returns), no panic.

- [ ] **Step 5: Live tmux acceptance — single-config + explicit-flag skip the picker**

```bash
# Single config in the discovery dir → no picker.
rm -f "$PICKDIR/cfg/edaptor/two.toml"
tmux new-session -d -s edtv -x 210 -y 50
tmux send-keys -t edtv "export EDAPTOR_TEST_ADMIN_PW=adminpassword XDG_CONFIG_HOME=$PICKDIR/cfg" Enter
tmux send-keys -t edtv '/home/oetiker/scratch/cargo-target/debug/edaptor-tv' Enter
sleep 4
tmux capture-pane -t edtv -p | sed -n '1,4p'     # straight into the main TUI, no picker
tmux send-keys -t edtv M-x ; sleep 0.5
tmux kill-session -t edtv 2>/dev/null

# Explicit --config → no discovery, no picker.
tmux new-session -d -s edtv -x 210 -y 50
tmux send-keys -t edtv 'export EDAPTOR_TEST_ADMIN_PW=adminpassword' Enter
tmux send-keys -t edtv '/home/oetiker/scratch/cargo-target/debug/edaptor-tv --config examples/demo-config.toml' Enter
sleep 4
tmux capture-pane -t edtv -p | sed -n '1,4p'     # straight into the main TUI
tmux send-keys -t edtv M-x ; sleep 0.5
tmux kill-session -t edtv 2>/dev/null
```
Expected: both paths skip the picker and load the main TUI directly. (Note: `--config` discovery is skipped even though `XDG_CONFIG_HOME` is unset here.)

- [ ] **Step 6: Update `CHANGES.md`**

Add under the unreleased section (tvision preview area), matching the existing entry style:

```markdown
- **tvision UI: config picker at startup.** When more than one config is
  discovered in `~/.config/edaptor/` or `/etc/edaptor/`, a Turbo-Vision
  "Select configuration" dialog now lists them (name + description + path) so
  you can choose one; a single discovered config or an explicit `--config`
  skips the picker. (Startup-flow groundwork for the M5 cutover.)
```

- [ ] **Step 7: Commit**

```bash
git add src/bin/edaptor-tv.rs src/tui/mod.rs CHANGES.md
git commit -F - <<'EOF'
feat(tui): wire edaptor-tv through the startup config resolution

edaptor-tv now resolves its config path via tui::startup::resolve_config_path
(discovery + picker) instead of a hardcoded default, so the startup flow is
live-testable before the M5b cutover wires it into main.rs. Live-verified:
two-config picker (nav + detail + Enter + Esc), single-config and --config
skip the picker.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

## Final gate (whole milestone)

- [ ] `make check` green (fmt + clippy `-D warnings` + tests).
- [ ] Facade guards print nothing.
- [ ] Picker live-verified (nav, detail pane, Enter loads, Esc cancels); single-config + `--config` skip it.
- [ ] `src/ui/config_picker.rs` (ratatui) untouched.
- [ ] `CHANGES.md` updated.
- [ ] Update `docs/HANDOVER.md`: M5a done; next is M5b (cutover + the three reconciliations, with the schema-aware last-member note from the spec §9).

## Self-review notes (coverage check against the spec)

- Spec §3 ordering: M5a delivers path resolution + picker only; `Config::load` / password / bootstrap stay in the caller (`edaptor-tv` now, `main.rs` at cutover) — Task 4 wires the path resolution into the existing caller, leaving the rest unchanged. ✓
- Spec §4 two-Program structure: Task 2 `run_config_picker`. ✓
- Spec §5 ListBox + detail pane: Task 1. ✓
- Spec §6 module layout (`startup.rs`, `dialog/config_picker.rs`, `edaptor-tv` wiring): Tasks 1–4. ✓
- Spec §7 testing (path-resolution unit tests, dialog headless tests, live tmux): Tasks 1, 3, 4. ✓
- Spec §9/§10: deferred notes carried in the spec; handover update in the final gate. ✓
