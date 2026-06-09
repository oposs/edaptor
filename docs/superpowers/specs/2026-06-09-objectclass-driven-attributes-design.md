---
name: objectclass-driven-attributes
description: Dynamic attribute list management driven by objectClass edits — auto-inject new fields, cross out orphaned ones, and pick objectClasses from a schema-seeded list.
metadata:
  type: project
---

# ObjectClass-Driven Attribute Management

**Date:** 2026-06-09  
**Status:** Approved

## Problem

When a user adds an auxiliary objectClass (e.g. `sambaSamAccount`) to an entry, the newly required and permitted attributes do not appear in the edit form. The user has no way to fill in mandatory attributes before saving, causing the server to reject the operation with error 65 (objectClassViolation). Conversely, when an objectClass is removed, attributes that are no longer permitted remain silently in the form and would cause the same error on save.

Additionally, the `objectClass` field offers no guidance — the user must know the exact OC name to type it in.

## Goals

1. When the `objectClass` field changes, immediately inject new fields for every attribute that enters the MUST∪MAY set.
2. Mark fields whose attribute is no longer permitted by any remaining objectClass as _orphaned_ (visually crossed out, auto-deleted on save).
3. Provide a schema-seeded picker for the `objectClass` field so users can tick the objectClasses they want.
4. Bundle all changes (objectClass add/remove + attribute add/delete) into a single atomic `ModifyRequest` — as required by RFC 4511 §4.6.

## Non-Goals

- No config changes required. The objectClass picker is auto-injected, not declared in `[profile.widget]`.
- No changes to the diff/save protocol. The existing `changeset::diff` handles everything once `current_values()` returns `[]` for orphaned fields.
- Auxiliary objectClasses only in practice, but the feature is class-agnostic — it works for any objectClass addition or removal.

## Architecture

### Approach: Flag + hook at value-editor commit (Approach A)

Minimal footprint: two new data flags, one new widget kind, one new `EditForm` method, one render change. The existing `diff()`, validation, and save paths are unchanged or need only a targeted guard.

```
objectClass picker commits
        │
        ▼
value_editor::commit_picker_values()
        │  (detects objectClass field)
        ▼
EditForm::sync_schema_fields(schema)
        │
        ├─ inject new EditFields for attrs entering MUST∪MAY
        ├─ set orphaned=true on fields leaving MUST∪MAY
        ├─ update must flag on surviving fields
        └─ order_fields()
                │
                ▼
        EditForm::to_edit_entry()
                │  orphaned.current_values() == []
                ▼
        changeset::diff()  →  Delete ops for orphaned attrs
                │
                ▼
        Single atomic ModifyRequest to server
```

## Data Model Changes

### `EditField` — one new flag

```rust
/// True when this attribute is no longer permitted by the current objectClasses.
/// Rendered CROSSED_OUT+DIM. current_values() returns [] → diff emits Delete.
pub orphaned: bool,
```

`current_values()` gains a short-circuit:

```rust
pub fn current_values(&self) -> Vec<String> {
    if self.orphaned {
        return vec![];   // diff will emit Delete for this attr
    }
    // ... existing logic
}
```

### `SchemaModel` — one new method

```rust
/// All known objectClass primary names, sorted case-insensitively.
pub fn object_class_names(&self) -> Vec<String>
```

Used to seed the objectClass picker candidate list.

### `WidgetKind` — one new variant

```rust
/// Auto-injected on the objectClass field. Candidates come from the schema
/// at open-time; no LDAP search is performed.
ObjectClassPicker,
```

No payload. The variant is never written to config files.

### `EditForm` — one new method

```rust
pub fn sync_schema_fields(&mut self, schema: &SchemaModel)
```

Algorithm:

1. Read current objectClass values from the `objectClass` field's `current_values()`.
2. Call `schema.effective_attributes(&oc_refs)` → `ResolvedAttributes { must, may }`.
3. Let `allowed = must ∪ may ∪ {"objectClass"}`.
4. For each `attr` in `allowed` not already present in `fields`: inject a new empty `EditField` (editable, not orphaned, `must` set appropriately).
5. For each existing field: set `orphaned = !allowed.contains(label)` (case-insensitive). Update `must` flag.
6. Call `order_fields()` to re-sort (orphaned fields sort to the bottom of their bucket since they have no value).

