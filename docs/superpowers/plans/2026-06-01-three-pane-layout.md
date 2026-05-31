# Three-Pane Browser/Editor Layout Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace edaptor's single-tree + modal-dialog TUI with a frameless three-pane layout (branch tree | leaf list + incremental search | live scrollable entry form), backed by an eager full-structure load.

**Architecture:** A worker subtree+paged scan loads the whole DIT structure once at startup into a pure, tty-free structure model. The TUI is a frameless `SplitContainer` (a `turbo_vision::Group` + two draggable dividers) hosting three panes built from stock TV widgets: `OutlineViewer` (branches), `ListBox`+`InputLine` (leaves + search), and a `Group`-of-rows scrolling form (live edit, Save/Cancel). All existing diff/validate/LDIF/write logic is reused unchanged; the facade boundary (only `src/ui/facade.rs` may `use turbo_vision`) is preserved.

**Tech Stack:** Rust, ldap3 0.12 (sync `LdapConn`, RFC 2696 paged adapter), turbo-vision 1.2.0, anyhow.

**Source-of-truth references (read before coding):**
- Spec: `docs/superpowers/specs/2026-06-01-three-pane-layout-design.md`
- TV API verification: `docs/superpowers/research/2026-06-01-tv-api-verification-3pane.md`
- ldap3 paged API: `docs/superpowers/research/2026-06-01-ldap3-0.12-paged-subtree.md`
- Existing facade pattern to mirror: `src/ui/facade.rs` (the `DitOutline` wrapper, lines 133–229)

**Process rules for executors (from project memory `edaptor-m4-handoff`):**
- Do NOT consult an advisor; do NOT stop mid-task. Write code, run the checks, commit each task.
- After each task: `cargo build`, `cargo test`, and `cargo clippy --all-targets -- -D warnings` must all pass. `cargo fmt` before commit.
- Preserve the facade boundary: only `src/ui/facade.rs` may `use turbo_vision`. Verify with:
  `! grep -rl "use turbo_vision" src | grep -v "src/ui/facade.rs"` (must print nothing).
- Live tests are gated by `EDAPTOR_TEST_LDAP_URI`; they SKIP when unset (mirror existing `tests/live_write.rs`).

---

## Phasing & mergeability

- **Phase A (Tasks 1–5): pure, tty-free foundation.** Worker paged scan + structure model + read-only derivation + dirty/guard state machine. Fully unit-tested; mergeable on its own (no UI change yet).
- **Phase B (Tasks 6–9): facade widgets.** SplitContainer, scrolling form pane, leaf-list pane. Tty-only; verified by compile + a frameless smoke + manual/live.
- **Phase C (Tasks 10–11): wiring + live.** Replace the mount in `run_tui`, eager load at startup, selection→panes flow, guard, create/delete reflow, refresh; live round-trip test.

Each task below is independently committable and leaves the build green.

---

## File Structure

**Create:**
- `src/workflows/structure.rs` — pure DIT structure model (tree build, branch/leaf, leaves-of-branch, incremental filter, promote/demote). tty-free, heavily unit-tested.
- `src/ui/form_state.rs` — pure dirty-state + Save/Discard/Stay guard decision. tty-free, unit-tested.
- `tests/live_structure.rs` — gated live test for the subtree paged scan.

**Modify:**
- `src/ldap/worker.rs` — add `SearchScope::Subtree`, `Request::LoadStructure`, `search_subtree_paged`, and the `StructureEntries` response.
- `src/config/mod.rs` — add `ServerConfig.read_only` (default false) + `is_anonymous()` helper on `AuthConfig`; add `Config::is_read_only()`.
- `src/workflows/mod.rs` — `pub mod structure;`
- `src/ui/mod.rs` — `pub mod form_state;`
- `src/ui/facade.rs` — add `SplitContainer`, `FormPane`, `LeafListPane` views and their builders/mount; keep the boundary.
- `src/main.rs` (`run_tui`) — eager load at startup; mount the SplitContainer; wire selection→pane2→pane3, the dirty guard, create/delete reflow, and refresh.

**Reused unchanged:** `src/form/changeset.rs`, `src/form/validate.rs`, `src/ldap/ldif.rs`, `src/ldap/result.rs`, `src/ui/form.rs`, `src/workflows/read_flow.rs`, the worker write paths.

---

## PHASE A — pure foundation

### Task 1: Worker — `SearchScope::Subtree`

**Files:**
- Modify: `src/ldap/worker.rs` (the `SearchScope` enum ~line 34, and `scope_to_ldap3` ~line 275)
- Test: inline `#[cfg(test)]` in `src/ldap/worker.rs`

- [ ] **Step 1: Extend the failing test**

In `src/ldap/worker.rs` `mod tests`, replace the body of `scope_maps_to_ldap3` with:

```rust
    #[test]
    fn scope_maps_to_ldap3() {
        assert!(matches!(scope_to_ldap3(SearchScope::Base), Scope::Base));
        assert!(matches!(
            scope_to_ldap3(SearchScope::OneLevel),
            Scope::OneLevel
        ));
        assert!(matches!(
            scope_to_ldap3(SearchScope::Subtree),
            Scope::Subtree
        ));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib scope_maps_to_ldap3`
Expected: FAIL to compile — `no variant named Subtree found for enum SearchScope`.

- [ ] **Step 3: Add the variant and mapping**

In the `SearchScope` enum add `Subtree`:

```rust
pub enum SearchScope {
    /// The base entry itself (read a single entry).
    Base,
    /// The immediate children of the base (one-level browse).
    OneLevel,
    /// The entire subtree under the base (used for the eager structure scan).
    Subtree,
}
```

In `scope_to_ldap3` add the arm:

```rust
fn scope_to_ldap3(scope: SearchScope) -> Scope {
    match scope {
        SearchScope::Base => Scope::Base,
        SearchScope::OneLevel => Scope::OneLevel,
        SearchScope::Subtree => Scope::Subtree,
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib scope_maps_to_ldap3`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/ldap/worker.rs
git commit -m "feat(worker): add SearchScope::Subtree"
```

---

### Task 2: Worker — paged subtree scan + `LoadStructure` request

**Files:**
- Modify: `src/ldap/worker.rs` (`Request` enum ~line 57; `Response` enum ~line 123; `worker_loop` ~line 299; add `run_load_structure` near `run_search` ~line 426)
- Test: inline tests + the live test is Task 5.

Verified API (from the ldap3 report): use the adapter chain `EntriesOnly` then `PagedResults::new(page_size: i32)` with `conn.streaming_search_with(...)`, drain `next()` to `None`, then `stream.result().success()?`. No Cargo change.

- [ ] **Step 1: Add a `StructureNodeRaw` payload + `Response::StructureEntries`**

The structure scan returns minimal per-entry data. Add this struct near `LdapEntry` (~line 46) in `src/ldap/worker.rs`:

```rust
/// One entry from the eager structure scan: DN + display label inputs + objectClass.
/// Deliberately minimal (no full attributes) so a 100k-entry directory stays cheap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructureNodeRaw {
    /// Distinguished name (the structural key).
    pub dn: String,
    /// `cn` first value, if present (label preference 1).
    pub cn: Option<String>,
    /// `description` first value, if present (label preference 2).
    pub description: Option<String>,
    /// objectClass values (kept for future domain classification).
    pub object_classes: Vec<String>,
}
```

Add to the `Response` enum (after `Entries`/`SearchError`, ~line 134):

```rust
    /// Result of a [`Request::LoadStructure`] eager scan; `id` echoes the request.
    StructureEntries {
        /// Correlation id.
        id: u64,
        /// Every entry under the base (paged), minimal payload.
        nodes: Vec<StructureNodeRaw>,
    },
    /// A failed [`Request::LoadStructure`]; `id` echoes the request. `truncated`
    /// is true when the server refused to page (rc 3/4/11) so the UI can fall back
    /// to lazy one-level browsing.
    StructureError {
        /// Correlation id.
        id: u64,
        /// Human-readable error message.
        msg: String,
        /// True if the failure was a size/time/admin limit (fallback signal).
        truncated: bool,
    },
```

- [ ] **Step 2: Add `Request::LoadStructure`**

In the `Request` enum (after `Search`, ~line 72):

```rust
    /// Eagerly load the entire subtree structure under `base` (paged). `id` is
    /// echoed in the reply for correlation.
    LoadStructure {
        /// Correlation id.
        id: u64,
        /// Base DN to scan (the whole subtree below + including it).
        base: String,
        /// Paged-results page size (e.g. 500).
        page_size: i32,
    },
```

- [ ] **Step 3: Write the failing test for the limit-detection helper**

The only pure-testable piece here is "is this rc a paging-limit fallback?". Add to `mod tests`:

```rust
    #[test]
    fn limit_rc_triggers_truncation_fallback() {
        assert!(is_limit_rc(3)); // timeLimitExceeded
        assert!(is_limit_rc(4)); // sizeLimitExceeded
        assert!(is_limit_rc(11)); // adminLimitExceeded
        assert!(!is_limit_rc(0));
        assert!(!is_limit_rc(32)); // noSuchObject
    }
