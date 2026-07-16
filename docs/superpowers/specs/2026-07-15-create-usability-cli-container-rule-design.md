# Design: make the create-template concept usable — `tui-create` launcher + TUI container rule

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

2. **No quick launcher into a create form.** `src/main.rs` has `check` / `schema` /
   `passwd` only. To make (say) a new user, an operator must launch the TUI, navigate to
   the right OU, and hit New. There is no way to jump straight into a named profile's
   create form.

## Scope

One spec, two independent code paths:

- **Part 1** — a container-choice rule in the TUI create flow (`src/ui/app.rs` +
  `src/workflows/create.rs` + a small dialog).
- **Part 2** — an `edaptor tui-create <profile> [--container <DN>]` subcommand that
  launches the TUI straight into a profile's create form (`src/main.rs` + `src/ui`).

Both reuse existing machinery: Part 1 the pure `dn_boundary_match` / create planners,
Part 2 the **entire interactive create flow** (`open_create` and everything it drives —
widgets, live templates, autonumber, confirm, save). No new write path or headless
planner is introduced.

Out of scope: companion private-group creation / any multi-entry add (item (b)); a
*headless* (non-TUI) create/write path — `tui-create` deliberately drops into the
interactive create form, so secrets, autonumber, confirm and write are handled by the
existing TUI machinery, not re-implemented.

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

## Part 2 — `edaptor tui-create` subcommand

Instead of a headless write path, `tui-create` is a **launcher**: it starts the normal
TUI but, once the schema has loaded, drops straight into a profile's create form rather
than idling on browse. Everything after that — widgets, live templates, autonumber,
password entry, the confirm dialog, the write — is the **existing interactive create
flow**, unchanged. Cancelling or saving leaves the operator in the normal app.

### Invocation

```
edaptor tui-create [<profile>] [--container <DN>]
```

- `<profile>` — **optional** positional, matched case-insensitively against
  `config.profiles`.
  - Given and known → open that profile's create form.
  - Given but **unknown** → error listing the available profile names; exits before
    launching the TUI (fail fast, no wasted terminal takeover).
  - **Omitted** → launch the TUI and immediately show the profile-chooser dialog over
    **all** profiles; the operator's pick opens the create form.
- `--container <DN>` — where the new object lands. **Defaults to the chosen profile's
  `search_base`.** It is a direct override (advanced use), so the Part-1
  ask-which-container dialog never fires here — there is no "current branch" to be
  ambiguous against. A blank/whitespace value is rejected; an otherwise-malformed DN is
  left for the server to reject at write time.

### Startup-action wiring

The create flow already exists (`open_create` in `src/ui/app.rs`); Part 2 only needs a
way to *trigger* it at launch. Thread an optional startup action from `main` through
`ui::run` into the app:

```rust
pub enum StartupAction {
    /// Open a create form for this profile under this container as soon as the
    /// schema is ready. `container` defaults to the profile's search_base.
    Create { profile_idx: usize, container: String },
    /// No profile named on the command line → show the all-profiles chooser first,
    /// then open_create with the picked profile (container from --container or its
    /// search_base).
    ChooseThenCreate { container: Option<String> },
}
```

- `ui::run(config, password, startup: Option<StartupAction>)` — the existing no-subcommand
  path passes `None` (unchanged behaviour). `tui-create` passes `Some(...)`.
- The action fires **after the initial schema load completes** (the create form is
  schema-driven; `open_create` reads `st.read_flow.schema()`). Concretely: the app's
  post-load step checks a stored `pending_startup` and, if present, dispatches it once —
  `Create` → `open_create(state, profile_idx, &container)`; `ChooseThenCreate` → run the
  profile chooser over all profiles, then `open_create` with the pick and the resolved
  container. Then clear it so it never re-fires.
- Profile-name → index resolution for the `Create` case happens in `main` (so an unknown
  name errors before the TUI starts). `container` is resolved in `main` too: `--container`
  if given, else `profiles[idx].search_base`.

### `src/main.rs`

Add `Command::TuiCreate { profile: Option<String>, container: Option<String> }` (clap;
subcommand name `tui-create`). In `main`:

1. If `profile` is `Some`, resolve it against `config.profiles` (case-insensitive);
   unknown → `Err` listing names. Build `StartupAction::Create { profile_idx, container:
   --container.unwrap_or(search_base) }`.
2. If `profile` is `None` → `StartupAction::ChooseThenCreate { container }`.
3. Call `run_tui(config, password, Some(action))`.

The `--config` global flag and bind-password resolution are unchanged.

---

## Error handling

- **Fail before takeover.** `tui-create` resolves the profile name in `main` *before*
  launching the TUI, so an unknown name prints an actionable error (with the list of
  valid names) and exits non-zero without ever taking over the terminal. A blank
  `--container` is rejected the same way.
- **In-form errors** (schema validation, autonumber exhaustion, write failure) surface
  through the **existing** create / confirm / save UI — `tui-create` changes only *where
  the form opens*, not how it validates or writes. Secrets stay masked in the confirm
  preview exactly as today.
- **Part 1** never silently relocates: on any unexpected container relationship the pure
  helper defaults to the current branch.

## Testing

Pure unit tests (no live LDAP):

- `resolve_create_container` (Part 1) — the three cases: equal, current-inside-home
  (both → `Unambiguous`), current-above-home (→ `Ask` with correct `here`/`home`); plus
  a case-insensitivity check.
- Profile-name resolution (Part 2) — a small pure helper mapping an optional name +
  `config.profiles` to a profile index, or a "not found; valid names are …" error,
  case-insensitive. This is the only headless logic `tui-create` adds; keeping it a pure
  function makes it testable without the TUI.

Part 2's create form is the existing, already-tested interactive flow; the new surface
is just the launch trigger. `tui-create`'s end-to-end behaviour (form opens on the right
profile/container; chooser appears when the name is omitted) is verified manually against
the podman demo LDAP.

## Docs (part of "done")

- **`CHANGES.md`** — Unreleased: the container-choice prompt (Part 1), and the new
  `tui-create` subcommand (Part 2).
- **mdBook (`docs/src/`)** — document the container-choice behaviour on the create /
  create-templates page, and add `edaptor tui-create` to the CLI/commands reference
  (invocation, optional `<profile>` with chooser fallback, `--container` default =
  `search_base`, note that it opens the interactive create form). Update `SUMMARY.md`
  if a new page is added.
- **README** — orientation only; a one-line pointer to the new subcommand if warranted,
  no reference detail.

## Build sequence (for the plan)

1. Part 1 pure core (`resolve_create_container` + tests).
2. Part 1 dialog + `app.rs` wiring.
3. Part 2 `StartupAction` plumbing through `ui::run` + the post-load dispatch + the pure
   profile-name resolver (+ tests).
4. Part 2 `main.rs` `tui-create` subcommand.
5. Docs + `CHANGES.md`.

Each step keeps `make check` green.
