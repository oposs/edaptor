//! Dev binary for the in-progress tvision UI (M1-M5a). Deleted at the M5b cutover.
//! Usage: `cargo run -j4 --bin edaptor-tv -- [--config <path>]`
//! With no --config, discovers configs in ~/.config/edaptor and /etc/edaptor and
//! shows the picker if more than one is found. Password from EDAPTOR_TEST_ADMIN_PW
//! (demo: adminpassword).

use std::path::PathBuf;

use anyhow::{anyhow, Result};
use edaptor::config::Config;

/// Parse only the `--config <path>` / `--config=<path>` flag; everything else is
/// ignored. Returns `None` when no flag is present (→ discovery + picker).
fn config_flag<I: IntoIterator<Item = String>>(args: I) -> Option<PathBuf> {
    let mut iter = args.into_iter();
    while let Some(a) = iter.next() {
        if a == "--config" {
            return iter.next().map(PathBuf::from);
        } else if let Some(p) = a.strip_prefix("--config=") {
            return Some(PathBuf::from(p));
        }
    }
    None
}

fn main() -> Result<()> {
    let cli_config = config_flag(std::env::args().skip(1));
    let path = match edaptor::tui::startup::resolve_config_path(cli_config)? {
        Some(p) => p,
        None => return Ok(()), // user cancelled the picker
    };
    let config = Config::load(&path)?;
    let password = std::env::var("EDAPTOR_TEST_ADMIN_PW")
        .map_err(|_| anyhow!("set EDAPTOR_TEST_ADMIN_PW (demo: adminpassword)"))?;
    edaptor::tui::run(config, password)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flag(args: &[&str]) -> Option<String> {
        config_flag(args.iter().map(|s| s.to_string())).map(|p| p.to_string_lossy().into_owned())
    }

    #[test]
    fn flag_with_separate_value() {
        assert_eq!(flag(&["--config", "a.toml"]).as_deref(), Some("a.toml"));
    }

    #[test]
    fn flag_with_equals() {
        assert_eq!(flag(&["--config=foo.toml"]).as_deref(), Some("foo.toml"));
    }

    #[test]
    fn no_flag_is_none() {
        assert_eq!(flag(&[]), None);
        assert_eq!(flag(&["something"]), None);
    }
}
