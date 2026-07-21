# Design: delete entries (with companion group + membership stripping)

**Date:** 2026-07-20

## Problem

eDAPtor can create and modify entries but cannot **delete** them. The transport
layer has been ready the whole time — `Request::Delete` / `run_delete`
(`src/ldap/worker.rs:160,481,632`) exist and are live-tested
(`tests/live_write.rs:203`) — but nothing in the UI or the write flow reaches
them. `UiAction::DeleteEntry` (`src/app.rs:13`) is a dead placeholder with zero
callers.

Deleting a POSIX user is not a single-entry operation in practice:

1. The profile may have created a **companion user-private group** alongside the
   user (see `2026-07-16-companion-entry-on-create-design.md`). Deleting the user
   should offer to remove that group too.
2. The user's DN/uid is referenced by **other groups** (`member`,
   `uniqueMember`, `memberUid`). Left behind, these become dangling references.

A second, initially separate bug turns out to share the same seam and is folded
in here: **newly created entries do not appear in the entry list until restart.**

## Root cause of the list bug

The entry list is not a live server search, contrary to expectation.
`UiState::leaf_rows()` (`src/ui/state.rs:800`) is a pure projection of
`UiState::structure`, an in-memory snapshot built **once** at bootstrap from a
single paged `LoadStructure` scan (`src/ui/state.rs:934`).

`Structure::add_child` (`src/workflows/structure.rs:169`) carries the doc comment
*"Add a child node (e.g. after a successful create)"* and `Structure::remove`
(`:190`) its delete counterpart. **Neither has a production caller** — a
repo-wide grep finds only their definitions and unit tests. `state.structure` is
read-only after construction.

The `WriteOutcome::Created` arm (`src/ui/state.rs:597`) sets `list_dirty = true`,
which only forces a **repaint of the unchanged cache** (`src/ui/pump.rs:87`,
`src/ui/panes/leaf.rs:237`), and calls `reread_public`, which is a
`SearchScope::Base` read of the single new DN feeding only the form pane
(`src/workflows/read_flow.rs:57`). Neither re-runs the container scan. Hence the
row is missing until a restart rebuilds `structure`.

Delete needs `Structure::remove` wired for exactly the same reason, so both are
addressed together.

## Decided behaviour

Delete is **opt-in per profile**, confirmed explicitly, and **never recursive**.
Deleting an entry optionally removes its companion group and always strips the
entry from groups that reference it.

---

## 1. Config — `[profile.delete]`

A new optional table on `EntryProfile`, sitting beside the existing `companion`
field:

```toml
[[profile]]
name = "User"
# ... existing keys ...

[profile.delete]
enabled = true
# also offer to remove the companion group this profile creates
companion = true
```

```rust
#[serde(default)]
pub delete: Option<DeleteSpec>,
```

Adding this field requires updating the `EntryProfile` literal initializers at
`src/workflows/create.rs:563` and `:970` and the constructors in
`src/workflows/test_fixtures.rs`.

A global kill-switch is **not** added; `server.read_only` already covers it.

## 2. Gating

Delete is unavailable unless all hold:

1. `Config::is_read_only()` is false (`src/config/mod.rs:392` — covers both
   `server.read_only` and anonymous bind). **This makes delete the first real
   consumer of that flag**, which is currently declared but never read anywhere
   in `src/ui/`. It must be threaded into `UiState` at `bootstrap`
   (`src/ui/state.rs:888`).
2. The selected entry matches a profile with `delete.enabled = true`.
3. The entry is a leaf.

Leaf-ness is **not** pre-checked. The server enforces it and
`src/ldap/result.rs:22` already maps rc 66 to *"Operation not allowed on
non-leaf entry (it still has children)"*. Recursive/subtree delete is never
performed.

When delete is unavailable the menu item is **shown but disabled**, so it stays
discoverable.

## 3. UI wiring

- `src/ui/mod.rs:47` — add `pub const DELETE: tv::Command =
  tv::Command::custom("edaptor.delete");` beside `CREATE`.
