# M5b — Cutover (single-UI tvision tree)

**Status:** design (approved in brainstorming 2026-06-28)
**Milestone:** M5b — the cutover half of the umbrella's M5. M5a (startup flow) is
DONE (`3344b77`). The **three reconciliations** (X-ORDERED editing, schema-aware
last-member pre-validation, live sambaDomain discovery) are a separate **M5c**
cycle — NOT in this spec.
**Umbrella:** [`2026-06-23-tvision-ui-migration-umbrella-design.md`](2026-06-23-tvision-ui-migration-umbrella-design.md) §6 M5 / §7.

## 1. Goal & scope

Collapse the dual-UI tree to a single tvision UI. After M5b:

- the `edaptor` binary runs the **tvision** UI (the ratatui UI is gone);
- `src/ui/` **is** the tvision code (renamed from `src/tui/`); there is no `src/tui/`;
- the `edaptor-tv` dev binary is deleted;
- the `ratatui` / `tui-tree-widget` / `tui-prompts` / `crossterm` deps are dropped;
- `make check` is green and the facade guards are rewritten for a single UI.

**In scope:** the mechanical cutover only (rewire → delete → rename → docs/guards).
**Out of scope (→ M5c):** X-ORDERED editing, schema-aware last-member
pre-validation, live sambaDomain discovery. These are independent feature work on
the post-cutover tree; the cutover does not block on them (X-ORDERED read-only is
documented, last-member is server-enforced, sambaDomain is config-driven).

## 2. Approach — staged, compiler-guided

The cutover is done in four stages, **each of which compiles and passes `make
check`**, so the work is bisectable and reviewable rather than one unbisectable
big-bang diff. A pure rename cannot come first (the target `src/ui/` is occupied
by ratatui until it is deleted), so the order is rewire → delete → rename → docs.

### Stage 1 — Rewire `main.rs` to the tvision UI (while `src/tui` still exists)

In `src/main.rs`:
- `run_tui` calls `edaptor::tui::run(config, password)` instead of
  `edaptor::ui::app::run(config, password)` (ratatui).
- Replace the inline config-discovery block (the `discover_configs()` +
  `match candidates.len()` + `ui::config_picker::pick_config`) with a single call
  to `edaptor::tui::startup::resolve_config_path(config /* the --config flag */)`,
  which already encapsulates the 0/1/many logic + the tvision picker and returns
  `Ok(Some(path))` / `Ok(None)` (user cancelled → clean exit) / `Err` (none found).
- The password step is unchanged: `config.auth.password_source.resolve()` (M5a
  deliberately left password resolution to the caller; `main.rs` already does it).
  The demo config uses `password_source = "env:EDAPTOR_TEST_ADMIN_PW"`, so the
  demo password resolves non-interactively; a `prompt` source still works because
  `resolve()` runs on a clean terminal *before* the tvision program starts.
- The `Check` / `Schema` / `Passwd` subcommands are untouched.

After Stage 1 **both** binaries run tvision; the ratatui tree is dead code but
still compiles (its items are `pub`, so the dead-code lint does not fire). This
stage is independently testable: `cargo run --bin edaptor -- --config
examples/demo-config.toml` launches the tvision UI.

### Stage 2 — Delete the ratatui tree + dev binary + deps

- `git rm -r src/ui` (16 files, ~9 438 LOC).
- `git rm src/bin/edaptor-tv.rs`; remove its `[[bin]]` block from `Cargo.toml`.
- `lib.rs`: remove `pub mod ui;` (the ratatui module) — leave `pub mod tui;` for now.
- Drop from `Cargo.toml` `[dependencies]`: `ratatui`, `tui-tree-widget`,
  `tui-prompts`, **and `crossterm`** (verified: `crossterm` has zero direct uses
  outside `src/ui/`; tvision-rs pulls its own copy transitively). `gen-testdata`
  stays.

After Stage 2 only the tvision tree (`src/tui/`) remains, `main.rs` drives it,
and `make check` is green with the smaller dependency set.

