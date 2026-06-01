//! edaptor CLI. With no subcommand it launches the TUI (browse + read + write);
//! the `check` / `schema` subcommands keep the M1/M2 headless pipelines.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use edaptor::app::{build_menu_defs, LoopEvent, UiAction};
use edaptor::config::Config;
use edaptor::form::changeset::{diff, EditEntry, ModOp};
use edaptor::form::validate::{plan_save, validate, SavePlan};
use edaptor::ldap::ldif::{render_add, render_changeset};
use edaptor::ldap::worker::{Request, Response, WorkerHandle};
use edaptor::schema::SchemaModel;
use edaptor::ui::facade::{self, FormHandles, LeafHandles, Shell};
use edaptor::ui::form::FormModel;
use edaptor::ui::form_state::{guard_decision, GuardOutcome};
use edaptor::workflows::create::{build_add_entry, empty_form_for_profile};
use edaptor::workflows::read_flow::{ReadFlow, ReadOutcome};
use edaptor::workflows::structure::{Structure, StructureInput};
use edaptor::SchemaReport;

#[derive(Parser)]
#[command(name = "edaptor", about = "TUI for editing OpenLDAP directories")]
struct Cli {
    /// Path to the configuration file
    /// (default: $XDG_CONFIG_HOME/edaptor/config.toml or ~/.config/edaptor/config.toml).
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Connect, bind, and print a schema summary.
    Check,
    /// Resolve and print the effective attributes of an object class.
    Schema {
        /// Object class name, e.g. inetOrgPerson
        object_class: String,
    },
    /// Set a synced Unix + Samba password on an entry (TLS-only). Prompts for the
    /// new password twice; updates `userPassword` and, for a `sambaSamAccount`,
    /// `sambaNTPassword` + `sambaPwdLastSet` in one atomic MODIFY.
    Passwd {
        /// Target entry DN, e.g. uid=alice,ou=people,dc=example,dc=org
        dn: String,
    },
}

fn default_config_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join("edaptor/config.toml");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config/edaptor/config.toml")
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config_path = cli.config.clone().unwrap_or_else(default_config_path);
    let config = Config::load(&config_path)?;
    let password = config
        .auth
        .password_source
        .resolve()
        .context("resolving bind password")?;

    match cli.command {
        None => run_tui(config, password)?,
        Some(Command::Check) => {
            let summary = edaptor::run_check(config, password)?;
            println!("Connected to {}", summary.uri);
            if let Some(dn) = &summary.bind_dn {
                println!("Bound as {dn}");
            }
            println!(
                "Subschema: {} objectClasses, {} attributeTypes, {} ldapSyntaxes",
                summary.object_class_count, summary.attribute_type_count, summary.ldap_syntax_count
            );
        }
        Some(Command::Schema { object_class }) => {
            let report: SchemaReport = edaptor::run_schema(config, password, &object_class)?;
            print_schema(&report);
        }
        Some(Command::Passwd { dn }) => {
            let new_password = prompt_new_password()?;
            let confirmation = edaptor::run_passwd(config, password, &dn, &new_password)?;
            println!("{confirmation}");
        }
    }
    Ok(())
}

/// Prompt for the new password twice (no echo) and confirm the two entries match.
/// Errors if they differ, so a typo never silently sets a wrong password.
fn prompt_new_password() -> Result<String> {
    let first = rpassword::prompt_password("New password: ").context("reading new password")?;
    let second = rpassword::prompt_password("Retype new password: ")
        .context("reading password confirmation")?;
    if first != second {
        return Err(anyhow::anyhow!("the two passwords do not match"));
    }
    Ok(first)
}

