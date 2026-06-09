//! edaptor CLI. With no subcommand it launches the TUI (browse + read + write);
//! the `check` / `schema` subcommands keep the M1/M2 headless pipelines.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use edaptor::config::Config;
use edaptor::SchemaReport;

#[derive(Parser)]
#[command(name = "edaptor", about = "TUI for editing OpenLDAP directories")]
struct Cli {
    /// Path to the configuration file.
    /// Without this flag, edaptor searches ~/.config/edaptor/ and /etc/edaptor/
    /// for *.toml files. If exactly one is found it is used automatically;
    /// if multiple are found a picker is shown.
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

fn main() -> Result<()> {
    let cli = Cli::parse();
    let Cli { config, command } = cli;
    let config_path: PathBuf = if let Some(p) = config {
        p
    } else {
        let candidates = edaptor::config::discovery::discover_configs();
        match candidates.len() {
            0 => anyhow::bail!(
                "no config found in ~/.config/edaptor/ or /etc/edaptor/; \
                 use --config to specify one"
            ),
            1 => candidates.into_iter().next().unwrap().path,
            _ => match edaptor::ui::config_picker::pick_config(candidates)? {
                Some(p) => p,
                None => return Ok(()),
            },
        }
    };
    let config = Config::load(&config_path)?;
    let password = if config.auth.needs_password() {
        config
            .auth
            .password_source
            .resolve()
            .context("resolving bind password")?
    } else {
        String::new()
    };

    match command {
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

/// Launch the three-pane ratatui TUI. The event loop, state, rendering and the
/// write-path orchestration all live in [`edaptor::ui::app`]; this just hands off
/// the connection details.
fn run_tui(config: Config, password: String) -> Result<()> {
    edaptor::ui::app::run(config, password)
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
