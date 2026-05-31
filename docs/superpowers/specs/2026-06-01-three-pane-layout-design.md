# edaptor — Three-Pane Browser/Editor Layout (Design)

Date: 2026-06-01
Status: Approved (brainstorm complete; ready for implementation planning)
Supersedes: the M3/M4.1 single-tree + modal-dialog interaction model for the TUI

## 1. Motivation

The current TUI is a single `OutlineViewer` tree (`DitOutline` in a `Window`) plus a
**modal** edit `Dialog`. Two problems:

1. The modal dialog discards navigation context every time you edit, and it cannot
   show the directory and the entry at the same time.
2. A single lazy-expanded tree is poor for large flat containers (e.g. an `ou=users`
   with thousands of entries) — there is no list view and no search.

This redesign replaces that with a persistent **three-pane** layout, modeled on a
desktop/IDE: branch tree | leaf list (+ incremental search) | live entry form. It is
the UI shell for the users/groups domain tier (M6).

## 2. Goals / Non-Goals

**Goals**

- Three side-by-side panes with **mouse-draggable** dividers, frameless (no window
  chrome around the whole layout — it *is* the desktop).
- Pane 1: a tree of **branches only** (entries that have children).
- Pane 2: the **leaves** directly under the selected branch, with an **incremental
  search** box, plus a `‹self›` row representing the branch entry itself.
- Pane 3: a **scrollable, persistent** entry-edit form with **Save / Cancel** that
  re-spins live as the pane-2 highlight moves.
- **Eager-load** the entire DIT structure at startup so navigation is instant and
  branch/leaf classification is exact and local.
- Reuse the existing diff/validate/LDIF/write machinery unchanged.

**Non-Goals (explicitly deferred)**

- **Live change notification** (RFC 4533 Content Sync / syncrepl refreshAndPersist).
  Recorded as the natural future upgrade; this redesign uses manual + after-write
  refresh.
- **Rich multi-valued attribute editing** (per-value rows). The existing
  newline-joined single-`InputLine` behavior is kept for this redesign; a `Memo`-based
  multi-value editor is a follow-up.
- **Per-entry write-permission detection** — not possible against OpenLDAP (see §4).
- Millions-of-entries scale (eager load targets hundreds to low hundred-thousands).

## 3. Hard LDAP / OpenLDAP constraints (the "why" behind the decisions)

These are verified facts that shaped the design (see also the project memory note
`edaptor-ldap-constraints`):

1. **No cheap "has children" signal.** LDAP gives no structural leaf/branch flag, and
   *any* entry may have children (OpenLDAP does not enforce DIT structure rules by
   default). → We **eager-load** the structure so "has children" is a free local fact.
2. **No per-entry write-permission check.** The *Get Effective Rights* control is not
   implemented by OpenLDAP. → Read-only is a **global** mode, not per-entry; plus
   graceful `insufficientAccess` handling at Save.
3. **Server size limits** (`olcSizeLimit`, default 500) truncate large results unless
   bound as rootdn. → The eager scan **must** use **Simple Paged Results (RFC 2696)**.
4. **No Persistent Search** in OpenLDAP, but **RFC 4533 Content Sync** is supported.
   → Live updates deferred; RFC 4533 refreshAndPersist is the future path.

## 4. Architecture overview

```
                       app.desktop
                            │
                   ┌────────▼─────────┐
                   │  SplitContainer  │  (frameless, mouse-draggable dividers)
                   │   (Group-backed) │
                   └───┬──────┬───────┘
            ┌──────────┘      │        └───────────┐
       ┌────▼────┐      ┌─────▼──────┐      ┌───────▼────────┐
       │ Pane 1  │      │  Pane 2    │      │   Pane 3       │
       │ branch  │      │ search +   │      │ scrollable     │
       │ tree    │      │ leaf list  │      │ entry form     │
       │(Outline │      │(InputLine +│      │(Scroller +     │
       │ Viewer) │      │ ListBox)   │      │ InputLine/...) │
       └─────────┘      └────────────┘      └────────────────┘

  Structure model (tty-free)        Worker thread (only network I/O)
  ┌───────────────────────┐         ┌────────────────────────────────┐
  │ DitStructure: full     │◀───────│ Subtree + paged search (RFC2696)│
  │ node tree, branch/leaf │  build │ Base read (on-demand, pane 3)   │
  │ classification, filter │        │ Modify/Add/ModRdn/Delete (reuse)│
  └───────────────────────┘         └────────────────────────────────┘
```

**Boundary rule unchanged:** `src/ui/facade.rs` remains the only module that may
`use turbo_vision`. All new pure logic (structure model, leaf filtering, dirty-state
machine, create-selection) lives in tty-free modules and is unit-tested.

### 4.1 Turbo Vision components used (verified present in turbo-vision 1.2.0)

All panes are built from **stock** Turbo Vision widgets; the only genuinely custom
code is the divider bar (draw + drag) — TV has no splitter widget.

