//! edaptor CLI. With no subcommand it launches the TUI (browse + read + write);
//! the `check` / `schema` subcommands keep the M1/M2 headless pipelines.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use edaptor::config::Config;
use edaptor::form::changeset::{diff, EditEntry, ModOp};
use edaptor::form::validate::{plan_save, validate, SavePlan};
use edaptor::ldap::ldif::render_changeset;
use edaptor::ldap::worker::{Request, WorkerHandle};
use edaptor::schema::SchemaModel;
use edaptor::ui::form::FormModel;
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

/// Launch the three-pane ratatui TUI.
///
/// P0: an empty three-pane shell — it initialises the terminal, draws the
/// panes, cycles focus on F6/Tab, and quits on `q` / `Alt+X` / `Ctrl+C`. The
/// worker spawn, eager structure scan, read-flow, save/create/delete
/// orchestration and overlays are wired into [`edaptor::ui::app::App`] in the
/// following phases; the app-free helpers below (`prepare_save`,
/// `submit_prepared`, `compute_rows`, …) port verbatim and are reused then.
fn run_tui(_config: Config, _password: String) -> Result<()> {
    edaptor::ui::app::run()?;
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
