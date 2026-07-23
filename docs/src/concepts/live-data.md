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

Creating an entry adds it to the list and selects it — provided you created it
in the container you are looking at; a profile that creates into its own
configured container puts the entry there, so switch to that container to see
it. Renaming an entry moves it.
Editing an attribute that appears in a label re-renders that label in the tree
and the entry list. This happens without a rescan: every entry eDAPtor reads —
including the read that follows a save — refreshes that entry in the model.

Renaming a **container**, however, does trigger a rescan: every entry under it
changed its DN on the server, so no local reflow could be correct. eDAPtor runs
the same scan **Alt+R** does and keeps you on the container under its new name.

## Alt+R reloads the tree

Structural changes made by other clients (a new container, a deleted subtree)
cannot be observed locally. **Alt+R** re-runs the full scan, keeping your place:
the selected container and entry are restored when they still exist. The status
line reports how many entries were loaded, and a failed reload raises an error
dialog rather than passing silently for a successful one. The scan blocks briefly
on large directories. Your open edit form is left untouched as long as the
entry you were editing is still there, so unsaved changes are never at risk.

If the entry you were editing is gone when the projection is rebuilt — because
another client deleted or renamed it — edaptor tells you rather than quietly
moving you somewhere else. With no unsaved changes the form is cleared and the
status line names the entry. With unsaved changes nothing is thrown away: you
are asked whether to keep editing, discard your changes, or re-create the entry
from the values still on screen.
