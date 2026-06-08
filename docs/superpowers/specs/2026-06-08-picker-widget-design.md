# Picker & Membership Widgets — fold `[profile.picker]` into the palette

**Date:** 2026-06-08
**Status:** approved (pending written-spec review)
**Area:** `eDAPtor` — the configurable-field widget palette + the candidate-picker / membership engine.

## Problem

Pickers are eDAPtor's other config-driven "rich attribute" system, living in a
**parallel namespace** (`[profile.picker.<attr>]`, `config::relation`,
`App.pickers`, `tag_picker_fields`, `EditField.picker`) — separate from the
`[profile.widget.<attr>]` palette that now hosts `choice` and `password`. The
goal is **one palette**: pickers become widget kinds.

The current `[profile.picker.<attr>]` also overloads **three** behaviors behind
subtle field combinations, distinguished mainly by whether an optional
`fanout_attr` key is present:

1. **Value lookup** (`gidNumber`) — pick one candidate, store its scalar in *this* entry.
2. **DN/scalar list** (`member`, `memberUid`) — pick candidates, store their DNs/uids in *this* entry's attribute.
3. **Fan-out back-ref** (`memberOf`) — pick candidates, write *this* entry's DN into a back-ref attr on **each** picked candidate; this entry's attribute is overlay-maintained and never written.

We split these into two explicit kinds so the behavior is named, not toggled.

## Goals

1. Two new palette kinds replacing `[profile.picker.<attr>]`:
   - **`kind = "picker"`** — store the picked value(s) in *this* entry's attribute (covers #1 + #2).
   - **`kind = "membership"`** — fan *this* entry's DN out into a back-ref attr on each picked candidate (covers #3).
2. **`candidate`** may be a profile-name *reference* **or** an *inline scope* table, so you can pick from entries that have no managed profile.
3. Remove `[profile.picker.<attr>]` / `PickerSpec` / `App.pickers` / `EditField.picker` / `tag_picker_fields` / `picker_for` / `resolve_pickers` outright (no userbase → no back-compat).

## Non-goals

- **No change to the picker engine.** Live candidate search (`service_picker_search`, the `Response::Entries` intercept), candidate resolution, toggle/dedup, single-vs-multi commit, and the membership **fan-out save** (`membership_fanout`, combined-save) are behavior-preserving — only the binding's config front-end and storage location change.
- **Membership is always multi-select** (no `select` key).
- No new search/scoping capabilities beyond inline scope (e.g. no server-side paging changes).

## Locked decisions

| Decision | Value |
|---|---|
| Kinds | `kind = "picker"` (store-here) and `kind = "membership"` (fan-out) |
| `picker` fields | `candidate`, `store` (`"dn"` sentinel or attr name), `select` (`"single"`/`"multi"`/`"auto"`) |
| `membership` fields | `candidate`, `via` (the back-ref attr written on each candidate); always multi, **no `select`** |
| `candidate` | a profile-name **string** OR an **inline table** `{ base, object_classes, search_attrs?, label? }` |
| Internal repr | both kinds resolve to the **existing `PickerBinding`/`CandidateScope`**; `membership` → `fanout_attr = Some(via)`, `select = Multi`. The palette enum gets one `WidgetKind::Picker(PickerBinding)` arm (runtime already branches on `fanout_attr`) |
| Removed | `[profile.picker]`, `PickerSpec`, `EntryProfile.pickers`, `resolve_pickers`, `picker_for`, `tag_picker_fields`, `EditField.picker`, `App.pickers` — clean break, no alias |
| Engine | unchanged (search, fan-out, combined-save) |

## Config schema

```toml
# Value lookup — single. Stores the chosen group's gidNumber on this entry.
[profile.widget.gidNumber]
kind      = "picker"
candidate = "posixgroup"     # a [[profile]] name (reference)
store     = "gidNumber"      # "dn" sentinel, or a candidate attribute
select    = "single"

# DN list — multi. Stores picked users' DNs in this entry's `member`.
[profile.widget.member]
kind      = "picker"
candidate = "user"
store     = "dn"
select    = "multi"

# Membership — fan this entry's DN into `member` on each picked group.
[profile.widget.memberOf]
kind      = "membership"
candidate = "group"
via       = "member"

# Inline candidate scope (no managed profile needed):
[profile.widget.secretary]
kind      = "picker"
store     = "dn"
select    = "single"
candidate = { base = "ou=people,dc=example,dc=org", object_classes = ["inetOrgPerson"], search_attrs = ["cn", "uid"], label = "{cn} ({uid})" }
```

### Serde model (`src/config/mod.rs`)

```rust
/// A candidate source: a [[profile]] name, or an inline search scope.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(untagged)]
pub enum CandidateRef {
    /// Name of a [[profile]] whose scope (base/object_classes/search_attrs/label) is reused.
    Profile(String),
    /// An inline candidate scope.
    Inline(InlineScope),
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
pub struct InlineScope {
    pub base: String,
    pub object_classes: Vec<String>,
    #[serde(default)]
    pub search_attrs: Vec<String>,
    #[serde(default)]
    pub label: Option<String>,
}

// Added to the existing tagged WidgetSpecCfg enum:
//   Picker     { candidate: CandidateRef, #[serde(default="default_store")] store: String,
//                #[serde(default="default_select")] select: String },
//   Membership { candidate: CandidateRef, via: String },
```

