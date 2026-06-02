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

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use anyhow::{anyhow, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use tui_prompts::{State, TextState};
use tui_tree_widget::{TreeItem, TreeState};

use crate::app::UiAction;
use crate::config::relation::{backref_lookup, resolve_relations, ResolvedRelation};
use crate::config::{Config, EntryProfile};
use crate::form::changeset::{diff, ChangeSet, EditEntry, ModOp};
use crate::form::validate::{plan_save, validate, SavePlan, ValidationError};
use crate::ldap::ldif::{render_add, render_changeset, render_changesets};
use crate::ldap::worker::{Request, Response, SearchScope, StructureNodeRaw, WorkerHandle};
use crate::schema::SchemaModel;
use crate::ui::edit_form::{build_edit_form, value_set_eq, EditForm, FormMode, ValueEditor};
use crate::ui::form_state::{guard_decision, GuardChoice, GuardOutcome};
use crate::ui::picker::PICKER_SEARCH_CAP;
use crate::ui::view;
use crate::workflows::create::{build_add_entry, empty_form_for_profile};
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
/// keys (plan §3.4).
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
    /// The Save/Discard/Stay guard shown when leaving a dirty form — by changing
    /// the selection, moving focus off the form pane, or quitting. Carries the
    /// pending [`GuardIntent`] to resume once the user chooses.
    Guard {
        /// What to do once the guard is resolved.
        intent: GuardIntent,
    },
}

/// What a dirty-form [`Overlay::Guard`] should resume once the user resolves it.
#[derive(Clone)]
pub enum GuardIntent {
    /// Navigate the form to a leaf DN (`None` clears it) — the selection-change
    /// guard fired from `reconcile`.
    Nav(Option<String>),
    /// Move focus to another pane — the Tab/Shift-Tab-off-the-form guard.
    Focus(Pane),
    /// Quit the application — the Alt+X / quit-while-dirty guard.
    Quit,
}

/// What a confirmed [`Overlay::Confirm`] (or resolved [`Overlay::Guard`]) should
/// do once the worker is available.
pub enum PendingAction {
    /// Submit a prepared save plan against `dn`; `nav` is a deferred navigation
    /// target (set when a guard's Save resolves), serviced after the write.
    Save {
        /// The save plan to submit on confirm.
        plan: SavePlan,
        /// The (old) DN the plan targets.
        dn: String,
        /// A deferred navigation target (the entry to move to after the save).
        nav: Option<String>,
    },
    /// Submit an `Add` for a newly created entry, then splice it into the tree.
    Create {
        /// The new entry's DN.
        dn: String,
        /// The new entry's attributes.
        attrs: BTreeMap<String, Vec<String>>,
        /// The container DN the entry is added under (for the structure splice).
        parent: String,
    },
    /// Submit a `Delete` for `dn`, then reflow the structure.
    Delete {
        /// The DN to delete.
        dn: String,
    },
    /// A resolved dirty-form guard: perform `intent`, running the save flow first
    /// when `save` is true (Save) or proceeding directly when false (Discard).
    ResolveGuard {
        /// What to do (navigate / change focus / quit).
        intent: GuardIntent,
        /// Whether to save the dirty form before performing the intent.
        save: bool,
    },
    /// A combined membership save: own-entry MODIFY + per-holder fan-out MODIFYs,
    /// applied synchronously (spec §6.3). `entry_dn` is the edited candidate entry.
    CombinedSave {
        /// The candidate entry being saved.
        entry_dn: String,
        /// Own-entry attribute mods (empty when only membership changed).
        own_mods: Vec<ModOp>,
        /// Per-holder fan-out: (group_dn, Add/Delete op for holder_attr).
        fanout: Vec<(String, ModOp)>,
        /// A dirty-form guard intent to perform after a successful save (set when
        /// the combined save is reached via a Save-then-resume guard); `None` for
        /// a plain F2 save.
        then_intent: Option<GuardIntent>,
    },
}

/// What the run-loop should do when a write's `WriteOk` arrives, keyed by id.
enum PostWrite {
    /// A form save (Modify / RenameOnly): re-read `reread_dn` into the form,
    /// unless `nav` is set (a guard Save) in which case navigate there instead.
    Save {
        /// The DN to re-read once the write succeeds.
        reread_dn: String,
        /// A deferred navigation target (the entry the user moved to while dirty).
        nav: Option<String>,
        /// Quit once the write succeeds (a quit-while-dirty guard's Save).
        then_quit: bool,
    },
    /// A create: splice the new entry into the eager [`Structure`] under `parent`.
    Created {
        /// The container the entry was added under.
        parent: String,
        /// The new entry's structure row.
        input: StructureInput,
    },
    /// A delete: drop `dn` from the [`Structure`] and reflow.
    Deleted {
        /// The removed entry's DN.
        dn: String,
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
    /// Resolved membership relations (built once from config).
    pub relations: Vec<ResolvedRelation>,
    /// Correlation id of the latest in-flight picker search (stale ids ignored).
    pub picker_search_id: Option<u64>,
    /// The picker search term last submitted (delta detection in the loop).
    pub picker_last_query: String,
}

/// Spawn the worker, fetch the schema + eager structure, then run the TUI.
pub fn run(config: Config, password: String) -> Result<()> {
    let base_dn = config.server.base_dn.clone();
    let read_only = config.is_read_only();
    let profiles = config.profiles.clone();
    let relations = resolve_relations(&config.profiles, &config.relations);

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
                if truncated {
                    " — result truncated"
                } else {
                    ""
                }
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
        relations,
        picker_search_id: None,
        picker_last_query: String::new(),
    };

    let mut terminal = ratatui::init();
    let res = event_loop(
        &mut terminal,
        &mut app,
        &worker,
        &mut read_flow,
        structure,
        &profiles,
        &base_dn,
    );
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
    mut structure: Structure,
    profiles: &[EntryProfile],
    base_dn: &str,
) -> Result<()> {
    // Write-tracking maps (orchestration locals, plan §2.1).
    let mut post: HashMap<u64, PostWrite> = HashMap::new();
    let mut pending_followups: HashMap<u64, (String, Vec<ModOp>, Option<String>)> = HashMap::new();

    loop {
        terminal.draw(|f| view::ui(f, app))?;

        // 1) Drain ALL pending worker responses (writes first, then read forms).
        while let Some(resp) = worker.poll() {
            handle_worker_response(
                app,
                &resp,
                worker,
                read_flow,
                &mut structure,
                &mut post,
                &mut pending_followups,
            );
        }

        // 2) Poll input with a timeout so the worker drain keeps ticking. An open
        //    overlay captures every key (plan §3.4).
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Release {
                    if app.overlay.is_some() {
                        if let Some(action) = overlay_key(app, key) {
                            execute_pending(
                                app,
                                action,
                                worker,
                                read_flow,
                                profiles,
                                &mut post,
                                &mut pending_followups,
                            );
                        }
                    } else if let Some(action) = dispatch_key(app, key, &structure) {
                        handle_action(
                            app,
                            action,
                            worker,
                            read_flow,
                            &mut structure,
                            profiles,
                            base_dn,
                        );
                    }
                }
            }
        }

        // 3) Reconcile UI deltas (no-op while an overlay holds the keys).
        if app.overlay.is_none() {
            reconcile(app, &structure, worker, read_flow);
        }

        // 4) Service picker type-ahead (runs regardless of reconcile gate).
        service_picker_search(app, worker);

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
    structure: &mut Structure,
    post: &mut HashMap<u64, PostWrite>,
    pending_followups: &mut HashMap<u64, (String, Vec<ModOp>, Option<String>)>,
) {
    match resp {
        // Intercept picker search results before the read-flow routing.
        Response::Entries { id, entries, .. } if app.picker_search_id == Some(*id) => {
            if let Some(Overlay::ValueEditor(ve)) = app.overlay.as_mut() {
                if let Some(p) = ve.picker.as_mut() {
                    let results = entries
                        .iter()
                        .map(|e| crate::ui::picker::Candidate {
                            dn: e.dn.clone(),
                            label: crate::ui::picker::candidate_label(&e.dn, &e.attrs),
                        })
                        .collect();
                    p.set_results(results);
                    // Heuristic: if the result count hit the cap, the server may
                    // have more matching entries — signal the view to show a hint.
                    p.truncated = entries.len() as i32 >= PICKER_SEARCH_CAP;
                }
            }
        }
        Response::WriteOk { id, .. } => {
            if let Some((new_dn, mods, nav)) = pending_followups.remove(id) {
                // A rename's MODRDN succeeded: apply the deferred mods to the new
                // DN, then navigate to the guard's target (if any) or the new DN.
                let _ = worker.submit(Request::Modify {
                    id: next_id(),
                    dn: new_dn.clone(),
                    changes: mods,
                });
                let target = nav.unwrap_or(new_dn);
                rebind_selection(app, &target);
                let _ = read_flow.request_entry(worker, &target, None);
                return;
            }
            match post.remove(id) {
                Some(PostWrite::Save {
                    reread_dn,
                    nav,
                    then_quit,
                }) => {
                    app.status = "Saved.".to_string();
                    // A quit-while-dirty guard's Save defers the quit until the
                    // write lands, so the write is never lost.
                    if then_quit {
                        app.should_quit = true;
                        return;
                    }
                    // A guard's Save defers navigation to the moved-to entry; an
                    // ordinary save just re-reads the entry that was saved.
                    // NOTE: do NOT recompute rows from `structure` here — it is not
                    // updated on a rename, so it would overwrite the rebind below
                    // with the stale old DN (spurious guard + dropped re-read). The
                    // structure is reflowed only on Created/Deleted; a rename's
                    // leaf-label staleness self-heals on Refresh (F5).
                    let target = nav.unwrap_or(reread_dn);
                    rebind_selection(app, &target);
                    let _ = read_flow.request_entry(worker, &target, None);
                }
                Some(PostWrite::Created { parent, input }) => {
                    app.status = "Created.".to_string();
                    // The pane-3 create form has been committed — drop it so the
                    // clobber guard no longer blocks reads and `reconcile` re-reads
                    // the current tree selection into the form pane (the new entry
                    // now appears in the leaf list).
                    if app.form.as_ref().map(|f| f.is_new()).unwrap_or(false) {
                        app.form = None;
                        app.form_focus = 0;
                        app.form_scroll = 0;
                        app.last_seen_leaf = None;
                    }
                    // A new child may turn a former leaf into a branch → rebuild
                    // the tree; always refresh the leaf rows.
                    if structure.add_child(&parent, input) {
                        app.tree_items = build_tree_items(structure);
                    }
                    app.rows = compute_rows(structure, &app.current_branch, &app.last_search);
                }
                Some(PostWrite::Deleted { dn }) => {
                    app.status = "Deleted.".to_string();
                    let was_branch = structure.get(&dn).map(|n| n.is_branch()).unwrap_or(false);
                    let demoted = structure.remove(&dn);
                    if was_branch || demoted {
                        app.tree_items = build_tree_items(structure);
                    }
                    // Clear the form if it was showing the now-deleted entry.
                    if app.form.as_ref().map(|f| f.dn == dn).unwrap_or(false) {
                        app.form = None;
                    }
                    app.rows = compute_rows(structure, &app.current_branch, &app.last_search);
                    app.last_seen_leaf = None;
                }
                None => app.status = "Saved.".to_string(),
            }
        }
        Response::WriteError { id, msg } => {
            // Drop any tracking for the failed write so its maps do not leak, and
            // re-sync the awaited DN to the entry actually shown — otherwise a
            // failed guard-Save would leave `last_seen_leaf` pointing at the
            // moved-to entry, silently silencing the dirty guard.
            post.remove(id);
            pending_followups.remove(id);
            if let Some(form) = app.form.as_ref() {
                app.last_seen_leaf = Some(form.dn.clone());
            }
            app.overlay = Some(Overlay::Error { text: msg.clone() });
        }
        // on_response consumes the pending id, so call it exactly once.
        _ => match read_flow.on_response(resp) {
            ReadOutcome::Form { model, .. } => {
                // Rapid leaf navigation submits overlapping base-reads; the worker
                // is FIFO so an older read can resolve first. Install only the
                // response whose DN matches the entry the user is currently on,
                // else a stale entry would flash (and clobber edits). Also defer
                // installation while an editing overlay (create / value editor) is
                // open, so a late base-read cannot replace `app.form` under it.
                if should_install_form(app, &model.title) {
                    app.form = Some(build_edit_form(
                        &model,
                        read_flow.schema(),
                        app.read_only,
                        &app.relations,
                    ));
                    app.form_focus = 0;
                    app.form_scroll = 0;
                    app.status.clear();
                }
            }
            // Promote a read failure to an Error overlay so it is visible (P5-T2);
            // previously it only landed in the easily-missed status line. Gate on
            // `overlay.is_none()` — same as the Form arm above — so a late base-read
            // that errors cannot clobber an in-progress create / value-editor
            // overlay; in that case fall back to the status line.
            ReadOutcome::Error(msg) => {
                if app.overlay.is_none() {
                    app.overlay = Some(Overlay::Error { text: msg });
                } else {
                    app.status = msg;
                }
            }
            ReadOutcome::Ignored => {}
        },
    }
}