```

- [ ] **Step 4: Run test to verify it fails**

Run: `cargo test --lib limit_rc_triggers_truncation_fallback`
Expected: FAIL — `cannot find function is_limit_rc`.

- [ ] **Step 5: Implement the helper, the scan, and the worker arm**

Add the imports at the top of `src/ldap/worker.rs` (next to the existing `use ldap3::{...}`):

```rust
use ldap3::adapters::{Adapter, EntriesOnly, PagedResults};
```

Add the helper (near `write_response`, worker-private):

```rust
/// True for the LDAP result codes that mean "the server capped the result set"
/// (time/size/admin limit). Used to decide whether to fall back to lazy browsing.
fn is_limit_rc(rc: u32) -> bool {
    matches!(rc, 3 | 4 | 11)
}
```

Add the scan function (near `run_search`, ~line 426):

```rust
/// Page through the entire subtree under `base` (RFC 2696) and return minimal
/// per-entry structure data. Bypasses the server's per-request size limit. On a
/// time/size/admin limit it returns the entries gathered so far paired with a
/// `truncated` flag so the caller can fall back to lazy browsing.
fn run_load_structure(
    conn: &mut LdapConn,
    base: &str,
    page_size: i32,
) -> std::result::Result<Vec<StructureNodeRaw>, (String, bool)> {
    let adapters: Vec<Box<dyn Adapter<_, _>>> = vec![
        Box::new(EntriesOnly::new()),
        Box::new(PagedResults::new(page_size)),
    ];
    let attrs = vec![
        "cn".to_string(),
        "description".to_string(),
        "objectClass".to_string(),
    ];
    let mut stream = conn
        .streaming_search_with(adapters, base, Scope::Subtree, "(objectClass=*)", attrs)
        .map_err(|e| (format!("{e}"), false))?;

    let mut out = Vec::new();
    loop {
        match stream.next() {
            Ok(Some(re)) => {
                let se = SearchEntry::construct(re);
                out.push(structure_node_from(se));
            }
            Ok(None) => break,
            Err(e) => return Err((format!("{e}"), false)),
        }
    }

    match stream.result().success() {
        Ok(_) => Ok(out),
        Err(ldap3::LdapError::LdapResult { result }) if is_limit_rc(result.rc) => {
            // Partial: hand back what we paged plus the fallback signal.
            Err((result_code_message(result.rc, &result.text), true))
        }
        Err(e) => Err((format!("{e}"), false)),
    }
}

/// First value of a (case-sensitive ldap3 key) attribute from a SearchEntry.
fn first_attr(se: &SearchEntry, attr: &str) -> Option<String> {
    se.attrs.get(attr).and_then(|v| v.first().cloned())
}

/// Flatten a SearchEntry into the minimal structure payload.
fn structure_node_from(se: SearchEntry) -> StructureNodeRaw {
    let cn = first_attr(&se, "cn");
    let description = first_attr(&se, "description");
    let object_classes = se.attrs.get("objectClass").cloned().unwrap_or_default();
    StructureNodeRaw {
        dn: se.dn,
        cn,
        description,
        object_classes,
    }
}
```

Add the arm in `worker_loop` (after the `Search` arm, ~line 324):

```rust
            Request::LoadStructure { id, base, page_size } => {
                let resp = match run_load_structure(conn, &base, page_size) {
                    Ok(nodes) => Response::StructureEntries { id, nodes },
                    Err((msg, truncated)) => Response::StructureError { id, msg, truncated },
                };
                let _ = reply.send(resp);
            }
```

- [ ] **Step 6: Run tests**

Run: `cargo test --lib limit_rc_triggers_truncation_fallback && cargo build`
Expected: PASS + clean build. (`ldap3::LdapError` and `result.rc` are the verified shapes; if the `LdapError::LdapResult { result }` pattern needs the full path, use `ldap3::result::LdapError::LdapResult { result }`.)

- [ ] **Step 7: Commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/ldap/worker.rs
git commit -m "feat(worker): eager subtree paged structure scan (LoadStructure)"
```

---

### Task 3: Pure structure model — tree build + branch/leaf

**Files:**
- Create: `src/workflows/structure.rs`
- Modify: `src/workflows/mod.rs` (add `pub mod structure;`)
- Test: inline `#[cfg(test)]` in `src/workflows/structure.rs`

This module is the heart of the eager model. It is generic-free, tty-free, and does NOT depend on the worker's `StructureNodeRaw` type by reference — instead it accepts a plain input struct so it can be unit-tested without ldap. The caller (main.rs) maps `StructureNodeRaw → StructureInput`.

- [ ] **Step 1: Write the failing tests**

Create `src/workflows/structure.rs`:

```rust
//! Pure, tty-free model of the eagerly-loaded DIT structure.
//!
//! Built once from the flat paged scan, it answers — without further queries —
//! which entries are branches (have children), the leaves directly under a branch,
//! incremental-search filtering of those leaves, and the local reflow when an entry
//! gains its first child (promote) or loses its last (demote).

use std::collections::BTreeMap;

/// Input row for building the structure (mapped from the worker's `StructureNodeRaw`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructureInput {
    /// Distinguished name.
    pub dn: String,
    /// `cn` first value, if any.
    pub cn: Option<String>,
    /// `description` first value, if any.
    pub description: Option<String>,
    /// objectClass values.
    pub object_classes: Vec<String>,
}

/// One node in the structure model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructureNode {
    /// Distinguished name (the key).
    pub dn: String,
    /// Display label (cn → description → RDN).
    pub label: String,
    /// objectClass values.
    pub object_classes: Vec<String>,
    /// Child DNs, in input order.
    pub children: Vec<String>,
}

impl StructureNode {
    /// A node is a branch iff it has at least one child.
    pub fn is_branch(&self) -> bool {
        !self.children.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(dn: &str, cn: Option<&str>, desc: Option<&str>) -> StructureInput {
        StructureInput {
            dn: dn.to_string(),
            cn: cn.map(str::to_string),
            description: desc.map(str::to_string),
            object_classes: vec![],
        }
    }

    fn fixture() -> Structure {
        // dc=example,dc=org
        //   ou=users        (branch: has jane)
        //     uid=jane
        //   ou=empty        (leaf: no children)
        Structure::build(
            "dc=example,dc=org",
            vec![
                input("dc=example,dc=org", None, Some("Example")),
                input("ou=users,dc=example,dc=org", None, None),
                input("uid=jane,ou=users,dc=example,dc=org", Some("Jane Doe"), None),
                input("ou=empty,dc=example,dc=org", None, None),
            ],
        )
    }

    #[test]
    fn label_prefers_cn_then_description_then_rdn() {
        let s = fixture();
        assert_eq!(s.get("uid=jane,ou=users,dc=example,dc=org").unwrap().label, "Jane Doe");
        assert_eq!(s.get("dc=example,dc=org").unwrap().label, "Example");
        assert_eq!(s.get("ou=users,dc=example,dc=org").unwrap().label, "ou=users");
    }

    #[test]
    fn branch_is_has_children() {
        let s = fixture();
        assert!(s.get("ou=users,dc=example,dc=org").unwrap().is_branch());
        assert!(!s.get("ou=empty,dc=example,dc=org").unwrap().is_branch());
        assert!(!s.get("uid=jane,ou=users,dc=example,dc=org").unwrap().is_branch());
        assert!(s.get("dc=example,dc=org").unwrap().is_branch());
    }

    #[test]
    fn root_is_always_present_even_if_childless() {
        let s = Structure::build("dc=example,dc=org", vec![input("dc=example,dc=org", None, None)]);
        assert!(s.get("dc=example,dc=org").is_some());
        assert_eq!(s.root_dn(), "dc=example,dc=org");
    }

    #[test]
    fn branch_dns_lists_only_branches_plus_root() {
        let s = fixture();
        let mut branches = s.branch_dns();
        branches.sort();
        assert_eq!(
            branches,
            vec!["dc=example,dc=org", "ou=users,dc=example,dc=org"]
        );
    }

    #[test]
    fn leaves_of_lists_only_leaf_children() {
        let s = fixture();
        // Under root: ou=users is a branch (excluded), ou=empty is a leaf (included).
        let leaves: Vec<&str> = s
            .leaves_of("dc=example,dc=org")
            .iter()
            .map(|n| n.dn.as_str())
            .collect();
        assert_eq!(leaves, vec!["ou=empty,dc=example,dc=org"]);
        // Under ou=users: uid=jane is a leaf.
        let leaves: Vec<&str> = s
            .leaves_of("ou=users,dc=example,dc=org")
            .iter()
            .map(|n| n.dn.as_str())
            .collect();
        assert_eq!(leaves, vec!["uid=jane,ou=users,dc=example,dc=org"]);
    }

    #[test]
    fn filter_leaves_is_case_insensitive_substring_on_label() {
        let s = fixture();
        let hits: Vec<&str> = s
            .filter_leaves("ou=users,dc=example,dc=org", "jane")
            .iter()
            .map(|n| n.dn.as_str())
            .collect();
        assert_eq!(hits, vec!["uid=jane,ou=users,dc=example,dc=org"]);
        assert!(s.filter_leaves("ou=users,dc=example,dc=org", "zzz").is_empty());
        // Empty filter returns all leaves.
        assert_eq!(s.filter_leaves("ou=users,dc=example,dc=org", "").len(), 1);
    }

    #[test]
    fn promote_marks_parent_as_branch_on_first_child() {
        let mut s = fixture();
        // ou=empty is a leaf; add a child under it.
        let changed = s.add_child(
            "ou=empty,dc=example,dc=org",
            input("cn=x,ou=empty,dc=example,dc=org", Some("X"), None),
        );
        assert!(changed, "leaf->branch is a reflow");
        assert!(s.get("ou=empty,dc=example,dc=org").unwrap().is_branch());
    }

    #[test]
    fn demote_marks_parent_as_leaf_on_last_child_removed() {
        let mut s = fixture();
        let changed = s.remove("uid=jane,ou=users,dc=example,dc=org");
        assert!(changed, "branch->leaf is a reflow");
        assert!(!s.get("ou=users,dc=example,dc=org").unwrap().is_branch());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib structure::`