- `src/ui/app.rs:24-47` — status line `.item("~Alt-D~ Delete", alt('d'), DELETE)`
  and menu `.command_key("~D~elete", DELETE, alt('d'), "Alt-D")`.
- `src/ui/app.rs:115` `dispatch` — a `DELETE` arm calling `do_delete`.

`do_delete` mirrors `do_create` (`src/ui/app.rs:666`) and **must follow the same
borrow discipline**: plan under `state.borrow()`, drop the borrow before any
`exec_view_focused`, then re-acquire with the split-borrow idiom
(`let UiState { worker, write_flow, .. } = &mut *st;`).

The entry acted on is `UiState::current_leaf` (`src/ui/state.rs:154`).

## 4. Planning and probing

A new `src/workflows/delete.rs` holds the **pure** planner `plan_delete`,
mirroring `plan_create`. It resolves the profile, applies gating, and derives the
companion group DN from the same `[profile.companion]` template that created it
(`create.rs:76 plan_companion`) — rather than inventing a separate matching rule.

Two probes precede the confirmation:

**a) Companion group** (base-scope read), three outcomes:

| Probe result | Behaviour |
|---|---|
| Does not exist | Say nothing, delete only the primary |
| Exists, this user is the only member | Offer it for deletion |
| Exists, has other members | **Do not offer.** Report *"group `foo` has 3 other members, left in place"* |

A group with other members is no longer a private group; leaving it is the
recoverable choice.

**b) Referencing groups** — a subtree search
`(|(member=<dn>)(uniqueMember=<dn>)(memberUid=<uid>))`. This reverse query does
**not exist today** in any form and is new code; `escape_filter` and
`build_equality_filter` (`src/workflows/resolve_flow.rs:23`) supply the escaping
primitives.

**The two probes overlap.** The companion group references the user via
`memberUid` and therefore also matches probe (b). The companion DN is
**excluded from the referencing-group set** once probe (a) has claimed it: if the
companion is being deleted, stripping its membership first would be a pointless
write against an entry about to disappear; if it was *not* offered (because it
has other members), it falls back to being an ordinary referencing group and is
stripped normally. Resolve this in `plan_delete` so the two sets are disjoint
before any write is submitted.

### Last-member block

If the user is the **sole member** of a group whose membership attribute is
MUST (e.g. `member` on `groupOfNames`), the **entire delete is blocked** before
any write, listing the offending groups: *"user is the last member of
`cn=admins`; remove the group first or add another member."*

This reuses the existing helpers `would_empty` (`src/workflows/save.rs:383`),
`last_member_block` (`:394`) and `membership_attr_is_must` (`:421`), and is
consistent with what the save path already enforces. It is the only option with
no half-finished state.

`memberUid` on `posixGroup` is MAY, so emptying those groups is fine and is not
blocked — `src/workflows/write_flow.rs:26` already documents this asymmetry
deliberately.

## 5. Confirmation dialog

`src/ui/dialog/confirm.rs:31` is hardcoded to *"Confirm save"* with
`~S~ave`/`~C~ancel`. Delete needs either a generalized
`confirm::build_with(title, buttons, text)` or a sibling
`src/ui/dialog/confirm_delete.rs`.

It lists every DN to be removed, every group to be modified, and any
"left in place" notes. **Cancel is the default button**, not OK.

`src/ldap/ldif.rs` renders add/modify/modrdn only; a `changetype: delete` stanza
is added there so the preview matches the create path's LDIF style.

## 6. Write ordering

Legs are submitted in this order:

1. Strip the user from each referencing group (one `Modify` per group, reusing
   the `membership_fanout` fan-out shape from `src/workflows/save.rs:178` and the
   tracked-batch machinery of `submit_combined`,
   `src/workflows/write_flow.rs:520`).
2. Delete the companion group, if offered and confirmed.
3. Delete the primary entry.

The primary is last: a failure partway leaves a user whose group memberships are
partly stripped — visible and repairable — rather than an orphaned group with a
dangling `gidNumber`.

### Non-atomicity is accepted and surfaced

