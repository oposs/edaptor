# Inline multi-value fields: bulleted lists, in-place editing, launch fields, footer hints

Date: 2026-07-04
Status: approved (design)
Branch: `feat/shuttle-widget`

## Problem

Multi-value attributes render today as a **single-line joined summary** in one
`InputLine` per field (`mail: a@x, b@x`), and every rich editor (free-text
multi-value, ordered, object-class, membership, password) is reached by opening a
**modal dialog**. This is opaque: the user cannot see the individual values in the
form, cannot tell how many there are without opening a dialog, and every edit is a
context switch into and out of a popup.

The user wants the form to show and edit multi-value data **in place** as a
bulleted list, with editing behaviour that matches each attribute's nature:

```
label1: - value1
        - value2
          with second line
        - value3

label2: <not set>

ObjectClass: - class1
             - class2

Password: *****
```

## Goals

- Multi-value fields render as an **inline bulleted list**, variable height, with
  the label right-aligned on the first line and continuation lines blank in the
  label column.
- **Free-text** multi-value lists are edited **in place** — the cursor lives in
  the form, no popup. Enter adds a value, Ctrl-Enter adds a newline within a
  value, deleting a value's content and its marker removes it.
- **Ordered** lists (e.g. `olcAccess`) edit in place too and can be **reordered**.
- **Shuttle / choice / password** fields render a read-only block that
  **highlights as a whole** on focus; any action key opens the existing modal.
- The bottom status line shows **dynamic, context-sensitive editing hints** that
  change with the focused field's type and state.
- Single-value text fields are **unchanged** (edit in place, no bullet).

## Non-goals

- No change to the LDAP write path, changeset/diff logic, or `staged_commit`
  semantics — inline editing feeds the *same* `EditField.values`.
- No reordering of plain (unordered) multi-value attributes: an LDAP multi-value
  attribute is a set; reordering it is a semantic no-op (`changeset` already
  treats a pure reorder of an unordered field as no change), so we do not offer a
  reorder affordance there.
- No change to the tree / leaf-list panes or the DN header row.

## Architecture

### From fixed rows to variable-height field blocks

Today the form pane (`src/ui/panes/form.rs`) is a `ScrollGroup` of **fixed
height-1 rows**: field `i` is a `FieldLabel` + an `InputLine`, both at `y = i`.
`rebuild_cells` lays them out at `y = row_index` and `render` pushes a joined
summary string into each `InputLine`.

The new model makes each field **one composite value-view** whose height is
dynamic. The `ScrollGroup` stacks the field blocks at `y`-offsets computed by
**summing block heights** (the DN header stays at row 0). The label prints
right-aligned on the block's **first** line; continuation lines leave the label
column blank.

### Three value-view types

Dispatched from the field's widget kind (`widget_for` / the `WidgetKind` the field
carries). Each is a self-contained `View` with one clear responsibility.

| View | Field kinds | Height | Behaviour |
|------|-------------|--------|-----------|
| `TextValueView` | single-value text/int (`cn`, `sn`, `uidNumber`), and read-only/computed fields | 1 | inline edit in place, no bullet — today's behaviour, wrapped in the new block model |
| `ListValueView` | multi-value free-text (`MultiValueWidget`) + ordered (`OrderedWidget`) | N lines (≥1) | **new** inline bullet editor; cursor lives inside |
| `LaunchValueView` | object-class, memberships/pickers, single-DN picker, choice, password | multi → N bulleted lines; single → 1 line | read-only display block; whole block highlights on focus; an action key opens the existing modal |

- Empty `ListValueView` and empty multi `LaunchValueView` collapse to a single
  line `label: <not set>` (dim). This **supersedes** the interim
  `<press ENTER to add Value(s)>` placeholder — the empty state is now `<not set>`.
- The free-text and ordered **modals are retired** (`MultiValueEditor`,
  `OrderedEditor`) — replaced by inline editing. The shuttle / object-class /
  choice / picker / password modals **stay**, now launched by `LaunchValueView`.

### Navigation

- `Tab` — switch panes (unchanged).
- `↑ / ↓` — move the cursor. Crossing the top/bottom display line of a block moves
  to the adjacent field. On a `LaunchValueView` the block is highlighted as a
  whole, so `↑ / ↓` jump straight to the neighbouring field.
- `← / → / Home / End / PgUp / PgDn` — cursor movement only; never launch.
- Every other key is an **action**: edits inline (`Text` / `List` views) or
  launches the modal (`Launch` views). For a `LaunchValueView`, Enter and any
  printable key both open the editor.

The `ScrollGroup` scrolls to keep the cursor's absolute `y` visible as the cursor
moves through variable-height blocks.

## `ListValueView` — the inline bullet editor

The core new widget. Its editing model is a **pure, ctx-free unit** so it is
unit-testable in isolation (like the existing widgets); the `View` wrapper adds
rendering, focus, and event threading.

### Model and rendering

- Backing state is the field's `Vec<String>`; a value may contain `\n` for
  continuation lines.
