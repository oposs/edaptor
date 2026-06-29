# M5c — the three reconciliations (design)

**Date:** 2026-06-29 · **Milestone:** M5c · **Branch:** `feat/tvision-ui`

M5c closes the three reconciliations carried forward from the tvision-rs UI
migration (see `docs/HANDOVER.md` NEXT ACTION banner). They are independent
subsystems and are delivered in one brainstorm → plan → implement cycle, in this
order: (1) X-ORDERED editing, (2) schema-aware last-member pre-validation,
(3) live `sambaDomain` discovery.

The guiding constraint throughout: **the neutral domain layer
(`config`/`form`/`ldap`/`schema`/`samba`/`workflows`) already carries the
load-bearing plumbing for all three.** M5c is mostly the tvision *display* side
plus selective wiring; it adds NO new form-core abstractions and keeps the locked
`submit_combined` caller contract intact.

---

## 1. X-ORDERED editing (display side)

### Problem

X-ORDERED multi-valued attributes (OpenLDAP `X-ORDERED 'VALUES'`, values prefixed
`{0}`, `{1}`, …) are declared editable via the widget palette
(`[profile.widget.<attr>]` `kind = "x_ordered"` → `WidgetKind::XOrdered`, which
sets `EditField.ordered = true` in `workflows::widget_bind`). The neutral diff
already special-cases them — `form::changeset::diff` takes an `x_ordered_attrs`
set and emits a single `ModOp::Replace` when the value list (incl. order) changes
(`changeset.rs` ~L209–226); `write_flow.rs` ~L145 builds that set from
`field.ordered`.

But `src/ui/widget.rs::widget_for` has **no `XOrdered` arm**: a field carrying
`widget_binding = Some(WidgetKind::XOrdered)` misses the generic multivalue arm
(which requires `widget_binding.is_none()`) and falls through to read-only
`PlainWidget`. So X-ORDERED fields are read-only in tvision today, and
`docs/src/configuration/widgets.md`'s "editable" claim is currently false.

### Design

A **dedicated editor** `src/ui/ordered.rs`, forked from `src/ui/multivalue.rs`,
owns the `{n}` concern so the plain multivalue editor never learns about ordering
prefixes.

- **Routing.** Add to `widget_for` (before the generic multivalue arm):
  ```rust
  } else if matches!(field.widget_binding, Some(WidgetKind::XOrdered)) {
      Box::new(crate::ui::ordered::OrderedWidget)
  ```
- **Two pure helpers** (the entire `{n}` contract, fully unit-tested):
  - `strip_ordering(s: &str) -> &str` — if `s` starts with `{`, a run of ASCII
    digits, then `}`, return the slice after `}`; otherwise return `s` unchanged.
    Strips a leading ordering index ONLY (a `{` later in the payload is left
    alone; a `{` not closing a pure-digit run is left alone).
  - `reconstruct(rows: &[String]) -> Vec<String>` — map each row at index `i` to
    `format!("{{{i}}}{row}")`. Reassigns contiguous `{0..n-1}` indices by current
    row order.
- **Display strips, commit reconstructs.** The list box and edit line show
  `strip_ordering(value)` — the user never sees `{n}`. `update_staged()` emits
  `CommitOutcome::SetValues(reconstruct(rows))`, i.e. `{n}`-prefixed values in
  current row order, dropping empty rows first.
- **Neutral layer unchanged (load-bearing).** Because staged values carry `{n}`,
  `diff` compares `{n}`-prefixed original (as fetched from LDAP) against
  `{n}`-prefixed new. A pure reorder yields a real difference → one `Replace`.
  `to_edit_entry` / baseline handling need no change.
- **Keys** are identical to `multivalue`: Insert / Alt+A add (at `sel+1`),
  Delete / Alt+D delete, Alt+↑ / Alt+↓ reorder, char/Backspace edit the selected
  row. Reorder is the central operation for ordered attrs.

