# Config Discovery & Startup Picker Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Discover `*.toml` configs in `~/.config/edaptor/` and `/etc/edaptor/`, start silently when exactly one is found, show a ratatui picker when multiple are found, and let config files carry an optional `[meta]` table for display.

**Architecture:** Four tasks in sequence — (1) add `MetaConfig` to the config module, (2) add a `discovery` module that returns `Vec<ConfigCandidate>`, (3) build a standalone ratatui picker in the `ui` module, (4) wire everything together in `main.rs`. No new crate dependencies.

**Tech Stack:** Rust stable, ratatui (already a dependency), crossterm (already a dependency), toml + serde (already dependencies), std::fs::read_dir for directory listing.

---

### Task 1: `MetaConfig` struct and `[meta]` table support

**Files:**
- Modify: `src/config/mod.rs`

- [ ] **Step 1: Add `MetaConfig` struct and wire it into `Config`**

In `src/config/mod.rs`, directly after the `//! Configuration:` doc comment line and before
the `pub mod defaults;` block, the `Config` struct currently starts at line ~16. Add
`MetaConfig` as a new public struct and insert it as an optional field on `Config`:

```rust
// Add this struct anywhere in src/config/mod.rs (before or after other structs — keep it near Config):
#[derive(Debug, Default, Deserialize)]
pub struct MetaConfig {
    pub name: Option<String>,
    pub description: Option<String>,
}
```

Then add a `meta` field to `Config`:

```rust
#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub meta: MetaConfig,           // ← add this line
    pub server: ServerConfig,
    pub auth: AuthConfig,
    #[serde(default, rename = "profile")]
    pub profiles: Vec<EntryProfile>,
    #[serde(default)]
    pub samba: SambaConfig,
    #[serde(default)]
    pub tree: TreeConfig,
}
```

- [ ] **Step 2: Write failing tests for `MetaConfig` parsing**

At the bottom of the `#[cfg(test)]` block in `src/config/mod.rs`, add:

```rust
#[test]
fn meta_config_parses_both_fields() {
    let cfg: Config = toml::from_str(
        r#"
        [meta]
        name        = "carbo-link production"
        description = "dc=carbo-link,dc=com via ldapi"
        [server]
        uri     = "ldap://x"
        base_dn = "dc=x"
        [auth]
        method  = "simple"
        bind_dn = "cn=admin,dc=x"
        "#,
    )
    .unwrap();
    assert_eq!(cfg.meta.name.as_deref(), Some("carbo-link production"));
    assert_eq!(
        cfg.meta.description.as_deref(),
        Some("dc=carbo-link,dc=com via ldapi")
    );
}

#[test]
fn meta_config_absent_gives_none_fields() {
    let cfg: Config = toml::from_str(
        r#"
        [server]
        uri     = "ldap://x"
        base_dn = "dc=x"
        [auth]
        method  = "simple"
        bind_dn = "cn=admin,dc=x"
        "#,
    )
    .unwrap();
    assert!(cfg.meta.name.is_none());
    assert!(cfg.meta.description.is_none());
}

#[test]
fn meta_config_partial_fields_allowed() {
    let cfg: Config = toml::from_str(
        r#"
        [meta]
        name = "only a name"
        [server]
        uri     = "ldap://x"
        base_dn = "dc=x"
        [auth]
        method  = "simple"
        bind_dn = "cn=admin,dc=x"
        "#,
    )
    .unwrap();
    assert_eq!(cfg.meta.name.as_deref(), Some("only a name"));
    assert!(cfg.meta.description.is_none());
}
```

- [ ] **Step 3: Run tests to confirm they fail**

```bash
cargo test -j4 meta_config 2>&1 | grep -E "FAILED|error|ok"
```

Expected: compile error (field `meta` not yet on `Config`) or test failure.

- [ ] **Step 4: Verify the implementation compiles and tests pass**

```bash
cargo test -j4 meta_config 2>&1 | grep -E "FAILED|error|ok"
```

Expected: three lines ending in `ok`.

- [ ] **Step 5: Commit**

```bash
git add src/config/mod.rs
git commit -m "feat(config): add optional [meta] table with name and description"
```

---

### Task 2: Config discovery module

**Files:**
- Create: `src/config/discovery.rs`
- Modify: `src/config/mod.rs` (add `pub mod discovery;`)

- [ ] **Step 1: Create `src/config/discovery.rs` with the public types**

