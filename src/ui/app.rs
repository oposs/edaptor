//! ratatui application state and the event loop.
//!
//! This replaces the old turbo-vision facade + `Shell::run_loop` callback. The
//! loop is immediate-mode: it owns all state as plain data ([`App`]) and
//! re-renders every frame, so the shared `Rc<RefCell>` pane handles and the
//! `CM_*` refresh broadcasts collapse away.
//!
//! Borrow split (plan §2.1): the worker, read-flow, structure and the write
//! tracking maps live as locals in [`run`]/[`event_loop`]; `App` holds only the
//! UI-facing state (including the modal `overlay`). `terminal.draw`'s `&mut App`
//! borrow is scoped to the closure, so it never collides with the orchestration
//! borrows that follow it each tick.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{anyhow, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tui_prompts::{State, TextState};
use tui_tree_widget::{TreeItem, TreeState};

use crate::app::UiAction;
use crate::config::Config;
use crate::form::changeset::{diff, EditEntry, ModOp};
use crate::form::validate::{plan_save, validate, SavePlan, ValidationError};
use crate::ldap::ldif::render_changeset;
use crate::ldap::worker::{Request, Response, StructureNodeRaw, WorkerHandle};
use crate::schema::SchemaModel;
use crate::ui::edit_form::{build_edit_form, EditForm, ValueEditor};
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

/// A modal overlay drawn on top of the panes; while one is open it captures all
/// keys (plan §3.4). More variants (ValueEditor, Guard, CreateForm) arrive in
/// later phases.
pub enum Overlay {
    /// A yes/no confirmation (e.g. the save LDIF preview) carrying the action to
    /// run on confirm.
    Confirm {
        /// Dialog title.
        title: String,
        /// Body text (e.g. the LDIF preview).
        body: String,
        /// What to do when the user confirms.
        action: PendingAction,
    },
    /// An error message; any key dismisses it.
    Error {
        /// The message to show.
        text: String,
    },
    /// The multi-value popup editor (Enter on a multi field).
    ValueEditor(ValueEditor),
}

/// What a confirmed [`Overlay::Confirm`] should do. Grows in P4 (create/delete).
pub enum PendingAction {
    /// Submit a prepared save plan against `dn`.
    Save {
        /// The save plan to submit on confirm.
        plan: SavePlan,
        /// The (old) DN the plan targets.
        dn: String,
    },
}

/// What the run-loop should do when a write's `WriteOk` arrives, keyed by id.
enum PostWrite {
    /// A form save (Modify / RenameOnly): re-read `reread_dn` into the form.
    Save {
        /// The DN to re-read once the write succeeds.
        reread_dn: String,
    },
}

/// The whole UI state. The event loop owns one of these and re-renders it every
/// frame.
pub struct App {
    /// Which pane has focus.
    pub focus: Pane,
    /// Set to `true` to exit the event loop on the next tick.
    pub should_quit: bool,
    /// Global read-only mode (no editing / writes).
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

    /// The open modal overlay, if any (captures keys while present).
    pub overlay: Option<Overlay>,
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
        overlay: None,
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
    // Write-tracking maps (orchestration locals, plan §2.1).
    let mut post: HashMap<u64, PostWrite> = HashMap::new();
    let mut pending_followups: HashMap<u64, (String, Vec<ModOp>)> = HashMap::new();

    loop {
        terminal.draw(|f| view::ui(f, app))?;

        // 1) Drain ALL pending worker responses (writes first, then read forms).
        while let Some(resp) = worker.poll() {
            handle_worker_response(app, &resp, worker, read_flow, &mut post, &mut pending_followups);
        }

        // 2) Poll input with a timeout so the worker drain keeps ticking. An open
        //    overlay captures every key (plan §3.4).
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Release {
                    if app.overlay.is_some() {
                        if let Some(action) = overlay_key(app, key) {
                            execute_pending(app, action, worker, &mut post, &mut pending_followups);
                        }
                    } else if let Some(action) = dispatch_key(app, key) {
                        handle_action(app, action, read_flow);
                    }
                }
            }
        }

        // 3) Reconcile UI deltas (no-op while an overlay holds the keys).
        if app.overlay.is_none() {
            reconcile(app, structure, worker, read_flow);
        }

        if app.should_quit {
            return Ok(());
        }
    }
}

