# Design: the `lookup` widget — scalar value with a friendly-name popup

**Date:** 2026-07-05
**Status:** approved (brainstorming), pending implementation plan
**Worked example:** `gidNumber` (a POSIX group's numeric GID, shown with its group name)

## Problem

`gidNumber` currently uses `kind = "picker"` with `store = "gidNumber"`: a search
box over a radio list that commits the chosen group's numeric GID. Two things are
missing for this attribute:

1. **The form shows only the raw number.** An operator looking at `5000` cannot
   tell which group that is without opening the editor.
2. **You cannot type a bare number.** The picker only lets you commit values that
   came back from a candidate search; there is no path to enter an arbitrary GID
   that does not (yet) correspond to a managed group.

We want a field that behaves like a classic editable combobox: type a number
freely, *or* filter a list of groups by name and pick one — and always display the
number together with its resolved group name.

## Non-goals

- Not a replacement for `picker`. `picker` (DN lists, `member`, `secretary`,
  multi-select shuttles) stays exactly as it is.
- Single-value scalars only. No multi-value / fan-out / DN-store variants.
- No "clear to empty" affordance beyond leaving the input empty (a MUST field
  cannot be emptied anyway; see Validity).

## Decisions (from brainstorming)

- **New general widget kind** — `WidgetKind::Lookup`, usable for any scalar
  attribute that resolves to a friendly name; `gidNumber` is the documented
  example. (Not a `gidNumber` special-case, not an extension of `picker`.)
- **Live name resolution in the form** — the always-visible form row shows
  `<value> (<name>)`, resolving the name via a background directory lookup.
- **The input is the source of truth** inside the popup (value-in-input model).
- **Kind name:** `lookup`. **List-row format:** `<name> (<value>)`, e.g.
  `staff (5000)`.

## Config surface

```toml
[profile.widget.gidNumber]
kind      = "lookup"
candidate = "posixgroup"      # profile name OR inline scope table (same as picker)
store     = "gidNumber"       # attr written into THIS entry, AND the candidate
                              #   attr matched on for reverse name-resolution
label     = "{cn}"            # label template for the resolved candidate's name
```

- **`kind`** *(required)* — `"lookup"`.
- **`candidate`** *(required)* — reuses the picker candidate machinery verbatim: a
  `[[profile]]` name string, or an inline scope table
  `{ base, object_classes, search_attrs, label }`.
- **`store`** *(required)* — the scalar written into this entry's field. It also
  doubles as the **match key**: to resolve a name, edaptor searches the candidate
  for an entry whose `store` attribute equals the current value.
- **`label`** *(optional, default `"{cn}"`)* — a label template
  (`crate::config::label`) rendered against the resolved candidate entry to produce
  the friendly name (e.g. `{cn}`, `{cn} ({description})`).

Config errors: a `lookup` binding on a multi-valued attribute, or a missing
`candidate` / `store`, is rejected at config-load time with a clear message.

## Form-pane display

A `lookup` field classifies as a `Launch` field (`ValueKind::Launch`) — it opens
the popup on an action key and renders whatever the widget's `present()` returns.
It renders:

| State | Row text |
|-------|----------|
| Value present, name resolved | `5000 (staff)` |
| Value present, lookup in flight | `5000 (…)` |
| Value present, no matching candidate | `5000` |
| Empty | `‹none›` |

### Resolution mechanism

Mirrors the existing async autonumber flow (async search → `form_needs_render` →
repaint):

1. When a `lookup` field with a non-empty value is rendered and its name is not yet
   cached, edaptor submits a reverse `SearchFlow` on the worker: scope = the
   `candidate` scope, filter = `store == value`, requested attrs = the `label`
   template's fields.
2. The result is cached in `UiState` keyed by `(candidate-scope-key, value)`:
   - resolved → the rendered label string,
   - not found → a sentinel "unresolved" marker (so we do not re-search forever).
3. Setting the cache flips `form_needs_render`; the form repaints and reads the
   cache.
4. **`present()` stays pure.** The resolved name is carried on the `EditField` (a
   new transient `resolved_label: Option<ResolvedName>` populated from the cache
   during form render), so the `LookupWidget::present(&EditField)` implementation
   renders `<value> (resolved_label)` without any directory access.

The cache persists for the session so re-opening the same entry (or showing many
entries sharing a GID) does not re-query. In-flight de-duplication: a value already
being searched is not submitted again.

## The edit popup

```
┌─ Select gidNumber ─────────────────────────────────────┐
│                                                          │
│  [5000 (staff)________________]   [ ~O~K ]  [ ~C~ancel ] │
│                                                          │
│  ┌────────────────────────────────────────────────────┐ │
│  │ staff (5000)                                       ▲ │ │
│  │ users (100)                                        █ │ │
│  │ wheel (10)                                         ▼ │ │
│  └────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────┘
```

- **Input row:** the editable `InputLine` on the left; **OK** and **Cancel** on the
  same row to its right (not anchored at the dialog bottom).
- **List:** a `ListBox` below, spanning the full inner width — from the input's left
  edge to the buttons' right edge.
- **Focus** starts in the input; arrow keys forward to the list (the search-over-
  list idiom used by `PickerDialog` / `LeafPane`).
- **Candidates load** via the same async `SearchFlow` the picker uses: an empty
  query returns all candidates (capped); typing narrows. Rows render as
  `<name> (<value>)` = `label` template + `(store-value)`.

### Interaction — value-in-input model

The input `InputLine` text is authoritative. Define `parse(input)`:

- Take the **leading run of ASCII digits**. If non-empty → `pending_value =
  that integer` (free numeric entry, valid even with no matching group).
- The remaining/whole text is the **filter string** applied to the list
  (matches `label` text case-insensitively, and matches `store` value as a
  numeric prefix when the text is digits).

Behavior:

- **On each keystroke:** re-filter the list from the input text. If the text is a
  number that exactly matches a candidate's `store` value, **highlight that row**.
- **Picking a row** (Enter on the list / click): set the input text to
  `<value> (<name>)` and set `pending_value` to that row's `store` value. A
  selection made this way is not re-filtered from the auto-filled text until the
  user types again (selection supersedes filtering).
- **Validity — OK enabled ⇔** the input has a leading integer **or** a row is
  currently selected. Empty input with no selection, or pure non-numeric text with
  no selection ⇒ OK disabled (grayed).
- **OK commits** `pending_value` as a single `SetValues([pending_value])` into the
  field — the same commit path `PickerDialog` uses. **Cancel** discards.

`pending_value` is always the integer GID; the `(name)` in the input is display
sugar parsed off, never committed.

## Components / files

- `src/config/widget.rs` — add `WidgetKind::Lookup(LookupBinding)`; parse a new
  `WidgetSpecCfg::Lookup { candidate, store, label }`; validation (single-value,
  required keys).
- `src/config/relation.rs` (or a small new module) — `LookupBinding { scope,
  store, label_template }`, reusing `scope_of` / `PickerBinding`'s scope resolver.
- `src/config/resolver.rs` — surface `WidgetKind::Lookup` from explicit profile
  config (layer 3).
- `src/ui/lookup.rs` *(new)* — `LookupWidget` (`FieldWidget`: `present`,
  `activate`), `LookupEditor` (`FieldEditor`), `LookupDialog` (the interactive
  `Dialog`). Pure `LookupInputModel` for parse/validity/selection, unit-tested
  headless (template: the `pick_state` / `PickerDialog` tests).
- `src/ui/widget.rs` — route `WidgetKind::Lookup` in `widget_for` /
  `is_modal_field`; `present_field` path unaffected (present comes from the widget).
- `src/ui/panes/form.rs` — `value_kind` already routes modal fields to `Launch`;
  add the `lookup` → `Launch` branch alongside the picker.
- `src/workflows/edit_form.rs` — add the transient `resolved_label` seam on
  `EditField` (or an equivalent lookup-name carrier).
- `src/ui/state.rs` (`UiState`) — the `(scope, value) → name` resolution cache +
  in-flight set; the async result handler that fills it and sets
  `form_needs_render`.
- `src/workflows/search_flow.rs` — a reverse-lookup search variant (`store ==
  value`), if not expressible with the existing search entry point.

## Testing

Pure model (no Dialog):

- `parse("5000")` → value 5000, filter "5000"; `parse("5000 (staff)")` → value
  5000; `parse("staff")` → no value, filter "staff"; `parse("")` → no value.
- OK-validity arms: leading number → enabled; selected row → enabled; empty →
  disabled; pure text, no selection → disabled.
- Selection fills input with `<value> (<name>)` and sets `pending_value`.
- Numeric input that matches a candidate highlights that row.

Reverse resolution:

- Cache miss submits exactly one search; cache hit submits none; in-flight
  de-dup.
- Render states: `5000 (…)` (in flight) → `5000 (staff)` (resolved) → `5000`
  (unresolved/not found).

Headless dialog test (template: `PickerDialog` harness): open, type, filter, pick,
OK commits the integer.

Config: `lookup` binding parses; multi-value / missing-key bindings are rejected.

## Docs / changelog (part of "done")

- `docs/src/configuration/widgets.md` — new `## The lookup kind` section; update the
  widget table; switch the `gidNumber` worked example from `picker` to `lookup`
  (note `picker` with `store` is still valid for the number-only behavior).
- `CHANGES.md` — an entry under the current unreleased section.
- `examples/config.toml` / `examples/demo-config.toml` — point `gidNumber` at the
  new kind so `make run` exercises it.
- `README.md` — verify the widget-kind list mentions `lookup`; no reference detail.

## Open items (resolve during planning)

- Exact home of `LookupBinding` (reuse `relation.rs` vs a new module).
- Whether the reverse-lookup search reuses `SearchFlow` as-is with an equality
  filter or needs a thin new entry point.
- The precise `EditField` carrier for the resolved name (a typed
  `resolved_label` vs a generic per-field display-hint).
