# DIT Tree Label — Configurable, Presence-Keyed, Width-Aware

**Date:** 2026-06-05
**Status:** approved (pending written-spec review)
**Area:** `eDAPtor` — the DIT navigation tree (pane 1) branch-node labels.

## Problem

Branch nodes in the DIT pane are labelled by a hardcoded rule in
`src/workflows/structure.rs::label_for`: **`cn` → else `description` → else
RDN**. So an `ou=people` node that carries `description: "People"` renders as just
**"People"** — the structural `ou=people` context is dropped the moment a
description exists, which reads oddly and loses the identity of the container.

## Goals

1. Always keep the structural RDN visible by default (e.g. `ou=people (People)`).
2. Make the label **configurable** with **presence-keyed variants**: an ordered
   set of rules; the first rule whose required attributes are all present wins.
3. When the pane is too narrow, degrade gracefully with a **segment trimmer**
   that shortens the rightmost segment first and drops a segment whole once its
   templated value is consumed — so the RDN (leftmost) survives longest.

## Non-goals

- Leaf-entry labels (pane 2) and the per-profile `label` template are
  **unchanged**. This is DIT-tree (pane 1, branch nodes) only.
- The pane-2 **`‹self›` row** (which shows the selected branch as
  `‹self› {node.label}`, `structure_view.rs:81`) keeps the existing
  cn→description→RDN label — it is not part of the DIT tree and stays as-is.
- No per-node manual label overrides; no user-configurable truncation policy
  (the trimmer behaviour is fixed).

## Locked decisions

| Decision | Value |
|---|---|
| Config table | `[[tree.label]]` (array of rules), optional |
| Rule shape | `when = [attrs…]` (optional) + `template = "…"` |
| Match semantics | first rule whose `when` attrs are all present (non-empty) wins; a rule with no `when` is the unconditional fallback |
| Reserved token | `{rdn}` → the node's RDN (e.g. `ou=people`); other `{attr}` resolve as today |
| Built-in default | `{rdn} ({cn})` if cn present · else `{rdn} ({description})` if description present · else `{rdn}` |
| Truncation | segment trimmer (below), RDN-wins by construction |
| Scope | DIT tree branch nodes only |

## Config schema

A new optional top-level `[tree]` table holding an ordered `[[tree.label]]`
array:

```toml
[[tree.label]]
when     = ["description"]
template = "{rdn} ({description})"

[[tree.label]]
template = "{rdn}"            # fallback: no `when` → always matches
```

- `when` (default `[]`): attribute names that must **all** be present on the node
  (present = the attribute exists with a non-empty first value). An empty/omitted
  `when` always matches. Attribute-name matching is case-insensitive (LDAP
  attribute names are case-insensitive).
- `template`: a label template reusing the existing `{field}` substitution
  (`src/config/label.rs`), plus the reserved `{rdn}` token. An unknown `{field}`
  renders empty (existing `render_label` behaviour).
- If `[[tree.label]]` is absent entirely, the **built-in default rule set** is
  used (the three rules in "Locked decisions").

### Serde types (`src/config/mod.rs`)

```rust
#[derive(Debug, Default, Deserialize)]
pub struct TreeConfig {
    #[serde(default)]
    pub label: Vec<TreeLabelRule>,
}

#[derive(Debug, Deserialize)]
pub struct TreeLabelRule {
    #[serde(default)]
    pub when: Vec<String>,
    pub template: String,
}
```

Add `#[serde(default)] pub tree: TreeConfig` to `Config`.

## Rendering model

The model must NOT pre-truncate (truncation needs the render-time pane width), so
it carries enough to render+fit per frame.

1. **Compile** (`compile_tree_rules(&TreeConfig) -> Vec<CompiledTreeRule>`): parse
   each rule's `template` via `parse_label_template` into `Vec<LabelSeg>`; if the
   config list is empty, substitute the default rule set. `CompiledTreeRule {
   when: Vec<String>, template: Vec<LabelSeg> }`.
2. **Scan attrs:** `tree_template_attrs(&rules)` unions every `{field}` name
   referenced by any rule's template, **excluding the reserved `rdn`**. This is
   unioned into the structure-scan `scan_attrs` (today `label_rule_attrs(&rules)`
   at `src/ui/app/mod.rs:130` and `src/ui/app/action.rs`) so templated attributes
   are actually fetched by `LoadStructure`. (`cn`/`description` are already in the
   structural minimum.)
3. **Evaluate** (`eval_tree_label(&rules, node) -> Vec<Segment>`): pick the first
   rule whose `when` attrs are all present on the node (case-insensitive,
   non-empty first value); render its template into **pieces** that retain
   provenance, then split into space-delimited segments. The `{rdn}` field binds
   to the node's RDN (`dn.split(',').next()`). If no rule matches (only possible
   with a misconfigured list and no fallback), fall back to a single `{rdn}`
   segment.

### Provenance-aware rendering

`render_label` returns a flat `String`; the trimmer needs to know which characters
are templated vs literal. Add a sibling renderer:

```rust
pub struct Piece { pub text: String, pub from_field: bool }
// Render segs against attrs (+ injected `rdn`), producing ordered pieces:
//   LabelSeg::Lit  -> Piece{text, from_field:false}
//   LabelSeg::Field-> Piece{text=value, from_field:true}   (empty value -> empty piece kept)
pub fn render_pieces(segs: &[LabelSeg], attrs: &BTreeMap<String,Vec<String>>, rdn: &str) -> Vec<Piece>;
```