/// Feed a polled worker [`Response`] to the write-tracking maps and the read
/// flow. Writes are handled first (re-read after a save); otherwise a built form
/// is installed (only when its DN matches the current selection — see below).
fn handle_worker_response(
    app: &mut App,
    resp: &Response,
    worker: &WorkerHandle,
    read_flow: &mut ReadFlow,
    post: &mut HashMap<u64, PostWrite>,
    pending_followups: &mut HashMap<u64, (String, Vec<ModOp>)>,
) {
    match resp {
        Response::WriteOk { id, .. } => {
            if let Some((new_dn, mods)) = pending_followups.remove(id) {
                // A rename's MODRDN succeeded: apply the deferred mods to the new
                // DN, then re-read it.
                let _ = worker.submit(Request::Modify {
                    id: next_id(),
                    dn: new_dn.clone(),
                    changes: mods,
                });
                rebind_selection(app, &new_dn);
                let _ = read_flow.request_entry(worker, &new_dn, None);
                return;
            }
            if let Some(PostWrite::Save { reread_dn }) = post.remove(id) {
                app.status = "Saved.".to_string();
                rebind_selection(app, &reread_dn);
                let _ = read_flow.request_entry(worker, &reread_dn, None);
            } else {
                app.status = "Saved.".to_string();
            }
        }
        Response::WriteError { msg, .. } => {
            app.overlay = Some(Overlay::Error { text: msg.clone() });
        }
        // on_response consumes the pending id, so call it exactly once.
        _ => match read_flow.on_response(resp) {
            ReadOutcome::Form { model, .. } => {
                // Rapid leaf navigation submits overlapping base-reads; the worker
                // is FIFO so an older read can resolve first. Install only the
                // response whose DN matches the entry the user is currently on,
                // else a stale entry would flash (and, from P2, clobber edits).
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
        },
    }
}

/// Translate a key into an `App` mutation (gated by the focused pane), returning
/// a [`UiAction`] for the few keys the loop must service with the worker.
fn dispatch_key(app: &mut App, key: KeyEvent) -> Option<UiAction> {
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    // Global quit + focus cycle. Bare `q` quits only from the tree pane, where
    // there is no text entry to swallow it; the search/edit panes need the key.
    if (alt && matches!(key.code, KeyCode::Char('x') | KeyCode::Char('X')))
        || (ctrl && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C')))
    {
        app.should_quit = true;
        return None;
    }
    if matches!(key.code, KeyCode::F(6) | KeyCode::Tab) {
        app.focus = next_pane(app.focus);
        return None;
    }
    // Save / Cancel (writable mode only).
    if !app.read_only {
        match key.code {
            KeyCode::F(2) => return Some(UiAction::FormSave),
            KeyCode::F(3) => return Some(UiAction::FormCancel),
            _ => {}
        }
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
                // Enter on an editable multi-value field opens the value-editor
                // popup; on a single field it is a no-op.
                KeyCode::Enter => open_value_editor(app),
                // Otherwise edit the focused single-value field inline.
                _ => edit_focused_field(app, key),
            }
        }
    }
    None
}

/// Route a key to the focused field's inline editor, if it is an editable
/// single-value field (multi-value fields are edited via the popup).
fn edit_focused_field(app: &mut App, key: KeyEvent) {
    let focus = app.form_focus;
    if let Some(form) = app.form.as_mut() {
        if let Some(field) = form.fields.get_mut(focus) {
            if field.editable && !field.multi {
                field.editor.handle_key_event(key);
            }
        }
    }
}

