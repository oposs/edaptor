//! Pure rendering for the ratatui UI.
//!
//! Panes use the terminal's default colours (a light/white background); the
//! ACTIVE pane is marked by a bold **double** border, inactive panes by a dim
//! single border — no background inversion. The focused pane's hotkeys live in
//! the full-width status line (the narrow panes clip them in a border). Values
//! are rendered
//! through `Paragraph`, which grapheme-clips correctly — there is NO byte-slicing
//! of values anywhere here, which is the whole reason for leaving turbo-vision
//! (its `InputLine` byte-sliced UTF-8 and panicked on an umlaut at the cut).

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;
use tui_prompts::State;
use tui_tree_widget::Tree;

use crate::ui::app::{App, Overlay, Pane};
use crate::ui::edit_form::{EditField, ValueEditor};
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
/// Shows STATE plus the focused pane's hotkeys, ordered so the fixed-width
/// affordances survive a no-wrap clip and the variable-length DN clips last:
/// - a `[read-only]` tag in read-only mode;
/// - the transient `app.status` (e.g. "Saved." / "Created.") when present;
/// - the focused pane's key hints (they live here, not in the pane borders,
///   because the narrow panes clip them — see [`pane_hints`]);
/// - `Alt+X Quit` so the global quit is discoverable anywhere;
/// - LAST, the current DN with a trailing ` *` dirty marker when a form is
///   loaded — placed last because a deep DN would otherwise push the hints and
///   quit off the right edge; the DN is also shown in the form pane title.
pub fn status_line(app: &App) -> String {
    let mut parts: Vec<String> = Vec::new();
    if app.read_only {
        parts.push("[read-only]".to_string());
    }
    if !app.status.is_empty() {
        parts.push(app.status.clone());
    }
    parts.push(pane_hints(app.focus, app.read_only).to_string());
    parts.push("Alt+X Quit".to_string());
    if let Some(form) = app.form.as_ref() {
        let dirty = if form.is_dirty() { " *" } else { "" };
        parts.push(format!("{}{dirty}", form.dn));
    }
    parts.join("   ·   ")
}

/// The focused pane's hotkey hints, shown in the (full-width) status line.
/// Read-only mode drops the write keys. Pure. The narrow Tree/Leaf panes can't
/// hold these in their bottom border without clipping mid-word, so the hints
/// live in the status line and follow focus.
fn pane_hints(pane: Pane, read_only: bool) -> &'static str {
    match (pane, read_only) {
        (Pane::Tree, _) => "↑↓ Move · ←→ Fold · Alt+R Refresh",
        (Pane::Leaf, false) => "↑↓ Select · Type to search · Alt+N New · Alt+D Del",
        (Pane::Leaf, true) => "↑↓ Select · Type to search",
        (Pane::Form, false) => "↑↓ Field · ↵ Edit · Alt+S Save · Alt+C Cancel",
        (Pane::Form, true) => "↑↓ Field",
    }
}

/// Render the whole frame from `app`: the 3-column pane area, then a 1-row status
/// line at the bottom. Overlays render over the WHOLE frame (`f.area()`).
pub fn ui(f: &mut Frame, app: &mut App) {
    let chunks = Layout::vertical([
        Constraint::Min(0),    // pane area
        Constraint::Length(1), // status line
    ])
    .split(f.area());

    let cols = Layout::horizontal(COLUMNS).split(chunks[0]);
    render_tree(f, app, cols[0]);
    render_leaf(f, app, cols[1]);
    render_form(f, app, cols[2]);

    render_status_line(f, app, chunks[1]);

    if app.overlay.is_some() {
        render_overlay(f, app);
    }
}

/// Render the bottom status line (state: read-only / status / DN / dirty / quit).
fn render_status_line(f: &mut Frame, app: &App, area: Rect) {
    f.render_widget(
        Paragraph::new(status_line(app)).style(Style::default().add_modifier(Modifier::DIM)),
        area,
    );
}