Expected: FAIL — `cannot find type Structure`.

- [ ] **Step 3: Implement `Structure`**

Append to `src/workflows/structure.rs` (above `#[cfg(test)]`):

```rust
/// The whole eager structure: a DN→node map plus the root DN.
#[derive(Debug, Clone)]
pub struct Structure {
    root_dn: String,
    nodes: BTreeMap<String, StructureNode>,
}

/// The leftmost RDN of a DN (`uid=jane` from `uid=jane,ou=users,...`).
fn rdn_of(dn: &str) -> &str {
    dn.split(',').next().unwrap_or(dn).trim()
}

/// The parent DN (everything after the first comma), or `None` at the top.
fn parent_of(dn: &str) -> Option<&str> {
    dn.split_once(',').map(|(_, rest)| rest.trim())
}

/// Choose a label: cn → description → RDN.
fn label_for(input: &StructureInput) -> String {
    if let Some(cn) = input.cn.as_ref().filter(|s| !s.is_empty()) {
        return cn.clone();
    }
    if let Some(d) = input.description.as_ref().filter(|s| !s.is_empty()) {
        return d.clone();
    }
    rdn_of(&input.dn).to_string()
}

impl Structure {
    /// Build the model from the flat scan. Parent/child links are derived from DN
    /// suffixes (case-insensitive). The `root` DN is always present even if the
    /// scan returned nothing for it, so a first child can be created.
    pub fn build(root: &str, inputs: Vec<StructureInput>) -> Structure {
        let mut nodes: BTreeMap<String, StructureNode> = BTreeMap::new();

        // First pass: create every node.
        for inp in &inputs {
            nodes.insert(
                inp.dn.clone(),
                StructureNode {
                    dn: inp.dn.clone(),
                    label: label_for(inp),
                    object_classes: inp.object_classes.clone(),
                    children: Vec::new(),
                },
            );
        }
        // Ensure the root exists.
        nodes.entry(root.to_string()).or_insert_with(|| StructureNode {
            dn: root.to_string(),
            label: rdn_of(root).to_string(),
            object_classes: Vec::new(),
            children: Vec::new(),
        });

        // Second pass: link each node to its parent (in input order).
        for inp in &inputs {
            if let Some(parent) = parent_of(&inp.dn) {
                // Only link if the parent is a known node (within the loaded base).
                if let Some(p) = nodes.get_mut(parent) {
                    if !p.children.iter().any(|c| c.eq_ignore_ascii_case(&inp.dn)) {
                        p.children.push(inp.dn.clone());
                    }
                }
            }
        }

        Structure {
            root_dn: root.to_string(),
            nodes,
        }
    }

    /// The root (base) DN.
    pub fn root_dn(&self) -> &str {
        &self.root_dn
    }

    /// Look up a node by exact DN.
    pub fn get(&self, dn: &str) -> Option<&StructureNode> {
        self.nodes.get(dn)
    }

    /// All branch DNs (have ≥1 child); the root is included if it is a branch.
    pub fn branch_dns(&self) -> Vec<String> {
        self.nodes
            .values()
            .filter(|n| n.is_branch())
            .map(|n| n.dn.clone())
            .collect()
    }

    /// The leaf children (no children of their own) directly under `branch_dn`,
    /// in input order.
    pub fn leaves_of(&self, branch_dn: &str) -> Vec<&StructureNode> {
        let Some(branch) = self.nodes.get(branch_dn) else {
            return Vec::new();
        };
        branch
            .children
            .iter()
            .filter_map(|c| self.nodes.get(c))
            .filter(|c| !c.is_branch())
            .collect()
    }

    /// `leaves_of` filtered by a case-insensitive substring over the label. An
    /// empty `query` returns all leaves.
    pub fn filter_leaves(&self, branch_dn: &str, query: &str) -> Vec<&StructureNode> {
        let q = query.to_lowercase();
        self.leaves_of(branch_dn)
            .into_iter()
            .filter(|n| q.is_empty() || n.label.to_lowercase().contains(&q))
            .collect()
    }

    /// Add a child node (e.g. after a successful create). Returns true if the
    /// parent changed leaf→branch (a reflow the UI must reflect).
    pub fn add_child(&mut self, parent_dn: &str, child: StructureInput) -> bool {
        let was_branch = self.get(parent_dn).map(|n| n.is_branch()).unwrap_or(false);
        let node = StructureNode {
            dn: child.dn.clone(),
            label: label_for(&child),
            object_classes: child.object_classes.clone(),
            children: Vec::new(),
        };
        self.nodes.insert(child.dn.clone(), node);
        if let Some(p) = self.nodes.get_mut(parent_dn) {
            if !p.children.iter().any(|c| c.eq_ignore_ascii_case(&child.dn)) {
                p.children.push(child.dn);
            }
        }
        let is_branch = self.get(parent_dn).map(|n| n.is_branch()).unwrap_or(false);
        !was_branch && is_branch
    }

    /// Remove a node (e.g. after delete). Returns true if its parent changed
    /// branch→leaf (a reflow the UI must reflect).
    pub fn remove(&mut self, dn: &str) -> bool {
        let parent = parent_of(dn).map(str::to_string);
        self.nodes.remove(dn);
        if let Some(parent) = parent {
            let was_branch = self.get(&parent).map(|n| n.is_branch()).unwrap_or(false);
            if let Some(p) = self.nodes.get_mut(&parent) {
                p.children.retain(|c| !c.eq_ignore_ascii_case(dn));
            }
            let is_branch = self.get(&parent).map(|n| n.is_branch()).unwrap_or(false);
            return was_branch && !is_branch;
        }
        false
    }
}
```

- [ ] **Step 4: Wire the module**

In `src/workflows/mod.rs` add:

```rust
pub mod structure;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib structure::`
Expected: PASS (all 8 tests).

- [ ] **Step 6: Commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/workflows/structure.rs src/workflows/mod.rs
git commit -m "feat(structure): pure eager DIT model (branch/leaf, leaves-of, filter, reflow)"
```

---

### Task 4: Read-only mode derivation

**Files:**
- Modify: `src/config/mod.rs` (`ServerConfig` ~line 63; `AuthConfig` ~line 107; `impl Config` ~line 126)
- Test: inline `#[cfg(test)]` in `src/config/mod.rs`

Decision rule (spec §5.8): read-only iff the config flag is set OR the bind is anonymous (no `bind_dn`).

- [ ] **Step 1: Write the failing tests**

Add to `src/config/mod.rs` `mod tests`:

```rust
    #[test]
    fn read_only_flag_forces_read_only() {
        let toml = r#"
            [server]
            uri = "ldap://x"
            base_dn = "dc=x"
            read_only = true
            [auth]
            bind_dn = "cn=admin,dc=x"
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert!(cfg.is_read_only());
    }

    #[test]
    fn anonymous_bind_is_read_only() {
        let toml = r#"
            [server]
            uri = "ldap://x"
            base_dn = "dc=x"
            [auth]
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert!(cfg.auth.is_anonymous());
        assert!(cfg.is_read_only());
    }

    #[test]
    fn bound_writable_is_not_read_only() {
        let toml = r#"
            [server]
            uri = "ldap://x"
            base_dn = "dc=x"
            [auth]
            bind_dn = "cn=admin,dc=x"
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert!(!cfg.is_read_only());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib read_only`
Expected: FAIL — `no method named is_read_only` / `no field read_only`.

- [ ] **Step 3: Implement**

In `ServerConfig` add the field (after `start_tls`):

```rust
    /// Global read-only mode. When true (or when the bind is anonymous), the TUI
    /// hides Save/Cancel and create/delete actions (spec §5.8).
    #[serde(default)]
    pub read_only: bool,
```

Add to `impl AuthConfig` (create the impl block if absent, right after the `AuthConfig` struct):

```rust
impl AuthConfig {
    /// True when no bind DN is configured (anonymous bind).
    pub fn is_anonymous(&self) -> bool {
        self.bind_dn.as_deref().map(str::trim).unwrap_or("").is_empty()
    }
}
```

In `impl Config` add:

