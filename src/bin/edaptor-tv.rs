//! Dev binary for the in-progress tvision UI (M1-M4). Deleted at the M5 cutover.
//! Usage: `cargo run -j4 --bin edaptor-tv -- [config.toml]`
//! Config path defaults to examples/demo-config.toml; password from
//! EDAPTOR_TEST_ADMIN_PW (demo: adminpassword).

use std::path::PathBuf;

use anyhow::{anyhow, Result};
use edaptor::config::Config;

fn main() -> Result<()> {
    let path: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("examples/demo-config.toml"));
    let config = Config::load(&path)?;
    let password = std::env::var("EDAPTOR_TEST_ADMIN_PW")
        .map_err(|_| anyhow!("set EDAPTOR_TEST_ADMIN_PW (demo: adminpassword)"))?;
    edaptor::tui::run(config, password)
}