The `objectClass` field itself is never orphaned.

## ObjectClass Picker

### Auto-injection

In `build_loaded_form` and `build_new_entry_form`, after `build_edit_form` / field construction, tag the `objectClass` field with `WidgetKind::ObjectClassPicker` when it exists (case-insensitive lookup). No config entry needed.

### Open behaviour

When the user presses Enter on the `objectClass` field, `ValueEditor::open_objectclass(field_idx, field, schema)` is called:

- `results` = all `schema.object_class_names()` as `Candidate { dn: name, label: name, store_value: name }`
- `selected` = the field's current values as pre-ticked candidates
- `picker` = `Some(PickerState { selected, results, key_ci: false, ... })`
- `binding` = `None` (no LDAP search)
- A new `objectclass: bool` flag distinguishes this from a normal picker (used to skip LDAP search service and to trigger `sync_schema_fields` on commit)

### Search

Client-side filtering only. `service_picker_search` skips entries where `ve.objectclass == true` (binding is None).

### Commit

On Enter/commit, the selected OC names replace the `objectClass` field's `values`. Immediately after write-back, `sync_schema_fields(schema)` is called. The schema reference is threaded through the commit path from `Ctx` (which already has `read_flow.schema()`).

## Render Changes

In `view.rs`, the field-row renderer gains one new style branch:

```rust
if field.orphaned {
    style = style
        .add_modifier(Modifier::CROSSED_OUT)
        .add_modifier(Modifier::DIM);
    // Drop the * MUST marker — the field is leaving, not required
}
```

Orphaned fields are rendered read-only (their editor is bypassed; the crossed-out value is shown as-is). The user can see exactly what values will be deleted on save.

## Save Path

No changes to `changeset::diff()`. Because `current_values()` returns `[]` for orphaned fields, the diff emits `Delete { attr, values: [] }` (delete whole attribute) automatically.

`form/validate.rs` skips MUST checks for orphaned fields — they are being removed, not required to be filled.

The resulting `ChangeSet` contains:
- `Replace { attr: "objectClass", values: [...new list...] }`
- `Add` ops for each new attribute value the user filled in
- `Delete` ops for each orphaned attribute

All sent as a single `Request::Modify` → single `ModifyRequest` → one atomic server operation.

## Revert (Alt+C)

`revert_form` resets all fields to baseline values. Orphaned fields are reset to their baseline values and `orphaned` is cleared. The field list is not re-synced on revert — the baseline objectClass values are restored along with everything else, so the form returns to exactly the server state.

## Error Handling

If `sync_schema_fields` encounters an objectClass name not in the schema (e.g. a custom OC the server knows but the subschema parse failed on), it silently skips injection for that class. Unknown existing fields are not orphaned — only fields provably outside the allowed set are marked.

## Files Touched

| File | Change |
|---|---|
| `src/schema/model.rs` | Add `object_class_names()` |
| `src/config/widget.rs` | Add `WidgetKind::ObjectClassPicker` variant |
| `src/ui/edit_form.rs` | Add `orphaned` to `EditField`; update `current_values()`; add `sync_schema_fields()`; add `ValueEditor::open_objectclass()`; inject picker in `build_edit_form` |
| `src/ui/app/action.rs` | Thread schema into `build_loaded_form` for injection (already available) |
| `src/ui/edit_form.rs` (`ValueEditor`) | Add `objectclass: bool` flag to distinguish schema picker from LDAP picker |
| `src/ui/app/value_editor.rs` | Detect objectClass commit; call `sync_schema_fields`; skip LDAP search for OC picker |
| `src/ui/view.rs` | Apply `CROSSED_OUT + DIM` style for orphaned fields |
| `src/form/validate.rs` | Skip MUST checks for orphaned fields |
| `src/ui/app/create.rs` | Inject `ObjectClassPicker` in `build_new_entry_form` |

## Testing

- Unit test `sync_schema_fields`: adding an OC injects its MUST+MAY fields; removing an OC orphans its exclusive attrs.
- Unit test `current_values` short-circuit: orphaned field returns `[]`.
- Unit test `object_class_names`: sorted, primary names only.
- Unit test `diff` via orphaned form: produces Delete ops for orphaned attrs.
- Unit test validate: orphaned fields are skipped in MUST checks.
