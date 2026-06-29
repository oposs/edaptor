# TUI polish: light theme + interaction rough edges

**Date:** 2026-06-29
**Status:** Design — approved for planning

## Motivation

Interactive testing surfaced ~14 rough edges in the three-pane browser and its
dialogs: inconsistent panel backgrounds, no clear active-panel indication, weak
focus/selection contrast, an invisible cursor, Tab navigation that descends into
panels, missing scrollbars, and several widget/dialog affordance gaps. This pass
fixes all of them. The unifying decision is to **move from the dark
`classic_blue` palette to a light (Solarized Light) scheme**, which also solves
cursor visibility for free (the hardware cursor is rendered dark/inverse by the
terminal and is only visible on a light surface).

Theming is **code-level only** — no TOML config surface — but all color choices
are centralized in one builder so they are trivial to tune and could be exposed
as config later.

## Rough-edge inventory → workstream

| # | Reported issue | Workstream |
|---|----------------|------------|
| 1 | Panels have different base bg (blue vs cyan) | 1 Theme |
| 2 | Tab descends into the last panel | 2 Navigation |
| 3 | Active panel not visually distinct | 1 Theme |
| 4 | Active element has no uniform coloring (esp. form) | 1 Theme |
| 5 | Active item should differ between active/inactive panel | 1 Theme |
| 6 | Clicking a form label does not activate it | 2 Navigation |
| 7 | Not clear which form items are editable | 1 Theme |
| 8 | All three panels need a scrollbar, only when active | 3 Scrollbars |
| 9 | Popups need scrollbars too | 3 Scrollbars |
| 10 | Save/Discard/Stay buttons too narrow | 5 Dialog polish |
| 11 | Object-types: use dual-list instead of checklist | 4 Widgets |
| 12 | Search filter has no prompt | 4 Widgets |
| 13 | Dark cursor on dark blue is invisible | 1 Theme (light scheme) |
| 14 | Multivalue: unclear how to add/remove | 4 Widgets |

## Framework facts (tvision-rs 0.3.0)

- `Theme` is `#[derive(Clone)]` with `set_style(role, style)`; built from
  `Theme::classic_blue()`. 75 `Role` slots; `Style { fg, bg, modifiers }` with
  `Color::Rgb(u8,u8,u8)` for true color. We clone and override.
- The blue-vs-cyan panel difference is **per-widget role painting**, not a config:
  the leaf `ListBox` paints `ListNormalActive/Inactive` (cyan bg in classic_blue)
  while tree/form use blue-family roles. Unifying = overriding those role
  backgrounds to one value.
- `ListViewer`/`Outline` already pick **active vs inactive** role variants based on
  whether their pane is in the active focus chain (`state.selected && state.active`),
  and **focused vs normal** item via `ListFocused`/`OutlineFocused`. So most of the
  active-panel and active-item contrast falls out of the role table — no custom
  draw needed for the list/leaf pane.
- The tree (`Outline`) and form have **no built-in active/inactive role pair**, so
  their active-panel tint needs a small focus-keyed background fill in `draw()`.
- Cursor is a hardware cursor: not styleable. `block_cursor()` makes it a block;
  visibility comes from sitting on a light field.
- `ScrollBar::new(bounds)` (1-cell wide → vertical); `ListBox`/`Outline` accept a
  scrollbar `ViewId` at construction and the framework syncs them. Visibility is
  toggled with `set_visible`.

## Workstream 1 — Light theme

New module `src/ui/theme.rs` exposing `edaptor_theme() -> Theme`. `app.rs:731`
calls it instead of `Theme::classic_blue()`. All palette decisions live here.

### Palette (Solarized Light)

| Slot | Hex | Used for |
|------|-----|----------|
| desktop bg | `#e3ddc8` | desktop behind panels (darker tan, separates panels) |
| inactive panel bg | `#eee8d5` (base2) | panel surfaces when not focused |
| active panel bg | `#fdf6e3` (base3) | panel surface when focused (brightest) |
| body text | `#586e75` (base01) | normal text |
| secondary/disabled | `#93a1a1` (base1) | frames, disabled, scrollbar thumb |
| input field bg | `#fffdf3` | editable fields (brightest, signals "type here") |
| current item | bg `#268bd2` / fg `#fdf6e3` | focused list/tree/form item |
| multi-selected | bg `#b5cdd8` / fg `#586e75` | staged/checked items |
| accent / title / prompt | `#268bd2` (blue) | titles, filter prompt, headers |
| scrollbar track | `#eee8d5` | bar trough |

(Exact hexes finalized in implementation; canonical Solarized values are the
reference.)

### Role mapping

- **Uniform panel bg (1):** set bg of `ListNormalActive`, `ListNormalInactive`,
  `OutlineNormal`, `Normal`, `InputNormal`, and the frame interiors to the panel
  surfaces — no more cyan.
- **Active panel tint (3) & active item differs by panel (5):**
  - Lists/leaf: `ListNormalActive`/`ListNormalInactive` (and the active vs inactive
    flavor of the focused-item role) get the cream vs base2 backgrounds — the
    framework already swaps them by focus, so the focused panel reads brighter and
    its current row is more saturated than an inactive panel's.
  - Tree (`Outline`) and form (`FormPane`/`ScrollGroup`): add a focus-keyed
    background fill in `draw()` (cream when `state.focused`, base2 otherwise) so
    they match the list behavior despite lacking active/inactive roles.
