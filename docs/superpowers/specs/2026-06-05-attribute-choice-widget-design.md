# Attribute Choice Widget — Pick-from-Vocabulary ↔ Encoded String

**Date:** 2026-06-05
**Status:** approved (pending written-spec review)
**Area:** `eDAPtor` — the edit-form attribute widgets (pane 3) + config binding.

## Problem

Several LDAP attributes encode a **set or single choice drawn from a fixed
vocabulary** into one string value, and today eDAPtor edits them as raw text
boxes:

- **`sambaAcctFlags`** — a bracketed, fixed-width letter field, e.g.
  `[DU         ]` (D = disabled, U = normal user, X = password never expires,
  N = no password required, …). Cryptic to read, error-prone to hand-edit, and
  trivially corruptible (wrong width / dropped `U`).
- **`loginShell`** — a single value that should come from a small set of valid
  shells (`/bin/bash`, `/bin/sh`, `/sbin/nologin`, …). A free text box lets you
  fat-finger a non-existent shell.

There is already a hardcoded, serialize-only `samba_acct_flags(disabled)`
(`src/samba/account.rs:22`) used by the **create** flow — it emits `[U          ]`
/ `[DU         ]` for the D/U bits only, can not parse, and there is no edit
path. Once an account exists, nothing toggles its flags through the UI.

The same shape recurs across the LDAP world (bitmask integers like
`userAccountControl` / `sambaPwdProperties`, delimited token lists), so the fix
should generalise the *one* widget — not special-case sambaAcctFlags.

## Goals

1. A **generic, config-driven `choice` widget**: pick one (or many) options from
   a vocabulary declared in config, (de)serialise to/from a single attribute
   string. Same checklist/select overlay for every instance; only the
   serialiser differs by `format`.
2. Ship it wired for **two real attributes already in the test data**:
   `sambaAcctFlags` (multi, bracketed) and `loginShell` (single, plain).
3. **Lossless:** never silently drop encoded tokens the UI did not surface
   (preserve `U`/`W`/`S`/`I` on samba flags; preserve an off-list current
   `loginShell`).
4. Fold the existing `samba_acct_flags()` onto the new serialiser so create +
   edit share one code path.
5. Be **future-shaped at near-zero cost**: reserve `bitmask` / `delimited`
   formats and document the extension points, without building what nothing yet
   uses.

## Non-goals (explicitly deferred)

- **No `Widget` trait / plugin registry / general `EditorModel` framework.** The
  palette is modelled as an **enum + match**, following the repo grain
  (`WidgetSpec`, `FieldKind`, `StoreKey`, `Cardinality`). A trait boundary is
  introduced only when a second widget *kind* or scripting actually arrives.
- **No embedded scripting** (Lua/Rhai/WASM). Explored and deferred; the
  parse/serialize boundary is the seam any future scripting would target.
- **No migration** of the existing picker / password / boolean / binary editors
  onto `[profile.widget.<attr>]`. They keep working unchanged; they are named as
  future migration candidates only.
- **No `bitmask` / `delimited` serialisers built now** — reserved enum variants,
  documented, compiler-tracked, unimplemented (`todo!`/explicit error until
  wired).
- No entry-level (whole-entry) handlers; this is attribute-value only.

## Locked decisions

