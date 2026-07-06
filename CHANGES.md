# Changelog

All notable changes to eDAPtor are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/), and the project aims to follow
[Semantic Versioning](https://semver.org/).

## Unreleased

### New

- **New `lookup` widget kind.** A scalar attribute (e.g. `gidNumber`) is shown in
  the form as `<value> (<name>)` and edited via an editable-combobox popup: type a
  number freely or filter a candidate list and pick one. See
  [Widgets → The lookup kind](https://oposs.github.io/edaptor/configuration/widgets.html#the-lookup-kind).

- **Dynamic footer hints:** the bottom status line now shows context-sensitive
  keyboard hints that update as focus moves between fields. Each field kind
  displays its own hint (e.g. `↑↓ move · Enter next field` for text fields,
  reorder instructions for ordered list handles). No configuration required.

- **Inline multi-value editing:** free-text and ordered multi-value fields
  (e.g. `mail`, `olcAccess`) are now edited **in place** in the entry form — no
  modal dialog. Type directly into the bulleted list; **Enter** adds a value,
  **Ctrl+J** adds a wrapped continuation line (portable across terminals, unlike
  Ctrl+Enter which most terminals cannot distinguish from Enter),
  **Backspace/Delete** remove characters and empty items, **Home/End** and
  **←/→** move within an item, and **↑/↓** move between rows and cross to the
  neighbouring field at the top/bottom edge. Ordered fields additionally support
  **Ctrl+↑/↓** reorder and a `≡` handle (reachable with **←** at the start of any
  item). The field's block grows and shrinks
  live as values are added or removed. Built on the new `ListValueView` (a tvision
  `View` wrapping `ListModel`).

- Picker dialogs (object classes, membership, multi-select pickers) now use the
  conventional transfer layout — **Available on the left, the selected set on the
  right** — with wide **Add** and **Remove** buttons under their respective
  columns. The button under the currently-focused list is **highlighted** as the
  dialog's default action; both stay usable at all times (click, Alt-A/Alt-R, or
  Insert/Delete/Enter) and Tab skips them. The dialogs are **resizable** — drag
  the lower-right corner and the columns, buttons, and OK/Cancel reflow — down to
  a minimum size that keeps the two columns usable.

- tvision UI: clicking a form **label** now focuses the corresponding input field
  directly, without needing to navigate to it with the arrow keys.

- **Readable attribute labels:** the entry form now appends a short, human-readable
  hint next to cryptic attribute names — e.g. `sn (surname)`, `l (location)`,
  `ou (org. unit)` — so their meaning is obvious at a glance. Hints come from a small
  curated table of common LDAP abbreviations; descriptive or unlisted attributes
  (`givenName`, `uidNumber`, …) show their bare name.

- tvision UI: the entry list now shows a **`Filter:`** label to the left of the
  incremental-search box, making the filter prompt immediately recognisable.

- The samba domain SID is now discovered live from a `sambaDomain` directory
  entry at startup (when a `sambaSID` widget is configured), falling back to the
  configured `[samba] domain_sid`. When discovery succeeds, the algorithmic RID
  base is also taken from the directory (`sambaAlgorithmicRidBase`, default 1000),
  overriding any `[samba] algorithmic_rid_base` in config.

- **The TUI is now Turbo-Vision–based.** The user interface was migrated from
  ratatui to [tvision-rs](https://github.com/oetiker/tvision-rs): a frameless
  full-screen three-pane browser/editor with modal rich-field editors. The old
  ratatui implementation and the transitional `edaptor-tv` dev binary were
  removed, along with the `ratatui` / `tui-tree-widget` / `tui-prompts` /
  `crossterm` dependencies.

- **tvision UI: config picker at startup.** When more than one config is
  discovered in `~/.config/edaptor/` or `/etc/edaptor/`, a Turbo-Vision
  "Select configuration" dialog now lists them (name + description + path) so
  you can choose one; a single discovered config or an explicit `--config`
  skips the picker. (Startup-flow groundwork for the M5 cutover.)

- **tvision UI:** Save now performs the combined membership write end-to-end. When
  a form's membership (fan-out) field has staged changes, Save plans a combined
  save — the user's own MODIFY plus one MODIFY per touched group — previews the
  full multi-stanza LDIF in the confirm dialog, and on OK submits the multi-entry
  fan-out batch. The user's own `memberOf` is never written directly (it is
  overlay-maintained); only each group's `member` is modified. Forms without a
  membership change keep the existing single-entry Save path. A rename combined
  with a membership change is refused with a clear message (do them as separate
  saves).

- Removing the last member of a required-membership group (`groupOfNames` /
  `groupOfUniqueNames`) is now blocked client-side before saving, with a clear
  message; emptying a `posixGroup` (`memberUid` is optional) is still allowed.

- **tvision UI:** membership editor — a two-column "mover" dialog for fan-out
  `picker` fields (a `picker` with a `fanout` attribute, e.g. a user's group
  memberships). The left **Available** column is a search box over live LDAP
  candidates (groups) that refreshes as you type; the right **Members** column is
  the staged set, seeded from the field's current values. Insert / → moves the
  highlighted Available row into Members (de-duplicated by DN, case-insensitive);
  Delete / ← removes the highlighted Members row; Tab flips which column Up/Down
  navigate; Space types into the search box; Enter confirms the dialog. OK stages
  the chosen member set (the per-group fan-out write is produced by the
  combined-save path); Cancel discards it. The underlying write flow correlates
  the user's own MODIFY plus one MODIFY per touched group as a single batch that
  reports success only once every leg has landed.

- **tvision UI:** picker widget with live LDAP search. A `picker`-bound field
  (e.g. `gidNumber`, `member`, `memberUid`) opens a modal with a search box over
  a candidate list that refreshes as you type (server-side capped at 100).
  Single-select uses radio markers (a pick replaces); multi-select uses
  checkboxes (Insert toggles), with the current selection shown selected-first.
  Space types into the search box (so multi-word substring searches work); Enter
  confirms the dialog. A pick stores the candidate DN (`store = "dn"`) or a chosen
  scalar attribute (e.g. the group's `gidNumber`).

- **tvision UI:** activating a `sambaSID` field auto-generates the SID
  immediately (no popup) from the entry's `uidNumber` and the configured Samba
  domain. A missing `uidNumber` or unconfigured domain SID shows an error dialog
  and leaves the field manually editable.

- **tvision UI:** `choice` widgets (`loginShell`, `sambaAcctFlags`, …) are now
  editable via a modal radio / checkbox list. Single-select (Plain format)
  replaces the selection; multi-select (Bracketed format) toggles tokens while
  preserving any unmanaged letters (lossless merge). The dialog seeds the
  pre-checked state from the field's current value and stages the result live.

- Passwords can be set via a masked New/Confirm editor (create and edit),
  writing the configured attribute (plus the Samba NT hash when enabled); the
  editor refuses on an unencrypted connection.
- Create-form numeric fields with a `{next:MIN-MAX}` default auto-allocate the
  next free value via a background directory scan.
- New entries can be created from a profile in the tvision UI (Alt+N): a profile
  chooser (or single-profile fast path), a create-mode form with
  auto-injected/editable objectClass and live DN, validated and submitted as an
  LDAP ADD.
- **tvision UI:** the `objectClass` field is now editable via a schema-seeded
  multi-select picker (search + tick). Changing the set regenerates the form's
  fields live — newly-allowed attributes appear, now-disallowed ones are marked
  orphaned (dropped on save) — driven by a typed resync outcome.
- tvision UI (preview, `edaptor-tv`): the entry form is now editable for plain
  single-value attributes, with an LDIF-preview save confirmation, async writes
  (MODIFY + rename/MODRDN), and dirty-change guards on navigation and quit.
- tvision UI (preview): keyboard navigation — Tab/Shift-Tab switch panes;
  within a pane the arrow keys navigate (tree branches, leaf list while the
  search box keeps focus, and form fields). `edaptor-tv` also accepts
  `--config <path>` (matching the main binary).
- tvision UI (preview): the main window now runs frameless full-screen — the
  three panes fill the terminal edge-to-edge (no window border or title bar),
  while the menu bar and status line are kept.
- tvision UI (preview): the three panes now fill their area and the entry form
  scrolls — a vertical scrollbar appears when an entry has more attributes than
  fit, and every attribute is reachable (the former 32-row display cap is gone).

### Changed

- **Navigating the tree resets the entries search.** Moving the highlight in the
  branch tree (pane 1) now clears any active incremental search string in the
  entries list (pane 2), so the newly selected branch is always listed unfiltered
  instead of staying narrowed by the previous branch's find query.

- The entry form now lays out each attribute as **one variable-height block**
  instead of a single fixed row. Single-value text fields keep their inline
  editor. Free-text and ordered multi-value fields are edited **in place** as a
  bulleted list (no modal). Object-class, membership, picker, choice, and
  password fields render as a **read-only block** — a bulleted list, `<not set>`
  when empty, or `*****` for secrets — that highlights as a whole when focused;
  pressing any action key on such a block opens its existing modal editor.

- The main browser now splits its width **one-third / two-thirds**: the left
  column (branch tree + members list) takes a third and the entry form fills the
  remaining two-thirds, giving the editor more room.

- Upgraded **tvision-rs 0.10 → 0.11**, which adds a public `InputLine` caret API
  (`home`/`end`/`set_cursor_pos`) and derives the screen cursor from the caret
  offset on every frame. The form's caret-homing (see below) now calls `home()`
  instead of reaching into the field's internals.

- Multi-select `picker` fields (`memberUid`, `member`) now use the two-column
  Shuttle editor (Available | Members) with type-to-find, matching the
  objectClass and membership editors. The single-list checkbox picker is now
  single-select only. Fixed-vocabulary `choice` fields are unchanged. The form-pane summary for these fields now shows a member count (e.g. "‹3 members›") instead of the comma-joined values.

- Upgraded **tvision-rs 0.4 → 0.9** and moved all focus-driven highlighting onto
  the framework. The three browser panes now recede as a unit when unfocused via
  `Group::set_surface` (the pane background) and `ctx.owner_active()` (the list,
  outline, input and label content), and the entry form keeps its single-well
  look via the framework's default three-surface model (0.9): only the focused
  field is the bright well, others sit on the pane surface, an inactive pane
  recedes. The two-column shuttle now shows its **active/passive columns** for
  free — the focused list is bright, its sibling receded (`ListSurface`) — because
  each list's surface keys on its own focus, not the whole group's. This deletes
  edaptor's manual focus code: the leaf pane's list-`active` mirror, the form's
  per-draw label focus push, its `ScrollGroup` focus mirror, and its hand-rolled
  value-cell dimming and pane-background fills. No visual change to the panes; the
  internal focus-aware theme roles use the current names (`ListNormal`/`ListSurface`/
  `ListInactive`, `Outline*`, `Input*`).

- tvision UI: the **entry form pane** now matches the tree and entry panes
  visually. The header reads as a **title** (`dn` label + the DN value in bold);
  field **labels are right-aligned** — a shade lighter than the values — in a
  column sized to the longest label, so short and long names line up against their
  values; the **selected field's label gets the blue current-row highlight** (like
  the tree/leaf panes) while its value is the one bright "type here" well;
  non-selected fields carry **no special background** (plain text on the pane), and
  the whole pane **recedes to the deselected tone when it loses focus** (like the
  tree/leaf panes); the value editors **stretch to fill** the remaining width
  (reflowing when the splitter is dragged); and **`objectClass` is pinned to the
  top** of the form. Entering a field now places the caret at the **start** instead
  of selecting the whole value (which the first keystroke would have wiped).

- tvision UI: incremental search is now the list's **own built-in find** (from
  tvision-rs 0.4) instead of a separate search box. In the **Entries** pane and
  the `objectClass` editor, just type while the list is focused to filter it in
  place and highlight the match (Backspace widens, Esc clears) — the standalone
  `Filter:` box and the Shuttle's search box are gone. A query that matches
  nothing shows a **`No match: <query>`** placeholder (in the Entries pane the
  `‹self›` container row is filtered by the query too, so a non-matching search
  really does empty the list). The membership editor's candidate search still
  queries the directory (so it is not limited to an already-loaded page); typing
  there highlights matches and re-runs the LDAP search. Built on **tvision-rs
  0.4.0** (upgraded from 0.3).