```rust
//! Discovers `*.toml` configs in the user and system config directories.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::MetaConfig;

/// A discovered config file with its parsed `[meta]` section.
#[derive(Debug)]
pub struct ConfigCandidate {
    pub path: PathBuf,
    pub meta: MetaConfig,
}

impl ConfigCandidate {
    /// Display name: `meta.name` if set, otherwise the file stem.
    pub fn display_name(&self) -> String {
        self.meta.name.clone().unwrap_or_else(|| {
            self.path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string()
        })
    }
}

/// Discover configs from the default locations:
///   1. `$XDG_CONFIG_HOME/edaptor/` (or `~/.config/edaptor/`)
///   2. `/etc/edaptor/`
///
/// Within each location files are sorted alphabetically. Results are
/// deduplicated by canonical path. Missing directories are silently skipped.
pub fn discover_configs() -> Vec<ConfigCandidate> {
    let user_dir = user_config_dir();
    let system_dir = PathBuf::from("/etc/edaptor");
    collect_from_dirs(&[&user_dir, &system_dir])
}

fn user_config_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join("edaptor");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config/edaptor")
}

/// Collect and sort candidates from each directory in order, then dedup by
/// canonical path. Extracted for unit testing with temporary directories.
pub(crate) fn collect_from_dirs(dirs: &[&Path]) -> Vec<ConfigCandidate> {
    let mut candidates: Vec<ConfigCandidate> = Vec::new();

    for dir in dirs {
        let mut dir_candidates = toml_files_in(dir);
        dir_candidates.sort_by(|a, b| {
            a.path.file_name().cmp(&b.path.file_name())
        });
        candidates.extend(dir_candidates);
    }

    // Dedup by canonical path (handles symlinks pointing to the same file).
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|c| {
        let key = c.path.canonicalize().unwrap_or_else(|_| c.path.clone());
        seen.insert(key)
    });

    candidates
}

/// Return all `*.toml` files in `dir`. Missing or unreadable directory → empty vec.
fn toml_files_in(dir: &Path) -> Vec<ConfigCandidate> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut result = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("toml") {
            let meta = read_meta(&path);
            result.push(ConfigCandidate { path, meta });
        }
    }
    result
}

/// Attempt to parse just the `[meta]` section from a TOML file.
/// Returns `MetaConfig::default()` on any I/O or parse error (and prints a
/// warning to stderr so the operator knows the file was skipped).
fn read_meta(path: &Path) -> MetaConfig {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("warning: cannot read {}: {e}", path.display());
            return MetaConfig::default();
        }
    };
    #[derive(Deserialize, Default)]
    struct Partial {
        #[serde(default)]
        meta: MetaConfig,
    }
    toml::from_str::<Partial>(&text)
        .map(|p| p.meta)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(dir: &Path, name: &str, body: &str) {
        fs::write(dir.join(name), body).unwrap();
    }

    #[test]
    fn empty_dir_returns_no_candidates() {
        let dir = tempfile::tempdir().unwrap();
        assert!(collect_from_dirs(&[dir.path()]).is_empty());
    }

    #[test]
    fn non_toml_files_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "config.yaml", "");
        write(dir.path(), "README.md", "");
        assert!(collect_from_dirs(&[dir.path()]).is_empty());
    }

    #[test]
    fn files_sorted_alphabetically_within_dir() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "z.toml", "");
        write(dir.path(), "a.toml", "");
        write(dir.path(), "m.toml", "");
        let names: Vec<_> = collect_from_dirs(&[dir.path()])
            .iter()
            .map(|c| c.path.file_name().unwrap().to_str().unwrap().to_string())
            .collect();
        assert_eq!(names, ["a.toml", "m.toml", "z.toml"]);
    }

    #[test]
    fn user_dir_candidates_come_before_system_dir() {
        let user = tempfile::tempdir().unwrap();
        let system = tempfile::tempdir().unwrap();
        write(user.path(), "user.toml", "");
        write(system.path(), "system.toml", "");
        let candidates = collect_from_dirs(&[user.path(), system.path()]);
        assert_eq!(candidates[0].path.file_name().unwrap(), "user.toml");
        assert_eq!(candidates[1].path.file_name().unwrap(), "system.toml");
    }

    #[test]
    fn dedup_removes_symlinked_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "real.toml", "");
        // Point both dirs at the same physical directory — every file appears twice.
        let candidates = collect_from_dirs(&[dir.path(), dir.path()]);
        assert_eq!(candidates.len(), 1);
    }

    #[test]
    fn missing_dir_is_silently_skipped() {
        let missing = PathBuf::from("/tmp/edaptor-does-not-exist-abc123");
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.toml", "");
        let candidates = collect_from_dirs(&[&missing, dir.path()]);
        assert_eq!(candidates.len(), 1);
    }

    #[test]
    fn meta_name_from_file_parsed() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "prod.toml",
            r#"[meta]
name = "production"
description = "the prod server"
"#,
        );
        let candidates = collect_from_dirs(&[dir.path()]);
        assert_eq!(candidates[0].display_name(), "production");
        assert_eq!(
            candidates[0].meta.description.as_deref(),
            Some("the prod server")
        );
    }

    #[test]
    fn display_name_falls_back_to_file_stem() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "my-server.toml", "");
        let candidates = collect_from_dirs(&[dir.path()]);
        assert_eq!(candidates[0].display_name(), "my-server");
    }

    #[test]
    fn unreadable_meta_gives_default() {
        // A file with invalid TOML should not panic, just give empty meta.
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "bad.toml", "[[[[not valid toml");
        let candidates = collect_from_dirs(&[dir.path()]);
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].meta.name.is_none());
    }
}
```

