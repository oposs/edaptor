//! ratatui application state and the event loop.
//!
//! This replaces the old turbo-vision facade + `Shell::run_loop` callback. The
//! loop is immediate-mode: it owns all state as plain data ([`App`]) and
//! re-renders every frame, so the shared `Rc<RefCell>` pane handles and the
//! `CM_*` refresh broadcasts collapse away.
//!
//! P0 is an empty three-pane shell: it draws the panes, cycles focus on F6/Tab,
//! and quits on `q` / `Alt+X` / `Ctrl+C`. The worker, structure, form, overlays
//! and orchestration are wired in later phases.

use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::ui::view;

/// Which of the three panes currently has focus.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pane {
    /// Pane 1 — the branch tree (DIT outline).
    Tree,
    /// Pane 2 — the leaf list + incremental search.
    Leaf,
    /// Pane 3 — the live edit form.
    Form,
}

/// The whole UI state. The event loop owns one of these and re-renders it every
/// frame. Grows as later phases wire in the tree, leaf list, form and overlays.
pub struct App {
    /// Which pane has focus.
    pub focus: Pane,
    /// Set to `true` to exit the event loop on the next tick.
    pub should_quit: bool,
}

impl App {
    /// A fresh app focused on the tree pane.
    pub fn new() -> Self {
        App {
            focus: Pane::Tree,
            should_quit: false,
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

/// Initialise the terminal, run the event loop, and restore the terminal on exit.
pub fn run() -> io::Result<()> {
    let mut terminal = ratatui::init();
    let mut app = App::new();
    let res = run_loop(&mut terminal, &mut app);
    ratatui::restore();
    res
}

/// The draw / poll loop. Uses a polled read (not a blocking `event::read`) so a
/// later phase can drain the LDAP worker every tick without the input read
/// starving it (plan §2.2).
fn run_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> io::Result<()> {
    loop {
        terminal.draw(|f| view::ui(f, app))?;

        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Release {
                    continue;
                }
                dispatch_key(app, key);
            }
        }

        if app.should_quit {
            return Ok(());
        }
    }
}

/// Translate a key into an `App` mutation. P0 handles only quit and focus cycle.
fn dispatch_key(app: &mut App, key: KeyEvent) {
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Char('x') | KeyCode::Char('X') if alt => app.should_quit = true,
        KeyCode::Char('c') | KeyCode::Char('C') if ctrl => app.should_quit = true,
        KeyCode::F(6) | KeyCode::Tab => app.focus = next_pane(app.focus),
        _ => {}
    }
}

/// The focus cycle order: Tree → Leaf → Form → Tree.
fn next_pane(focus: Pane) -> Pane {
    match focus {
        Pane::Tree => Pane::Leaf,
        Pane::Leaf => Pane::Form,
        Pane::Form => Pane::Tree,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_cycles_tree_leaf_form() {
        assert_eq!(next_pane(Pane::Tree), Pane::Leaf);
        assert_eq!(next_pane(Pane::Leaf), Pane::Form);
        assert_eq!(next_pane(Pane::Form), Pane::Tree);
    }
}