- Cursor = `(item_index, grapheme_offset)` into the flattened item text, plus a
  distinct **handle** position (see reordering) for ordered fields.
- Rendering: each item's first display line is `- ` + its first text line;
  `\n`-split remainder renders as continuation lines indented to align under the
  text (under the first char after `- `). Grapheme stepping reuses tvision's
  `text::next` / `text::prev`.

### Editing keys

- **Printable char** → insert at cursor. On a `<not set>` field the first key
  creates item 0 (`- ` + char).
- **Enter** → split the current item at the cursor; text after the cursor becomes
  a new item below (at end → a new empty item).
- **Ctrl-Enter** → insert `\n` at the cursor (a continuation line within the item).
- **Backspace** → delete the char before the cursor; at offset 0 of an item, merge
  this item into the previous one — so emptying an item and pressing Backspace once
  more removes its `- ` marker. Backspace at offset 0 of item 0 does nothing.
- **Delete** → forward delete; at the end of an item, pull the next item up.
- **Removing the last item** → the field reverts to `<not set>`.

### Reordering (ordered fields only)

Ordered fields (`olcAccess`-style) offer **two** reorder mechanics; plain
multi-value lists offer neither (static `-`, cursor never enters the handle).

1. **Ctrl-↑ / Ctrl-↓** — move the current item up/down regardless of cursor
   position within it.
2. **Handle drag** — pressing **←** at text offset 0 moves the cursor **onto the
   marker**. While there the marker renders as a **wide hamburger `≡`** (the
   drag-to-reorder handle); plain **↑ / ↓** move the item up/down, and **→** (or
   typing) returns to the text and restores the `-`.

`{n}` ordering prefixes are renumbered on commit, reusing the existing ordered
reconstruct logic (`src/ui/ordered.rs`).

### Commit integration

Inline edits write back into `edit_form.fields[i].values` directly. The same
trim-and-drop-empties rule the retired modal applied is applied on focus-out /
commit. The existing changeset/diff and dirty-marker logic is therefore unchanged
— there is **no new commit path**, and reordering an unordered field remains a
no-op through `changeset` exactly as today.

## Dynamic footer hints

The bottom `StatusLine` (`init_status_line` in `src/ui/app.rs`) is driven by
**help-context**: the focused field's view type and state select a hint string,
updated on every focus / state change (tvision `StatusLine` supports
help-context-selected defs and a dynamic hint callback; whether the focused view's
help-context auto-propagates or must be pushed explicitly on focus change is
verified during implementation). The global `Alt-N / Alt-S / Alt-X` actions stay;
the context hint occupies the remaining width.

| Focused field state | Hint |
|---------------------|------|
| `TextValueView` | `↑↓ move · Enter next field` |
| `ListValueView`, has items | `Enter add · Ctrl-Enter newline · Backspace empties→removes · ↑↓ move` |
| `ListValueView`, ordered | append `· Ctrl-↑↓ or ← handle to reorder` |
| `ListValueView`, empty | `Type to add first value` |
| `ListValueView`, on `≡` handle | `↑↓ reorder · → back to text` |
| `LaunchValueView`, shuttle/choice/picker | `any key: open picker · ↑↓ move` |
| `LaunchValueView`, password | `any key: edit password` |

Exact wording is refined during implementation; the mechanism (help-context →
hint) is the load-bearing decision.

## Testing

- **`ListValueView` model** (pure, ctx-free): value↔display mapping, cursor moves
  across items and continuation lines, every edit op (insert, Enter split,
  Ctrl-Enter newline, Backspace-merge / marker-remove, Delete-pull-up), `<not set>`
  transitions (first key creates item 0; removing the last item reverts), ordered
  reorder via Ctrl-↑/↓ and via the `≡` handle, `{n}` renumber on commit.
- **View / navigation** (integration, following current form-pane test patterns):
  variable-height block stacking and `y`-offset computation, cursor crossing block
  boundaries into adjacent fields, `LaunchValueView` whole-block highlight and
  action-key launch, nav-key vs action-key classification.
- **Footer**: help-context selects the right hint for each field type / state.
- **Regression**: single-value text fields unchanged; changeset diff still a no-op
  for a pure reorder of an unordered field.

## Rollout

One change (single implementation plan) covering the whole model. The order within
the plan is expected to be: variable-height block model + `TextValueView` and
`LaunchValueView` (preserving current behaviour) → `ListValueView` inline editor →
reordering → footer hints → retire the free-text/ordered modals. `CHANGES.md`,
`README.md` skeleton, and the mdBook widgets page are updated as part of "done".

## Superseded / affected work

- The interim `<press ENTER to add Value(s)>` empty-multi-value placeholder
  (committed `88b8032`) is replaced by the `<not set>` empty state.
- The `[+ Add] / [- Del]` buttons and the modal for **free-text** multi-value
  fields become dead once inline editing lands; remove them with the retired
  modal. Shuttle move buttons and the resizable picker work are unaffected.
