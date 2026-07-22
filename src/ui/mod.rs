//! tvision-rs UI: the three-pane LDAP browser/editor. Built under `src/tui/`
//! during M1-M4 and renamed to `src/ui/` at the M5b cutover; it is now the sole
//! UI. Only this module tree may `use tvision_rs`.

mod app;
pub(crate) mod help_ctx;
pub(crate) mod lookup;
pub(crate) mod theme;
// Keep `pub`: builders and guard_decision are not yet called from non-test code
// (wired in Task 8). `pub` suppresses the dead_code lint without `#[allow]`.
pub(crate) mod choice;
pub mod dialog;
pub(crate) mod multi_picker;
pub(crate) mod oc_picker;
pub(crate) mod ordered;
pub(crate) mod panes;
pub(crate) mod picker;
pub(crate) mod pump;
pub(crate) mod pw_editor;
pub(crate) mod scroll_group;
pub(crate) mod shuttle;
// `pub` (not `pub(crate)`): the `edaptor` binary (`src/main.rs`) calls
// `edaptor::ui::startup::resolve_config_path`, so the module must be a
// crate-external item.
pub mod startup;
mod state;
#[cfg(test)]
pub(crate) mod test_support;
// Keep `pub`: the FieldWidget plugin contract is defined here for M1 and
// consumed in M2. `pub` keeps the as-yet-unused contract types visible as
// public API surface so they are NOT dead_code — no `#[allow]` needed.
pub mod widget;

use std::cell::RefCell;
use std::rc::Rc;

pub use state::UiState;

/// Shared mutable app state, cloned into each pane factory closure.
pub type Shared = Rc<RefCell<UiState>>;

/// Broadcast command: re-render all panes from current `UiState`.
pub const REFRESH: tv::Command = tv::Command::custom("edaptor.refresh");

/// Form pane command: activate the focused field's modal editor.
pub const ACTIVATE: tv::Command = tv::Command::custom("edaptor.activate_field");

/// App-level commands routed to `app::dispatch` via `run_app`.
pub const SAVE: tv::Command = tv::Command::custom("edaptor.save");
pub const CREATE: tv::Command = tv::Command::custom("edaptor.create");
pub const REQUEST_QUIT: tv::Command = tv::Command::custom("edaptor.request_quit");
pub const GUARD_NAV: tv::Command = tv::Command::custom("edaptor.guard_nav");
pub const SHOW_ERROR: tv::Command = tv::Command::custom("edaptor.show_error");

pub const STARTUP: tv::Command = tv::Command::custom("edaptor.startup");

/// Re-run the eager structure scan (Alt+R) — the escape hatch for structure
/// staleness that no local reflow can see (another client created a container).
pub const RELOAD: tv::Command = tv::Command::custom("edaptor.reload");

/// A one-shot action to run once the TUI has started (schema is already loaded by
/// `bootstrap`). Carried on `UiState::pending_startup`, posted by the pump as the
/// `STARTUP` command, and executed once in `app::dispatch`.
#[derive(Debug, Clone)]
pub enum StartupAction {
    /// Open a create form for `profile_idx` under `container`.
    Create {
        profile_idx: usize,
        container: String,
    },
    /// Show the all-profiles chooser, then open a create form for the pick under
    /// `container` (the pick's `search_base` when `None`).
    ChooseThenCreate { container: Option<String> },
}

use anyhow::Result;
use tvision_rs::{self as tv, CrosstermBackend};

use crate::config::Config;

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
