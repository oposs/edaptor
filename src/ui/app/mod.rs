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
use crossterm::event::{self, Event, KeyEventKind};
use tui_prompts::{State, TextState};
use tui_tree_widget::{TreeItem, TreeState};

use crate::app::UiAction;
use crate::config::relation::resolve_pickers;
use crate::config::{Config, EntryProfile};
use crate::form::changeset::{diff, ChangeSet, EditEntry, ModOp};
use crate::form::validate::{plan_save, validate, SavePlan, ValidationError};
use crate::ldap::ldif::{render_add, render_changeset, render_changesets};
use crate::ldap::worker::{Request, Response, SearchScope, WorkerHandle};
use crate::schema::SchemaModel;
use crate::ui::edit_form::{build_edit_form, value_set_eq, EditForm, FormMode};
use crate::ui::picker::PICKER_SEARCH_CAP;
use crate::ui::view;
use crate::workflows::create::{build_add_entry, empty_form_for_profile, profiles_for_container};
use crate::workflows::read_flow::{ReadFlow, ReadOutcome};
use crate::workflows::structure::Structure;

mod input;
mod overlay;
mod structure_view;
#[cfg(test)]
mod test_support;
pub use overlay::{GuardIntent, Overlay, PendingAction};
pub(crate) use overlay::PostWrite;
pub(crate) use structure_view::{
    build_tree_items, compute_rows, label_rule_attrs, label_rules, structure_input_from_attrs,
    structure_inputs, LabelRule,
};
pub(crate) use input::{dispatch_key, membership_candidate_label, overlay_key, service_picker_search};

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
    /// Resolved picker bindings (built once from config profiles). Drives field
    /// population.
    pub pickers: Vec<crate::config::relation::ResolvedPicker>,
    /// Compiled column-2 label rules (built once from `config.profiles`).
    pub(crate) label_rules: Vec<LabelRule>,
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
    let pickers = resolve_pickers(&config.profiles);
    // Compile the per-profile column-2 label rules and the attrs the scan must fetch.
    let rules = label_rules(&profiles);
    let scan_attrs = label_rule_attrs(&rules);

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
        attrs: scan_attrs,
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
    let rows = compute_rows(&structure, &current_branch, "", &rules);
    let mut tree_state = TreeState::default();
    tree_state.open(vec![current_branch.clone()]);
    // Highlight the root node from the start so column 1 always shows a selection.
    tree_state.select(vec![current_branch.clone()]);
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
        pickers,
        label_rules: rules,
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
                profiles,
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
                                base_dn,
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
#[allow(clippy::too_many_arguments)] // central response handler; each arg is needed
fn handle_worker_response(
    app: &mut App,
    resp: &Response,
    worker: &WorkerHandle,
    read_flow: &mut ReadFlow,
    structure: &mut Structure,
    profiles: &[EntryProfile],
    post: &mut HashMap<u64, PostWrite>,
    pending_followups: &mut HashMap<u64, (String, Vec<ModOp>, Option<String>)>,
) {
    match resp {
        // Intercept picker search results before the read-flow routing.
        Response::Entries { id, entries, .. } if app.picker_search_id == Some(*id) => {
            if let Some(Overlay::ValueEditor(ve)) = app.overlay.as_mut() {
                // Whether these results answer an active search term — drives the
                // picker row ordering (matches-first vs selected-first).
                let searching = !ve.search.value().trim().is_empty();
                let binding = ve.binding.as_deref().cloned();
                if let (Some(binding), Some(p)) = (binding, ve.picker.as_mut()) {
                    let label_template = binding.scope.label_template.clone();
                    let results: Vec<crate::ui::picker::Candidate> = entries
                        .iter()
                        .filter_map(|e| {
                            let store_value = match &binding.store {
                                crate::config::relation::StoreKey::Dn => e.dn.clone(),
                                crate::config::relation::StoreKey::Attr(a) => {
                                    crate::ui::picker::pick_value(&e.attrs, a)?
                                }
                            };
                            Some(crate::ui::picker::Candidate {
                                dn: e.dn.clone(),
                                label: membership_candidate_label(
                                    label_template.as_deref(),
                                    &e.dn,
                                    &e.attrs,
                                ),
                                store_value,
                            })
                        })
                        .collect();
                    // Upgrade seeded selection labels (and real DNs for scalar
                    // stores) by matching on the STORE VALUE, not the DN. So a
                    // saved member shows "Bob Baker (bob)" once results arrive.
                    let ci = p.key_ci;
                    for sel in p.selected.iter_mut() {
                        if let Some(r) = results.iter().find(|r| {
                            if ci {
                                r.store_value.eq_ignore_ascii_case(&sel.store_value)
                            } else {
                                r.store_value == sel.store_value
                            }
                        }) {
                            sel.label = r.label.clone();
                            sel.dn = r.dn.clone();
                        }
                    }
                    p.set_results(results);
                    p.search_active = searching;
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
                    // leaf-label staleness self-heals on Refresh (Alt+R).
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
                    app.rows = compute_rows(
                        structure,
                        &app.current_branch,
                        &app.last_search,
                        &app.label_rules,
                    );
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
                    app.rows = compute_rows(
                        structure,
                        &app.current_branch,
                        &app.last_search,
                        &app.label_rules,
                    );
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
                    app.form = Some(build_loaded_form(
                        &model,
                        read_flow.schema(),
                        app.read_only,
                        &app.pickers,
                        profiles,
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
                prepare_create(app, worker, read_flow, profiles, base_dn);
                return;
            }
            // Try the combined membership path first; fall back to the single-entry
            // path when no backref field actually changed. No guard intent here —
            // a plain Alt+S save has nothing to resume afterward.
            if let Some(ov) = combined_save_overlay(form, read_flow.schema(), profiles, None) {
                app.overlay = Some(ov);
                return;
            }
            // Normal single-entry save (folds in any password change when the
            // entry matches a password-profile).
            let prep = match prepare_edit_save(
                form,
                read_flow.schema(),
                profiles,
                now_unix_secs_or_zero(),
            ) {
                Ok(p) => p,
                Err(text) => {
                    app.overlay = Some(Overlay::Error { text });
                    return;
                }
            };
            match prep {
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
        UiAction::NewEntry(i) => open_create_form(app, read_flow, profiles, i, base_dn),
        UiAction::NewEntryChoose => {
            // Offer profiles whose search_base matches the current container;
            // fall back to all profiles so Alt+N always works.
            let mut matches = profiles_for_container(profiles, &app.current_branch);
            if matches.is_empty() {
                matches = (0..profiles.len()).collect();
            }
            match matches.len() {
                0 => {}
                1 => open_create_form(app, read_flow, profiles, matches[0], base_dn),
                _ => {
                    let entries = matches
                        .iter()
                        .map(|&i| (i, profiles[i].name.clone()))
                        .collect();
                    app.overlay = Some(Overlay::ChooseProfile { entries, sel: 0 })
                }
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
        attrs: label_rule_attrs(&app.label_rules),
    }) {
        Ok(Response::StructureEntries { nodes, .. }) => {
            *structure = Structure::build(base_dn, structure_inputs(nodes));
            app.tree_items = build_tree_items(structure);
            if structure.get(&app.current_branch).is_none() {
                app.current_branch = base_dn.to_string();
            }
            app.rows = compute_rows(
                structure,
                &app.current_branch,
                &app.last_search,
                &app.label_rules,
            );
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

/// Revert every field to its baseline (Alt+C cancel): drop multi-value edits and
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
/// a rename, so the leaf label / tree fully re-sync on the next Refresh (Alt+R).
fn rebind_selection(app: &mut App, dn: &str) {
    app.last_seen_leaf = Some(dn.to_string());
    if let Some(row) = app.rows.get_mut(app.leaf_sel) {
        row.1 = dn.to_string();
    }
}

/// Run a confirmed [`PendingAction`] (submits to the worker / navigates).
#[allow(clippy::too_many_arguments)] // central write dispatcher; each arg is needed
fn execute_pending(
    app: &mut App,
    action: PendingAction,
    worker: &WorkerHandle,
    read_flow: &mut ReadFlow,
    profiles: &[EntryProfile],
    base_dn: &str,
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
        PendingAction::OpenCreate { profile_idx } => {
            open_create_form(app, read_flow, profiles, profile_idx, base_dn);
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
                prepare_create(app, worker, read_flow, profiles, base_dn);
                return;
            }
            // A membership-bearing save runs synchronously through CombinedSave;
            // the pending guard intent rides along and is performed on success.
            if let Some(ov) =
                combined_save_overlay(form, read_flow.schema(), profiles, Some(intent.clone()))
            {
                app.overlay = Some(ov);
                return;
            }
            // Normal single-entry save (folds in any password change).
            let prep = match prepare_edit_save(
                form,
                read_flow.schema(),
                profiles,
                now_unix_secs_or_zero(),
            ) {
                Ok(p) => p,
                Err(text) => {
                    app.overlay = Some(Overlay::Error { text });
                    return;
                }
            };
            match prep {
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
                profiles,
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
            app.rows = compute_rows(structure, &app.current_branch, &search, &app.label_rules);
            app.leaf_sel = 0;
            app.last_seen_leaf = None;
        }
    }

    // 2) Search string changed → recompute the rows, keep the selection in range.
    if search != app.last_search {
        app.last_search = search.clone();
        app.rows = compute_rows(structure, &app.current_branch, &search, &app.label_rules);
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
pub(crate) fn guard_if_dirty(app: &mut App, intent: GuardIntent) -> bool {
    if app.form.as_ref().map(|f| f.is_dirty()).unwrap_or(false) {
        app.overlay = Some(Overlay::Guard { intent });
        true
    } else {
        false
    }
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

/// Build an edit form for a loaded entry and, when the entry is an instance of a
/// password-profile, inject the masked password + confirm fields (suppressing the
/// schema's password field), so the password can be changed inline. Skipped in
/// read-only sessions — the injected field is editable. The single edit-form
/// build seam used by the read flow and the post-combined-save reload.
fn build_loaded_form(
    model: &crate::ui::form::FormModel,
    schema: &SchemaModel,
    read_only: bool,
    pickers: &[crate::config::relation::ResolvedPicker],
    profiles: &[EntryProfile],
) -> EditForm {
    let mut form = build_edit_form(model, schema, read_only);
    if !read_only {
        let ocs = object_classes_of(&form);
        if let Some(spec) = profile_for_entry(profiles, &ocs).and_then(|p| p.password.as_ref()) {
            crate::ui::edit_form::inject_password_fields(&mut form, spec);
        }
    }
    // Tag picker-bound fields so Enter opens the unified picker overlay.
    let ocs = object_classes_of(&form);
    crate::ui::edit_form::tag_picker_fields(&mut form, pickers, &ocs, read_only);
    // Final step: order fields after injection/tagging set secret/picker flags.
    crate::ui::edit_form::order_fields(&mut form);
    form
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

/// A copy of `cs` with the values of any `Add`/`Replace` touching a masked
/// attribute replaced by `********`, for the confirm preview — never show a
/// cleartext password or NT hash. `sambaPwdLastSet` is not secret and is left
/// intact (it is not in `mask_attrs`). Pure.
fn mask_changeset_secrets(cs: &ChangeSet, mask_attrs: &[String]) -> ChangeSet {
    let is_masked = |attr: &str| mask_attrs.iter().any(|a| a.eq_ignore_ascii_case(attr));
    let mut out = cs.clone();
    for m in &mut out.mods {
        match m {
            ModOp::Replace { attr, values } | ModOp::Add { attr, values } if is_masked(attr) => {
                *values = vec!["********".to_string()];
            }
            _ => {}
        }
    }
    out
}

/// Validate + diff the edited entry against the `original` (baseline) and, if
/// there is a real change, return a ready [`SavePlan`] with an LDIF preview.
///
/// `password_mods` (REPLACE ops produced by the edit-password path) are folded
/// into the changeset so one source of truth drives both the plan and the
/// preview — a password-only edit (empty attribute diff) is still a change.
/// `mask_attrs` lists the attributes whose values to mask in the preview LDIF.
fn prepare_save(
    schema: &SchemaModel,
    original: &EditEntry,
    edited: &EditEntry,
    object_classes: &[String],
    password_mods: &[ModOp],
    mask_attrs: &[String],
) -> PrepareSave {
    let oc_refs: Vec<&str> = object_classes.iter().map(|s| s.as_str()).collect();
    let errors = validate(edited, schema, &oc_refs);
    if !errors.is_empty() {
        return PrepareSave::Invalid(errors);
    }
    let mut cs = match diff(original, edited) {
        Ok(cs) => cs,
        Err(e) => return PrepareSave::DiffError(e.to_string()),
    };
    cs.mods.extend(password_mods.iter().cloned());
    if cs.is_empty() {
        return PrepareSave::NoChanges;
    }
    let ldif = render_changeset(&mask_changeset_secrets(&cs, mask_attrs));
    PrepareSave::Ready {
        plan: plan_save(cs),
        dn: original.dn.clone(),
        ldif,
    }
}

/// Build the `(original, edited, object_classes)` for a single-entry edit save,
/// fold in any password change when the loaded entry matches a password-profile,
/// and return the resulting [`PrepareSave`]. `Err(text)` signals a confirm
/// mismatch (the caller surfaces it as an Error overlay). `now_secs` is injected
/// so the planning stays testable. Used by both the plain Alt+S save and the
/// guard-resume save so password edits work from either entry point.
fn prepare_edit_save(
    form: &EditForm,
    schema: &SchemaModel,
    profiles: &[EntryProfile],
    now_secs: u64,
) -> Result<PrepareSave, String> {
    // Strip fan-out labels from the baseline so `diff` does not emit a spurious
    // Delete for attrs whose changes drive the per-candidate fan-out save.
    let fanout_lbls = form.fanout_labels();
    let mut original = EditEntry {
        dn: form.dn.clone(),
        attrs: form.baseline.clone(),
    };
    for l in &fanout_lbls {
        original.attrs.remove(l);
    }
    let mut edited = form.to_edit_entry();
    let object_classes = object_classes_of(form);
    let (password_mods, mask_attrs) =
        match profile_for_entry(profiles, &object_classes).and_then(|p| p.password.clone()) {
            Some(spec) => stage_edit_password(
                &spec,
                &object_classes,
                &mut original.attrs,
                &mut edited.attrs,
                now_secs,
            )?,
            None => (Vec::new(), Vec::new()),
        };
    Ok(prepare_save(
        schema,
        &original,
        &edited,
        &object_classes,
        &password_mods,
        &mask_attrs,
    ))
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
    profiles: &[EntryProfile],
    now_secs: u64,
) -> CombinedPlan {
    let fanout = form.fanout_labels();
    if fanout.is_empty() {
        return CombinedPlan::NoMembershipChange;
    }

    // Did any fan-out field actually change its value set?
    let changed = form.fields.iter().any(|f| {
        if !fanout.contains(&f.label) {
            return false;
        }
        let base = form.baseline.get(&f.label).cloned().unwrap_or_default();
        !value_set_eq(&f.current_values(), &base)
    });
    if !changed {
        return CombinedPlan::NoMembershipChange;
    }

    // Own-entry: strip fan-out labels from both sides, validate + diff.
    let object_classes = object_classes_of(form);
    let oc_refs: Vec<&str> = object_classes.iter().map(|s| s.as_str()).collect();
    let mut original = EditEntry {
        dn: form.dn.clone(),
        attrs: form.baseline.clone(),
    };
    let mut edited = form.to_edit_entry(); // already omits fan-out fields
    for l in &fanout {
        original.attrs.remove(l);
        edited.attrs.remove(l);
    }

    // Stage any password change the same way the single-entry path does: strip the
    // injected password pseudo-fields from BOTH sides (so a blank field never diffs
    // to a Delete that would clobber the stored password, and the `(confirm)`
    // pseudo-attribute never leaks), and collect the REPLACE mods to fold into the
    // own-entry MODIFY. A confirm mismatch blocks the whole combined save.
    let (password_mods, mask_attrs) =
        match profile_for_entry(profiles, &object_classes).and_then(|p| p.password.clone()) {
            Some(spec) => match stage_edit_password(
                &spec,
                &object_classes,
                &mut original.attrs,
                &mut edited.attrs,
                now_secs,
            ) {
                Ok(x) => x,
                Err(text) => return CombinedPlan::Blocked(text),
            },
            None => (Vec::new(), Vec::new()),
        };

    let errors = validate(&edited, schema, &oc_refs);
    if !errors.is_empty() {
        return CombinedPlan::Invalid(errors);
    }
    let mut own_cs = match diff(&original, &edited) {
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
    own_cs.mods.extend(password_mods);

    // Fan-out: one set of Add/Delete MODIFYs per fan-out field that changed.
    let mut fanout_ops: Vec<(String, ModOp)> = Vec::new();
    let mut preview_sets: Vec<ChangeSet> = Vec::new();
    if !own_cs.is_empty() {
        // Mask the password values in the preview only; `own_mods` keeps the real
        // cleartext/hash for the apply.
        preview_sets.push(mask_changeset_secrets(&own_cs, &mask_attrs));
    }
    for f in form.fields.iter().filter(|f| fanout.contains(&f.label)) {
        let Some(attr) = f.picker.as_ref().and_then(|b| b.fanout_attr.clone()) else {
            continue;
        };
        let base = form.baseline.get(&f.label).cloned().unwrap_or_default();
        let ops = membership_fanout(&form.dn, &base, &f.current_values(), &attr);
        for (gdn, op) in ops {
            preview_sets.push(ChangeSet {
                dn: gdn.clone(),
                modrdn: None,
                mods: vec![op.clone()],
            });
            fanout_ops.push((gdn, op));
        }
    }

    CombinedPlan::Ready {
        entry_dn: form.dn.clone(),
        own_mods: own_cs.mods,
        fanout: fanout_ops,
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
    profiles: &[EntryProfile],
    then_intent: Option<GuardIntent>,
) -> Option<Overlay> {
    match plan_combined_save(form, schema, profiles, now_unix_secs_or_zero()) {
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
fn reload_form_sync(
    app: &mut App,
    worker: &WorkerHandle,
    read_flow: &ReadFlow,
    profiles: &[EntryProfile],
    dn: &str,
) {
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
            app.form = Some(build_loaded_form(
                &model,
                read_flow.schema(),
                app.read_only,
                &app.pickers,
                profiles,
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
#[allow(clippy::too_many_arguments)] // synchronous combined-save dispatcher; each arg is needed
fn apply_combined_save(
    app: &mut App,
    worker: &WorkerHandle,
    read_flow: &mut ReadFlow,
    profiles: &[EntryProfile],
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
    reload_form_sync(app, worker, read_flow, profiles, entry_dn);

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

/// Decide an allocation from a (possibly truncated) directory scan. Refuses when
/// the scan was truncated by a server limit — never allocates over a partial set
/// (a silent duplicate would be worse than a constraint violation).
fn decide_allocation(values: &[u64], truncated: bool, min: u64, max: u64) -> Result<u64, String> {
    if truncated {
        return Err(
            "refusing to allocate: the number scan hit a server size limit \
             (bind with a higher-limit identity or configure a counter)"
                .to_string(),
        );
    }
    crate::config::defaults::next_in_range(values, min, max)
}

/// Allocate the next free numeric `attr` in `[min,max]` by scanning the whole
/// subtree from `base_dn`. Refuses if the scan was truncated (spec D6). Synchronous.
fn allocate_number(
    worker: &WorkerHandle,
    base_dn: &str,
    attr: &str,
    min: u64,
    max: u64,
) -> Result<u64, String> {
    let resp = worker
        .request(Request::Search {
            id: next_id(),
            base: base_dn.to_string(),
            scope: SearchScope::Subtree,
            filter: format!("({attr}=*)"),
            attrs: vec![attr.to_string()],
            size_limit: None,
        })
        .map_err(|e| e.to_string())?;
    let (entries, truncated) = match resp {
        Response::Entries {
            entries, truncated, ..
        } => (entries, truncated),
        Response::SearchError { msg, .. } => return Err(msg),
        _ => return Err("unexpected response while allocating".to_string()),
    };
    let mut values: Vec<u64> = Vec::new();
    for e in &entries {
        if let Some((_, vs)) = e.attrs.iter().find(|(k, _)| k.eq_ignore_ascii_case(attr)) {
            for v in vs {
                if let Ok(n) = v.trim().parse::<u64>() {
                    values.push(n);
                }
            }
        }
    }
    decide_allocation(&values, truncated, min, max)
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
pub(crate) fn next_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1_000_000);
    COUNTER.fetch_add(1, Ordering::Relaxed)
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
/// Extract + validate the password from edited create/edit attrs. Removes BOTH
/// the primary and confirm pseudo-attributes from `attrs` (confirm is never a real
/// attribute). Returns `Ok(None)` when no password was entered, `Ok(Some(pw))` for
/// a confirmed password, `Err` when the two entries disagree. Pure.
fn stage_password(
    spec: &crate::config::PasswordSpec,
    attrs: &mut std::collections::BTreeMap<String, Vec<String>>,
) -> Result<Option<String>, String> {
    let (primary, confirm) = crate::ui::edit_form::password_field_labels(spec);
    let take = |attrs: &std::collections::BTreeMap<String, Vec<String>>, label: &str| {
        attrs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(label))
            .and_then(|(_, v)| v.first().cloned())
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    let pw = take(attrs, &primary);
    let cf = take(attrs, &confirm);
    attrs.retain(|k, _| !k.eq_ignore_ascii_case(&primary) && !k.eq_ignore_ascii_case(&confirm));
    if pw.is_empty() {
        return Ok(None);
    }
    if pw != cf {
        return Err("Passwords do not match.".to_string());
    }
    Ok(Some(pw))
}

/// A copy of `attrs` with the password-related attribute values masked, for the
/// LDIF confirm preview (never show the cleartext or the NT hash). Pure.
fn mask_password_attrs(
    attrs: &std::collections::BTreeMap<String, Vec<String>>,
    ldap_attribute: &str,
) -> std::collections::BTreeMap<String, Vec<String>> {
    let mut out = attrs.clone();
    for key in [ldap_attribute, "sambaNTPassword", "sambaPwdLastSet"] {
        if let Some(k) = out.keys().find(|k| k.eq_ignore_ascii_case(key)).cloned() {
            out.insert(k, vec!["********".to_string()]);
        }
    }
    out
}

/// Wall-clock seconds since the Unix epoch (0 on a pre-epoch clock). The one
/// impure call in the password paths; isolated so the planners stay pure.
fn now_unix_secs_or_zero() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The first profile that satisfies `pred` AND whose (non-empty) object classes
/// are all present (case-insensitive) in `entry_ocs` — i.e. the loaded entry is
/// an instance of that profile. Tie-break: config order (declare the more
/// specific profile first). Shared core of the password/lookup resolvers below;
/// only the `pred` differs, so the object-class subset check stays identical.
/// Pure.
fn profile_for_entry_where<'a>(
    profiles: &'a [EntryProfile],
    entry_ocs: &[String],
    pred: impl Fn(&EntryProfile) -> bool,
) -> Option<&'a EntryProfile> {
    profiles.iter().find(|p| {
        pred(p)
            && !p.object_classes.is_empty()
            && p.object_classes
                .iter()
                .all(|oc| entry_ocs.iter().any(|e| e.eq_ignore_ascii_case(oc)))
    })
}

/// The first configured profile that declares a `[profile.password]` block and
/// whose object classes all match `entry_ocs`. `None` when no password-profile
/// matches. Thin wrapper over [`profile_for_entry_where`]. Pure.
fn profile_for_entry<'a>(
    profiles: &'a [EntryProfile],
    entry_ocs: &[String],
) -> Option<&'a EntryProfile> {
    profile_for_entry_where(profiles, entry_ocs, |p| p.password.is_some())
}

/// Edit-path password mods: the same `(attr, values)` pairs as create
/// (`password_add_attrs`), mapped to REPLACE ops so the new credential overwrites
/// the old within one atomic MODIFY. Honors `ldap_attribute` and Samba. Pure.
fn password_replace_mods(
    clear: &str,
    ldap_attribute: &str,
    samba: bool,
    now_secs: u64,
) -> Vec<ModOp> {
    crate::samba::password::password_add_attrs(clear, ldap_attribute, samba, now_secs)
        .into_iter()
        .map(|(attr, values)| ModOp::Replace { attr, values })
        .collect()
}

/// Compute the password contribution to an edit save. Always strips the password
/// pseudo-fields (primary + confirm) from BOTH `baseline` and `edited`, so the
/// injected masked field never appears as an attribute diff — an un-stripped
/// baseline still carrying the directory's stored hash would otherwise diff to a
/// spurious Delete. When a confirmed new password was entered, also strips the
/// Samba secret attrs from both sides (the REPLACE mods are then their sole
/// source) and returns those mods plus the attrs to mask in the preview. Returns
/// empty vecs when the field was left blank. Pure (clock injected as `now_secs`).
fn stage_edit_password(
    spec: &crate::config::PasswordSpec,
    object_classes: &[String],
    baseline: &mut std::collections::BTreeMap<String, Vec<String>>,
    edited: &mut std::collections::BTreeMap<String, Vec<String>>,
    now_secs: u64,
) -> Result<(Vec<ModOp>, Vec<String>), String> {
    let (primary, confirm) = crate::ui::edit_form::password_field_labels(spec);
    let strip = |m: &mut std::collections::BTreeMap<String, Vec<String>>, labels: &[&str]| {
        m.retain(|k, _| !labels.iter().any(|l| k.eq_ignore_ascii_case(l)));
    };
    // `primary` == spec.ldap_attribute; drop both pseudo-fields from the baseline
    // so the stored value never diffs against the (blank) form field.
    strip(baseline, &[primary.as_str(), confirm.as_str()]);
    // stage_password validates the confirm match and removes both pseudo-fields
    // from `edited`, returning the cleartext (or None when left blank).
    let clear = match stage_password(spec, edited)? {
        Some(pw) => pw,
        None => return Ok((Vec::new(), Vec::new())),
    };
    let samba = spec.samba
        && object_classes
            .iter()
            .any(|o| o.eq_ignore_ascii_case("sambaSamAccount"));
    if samba {
        strip(baseline, &["sambaNTPassword", "sambaPwdLastSet"]);
        strip(edited, &["sambaNTPassword", "sambaPwdLastSet"]);
    }
    let mods = password_replace_mods(&clear, &spec.ldap_attribute, samba, now_secs);
    let mut mask = vec![spec.ldap_attribute.clone()];
    if samba {
        mask.push("sambaNTPassword".to_string());
    }
    Ok((mods, mask))
}

/// Apply literal/template defaults to still-empty fields (pure); return the
/// autonumber requests `(attr, min, max)` that still need a directory scan.
fn apply_static_defaults(
    defaults: &crate::config::defaults::ProfileDefaults,
    attrs: &mut std::collections::BTreeMap<String, Vec<String>>,
) -> Vec<(String, u64, u64)> {
    use crate::config::defaults::{plan_defaults, Resolution};
    let mut autonum = Vec::new();
    for res in plan_defaults(defaults, attrs) {
        match res {
            Resolution::Fill { attr, value } => {
                attrs.insert(attr, vec![value]);
            }
            Resolution::NeedsAutonumber { attr, min, max } => autonum.push((attr, min, max)),
        }
    }
    autonum
}

fn prepare_create(
    app: &mut App,
    worker: &WorkerHandle,
    read_flow: &mut ReadFlow,
    profiles: &[EntryProfile],
    base_dn: &str,
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
    let mut edited = form.to_edit_entry();
    // Fill empty fields from the profile's defaults; autonumber fields need a
    // synchronous directory scan (which may refuse on a truncated result).
    let autonum = apply_static_defaults(&profile.defaults, &mut edited.attrs);
    for (attr, min, max) in autonum {
        match allocate_number(worker, base_dn, &attr, min, max) {
            Ok(n) => {
                edited.attrs.insert(attr, vec![n.to_string()]);
            }
            Err(text) => {
                app.overlay = Some(Overlay::Error { text });
                return;
            }
        }
    }
    // Strip the password + confirm pseudo-fields (validating they match) BEFORE
    // building/validating the entry; the cleartext is injected into the real Add
    // afterwards and masked in the preview.
    let password = match &profile.password {
        Some(spec) => match stage_password(spec, &mut edited.attrs) {
            Ok(pw) => pw,
            Err(text) => {
                app.overlay = Some(Overlay::Error { text });
                return;
            }
        },
        None => None,
    };
    match plan_create(read_flow.schema(), profile, &container, &edited) {
        CreatePrep::Confirm {
            dn,
            mut attrs,
            container,
            ldif,
        } => {
            // Inject the password (cleartext + optional Samba hashes) into the real
            // Add, and mask those values in the preview body.
            let body = match (&profile.password, &password) {
                (Some(spec), Some(cleartext)) => {
                    let samba = spec.samba
                        && attrs
                            .get("objectClass")
                            .map(|ocs| {
                                ocs.iter()
                                    .any(|o| o.eq_ignore_ascii_case("sambaSamAccount"))
                            })
                            .unwrap_or(false);
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    for (k, v) in crate::samba::password::password_add_attrs(
                        cleartext,
                        &spec.ldap_attribute,
                        samba,
                        now,
                    ) {
                        attrs.insert(k, v);
                    }
                    render_add(&dn, &mask_password_attrs(&attrs, &spec.ldap_attribute))
                }
                _ => ldif,
            };
            app.overlay = Some(Overlay::Confirm {
                title: "Create this entry?".to_string(),
                body,
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
    pickers: &[crate::config::relation::ResolvedPicker],
    profile_idx: usize,
    container: String,
) -> EditForm {
    let model = empty_form_for_profile(schema, profile);
    let mut form = build_edit_form(&model, schema, false);
    for field in &mut form.fields {
        if field.editable {
            field.multi = false;
        }
    }
    form.mode = FormMode::Create {
        profile_idx,
        container,
    };
    // When the profile declares a password, replace the schema password field
    // with the masked password + confirm fields.
    if let Some(spec) = &profile.password {
        crate::ui::edit_form::inject_password_fields(&mut form, spec);
    }
    // Tag picker-bound fields so Enter opens the unified picker overlay.
    let ocs = object_classes_of(&form);
    crate::ui::edit_form::tag_picker_fields(&mut form, pickers, &ocs, false);
    // Final step: order fields after injection/tagging set secret/picker flags.
    crate::ui::edit_form::order_fields(&mut form);
    form
}

/// Install a fresh Create-mode form for `profiles[i]` into pane 3 and focus it.
/// The container is the profile's `search_base` (or `base_dn` when empty).
fn open_create_form(
    app: &mut App,
    read_flow: &mut ReadFlow,
    profiles: &[EntryProfile],
    i: usize,
    base_dn: &str,
) {
    let Some(profile) = profiles.get(i) else {
        return;
    };
    let container = if profile.search_base.is_empty() {
        base_dn.to_string()
    } else {
        profile.search_base.clone()
    };
    let form = build_new_entry_form(read_flow.schema(), profile, &app.pickers, i, container);
    app.form = Some(form);
    app.form_focus = 0;
    app.form_scroll = 0;
    app.overlay = None;
    // Focus the form pane so keystrokes edit the new entry's fields.
    app.focus = Pane::Form;
    app.status = format!(
        "New {} — fill fields, Alt+S to create, Esc to cancel.",
        profile.name
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::app::test_support::*;
    use crate::ui::edit_form::FormMode;

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

    use crate::schema::FieldKind;
    use crate::ui::edit_form::EditField;
    use crate::ui::form::WidgetSpec;

    #[test]
    fn build_new_entry_form_is_create_mode_and_single_value() {
        let form = build_new_entry_form(
            &user_schema(),
            &create_user_profile(),
            &[],
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

    /// Build a user EditForm with:
    /// - own change: description baseline→["old desc"], values→["new desc"]
    /// - memberOf change: baseline→[g1], values→[g2]
    fn user_form_own_and_memberof_change() -> EditForm {
        use crate::config::relation::CandidateScope;

        let scope = CandidateScope {
            base: "ou=groups,dc=x".into(),
            object_classes: vec!["groupOfNames".into()],
            search_attrs: vec!["cn".into()],
            label_template: None,
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
            picker: None,
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
            picker: None,
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
            picker: Some(crate::config::relation::PickerBinding {
                attr: "memberOf".into(),
                scope: scope.clone(),
                store: crate::config::relation::StoreKey::Dn,
                select: None,
                fanout_attr: Some("member".into()),
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
        use crate::config::relation::CandidateScope;

        let scope = CandidateScope {
            base: "ou=groups,dc=x".into(),
            object_classes: vec!["groupOfNames".into()],
            search_attrs: vec!["cn".into()],
            label_template: None,
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
            picker: None,
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
            picker: Some(crate::config::relation::PickerBinding {
                attr: "memberOf".into(),
                scope,
                store: crate::config::relation::StoreKey::Dn,
                select: None,
                fanout_attr: Some("member".into()),
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
        let plan = plan_combined_save(&form, &schema, &[], 0);
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
        assert!(
            matches!(
                plan_combined_save(&form, &schema, &[], 0),
                CombinedPlan::Blocked(_)
            ),
            "rename + membership change must be Blocked"
        );
    }

    /// A password-profile entry edited via the combined (membership) save path must
    /// not let the injected password pseudo-fields leak into the own-entry MODIFY:
    /// a BLANK field must never clobber the stored password, and the `(confirm)`
    /// field must never become a real attribute.
    fn pw_user_form_with_memberof_change() -> (EditForm, Vec<EntryProfile>) {
        let mut form = user_form_own_and_memberof_change();
        // The directory returned the stored password hash on the entry.
        form.baseline
            .insert("userPassword".into(), vec!["{SSHA}old".into()]);
        let spec = crate::config::PasswordSpec {
            ldap_attribute: "userPassword".into(),
            samba: false,
        };
        crate::ui::edit_form::inject_password_fields(&mut form, &spec);
        let mut profile = create_user_profile();
        profile.object_classes = vec!["testUser".into()];
        profile.password = Some(spec);
        (form, vec![profile])
    }

    fn own_mods_touch(mods: &[ModOp], attr: &str) -> bool {
        mods.iter().any(|m| {
            let a = match m {
                ModOp::Add { attr, .. }
                | ModOp::Delete { attr, .. }
                | ModOp::Replace { attr, .. } => attr,
            };
            a.eq_ignore_ascii_case(attr)
        })
    }

    #[test]
    fn combined_save_blank_password_does_not_clobber_or_leak() {
        let (form, profiles) = pw_user_form_with_memberof_change();
        // Password fields left blank by the operator.
        match plan_combined_save(&form, &user_schema(), &profiles, 1_700_000_000) {
            CombinedPlan::Ready { own_mods, ldif, .. } => {
                assert!(
                    !own_mods_touch(&own_mods, "userPassword"),
                    "blank password must not emit a userPassword mod (clobber!)"
                );
                assert!(
                    !own_mods_touch(&own_mods, "userPassword (confirm)"),
                    "confirm pseudo-field must never become a real attribute"
                );
                assert!(
                    !ldif.contains("(confirm)"),
                    "confirm field must not leak to preview"
                );
            }
            _ => panic!("expected Ready"),
        }
    }

    #[test]
    fn combined_save_sets_password_as_replace_and_masks_preview() {
        let (mut form, profiles) = pw_user_form_with_memberof_change();
        // Operator typed a new password into both injected fields.
        for f in form.fields.iter_mut() {
            if f.label.eq_ignore_ascii_case("userPassword")
                || f.label.eq_ignore_ascii_case("userPassword (confirm)")
            {
                f.editor = TextState::new().with_value("hunter2".to_string());
            }
        }
        match plan_combined_save(&form, &user_schema(), &profiles, 1_700_000_000) {
            CombinedPlan::Ready { own_mods, ldif, .. } => {
                assert!(
                    own_mods.contains(&ModOp::Replace {
                        attr: "userPassword".into(),
                        values: vec!["hunter2".into()],
                    }),
                    "new password must be a REPLACE in own_mods"
                );
                assert!(
                    !own_mods_touch(&own_mods, "userPassword (confirm)"),
                    "confirm pseudo-field must never become a real attribute"
                );
                assert!(ldif.contains("********"), "preview masks the password");
                assert!(
                    !ldif.contains("hunter2"),
                    "cleartext must not appear in preview"
                );
            }
            _ => panic!("expected Ready"),
        }
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
            &[],
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
            &[],
            0,
            "ou=people,dc=example,dc=org".to_string(),
        ));
        revert_form(&mut app);
        assert!(app.form.is_none(), "create form discarded on cancel");
    }

    #[test]
    fn stage_password_strips_fields_validates_match_and_empty() {
        use crate::config::PasswordSpec;
        use std::collections::BTreeMap;
        let spec = PasswordSpec {
            ldap_attribute: "userPassword".into(),
            samba: false,
        };
        // matching pair → Some, both pseudo-fields stripped, other attrs kept
        let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        attrs.insert("userPassword".into(), vec!["hunter2".into()]);
        attrs.insert("userPassword (confirm)".into(), vec!["hunter2".into()]);
        attrs.insert("cn".into(), vec!["Alice".into()]);
        assert_eq!(
            stage_password(&spec, &mut attrs).unwrap(),
            Some("hunter2".to_string())
        );
        assert!(!attrs.contains_key("userPassword"));
        assert!(!attrs.contains_key("userPassword (confirm)"));
        assert!(attrs.contains_key("cn"));
        // mismatch → Err
        let mut a2: BTreeMap<String, Vec<String>> = BTreeMap::new();
        a2.insert("userPassword".into(), vec!["a".into()]);
        a2.insert("userPassword (confirm)".into(), vec!["b".into()]);
        assert!(stage_password(&spec, &mut a2).is_err());
        // empty → None
        let mut a3: BTreeMap<String, Vec<String>> = BTreeMap::new();
        a3.insert("userPassword".into(), vec!["".into()]);
        a3.insert("userPassword (confirm)".into(), vec!["".into()]);
        assert_eq!(stage_password(&spec, &mut a3).unwrap(), None);
    }

    #[test]
    fn mask_password_attrs_masks_secret_values_only() {
        use std::collections::BTreeMap;
        let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        attrs.insert("userPassword".into(), vec!["hunter2".into()]);
        attrs.insert("sambaNTPassword".into(), vec!["DEADBEEF".into()]);
        attrs.insert("cn".into(), vec!["Alice".into()]);
        let m = mask_password_attrs(&attrs, "userPassword");
        assert_eq!(m.get("userPassword"), Some(&vec!["********".to_string()]));
        assert_eq!(
            m.get("sambaNTPassword"),
            Some(&vec!["********".to_string()])
        );
        assert_eq!(m.get("cn"), Some(&vec!["Alice".to_string()]));
    }

    #[test]
    fn prepare_save_folds_password_mods_and_masks_preview() {
        use std::collections::BTreeMap;
        // No attribute diff (original == edited): a password-only edit is still a
        // change. The real plan carries the cleartext + hash; the preview masks them.
        let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        attrs.insert("uid".into(), vec!["alice".into()]);
        let entry = EditEntry {
            dn: "uid=alice,ou=people,dc=example,dc=org".into(),
            attrs,
        };
        let pw_mods = vec![
            ModOp::Replace {
                attr: "userPassword".into(),
                values: vec!["hunter2".into()],
            },
            ModOp::Replace {
                attr: "sambaNTPassword".into(),
                values: vec!["DEADBEEF".into()],
            },
        ];
        let mask = vec!["userPassword".to_string(), "sambaNTPassword".to_string()];
        match prepare_save(
            &user_schema(),
            &entry,
            &entry,
            &["testUser".to_string()],
            &pw_mods,
            &mask,
        ) {
            PrepareSave::Ready { plan, ldif, .. } => {
                // Preview masks both secrets, never the cleartext or hash.
                assert!(ldif.contains("********"), "preview must mask secrets");
                assert!(!ldif.contains("hunter2"), "cleartext must not appear");
                assert!(!ldif.contains("DEADBEEF"), "NT hash must not appear");
                // The real plan carries the unmasked values.
                match plan {
                    SavePlan::Modify(mods) => {
                        assert!(mods.contains(&ModOp::Replace {
                            attr: "userPassword".into(),
                            values: vec!["hunter2".into()],
                        }));
                        assert!(mods.contains(&ModOp::Replace {
                            attr: "sambaNTPassword".into(),
                            values: vec!["DEADBEEF".into()],
                        }));
                    }
                    _ => panic!("expected Modify"),
                }
            }
            _ => panic!("expected Ready"),
        }
    }

    #[test]
    fn prepare_save_no_password_no_diff_is_no_changes() {
        use std::collections::BTreeMap;
        let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        attrs.insert("uid".into(), vec!["alice".into()]);
        let entry = EditEntry {
            dn: "uid=alice,ou=people,dc=example,dc=org".into(),
            attrs,
        };
        assert!(matches!(
            prepare_save(
                &user_schema(),
                &entry,
                &entry,
                &["testUser".to_string()],
                &[],
                &[]
            ),
            PrepareSave::NoChanges
        ));
    }

    #[test]
    fn profile_for_entry_requires_oc_subset_and_password_spec() {
        use crate::config::PasswordSpec;
        let mut pw_user = create_user_profile();
        pw_user.object_classes = vec!["inetOrgPerson".into(), "posixAccount".into()];
        pw_user.password = Some(PasswordSpec {
            ldap_attribute: "userPassword".into(),
            samba: false,
        });
        // A profile with no password block must never match.
        let mut plain = create_user_profile();
        plain.object_classes = vec!["inetOrgPerson".into()];
        plain.password = None;
        let profiles = vec![plain, pw_user];

        let ocs = vec![
            "top".to_string(),
            "inetOrgPerson".to_string(),
            "posixAccount".to_string(),
        ];
        let m = profile_for_entry(&profiles, &ocs).expect("password profile matches");
        assert!(m.password.is_some());
        assert_eq!(m.object_classes.len(), 2);
        // Entry missing posixAccount: the 2-OC profile no longer matches, and the
        // plain profile has no password → None.
        assert!(profile_for_entry(&profiles, &["inetOrgPerson".to_string()]).is_none());
    }

    #[test]
    fn stage_edit_password_blank_yields_no_mods_and_strips_pseudo_fields() {
        use std::collections::BTreeMap;
        // baseline carries the directory's stored hash; edited carries the blank
        // injected fields. After staging, neither side keeps the password attr.
        let mut baseline: BTreeMap<String, Vec<String>> = BTreeMap::new();
        baseline.insert("userPassword".into(), vec!["{SSHA}deadbeef".into()]);
        baseline.insert("cn".into(), vec!["Alice".into()]);
        let mut edited: BTreeMap<String, Vec<String>> = BTreeMap::new();
        edited.insert("userPassword".into(), vec!["".into()]);
        edited.insert("userPassword (confirm)".into(), vec!["".into()]);
        edited.insert("cn".into(), vec!["Alice".into()]);

        let (mods, mask) = stage_edit_password(
            &pw_spec(false),
            &[],
            &mut baseline,
            &mut edited,
            1_700_000_000,
        )
        .unwrap();
        assert!(mods.is_empty(), "blank password produces no mods");
        assert!(mask.is_empty());
        assert!(
            !baseline.contains_key("userPassword"),
            "baseline hash stripped"
        );
        assert!(!edited.contains_key("userPassword"));
        assert!(!edited.contains_key("userPassword (confirm)"));
        assert!(baseline.contains_key("cn") && edited.contains_key("cn"));
    }

    #[test]
    fn stage_edit_password_set_yields_replace_and_strips_baseline_hash() {
        use std::collections::BTreeMap;
        let mut baseline: BTreeMap<String, Vec<String>> = BTreeMap::new();
        baseline.insert("userPassword".into(), vec!["{SSHA}old".into()]);
        let mut edited: BTreeMap<String, Vec<String>> = BTreeMap::new();
        edited.insert("userPassword".into(), vec!["hunter2".into()]);
        edited.insert("userPassword (confirm)".into(), vec!["hunter2".into()]);

        let (mods, mask) = stage_edit_password(
            &pw_spec(false),
            &[],
            &mut baseline,
            &mut edited,
            1_700_000_000,
        )
        .unwrap();
        assert_eq!(
            mods,
            vec![ModOp::Replace {
                attr: "userPassword".into(),
                values: vec!["hunter2".into()],
            }]
        );
        assert_eq!(mask, vec!["userPassword".to_string()]);
        assert!(!baseline.contains_key("userPassword"), "old hash stripped");
    }

    #[test]
    fn stage_edit_password_samba_includes_nt_hash_and_strips_samba_attrs() {
        use std::collections::BTreeMap;
        let mut baseline: BTreeMap<String, Vec<String>> = BTreeMap::new();
        baseline.insert("sambaNTPassword".into(), vec!["OLDHASH".into()]);
        baseline.insert("sambaPwdLastSet".into(), vec!["1".into()]);
        let mut edited: BTreeMap<String, Vec<String>> = BTreeMap::new();
        edited.insert("userPassword".into(), vec!["hunter2".into()]);
        edited.insert("userPassword (confirm)".into(), vec!["hunter2".into()]);

        let ocs = vec!["sambaSamAccount".to_string()];
        let (mods, mask) = stage_edit_password(
            &pw_spec(true),
            &ocs,
            &mut baseline,
            &mut edited,
            1_700_000_000,
        )
        .unwrap();
        // The NT hash REPLACE is present and equals the M5 nthash of the cleartext.
        assert!(mods.contains(&ModOp::Replace {
            attr: "sambaNTPassword".into(),
            values: vec![crate::samba::nthash::nt_hash("hunter2")],
        }));
        assert!(mask.contains(&"sambaNTPassword".to_string()));
        assert!(
            !baseline.contains_key("sambaNTPassword"),
            "old NT hash stripped"
        );
        assert!(!baseline.contains_key("sambaPwdLastSet"));
    }

    #[test]
    fn stage_edit_password_mismatch_errors() {
        use std::collections::BTreeMap;
        let mut baseline: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut edited: BTreeMap<String, Vec<String>> = BTreeMap::new();
        edited.insert("userPassword".into(), vec!["a".into()]);
        edited.insert("userPassword (confirm)".into(), vec!["b".into()]);
        assert!(
            stage_edit_password(&pw_spec(false), &[], &mut baseline, &mut edited, 0).is_err(),
            "confirm mismatch must error"
        );
    }

    #[test]
    fn apply_static_defaults_fills_literals_templates_and_surfaces_autonumber() {
        use crate::config::defaults::{parse_default_value, DefaultValue, ProfileDefaults};
        use std::collections::BTreeMap;
        let mut d = ProfileDefaults::default();
        d.entries.insert(
            "loginShell".into(),
            DefaultValue::Literal("/bin/bash".into()),
        );
        d.entries.insert(
            "homeDirectory".into(),
            parse_default_value("/home/{uid}").unwrap(),
        );
        d.entries.insert(
            "uidNumber".into(),
            parse_default_value("{next:10000-60000}").unwrap(),
        );
        let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        attrs.insert("uid".into(), vec!["alice".into()]);
        let autonum = apply_static_defaults(&d, &mut attrs);
        assert_eq!(
            attrs.get("loginShell"),
            Some(&vec!["/bin/bash".to_string()])
        );
        assert_eq!(
            attrs.get("homeDirectory"),
            Some(&vec!["/home/alice".to_string()])
        );
        // autonumber is NOT filled here (needs a worker scan); it's surfaced.
        assert_eq!(autonum, vec![("uidNumber".to_string(), 10000, 60000)]);
        assert!(!attrs.contains_key("uidNumber"));
    }

    #[test]
    fn allocation_refuses_on_truncation() {
        assert!(decide_allocation(&[10000], true, 10000, 60000).is_err());
        assert_eq!(
            decide_allocation(&[10000], false, 10000, 60000).unwrap(),
            10001
        );
        assert_eq!(decide_allocation(&[], false, 10000, 60000).unwrap(), 10000);
    }
}