- tvision UI: the main view was **re-laid out** — the directory tree (top-left)
  and the selected branch's entries (bottom-left) now share the left column, with
  the detail/edit form filling the full-height right column. Previously the three
  panes sat side by side. Tab order is unchanged (tree → entries → form).

- tvision UI: the interface now uses a **light Solarized-Light colour scheme** —
  cream/tan panels, dark slate text, blue accent — replacing the previous
  dark classic-blue palette. The theme is applied uniformly and is not
  user-configurable.

- tvision UI: all three panes share a uniform background; the focused pane is
  rendered in a brighter cream tone, unfocused panes are slightly greyed. Within
  the entry form, editable fields have a visibly brighter background than
  read-only labels, making edit affordances immediately apparent. The
  active/inactive contrast was **strengthened**: inactive panes now recede to the
  desktop tone (the previous one-step Solarized difference was imperceptible on
  most terminals).

- tvision UI: the **splitter dividers** between the three panes are now part of the
  light theme (a slate line on the desktop tone; blue while being dragged). They
  previously rendered with the old dark classic-blue defaults.

- tvision UI: the shared two-column editors (`objectClass` and group membership)
  are now a self-contained **Shuttle** view (an embedded `tvision-rs` widget,
  replacing the old `DualList` controller). Each column has **scroll bars** (shown
  while active) and the dialog carries visible **[Add]** / **[Remove]** buttons
  (Alt-A / Alt-R). To move a row: **Insert** (toward Selected) / **Delete** (away),
  the buttons, or **Enter while a list holds focus** (Enter elsewhere — e.g. the
  search box — still triggers the dialog's default **OK**). The previous **→/←
  move keys were removed** — those arrows now do normal focus traversal.

- tvision UI: in the dual-list editors (`objectClass` / membership), **Tab /
  Shift-Tab now move focus between the screen elements** (search box, the two
  lists, the Add/Remove/OK/Cancel buttons) the standard way. Previously Tab was
  consumed to flip which column the arrow keys drove, so it appeared to do
  nothing and the buttons were unreachable by keyboard. The arrow keys now drive
  whichever list holds focus.

- tvision UI: the **mouse wheel** in the entry form now moves the focused field
  (scrolling the form to follow it), so wheel scrolling advances the cursor
  instead of sliding the content out from under a stationary cursor.

- tvision UI: **Tab / Shift-Tab now cycle the three panes only** — they no longer
  descend into a pane's internal fields. Use the arrow keys to move within a pane.

- tvision UI: the tree and entry-list panes, and the entry form, now show a
  vertical scrollbar **only while focused and when content overflows**; the
  scrollbar disappears when the pane loses focus.

- tvision UI: the `objectClass` editor is now a two-column "mover" (the shared
  Shuttle view): the active classes sit on the left, the remaining known classes
  on the right. Highlight a class and press **Insert** (or **Enter**, or **[Add]**)
  to add it, **Delete** (or **[Remove]**) to remove it; Tab moves focus between the
  search box, the two lists and the buttons; the search box filters the available
  column. STRUCTURAL classes that were already on the entry are shown but locked
  (non-removable); a structural class added this session stays removable. The
  committed class set and downstream schema resync are unchanged.

- tvision UI: the group **membership** editor (the Shuttle's Available column) now
  hides candidates that are already members — the staged set is no longer offered
  for re-adding (previously such rows showed with a `✓` marker).

- tvision UI (preview): now builds against the released `tvision-rs` 0.3.0; the
  temporary git-pin for `exec_view_focused` has been removed.

### Fixed

- tvision UI: in an ordered (X-ORDERED) inline list, the `≡` reorder handle is now
  reachable from **every** item, not just the first. Pressing **←** at the start of
  a bullet steps onto that item's handle first; a further **←** then moves to the
  previous line. Previously **←** at the start of any line but the first jumped
  straight to the previous item, so those items' handles could never be reached.

- tvision UI: navigating between entries that have an **X-ORDERED** field (e.g. a
  group's `description` configured as `x_ordered`) no longer pops the spurious
  "unsaved changes" guard when nothing was edited. The inline editor canonicalises
  the `{n}` ordering prefixes on load, so a value stored without them (or with
  non-canonical ones) looked changed the instant the form synced. The dirty check
  now compares the ordered sequence with the `{n}` prefixes stripped — a genuine
  reorder or content edit is still detected, but pure re-serialization is not.

- tvision UI: editing a multi-value field whose inline list is **taller than the
  form pane** (e.g. the `memberUid` of a large group) no longer makes the view
  "ping-pong" — flipping between the list's top and bottom edge on every frame so
  the caret could never be seen. The form now scrolls to keep the **caret row**
  visible while editing an oversized list block, instead of trying to fit the
  whole block (which is impossible) and oscillating.

- tvision UI: the entry form's value editors now line up flush with the read-only
  value blocks (DN, lists, launch fields). Editable text fields render as an
  `InputLine`, which insets its content one column; that column is now reclaimed so
  values like `homeDirectory` / `uidNumber` no longer sit one character to the
  right of everything else.

- tvision UI: the **Discard** button in the unsaved-changes guard dialog is no
  longer cramped. The standard 10-column button was too narrow for its label, so
  the text ran into the drop shadow; the guard's buttons now use a wider face that
  keeps a padding column after every label.

- tvision UI: the object-class and membership picker dialogs can now be closed
  with the frame's **close icon** (top-left `[■]`) again. The embedded transfer
  widget was sized to the full dialog rect, so it sat on top of the frame border
  and swallowed the close/move/resize hot-zones (Esc and the Cancel button were
  unaffected). The widget is now a tight, self-contained control — no built-in
  outer padding, laid out edge-to-edge — that the dialog places in its interior
  (a 1-cell breathing gap inside the frame, above the OK/Cancel row), so the frame
  controls stay hittable.

- tvision UI: moving through the entry form now places the text caret at the
  **beginning** of each field instead of the end. Turbo Vision select-alls a field
  on focus (caret at the end); the form clears that selection and homes the caret,
  and a background repaint (e.g. an autonumber result arriving) no longer bumps the
  focused field's caret to the end.

- tvision UI: the **focused pane is now clearly indicated** in the three-pane
  browser. The pane with keyboard focus shows the bright (base3) surface with a
  blue current-row highlight, while the two unfocused panes recede to the darker
  desktop tone and show their current row in a faded blue. Previously the
  entry-list pane never dimmed and always drew the bright blue cursor (its list
  stayed "active" because the `selected` state does not fan out of the owning
  group), and the tree/form pane backgrounds keyed off `active` — true for every
  pane in the window — so neither dimmed. The list pane now syncs its `active`
  flag to the pane's real focus, and the **tree pane brightens/dims with focus**
  via tvision-rs 0.5's new focus-aware outline surface
  (`OutlineNormal`/`OutlineNormalInactive`).

- tvision UI: the **mouse wheel now scrolls the pane under the pointer** in the
  three-pane main view. Previously the wheel always scrolled one pane (the form)
  regardless of where the cursor was, because tvision delivers wheel events
  non-positionally; each pane now ignores a wheel whose cursor is over a sibling
  so it reaches the pane actually under the pointer.

- tvision UI: the entry form pane's **scroll bar now tracks the cursor (the
  focused field) like the list panes**, instead of the raw scroll offset. The
  thumb follows the highlight as you move through fields, the bar shows a thumb
  the moment the form opens (previously it had none until the first scroll), and
  dragging the thumb now moves the focused field.

- tvision UI: in the entry form pane, **arrow-down on the last field (and the
  mouse wheel at the bottom) no longer wraps focus back to the top** — focus now
  clamps at the first and last field, matching the list panes. Previously moving
  past either end jumped to the opposite end.

- tvision UI: in the two-column editors (`objectClass` / membership), the **mouse
  wheel now scrolls the column under the pointer** instead of always scrolling one
  fixed column. The scroll bar consumed the wheel non-positionally, so whichever
  column's bar the widget offered first ate every wheel event regardless of where
  the cursor was; the Shuttle now routes the wheel to the hovered column's list.

- tvision UI: in the `objectClass` editor, **a structural class added during the
  edit could not be removed again** — every STRUCTURAL class was locked, including
  ones just added. Now only the structural classes that were already on the entry
  at open are locked (marked `*`); a structural class you add this session stays
  removable, so an add can be undone.

- tvision UI: in the dual-list editors (`objectClass` / membership), **Remove (and
  the Delete key / [Remove] button) could silently do nothing**. The list widget
  re-sorts its rows internally (and the lock marker shifts locked rows to the
  end), so the highlighted *display* row no longer lined up with the underlying
  row index — Remove often targeted a locked structural class and was rejected.
  The highlight is now resolved by matching the displayed row, so Remove always
  acts on the class you selected.

- tvision UI: **adding or removing an objectClass crashed the application**
  (`index out of bounds` in the form pane). The form only rebuilt its cells when
  the entry's DN changed, but an objectClass change regenerates the MUST/MAY
  fields on the *same* entry — so the field list grew while the cell vector went
  stale. The form now rebuilds its cells whenever the field set changes size.

- tvision UI: the entry form could **freeze the application** when the focused
  field was scrolled outside the visible area (e.g. via mouse-wheel scrolling) —
  the hardware cursor was driven off the pane. The form now suppresses the cursor
  while the focused field is off-screen, and wheel scrolling keeps the focused
  field (and cursor) on screen, so the freeze can no longer occur.

- tvision UI: the entry form's scrollbar showed whenever the form overflowed, even
  when the form pane was **not** focused — inconsistent with the tree/list panes.
  It now follows the same rule: visible only while the form pane is focused and
  overflowing.

- tvision UI: the unsaved-changes guard dialog (Save / Discard / Stay) was too
  narrow — buttons touched the dialog edge. It is now wider so all three buttons
  have adequate spacing.

- tvision UI (preview): the Save-confirm and unsaved-changes guard dialogs opened
  with the wrong button focused (Cancel / Stay), so pressing Enter cancelled the
  save instead of confirming it — a save could not be completed by keyboard. The
  dialogs now open with the primary action (Save) focused. (Uses tvision-rs's
  `exec_view_focused`, released in 0.3.0.)
- tvision UI (preview): switching to another entry while a form had unsaved
  changes did nothing when driven by the **mouse** — no save prompt, and the form
  got stuck. The unsaved-changes guard now fires consistently for keyboard *and*
  mouse: a dirty form is **pinned** (no other entry is shown) until you choose
  Save / Discard / Stay, and **Stay** snaps the list highlight back to the form on
  screen. When the form is clean it still follows the highlight as you browse.
- tvision UI (preview): cancelling the save-confirm raised by an unsaved-changes
  guard now snaps the list highlight back to the form being edited; and changing
  branch in the tree while the form is dirty now raises the same guard (Stay
  reverts the tree, Discard/Save behave as on a leaf change).

## 0.4.0 - 2026-06-12

### New

- **Space toggles/selects in choice widgets.** In a fixed checkbox/radio list
  (e.g. `loginShell`, `sambaAcctFlags`), pressing Space now toggles a multi-select
  option or radio-selects a single-select option — the same as Enter. Search
  pickers are unchanged: Space there is still a literal search character.
- **`edaptor passwd` accepts a bare username**, not just a full DN. A username
  (any argument without an `=`) is resolved to a DN by searching every configured
  profile's `search_base` for `(<rdn_attr>=<username>)`. The lookup runs **before**
  the password prompt, so an unknown or ambiguous username fails immediately
  (with the matching DNs listed on ambiguity) instead of after typing the
  password twice. The resolved DN is printed before the prompt for confirmation.
- `examples/oposs-openldap.toml` — ready-to-use config template for directories
  managed by the [oposs.openldap](https://github.com/oposs/oposs.openldap)
  Ansible role (POSIX users, groupOfNames + posixGroup, Samba and mailAccount
  optional blocks).
- **`auth.method = "external"`** — SASL EXTERNAL bind over a Unix domain socket
  (`ldapi:///`). Allows password-free root access when running on the slapd
  host; no TLS required. `ldapi://` connections are also treated as secure for
  password-change operations.
- **Config auto-discovery** — edaptor now searches `~/.config/edaptor/*.toml`
  and `/etc/edaptor/*.toml` at startup. A single config is used silently;
  multiple configs trigger a ratatui picker. The `--config` flag bypasses
  discovery as before.
- **`[meta]` table** in config files — optional `name` and `description` fields
  displayed in the startup picker.
- **ObjectClass-driven attributes**: When the `objectClass` field is edited via
  the new schema-seeded picker, the edit form immediately injects fields for all
  MUST and MAY attributes introduced by the new class, and marks attributes no
  longer permitted by any remaining class as _orphaned_ (shown crossed out). All
  changes — objectClass modification, new attribute values, and attribute deletions
  — are sent as a single atomic LDAP `ModifyRequest`.
- **`sambaSID` auto-generate**: an empty `sambaSID` field shows
  `⟨Enter to auto-generate⟩`. Pressing Enter computes the SID from the entry's
  `uidNumber` and the domain context; missing prerequisites (no domain SID,
  empty/non-numeric `uidNumber`) surface a specific error. The field stays
  editable for manual override. The domain SID is discovered at startup from a
  live `sambaDomain` entry, falling back to `[samba].domain_sid` in the config.
- **Next-number allocation on demand**: in a create form, a field whose default
  is `{next:MIN-MAX}` (e.g. `uidNumber`, `gidNumber`) now shows
  `⟨Enter to allocate⟩`. Pressing Enter scans the directory and fills the next
  free number immediately, so dependent widgets (like `sambaSID` auto-generate)
  can use it before save. Skipping it still works — the value is allocated at
  save time as before. The field stays editable for manual override.
### Changed

- Widget configuration is now auto-applied for standard LDAP schemas
  (posixAccount, posixGroup, shadowAccount, sambaSamAccount, groupOfNames,
  groupOfUniqueNames, inetOrgPerson, OpenLDAP cn=config). A typical deployment
  no longer needs `[profile.widget]` entries for these well-known attributes.
- Attributes flagged `NO-USER-MODIFICATION` in the server's subschema are
  automatically rendered read-only, even without explicit widget config.
- `userPassword` is now automatically treated as a password field for entries
  with `person`, `inetOrgPerson`, or `posixAccount` objectClasses.
- New widget kind `readonly`: marks an attribute display-only (excluded from
  the changeset). Available in user config for custom schemas.
- New widget kind `x_ordered`: handles OpenLDAP X-ORDERED attributes
  (`{n}` prefix management). Available in user config for custom schemas.
- `memberOf`, `sambaNTPassword`, `sambaLMPassword` are now read-only by
  default via the built-in schema bundle (previously hardcoded).
- `j`/`k` accepted as aliases for ↓/↑ in the tree pane, profile-chooser overlay,
  and config-discovery picker.

### Fixed

- `auth.method = "external"` (SASL EXTERNAL / ldapi) no longer forces read-only
  mode. Only a `simple` bind with no `bind_dn` is treated as anonymous.

## 0.3.0 - 2026-06-09

### New

- **Configurable widget palette** via `[profile.widget.<attr>]` — bind an
  attribute to a richer editor than a plain text box:
  - `kind = "choice"` — pick from a fixed vocabulary and (de)serialize a single
    value. Shipped for `sambaAcctFlags` (multi-select account flags, losslessly
    preserving flags the UI does not surface) and `loginShell` (single-select
    from a configured shell list). Read-only fields show a human-readable summary;
    Enter opens a checklist/radio popup.
  - `kind = "password"` — turns the attribute into a masked **set-password
    field**. Enter on it (or on any derived Samba password attribute) opens a
    New+Confirm popup that updates `userPassword` and, when `samba = true`,
    `sambaNTPassword` + `sambaPwdLastSet` in one atomic change.
  - `kind = "picker"` — populate an attribute from a live candidate search and
    store the picked value(s) in this entry (value lookup like `gidNumber`, or a
    DN/scalar list like `member`/`memberUid`). `candidate` is a `[[profile]]`
    name or an inline `{ base, object_classes, … }` scope.
  - `kind = "membership"` — fan this entry's DN into a back-reference attribute
    (`via`) on each picked candidate (e.g. `memberOf` writes `member` on each
    chosen group).
### Changed

- **Pickers are now configured with `[profile.widget.<attr>] kind = "picker"` /
  `"membership"`** instead of `[profile.picker.<attr>]`, which has been removed.
- **Passwords are now configured with `[profile.widget.<attr>] kind = "password"`**
  instead of `[profile.password]`, which has been removed.
- **Password and hash attributes (`userPassword`, `sambaNTPassword`,
  `sambaLMPassword`) are read-only inline** and changed only through the
  set-password popup — preventing a typed cleartext password from being written
  verbatim into a hash attribute.
- **Password changes require an encrypted connection** (`ldaps://` or
  `start_tls = true`); the popup refuses to open on a plaintext connection.

### Fixed

- Password-profile entries no longer appear permanently "dirty" (which popped a
  spurious Save/Discard/Stay guard on every navigation between entries).
- Secret values (passwords, NT hashes) are no longer shown in clear in the change
  (LDIF) preview — they are masked as `********` regardless of how they enter the
  change.

## 0.2.0 - 2026-06-08

### New

- Configurable, presence-keyed, width-aware DIT tree labels via `[[tree.label]]`
  rules. The structural RDN is now always shown by default (e.g.
  `ou=people (People)`), and narrow panes degrade gracefully while keeping the RDN.
### Changed

- DIT tree branch labels now always include the RDN by default (previously a
  node with a `description` showed only the description).

### Fixed

## 0.1.0 - 2026-06-04

### New

- Schema-driven TUI for administering an OpenLDAP directory: browse, create,
  edit, rename, and delete users and groups with forms generated from live
  `objectClass` definitions (`cn=subschema`).
- TOML configuration with entry profiles, defaults (literal / template /
  `{next:MIN-MAX}` auto-number), inline passwords, the full Samba lifecycle, and
  unified `[profile.picker.<attr>]` candidate pickers.
- Three-pane ratatui interface with symmetric membership editing and on-demand
  LDIF preview of the exact change before it is applied.
- rustls TLS backend (custom CA, optional StartTLS, connect timeout).
- Provisioned podman test server (`scripts/test-ldap.sh`) and `edaptor passwd <dn>` CLI.
### Changed

### Fixed
