# Design: unified, configurable picker binding

**Date:** 2026-06-03
**Status:** Approved (design); implementation plan to follow
**Branch (suggested):** `feat-unified-picker`

## Problem

edaptor edits relation/lookup attributes through a popup picker, but the binding
between *a field* and *how it is populated* exists in **three forked forms**:

1. **`[[relation]]`** — a symmetric holder↔candidate membership (e.g.
   `group.member` ↔ `user.memberOf`). Multi-select; stores **DNs**; the
   candidate's `back_attr` (`memberOf`) is synthetic and edits **fan out** to
   holders. Resolved in `src/config/relation.rs`.
2. **`[profile.lookup.<attr>]`** (`LookupSpec`) — single-select; stores a
   **scalar attribute** (e.g. `gidNumber`) of the chosen entry; no back-ref, no
   seeding.
3. *(requested, not yet built)* a **multi-select scalar** pick (e.g. `memberUid`
   storing each user's `uid`), which fits neither path.

The result is three config concepts, three `ValueEditor::open*` constructors,
and three commit branches in `src/ui/app.rs`. They differ only in: **what is
stored per pick** (a DN or a scalar attr), **cardinality** (single/multi), and
**whether the field is written directly or fans out**.

## Goal

Replace all three with **one configurable picker binding** declared per
`(profile, attribute)`, compiling to a single internal `PickerBinding` that one
engine consumes. Specifically:

- `member`, `gidNumber`, `memberUid`, and `memberOf` all become
  `[profile.picker.<attr>]` declarations differing only in their knobs.
- `memberUid` becomes editable via the same multi-select user picker as
  `member` (the original motivating request).
- The picker UI already unified this session (scroll, matches-first ordering,
  multi-select) is **untouched**; only the *binding + commit* paths merge.

## Current state (what merges)

- `src/config/mod.rs`: `LookupSpec`, `EntryProfile.lookups`, `EntryProfile.label`.
- `src/config/relation.rs`: `Relation`, `ResolvedRelation`, `RelationRole`,
  `CandidateScope`, `resolve_relations`, `holder_lookup`, `backref_lookup`.
  `CandidateScope` and the label machinery are **kept and reused**.
- `src/ui/edit_form.rs`: `EditField.relation`, `EditField.lookup`,
  `FieldRelation`; `ValueEditor::{open, open_picker, open_lookup}`; the
  field-tagging that sets `relation`/`lookup`.
- `src/ui/app.rs`: the field-open dispatch (`src/ui/app.rs:750` region), the
  Alt+S commit branches (`:810`), the Enter single-vs-toggle branch (`:867`),
  and `service_picker_search` param resolution (`:989`).
- `src/ui/picker.rs`: `PickerState` identity (currently keyed on DN via
  `same_dn`).

## Design

### Config surface — `[profile.picker.<attr>]`

A picker binding is declared on the profile whose entries **own** the field. It
replaces both `[[relation]]` and `[profile.lookup.*]`.

```toml
# "group" profile (groupOfNames) — direct DN membership
[profile.picker.member]
candidate = "user"           # a [[profile]] name; supplies the candidate scope

# "user" profile — single scalar lookup
[profile.picker.gidNumber]
candidate = "posixgroup"
store     = "gidNumber"      # store this attr of each pick (default: "dn")

# "posixgroup" profile — multi scalar
[profile.picker.memberUid]
candidate = "user"
store     = "uid"

# "user" profile — synthetic back-ref, declared explicitly
[profile.picker.memberOf]
candidate   = "group"
store       = "dn"
fanout_attr = "member"       # never written directly; tick a group ⇒ write `member` on it
```

Raw config type (in a new `src/config/picker.rs`):

```rust
#[derive(Debug, Deserialize, Clone)]
pub struct PickerSpec {
    /// `[[profile]]` name supplying the candidate search scope.
    pub candidate: String,
    /// What to store per pick: the sentinel `"dn"` (default) or an attribute name.
    #[serde(default = "default_store")]
    pub store: String,
    /// Cardinality: "auto" (from the attribute's schema arity), "single", "multi".
    #[serde(default = "default_select")]
    pub select: String,
    /// Present ⇒ synthetic back-ref: the field is not written; this entry's DN is
    /// added/removed in `fanout_attr` on each picked candidate (e.g. memberOf→member).
    #[serde(default)]
    pub fanout_attr: Option<String>,
}
// default_store() == "dn"; default_select() == "auto"
```

`EntryProfile` gains `#[serde(default, rename = "picker")] pub pickers:
BTreeMap<String, PickerSpec>` and **loses** `lookups`. (`label` stays — it is the
profile's display-label template, reused by candidate scopes.)

### Resolved binding

`PickerSpec` resolves (against the profile list) to a `PickerBinding` the UI
consumes. Resolution mirrors `resolve_relations`: the named `candidate` profile
yields a `CandidateScope` (object classes, base, search attrs, label template);
an unknown `candidate` drops the binding (caller warns).

```rust
pub enum Cardinality { Single, Multi }

pub enum StoreKey { Dn, Attr(String) }   // "dn" → Dn, else Attr(name)

pub struct PickerBinding {
    pub attr: String,              // the field this binds (e.g. "memberUid")
    pub scope: CandidateScope,     // resolved candidate search scope
    pub store: StoreKey,           // what each pick contributes / the identity key
    pub select: Option<Cardinality>, // None = derive from the field's schema arity
    pub fanout_attr: Option<String>, // Some ⇒ write this attr on each picked candidate
}
```

`select = "auto"` resolves to `None` and the form fills it from the attribute's
schema arity (`EditField.multi` → `Multi`, else `Single`). `gidNumber` is
SINGLE-VALUE → single; `member`/`memberUid`/`memberOf` are multi.

### Identity = the store key

`PickerState` currently keys candidates by DN (`same_dn`). It generalizes to key
by the **store key**:

- `store = "dn"` → key is the candidate DN (current behavior; DN-normalized,
  case-insensitive compare).
- `store = "<attr>"` → key is the stored scalar (e.g. the `uid`); exact compare.

A `Candidate` carries `dn` (always — the real entry DN, needed as the fan-out
target and as the key when `store = dn`), `label`, and `store_value` (the scalar
to commit; equals the DN when `store = dn`). Seeding, dedupe, toggle, and the
matches-first/scroll behavior all operate on the key, unchanged otherwise.

### Open / seed

One `ValueEditor::open(field_idx, field, binding, scope_resolver)` replaces the
three constructors:

- **Seed `selected`** from the field's current values, each as a `Candidate`
  whose `store_value`/key is that value (and `dn = value` when `store = dn`; for
  scalar stores the DN is unknown until results arrive and is not needed unless
  `fanout_attr` is set — see below). Label is the bare value until a search
  result upgrades it (same upgrade logic as today).
- **Cardinality** from `binding.select` (or schema arity).

### Search

`service_picker_search` builds its filter/attrs from `binding.scope` (object
classes, search attrs) exactly as the membership path does today, and requests
the `store` attribute plus the label-template attributes. Results map to
`Candidate { dn: hit.dn, store_value: key_of(hit), label: template(hit) }`,
where `key_of` is the DN for `store = dn` else `pick_value(hit, store)`. Hits
lacking the store value are skipped (e.g. a candidate with no `uid`).

### Commit (Alt+S)

Branches only on `fanout_attr`:

- **No `fanout_attr`** (direct write): set the field's value(s) to the selected
  candidates' `store_value`s. Single → exactly one (replaces, as `gidNumber`
  does today, writing both `editor` and `values`). Multi → the set
  (`member`/`memberUid`).
- **`fanout_attr = X`** (synthetic back-ref): do **not** write the field.
  Diff selected vs the seeded baseline; for each **added** candidate, add THIS
  entry's DN to attribute `X` on the candidate's entry; for each **removed**
  candidate, delete it. This is exactly the present `RelationRole::BackRef`
  fan-out (preserved, just reached via the binding). Here `store = dn`, so
  `candidate.dn` is known for every seeded and result row.

### Editability of synthetic fields

`memberOf` is operational (`NO-USER-MODIFICATION`) and the schema-driven form
marks such attributes read-only. A field carrying a `PickerBinding` with
`fanout_attr` is forced **editable** (the value is never written to the field
itself — it fans out), overriding the operational read-only default. Fields with
a non-fanout binding follow the normal editable rules.

### Field tagging

When building the edit form for an entry, the entry's matching profile(s)
contribute their `[profile.picker.*]` bindings: a field whose attribute name
matches a declared picker is tagged with the resolved `PickerBinding`. (This
replaces both `tag_lookup_fields` and the `holder_lookup`/`backref_lookup`
relation tagging.) Profile matching is by object class as today.

### Config migration

Breaking, clean cut (the app is early-development and these are first-party
configs):

- Remove `[[relation]]` and `[profile.lookup.*]` parsing and types.
- Rewrite `examples/demo-config.toml` with `[profile.picker.*]`, adding a new
  `posixgroup` profile (`object_classes = ["posixGroup"]`, `search_base =
  ou=groups,…`) so `memberUid` gets a binding, and declaring `member`
  (group), `gidNumber` (user), `memberUid` (posixgroup), and `memberOf` (user,
  fan-out `member`).
- Update the README config example to the new shape.

## Affected components

Modified:
- `src/config/mod.rs` — `EntryProfile`: drop `lookups`, add `pickers`; drop
  `LookupSpec`.
- `src/config/relation.rs` → repurpose as `src/config/picker.rs` (or rename):
  keep `CandidateScope` + label reuse; replace `Relation`/`ResolvedRelation`
  with `PickerSpec`/`PickerBinding` + `resolve_pickers`.
- `src/ui/edit_form.rs` — `EditField`: drop `relation`/`lookup`, add
  `picker: Option<PickerBinding>`; one `ValueEditor::open`; binding-driven
  tagging; force-editable for fan-out fields.
- `src/ui/picker.rs` — key by store value, not DN; `Candidate` gains
  `store_value`.
- `src/ui/app.rs` — one field-open dispatch, one search-param builder, one
  commit (branch on `fanout_attr`), one Enter handler (single vs toggle from
  cardinality).
- `examples/demo-config.toml`, `README.md`.

Kept/reused: `CandidateScope`, `config::label::*`, the picker UI in
`src/ui/view.rs`, the fan-out modify mechanics.

## Testing

- **Config (`picker.rs`)**: parse `[profile.picker.*]`; resolve `candidate` →
  scope; default `store="dn"`, `select="auto"`; unknown candidate dropped;
  `fanout_attr` parsed.
- **`PickerState` identity**: keyed by store value — dedupe/seed/toggle for
  `store="dn"` (DN-normalized) and `store="uid"` (exact); existing ordering and
  scroll tests still pass.
- **Commit**: direct single (replace), direct multi (set of store values), and
  fan-out (added/removed candidates produce the right per-holder modifies),
  golden-pinned in unit tests where pure.
- **edit_form tagging**: a field matching a `[profile.picker.<attr>]` gets a
  binding; `fanout` fields are editable despite operational read-only.
- **Live (gated, against the test server)**: `member` round-trips DNs;
  `gidNumber` single-pick writes the scalar; `memberUid` multi-pick writes the
  `uid` set; `memberOf` fan-out adds/removes `member` on the picked groups.
  Reuse/extend `tests/live_membership.rs` + `tests/live_templates.rs`.
- The existing live test that adds a bare `top`+`posixGroup` entry is unaffected
  (still nis/structural — this design does not touch the server schema).

## Out of scope

- Server schema changes (no rfc2307bis; `posixGroup` stays structural — the
  separate `engineering-unix` posix groups remain as-is).
- The picker popup UI (scroll, ordering, markers) — already done this session.
- Reordering/uniqueness semantics beyond what the current editors do.
- Any change to password/default/autonumber profile features.

## File inventory

Created:
- `src/config/picker.rs` (or `relation.rs` renamed): `PickerSpec`,
  `PickerBinding`, `StoreKey`, `Cardinality`, `resolve_pickers`,
  `CandidateScope` (moved/kept).

Modified:
- `src/config/mod.rs`, `src/ui/edit_form.rs`, `src/ui/picker.rs`,
  `src/ui/app.rs`, `examples/demo-config.toml`, `README.md`.

Removed:
- `LookupSpec`, `Relation`/`ResolvedRelation`/`RelationRole` (and their
  `[[relation]]`/`[profile.lookup]` config) — superseded by the unified picker.
