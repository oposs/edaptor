//! ratatui application state and the event loop.
//!
//! This replaces the old turbo-vision facade + `Shell::run_loop` callback. The
//! loop is immediate-mode: it owns all state as plain data ([`App`]) and
//! re-renders every frame, so the shared `Rc<RefCell>` pane handles and the
//! `CM_*` refresh broadcasts collapse away.
//!
//! Borrow split (plan §2.1): the worker, read-flow and structure live as locals
//! in [`run`]; `App` holds only the UI-facing state. `terminal.draw`'s `&mut
//! App` borrow is scoped to the closure, so it never collides with the
//! orchestration borrows that follow it each tick.

use std::time::Duration;

use anyhow::{anyhow, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tui_prompts::{State, TextState};
use tui_tree_widget::{TreeItem, TreeState};

use crate::config::Config;
use crate::ldap::worker::{Request, Response, StructureNodeRaw, WorkerHandle};
use crate::schema::SchemaModel;
use crate::ui::edit_form::{build_edit_form, EditForm};
use crate::ui::view;
use crate::workflows::read_flow::{ReadFlow, ReadOutcome};
use crate::workflows::structure::{Structure, StructureInput};

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
/// frame. (Overlays, dirty/baseline tracking and write state arrive in P2+.)
pub struct App {
    /// Which pane has focus.
    pub focus: Pane,
    /// Set to `true` to exit the event loop on the next tick.
    pub should_quit: bool,
    /// Global read-only mode (no editing / writes); affects later phases.
    pub read_only: bool,

    // Pane 1 — branch tree.
    /// Selection / expansion state for the tree widget.
    pub tree_state: TreeState<String>,
    /// The tree items (built once from the eager [`Structure`]).
    pub tree_items: Vec<TreeItem<'static, String>>,

    // Pane 2 — leaf list + search.
    /// The branch whose leaves are listed in pane 2.
    pub current_branch: String,
    /// The last search string applied to `rows` (delta detection in `reconcile`).
    pub last_search: String,
    /// The pane-2 rows: `(label, dn)`.
    pub rows: Vec<(String, String)>,
    /// The highlighted row index in `rows`.
    pub leaf_sel: usize,
    /// The incremental-search edit state.
    pub search: TextState<'static>,
    /// The DN last navigated to (delta detection so a base-read fires once).
    pub last_seen_leaf: Option<String>,

    // Pane 3 — edit form.
    /// The form for the selected entry, or `None` when nothing is selected.
    pub form: Option<EditForm>,
    /// The focused field index within `form`.
    pub form_focus: usize,
    /// The first visible field index (manual scroll viewport).
    pub form_scroll: usize,

    /// Transient status / error text.
    pub status: String,
}

/// Spawn the worker, fetch the schema + eager structure, then run the TUI.
pub fn run(config: Config, password: String) -> Result<()> {
    let base_dn = config.server.base_dn.clone();
    let read_only = config.is_read_only();

    // Sync startup: spawn the worker, fetch the schema, scan the structure.
    let worker = WorkerHandle::spawn(config, password)?;
    let raw = match worker.request(Request::FetchSubschema)? {
        Response::Subschema(raw) => raw,
        Response::Error(e) => return Err(anyhow!(e)),
        _ => return Err(anyhow!("unexpected response to FetchSubschema")),
    };
    let schema = SchemaModel::from_raw(&raw);

    let nodes = match worker.request(Request::LoadStructure {
        id: 0,
        base: base_dn.clone(),
        page_size: 500,
    })? {
        Response::StructureEntries { nodes, .. } => nodes,
        Response::StructureError { msg, truncated, .. } => {
            eprintln!(
                "warning: structure scan failed ({msg}){}; browsing root only",
                if truncated { " — result truncated" } else { "" }
            );
            Vec::new()
        }
        Response::Error(e) => return Err(anyhow!(e)),
        _ => return Err(anyhow!("unexpected response to LoadStructure")),
    };
    let structure = Structure::build(&base_dn, structure_inputs(nodes));
    let mut read_flow = ReadFlow::new(schema);

    // Seed the UI state.
    let current_branch = structure.root_dn().to_string();
    let rows = compute_rows(&structure, &current_branch, "");
    let mut tree_state = TreeState::default();
    tree_state.open(vec![current_branch.clone()]);
    let mut app = App {
        focus: Pane::Tree,
        should_quit: false,
        read_only,
        tree_state,
        tree_items: build_tree_items(&structure),
        current_branch,
        last_search: String::new(),
        rows,
        leaf_sel: 0,
        search: TextState::new(),
        last_seen_leaf: None,
        form: None,
        form_focus: 0,
        form_scroll: 0,
        status: String::new(),
    };

    let mut terminal = ratatui::init();
    let res = event_loop(&mut terminal, &mut app, &worker, &mut read_flow, &structure);
    ratatui::restore();
    res
}

/// The draw / poll loop (plan §2.2). A polled read (not blocking) lets the
/// worker drain run every tick without input starving it.
fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    worker: &WorkerHandle,
    read_flow: &mut ReadFlow,
    structure: &Structure,
) -> Result<()> {
    loop {
        terminal.draw(|f| view::ui(f, app))?;

        // 1) Drain ALL pending worker responses (read-flow → form).
        while let Some(resp) = worker.poll() {
            handle_worker_response(app, &resp, read_flow);
        }

        // 2) Poll input with a timeout so the worker drain keeps ticking.
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Release {
                    dispatch_key(app, key);
                }
            }
        }

        // 3) Reconcile UI deltas: branch switch, search filter, leaf base-read.
        reconcile(app, structure, worker, read_flow);

        if app.should_quit {
            return Ok(());
        }
    }
}