| Element            | Component (crate path)                                  |
|--------------------|--------------------------------------------------------|
| Container + focus  | `views::group::Group` (`add`, `select_next/previous`, `broadcast`) |
| Pane 1 tree        | `views::outline::{OutlineViewer, Node}` (= TOutline)   |
| Pane 2 list        | `views::listbox::ListBox` (or `sorted_listbox::SortedListBox`) |
| Pane 2 search box  | `views::input_line::InputLine`                         |
| Pane 3 scrolling   | `views::scroller::Scroller` + `views::scrollbar::ScrollBar` |
| Pane 3 fields      | `views::input_line::InputLine`, `static_text::StaticText`, `button::Button` |

`Group` provides Tab focus cycling between panes for free; `Scroller` provides
`delta`/`limit`/`scroll_to`/`set_limit` + scrollbar plumbing.

## 5. Component design

### 5.1 SplitContainer (facade, custom View)

A frameless container mounted on `app.desktop` (replaces the current single DIT
`Window`). Holds three child views and two divider x-positions.

Responsibilities:

- Lay out the three children into columns separated by 1-column dividers; on
  `set_bounds` rescale split fractions then re-`set_bounds` each child.
- Draw the two vertical `│` dividers.
- Mouse: `MouseDown` on a divider starts a drag (set `SF_RESIZING` to keep capture);
  `MouseMove` updates the split and re-lays-out; `MouseUp` ends it. Clamp so each pane
  ≥ a minimum width.
- Focus: delegate Tab/Shift-Tab to `Group` focus cycling; clicking a pane focuses it.
- `update_cursor` forwards to the focused pane so the active `InputLine` shows its
  caret.

The corrected version of the existing skeleton
(`docs/superpowers/research/2026-05-30-splitcontainer-skeleton.md`) uses the public
`turbo_vision::` paths and, where possible, defers child management/focus to `Group`
rather than re-implementing it.

### 5.2 Eager structure load (worker + model)

**Worker additions** (`src/ldap/worker.rs`):

- Add `SearchScope::Subtree` (maps to `ldap3::Scope::Subtree`).
- Add **paged search** support (RFC 2696) for the structure scan: page through the
  whole subtree (~1000 entries/page) accumulating results, so server size limits do
  not truncate. (ldap3 0.12's paged-results adapter; verify exact API during the
  implementation spike.)
- A dedicated request, e.g. `Request::LoadStructure { id, base }`, returning DNs +
  `cn`, `description`, `objectClass` only (minimal payload for labels + tree rules).

**Structure model** (new tty-free module, e.g. `src/workflows/structure.rs`):

- Build a node tree from the flat paged result by DN parent/child relationships.
- `branch = has ≥1 child`; `leaf = no children`. Pure, unit-tested.
- Provide: the branch subtree (for pane 1), the leaves of a given branch (for pane 2),
  and incremental-search filtering over a branch's leaves.
- Label selection reuses the existing `node_label` rule (cn → description → RDN).

**Fallback:** if the scan hits a size/time limit the server still enforces, surface a
non-silent notice and fall back to lazy one-level expansion for that subtree (the
existing `BrowserState` lazy path remains available).

### 5.3 Pane 1 — branch tree

`OutlineViewer<StructureNode>` showing **branches only**, with the base DN always
present as the root (even when momentarily childless, so a first child can be created).
Selecting a branch sets the "current branch" that drives pane 2. Reuses the
`DitOutline` wrapper pattern (shared `Rc` selection, no downcast).

### 5.4 Pane 2 — leaf list + incremental search

- An `InputLine` search box on top; a `ListBox` below.
- Contents: a `‹self›` row (the branch entry itself — editable like any leaf), then the
  branch's leaf children.
- Typing in the search box filters the list locally (case-insensitive substring over
  the display label) — pure, unit-tested filter function.
- Moving the list highlight selects the "current entry" that drives pane 3.

### 5.5 Pane 3 — live, scrollable entry form

- On selecting a pane-2 entry, issue an on-demand **base read** for its full attributes
  (existing `ReadFlow` / `FormModel` path), then render the form into the pane.
- Layout: one row per field (label + value editor), inside a `Scroller` so a tall entry
  scrolls (PgUp/PgDn, wheel, scrollbar). A persistent **Save / Cancel** bar at the
  bottom.
- Editors reuse the existing rules: `InputLine` per editable field (multi-values joined
  by newline — unchanged for this redesign), `StaticText` for read-only kinds
  (`memberOf`, binary notes, disabled checkboxes).
- Save reuses `collect_edit_entry` → `diff` → `validate` → `plan_save` →
  worker `Modify`/`ModRdn`; Cancel reverts to the last-read values.

### 5.6 Live re-spin + dirty guard (state machine, tty-free)

- Pane 3 has a **clean/dirty** state. It becomes dirty when any bound value differs
  from the last-read baseline.
- Moving the pane-2 highlight:
  - **clean** → immediately re-read + re-render pane 3.
  - **dirty** → pop a **Save / Discard / Stay** dialog:
    - *Save* → run the existing save flow, then move.
    - *Discard* → drop edits, move.
    - *Stay* → cancel the move, keep editing.
