# Migrate multi-select pickers to the Shuttle

Date: 2026-07-02
Branch: `feat/shuttle-widget`
Status: approved design, ready for implementation plan

## Problem

`memberUid` (and `member`) open the pre-Shuttle multi-select picker
(`PickerDialog` in `src/ui/picker.rs`): a single `ListBox` with inline
checkboxes (`[x]/[ ]/[-]`) and a search box on top. Meanwhile the objectClass
picker (`oc_picker.rs`) and the fan-out membership editor (`membership.rs`)
already use the two-column `Shuttle` transfer widget (`src/ui/shuttle.rs`). The
multi-select `PickerDialog` is the **last picker not backed by the Shuttle**.

The goal is one consistent two-column multi-select UX and one code path,
eliminating the divergent checkbox-list dialog.

## Key insight

`MembershipDialog` (`membership.rs`) is *already* a generic Shuttle multi-select
picker:

- it handles both a **DN store** and a **scalar store** (`store_attr:
  Option<String>`, resolved from `binding.store`) — exactly what `memberUid`
  (scalar `uid`) and `member` (DN) need;
- it is server-backed (async `submit_search` + pump `REFRESH` → `sync_results`),
  filters already-selected rows out of Available, seeds Selected from the field's
  values, and stages `CommitOutcome::SetValues`.

The **only** fanout-specific behaviour — expanding the change into one MODIFY per
picked candidate — happens at *save time* in the combined-save path, keyed off
`binding.fanout_attr`, **not** in the dialog. So fanout vs non-fanout is
orthogonal to the dialog.

That reframes the migration: the natural routing key is **cardinality**
(Shuttle is inherently two-column/multi), not fanout.

## Decisions

- **Scope:** all multi-select pickers (`memberUid` + `member`), not just
  `memberUid`. Single-select pickers (e.g. `gidNumber`) stay on the radio list.
- **Approach:** generalize — one Shuttle dialog serves every multi-select
  picker (fanout and non-fanout). No duplicated Shuttle plumbing.
- **Naming:** rename `membership.rs` → `multi_picker.rs`;
  `MembershipWidget/Editor/Dialog` → `MultiPickerWidget/Editor/Dialog`.

## Design

### Module split

- **`src/ui/multi_picker.rs`** (renamed from `membership.rs`): the Shuttle
  two-column dialog, now the editor for **all** multi-select `WidgetKind::Picker`
  bindings — fanout (`memberOf`) and non-fanout (`member`, `memberUid`). No
  behavioural change to the existing fanout path; it already handles DN and
  scalar stores and stages `SetValues`.
- **`src/ui/picker.rs`**: reduced to **single-select only** (radio list). Drop
  the `Cardinality::Multi` code paths: the `[x]/[ ]/[-]` checkbox markers, the
  multi toggle, and their tests. `PickerDialog` loses its `cardinality` field;
  `pick_at` always replaces the selection; `marker` becomes radio-only.

### Dispatch (`src/ui/widget.rs`)

Route `Some(WidgetKind::Picker(b))` by **resolved cardinality** instead of by
fanout:

- fanout (`b.fanout_attr.is_some()`) **or** resolved-multi → `MultiPickerWidget`
- single, non-fanout → `PickerWidget`

Resolved cardinality = `b.select.unwrap_or(if field.multi { Multi } else {
Single })`. Extract this rule (currently inline in `PickerEditor::into_view`)
into one shared helper — a `PickerBinding::cardinality(field_multi: bool) ->
Cardinality` method in `src/config/relation.rs` — so `widget_for` and the editors
cannot disagree.

`is_modal_field` returns true for **every** picker (single and multi are both
modal), so its two Picker arms collapse into one — no cardinality logic needed
there.

### Data flow (unchanged from membership)

form-pane activate → `MultiPickerEditor::into_view` → `MultiPickerDialog`
embeds a `Shuttle` → `reset_current` seeds Selected from `field.values`, submits
an empty candidate search → pump `REFRESH` → `sync_results` fills Available
(minus already-selected) → user moves (Insert/Delete/Enter/Add/Remove) →
`CMD_SHUTTLE_CHANGED` → `update_staged` writes `SetValues(store_values)` → OK
applies `staged_commit`; the save path expands the fan-out if
`binding.fanout_attr` is set (unchanged).

## User-visible behaviour change

`memberUid` and `member` change from the checkbox list to the two-column
**Available | Members** Shuttle: type-to-find on Available (server-backed
re-query), Add/Remove/Insert/Delete/Enter moves, already-selected rows filtered
out of Available — identical to the objectClass and membership editors. The
`[-]` "saved-but-removed" intermediate marker is **dropped** (a removed row
simply returns to Available). The staged commit is unchanged (`SetValues`), so
save and fan-out expansion are unaffected.

## Testing

- **`multi_picker.rs`**: keep the renamed membership tests (fanout path
  unchanged). **Add** a non-fanout multi case — `memberUid` with a scalar `uid`
  store — proving it routes here, seeds the Selected set, moves a candidate, and
  stages `SetValues([uid])`.
- **`picker.rs`**: delete the multi-select tests
  (`multi_toggle_stages_selected_store_value`,
  `space_does_not_toggle_candidate`, the multi bits of `present`/marker); keep
  and adjust the single-select tests (`single_pick_replaces_selection`,
  seeding).
- **`widget.rs`**: extend the routing test — a multi non-fanout Picker field
  routes to `MultiPickerWidget`; a single non-fanout Picker field routes to
  `PickerWidget`.
- **Integration:** unlike `memberOf` (operational, overlay-maintained, not a
  reachable demo form field), `memberUid` **is** a reachable field on
  `posixGroup` in the demo config. Add a `tv_member_uid` integration test that
  drives the real Shuttle live end-to-end (seed, search, move, save), mirroring
  `tv_membership`.
- `make check` (fmt + clippy -D warnings + tests) green before done.

## Docs / changelog

- **`CHANGES.md`**: entry under the unreleased section — multi-select pickers
  (`memberUid`, `member`) now use the two-column Shuttle editor
  (Available | Members) with type-to-find, matching the objectClass and
  membership editors; the single-list checkbox picker is now single-select only.
- **`docs/src/configuration/widgets.md`**: in the `picker` kind section, note
  that multi-select pickers present the two-column shuttle (Available | Selected)
  while single-select uses a radio list.
- **README**: unchanged (stays an overview).

## Out of scope / non-goals

- Single-select pickers keep the radio list — no Shuttle single-select
  affordance.
- No change to fan-out save semantics or the combined-save path.
- No change to the `x_ordered` (`OrderedDialog`) free-text editor.
