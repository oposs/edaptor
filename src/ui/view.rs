//! Pure rendering for the ratatui UI.
//!
//! Mirrors the proven spike's render functions. Each pane owns its background so
//! the active pane can be a solid light fill (the focus highlight the old
//! turbo-vision palette chain could not do cleanly). P0 renders empty panes; the
//! tree / leaf list / form / overlays are filled in by later phases.

use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders};
use ratatui::Frame;

use crate::ui::app::{App, Pane};

/// The three-pane column split: branch tree | leaf list | edit form.
const COLUMNS: [Constraint; 3] = [
    Constraint::Percentage(26),
    Constraint::Percentage(28),
    Constraint::Percentage(46),
];

/// Render the whole frame from `app`.
pub fn ui(f: &mut Frame, app: &App) {
    let cols = Layout::horizontal(COLUMNS).split(f.area());
    f.render_widget(pane_block("DIT", app.focus == Pane::Tree), cols[0]);
    f.render_widget(pane_block("Entries", app.focus == Pane::Leaf), cols[1]);
    f.render_widget(pane_block("Entry", app.focus == Pane::Form), cols[2]);
}

/// A bordered pane block. The focused pane gets a solid light background and a
/// bold yellow border so the active pane is obvious even on a mono terminal;
/// inactive panes are dim. (Spike `pane_block`.)
pub fn pane_block(title: &str, focused: bool) -> Block<'static> {
    let b = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} "));
    if focused {
        b.style(Style::default().bg(Color::White).fg(Color::Black))
            .border_style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )
    } else {
        b.style(Style::default().bg(Color::Black).fg(Color::Gray))
            .border_style(Style::default().fg(Color::DarkGray))
    }
}