- A `*` marker in the pane-3 title indicates dirty. The guard decision (given
  clean/dirty + chosen action) is a pure function, unit-tested.

### 5.7 Create / delete & pane reflow

- **Create** ("New") offers **configured profiles + a generic objectClass picker**
  (reuses `empty_form_for_profile` and the generic schema-driven form). The new entry
  is created under the currently selected branch (or under `‹self›`).
- After a successful create, an entry that gains its **first** child is **promoted**
  leaf→branch (appears in pane 1); deleting the **last** child demotes branch→leaf.
  These reflows are computed locally against the structure model — no extra queries.
- **Delete** keeps the existing confirmation; after delete the parent's children list
  is recomputed locally and panes reflow.

### 5.8 Read-only mode

- Global read-only is set by a config flag (`read_only = true`) **and/or** detection of
  an **anonymous bind** (no `bind_dn`).
- In read-only mode pane 3 shows **no Save/Cancel** and renders all fields read-only;
  Create/Delete actions are suppressed.
- Independent of mode, a write rejected with `insufficientAccess` (rc 50) is surfaced
  to the operator (no silent success) — already handled by `WriteError` mapping.

### 5.9 Refresh & staleness

- A manual **Refresh** action re-runs the eager structure scan.
- The existing refresh-after-write behavior is retained (re-read the affected entry and
  recompute the affected container locally).
- Live sync (RFC 4533 refreshAndPersist) is out of scope; the eager model is the
  correct foundation to add it onto later.

## 6. Config additions

```toml
[server]
# ... existing ...
read_only = false          # NEW: global read-only mode (also implied by anonymous bind)
```

(Profiles already exist and are reused for Create. No other config changes required.)

## 7. Data flow summary

1. **Startup:** spawn worker → fetch subschema (existing) → `LoadStructure` (subtree,
   paged) → build structure model → mount `SplitContainer` with the three panes.
2. **Navigate:** select branch (pane 1) → pane 2 lists its leaves + `‹self›` → highlight
   an entry → pane 3 base-reads + renders.
3. **Edit:** type in pane 3 → dirty; Save → diff/validate/write → on `WriteOk` re-read +
   local reflow; moving away while dirty → Save/Discard/Stay.
4. **Create/Delete:** profile/generic create or delete → write → local reflow
   (promote/demote) → pane 3 reflects.
5. **Refresh:** manual → re-run eager scan.

## 8. Testing strategy

**Pure / tty-free (unit-tested):**

- Structure model: tree build from flat DNs, branch/leaf classification,
  leaves-of-branch, promote/demote reflow.
- Pane 2 incremental-search filter.
- Pane 3 dirty-state machine + Save/Discard/Stay guard decision.
- Create-selection (profile vs generic) wiring.
- Paged-search request assembly (scope/page-size), and read-only-mode derivation
  (flag ∨ anonymous bind).

**Tty / facade (not unit-tested; covered by live + manual on a real terminal):**

- `SplitContainer` divider drag + layout, the three panes, pane-3 scrolling, focus
  cycling.

**Live (podman OpenLDAP, gated by `EDAPTOR_TEST_LDAP_URI`):**

- Subtree paged structure load returns the full tree past the default size limit.
- End-to-end edit/create/delete round-trip through the new panes' save path.

## 9. Risks & open items (to resolve during the implementation spike)

1. **Pane-3 scrolling pattern.** `Scroller` is documented as a base for text
   viewers/editors; it manages offset/limit but may not auto-host arbitrary child
   `InputLine`s. We may need to reposition field editors by the scroll delta ourselves.
   A spike (mirroring the M4.1 TV spike) confirms the exact pattern before planning
   locks the approach. *This is the main implementation risk.*
2. **ldap3 0.12 paged-results API.** Confirm the exact adapter/iterator for RFC 2696
   and how to thread the cookie on the worker's sync `LdapConn`.
3. **Divider drag focus capture.** Confirm `SF_RESIZING` (or the crate's equivalent)
   keeps mouse capture during a drag, as the skeleton assumes.
4. **Eager load progress UX.** Show progress during the initial scan; decide
   sync-at-startup vs background-with-spinner.

## 10. Reuse map (what stays unchanged)

- `form/changeset.rs` (diff → ChangeSet, MODRDN detection) — unchanged.
- `form/validate.rs` (MUST/single-value/syntax + `plan_save`) — unchanged.
- `ldap/ldif.rs` (LDIF render for preview) — unchanged.
- `ldap/result.rs` (rc → human message, incl. `insufficientAccess`) — unchanged.
- Worker write paths (`Modify`/`Add`/`ModRdn`/`Delete`) — unchanged; only **read** side
  gains Subtree + paged.
- `ui/form.rs` (`FormModel`, `WidgetSpec`) — reused; presentation moves from modal
  `Dialog` to the pane.

The modal `edit_entry_dialog` / `build_entry_dialog` become the embedded pane-3 form;
the single-window `DitOutline` mount becomes the pane-1 child of `SplitContainer`.
