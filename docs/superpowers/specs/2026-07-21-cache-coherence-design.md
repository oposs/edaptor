# Design: cache coherence (Spec 2 of the real-time consistency round)

**Date:** 2026-07-21 · **Umbrella:** `2026-07-21-realtime-consistency-design.md`
**Depends on:** Spec 1 (optimistic concurrency, merged as `30273b0`)

## Problem

edaptor builds an in-memory projection of the directory at startup and then
trusts it for the rest of the session. Nothing invalidates it:

- **`UiState::structure`** is built once from the eager scan.
  `Structure::add_child` / `Structure::remove` exist, are doc-commented *"e.g.
  after a successful create"*, and have **no production callers**. A newly created
  entry therefore does not appear in the entry list until restart — the original
  user-reported bug. A rename (an RDN-attribute edit, which edaptor already
  performs via MODRDN) leaves the node under its **old DN**, so the row points at
  an entry that no longer exists.
- **`lookup_cache`** is only ever `insert`ed and `contains_key`ed — never
  `remove`d or `clear`ed. It caches negative results too, so a label stays wrong
  for the whole session, including after edaptor itself renamed the entry.
- **Incremental find answers from cached copies.** The entry list's find filters
  the cached `Structure` client-side, and the `lookup` combobox loads its
  candidates **once** with an empty term, capped at `PICKER_SEARCH_CAP` (**100**),
  then narrows that capped set client-side — so a candidate ranked past the cap is
  unreachable no matter what the user types, and one created after the dialog
  opened never appears.
- **`UiAction::Refresh` (Alt+R) is declared but never produced or handled** —
  dead vocabulary, so there is no escape hatch when the projection is stale.

The three-pane design (`2026-06-01-three-pane-layout-design.md` §5.9, §7.4)
promised the after-write reflow and the manual refresh. Neither was built.

## Goal

Every list edaptor shows reflects directory reality: writes are reflected
immediately, incremental find is answered by the server, and one keystroke
rebuilds the projection from scratch.

## Non-goals

RFC 4533 syncrepl live push; re-probing the schema, root DSE or samba domain
mid-session (audit rows 6–8, out of scope for the whole round); the autonumber
TOCTOU (Spec 3); the delete flow (Spec 4); the objectClass picker, whose find
narrows the complete in-memory schema list and has no server-side counterpart.

## Components

### 1. `Structure::upsert` — one mutation point

```rust
/// Insert or update the node for `input.dn`, preserving any existing children,
/// and link it under its parent when that parent is known. Returns true when the
/// tree pane must rebuild.
pub fn upsert(&mut self, input: StructureInput) -> bool
```

- **Preserves `children`** on an existing node — upserting a container must never
  orphan its subtree.
- Links the node into `parent_of(dn)`'s `children` when that parent node exists,
  deduped case-insensitively (as `build`/`add_child` do today). An entry whose
  parent is outside the loaded base is inserted but stays unlinked, hence
  invisible — the same behaviour `build` has for such entries.
- Returns `true` when the parent flipped leaf→branch, or when an existing
  **branch**'s label-relevant attributes changed (the tree pane renders branch
  labels from `attrs`). A freshly inserted node has no children, so it is never a
  branch on insert.

`add_child` is **deleted** — `upsert` subsumes it (its tests migrate). `remove`
keeps its current semantics and finally gets callers.

### 2. Feeding it: every entry read

`ReadOutcome::Form` grows `dn: String` and `attrs: BTreeMap<String, Vec<String>>`
(the raw entry attributes). The read already requests `*`, so the label
attributes come for free — no extra round-trip.

`UiState` gains `scan_attrs: Vec<String>` (the `structure_scan_attrs(&label_rules,
&tree_rules)` list already computed at bootstrap, now stored) and:

```rust
fn upsert_from_read(&mut self, dn: &str, attrs: &BTreeMap<String, Vec<String>>)
```

