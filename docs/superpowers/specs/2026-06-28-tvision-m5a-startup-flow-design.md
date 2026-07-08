# M5a — Startup flow (tvision config-picker + pre-TUI sequence)

**Status:** design (approved in brainstorming 2026-06-28)
**Milestone:** M5a — the first half of the umbrella's M5. M5b (cutover + carried
reconciliations) is a separate spec → plan → implement cycle.
**Umbrella:** [`2026-06-23-tvision-ui-migration-umbrella-design.md`](2026-06-23-tvision-ui-migration-umbrella-design.md) §6 M5.

## 1. Why this is split out

The umbrella's M5 bundles three distinct workstreams: a net-new **startup flow**,
an irreversible **big-bang cutover** (rename `src/tui`→`src/ui`, delete the ~9.4k
LOC ratatui tree + `edaptor-tv` + spike artifacts, drop the `ratatui`/`tui-*`
deps, dedup parity copies), and a cluster of **carried reconciliations**
(X-ORDERED editing, last-member pre-validation, sambaDomain discovery). Mixing a
reviewable new feature with an unrevertable delete in one cycle is the wrong
risk. **M5a delivers the startup flow only**, while the ratatui UI and the
`edaptor-tv` dev binary still coexist, so it is fully testable in isolation. M5b
performs the cutover and the reconciliations.

## 2. Scope

**In scope (M5a):**
- A tvision config-picker `Dialog`, shown when config discovery finds more than one
  candidate, replacing the ratatui `src/ui/config_picker.rs`.
- The full pre-TUI startup sequence wired in `src/tui/**`: config-path resolution
  (`--config` / discovery / picker) → `Config::load` → password resolution →
  `bootstrap` → main `Program`.
- Wiring through the `edaptor-tv` dev binary so the flow is live-testable **now**.
  `main.rs` adopts the same entry point at the M5b cutover (a one-line swap then).

**Out of scope (→ M5b):**
- Cutover mechanics (rename/delete/drop-deps), parity-copy dedup.
- X-ORDERED editing, last-member client-side pre-validation, sambaDomain discovery.
- Status-line / menu / mouse polish beyond what the picker itself needs.

## 3. The startup sequence (ordering is load-bearing)

The order is forced by two constraints: (a) config-path resolution needs **no**
LDAP connection and must precede everything; (b) `PasswordSource::Prompt` calls
`rpassword::prompt_password`, which reads from the **real terminal** and must run
**outside** any tvision alt-screen.

1. **Resolve config path.** `--config <p>` wins outright. Otherwise
   `config::discovery::discover_configs()`:
   - `0` candidates → error (`no config found … use --config`), exit non-zero.
   - `1` candidate → use it (no picker).
   - `>1` candidates → run the **picker Program** (§4). Cancel → clean exit (`Ok`).
2. **`Config::load(path)`** — parse the chosen file.
3. **Resolve password.** `config.auth.needs_password()` → `password_source.resolve()`.
   For `PasswordSource::Prompt` this prompts on the clean terminal. This step runs
   *after* the picker Program has fully torn down (terminal restored) and *before*
   the main `Program` is constructed.
4. **`bootstrap(config, password)`** — LDAP connect + schema fetch (unchanged).
5. **Main `Program` / `run_app`** — unchanged (`app::build_program` + dispatch).

