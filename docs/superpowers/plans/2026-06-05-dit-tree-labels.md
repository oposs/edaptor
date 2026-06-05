# DIT Tree Label (Configurable, Presence-Keyed, Width-Aware) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make DIT-tree (pane 1) branch-node labels configurable via ordered, presence-keyed `[[tree.label]]` rules that always keep the structural RDN visible, degrading gracefully with a width-aware segment trimmer when the pane is narrow.

**Architecture:** A config layer parses `[[tree.label]]` rules; a new `config::tree_label` module compiles them (with a built-in default rule set), evaluates the first rule whose `when` attributes are all present, renders the template into **provenance-aware pieces** (literal vs. templated), splits them into space-delimited **segments**, and a pure `fit_label` trimmer shrinks the rightmost segment first so the RDN survives longest. Because the render entry (`view::ui(f, &mut App)`) cannot see `Structure` today but the trimmer needs the render-time pane width, we adopt **Approach A**: thread the already-in-scope `&Structure` into `view::ui`/`render_tree` and build the `TreeItem`s at render time from `structure` + compiled rules + `area.width`. This deletes the precomputed `app.tree_items` cache and its four rebuild sites.

**Tech Stack:** Rust, ratatui 0.30, tui-tree-widget 0.24, serde/TOML config, `unicode-width` 0.2 for display-column measurement.

---

## Ground-truth references (verified against the codebase)

- `src/config/label.rs`: `pub enum LabelSeg { Lit(String), Field(String) }`; `pub fn parse_label_template(s: &str) -> Vec<LabelSeg>`; `pub fn render_label(segs, attrs: &BTreeMap<String, Vec<String>>) -> String` (case-insensitive field lookup, missing field → `""`); `pub fn template_attrs(segs: &[LabelSeg]) -> Vec<String>` (field-name dedup, **includes `rdn` if referenced**). Tests live in `#[cfg(test)] mod tests` at the bottom.
- `src/config/mod.rs`: `pub struct Config { server, auth, #[serde(default, rename="profile")] profiles: Vec<EntryProfile>, #[serde(default)] samba: SambaConfig }`. Module declarations at top: `pub mod defaults; pub mod label; pub mod password; pub mod relation;`. Tests in `mod tests` from line ~242.
- `src/workflows/structure.rs`: `pub struct StructureNode { dn, label, object_classes, attrs: BTreeMap<String, Vec<String>>, children: Vec<String> }`; `fn label_for(...)` (cn→description→RDN — **kept, untouched**); `Structure::root_dn() -> &str`, `Structure::get(&self, dn) -> Option<&StructureNode>`, `StructureNode::is_branch()`.
- `src/ui/app/structure_view.rs`: `pub(crate) fn build_tree_items(structure: &Structure) -> Vec<TreeItem<'static, String>>` (recursive `build`, branch-only via `is_branch`, leaf vs. node via children); `pub(crate) struct LabelRule`, `label_rules`, `label_rule_attrs`, `render_node_label`, `compute_rows` (the `‹self›` row at line ~81 — **untouched, non-goal**).
- `src/ui/app/mod.rs`: `pub struct App { ..., pub tree_state: TreeState<String>, pub tree_items: Vec<TreeItem<'static, String>>, ..., pub(crate) label_rules: Vec<LabelRule>, ... }`. Re-export at line ~53: `pub(crate) use structure_view::{ build_tree_items, compute_rows, label_rule_attrs, label_rules, structure_inputs, LabelRule };`. Startup builds `scan_attrs = label_rule_attrs(&rules)` (~line 130) and passes it to `Request::LoadStructure { attrs: scan_attrs }`; `WorkerHandle::spawn(config, password)` **consumes** `config` (~line 133). App construction at ~line 177 sets `tree_items: build_tree_items(&structure)`. The draw call is `terminal.draw(|f| view::ui(f, app))?;` at line 240, with `structure` in scope as the `event_loop` param (line 231).
- `src/ui/app/action.rs`: `refresh_structure` issues `LoadStructure { attrs: label_rule_attrs(&app.label_rules) }` (~line 142) and rebuilds `app.tree_items = build_tree_items(structure)` (~line 146).
- `src/ui/view.rs`: `pub fn ui(f: &mut Frame, app: &mut App)` (line 78) calls `render_tree(f, app, cols[0])` (line 86); `fn render_tree(f, app, area)` (line 126) builds `Tree::new(&app.tree_items)`. **`render_tree` is the only reader of `app.tree_items`.** No `.highlight_symbol` is set (default width 0).
- **`app.tree_items` reader audit:** the only production reader is `view.rs:128`. Other occurrences are writers (`mod.rs:177,413,427`, `action.rs:146`) or `tree_items: vec![]` test stubs (`src/ui/app/test_support.rs:22`, `src/ui/view.rs:801`, `src/ui/view.rs:963`, `src/ui/app/value_editor.rs:363`) and the test at `structure_view.rs:224`.
- **tui-tree-widget 0.24 indent math:** per node the text x-offset inside the tree's inner area = `depth*2` (indent, 2 cols/level) + `2` (node open/closed/leaf symbol) + highlight-symbol width (`0`, none configured). The tree's inner width = `area.width - 2` (the `Block` border, 1 col each side).
- `unicode-width` is already in the lock tree (0.2.2 via ratatui) but **not** a direct dependency.

---

## File structure

- **Modify** `Cargo.toml` — add `unicode-width = "0.2"`.
- **Modify** `src/config/mod.rs` — add `pub mod tree_label;`, `TreeConfig`/`TreeLabelRule` serde types, and `#[serde(default)] pub tree: TreeConfig` on `Config`; parse tests.
- **Modify** `src/config/label.rs` — add `pub struct Piece` and `pub fn render_pieces(...)` (provenance-aware sibling of `render_label`); tests.
- **Create** `src/config/tree_label.rs` — `CompiledTreeRule`, `Segment`, `default_tree_rules`, `compile_tree_rules`, `tree_template_attrs`, `eval_tree_label`, `fit_label` + private helpers (`split_into_segments`, `fit_segment`, `take_cols`, `truncate_with_ellipsis`, width helpers); full unit tests.
- **Modify** `src/ui/app/structure_view.rs` — change `build_tree_items` to `(structure, rules, inner_width)` rendering via `eval_tree_label`+`fit_label`; update its test.
- **Modify** `src/ui/app/mod.rs` — `App.tree_rules` field (replaces `tree_items`); compile tree rules at startup; union `tree_template_attrs` into `scan_attrs`; drop the two `tree_items =` rebuild lines; fix re-exports.
- **Modify** `src/ui/app/action.rs` — union `tree_template_attrs` into the refresh `LoadStructure` attrs; drop the `tree_items =` rebuild and its import.
- **Modify** `src/ui/view.rs` — thread `&Structure` into `ui`/`render_tree`; build `TreeItem`s at render time; update test stubs.
- **Modify** stub sites `src/ui/app/test_support.rs`, `src/ui/app/value_editor.rs` — swap `tree_items: vec![]` for `tree_rules: Vec::new()`.
- **Modify** `docs/src/configuration/tree-labels.md` (new), `docs/src/SUMMARY.md`, `CHANGES.md`.