### Stage 3 — Rename `src/tui/` → `src/ui/`

- `git mv src/tui src/ui` (move the whole tree, including `dialog/`, `panes/`).
- `lib.rs`: `pub mod tui;` → `pub mod ui;`.
- Update references:
  - inside the moved tree: `crate::tui::` → `crate::ui::` (all occurrences);
  - `src/main.rs`: `edaptor::tui::` → `edaptor::ui::`;
  - facade-purity doc-comments in the neutral files that name the UI module —
    `src/workflows/pick_state.rs:5` and `src/workflows/search_flow.rs:21` both say
    "no `crate::tui`" — reword to reference the (now sole) `crate::ui`.
- The gated `tv_*` integration tests reference `edaptor::workflows` / `edaptor::config`,
  NOT the UI module (grep-confirmed), so they need no path changes. The compiler is
  the backstop: a clean `cargo build --all-targets` proves no stale `tui` path remains.

After Stage 3 the tree is single-UI under `src/ui/`, named tvision-internally.

### Stage 4 — Facade guards + docs

- **Facade guards** (the canonical pair lives in `CLAUDE.md`, `docs/HANDOVER.md`,
  and each milestone plan). Rewrite to single-UI:
  - tvision pinned to `src/ui/` only:
    `! grep -rl "use tvision_rs" src | grep -vE "^src/ui/"`
  - a regression guard that **no** `ratatui`/`tui_*` import survives anywhere:
    `! grep -rl "use ratatui\|use tui_" src`
  (`src/main.rs` calls the UI through the crate's public API and does not itself
  `use tvision_rs`, so it stays outside the tvision guard.)
- **`CLAUDE.md`**: rewrite the "two UIs coexist" framing (the `## Config model`
  area and the migration notes) to a single tvision UI; drop `edaptor-tv` from the
  build/run commands; update the facade-boundary rule and the guard commands.
- **`README.md`**: remove dual-UI / migration-preview language; reflect that
  edaptor is a tvision-rs TUI.
- **`CHANGES.md`**: one unreleased entry — the TUI is now Turbo-Vision-based; the
  ratatui implementation and the `edaptor-tv` dev binary were removed; deps
  trimmed. (No version bump — release is a separate concern per §7 of the umbrella.)
- **`docs/HANDOVER.md`**: M5b done → M5c next (the three reconciliations).
- **mdBook (`docs/src/`)**: scan for migration-preview / "tvision preview" /
  dual-UI wording and correct it. **Exception — `widgets.md` X-ORDERED:** leave its
  "stripped for display and reconstructed on save" (editable) wording as-is — see §4.
- **Doc-comment cleanup in `src/workflows/`**: the "parity copy … dedup at M5" /
  "cf. the ratatui `ui::edit_form`, deleted at M5" comments
  (`pick_state.rs:3,7`; `edit_form.rs:4,44`) now describe a completed deletion —
  reword to past tense / drop the "deferred" framing so they are not misleading.

## 3. Parity-copy "dedup" — what it actually is

The handover lists parity copies to dedup at M5. Reconnaissance shows the dedup is
**mostly automatic**, not a manual code merge:

- `ui/app/save.rs` already **imports** the neutral logic from `workflows::save`
  (`prepare_save`, `would_empty`, `membership_fanout`, `stage_pending_password`,
  `plan_combined_save`, …); only the ratatui orchestration wrapper dies with the
  tree. `workflows::save` survives unchanged as the sole version.
- `ui::edit_form` (1 718 LOC, with `tui_prompts::TextState`) and `ui::picker`
  (626 LOC) are the ratatui copies; they are **deleted** in Stage 2. The neutral
  `workflows::edit_form` (583 LOC) and `workflows::pick_state` (624 LOC) become the
  sole versions automatically — the tvision UI already uses them.
- `workflows::write_flow` is already the single neutral write path.

So the only "dedup" action beyond deleting `src/ui` is the **doc-comment cleanup**
in §2 Stage 4 (the comments that still frame these as deferred-until-M5 copies).