- [ ] **Step 2: Register the module in `src/config/mod.rs`**

Add at the top of `src/config/mod.rs`, alongside the other `pub mod` lines:

```rust
pub mod discovery;
```

- [ ] **Step 3: Run tests**

```bash
cargo test -j4 discovery 2>&1 | grep -E "FAILED|error|ok"
```

Expected: nine `ok` lines, no failures.

- [ ] **Step 4: Commit**

```bash
git add src/config/discovery.rs src/config/mod.rs
git commit -m "feat(config): add config discovery module (XDG + /etc/edaptor)"
```

---

### Task 3: Ratatui config picker

**Files:**
- Create: `src/ui/config_picker.rs`
- Modify: `src/ui/mod.rs` (add `pub mod config_picker;`)

- [ ] **Step 1: Create `src/ui/config_picker.rs`**

```rust
//! Full-screen ratatui picker shown when multiple configs are discovered.

use std::path::PathBuf;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::Alignment;
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::config::discovery::ConfigCandidate;

/// Show a full-screen ratatui picker and return the selected path.
/// Returns `None` if the user presses `q` or `Esc` (caller should exit cleanly).
pub fn pick_config(candidates: Vec<ConfigCandidate>) -> Result<Option<PathBuf>> {
    let mut terminal = ratatui::init();
    let result = run_picker(&mut terminal, &candidates);
    ratatui::restore();
    result
}

fn run_picker(
    terminal: &mut ratatui::DefaultTerminal,
    candidates: &[ConfigCandidate],
) -> Result<Option<PathBuf>> {
    let mut selected = 0usize;
    loop {
        terminal.draw(|f| render(f, candidates, selected))?;
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Release {
                continue;
            }
            match key.code {
                KeyCode::Up => {
                    if selected == 0 {
                        selected = candidates.len() - 1;
                    } else {
                        selected -= 1;
                    }
                }
                KeyCode::Down => {
                    selected = (selected + 1) % candidates.len();
                }
                KeyCode::Enter => {
                    return Ok(Some(candidates[selected].path.clone()));
                }
                KeyCode::Char('q') | KeyCode::Esc => {
                    return Ok(None);
                }
                _ => {}
            }
        }
    }
}

fn render(f: &mut Frame, candidates: &[ConfigCandidate], selected: usize) {
    let area = f.area();

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Select configuration ")
        .title_bottom(
            Line::from(" ↑↓ navigate  Enter select  q quit ")
                .alignment(Alignment::Center),
        );
    let inner = block.inner(area);
    f.render_widget(block, area);

    let dim = Style::default().add_modifier(Modifier::DIM);
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let selected_style = Style::default()
        .add_modifier(Modifier::BOLD)
        .add_modifier(Modifier::REVERSED);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::raw(""));

    for (i, candidate) in candidates.iter().enumerate() {
        let is_selected = i == selected;
        let prefix = if is_selected { "▶ " } else { "  " };

        let name_style = if is_selected { selected_style } else { bold };
        let text_style = if is_selected { selected_style } else { Style::default() };
        let path_style = if is_selected { selected_style } else { dim };

        lines.push(Line::from(vec![
            Span::styled(prefix, name_style),
            Span::styled(candidate.display_name(), name_style),
        ]));

        let desc = candidate.meta.description.as_deref().unwrap_or("");
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(desc, text_style),
        ]));

        let path_str = candidate.path.to_string_lossy().to_string();
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(path_str, path_style),
        ]));

        lines.push(Line::raw(""));
    }

    f.render_widget(Paragraph::new(lines), inner);
}
```

- [ ] **Step 2: Register the module in `src/ui/mod.rs`**