- **Active element uniform coloring (4):** `ListFocused`, `OutlineFocused`, and the
  form's current-field highlight all use the same accent (`#268bd2` bg / cream fg),
  so "the current thing" looks identical in every pane including the form.
- **Editable affordance (7):** editable field roles (`InputNormal`, password/choice
  editor surfaces) use `input_bg` (`#fffdf3`), brighter than the panel — read-only
  labels stay on the panel surface. Editable = brightest.
- **Cursor (13):** focused inputs call `block_cursor()`; the block is visible on the
  light input field. No promise of a recolored cursor (not possible).

## Workstream 2 — Navigation

- **Tab reserved for panels (2):** Tab/Shift-Tab only cycle the three panes; they
  must not descend into a pane's internal field list. The form pane currently lets
  Tab enter its `ScrollGroup` field chain — change so within-pane movement is
  arrows/PgUp/PgDn only, and Tab always bubbles to the splitter. Verify leaf/tree
  already behave (they navigate lists with arrows).
- **Click label to activate (6):** in `FormPane`, a mouse click on a field's label
  cells focuses/activates that field's editor, not only a click on the editor cell.
  Map label hit-test → focus the associated child id.

## Workstream 3 — Scrollbars

- **Per-pane bars, focus-gated (8):** tree and leaf panes gain a vertical
  `ScrollBar` sibling (form already has one via `ScrollGroup`). Per the chosen
  behavior, the bar **and its 1-col gutter are shown only when the pane is focused
  AND content overflows** — fully hidden (column reclaimed) otherwise. Accept the
  1-column reflow on focus change. Toggle via `set_visible` keyed on
  `state.focused && overflow`.
- **Popup scrollbars (9):** dialogs whose lists can exceed their height (dual-list,
  picker, choice, multivalue) get scrollbars wired to their list views. Add where
  missing.

## Workstream 4 — Widgets

- **Dual-list extraction (11):** create `src/ui/dual_list.rs` — a reusable
  two-pane widget (available-left / members-right, column headers, `Tab` flips the
  active column, `Ins`/`→` moves to members, `Del`/`←` removes, with the
  available-side search box). Extract from the current `membership.rs` layout.
  Rewire both:
  - `membership.rs` → consume `DualList`.
  - `oc_picker.rs` → replace the single checklist with `DualList` (active object
    classes on the left, available on the right). Structural/required classes that
    cannot be removed are shown but non-removable (kept on the left, move/remove
    rejected with feedback).
- **Multivalue add/remove (14):** add visible `[+ Add]` and `[- Del]` buttons to
  `multivalue.rs` (keyboard `Ins`/`Del` still work). Buttons sit below the value
  list.
- **Search filter prompt (12):** the leaf-pane filter input (`leaf.rs:35`, bare
  full-width) gets a visible left-aligned `Filter:` prompt label preceding the
  input (placeholder text rejected — it vanishes once typing starts). The input
  shrinks by the prompt width.

## Workstream 5 — Dialog polish

- **Button widths (10):** widen the Save/Discard/Stay guard dialog
  (`dialog/guard.rs`, currently 56 wide) and/or pad the button captions so
  "Discard" no longer touches the panel edge. Audit `confirm.rs` for the same.

## Affected files

- New: `src/ui/theme.rs`, `src/ui/dual_list.rs`
- `src/ui/app.rs` (theme wiring ~731; splitter/Tab)
- `src/ui/panes/tree.rs`, `panes/leaf.rs`, `panes/form.rs` (focus fill, scrollbars,
  label click, filter prompt, Tab containment)
- `src/ui/scroll_group.rs` (focus-gated bar visibility pattern, reuse for panes)
- `src/ui/membership.rs`, `src/ui/oc_picker.rs` (consume `DualList`)
- `src/ui/multivalue.rs` (buttons)
- `src/ui/dialog/guard.rs`, `src/ui/dialog/confirm.rs` (button widths)

## Out of scope / non-goals

- No TOML/config theming surface (explicitly deferred; code-level only).
- No recolored cursor (hardware-cursor limitation).
- No changes to LDAP/data logic — purely presentation and interaction.
- No unrelated refactoring beyond the `DualList` extraction needed for #11.

## Testing & verification

- `make check` (fmt + clippy -D warnings + tests), `cargo test -j4`.
- Manual against the podman demo server (`scripts/test-ldap.sh start`,
  `cargo run -- --config examples/demo-config.toml`): verify uniform light panels,
  active-panel cream tint, accent on current item in all three panes, visible block
  cursor in inputs, Tab cycling panels only, click-to-focus form labels,
  focus-gated scrollbars on each pane and in overflowing dialogs, object-types as a
  dual-list, multivalue buttons, `Filter:` prompt, and the widened guard dialog.
- Update `CHANGES.md` (user-visible: new light theme + interaction fixes) and the
  relevant `docs/src/` pages (widgets page for object-types dual-list and
  multivalue buttons; any theming note).

## Open implementation nuances (decide during build)

- Exact Solarized hexes and which of the 75 roles need overriding (the table above
  is the intent; a full role sweep will catch stragglers like menu/status-line
  surfaces so nothing stays dark-blue).
- Tree/form focus-fill: confirm the fill paints behind child widgets without being
  overpainted into invisibility (children paint their own cells; the fill shows in
  margins/empty rows — acceptable, matches list behavior).
