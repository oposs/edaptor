# The Three-Pane TUI

eDAPtor presents the directory as three side-by-side panes — a navigation
tree, an entry list, and a detail/edit form — over a single bottom status
line. The layout is persistent: you keep the directory, the current container's
entries, and the selected entry's form all on screen at once, instead of losing
context to a modal dialog every time you edit.

## The three panes

```
┌─ DIT ───────┐┌─ Entries ────────┐╔═ Entry — uid=bob,ou=people,… ═╗
│ dc=example  ││ /                │║ uid           bob             ║
│ ├─ people   ││ ‹self› people    │║ cn            Bob Baker       ║
│ └─ groups   ││ Bob Baker (bob)  │║ sn            Baker           ║
│             ││ Babs Carr (babs) │║ givenName     Bob             ║
│             ││ Carl Diaz (carl) │║ mail          bob@example.org ║
│             ││ …                │║ uidNumber     10001           ║
│             ││                  │║ …                             ║
└─────────────┘└──────────────────┘╚═══════════════════════════════╝
 ↑↓ Field · ↵ Edit · Alt+S Save · Alt+C Cancel · Alt+X Quit
```

*(The form pane is focused here, so it carries the bold double border and a
brighter background. In a real terminal the active border is drawn in a blue
accent colour, which ASCII cannot show.)*

- **DIT (navigation tree)** — the directory's branch structure: every container
  (an entry that has children), with the base DN as the root. The whole
  structure is loaded eagerly at startup, so navigation is instant and eDAPtor
  knows exactly which nodes are branches and which are leaves. Selecting a branch
  drives the entry list. Move with `↑↓`, fold/unfold a branch with `←→`.

- **Entries (entry list)** — the entries directly under the selected branch. The
  top row is an incremental-search box with a `Filter:` label; below it is a
  `‹self›` row representing the branch entry itself (editable like any other
  entry), followed by the branch's leaf entries. Each entry is shown with its
  profile **label** — for example `Bob Baker (bob)` from a `label = "{cn} ({uid})"`
  — rather than a raw DN. Just start typing to filter the list (the search
  matches against the rendered label). Moving the highlight with `↑↓` selects the
  current entry and loads it into the form.

- **Entry (detail/edit form)** — a scrollable form for the selected entry, one
  row per attribute (label on the left, value on the right). The form is
  generated from the entry's `objectClass` definitions in the live schema, so it
  always matches what the directory actually allows. It re-loads as you move the
  highlight in the entry list. The pane title shows the current DN (or
  `New entry` while creating). Move between fields with `↑↓`, open a field for
  editing with `↵`.

## Focus and the status line

Exactly one pane is focused at a time. The interface uses a **light Solarized-Light
colour scheme**: cream/tan panels, dark slate text, and a blue accent. The focused
pane is rendered in a brighter cream tone; unfocused panes are slightly greyed.
Within the entry form, **editable fields have a visibly brighter background** than
read-only labels, making the edit affordance immediately apparent. A vertical
scrollbar appears in a pane **only while it is focused** and only when the content
overflows the visible height.

- **`Tab`** moves focus forward (DIT → Entries → Entry → DIT) — it cycles panes
  only and does **not** descend into a pane's internal fields.
- **`Shift-Tab`** moves focus backward.
- Use the **arrow keys** to move within the focused pane.
- **Clicking a form label** moves focus to that field's input directly.

Moving focus off the form pane while it has unsaved edits opens the dirty-guard
(see [Creating, Editing, Renaming, Deleting](crud.md)).

Key hints live in the single **status line** at the bottom of the screen rather
than in the pane borders (the narrow panes would clip them). The status line
follows focus and shows, in order:

- a `[read-only]` tag when the session is read-only;
- the transient status message (e.g. `Saved.` / `Created.`) when there is one;
- the **focused pane's hotkeys**:
  - DIT — `↑↓ Move · ←→ Fold · Alt+R Refresh`
  - Entries — `↑↓ Select · Type to search · Alt+N New · Alt+D Del`
  - Entry — `↑↓ Field · ↵ Edit · Alt+S Save · Alt+C Cancel`
- `Alt+X Quit`, so the global quit is discoverable from anywhere;
- last, the current DN with a trailing `*` when the form has unsaved edits.

In read-only mode the write keys (`Alt+N`, `Alt+D`, `Alt+S`, `Alt+C`) are
dropped from the hints, and the form renders its fields as non-editable;
`Alt+R Refresh` remains available because it only re-reads.
