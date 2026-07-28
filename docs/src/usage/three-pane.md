# The Three-Pane TUI

eDAPtor presents the directory as three panes — a navigation tree, an entry
list, and a detail/edit form — over a single bottom status line. The tree
(top-left) and the selected branch's entries (bottom-left) share the left
column; the detail/edit form fills the full-height right column. The layout is
persistent: you keep the directory, the current container's entries, and the
selected entry's form all on screen at once, instead of losing context to a
modal dialog every time you edit.

## The three panes

```
┌─ DIT ───────────┐╔═ Entry — uid=bob,ou=people,… ═╗
│ dc=example      │║ uid           bob             ║
│ ├─ people       │║ cn            Bob Baker       ║
│ └─ groups       │║ sn            Baker           ║
├─ Entries ───────┤║ givenName     Bob             ║
│ ‹self› people   │║ mail          bob@example.org ║
│ Bob Baker (bob) │║ uidNumber     10001           ║
│ Babs Carr (babs)│║ …                             ║
│ Carl Diaz (carl)│║                               ║
└─────────────────┘╚═══════════════════════════════╝
 ↑↓ Field · ↵ Edit · Alt+S Save · Alt+C Cancel · Alt+X Quit
```

*(The form pane is focused here, so it carries the bold double border and a
brighter background. In a real terminal the active border is drawn in a blue
accent colour, which ASCII cannot show.)*

- **DIT (navigation tree)** — the directory's container structure, with the base
  DN as the root. A **container** is an entry classed as `organizationalUnit`,
  `organization`, `dcObject`, `domain` or `container` (case-insensitive) — OR
  simply any entry that has children, whatever its class. This means an *empty*
  OU still shows up here, not just OUs that happen to already hold something. The
  whole structure is loaded eagerly at startup, so navigation is instant. Selecting
  a container drives the entry list. Move with `↑↓`, fold/unfold with `←→`. Moving
  to a different container clears any active entry-list filter, so the new
  container always lists unfiltered.

- **Entries (entry list)** — every entry directly under the selected container:
  sub-containers **and** leaf entries alike. The first row is a `‹self›` row
  representing the container entry itself (editable like any other entry),
  followed by its direct children in directory order. A child that is itself a
  container (e.g. a sub-OU) is marked with a `▸ ` prefix so it reads visually
  distinct from a plain entry; selecting it opens *its own* entry in the form for
  editing — exactly like the `‹self›` row — it does not navigate the tree. Each
  entry is shown with its profile **label** — for example `Bob Baker (bob)` from a
  `label = "{cn} ({uid})"` — rather than a raw DN (a sub-container without a
  matching label rule falls back to its RDN). Just start typing to filter the
  list in place (the incremental find matches against the rendered label and
  highlights the match, and runs as a live one-level directory search that
  returns sub-containers too; Backspace widens, Esc clears). Moving the
  highlight with `↑↓` selects the current entry and loads it into the form.

- **Entry (detail/edit form)** — a scrollable form for the selected entry. Each
  attribute is a variable-height block: single-value text fields show one line;
  multi-value fields expand to fit their values as an inline bulleted list. Cryptic
  attribute names carry a short readable hint — for example `sn (surname)`,
  `l (location)` or `ou (org. unit)` — so their meaning is obvious at a glance. The
  form is generated from the entry's `objectClass` definitions in the live schema,
  so it always matches what the directory actually allows. It re-loads as you move
  the highlight in the entry list. The pane title shows the current DN (or
  `New entry` while creating); a DN too long for the pane is cut at the *end*,
  with a `…` marking the cut, so the telling front part stays readable. Move
  between fields with `↑↓`, or page through the form with `PageUp`/`PageDown`;
  the status line shows context-sensitive editing hints for the focused field.

  **Every field is a stop, editable or not.** A read-only field still takes
  focus, so you can move the caret through it, scroll a value wider than its cell
  (`◄` / `►` mark the hidden part), select it and copy it with `Ctrl+C` — without
  that, a long DN could never be read to its end. Only *changing* it is refused:
  typing, deleting, cutting or pasting pops a dialog naming the field and saying
  why it will not budge (the server maintains it, the schema marks it
  `NO-USER-MODIFICATION`, the session is read-only, and so on).

  Multi-value blocks scroll the same way. In a read-only block — a group's
  `member` list, an `objectClass` set — `←`/`→` move the whole block sideways; in
  an inline multi-value editor the view follows the caret as you walk a long
  value. Either way the scroll returns to the left edge when focus lands on the
  field, so a value is never met mid-scroll.

  Below the attributes, separated by a blank line, sits the **audit block**: the
  server-maintained `createTimestamp`, `creatorsName`, `modifyTimestamp` and
  `modifiersName` of the entry, labelled simply `created`, `created by`,
  `modified` and `modified by` (their real names are long, and the label column
  is sized to the longest label in the whole form). These are *operational*
  attributes — the directory maintains them, so eDAPtor shows them read-only:
  reachable and copyable like any other read-only field, but never part of a
  save. Timestamps are rendered in your machine's local time
  (`2026-07-28 13:03:22`) rather than the raw LDAP `20260728110322Z`. The block
  refreshes after every save, so `modified` always reflects the write you just
  made. Servers that do not return these attributes (or a bind DN not permitted
  to read them) simply show no block.

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
- The **mouse wheel** scrolls the pane under the pointer: it moves the highlight
  in the tree or entry list, and in the entry form it moves between fields,
  scrolling the form so the focused field stays on screen.
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
  - Entry — **dynamic hints** that update with the focused field's type and
    state (e.g. `↑↓ move · Enter next field` for a plain text field,
    `Enter add · Ctrl-J newline · Backspace empties→removes · ↑↓ move`
    for an inline multi-value list, `any key: open picker · ↑↓ move` for a
    launch field such as `objectClass` or `memberOf`). `Alt+S Save · Alt+C Cancel`
    are always available.
- `Alt+X Quit`, so the global quit is discoverable from anywhere;
- last, the current DN with a trailing `*` when the form has unsaved edits.

In read-only mode the write keys (`Alt+N`, `Alt+D`, `Alt+S`, `Alt+C`) are
dropped from the hints, and the form renders its fields as non-editable;
`Alt+R Refresh` remains available because it only re-reads.