/// Launch the three-pane TUI (M6): spawn the worker, fetch the schema and the
/// whole DIT structure eagerly, build the frameless SplitContainer (branch tree |
/// leaf list + search | live edit form), and run the manual loop. A single
/// `on_event` callback owns the structure / read-flow / write / pane state.
///
/// The loop drives panes 2 and 3 through shared `Rc<RefCell>` handles + refresh
/// broadcasts (the panes publish their selection / dirty / live edit back), and
/// reads the tree selection through the Shell's published handle (surfaced as
/// `UiAction::Activate`). On a branch selection it recomputes pane-2 rows; on a
/// leaf selection it runs the dirty guard then base-reads the entry into the form;
/// Save validates + diffs + writes; create stays on the modal dialog; delete and
/// create reflow the eager [`Structure`]; Refresh re-runs the scan.
///
/// All network I/O happens on the worker thread; the loop never blocks on it
/// (except the synchronous startup + Refresh scans). No `turbo_vision` type is
/// named in this module — the facade keeps the boundary.
///
/// `too_many_lines` is allowed: this is the single wiring point where the event
/// closure must inline every `app`-touching call (the facade boundary forbids
/// passing `Application` to a helper). The pure decision logic it calls
/// (`validate`/`diff`/`plan_save`/`guard_decision`/`Structure` ops) is factored
/// out and unit-tested.
#[allow(clippy::too_many_lines)]
fn run_tui(config: Config, password: String) -> Result<()> {
    let base_dn = config.server.base_dn.clone();
    let profiles = config.profiles.clone();
    let read_only = config.is_read_only();
    let menu_defs = build_menu_defs(&profiles);

    // Spawn the worker; fetch schema + the eager structure up front (sync startup).
    let worker = WorkerHandle::spawn(config, password)?;
    let raw = match worker.request(Request::FetchSubschema)? {
        Response::Subschema(raw) => raw,
        Response::Error(e) => return Err(anyhow::anyhow!(e)),
        _ => return Err(anyhow::anyhow!("unexpected response to FetchSubschema")),
    };
    let schema = SchemaModel::from_raw(&raw);

    // Eager full-structure paged scan. On a paging-limit truncation or error we
    // fall back to a root-only structure (the worker discards partial nodes on a
    // limit, so true lazy fallback is a documented scope cut) and note it.
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
        Response::Error(e) => return Err(anyhow::anyhow!(e)),
        _ => return Err(anyhow::anyhow!("unexpected response to LoadStructure")),
    };
    let mut structure = Structure::build(&base_dn, structure_inputs(nodes));

    let mut read_flow = ReadFlow::new(schema);

    // Shared pane handles (broadcast-push): the loop writes rows/model + broadcasts
    // a refresh; the panes publish selection / dirty / edit / dn back.
    let leaf = LeafHandles::default();
    let form = FormHandles::default();

    // Pane-1 branch tree (Rc shared with the mounted DitOutline; kept for rebuilds).
    let root = facade::build_structure_tree(&structure);

    // Seed pane 2 with the root branch's rows before mounting so it is non-empty on
    // the first frame (LeafListPane::new rebuilds from this handle).
    let mut current_branch = structure.root_dn().to_string();
    let mut last_search = String::new();
    *leaf.rows.borrow_mut() = compute_rows(&structure, &current_branch, &last_search);

    let mut shell = Shell::new(&menu_defs)?;
    shell.mount_split(root.clone(), read_only, leaf.clone(), form.clone());

    // Loop state.
    let mut last_seen_leaf: Option<String> = None;
    let mut current_form: Option<(FormModel, Vec<String>)> = None;
    let mut pending_followups: HashMap<u64, (String, Vec<ModOp>)> = HashMap::new();
    let mut post: HashMap<u64, PostWrite> = HashMap::new();

    let profile_count = profiles.len();
    shell.run_loop(
        |app, event| match event {
            LoopEvent::Idle => {
                // 1) Drain worker responses (write results, then read forms).
                while let Some(resp) = worker.poll() {
                    match &resp {
                        Response::WriteOk { id, dn: _ } => {
                            if let Some((new_dn, mods)) = pending_followups.remove(id) {
                                // Rename done: apply deferred mods to the new DN and
                                // re-read it into the form.
                                let _ = worker.submit(Request::Modify {
                                    id: next_id(),
                                    dn: new_dn.clone(),
                                    changes: mods,
                                });
                                let _ = read_flow.request_entry(&worker, &new_dn, None);
                                continue;
                            }
                            if let Some(pw) = post.remove(id) {
                                match pw {
                                    PostWrite::Save { reread_dn, nav } => {
                                        facade::info(app, "Saved.");
                                        let target = nav.unwrap_or(reread_dn);
                                        let _ = read_flow.request_entry(&worker, &target, None);
                                        *leaf.rows.borrow_mut() =
                                            compute_rows(&structure, &current_branch, &last_search);
                                        facade::refresh_leaf(app);
                                    }
                                    PostWrite::Created { parent, input } => {
                                        facade::info(app, "Created.");
                                        if structure.add_child(&parent, input) {
                                            facade::rebuild_structure_tree(&root, &structure);
                                            facade::refresh_tree(app);
                                        }
                                        *leaf.rows.borrow_mut() =
                                            compute_rows(&structure, &current_branch, &last_search);
                                        facade::refresh_leaf(app);
                                    }
                                    PostWrite::Deleted { dn } => {
                                        facade::info(app, "Deleted.");
                                        let was_branch = structure
                                            .get(&dn)
                                            .map(|n| n.is_branch())
                                            .unwrap_or(false);
                                        let demoted = structure.remove(&dn);
                                        if was_branch || demoted {
                                            facade::rebuild_structure_tree(&root, &structure);
                                            facade::refresh_tree(app);
                                        }
                                        if *form.dn.borrow() == dn {
                                            *form.model.borrow_mut() = None;
                                            current_form = None;
                                            facade::refresh_form(app);
                                        }
                                        *leaf.rows.borrow_mut() =
                                            compute_rows(&structure, &current_branch, &last_search);
                                        facade::refresh_leaf(app);
                                        last_seen_leaf = None;
                                    }
                                }
                                continue;
                            }
                            // Untracked write (e.g. a rename's follow-up MODIFY).
                            facade::info(app, "Saved.");
                            continue;
                        }
                        Response::WriteError { msg, .. } => {
                            facade::confirm_error(app, msg);
                            continue;
                        }
                        _ => {}
                    }
                    // Read-flow result → drive the form pane.
                    match read_flow.on_response(&resp) {
                        ReadOutcome::Form {
                            model,
                            object_classes,
                        } => {
                            current_form = Some((model.clone(), object_classes));
                            *form.model.borrow_mut() = Some(model);
                            facade::refresh_form(app);
                        }
                        ReadOutcome::Error(msg) => facade::confirm_error(app, &msg),
                        ReadOutcome::Ignored => {}
                    }
                }

                // 2) Search box changed → recompute pane-2 rows.
                let search = leaf.search.borrow().clone();
                if search != last_search {
                    last_search = search;
                    *leaf.rows.borrow_mut() =
                        compute_rows(&structure, &current_branch, &last_search);
                    facade::refresh_leaf(app);
                }

                // 3) Leaf selection changed → dirty guard → navigate the form.
                let sel = leaf.selected.borrow().clone();
                if sel != last_seen_leaf {
                    let dirty = *form.dirty.borrow();
                    let choice = if dirty {
                        Some(facade::confirm_guard(app))
                    } else {
                        None
                    };
                    match guard_decision(dirty, choice) {
                        GuardOutcome::Cancel => {
                            // Stay: keep editing. Advance last_seen so the guard does
                            // not re-fire every tick (highlight/form may now differ —
                            // a known T11 wrinkle).
                            last_seen_leaf = sel;
                        }
                        GuardOutcome::Proceed => {
                            last_seen_leaf = sel.clone();
                            if navigate_form(&worker, &mut read_flow, &form, &mut current_form, sel)
                            {
                                facade::refresh_form(app);
                            }
                        }
                        GuardOutcome::SaveThenProceed => {
                            last_seen_leaf = sel.clone();
                            let prepared = current_form.clone().zip(form.edit.borrow().clone());
                            if let Some(((model, ocs), edited)) = prepared {
                                match prepare_save(read_flow.schema(), &model, &ocs, &edited) {
                                    PrepareSave::Ready { plan, dn, .. } => submit_prepared(
                                        plan,
                                        &dn,
                                        sel,
                                        &worker,
                                        &mut post,
                                        &mut pending_followups,
                                    ),
                                    PrepareSave::NoChanges => {
                                        if navigate_form(
                                            &worker,
                                            &mut read_flow,
                                            &form,
                                            &mut current_form,
                                            sel,
                                        ) {
                                            facade::refresh_form(app);
                                        }
                                    }
                                    PrepareSave::Invalid(errs) => facade::confirm_error(
                                        app,
                                        &facade::format_validation_errors(&errs),
                                    ),
                                    PrepareSave::DiffError(e) => facade::confirm_error(app, &e),
                                }
                            }
                        }
                    }
                }
            }
            LoopEvent::Action(action) => match action {
                UiAction::Activate { dn, .. } => {
                    // Pane 1 shows branches only; switch pane 2 to the selected one.
                    if structure.get(&dn).is_some() {
                        current_branch = dn;
                        last_search = leaf.search.borrow().clone();
                        *leaf.rows.borrow_mut() =
                            compute_rows(&structure, &current_branch, &last_search);
                        facade::refresh_leaf(app);
                        last_seen_leaf = None;
                    }
                }
                UiAction::FormSave => {
                    let prepared = current_form.clone().zip(form.edit.borrow().clone());
                    if let Some(((model, ocs), edited)) = prepared {
                        match prepare_save(read_flow.schema(), &model, &ocs, &edited) {
                            PrepareSave::Invalid(errs) => {
                                facade::confirm_error(app, &facade::format_validation_errors(&errs))
                            }
                            PrepareSave::DiffError(e) => facade::confirm_error(app, &e),
                            PrepareSave::NoChanges => facade::info(app, "No changes."),
                            PrepareSave::Ready { plan, dn, ldif } => {
                                if facade::confirm(
                                    app,
                                    &format!("Apply these changes to the directory?\n\n{ldif}"),
                                ) {
                                    submit_prepared(
                                        plan,
                                        &dn,
                                        None,
                                        &worker,
                                        &mut post,
                                        &mut pending_followups,
                                    );
                                }
                            }
                        }
                    }
                }
                UiAction::FormCancel => {
                    // Revert by re-pushing the baseline model into the form pane.
                    if let Some((model, _)) = &current_form {
                        *form.model.borrow_mut() = Some(model.clone());
                        facade::refresh_form(app);
                    }
                }
                UiAction::NewEntry(i) => {
                    // Create (ADD) stays on the modal dialog (bounded blast radius):
                    // empty schema-driven form → validate → confirm (LDIF) → submit;
                    // the WriteOk splices the new entry into the eager Structure.
                    if let Some(profile) = profiles.get(i) {
                        let container = if profile.search_base.is_empty() {
                            facade::root_dn(&root)
                        } else {
                            profile.search_base.clone()
                        };
                        let model = empty_form_for_profile(read_flow.schema(), profile);
                        if let Some(edited) = facade::edit_entry_dialog(app, &model) {
                            let oc_refs = [profile.object_class.as_str()];
                            let errors = validate(&edited, read_flow.schema(), &oc_refs);
                            if !errors.is_empty() {
                                facade::confirm_error(
                                    app,
                                    &facade::format_validation_errors(&errors),
                                );
                            } else {
                                let rdn_value = edited
                                    .attrs
                                    .iter()
                                    .find(|(k, _)| k.eq_ignore_ascii_case(&profile.rdn_attr))
                                    .and_then(|(_, v)| v.first().cloned())
                                    .unwrap_or_default();
                                if rdn_value.trim().is_empty() {
                                    facade::confirm_error(
                                        app,
                                        "The RDN attribute must have a value.",
                                    );
                                } else {
                                    let (dn, attrs) = build_add_entry(
                                        profile,
                                        &container,
                                        rdn_value.trim(),
                                        &edited,
                                    );
                                    let ldif = render_add(&dn, &attrs);
                                    if facade::confirm(
                                        app,
                                        &format!("Create this entry?\n\n{ldif}"),
                                    ) {
                                        let id = next_id();
                                        let input = structure_input_from_attrs(&dn, &attrs);
                                        let _ = worker.submit(Request::Add { id, dn, attrs });
                                        post.insert(
                                            id,
                                            PostWrite::Created {
                                                parent: container,
                                                input,
                                            },
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
                UiAction::DeleteEntry(_) => {
                    // Delete the entry shown in pane 3 (the form), not the tree
                    // branch (spec §12 F2). The WriteOk reflows the Structure.
                    let dn = form.dn.borrow().clone();
                    if !dn.is_empty()
                        && facade::confirm(app, &format!("Delete this entry?\n\n{dn}"))
                    {
                        let id = next_id();
                        let _ = worker.submit(Request::Delete { id, dn: dn.clone() });
                        post.insert(id, PostWrite::Deleted { dn });
                    }
                }
                UiAction::Refresh => {
                    // Re-run the eager scan and rebuild the panes (spec §5.9).
                    match worker.request(Request::LoadStructure {
                        id: 0,
                        base: base_dn.clone(),
                        page_size: 500,
                    }) {
                        Ok(Response::StructureEntries { nodes, .. }) => {
                            structure = Structure::build(&base_dn, structure_inputs(nodes));
                            facade::rebuild_structure_tree(&root, &structure);
                            facade::refresh_tree(app);
                            if structure.get(&current_branch).is_none() {
                                current_branch = base_dn.clone();
                            }
                            last_search = leaf.search.borrow().clone();
                            *leaf.rows.borrow_mut() =
                                compute_rows(&structure, &current_branch, &last_search);
                            facade::refresh_leaf(app);
                            last_seen_leaf = None;
                        }
                        Ok(Response::StructureError { msg, .. }) => {
                            facade::confirm_error(app, &msg);
                        }
                        _ => facade::confirm_error(app, "refresh failed"),
                    }
                }
                UiAction::None => {}
            },
        },
        profile_count,
    );
    Ok(())
}

/// What the run-loop should do when a write's `WriteOk` arrives, looked up by the
/// write's correlation id. App-free (names no `turbo_vision` type).
enum PostWrite {
    /// A form save (Modify / RenameOnly): re-read `reread_dn` into the form, unless
    /// `nav` is set (a SaveThenProceed guard) in which case navigate there instead.
    Save {
        /// The DN to re-read into the form when no deferred navigation is pending.
        reread_dn: String,
        /// A deferred navigation target (the entry the user moved to while dirty).
        nav: Option<String>,
    },
    /// A create: splice the new entry into the eager [`Structure`] under `parent`.
    Created {
        /// The container the entry was added under.
        parent: String,
        /// The new entry's structure row.
        input: StructureInput,
    },
    /// A delete: drop `dn` from the [`Structure`].
    Deleted {
        /// The removed entry's DN.
        dn: String,
    },
}

/// The outcome of preparing a form save: either a reason it cannot proceed, or a
/// ready-to-submit [`SavePlan`] plus its LDIF preview. App-free.
enum PrepareSave {
    /// Client-side validation failed.
    Invalid(Vec<edaptor::form::validate::ValidationError>),
    /// The diff could not be computed (e.g. multi-valued RDN).
    DiffError(String),
    /// The edited entry equals the baseline — nothing to do.
    NoChanges,
    /// A ready plan, its target DN, and the LDIF preview of the change.
    Ready {
        /// The save plan to submit.
        plan: SavePlan,
        /// The (old) DN the plan targets.
        dn: String,
        /// LDIF preview text for the confirmation dialog.
        ldif: String,
    },
}

/// The pane-2 rows for `branch` filtered by `search`: a `‹self›` row for the branch
/// entry itself, then its leaf children (label, DN). Pure / app-free.
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

/// Validate + diff the edited entry against its model baseline and, if there is a
/// real change, return a ready [`SavePlan`] with an LDIF preview. App-free, so the
/// app-touching confirm/error display stays inline in the loop.
fn prepare_save(
    schema: &SchemaModel,
    model: &FormModel,
    object_classes: &[String],
    edited: &EditEntry,
) -> PrepareSave {
    let oc_refs: Vec<&str> = object_classes.iter().map(|s| s.as_str()).collect();
    let errors = validate(edited, schema, &oc_refs);
    if !errors.is_empty() {
        return PrepareSave::Invalid(errors);
    }
    let original = edit_entry_from_model(&model.title, model);
    let cs = match diff(&original, edited) {
        Ok(cs) => cs,
        Err(e) => return PrepareSave::DiffError(e.to_string()),
    };
    if cs.is_empty() {
        return PrepareSave::NoChanges;
    }
    let ldif = render_changeset(&cs);
    PrepareSave::Ready {
        plan: plan_save(cs),
        dn: model.title.clone(),
        ldif,
    }
}

/// Submit the worker request(s) for a prepared [`SavePlan`] and record how to react
/// to the resulting `WriteOk` in `post`. A `Rename` with follow-up mods defers them
/// to the rename's `WriteOk` (via `pending_followups`), because the MODIFY must
/// target the post-rename DN. App-free.
fn submit_prepared(
    plan: SavePlan,
    old_dn: &str,
    nav: Option<String>,
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
                    nav,
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
                },
            );
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

/// Build the eager-[`Structure`] input row for a freshly created entry from its
/// DN and the attributes that were sent (label preference cn → description → RDN
/// is applied later by the structure model). App-free.
fn structure_input_from_attrs(
    dn: &str,
    attrs: &std::collections::BTreeMap<String, Vec<String>>,
) -> StructureInput {
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

/// Map the worker's raw structure rows into the pure model's input rows. App-free.
fn structure_inputs(nodes: Vec<edaptor::ldap::worker::StructureNodeRaw>) -> Vec<StructureInput> {
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

/// Navigate the form pane to `sel`: base-read the selected DN into the form, or —
/// when `sel` is `None` (empty leaf list) — clear it. Returns `true` when the form
/// was cleared, so the caller (which holds `app`) can broadcast the form refresh.
/// App-free (names no `turbo_vision` type).
fn navigate_form(
    worker: &WorkerHandle,
    read_flow: &mut ReadFlow,
    form: &FormHandles,
    current_form: &mut Option<(FormModel, Vec<String>)>,
    sel: Option<String>,
) -> bool {
    match sel {
        Some(dn) => {
            let _ = read_flow.request_entry(worker, &dn, None);
            false
        }
        None => {
            *form.model.borrow_mut() = None;
            *current_form = None;
            true
        }
    }
}

/// The parent DN (everything after the first comma), or `None` if `dn` has no
/// parent component.
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

/// Build the `EditEntry` baseline (original values) from a `FormModel`. Mirrors
/// the facade's dialog seeding so the diff is computed against the same baseline.
fn edit_entry_from_model(dn: &str, model: &FormModel) -> EditEntry {
    let mut attrs = std::collections::BTreeMap::new();
    for field in &model.fields {
        attrs.insert(field.label.clone(), field.values.clone());
    }
    EditEntry {
        dn: dn.to_string(),
        attrs,
    }
}

/// Monotonic correlation id for write requests. A process-global counter starting
/// at a high base keeps write ids distinct from read/browse ids (which start at 1
/// and grow slowly) and unique among in-flight writes.
fn next_id() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(1_000_000);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

fn print_schema(report: &SchemaReport) {
    println!(
        "Object class '{}' — {} effective attributes ({} schema parse warnings)",
        report.object_class,
        report.attributes.len(),
        report.parse_warnings
    );
    for a in &report.attributes {
        println!(
            "  {:<28} {:<4} {:?}{}",
            a.name,
            if a.required { "MUST" } else { "MAY" },
            a.kind,
            if a.single_value {
                " (single-valued)"
            } else {
                ""
            }
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_dn_strips_leftmost_component() {
        assert_eq!(
            parent_dn("cn=Alice,ou=people,dc=example,dc=org"),
            Some("ou=people,dc=example,dc=org")
        );
        assert_eq!(parent_dn("dc=org"), None);
    }

    #[test]
    fn compose_renamed_dn_replaces_rdn() {
        assert_eq!(
            compose_renamed_dn("cn=Alice,ou=people,dc=org", "cn=Bob"),
            "cn=Bob,ou=people,dc=org"
        );
    }

    #[test]
    fn next_id_is_monotonic_and_high() {
        let a = next_id();
        let b = next_id();
        assert!(b > a);
        assert!(a >= 1_000_000);
    }
}
