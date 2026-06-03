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

use crate::config::relation::resolve_pickers;
use crate::config::{Config, EntryProfile};
use crate::form::changeset::{EditEntry, ModOp};
use crate::form::validate::validate;
use crate::ldap::ldif::render_add;
use crate::ldap::worker::{Request, Response, WorkerHandle};
use crate::schema::SchemaModel;
use crate::ui::edit_form::{build_edit_form, EditForm, FormMode};
use crate::ui::picker::PICKER_SEARCH_CAP;
use crate::ui::view;
use crate::workflows::create::{build_add_entry, empty_form_for_profile};
use crate::workflows::read_flow::{ReadFlow, ReadOutcome};
use crate::workflows::structure::Structure;

mod action;
mod input;
mod overlay;
mod save;
mod structure_view;
#[cfg(test)]
mod test_support;
pub use overlay::{GuardIntent, Overlay, PendingAction};
pub(crate) use overlay::PostWrite;
pub(crate) use structure_view::{
    build_tree_items, compute_rows, label_rule_attrs, label_rules, structure_inputs, LabelRule,
};
pub(crate) use input::{dispatch_key, membership_candidate_label, overlay_key, service_picker_search};
pub(crate) use action::{
    build_loaded_form, execute_pending, guard_if_dirty, handle_action, object_classes_of,
    perform_guard_intent, rebind_selection, reconcile, should_install_form,
};
pub(crate) use save::{
    allocate_number, apply_combined_save, combined_save_overlay, format_validation_errors,
    prepare_edit_save, submit_prepared, PrepareSave,
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
pub(crate) fn now_unix_secs_or_zero() -> u64 {
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
pub(crate) fn profile_for_entry<'a>(
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
pub(crate) fn stage_edit_password(
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

pub(crate) fn prepare_create(
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
pub(crate) fn build_new_entry_form(
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
pub(crate) fn open_create_form(
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
    fn next_id_is_monotonic_and_high() {
        let a = next_id();
        let b = next_id();
        assert!(b > a && a >= 1_000_000);
    }

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

}
