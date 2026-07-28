# Entry meta block (operational timestamps) + title truncation marker

**Date:** 2026-07-28
**Status:** design, approved in conversation
**Scope:** panel 3 (the entry form pane)

## Goal

Panel 3 shows the schema-driven form for the selected entry. Two changes:

1. After the form, separated by one blank row, show the entry's server-maintained
   audit values read-only: **createTimestamp, creatorsName, modifyTimestamp,
   modifiersName**. Timestamps render as human-readable local time.
2. When the `dn` title on the panel's `═══` rule does not fit, append `…` so the
   truncation is visible.

## Non-goals

- No configuration knob. The block is always shown when the server returns the
  attributes; profiles do not opt in or out.
- No other operational attributes (`entryUUID`, `pwdChangedTime`,
  `structuralObjectClass`, …). If those turn out to be wanted, they are a later,
  separate decision.
- The values are display-only: never editable, never written, never diffed.

## Background: why these attributes are missing today

`ReadFlow::request_entry` (`src/workflows/read_flow.rs:78`) asks for
`["*", "entryCSN"]`. In LDAP, `*` means *all user attributes* — operational
attributes are returned only when named explicitly, which is why `entryCSN` is
already listed by name (the optimistic-concurrency work learned this the hard
way). So the four audit attributes must be added to that list.

The save path re-reads the entry after a successful write
(`UiState::reread`, `src/ui/state.rs:884`), so the block refreshes itself after a
save with no extra plumbing.

## Design

### 1. Fetch

`request_entry` requests:

```
["*", "entryCSN", "createTimestamp", "creatorsName", "modifyTimestamp", "modifiersName"]
```

Servers that do not return some of them simply yield fewer rows.

### 2. Model: a `meta` flag on the field

`FormField` (`src/workflows/form_model.rs`) and `EditField`
(`src/workflows/edit_form.rs`) each gain `meta: bool`, defaulting to `false`.

`build_form_model` appends, **after** all schema-driven fields, one meta field per
audit attribute that the entry actually carries, in the fixed order
`createTimestamp, creatorsName, modifyTimestamp, modifiersName`. Timestamp values
are formatted at this point (see §4); DN values pass through verbatim.

`build_edit_form` carries the flag through and forces `editable = false` for meta
fields.

**Why in `fields` and not a parallel list.** `FormPane` keeps index-parallel
`label_ids` / `value_ids` / `kinds` / `labels` / `block_heights` vectors, all
indexed by field position (`focused_field_idx`, `layout_blocks`, `render`). A
separate `meta` list would need a second, desyncable rendering path; a flag on the
existing field keeps one path.

**The cost of that choice** is that every place which treats "a field" as "an
attribute we own and may write" must skip meta fields. All of them:

| Site | Why | Action |
|---|---|---|
| `EditForm::to_edit_entry` (`edit_form.rs:138`) | feeds `validate()`, which would reject operational attrs as not-allowed-by-objectClass and **break saving** | filter `!f.meta` |
| `save.rs:296` `original` | baseline side of the diff must match | filter `!f.meta` |
| `write_flow.rs:263` `original` | same, single-entry path | filter `!f.meta` |
| `EditForm::sync_schema_fields` (`edit_form.rs:151`) | flags fields outside MUST∪MAY as `orphaned`, and orphaned fields **emit a Delete** | never orphan meta fields |
| `order_fields` (`edit_form.rs`) | reorders by profile `show` / MUST / MAY | meta fields stay last, after the orphan bucket |
| `is_dirty` / `dirty_labels` | `values == baseline` already makes them clean, but relying on that is fragile | filter `!f.meta` |

### 3. Render

- `cell_focusable` returns `false` for meta fields → the value renders as a
  disabled `InputLine`, takes no Tab stop, and cannot be edited. This is the same
  treatment `creatorsName` already gets in the form's read-only tests.
- `layout_blocks` (`form.rs:419`) bumps `y` by one extra row before the first meta
  field. That single row is the blank separator.
- Labels use the existing curated-hint mechanism (`ATTR_HINTS`, `form.rs:60`),
  extended with:

  ```
  ("createtimestamp", "created"),   ("creatorsname",  "created by"),
  ("modifytimestamp", "modified"),  ("modifiersname", "modified by"),
  ```

  so the rows read `createTimestamp (created)` etc. These are long labels and will
  widen the label column via `label_col_width`; that is accepted.

Resulting shape:

```
                 mail (email)  - jsmith2@example.org

     createTimestamp (created)  2026-06-14 09:12:44
   creatorsName (created by)    cn=admin,dc=example,dc=org
   modifyTimestamp (modified)   2026-07-28 13:03:22
  modifiersName (modified by)   cn=admin,dc=example,dc=org
```

### 4. Time formatting

New pure module `src/workflows/gtime.rs`:

```rust
pub fn format_generalized_time(raw: &str) -> String
```

- Parses LDAP GeneralizedTime: `YYYYMMDDHHMMSS[.fff]Z` and the
  `YYYYMMDDHHMMSS[.fff]±hhmm` form. Fractional seconds are dropped.
- Renders `YYYY-MM-DD HH:MM:SS` in **local** time.
- Falls back to `YYYY-MM-DD HH:MM:SS UTC` when the local offset is unknown.
- Anything unparsable is returned verbatim — a weird value is shown, never hidden.

**Local offset.** The `time` crate is already in `Cargo.lock` transitively; it
becomes a direct dependency with the `local-offset` feature. `time` refuses to
read the local offset once the process is multi-threaded, so `main()` captures it
**once, before the LDAP worker thread is spawned**, into a `OnceLock<UtcOffset>`
that `gtime` reads. If the capture fails, the UTC fallback applies.

### 5. Title truncation marker

In `FieldLabel::draw` (`src/ui/panes/field_label.rs`), `LabelKind::Title`: when the
text is wider than the space available in the cell, cut it to fit and append `…`
(width-aware, via `unicode-width`, so a wide glyph is never split). Applies to both
the rule and plain title paths.

Verified against the live app before designing: the title already truncates at the
*end* (`uid=jsmith2,ou=people,dc=example,` — `dc=org` dropped), so only the marker
is missing. The `…` seen in Create mode is `composed_create_dn`'s placeholder for
the not-yet-typed RDN value (`edit_form.rs:349`), unrelated to truncation.

`LabelKind::Label` is out of scope — long attribute labels keep today's behaviour.

## Testing

Unit (headless, the repo's normal level):

- `gtime`: `Z` input, `±hhmm` input, fractional seconds, garbage passthrough,
  UTC fallback.
- `build_form_model`: meta fields appended last, only for attributes the entry
  carries, in the fixed order; absent attributes produce no row.
- `to_edit_entry` / `save` diff: an entry carrying the audit attributes produces
  **no** modifications and passes `validate()`.
- `sync_schema_fields`: an objectClass change does not mark meta fields orphaned.
- `FormPane`: one blank row between the last schema field and the first meta row;
  meta value cells are disabled (no Tab stop).
- `FieldLabel`: a too-long title renders with a trailing `…`; a fitting one does
  not.

Live check against the podman demo server (`scripts/test-ldap.sh start`) via tmux,
confirming the block appears with real server values and that saving an edited
entry still works.

## Documentation

- `CHANGES.md` — entry under the unreleased section.
- `docs/src/usage/three-pane.md` — the panel-3 section gains a short paragraph on
  the meta block (which attributes, that they are read-only, local-time
  rendering).
- `README.md` — no change; it does not describe panel internals.