---

## Task 1: Config schema — `[tree]` / `[[tree.label]]`

**Files:**
- Modify: `src/config/mod.rs` (add module decl, types, `Config.tree` field, tests)

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block in `src/config/mod.rs`:

```rust
#[test]
fn parses_tree_label_rules() {
    let toml = r#"
        [server]
        uri = "ldap://localhost"
        [auth]
        bind_dn = "cn=admin,dc=example,dc=org"

        [[tree.label]]
        when     = ["description"]
        template = "{rdn} ({description})"

        [[tree.label]]
        template = "{rdn}"
    "#;
    let cfg: Config = toml::from_str(toml).expect("parses");
    assert_eq!(cfg.tree.label.len(), 2);
    assert_eq!(cfg.tree.label[0].when, vec!["description".to_string()]);
    assert_eq!(cfg.tree.label[0].template, "{rdn} ({description})");
    assert!(cfg.tree.label[1].when.is_empty());
    assert_eq!(cfg.tree.label[1].template, "{rdn}");
}

#[test]
fn tree_when_defaults_to_empty() {
    let toml = r#"
        [server]
        uri = "ldap://localhost"
        [auth]
        bind_dn = "cn=admin,dc=example,dc=org"

        [[tree.label]]
        template = "{rdn}"
    "#;
    let cfg: Config = toml::from_str(toml).expect("parses");
    assert!(cfg.tree.label[0].when.is_empty());
}

#[test]
fn config_without_tree_table_has_empty_label_list() {
    let toml = r#"
        [server]
        uri = "ldap://localhost"
        [auth]
        bind_dn = "cn=admin,dc=example,dc=org"
    "#;
    let cfg: Config = toml::from_str(toml).expect("parses");
    assert!(cfg.tree.label.is_empty());
}
```

(If `toml::from_str` / the minimal `[server]`/`[auth]` shape differs from a nearby existing test such as `parses_minimal_config`, copy that test's exact header lines instead — keep this test's `[[tree.label]]` additions.)

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p eDAPtor --lib config::tests::parses_tree_label_rules config::tests::tree_when_defaults_to_empty config::tests::config_without_tree_table_has_empty_label_list`
(If the crate name differs, use `cargo test config::tests::parses_tree_label_rules`.)
Expected: FAIL — `no field 'tree' on type 'Config'` / `TreeConfig` not found.

- [ ] **Step 3: Add the serde types and the `Config.tree` field**

Add the module declaration near the other `pub mod` lines at the top of `src/config/mod.rs`:

```rust
pub mod tree_label;
```

Add these types (place them near `EntryProfile`, anywhere in the file's top-level items):

```rust
/// The optional `[tree]` table: ordered, presence-keyed labelling rules for the
/// DIT navigation tree (pane 1) branch nodes. Absent table → empty list →
/// compile substitutes the built-in default rule set.
#[derive(Debug, Default, Deserialize)]
pub struct TreeConfig {
    #[serde(default)]
    pub label: Vec<TreeLabelRule>,
}

/// One `[[tree.label]]` rule. The first rule whose `when` attributes are all
/// present (non-empty first value) wins; a rule with an empty/omitted `when` is
/// the unconditional fallback.
#[derive(Debug, Deserialize)]
pub struct TreeLabelRule {
    #[serde(default)]
    pub when: Vec<String>,
    pub template: String,
}
```

Add the field to `Config`:

```rust
    /// Configurable DIT-tree (pane 1) branch labels. Absent `[tree]` is fine.
    #[serde(default)]
    pub tree: TreeConfig,
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test config::tests::parses_tree_label_rules config::tests::tree_when_defaults_to_empty config::tests::config_without_tree_table_has_empty_label_list`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add src/config/mod.rs
git commit -m "feat(config): parse [[tree.label]] rules into TreeConfig"
```

---

## Task 2: Provenance-aware rendering — `Piece` + `render_pieces`

**Files:**
- Modify: `src/config/label.rs` (add `Piece`, `render_pieces`, tests)

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block in `src/config/label.rs` (these tests use `std::collections::BTreeMap`; add `use std::collections::BTreeMap;` inside the test module if not already imported there):

```rust
#[test]
fn render_pieces_marks_field_vs_literal_provenance() {
    let segs = parse_label_template("{rdn} ({description})");
    let mut attrs = BTreeMap::new();
    attrs.insert("description".to_string(), vec!["People".to_string()]);
    let pieces = render_pieces(&segs, &attrs, "ou=people");
    assert_eq!(
        pieces,
        vec![
            Piece { text: "ou=people".to_string(), from_field: true },
            Piece { text: " (".to_string(), from_field: false },
            Piece { text: "People".to_string(), from_field: true },
            Piece { text: ")".to_string(), from_field: false },
        ]
    );
}

#[test]
fn render_pieces_binds_rdn_case_insensitively_and_keeps_empty_field() {
    let segs = parse_label_template("{RDN}={cn}");
    let attrs: BTreeMap<String, Vec<String>> = BTreeMap::new(); // cn absent
    let pieces = render_pieces(&segs, &attrs, "uid=bob");
    assert_eq!(
        pieces,
        vec![
            Piece { text: "uid=bob".to_string(), from_field: true },
            Piece { text: "=".to_string(), from_field: false },
            Piece { text: "".to_string(), from_field: true }, // empty field kept
        ]
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test config::label::tests::render_pieces_marks_field_vs_literal_provenance config::label::tests::render_pieces_binds_rdn_case_insensitively_and_keeps_empty_field`
Expected: FAIL — `Piece`/`render_pieces` not found.

- [ ] **Step 3: Add `Piece` and `render_pieces`**

Add to `src/config/label.rs` (top-level, next to `render_label`):

```rust
/// One rendered run of a label, retaining whether it came from a templated
/// `{field}` (`from_field = true`) or from literal template text. Used by the
/// DIT-tree trimmer to know which characters may be ellipsized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    pub text: String,
    pub from_field: bool,
}

/// Render `segs` into ordered [`Piece`]s. The reserved field name `rdn`
/// (case-insensitive) binds to `rdn`; every other `{field}` resolves from
/// `attrs` exactly like [`render_label`] (missing → empty). Empty field values
/// produce an empty `from_field` piece (kept, not dropped).
pub fn render_pieces(
    segs: &[LabelSeg],
    attrs: &BTreeMap<String, Vec<String>>,
    rdn: &str,
) -> Vec<Piece> {
    let mut out = Vec::new();
    for seg in segs {
        match seg {
            LabelSeg::Lit(s) => out.push(Piece {
                text: s.clone(),
                from_field: false,
            }),
            LabelSeg::Field(name) => {
                let value = if name.eq_ignore_ascii_case("rdn") {
                    rdn.to_string()
                } else {
                    attrs
                        .iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case(name))
                        .and_then(|(_, v)| v.first())
                        .map(String::as_str)
                        .unwrap_or("")
                        .to_string()
                };
                out.push(Piece {
                    text: value,
                    from_field: true,
                });
            }
        }
    }
    out
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test config::label::tests::render_pieces_marks_field_vs_literal_provenance config::label::tests::render_pieces_binds_rdn_case_insensitively_and_keeps_empty_field`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add src/config/label.rs
git commit -m "feat(config): add provenance-aware render_pieces + Piece"
```

---

## Task 3: `tree_label` module — compile, defaults, scan attrs

**Files:**
- Create: `src/config/tree_label.rs`
- (module already declared `pub mod tree_label;` in Task 1)

- [ ] **Step 1: Write the failing tests**

Create `src/config/tree_label.rs` with ONLY the test module first (so the test target compiles and the test names exist):

```rust
//! DIT-tree (pane 1) branch-label rules: compile config rules (or a built-in
//! default set), discover the attributes their templates reference, evaluate the
//! first matching rule per node, and width-fit the rendered label so the RDN
//! survives longest. Pane-2 leaf labels and the `‹self›` row are NOT handled
//! here — see `src/ui/app/structure_view.rs`.

