//! tvision-rs UI (migration target). Built under `src/tui/` during M1-M4 and
//! run via the `edaptor-tv` dev binary; renamed to `src/ui/` at the M5 cutover.
//! Only this module tree (and `src/bin/edaptor-tv.rs`) may `use tvision_rs`.

pub mod panes;
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
