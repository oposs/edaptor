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
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;
use tui_prompts::State;
use tui_tree_widget::Tree;

use crate::ui::app::{App, Overlay, Pane};
use crate::ui::edit_form::{EditField, EditForm, ValueEditor};
use crate::ui::form::WidgetSpec;

/// The three-pane column split: branch tree | leaf list | edit form.
const COLUMNS: [Constraint; 3] = [
    Constraint::Percentage(26),
    Constraint::Percentage(28),
    Constraint::Percentage(46),
];

/// Width of the label column in the form pane.
const LABEL_WIDTH: u16 = 18;

/// The bottom status-line text for `app` (pure, no ratatui types — unit-tested).
///
/// Layout (plan §3.5 / the old `build_status_line` logic):
/// - Always start with the key hints: `Alt+X Quit`, plus `F2 Save  F3 Cancel`
///   when NOT read-only (read-only mode suppresses every write affordance).
/// - In read-only mode, prepend a `[read-only]` tag so the state is visible.
/// - When a form is loaded, append the current DN, with a trailing ` *` dirty
///   marker when the form has unsaved edits.
/// - A non-empty transient `app.status` (e.g. "Saved." / "Created.") takes the
///   place of the key hints (it is the more useful thing to surface), but the DN
///   and dirty marker are still appended.
pub fn status_line(app: &App) -> String {
    let mut head = if app.status.is_empty() {
        let mut hints = String::from("Alt+X Quit");
        if !app.read_only {
            hints.push_str("  F2 Save  F3 Cancel");
        }
        hints
    } else {
        app.status.clone()
    };
    if app.read_only {
        head = format!("[read-only]  {head}");
    }
    if let Some(form) = app.form.as_ref() {
        head.push_str("  │  ");
        head.push_str(&form.dn);
        if form.is_dirty() {
            head.push_str(" *");
        }
    }
    head
}

/// The top menu-bar text for `app`: each menu entry's label with its trigger key
/// shown, derived from `app.menu_defs`. Profile-create entries get `[Alt+N]`,
/// Delete gets `[F8]`, Refresh gets `[F5]`, Quit gets `[Alt+X]` — matching the
/// keys actually wired in `dispatch_key`. Pure (no ratatui types).
pub fn menu_bar(app: &App) -> String {
    use crate::app::{CM_DELETE, CM_PROFILE_BASE, CM_QUIT, CM_REFRESH};
    let mut parts: Vec<String> = Vec::new();
    let mut profile_n = 0u16;
    for def in &app.menu_defs {
        let key = if def.command == CM_DELETE {
            "F8".to_string()
        } else if def.command == CM_REFRESH {
            "F5".to_string()
        } else if def.command == CM_QUIT {
            "Alt+X".to_string()
        } else if def.command >= CM_PROFILE_BASE {
            profile_n += 1;
            format!("Alt+{profile_n}")
        } else {
            continue;
        };
        parts.push(format!("[{key}] {}", def.label));
    }
    parts.join("  ")
}

/// Render the whole frame from `app`: a 1-row menu bar on top, the 3-column pane
/// area in the middle, and a 1-row status line at the bottom. Overlays still
/// render over the WHOLE frame (`render_overlay` uses `f.area()`).
pub fn ui(f: &mut Frame, app: &mut App) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // menu bar
        Constraint::Min(0),    // pane area
        Constraint::Length(1), // status line
    ])
    .split(f.area());

    render_menu_bar(f, app, chunks[0]);

    let cols = Layout::horizontal(COLUMNS).split(chunks[1]);
    render_tree(f, app, cols[0]);
    render_leaf(f, app, cols[1]);
    render_form(f, app, cols[2]);

    render_status_line(f, app, chunks[2]);

    if app.overlay.is_some() {
        render_overlay(f, app);
    }
}

/// Render the top menu bar (the profile/Delete/Refresh/Quit labels with their
/// trigger keys) onto `area`.
fn render_menu_bar(f: &mut Frame, app: &App, area: Rect) {
    f.render_widget(
        Paragraph::new(menu_bar(app)).style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        area,
    );
}