### Accepted caveat

The first save after editing may emit one *normalizing* `Replace` if the server's
stored indices were not exactly `{0..n-1}` (e.g. sparse indices). The server
re-normalizes on write; the result is correct and the spurious diff is harmless.
Documented in the editor's module doc-comment.

### Tests

- Unit (pure, no LDAP): `strip_ordering` (prefixed, non-prefixed/legacy value,
  payload containing `{`/`}` that is not an index, empty string); `reconstruct`
  (index assignment, empty-row drop); round-trip `reconstruct(strip…)` and that a
  reorder produces a changed value vector.
- Live exercise: declare an `x_ordered` widget on a benign multi-valued attr
  (`description`) in `examples/demo-config.toml` so the editor is drivable via the
  tmux harness (add/delete/reorder → Save → re-read shows reordered `{n}`).
- Doc: flip `docs/src/configuration/widgets.md` back to "editable" (it was left
  as-is at M5b for M5c to make true).

---

## 2. Schema-aware last-member pre-validation

### Problem

Removing a user from a group must be blocked client-side **only** when the group's
membership attribute is MUST — `member` in `groupOfNames`, `uniqueMember` in
`groupOfUniqueNames` (≥1 required). It must NOT be blocked for MAY membership —
`memberUid` in `posixGroup` is MAY, so an empty `posixGroup` is legal.

The guard already exists: `workflows::save::would_empty(current_members, member)`
returns true only when the member is the sole current member, and
`WriteFlow::submit_combined` runs a pre-validation loop over the fanout `Delete`
ops, returning `Err(msg)` (nothing submitted) when a removal `would_empty` a
group. Its caller contract is documented + locked.

Two gaps: (a) `app::dispatch` passes an **empty** `group_members` map, so the
guard never fires client-side (the LDAP server enforces, surfaced as a
`WriteOutcome::Error`); (b) the guard is not schema-aware — it would block MAY
groups too if the map were naively populated.

### Design

Close both gaps on the **populate** side. `submit_combined`'s signature and its
existing pre-validation loop are **unchanged** (contract preserved); the
schema decision lives in how the map is built.

- **Selective populate (the schema gate).** New neutral helper:
  ```rust
  // workflows::save
  pub fn membership_attr_is_must(
      schema: &SchemaModel,
      object_classes: &[String],
      attr: &str,
  ) -> bool
  ```
  true iff `attr` is a MUST attribute of any of `object_classes` per schema.
  Only groups for which this is true are inserted into the `group_members` map.
  MAY groups are omitted → `would_empty` sees an empty slice (`group_members.get`
  → `&[]`, `len()==1` false) → never blocks.
- **Live fetch (blocking, no new async flow).** In `app::dispatch`, after
  `plan_combined_save` produces the fanout, collect each group DN carrying a
  `ModOp::Delete` on its membership attr. For each, issue a blocking
  `worker.request(Request::Search { base = group_dn, scope = Base,
  filter = "(objectClass=*)", attrs = [objectClass, <membership attr>],
  size_limit: Some(1) })` — the same blocking pattern as `discover_samba_domain`.
  From the response, read the group's `objectClass` list and current members;
  call `membership_attr_is_must`; if MUST, insert `group_dn → members` into the
  map. Best-effort: a fetch error leaves that group out of the map (server still
  enforces MUST as backstop).
- **UX ordering.** Run the same `would_empty` precheck over the populated map
  **before** opening the confirm dialog. If a removal would empty a MUST group,
  show the existing error dialog and abort (nothing submitted). Otherwise open the
  confirm LDIF as today; `submit_combined`'s loop remains as defense-in-depth.
- **Membership attr resolution.** The membership widget binding carries the
  fanout attr (`WidgetKind::Picker(b)` with `b.fanout_attr = Some(..)`), and each
  fanout `ModOp::Delete { attr, .. }` carries it — use that attr both for the
  fetch `attrs` list and the MUST check.