```rust
    /// Global read-only: the explicit flag OR an anonymous bind (spec §5.8).
    pub fn is_read_only(&self) -> bool {
        self.server.read_only || self.auth.is_anonymous()
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib read_only && cargo test --lib config`
Expected: PASS (and the existing config tests still pass).

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/config/mod.rs
git commit -m "feat(config): global read-only mode (flag or anonymous bind)"
```

---

### Task 5: Live test — paged subtree scan against OpenLDAP

**Files:**
- Create: `tests/live_structure.rs`

Mirror the gating pattern of `tests/live_write.rs` (read it first for the exact config/spawn helper shape; reuse the same env var `EDAPTOR_TEST_LDAP_URI`).

- [ ] **Step 1: Write the gated live test**

Create `tests/live_structure.rs`:

```rust
//! Live test (gated by EDAPTOR_TEST_LDAP_URI): the eager subtree paged scan
//! returns the full structure under the base, including entries past the default
//! size limit. SKIPS cleanly when the env var is unset.

use std::collections::BTreeMap;

use edaptor::config::{AuthConfig, AuthMethod, Config, ServerConfig, TlsConfig};
use edaptor::ldap::worker::{Request, Response, WorkerHandle};

fn test_uri() -> Option<String> {
    std::env::var("EDAPTOR_TEST_LDAP_URI").ok()
}

fn config_for(uri: &str) -> Config {
    Config {
        server: ServerConfig {
            uri: uri.to_string(),
            base_dn: "dc=example,dc=org".to_string(),
            start_tls: false,
            read_only: false,
            timeout_secs: 10,
            tls: TlsConfig::default(),
        },
        auth: AuthConfig {
            method: AuthMethod::Simple,
            bind_dn: Some("cn=admin,dc=example,dc=org".to_string()),
            password_source: Default::default(),
        },
        profiles: vec![],
        samba: Default::default(),
    }
}

#[test]
fn eager_structure_scan_returns_subtree() {
    let Some(uri) = test_uri() else {
        eprintln!("SKIP: EDAPTOR_TEST_LDAP_URI not set");
        return;
    };
    let cfg = config_for(&uri);
    let worker = WorkerHandle::spawn(cfg, "adminpassword".to_string())
        .expect("worker should connect+bind");

    let resp = worker
        .request(Request::LoadStructure {
            id: 1,
            base: "dc=example,dc=org".to_string(),
            page_size: 2, // tiny page size forces multiple pages
        })
        .expect("structure scan should reply");

    match resp {
        Response::StructureEntries { id, nodes } => {
            assert_eq!(id, 1);
            // The seeded directory has the base + ou=users + ou=groups + entries.
            assert!(
                nodes.iter().any(|n| n.dn == "dc=example,dc=org"),
                "base present"
            );
            assert!(
                nodes.iter().any(|n| n.dn.starts_with("ou=users")),
                "ou=users present"
            );
            assert!(nodes.len() >= 3, "expected several entries, got {}", nodes.len());
        }
        other => panic!("expected StructureEntries, got {other:?}"),
    }
    let _ = BTreeMap::<String, String>::new(); // keep import used if assertions trimmed
}
```

NOTE: if `Config`/`ServerConfig` field construction differs from the above (e.g. extra fields), match the actual struct — read `src/config/mod.rs`. The `Response` enum must derive `Debug` for the `{other:?}` panic; it already does (Task 2 keeps the existing `#[derive]`-free enum — if `Response` has no `Debug`, add `#[derive(Debug)]` to it in Task 2 and keep it here).

- [ ] **Step 2: Run it unset (must SKIP, not fail)**

Run: `cargo test --test live_structure`
Expected: PASS with `SKIP:` printed (env var unset).

- [ ] **Step 3: Run it against podman OpenLDAP**

```bash
scripts/test-ldap.sh start
EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389 cargo test --test live_structure -- --nocapture
scripts/test-ldap.sh stop
```

Expected: PASS with the subtree assertions met. If `Response` lacks `Debug`, add `#[derive(Debug)]` to it (worker.rs) and re-run.

- [ ] **Step 4: Commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add tests/live_structure.rs src/ldap/worker.rs
git commit -m "test(live): eager subtree paged scan returns full structure (gated)"
```

---

## PHASE B — facade widgets (tty-only)

> These views require a terminal and are not unit-tested, exactly like the existing
> facade widgets. They are verified by: (1) `cargo build` + `clippy`; (2) the boundary
> grep; (3) the existing `m3_smoke` pty test still launching/quitting; (4) manual run in
> Task 11. Each task adds a tiny tty-free assertion where one is meaningful (e.g. a
> pure geometry helper) so there is at least one fast check.

### Task 6: `SplitContainer` — frameless 3-pane Group + draggable dividers

**Files:**
- Modify: `src/ui/facade.rs` (add after the `DitOutline` block)
- Test: inline `#[cfg(test)]` in `src/ui/facade.rs` for the pure geometry helper.

Verified facts used (TV report §B, §D): `Group::new/add/handle_event/draw/select_next`; `Group` impls `View`; mounting a bare Group on the desktop is allowed; mouse capture works while the focused child (here the SplitContainer itself) has `SF_DRAGGING` in its `state()`; `event.mouse.pos.x`; `EventType::{MouseDown,MouseMove,MouseUp}`; `SF_DRAGGING = 0x080`. Required `View` methods include `get_palette` (easy to miss).

- [ ] **Step 1: Add a pure column-geometry helper + its test**

The only tty-free logic is "given outer bounds + two divider x's, compute the three pane rects and clamp the dividers". Add this near the top of `facade.rs` (it needs only `Rect`):

```rust
/// Minimum width any pane may shrink to.
const MIN_PANE_W: i16 = 8;
/// Columns a divider occupies.
const DIVIDER_W: i16 = 1;

/// Clamp two divider x-positions so every pane keeps `MIN_PANE_W` and the dividers
/// stay ordered, within the absolute interior `[left, right)`.
fn clamp_dividers(left: i16, right: i16, mut d0: i16, mut d1: i16) -> (i16, i16) {
    d0 = d0
        .max(left + MIN_PANE_W)
        .min(right - 2 * MIN_PANE_W - DIVIDER_W);
    d1 = d1
        .max(d0 + DIVIDER_W + MIN_PANE_W)
        .min(right - MIN_PANE_W);
    (d0, d1)
}

/// The three absolute pane rects for a SplitContainer of bounds `b` with dividers
/// at `d0`/`d1` (already clamped).
fn pane_rects(b: Rect, d0: i16, d1: i16) -> [Rect; 3] {
    let (top, bottom) = (b.a.y, b.b.y);
    [
        Rect::new(b.a.x, top, d0, bottom),
        Rect::new(d0 + DIVIDER_W, top, d1, bottom),
        Rect::new(d1 + DIVIDER_W, top, b.b.x, bottom),
    ]
}
```

Add the test in `mod tests`:

```rust
    #[test]
    fn dividers_clamp_and_panes_tile() {
        // Interior x in [0,60). Push dividers to absurd values; expect clamping.
        let (d0, d1) = clamp_dividers(0, 60, -100, 1000);
        assert!(d0 >= 8 && d1 > d0 && d1 <= 52);
        let panes = pane_rects(Rect::new(0, 0, 60, 10), d0, d1);
        // Panes are left-to-right, non-overlapping, separated by the divider column.
        assert_eq!(panes[0].a.x, 0);
        assert_eq!(panes[1].a.x, d0 + 1);
        assert_eq!(panes[2].b.x, 60);
        assert!(panes[0].b.x <= panes[1].a.x);
        assert!(panes[1].b.x <= panes[2].a.x);
    }
```

- [ ] **Step 2: Run the helper test (fails)**

Run: `cargo test --lib dividers_clamp_and_panes_tile`
Expected: FAIL — helpers not defined. (You just added them, so it should compile-fail only if names differ; fix and continue.)

- [ ] **Step 3: Implement `SplitContainer`**

Add imports at the top of `facade.rs` if not already present:

```rust
use turbo_vision::core::state::SF_DRAGGING;
use turbo_vision::core::draw::DrawBuffer;
use turbo_vision::views::group::Group;
use turbo_vision::views::view::write_line_to_terminal;
```

Add the struct + impl (after the `DitOutline` `impl View` block):

