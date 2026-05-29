//! edaptor CLI. With no subcommand it launches the M3 read-only TUI; the
//! `check` / `schema` subcommands keep the M1/M2 headless pipelines.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use edaptor::app::build_menu_defs;
use edaptor::config::Config;
use edaptor::ldap::worker::{Request, Response, WorkerHandle};
use edaptor::schema::SchemaModel;
use edaptor::ui::facade::{self, Shell};
use edaptor::workflows::browser::{BrowserNode, BrowserState};
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

/// Launch the read-only TUI shell: spawn the worker, fetch the schema
/// synchronously, build the profile-derived menu and the DIT browser rooted at
/// the base DN, then run the manual loop. The idle hook drains the worker's
/// non-blocking response channel and routes each response to the browser (child
/// expansion) or the read flow (entry form), surfacing errors via a message box.
///
/// All network I/O happens on the worker thread; the loop never blocks on it.
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
    // The outline view is owned by the shell's desktop in a fuller wiring; for
    // M3 the browser drives data, and the read form is the visible deliverable.
    let _outline = facade::build_outline(root);

    let mut shell = Shell::new(&menu_defs)?;
    shell.run_loop(|app| {
        // Drain every pending worker response this idle tick (non-blocking).
        while let Some(resp) = worker.poll() {
            // Browser child-expansion responses attach to their node.
            if let Some((node, kids)) = browser.on_response(&resp) {
                facade::attach_children(&node, kids);
                continue;
            }
            // Otherwise it may be a read-flow result.
            match read_flow.on_response(&resp) {
                ReadOutcome::Form(model) => facade::show_entry_dialog(app, &model),
                ReadOutcome::Error(msg) => facade::confirm_error(app, &msg),
                ReadOutcome::Ignored => {}
            }
        }
    });
    Ok(())
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
