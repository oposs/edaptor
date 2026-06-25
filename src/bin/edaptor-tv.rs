//! Dev binary for the in-progress tvision UI (M1-M4). Deleted at the M5 cutover.
//! Usage: `cargo run -j4 --bin edaptor-tv -- [--config <path> | <path>]`
//! Config path defaults to examples/demo-config.toml; password from
//! EDAPTOR_TEST_ADMIN_PW (demo: adminpassword).

use std::path::PathBuf;

use anyhow::{anyhow, Result};
use edaptor::config::Config;

/// Resolve the config path from CLI args. Accepts `--config <path>`,
/// `--config=<path>`, or a bare positional path; falls back to the demo config.
/// (Mirrors the main `edaptor` binary's `--config` flag so the same invocation
/// works against either binary.)
fn config_path_from_args<I: IntoIterator<Item = String>>(args: I) -> PathBuf {
    let mut iter = args.into_iter();
    while let Some(a) = iter.next() {
        if a == "--config" {
            if let Some(p) = iter.next() {
                return PathBuf::from(p);
            }
        } else if let Some(p) = a.strip_prefix("--config=") {
            return PathBuf::from(p);
        } else if !a.starts_with('-') {
            return PathBuf::from(a);
        }
    }
    PathBuf::from("examples/demo-config.toml")
}

fn main() -> Result<()> {
    let path = config_path_from_args(std::env::args().skip(1));
    let config = Config::load(&path)?;
    let password = std::env::var("EDAPTOR_TEST_ADMIN_PW")
        .map_err(|_| anyhow!("set EDAPTOR_TEST_ADMIN_PW (demo: adminpassword)"))?;
    edaptor::tui::run(config, password)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> String {
        config_path_from_args(args.iter().map(|s| s.to_string()))
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn flag_with_separate_value() {
        assert_eq!(
            p(&["--config", "examples/demo-config.toml"]),
            "examples/demo-config.toml"
        );
    }

    #[test]
    fn flag_with_equals() {
        assert_eq!(p(&["--config=foo.toml"]), "foo.toml");
    }

    #[test]
    fn bare_positional() {
        assert_eq!(p(&["foo.toml"]), "foo.toml");
    }

    #[test]
    fn default_when_empty() {
        assert_eq!(p(&[]), "examples/demo-config.toml");
    }

    #[test]
    fn flag_takes_precedence_over_trailing_positional_form() {
        // `--config X` consumes X as the value (not treated as a bare positional).
        assert_eq!(p(&["--config", "a.toml"]), "a.toml");
    }
}