Multi-leg writes here are **not atomic**. The existing combined-save path already
admits this (`src/workflows/write_flow.rs:678` warns a batch may be *"only
partially applied"*). The worker has `AddAtomic` (RFC 5805) but **no delete
equivalent**, and OpenLDAP transaction support is patchy.

Decision: accept partial application and report it honestly in the error text
(e.g. *"the group was removed but the user was not — retry"*). Building
transactional delete is explicitly deferred.

## 7. Write flow and outcome application

- `src/workflows/write_flow.rs` — add `WriteIntent::Delete { dn, .. }`,
  `WriteOutcome::Deleted { dn }`, and `submit_delete`, following the existing
  `alloc()` → `worker.submit(...)` → `pending.insert(id, intent)` shape.
- `src/ui/state.rs:530 apply_write_outcome` — a `Deleted` arm that calls
  `Structure::remove` for each deleted DN, clears `edit_form` / `current_leaf`,
  selects a neighbouring row, and sets `list_dirty = true`.

## 8. Structure mutation (fixes the create-invisibility bug)

Landed **first**, as its own piece, since delete builds on the same seam:

- `WriteOutcome::Created` (`src/ui/state.rs:597`) calls `structure.add_child(...)`,
  sourcing label attrs (`cn`, `description`, per `structure_scan_attrs`) from
  `edit_form`, or refreshing the node when the pending base-scope re-read lands
  in the `ReadOutcome::Form` branch (`src/ui/state.rs:239`).
- Handle `add_child`'s return value — it reports whether the parent flipped
  leaf→branch, which the tree pane (`src/ui/panes/tree.rs:20`) must reflect.
- Clear `state.search` on create. It is currently only cleared on tree navigation
  (`commit_branch`, `src/ui/state.rs:788`), so a stale incremental-find query
  would hide the new row even once insertion works.
- Ensure `current_leaf_row()` (`src/ui/state.rs:815`) finds the new entry, so the
  highlight snaps to it. Today `current_leaf` is set to the new DN but the row
  lookup returns `None`.

## 9. Testing

There is **no mock LDAP server**. Three existing harnesses cover this:

- **`WorkerHandle::recording()`** (`src/ldap/worker.rs:331`, `#[cfg(test)]`,
  `pub(crate)`) — asserts exactly which requests were submitted, including leg
  ordering. Being `pub(crate)`, these tests must live in `src/`, not `tests/`.
- **`UiState::pump_responses_for_test()`** (`src/ui/state.rs:308`) plus a new
  `insert_delete_intent_for_test` alongside the existing seeding helpers
  (`src/workflows/write_flow.rs:697`) — drives outcome application without a
  worker.
- **`tests/live_write.rs:200`** already performs a real DELETE and the non-leaf
  failure case against the podman server — direct precedent for a live
  end-to-end test.

Required new coverage:

- Regression for the list bug: create → `structure` gains the node →
  `leaf_rows()` contains it. The existing test at `src/ui/state.rs:1989` asserts
  only the current insufficient behaviour and must be extended, not left as-is.
- `plan_delete` unit tests: gating, companion derivation, the three companion
  probe outcomes, the last-member block.
- Leg ordering via `recording()`.
- `Deleted` outcome removes the node from `structure`.

Extend `user_schema()` in `src/workflows/test_fixtures.rs` with
`posixAccount`/`posixGroup` as needed.

## Scope

In scope, in dependency order:

1. Structure mutation wiring (§8) — fixes the create-invisibility bug on its own.
2. Config `[profile.delete]` (§1) and `read_only` threading (§2).
3. `plan_delete` + probes + last-member block (§4).
4. Confirm dialog + LDIF delete stanza (§5).
5. `submit_delete` + outcome application (§6, §7).
6. UI command/menu wiring (§3).
7. Docs: `CHANGES.md`, the mdBook config reference page for `[profile.delete]`,
   `examples/config.toml`, and the README skeleton if the config shape shown
   there changes.

Explicitly out of scope: recursive/subtree delete; RFC 5805 transactional
delete; a per-entry delete override outside the profile; bulk/multi-select
delete.
