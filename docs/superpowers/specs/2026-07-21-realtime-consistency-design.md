# Design: real-time consistency (optimistic concurrency + cache coherence)

**Date:** 2026-07-21

## Problem

eDAPtor was designed to work against **live directory reality**, but the
implementation drifted into trusting locally cached copies of server data that
are never re-validated. A read-only audit (see below) found nine such caches.
Three matter:

1. **Silent lost update on save.** The value diff runs against `baseline`, the
   attribute values captured when the entry was read (`src/ui/state.rs:243`,
   `src/workflows/save.rs:293`). Nothing re-checks the entry before the MODIFY.
   If another client changed an attribute in between, our diff emits a `Replace`
   (single-valued) or whole-attribute `Delete` that overwrites their change — and
   the UI reports "Saved." There is **no optimistic-concurrency mechanism
   anywhere in the codebase** (no assertion control, no `entryCSN` check, no
   pre-/post-read); grep for `assertion`/`entryCSN`/`pre_read` returns nothing.

2. **Duplicate autonumber allocation.** `open_create` scans for the next free
   `uidNumber` when the form *opens* (`src/ui/app.rs:449`,
   `src/workflows/alloc_flow.rs:57`), then never re-checks at submit. Two admins
   creating users in the same window both receive the same number; both ADDs
   succeed unless a uniqueness overlay is configured. This is the one
   data-corrupting bug rather than a UX bug.

3. **Stale local model / caches.** The entry list is a pure projection of
   `UiState::structure`, built once at bootstrap and never mutated
   (`src/workflows/structure.rs:169,190` `add_child`/`remove` have no production
   callers). `lookup_cache` (`src/ui/state.rs:80`) is never invalidated — grep
   finds only `insert`/`contains_key`, no `remove`/`clear` — so labels (including
   negatively-cached ones) stay wrong for the whole session, even after eDAPtor's
   *own* rename. Both the after-write local reflow and a manual Refresh action
   were **specified in the original three-pane design**
   (`2026-06-01-three-pane-layout-design.md` §5.9, §7.4) and never built.

The original design deliberately chose an eagerly-loaded local structure model
(there is no cheap "has children" signal in LDAP, so branch/leaf classification
*must* be local) and deferred live push (RFC 4533). That decision stands. What
was lost is the **maintenance** of the caches the design assumed — after-write
reflow, manual refresh, and any concurrency check on writes.

## Feasibility (verified, not assumed)

Against the podman test server (`dc=example,dc=org`):

- Root DSE advertises `1.3.6.1.1.12` (RFC 4528 Assertion), `1.3.6.1.1.13.1` /
  `1.3.6.1.1.13.2` (RFC 4527 Pre-Read / Post-Read).
- `entryCSN` is present with microsecond precision. Two sample users share an
  identical `modifyTimestamp` (`20260717071723Z`) but distinct `entryCSN`
  (`.439475Z` vs `.439863Z`) — confirming `modifyTimestamp` is too coarse and
  `entryCSN` is the right version token.
- Assertion control **honoured, not just advertised**: `ldapsearch -e
  "assert=(entryCSN=<current>)"` → `result: 0`; a wrong CSN → `result: 122
  Assertion Failed`.
- `ldap3` 0.12.1 (pinned in Cargo.lock) exposes `Assertion`, `PreRead`,
  `PostRead` as **first-class typed controls** — no manual BER encoding.
  `with_controls(vec![...])` chains onto `.modify()`/`.add()`/`.delete()`;
  `LdapResult.ctrls` carries the post-read entry back via `PostReadResp`.
  eDAPtor already builds a raw control for RFC 5805 transactions
  (`src/ldap/worker.rs:578`), proving the plumbing.

Three `ldap3` sharp edges to respect:

- `Assertion::new()` **panics on a malformed filter** (`parse(f).expect(...)`).
  The filter must be built with `ldap_escape` or validated via the public
  `parse_filter` before use.
- `.critical()` cannot chain onto `Assertion::new()` (it returns `RawControl`).
  Use the struct literal: `Assertion { filter: f }.critical().into()`.
- rc 122 currently renders as `"LDAP error 122: …"` — `src/ldap/result.rs`
  handles a fixed set and falls through. One-line addition plus a test.

## Decomposition

Four independently-shippable specs, in dependency order. Each leaves the tree
better than it found it. **This umbrella document records the shared decisions;
each sub-project gets its own spec → plan → implementation cycle.**

### Spec 1 — Optimistic concurrency (the foundation)

Capture `entryCSN` at read time as a per-entry version token, stored alongside
`baseline` on the edit form. On every MODIFY and DELETE, attach:

- **Assertion** `(entryCSN=<captured>)`, **marked critical** (struct-literal
  route). Critical matters: a non-supporting server must reject the operation
  (rc 12 `unavailableCriticalExtension`) rather than silently applying a blind
  write — which is exactly what the capability probe (below) prevents us from
  ever reaching.
- **Post-Read** requesting `entryCSN` (+ the written attributes), so the write
  response carries the new baseline and CSN back in one round trip — no second
  read.

On rc 122 (`assertionFailed`), the entry changed underneath us. **Retry policy
(decided: rebase-on-no-overlap):**

- Re-read the entry (fresh baseline + CSN).
- Compute the set of attributes *we* are writing and the set the other party
  changed (baseline-vs-new diff).
- **Disjoint** → rebase our diff onto the new baseline and resubmit silently
  (common case: two admins editing different fields of one user).
