# Profile object-class bundles in the objectClass picker

**Status:** design approved, ready for implementation plan
**Date:** 2026-06-12

## Problem

`[[profile]]` entries are create-time presets: they decide an entry's initial
object-class set, defaults, and create-form layout. At **edit** time profiles
play no role in the field set — the form is driven entirely by the entry's live,
editable `objectClass` set, and the objectClass picker lists raw schema OC names
one at a time.

So promoting an entry at edit time (e.g. turning a bare `inetOrgPerson`
contractor into a full posix account) means the operator has to know and tick
each individual object class — `posixAccount`, `shadowAccount`, … — by hand. The
sensible bundles the operator already declared in their config are not offered.

## Goal

Surface each configured profile's `object_classes` as a one-pick **bundle** in
the objectClass picker popup, so the operator can apply a known-good set of
object classes in a single action. This makes profiles useful at edit time, not
just create time.

## Behaviour

A pinned **"Profiles"** section at the very top of the objectClass picker popup
lists every configured `[[profile]]` (in config order) that has a non-empty
`object_classes`.

```
Profiles
  employee   (3 classes)  ✓
  contractor (1 class)
─────────────────────────────
[x] inetOrgPerson
[x] posixAccount
[x] shadowAccount
[ ] sambaSamAccount
...
└ employee: inetOrgPerson, posixAccount, shadowAccount   (bottom hint line)
```

Rules:

- **Additive merge.** Selecting a bundle row appends every OC in the bundle that
  is not already in the current selection. It never removes anything. (Removing
  an objectClass in LDAP would orphan the attributes it permits, so a bundle pick
  must never strip a class.)
- **Satisfied state.** A bundle whose OCs are *all* already present renders
  satisfied (✓ + dimmed). Selecting a satisfied bundle is a no-op.
- **Always shown, stable layout.** Every non-empty profile appears as a bundle
  row regardless of how many of its OCs are already applied. Rows do not appear
  or disappear as classes are applied; only their satisfied marker changes.
- **Pinned regardless of filter.** Bundle rows stay at the top even when the
  search box has a term. The filter applies only to the real OC list below.
- **Compact rows, expansion on demand.** A bundle row shows the profile name plus
  a class count (`(3 classes)`), never the full inline expansion (which would
  overflow a narrow popup). The full expansion of the *highlighted* bundle is
  shown in the popup's bottom hint line.
- **Edit and create.** The feature applies wherever the objectClass picker
  appears. Both the edit path and the create path inject the same
  `ObjectClassPicker` widget, so bundles show in both.

## Components / integration points

1. **Picker state** (`src/ui/picker.rs`)
   - Add an optional bundle list to `PickerState`, e.g.
     `bundles: Vec<OcBundle>` where
     `OcBundle { name: String, object_classes: Vec<String> }`. Empty for every
     picker except the objectClass picker.
   - Add a pure satisfied check: a bundle is satisfied when every one of its
     `object_classes` is present (case-insensitive) in the current `selected`
     set (keyed by `store_value`, which is the OC name for this picker).

2. **Visible-row assembly** (`src/ui/picker.rs`)
   - Bundle rows render above the existing selected/results rows and are exempt
     from the `search_active` / filter reordering that governs candidate rows.
     They are a distinct row kind, not `Candidate`s.

3. **OC picker open** (`src/ui/app/value_editor.rs`, `open_objectclass` and the
   OC-picker seeding around line 375)
   - Seed `bundles` from the resolved config profiles: every `[[profile]]` with a
     non-empty `object_classes`, preserving config order.

4. **Selection handling** (`src/ui/app/value_editor.rs`)
   - When the cursor is on a bundle row, the select key performs the additive
     merge into `selected` (OC names) instead of toggling a candidate.
   - Commit (`Alt+S`) is unchanged: it writes the resulting OC set, which then
     drives the existing `objectclass_sync_pending` → `sync_schema_fields` path
     exactly as today.

5. **Render** (`src/ui/view.rs`, `render_value_editor` ~line 484)
   - Render the pinned Profiles section with name + count + satisfied marker.
   - Show the highlighted bundle's full expansion in the block's bottom hint area
     (`.title_bottom(...)`, ~line 501), where key hints already live.

## Data flow

```
config.profiles
  → OcBundle list (seeded at OC-picker open, non-empty object_classes only)
  → bundle row pick → additive merge into PickerState.selected (OC names)
  → Alt+S commit → objectClass field written
  → existing objectclass_sync_pending path → sync_schema_fields pulls in the
    new OCs' schema fields and widgets
```

No new save logic. The bundle is purely a faster way to populate the existing
objectClass selection.

## Edge cases

- **Profile with empty `object_classes`** → skipped; nothing to merge.
- **Duplicate profile names** → each listed; harmless.
- **Bundle OC not in the server schema** → still merged into the field; commit /
  schema-sync handles unknown OCs as it does today. Verify this path during
  implementation (an unknown OC must not panic the schema sync).

## Testing

Pure-logic unit tests in `src/ui/picker.rs`:

- Satisfied detection: a bundle is satisfied iff all its OCs are in `selected`
  (case-insensitive).
- Additive merge appends only the missing OCs and preserves existing selection.
- Bundle rows survive a non-empty filter term (still present and pinned).
- Empty-`object_classes` profiles are excluded from the bundle list.

`src/ui/app/value_editor.rs`:

- Opening the OC picker seeds `bundles` from config profiles (count and order).
- Picking a bundle row leaves the corresponding real OC rows selected and the
  resulting commit set contains the union.

## Docs / changelog

- `CHANGES.md`: user-visible entry under the unreleased section ("objectClass
  picker now offers configured profiles as one-pick object-class bundles").
- mdBook: note the behaviour in the objectClass-editing / CRUD usage page and
  cross-reference from `docs/src/configuration/entry-profiles.md` (profiles are
  now also edit-time bundles, not only create presets).

## Non-goals

- No "replace selection" or "toggle/remove" bundle semantics — additive only.
- No filtering of which bundles appear based on the current entry.
- No new config syntax. Bundles are derived from existing `[[profile]]`
  `object_classes`; nothing new to declare.
