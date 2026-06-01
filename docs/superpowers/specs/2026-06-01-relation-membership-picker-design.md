# Relation Membership Picker — Design Specification

**Date:** 2026-06-01
**Status:** Approved design (pre-implementation)
**Working directory:** `/home/oetiker/checkouts/ldapedit` (crate/binary `edaptor`)
**Branch context:** built on `feat-three-pane` (== `main` at HEAD `41f90a1`, ratatui UI)

## 1. Summary

Add **symmetric membership editing** to edaptor without a bespoke dual-pane
screen. Instead, the existing multi-value popup (`ValueEditor`) gains a **picker
mode**: when the attribute being edited is a configured *relation* attribute, the
popup stops being free-text rows and becomes a **searchable candidate picker** —
a live, size-capped type-ahead over the candidate population, with the currently
selected entries always visible.

This works in **both directions** from a single config declaration:

- **Holder side** — editing a group's `member`: the picker lists candidate users;
  Save writes the group entry through the existing single-entry path.
- **Back-reference side** — editing a user's `memberOf`: the picker lists candidate
  groups; Save **fans out** `member` MODIFYs across the affected group entries
  (`memberOf` itself is overlay-maintained and never written).

The design reuses the existing diff/validate/LDIF/write machinery and the worker
protocol almost entirely; the only genuinely new surface is the picker UI mode,
the `[[relation]]` config concept, and the back-reference fan-out at Save time.

This supersedes the dual-pane "membership transfer" sketch from the original
edaptor design (`2026-05-29-edaptor-design.md` §7/E-series): same goal, simpler
mechanism reusing the value editor.

## 2. Decisions (locked during brainstorming)

| Topic | Decision |
|---|---|
| Mechanism | A **picker mode** of the existing `ValueEditor` popup, not a new screen |
| Candidate source | **Fresh live LDAP search** on each (debounced) keystroke — catches entries created since startup |
| Search bound | **Size-limited to ~20 matches** per search — fast and robust on huge containers; no full-population fetch |
| Empty search box | Shows the **current selection only**; the user types to discover more |
| Selected visibility | The selected set is held **separately from search results**, so selected rows **always stay visible** even when a filter would exclude them |
| Storage vs display | Stores **DNs**; displays **human labels** (`cn (uid)`) per the candidate template |
| Direction | **Symmetric** — both holder side and back-reference side, delivered together |
| Config model | **One `[[relation]]` block** declares both ends + candidate scope + the inverse, once |
| Commit model | **Rides the entry form's Save** — the picker is an alternate editing widget for a field, not a separate apply action |
| Reverse seed | The user's current group set seeds from their **`memberOf`** (already loaded, overlay-maintained) |
| Atomicity | LDAP has **no transaction**; client-side pre-validate what we can, apply in deterministic order, **report partial results** |

## 3. Config: the `[[relation]]` block

A new top-level config concept declaring a symmetric membership link **once**:

```toml
[[relation]]
name        = "group-membership"
holder      = "group"      # template whose entry OWNS the link attribute
holder_attr = "member"     # the real, writable attribute on the holder
candidate   = "user"       # template that scopes the picker's candidate search
back_attr   = "memberOf"   # virtual back-reference field shown on the candidate side
```

- The **candidate search scope** (base, objectClass, search attributes, label) is
  taken from the `candidate` template's existing `EntryProfile` fields
  (`search_base`, `object_class`, `search_attributes`/`show`, `label`). No scope
  is duplicated in the relation block.
- From this one block **both pickers are derived**:
  - On a `holder` (group) form, `holder_attr` (`member`) renders the picker with
    `candidate` (user) candidates.
  - On a `candidate` (user) form, `back_attr` (`memberOf`) becomes an **editable
    back-reference** picker with `holder` (group) candidates; commits fan out to
    `holder_attr` on each affected holder.
- **Nested groups** are just another `[[relation]]` with `candidate = "group"`.
- Multiple `[[relation]]` blocks may target the same template; each contributes
  one picker-enabled field.

The relation set is resolved into two lookups at startup:
`(template, attr) → relation` for the holder side and the back-ref side, so the
form builder can flag the relevant field when it constructs an `EditForm`.