/// Open the multi-value popup over the focused field, if it is an editable
/// multi-valued field.
fn open_value_editor(app: &mut App) {
    let focus = app.form_focus;
    let editor = app.form.as_ref().and_then(|form| {
        form.fields
            .get(focus)
            .filter(|f| f.multi && f.editable)
            .map(|f| ValueEditor::open(focus, f))
    });
    if let Some(ve) = editor {
        app.overlay = Some(Overlay::ValueEditor(ve));
    }
}

/// Handle a key inside the multi-value popup (spike `popup_key`): nav (↑↓),
/// reorder (Alt+↑↓), insert (Alt+a / Insert), delete (Alt+d), commit (F2,
/// dropping empties), cancel (Esc / F3); any other key edits the selected row.
fn value_editor_key(app: &mut App, key: KeyEvent) {
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match (key.code, alt) {
        (KeyCode::Esc, _) | (KeyCode::F(3), _) => {
            app.overlay = None;
        }
        (KeyCode::F(2), _) => {
            // Commit: write the trimmed, non-empty values back into the field.
            if let Some(Overlay::ValueEditor(ve)) = app.overlay.take() {
                let values = ve.committed_values();
                if let Some(field) = app.form.as_mut().and_then(|f| f.fields.get_mut(ve.field)) {
                    field.values = values;
                }
            }
        }
        _ => {
            let Some(Overlay::ValueEditor(ve)) = app.overlay.as_mut() else {
                return;
            };
            // Invariant: sel is always a valid index (or 0 when empty). Every arm
            // below preserves it, but normalize defensively so the reorder swaps
            // can never index out of bounds regardless of edit history.
            ve.sel = ve.sel.min(ve.rows.len().saturating_sub(1));
            match (key.code, alt) {
                (KeyCode::Up, false) => ve.sel = ve.sel.saturating_sub(1),
                (KeyCode::Down, false) => ve.sel = (ve.sel + 1).min(ve.rows.len().saturating_sub(1)),
                (KeyCode::Up, true) => {
                    if ve.sel > 0 {
                        ve.rows.swap(ve.sel, ve.sel - 1);
                        ve.sel -= 1;
                    }
                }
                (KeyCode::Down, true) => {
                    if ve.sel + 1 < ve.rows.len() {
                        ve.rows.swap(ve.sel, ve.sel + 1);
                        ve.sel += 1;
                    }
                }
                (KeyCode::Char('a'), true) | (KeyCode::Insert, _) => {
                    let at = (ve.sel + 1).min(ve.rows.len());
                    ve.rows.insert(at, TextState::new());
                    ve.sel = at;
                }
                (KeyCode::Char('d'), true) => {
                    if !ve.rows.is_empty() {
                        ve.rows.remove(ve.sel);
                        ve.sel = ve.sel.min(ve.rows.len().saturating_sub(1));
                    }
                }
                // Any other key edits the selected row's text.
                _ => {
                    if let Some(row) = ve.rows.get_mut(ve.sel) {
                        row.handle_key_event(key);
                    }
                }
            }
        }
    }
}

/// Service a [`UiAction`] that needs the worker / schema. P2 handles save and
/// cancel; create/delete/refresh arrive in P4.
fn handle_action(app: &mut App, action: UiAction, read_flow: &mut ReadFlow) {
    match action {
        UiAction::FormSave => {
            let Some(form) = app.form.as_ref() else { return };
            let original = EditEntry {
                dn: form.dn.clone(),
                attrs: form.baseline.clone(),
            };
            let edited = form.to_edit_entry();
            let object_classes = object_classes_of(form);
            match prepare_save(read_flow.schema(), &original, &edited, &object_classes) {
                PrepareSave::Ready { plan, dn, ldif } => {
                    app.overlay = Some(Overlay::Confirm {
                        title: "Apply these changes?".to_string(),
                        body: ldif,
                        action: PendingAction::Save { plan, dn },
                    });
                }
                PrepareSave::NoChanges => app.status = "No changes.".to_string(),
                PrepareSave::Invalid(errs) => {
                    app.overlay = Some(Overlay::Error {
                        text: format_validation_errors(&errs),
                    })
                }
                PrepareSave::DiffError(e) => app.overlay = Some(Overlay::Error { text: e }),
            }
        }
        UiAction::FormCancel => revert_form(app),
        _ => {}
    }
}

