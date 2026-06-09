# Config Discovery & Startup Picker — Design

**Date:** 2026-06-09
**Status:** Approved

## Problem

Today edaptor looks for its config in exactly one place (`~/.config/edaptor/config.toml`) and
crashes with a file-not-found error if it isn't there. There is no way to install shared configs
in `/etc/edaptor/` and no way for a user to have multiple configs for different directories and
pick between them at launch.

## Goals

1. Search two locations automatically: user (`~/.config/edaptor/`) and system (`/etc/edaptor/`).
2. Single config found → start silently (no change in UX).
3. Multiple configs found → show a ratatui picker so the user selects one.
4. `--config` flag always bypasses discovery entirely.
5. Config files can carry a `[meta]` table (name + description) for display in the picker.

---

## Section 1 — Config discovery

### New module: `src/config/discovery.rs`

Public surface:

```rust
pub struct ConfigCandidate {
    pub path:    PathBuf,
    pub meta:    MetaConfig,   // name + description (possibly defaulted)
}

pub fn discover_configs() -> Vec<ConfigCandidate>
```

### Search locations

Collected in this order:

| Priority | Location |
|----------|----------|
| 1 (user) | `$XDG_CONFIG_HOME/edaptor/*.toml` (falls back to `~/.config/edaptor/*.toml`) |
| 2 (system) | `/etc/edaptor/*.toml` |

Within each location files are sorted alphabetically by filename.
Results are deduped by canonical path before returning.

### Startup logic in `main.rs`

```
--config given          → load that path directly (no discovery)
0 candidates found      → error: "no config found in ~/.config/edaptor/ or /etc/edaptor/"
1 candidate found       → use it silently
2+ candidates found     → invoke the picker, use the selected path
```

### Meta parsing during discovery

During discovery, only the `[meta]` table is parsed from each file (partial TOML parse). Full
config parsing happens only after a path is selected. This means a malformed config that the user
never picks does not produce an error at discovery time.

---

## Section 2 — `[meta]` table

### Config file addition

```toml
[meta]
name        = "carbo-link production"
description = "dc=carbo-link,dc=com via ldapi (ds-carbo-feh)"
```

Both fields are optional. The section itself is optional.

### New struct in `src/config/mod.rs`

```rust
#[derive(Debug, Default, Deserialize)]
pub struct MetaConfig {
    pub name:        Option<String>,
    pub description: Option<String>,
}
```

`MetaConfig` is also added as an optional field on `Config`:

```rust
pub struct Config {
    #[serde(default)]
    pub meta:   MetaConfig,
    pub server: ServerConfig,
    pub auth:   AuthConfig,
    // ...
}
```

### Picker display fallbacks

| Field | Present | Absent fallback |
|-------|---------|-----------------|
| `name` | shown as-is | file stem (`ds-carbo-feh` from `ds-carbo-feh.toml`) |
| `description` | shown as second line | second line is blank |
| path | always shown as third line | — |

---

## Section 3 — Picker UI

### New module: `src/ui/config_picker.rs`

Public surface:

```rust
/// Show a full-screen ratatui picker and return the selected path.
/// Returns `None` if the user presses q/Esc (caller should exit cleanly).
pub fn pick_config(candidates: Vec<ConfigCandidate>) -> Result<Option<PathBuf>>
```

### Layout

```
┌─ Select configuration ───────────────────────────────────────────┐
│                                                                    │
│  ▶ carbo-link production                                          │
│    dc=carbo-link,dc=com via ldapi (ds-carbo-feh)                  │
│    /etc/edaptor/ds-carbo-feh.toml                                 │
│                                                                    │
│    example                                                         │
│                                                                    │
│    /etc/edaptor/example.toml                                       │
│                                                                    │
└─ ↑↓ navigate  Enter select  q quit ───────────────────────────────┘
```

- Each candidate occupies three lines: name (bold), description, path (dim).
- The selected item is highlighted with `▶` and a distinct style.
- `↑` / `↓` move the cursor; wraps at top and bottom.
- `Enter` returns the selected path.
- `q` / `Esc` returns `None`; `main.rs` treats this as a clean exit (exit code 0).

### Terminal lifecycle

Uses `ratatui::init()` / `ratatui::restore()` — the same pair used by the main TUI. The picker
runs, restores the terminal, then returns; the main TUI calls `ratatui::init()` again. This
produces no flicker because both use the alternate screen.

---

## Section 4 — Integration

### `main.rs` changes

```rust
let config_path = if let Some(p) = cli.config {
    p
} else {
    let candidates = config::discovery::discover_configs();
    match candidates.len() {
        0 => bail!("no config found in ~/.config/edaptor/ or /etc/edaptor/; \
                    use --config to specify one"),
        1 => candidates.into_iter().next().unwrap().path,
        _ => match ui::config_picker::pick_config(candidates)? {
            Some(p) => p,
            None    => return Ok(()),   // user pressed q/Esc
        },
    }
};
let config = Config::load(&config_path)?;
```

### `src/lib.rs` / module tree

- `config::discovery` — new public module
- `ui::config_picker` — new public module (alongside existing `ui::app`)

### Headless subcommands (`check`, `schema`, `passwd`)

Discovery and the picker run for all entry points, including headless subcommands. If a user
wants to script these against a specific config they use `--config` to bypass discovery.

---

## Error handling

| Situation | Behaviour |
|-----------|-----------|
| `/etc/edaptor/` does not exist | silently skipped (not an error) |
| `~/.config/edaptor/` does not exist | silently skipped |
| A `*.toml` file is unreadable during discovery | printed to stderr, skipped |
| Selected config fails to parse | existing error path (unchanged) |

---

## Testing

- Unit tests in `discovery.rs`: mock filesystem paths, verify sort order, dedup, fallback name.
- Unit tests in `config/mod.rs`: `MetaConfig` parses correctly; absent `[meta]` gives defaults.
- No automated test for the picker UI (interactive ratatui widget — covered by manual smoke test).