which projects the raw attributes onto `scan_attrs` + `objectClass` (so a node
never holds the entry's entire attribute set), builds a `StructureInput`, calls
`structure.upsert`, sets `list_dirty`, and sets `tree_dirty` when upsert returned
true. It is called wherever a `ReadOutcome::Form` is applied — which is a single
path shared by navigation clicks and post-write re-reads.

Consequences, all from that one call site:

| Situation | Effect |
|---|---|
| Create | the post-write re-read upserts the new node → the row appears |
| Edit changing `cn`/`description` | leaf row and tree label refresh |
| Navigating to any entry | that node self-heals from live data |

Two write-side additions:

- **Rename.** `WriteOutcome::Saved { dn, .. }` carries the post-MODRDN DN. When
  it differs (case-insensitively) from the DN the form was loaded with,
  `structure.remove(old_dn)` runs before the re-read upserts the new DN. No extra
  state: the old DN is the form's own DN.
- **Create.** `WriteOutcome::Created` additionally clears `state.search` (a stale
  find query must not hide the new row) and sets
  `set_leaf_row = current_leaf_row()` so the highlight snaps to the new entry.

### 3. `tree_dirty` — the tree pane can rebuild

`TreePane` builds its `Node` tree once in `new()` and has no rebuild path today.
It gains one: on a `REFRESH` broadcast with `tree_dirty` set, re-run
`build_branch_nodes`, swap the outline root, `ov_update`, refresh
`state.branch_dns`, and restore the highlight **by DN** (row indices shift when
the node set changes), then clear the flag — mirroring how `LeafPane` consumes
`list_dirty`. `pump.rs`, which today broadcasts `REFRESH` when `list_dirty` is
set, also broadcasts when `tree_dirty` is set.

### 4. Every incremental find is answered by the server

Audit of every find-enabled list:

| List | Find mode | Rows today | Verdict |
|---|---|---|---|
| Entry list (`panes/leaf.rs:55`) | `Highlight` | cached `Structure`, filtered client-side | **fix** |
| Lookup combobox (`lookup.rs:164`) | `Filter` | one-shot capped load, narrowed client-side | **fix** |
| Shuttle, Available (`multi_picker.rs:184`) | `Highlight` | re-queries on `LIST_FIND_CHANGED` | already correct — the pattern to copy |
| Shuttle, Selected (`shuttle.rs:239`) | `Filter` | the staged local selection | correct: a complete local set |
| Single picker (`picker.rs`) | own `InputLine` | `SearchFlow` search-as-you-type | already correct |
| objectClass picker (`oc_picker.rs:149`) | `Filter` | in-memory schema list | correct: complete locally, no server list |

**4a. Entry list.** New `src/workflows/leaf_search.rs`, modelled on `SearchFlow`:

```rust
pub struct LeafSearchFlow { /* next_id, latest */ }
pub fn request(&mut self, worker: &WorkerHandle, branch_dn: &str, query: &str,
               attrs: &[String]) -> Result<u64>;
pub fn on_response(&mut self, resp: &Response) -> LeafSearchOutcome;
```

- `SearchScope::OneLevel` under `current_branch`. The filter dimensions are the
  **column-2 label-rule attributes** (`labels::label_rule_attrs`) — what the user
  actually sees in the list — combined as `(|(<a>=*q*)…)` with `q` through the
  existing `pick_state::escape_filter`. The **requested** attributes are the
  wider `scan_attrs`, so an upserted node carries what the tree pane needs too.
  `size_limit = LEAF_SEARCH_CAP` (500) with a `truncated` flag.
- **Supersede, no debounce.** Every keystroke submits; only the newest
  correlation id is applied (`SearchFlow`'s `latest` discipline). One-level
  searches are cheap and this keeps typing responsive; a timer debounce is a
  later optimisation if it proves chatty.
- Results are **upserted into `Structure`** (component 1) before rendering, so
  entries other clients created become permanent local nodes rather than
  transient rows.

`UiState` gains `leaf_search_rows: Option<Vec<String>>` (matching DNs) and
`leaf_search_truncated: bool`. `leaf_rows()` stays the single row source for the
pane and the selection→DN mapping:

| State | Rows |
|---|---|
| `search` empty | structure projection (unchanged) |
| query active, results in hand | live DNs rendered through the same label rules, branches excluded, sorted by label |
| query active, search in flight | previous rows stay on screen (no flicker) |
| query active, search failed | fall back to `filter_leaves` + a status message — never blank because of a transient error |

`commit_branch` already clears `search` on a branch switch; it additionally
clears `leaf_search_rows`.

**4b. Lookup combobox.** `FindMode::Filter` → `FindMode::Highlight`, and a typed
input change submits a fresh candidate search whose results replace the candidate
set — the `multi_picker` pattern. The wrinkle: this dialog's `InputLine` doubles
as the committed value, and picking a row writes `"5000 (staff)"` back into it. A
re-query must fire only on **typing**, not on that write-back. That suppression
turns out to need no new code: `mirror_focused` already syncs `last_input` when it
writes a picked row back, and the change detector compares against it.

**No truncation notice.** An earlier draft of this spec had the dialog title report
when the candidate list was capped at `PICKER_SEARCH_CAP`. Dropped deliberately:
once every keystroke re-queries, typing reaches candidates past the cap, which is
the harm the notice was meant to warn about. The cap is documented in prose on the
`docs/src/concepts/live-data.md` page instead.

### 5. Refresh and cache invalidation

Alt+R becomes a real action. The `UiAction::Refresh` variant that documents it
today lives in a vocabulary (`src/app.rs`) with **no producers and no
consumers** — the app's real mechanism is a `tv::Command` posted from the menu
and handled in `ui::app::dispatch`. So this adds a `RELOAD` command plus a File
menu entry and **deletes** the dead enum rather than wiring it. The handler:

1. Blocking `Request::LoadStructure` (the bootstrap path, `worker.request`),
   requesting `scan_attrs`.
2. Rebuild `Structure`; set `list_dirty` + `tree_dirty`; clear `lookup_cache`,
   `search` and `leaf_search_rows`.
3. Preserve `current_branch` and `current_leaf` **by DN** when they still exist;
   otherwise fall back to the base DN / no leaf.
4. Leave the edit form untouched — unsaved edits are never at risk, so no
   dirty-form guard is needed.
5. Status reports the entry count; on failure the status reports the error and
   the previous structure is kept.

The TUI is frozen for the duration of the scan (no spinner) — acceptable at the
demo directory's ~600 entries and consistent with bootstrap.

`lookup_cache.clear()` additionally runs on every `WriteOutcome::Saved` and
`WriteOutcome::Created`. Clearing wholesale cannot miss a case — renamed groups,
changed `gidNumber`s, and stale **negative** entries for things just created —
and re-resolution is lazy, asynchronous and limited to fields on screen.

## Error handling

| Failure | Behaviour |
|---|---|
| Refresh scan fails | status shows the error; previous structure kept |
| Leaf search fails | status shows the error; rows fall back to the cached filter |
| Leaf search truncated | rows shown with a truncation note in the status |
| Re-read after write fails | existing behaviour (status error); structure simply not upserted |
| Upserted entry's parent outside the loaded base | node inserted unlinked, hence not shown — same as `build` |

## Testing

- `structure.rs`: upsert preserves children; upsert links to a known parent;
  parent leaf→branch flip returns true; branch label change returns true; a
  rename modelled as `remove(old)` + `upsert(new)` leaves no stale node.
- `leaf_search.rs`: filter shape over configured attrs; `escape_filter` applied;
  a stale correlation id is ignored while the newest is applied; truncation flag.
- `state.rs`: `Created` clears `search` and sets `set_leaf_row`; a write clears
  `lookup_cache`; a `Saved` whose DN differs removes the old node; `leaf_rows()`
  honours each of the four states in the table above; Refresh preserves a
  still-existing branch and falls back when it is gone.
- `panes/tree.rs`: a `REFRESH` with `tree_dirty` rebuilds the node set and keeps
  the highlighted DN.
- `panes/leaf.rs`: a find edit submits a search rather than filtering locally.
- Live (gated on `EDAPTOR_TEST_LDAP_URI`): an entry created out-of-band is found
  by the entry list's find without a restart.

## Documentation

- `docs/src/`: a page covering live find, the after-write reflow, and Alt+R;
  registered in `SUMMARY.md`.
- `CHANGES.md`: entries for the create/rename visibility fix, server-backed
  incremental find, Alt+R, and lookup-cache invalidation.
- Alt+R is discoverable through its File-menu entry (`help_ctx.rs` holds
  field-level hints only, so it is not the right home for a global key).

## Risks

- **Chattiness.** One search per keystroke per find-enabled list. Superseding
  bounds concurrency to one in-flight request per list; debounce stays available.
- **Blocking refresh.** A large directory makes Alt+R feel like a hang. Mitigated
  by it being explicit and user-initiated; async refresh is the fallback plan.
- **DN case.** `Structure`'s map is keyed by the exact DN string; child links
  compare case-insensitively. A server echoing a different DN case than the scan
  would produce a duplicate node. Pre-existing behaviour, unchanged here, noted
  so it is not mistaken for new.