/// Render the bottom status line (key hints / transient status + DN + dirty
/// marker) onto `area`.
fn render_status_line(f: &mut Frame, app: &App, area: Rect) {
    f.render_widget(
        Paragraph::new(status_line(app)).style(Style::default().bg(Color::Blue).fg(Color::White)),
        area,
    );
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

/// The display string for a field:
/// - secret → a run of `•` (never the cleartext), tracking the live editor when
///   editable so masking length follows typing;
/// - multi  → `‹N set|ordered› v1; v2; …`;
/// - checkbox/binary → the widget rendering (`[x]` / `<N bytes>`);
/// - editable single → the live editor value (so typing is visible);
/// - read-only single → the stored value.
fn field_display_value(fld: &EditField) -> String {
    if fld.secret {
        return "•".repeat(secret_len(fld));
    }
    if fld.multi {
        let n = fld.values.len();
        let tag = if fld.ordered { "ordered" } else { "set" };
        return format!("‹{n} {tag}› {}", fld.values.join("; "));
    }
    match &fld.widget {
        WidgetSpec::DisabledCheckBox(b) => (if *b { "[x]" } else { "[ ]" }).to_string(),
        WidgetSpec::BinaryNote(bytes) => format!("<{bytes} bytes>"),
        _ if fld.editable => fld.editor.value().to_string(),
        _ => fld.values.first().cloned().unwrap_or_default(),
    }
}

/// The number of `•` to render for a secret field: the live editor length for an
/// editable single-value field, else the stored values' total length.
fn secret_len(fld: &EditField) -> usize {
    if fld.editable && !fld.multi {
        fld.editor.value().chars().count()
    } else {
        fld.values.iter().map(|v| v.chars().count()).sum()
    }
}

/// Draw a centered modal overlay (a `Clear` + bordered `Block`) over the panes.
/// Confirm shows the body (e.g. the LDIF preview) with a Yes/No hint; Error
/// shows the message; ValueEditor is the multi-value popup. Keys are captured by
/// `app::overlay_key` while one is open.
fn render_overlay(f: &mut Frame, app: &App) {
    let area = f.area();
    let (title, body, hint, border) = match app.overlay.as_ref() {
        Some(Overlay::Confirm { title, body, .. }) => {
            (title.clone(), body.clone(), " [Y]es   [N]o ", Color::Yellow)
        }
        Some(Overlay::Error { text }) => (
            "Error".to_string(),
            text.clone(),
            " press any key ",
            Color::Red,
        ),
        Some(Overlay::Guard { .. }) => (
            "Unsaved changes".to_string(),
            "This entry has unsaved edits.\nSave them before moving on?".to_string(),
            " [S]ave   [D]iscard   [C]ancel ",
            Color::Yellow,
        ),
        Some(Overlay::ValueEditor(ve)) => {
            render_value_editor(f, ve, area);
            return;
        }
        Some(Overlay::CreateForm { form, focus, .. }) => {
            render_create_form(f, form, *focus, area);
            return;
        }
        None => return,
    };

    let body_lines = body.lines().count().max(1) as u16;
    let w = 76.min(area.width.saturating_sub(4)).max(20);
    let h = (body_lines + 4)
        .clamp(7, area.height.saturating_sub(2).max(7))
        .min(area.height.saturating_sub(2).max(7));
    let rect = centered(w, h, area);

    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} "))
        .title_bottom(hint)
        .style(Style::default().bg(Color::Rgb(30, 30, 40)).fg(Color::White))
        .border_style(Style::default().fg(border).add_modifier(Modifier::BOLD));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    f.render_widget(
        Paragraph::new(body)
            .wrap(Wrap { trim: false })
            .style(Style::default().bg(Color::Rgb(30, 30, 40)).fg(Color::White)),
        inner,
    );
}

