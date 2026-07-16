# Design: companion entry on create (user private group)

**Date:** 2026-07-16 · **Branch:** `feat/usability` · **Item (b)** of the usability batch
(the last one; see `docs/HANDOVER.md`). Item (c) is done.

## Problem

`create::plan_create` builds exactly **one** `Add`. A common need — creating a POSIX
user together with its **user-private group** (a `posixGroup` with `cn = uid` and the
same `gidNumber`) — requires a *second* entry created alongside the first. Today the
operator must create the group by hand, and there is no way to declare "this profile
also spawns a companion entry."

## Decided behaviour

A profile may declare **one companion entry**, created alongside the primary whenever
that profile creates an entry. The two writes are **atomic when the server supports LDAP
transactions (RFC 5805)** — either both entries are created or neither is — and fall
back to a **sequential companion-first** write otherwise. Both stanzas appear in one
confirm preview. Because `edaptor tui-create` (item (c)) flows through the same
`do_create`, it inherits companions for free.

Settled defaults (see "Open points settled" at the end):
- The companion is created **unconditionally** when declared (no per-create toggle; a
  companion attribute that resolves empty and is MUST simply fails schema validation and
  is reported before any write).
- The companion has **no autonumber and no password** of its own — its attributes are
  literals or `{attr}` templates resolved against the primary's already-composed
  attributes (so `gidNumber = "{gidNumber}"` mirrors the user's allocated gid).

## Scope

New/changed, in dependency order:

- **Config** (`src/config/`): a `CompanionSpec` + `companion: Option<CompanionSpec>` on
  `EntryProfile`.
- **Pure planning** (`src/workflows/create.rs`): `plan_companion`.
- **Capability detection** (`src/ldap/`, surfaced on `UiState`): parse the root DSE
  `supportedExtension` into a `supports_txn` flag at connect.
- **Worker** (`src/ldap/worker.rs`): a `Request::AddAtomic` that runs
  `StartTxn → Add* (under the txn control) → EndTxn`.
- **Write flow** (`src/workflows/write_flow.rs`): an atomic submit path and a sequential
  fallback path.
- **UI** (`src/ui/app.rs`, `src/ui/dialog/confirm.rs`): `do_create` plans + previews
  both stanzas and dispatches to the right write path.
- **Docs**: an mdBook page, `examples/config.toml`, `CHANGES.md`.

Out of scope (possible later extensions, deliberately not built): more than one companion
per profile; a companion with its own `{next:…}` autonumber; a companion password
widget; a per-create toggle / `when` guard.

---

## 1. Config — `[profile.companion]`

A new optional table on `EntryProfile`:

```toml
[profile.companion]
object_classes = ["posixGroup"]
rdn_attr       = "cn"
search_base    = "ou=groups,dc=example,dc=org"

[profile.companion.attributes]
cn        = "{cn}"          # templates resolve against the PRIMARY's final attrs
gidNumber = "{gidNumber}"   # mirrors the user's already-allocated gid
memberUid = "{uid}"
```

```rust
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CompanionSpec {
    pub object_classes: Vec<String>,
    #[serde(default)]
    pub rdn_attr: String,
    #[serde(default)]
    pub search_base: String,
    #[serde(default)]
    pub attributes: std::collections::BTreeMap<String, String>,
}
```

- `EntryProfile` gains `#[serde(default)] pub companion: Option<CompanionSpec>`.
- `attributes` values use the **existing** `{attr}`-template / literal syntax parsed by
  `config::defaults` (`parse_default_value` / `resolve_template`). `objectClass` is not
  an `attributes` key — it comes from `object_classes`. A `{next:MIN-MAX}` autonumber in a
  companion attribute is **unsupported** (companions carry no independent allocation) and
  is rejected at config load with an error naming the profile.
- Validation at load: if `companion` is `Some`, `object_classes` must be non-empty,
  `rdn_attr` non-empty, `search_base` non-empty, and `rdn_attr` must appear as a key in
  `attributes` (so the RDN has a value source). A violation is a config load error naming
  the profile.

## 2. Pure planning — `plan_companion` (`src/workflows/create.rs`)

```rust
pub struct CompanionAdd {
    pub dn: String,
    pub attrs: BTreeMap<String, Vec<String>>,
    pub ldif: String,
}

pub fn plan_companion(
    spec: &CompanionSpec,
    primary_attrs: &BTreeMap<String, Vec<String>>,
    schema: &SchemaModel,
) -> Result<CompanionAdd, String>
```

- For each `(attr, template)` in `spec.attributes`: resolve the template against
  `primary_attrs` using the same engine `apply_static_defaults` uses
  (`config::defaults` template resolution + case-insensitive `first_value`); trim and
  drop empty results.
- Compose `objectClass = ["top"] + spec.object_classes` (deduped, case-insensitive — the
  same rule as `build_add_entry`).
- RDN value = the resolved value of `spec.rdn_attr` from the attr set; empty → `Err`.
  `dn = format!("{}={},{}", rdn_attr, rdn_value, spec.search_base)`.
- `validate` the composed entry against `schema` for `spec.object_classes`; on failure →
  `Err(format_validation_errors(...))`.
- `ldif = render_add(&dn, &attrs)`.

Pure, tvision-free, unit-tested. `primary_attrs` is exactly the `attrs` map that
`plan_create` already returns in `CreatePrep::Confirm` — it includes the RDN,
objectClass, defaults, and the allocated autonumber, so `{gidNumber}` resolves to the
user's real gid.

## 3. Capability detection — `supports_txn`

RFC 5805 OIDs: `1.3.6.1.1.21.1` (StartTransaction), `1.3.6.1.1.21.3` (EndTransaction).

- At connect, after bind, the worker issues a `Base`-scoped search on `""` requesting
  `supportedExtension` (the root DSE). A pure helper
  `txn_supported(exts: &[String]) -> bool` returns true iff BOTH OIDs are present.
- The result is returned from the worker's connect/bootstrap and stored on `UiState`
  (new `pub server_supports_txn: bool`, initialised `false` in `new_for_test` and set in
  `bootstrap`). The root-DSE read joins the existing schema/structure bootstrap reads.
- `txn_supported` is pure and unit-tested (both OIDs → true; either missing → false).

## 4. Worker — `Request::AddAtomic`

```rust
Request::AddAtomic { id: u64, entries: Vec<(String, BTreeMap<String, Vec<String>>)> }
```

Handler (`run_add_atomic`, sync ldap3 — the API is confirmed present in `ldap3` 0.12):

```text
let txn_id = conn.extended(StartTxn)?.0.parse::<StartTxnResp>()?.txn_id;   // 1.3.6.1.1.21.1
for (dn, attrs) in &entries {
    let r = conn.with_controls(TxnSpec { txn_id: &txn_id }).add(dn, entry_of(attrs));
    if r is Err or non-success rc { conn.extended(EndTxn { txn_id, commit: false })?; return WriteError }
}
conn.extended(EndTxn { txn_id: &txn_id, commit: true })?;                   // 1.3.6.1.1.21.3
→ WriteOk { id }
```

- Entries are submitted **companion first, then primary** (order is irrelevant to
  atomicity, kept for parity with the fallback).
- Any Add failure, or an `EndTxn(commit)` failure, aborts the transaction
  (`EndTxn(commit:false)` best-effort) and yields `WriteError { id, msg }` — **nothing is
  left written**.
- `Request::Add` is unchanged and still used for every single-entry create and the
  fallback path.

## 5. Write flow — atomic path + sequential fallback

`src/workflows/write_flow.rs`. Both paths share `plan_create` + `plan_companion` +
the confirm preview; they diverge only at submit + response correlation.

**Atomic path** (`server_supports_txn == true`):
- `submit_create_atomic(worker, entries: Vec<(dn, attrs)>, reread_dn, quit_after)` submits
  one `Request::AddAtomic` tracked by a `WriteIntent::Create { dn: reread_dn, quit_after }`
  (reuse the existing intent — the outcome is identical: one `WriteOk` → `Created`, reread
  the primary). One `WriteError` → `Error`, nothing written.

**Sequential fallback** (`server_supports_txn == false`):
- New `WriteIntent::CompanionThenPrimary { primary_dn, primary_attrs, quit_after }`
  attached to the companion `Add`.
- `on_response`: companion `WriteOk` → new `WriteOutcome::NeedFollowupCreate { dn, attrs,
  quit_after }`; companion `WriteError` → `WriteOutcome::Error(msg)` (no primary submitted
  — the abort is automatic). This mirrors `RenameThenModify → NeedFollowupModify` exactly.
- The follow-up primary `Add` is submitted by `submit_followup_create(...)` and tracked as
  the existing `WriteIntent::Create`, so its `WriteOk` → `Created` and its `WriteError` →
  `Error`. The primary-failed error is enriched to name the already-created companion DN
  so the operator can remove/retry it.

Both paths end in the existing `Created { reread_dn, quit_after }` outcome, so the UI's
post-create reread/status/quit handling is unchanged.

## 6. UI — `do_create` + confirm

`src/ui/app.rs` `do_create`:
1. `plan_create` → `CreatePrep::Confirm { dn, attrs, ldif }` (unchanged), then fold any
   staged password (unchanged).
2. If the profile has a `companion`, call `plan_companion(spec, &attrs, schema)`:
   - `Err(msg)` → show the error dialog, abort (no write).
   - `Ok(CompanionAdd { dn: c_dn, attrs: c_attrs, ldif: c_ldif })` → continue.
3. Confirm preview: when a companion exists, the confirm dialog shows **both** labelled
   LDIF stanzas (primary then companion) in one dialog; otherwise the single stanza as
   today. `dialog/confirm::build` takes the already-composed preview string, so this is
   just a longer string — no dialog API change.
4. On OK:
   - companion present + `server_supports_txn` → `submit_create_atomic(worker,
     vec![(c_dn, c_attrs), (dn, attrs)], reread_dn = dn, quit_after)`.
   - companion present + not supported → `submit_create_with_companion(worker, c_dn,
     c_attrs, dn, attrs, quit_after)` (sequential).
   - no companion → today's `submit_create` (unchanged).

The borrow discipline mirrors the existing `do_create`: the plan (both stanzas) is
computed in one short borrow that drops before `exec_view_focused`; the submit borrow is
taken after the confirm returns.

## Error handling

- **Config**: an incomplete `[profile.companion]` (empty object_classes / rdn_attr /
  search_base, or `rdn_attr` absent from `attributes`) is a load-time error naming the
  profile.
- **Plan**: an empty RDN or a schema-validation failure on the companion → error dialog,
  nothing written (the primary is not created either — the create is aborted at plan
  time, before any write).
- **Atomic write**: any failure rolls the transaction back; `WriteError` surfaces the
  server message; nothing is written.
- **Sequential write**: companion failure → error, primary not attempted. Primary failure
  after a committed companion → error naming the orphan companion DN so the operator can
  clean up. This partial state is only reachable when the server lacks transaction
  support.
- Secrets: not applicable — companions carry no password (a password widget on a
  companion is out of scope).

## Testing

Pure unit tests (no live LDAP):
- `plan_companion`: template resolution against `primary_attrs` (`{cn}`, `{gidNumber}`,
  `{uid}`), objectClass compose (top-first, deduped), RDN compose, empty-value drop,
  empty-RDN error, schema-validation error (missing MUST `gidNumber`).
- `CompanionSpec` TOML parse + the load-time completeness validation (each missing-field
  case errors; a complete spec parses).
- `txn_supported(exts)`: both OIDs → true; either missing → false; empty → false.
- write-flow: atomic path (`AddAtomic` `WriteOk` → `Created`; `WriteError` → `Error`) and
  sequential path (`CompanionThenPrimary` companion-ok → `NeedFollowupCreate`;
  companion-error → `Error`, no follow-up; primary-error names the orphan).

Manual / integration (against the podman demo LDAP, which advertises txn): create a user
via a companion-declaring profile and confirm BOTH entries land atomically; point at a
non-txn server (or simulate) to exercise the fallback. The worker's `AddAtomic` txn
mechanics are thin and verified here rather than by unit test.

## Docs (part of "done")

- **mdBook**: a new Configuration page "Companion entries" (`docs/src/configuration/`),
  added to `SUMMARY.md`, documenting `[profile.companion]`, the template resolution
  against the primary, and the atomic/fallback write behaviour.
- **`examples/config.toml`**: a `[profile.companion]` block on the user profile (kept
  consistent with `docs/src/configuration/full-example.md`).
- **`CHANGES.md`**: Unreleased — the companion-entry feature and its atomic/fallback
  semantics.
- **README** stays orientation-only.

## Build sequence (for the plan)

1. `CompanionSpec` config type + `EntryProfile.companion` + load-time validation (+ parse
   tests).
2. `plan_companion` pure planner (+ tests).
3. `txn_supported` parser + root-DSE read at connect + `UiState.server_supports_txn`
   (+ parser tests).
4. Worker `Request::AddAtomic` (StartTxn/Add-under-control/EndTxn).
5. Write-flow: `submit_create_atomic` + the sequential `CompanionThenPrimary`
   intent/outcome + submit helpers (+ tests for both).
6. `do_create` + confirm two-stanza preview + capability dispatch.
7. Docs + `examples/config.toml` + `CHANGES.md`.

Each step keeps `make check` green.

## Open points settled (flag on review if you disagree)

- **Unconditional companion** (no per-create toggle / `when` guard). A companion whose
  MUST attribute resolves empty errors clearly at plan time.
- **No companion autonumber/password.** The companion mirrors the primary's
  already-resolved values via templates.
- **One companion per profile** (`Option<CompanionSpec>`, not a list). A list is a
  mechanical later extension if needed.
