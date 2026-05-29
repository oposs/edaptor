//! edaptor CLI. With no subcommand it launches the TUI (browse + read + write);
//! the `check` / `schema` subcommands keep the M1/M2 headless pipelines.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use edaptor::app::{build_menu_defs, LoopEvent, UiAction};
use edaptor::config::Config;
use edaptor::form::changeset::{diff, EditEntry, ModOp, ModRdn};
use edaptor::form::validate::{plan_save, validate, SavePlan};
use edaptor::ldap::ldif::render_add;
use edaptor::ldap::worker::{Request, Response, WorkerHandle};
use edaptor::schema::SchemaModel;
use edaptor::ui::facade::{self, Shell};
use edaptor::ui::form::FormModel;
use edaptor::workflows::browser::{on_select, BrowserNode, BrowserState, SelectAction};
use edaptor::workflows::create::{build_add_entry, empty_form_for_profile};
use edaptor::workflows::read_flow::{ReadFlow, ReadOutcome};
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
    }
    Ok(())
}

/// Launch the TUI: spawn the worker, fetch the schema synchronously, build the
/// profile-derived menu and the DIT browser rooted at the base DN, then run the
/// manual loop. A single `on_event` callback owns the browser / read-flow / write
/// state. On `Idle` it drains the worker channel (browser expansion, read forms,
/// write results); on `Action` it expands/reads (tree) or opens the editable
/// form / delete confirm. After every successful write the affected entry/tree
/// is re-read (spec §10, no silent success).
///
/// All network I/O happens on the worker thread; the loop never blocks on it. No
/// `turbo_vision` type is named in this module — the facade keeps the boundary.
///
/// `too_many_lines` is allowed: this is the single wiring point where the event
/// closure must inline every `app`-touching call (the facade boundary forbids
/// passing `Application` to a helper). The pure decision logic it calls
/// (`validate`/`diff`/`plan_save`/`build_add_entry`) is factored out and tested.
#[allow(clippy::too_many_lines)]
fn run_tui(config: Config, password: String) -> Result<()> {
    let base_dn = config.server.base_dn.clone();
    let profiles = config.profiles.clone();
    let menu_defs = build_menu_defs(&profiles);

    // Spawn the worker and fetch the schema up front (synchronous startup path).
    let worker = WorkerHandle::spawn(config, password)?;
    let raw = match worker.request(Request::FetchSubschema)? {
        Response::Subschema(raw) => raw,
        Response::Error(e) => return Err(anyhow::anyhow!(e)),
        _ => return Err(anyhow::anyhow!("unexpected response to FetchSubschema")),
    };
    let schema = SchemaModel::from_raw(&raw);

    let mut browser: BrowserState<facade::BrowserNodeRef> = BrowserState::new(base_dn.clone());
    let mut read_flow = ReadFlow::new(schema);

    // Root the browser at the base DN and request its children once.
    let root = facade::new_node(BrowserNode {
        dn: base_dn.clone(),
        label: base_dn,
        loaded: false,
        object_classes: Vec::new(),
    });
    browser.request_children(&worker, &root)?;

    let mut shell = Shell::new(&menu_defs)?;
    // Mount the DIT outline as a real Turbo Vision Window on the desktop, so the
    // tree has a frame and gets mouse + keyboard routing (M4.1). The outline
    // shares the node Rc tree with `root`, so we keep `root` to resolve nodes by
    // DN on expansion and to drive the refresh broadcast after lazy load.
    shell.mount_outline(root.clone());

    // After a MODRDN whose changeset also has attribute mods, the follow-up
    // MODIFY must run against the NEW DN once the rename's WriteOk arrives. Track
    // `rename id -> (new dn, deferred mods)`; the idle loop submits the follow-up
    // when it sees the matching WriteOk (D4 correlation).
    let mut pending_followups: HashMap<u64, (String, Vec<ModOp>)> = HashMap::new();

    let profile_count = profiles.len();
    shell.run_loop(
        |app, event| match event {
            LoopEvent::Idle => {
                while let Some(resp) = worker.poll() {
                    // Browser child-expansion responses attach to their node, then
                    // refresh the windowed tree so the new children render (lazy
                    // expand; the broadcast triggers OutlineViewer::rebuild_display).
                    if let Some((node, kids)) = browser.on_response(&resp) {
                        facade::attach_children(&node, kids);
                        facade::refresh_tree(app);
                        continue;
                    }
                    // Write results (spec §10, no silent success).
                    match &resp {
                        Response::WriteOk { id, dn } => {
                            if let Some((new_dn, mods)) = pending_followups.remove(id) {
                                // Rename done — now apply the deferred mods to the
                                // new DN and re-read it.
                                let _ = worker.submit(Request::Modify {
                                    id: next_id(),
                                    dn: new_dn.clone(),
                                    changes: mods,
                                });
                                let _ = read_flow.request_entry(&worker, &new_dn, None);
                                refresh_parent(&worker, &mut browser, &root, &new_dn);
                            } else {
                                facade::info(app, "Saved.");
                                let _ = read_flow.request_entry(&worker, dn, None);
                                refresh_parent(&worker, &mut browser, &root, dn);
                            }
                            continue;
                        }
                        Response::WriteError { msg, .. } => {
                            facade::confirm_error(app, msg);
                            continue;
                        }
                        _ => {}
                    }
                    // Otherwise it may be a read-flow result.
                    match read_flow.on_response(&resp) {
                        ReadOutcome::Form {
                            model,
                            object_classes,
                        } => {
                            // Editable form + save flow (Task 5). Pure decision
                            // logic (validate/diff/plan_save) is unit-tested; the
                            // dialog/message boxes here are tty-only.
                            let Some(edited) = facade::edit_entry_dialog(app, &model) else {
                                continue; // Cancelled.
                            };
                            let oc_refs: Vec<&str> =
                                object_classes.iter().map(|s| s.as_str()).collect();
                            let errors = validate(&edited, read_flow.schema(), &oc_refs);
                            if !errors.is_empty() {
                                facade::confirm_error(
                                    app,
                                    &facade::format_validation_errors(&errors),
                                );
                                continue;
                            }
                            let original = edit_entry_from_model(&model.title, &model);
                            let cs = match diff(&original, &edited) {
                                Ok(cs) => cs,
                                Err(e) => {
                                    facade::confirm_error(app, &format!("{e}"));
                                    continue;
                                }
                            };
                            if cs.is_empty() {
                                facade::info(app, "No changes.");
                                continue;
                            }
                            if !facade::confirm(app, "Apply these changes to the directory?") {
                                continue;
                            }
                            submit_save(
                                plan_save(cs),
                                &model.title,
                                &worker,
                                &mut pending_followups,
                            );
                        }
                        ReadOutcome::Error(msg) => facade::confirm_error(app, &msg),
                        ReadOutcome::Ignored => {}
                    }
                }
            }
            LoopEvent::Action(action) => match action {
                UiAction::Activate { dn, loaded } => match on_select(&dn, loaded) {
                    SelectAction::Expand(target_dn) => {
                        if let Some(node) = facade::find_node(&root, &target_dn) {
                            let _ = browser.request_children(&worker, &node);
                        }
                    }
                    SelectAction::Read(target_dn) => {
                        let _ = read_flow.request_entry(&worker, &target_dn, None);
                    }
                    SelectAction::None => {}
                },
                UiAction::NewEntry(i) => {
                    // Create (ADD): open an empty schema-driven form for the
                    // profile's object class, validate, confirm (with LDIF), submit.
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
                                        let _ = worker.submit(Request::Add {
                                            id: next_id(),
                                            dn,
                                            attrs,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
                UiAction::DeleteEntry(dn) => {
                    // Delete (with confirm, spec §12 F2). The WriteOk handler
                    // re-reads the parent container.
                    if facade::confirm(app, &format!("Delete this entry?\n\n{dn}")) {
                        let _ = worker.submit(Request::Delete { id: next_id(), dn });
                    }
                }
                UiAction::None => {}
            },
        },
        profile_count,
    );
    Ok(())
}

/// Submit the worker request(s) implied by a [`SavePlan`]. A `Rename` defers its
/// attribute mods until the rename's WriteOk (tracked in `pending_followups`),
/// because the follow-up MODIFY must target the post-rename DN. App-free, so it
/// names no `turbo_vision` type.
fn submit_save(
    plan: SavePlan,
    old_dn: &str,
    worker: &WorkerHandle,
    pending_followups: &mut HashMap<u64, (String, Vec<ModOp>)>,
) {
    match plan {
        SavePlan::Nothing => {}
        SavePlan::Modify(mods) => {
            let _ = worker.submit(Request::Modify {
                id: next_id(),
                dn: old_dn.to_string(),
                changes: mods,
            });
        }
        SavePlan::RenameOnly(modrdn) => {
            submit_modrdn(worker, old_dn, modrdn);
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

/// Submit a MODRDN with a fresh correlation id.
fn submit_modrdn(worker: &WorkerHandle, old_dn: &str, modrdn: ModRdn) {
    let _ = worker.submit(Request::ModRdn {
        id: next_id(),
        dn: old_dn.to_string(),
        new_rdn: modrdn.new_rdn,
        delete_old: modrdn.delete_old,
        new_superior: modrdn.new_superior,
    });
}

/// Re-read the parent container of `dn` in the tree after a write (clear + re-
/// request its children) so the change is reflected (Decision D4, re-read). No-op
/// if the parent isn't currently in the tree.
fn refresh_parent(
    worker: &WorkerHandle,
    browser: &mut BrowserState<facade::BrowserNodeRef>,
    root: &facade::BrowserNodeRef,
    dn: &str,
) {
    if let Some(parent) = parent_dn(dn) {
        if let Some(node) = facade::find_node(root, parent) {
            facade::clear_children(&node);
            let _ = browser.request_children(worker, &node);
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
