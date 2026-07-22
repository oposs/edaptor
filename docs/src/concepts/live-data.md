# Live data, find and reload

eDAPtor loads the directory tree once at startup and keeps that model in step
with the directory as you work. Three mechanisms do it.

## Searching always asks the server

Typing in the entry list runs a one-level search under the selected container,
matching the attributes your `label` template renders. An entry another
administrator created seconds ago is therefore findable immediately — the find
is never answered from the copy loaded at startup. Matches are folded into the
local model, so they stay listed after you clear the query. At most 500
matches are returned per find; when that limit is hit the status line says so
and asks you to narrow the query.

The `lookup` field's candidate list works the same way: every keystroke asks
the server, capped at 100 candidates per query, so candidates beyond the first
page are reachable by typing.

## Writes update the list immediately

Creating an entry adds it to the list and selects it. Renaming one moves it.
Editing an attribute that appears in a label re-renders that label in the tree
and the entry list. This happens without a rescan: every entry eDAPtor reads —
including the read that follows a save — refreshes that entry in the model.

## Alt+R reloads the tree

Structural changes made by other clients (a new container, a deleted subtree)
cannot be observed locally. **Alt+R** re-runs the full scan, keeping your place:
the selected container and entry are restored when they still exist. The scan
blocks briefly on large directories. Your open edit form is left untouched, so
unsaved changes are never at risk.
