# Design: make the create-template concept usable — CLI `create` + TUI container rule

**Date:** 2026-07-15 · **Branch:** `feat/usability` · **Item (c)** of the usability batch
(see `docs/HANDOVER.md`). Item (b), the companion group / multi-add, is a **separate**
cycle and explicitly out of scope here.

## Problem

The "create-template concept" already works (New → profile chooser → schema form →
defaults/autonumber → confirm → add), but two gaps make it hazardous or incomplete:

1. **Wrong-container hazard (TUI).** `create::profiles_for_container` matches a profile
   whenever its `search_base` and the current tree branch lie on the same DN path —
   equal, or either a suffix of the other. So standing *above* a profile's home OU
   (e.g. at the tree root `dc=example,dc=org`), a "User" profile with
   `search_base = ou=people,dc=example,dc=org` is still offered, but `open_create` uses
   `container = current_branch` (the root). The object is composed as
   `uid=alice,dc=example,dc=org` — the **wrong place**.

2. **No `edaptor create` CLI subcommand.** `src/main.rs` has `check` / `schema` /
   `passwd` only. Operators want a scriptable, headless way to create an entry from a
   profile without launching the TUI.

## Scope

One spec, two independent code paths:

- **Part 1** — a container-choice rule in the TUI create flow (`src/ui/app.rs` +
  `src/workflows/create.rs` + a small dialog).
- **Part 2** — a flag-driven `edaptor create` subcommand (`src/main.rs` +
  `src/lib.rs`).

Both lean entirely on **existing pure cores** (`apply_static_defaults`, `plan_create`,
`fold_create_password`, `build_add_entry`, `render_add`, `decide_allocation`) plus the
`run_passwd` worker pattern. No new write primitive is introduced.

Out of scope: companion private-group creation / any multi-entry add (item (b)); a
`--container` override for the CLI (YAGNI); an interactive TUI-style CLI prompt flow
(the decided CLI shape is flag-driven).

---

## Part 1 — TUI container rule (ask-which-container)

### Decided behaviour

