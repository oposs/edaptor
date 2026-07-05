# Widgets

A `[profile.widget.<attr>]` binding gives the field for attribute `<attr>` a
**richer editor than a plain text box**. It is eDAPtor's extensible *widget
palette*: the required `kind` key selects the behaviour, and each kind brings its
own display and editing rules.

Four kinds are available today, and more can be added without changing existing
configuration:

| `kind` | Editor | Use it for |
|---|---|---|
| [`choice`](#the-choice-kind) | a checklist (multi) / radio list (single) over a fixed set of options | enumerated or flag attributes — `loginShell`, `sambaAcctFlags` |
| [`password`](#the-password-kind) | a masked **New + Confirm** set-password popup | password / hash attributes — `userPassword`, with optional Samba sync |
| [`lookup`](#the-lookup-kind) | an editable-combobox popup: type a number or filter a candidate list and pick one | scalar values shown with a friendly name (`gidNumber` → group name) |
| [`picker`](#the-picker-kind) | a live candidate search; stores the picked value(s) in this entry | value lookup (`gidNumber`) and DN/scalar lists (`member`, `memberUid`) |
| [`membership`](#the-membership-kind) | a live candidate search; fans this entry's DN into a back-ref attr on each pick | back-reference views (`memberOf`) |

Additionally, the **`objectClass` field** receives an [auto-injected dual-list editor](#objectclass-editor-auto-injected)
with no configuration required — it is always present when editing any entry.

A widget is declared as a sub-table of an [entry profile](entry-profiles.md),
keyed by the attribute it edits, e.g. `[profile.widget.loginShell]`.

## The `password` kind

The `password` kind turns the named attribute into a **masked, set-password
field**. Pressing Enter on the field (or on any derived password attribute such
as `sambaNTPassword`) opens a popup where the operator types the new password
twice to confirm. The value is sent in cleartext to the directory; the LDIF
preview shows `********` instead.

```toml
[profile.widget.userPassword]
kind  = "password"
samba = false
```

The TOML table key (`userPassword` above) is the **primary cleartext attribute**
written to the directory.

### Options

- **`kind`** *(required)* — must be `"password"`.
- **`samba`** *(optional, default `false`)* — when `true`, eDAPtor also writes:
  - **`sambaNTPassword`** — the NT hash of the password, computed client-side.
  - **`sambaPwdLastSet`** — the timestamp of the change.
  The entry must carry the **`sambaSamAccount`** object class for these attributes
  to be valid; the Samba SID is derived from the directory's `sambaDomain` entry.

### TLS requirement

Password changes require an encrypted connection. eDAPtor refuses to send a
cleartext password over an unencrypted link — configure the server with
`ldaps://` or `start_tls = true`.

### Worked example

```toml
# Samba-enabled password widget: writes userPassword + sambaNTPassword/sambaPwdLastSet.
[profile.widget.userPassword]
kind  = "password"
samba = true
```

Pressing Enter on the `userPassword` field (or on `sambaNTPassword`) opens the
"New password" + "Confirm" popup. On confirmation eDAPtor writes all three
attributes in a single atomic MODIFY.

## The `choice` kind

The `choice` kind presents a fixed vocabulary of options as a checklist
(multi-select) or a radio list (single-select) and stores the selection in one
attribute value.

```toml
[profile.widget.sambaAcctFlags]
kind   = "choice"
select = "multi"
format = "bracketed"
options = [
  { value = "D", label = "Disabled" },
  { value = "X", label = "Password never expires" },
  { value = "N", label = "No password required" },
]
```

### Options

- **`kind`** *(required)* — must be `"choice"`.
- **`select`** *(required)* — cardinality:
  - `"single"` — at most one option may be checked. The stored value is
    replaced by the chosen option (e.g. `loginShell`).
  - `"multi"` — multiple options may be checked simultaneously (e.g. Samba
    account flags).
- **`format`** *(required)* — how options are encoded in the LDAP attribute:
  - `"plain"` — the attribute value **is** the chosen option's `value`. Use
    this for simple string attributes like `loginShell`, where only one value
    is stored at a time.
  - `"bracketed"` — Samba `sambaAcctFlags`-style encoding: a fixed-width
    bracketed string of single-character flags, e.g. `[DUW        ]`. Letters
    not included in the widget's option list are **preserved losslessly** on
    every edit (see [Lossless behaviour](#lossless-behaviour) below).
  - `"bitmask"` / `"delimited"` — reserved for future use; eDAPtor will reject
    a config that specifies these.
- **`options`** *(required, non-empty)* — a list of `{ value, label }` records:
  - `value` — the token stored in the encoded value (a flag letter, a path, …).
  - `label` — the human-facing text shown in the checklist and in the
    read-only summary.

## Worked examples

### `loginShell` — single-select, plain format

```toml
[profile.widget.loginShell]
kind   = "choice"
select = "single"
format = "plain"
options = [
  { value = "/bin/bash",     label = "Bash" },
  { value = "/bin/sh",       label = "POSIX sh" },
  { value = "/sbin/nologin", label = "No login" },
]
```

The stored value is exactly the chosen `value` string. If the current value is
not in the list (e.g. `/usr/bin/zsh`), the raw value is shown as a fallback in
the read-only summary and the checklist starts with nothing pre-selected.

### `sambaAcctFlags` — multi-select, bracketed format

```toml
[profile.widget.sambaAcctFlags]
kind   = "choice"
select = "multi"
format = "bracketed"
options = [
  { value = "D", label = "Disabled" },
  { value = "X", label = "Password never expires" },
  { value = "N", label = "No password required" },
]
```

Samba stores account flags as a fixed-width bracketed string such as
`[UW         ]`. Each letter represents a flag; the `U` (normal account) and
`W` (workstation trust account) flags are maintained by Samba itself and are
**not listed in the widget options** — they will be preserved untouched.

## Lossless behaviour

When you save changes via a choice widget, eDAPtor:

1. **Parses** the current attribute value into the full token set.
2. **Applies** only the options known to the widget: checked options are added,
   unchecked options are removed.
3. **Preserves** any tokens *not listed in the options* exactly as they were.

For bracketed format this means the `U` (normal-user) and other Samba-internal
flags survive an edit even though they do not appear in the checklist. For plain
format there is only one token so lossless preservation is not relevant.

## The `picker` kind

The `picker` kind turns the named attribute into a **live candidate search
field**. Pressing Enter opens an overlay that searches entries from a linked
profile; the operator selects one or more candidates and eDAPtor writes the
right value(s) into this entry's attribute.

A **single-select** picker (`select = "single"`) presents a search box over a
radio list. A **multi-select** picker (`select = "multi"`, the default for
multi-valued attributes such as `member` and `memberUid`) presents a two-column
**Available (left) | Selected (right)** shuttle: type to search, then
Insert/Enter/[Add] to move a candidate in and Delete/[Remove] to move it out.
A wide **Add** button sits under the Available column and a wide **Remove** button
under the Selected column; the one under the focused list is **highlighted**, both
stay usable at all times, and Tab skips them. The dialog is resizable — drag its
lower-right corner and the columns reflow, down to a minimum that keeps both
columns usable.

```toml
[profile.widget.gidNumber]
kind      = "picker"
candidate = "posixgroup"
store     = "gidNumber"
select    = "single"
```

### Options

- **`kind`** *(required)* — must be `"picker"`.
- **`candidate`** *(required)* — the source of candidates. Either:
  - A **`[[profile]]` name string** (e.g. `"posixgroup"`) — the picker searches
    that profile's `search_base` and matches on its `search_attrs`.
  - An **inline scope table** — when you need a candidate set that has no managed
    profile:
    ```toml
    candidate = { base = "ou=people,dc=example,dc=org", object_classes = ["inetOrgPerson"], search_attrs = ["cn", "uid"], label = "{cn} ({uid})" }
    ```
    Keys: `base` (required), `object_classes` (required), `search_attrs`
    (optional, defaults to `["cn"]`), `label` (optional, defaults to `cn`).
- **`store`** *(default `"dn"`)* — what to write per pick:
  - `"dn"` — stores the candidate's full distinguished name.
  - Any other value is treated as a **candidate attribute name** whose scalar
    value is read from each picked entry and written instead (e.g. `"gidNumber"`
    stores the chosen group's numeric GID, not its DN).
- **`select`** *(default `"auto"`)* — cardinality override:
  - `"auto"` — derives single vs. multi from the attribute's schema arity.
  - `"single"` — at most one candidate may be picked.
  - `"multi"` — multiple candidates may be picked.

### Worked examples

#### `gidNumber` — single-select, stores a scalar

```toml
[profile.widget.gidNumber]
kind      = "picker"
candidate = "posixgroup"
store     = "gidNumber"
select    = "single"
```

A single-select picker over `posixgroup` entries. Because `store = "gidNumber"`,
eDAPtor writes the **chosen group's `gidNumber` scalar** into the user's
`gidNumber` field — not the group's DN.

> For `gidNumber` you will usually prefer the [`lookup` kind](#the-lookup-kind),
> which additionally shows the group name in the form and lets you type a bare
> number. The `picker` form above remains valid for pick-only behavior.

#### `member` — multi-select, stores DNs

```toml
[profile.widget.member]
kind      = "picker"
candidate = "user"
store     = "dn"
select    = "multi"
```

A multi-select picker over `user` entries. Each picked user's **DN** is written
into the group's `member` attribute. `store = "dn"` is the default, so the
`store` key may be omitted; `select = "multi"` is also the schema default for
`member`, so `select` may be omitted too.

#### `secretary` — inline scope, single-select

```toml
[profile.widget.secretary]
kind      = "picker"
store     = "dn"
select    = "single"
candidate = { base = "ou=people,dc=example,dc=org", object_classes = ["inetOrgPerson"], search_attrs = ["cn", "uid"], label = "{cn} ({uid})" }
```

Uses an inline candidate scope rather than a named profile. Useful when you need
a picker over a subset of the directory that does not have (or need) a full
`[[profile]]` entry of its own.

## The `lookup` kind

The `lookup` kind turns a **scalar attribute into a value shown with its friendly
name**. In the form the field renders as `<value> (<name>)` — e.g. `5000 (staff)`
for a `gidNumber` — by resolving the value against a candidate profile. Pressing
Enter opens an **editable combobox**: type a number freely, or filter a list of
candidates by name and pick one.

```toml
[profile.widget.gidNumber]
kind      = "lookup"
candidate = "posixgroup"
store     = "gidNumber"
label     = "{cn}"
```

### Options

- **`kind`** *(required)* — must be `"lookup"`.
- **`candidate`** *(required)* — the source of candidates. Same as
  [`picker`](#the-picker-kind): a `[[profile]]` name string, or an inline scope
  table.
- **`store`** *(required)* — the scalar attribute stored in this entry's field. It
  is **also the match key**: the friendly name is resolved by searching the
  candidate for an entry whose `store` attribute equals the current value.
- **`label`** *(optional, default `{cn}`)* — a label template rendered against the
  resolved candidate to produce the friendly name (e.g. `{cn}`, `{cn} ({description})`).
  Defaults to the candidate profile's own `label`, else `{cn}`.

### The edit popup

The popup is an **editable combobox**:

- The **input** is authoritative. Its leading integer is the value that will be
  stored — you can type any number, even one with no matching candidate.
- Typing filters the **list** below (by name, or by numeric prefix). Rows show
  `<name> (<value>)`, e.g. `staff (5000)`.
- Picking a row (Enter/click) fills the input with `<value> (<name>)`.
- **OK** is enabled only when the input has a leading number; it stores that
  number.

The always-visible form shows `<value> (<name>)` — a brief `<value> (…)` while the
name resolves in the background, and just `<value>` if no candidate matches.

### `lookup` vs `picker`

Both can store a group's `gidNumber`. Use `picker` when you only ever pick an
existing group and want a search-over-radio-list. Use `lookup` when you also want
to **type an arbitrary number** and to **see the group name in the form** without
opening the editor.

## The `membership` kind

The `membership` kind is a **fan-out picker**: when the operator selects
candidates, eDAPtor does **not** write this attribute on the current entry.
Instead it adds (or removes) this entry's DN in the `via` attribute on each
**picked candidate**. This is the right model for overlay-maintained
back-references such as `memberOf`, where the attribute is kept in sync by
OpenLDAP's `memberof` overlay and must **never be written directly**.

```toml
[profile.widget.memberOf]
kind      = "membership"
candidate = "group"
via       = "member"
```

The overlay sees the `member` change on the group and updates `memberOf` on the
user automatically.

### Options

- **`kind`** *(required)* — must be `"membership"`.
- **`candidate`** *(required)* — the source of candidates. Same as for
  [`picker`](#the-picker-kind): a `[[profile]]` name string or an inline scope
  table.
- **`via`** *(required)* — the attribute on each picked **candidate** that
  receives this entry's DN. This attribute is always treated as multi-valued;
  eDAPtor adds or removes exactly one DN value per toggled candidate.

There is no `store` or `select` key for `membership` — storage is always DN,
and the cardinality is always multi (the overlay collects one entry per group
membership).

### Worked example

#### `memberOf` — fan-out into `member`

```toml
[profile.widget.memberOf]
kind      = "membership"
candidate = "group"
via       = "member"
```

Pressing Enter on the `memberOf` field opens a picker over `group` entries.
Ticking a group does **not** write `memberOf` on the current user; instead
eDAPtor adds (or removes) the user's DN in that group's `member` attribute.
OpenLDAP's `memberof` overlay then keeps the user's `memberOf` values in sync
automatically.

The membership *workflow* — editing from either side, incremental search, the
fan-out write model — is described in [Membership Editing](../usage/membership.md).

### `readonly`

Marks the attribute as display-only. It is rendered in the form but excluded from
the save changeset — the user cannot edit it. Use this for overlay-maintained
back-references or any attribute your schema generates automatically.

```toml
[profile.widget.myOverlayAttr]
kind = "readonly"
```

Built-in assignments: `memberOf` (all standard object classes), `sambaNTPassword`,
`sambaLMPassword`. Additionally, any attribute the server marks
`NO-USER-MODIFICATION` in the subschema is treated as readonly automatically.

### `x_ordered`

For OpenLDAP **X-ORDERED** multi-value attributes (e.g. `olcAccess`,
`olcDbIndex`). These are edited **in place** in the entry form as a bulleted
list, with full reorder support (see [Inline multi-value editing](#inline-multi-value-editing)
below). The `{n}` ordering prefix is hidden during editing and regenerated from
row order when the entry is saved. Changing the set of values or their order
produces a single `REPLACE` operation.

```toml
[profile.widget.myOrderedAttr]
kind = "x_ordered"
```

Built-in assignments: `olcAccess`, `olcDbIndex`, `olcSuffix`, `olcRootDN`,
`olcLimits`, `olcSyncrepl` (all under the `olcGlobal` / `olcDatabaseConfig`
object classes).

## Inline multi-value editing

Any multi-value attribute that accepts free-text or ordered values is edited
**in place** in the entry form — no modal dialog. The field renders as a
bulleted list (`- value`) that grows and shrinks live as you add or remove
items. An empty field shows `<not set>`. No configuration is needed for this
behaviour.

### Key bindings

| Key | Action |
|-----|--------|
| **Printable char** | Insert character at cursor. On an empty field the first keystroke creates the first item. |
| **Enter** | Split the current item at the cursor — text after the cursor becomes a new item below; at end of item adds a new empty item. |
| **Ctrl+J** | Insert a continuation line (`\n`) within the current item. (**Ctrl+Enter** does the same where the terminal can distinguish it, but most cannot — see note below — so **Ctrl+J** is the portable binding.) |
| **Backspace** | Delete the character before the cursor. At offset 0 of an item, merge this item into the previous one; backspace once more on an empty item to remove its `- ` marker entirely. At the start of the first item, does nothing. |
| **Delete** | Delete the character after the cursor. At the end of an item, pull the next item up (merge). |
| **←** / **→** | Move cursor left / right within the item. |
| **Home** / **End** | Move cursor to the start / end of the item. |
| **↑** / **↓** | Move cursor to the item above / below. Crossing the top or bottom edge of the field moves focus to the neighbouring field. |

> **Why Ctrl+J and not Ctrl+Enter?** Terminals only tell an application that
> *Ctrl* was held together with *Enter* when the modern Kitty keyboard protocol
> is active, which edaptor does not negotiate; on ordinary terminals Ctrl+Enter is
> indistinguishable from a plain Enter. Ctrl+J, by contrast, is delivered as a
> literal line-feed that is always distinct from Enter, so it works everywhere.
> (Ctrl+M is *not* an alternative — it is a carriage return, i.e. Enter itself.)

### Reordering (ordered / X-ORDERED fields)

`x_ordered` attributes (e.g. `olcAccess`) additionally support two reorder
mechanics that plain multi-value fields do not:

| Key | Action |
|-----|--------|
| **Ctrl+↑** / **Ctrl+↓** | Move the current item up / down regardless of cursor position. |
| **←** at offset 0 | Move the cursor onto **that item's** `≡` handle — works on every item, not just the first. While on the handle, plain **↑** / **↓** move the item up / down; **→** (or any printable key) returns the cursor to the text, and **←** again steps to the end of the previous item. |

The `{n}` ordering prefix is hidden during editing and regenerated from row
order on save.

### Commit and empty state

Empty rows are dropped automatically when focus leaves the field or the entry
is saved. Removing the last item reverts the field to `<not set>`. The result
feeds directly into `EditField.values` — the same changeset/diff path as
single-value fields; no new write path is introduced.

### Footer hints

The bottom status line shows context-sensitive hints for the focused field:

| Focused field state | Hint |
|---------------------|------|
| `ListValueView`, has items | `Enter add · Ctrl-J newline · Backspace empties→removes · ↑↓ move` |
| `ListValueView`, ordered (has items) | appends `· Ctrl-↑↓ or ← handle to reorder` |
| `ListValueView`, empty | `Type to add first value` |
| `ListValueView`, on `≡` handle | `↑↓ reorder · → back to text` |

## `objectClass` Editor (auto-injected)

The `objectClass` field automatically receives a schema-seeded **two-column
editor** (the shared *Shuttle* view). No configuration is needed — it is always
present when editing any entry. Pressing Enter on the `objectClass` field opens a
two-column picker:

- **Left column (Available)** — the objectClasses known from the server's
  subschema. Type while this list is focused to filter it in place (incremental
  find).
- **Right column (Active)** — the entry's *active* objectClasses (currently
  applied).

A wide **Add** button sits under the Available (left) column and a wide **Remove**
button under the Active (right) column. The button under the focused list is
**highlighted**; both stay usable at all times, and Tab skips both. The dialog is
resizable: drag its lower-right corner and the columns, buttons, and OK/Cancel
reflow, down to a minimum that keeps both columns usable.

| Action | Keys |
|--------|------|
| Move focus between the two lists and OK/Cancel | Tab / Shift-Tab |
| Add (highlighted available → active) | Insert, the **[Add]** button (Alt+A), or Enter on the available list |
| Remove (highlighted active → available) | Delete, the **[Remove]** button (Alt+R), or Enter on the active list |
| Filter the available column | type while the available list is focused (Backspace widens, Esc clears) |
| Commit | the **OK** button |

**STRUCTURAL** classes that were already on the entry are shown in the active
column but are **locked** (marked `*`) and cannot be removed; a structural class
added during this edit can still be removed again.

After committing, the edit form immediately reflects the schema change:

- **New attributes appear** for all MUST and MAY attributes introduced by the
  newly added classes.
- **Existing attributes are orphaned** (shown **crossed out** and dimmed) if they
  are no longer permitted by any of the remaining classes. Orphaned attributes
  are automatically deleted when the entry is saved.

All changes — objectClass modifications, new attribute values, and orphaned
attribute deletions — are sent as a single atomic LDAP `ModifyRequest`.

### Reverting objectClass changes

To abandon objectClass changes without saving, press **Alt+C** in the edit
form — the popup closes, all injected fields disappear, and the form returns to
the server's current state.

## `sambaSID` auto-generate (auto-injected)

When a Samba domain context is available, the `sambaSID` field gains a one-key
auto-generate action. This is most useful right after adding the
`sambaSamAccount` objectClass, which makes `sambaSID` a new mandatory field.

The domain SID is resolved at startup: edaptor first looks for a live
`sambaDomain` entry in the directory (reading its `sambaSID` and
`sambaAlgorithmicRidBase`); if none is found, it falls back to the `[samba]`
table's `domain_sid` / `algorithmic_rid_base` keys. When neither is available the
feature is disabled.

While the field is empty it shows the affordance **`⟨Enter to auto-generate⟩`**.
Pressing **Enter** computes the value from the entry's `uidNumber` and the domain
context:

```
sambaSID = {domain_sid}-{uidNumber * 2 + algorithmic_rid_base}
```

If a prerequisite is missing, an error overlay explains what to fix:

- no `domain_sid` configured → add it to the `[samba]` table;
- `uidNumber` still empty → fill it in first (for a new entry, set or auto-number
  `uidNumber`, then return to `sambaSID`);
- `uidNumber` not numeric.

The field remains a plain editor: you can also type a `sambaSID` by hand to
override the generated value. The `[samba]` configuration keys (`domain_sid`,
`algorithmic_rid_base`) are documented in the example configs.
