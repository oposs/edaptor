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
2. Move between fields with `↑↓`, open the highlighted field with `↵`, and type
   the new value.
3. As soon as a value differs from what was read, the form is **dirty**: a `*`
   appears next to the DN in the status line.
4. Press **`Alt+S`** to save. eDAPtor diffs your edits against the original
   entry, builds the change, shows the LDIF preview, and applies it on
   confirmation. If nothing actually changed, it reports that instead of writing.
5. Press **`Alt+C`** to cancel, reverting the form to the last-read values.

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