/// Draw the multi-value popup editor: one inline row per value with a selection
/// marker, the ordered/set hint in the title, and secret rows masked. (Spike
/// `render_popup`; values rendered via `Paragraph`, never byte-sliced.)
fn render_value_editor(f: &mut Frame, ve: &ValueEditor, area: Rect) {
    let avail = area.height.saturating_sub(2).max(7);
    let h = (ve.rows.len() as u16 + 5).clamp(7, avail).min(avail);
    let w = 64.min(area.width.saturating_sub(4)).max(20);
    let rect = centered(w, h, area);
    f.render_widget(Clear, rect);

    let kind = if ve.ordered {
        "ordered — reorder matters"
    } else {
        "set — reorder cosmetic"
    };
    let bg = Color::Rgb(30, 30, 40);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Edit {} ({kind}) ", ve.label))
        .title_bottom(" Alt+↑↓ move  Alt+a add  Alt+d del  F2 save  Esc cancel ")
        .style(Style::default().bg(bg).fg(Color::White))
        .border_style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        );
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    for (i, row) in ve.rows.iter().enumerate() {
        if i as u16 >= inner.height {
            break;
        }
        let y = inner.y + i as u16;
        let selected = i == ve.sel;
        let marker = format!("{:>2} {} ", i + 1, if selected { "▶" } else { " " });
        f.render_widget(
            Paragraph::new(marker).style(Style::default().bg(bg).fg(if selected {
                Color::Yellow
            } else {
                Color::DarkGray
            })),
            Rect::new(inner.x, y, 5.min(inner.width), 1),
        );
        let vr = Rect::new(inner.x + 5, y, inner.width.saturating_sub(5), 1);
        // Secret multi-values are masked in the popup too (never the cleartext).
        let display = if ve.secret {
            "•".repeat(row.value().chars().count())
        } else {
            row.value().to_string()
        };
        let rstyle = if selected {
            Style::default().bg(Color::Rgb(60, 60, 80)).fg(Color::White)
        } else {
            Style::default().bg(bg).fg(Color::Gray)
        };
        f.render_widget(Paragraph::new(display).style(rstyle), vr);
        if selected {
            let col = (row.position() as u16).min(vr.width.saturating_sub(1));
            f.set_cursor_position((vr.x + col, y));
        }
    }
}

/// Draw the create-entry overlay: the editable form for a new entry, hosted in a
/// centered modal (reuses [`field_display_value`] and the pane-3 row layout). The
/// focused, editable single-value field carries the cursor. (P4-T2.)
fn render_create_form(f: &mut Frame, form: &EditForm, focus: usize, area: Rect) {
    let n = form.fields.len();
    let avail = area.height.saturating_sub(2).max(7);
    let h = (n as u16 + 4).clamp(7, avail).min(avail);
    let w = 72.min(area.width.saturating_sub(4)).max(20);
    let rect = centered(w, h, area);
    f.render_widget(Clear, rect);

    let bg = Color::Rgb(30, 30, 40);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", form.dn))
        .title_bottom(" ↑↓ field   F2 create   Esc cancel ")
        .style(Style::default().bg(bg).fg(Color::White))
        .border_style(
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        );
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    if inner.height == 0 {
        return;
    }

    let label_w = LABEL_WIDTH.min(inner.width);
    let viewport = inner.height as usize;
    let scroll = clamp_scroll(focus, 0, viewport, n);
    for (row, idx) in (scroll..n).enumerate() {
        if row >= viewport {
            break;
        }
        let y = inner.y + row as u16;
        let fld = &form.fields[idx];
        let is_focused = idx == focus;

        let label_style = if is_focused {
            Style::default()
                .bg(bg)
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().bg(bg).fg(Color::Gray)
        };
        let star = if fld.must { "*" } else { " " };
        f.render_widget(
            Paragraph::new(format!("{star}{}", fld.label)).style(label_style),
            Rect::new(inner.x, y, label_w, 1),
        );

        let val_rect = Rect::new(inner.x + label_w, y, inner.width.saturating_sub(label_w), 1);
        f.render_widget(
            Paragraph::new(field_display_value(fld))
                .style(Style::default().bg(bg).fg(Color::White)),
            val_rect,
        );
        if is_focused && fld.editable && !fld.multi {
            let col = (fld.editor.position() as u16).min(val_rect.width.saturating_sub(1));
            f.set_cursor_position((val_rect.x + col, y));
        }
    }
}