```rust
/// A frameless three-column container with two mouse-draggable vertical dividers.
/// Wraps a [`Group`] that owns the three pane child views (so Tab focus cycling and
/// child event routing come for free); the SplitContainer adds only divider drawing
/// and drag (TV has no splitter widget). Mounted directly on `app.desktop`.
///
/// Drag capture: while a divider is being dragged the SplitContainer sets
/// `SF_DRAGGING` on its own `state()`, so the desktop keeps feeding it MouseMove/Up
/// even when the cursor leaves the divider column (TV report §D — same mechanism by
/// which `Window` rides `Frame`'s flag).
pub struct SplitContainer {
    inner: Group,
    bounds: Rect,
    /// Absolute x of divider 0 (panes 0|1) and divider 1 (panes 1|2).
    divider_x: [i16; 2],
    /// Which divider is being dragged, if any.
    dragging: Option<usize>,
    state: StateFlags,
    palette_chain: Option<PaletteChainNode>,
}

impl SplitContainer {
    /// Build from three already-constructed pane views (left→right). Their incoming
    /// bounds are ignored; `layout` assigns columns.
    pub fn new(
        bounds: Rect,
        left: Box<dyn View>,
        middle: Box<dyn View>,
        right: Box<dyn View>,
    ) -> Self {
        let w = bounds.b.x - bounds.a.x;
        let (d0, d1) = clamp_dividers(
            bounds.a.x,
            bounds.b.x,
            bounds.a.x + w / 3,
            bounds.a.x + (2 * w) / 3,
        );
        let mut inner = Group::new(bounds);
        // Group::add converts relative→absolute, but we immediately re-set_bounds to
        // absolute pane rects, so the add-bounds don't matter.
        inner.add(left);
        inner.add(middle);
        inner.add(right);
        let mut me = SplitContainer {
            inner,
            bounds,
            divider_x: [d0, d1],
            dragging: None,
            state: 0,
            palette_chain: None,
        };
        me.layout();
        me.inner.set_initial_focus();
        me
    }

    /// Assign each pane its absolute column rect.
    fn layout(&mut self) {
        let rects = pane_rects(self.bounds, self.divider_x[0], self.divider_x[1]);
        for (i, r) in rects.iter().enumerate() {
            self.inner.child_at_mut(i).set_bounds(*r);
        }
    }

    /// If `x` is exactly on a divider column, which one (0/1)?
    fn divider_at(&self, x: i16, y: i16) -> Option<usize> {
        if y < self.bounds.a.y || y >= self.bounds.b.y {
            return None;
        }
        self.divider_x.iter().position(|&dx| x == dx)
    }
}

impl View for SplitContainer {
    fn bounds(&self) -> Rect {
        self.bounds
    }

    fn set_bounds(&mut self, new: Rect) {
        // Rescale divider fractions to the new width, then re-clamp + re-layout.
        let old_w = (self.bounds.b.x - self.bounds.a.x).max(1);
        let new_w = (new.b.x - new.a.x).max(1);
        for d in &mut self.divider_x {
            let frac = (*d - self.bounds.a.x) as f32 / old_w as f32;
            *d = new.a.x + (frac * new_w as f32).round() as i16;
        }
        self.bounds = new;
        self.inner.set_bounds(new);
        let (d0, d1) = clamp_dividers(new.a.x, new.b.x, self.divider_x[0], self.divider_x[1]);
        self.divider_x = [d0, d1];
        self.layout();
    }

    fn draw(&mut self, terminal: &mut Terminal) {
        self.inner.set_palette_chain(self.palette_chain.clone());
        self.inner.draw(terminal);
        // Paint the two divider columns.
        let attr = self.map_color(1);
        for &x in &self.divider_x {
            for y in self.bounds.a.y..self.bounds.b.y {
                let mut buf = DrawBuffer::new(DIVIDER_W as usize);
                buf.move_char(0, '│', attr, DIVIDER_W as usize);
                write_line_to_terminal(terminal, x, y, &buf);
            }
        }
    }

    fn handle_event(&mut self, event: &mut Event) {
        match event.what {
            EventType::MouseDown => {
                if let Some(i) = self.divider_at(event.mouse.pos.x, event.mouse.pos.y) {
                    self.dragging = Some(i);
                    self.state |= SF_DRAGGING; // keep capture (TV report §D)
                    event.clear();
                    return;
                }
            }
            EventType::MouseMove => {
                if let Some(i) = self.dragging {
                    let x = event.mouse.pos.x;
                    self.divider_x[i] = x;
                    let (d0, d1) = clamp_dividers(
                        self.bounds.a.x,
                        self.bounds.b.x,
                        self.divider_x[0],
                        self.divider_x[1],
                    );
                    self.divider_x = [d0, d1];
                    self.layout();
                    event.clear();
                    return;
                }
            }
            EventType::MouseUp => {
                if self.dragging.is_some() {
                    self.dragging = None;
                    self.state &= !SF_DRAGGING;
                    event.clear();
                    return;
                }
            }
            _ => {}
        }
        // Not a divider gesture: delegate to the Group (routing, Tab, keyboard).
        self.inner.handle_event(event);
    }

    fn can_focus(&self) -> bool {
        true
    }

    fn state(&self) -> StateFlags {
        self.state
    }

    fn set_state(&mut self, state: StateFlags) {
        self.state = state;
    }

    fn set_focus(&mut self, focused: bool) {
        self.set_state_flag(SF_FOCUSED, focused);
        if focused {
            self.inner.set_initial_focus();
        }
    }

    fn update_cursor(&self, terminal: &mut Terminal) {
        if let Some(child) = self.inner.focused_child() {
            child.update_cursor(terminal);
        }
    }

    fn set_palette_chain(&mut self, node: Option<PaletteChainNode>) {
        self.palette_chain = node;
    }

    fn get_palette_chain(&self) -> Option<&PaletteChainNode> {
        self.palette_chain.as_ref()
    }

    fn get_palette(&self) -> Option<Palette> {
        None
    }
}
```

- [ ] **Step 4: Build + helper test + boundary check**

Run:
```bash
cargo build && cargo test --lib dividers_clamp_and_panes_tile
! grep -rl "use turbo_vision" src | grep -v "src/ui/facade.rs"
```
Expected: build OK, test PASS, grep prints nothing. If `DrawBuffer`/`write_line_to_terminal`/`move_char` paths differ, fix against the crate (they are the same ones the skeleton used: `core::draw::DrawBuffer`, `views::view::write_line_to_terminal`).

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/ui/facade.rs
git commit -m "feat(ui): frameless SplitContainer (Group + draggable dividers)"
```

---

### Task 7: `FormPane` — live, scrollable edit form

**Files:**
- Modify: `src/ui/facade.rs`
- Test: inline `#[cfg(test)]` for the pure scroll-clamp helper.

Verified approach (TV report §C): NOT `Scroller`. Own an inner `Group` of row views + a manual `delta` + a `ScrollBar`; translate the Group origin on scroll (keep `dw==dh==0`). Reuse the existing field rendering rules (`field_is_editable`, `field_display`) and the `collect_edit_entry` machinery. Save/Cancel are `Button`s; in read-only mode they are omitted and fields are `StaticText`.

This pane exposes a small API the wiring layer (Task 10/11) drives:
- `FormPane::new(bounds, read_only)` — empty pane.
- `set_model(&mut self, model: &FormModel)` — (re)builds the rows from a `FormModel`; resets dirty + scroll.
- `clear(&mut self)` — show "no selection".
- `is_dirty(&self) -> bool` — any bound value differs from the baseline.
- `take_edit(&self) -> Option<EditEntry>` — collect current values (None if empty/cleared).
- `command pumped via handle_event`: emits `CM_FORM_SAVE` / `CM_FORM_CANCEL` (new local command ids) when the Save/Cancel buttons fire, so the loop reacts.

- [ ] **Step 1: Add the pure scroll-clamp helper + test**

```rust
/// Clamp a desired vertical scroll `delta` to `[0, max(0, content_h - viewport_h)]`.
fn clamp_scroll(delta: i16, content_h: i16, viewport_h: i16) -> i16 {
    let max = (content_h - viewport_h).max(0);
    delta.max(0).min(max)
}
```

Test:
```rust
    #[test]
    fn scroll_clamps_to_content() {
        assert_eq!(clamp_scroll(-5, 20, 10), 0);
        assert_eq!(clamp_scroll(100, 20, 10), 10); // 20-10
        assert_eq!(clamp_scroll(3, 20, 10), 3);
        assert_eq!(clamp_scroll(5, 8, 10), 0); // content shorter than viewport
    }
```

- [ ] **Step 2: Run it (fails)**

Run: `cargo test --lib scroll_clamps_to_content`
Expected: FAIL until the helper exists.

- [ ] **Step 3: Implement `FormPane`**

Key structure (full code — adapt method bodies to the verified signatures):

