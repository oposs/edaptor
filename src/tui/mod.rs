//! tvision-rs UI (migration target). Built under `src/tui/` during M1-M4 and
//! run via the `edaptor-tv` dev binary; renamed to `src/ui/` at the M5 cutover.
//! Only this module tree (and `src/bin/edaptor-tv.rs`) may `use tvision_rs`.

mod app;
pub(crate) mod panes;
pub(crate) mod pump;
mod state;
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

/// App-level commands routed to `app::dispatch` via `run_app`.
pub const SAVE: tv::Command = tv::Command::custom("edaptor.save");
pub const REQUEST_QUIT: tv::Command = tv::Command::custom("edaptor.request_quit");
pub const GUARD_NAV: tv::Command = tv::Command::custom("edaptor.guard_nav");
pub const SHOW_ERROR: tv::Command = tv::Command::custom("edaptor.show_error");

use anyhow::Result;
use tvision_rs::{self as tv, CrosstermBackend};

use crate::config::Config;

/// Spawn the worker, fetch schema + structure, then run the TUI.
pub fn run(config: Config, password: String) -> Result<()> {
    let state: Shared = Rc::new(RefCell::new(state::bootstrap(config, password)?));
    let backend = Box::new(CrosstermBackend::new()?);
    let mut program = app::build_program(backend, state);
    program.run_app(|_prog, _cmd| {});
    Ok(())
}
