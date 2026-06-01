//! Pure rendering for the ratatui UI.
//!
//! Mirrors the proven spike's render functions. Each pane owns its background so
//! the active pane is a solid light fill (the focus highlight the old
//! turbo-vision palette chain could not do cleanly). Values are rendered through
//! `Paragraph`, which grapheme-clips correctly — there is NO byte-slicing of
//! values anywhere here, which is the whole reason for leaving turbo-vision (its
//! `InputLine` byte-sliced UTF-8 and panicked on an umlaut straddling the cut).

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use tui_prompts::State;
use tui_tree_widget::Tree;

use crate::ui::app::{App, Pane};
use crate::ui::edit_form::EditField;
use crate::ui::form::WidgetSpec;

/// The three-pane column split: branch tree | leaf list | edit form.
const COLUMNS: [Constraint; 3] = [
    Constraint::Percentage(26),
    Constraint::Percentage(28),
    Constraint::Percentage(46),
];

/// Width of the label column in the form pane.
const LABEL_WIDTH: u16 = 18;

/// Render the whole frame from `app`.
pub fn ui(f: &mut Frame, app: &mut App) {
    let cols = Layout::horizontal(COLUMNS).split(f.area());
    render_tree(f, app, cols[0]);
    render_leaf(f, app, cols[1]);
    render_form(f, app, cols[2]);
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

/// Pane 1 — the branch tree (DIT outline). Stateful: selection lives in
/// `app.tree_state`; `reconcile` reads it to switch the leaf pane's branch.
fn render_tree(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Pane::Tree;
    let tree = Tree::new(&app.tree_items)
        .expect("tree item ids are unique DNs")
        .block(pane_block("DIT", focused))
        .highlight_style(Style::default().bg(Color::Blue).fg(Color::White));
    f.render_stateful_widget(tree, area, &mut app.tree_state);
}

/// Pane 2 — the incremental-search box over the leaf list. The selected row is
/// highlighted; the list scrolls so the selection stays visible.
fn render_leaf(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Pane::Leaf;
    let block = pane_block("Entries", focused);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 {
        return;
    }
    let base = pane_style(focused);

    // Search line at the top.
    let search_line = format!("/ {}", app.search.value());
    f.render_widget(
        Paragraph::new(search_line).style(base.fg(if focused {
            Color::Blue
        } else {
            Color::DarkGray
        })),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    // Leaf rows below the search line, scrolled to keep the selection visible.
    let list_y = inner.y + 1;
    let list_h = inner.height.saturating_sub(1) as usize;
    if list_h > 0 {
        let off = app.leaf_sel.saturating_sub(list_h.saturating_sub(1));
        for (row, (label, _dn)) in app.rows.iter().enumerate().skip(off).take(list_h) {
            let y = list_y + (row - off) as u16;
            let selected = row == app.leaf_sel;
            let style = if selected {
                Style::default().bg(Color::Blue).fg(Color::White)
            } else {
                base
            };
            f.render_widget(
                Paragraph::new(label.clone()).style(style),
                Rect::new(inner.x, y, inner.width, 1),
            );
        }
    }

    // The cursor sits in the search box while this pane is focused.
    if focused {
        let col = (app.search.position() as u16).min(inner.width.saturating_sub(3));
        f.set_cursor_position((inner.x + 2 + col, inner.y));
    }
}

/// Pane 3 — the live edit form. Renders one row per field within a manual
/// scroll viewport; the focused field's label is highlighted. P1 is read-only
/// (no cursor); P2 adds inline editing and the cursor.
fn render_form(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Pane::Form;
    let title = match &app.form {
        Some(form) => format!("Entry — {}", form.dn),
        None => "Entry".to_string(),
    };
    let block = pane_block(&title, focused);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    let n = match &app.form {
        Some(form) => form.fields.len(),
        None => return,
    };
    let viewport = inner.height as usize;
    // Two-directional scroll clamp so the focused field is always visible.
    app.form_scroll = clamp_scroll(app.form_focus, app.form_scroll, viewport, n);
    let scroll = app.form_scroll;

    let base = pane_style(focused);
    let form = app.form.as_ref().expect("checked above");
    let label_w = LABEL_WIDTH.min(inner.width);

    for (row, idx) in (scroll..n).enumerate() {
        if row >= viewport {
            break;
        }
        let y = inner.y + row as u16;
        let fld = &form.fields[idx];
        let is_focused_field = focused && idx == app.form_focus;

        // Label cell, with a `*` MUST marker.
        let label_style = if is_focused_field {
            base.fg(Color::Blue).add_modifier(Modifier::BOLD)
        } else {
            base
        };
        let star = if fld.must { "*" } else { " " };
        f.render_widget(
            Paragraph::new(format!("{star}{}", fld.label)).style(label_style),
            Rect::new(inner.x, y, label_w, 1),
        );

        // Value cell — rendered via Paragraph (grapheme-clipped, never sliced).
        let val_rect = Rect::new(inner.x + label_w, y, inner.width.saturating_sub(label_w), 1);
        let display = field_display_value(fld);
        let vstyle = if fld.multi {
            base.fg(if is_focused_field {
                Color::Magenta
            } else {
                Color::DarkGray
            })
        } else {
            base
        };
        f.render_widget(Paragraph::new(display).style(vstyle), val_rect);

        // Cursor for the focused, editable single-value field (P2+; read-only
        // mode and read-only kinds never get one).
        if is_focused_field && fld.editable && !fld.multi {
            let col = (fld.editor.position() as u16).min(val_rect.width.saturating_sub(1));
            f.set_cursor_position((val_rect.x + col, y));
        }
    }
}

/// The base text style for a pane, by focus (solid light when active, dim when
/// not). Shared by the leaf and form panes so their backgrounds match the block.
fn pane_style(focused: bool) -> Style {
    if focused {
        Style::default().bg(Color::White).fg(Color::Black)
    } else {
        Style::default().bg(Color::Black).fg(Color::Gray)
    }
}

/// The read-only display string for a field:
/// - secret → a run of `•` (never the cleartext);
/// - multi  → `‹N set|ordered› v1; v2; …`;
/// - checkbox/binary → the widget rendering (`[x]` / `<N bytes>`);
/// - otherwise the single value.
fn field_display_value(fld: &EditField) -> String {
    if fld.secret {
        let n: usize = fld.values.iter().map(|v| v.chars().count()).sum();
        return "•".repeat(n);
    }
    if fld.multi {
        let n = fld.values.len();
        let tag = if fld.ordered { "ordered" } else { "set" };
        return format!("‹{n} {tag}› {}", fld.values.join("; "));
    }
    match &fld.widget {
        WidgetSpec::DisabledCheckBox(b) => (if *b { "[x]" } else { "[ ]" }).to_string(),
        WidgetSpec::BinaryNote(bytes) => format!("<{bytes} bytes>"),
        _ => fld.values.first().cloned().unwrap_or_default(),
    }
}

/// Clamp `scroll` so the focused row stays within the `viewport` rows shown,
/// in BOTH directions (the spike's `ensure_visible` only handled scroll-up).
/// Pure and unit-tested.
pub fn clamp_scroll(focus: usize, scroll: usize, viewport: usize, n: usize) -> usize {
    if viewport == 0 || n == 0 {
        return 0;
    }
    let mut s = scroll.min(n.saturating_sub(1));
    if focus < s {
        s = focus;
    } else if focus >= s + viewport {
        s = focus + 1 - viewport;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::{clamp_scroll, render_form};
    use crate::schema::FieldKind;
    use crate::ui::app::{App, Pane};
    use crate::ui::edit_form::{EditField, EditForm};
    use crate::ui::form::WidgetSpec;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::Terminal;
    use tui_prompts::TextState;
    use tui_tree_widget::TreeState;

    /// Build a one-field form whose single text value is `value`.
    fn app_with_value(value: &str) -> App {
        let field = EditField {
            label: "description".to_string(),
            must: false,
            editable: false,
            multi: false,
            secret: false,
            ordered: false,
            values: vec![value.to_string()],
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            editor: TextState::new(),
        };
        App {
            focus: Pane::Form,
            should_quit: false,
            read_only: false,
            tree_state: TreeState::default(),
            tree_items: vec![],
            current_branch: String::new(),
            last_search: String::new(),
            rows: vec![],
            leaf_sel: 0,
            search: TextState::new(),
            last_seen_leaf: None,
            form: Some(EditForm {
                dn: "cn=test".to_string(),
                fields: vec![field],
            }),
            form_focus: 0,
            form_scroll: 0,
            status: String::new(),
        }
    }

    /// Collect the visible text of buffer row `y` over `[x0, x0+w)` as a String.
    fn row_text(buffer: &ratatui::buffer::Buffer, x0: u16, y: u16, w: u16) -> String {
        let mut s = String::new();
        for x in x0..x0 + w {
            s.push_str(buffer[(x, y)].symbol());
        }
        s
    }

    /// Render the form pane with a German (umlaut/ß) value WIDER than its value
    /// cell, at several narrow widths, and prove ratatui's `Paragraph` clips on a
    /// grapheme boundary instead of panicking.
    ///
    /// This re-bears the lesson of the deleted `tests/utf8_inputline_repro.rs`:
    /// the old turbo-vision `InputLine` did `&text[start..end]` BYTE slicing and
    /// panicked when a multibyte UTF-8 char (ö ü ä ß, the em dash —) straddled
    /// the cut. The migration's whole reason-for-being is that this no longer
    /// happens. The value cell here (inner width minus the 18-col label) is only a
    /// few columns wide, so the truncation point lands in the middle of a
    /// multibyte char at several of these widths.
    #[test]
    fn umlaut_value_wider_than_cell_does_not_panic() {
        const VALUE: &str = "Jörg Müller — Geschäftsführer Königsallee Düsseldorf Paßwort äöüß";

        // 1) A single deliberately narrow render: value cell only a few cols wide.
        let mut app = app_with_value(VALUE);
        let backend = TestBackend::new(26, 6);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_form(f, &mut app, f.area()))
            .expect("render must not panic on a multibyte boundary cut");

        let buffer = terminal.backend().buffer();
        // The block border eats 1 col/row; the label cell is min(18, inner.width)
        // wide and starts at inner.x. With width 26 the inner width is 24, the
        // label cell is 18, and the value cell starts at inner.x + 18.
        let inner_x = 1; // left border
        let label_w = super::LABEL_WIDTH.min(24);
        let label_row = row_text(buffer, inner_x, 1, label_w);
        assert!(
            label_row.contains("description"),
            "label should render; got {label_row:?}"
        );

        // The value cell must hold a valid grapheme prefix of VALUE: no U+FFFD
        // replacement char, and the trimmed visible chars must all be a char
        // prefix of the original.
        let val_x = inner_x + label_w;
        let val_w = 24u16.saturating_sub(label_w);
        let val_row = row_text(buffer, val_x, 1, val_w);
        assert!(
            !val_row.contains('\u{FFFD}'),
            "no replacement char in value cell; got {val_row:?}"
        );
        let trimmed = val_row.trim_end();
        assert!(
            VALUE.starts_with(trimmed),
            "value cell must be a grapheme prefix of the original; got {trimmed:?}"
        );

        // 2) Sweep narrow widths so the multibyte boundary is crossed at several
        //    points — each must render without panicking.
        for w in 20u16..30 {
            let mut app = app_with_value(VALUE);
            let backend = TestBackend::new(w, 6);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|f| {
                    // Use the full frame area; render_form does its own clamping.
                    render_form(f, &mut app, Rect::new(0, 0, w, 6));
                })
                .unwrap_or_else(|_| panic!("render panicked at width {w}"));
            let buffer = terminal.backend().buffer();
            // Whatever survived the clip is still valid UTF-8 (no U+FFFD).
            for y in 0..6 {
                let line = row_text(buffer, 0, y, w);
                assert!(
                    !line.contains('\u{FFFD}'),
                    "no replacement char at width {w}, row {y}; got {line:?}"
                );
            }
        }
    }

    #[test]
    fn scroll_unchanged_when_focus_visible() {
        assert_eq!(clamp_scroll(3, 0, 10, 20), 0);
        assert_eq!(clamp_scroll(5, 2, 10, 20), 2);
    }

    #[test]
    fn scrolls_down_when_focus_below_viewport() {
        // viewport shows rows [scroll, scroll+5); focus 7 needs scroll 3.
        assert_eq!(clamp_scroll(7, 0, 5, 20), 3);
        assert_eq!(clamp_scroll(19, 0, 5, 20), 15);
    }

    #[test]
    fn scrolls_up_when_focus_above_viewport() {
        assert_eq!(clamp_scroll(2, 8, 5, 20), 2);
    }

    #[test]
    fn handles_empty_and_zero_viewport() {
        assert_eq!(clamp_scroll(0, 0, 0, 20), 0);
        assert_eq!(clamp_scroll(0, 5, 10, 0), 0);
    }
}