## 4. The picker (a mode of `ValueEditor`)

Today `ValueEditor` (`src/ui/edit_form.rs`) holds one `TextState` row per value and
commits `committed_values() -> Vec<String>`. In **picker mode** it instead holds:

- a **search `TextState`** (the incremental-search box),
- a **selected set** of `(dn, label)` — seeded from the field's current values,
- the **latest search results** `Vec<(dn, label)>` (≤ 20), and
- a selection cursor.

### 4.1 Rendering (`render_value_editor` picker branch)
- **Top:** the search box.
- **Body:** rows = **selection (always shown, marked `[x]`)** followed by **search
  matches not already selected (`[ ]`)**, deduped by DN. Each row shows the
  **label**, with the raw DN dimmed / on-demand.
- Empty search box → body is the selection only.

### 4.2 Interaction
- Typing edits the search box → triggers a **debounced** candidate search
  (§5.1). Results replace the previous match list.
- `Space` toggles the cursor row's membership in the **selected set**. Toggling a
  search match adds it (and it gains `[x]`); toggling a selected row removes it
  (it stays visible, now `[ ]`, until the popup closes if it no longer matches).
- The selected set is the editor's authoritative state; it is **independent of the
  search results**, which is exactly why selected entries never disappear while
  filtering.
- Close/commit returns the selected **DN set** to the field (same slot as
  `committed_values`).

### 4.3 Labels
Candidate labels are rendered from the candidate template's `label` /
`search_attributes` (reuse the existing structure/view label logic). The picker
stores DNs; the user never has to read a raw DN unless they ask.

## 5. Worker protocol additions (mostly reuse)

The worker (`src/ldap/worker.rs`) already provides everything structurally needed:
`Request::Search { id, … }` with request-id correlation → `Response::Entries { id,
entries }`; per-DN `Request::Modify` → `Response::WriteOk/WriteError { id }`; a
non-blocking poll loop; and size-limit/`truncated` handling (`is_limit_rc`).

### 5.1 Picker search → reuse `Request::Search`
- Add a `size_limit` field (cap 20) to `Request::Search` if not already
  parameterized; the picker sets it to the relation's cap.
- Filter = substring OR across the candidate template's `search_attributes` for the
  typed term, scoped to the candidate `search_base` + `object_class`.
- **Debounce + staleness for free:** each keystroke submits a Search with a fresh
  `id`; the app tracks the latest picker-search `id` and **discards** `Entries`
  whose `id` is stale. No new mechanism.

### 5.2 Fan-out write → reuse `Request::Modify`
- The back-reference commit is **N ordinary `Modify` requests** — one per affected
  holder (group): `add member=<candidateDN>` or `delete member=<candidateDN>`.
- **No new write variant.** The app collects per-`id` `WriteOk`/`WriteError` and
  assembles the result report.

### 5.3 Re-read
- After the batch, re-read the set of touched DNs (holder entry + every affected
  group) via the existing entry-read path, widened from one DN to N.

## 6. Save semantics

The picker is an **alternate editing widget for a field**, so Save stays the single
form action. An `EditField` carries a relation role:

- `RelationRole::None` — ordinary field.
- `RelationRole::Holder` — a real attribute edited via the picker
  (e.g. `group.member`).
- `RelationRole::BackRef { relation }` — a virtual back-ref
  (e.g. `user.memberOf`); **excluded from the entry's own changeset**.

### 6.1 Holder side (`group.member`)
The picker commits the selected DN set to the field; the existing
`diff(original, edited) → ChangeSet` single-entry path writes it **unchanged**.
The groupOfNames last-member rule (spec §8 of the base design) applies as today.

### 6.2 Back-reference side (`user.memberOf`)
At Save, for the back-ref field's **before** (baseline `memberOf`) and **after**
(picker selection) sets:
- `added = after \ before` → for each, a `Modify` adding `member=<userDN>` to that group.
- `removed = before \ after` → for each, a `Modify` deleting `member=<userDN>` from that group.

### 6.3 Combined Save flow
1. Build the **own-entry changeset** (all non-back-ref fields) + the **fan-out
   changesets** (one per affected holder).