/// Translate a key into an `App` mutation (gated by the focused pane), returning
/// a [`UiAction`] for the few keys the loop must service with the worker.
fn dispatch_key(app: &mut App, key: KeyEvent, structure: &Structure) -> Option<UiAction> {
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    // Global quit. Quitting while the form has unsaved edits opens the guard
    // first (the user picks Save / Discard / Stay) rather than dropping them.
    if (alt && matches!(key.code, KeyCode::Char('x') | KeyCode::Char('X')))
        || (ctrl && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C')))
    {
        if !guard_if_dirty(app, GuardIntent::Quit) {
            app.should_quit = true;
        }
        return None;
    }
    // Focus cycle: F6 / Tab forward, Shift-Tab (BackTab) backward. Moving focus
    // OFF a dirty form opens the guard, carrying the destination pane.
    if matches!(key.code, KeyCode::F(6) | KeyCode::Tab | KeyCode::BackTab) {
        let dest = if key.code == KeyCode::BackTab {
            prev_pane(app.focus)
        } else {
            next_pane(app.focus)
        };
        if app.focus == Pane::Form && guard_if_dirty(app, GuardIntent::Focus(dest)) {
            return None;
        }
        app.focus = dest;
        return None;
    }
    // Refresh is allowed even in read-only mode (it only re-reads).
    if matches!(key.code, KeyCode::F(5)) {
        return Some(UiAction::Refresh);
    }
    // Bare `q` quits, but ONLY from the Tree pane — the search box and form need
    // the key for text entry. Guarded when the form has unsaved edits.
    if app.focus == Pane::Tree && key.code == KeyCode::Char('q') {
        if !guard_if_dirty(app, GuardIntent::Quit) {
            app.should_quit = true;
        }
        return None;
    }
    // Save / Cancel / Create / Delete (writable mode only). Read-only mode
    // suppresses every write affordance (P4-T4). These keys are surfaced in the
    // status-line hints (view::pane_hints); F7 creates under the first profile.
    if !app.read_only {
        match key.code {
            KeyCode::F(2) => return Some(UiAction::FormSave),
            KeyCode::F(3) => return Some(UiAction::FormCancel),
            KeyCode::F(7) => return Some(UiAction::NewEntry(0)),
            // F8 deletes the entry currently shown in the form pane (spec §12).
            KeyCode::F(8) => {
                return app
                    .form
                    .as_ref()
                    .filter(|f| !f.dn.is_empty())
                    .map(|f| UiAction::DeleteEntry(f.dn.clone()));
            }
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
                KeyCode::PageDown => {
                    app.form_focus = (app.form_focus + 10).min(n.saturating_sub(1))
                }
                // Enter on an editable multi-value field opens the value-editor
                // popup; on a single field it is a no-op.
                KeyCode::Enter => open_value_editor(app, structure),
                // Esc cancels a create form (parity with the old modal's Esc); on
                // an edit form Esc is a no-op (F3 reverts edits).
                KeyCode::Esc if app.form.as_ref().map(|f| f.is_new()).unwrap_or(false) => {
                    return Some(UiAction::FormCancel)
                }
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

/// Open the multi-value popup over the focused field. Relation fields open in
/// picker mode; plain multi-valued fields open in free-text mode.
fn open_value_editor(app: &mut App, structure: &Structure) {
    let focus = app.form_focus;
    let Some(form) = app.form.as_ref() else {
        return;
    };
    let Some(field) = form.fields.get(focus) else {
        return;
    };
    if field.relation.is_some() && field.multi && field.editable {
        // Picker mode: label DNs from the loaded structure (fallback = the DN).
        let label_of = |dn: &str| {
            structure
                .get(dn)
                .map(|n| n.label.clone())
                .unwrap_or_else(|| dn.to_string())
        };
        let ve = ValueEditor::open_picker(focus, field, label_of);
        app.overlay = Some(Overlay::ValueEditor(ve));
        app.picker_last_query.clear();
        app.picker_search_id = None;
    } else if field.multi && field.editable {
        let ve = ValueEditor::open(focus, field);
        app.overlay = Some(Overlay::ValueEditor(ve));
    }
}

/// Keys inside the picker: Esc/F3 cancel; F2 commit selected DNs to the field;
/// ↑↓ move; Space toggle; any other key edits the search box (the tick-based
/// `service_picker_search` turns a changed query into a live candidate search).
fn picker_editor_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc | KeyCode::F(3) => {
            app.overlay = None;
            app.picker_search_id = None;
            app.picker_last_query.clear();
        }
        KeyCode::F(2) => {
            if let Some(Overlay::ValueEditor(ve)) = app.overlay.take() {
                if let Some(picker) = &ve.picker {
                    if let Some(field) = app.form.as_mut().and_then(|f| f.fields.get_mut(ve.field))
                    {
                        field.values = picker.selected_dns();
                    }
                }
            }
            app.picker_search_id = None;
            app.picker_last_query.clear();
        }
        KeyCode::Up => {
            if let Some(Overlay::ValueEditor(ve)) = app.overlay.as_mut() {
                if let Some(p) = ve.picker.as_mut() {
                    p.move_cursor(-1);
                }
            }
        }
        KeyCode::Down => {
            if let Some(Overlay::ValueEditor(ve)) = app.overlay.as_mut() {
                if let Some(p) = ve.picker.as_mut() {
                    p.move_cursor(1);
                }
            }
        }
        KeyCode::Char(' ') => {
            if let Some(Overlay::ValueEditor(ve)) = app.overlay.as_mut() {
                if let Some(p) = ve.picker.as_mut() {
                    p.toggle_cursor();
                }
            }
        }
        _ => {
            if let Some(Overlay::ValueEditor(ve)) = app.overlay.as_mut() {
                ve.search.handle_key_event(key);
            }
        }
    }
}

/// Handle a key inside the multi-value popup (spike `popup_key`): nav (↑↓),
/// reorder (Alt+↑↓), insert (Alt+a / Insert), delete (Alt+d), commit (F2,
/// dropping empties), cancel (Esc / F3); any other key edits the selected row.
fn value_editor_key(app: &mut App, key: KeyEvent) {
    // Picker mode has its own key map (search box + selection toggle).
    if matches!(&app.overlay, Some(Overlay::ValueEditor(ve)) if ve.picker.is_some()) {
        picker_editor_key(app, key);
        return;
    }
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
                (KeyCode::Down, false) => {
                    ve.sel = (ve.sel + 1).min(ve.rows.len().saturating_sub(1))
                }
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

/// When a picker is open and its search term changed, submit a fresh size-capped
/// candidate search (stale ids are discarded in `handle_worker_response`). Empty
/// term → clear results (selection-only view). Mirrors the leaf incremental search.
fn service_picker_search(app: &mut App, worker: &WorkerHandle) {
    let Some(Overlay::ValueEditor(ve)) = app.overlay.as_ref() else {
        return;
    };
    if ve.picker.is_none() {
        return;
    }
    let query = ve.search.value().to_string();
    if query == app.picker_last_query {
        return;
    }
    app.picker_last_query = query.clone();
    let Some(scope) = ve.scope.clone() else {
        return;
    };

    if query.is_empty() {
        if let Some(Overlay::ValueEditor(ve)) = app.overlay.as_mut() {
            if let Some(p) = ve.picker.as_mut() {
                p.set_results(Vec::new());
                p.truncated = false;
            }
        }
        app.picker_search_id = None;
        return;
    }
    let id = next_id();
    app.picker_search_id = Some(id);
    let filter =
        crate::ui::picker::build_member_filter(&scope.object_classes, &scope.search_attrs, &query);
    let _ = worker.submit(Request::Search {
        id,
        base: scope.base,
        scope: SearchScope::Subtree,
        filter,
        attrs: vec!["cn".to_string()],
        size_limit: Some(PICKER_SEARCH_CAP),
    });
}

/// Service a [`UiAction`] that needs the worker / schema. Save and cancel build
/// confirm overlays; create opens an editable overlay; delete opens a confirm;
/// refresh re-runs the eager scan synchronously and rebuilds the panes.
fn handle_action(
    app: &mut App,
    action: UiAction,
    worker: &WorkerHandle,
    read_flow: &mut ReadFlow,
    structure: &mut Structure,
    profiles: &[EntryProfile],
    base_dn: &str,
) {
    match action {
        UiAction::FormSave => {
            let Some(form) = app.form.as_ref() else {
                return;
            };
            if form.is_new() {
                prepare_create(app, worker, read_flow, profiles);
                return;
            }
            // Try the combined membership path first; fall back to the single-entry
            // path when no backref field actually changed. No guard intent here —
            // a plain F2 save has nothing to resume afterward.
            if let Some(ov) = combined_save_overlay(form, read_flow.schema(), &app.relations, None)
            {
                app.overlay = Some(ov);
                return;
            }
            // Normal single-entry save — strip backref labels from baseline so
            // `diff` does not emit a spurious Delete for server-maintained attrs.
            let backref_lbls = form.backref_labels();
            let mut original = EditEntry {
                dn: form.dn.clone(),
                attrs: form.baseline.clone(),
            };
            for l in &backref_lbls {
                original.attrs.remove(l);
            }
            let edited = form.to_edit_entry();
            let object_classes = object_classes_of(form);
            match prepare_save(read_flow.schema(), &original, &edited, &object_classes) {
                PrepareSave::Ready { plan, dn, ldif } => {
                    app.overlay = Some(Overlay::Confirm {
                        title: "Apply these changes?".to_string(),
                        body: ldif,
                        action: PendingAction::Save {
                            plan,
                            dn,
                            nav: None,
                        },
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
        UiAction::NewEntry(i) => {
            if let Some(profile) = profiles.get(i) {
                let container = if profile.search_base.is_empty() {
                    structure.root_dn().to_string()
                } else {
                    profile.search_base.clone()
                };
                let form = build_new_entry_form(read_flow.schema(), profile, i, container);
                app.form = Some(form);
                app.form_focus = 0;
                app.form_scroll = 0;
                app.overlay = None;
                // Focus the form pane so keystrokes edit the new entry's fields.
                app.focus = Pane::Form;
                app.status = format!(
                    "New {} — fill fields, F2 to create, Esc to cancel.",
                    profile.name
                );
            }
        }
        UiAction::DeleteEntry(dn) => {
            if !dn.is_empty() {
                app.overlay = Some(Overlay::Confirm {
                    title: "Delete this entry?".to_string(),
                    body: dn.clone(),
                    action: PendingAction::Delete { dn },
                });
            }
        }
        UiAction::Refresh => refresh_structure(app, worker, structure, base_dn),
        UiAction::None => {}
    }
}

/// Re-run the eager structure scan (synchronous, like startup) and rebuild the
/// tree + leaf panes. Keeps the current branch if it still exists, else falls
/// back to the base DN. (Port of the old `UiAction::Refresh` arm.)
fn refresh_structure(
    app: &mut App,
    worker: &WorkerHandle,
    structure: &mut Structure,
    base_dn: &str,
) {
    match worker.request(Request::LoadStructure {
        id: 0,
        base: base_dn.to_string(),
        page_size: 500,
    }) {
        Ok(Response::StructureEntries { nodes, .. }) => {
            *structure = Structure::build(base_dn, structure_inputs(nodes));
            app.tree_items = build_tree_items(structure);
            if structure.get(&app.current_branch).is_none() {
                app.current_branch = base_dn.to_string();
            }
            app.rows = compute_rows(structure, &app.current_branch, &app.last_search);
            app.leaf_sel = 0;
            app.last_seen_leaf = None;
            app.status = "Refreshed.".to_string();
        }
        Ok(Response::StructureError { msg, .. }) => {
            app.overlay = Some(Overlay::Error { text: msg })
        }
        _ => {
            app.overlay = Some(Overlay::Error {
                text: "refresh failed".to_string(),
            })
        }
    }
}

/// Whether a freshly base-read form for `title` should replace `app.form` now:
/// it must match the entry the user is currently on, no overlay may be open, and
/// an in-progress (unsaved) create form must not be clobbered by a late read of
/// the previous selection.
fn should_install_form(app: &App, title: &str) -> bool {
    app.last_seen_leaf
        .as_deref()
        .map(|dn| dn.eq_ignore_ascii_case(title))
        .unwrap_or(false)
        && app.overlay.is_none()
        && !app.form.as_ref().map(|f| f.is_new()).unwrap_or(false)
}

/// Revert every field to its baseline (F3 cancel): drop multi-value edits and
/// reseed each single-value editor from the original values. An unsaved create
/// form has no baseline to revert to, so cancel simply discards it.
fn revert_form(app: &mut App) {
    if app.form.as_ref().map(|f| f.is_new()).unwrap_or(false) {
        app.form = None;
        app.form_focus = 0;
        app.form_scroll = 0;
        app.status.clear();
        // Forget the awaited DN so the next reconcile tick re-reads the currently
        // selected leaf into the form pane (instead of leaving it blank).
        app.last_seen_leaf = None;
        return;
    }
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
/// Only the selected row's DN is rebound; the eager `Structure` is not updated on
/// a rename, so the leaf label / tree fully re-sync on the next Refresh (F5).
fn rebind_selection(app: &mut App, dn: &str) {
    app.last_seen_leaf = Some(dn.to_string());
    if let Some(row) = app.rows.get_mut(app.leaf_sel) {
        row.1 = dn.to_string();
    }
}

/// Handle a key while an overlay is open. Returns the action to run when the
/// user confirms a [`Overlay::Confirm`] or resolves a [`Overlay::Guard`];
/// otherwise dismisses / consumes the key.
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
        Some(Overlay::Guard { .. }) => guard_key(app, key),
        None => None,
    }
}

/// Resolve the Save/Discard/Stay guard. Maps the key to a [`GuardChoice`], runs
/// the pure [`guard_decision`], and turns the outcome into a [`PendingAction`]
/// that performs the pending [`GuardIntent`] (or, for Stay, keeps editing).
fn guard_key(app: &mut App, key: KeyEvent) -> Option<PendingAction> {
    let choice = match key.code {
        KeyCode::Char('s') | KeyCode::Char('S') | KeyCode::F(2) => GuardChoice::Save,
        KeyCode::Char('d') | KeyCode::Char('D') => GuardChoice::Discard,
        KeyCode::Char('c') | KeyCode::Char('C') | KeyCode::Esc | KeyCode::F(3) => GuardChoice::Stay,
        _ => return None, // ignore unrelated keys; the guard stays open
    };
    let intent = match &app.overlay {
        Some(Overlay::Guard { intent }) => intent.clone(),
        _ => return None,
    };
    app.overlay = None;
    match guard_decision(true, Some(choice)) {
        GuardOutcome::Cancel => {
            // Stay: keep editing. For a selection-change guard, advance last_seen
            // to the moved-to entry so it does not re-fire every tick (the
            // highlight now differs from the form — a known wrinkle). For a
            // focus/quit guard there is nothing to advance.
            if let GuardIntent::Nav(target) = intent {
                app.last_seen_leaf = target;
            }
            None
        }
        GuardOutcome::Proceed => Some(PendingAction::ResolveGuard {
            intent,
            save: false,
        }),
        GuardOutcome::SaveThenProceed => Some(PendingAction::ResolveGuard { intent, save: true }),
    }
}

/// Run a confirmed [`PendingAction`] (submits to the worker / navigates).
fn execute_pending(
    app: &mut App,
    action: PendingAction,
    worker: &WorkerHandle,
    read_flow: &mut ReadFlow,
    profiles: &[EntryProfile],
    post: &mut HashMap<u64, PostWrite>,
    pending_followups: &mut HashMap<u64, (String, Vec<ModOp>, Option<String>)>,
) {
    match action {
        PendingAction::Save { plan, dn, nav } => {
            submit_prepared(plan, &dn, nav, false, worker, post, pending_followups);
            app.status = "Saving…".to_string();
        }
        PendingAction::Create { dn, attrs, parent } => {
            let id = next_id();
            let input = structure_input_from_attrs(&dn, &attrs);
            let _ = worker.submit(Request::Add { id, dn, attrs });
            post.insert(id, PostWrite::Created { parent, input });
            app.status = "Creating…".to_string();
        }
        PendingAction::Delete { dn } => {
            let id = next_id();
            let _ = worker.submit(Request::Delete { id, dn: dn.clone() });
            post.insert(id, PostWrite::Deleted { dn });
            app.status = "Deleting…".to_string();
        }
        PendingAction::ResolveGuard {
            intent,
            save: false,
        } => {
            // Discard: drop the edits and perform the intent now.
            perform_guard_intent(app, worker, read_flow, intent);
        }
        PendingAction::ResolveGuard { intent, save: true } => {
            // Save, then perform the intent. Navigation defers to the write's
            // WriteOk (the re-read must target the post-save DN); a focus change
            // applies immediately; a quit defers to the WriteOk so it isn't lost.
            let Some(form) = app.form.as_ref() else {
                perform_guard_intent(app, worker, read_flow, intent);
                return;
            };
            // A create form has no diff baseline: route "Save" to the create
            // confirm (an Add). The pending guard intent is dropped — the user
            // confirms the create, then navigates explicitly (full
            // save-then-resume for create is out of scope here).
            if form.is_new() {
                prepare_create(app, worker, read_flow, profiles);
                return;
            }
            // A membership-bearing save runs synchronously through CombinedSave;
            // the pending guard intent rides along and is performed on success.
            if let Some(ov) = combined_save_overlay(
                form,
                read_flow.schema(),
                &app.relations,
                Some(intent.clone()),
            ) {
                app.overlay = Some(ov);
                return;
            }
            // Normal single-entry save — strip backref labels from baseline.
            let backref_lbls = form.backref_labels();
            let mut original = EditEntry {
                dn: form.dn.clone(),
                attrs: form.baseline.clone(),
            };
            for l in &backref_lbls {
                original.attrs.remove(l);
            }
            let edited = form.to_edit_entry();
            let object_classes = object_classes_of(form);
            match prepare_save(read_flow.schema(), &original, &edited, &object_classes) {
                PrepareSave::Ready { plan, dn, .. } => {
                    app.status = "Saving…".to_string();
                    match intent {
                        GuardIntent::Nav(target) => {
                            // Advance the awaited DN ONLY now that we commit, so a
                            // later failure cannot silence the dirty guard.
                            app.last_seen_leaf = target.clone();
                            submit_prepared(
                                plan,
                                &dn,
                                target,
                                false,
                                worker,
                                post,
                                pending_followups,
                            );
                        }
                        GuardIntent::Focus(pane) => {
                            submit_prepared(
                                plan,
                                &dn,
                                None,
                                false,
                                worker,
                                post,
                                pending_followups,
                            );
                            app.focus = pane;
                        }
                        GuardIntent::Quit => {
                            submit_prepared(plan, &dn, None, true, worker, post, pending_followups);
                        }
                    }
                }
                // Nothing to save after all → just perform the intent.
                PrepareSave::NoChanges => perform_guard_intent(app, worker, read_flow, intent),
                PrepareSave::Invalid(errs) => {
                    app.overlay = Some(Overlay::Error {
                        text: format_validation_errors(&errs),
                    })
                }
                PrepareSave::DiffError(e) => app.overlay = Some(Overlay::Error { text: e }),
            }
        }
        PendingAction::CombinedSave {
            entry_dn,
            own_mods,
            fanout,
            then_intent,
        } => {
            apply_combined_save(
                app,
                worker,
                read_flow,
                &entry_dn,
                own_mods,
                fanout,
                then_intent,
            );
        }
    }
}

/// Perform a guard intent WITHOUT saving (Discard, or a save that turned out to
/// be a no-op): navigate / change focus (dropping the form's edits) / quit.
fn perform_guard_intent(
    app: &mut App,
    worker: &WorkerHandle,
    read_flow: &mut ReadFlow,
    intent: GuardIntent,
) {
    match intent {
        GuardIntent::Nav(target) => navigate_to(app, worker, read_flow, target),
        GuardIntent::Focus(pane) => {
            // Drop the edits so the (still-shown) form is clean, then move focus.
            revert_form(app);
            app.focus = pane;
        }
        GuardIntent::Quit => app.should_quit = true,
    }
}

/// Navigate the form pane to `target`: base-read the DN, or clear the form when
/// the target is `None` (empty leaf list). Records `target` as the awaited DN.
fn navigate_to(
    app: &mut App,
    worker: &WorkerHandle,
    read_flow: &mut ReadFlow,
    target: Option<String>,
) {
    // Leaving an unsaved create form discards it, so the clobber guard in
    // `should_install_form` does not then block the destination entry's form.
    if app.form.as_ref().map(|f| f.is_new()).unwrap_or(false) {
        app.form = None;
    }
    app.last_seen_leaf = target.clone();
    match target {
        Some(dn) => {
            let _ = read_flow.request_entry(worker, &dn, None);
        }
        None => app.form = None,
    }
}

/// Reconcile UI deltas each tick: a tree-selection branch switch, a search
/// filter change, and a leaf-selection change (which fires a base-read whose
/// result fills the form). No dirty guard yet (that is P4).
fn reconcile(
    app: &mut App,
    structure: &Structure,
    worker: &WorkerHandle,
    read_flow: &mut ReadFlow,
) {
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

    // 3) Selected leaf DN changed → dirty guard, then base-read it into the form
    //    (or clear it). A dirty form opens the Save/Discard/Stay guard instead of
    //    navigating; the guard carries the target and resolves in `guard_key`.
    let sel_dn = app.rows.get(app.leaf_sel).map(|(_, dn)| dn.clone());
    if sel_dn != app.last_seen_leaf {
        let dirty = app.form.as_ref().map(|f| f.is_dirty()).unwrap_or(false);
        if dirty {
            // Do NOT advance last_seen yet — the guard remembers the target.
            app.overlay = Some(Overlay::Guard {
                intent: GuardIntent::Nav(sel_dn),
            });
            return;
        }
        navigate_to(app, worker, read_flow, sel_dn);
    }
}

/// If the form has unsaved edits, open the Save/Discard/Stay guard carrying
/// `intent` and return `true` (the caller should stop). Otherwise return `false`.
fn guard_if_dirty(app: &mut App, intent: GuardIntent) -> bool {
    if app.form.as_ref().map(|f| f.is_dirty()).unwrap_or(false) {
        app.overlay = Some(Overlay::Guard { intent });
        true
    } else {
        false
    }
}

/// The forward focus cycle: Tree → Leaf → Form → Tree.
fn next_pane(focus: Pane) -> Pane {
    match focus {
        Pane::Tree => Pane::Leaf,
        Pane::Leaf => Pane::Form,
        Pane::Form => Pane::Tree,
    }
}

/// The backward focus cycle (Shift-Tab): Tree → Form → Leaf → Tree.
fn prev_pane(focus: Pane) -> Pane {
    match focus {
        Pane::Tree => Pane::Form,
        Pane::Form => Pane::Leaf,
        Pane::Leaf => Pane::Tree,
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
    nav: Option<String>,
    then_quit: bool,
    worker: &WorkerHandle,
    post: &mut HashMap<u64, PostWrite>,
    pending_followups: &mut HashMap<u64, (String, Vec<ModOp>, Option<String>)>,
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
                    nav,
                    then_quit,
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
            post.insert(
                id,
                PostWrite::Save {
                    reread_dn: new_dn,
                    nav,
                    then_quit,
                },
            );
        }
        SavePlan::Rename { modrdn, then_mods } => {
            let id = next_id();
            let new_dn = compose_renamed_dn(old_dn, &modrdn.new_rdn);
            pending_followups.insert(id, (new_dn, then_mods, nav));
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

/// Outcome of planning a save for a form that has BackRef (membership) changes.
#[derive(Debug)]
enum CombinedPlan {
    /// No BackRef field changed → caller uses the normal single-entry path.
    NoMembershipChange,
    /// Own-entry mods + per-holder fan-out, with the combined LDIF preview.
    Ready {
        entry_dn: String,
        own_mods: Vec<ModOp>,
        fanout: Vec<(String, ModOp)>,
        ldif: String,
    },
    /// Rename combined with a membership change — not supported in v1 (spec §6.3).
    Blocked(String),
    /// Client-side validation failed.
    Invalid(Vec<ValidationError>),
    /// The own-entry diff could not be computed (e.g. multi-valued RDN).
    DiffError(String),
}

/// Plan a combined save: own-entry diff (backref stripped from BOTH sides) plus
/// the fan-out from each BackRef field's baseline→selection delta. Blocks a
/// rename combined with a membership change (v1 simplification, spec §6.3).
///
/// Returns `NoMembershipChange` when no backref field actually changed value,
/// so the caller can fall through to the normal single-entry `prepare_save` path.
fn plan_combined_save(
    form: &EditForm,
    schema: &SchemaModel,
    relations: &[ResolvedRelation],
) -> CombinedPlan {
    let backref = form.backref_labels();
    if backref.is_empty() {
        return CombinedPlan::NoMembershipChange;
    }

    // Did any backref field actually change its value set?
    let changed = form.fields.iter().any(|f| {
        if !backref.contains(&f.label) {
            return false;
        }
        let base = form.baseline.get(&f.label).cloned().unwrap_or_default();
        !value_set_eq(&f.current_values(), &base)
    });
    if !changed {
        return CombinedPlan::NoMembershipChange;
    }

    // Own-entry: strip backref labels from both sides, validate + diff.
    let object_classes = object_classes_of(form);
    let oc_refs: Vec<&str> = object_classes.iter().map(|s| s.as_str()).collect();
    let mut original = EditEntry {
        dn: form.dn.clone(),
        attrs: form.baseline.clone(),
    };
    let mut edited = form.to_edit_entry(); // already omits backref fields
    for l in &backref {
        original.attrs.remove(l);
        edited.attrs.remove(l);
    }

    let errors = validate(&edited, schema, &oc_refs);
    if !errors.is_empty() {
        return CombinedPlan::Invalid(errors);
    }
    let own_cs = match diff(&original, &edited) {
        Ok(c) => c,
        Err(e) => return CombinedPlan::DiffError(e.to_string()),
    };
    if own_cs.modrdn.is_some() {
        return CombinedPlan::Blocked(
            "Rename and membership changes can't be saved together — \
             do them in separate saves."
                .into(),
        );
    }

    // Fan-out: one set of Add/Delete MODIFYs per backref field that changed.
    let mut fanout: Vec<(String, ModOp)> = Vec::new();
    let mut preview_sets: Vec<ChangeSet> = Vec::new();
    if !own_cs.is_empty() {
        preview_sets.push(own_cs.clone());
    }
    for f in form.fields.iter().filter(|f| backref.contains(&f.label)) {
        let Some(rel) = backref_lookup(relations, &object_classes, &f.label) else {
            continue;
        };
        let base = form.baseline.get(&f.label).cloned().unwrap_or_default();
        let ops = membership_fanout(&form.dn, &base, &f.current_values(), &rel.holder_attr);
        for (gdn, op) in ops {
            preview_sets.push(ChangeSet {
                dn: gdn.clone(),
                modrdn: None,
                mods: vec![op.clone()],
            });
            fanout.push((gdn, op));
        }
    }

    CombinedPlan::Ready {
        entry_dn: form.dn.clone(),
        own_mods: own_cs.mods,
        fanout,
        ldif: render_changesets(&preview_sets),
    }
}

/// Map a `CombinedPlan` to the overlay that should be shown, or `None` when
/// there is no membership change (caller falls through to the single-entry save
/// path). Extracted to avoid duplicating the match in `FormSave` and
/// `SaveThenNavigate`.
fn combined_save_overlay(
    form: &EditForm,
    schema: &SchemaModel,
    relations: &[ResolvedRelation],
    then_intent: Option<GuardIntent>,
) -> Option<Overlay> {
    match plan_combined_save(form, schema, relations) {
        CombinedPlan::Ready {
            entry_dn,
            own_mods,
            fanout,
            ldif,
        } => Some(Overlay::Confirm {
            title: "Apply these changes?".to_string(),
            body: ldif,
            action: PendingAction::CombinedSave {
                entry_dn,
                own_mods,
                fanout,
                then_intent,
            },
        }),
        CombinedPlan::Blocked(msg) => Some(Overlay::Error { text: msg }),
        CombinedPlan::Invalid(errs) => Some(Overlay::Error {
            text: format_validation_errors(&errs),
        }),
        CombinedPlan::DiffError(e) => Some(Overlay::Error { text: e }),
        CombinedPlan::NoMembershipChange => None,
    }
}

/// Synchronously re-read `dn` and rebuild the form so it reflects the directory
/// after a combined save. Installs the fresh form directly without depending on
/// the async poll loop or the overlay-gated install path.
fn reload_form_sync(app: &mut App, worker: &WorkerHandle, read_flow: &ReadFlow, dn: &str) {
    rebind_selection(app, dn);
    if let Ok(Response::Entries { entries, .. }) = worker.request(Request::Search {
        id: next_id(),
        base: dn.to_string(),
        scope: SearchScope::Base,
        filter: "(objectClass=*)".to_string(),
        attrs: vec!["*".to_string()],
        size_limit: None,
    }) {
        if let Some(entry) = entries.first() {
            let model = read_flow.form_for(entry, &[]);
            app.form = Some(build_edit_form(
                &model,
                read_flow.schema(),
                app.read_only,
                &app.relations,
            ));
            app.form_focus = 0;
            app.form_scroll = 0;
        }
    }
}

/// Apply a combined membership save SYNCHRONOUSLY (mirrors `refresh_structure`):
/// pre-validate last-member on every removal, abort the whole batch if any would
/// empty a group, then apply own-entry mods + each fan-out MODIFY, collecting a
/// partial-failure report, and finally re-read the edited entry (synchronous).
fn apply_combined_save(
    app: &mut App,
    worker: &WorkerHandle,
    read_flow: &mut ReadFlow,
    entry_dn: &str,
    own_mods: Vec<ModOp>,
    fanout: Vec<(String, ModOp)>,
    then_intent: Option<GuardIntent>,
) {
    // 1. Pre-validate: for each Delete, Base-read the group's current holder_attr
    //    values; block the whole batch if any removal would empty a group.
    //    A read failure is treated conservatively — also blocked.
    let mut blocked: Vec<String> = Vec::new();
    for (gdn, op) in &fanout {
        if let ModOp::Delete { attr, values } = op {
            match read_group_members(worker, gdn, attr) {
                None => {
                    blocked.push(format!("{gdn}: could not verify members"));
                }
                Some(members) => {
                    if let Some(member) = values.first() {
                        if would_empty(&members, member) {
                            blocked.push(format!("{gdn}: would remove last member"));
                        }
                    }
                }
            }
        }
    }
    if !blocked.is_empty() {
        // No write happened — leave form and user's edits intact, no re-read.
        app.overlay = Some(Overlay::Error {
            text: format!(
                "Cannot save — membership change blocked:\n- {}",
                blocked.join("\n- ")
            ),
        });
        return;
    }

    // 2. Apply own-entry mods, then each fan-out MODIFY; collect failures.
    let mut failures: Vec<String> = Vec::new();
    if !own_mods.is_empty() {
        if let Some(msg) = apply_one_modify(worker, entry_dn, own_mods) {
            failures.push(format!("{entry_dn}: {msg}"));
        }
    }
    for (gdn, op) in fanout {
        if let Some(msg) = apply_one_modify(worker, &gdn, vec![op]) {
            failures.push(format!("{gdn}: {msg}"));
        }
    }

    // 3. Re-read the entry synchronously so the form reflects the directory
    // state immediately (before setting status/overlay). This avoids the
    // async install gate clearing the partial-failure message on the next
    // poll iteration.
    reload_form_sync(app, worker, read_flow, entry_dn);

    if failures.is_empty() {
        app.status = "Saved.".to_string();
        // Resume the pending guard intent (focus change / navigation / quit) only
        // on a clean save; on partial failure keep the user on the entry with the
        // error visible.
        if let Some(intent) = then_intent {
            perform_guard_intent(app, worker, read_flow, intent);
        }
    } else {
        app.overlay = Some(Overlay::Error {
            text: format!("Saved with errors:\n- {}", failures.join("\n- ")),
        });
    }
}

/// Base-read a group's current `holder_attr` values (synchronous).
/// Returns `None` on read error or unexpected response (caller treats this
/// conservatively), `Some(members)` on a successful read.
fn read_group_members(
    worker: &WorkerHandle,
    group_dn: &str,
    holder_attr: &str,
) -> Option<Vec<String>> {
    match worker.request(Request::Search {
        id: next_id(),
        base: group_dn.to_string(),
        scope: SearchScope::Base,
        filter: "(objectClass=*)".to_string(),
        attrs: vec![holder_attr.to_string()],
        size_limit: None,
    }) {
        Ok(Response::Entries { entries, .. }) => Some(
            entries
                .into_iter()
                .next()
                .and_then(|e| {
                    e.attrs
                        .into_iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case(holder_attr))
                        .map(|(_, v)| v)
                })
                .unwrap_or_default(),
        ),
        _ => None,
    }
}

/// Apply one MODIFY synchronously; return `Some(human message)` on failure.
fn apply_one_modify(worker: &WorkerHandle, dn: &str, changes: Vec<ModOp>) -> Option<String> {
    match worker.request(Request::Modify {
        id: next_id(),
        dn: dn.to_string(),
        changes,
    }) {
        Ok(Response::WriteOk { .. }) => None,
        Ok(Response::WriteError { msg, .. }) => Some(msg),
        Ok(_) => Some("unexpected response".to_string()),
        Err(e) => Some(e.to_string()),
    }
}

/// Per-holder MODIFYs for a membership change on the candidate's back-ref field.
/// `entry_dn` is the candidate (user) DN written into each holder's `holder_attr`.
/// Added groups get an Add; removed groups get a Delete. Order: adds, then deletes.
fn membership_fanout(
    entry_dn: &str,
    baseline: &[String],
    selected: &[String],
    holder_attr: &str,
) -> Vec<(String, ModOp)> {
    let has = |set: &[String], dn: &str| set.iter().any(|x| x.eq_ignore_ascii_case(dn));
    let mut out = Vec::new();
    for g in selected {
        if !has(baseline, g) {
            out.push((
                g.clone(),
                ModOp::Add {
                    attr: holder_attr.to_string(),
                    values: vec![entry_dn.to_string()],
                },
            ));
        }
    }
    for g in baseline {
        if !has(selected, g) {
            out.push((
                g.clone(),
                ModOp::Delete {
                    attr: holder_attr.to_string(),
                    values: vec![entry_dn.to_string()],
                },
            ));
        }
    }
    out
}

/// True when removing `member` would leave the group with no members (groupOfNames
/// requires ≥1). Only fires when `member` is the SOLE current member. False for
/// empty input (the group is already empty — not our removal's fault).
fn would_empty(current_members: &[String], member: &str) -> bool {
    current_members.len() == 1 && current_members[0].eq_ignore_ascii_case(member)
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

/// Build the eager-[`Structure`] input row for a freshly created entry from its
/// DN and the attributes that were sent (the structure model derives the display
/// label from cn → description → RDN). Pure.
fn structure_input_from_attrs(dn: &str, attrs: &BTreeMap<String, Vec<String>>) -> StructureInput {
    let first = |name: &str| {
        attrs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .and_then(|(_, v)| v.first().cloned())
    };
    let object_classes = attrs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("objectClass"))
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    StructureInput {
        dn: dn.to_string(),
        cn: first("cn"),
        description: first("description"),
        object_classes,
    }
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
                if structure
                    .get(child_dn)
                    .map(|c| c.is_branch())
                    .unwrap_or(false)
                {
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

/// Outcome of planning a create from a Create-mode form (pure).
enum CreatePrep {
    /// Ready to confirm: composed DN, attribute set, container, and LDIF preview.
    Confirm {
        dn: String,
        attrs: BTreeMap<String, Vec<String>>,
        container: String,
        ldif: String,
    },
    /// A blocking problem (RDN missing, schema validation failure).
    Error(String),
}

/// Pure: validate a Create-mode form's edited entry and produce the confirm data.
fn plan_create(
    schema: &SchemaModel,
    profile: &EntryProfile,
    container: &str,
    edited: &EditEntry,
) -> CreatePrep {
    let rdn_value = edited
        .attrs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(&profile.rdn_attr))
        .and_then(|(_, v)| v.first().cloned())
        .unwrap_or_default();
    if rdn_value.trim().is_empty() {
        return CreatePrep::Error("The RDN attribute must have a value.".to_string());
    }
    let (dn, attrs) = build_add_entry(profile, container, rdn_value.trim(), edited);
    let oc_refs: Vec<&str> = profile.object_classes.iter().map(String::as_str).collect();
    let full = EditEntry {
        dn: dn.clone(),
        attrs: attrs.clone(),
    };
    let errors = validate(&full, schema, &oc_refs);
    if !errors.is_empty() {
        return CreatePrep::Error(format_validation_errors(&errors));
    }
    let ldif = render_add(&dn, &attrs);
    CreatePrep::Confirm {
        dn,
        attrs,
        container: container.to_string(),
        ldif,
    }
}

/// Validate a Create-mode pane-3 form and open the create LDIF confirm.
fn prepare_create(
    app: &mut App,
    _worker: &WorkerHandle,
    read_flow: &mut ReadFlow,
    profiles: &[EntryProfile],
) {
    let Some(form) = app.form.as_ref() else {
        return;
    };
    let (profile_idx, container) = match &form.mode {
        FormMode::Create {
            profile_idx,
            container,
        } => (*profile_idx, container.clone()),
        FormMode::Edit => return,
    };
    let Some(profile) = profiles.get(profile_idx) else {
        return;
    };
    let edited = form.to_edit_entry();
    match plan_create(read_flow.schema(), profile, &container, &edited) {
        CreatePrep::Confirm {
            dn,
            attrs,
            container,
            ldif,
        } => {
            app.overlay = Some(Overlay::Confirm {
                title: "Create this entry?".to_string(),
                body: ldif,
                action: PendingAction::Create {
                    dn,
                    attrs,
                    parent: container,
                },
            });
        }
        CreatePrep::Error(text) => {
            app.overlay = Some(Overlay::Error { text });
        }
    }
}

/// Build an empty Create-mode pane-3 form for `profile` (index `profile_idx`),
/// to be added under `container`. Editable fields are forced single-value so the
/// mandatory attributes can be typed inline (a second value is added post-create
/// via the value-editor popup). No relations are attached on create (parity with
/// the previous modal create path).
fn build_new_entry_form(
    schema: &SchemaModel,
    profile: &EntryProfile,
    profile_idx: usize,
    container: String,
) -> EditForm {
    let model = empty_form_for_profile(schema, profile);
    let mut form = build_edit_form(&model, schema, false, &[]);
    for field in &mut form.fields {
        if field.editable {
            field.multi = false;
        }
    }
    form.mode = FormMode::Create {
        profile_idx,
        container,
    };
    form
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::edit_form::FormMode;

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
            picker: None,
            search: TextState::new(),
            scope: None,
            role: None,
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
            relations: vec![],
            picker_search_id: None,
            picker_last_query: String::new(),
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

    /// A bare App (no form) with the given read-only flag, for dispatch tests.
    fn bare_app(read_only: bool) -> App {
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
            status: String::new(),
            relations: vec![],
            picker_search_id: None,
            picker_last_query: String::new(),
        }
    }

    /// Install a one-field form carrying `dn` so F8/delete has a target.
    fn with_form(mut app: App, dn: &str) -> App {
        use crate::schema::FieldKind;
        use crate::ui::edit_form::EditField;
        use crate::ui::form::WidgetSpec;
        app.form = Some(EditForm {
            dn: dn.to_string(),
            fields: vec![EditField {
                label: "cn".to_string(),
                must: true,
                editable: true,
                multi: false,
                secret: false,
                ordered: false,
                values: vec!["x".to_string()],
                kind: FieldKind::Text,
                widget: WidgetSpec::ReadOnlyText,
                editor: TextState::new().with_value("x".to_string()),
                relation: None,
            }],
            baseline: Default::default(),
            mode: FormMode::Edit,
        });
        app
    }

    fn fkey(n: u8) -> KeyEvent {
        KeyEvent::new(KeyCode::F(n), KeyModifiers::NONE)
    }

    /// A minimal empty structure for tests that call `dispatch_key` (structure
    /// is only used when Enter opens the picker; these tests don't exercise that).
    fn empty_structure() -> Structure {
        Structure::build("dc=test", vec![])
    }

    #[test]
    fn f5_refreshes_even_in_read_only() {
        let s = empty_structure();
        assert_eq!(
            dispatch_key(&mut bare_app(true), fkey(5), &s),
            Some(UiAction::Refresh)
        );
        assert_eq!(
            dispatch_key(&mut bare_app(false), fkey(5), &s),
            Some(UiAction::Refresh)
        );
    }

    #[test]
    fn f7_creates_first_profile_when_writable_only() {
        let s = empty_structure();
        assert_eq!(
            dispatch_key(&mut bare_app(false), fkey(7), &s),
            Some(UiAction::NewEntry(0))
        );
        // Read-only mode suppresses create (P4-T4); the key falls through to nav.
        assert_eq!(dispatch_key(&mut bare_app(true), fkey(7), &s), None);
    }

    #[test]
    fn f8_deletes_the_form_entry_when_writable() {
        let s = empty_structure();
        let mut app = with_form(bare_app(false), "cn=Alice,dc=example,dc=org");
        assert_eq!(
            dispatch_key(&mut app, fkey(8), &s),
            Some(UiAction::DeleteEntry(
                "cn=Alice,dc=example,dc=org".to_string()
            ))
        );
        // No form → nothing to delete.
        assert_eq!(dispatch_key(&mut bare_app(false), fkey(8), &s), None);
        // Read-only suppresses delete.
        let mut ro = with_form(bare_app(true), "cn=Alice,dc=example,dc=org");
        assert_eq!(dispatch_key(&mut ro, fkey(8), &s), None);
    }

    #[test]
    fn focus_cycles_both_directions() {
        let s = empty_structure();
        assert_eq!(prev_pane(Pane::Tree), Pane::Form);
        assert_eq!(prev_pane(Pane::Form), Pane::Leaf);
        assert_eq!(prev_pane(Pane::Leaf), Pane::Tree);
        // Tab forward and Shift-Tab back are inverses through dispatch_key.
        let mut app = bare_app(false); // no form → no guard
        dispatch_key(
            &mut app,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            &s,
        );
        assert_eq!(app.focus, Pane::Leaf);
        dispatch_key(
            &mut app,
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE),
            &s,
        );
        assert_eq!(app.focus, Pane::Tree);
        dispatch_key(
            &mut app,
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE),
            &s,
        );
        assert_eq!(app.focus, Pane::Form);
    }

    #[test]
    fn tab_off_a_dirty_form_opens_the_focus_guard() {
        // with_form has a value but an empty baseline → it is dirty.
        let mut app = with_form(bare_app(false), "cn=Alice,dc=example,dc=org");
        app.focus = Pane::Form;
        dispatch_key(
            &mut app,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            &empty_structure(),
        );
        // Focus did NOT move; the guard opened carrying the destination pane.
        assert_eq!(app.focus, Pane::Form);
        assert!(matches!(
            app.overlay,
            Some(Overlay::Guard {
                intent: GuardIntent::Focus(Pane::Tree)
            })
        ));
    }

    #[test]
    fn quit_while_dirty_opens_the_guard_else_quits() {
        let altx = || KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT);
        // Clean (no form) → Alt+X quits immediately.
        let mut clean = bare_app(false);
        dispatch_key(&mut clean, altx(), &empty_structure());
        assert!(clean.should_quit);
        // Dirty form → Alt+X opens the quit guard, does NOT quit yet.
        let mut dirty = with_form(bare_app(false), "cn=Alice,dc=example,dc=org");
        dispatch_key(&mut dirty, altx(), &empty_structure());
        assert!(!dirty.should_quit);
        assert!(matches!(
            dirty.overlay,
            Some(Overlay::Guard {
                intent: GuardIntent::Quit
            })
        ));
    }

    #[test]
    fn guard_key_maps_choices_to_intents() {
        let plain = |c| KeyEvent::new(c, KeyModifiers::NONE);
        let nav_guard = || Overlay::Guard {
            intent: GuardIntent::Nav(Some("cn=next".to_string())),
        };

        // Stay (Cancel): no action; for a Nav intent, last_seen advances to the
        // target so the guard does not re-fire.
        let mut app = bare_app(false);
        app.overlay = Some(nav_guard());
        assert!(guard_key(&mut app, plain(KeyCode::Char('c'))).is_none());
        assert_eq!(app.last_seen_leaf.as_deref(), Some("cn=next"));
        assert!(app.overlay.is_none());

        // Discard → ResolveGuard { save: false }.
        let mut app = bare_app(false);
        app.overlay = Some(nav_guard());
        assert!(matches!(
            guard_key(&mut app, plain(KeyCode::Char('d'))),
            Some(PendingAction::ResolveGuard {
                intent: GuardIntent::Nav(Some(t)),
                save: false,
            }) if t == "cn=next"
        ));

        // Save → ResolveGuard { save: true }.
        let mut app = bare_app(false);
        app.overlay = Some(nav_guard());
        assert!(matches!(
            guard_key(&mut app, plain(KeyCode::Char('s'))),
            Some(PendingAction::ResolveGuard { save: true, .. })
        ));
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
            (
                "Jane".to_string(),
                "uid=jane,ou=users,dc=example,dc=org".to_string()
            )
        );
        assert_eq!(
            compute_rows(&s, "ou=users,dc=example,dc=org", "zzz").len(),
            1
        );
    }

    #[test]
    fn tree_items_contain_only_branches() {
        let s = structure();
        let items = build_tree_items(&s);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].children().len(), 1);
    }

    // ── 4.4 helpers ────────────────────────────────────────────────────────────

    /// App with a one-field `member` form (index 0) and no overlay.
    fn test_app_with_form_field_member() -> App {
        use crate::config::relation::{CandidateScope, RelationRole};
        use crate::schema::FieldKind;
        use crate::ui::edit_form::{EditField, FieldRelation};
        use crate::ui::form::WidgetSpec;
        let scope = CandidateScope {
            base: "ou=people".into(),
            object_classes: vec!["inetOrgPerson".into()],
            search_attrs: vec!["uid".into()],
        };
        let field = EditField {
            label: "member".into(),
            must: false,
            editable: true,
            multi: true,
            secret: false,
            ordered: false,
            values: vec![],
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            editor: TextState::new(),
            relation: Some(FieldRelation {
                role: RelationRole::Holder,
                scope,
            }),
        };
        let mut app = bare_app(false);
        app.form = Some(EditForm {
            dn: "cn=g1,ou=groups".into(),
            fields: vec![field],
            baseline: Default::default(),
            mode: FormMode::Edit,
        });
        app
    }

    /// A ValueEditor in picker mode over field `idx`, empty selection.
    fn make_picker_ve(idx: usize) -> ValueEditor {
        use crate::config::relation::{CandidateScope, RelationRole};
        let scope = CandidateScope {
            base: "ou=people".into(),
            object_classes: vec!["inetOrgPerson".into()],
            search_attrs: vec!["uid".into()],
        };
        ValueEditor {
            field: idx,
            label: "member".into(),
            ordered: false,
            secret: false,
            rows: vec![],
            sel: 0,
            picker: Some(crate::ui::picker::PickerState::new(vec![])),
            search: TextState::new(),
            scope: Some(scope),
            role: Some(RelationRole::Holder),
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn picker_space_toggles_and_f2_commits_dns() {
        use crate::ui::picker::Candidate;
        let mut app = test_app_with_form_field_member();
        let mut ve = make_picker_ve(0);
        ve.picker.as_mut().unwrap().set_results(vec![Candidate {
            dn: "uid=a,ou=people".into(),
            label: "a".into(),
        }]);
        app.overlay = Some(Overlay::ValueEditor(ve));
        // Space toggles the cursor row (a) into the selection.
        value_editor_key(&mut app, key(KeyCode::Char(' ')));
        // F2 commits the selected DNs into the field.
        value_editor_key(&mut app, key(KeyCode::F(2)));
        let f = &app.form.as_ref().unwrap().fields[0];
        assert_eq!(f.values, vec!["uid=a,ou=people".to_string()]);
        assert!(app.overlay.is_none());
    }

    #[test]
    fn fanout_adds_and_removes_per_group() {
        let out = membership_fanout(
            "uid=ann,ou=people",
            &["cn=g1,ou=groups".to_string(), "cn=g2,ou=groups".to_string()], // baseline groups
            &["cn=g2,ou=groups".to_string(), "cn=g3,ou=groups".to_string()], // new selection
            "member",
        );
        // g3 gains ann; g1 loses ann; g2 unchanged.
        assert_eq!(
            out,
            vec![
                (
                    "cn=g3,ou=groups".to_string(),
                    ModOp::Add {
                        attr: "member".into(),
                        values: vec!["uid=ann,ou=people".into()]
                    }
                ),
                (
                    "cn=g1,ou=groups".to_string(),
                    ModOp::Delete {
                        attr: "member".into(),
                        values: vec!["uid=ann,ou=people".into()]
                    }
                ),
            ]
        );
    }

    #[test]
    fn fanout_is_case_insensitive_on_dns() {
        let out = membership_fanout(
            "uid=ann,ou=people",
            &["CN=G1,OU=GROUPS".into()],
            &["cn=g1,ou=groups".into()],
            "member",
        );
        assert!(
            out.is_empty(),
            "same DN in different case must not produce add/delete"
        );
    }

    #[test]
    fn would_empty_only_when_sole_member() {
        assert!(would_empty(
            &["uid=ann,ou=people".to_string()],
            "uid=ann,ou=people"
        ));
        assert!(!would_empty(
            &[
                "uid=ann,ou=people".to_string(),
                "uid=bob,ou=people".to_string()
            ],
            "uid=ann,ou=people"
        ));
        // Already empty: not our removal's fault.
        assert!(!would_empty(&[], "uid=ann,ou=people"));
    }

    // ── 5.3 helpers ────────────────────────────────────────────────────────────

    use crate::ldap::worker::RawSubschema;
    use crate::schema::FieldKind;
    use crate::ui::edit_form::{EditField, FieldRelation};
    use crate::ui::form::WidgetSpec;

    /// Minimal schema for user (inetOrgPerson-like) with uid, description, memberOf.
    fn user_schema() -> SchemaModel {
        let raw = RawSubschema {
            object_classes: vec![
                // No SUP top so validate does not require objectClass in the entry.
                "( 1.2.3.4 NAME 'testUser' STRUCTURAL MUST uid MAY ( description $ memberOf ) )".to_string(),
            ],
            attribute_types: vec![
                "( 0.9.2342.19200300.100.1.1 NAME 'uid' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 SINGLE-VALUE )".to_string(),
                "( 2.5.4.13 NAME 'description' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".to_string(),
                "( 1.2.840.113556.1.2.102 NAME 'memberOf' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".to_string(),
            ],
            ldap_syntaxes: vec![],
        };
        SchemaModel::from_raw(&raw)
    }

    fn create_user_profile() -> EntryProfile {
        EntryProfile {
            name: "User".into(),
            object_classes: vec!["testUser".into()],
            rdn_attr: "uid".into(),
            search_base: "ou=people,dc=example,dc=org".into(),
            show: vec!["uid".into()],
            search_attrs: vec![],
            defaults: Default::default(),
        }
    }

    #[test]
    fn build_new_entry_form_is_create_mode_and_single_value() {
        let form = build_new_entry_form(
            &user_schema(),
            &create_user_profile(),
            0,
            "ou=people,dc=example,dc=org".to_string(),
        );
        assert!(form.is_new());
        match &form.mode {
            FormMode::Create {
                profile_idx,
                container,
            } => {
                assert_eq!(*profile_idx, 0);
                assert_eq!(container, "ou=people,dc=example,dc=org");
            }
            _ => panic!("expected Create mode"),
        }
        // every editable field is forced single-value for inline create
        assert!(form.fields.iter().all(|f| !(f.editable && f.multi)));
    }

    /// Resolved relations for the user schema: memberOf ↔ group.member.
    fn user_relations() -> Vec<ResolvedRelation> {
        use crate::config::relation::CandidateScope;
        vec![ResolvedRelation {
            name: "group-membership".into(),
            holder_oc: "groupOfNames".into(),
            holder_attr: "member".into(),
            candidate_oc: "testUser".into(),
            back_attr: "memberOf".into(),
            candidate_scope: CandidateScope {
                base: "ou=people,dc=x".into(),
                object_classes: vec!["testUser".into()],
                search_attrs: vec!["uid".into()],
            },
            holder_scope: CandidateScope {
                base: "ou=groups,dc=x".into(),
                object_classes: vec!["groupOfNames".into()],
                search_attrs: vec!["cn".into()],
            },
        }]
    }

    /// Build a user EditForm with:
    /// - own change: description baseline→["old desc"], values→["new desc"]
    /// - memberOf change: baseline→[g1], values→[g2]
    fn user_form_own_and_memberof_change() -> EditForm {
        use crate::config::relation::CandidateScope;

        let scope = CandidateScope {
            base: "ou=groups,dc=x".into(),
            object_classes: vec!["groupOfNames".into()],
            search_attrs: vec!["cn".into()],
        };

        let uid_field = EditField {
            label: "uid".into(),
            must: true,
            editable: true,
            multi: false,
            secret: false,
            ordered: false,
            values: vec!["ann".into()],
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            editor: TextState::new().with_value("ann".to_string()),
            relation: None,
        };

        let desc_field = EditField {
            label: "description".into(),
            must: false,
            editable: true,
            multi: true,
            secret: false,
            ordered: false,
            values: vec!["new desc".into()],
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            editor: TextState::new(),
            relation: None,
        };

        let memberof_field = EditField {
            label: "memberOf".into(),
            must: false,
            editable: true,
            multi: true,
            secret: false,
            ordered: false,
            values: vec!["cn=g2,ou=groups,dc=x".into()],
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            editor: TextState::new(),
            relation: Some(FieldRelation {
                role: crate::config::relation::RelationRole::BackRef,
                scope: scope.clone(),
            }),
        };

        let mut baseline = BTreeMap::new();
        baseline.insert("objectClass".into(), vec!["testUser".into()]);
        baseline.insert("uid".into(), vec!["ann".into()]);
        baseline.insert("description".into(), vec!["old desc".into()]);
        baseline.insert("memberOf".into(), vec!["cn=g1,ou=groups,dc=x".into()]);

        EditForm {
            dn: "uid=ann,ou=people,dc=x".into(),
            fields: vec![uid_field, desc_field, memberof_field],
            baseline,
            mode: FormMode::Edit,
        }
    }

    /// Build a user EditForm where the RDN attr (uid) is changed AND memberOf changes.
    fn user_form_rename_and_memberof_change() -> EditForm {
        use crate::config::relation::{CandidateScope, RelationRole};

        let scope = CandidateScope {
            base: "ou=groups,dc=x".into(),
            object_classes: vec!["groupOfNames".into()],
            search_attrs: vec!["cn".into()],
        };

        // uid changed from "ann" → "bob" (triggers modrdn in diff)
        let uid_field = EditField {
            label: "uid".into(),
            must: true,
            editable: true,
            multi: false,
            secret: false,
            ordered: false,
            values: vec!["ann".into()],
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            editor: TextState::new().with_value("bob".to_string()),
            relation: None,
        };

        let memberof_field = EditField {
            label: "memberOf".into(),
            must: false,
            editable: true,
            multi: true,
            secret: false,
            ordered: false,
            values: vec!["cn=g2,ou=groups,dc=x".into()],
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            editor: TextState::new(),
            relation: Some(FieldRelation {
                role: RelationRole::BackRef,
                scope,
            }),
        };

        let mut baseline = BTreeMap::new();
        baseline.insert("objectClass".into(), vec!["testUser".into()]);
        baseline.insert("uid".into(), vec!["ann".into()]);
        baseline.insert("memberOf".into(), vec!["cn=g1,ou=groups,dc=x".into()]);

        EditForm {
            dn: "uid=ann,ou=people,dc=x".into(),
            fields: vec![uid_field, memberof_field],
            baseline,
            mode: FormMode::Edit,
        }
    }

    #[test]
    fn plan_combined_save_splits_own_and_fanout() {
        let form = user_form_own_and_memberof_change();
        let schema = user_schema();
        let relations = user_relations();
        let plan = plan_combined_save(&form, &schema, &relations);
        let (own_mods, fanout, _entry_dn) = match plan {
            CombinedPlan::Ready {
                own_mods,
                fanout,
                entry_dn,
                ..
            } => (own_mods, fanout, entry_dn),
            other => panic!("expected Ready, got {:?}", other),
        };
        // own_mods touches description, NOT memberOf.
        assert!(
            own_mods.iter().all(|m| {
                let attr = match m {
                    ModOp::Add { attr, .. }
                    | ModOp::Delete { attr, .. }
                    | ModOp::Replace { attr, .. } => attr,
                };
                !attr.eq_ignore_ascii_case("memberOf")
            }),
            "own_mods must not contain memberOf"
        );
        // fanout: g2 gains the user, g1 loses the user.
        assert_eq!(fanout.len(), 2, "expected 2 fanout ops (add g2, delete g1)");
    }

    #[test]
    fn rename_plus_membership_is_blocked() {
        let form = user_form_rename_and_memberof_change();
        let schema = user_schema();
        let relations = user_relations();
        assert!(
            matches!(
                plan_combined_save(&form, &schema, &relations),
                CombinedPlan::Blocked(_)
            ),
            "rename + membership change must be Blocked"
        );
    }

    #[test]
    fn plan_create_builds_confirm_with_composed_dn() {
        use std::collections::BTreeMap;
        let mut attrs = BTreeMap::new();
        attrs.insert("uid".to_string(), vec!["alice".to_string()]);
        let edited = EditEntry {
            dn: String::new(),
            attrs,
        };
        let prep = plan_create(
            &user_schema(),
            &create_user_profile(),
            "ou=people,dc=example,dc=org",
            &edited,
        );
        match prep {
            CreatePrep::Confirm { dn, .. } => {
                assert_eq!(dn, "uid=alice,ou=people,dc=example,dc=org")
            }
            CreatePrep::Error(e) => panic!("unexpected error: {e}"),
        }
    }

    #[test]
    fn plan_create_errors_when_rdn_missing() {
        use std::collections::BTreeMap;
        let edited = EditEntry {
            dn: String::new(),
            attrs: BTreeMap::new(),
        };
        let prep = plan_create(
            &user_schema(),
            &create_user_profile(),
            "ou=people,dc=example,dc=org",
            &edited,
        );
        assert!(matches!(prep, CreatePrep::Error(_)));
    }

    #[test]
    fn should_install_blocks_a_late_read_over_a_create_form() {
        let mut app = bare_app(false);
        app.last_seen_leaf = Some("uid=bob,ou=people,dc=example,dc=org".to_string());
        app.form = Some(build_new_entry_form(
            &user_schema(),
            &create_user_profile(),
            0,
            "ou=people,dc=example,dc=org".to_string(),
        ));
        // A base-read for the prior selection must NOT clobber the create form.
        assert!(!should_install_form(
            &app,
            "uid=bob,ou=people,dc=example,dc=org"
        ));
    }

    #[test]
    fn should_install_allows_matching_edit_form_without_overlay() {
        let mut app = with_form(bare_app(false), "cn=Alice,dc=example,dc=org");
        app.last_seen_leaf = Some("cn=Alice,dc=example,dc=org".to_string());
        assert!(should_install_form(&app, "cn=Alice,dc=example,dc=org"));
        // An open overlay blocks installation.
        app.overlay = Some(Overlay::Error {
            text: "x".to_string(),
        });
        assert!(!should_install_form(&app, "cn=Alice,dc=example,dc=org"));
    }

    #[test]
    fn revert_discards_an_unsaved_create_form() {
        let mut app = bare_app(false);
        app.form = Some(build_new_entry_form(
            &user_schema(),
            &create_user_profile(),
            0,
            "ou=people,dc=example,dc=org".to_string(),
        ));
        revert_form(&mut app);
        assert!(app.form.is_none(), "create form discarded on cancel");
    }

    #[test]
    fn esc_cancels_a_create_form_but_is_a_noop_on_an_edit_form() {
        let s = empty_structure();
        // A create form: Esc requests cancel.
        let mut app = bare_app(false);
        app.focus = Pane::Form;
        app.form = Some(build_new_entry_form(
            &user_schema(),
            &create_user_profile(),
            0,
            "ou=people,dc=example,dc=org".to_string(),
        ));
        assert_eq!(
            dispatch_key(&mut app, key(KeyCode::Esc), &s),
            Some(UiAction::FormCancel)
        );
        // An edit form: Esc does not cancel (F3 reverts instead).
        let mut app2 = with_form(bare_app(false), "cn=Alice,dc=example,dc=org");
        app2.focus = Pane::Form;
        assert_eq!(dispatch_key(&mut app2, key(KeyCode::Esc), &s), None);
    }
}
