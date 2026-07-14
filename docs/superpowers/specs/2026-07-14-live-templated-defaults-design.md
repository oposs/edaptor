# Live templated defaults (create-mode autofill) — design

**Date:** 2026-07-14 · **Status:** approved, ready for planning
**Feature:** (a) of the usability batch on branch `feat/usability`.

## Problem

When creating a new user, `commonName` (`cn`) and `displayName` are usually just
`{givenName} {sn}`. Today the operator types those by hand. eDAPtor already has a
template engine — `[profile.defaults] cn = "{givenName} {sn}"` — but a template
default fires **once** at form build (`apply_static_defaults`), and at create time
the sources (`givenName`, `sn`) are still empty, so it never fills.

We want the target fields to **fill and keep updating live** as the operator types
the sources, for as long as the operator hasn't taken the field over.

## Decisions (agreed)

- **Config surface:** *no new config*. Reuse `[profile.defaults]`. A template
  default becomes **live in create mode**. This is a documented behaviour change.
- **Scope:** **create mode only**. Editing an existing entry never live-rewrites
  `cn`/`displayName`.
- **Re-arm:** clearing an auto field back to empty **re-arms** it (auto resumes).
- **Incomplete sources:** while a `{field}` source is empty, the auto target shows
  **empty** (a faithful mirror of the sources), not a half-built string.
- **Literals and autonumbers are unchanged** — they keep their one-shot behaviour.

## Behaviour specification

A **template default** is a `[profile.defaults]` entry whose value parses to
`DefaultValue::Template` (contains `{field}` placeholders). Literals and
`{next:MIN-MAX}` autonumbers are **not** live.

Each template target carries a small latch: `auto: bool` + `last_written: String`.

Per recompute pass (create mode, run after each event):

1. Read the target's current first value `v`.
2. **Detect an operator write:** if `v != last_written`, the value changed by a
   path other than our own last write, so re-evaluate ownership:
   `auto = v.is_empty()` (empty ⇒ re-arm; non-empty ⇒ operator owns it).
3. **If `auto`:** compute the template output `out`:
   - `Some(out)` (all sources non-empty): if `out != v`, write `out` into the
     field and set `last_written = out`.
   - `None` (a source is empty): if `v` is non-empty, clear the field to `""` and
     set `last_written = ""`.
4. **If not `auto`:** leave the field alone.

`last_written` is what distinguishes *our* programmatic writes from *operator*
edits: after we write `out`, `v == last_written`, so the next pass does not mistake
our own value for an operator override. This is what makes both re-arm-on-empty and
clear-on-incomplete correct without fighting the operator's cursor.

### Worked examples (target `cn = "{givenName} {sn}"`)

| Step | Action | givenName | sn | cn shown | auto |
|------|--------|-----------|----|----------|------|
| open | — | (empty) | (empty) | (empty) | true |
| 1 | type givenName `John` | John | (empty) | (empty, sources incomplete) | true |
| 2 | type sn `Doe` | John | Doe | `John Doe` | true |
| 3 | edit givenName `Jon` | Jon | Doe | `Jon Doe` (live) | true |
| 4 | operator edits cn `Johnny D` | Jon | Doe | `Johnny D` | false |
| 5 | edit givenName `Jonathan` | Jonathan | Doe | `Johnny D` (locked) | false |
| 6 | operator clears cn | Jonathan | Doe | `Jonathan Doe` (re-armed) | true |

`displayName` behaves identically and independently.

## Architecture

Three layers, matching the existing pure-core / model / view split.

### 1. Pure core — `src/config/defaults.rs`

```rust
/// Per-target live-template latch. `segs` is the parsed template; `auto` tracks
/// whether the target still belongs to the template; `last_written` is the value
/// we last wrote (to tell our writes apart from the operator's).
pub struct LiveTemplateState {
    pub segs: Vec<Seg>,
    pub auto: bool,
    pub last_written: String,
}

/// Build the initial live-template latches from a profile's Template defaults.
/// Literals/autonumbers are skipped. `auto` starts true, `last_written` empty.
pub fn live_templates(d: &ProfileDefaults) -> BTreeMap<String, LiveTemplateState>;

/// Recompute all auto targets against `current` field values, mutating the
/// latches. Returns the `(attr, new_value)` changes to apply to the form. Pure
/// (no I/O). Implements the per-pass rule above.
pub fn recompute_live(
    states: &mut BTreeMap<String, LiveTemplateState>,
    current: &BTreeMap<String, Vec<String>>,
) -> Vec<(String, String)>;
```

Reuses the existing `resolve_template(&segs, current)` (returns `None` when any
source is empty) and `Seg`.

### 2. Model — `src/workflows/edit_form.rs` + `src/workflows/create.rs`

- `EditForm` gains `live_templates: BTreeMap<String, LiveTemplateState>`, default
  empty. Empty in edit mode ⇒ the whole feature is inert there.
- `build_create_form` populates it via `defaults::live_templates(&profile.defaults)`
  after the fields are built. (The one-shot `apply_static_defaults` pass is
  unchanged; at create time the template targets stay empty and hand off to the
  live latches.)

### 3. View — `src/ui/panes/form.rs`

The `EditForm` (with its `live_templates` latches) lives in shared `UiState`
(`self.state.borrow().edit_form`); `sync_into_form()` has just written the on-screen
editor values back into it. In `handle_event`, after `sync_into_form()`, add a
create-mode-only step `self.apply_live_templates(ctx)`.

`apply_live_templates` (respecting the existing borrow discipline — borrow shared
state, compute, release, then write editors):
1. Under a short borrow of `edit_form`: confirm `mode` is `Create`; snapshot
   current field values into a `BTreeMap<attr, Vec<String>>` (case-insensitive attr
   match, mirroring existing helpers); run
   `recompute_live(&mut edit_form.live_templates, &current)` and also update
   `edit_form.fields[i].values` for each returned change. Collect the
   `(field_index, value)` changes.
2. Release the borrow, then for each change call `set_value_text(i, value)` to push
   the new text into the on-screen editor.

Targets are never the currently-focused field in practice (the operator edits a
source; a different field updates), so `set_value_text` never disturbs the caret.
No extra `render()` is needed — `set_value_text` updates the on-screen editor
directly; the per-event header recompute already covers the RDN-header case.

## Testing

**Pure (`defaults.rs`):**
- `live_templates` picks up only Template entries (skips literal/autonumber).
- fills when both sources present; updates live on a source change.
- stops when the operator writes a different value; stays stopped on further
  source changes.
- re-arms when the target is cleared to empty.
- clears the target when a source becomes empty (incomplete).
- `last_written` prevents our own write from being read back as an operator edit.

**Form-level (`form.rs`, create-mode fixture):**
- typing givenName then sn fills `cn` live.
- editing `cn` to a different value stops tracking.
- clearing `cn` re-arms and it refills.
- an edit-mode form with the same profile never rewrites `cn`
  (`live_templates` empty ⇒ inert).

## Docs & changelog

- `docs/src/configuration/defaults.md` — document that template defaults are live
  in create mode (fill + track sources until the operator overrides; clear to
  re-arm; edit mode unaffected). Add a `cn`/`displayName` example.
- `examples/config.toml` (+ the embedded full-example) — add the `cn`/`displayName`
  templates if not already present, with a short comment.
- `CHANGES.md` — "New" entry under Unreleased.

## Out of scope

- Edit-mode live tracking.
- Non-template (literal/autonumber) live behaviour.
- Multi-valued autofill: the latch manages the field's first value; a field the
  operator has grown to multiple values reads as operator-owned (`v != last_written`).