`#[serde(untagged)]` lets `candidate = "group"` parse as `Profile` and
`candidate = { … }` as `Inline`. (`untagged` tries variants in order; a bare
string can only match `Profile`, a table only `Inline` — unambiguous.)

## Resolution

`resolve_widgets` (in `src/config/widget.rs`) grows arms for `Picker` and
`Membership`, both producing a `WidgetKind::Picker(PickerBinding)`:

```rust
fn resolve_candidate(c: &CandidateRef, profiles: &[EntryProfile]) -> Result<CandidateScope, String> {
    match c {
        CandidateRef::Profile(name) => profiles.iter().find(|p| &p.name == name)
            .map(scope_of)               // reuse config::relation::scope_of
            .ok_or_else(|| format!("unknown candidate profile \"{name}\"")),
        CandidateRef::Inline(s) => Ok(CandidateScope {
            base: s.base.clone(),
            object_classes: s.object_classes.clone(),
            search_attrs: s.search_attrs.clone(),
            label_template: s.label.as_ref().map(|l| crate::config::label::parse_label_template(l)),
        }),
    }
}
```

- `Picker { candidate, store, select }` → `PickerBinding { attr, scope, store: StoreKey::from(store), select: parse_select(select), fanout_attr: None }`.
- `Membership { candidate, via }` → `PickerBinding { attr, scope, store: StoreKey::Dn, select: Some(Cardinality::Multi), fanout_attr: Some(via) }`.

The binding/scope **types stay in `config::relation`** — `PickerBinding`,
`CandidateScope`, `StoreKey`, `Cardinality`, `scope_of` are unchanged and
imported by `config::widget`. Only the config-parsing layer is deleted from
`relation.rs`: `resolve_pickers`, `picker_for`, and (from `config::mod`)
`PickerSpec`. This keeps the engine's home stable and minimizes churn.

## Wiring changes

`EditField.picker: Option<PickerBinding>` is **removed**; the binding lives in
`widget_binding` as `WidgetKind::Picker(PickerBinding)`. Every reader is
repointed to the `Picker` arm:

- `EditForm::fanout_labels` and `to_edit_entry` exclusion (`edit_form.rs`).
- `order_fields` "populated/special" bucket.
- `open_value_editor` — open the picker overlay when `widget_binding` is `Picker`.
- The fan-out save (`save.rs`: `f.picker.…fanout_attr` → the `Picker` arm) and the combined-save path (`App.pickers` arg → `App.widgets`).
- `tag_widget_fields` handles `Picker` (tag the bound field; a `membership`/fan-out binding forces the field editable, as `tag_picker_fields` does today).

`App.pickers` / `resolve_pickers` / `tag_picker_fields` / `picker_for` are
removed; their work folds into `App.widgets` / `resolve_widgets` /
`tag_widget_fields` (and a `picker arm` matcher where needed).

## Removed / migrated

- Delete: `[profile.picker.<attr>]`, `PickerSpec`, `EntryProfile.pickers`,
  `resolve_pickers`, `picker_for`, `tag_picker_fields`, `EditField.picker`,
  `App.pickers`.
- Keep (engine): `PickerBinding`, `CandidateScope`, `StoreKey`, `Cardinality`,
  `scope_of`, `PickerState`, `service_picker_search`, `membership_fanout`, the
  combined-save path.
- `examples/*.toml`: migrate `[profile.picker.{memberOf,member,memberUid,gidNumber}]`
  to `[profile.widget.<attr>]` with `kind = "picker"` / `"membership"`.

## Docs

- Fold `configuration/pickers.md` into `configuration/widgets.md` as the
  `picker` and `membership` kind sections (one home, as done for `password`).
  Update the kinds table and the `overview.md` orientation map (drop the separate
  Pickers row; Widgets now covers it).
- `usage/membership.md` (the membership *workflow*) stays; repoint its config link
  to `widgets.md`.

## Testing

- **config** (`config::widget`): `kind="picker"` (single/multi/auto, store dn vs attr) and `kind="membership"` resolve to the right `PickerBinding`; `candidate` as a profile ref vs inline scope both resolve; unknown profile ref errors. (A stray `select` on `membership` is **silently ignored** — serde default; we do NOT add `deny_unknown_fields`, which conflicts with internally-tagged enums.)
- **resolution parity**: a `kind="membership"` binding has `fanout_attr = Some(via)`, `select = Multi`; a `kind="picker"` binding has `fanout_attr = None`.
- **wiring** (`edit_form`): `tag_widget_fields` tags a picker/membership field via `widget_binding`; `fanout_labels` reports a membership field; `order_fields` buckets a picker field as special.
- **engine unchanged**: existing `relation`/`picker`/`save` fan-out + combined-save tests keep passing (only their construction switches from `resolve_pickers` to `resolve_widgets`).
- **examples**: `demo_config_widgets_resolve` asserts `memberOf` → membership, `gidNumber` → picker.
- **live smoke**: against the podman server — open the `memberOf` membership picker on a user, toggle a group, save → the group's `member` gains the user DN (fan-out); open `gidNumber`, pick a group → its `gidNumber` is stored. (Same flows that work today, via the new config.)

## Future extension points (named, not built)

- Inline scope could later grow a `filter` (extra LDAP filter) or `scope` (one-level vs subtree) key.
- A `kind="picker"` over a non-DN scalar with create-on-miss is out of scope.
- The remaining hardcoded handlers (boolean checkbox, binary, date, X-ORDERED) are separate future palette kinds (see the choice/password specs).