/// Revert every field to its baseline (F3 cancel): drop multi-value edits and
/// reseed each single-value editor from the original values.
fn revert_form(app: &mut App) {
    if let Some(form) = app.form.as_mut() {
        for field in &mut form.fields {
            let base = form.baseline.get(&field.label).cloned().unwrap_or_default();
            field.editor = TextState::new().with_value(base.first().cloned().unwrap_or_default());
            field.values = base;
        }
        app.status = "Reverted.".to_string();
    }
}

/// After a save re-reads `dn` (possibly a rename's new DN), point both the
/// awaited DN and the current leaf row at it, so the post-save base-read passes
/// the DN gate and `reconcile` does not fire a competing read of the old DN.
/// (The tree / leaf-label structure reflow is P4.)
fn rebind_selection(app: &mut App, dn: &str) {
    app.last_seen_leaf = Some(dn.to_string());
    if let Some(row) = app.rows.get_mut(app.leaf_sel) {
        row.1 = dn.to_string();
    }
}

/// Handle a key while an overlay is open. Returns the action to run when the
/// user confirms a [`Overlay::Confirm`]; otherwise dismisses / consumes the key.
fn overlay_key(app: &mut App, key: KeyEvent) -> Option<PendingAction> {
    match &app.overlay {
        Some(Overlay::Confirm { .. }) => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                // Take the overlay out to move its action.
                if let Some(Overlay::Confirm { action, .. }) = app.overlay.take() {
                    return Some(action);
                }
                None
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc | KeyCode::F(3) => {
                app.overlay = None;
                None
            }
            _ => None,
        },
        Some(Overlay::Error { .. }) => {
            // Any key dismisses an error.
            app.overlay = None;
            None
        }
        Some(Overlay::ValueEditor(_)) => {
            value_editor_key(app, key);
            None
        }
        None => None,
    }
}