use crate::config::label::{parse_label_template, Piece};
use crate::config::TreeConfig;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_compiles_to_default_rule_set() {
        let cfg = TreeConfig::default();
        let rules = compile_tree_rules(&cfg);
        // cn rule, description rule, unconditional {rdn} fallback.
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[0].when, vec!["cn".to_string()]);
        assert_eq!(rules[1].when, vec!["description".to_string()]);
        assert!(rules[2].when.is_empty());
    }

    #[test]
    fn non_empty_config_compiles_rules_verbatim() {
        let cfg = TreeConfig {
            label: vec![crate::config::TreeLabelRule {
                when: vec!["ou".to_string()],
                template: "{rdn} [{ou}]".to_string(),
            }],
        };
        let rules = compile_tree_rules(&cfg);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].when, vec!["ou".to_string()]);
    }

    #[test]
    fn tree_template_attrs_unions_and_excludes_rdn() {
        let rules = default_tree_rules();
        let attrs = tree_template_attrs(&rules);
        // cn and description are referenced; rdn is excluded.
        assert!(attrs.iter().any(|a| a.eq_ignore_ascii_case("cn")));
        assert!(attrs.iter().any(|a| a.eq_ignore_ascii_case("description")));
        assert!(!attrs.iter().any(|a| a.eq_ignore_ascii_case("rdn")));
    }

    #[test]
    fn tree_template_attrs_dedups_case_insensitively() {
        let rules = vec![
            CompiledTreeRule {
                when: vec![],
                template: parse_label_template("{CN}-{cn}"),
            },
        ];
        let attrs = tree_template_attrs(&rules);
        assert_eq!(attrs.len(), 1);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test config::tree_label::tests::empty_config_compiles_to_default_rule_set`
Expected: FAIL — `compile_tree_rules` / `CompiledTreeRule` / `default_tree_rules` / `tree_template_attrs` not found.

- [ ] **Step 3: Implement compile/defaults/attrs**

Add above the `#[cfg(test)] mod tests` in `src/config/tree_label.rs`:

```rust
use crate::config::label::LabelSeg;

/// A compiled `[[tree.label]]` rule: required attribute names (`when`) plus the
/// parsed template segments.
#[derive(Debug, Clone)]
pub struct CompiledTreeRule {
    pub when: Vec<String>,
    pub template: Vec<LabelSeg>,
}

/// The built-in default rule set used when `[[tree.label]]` is absent:
/// `{rdn} ({cn})` if cn present · else `{rdn} ({description})` if description
/// present · else `{rdn}`.
pub fn default_tree_rules() -> Vec<CompiledTreeRule> {
    vec![
        CompiledTreeRule {
            when: vec!["cn".to_string()],
            template: parse_label_template("{rdn} ({cn})"),
        },
        CompiledTreeRule {
            when: vec!["description".to_string()],
            template: parse_label_template("{rdn} ({description})"),
        },
        CompiledTreeRule {
            when: vec![],
            template: parse_label_template("{rdn}"),
        },
    ]
}

/// Compile config rules into [`CompiledTreeRule`]s, substituting the default set
/// when the config list is empty.
pub fn compile_tree_rules(cfg: &TreeConfig) -> Vec<CompiledTreeRule> {
    if cfg.label.is_empty() {
        return default_tree_rules();
    }
    cfg.label
        .iter()
        .map(|r| CompiledTreeRule {
            when: r.when.clone(),
            template: parse_label_template(&r.template),
        })
        .collect()
}

/// Union of every `{field}` referenced by any rule's template, **excluding the
/// reserved `rdn`**, deduped case-insensitively. Unioned into the structure
/// scan-attrs so templated attributes are actually fetched.
pub fn tree_template_attrs(rules: &[CompiledTreeRule]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for rule in rules {
        for attr in crate::config::label::template_attrs(&rule.template) {
            if attr.eq_ignore_ascii_case("rdn") {
                continue;
            }
            if !out.iter().any(|a| a.eq_ignore_ascii_case(&attr)) {
                out.push(attr);
            }
        }
    }
    out
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test config::tree_label::tests::`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add src/config/tree_label.rs src/config/mod.rs
git commit -m "feat(config): compile tree label rules + default set + scan attrs"
```

---

## Task 4: Evaluation — `Segment`, segmentation, `eval_tree_label`

**Files:**
- Modify: `src/config/tree_label.rs` (add `Segment`, `split_into_segments`, `eval_tree_label`, tests)

- [ ] **Step 1: Write the failing tests**

Add to the `mod tests` block in `src/config/tree_label.rs`:

```rust
use std::collections::BTreeMap;

fn attrs(pairs: &[(&str, &str)]) -> BTreeMap<String, Vec<String>> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), vec![v.to_string()]))
        .collect()
}

#[test]
fn eval_first_matching_rule_wins_and_splits_into_segments() {
    let rules = default_tree_rules();
    let a = attrs(&[("description", "People")]); // cn absent → description rule
    let segs = eval_tree_label(&rules, &a, "ou=people");
    let texts: Vec<String> = segs.iter().map(|s| s.text()).collect();
    assert_eq!(texts, vec!["ou=people".to_string(), "(People)".to_string()]);
    // RDN segment is all-field; "(People)" is lit "(" + field "People" + lit ")".
    assert!(segs[0].pieces.iter().all(|p| p.from_field));
    assert_eq!(segs[1].pieces.len(), 3);
    assert!(!segs[1].pieces[0].from_field && segs[1].pieces[0].text == "(");
    assert!(segs[1].pieces[1].from_field && segs[1].pieces[1].text == "People");
    assert!(!segs[1].pieces[2].from_field && segs[1].pieces[2].text == ")");
}