/// Feed a polled worker [`Response`] to the read flow and, on a built form,
/// install it as the editable form (read-only-aware in P1).
fn handle_worker_response(app: &mut App, resp: &Response, read_flow: &mut ReadFlow) {
    match read_flow.on_response(resp) {
        ReadOutcome::Form { model, .. } => {
            // Rapid leaf navigation submits overlapping base-reads; the worker is
            // FIFO, so an older read can resolve first. Install only the response
            // whose DN matches the entry the user is currently on — otherwise a
            // stale entry would flash now (and, from P2, clobber edit state).
            let current = app
                .last_seen_leaf
                .as_deref()
                .map(|dn| dn.eq_ignore_ascii_case(&model.title))
                .unwrap_or(false);
            if current {
                app.form = Some(build_edit_form(&model, read_flow.schema(), app.read_only));
                app.form_focus = 0;
                app.form_scroll = 0;
                app.status.clear();
            }
        }
        ReadOutcome::Error(msg) => app.status = msg,
        ReadOutcome::Ignored => {}
    }
}

/// Translate a key into an `App` mutation, gated by the focused pane. Editing
/// (form text input, multi-value popup) arrives in P2/P3.
fn dispatch_key(app: &mut App, key: KeyEvent) {
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    // Global quit + focus cycle. Bare `q` quits only from the tree pane, where
    // there is no text entry to swallow it; the search/edit panes need the key.
    if (alt && matches!(key.code, KeyCode::Char('x') | KeyCode::Char('X')))
        || (ctrl && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C')))
    {
        app.should_quit = true;
        return;
    }
    if matches!(key.code, KeyCode::F(6) | KeyCode::Tab) {
        app.focus = next_pane(app.focus);
        return;
    }

    match app.focus {
        Pane::Tree => match key.code {
            KeyCode::Up => {
                app.tree_state.key_up();
            }
            KeyCode::Down => {
                app.tree_state.key_down();
            }
            KeyCode::Left => {
                app.tree_state.key_left();
            }
            KeyCode::Right => {
                app.tree_state.key_right();
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                app.tree_state.toggle_selected();
            }
            KeyCode::Char('q') => app.should_quit = true,
            _ => {}
        },
        Pane::Leaf => match key.code {
            KeyCode::Up => app.leaf_sel = app.leaf_sel.saturating_sub(1),
            KeyCode::Down => app.leaf_sel = next_index(app.leaf_sel, app.rows.len()),
            KeyCode::PageUp => app.leaf_sel = app.leaf_sel.saturating_sub(10),
            KeyCode::PageDown => {
                app.leaf_sel = (app.leaf_sel + 10).min(app.rows.len().saturating_sub(1))
            }
            // Everything else (text, backspace, …) edits the search box.
            _ => {
                app.search.handle_key_event(key);
            }
        },
        Pane::Form => {
            let n = app.form.as_ref().map(|f| f.fields.len()).unwrap_or(0);
            match key.code {
                KeyCode::Up => app.form_focus = app.form_focus.saturating_sub(1),
                KeyCode::Down => app.form_focus = next_index(app.form_focus, n),
                // Scroll follows focus: clamp_scroll (in render_form) is the sole
                // authority for form_scroll, so paging only moves the focus.
                KeyCode::PageUp => app.form_focus = app.form_focus.saturating_sub(10),
                KeyCode::PageDown => app.form_focus = (app.form_focus + 10).min(n.saturating_sub(1)),
                // Editing the focused field arrives in P2.
                _ => {}
            }
        }
    }
}

/// Reconcile UI deltas each tick: a tree-selection branch switch, a search
/// filter change, and a leaf-selection change (which fires a base-read whose
/// result fills the form). No dirty guard yet (that is P4).
fn reconcile(app: &mut App, structure: &Structure, worker: &WorkerHandle, read_flow: &mut ReadFlow) {
    let search = app.search.value().to_string();

    // 1) Tree selection changed → switch the leaf pane to that branch.
    if let Some(sel) = app.tree_state.selected().last().cloned() {
        if sel != app.current_branch && structure.get(&sel).is_some() {
            app.current_branch = sel;
            app.rows = compute_rows(structure, &app.current_branch, &search);
            app.leaf_sel = 0;
            app.last_seen_leaf = None;
        }
    }

    // 2) Search string changed → recompute the rows, keep the selection in range.
    if search != app.last_search {
        app.last_search = search.clone();
        app.rows = compute_rows(structure, &app.current_branch, &search);
        if app.leaf_sel >= app.rows.len() {
            app.leaf_sel = app.rows.len().saturating_sub(1);
        }
    }

    // 3) Selected leaf DN changed → base-read it into the form (or clear it).
    let sel_dn = app.rows.get(app.leaf_sel).map(|(_, dn)| dn.clone());
    if sel_dn != app.last_seen_leaf {
        app.last_seen_leaf = sel_dn.clone();
        match sel_dn {
            Some(dn) => {
                let _ = read_flow.request_entry(worker, &dn, None);
            }
            None => app.form = None,
        }
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

/// Next selectable index, clamped to `[0, len)` (saturating at the bottom).
fn next_index(cur: usize, len: usize) -> usize {
    (cur + 1).min(len.saturating_sub(1))
}

/// The pane-2 rows for `branch` filtered by `search`: a `‹self›` row for the
/// branch entry itself, then its leaf children `(label, dn)`. Pure.
fn compute_rows(structure: &Structure, branch: &str, search: &str) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    if let Some(node) = structure.get(branch) {
        rows.push((format!("‹self› {}", node.label), branch.to_string()));
    }
    for leaf in structure.filter_leaves(branch, search) {
        rows.push((leaf.label.clone(), leaf.dn.clone()));
    }
    rows
}

/// Map the worker's raw structure rows into the pure model's input rows. Pure.
fn structure_inputs(nodes: Vec<StructureNodeRaw>) -> Vec<StructureInput> {
    nodes
        .into_iter()
        .map(|n| StructureInput {
            dn: n.dn,
            cn: n.cn,
            description: n.description,
            object_classes: n.object_classes,
        })
        .collect()
}

/// Build the pane-1 tree items from the eager [`Structure`]. Only branch nodes
/// appear in the tree (leaves are listed in pane 2); the identifier is the DN so
/// `tree_state.selected()` yields the branch DN. (Port of the facade's
/// `build_structure_tree`.)
fn build_tree_items(structure: &Structure) -> Vec<TreeItem<'static, String>> {
    fn build(structure: &Structure, dn: &str) -> TreeItem<'static, String> {
        let label = structure.get(dn).map(|n| n.label.clone()).unwrap_or_else(|| {
            dn.split(',').next().unwrap_or(dn).trim().to_string()
        });
        let mut children = Vec::new();
        if let Some(n) = structure.get(dn) {
            for child_dn in &n.children {
                if structure.get(child_dn).map(|c| c.is_branch()).unwrap_or(false) {
                    children.push(build(structure, child_dn));
                }
            }
        }
        if children.is_empty() {
            TreeItem::new_leaf(dn.to_string(), label)
        } else {
            TreeItem::new(dn.to_string(), label, children).expect("DNs are unique ids")
        }
    }
    vec![build(structure, structure.root_dn())]
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

    #[test]
    fn next_index_clamps_at_end() {
        assert_eq!(next_index(0, 3), 1);
        assert_eq!(next_index(2, 3), 2);
        assert_eq!(next_index(0, 0), 0);
    }

    fn structure() -> Structure {
        Structure::build(
            "dc=example,dc=org",
            vec![
                StructureInput {
                    dn: "dc=example,dc=org".into(),
                    cn: None,
                    description: Some("Example".into()),
                    object_classes: vec![],
                },
                StructureInput {
                    dn: "ou=users,dc=example,dc=org".into(),
                    cn: None,
                    description: None,
                    object_classes: vec![],
                },
                StructureInput {
                    dn: "uid=jane,ou=users,dc=example,dc=org".into(),
                    cn: Some("Jane".into()),
                    description: None,
                    object_classes: vec![],
                },
            ],
        )
    }

    #[test]
    fn compute_rows_lists_self_then_leaves() {
        let s = structure();
        let rows = compute_rows(&s, "ou=users,dc=example,dc=org", "");
        assert_eq!(rows[0].0, "‹self› ou=users");
        assert_eq!(rows[1], ("Jane".to_string(), "uid=jane,ou=users,dc=example,dc=org".to_string()));
        // Search filters the leaves (the self row is always present).
        let filtered = compute_rows(&s, "ou=users,dc=example,dc=org", "zzz");
        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn tree_items_contain_only_branches() {
        let s = structure();
        let items = build_tree_items(&s);
        // Root has exactly one branch child (ou=users); uid=jane is a leaf.
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].children().len(), 1);
    }
}
