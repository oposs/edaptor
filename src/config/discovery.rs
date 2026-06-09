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