```rust
/// Local command ids emitted by [`FormPane`] when its buttons fire.
const CM_FORM_SAVE: CommandId = 2200;
const CM_FORM_CANCEL: CommandId = 2201;

/// One editable row's attribute name + the shared String its InputLine mutates.
type RowBinding = (String, Rc<RefCell<String>>);

/// The live, scrollable entry-edit pane (pane 3). Holds an inner [`Group`] of
/// label+editor rows translated by a manual scroll `delta`, plus a Save/Cancel bar
/// (omitted in read-only mode). Dirty = any binding differs from its baseline.
pub struct FormPane {
    bounds: Rect,
    read_only: bool,
    inner: Group,
    bindings: Vec<RowBinding>,
    /// Baseline values per attribute (for dirty detection + Cancel revert).
    baseline: std::collections::BTreeMap<String, Vec<String>>,
    /// The DN being edited (form title), empty when cleared.
    dn: String,
    content_h: i16,
    scroll: i16,
    state: StateFlags,
    palette_chain: Option<PaletteChainNode>,
}

impl FormPane {
    pub fn new(bounds: Rect, read_only: bool) -> Self {
        FormPane {
            bounds,
            read_only,
            inner: Group::new(bounds),
            bindings: Vec::new(),
            baseline: std::collections::BTreeMap::new(),
            dn: String::new(),
            content_h: 0,
            scroll: 0,
            state: 0,
            palette_chain: None,
        }
    }

    /// Rebuild the rows from a model (called when pane-2 selection changes).
    pub fn set_model(&mut self, model: &FormModel) {
        self.dn = model.title.clone();
        self.baseline.clear();
        for f in &model.fields {
            self.baseline.insert(f.label.clone(), f.values.clone());
        }
        // Rebuild the inner Group fresh.
        self.inner = Group::new(self.bounds);
        self.bindings.clear();
        let width = self.bounds.b.x - self.bounds.a.x;
        let mut y = self.bounds.a.y;
        for field in &model.fields {
            let label = if field.is_must {
                format!("{} *", field.label)
            } else {
                field.label.clone()
            };
            self.inner.add(Box::new(StaticText::new(
                Rect::new(self.bounds.a.x, y, self.bounds.a.x + 18, y + 1),
                &label,
            )));
            if !self.read_only && field_is_editable(field) {
                let seed = field.values.join("\n");
                let data = Rc::new(RefCell::new(seed));
                let input = InputLine::new(
                    Rect::new(self.bounds.a.x + 19, y, self.bounds.a.x + width - 1, y + 1),
                    1024,
                    data.clone(),
                );
                self.inner.add(Box::new(input));
                self.bindings.push((field.label.clone(), data));
            } else {
                let value = field_display(&field.widget, &field.values);
                self.inner.add(Box::new(StaticText::new(
                    Rect::new(self.bounds.a.x + 19, y, self.bounds.a.x + width - 1, y + 1),
                    &value,
                )));
            }
            y += 1;
        }
        // Save/Cancel buttons (skipped in read-only mode).
        if !self.read_only {
            self.inner.add(Box::new(Button::new(
                Rect::new(self.bounds.a.x, y + 1, self.bounds.a.x + 10, y + 2),
                "~S~ave",
                CM_FORM_SAVE,
                true,
            )));
            self.inner.add(Box::new(Button::new(
                Rect::new(self.bounds.a.x + 12, y + 1, self.bounds.a.x + 22, y + 2),
                "~C~ancel",
                CM_FORM_CANCEL,
                false,
            )));
            y += 2;
        }
        self.content_h = y - self.bounds.a.y;
        self.scroll = 0;
        self.inner.set_initial_focus();
    }

    /// Show "nothing selected".
    pub fn clear(&mut self) {
        self.dn.clear();
        self.bindings.clear();
        self.baseline.clear();
        self.inner = Group::new(self.bounds);
        self.content_h = 0;
        self.scroll = 0;
    }

    /// The DN currently loaded (empty if cleared).
    pub fn dn(&self) -> &str {
        &self.dn
    }

    /// True if any editable binding differs from its baseline.
    pub fn is_dirty(&self) -> bool {
        for (label, data) in &self.bindings {
            let current: Vec<String> = data
                .borrow()
                .split('\n')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            let base = self.baseline.get(label).cloned().unwrap_or_default();
            if current != base {
                return true;
            }
        }
        false
    }

    /// Collect the current edited entry (None if cleared).
    pub fn take_edit(&self) -> Option<EditEntry> {
        if self.dn.is_empty() {
            return None;
        }
        let mut attrs = self.baseline.clone();
        for (label, data) in &self.bindings {
            let values: Vec<String> = data
                .borrow()
                .split('\n')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            attrs.insert(label.clone(), values);
        }
        Some(EditEntry {
            dn: self.dn.clone(),
            attrs,
        })
    }

    /// Revert bindings to baseline (Cancel).
    pub fn revert(&mut self) {
        for (label, data) in &self.bindings {
            let base = self.baseline.get(label).cloned().unwrap_or_default();
            *data.borrow_mut() = base.join("\n");
        }
    }

    fn viewport_h(&self) -> i16 {
        self.bounds.b.y - self.bounds.a.y
    }

    fn apply_scroll(&mut self, new_scroll: i16) {
        let clamped = clamp_scroll(new_scroll, self.content_h, self.viewport_h());
        let dy = clamped - self.scroll;
        if dy != 0 {
            let gb = self.inner.bounds();
            // Pure translation: move a.y and b.y by the same amount (dw=dh=0).
            self.inner
                .set_bounds(Rect::new(gb.a.x, gb.a.y - dy, gb.b.x, gb.b.y - dy));
            self.scroll = clamped;
        }
    }
}
```

`impl View for FormPane` — delegate to `inner` for `draw`/keyboard, add scroll on PgUp/PgDn + wheel, and push a viewport clip in `draw` so off-pane rows don't paint over neighbours. Minimum impl:

```rust
impl View for FormPane {
    fn bounds(&self) -> Rect { self.bounds }

    fn set_bounds(&mut self, b: Rect) {
        self.bounds = b;
        self.inner.set_bounds(b);
    }

    fn draw(&mut self, terminal: &mut Terminal) {
        self.inner.set_palette_chain(self.palette_chain.clone());
        // The host (SplitContainer) clips to this pane's column; the inner Group
        // also clips to its own bounds. Rows scrolled above/below are clipped.
        self.inner.draw(terminal);
    }

    fn handle_event(&mut self, event: &mut Event) {
        match event.what {
            EventType::MouseWheelDown => { self.apply_scroll(self.scroll + 1); event.clear(); return; }
            EventType::MouseWheelUp => { self.apply_scroll(self.scroll - 1); event.clear(); return; }
            EventType::Keyboard => {
                match event.key_code {
                    KB_PGDN => { self.apply_scroll(self.scroll + self.viewport_h()); event.clear(); return; }
                    KB_PGUP => { self.apply_scroll(self.scroll - self.viewport_h()); event.clear(); return; }
                    _ => {}
                }
            }
            _ => {}
        }
        self.inner.handle_event(event);
    }

    fn can_focus(&self) -> bool { true }
    fn state(&self) -> StateFlags { self.state }
    fn set_state(&mut self, s: StateFlags) { self.state = s; }
    fn set_focus(&mut self, focused: bool) {
        self.set_state_flag(SF_FOCUSED, focused);
        if focused { self.inner.set_initial_focus(); }
    }
    fn update_cursor(&self, terminal: &mut Terminal) {
        if let Some(c) = self.inner.focused_child() { c.update_cursor(terminal); }
    }
    fn set_palette_chain(&mut self, n: Option<PaletteChainNode>) { self.palette_chain = n; }
    fn get_palette_chain(&self) -> Option<&PaletteChainNode> { self.palette_chain.as_ref() }
    fn get_palette(&self) -> Option<Palette> { None }
}
```

Add `KB_PGUP, KB_PGDN` to the `turbo_vision::core::event` import line.

- [ ] **Step 4: Build + test + boundary**

Run:
```bash
cargo build && cargo test --lib scroll_clamps_to_content
! grep -rl "use turbo_vision" src | grep -v "src/ui/facade.rs"
```
Expected: build OK, test PASS, grep empty. Resolve any signature drift against the crate (e.g. `Button::new` arity is verified at facade.rs:549; `StaticText::new` and `InputLine::new` likewise already used in this file).

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/ui/facade.rs
git commit -m "feat(ui): FormPane — live scrollable edit form (Group rows + manual scroll)"
```

---

### Task 8: `LeafListPane` — leaf list + incremental search

**Files:**
- Modify: `src/ui/facade.rs`
- Test: none new (pure filtering already covered by `Structure::filter_leaves`); a build + boundary check suffices.

Verified facts (TV report §E): plain `ListBox` (supports downcast, `set_items`, `get_selection`/`get_selected_item`); a separate filter `InputLine`; lists do NOT emit selection-change — poll `get_selection()` after each event. The pane owns the search `InputLine` (top row) and a `ListBox` (rest). It does NOT own the structure; the wiring layer calls `set_rows(labels, dns)` whenever the branch or filter changes.

API the pane exposes:
- `LeafListPane::new(bounds)`.
- `set_rows(&mut self, rows: Vec<(String /*display*/, String /*dn*/)>)` — replaces the list (the wiring layer computes these from `Structure`, prepending the `‹self›` row).
- `search_text(&self) -> String` — current filter box content (the loop reads this to recompute rows).
- `selected_dn(&self) -> Option<String>` — DN of the highlighted row.

- [ ] **Step 1: Implement `LeafListPane`**

```rust
/// Pane 2: an incremental-search `InputLine` over a `ListBox` of the current
/// branch's leaves (plus a `‹self›` row for the branch entry itself). The pane is
/// passive: the wiring layer recomputes rows from the [`Structure`] whenever the
/// branch selection or the search text changes, and reads `selected_dn()` to drive
/// pane 3. Selection changes are detected by polling (lists emit no change event).
pub struct LeafListPane {
    bounds: Rect,
    inner: Group,
    search: Rc<RefCell<String>>,
    /// Parallel to the ListBox items: the DN for each visible row.
    row_dns: Vec<String>,
    state: StateFlags,
    palette_chain: Option<PaletteChainNode>,
}

