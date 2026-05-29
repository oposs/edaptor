//! edaptor CLI. M1: connect, bind, print a schema summary, then exit.
//! (The TUI replaces this default action in M3.)

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;

use edaptor::config::Config;

#[derive(Parser)]
#[command(name = "edaptor", about = "TUI for editing OpenLDAP directories")]
struct Cli {
    /// Path to the configuration file
    /// (default: $XDG_CONFIG_HOME/edaptor/config.toml or ~/.config/edaptor/config.toml).
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,
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
    let config_path = cli.config.unwrap_or_else(default_config_path);

    let config = Config::load(&config_path)?;
    let password = config
        .auth
        .password_source
        .resolve()
        .context("resolving bind password")?;

    let summary = edaptor::run_check(config, password)?;

    println!("Connected to {}", summary.uri);
    if let Some(dn) = &summary.bind_dn {
        println!("Bound as {dn}");
    }
    println!(
        "Subschema: {} objectClasses, {} attributeTypes, {} ldapSyntaxes",
        summary.object_class_count, summary.attribute_type_count, summary.ldap_syntax_count
    );
    Ok(())
}