2. **One LDIF preview** spanning all entries (the edited entry, if its own
   attributes changed, plus every affected holder/group). (Base design F1:
   preview before apply.)
3. **Client-side pre-validation** of what is computable: groupOfNames
   **last-member rule** on each removal (using each group's current member count),
   plus MUST / single-value / syntax on the own entry.
4. **Apply** in deterministic order. ACL/policy rejections **cannot** be probed
   ahead of time (LDAP has no "may I?"), so they surface per-op during apply.
5. **Re-read** all touched entries (no silent success — base design §10).
6. On a mid-batch failure, **report exactly what landed** and what did not.

### 6.4 Dirty guard
The dirty check widens to include back-ref fields: a changed `memberOf` selection
marks the form dirty (so the existing navigation/quit guard prompts), and the
combined Save is what clears it.

## 7. Error handling

- **Last-member rule** (forward and reverse): a removal that would empty a
  `groupOfNames` is blocked **before** any write, with the existing clear message
  and options. The reverse direction can trip this on several groups in one Save;
  all are reported together.
- **Partial fan-out failure:** the result report lists each affected group with
  its outcome (applied / failed + human-mapped reason). The entry is left in a
  known state because every touched DN is re-read.
- **Result-code mapping** reuses the existing `result_code_message` table.
- **Size-limited search:** the picker search is intentionally capped at ~20; when
  more candidates match the typed term, that is communicated in the picker (e.g. a
  "type to narrow" hint), never silently hidden.

## 8. Testing strategy

**Headless / pure (the bulk):**
- `[[relation]]` parsing → both pickers derived; inverse `(template, attr)` lookups
  resolve correctly; nested-group relation parses.
- Selection model: `selected ∪ matches` dedup; `Space` toggle; empty term →
  selected-only; **a selected row stays visible while a filter excludes it** (the
  core requirement, asserted directly).
- Back-ref diff → fan-out: before/after group set → correct per-group add/delete
  `ModOp`s; last-member removal flagged.
- Combined save: own-entry changeset + fan-out merged; **golden-file LDIF**
  spanning multiple DNs.
- Candidate label rendering (`cn (uid)` from the candidate template).
- Partial-failure **report assembly** unit-tested with a fake worker (covered
  without provoking a real ACL denial).

**Integration (gated by `EDAPTOR_TEST_LDAP_URI`, podman slapd — skips when unset):**
- Picker search returns capped, substring-filtered results.
- Forward: edit `group.member` via picker → written; re-read confirms.
- Reverse: edit `user.memberOf` → affected groups' `member` updated; `memberOf`
  reflects the change after the overlay refresh.
- Last-member removal blocked with a clear message.

**UI:** the picker overlay is smoke-tested for construction/render only (project
convention — logic lives below the facade).

## 9. Out of scope (deferred)

- A standalone dual-pane membership screen (this design replaces the need).
- Owner/manager relations beyond member/memberOf (the `[[relation]]` shape admits
  them later; not built now).
- Samba group-mapping side effects of membership changes (handled by the existing
  Samba milestone surface, not this feature).
- True transactional multi-entry apply (LDAP has no transaction; not attainable).

## 10. Affected modules (orientation for planning)

- `src/config/mod.rs` — new `Relation` struct + `relations: Vec<Relation>`;
  resolution into holder/back-ref lookups.
- `src/ui/edit_form.rs` — `RelationRole` on `EditField`; picker-mode state on
  `ValueEditor` (search box, selected set, results); back-ref field excluded from
  `to_edit_entry`; dirty check widened.
- `src/ui/view.rs` — picker branch in `render_value_editor`.
- `src/ui/app.rs` — picker key handling (typing → debounced search, `Space`
  toggle); latest-search-id tracking; combined Save orchestration (own changeset +
  fan-out, preview, pre-validate, apply, re-read, report).
- `src/ldap/worker.rs` — `size_limit` on `Request::Search` (if not present).
- `src/form/changeset.rs` — reuse; back-ref fan-out builds standard `ModOp`s.
- Tests under `tests/` and module unit tests as in §8.