- **Overlap** → do not retry. Prompt the operator with the specific conflict:
  *"`telephoneNumber` was changed by someone else since you opened this entry
  (now `222`; you're setting `333`). Reload / Overwrite / Cancel."*
  Blind rebase-and-retry is explicitly rejected — it reintroduces the lost update
  the assertion exists to prevent.

**Capability probe (decided: degrade with one-time warning):** at connect, parse
the root DSE `supportedControl` for `1.3.6.1.1.12`. eDAPtor already fetches the
root DSE (`src/ui/state.rs:925`), so this is nearly free. If absent, degrade to
current blind-write behaviour **and** show a one-time status-line warning:
*"server does not support optimistic concurrency; concurrent edits may be lost."*
Silent degradation is rejected (loses the protection invisibly); hard refusal is
rejected (would brick otherwise-fine directories, and blind writes are today's
behaviour anyway). When the control is unsupported we must **not** attach the
critical assertion, or every write would fail rc 12.

Also fixes the membership fan-out legs (`src/workflows/save.rs:178`,
`src/workflows/write_flow.rs:520`): each per-group MODIFY gets its own assertion,
turning today's blind `Add`/`Delete` (which error `noSuchAttribute` /
`attributeOrValueExists` on concurrent membership change) into a detectable,
rebaseable conflict.

Scope touches: `src/ldap/worker.rs` (attach controls, read response controls),
`src/ldap/result.rs` (rc 122 + rc 12 mapping), `src/workflows/read_flow.rs`
(capture `entryCSN`), `src/workflows/edit_form.rs` (store CSN),
`src/workflows/save.rs` / `write_flow.rs` (assertion on every write leg, rebase
logic), `src/ui/state.rs` (probe flag, conflict dialog), a new conflict dialog.

### Spec 2 — Cache coherence

Implements the maintenance the original design specified but never built:

- **After-write local reflow.** Wire `Structure::add_child` on
  `WriteOutcome::Created` and `Structure::remove` on delete/rename, sourcing
  label attrs from the post-read entry (Spec 1 gives us that entry for free).
  Handle `add_child`'s parent-flip return so the tree pane updates. Fixes the
  create-invisibility bug.
- **Manual Refresh action.** Make `LoadStructure` callable outside bootstrap
  (currently its only caller is `src/ui/state.rs:932`) and bind a Refresh
  command — the escape hatch for genuine multi-client structure staleness.
- **`lookup_cache` invalidation.** Clear/refresh affected keys on our own writes
  (rename, create, delete), so eDAPtor stops showing labels it just changed.
  Consider dropping negative-cache entries on any create.
- Clear `state.search` on create so a stale incremental-find query cannot hide
  the new row; ensure `current_leaf_row()` finds the new entry so the highlight
  snaps to it.

### Spec 3 — Autonumber allocation via counter entry

Assertion control on the *new* entry cannot detect that another client grabbed
the same number on a *different* entry. The clean fix is a **counter entry**
allocated by compare-and-swap, built from the same primitive Spec 1 proves:
read counter value N, `modify` it to N+1 asserting `(value=N)`, retry on rc 122.
Atomic allocation, no TOCTOU window. Falls back to the current form-open scan
when no counter entry is configured (with the existing truncation refusal
retained). Config: a per-attribute counter-DN declaration.

### Spec 4 — Delete entries (shelved, resumes here)

The delete design already drafted
(`2026-07-20-delete-entries-design.md`) survives mostly intact and becomes much
smaller once Specs 1–2 exist: DELETE gets an assertion for free, the structure
reflow is already wired, and `lookup_cache` invalidation is already handled. The
companion-group removal, reverse membership stripping, and last-member block are
unchanged.

## Cache audit (full record)

Ranked by likelihood × severity. Kept here so the analysis outlives the
conversation.

| # | Cache | Location | Invalidated? | Worst outcome | Addressed by |
|---|---|---|---|---|---|
| 1 | `EditField::baseline` (save diff) | `state.rs:243`, `save.rs:293` | only on re-read after own save | **silent lost update** | Spec 1 |
| 2 | fan-out membership baseline | `save.rs:341`, `edit_form.rs:200` | never | partial save; wrong tick state | Spec 1 |
| 3 | autonumber value | `app.rs:449`, `alloc_flow.rs:57` | never; no re-check at ADD | **duplicate uidNumber** | Spec 3 |
| 4 | `lookup_cache` (incl. negative) | `state.rs:80,463,495` | **never — no remove/clear** | permanently wrong labels | Spec 2 |
| 5 | `search_results` | `state.rs:75,514` | overwritten per search, unkeyed | picker misses new entries; label cross-talk | (watch; low) |
| 6 | `SchemaModel` | `state.rs:917` | never | validation vs stale schema | (out of scope) |
| 7 | `server_supports_txn` | `state.rs:925` | never | wrong create path | (out of scope) |
| 8 | `samba_domain` | `state.rs:889` | never | silently wrong SID | (out of scope) |
| 9 | `structure`-derived tree/leaf views | `state.rs:800`, `panes/tree.rs:20` | never | stale navigation all session | Spec 2 |

Out-of-scope items (6, 7, 8) change only rarely and typically accompany a server
restart; 5 is display-only and fragile but low-impact. They are recorded, not
fixed, in this round.

## Cross-cutting principle

The one deliberate live re-read today is `fetch_group_members_for_must`
(`src/workflows/write_flow.rs:30`), used only for a refusal check. The target
state: **every write asserts the version it was based on**, and the version
travels back on the write response — so decisions are made against directory
reality, not a copy taken earlier, without adopting RFC 4533 live push.

## Out of scope (whole round)

RFC 4533 syncrepl live push; per-entry write-permission detection (not possible
against OpenLDAP); schema/root-DSE/samba re-probing mid-session; the unkeyed
`search_results` slot redesign.