/// Center a `w`×`h` rect within `area` (clamped to fit). (Spike `centered`,
/// re-expressing the facade's `center_origin` math.)
pub fn centered(w: u16, h: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Rect::new(x, y, w.min(area.width), h.min(area.height))
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
    use super::{centered, clamp_scroll, field_display_value, menu_bar, render_form, status_line};
    use crate::app::{build_menu_defs, MenuDef, CM_PROFILE_BASE};
    use crate::config::EntryProfile;
    use crate::schema::FieldKind;
    use crate::ui::app::{App, Pane};
    use crate::ui::edit_form::{EditField, EditForm};
    use crate::ui::form::WidgetSpec;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::Terminal;
    use tui_prompts::TextState;
    use tui_tree_widget::TreeState;

    /// Build a secret single-value text field carrying `value`.
    fn secret_field(value: &str) -> EditField {
        EditField {
            label: "userPassword".to_string(),
            must: false,
            editable: true,
            multi: false,
            secret: true,
            ordered: false,
            values: vec![value.to_string()],
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            editor: TextState::new().with_value(value.to_string()),
            relation: None,
        }
    }

    #[test]
    fn secret_field_renders_masked_never_cleartext() {
        let field = secret_field("topSecret-Paßwort");
        let shown = field_display_value(&field);
        assert!(
            !shown.contains("topSecret"),
            "must not leak cleartext: {shown:?}"
        );
        assert!(shown.chars().all(|c| c == '•'));
        // Mask length tracks the live editor (grapheme count, not bytes).
        assert_eq!(shown.chars().count(), "topSecret-Paßwort".chars().count());
    }

    #[test]
    fn centered_centers_and_clamps_to_area() {
        let area = Rect::new(0, 0, 100, 40);
        let r = centered(40, 10, area);
        assert_eq!((r.x, r.y, r.width, r.height), (30, 15, 40, 10));
        // Oversized requests clamp to the area.
        let big = centered(200, 100, area);
        assert_eq!((big.width, big.height), (100, 40));
    }

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
            relation: None,
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
                baseline: Default::default(),
            }),
            form_focus: 0,
            form_scroll: 0,
            overlay: None,
            status: String::new(),
            menu_defs: vec![],
            relations: vec![],
            picker_search_id: None,
            picker_last_query: String::new(),
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

    /// A bare App with no form, the given read-only flag, and an optional
    /// transient status, for status-line tests.
    fn status_app(read_only: bool, status: &str) -> App {
        App {
            focus: Pane::Tree,
            should_quit: false,
            read_only,
            tree_state: TreeState::default(),
            tree_items: vec![],
            current_branch: String::new(),
            last_search: String::new(),
            rows: vec![],
            leaf_sel: 0,
            search: TextState::new(),
            last_seen_leaf: None,
            form: None,
            form_focus: 0,
            form_scroll: 0,
            overlay: None,
            status: status.to_string(),
            menu_defs: vec![],
            relations: vec![],
            picker_search_id: None,
            picker_last_query: String::new(),
        }
    }

    /// Install a one-field `cn` form carrying `dn`; `dirty` controls whether the
    /// editor value diverges from the baseline (so `is_dirty()` is deterministic).
    fn with_cn_form(mut app: App, dn: &str, dirty: bool) -> App {
        use std::collections::BTreeMap;
        let mut baseline: BTreeMap<String, Vec<String>> = BTreeMap::new();
        baseline.insert("cn".to_string(), vec!["original".to_string()]);
        let value = if dirty { "edited" } else { "original" };
        app.form = Some(EditForm {
            dn: dn.to_string(),
            fields: vec![EditField {
                label: "cn".to_string(),
                must: true,
                editable: true,
                multi: false,
                secret: false,
                ordered: false,
                values: vec![value.to_string()],
                kind: FieldKind::Text,
                widget: WidgetSpec::ReadOnlyText,
                editor: TextState::new().with_value(value.to_string()),
                relation: None,
            }],
            baseline,
        });
        app
    }

    #[test]
    fn status_line_writable_shows_save_cancel_hints() {
        let s = status_line(&status_app(false, ""));
        assert!(s.contains("Alt+X Quit"));
        assert!(s.contains("F2 Save"));
        assert!(s.contains("F3 Cancel"));
        assert!(!s.contains("[read-only]"));
    }

    #[test]
    fn status_line_read_only_tags_and_suppresses_save_cancel() {
        let s = status_line(&status_app(true, ""));
        assert!(s.contains("[read-only]"));
        assert!(s.contains("Alt+X Quit"));
        assert!(!s.contains("F2 Save"));
        assert!(!s.contains("F3 Cancel"));
    }

    #[test]
    fn status_line_appends_dn_without_dirty_marker() {
        let app = with_cn_form(status_app(false, ""), "cn=Alice,dc=example,dc=org", false);
        let s = status_line(&app);
        assert!(s.contains("cn=Alice,dc=example,dc=org"));
        assert!(!s.trim_end().ends_with('*'));
    }

    #[test]
    fn status_line_appends_dirty_marker_when_form_edited() {
        let app = with_cn_form(status_app(false, ""), "cn=Alice,dc=example,dc=org", true);
        let s = status_line(&app);
        assert!(s.contains("cn=Alice,dc=example,dc=org"));
        assert!(s.trim_end().ends_with('*'));
    }

    #[test]
    fn status_line_surfaces_transient_status_with_dn() {
        let app = with_cn_form(
            status_app(false, "Saved."),
            "cn=Bob,dc=example,dc=org",
            false,
        );
        let s = status_line(&app);
        assert!(s.contains("Saved."));
        // The transient status replaces the key hints.
        assert!(!s.contains("F2 Save"));
        // …but the DN is still appended.
        assert!(s.contains("cn=Bob,dc=example,dc=org"));
    }

    #[test]
    fn menu_bar_shows_labels_with_trigger_keys() {
        let profiles = vec![
            EntryProfile {
                name: "Users".to_string(),
                object_class: "inetOrgPerson".to_string(),
                ..Default::default()
            },
            EntryProfile {
                name: "Groups".to_string(),
                object_class: "groupOfNames".to_string(),
                ..Default::default()
            },
        ];
        let mut app = status_app(false, "");
        app.menu_defs = build_menu_defs(&profiles);
        let bar = menu_bar(&app);
        // Profiles get Alt+1 / Alt+2 in config order; Delete/Refresh/Quit keep
        // their wired keys.
        assert!(bar.contains("[Alt+1] Users"));
        assert!(bar.contains("[Alt+2] Groups"));
        assert!(bar.contains("[F8] Delete"));
        assert!(bar.contains("[F5] Refresh"));
        assert!(bar.contains("[Alt+X] Quit"));
    }

    #[test]
    fn menu_bar_handles_no_profiles() {
        let mut app = status_app(false, "");
        // No profiles: build_menu_defs(&[]) still yields Delete/Refresh/Quit.
        app.menu_defs = build_menu_defs(&[]);
        let bar = menu_bar(&app);
        assert!(!bar.contains("Alt+1"));
        assert!(bar.contains("[F8] Delete"));
        assert!(bar.contains("[Alt+X] Quit"));
    }

    #[test]
    fn menu_bar_unknown_command_is_skipped() {
        // A stray command id (not a known CM_*) is silently dropped.
        let mut app = status_app(false, "");
        app.menu_defs = vec![
            MenuDef {
                label: "Mystery".to_string(),
                command: 42,
            },
            MenuDef {
                label: "Users".to_string(),
                command: CM_PROFILE_BASE,
            },
        ];
        let bar = menu_bar(&app);
        assert!(!bar.contains("Mystery"));
        assert!(bar.contains("[Alt+1] Users"));
    }
}
