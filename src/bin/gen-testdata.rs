//! Generate the edaptor test-directory LDIF (deterministic).
//! Default output: scripts/ldap-provision/data/testdata.ldif

use std::io::Write;

use clap::Parser;
use edaptor::testdata::{generate, to_ldif, GenOpts};

#[derive(Parser)]
#[command(about = "Generate the edaptor test-directory LDIF (deterministic).")]
struct Cli {
    /// Number of users to generate.
    #[arg(long, default_value_t = 600)]
    users: usize,
    /// Output path, or '-' for stdout.
    #[arg(long, default_value = "scripts/ldap-provision/data/testdata.ldif")]
    out: String,
    /// Base DN for all entries.
    #[arg(long, default_value = "dc=example,dc=org")]
    base_dn: String,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let opts = GenOpts {
        users: cli.users,
        base_dn: cli.base_dn,
        ..Default::default()
    };
    let ldif = to_ldif(&generate(&opts), &opts);
    if cli.out == "-" {
        std::io::stdout().write_all(ldif.as_bytes())?;
    } else {
        std::fs::write(&cli.out, &ldif)?;
        eprintln!("wrote {} ({} bytes)", cli.out, ldif.len());
    }
    Ok(())
}