Failures at 2/3/4 bubble up as `Result` (today's behaviour) — no connecting
dialog (YAGNI).

## 4. The picker as a separate, short-lived `Program`

The main `Program` is built from an **already-bootstrapped** `Shared` state
(`app::build_program` inserts the panes from live data), so the picker cannot be a
modal inside it — there is no connection yet. The picker therefore runs in its
**own minimal `Program`** that exec-views the picker `Dialog`, captures the chosen
path, and tears down — restoring the terminal — before step 3's password prompt.
**Two sequential `Program` lifetimes**; each owns its own `CrosstermBackend` and
enters/leaves the alt-screen on construct/drop.

`run_config_picker(candidates: Vec<ConfigCandidate>) -> Result<Option<PathBuf>>`:
- Builds a `CrosstermBackend` + a minimal `Program` (empty desktop; the picker is
  exec-viewed, not inserted as the window).
- Mechanics mirror the established in-app dialog-exec pattern (`app::dispatch`):
  the only `exec_view` site is inside `run_app`'s `(prog, cmd)` closure. A
  zero-area pump view posts a one-shot `SHOW_PICKER` command on its first timer
  tick (exactly how the main app posts `FULLSCREEN` once); the closure catches it,
  `exec_view`s the dialog, reads the staged selection, and ends the program via
  `prog.end_modal(Command::QUIT)`.
- Returns `Some(path)` on `OK`, `None` on `CANCEL`/window-close.

The picker selection is **staged into the dialog's own small shared cell** on
selection-change and on open, then read after `exec_view` returns `OK` — the same
idiom `dialog::profile_chooser` uses (`reset_current` seeds index 0; `handle_event`
updates it after nav). The picker uses a private `Rc<RefCell<…>>`, **not** the app
`UiState` (which does not exist yet at this point).

## 5. The picker `Dialog` (ListBox + detail pane)

Built like `dialog::profile_chooser` / `oc_picker`: a `Dialog` wrapping a
`#[delegate(to = dlg)]` view, centered, title `"Select configuration"`.

- **`ListBox`** of candidate **display-names** (`ConfigCandidate::display_name()`).
- **Detail area** below the list: two read-only cells (the `ro_cell` disabled-
  `InputLine` idiom from `panes/form.rs` / `pw_editor.rs`) showing the **highlighted**
  candidate's `meta.description` and **full path**. Updated on selection-change via
  the `ListBox::value()`-diff idiom (the leaf pane's pattern; `StaticText` has no
  `set_value`, hence `ro_cell`).
- **Buttons:** `~O~K` (default) / `~C~ancel`, `Command::OK` / `Command::CANCEL`,
  right-aligned — so `exec_view` returns which was pressed.
- **Keys:** Up/Down navigate the list; Enter = OK (confirm highlighted); Esc =
  Cancel. Consistent with the M4 dialog convention (Enter confirms OK).

Layout sizing follows `profile_chooser` (list rows clamped to a sane range; height
= frame + list + detail + buttons).

## 6. Module layout

- **New `src/tui/startup.rs`** — owns the sequence:
  - `resolve_config_path(cli_config: Option<PathBuf>) -> Result<Option<PathBuf>>`
    (discovery + 0/1/many branch + calls `run_config_picker`). `Ok(None)` = user
    cancelled → caller exits cleanly.
  - `run_config_picker(candidates) -> Result<Option<PathBuf>>` (the short-lived
    Program, §4).
- **New `src/tui/dialog/config_picker.rs`** — the picker `Dialog` view (§5), a
  sibling of `profile_chooser.rs`. `pub(crate)`; registered in `dialog/mod.rs`.
- **`src/bin/edaptor-tv.rs`** — call `startup::resolve_config_path(cli_config)`
  instead of its current hardcoded-default path resolution, so discovery + picker
  are exercised live now. Password stays from `EDAPTOR_TEST_ADMIN_PW` in the dev
  binary (the dev binary keeps its env shortcut; the real `Prompt`/env/command
  `password_source.resolve()` path is what `main.rs` adopts at cutover).
- **Facade boundary preserved:** all tvision code stays under `src/tui/**`; the
  domain `config::discovery` module (already UI-agnostic) is reused unchanged.

## 7. Testing

- **Discovery:** already unit-tested (`config::discovery`) — untouched.
- **Path resolution:** unit-test `resolve_config_path` branching (0 → err, 1 →
  that path, explicit `--config` short-circuits discovery) with the picker
  abstracted/injected so no TTY is needed.
- **Picker `Dialog`:** headless DRAW/event tests per the M4 dialog pattern
  (`Context::new` + `TimerQueue` + `Vec<Deferred>`): selection-change updates the
  detail cells to the highlighted candidate's description + path; the staged index
  tracks the highlight; OK yields the highlighted path, Cancel yields `None`. A
  build-smoke test in `dialog/mod.rs` (mirrors the existing ones) guards against
  dead-code under clippy `-D warnings`.
- **Live tmux acceptance:** point `edaptor-tv` at a temp dir containing two
  `*.toml` configs (via `XDG_CONFIG_HOME`), confirm the picker appears, arrows move
  the highlight + detail pane, Enter loads the chosen config into the TUI, Esc
  exits cleanly. Single-config and `--config` paths skip the picker.

## 8. Risks

- **Two sequential `Program`s on one terminal.** Each `CrosstermBackend` must
  cleanly enter/leave the alt-screen so the `rpassword` prompt lands on a normal
  terminal between them. Verified live in the tmux acceptance pass.
- **`exec_view`-from-`run_app` constraint.** The picker reuses the proven
  post-command → `exec_view` mechanism; no new framework surface.

## 9. Deferred to M5b — notes captured here so they survive

- **Last-member pre-validation must be schema-aware, not blanket.** Removing the
  last member is only illegal when the membership attribute is **MUST** for the
  entry's objectClasses: `groupOfNames`/`groupOfUniqueNames` (`member` /
  `uniqueMember` MUST) → empty group is an `objectClassViolation`, server rejects.
  `posixGroup` (`memberUid` MAY) → empty group is **legal**; removing the last
  member must **not** be blocked. The pre-validation needs the entry's
  objectClasses + the subschema MUST/MAY for the member attr (both already in the
  form/schema layer). It is a friendlier-earlier error, not a new restriction.
- X-ORDERED editing (`{n}` strip/reconstruct + routing arm) and sambaDomain
  discovery remain as the umbrella/handover describe them.

## 10. Acceptance (M5a)

- `make check` green (fmt + clippy `-D warnings` + tests); facade guards clean.
- Discovery with >1 config shows the tvision picker (live tmux); Enter loads the
  selection; Esc exits cleanly; single-config and `--config` skip the picker.
- The ratatui `config_picker.rs` is **not** touched (it dies at the M5b cutover).
- `CHANGES.md` updated; no doc claims about the cutover yet.