### Tests

- Unit: `membership_attr_is_must` (groupOfNames→member MUST true; groupOf
  UniqueNames→uniqueMember MUST true; posixGroup→memberUid MAY false; unknown
  attr false). Selective-populate logic (MUST group included, MAY group omitted)
  using a fixture schema.
- Live (gated, extend `tests/tv_membership.rs`): removing the last member of a
  `groupOfNames` is blocked client-side (nothing submitted, demo data intact);
  emptying a `posixGroup`'s `memberUid` succeeds.

---

## 3. Live `sambaDomain` discovery

### Problem

`UiState.samba_domain` is currently sourced from static config only
(`state.rs::samba_info_from_config`, read in `bootstrap` ~L642). The former
ratatui code discovered it live from a `sambaDomain` directory entry; that source
was deleted at the M5b cutover (`c7d6a04`).

### Design

Port the recovered `discover_samba_domain` (from `ba0f27e`) into `src/ui/state.rs`
as a module fn, **discovery preferred, config fallback**:

```rust
fn discover_samba_domain(worker: &WorkerHandle, base: &str)
    -> Option<crate::samba::SambaDomainInfo>
{
    let resp = worker.request(Request::Search {
        id: 0,
        base: base.to_string(),
        scope: SearchScope::Subtree,
        filter: "(objectClass=sambaDomain)".to_string(),
        attrs: vec!["sambaSID".into(), "sambaAlgorithmicRidBase".into()],
        size_limit: Some(5),
    }).ok()?;
    let Response::Entries { entries, .. } = resp else { return None };
    entries.iter().find_map(|e| crate::samba::sid::parse_samba_domain(&e.attrs))
}
```

- **Bootstrap wiring** (`state.rs` ~L642):
  ```rust
  let samba_domain = if samba_in_use {
      discover_samba_domain(worker, &base).or_else(|| samba_info_from_config(&config))
  } else {
      samba_info_from_config(&config)
  };
  ```
- **Gating (`samba_in_use`).** Attempt discovery only when a profile actually
  declares a `samba_sid` widget — that is exactly when the domain is needed for
  sambaSID auto-generation, and it avoids a wasted subtree search in non-samba
  deployments. Detected by scanning the resolved profiles' widget bindings for
  `WidgetKind::SambaSid`.
- **Best-effort.** A missing/unreadable/denied `sambaDomain` is not an error —
  discovery returns `None` and config is used.

### Tests

- Unit: `parse_samba_domain` is already covered; add a precedence test — discovery
  hit wins over config; discovery miss (`None`) falls back to config; no samba in
  use → config only.
- Live: verify against the demo server. If the demo directory carries no
  `sambaDomain` entry, the config-fallback path is what's exercised — note this in
  the implementation ledger rather than treating it as a failure.

---

## Cross-cutting "done" criteria

- `CHANGES.md` entry under the current unreleased section (X-ORDERED now editable;
  last-member client-side pre-validation; live sambaDomain discovery).
- `docs/src/configuration/widgets.md` updated (X-ORDERED editable claim made true).
- Facade guards print nothing (only `src/ui/**` may `use tvision_rs`; no ratatui/
  tui_* anywhere).
- `make check` green (fmt + clippy `-D warnings` + tests); gated live tests
  (`tv_membership`, `tv_picker`, plus the new X-ORDERED exercise) pass against the
  podman demo server.
- Subagent-driven TDD, atomic commits, crate compiles after each, commit trailer
  `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.

## Non-goals

- No new async flow (Seam 2 uses blocking `worker.request`, matching
  `discover_samba_domain`).
- No change to `submit_combined`'s signature or its locked caller contract.
- No new form-core abstractions; the M3 `widget_for` → modal-editor seam is reused.
- `Activation` stays `{Inline, Modal}` (no `Immediate`, per the M4 divergence note).
