//! tvision-rs UI (migration target). Built under `src/tui/` during M1-M4 and
//! run via the `edaptor-tv` dev binary; renamed to `src/ui/` at the M5 cutover.
//! Only this module tree (and `src/bin/edaptor-tv.rs`) may `use tvision_rs`.

mod app;
pub(crate) mod panes;
pub(crate) mod pump;
mod state;
pub mod widget;

use std::cell::RefCell;
use std::rc::Rc;

pub use state::UiState;

/// Shared mutable app state, cloned into each pane factory closure.
pub type Shared = Rc<RefCell<UiState>>;

/// Broadcast command: re-render all panes from current `UiState`.
pub const REFRESH: tv::Command = tv::Command::custom("edaptor.refresh");

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