/// A bordered pane block on the terminal's default background. The focused pane
/// gets a bold **double** border; inactive panes get a dim single border. Key
/// hints live in the status line (see [`pane_hints`]), not the bottom border.
pub fn pane_block(title: &str, focused: bool) -> Block<'static> {
    let b = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} "));
    if focused {
        b.border_type(BorderType::Double).border_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        b.border_type(BorderType::Plain)
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
        Some(form) if form.is_new() => "New entry".to_string(),
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

/// The base text style for a pane's content: the terminal default. Focus is shown
/// by the pane border (double vs single), not by a background fill, so both the
/// active and inactive panes share the default background. Inactive content is
/// dimmed so the active pane reads as primary.
fn pane_style(focused: bool) -> Style {
    if focused {
        Style::default()
    } else {
        Style::default().add_modifier(Modifier::DIM)
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
        Some(Overlay::ChooseProfile { entries, sel }) => {
            let body = entries
                .iter()
                .enumerate()
                .map(|(i, (_, name))| {
                    if i == *sel {
                        format!("> {name}")
                    } else {
                        format!("  {name}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            (
                "New entry — choose a profile".to_string(),
                body,
                " ↑↓ Move · Enter Select · Esc Cancel ",
                Color::Cyan,
            )
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
        .border_type(BorderType::Double)
        .title(format!(" {title} "))
        .title_bottom(hint)
        .border_style(Style::default().fg(border).add_modifier(Modifier::BOLD));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    f.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), inner);
}

/// Draw the multi-value popup editor: one inline row per value with a selection
/// marker, the ordered/set hint in the title, and secret rows masked. (Spike
/// `render_popup`; values rendered via `Paragraph`, never byte-sliced.)
fn render_value_editor(f: &mut Frame, ve: &ValueEditor, area: Rect) {
    // Picker mode: searchable candidate list with always-visible selection.
    if let Some(picker) = &ve.picker {
        let rect = centered(70, 20, area);
        f.render_widget(Clear, rect);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Double)
            .title(format!(" {} ", ve.label))
            .title_bottom(match (ve.lookup.is_some(), picker.truncated) {
                // Single-select value-lookup picker: Alt+S commits the chosen value.
                (true, true) => {
                    " ↑↓ move · Alt+S select · Alt+C cancel · type to search · more match — narrow search "
                }
                (true, false) => " ↑↓ move · Alt+S select · Alt+C cancel · type to search ",
                // Membership multi-select picker: Alt+Space toggles, Alt+S commits the set.
                (false, true) => {
                    " ↑↓ move · Alt+Space toggle · Alt+S save · Alt+C cancel · type to search · more match — narrow search "
                }
                (false, false) => " ↑↓ move · Alt+Space toggle · Alt+S save · Alt+C cancel · type to search ",
            })
            .border_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            );
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        if inner.height == 0 {
            return;
        }
        // First row: search box.
        let search_text = format!("Search: {}", ve.search.value());
        f.render_widget(
            Paragraph::new(search_text).style(Style::default().fg(Color::Blue)),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
        // Show terminal cursor at the insertion point within the search box.
        let prefix_width = "Search: ".len() as u16;
        let col = (ve.search.position() as u16).min(inner.width.saturating_sub(prefix_width + 1));
        f.set_cursor_position((inner.x + prefix_width + col, inner.y));
        // Remaining rows: visible candidates.
        let rows = picker.visible();
        let list_area_y = inner.y + 1;
        let list_height = inner.height.saturating_sub(1);
        for (i, row) in rows.iter().enumerate() {
            if i as u16 >= list_height {
                break;
            }
            let y = list_area_y + i as u16;
            let selected_cursor = i == picker.cursor;
            let star = if row.saved { "*" } else { " " };
            let check = if row.selected { "[x]" } else { "[ ]" };
            let line = format!("{star}{check} {}", row.candidate.label);
            let style = if selected_cursor {
                Style::default()
                    .bg(Color::Blue)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else if row.selected {
                Style::default().fg(Color::Green)
            } else {
                Style::default()
            };
            f.render_widget(
                Paragraph::new(line).style(style),
                Rect::new(inner.x, y, inner.width, 1),
            );
        }
        return;
    }

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
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .title(format!(" Edit {} ({kind}) ", ve.label))
        .title_bottom(" Alt+↑↓ move  Alt+a add  Alt+d del  Alt+S save  Alt+C cancel ")
        .border_style(
            Style::default()
                .fg(Color::Cyan)
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
            Paragraph::new(marker).style(Style::default().fg(if selected {
                Color::Blue
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
            Style::default().bg(Color::Blue).fg(Color::White)
        } else {
            Style::default()
        };
        f.render_widget(Paragraph::new(display).style(rstyle), vr);
        if selected {
            let col = (row.position() as u16).min(vr.width.saturating_sub(1));
            f.set_cursor_position((vr.x + col, y));
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
    use super::{
        centered, clamp_scroll, field_display_value, render_form, render_value_editor, status_line,
    };
    use crate::schema::FieldKind;
    use crate::ui::app::{App, Pane};
    use crate::ui::edit_form::{EditField, EditForm, FormMode};
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
            lookup: None,
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

    /// A saved member renders with a leading `*` marker in the picker popup.
    fn picker_value_editor_with_saved() -> crate::ui::edit_form::ValueEditor {
        use crate::ui::picker::{Candidate, PickerState};
        let selected = vec![Candidate {
            dn: "uid=bob,ou=people,dc=example,dc=org".to_string(),
            label: "Bob Baker (bob)".to_string(),
            value: None,
        }];
        crate::ui::edit_form::ValueEditor {
            field: 0,
            label: "member".to_string(),
            ordered: false,
            secret: false,
            rows: Vec::new(),
            sel: 0,
            picker: Some(PickerState::new(selected)),
            search: TextState::new(),
            scope: None,
            role: None,
            lookup: None,
            base: String::new(),
        }
    }

    #[test]
    fn picker_marks_saved_row_with_star() {
        let ve = picker_value_editor_with_saved();
        let (w, h) = (70u16, 20u16);
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_value_editor(f, &ve, Rect::new(0, 0, w, h)))
            .expect("picker render must not panic");
        let buffer = terminal.backend().buffer();
        // The saved candidate row carries the leading `*[x]` marker somewhere.
        let mut found = false;
        for y in 0..h {
            let line = row_text(buffer, 0, y, w);
            if line.contains("*[x] Bob Baker (bob)") {
                found = true;
                break;
            }
        }
        assert!(found, "saved selected row must render `*[x] ...`");
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
            lookup: None,
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
                mode: FormMode::Edit,
            }),
            form_focus: 0,
            form_scroll: 0,
            overlay: None,
            status: String::new(),
            relations: vec![],
            picker_search_id: None,
            picker_last_query: String::new(),
        }
    }

    #[test]
    fn render_form_titles_a_create_mode_form_as_new_entry() {
        let mut app = app_with_value("");
        if let Some(form) = app.form.as_mut() {
            form.mode = FormMode::Create {
                profile_idx: 0,
                container: "ou=people,dc=example,dc=org".to_string(),
            };
        }
        let w = 40;
        let backend = TestBackend::new(w, 6);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_form(f, &mut app, Rect::new(0, 0, w, 6)))
            .expect("render must not panic");
        let top = row_text(terminal.backend().buffer(), 0, 0, w);
        assert!(
            top.contains("New entry"),
            "create form titled 'New entry', got: {top:?}"
        );
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
                lookup: None,
            }],
            baseline,
            mode: FormMode::Edit,
        });
        app
    }

    #[test]
    fn status_line_shows_quit_and_focused_pane_hints() {
        // Key hints live in the status line and follow focus. The helper focuses
        // the Tree pane, so its hints (Alt+R Refresh) show — not the Form's.
        let s = status_line(&status_app(false, ""));
        assert!(s.contains("Alt+X Quit"));
        assert!(s.contains("Alt+R Refresh"));
        assert!(!s.contains("Alt+S Save"));
        assert!(!s.contains("[read-only]"));
    }

    #[test]
    fn status_line_hints_follow_focus() {
        let mut app = with_cn_form(status_app(false, ""), "cn=Alice,dc=example,dc=org", false);
        app.focus = Pane::Form;
        let s = status_line(&app);
        assert!(s.contains("Alt+S Save"));
        assert!(!s.contains("Alt+R Refresh"));
    }

    #[test]
    fn status_line_tags_read_only() {
        let s = status_line(&status_app(true, ""));
        assert!(s.contains("[read-only]"));
        assert!(s.contains("Alt+X Quit"));
    }

    #[test]
    fn status_line_shows_dn_and_dirty_marker() {
        let clean = with_cn_form(status_app(false, ""), "cn=Alice,dc=example,dc=org", false);
        let s = status_line(&clean);
        assert!(s.contains("cn=Alice,dc=example,dc=org"));
        assert!(!s.contains("cn=Alice,dc=example,dc=org *"));

        let dirty = with_cn_form(status_app(false, ""), "cn=Alice,dc=example,dc=org", true);
        assert!(status_line(&dirty).contains("cn=Alice,dc=example,dc=org *"));
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
        assert!(s.contains("cn=Bob,dc=example,dc=org"));
    }

    #[test]
    fn pane_hints_drop_write_keys_in_read_only() {
        use super::pane_hints;
        // The Entries pane surfaces create/delete; read-only hides them.
        assert!(pane_hints(Pane::Leaf, false).contains("Alt+N New"));
        assert!(pane_hints(Pane::Leaf, false).contains("Alt+D Del"));
        assert!(!pane_hints(Pane::Leaf, true).contains("Alt+N New"));
        // The Form pane surfaces Save/Cancel; read-only hides them.
        assert!(pane_hints(Pane::Form, false).contains("Alt+S Save"));
        assert!(!pane_hints(Pane::Form, true).contains("Alt+S Save"));
        // Refresh is allowed in read-only.
        assert!(pane_hints(Pane::Tree, true).contains("Alt+R Refresh"));
    }
}
