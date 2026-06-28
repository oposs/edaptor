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
        /// Target username or full DN, e.g. `alice` or
        /// `uid=alice,ou=people,dc=example,dc=org`. A bare username is resolved
        /// against the configured profiles' search bases.
        user: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let Cli { config, command } = cli;
    let config_path: PathBuf = match edaptor::tui::startup::resolve_config_path(config)? {
        Some(p) => p,
        None => return Ok(()), // user cancelled the config picker
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
        Some(Command::Passwd { user }) => {
            let confirmation = edaptor::run_passwd(config, password, &user, |dn| {
                println!("Setting password for {dn}");
                prompt_new_password()
            })?;
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

/// Launch the three-pane tvision TUI. The event loop, state, rendering and the
/// write-path orchestration all live in [`edaptor::tui`]; this just hands off
/// the connection details.
fn run_tui(config: Config, password: String) -> Result<()> {
    edaptor::tui::run(config, password)
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