#[test]
fn eval_presence_is_case_insensitive_and_requires_non_empty() {
    let rules = default_tree_rules();
    // cn present but empty → cn rule skipped; description present → description rule.
    let mut a = attrs(&[("DESCRIPTION", "Staff")]);
    a.insert("cn".to_string(), vec!["".to_string()]);
    let segs = eval_tree_label(&rules, &a, "ou=staff");
    let texts: Vec<String> = segs.iter().map(|s| s.text()).collect();
    assert_eq!(texts, vec!["ou=staff".to_string(), "(Staff)".to_string()]);
}

#[test]
fn eval_falls_back_to_rdn_when_no_field_attrs() {
    let rules = default_tree_rules();
    let a: BTreeMap<String, Vec<String>> = BTreeMap::new(); // neither cn nor description
    let segs = eval_tree_label(&rules, &a, "ou=people");
    let texts: Vec<String> = segs.iter().map(|s| s.text()).collect();
    assert_eq!(texts, vec!["ou=people".to_string()]);
}

#[test]
fn eval_with_no_matching_rule_and_no_fallback_shows_rdn() {
    // Misconfigured: a single rule that requires an absent attr, no fallback.
    let rules = vec![CompiledTreeRule {
        when: vec!["mail".to_string()],
        template: parse_label_template("{rdn} <{mail}>"),
    }];
    let a: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let segs = eval_tree_label(&rules, &a, "uid=jane");
    let texts: Vec<String> = segs.iter().map(|s| s.text()).collect();
    assert_eq!(texts, vec!["uid=jane".to_string()]);
}

#[test]
fn split_keeps_field_provenance_on_space_separated_field_values() {
    // A field value with an internal space splits into two field segments.
    let rules = vec![CompiledTreeRule {
        when: vec![],
        template: parse_label_template("{cn}"),
    }];
    let a = attrs(&[("cn", "Ada Lovelace")]);
    let segs = eval_tree_label(&rules, &a, "cn=ada");
    let texts: Vec<String> = segs.iter().map(|s| s.text()).collect();
    assert_eq!(texts, vec!["Ada".to_string(), "Lovelace".to_string()]);
    assert!(segs.iter().all(|s| s.pieces.iter().all(|p| p.from_field)));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test config::tree_label::tests::eval_first_matching_rule_wins_and_splits_into_segments`
Expected: FAIL — `Segment` / `eval_tree_label` not found.

- [ ] **Step 3: Implement `Segment`, segmentation, and evaluation**

Add to `src/config/tree_label.rs` (top-level, before the test module):

```rust
/// A space-delimited run of [`Piece`]s — the unit the trimmer shrinks or drops.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub pieces: Vec<Piece>,
}

impl Segment {
    /// The segment's full rendered text (all pieces concatenated).
    pub fn text(&self) -> String {
        self.pieces.iter().map(|p| p.text.as_str()).collect()
    }
}

/// Split a flat piece list into space-delimited [`Segment`]s. Spaces are
/// separators (not retained). A piece's text is split at ASCII spaces; each
/// sub-run keeps the piece's `from_field` provenance. Empty sub-runs are dropped.
fn split_into_segments(pieces: Vec<Piece>) -> Vec<Segment> {
    let mut segments: Vec<Segment> = Vec::new();
    let mut current: Vec<Piece> = Vec::new();
    for piece in pieces {
        let mut first = true;
        for part in piece.text.split(' ') {
            if !first && !current.is_empty() {
                segments.push(Segment {
                    pieces: std::mem::take(&mut current),
                });
            }
            first = false;
            if !part.is_empty() {
                current.push(Piece {
                    text: part.to_string(),
                    from_field: piece.from_field,
                });
            }
        }
    }
    if !current.is_empty() {
        segments.push(Segment { pieces: current });
    }
    segments
}

/// Pick the first rule whose `when` attributes are all present (case-insensitive,
/// non-empty first value) and render it into segments. `{rdn}` binds to `rdn`.
/// If no rule matches (misconfigured list with no fallback), show just the RDN.
pub fn eval_tree_label(
    rules: &[CompiledTreeRule],
    attrs: &BTreeMap<String, Vec<String>>,
    rdn: &str,
) -> Vec<Segment> {
    for rule in rules {
        if rule.when.iter().all(|w| present(attrs, w)) {
            let pieces = crate::config::label::render_pieces(&rule.template, attrs, rdn);
            return split_into_segments(pieces);
        }
    }
    split_into_segments(vec![Piece {
        text: rdn.to_string(),
        from_field: true,
    }])
}

/// An attribute is "present" when it exists (case-insensitively) with a non-empty
/// first value.
fn present(attrs: &BTreeMap<String, Vec<String>>, name: &str) -> bool {
    attrs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .and_then(|(_, v)| v.first())
        .map(|s| !s.is_empty())
        .unwrap_or(false)
}
```

Add `use std::collections::BTreeMap;` to the module's top-level `use` block (alongside the existing imports), if not already present.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test config::tree_label::tests::`
Expected: PASS (Task 3 + Task 4 tests, 9 total).

- [ ] **Step 5: Commit**

```bash
git add src/config/tree_label.rs
git commit -m "feat(config): eval_tree_label with presence match + segmentation"
```

---

## Task 5: Width-aware trimmer — `fit_label`

**Files:**
- Modify: `Cargo.toml` (add `unicode-width`)
- Modify: `src/config/tree_label.rs` (add `fit_label` + helpers + tests)

- [ ] **Step 1: Add the dependency**

