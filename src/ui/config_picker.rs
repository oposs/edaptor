//! Full-screen ratatui picker shown when multiple configs are discovered.

use std::path::PathBuf;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::Alignment;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::config::discovery::ConfigCandidate;

/// Show a full-screen ratatui picker and return the selected path.
/// Returns `None` if the user presses `q` or `Esc` (caller should exit cleanly).
pub fn pick_config(candidates: Vec<ConfigCandidate>) -> Result<Option<PathBuf>> {
    let mut terminal = ratatui::init();
    let result = run_picker(&mut terminal, &candidates);
    ratatui::restore();
    result
}

fn run_picker(
    terminal: &mut ratatui::DefaultTerminal,
    candidates: &[ConfigCandidate],
) -> Result<Option<PathBuf>> {
    let mut selected = 0usize;
    loop {
        terminal.draw(|f| render(f, candidates, selected))?;
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Release {
                continue;
            }
            match key.code {
                KeyCode::Up => {
                    if selected == 0 {
                        selected = candidates.len() - 1;
                    } else {
                        selected -= 1;
                    }
                }
                KeyCode::Down => {
                    selected = (selected + 1) % candidates.len();
                }
                KeyCode::Enter => {
                    return Ok(Some(candidates[selected].path.clone()));
                }
                KeyCode::Char('q') | KeyCode::Esc => {
                    return Ok(None);
                }
                _ => {}
            }
        }
    }
}

fn render(f: &mut Frame, candidates: &[ConfigCandidate], selected: usize) {
    let area = f.area();

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Select configuration ")
        .title_bottom(
            Line::from(" ↑↓ navigate  Enter select  q quit ")
                .alignment(Alignment::Center),
        );
    let inner = block.inner(area);
    f.render_widget(block, area);

    let dim = Style::default().add_modifier(Modifier::DIM);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let selected_style = Style::default()
        .add_modifier(Modifier::BOLD)
        .add_modifier(Modifier::REVERSED);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::raw(""));

    for (i, candidate) in candidates.iter().enumerate() {
        let is_selected = i == selected;
        let prefix = if is_selected { "▶ " } else { "  " };

        let name_style = if is_selected { selected_style } else { bold };
        let text_style = if is_selected { selected_style } else { Style::default() };
        let path_style = if is_selected { selected_style } else { dim };

        lines.push(Line::from(vec![
            Span::styled(prefix, name_style),
            Span::styled(candidate.display_name(), name_style),
        ]));

        let desc = candidate.meta.description.as_deref().unwrap_or("");
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(desc, text_style),
        ]));

        let path_str = candidate.path.to_string_lossy().to_string();
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(path_str, path_style),
        ]));

        lines.push(Line::raw(""));
    }

    f.render_widget(Paragraph::new(lines), inner);
}
