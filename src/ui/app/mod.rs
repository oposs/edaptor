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
use crossterm::event::{self, Event, KeyEventKind};
use tui_prompts::{State, TextState};
use tui_tree_widget::{TreeItem, TreeState};

use crate::config::relation::resolve_pickers;
use crate::config::{Config, EntryProfile};
use crate::form::changeset::ModOp;
use crate::ldap::worker::{Request, Response, WorkerHandle};
use crate::schema::SchemaModel;
use crate::ui::edit_form::EditForm;
use crate::ui::picker::PICKER_SEARCH_CAP;
use crate::ui::view;
use crate::workflows::read_flow::{ReadFlow, ReadOutcome};
use crate::workflows::structure::Structure;

mod action;
mod create;
mod input;
mod overlay;
mod save;
mod structure_view;
#[cfg(test)]
mod test_support;
pub(crate) use action::{
    build_loaded_form, guard_if_dirty, object_classes_of, perform_guard_intent, rebind_selection,
    should_install_form,
};
#[cfg(test)]
pub(crate) use create::build_new_entry_form;
pub(crate) use create::{open_create_form, prepare_create};
pub(crate) use input::{
    dispatch_key, membership_candidate_label, overlay_key, service_picker_search,
};
pub(crate) use overlay::PostWrite;
pub use overlay::{GuardIntent, Overlay, PendingAction};
pub(crate) use save::{allocate_number, combined_save_overlay, prepare_edit_save, submit_prepared};
pub(crate) use structure_view::{
    build_tree_items, compute_rows, label_rule_attrs, label_rules, structure_inputs, LabelRule,
};

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

/// Per-tick orchestration receiver (Phase 3). Bundles ONLY the co-mutated state
/// — `app`, the worker handle, the read flow, and the two write-tracking maps —
/// so `&mut self` methods don't conflict on disjoint reads. The read-only /
/// shared resources (`structure`, `profiles`, `base_dn`) stay explicit method
/// params, NOT bundled here, so disjoint reads of them survive `&mut self`. It is
/// reborrowed each tick so the scoped `terminal.draw(&mut App)` borrow is released
/// before orchestration runs (the borrow split, plan §2.1).
pub(crate) struct Ctx<'a> {
    pub(crate) app: &'a mut App,
    pub(crate) worker: &'a WorkerHandle,
    pub(crate) read_flow: &'a mut ReadFlow,
    pub(crate) post: &'a mut HashMap<u64, PostWrite>,
    pub(crate) pending_followups: &'a mut HashMap<u64, (String, Vec<ModOp>, Option<String>)>,
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

        // Per-tick receiver. Reborrow `app`/`read_flow` so the scoped
        // `terminal.draw` borrow above is released first (plan §2.1).
        let mut cx = Ctx {
            app: &mut *app,
            worker,
            read_flow: &mut *read_flow,
            post: &mut post,
            pending_followups: &mut pending_followups,
        };

        // 1) Drain ALL pending worker responses (writes first, then read forms).
        while let Some(resp) = cx.worker.poll() {
            cx.handle_worker_response(&resp, &mut structure, profiles);
        }

        // 2) Poll input with a timeout so the worker drain keeps ticking. An open
        //    overlay captures every key (plan §3.4).
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Release {
                    if cx.app.overlay.is_some() {
                        if let Some(action) = overlay_key(cx.app, key) {
                            cx.execute_pending(action, profiles, base_dn);
                        }
                    } else if let Some(action) = dispatch_key(cx.app, key, &structure) {
                        cx.handle_action(action, &mut structure, profiles, base_dn);
                    }
                }
            }
        }

        // 3) Reconcile UI deltas (no-op while an overlay holds the keys).
        if cx.app.overlay.is_none() {
            cx.reconcile(&structure);
        }

        // 4) Service picker type-ahead (runs regardless of reconcile gate).
        service_picker_search(cx.app, cx.worker);

        if cx.app.should_quit {
            return Ok(());
        }
    }
}

impl Ctx<'_> {
    /// Feed a polled worker [`Response`] to the write-tracking maps and the read
    /// flow. Writes are handled first (re-read after a save); otherwise a built
    /// form is installed (only when its DN matches the current selection — see
    /// below).
    pub(crate) fn handle_worker_response(
        &mut self,
        resp: &Response,
        structure: &mut Structure,
        profiles: &[EntryProfile],
    ) {
        // Reborrow the bundled fields into locals so the body below is the
        // original free-function body verbatim; the disjoint-field reborrows
        // preserve exactly the borrow split the separate params used to give.
        let app = &mut *self.app;
        let worker = self.worker;
        let read_flow = &mut *self.read_flow;
        let post = &mut *self.post;
        let pending_followups = &mut *self.pending_followups;
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
}

/// Monotonic correlation id for write requests, starting at a high base so write
/// ids never collide with the read/browse ids (which start at 1).
pub(crate) fn next_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1_000_000);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_id_is_monotonic_and_high() {
        let a = next_id();
        let b = next_id();
        assert!(b > a && a >= 1_000_000);
    }
}