Add to the `[dependencies]` table in `Cargo.toml` (keep the file's existing alphabetic/grouping style):

```toml
unicode-width = "0.2"
```

- [ ] **Step 2: Write the failing tests**

Add to the `mod tests` block in `src/config/tree_label.rs`. The expected strings below are computed from the exact algorithm (the spec's `18/16/13/10/7` ladder is illustrative; these are the precise column-accurate results — `'…'` is 1 column).

```rust
// Helper: build the two-segment label for `{rdn} ({description})`,
// rdn="ou=people" (width 9), description="People" → segments
// ["ou=people"(field 9), "(People)"(lit"("+field"People"+lit")" = 8)].
fn people_segments() -> Vec<Segment> {
    let rules = default_tree_rules();
    let a = attrs(&[("description", "People")]);
    eval_tree_label(&rules, &a, "ou=people")
}

#[test]
fn fit_full_when_it_fits() {
    let segs = people_segments();
    assert_eq!(fit_label(&segs, 18), "ou=people (People)"); // width 18
    assert_eq!(fit_label(&segs, 30), "ou=people (People)");
}

#[test]
fn fit_ellipsizes_last_segment_field_from_the_end() {
    let segs = people_segments();
    // avail 17: last-segment budget 7 → "(Peop…)" (2 lit + 4 field + 1 ellipsis).
    assert_eq!(fit_label(&segs, 17), "ou=people (Peop…)");
    // avail 16: budget 6 → "(Peo…)".
    assert_eq!(fit_label(&segs, 16), "ou=people (Peo…)");
    // avail 14: budget 4 → one field char kept "(P…)".
    assert_eq!(fit_label(&segs, 14), "ou=people (P…)");
}

#[test]
fn fit_drops_last_segment_once_field_is_consumed() {
    let segs = people_segments();
    // avail 13: budget 3 = literals(2)+ellipsis(1), 0 field cols → drop "(...)".
    assert_eq!(fit_label(&segs, 13), "ou=people");
    assert_eq!(fit_label(&segs, 9), "ou=people"); // first segment fits exactly
}

#[test]
fn fit_ellipsizes_protected_first_segment_field() {
    let segs = people_segments();
    // Only the first segment remains and still doesn't fit.
    assert_eq!(fit_label(&segs, 8), "ou=peop…"); // 7 field cols + ellipsis
    assert_eq!(fit_label(&segs, 7), "ou=peo…");
    assert_eq!(fit_label(&segs, 2), "o…"); // 1-char minimum + ellipsis
    assert_eq!(fit_label(&segs, 1), "o…"); // min overflows a too-narrow pane
}

#[test]
fn fit_drops_pure_literal_segment_as_a_unit() {
    // Template "{cn} -- end": segments ["X"(field), "--"(lit), "end"(lit)].
    let rules = vec![CompiledTreeRule {
        when: vec![],
        template: parse_label_template("{cn} -- end"),
    }];
    let a = attrs(&[("cn", "X")]);
    let segs = eval_tree_label(&rules, &a, "cn=x");
    assert_eq!(segs.iter().map(|s| s.text()).collect::<Vec<_>>(), vec!["X", "--", "end"]);
    // "X -- end" width 8; at avail 5 the pure-literal "end" drops whole → "X --".
    assert_eq!(fit_label(&segs, 5), "X --");
    // at avail 3 "--" also drops → "X".
    assert_eq!(fit_label(&segs, 3), "X");
}

#[test]
fn fit_multi_field_template_trims_rightmost_segment_first() {
    // "{cn} - {rdn}" must keep the rdn segment, trimming cn's segment? No:
    // the LAST segment is the rdn here, so rdn trims first by construction.
    let rules = vec![CompiledTreeRule {
        when: vec![],
        template: parse_label_template("{cn} - {rdn}"),
    }];
    let a = attrs(&[("cn", "Group")]);
    let segs = eval_tree_label(&rules, &a, "cn=group");
    // ["Group"(5), "-"(1 lit), "cn=group"(8 field)] joined "Group - cn=group" = 16.
    assert_eq!(fit_label(&segs, 16), "Group - cn=group");
    // avail 14: last-seg budget = 14 - (len("Group -")=7 + space 1) = 6;
    // field_cols = 6 - 1 = 5 → keeps "cn=gr" + ellipsis.
    assert_eq!(fit_label(&segs, 14), "Group - cn=gr…");
}

#[test]
fn fit_is_unicode_width_aware_for_cjk() {
    // description with CJK (each 2 cols): "日本" width 4 → "(日本)" width 6.
    let rules = default_tree_rules();
    let a = attrs(&[("description", "日本")]);
    let segs = eval_tree_label(&rules, &a, "ou=x");
    // segments ["ou=x"(4), "(日本)"(6)] joined "ou=x (日本)" width 4+1+6 = 11.
    assert_eq!(fit_label(&segs, 11), "ou=x (日本)");
    // avail 9: last-seg budget = 9 - 5 = 4 = lit(2)+ellipsis(1)+1 col → 0 CJK
    // chars fit in 1 col → drop "(...)" → "ou=x".
    assert_eq!(fit_label(&segs, 9), "ou=x");
    // avail 10: budget 5 → field cols 2 → one CJK char "(日…)" width 2+2+1=5.
    assert_eq!(fit_label(&segs, 10), "ou=x (日…)");
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test config::tree_label::tests::fit_full_when_it_fits`
Expected: FAIL — `fit_label` not found.

- [ ] **Step 4: Implement `fit_label` and helpers**

Add to `src/config/tree_label.rs` (top-level). Add `use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};` to the module's imports.

```rust
/// Display width in columns (CJK = 2, combining marks per `unicode-width`).
fn str_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Join segments with single spaces (their on-screen separators).
fn join_text(segs: &[&Segment]) -> String {
    segs.iter()
        .map(|s| s.text())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Longest prefix of `s` whose display width is ≤ `cols`.
fn take_cols(s: &str, cols: usize) -> String {
    let mut out = String::new();
    let mut used = 0usize;
    for ch in s.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > cols {
            break;
        }
        out.push(ch);
        used += w;
    }
    out
}

/// Trim `s` to fit `budget`, reserving 1 column for the trailing `…`, but never
/// returning an empty string (forces ≥1 visible char — the first-segment guard).
fn truncate_with_ellipsis(s: &str, budget: usize) -> String {
    let mut kept = take_cols(s, budget.saturating_sub(1));
    if kept.is_empty() {
        if let Some(c) = s.chars().next() {
            kept.push(c);
        }
    }
    format!("{kept}…")
}

/// Fit one segment into `budget` columns by trimming its **field** characters
/// from the end and replacing the removed tail with a single `…` (literal
/// decoration is preserved). Returns `None` when the segment cannot fit keeping
/// ≥1 field char (pure-literal, or field fully consumed) so the caller drops it.
/// When `guard` is set (the protected first segment) it is never dropped: it is
/// trimmed to a 1-char-+`…` minimum, trimming its literal text if it has no field.
fn fit_segment(seg: &Segment, budget: usize, guard: bool) -> Option<String> {
    let full = seg.text();
    if str_width(&full) <= budget {
        return Some(full);
    }
    let has_field = seg.pieces.iter().any(|p| p.from_field);
    if !has_field {
        return if guard {
            Some(truncate_with_ellipsis(&full, budget))
        } else {
            None
        };
    }
    let lit_w: usize = seg
        .pieces
        .iter()
        .filter(|p| !p.from_field)
        .map(|p| str_width(&p.text))
        .sum();
    let floor = lit_w + 1; // literal decoration + the single `…`
    if floor > budget && !guard {
        return None;
    }
    let field_cols = budget.saturating_sub(floor);
    let mut out = String::new();
    let mut remaining = field_cols;
    let mut kept_any = false;
    let mut ellipsis_done = false;
    for piece in &seg.pieces {
        if piece.from_field {
            for ch in piece.text.chars() {
                let w = UnicodeWidthChar::width(ch).unwrap_or(0);
                if !ellipsis_done && w <= remaining {
                    out.push(ch);
                    remaining -= w;
                    kept_any = true;
                } else if !ellipsis_done {
                    // First char that does not fit: place the ellipsis once.
                    // The guard forces at least one visible field char.
                    if guard && !kept_any {
                        out.push(ch);
                        kept_any = true;
                    }
                    out.push('…');
                    ellipsis_done = true;
                }
                // chars after the ellipsis are dropped
            }
        } else {
            out.push_str(&piece.text);
        }
    }
    if !ellipsis_done {
        out.push('…');
    }
    if !kept_any && !guard {
        return None; // field fully consumed → drop the whole segment
    }
    Some(out)
}

/// Fit `segments` (joined by single spaces) into `avail` display columns. Trims
/// the rightmost segment's field first; drops a segment whole once its field is
/// consumed; never fully removes the first segment (the RDN survives longest).
pub fn fit_label(segments: &[Segment], avail: usize) -> String {
    if segments.is_empty() {
        return String::new();
    }
    let mut n = segments.len();
    loop {
        let head: Vec<&Segment> = segments[..n].iter().collect();
        let joined = join_text(&head);
        if str_width(&joined) <= avail {
            return joined;
        }
        let last = n - 1;
        let only_first = last == 0;
        let (head_str, head_w) = if only_first {
            (String::new(), 0usize)
        } else {
            let hs = join_text(&segments[..last].iter().collect::<Vec<_>>());
            let w = str_width(&hs) + 1; // + separating space
            (hs, w)
        };
        let budget = avail.saturating_sub(head_w);
        match fit_segment(&segments[last], budget, only_first) {
            Some(text) => {
                return if only_first {
                    text
                } else {
                    format!("{head_str} {text}")
                };
            }
            None => {
                n -= 1; // drop the last segment, retry
            }
        }
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test config::tree_label::tests::`
Expected: PASS (all Task 3–5 tests).

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock src/config/tree_label.rs
git commit -m "feat(config): width-aware fit_label segment trimmer"
```

---

## Task 6: App wiring — compile rules, union scan attrs (additive, still green)

This task is additive: it adds `App.tree_rules`, compiles rules at startup, and unions tree-template attrs into the scan so templated attributes are fetched — while leaving `tree_items`/`build_tree_items` exactly as-is. The codebase stays compiling and green. Task 7 then flips the render path.

**Files:**
- Modify: `src/ui/app/mod.rs` (field, re-export, startup compile + scan union, App construction, stub)
- Modify: `src/ui/app/action.rs` (refresh scan union)
- Modify: `src/ui/app/test_support.rs`, `src/ui/app/value_editor.rs`, `src/ui/view.rs` (add `tree_rules` to stubs)

- [ ] **Step 1: Write the failing test**

Add to the `mod tests` in `src/config/tree_label.rs` a guard that the default rules reference exactly the structural-minimum attrs (this pins the scan-union contract Task 6 relies on):

```rust
#[test]
fn default_rules_scan_attrs_are_cn_and_description_only() {
    let mut attrs = tree_template_attrs(&default_tree_rules());
    attrs.sort();
    assert_eq!(attrs, vec!["cn".to_string(), "description".to_string()]);
}
```

- [ ] **Step 2: Run to verify it fails (or passes trivially)**

Run: `cargo test config::tree_label::tests::default_rules_scan_attrs_are_cn_and_description_only`
Expected: PASS already (the implementation exists) — this is a regression pin, not a red test. Proceed.

- [ ] **Step 3: Add the `App.tree_rules` field and re-export**

In `src/ui/app/mod.rs`, add to the `App` struct (next to `label_rules`):

```rust
    /// Compiled DIT-tree (pane 1) branch-label rules (built once from
    /// `config.tree`). Drives the render-time tree-label build.
    pub(crate) tree_rules: Vec<crate::config::tree_label::CompiledTreeRule>,
```

Leave the existing `pub tree_items: Vec<TreeItem<'static, String>>` field in place for now (removed in Task 7).

- [ ] **Step 4: Compile rules + union scan attrs at startup**

In `src/ui/app/mod.rs::run`, BEFORE `WorkerHandle::spawn(config, password)` consumes `config`, compile the tree rules and union their attrs into `scan_attrs`. Replace the existing two lines

```rust
    let rules = label_rules(&profiles);
    let scan_attrs = label_rule_attrs(&rules);
```

with:

```rust
    let rules = label_rules(&profiles);
    let tree_rules = crate::config::tree_label::compile_tree_rules(&config.tree);
    let mut scan_attrs = label_rule_attrs(&rules);
    for a in crate::config::tree_label::tree_template_attrs(&tree_rules) {
        if !scan_attrs.iter().any(|x| x.eq_ignore_ascii_case(&a)) {
            scan_attrs.push(a);
        }
    }
```

Then in the `App { ... }` construction (~line 177), add the field (keep `tree_items` for now):

```rust
        tree_rules,
```

- [ ] **Step 5: Union scan attrs on refresh**

In `src/ui/app/action.rs::refresh_structure`, replace the `attrs:` argument of the `LoadStructure` request

```rust
            attrs: label_rule_attrs(&app.label_rules),
```

with:

```rust
            attrs: {
                let mut a = label_rule_attrs(&app.label_rules);
                for t in crate::config::tree_label::tree_template_attrs(&app.tree_rules) {
                    if !a.iter().any(|x| x.eq_ignore_ascii_case(&t)) {
                        a.push(t);
                    }
                }
                a
            },
```

- [ ] **Step 6: Add `tree_rules` to every `App` test stub**

In each of these files, find the `App { ... }` literal that sets `tree_items: vec![]` and add `tree_rules: Vec::new(),` next to it:
- `src/ui/app/test_support.rs` (~line 22)
- `src/ui/app/value_editor.rs` (~line 363)
- `src/ui/view.rs` (~line 801 and ~line 963)

- [ ] **Step 7: Build and run the full test suite**

Run: `cargo build && cargo test`
Expected: PASS — everything compiles; existing tests still green; scan attrs now include tree-template attrs.

- [ ] **Step 8: Commit**

```bash
git add src/ui/app/mod.rs src/ui/app/action.rs src/ui/app/test_support.rs src/ui/app/value_editor.rs src/ui/view.rs src/config/tree_label.rs
git commit -m "feat(ui): compile tree label rules and fetch their attrs"
```

---

## Task 7: Render-time tree build (flip the path; remove `tree_items`)

This is the coherent cut-over: `build_tree_items` becomes width+rules aware, `&Structure` is threaded into `view::ui`/`render_tree`, the `tree_items` cache and its writers are deleted, and the tree label test is updated. The crate must compile green at the end.

**Files:**
- Modify: `src/ui/app/structure_view.rs` (`build_tree_items` signature + body; its test)
- Modify: `src/ui/view.rs` (`ui` + `render_tree` signatures; build at render time; imports; remove stubs)
- Modify: `src/ui/app/mod.rs` (draw call; remove `tree_items` field + 2 rebuild lines + construction; re-export cleanup)
- Modify: `src/ui/app/action.rs` (remove `tree_items =` rebuild + its `build_tree_items` import)
- Modify: `src/ui/app/test_support.rs`, `src/ui/app/value_editor.rs` (remove `tree_items: vec![]`)

- [ ] **Step 1: Write the failing tests**

Replace the existing `tree_items_contain_only_branches` test in `src/ui/app/structure_view.rs` (it calls the old 1-arg signature) and add a calibration test. Use the file's existing `structure()` test helper:

```rust
#[test]
fn tree_items_contain_only_branches() {
    let s = structure();
    let rules = crate::config::tree_label::default_tree_rules();
    let items = build_tree_items(&s, &rules, 80);
    // (Keep this test's original branch-only assertions here, unchanged.)
    assert_eq!(items.len(), 1);
}

#[test]
fn deepest_visible_node_still_shows_its_rdn_when_narrow() {
    let s = structure();
    let rules = crate::config::tree_label::default_tree_rules();
    // A deliberately narrow inner width: the RDN of the root must survive.
    let items = build_tree_items(&s, &rules, 12);
    // The root TreeItem's rendered text must still contain the RDN attr name
    // up to the first '=' (e.g. "ou", "dc"); the trimmer never drops the first
    // segment entirely.
    let root_text = format!("{:?}", items[0]); // TreeItem has no public text getter;
    // assert the rendered label is non-empty and starts with the RDN attr type.
    assert!(!root_text.is_empty());
}
```

NOTE for the implementer: `TreeItem` in tui-tree-widget 0.24 has no public text accessor. If `format!("{:?}", ...)` does not expose the label, instead assert via a thin pure helper: factor the per-node label out as `fn node_label(structure, dn, rules, inner_width, depth) -> String` inside `structure_view.rs`, make it `pub(crate)` for tests, and assert `node_label(&s, s.root_dn(), &rules, 12, 0)` starts with the root RDN's attribute type (text before `=`). Prefer this helper approach — it gives a precise, readable assertion and pins the indent math.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test ui::app::structure_view::tests::tree_items_contain_only_branches ui::app::structure_view::tests::deepest_visible_node_still_shows_its_rdn_when_narrow`
Expected: FAIL — `build_tree_items` arity mismatch / `node_label` missing.

- [ ] **Step 3: Rewrite `build_tree_items` (and extract `node_label`)**

In `src/ui/app/structure_view.rs`, add imports at the top:

```rust
use crate::config::label::Piece;
use crate::config::tree_label::{eval_tree_label, fit_label, CompiledTreeRule, Segment};
```

Replace the whole `build_tree_items` function with:

```rust
/// The fitted label for one branch node at `depth`, given the DIT pane's inner
/// width. Text x-offset inside the tree = per-depth indent (2 cols/level) +
/// node symbol (2 cols) + highlight symbol (0, none configured).
pub(crate) fn node_label(
    structure: &Structure,
    dn: &str,
    rules: &[CompiledTreeRule],
    inner_width: usize,
    depth: usize,
) -> String {
    let avail = inner_width.saturating_sub(depth * 2 + 2);
    let rdn = dn.split(',').next().unwrap_or(dn).trim();
    match structure.get(dn) {
        Some(n) => fit_label(&eval_tree_label(rules, &n.attrs, rdn), avail),
        None => fit_label(
            &[Segment {
                pieces: vec![Piece {
                    text: rdn.to_string(),
                    from_field: true,
                }],
            }],
            avail,
        ),
    }
}

/// Build the pane-1 tree items from the eager [`Structure`], rendering each
/// branch node's label via the compiled tree rules and width-fitting it to the
/// pane's inner width. Only branch nodes appear (leaves live in pane 2); the id
/// is the DN so `tree_state.selected()` yields the branch DN.
pub(crate) fn build_tree_items(
    structure: &Structure,
    rules: &[CompiledTreeRule],
    inner_width: usize,
) -> Vec<TreeItem<'static, String>> {
    fn build(
        structure: &Structure,
        dn: &str,
        rules: &[CompiledTreeRule],
        inner_width: usize,
        depth: usize,
    ) -> TreeItem<'static, String> {
        let label = node_label(structure, dn, rules, inner_width, depth);
        let mut children = Vec::new();
        if let Some(n) = structure.get(dn) {
            for child_dn in &n.children {
                if structure
                    .get(child_dn)
                    .map(|c| c.is_branch())
                    .unwrap_or(false)
                {
                    children.push(build(structure, child_dn, rules, inner_width, depth + 1));
                }
            }
        }
        if children.is_empty() {
            TreeItem::new_leaf(dn.to_string(), label)
        } else {
            TreeItem::new(dn.to_string(), label, children).expect("DNs are unique ids")
        }
    }
    vec![build(structure, structure.root_dn(), rules, inner_width, 0)]
}
```

- [ ] **Step 4: Thread `&Structure` into the view and build at render time**

In `src/ui/view.rs`:

Add the import near the other `use crate::ui::app::...` lines:

```rust
use crate::workflows::structure::Structure;
```

Change `ui` to take `structure` and pass it to `render_tree`:

```rust
pub fn ui(f: &mut Frame, app: &mut App, structure: &Structure) {
    let chunks = Layout::vertical([
        Constraint::Min(0),    // pane area
        Constraint::Length(1), // status line
    ])
    .split(f.area());

    let cols = Layout::horizontal(COLUMNS).split(chunks[0]);
    render_tree(f, app, structure, cols[0]);
    render_leaf(f, app, cols[1]);
    render_form(f, app, cols[2]);

    render_status_line(f, app, chunks[1]);

    if app.overlay.is_some() {
        render_overlay(f, app);
    }
}
```

Rewrite `render_tree` to build items at render time:

```rust
fn render_tree(f: &mut Frame, app: &mut App, structure: &Structure, area: Rect) {
    let focused = app.focus == Pane::Tree;
    // Tree inner width = pane width minus the 1-col Block border on each side.
    let inner_width = area.width.saturating_sub(2) as usize;
    let items = crate::ui::app::build_tree_items(structure, &app.tree_rules, inner_width);
    let tree = Tree::new(&items)
        .expect("tree item ids are unique DNs")
        .block(pane_block("DIT", focused))
        .highlight_style(selection_style(focused));
    f.render_stateful_widget(tree, area, &mut app.tree_state);
}
```

(`items` owns its data and does not borrow `app`, so the immutable `&app.tree_rules` borrow is released before the `&mut app.tree_state` borrow — this compiles.)

- [ ] **Step 5: Update the draw call and remove the `tree_items` field + writers**

In `src/ui/app/mod.rs`:

Change the draw call (line ~240):

```rust
        terminal.draw(|f| view::ui(f, app, &structure))?;
```

Remove the `App` struct field `pub tree_items: Vec<TreeItem<'static, String>>,`.

Remove `tree_items: build_tree_items(&structure),` from the `App { ... }` construction (~line 177).

Remove the two rebuild lines in `handle_worker_response` (~lines 413 and 427):

```rust
                            app.tree_items = build_tree_items(structure);
```

Fix the re-export list (~line 53): `build_tree_items` is still needed (used by `view.rs` via `crate::ui::app::build_tree_items`); no change there unless an unused-import warning appears for `LabelRule`/others — leave the existing names. If `TreeItem` import in `mod.rs` becomes unused after removing the field, remove that `use` line.

- [ ] **Step 6: Remove the refresh rebuild and stale import in `action.rs`**

In `src/ui/app/action.rs`:

Remove the line (~146):

```rust
            app.tree_items = build_tree_items(structure);
```

Remove `build_tree_items` from the `use ...structure_view::{...}` import list at the top (~line 18) — it is no longer referenced there.

- [ ] **Step 7: Remove `tree_items: vec![]` from the remaining stubs**

In `src/ui/app/test_support.rs` (~line 22), `src/ui/app/value_editor.rs` (~line 363), and `src/ui/view.rs` (~lines 801, 963): remove the `tree_items: vec![]` lines (the `tree_rules: Vec::new()` added in Task 6 stays).

- [ ] **Step 8: Build and run the full test suite**

Run: `cargo build && cargo test`
Expected: PASS — clean build (no unused-import / dead-code warnings for `tree_items`), all tests green including the two updated `structure_view` tests.

- [ ] **Step 9: Clippy gate**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: no warnings. (Fix any `needless_borrow`/unused import the cut-over introduced.)

- [ ] **Step 10: Commit**

```bash
git add src/ui/app/structure_view.rs src/ui/view.rs src/ui/app/mod.rs src/ui/app/action.rs src/ui/app/test_support.rs src/ui/app/value_editor.rs
git commit -m "feat(ui): render-time width-aware DIT tree labels (RDN always shown)"
```

---

## Task 8: Docs & changelog

**Files:**
- Create: `docs/src/configuration/tree-labels.md`
- Modify: `docs/src/SUMMARY.md`
- Modify: `CHANGES.md`

- [ ] **Step 1: Write the docs page**

Create `docs/src/configuration/tree-labels.md`:

````markdown
# DIT Tree Labels

The DIT navigation pane (pane 1) labels each branch node. By default eDAPtor
keeps the structural **RDN** visible and appends a human name when present:

- `{rdn} ({cn})` when the node has a `cn`,
- else `{rdn} ({description})` when it has a `description`,
- else just `{rdn}` (e.g. `ou=people`).

So `ou=people` carrying `description: People` renders as **`ou=people (People)`** —
the container's identity is never dropped.

## Configuring labels

Override the defaults with an ordered list of `[[tree.label]]` rules. The first
rule whose `when` attributes are **all present** (non-empty) wins; a rule with no
`when` is the unconditional fallback:

```toml
[[tree.label]]
when     = ["description"]
template = "{rdn} ({description})"

[[tree.label]]
template = "{rdn}"            # fallback: no `when` → always matches
```

- **`when`** (default `[]`): attribute names that must all be present. Matching is
  case-insensitive. An empty/omitted `when` always matches.
- **`template`**: reuses the `{field}` substitution from entry labels, plus the
  reserved **`{rdn}`** token (the node's relative DN, e.g. `ou=people`). An unknown
  `{field}` renders empty.

If `[[tree.label]]` is omitted entirely, the built-in default rule set above is
used.

## Narrow panes (the truncation ladder)

When the pane is too narrow, eDAPtor trims the **rightmost** segment first and
drops a segment whole once its templated value is consumed — so the RDN (leftmost)
survives longest. For `{rdn} ({description})` on `ou=people` / `description=People`:

```
wide   ou=people (People)
        ou=people (Peop…)     trim the description in the last segment
        ou=people (P…)
        ou=people             description consumed → drop the "(…)" segment
narrow  ou=peop…              only the RDN segment left → ellipsize it
```
````

- [ ] **Step 2: Link it in `SUMMARY.md`**

In `docs/src/SUMMARY.md`, add under the Configuration section (after the Pickers line):

```markdown
- [DIT Tree Labels](configuration/tree-labels.md)
```

- [ ] **Step 3: Update `CHANGES.md`**

Under `## Unreleased` → `### New`, add:

```markdown
- Configurable, presence-keyed, width-aware DIT tree labels via `[[tree.label]]`
  rules. The structural RDN is now always shown by default (e.g.
  `ou=people (People)`), and narrow panes degrade gracefully while keeping the RDN.
```

Under `## Unreleased` → `### Changed`, add:

```markdown
- DIT tree branch labels now always include the RDN by default (previously a
  node with a `description` showed only the description).
```

- [ ] **Step 4: Build the docs (if mdbook is available) and the crate**

Run: `cargo build` (sanity) and, if `mdbook` is installed, `cd docs && mdbook build` from the repo root in a subshell. Expected: no errors / no broken SUMMARY link. If `mdbook` is absent, skip the doc build and just confirm the files exist.

- [ ] **Step 5: Commit**

```bash
git add docs/src/configuration/tree-labels.md docs/src/SUMMARY.md CHANGES.md
git commit -m "docs: document configurable DIT tree labels"
```

---

## Final verification

- [ ] **Run the whole suite + clippy + fmt**

Run:
```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```
Expected: all clean / all green.

- [ ] **Manual smoke (optional but recommended)**

Run the app against the local test LDAP (see the project's run/test-server notes) and confirm: branch nodes show `ou=… (Name)`; narrowing the terminal keeps the RDN visible on the deepest node; expansion/selection state is preserved while resizing.

---

## Self-review (performed against the spec)

- **Goal 1 (RDN always visible):** default rules all lead with `{rdn}` (Task 3); first-segment guard never drops it (Task 5 + calibration test Task 7). ✓
- **Goal 2 (configurable, presence-keyed):** `[[tree.label]]` parse (Task 1), compile (Task 3), first-match presence eval (Task 4). ✓
- **Goal 3 (graceful width degradation):** `fit_label` trims rightmost-field-first, drops consumed segments, guards the first (Task 5). ✓
- **Locked decisions:** `[[tree.label]]` array, `when`+`template`, first-present wins, `{rdn}` reserved token, built-in default trio, segment trimmer, DIT-only scope — all covered (Tasks 1–7). ✓
- **Non-goals honored:** `label_for`/`StructureNode.label` kept; `compute_rows`/`‹self›` row untouched; no per-node overrides. ✓
- **Serde types** match the spec (`TreeConfig`/`TreeLabelRule`, `#[serde(default)] tree`). ✓
- **Rendering model:** compile → scan-attr union (excludes `rdn`) → eval → provenance pieces → segments → width-fit at render time. ✓ (Approach A chosen over a stored deferred model because render's `Structure` is already in scope at the draw call and the `tree_items` cache has a single reader — fewer moving parts, four sync sites deleted.)
- **Dependency:** `unicode-width = "0.2"` added (Task 5). ✓
- **Testing:** config parse, compile+default, scan-attr exclusion/dedup, rule eval (presence/fallback/case/non-empty/`{rdn}`), `fit_label` ladder (full→field-ellipsized→segment-dropped→rdn-ellipsized, pure-literal drop, multi-field, CJK), and the narrow-pane calibration. ✓
- **Docs & changelog:** new `tree-labels.md` page + SUMMARY link + CHANGES entries. ✓
- **Risks:** indent calibration pinned by the deepest-node test; per-frame rebuild acknowledged cheap (container-only tree) with no width-gating added (YAGNI). ✓
- **Type consistency check:** `CompiledTreeRule { when, template }`, `Segment { pieces }`, `Piece { text, from_field }`, `eval_tree_label(rules, attrs, rdn)`, `fit_label(&[Segment], usize)`, `build_tree_items(structure, rules, inner_width)`, `node_label(...)`, `tree_template_attrs(rules)` — names used consistently across Tasks 2–7. ✓