const CM_LEAF_SEARCH: CommandId = 2300;
const CM_LEAF_SELECT: CommandId = 2301;

impl LeafListPane {
    pub fn new(bounds: Rect) -> Self {
        let search = Rc::new(RefCell::new(String::new()));
        let mut inner = Group::new(bounds);
        // Row 0: "Search:" label + InputLine. Rows 1..: the ListBox.
        inner.add(Box::new(StaticText::new(
            Rect::new(bounds.a.x, bounds.a.y, bounds.a.x + 8, bounds.a.y + 1),
            "Search:",
        )));
        inner.add(Box::new(InputLine::new(
            Rect::new(bounds.a.x + 8, bounds.a.y, bounds.b.x, bounds.a.y + 1),
            256,
            search.clone(),
        )));
        inner.add(Box::new(ListBox::new(
            Rect::new(bounds.a.x, bounds.a.y + 1, bounds.b.x, bounds.b.y),
            CM_LEAF_SELECT,
        )));
        inner.set_initial_focus();
        LeafListPane {
            bounds,
            inner,
            search,
            row_dns: Vec::new(),
            state: 0,
            palette_chain: None,
        }
    }

    /// The ListBox is child index 2 (label, input, listbox).
    fn listbox_mut(&mut self) -> &mut ListBox {
        self.inner
            .child_at_mut(2)
            .as_any_mut()
            .downcast_mut::<ListBox>()
            .expect("child 2 is the ListBox")
    }
    fn listbox(&self) -> &ListBox {
        self.inner
            .child_at(2)
            .as_any()
            .downcast_ref::<ListBox>()
            .expect("child 2 is the ListBox")
    }

    /// Replace the visible rows (display label + DN). Keeps selection at 0.
    pub fn set_rows(&mut self, rows: Vec<(String, String)>) {
        let labels: Vec<String> = rows.iter().map(|(l, _)| l.clone()).collect();
        self.row_dns = rows.into_iter().map(|(_, d)| d).collect();
        self.listbox_mut().set_items(labels);
    }

    /// Current search-box text.
    pub fn search_text(&self) -> String {
        self.search.borrow().clone()
    }

    /// DN of the highlighted row, if any.
    pub fn selected_dn(&self) -> Option<String> {
        self.listbox()
            .get_selection()
            .and_then(|i| self.row_dns.get(i).cloned())
    }
}
```

`impl View for LeafListPane` delegates everything to `inner` (same shape as the other panes: `bounds/set_bounds/draw/handle_event` → inner; `can_focus`→true; `state`/`set_state`; `set_focus`→`inner.set_initial_focus()` when focused; `update_cursor`→focused child; `get_palette`→None; palette_chain stored). Note `as_any_mut`/`as_any` on `ListBox` are overridden in the crate (verified), so the downcast is safe.

- [ ] **Step 2: Build + boundary check**

Run:
```bash
cargo build
! grep -rl "use turbo_vision" src | grep -v "src/ui/facade.rs"
```
Expected: build OK, grep empty. Add `use turbo_vision::views::listbox::ListBox;` to imports.

- [ ] **Step 3: Commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/ui/facade.rs
git commit -m "feat(ui): LeafListPane — leaf ListBox + incremental search box"
```

---

### Task 9: Dirty-guard decision + Shell mount API

**Files:**
- Create: `src/ui/form_state.rs`
- Modify: `src/ui/mod.rs` (`pub mod form_state;`)
- Modify: `src/ui/facade.rs` (add `Shell::mount_split` + a `confirm_guard` helper)
- Test: inline tests in `src/ui/form_state.rs`

- [ ] **Step 1: Write the failing test for the guard decision**

Create `src/ui/form_state.rs`:

```rust
//! Pure decision logic for the "leave a dirty form" guard (spec §5.6).

/// What the user chose in the Save/Discard/Stay dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardChoice {
    Save,
    Discard,
    Stay,
}

/// What the navigation handler should do after consulting the guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardOutcome {
    /// Proceed with the pending navigation (re-spin pane 3).
    Proceed,
    /// Run the save flow first, then proceed.
    SaveThenProceed,
    /// Cancel the navigation; keep editing.
    Cancel,
}

/// Decide what to do when the selection is about to change.
/// Clean forms always proceed; dirty forms route by the user's choice.
pub fn guard_decision(dirty: bool, choice: Option<GuardChoice>) -> GuardOutcome {
    if !dirty {
        return GuardOutcome::Proceed;
    }
    match choice {
        Some(GuardChoice::Save) => GuardOutcome::SaveThenProceed,
        Some(GuardChoice::Discard) => GuardOutcome::Proceed,
        Some(GuardChoice::Stay) | None => GuardOutcome::Cancel,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_form_always_proceeds() {
        assert_eq!(guard_decision(false, None), GuardOutcome::Proceed);
        assert_eq!(
            guard_decision(false, Some(GuardChoice::Stay)),
            GuardOutcome::Proceed
        );
    }

    #[test]
    fn dirty_routes_by_choice() {
        assert_eq!(
            guard_decision(true, Some(GuardChoice::Save)),
            GuardOutcome::SaveThenProceed
        );
        assert_eq!(
            guard_decision(true, Some(GuardChoice::Discard)),
            GuardOutcome::Proceed
        );
        assert_eq!(
            guard_decision(true, Some(GuardChoice::Stay)),
            GuardOutcome::Cancel
        );
        assert_eq!(guard_decision(true, None), GuardOutcome::Cancel);
    }
}
```

- [ ] **Step 2: Run tests (fail), wire module, run (pass)**

Run: `cargo test --lib form_state::` → FAIL (module not declared).
Add `pub mod form_state;` to `src/ui/mod.rs`.
Run again: PASS.

- [ ] **Step 3: Add the facade guard dialog + `mount_split`**

In `facade.rs`, add a three-button confirm that maps to `GuardChoice` (uses `message_box`; reuse the verified `MF_*` constants already imported). Because TV's `message_box` is yes/no style, implement the three-way as a tiny custom `Dialog` with three buttons, returning the chosen `CommandId`:

```rust
use crate::ui::form_state::GuardChoice;

/// Modal Save / Discard / Stay dialog for the dirty-form guard (spec §5.6).
pub fn confirm_guard(app: &mut Application) -> GuardChoice {
    const CM_SAVE: CommandId = 2400;
    const CM_DISCARD: CommandId = 2401;
    const CM_STAY: CommandId = 2402;
    let mut d = Dialog::new(Rect::new(0, 0, 44, 8), "Unsaved changes");
    d.add(Box::new(StaticText::new(
        Rect::new(2, 1, 42, 3),
        "This entry has unsaved changes.",
    )));
    d.add(Box::new(Button::new(Rect::new(2, 4, 14, 5), "~S~ave", CM_SAVE, true)));
    d.add(Box::new(Button::new(Rect::new(15, 4, 28, 5), "~D~iscard", CM_DISCARD, false)));
    d.add(Box::new(Button::new(Rect::new(29, 4, 41, 5), "S~t~ay", CM_STAY, false)));
    d.set_initial_focus();
    match d.execute(app) {
        x if x == CM_SAVE => GuardChoice::Save,
        x if x == CM_DISCARD => GuardChoice::Discard,
        _ => GuardChoice::Stay,
    }
}
```