Add alongside the other `pub mod` lines in `src/ui/mod.rs`:

```rust
pub mod config_picker;
```

- [ ] **Step 3: Verify it compiles**

```bash
cargo build --bin edaptor -j4 2>&1 | grep -E "^error"
```

Expected: no output (clean build).

- [ ] **Step 4: Commit**

```bash
git add src/ui/config_picker.rs src/ui/mod.rs
git commit -m "feat(ui): add ratatui config picker for multi-config startup"
```

---

### Task 4: Wire discovery and picker into `main.rs`

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Replace `default_config_path` with discovery + picker**

Replace the entire `default_config_path` function and the config-path resolution in `main`:

Remove this function entirely:

```rust
fn default_config_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join("edaptor/config.toml");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config/edaptor/config.toml")
}
```

Replace the config-path resolution block in `main` (currently lines 52–53):

```rust
// OLD:
let config_path = cli.config.clone().unwrap_or_else(default_config_path);
let config = Config::load(&config_path)?;
```

With:

```rust
// NEW:
let config_path: PathBuf = if let Some(p) = cli.config {
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
```

Also update the `--config` doc comment in the `Cli` struct to reflect the new default behaviour:

```rust
/// Path to the configuration file.
/// Without this flag, edaptor searches ~/.config/edaptor/ and /etc/edaptor/
/// for *.toml files. If exactly one is found it is used automatically;
/// if multiple are found a picker is shown.
#[arg(long, global = true, value_name = "PATH")]
config: Option<PathBuf>,
```

The `use std::path::PathBuf;` import at the top of `main.rs` is already present — no change needed.

- [ ] **Step 2: Run full check**

```bash
make check 2>&1 | tail -5
```

Expected: `All checks passed!`

- [ ] **Step 3: Manual smoke test — single config**

```bash
mkdir -p ~/.config/edaptor
cp examples/demo-config.toml ~/.config/edaptor/demo.toml
cargo run --bin edaptor -- check
```

Expected: connects and prints schema summary without a picker appearing.

- [ ] **Step 4: Manual smoke test — multi-config picker**

```bash
cp examples/oposs-openldap.toml ~/.config/edaptor/oposs.toml
cargo run --bin edaptor -- check
```

Expected: ratatui picker appears with two entries. Navigate with ↑↓, press Enter to select. Pressing `q` exits cleanly with code 0.

- [ ] **Step 5: Commit**

```bash
git add src/main.rs
git commit -m "feat: wire config discovery and picker into startup

Replace single default_config_path() lookup with multi-location discovery.
0 configs → error; 1 config → silent start; 2+ → ratatui picker."
```

---

### Task 5: Update CHANGES.md and docs

**Files:**
- Modify: `CHANGES.md`
- Modify: `docs/src/configuration/server-auth.md` (or `overview.md` — wherever the getting-started section lives)

- [ ] **Step 1: Add CHANGES.md entry**

Under the `### New` heading in the `## Unreleased` section of `CHANGES.md`:

```markdown
- **Config auto-discovery** — edaptor now searches `~/.config/edaptor/*.toml`
  and `/etc/edaptor/*.toml` at startup. A single config is used silently;
  multiple configs trigger a ratatui picker. The `--config` flag bypasses
  discovery as before.
- **`[meta]` table** in config files — optional `name` and `description` fields
  displayed in the startup picker.
```

- [ ] **Step 2: Add a note to the docs**

In `docs/src/configuration/overview.md` (or the first page a reader hits), add a section
explaining where edaptor looks for its config. If that page doesn't exist, add it to
`docs/src/configuration/server-auth.md` at the very top:

```markdown
## Config file locations

Without `--config`, edaptor searches these directories for `*.toml` files:

| Location | Notes |
|----------|-------|
| `$XDG_CONFIG_HOME/edaptor/` (or `~/.config/edaptor/`) | per-user configs |
| `/etc/edaptor/` | system-wide configs |

If exactly one file is found it is loaded automatically. If multiple files exist
a picker is shown at startup. Use `--config /path/to/file.toml` to bypass
discovery entirely.

### Optional `[meta]` table

Add a `[meta]` block to make a config identifiable in the picker:

```toml
[meta]
name        = "carbo-link production"
description = "dc=carbo-link,dc=com via ldapi (ds-carbo-feh)"
```

Both fields are optional.
```

- [ ] **Step 3: Commit**

```bash
git add CHANGES.md docs/src/
git commit -m "docs: document config discovery and [meta] table"
```

---

### Task 6: Push

- [ ] **Push all commits**

```bash
git push
```
