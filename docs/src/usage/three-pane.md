# The Three-Pane TUI

edaptor presents the directory as three side-by-side panes — a navigation
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
 ↑↓ Field · ↵ Edit · Alt+S Save · Alt+C Cancel   ·   Alt+X Quit   ·   uid=bob,…,dc=example,dc=org
```

*(The form pane is focused here, so it carries the bold double border. In a real
terminal the active border is also drawn in cyan, which ASCII cannot show.)*

- **DIT (navigation tree)** — the directory's branch structure: every container
  (an entry that has children), with the base DN as the root. The whole
  structure is loaded eagerly at startup, so navigation is instant and edaptor
  knows exactly which nodes are branches and which are leaves. Selecting a branch
  drives the entry list. Move with `↑↓`, fold/unfold a branch with `←→`.

- **Entries (entry list)** — the entries directly under the selected branch. The
  top row is an incremental-search box (shown as `/ …`); below it is a
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

Exactly one pane is focused at a time. The focused pane is marked by a **bold,
double-line border** (drawn in cyan); the other two panes get a dim single-line
border. There is no background inversion — edaptor uses the terminal's default
(typically light/white) background everywhere, so it reads cleanly in any
color scheme.

- **`Tab`** moves focus forward (DIT → Entries → Entry → DIT).
- **`Shift-Tab`** moves focus backward.

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
