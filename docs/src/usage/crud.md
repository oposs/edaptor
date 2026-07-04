# Creating, Editing, Renaming, Deleting

All four operations happen in the [three-pane TUI](three-pane.md) and end the
same way: eDAPtor computes the exact change, shows you an LDIF preview, and only
then applies it. The mechanics of that final step are described under
[Change Flow](../concepts/change-flow.md); this page walks the four flows that
lead into it.

## Editing an entry

Select an entry in the **Entries** pane to load it into the **Entry** form. The
form is generated from the entry's `objectClass` definitions in the live schema,
so every editable attribute appears with the right cardinality and the
read-only / system attributes are shown but not editable.

1. Focus the form pane (`Tab` until the **Entry** pane has the double border).
2. Move between fields with `↑↓`. The **bottom status line** shows
   context-sensitive editing hints for the focused field.
3. **Single-value text fields** — type directly into the field.
4. **Multi-value free-text and ordered fields** — the field renders as an inline
   bulleted list; type, use **Enter** to add an item, **Backspace** to remove
   characters (and empty items), **↑/↓** to move between items, and
   **Ctrl+↑/↓** (or the `≡` handle via **←** at offset 0) to reorder ordered
   fields. See [Inline multi-value editing](../configuration/widgets.md#inline-multi-value-editing)
   for the full key reference.
5. **Object-class, membership, picker, choice, and password fields** — the
   field shows a read-only block (`<not set>` if empty, `*****` for passwords)
   that highlights as a whole when focused. Press any action key (or **Enter**)
   to open the field's editor modal.
6. As soon as a value differs from what was read, the form is **dirty**: a `*`
   appears next to the DN in the status line.
7. Press **`Alt+S`** to save. eDAPtor diffs your edits against the original
   entry, builds the change, shows the LDIF preview, and applies it on
   confirmation. If nothing actually changed, it reports that instead of writing.
8. Press **`Alt+C`** to cancel, reverting the form to the last-read values.

### Changing objectClasses

The `objectClass` field opens a schema-seeded **two-column editor** (the shared
*Shuttle* view): active (current) classes on the left, available classes on the
right. Highlight an available class and press **Insert** to move it into the
active set; highlight an active class and press **Delete** to remove it. **Enter**
while a list holds focus does the same move (toward the active set from the
available list, out of it from the active list). The same actions are available
as on-screen **[Add]** / **[Remove]** buttons (also reachable with **Alt+A** /
**Alt+R**). Each column shows a scroll bar when its list overflows. **Tab** /
**Shift-Tab** move focus between the two lists and the buttons; the arrow keys
drive whichever list is focused. Typing while the available list is focused
filters it in place (incremental find; Backspace widens, Esc clears). STRUCTURAL
classes that were already on the entry are shown locked (marked `*`) and cannot
be removed; a structural class you add during this edit can still be removed
again. Press the **OK** button to confirm, or the **Cancel** button to close the
editor without committing. The form immediately
updates: new MUST/MAY fields appear for
added classes, and any attribute no longer permitted by the remaining classes is
shown **crossed out** (it will be deleted on save). Press **Alt+C** in the form
to discard all objectClass changes and revert to the server state. See
[objectClass Editor](../configuration/widgets.md#objectclass-editor-auto-injected)
for full details.

### The dirty-guard

If you try to navigate away from — or quit while — a form has unsaved edits,
eDAPtor does not silently discard them. It opens a guard overlay
(*"This entry has unsaved edits. Save them before moving on?"*) offering
**Save**, **Discard**, or **Cancel**:

- **Save** runs the normal save flow, then completes the move (or the quit).
- **Discard** drops the edits and completes the move.
- **Cancel** stays on the entry so you can keep editing.

The guard fires when you change the selected entry, move focus off the form
pane, or quit (`Alt+X`).

## Creating an entry

Press **`Alt+N`** in the **Entries** pane to create a new entry under the
currently selected branch.

1. A **profile chooser** overlay lists the entry profiles from your config
   (`user`, `group`, …). Pick one with `↑↓` and `↵` (or `Esc` to cancel).
2. eDAPtor builds an empty form for that profile, pre-filling its
   [defaults](../configuration/defaults.md): literal values, templated values
   such as `homeDirectory = "/home/{uid}"`, and auto-numbered values such as
   `uidNumber = "{next:10000-60000}"`. Defaults only fill **empty** fields — once
   you type a value into a field, the default never overwrites it.
3. Fill in the remaining fields (the form title reads `New entry`).
4. Press **`Alt+S`**. The change is an LDIF *add*; review the preview and confirm
   to create the entry. The new entry appears in the **Entries** pane, and if it
   gives its parent its first child, the parent is promoted to a branch in the
   **DIT** tree.

## Renaming (ModRdn)

eDAPtor has no separate "rename" command — a rename is just an edit of the
entry's **naming attribute** (its RDN attribute, e.g. `uid` for a user or `cn`
for a group):

1. Edit the value of the naming attribute in the form and press **`Alt+S`**.
2. eDAPtor detects that the value naming the entry has changed and emits a
   **ModRdn** (rename) operation instead of a plain modify. If you also changed
   other attributes in the same save, the rename is applied first, then the
   modifications.
3. The LDIF preview shows the rename explicitly before you confirm.

Adding an *extra* value to a multi-valued naming attribute (without removing the
one that currently names the entry) is treated as an ordinary modify, not a
rename.

## Deleting an entry

Press **`Alt+D`** in the **Entries** pane to delete the highlighted entry.

1. A confirmation overlay appears (`[Y]es` / `[N]o`).
2. Confirm with **`Y`** to apply the delete; **`N`** (or `Esc`) aborts.
3. After a successful delete the parent's entry list is recomputed locally; if
   you removed the parent's last child, the parent is demoted from a branch back
   to a leaf in the **DIT** tree.

All of create, edit, rename, and delete funnel through the same
diff → ChangeSet → LDIF-preview → apply pipeline; see
[Change Flow](../concepts/change-flow.md) for the details and for how write
errors (such as `insufficientAccess`) are surfaced.