| Decision | Value |
|---|---|
| Config table | `[profile.widget.<attr>]` — per-attribute, under a `[[profile]]`, mirrors `[profile.picker.<attr>]` |
| Discriminator | `kind` (serde internally-tagged); only `kind = "choice"` implemented |
| Cardinality | `select = "single" \| "multi"` |
| Format (now) | `plain` (token *is* the value, single) · `bracketed` (samba letters, multi) |
| Format (reserved) | `bitmask` (OR option bit-values → int) · `delimited` (join tokens with a separator) |
| Option shape | `{ value = "…", label = "…" }` (`value` = the token/letter; `bit`/`sep` added when those formats land) |
| OC matching | `.any()` owner-objectClass overlap, like `picker_for` (NOT password's `.all()`) |
| Options location | **config only**; widget code carries no attribute-specific strings. Samba + loginShell shipped as a preset in `examples/demo-config.toml` |
| Lossless rule | parse → toggle only configured tokens → re-serialise, preserving unlisted tokens (bracketed) / off-list current value (plain) |
| Presentation | **set-labels summary** via the existing `field_display_value` cascade (e.g. `Disabled, No expire`; `—` when empty) |
| Dirty/save | normal single-value path; commit writes the assembled string into `field.editor` (the `current_values()` gateway) |
| samba refactor | `samba::account` exposes `parse_bracketed` / `serialize_bracketed`; `samba_acct_flags(disabled)` reimplemented on top |

## Config schema

A new optional per-attribute table under a profile, keyed by attribute name —
structurally identical to `[profile.picker.<attr>]`:

```toml
# Samba account flags — multi-select, bracketed letters.
[profile.widget.sambaAcctFlags]
kind   = "choice"
select = "multi"
format = "bracketed"
options = [
  { value = "D", label = "Disabled" },
  { value = "X", label = "Password never expires" },
  { value = "N", label = "No password required" },
]

# Login shell — single choice from a fixed vocabulary.
[profile.widget.loginShell]
kind   = "choice"
select = "single"
format = "plain"
options = [
  { value = "/bin/bash",    label = "Bash" },
  { value = "/bin/sh",      label = "POSIX sh" },
  { value = "/sbin/nologin", label = "No login" },
]
```

- `kind` (required): only `"choice"` accepted now; unknown kinds are a config
  parse error (so a typo fails loud, not silent).
- `select` (required): `"single"` | `"multi"`.
- `format` (required): `"plain"` | `"bracketed"` now; `"bitmask"` | `"delimited"`
  parse but error at resolve time with "not yet implemented" until wired.
- `options` (required, non-empty): ordered; render order = config order.

### Serde model (`src/config/mod.rs`)

```rust
// On EntryProfile, mirroring `pickers`:
#[serde(default)]
pub widgets: BTreeMap<String, WidgetSpecCfg>,   // keyed by attribute name

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum WidgetSpecCfg {
    Choice {
        select: String,               // "single" | "multi"
        format: String,               // "plain" | "bracketed" | (reserved) "bitmask" | "delimited"
        options: Vec<ChoiceOption>,   // non-empty
    },
    // future kinds (date, …) add variants here
}

#[derive(Deserialize, Clone)]
pub struct ChoiceOption {
    pub value: String,
    pub label: String,
    // pub bit: Option<u64>,   // added with `bitmask`
}
```

## Resolve → store → apply (mirror the picker pipeline)

The codebase already has the canonical parse→resolve→store-in-`App`→apply-at-build
flow for pickers and tree-labels. The widget follows it exactly.

1. **Resolve** (new `src/config/widget.rs`, sibling of `relation.rs`):
   `resolve_widgets(profiles) -> Vec<ResolvedWidget>`, run once in
   `ui/app/mod.rs::run`. Validates `select`/`format`, rejects empty `options`,
   normalises into:

   ```rust
   pub struct ResolvedWidget {
       pub owner_object_classes: Vec<String>,
       pub attr: String,
       pub widget: ChoiceWidget,        // resolved, ready to use
   }
   pub struct ChoiceWidget {
       pub select: Cardinality,         // reuse config::relation::Cardinality
       pub format: ChoiceFormat,        // enum Plain | Bracketed (Bitmask/Delimited reserved)
       pub options: Vec<ChoiceOption>,
   }
   ```

2. **Store**: `App.widgets: Vec<ResolvedWidget>` (alongside `App.pickers`).

3. **Apply at form build**: a new `tag_widget_fields(form, widgets, ocs,
   read_only)` step in `build_loaded_form` (`action.rs`) **and**
   `build_create_form` (`create.rs`), placed after `tag_picker_fields`. It looks
   up a field by attribute name with `.any()` OC matching (a new `widget_for`,
   modelled on `picker_for`) and attaches the resolved widget to the field.

   `EditField` gains `pub widget_choice: Option<ChoiceWidget>` (parallel to the
   existing `picker: Option<PickerBinding>`). A field is not both a picker and a
   choice; if config declares both for one attr, the widget wins (documented;
   harmless since they target different attrs in practice).

## The editor — a checklist/select overlay

A field tagged with a `choice` widget opens, on **Enter**, a small overlay that
is structurally the existing picker overlay **minus the LDAP search box**: a
static list of `options`, each a row `"{marker} {label}"`.

- `select = "multi"` → checkbox markers `[x]`/`[ ]`, **Space** toggles.
- `select = "single"` → radio markers `(x)`/`( )`, **Enter**/Space selects one
  (mutually exclusive).
- **Alt+S** commit, **Esc** / **Alt+C** cancel. Up/Down move the cursor.

Seeding: on open, parse the field's current value with the format's parser into a
set of present tokens, and pre-check the matching options.

### Reuse vs. new overlay

The existing `ValueEditor` picker mode (`src/ui/app/value_editor.rs`,
`src/ui/view.rs::render_value_editor`) already renders a toggleable
single/multi candidate list with markers. The implementation **reuses that
rendering and key machinery** with a *static* candidate source (no
`service_picker_search`, no worker round-trip). Concretely: a `ValueEditor`
constructor `open_choice(field_idx, field, widget)` that seeds `picker:
Some(PickerState::new(options_as_candidates, …))` and records the
`ChoiceWidget` (for the commit serialiser) instead of a `PickerBinding`. The
search box is suppressed when the source is static.

> If threading a static source through the picker overlay proves to entangle the
> binding/search assumptions, fall back to a dedicated minimal `Overlay::Choice`
> variant with its own tiny render+key fns. Decision made during implementation;
> the seam (parse → toggle → serialise → commit) is identical either way.

### Commit — merge-from-original, then the `current_values()` checkpoint

Losslessness is won or lost at commit, so the algorithm is fixed: **seed the
working set from the parsed *original* value, then apply only the configured
toggles** — never build the set from the checked options alone.

```rust
let mut set = parse(&field_original_value);   // e.g. {U, W}  ← U/W not in config
for opt in widget.options {                   // only D, X, N are shown
    if checked(opt) { set.insert(opt.value.clone()); }
    else            { set.remove(&opt.value); }
}
let new_value = serialize(&set);              // re-emits U, W untouched
```

Because the set is seeded from the original, every token the UI does **not**
surface (`U`, and trust flags `W`/`S`/`I`) survives untouched. Building the set
from only the checked options would silently drop them — exactly the data-loss
the "practical toggles, U stays implicitly set" decision exists to prevent. The
"original value" is the field's baseline value (the value at form-build), so
re-opening and re-committing the editor is idempotent.

Then **write `new_value` into `field.editor`** (and `field.values`), exactly as
the single-select picker does at `value_editor.rs:106-107`. This is mandatory
and is the same seam behind the password dirty-bug fixed earlier: for a
**single + editable** field, `current_values()` (`edit_form.rs:58`) reads
`field.editor`, **not** `field.values`. A choice field is single-valued (its
whole value is the one assembled string), so writing only `values` would leave
`is_dirty()` / `to_edit_entry()` blind to the edit. A test asserts this
explicitly.

## Parse / serialize contract

A small internal contract (a plain function pair selected by `ChoiceFormat`, not
a trait), each `(present_tokens) ↔ String`:

```
parse(s: &str)                        -> BTreeSet<String>   // tokens currently present
serialize(present: &BTreeSet<String>) -> String            // canonical encoded value
```

**Code/config boundary (important):** the *wire mechanics* of a format — its
delimiters, fixed width, and **canonical token order** — are intrinsic to the
format and live in **code**, not config. Config `options` only choose **which
tokens are user-toggleable and how they are labeled**. A user must not be able
to reorder/resize an encoding and thereby produce an invalid attribute value. So
`format = "bracketed"` *is* the samba account-flags encoding, owned by
`src/samba/account.rs`; the `options` list (D/X/N + labels) is purely the
labeled, toggleable subset.

### `plain` (single)

`parse("/bin/bash")` → `{"/bin/bash"}`; `serialize({t})` → `t`; empty set →
empty value (attribute delete). *Off-list preservation*: if the current value is
not among `options`, the editor shows it as a pre-selected, kept "(current) …"
row so a no-op commit never drops a valid custom shell.

### `bracketed` (multi, samba) — exact algorithm

Lives in `src/samba/account.rs`. The canonical universe is the 11 ACB flag
letters in `pdb_encode_acct_ctrl` order — which is *why* the interior is 11 wide:

```rust
const ACB_ORDER: [char; 11] = ['N','D','H','T','U','M','W','S','L','X','I'];
```

`parse(s) -> BTreeSet<char>`:
1. Strip a leading `[` and trailing `]` if present (tolerate their absence —
   best-effort on malformed input).
2. Take the interior and **drop spaces** (padding).
3. Collect the remaining chars into a set. No ordering needed (it is just "which
   letters are present"). Unknown/unexpected letters are **kept** (lossless), not
   rejected. Case-sensitive (samba letters are uppercase).

`serialize(set) -> String`:
1. Walk `ACB_ORDER`, emitting each letter present in the set → canonical order.
2. Append any set letters **not** in `ACB_ORDER` (genuinely unknown — rare;
   preserves losslessness), in sorted order.
3. Left-justify to width 11 with spaces, wrap in `[`…`]`.

Examples: `{D,U}` → `[DU         ]`; `{U}` → `[U          ]`; all 11 →
`[NDHTUMWSLXI]`; `{}` → `[           ]` (11 spaces). `samba_acct_flags(disabled)`
collapses to `serialize(if disabled {D,U} else {U})` (pins the existing golden).

Edge cases: missing brackets → best-effort parse; all configured flags off but a
preserved `U` present → `[U          ]` (U is never synthesised, only preserved
when already present); empty set → 11 spaces, left to the normal single-value
diff to decide replace-vs-delete.

### `bitmask` / `delimited` — reserved

Variants exist in `ChoiceFormat`; resolve returns an explicit "format not yet
implemented" error until wired. Documented shape: **bitmask** ORs each option's
`bit` into a decimal string and preserves unknown bits via a mask of the
configured bits; **delimited** joins tokens with a configured `sep` and
preserves unlisted tokens. Both follow the same merge-from-original commit rule.

## Presentation (read-only value cell)

No new render path. A choice field renders through the existing
`field_display_value` cascade (`src/ui/view.rs:296`) as a **set-labels
summary**: the `label`s of the options whose tokens are present, joined with
`, `; `—` when none present. (For `plain`/single this is just the chosen
option's label, or the raw value when off-list.) This slots beside the existing
`DisabledCheckBox`/`BinaryNote` arms.

Implementation: `field_display_value` consults `field.widget_choice` before the
generic single-value arm and, when present, formats the summary from the parsed
current value + the option labels.

## Save / dirty integration

Nothing special — and that is the point. Unlike password, a choice field has
**no synthetic field, no save-staging, no masking**: the assembled string *is*
the attribute value. It flows through `to_edit_entry()` and the normal
single-value `changeset::diff`. Toggling a flag changes `field.editor` →
`current_values()` differs from baseline → dirty → a normal `Replace` (or
delete-on-empty) mod. This is why `choice` is the right first widget: it
exercises the config seam + a new overlay without the heavy
injection/staging/masking hooks password needs.

## Future extension points (named, not built)

- **Build-time injection + save-staging**: a future widget needing a synthetic
  field and custom serialise-on-save would hook where `password` does —
  `inject_*` in `build_loaded_form`, `stage_*` in `prepare_edit_save`,
  `mask_changeset_secrets` for preview. (`edit_form.rs`, `save.rs`,
  `workflows/create.rs`.)
- **Candidate search**: a widget needing live LDAP candidates would hook where
  `picker` does (`resolve_pickers` / `picker_for` / `service_picker_search`).
- **More formats**: `bitmask`, `delimited` are reserved `ChoiceFormat` variants;
  the compiler flags every match site when they are wired.
- **More kinds**: `WidgetSpecCfg` is a tagged enum; `kind = "date"` etc. add a
  variant non-breakingly.
- **Migration candidates**: boolean checkbox → a `choice`(plain, single,
  TRUE/FALSE); eventually picker/password could move under
  `[profile.widget.<attr>]`. Out of scope here.

## Files

| File | Change |
|---|---|
| `src/config/mod.rs` | `EntryProfile.widgets: BTreeMap<String, WidgetSpecCfg>`; `WidgetSpecCfg` (tagged enum), `ChoiceOption` serde structs |
| `src/config/widget.rs` *(new)* | `ResolvedWidget`, `ChoiceWidget`, `ChoiceFormat`, `resolve_widgets`, `widget_for` (mirror `relation.rs`) |
| `src/samba/account.rs` | `parse_bracketed`, `serialize_bracketed` (canonical order, lossless); reimplement `samba_acct_flags` on top |
| `src/ui/edit_form.rs` | `EditField.widget_choice: Option<ChoiceWidget>`; `tag_widget_fields`; choice fields stay editable |
| `src/ui/app/value_editor.rs` | `ValueEditor::open_choice` + static-source commit that serialises and writes `field.editor`+`values` |
| `src/ui/view.rs` | suppress search box for static source; `field_display_value` set-labels summary for choice fields |
| `src/ui/app/action.rs`, `src/ui/app/create.rs` | call `tag_widget_fields` after `tag_picker_fields` |
| `src/ui/app/mod.rs` | `let widgets = resolve_widgets(&config.profiles);` store on `App` |
| `src/ui/app/input.rs` | Enter on a choice field opens the choice overlay |
| `examples/demo-config.toml`, `examples/config.toml` | `sambaAcctFlags` + `loginShell` widget presets |
| `docs/src/…` | document `[profile.widget.<attr>]` |

## Testing

- **samba parse/serialize** (`samba/account.rs` unit): round-trip
  `[DU         ]` ↔ `{D,U}`; canonical ordering for an out-of-order set; width is
  always 11; **lossless** — a `[UXW        ]` with config only managing D/X
  keeps `W`; `samba_acct_flags(false)` == `[U          ]`,
  `samba_acct_flags(true)` == `[DU         ]` (pin the existing golden).
- **config resolve** (`config/widget.rs` unit): valid choice resolves; empty
  `options` errors; unknown `format`/`select` errors; `bitmask`/`delimited`
  error "not yet implemented"; `widget_for` `.any()` OC matching.
- **dirty/commit** (`edit_form.rs` / `value_editor.rs` unit): a fresh choice
  field is **not** dirty; toggling a flag and committing writes `field.editor`
  so `current_values()` reflects it and `is_dirty()` is true; clearing all
  multi flags yields an empty value (delete); single `plain` off-list current
  value is preserved on a no-op commit.
- **presentation** (`view.rs` unit): set-labels summary for multi; single label
  for plain; `—` for empty.
- **live smoke**: navigate to a `ou=people` user, open `sambaAcctFlags`, toggle
  Disabled, save, re-read → `[DU         ]`; open `loginShell`, pick a shell,
  save → value updated. (tmux, per the TUI-debug gotchas.)