## 4. X-ORDERED documentation — deliberately deferred to M5c

After the cutover, X-ORDERED multi-valued attributes are **read-only** in the
shipping `edaptor` binary (the `widget_for` routing has no `XOrdered` arm; such
fields fall through to the read-only `PlainWidget`). `docs/src/configuration/widgets.md`
still describes X-ORDERED as editable ("`{n}` … stripped for display and
reconstructed on save"). This is a **known transient doc gap**, accepted because:

- M5c implements X-ORDERED editing immediately after, making the claim true again
  — a "now read-only" edit here would be thrown away in M5c;
- the affected attributes (`olcAccess`, `olcDbIndex`, … under `olcGlobal` /
  `olcDatabaseConfig`) are `cn=config`-only and absent from the demo and normal
  user/group editing, so the gap has no practical user impact;
- the work is unreleased (branch-only) for the entire M5b→M5c window.

The diff/save plumbing for X-ORDERED already exists in the neutral layer
(`form::changeset::diff` takes an `x_ordered_attrs` set and `write_flow.rs:145`
builds + passes it), so M5c's task is purely the tvision **display** side (a
`widget_for` `XOrdered` arm → an ordered multivalue editor that strips `{n}` on
display and reconstructs it from row order on commit).

## 5. Testing & acceptance

- `make check` green (fmt + clippy `-D warnings` + tests) after **every** stage.
- The rewritten facade guards print nothing.
- The gated live `tv_*` tests (`tv_edit_write`, `tv_objectclass`, `tv_create`,
  `tv_picker`, `tv_membership`) still pass vs the podman demo server
  (`EDAPTOR_TEST_LDAP_URI` set) — they exercise `workflows::*`, unaffected by the
  rename.
- **Live tmux acceptance against the real `edaptor` binary** (NOT `edaptor-tv`,
  which no longer exists): build `cargo build -j4 --bin edaptor`; with
  `EDAPTOR_TEST_ADMIN_PW=adminpassword`, run
  `/home/oetiker/scratch/cargo-target/debug/edaptor --config examples/demo-config.toml`;
  confirm the three-pane tvision UI launches, tree→leaf→form navigation works, and
  (with two discovered configs via `XDG_CONFIG_HOME`) the startup picker appears.
  The handover's tmux recipe is updated to use `edaptor` in place of `edaptor-tv`.
- `cargo build --all-targets` is the backstop that no stale `tui` path or dropped
  dependency remains.

## 6. Risks

- **Rename reference breakage.** Mitigated by staging (Stage 3 is a pure rename on
  an already-green tree) and the `cargo build --all-targets` backstop. Surface is
  small: `lib.rs`, `main.rs`, internal `crate::tui::`, two neutral doc-comments.
- **Dropping `crossterm`.** If any transitive need surfaces, tvision-rs's own
  `crossterm` is still in the tree; re-add only if a direct use is found (none is).
- **Irreversibility.** The ratatui deletion is the point of no return, but it is
  preceded by Stage 1 (the tvision UI already drives `edaptor`) and the whole work
  is on the unmerged `feat/tvision-ui` branch; `git revert` of the delete commit
  restores it if needed.

## 7. Acceptance (M5b)

- `edaptor` (the shipping binary) launches the tvision UI; `edaptor-tv` is gone.
- `src/tui/` no longer exists; the tvision code lives at `src/ui/`.
- No `ratatui` / `tui-tree-widget` / `tui-prompts` / `crossterm` in `Cargo.toml`;
  no `use ratatui` / `use tui_*` anywhere in `src`.
- `make check` green; rewritten facade guards clean; gated live tests pass; live
  tmux acceptance of the `edaptor` binary passes.
- `CLAUDE.md`, `README.md`, `CHANGES.md`, `HANDOVER.md`, and the mdBook reflect the
  single-UI reality; the stale `workflows` "dedup at M5" doc-comments are corrected.