A `Segment` is the run of pieces between spaces. Splitting: walk the pieces; a
literal piece may contain spaces and is split at them into separate segments
(spaces are separators, not kept). Field values are assumed space-free for
segmentation but are not required to be; if a field value contains spaces it is
split like any text (its sub-runs keep `from_field = true`).

## Segment trimmer (pure, width-aware)

```rust
// avail = display columns available for this node's label.
// Returns the fitted label string. Unicode display-width aware (unicode-width).
pub fn fit_label(segments: &[Segment], avail: usize) -> String;
```

Algorithm:

1. Join all segments with single spaces; if its display width ≤ `avail`, return it.
2. Otherwise operate on the **last** segment:
   a. Shorten that segment's **field** characters (`from_field == true`) from the
      end, replacing the removed tail with a single `…`, recompute the whole
      label width, and stop as soon as it fits.
   b. If the segment has **no field characters** (pure literal), or its field
      characters are fully consumed, **drop the entire segment** (all its pieces,
      literal decoration included) and retry from step 1 with the remaining
      segments.
3. Guard: the **first segment is never fully removed**. When only the first
   segment remains and it still doesn't fit, ellipsize its field (then, if it has
   no field, its literal) down to a minimum of one visible character + `…`.

Ellipsis `…` counts as one display column. All width comparisons use
`unicode-width` (display columns, CJK = 2), consistent with how ratatui measures.

Worked example — template `{rdn} ({description})`, node `ou=people` /
`description=People`, segments `["ou=people", "(People)"]`:

```
avail≥18  ou=people (People)
avail 16  ou=people (Peop…)     trim {description} in last segment
avail 13  ou=people (P…)
avail≈10  ou=people             {description} consumed -> drop "(...)" segment
avail 7   ou=peopl…             only first segment left -> ellipsize {rdn}
```

## Wiring into the view

- The app retains the `Structure` model and the compiled tree rules. The
  precomputed `app.tree_items` string cache is replaced by a **render-time build**
  (or a rebuild keyed on the DIT pane's inner width): `build_tree_items` gains the
  compiled rules and a per-node available-width computation and emits
  `TreeItem<'static,String>` whose label is `fit_label(...)`.
- Per-node `avail` = DIT pane **inner width** − **per-depth indent** − **highlight
  symbol width**, where the per-depth indent matches `tui-tree-widget`'s rendering
  (fixed columns per depth level + fold/leaf glyph). The exact constant is
  calibrated in implementation and pinned by a test asserting the deepest visible
  node still shows its `{rdn}`.
- Rebuilding `TreeItem`s every frame is safe: `TreeState` tracks selection/open by
  the DN id, which is unchanged, so expansion/selection state is preserved.
- `label_for` and `StructureNode.label` are **kept** — the pane-2 `‹self›` row
  still uses `node.label`. Only the tree-build path stops using `node.label`,
  rendering instead from the node's `attrs` + `dn` + compiled rules. The tree
  renderer reads `cn`/`description` (and any templated attrs) from `node.attrs`,
  which already holds the structural-minimum attributes plus the scan-attr union.

## Dependencies

- Add `unicode-width = "0.2"` (direct dep; already in the tree via ratatui — pin a
  matching major) for display-column measurement in `fit_label`.

## Testing

- **Config parse** (`src/config/mod.rs` tests): `[[tree.label]]` with/without
  `when`; absent `[tree]` → empty list → default rule set on compile.
- **Compile + default** (`tree_template_attrs`, default rules): default rules
  reference `cn`/`description`; `{rdn}` excluded from scan attrs; union/dedup
  case-insensitive.
- **Rule eval**: presence first-match, fallback rule, case-insensitive presence,
  non-empty requirement, `{rdn}` injection.
- **`fit_label` ladder** (table-driven, pure): full → field-ellipsized →
  segment-dropped → rdn-ellipsized; pure-literal segment dropped as a unit;
  first-segment minimum guard; multi-segment templates (e.g. `{cn} — {rdn}`);
  Unicode width (CJK value, combining marks treated by `unicode-width`).
- **Render/integration**: a narrow DIT pane still shows the RDN for branch nodes;
  selection/expansion state survives a width-driven rebuild.

## Docs & changelog

- Add a short **"DIT tree labels"** section to the Configuration docs (a
  `configuration/tree-labels.md` page in `SUMMARY.md`, or folded into
  `configuration/overview.md`), with the `[[tree.label]]` example, the `{rdn}`
  token, the default behaviour, and the truncation ladder as a fenced block.
- `CHANGES.md` `Unreleased / New`: configurable, presence-keyed, width-aware DIT
  tree labels (RDN now always shown by default).

## Risks & mitigations

- **Indent calibration**: an over-estimated indent wastes a couple of columns; an
  under-estimate lets the widget hard-clip past our ellipsis. Mitigated by the
  calibration test on the deepest visible node.
- **Per-frame rebuild cost**: DIT trees are small (containers only, leaves live in
  pane 2); rebuilding a few hundred `TreeItem`s per frame is negligible. If a
  pathological directory makes this hot, gate the rebuild on a width change.
- **Behaviour change**: the default now always shows the RDN, changing what
  existing users see. This is the intended fix and is called out in `CHANGES.md`.