Add `Shell::mount_split` (replaces `mount_outline`; keep `mount_outline` for now or delete in Task 10). It builds the three panes, wraps them in a `SplitContainer`, and adds it to the desktop. Because panes need to be reachable from the loop, store them behind the facade via accessor methods OR (simpler) keep the SplitContainer as the desktop's single child and add facade free functions that operate on `&mut Application` to read/update panes. To avoid downcast pain, the cleanest design: **Shell owns the three panes' shared handles** (the leaf-list search `Rc`, the form's bindings are internal). For this plan, expose:

```rust
impl Shell {
    /// Mount the three-pane SplitContainer as the desktop's content. Returns
    /// nothing; the loop drives panes via the broadcast commands and the shared
    /// selection handles created here.
    pub fn mount_split(
        &mut self,
        tree_root: BrowserNodeRef,
        read_only: bool,
    ) {
        let db = self.app.desktop.get_bounds();
        let tree = Box::new(DitOutline::new(
            Rect::new(0, 0, 1, 1),
            tree_root,
            self.selection.clone(),
        ));
        let leaves = Box::new(LeafListPane::new(Rect::new(0, 0, 1, 1)));
        let form = Box::new(FormPane::new(Rect::new(0, 0, 1, 1), read_only));
        let split = SplitContainer::new(db, tree, leaves, form);
        self.app.desktop.add(Box::new(split));
    }
}
```

NOTE on pane access from the loop: the simplest robust wiring (Task 10) is to keep the three panes addressable. Two acceptable options — pick one in Task 10 and document it:
- (a) Store `Rc<RefCell<…>>` handles to the leaf-list rows + form model inside `Shell` (like the existing `selection` handle), and have the panes read/write those handles. This keeps `main.rs` turbo-vision-free.
- (b) Add facade free functions `update_leaf_rows(app, rows)`, `form_set_model(app, model)`, etc. that locate the SplitContainer as `desktop`'s child and downcast (SplitContainer would then need `as_any` overridden, and to expose typed accessors to its `inner` children).

Option (a) matches the existing `DitOutline`/`selection` pattern and is recommended.

- [ ] **Step 4: Build + boundary + tests**

Run:
```bash
cargo build && cargo test --lib
! grep -rl "use turbo_vision" src | grep -v "src/ui/facade.rs"
```
Expected: all pass, grep empty.

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/ui/form_state.rs src/ui/mod.rs src/ui/facade.rs
git commit -m "feat(ui): dirty-form guard decision + Save/Discard/Stay dialog + mount_split"
```

---

## PHASE C — wiring + live

### Task 10: Wire eager load + three-pane flow into `run_tui`

**Files:**
- Modify: `src/main.rs` (`run_tui`, ~lines 127–332)
- Modify: `src/ui/facade.rs` (add the shared-handle accessors per Task 9 option (a))

This is the integration task. Design the shared state on `Shell` (option (a)): a `selection` handle (exists), a `current_branch: Rc<RefCell<Option<String>>>`, and command pumping. Implement the loop so:

1. **Startup:** spawn worker → `FetchSubschema` (existing) → `LoadStructure { base, page_size: 500 }` (synchronous `worker.request`, matching the existing `FetchSubschema` startup style). On `StructureError { truncated: true, .. }` fall back to the existing lazy `BrowserState` path and show a status note; on `truncated:false` error, surface and continue with an empty tree.
2. Build `Structure::build(base, nodes mapped from StructureNodeRaw)`. Keep it in a `RefCell` owned by the loop closure.
3. Build the pane-1 `OutlineViewer` tree from `Structure::branch_dns()` (construct `BrowserNode`s; attach children by parent links so the outline shows the branch hierarchy). Mount via `Shell::mount_split(root, config.is_read_only())`.
4. **Pane-1 selection → pane-2 rows:** when the tree selection (the existing `selection` handle / `CM_DIT_ACTIVATE`) changes to a branch DN, set `current_branch`, compute rows = `‹self›` + `Structure::filter_leaves(branch, search_text)`, and push them to the leaf pane.
5. **Search box → pane-2 rows:** each idle tick, read `leaf_pane.search_text()`; if changed since last tick, recompute rows.
6. **Pane-2 selection → pane-3:** each idle tick, read `leaf_pane.selected_dn()`; if it changed, run the guard: if `form.is_dirty()`, call `facade::confirm_guard(app)` and apply `guard_decision`; on `Proceed`/after save, issue a base read (`read_flow.request_entry`) for the new DN and, when the `ReadOutcome::Form` arrives, `form.set_model(&model)`.
7. **Save (`CM_FORM_SAVE`)**: `form.take_edit()` → `validate` → `diff` vs baseline `EditEntry` → confirm/LDIF (reuse) → `submit_save`. On `WriteOk`, re-read + `Structure` reflow (add/remove) + recompute panes.
8. **Cancel (`CM_FORM_CANCEL`)**: `form.revert()`.
9. **Create/Delete**: reuse the existing menu actions; on `WriteOk` update `Structure` (`add_child`/`remove`) and recompute pane-1 branches + pane-2 rows (promote/demote).
10. **Refresh**: a menu entry re-runs `LoadStructure` and rebuilds.

Because the loop closure already cannot pass `Application` to helpers (facade boundary), keep the pure decisions (`guard_decision`, `Structure` ops, `validate`/`diff`/`plan_save`) factored out (they are) and inline only the `app`-touching facade calls — exactly as the current `run_tui` does.

- [ ] **Step 1: Add the shared handles + accessors (facade)**

In `Shell`, add fields `current_branch: Rc<RefCell<Option<String>>>` and store `Rc` handles for the leaf rows + form model so the loop can drive them without downcasting. Provide `Shell` methods or facade free functions: `set_leaf_rows(&mut self, rows)`, `leaf_search_text(&self)`, `leaf_selected_dn(&self)`, `form_set_model(&mut self, &FormModel)`, `form_is_dirty(&self)`, `form_take_edit(&self)`, `form_revert(&mut self)`. Implement them by having `mount_split` keep `Rc<RefCell<…>>` clones of what it needs, mirroring the `selection` handle pattern. (If a pane must be mutated through the desktop view tree, give `SplitContainer` an `as_any_mut` override and typed accessors to its `inner` children; prefer the shared-handle route.)

- [ ] **Step 2: Rewrite `run_tui` startup + loop**

Replace the `mount_outline` call and the lazy-only loop with the flow above. Keep the existing write/idle handling (`Response::WriteOk`/`WriteError`, `read_flow.on_response`) and extend it: after a `WriteOk`, update the `Structure` and recompute panes; route `ReadOutcome::Form` to `form.set_model` (not the modal dialog) when the read was triggered by pane-2 navigation. Keep the old modal `edit_entry_dialog` path ONLY for create (or migrate create to the pane later — out of scope here; create can stay modal in this task to limit blast radius, then optionally move to the pane in a follow-up).

- [ ] **Step 3: Build + all tests + boundary**

Run:
```bash
cargo build && cargo test
! grep -rl "use turbo_vision" src | grep -v "src/ui/facade.rs"
```
Expected: build OK, all tests pass (live ones SKIP), grep empty.

- [ ] **Step 4: Manual smoke (pty)**

Confirm the existing `m3_smoke` pty test still launches + quits cleanly:
Run: `cargo test --test m3_smoke -- --nocapture` (or the actual smoke test name — check `tests/`).
Expected: PASS (alt-screen present, clean quit).

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings
git add src/main.rs src/ui/facade.rs
git commit -m "feat(tui): wire eager three-pane browse/edit flow with dirty guard"
```

---

### Task 11: Live + manual verification of the full flow

**Files:** none (verification task); fix-ups land in the relevant file with their own commit.

- [ ] **Step 1: Start a seeded server**

```bash
scripts/test-ldap.sh start   # ldap://localhost:1389, base dc=example,dc=org, admin pw adminpassword
cargo build --bin edaptor
```

- [ ] **Step 2: Write the try config** (per project memory recipe) to `/tmp/edaptor-try.toml`:

```toml
[server]
uri = "ldap://localhost:1389"
base_dn = "dc=example,dc=org"

[auth]
method = "simple"
bind_dn = "cn=admin,dc=example,dc=org"
password_source = "env:EDAPTOR_PW"

[[profile]]
name = "Users"
object_class = "inetOrgPerson"
rdn_attr = "cn"
search_base = "ou=users,dc=example,dc=org"
show = ["cn","sn","uid","mail","description"]
```

- [ ] **Step 3: Ask the user to run it on a real tty** (the agent cannot drive a tty):

> Please run this in your terminal and confirm the three panes appear, dividers drag with the mouse, selecting a branch lists its leaves, the search box filters, moving the highlight re-spins the form, editing + Save persists (and re-reads), and moving away while dirty prompts Save/Discard/Stay:
>
> `! EDAPTOR_PW=adminpassword /home/oetiker/scratch/cargo-target/debug/edaptor --config /tmp/edaptor-try.toml`

- [ ] **Step 4: Read-only check**

Ask the user to also run with `read_only = true` (or an anonymous bind, no `bind_dn`) and confirm Save/Cancel are absent and fields are read-only.

- [ ] **Step 5: Stop the server**

```bash
scripts/test-ldap.sh stop
```

- [ ] **Step 6: Record results + final commit if fix-ups were needed**

Document any deviations in the commit message; update the project memory `edaptor-project` milestone note after the user confirms.

---

## Self-Review (completed by plan author)

**Spec coverage:** §4.1 components → Tasks 6–8; §5.1 SplitContainer → Task 6; §5.2 eager load → Tasks 1,2,3,5; §5.3 pane-1 → Tasks 3,10; §5.4 pane-2 search → Tasks 3,8,10; §5.5 live form → Task 7; §5.6 dirty guard → Tasks 7,9,10; §5.7 create/delete reflow → Tasks 3,10; §5.8 read-only → Tasks 4,7,9,10; §5.9 refresh → Task 10; §6 config → Task 4; §8 testing → every task's tests + Tasks 5,11. No gaps.

**Placeholder scan:** all code steps contain real code; the two acknowledged judgment points (Task 9 pane-access option a/b, Task 10 create-stays-modal) are explicit design choices with a recommended default, not TODOs.

**Type consistency:** `StructureNodeRaw` (worker) → `StructureInput` (structure) mapping is explicit in Task 10; `Structure`/`StructureNode` method names (`build`, `get`, `branch_dns`, `leaves_of`, `filter_leaves`, `add_child`, `remove`, `root_dn`, `is_branch`) are used consistently; `GuardChoice`/`GuardOutcome`/`guard_decision` consistent across Tasks 9–10; `FormPane`/`LeafListPane` method names consistent across Tasks 7–10; command ids (`CM_FORM_SAVE/CANCEL`, `CM_LEAF_SELECT`) are unique and above existing ids.

**Known scope cut (intentional):** create-entry may remain on the existing modal dialog in Task 10 to bound the integration; moving create into pane 3 is a clean follow-up.