/// Run a confirmed [`PendingAction`] (submits to the worker).
fn execute_pending(
    app: &mut App,
    action: PendingAction,
    worker: &WorkerHandle,
    post: &mut HashMap<u64, PostWrite>,
    pending_followups: &mut HashMap<u64, (String, Vec<ModOp>)>,
) {
    match action {
        PendingAction::Save { plan, dn } => {
            submit_prepared(plan, &dn, worker, post, pending_followups);
            app.status = "Saving…".to_string();
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

/// The entry's objectClass values, read from a built form's baseline
/// (case-insensitive). Needed by the write path's client-side validation.
fn object_classes_of(form: &EditForm) -> Vec<String> {
    form.baseline
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("objectClass"))
        .map(|(_, v)| v.clone())
        .unwrap_or_default()
}

/// The outcome of preparing a form save.
enum PrepareSave {
    /// Client-side validation failed.
    Invalid(Vec<ValidationError>),
    /// The diff could not be computed (e.g. multi-valued RDN).
    DiffError(String),
    /// The edited entry equals the baseline — nothing to do.
    NoChanges,
    /// A ready plan, its target DN, and the LDIF preview.
    Ready {
        /// The save plan to submit.
        plan: SavePlan,
        /// The (old) DN the plan targets.
        dn: String,
        /// LDIF preview text for the confirmation overlay.
        ldif: String,
    },
}

/// Validate + diff the edited entry against the `original` (baseline) and, if
/// there is a real change, return a ready [`SavePlan`] with an LDIF preview.
fn prepare_save(
    schema: &SchemaModel,
    original: &EditEntry,
    edited: &EditEntry,
    object_classes: &[String],
) -> PrepareSave {
    let oc_refs: Vec<&str> = object_classes.iter().map(|s| s.as_str()).collect();
    let errors = validate(edited, schema, &oc_refs);
    if !errors.is_empty() {
        return PrepareSave::Invalid(errors);
    }
    let cs = match diff(original, edited) {
        Ok(cs) => cs,
        Err(e) => return PrepareSave::DiffError(e.to_string()),
    };
    if cs.is_empty() {
        return PrepareSave::NoChanges;
    }
    let ldif = render_changeset(&cs);
    PrepareSave::Ready {
        plan: plan_save(cs),
        dn: original.dn.clone(),
        ldif,
    }
}

/// Submit the worker request(s) for a prepared [`SavePlan`] and record how to
/// react to the resulting `WriteOk`. A rename with follow-up mods defers them to
/// the rename's `WriteOk` (the MODIFY must target the post-rename DN).
fn submit_prepared(
    plan: SavePlan,
    old_dn: &str,
    worker: &WorkerHandle,
    post: &mut HashMap<u64, PostWrite>,
    pending_followups: &mut HashMap<u64, (String, Vec<ModOp>)>,
) {
    match plan {
        SavePlan::Nothing => {}
        SavePlan::Modify(mods) => {
            let id = next_id();
            let _ = worker.submit(Request::Modify {
                id,
                dn: old_dn.to_string(),
                changes: mods,
            });
            post.insert(
                id,
                PostWrite::Save {
                    reread_dn: old_dn.to_string(),
                },
            );
        }
        SavePlan::RenameOnly(modrdn) => {
            let id = next_id();
            let new_dn = compose_renamed_dn(old_dn, &modrdn.new_rdn);
            let _ = worker.submit(Request::ModRdn {
                id,
                dn: old_dn.to_string(),
                new_rdn: modrdn.new_rdn,
                delete_old: modrdn.delete_old,
                new_superior: modrdn.new_superior,
            });
            post.insert(id, PostWrite::Save { reread_dn: new_dn });
        }
        SavePlan::Rename { modrdn, then_mods } => {
            let id = next_id();
            let new_dn = compose_renamed_dn(old_dn, &modrdn.new_rdn);
            pending_followups.insert(id, (new_dn, then_mods));
            let _ = worker.submit(Request::ModRdn {
                id,
                dn: old_dn.to_string(),
                new_rdn: modrdn.new_rdn,
                delete_old: modrdn.delete_old,
                new_superior: modrdn.new_superior,
            });
        }
    }
}

/// The parent DN (everything after the first comma), or `None` at the top.
fn parent_dn(dn: &str) -> Option<&str> {
    dn.split_once(',').map(|(_, rest)| rest)
}

/// Compose the post-rename DN: `<new_rdn>,<parent of old_dn>`.
fn compose_renamed_dn(old_dn: &str, new_rdn: &str) -> String {
    match parent_dn(old_dn) {
        Some(container) => format!("{new_rdn},{container}"),
        None => new_rdn.to_string(),
    }
}

/// Format a list of [`ValidationError`]s as one multi-line message.
fn format_validation_errors(errors: &[ValidationError]) -> String {
    let mut out = String::from("Cannot save — please fix:");
    for e in errors {
        let line = match e {
            ValidationError::MissingMust(a) => format!("missing required attribute: {a}"),
            ValidationError::MultiValueOnSingle(a) => format!("attribute is single-valued: {a}"),
            ValidationError::SyntaxInvalid { attr, reason } => format!("{attr}: {reason}"),
        };
        out.push_str("\n- ");
        out.push_str(&line);
    }
    out
}

/// Monotonic correlation id for write requests, starting at a high base so write
/// ids never collide with the read/browse ids (which start at 1).
fn next_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1_000_000);
    COUNTER.fetch_add(1, Ordering::Relaxed)
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
        let label = structure
            .get(dn)
            .map(|n| n.label.clone())
            .unwrap_or_else(|| dn.split(',').next().unwrap_or(dn).trim().to_string());
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

    /// A bare App carrying a value-editor overlay of `rows` rows (form left None;
    /// the popup nav/reorder paths only touch the overlay).
    fn app_with_value_editor(rows: usize) -> App {
        use crate::ui::edit_form::ValueEditor;
        let ve = ValueEditor {
            field: 0,
            label: "mail".into(),
            ordered: false,
            secret: false,
            rows: (0..rows)
                .map(|i| TextState::new().with_value(format!("v{i}")))
                .collect(),
            sel: 0,
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
            form: None,
            form_focus: 0,
            form_scroll: 0,
            overlay: Some(Overlay::ValueEditor(ve)),
            status: String::new(),
        }
    }

    /// Regression (P3 review): deleting every row and then navigating/reordering
    /// must not panic. The reorder swaps are bounded and `sel` is normalized, so
    /// `Vec::swap` is never called out of range on an empty popup.
    #[test]
    fn value_editor_delete_all_then_navigate_does_not_panic() {
        let alt = |c| KeyEvent::new(c, KeyModifiers::ALT);
        let plain = |c| KeyEvent::new(c, KeyModifiers::NONE);
        let mut app = app_with_value_editor(1);
        value_editor_key(&mut app, alt(KeyCode::Char('d'))); // delete the only row → empty
        value_editor_key(&mut app, plain(KeyCode::Down)); // must not advance sel out of range
        value_editor_key(&mut app, alt(KeyCode::Up)); // reorder on empty → no-op, no panic
        value_editor_key(&mut app, alt(KeyCode::Down)); // no panic
        value_editor_key(&mut app, alt(KeyCode::Char('a'))); // add a row back
        let Some(Overlay::ValueEditor(ve)) = &app.overlay else {
            panic!("popup should still be open");
        };
        assert_eq!(ve.rows.len(), 1);
        assert!(ve.sel < ve.rows.len());
    }

    /// Reorder swaps stay in range across a delete-driven shrink, too.
    #[test]
    fn value_editor_reorder_after_delete_is_bounded() {
        let alt = |c| KeyEvent::new(c, KeyModifiers::ALT);
        let mut app = app_with_value_editor(3);
        // Move to the last row, delete it, then try to reorder down (past the end).
        value_editor_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        value_editor_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        value_editor_key(&mut app, alt(KeyCode::Char('d'))); // delete last → sel clamps
        value_editor_key(&mut app, alt(KeyCode::Down)); // no panic
        value_editor_key(&mut app, alt(KeyCode::Up)); // no panic
        let Some(Overlay::ValueEditor(ve)) = &app.overlay else {
            panic!("popup should still be open");
        };
        assert!(ve.sel < ve.rows.len());
    }

    #[test]
    fn next_index_clamps_at_end() {
        assert_eq!(next_index(0, 3), 1);
        assert_eq!(next_index(2, 3), 2);
        assert_eq!(next_index(0, 0), 0);
    }

    #[test]
    fn compose_renamed_dn_replaces_rdn() {
        assert_eq!(
            compose_renamed_dn("cn=Alice,ou=people,dc=org", "cn=Bob"),
            "cn=Bob,ou=people,dc=org"
        );
        assert_eq!(compose_renamed_dn("dc=org", "dc=net"), "dc=net");
    }

    #[test]
    fn next_id_is_monotonic_and_high() {
        let a = next_id();
        let b = next_id();
        assert!(b > a && a >= 1_000_000);
    }

    #[test]
    fn validation_errors_format_as_bullets() {
        let errs = vec![
            ValidationError::MissingMust("sn".into()),
            ValidationError::MultiValueOnSingle("cn".into()),
        ];
        let out = format_validation_errors(&errs);
        assert!(out.contains("missing required attribute: sn"));
        assert!(out.contains("attribute is single-valued: cn"));
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
        assert_eq!(
            rows[1],
            ("Jane".to_string(), "uid=jane,ou=users,dc=example,dc=org".to_string())
        );
        assert_eq!(compute_rows(&s, "ou=users,dc=example,dc=org", "zzz").len(), 1);
    }

    #[test]
    fn tree_items_contain_only_branches() {
        let s = structure();
        let items = build_tree_items(&s);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].children().len(), 1);
    }
}