After the profile is known (single match, or the operator's pick from the chooser),
classify the current tree branch against that profile's `search_base`:

| Relationship (case-insensitive, DN-boundary) | Target container | Prompt? |
|---|---|---|
| `current == search_base` | current branch | no |
| current is **inside** `search_base` (current is deeper; `search_base` is a proper suffix of current) | current branch | no |
| current is an **ancestor** of `search_base` (current is a proper suffix of `search_base`) | operator chooses | **yes** |

The ambiguous case is exactly "you are standing above the profile's home OU." There the
flow pops a **two-choice container dialog**:

- **Here — `<current_branch>`** → container = current branch (create where you stand)
- **In `<search_base>`** → container = the profile's `search_base`

`profiles_for_container` already guarantees the two DNs are on the same path, so these
are the only two sensible targets; no free-form DN entry is offered.

### Pure core

Add to `src/workflows/create.rs`:

```rust
/// Where a create should land, given the operator's current tree branch and the
/// chosen profile's search_base. Pure. Callers pass DNs already known to be on the
/// same path (as guaranteed by `profiles_for_container`).
pub enum CreateContainer {
    /// Unambiguous — create here. Carries the resolved container DN.
    Unambiguous(String),
    /// Current branch is an ancestor of the profile's home OU — ask the operator.
    Ask { here: String, home: String },
}

pub fn resolve_create_container(current_branch: &str, search_base: &str) -> CreateContainer
```

Rules (all case-insensitive, DN-boundary via the existing `dn_boundary_match` helper):
- equal, or `search_base` is a proper suffix of `current_branch` (current is inside/at
  the home) → `Unambiguous(current_branch)`.
- `current_branch` is a proper suffix of `search_base` (current is above the home) →
  `Ask { here: current_branch, home: search_base }`.
- No other relationship reaches this function (guarded by `profiles_for_container`); if
  one somehow does, default to `Unambiguous(current_branch)` (never silently relocate).

### UI wiring

`src/ui/app.rs`, the `CREATE` command handler:

- Both the single-match arm (`[only]`) and the post-chooser arm currently call
  `open_create(state, idx, &container)` directly. Funnel both through one helper that:
  1. reads the chosen profile's `search_base`,
  2. calls `resolve_create_container(&current_branch, &search_base)`,
  3. on `Unambiguous(dn)` → `open_create(state, idx, &dn)`,
  4. on `Ask { here, home }` → exec a container-choice dialog; on OK, `open_create` with
     the picked DN; on cancel, do nothing (abort the create).

New dialog `src/ui/dialog/container_chooser.rs`, modeled on
`src/ui/dialog/profile_chooser.rs`: two labelled buttons/list rows, returns the chosen
DN via a `state` field (mirroring `chosen_profile`). Keep it minimal — two fixed
options, no filtering.

---

## Part 2 — `edaptor create` CLI subcommand

### Invocation

```
edaptor create --profile <NAME> [--password-stdin] [--yes] <attr=value> [<attr=value> ...]
```

- `--profile <NAME>` — profile name, matched case-insensitively against
  `config.profiles`. Omitted with **exactly one** profile configured → that profile is
  used. Omitted with **>1**, or an **unknown** name → error listing available names.
- `<attr=value>` positionals — split on the **first** `=`; attr and value trimmed;
  empty values dropped. Repeat an attr for multi-valued (`mail=a@x mail=b@x`).
  `objectClass` is **not** accepted here — it comes from the profile.
- `--password-stdin` — read exactly one line from stdin as the cleartext password
  (trailing newline stripped) and route it through the profile's password widget.
- `--yes` — write without the confirmation prompt.

### `run_create` in `src/lib.rs` (mirrors `run_passwd`)

Signature (headless, worker injected the same way as `run_passwd`):

```rust
pub fn run_create(
    config: Config,
    bind_password: String,
    profile_name: Option<&str>,
    attr_args: &[String],      // raw "attr=value" strings
    password_stdin: bool,
    assume_yes: bool,
) -> Result<String>            // Ok(message) e.g. "Created uid=alice,ou=people,…"
```

Flow:

1. **Resolve profile** from `config.profiles` (rules above). Empty `search_base` on the
   chosen profile → error. `container = profile.search_base`.
2. **Parse args** into `BTreeMap<String, Vec<String>>` (first-`=` split; trim; drop
   empties; repeat = multi-valued). Reject an explicit `objectClass=…` arg with a clear
   message (it is profile-controlled).
3. **TLS gate** — only when `--password-stdin` is set: enforce
   `samba::password::is_secure(&config.server)` (same guard as `passwd`). A plain create
   does not require TLS (matches `check` / `schema`).
4. **Spawn worker** (`WorkerHandle::spawn`), as `run_passwd` does.
5. **Static defaults** — `apply_static_defaults(&profile.defaults, &mut attrs)` fills
   literals + `{attr}` templates and returns the `{next:MIN-MAX}` autonumber requests.
6. **Autonumber scan** — for each `(attr, min, max)`: a synchronous
   `Request::Search { base: search_base, scope: Subtree, filter: "(<attr>=*)",
   attrs: [<attr>] }`, collect the integer values, `decide_allocation(values, truncated,
   min, max)` → fill `attrs[attr]`. On `Err` (size-limit truncation, range exhausted) →
   propagate the error.
7. **Plan** — build an `EditEntry { dn: "", attrs }` and call
   `plan_create(schema, profile, container, &edited)`:
   - `CreatePrep::Error(msg)` → return `Err`.
   - `CreatePrep::Confirm { dn, attrs, ldif, .. }` → continue with these.
8. **Password fold** — if `--password-stdin`:
   - Read one line from stdin. Resolve the profile's `ResolvedWidget`s and call
     `fold_create_password(&dn, &mut attrs, Some(clear), &widgets, now)`.
   - `Some(masked_ldif)` → use it as the preview.
   - `None` (no password widget matched the entry's object classes) → **error**: the
     operator asked to set a password but the profile has no password widget.
   If `--password-stdin` is **not** set, keep `plan_create`'s `ldif` unchanged.
9. **Preview + confirm gate** — print the (masked) LDIF. Then:
   - `--yes` → proceed to write.
   - not `--yes` **and** stdin is an interactive TTY **and** stdin was not consumed by
     `--password-stdin` → prompt `Proceed? [y/N]`; anything but yes aborts (`Err` or a
     clean "aborted" message — see error handling).
   - otherwise (non-interactive, or stdin already consumed) → **fail fast**: error
     "refusing to create without --yes". Never a silent no-op.
10. **Write** — `worker.request(Request::Add { id, dn, attrs })`; map `WriteOk` → ok,
    `WriteError { msg }` → `Err(msg)`, anything else → `Err(unexpected …)`. Then
    **re-read** the DN (`Request::Search` scope Base) to confirm it resolves, exactly as
    `passwd` does. Return `Ok("Created <dn>")`.

### `src/main.rs`

Add `Command::Create { profile, password_stdin, yes, attrs }` (clap), and in `main`
dispatch to `run_create`, printing the returned message. The `--config` global flag and
existing bind-password resolution are unchanged.

---

## Error handling

- **Fail fast, no silent success** — the `passwd` philosophy. Unknown profile, empty
  `search_base`, `objectClass=` arg, schema-validation failure (`plan_create` Error),
  autonumber exhaustion/truncation, `--password-stdin` without a password widget, TLS
  gate when setting a password, and "no `--yes` in a non-interactive context" all
  return `Err` with an actionable message; `main` surfaces it and exits non-zero.
- **Password never on the argv** — only via `--password-stdin`; the LDIF preview is
  always masked (`mask_password_attrs` inside `fold_create_password`).
- **Confirmation aborts cleanly** — declining the `[y/N]` prompt writes nothing and
  exits without an error stack (a plain "aborted, nothing created" message).

## Testing

Pure unit tests (no live LDAP):

- `resolve_create_container` — the three cases: equal, current-inside-home (both →
  `Unambiguous`), current-above-home (→ `Ask` with correct `here`/`home`); plus a
  case-insensitivity check.
- CLI arg parsing — first-`=` split (`homeDirectory=/home/x=y` keeps `/home/x=y`),
  trimming, empty-value drop, multi-valued via repeat, `objectClass=` rejected.
- Profile resolution — exactly-one default, unknown-name error, ambiguous (>1 without
  `--profile`) error, case-insensitive match.

The composition/validation/password/autonumber cores (`build_add_entry`, `plan_create`,
`fold_create_password`, `apply_static_defaults`, `decide_allocation`) are already
covered by existing tests; `run_create` orchestrates them, so its own coverage focuses
on the new argument/profile/confirm logic. Live-directory behaviour is verified manually
against the podman demo LDAP.

## Docs (part of "done")

- **`CHANGES.md`** — Unreleased: the container-choice prompt, and the new `create`
  subcommand.
- **mdBook (`docs/src/`)** — document the container-choice behaviour on the create /
  create-templates page, and add the `edaptor create` subcommand to the CLI/commands
  reference (invocation, `--profile`, `attr=value`, `--password-stdin`, `--yes`,
  container = `search_base`). Update `SUMMARY.md` if a new page is added.
- **README** — orientation only; a one-line pointer to the new subcommand if warranted,
  no reference detail.

## Build sequence (for the plan)

1. Part 1 pure core (`resolve_create_container` + tests).
2. Part 1 dialog + `app.rs` wiring.
3. Part 2 `run_create` + arg/profile parsing helpers + tests.
4. Part 2 `main.rs` subcommand.
5. Docs + `CHANGES.md`.

Each step keeps `make check` green.
