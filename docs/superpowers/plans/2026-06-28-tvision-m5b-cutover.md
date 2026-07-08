# M5b — Cutover Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Collapse the dual-UI tree to a single tvision UI — the `edaptor` binary runs the tvision UI, `src/ui/` *is* the tvision code (renamed from `src/tui/`), the ratatui tree + `edaptor-tv` + `ratatui`/`tui-*`/`crossterm` deps are gone, and `make check` is green.

**Architecture:** A **refactor, not a feature** — no behavior changes, so the existing 644-test suite + `cargo build --all-targets` are the safety net (green-throughout discipline, not red→green TDD). Done in four compiler-guided stages, each its own task and each leaving `make check` green and bisectable: (1) rewire `main.rs` to the tvision UI while both trees still exist; (2) delete the ratatui tree + dev binary + deps; (3) `git mv src/tui src/ui` + fix references; (4) facade guards + docs + live acceptance. A pure rename cannot come first because the target `src/ui/` is occupied by ratatui until it is deleted.

**Tech Stack:** Rust, tvision-rs 0.3, cargo.

**Spec:** [`2026-06-28-tvision-m5b-cutover-design.md`](../specs/2026-06-28-tvision-m5b-cutover-design.md)

## Global Constraints

- **Cap parallelism at 4 cores** (`cargo build -j4`, `cargo test -j4`, `cargo clippy -j4`). Cargo target dir is `/home/oetiker/scratch/cargo-target`; built binaries are under `/home/oetiker/scratch/cargo-target/debug/` (NOT `./target`).
- **This is a refactor:** preserve behavior. No new features, no logic changes. The neutral domain layer (`config`, `form`, `ldap`, `schema`, `samba`, `workflows`, the top-level `app`) is NOT touched except the two facade-purity doc-comments named in Task 3.
- **Do NOT touch `src/app.rs`** (`pub mod app;`) — it is neutral domain logic, unrelated to the ratatui `ui::app`; leave it (it stays `pub`, so it does not warn even if unused).
- **Keep `unicode-width`** — it is used by neutral `src/config/tree_label.rs`. Only `ratatui`, `tui-tree-widget`, `tui-prompts`, `crossterm` are dropped.
- After every task: `cargo fmt --check`, `cargo clippy -j4 --all-targets -- -D warnings` clean, `cargo test -j4` green.
- **Commit trailer:** `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. Use `git commit -F` (heredoc) for messages with backticks.
- The work is on the unmerged `feat/tvision-ui` branch (do NOT branch/worktree). The ratatui deletion is the point of no return but is preceded by Task 1 (tvision already drives `edaptor`) and revertable on-branch.

---

## File Structure (what changes, by task)

- **Task 1:** `src/main.rs` — entry rewired to `edaptor::tui::{run, startup::resolve_config_path}`.
- **Task 2:** delete `src/ui/**` (16 files) + `src/bin/edaptor-tv.rs`; `Cargo.toml` (drop `[[bin]] edaptor-tv` + 4 deps); `src/lib.rs` (remove `pub mod ui;`).
- **Task 3:** `git mv src/tui src/ui`; `src/lib.rs` (`pub mod tui;`→`pub mod ui;`); `src/main.rs` (`edaptor::tui::`→`edaptor::ui::`); `crate::tui::`→`crate::ui::` inside the moved tree; doc-comments in `src/workflows/{pick_state.rs,search_flow.rs}`.
- **Task 4:** `docs/HANDOVER.md` (facade guards + M5b-done banner), `CLAUDE.md`, `README.md`, `CHANGES.md`, `docs/src/**` (mdBook scan), doc-comments in `src/workflows/{pick_state.rs,edit_form.rs}`.

---

## Task 1: Rewire `main.rs` to the tvision UI

**Files:**
- Modify: `src/main.rs` (the config-path resolution block ~lines 49-64; `run_tui` body ~line 120)

**Interfaces:**
- Consumes: `edaptor::tui::run(config: Config, password: String) -> anyhow::Result<()>` (the tvision entry, `src/tui/mod.rs`); `edaptor::tui::startup::resolve_config_path(cli_config: Option<PathBuf>) -> anyhow::Result<Option<PathBuf>>` (M5a; `Ok(None)` = user cancelled picker → clean exit).
- Produces: the `edaptor` binary now launches the tvision UI. The ratatui `edaptor::ui::*` becomes unused (still compiles — its items are `pub`).

- [ ] **Step 1: Replace the config-path resolution block**

In `src/main.rs`, the current block is:

```rust
    let Cli { config, command } = cli;
    let config_path: PathBuf = if let Some(p) = config {
        p
    } else {
        let candidates = edaptor::config::discovery::discover_configs();
        match candidates.len() {
            0 => anyhow::bail!(
                "no config found in ~/.config/edaptor/ or /etc/edaptor/; \
                 use --config to specify one"
            ),
            1 => candidates.into_iter().next().unwrap().path,
            _ => match edaptor::ui::config_picker::pick_config(candidates)? {
                Some(p) => p,
                None => return Ok(()),
            },
        }
    };
```

Replace it with (the M5a `resolve_config_path` encapsulates the exact 0/1/many logic, using the tvision picker):

```rust
    let Cli { config, command } = cli;
    let config_path: PathBuf = match edaptor::tui::startup::resolve_config_path(config)? {
        Some(p) => p,
        None => return Ok(()), // user cancelled the config picker
    };
```

- [ ] **Step 2: Point `run_tui` at the tvision UI**

Change the `run_tui` body from:

```rust
fn run_tui(config: Config, password: String) -> Result<()> {
    edaptor::ui::app::run(config, password)
}
```

to:

```rust
fn run_tui(config: Config, password: String) -> Result<()> {
    edaptor::tui::run(config, password)
}
```

Also update its doc-comment if it says "ratatui" (change "three-pane ratatui TUI" → "three-pane tvision TUI"; the `edaptor::ui::app` reference becomes `edaptor::tui`).

- [ ] **Step 3: Build and gate**

Run:
```bash
cargo build -j4 --bin edaptor 2>&1 | tail -5
cargo fmt --check && cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -5
cargo test -j4 --lib 2>&1 | grep "test result" | tail -1
```
Expected: builds clean; clippy clean (ratatui `ui` is now unused but `pub`, so no dead-code warning); 644 lib tests pass.

- [ ] **Step 4: Live smoke — `edaptor` launches the tvision UI**

```bash
scripts/test-ldap.sh start
tmux kill-session -t edcut 2>/dev/null
tmux new-session -d -s edcut -x 210 -y 50
tmux send-keys -t edcut 'export EDAPTOR_TEST_ADMIN_PW=adminpassword' Enter
tmux send-keys -t edcut '/home/oetiker/scratch/cargo-target/debug/edaptor --config examples/demo-config.toml' Enter
sleep 4
tmux capture-pane -t edcut -p | sed -n '1,6p'   # expect the tvision three-pane UI (tree | leaf | form), NOT ratatui
tmux send-keys -t edcut M-x ; sleep 0.5          # Alt-X quit (clean form → exits)
tmux kill-session -t edcut 2>/dev/null
```
Expected: the capture shows the tvision UI chrome (the frameless full-screen three-pane layout with the menu bar / status line), confirming `edaptor` (not `edaptor-tv`) now runs tvision. Paste the capture into the report.

- [ ] **Step 5: Commit**

```bash
git add src/main.rs
git commit -F - <<'EOF'
feat(cutover): point the edaptor binary at the tvision UI

run_tui now calls edaptor::tui::run, and config-path resolution goes
through edaptor::tui::startup::resolve_config_path (the M5a tvision
picker), replacing the ratatui ui::app::run + ui::config_picker path.
The ratatui tree is now unused (deleted in the next task).

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

## Task 2: Delete the ratatui tree + dev binary + drop deps

**Files:**
- Delete: `src/ui/` (whole tree, 16 files); `src/bin/edaptor-tv.rs`
- Modify: `src/lib.rs` (remove `pub mod ui;`); `Cargo.toml` (remove `[[bin]] edaptor-tv`; drop 4 deps)

**Interfaces:**
- Consumes: nothing new. After Task 1, `main.rs` no longer references `edaptor::ui::*`.
- Produces: a tree where only the tvision UI (`src/tui/`) remains, driven by `main.rs`; deps trimmed.

- [ ] **Step 1: Confirm nothing outside `src/ui` still imports ratatui/tui_/crossterm**

Run (each must print nothing):
```bash
! grep -rl "use ratatui\|use tui_" src | grep -vE "^src/ui/"
! grep -rln "use crossterm\|crossterm::" src | grep -vE "^src/ui/"
! grep -rn "edaptor::ui::\|crate::ui::" src/main.rs src/lib.rs
```
Expected: all empty (Task 1 removed the last `main.rs` use of `edaptor::ui::`). If any print, STOP and report — the deletion would break the build.

- [ ] **Step 2: Delete the ratatui tree and the dev binary**

```bash
git rm -r src/ui
git rm src/bin/edaptor-tv.rs
```

- [ ] **Step 3: Remove `pub mod ui;` from `src/lib.rs`**

In `src/lib.rs`, the module block is:
```rust
pub mod app;
pub mod config;
pub mod form;
pub mod ldap;
pub mod passwd;
pub mod samba;
pub mod schema;
pub mod testdata;
pub mod tui;
pub mod ui;
pub mod workflows;
```
Delete the `pub mod ui;` line (leave `pub mod tui;` — renamed in Task 3). Also fix the crate doc-comment at the top (`//! … the ratatui UI adds the three-pane browser …`) to say tvision.

- [ ] **Step 4: Remove the `edaptor-tv` bin target and drop the deps from `Cargo.toml`**

Delete this block from `Cargo.toml`:
```toml
[[bin]]
name = "edaptor-tv"
path = "src/bin/edaptor-tv.rs"
```

In `[dependencies]`, delete these four lines (and tidy the now-stale comments above them — the "TUI stack (ratatui …)" and "tvision-rs UI (migration target). During M1-M4 …" comments):
```toml
ratatui = "0.30"
tui-tree-widget = "0.24"
tui-prompts = "0.6"
crossterm = "0.29"
```
Leave `unicode-width = "0.2"` and `tvision-rs = "0.3"`. Update the `tvision-rs` comment to drop the "migration target / M1-M4 / dev binary" framing (it is now THE UI).

- [ ] **Step 5: Build, gate, and confirm deps are gone**

```bash
cargo build -j4 --all-targets 2>&1 | tail -8
cargo fmt --check && cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -5
cargo test -j4 2>&1 | grep "test result" | tail -3
grep -nE "^(ratatui|tui-tree-widget|tui-prompts|crossterm) " Cargo.toml || echo "DEPS_DROPPED"
```
Expected: builds clean (no `edaptor-tv`, no `src/ui`); clippy clean; lib + gated tests pass (gated live skip without `EDAPTOR_TEST_LDAP_URI`); `DEPS_DROPPED` printed. If the build fails on a missing `crossterm`, a direct use was missed — report it (do not re-add blindly).

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -F - <<'EOF'
feat(cutover): delete the ratatui UI, edaptor-tv, and the ratatui deps

Remove src/ui (the ratatui three-pane UI, ~9.4k LOC) and the edaptor-tv
dev binary now that the edaptor binary runs tvision. Drop the
ratatui/tui-tree-widget/tui-prompts/crossterm dependencies (crossterm is
pulled transitively by tvision-rs; no direct use remains). lib.rs no
longer declares `pub mod ui`.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

## Task 3: Rename `src/tui/` → `src/ui/`

**Files:**
- Rename: `src/tui/` → `src/ui/` (whole tree incl. `dialog/`, `panes/`)
- Modify: `src/lib.rs` (`pub mod tui;`→`pub mod ui;`); `src/main.rs` (`edaptor::tui::`→`edaptor::ui::`); every `crate::tui::` inside the moved tree; `src/workflows/pick_state.rs:5` + `src/workflows/search_flow.rs:21` (facade-purity doc-comments)

**Interfaces:**
- Consumes: the single tvision tree from Task 2.
- Produces: the tvision UI now lives at `crate::ui` / `edaptor::ui`; `crate::tui` no longer exists.

- [ ] **Step 1: Move the tree**

```bash
git mv src/tui src/ui
```

- [ ] **Step 2: Rename the module in `src/lib.rs`**

Change `pub mod tui;` to `pub mod ui;` in `src/lib.rs`.

- [ ] **Step 3: Update all `crate::tui::` and `edaptor::tui::` references**

Rewrite intra-crate references (inside the moved tree) and the binary reference:
```bash
grep -rl "crate::tui" src/ui | xargs sed -i 's/crate::tui/crate::ui/g'
sed -i 's/edaptor::tui/edaptor::ui/g' src/main.rs
```
Then verify nothing stale remains anywhere:
```bash
grep -rn "crate::tui\b\|edaptor::tui\b\|mod tui\b\|::tui::" src | grep -v "//.*tui" || echo "NO_STALE_TUI_PATHS"
```
Expected: `NO_STALE_TUI_PATHS` (the only remaining `tui` mentions should be in prose comments, handled next).

- [ ] **Step 4: Fix the facade-purity doc-comments in the neutral files**

In `src/workflows/pick_state.rs` (line ~5) and `src/workflows/search_flow.rs` (line ~21), the comments assert no UI dependency and name both module paths, e.g.:
```rust
//! No ratatui, no tui_*, no tvision_rs, no crate::ui, no crate::tui.
```
There is now only one UI module (`crate::ui`). Reword each to drop the dead `crate::tui` reference, e.g.:
```rust
//! No tvision_rs, no crate::ui — pure domain logic.
```
(Match each file's existing wording; just remove the `crate::tui` clause and keep the assertion accurate.)

- [ ] **Step 5: Build, gate, and check the new facade guard**

```bash
cargo build -j4 --all-targets 2>&1 | tail -8
cargo fmt --check && cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -5
cargo test -j4 2>&1 | grep "test result" | tail -3
# New single-UI facade guards (must print nothing):
! grep -rl "use tvision_rs" src | grep -vE "^src/ui/"
! grep -rl "use ratatui\|use tui_" src
```
Expected: builds clean; clippy clean; tests pass; both guards print nothing (tvision only under `src/ui/`; zero ratatui/tui_ anywhere).

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -F - <<'EOF'
feat(cutover): rename src/tui -> src/ui (tvision is now the only UI)

git mv the tvision tree to src/ui; lib.rs declares `pub mod ui`; update
crate::tui -> crate::ui inside the tree and edaptor::tui -> edaptor::ui in
main.rs; drop the dead `crate::tui` mention from the workflows facade
doc-comments. Single-UI facade: tvision_rs lives only under src/ui.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

## Task 4: Facade guards + docs + live acceptance

**Files:**
- Modify: `docs/HANDOVER.md`, `CLAUDE.md`, `README.md`, `CHANGES.md`, `docs/src/**` (mdBook, as found), `src/workflows/pick_state.rs` (lines ~3,7), `src/workflows/edit_form.rs` (lines ~4,44)

**Interfaces:**
- Consumes: the single-UI tree from Task 3.
- Produces: docs + guards reflect the single tvision UI; the milestone is closed.

- [ ] **Step 1: Update the facade guards in `docs/HANDOVER.md`**

Find the two guard commands (the "Facade guards (must print nothing)" block, ~lines 466-467):
```bash
! grep -rl "use tvision_rs" src | grep -vE "^src/(tui/|bin/edaptor-tv.rs)"
! grep -rl "use ratatui\|use tui_" src | grep -vE "^src/ui/"
```
Replace with the single-UI guards:
```bash
! grep -rl "use tvision_rs" src | grep -vE "^src/ui/"
! grep -rl "use ratatui\|use tui_" src
```
Update the surrounding prose (the facade-boundary explanation) to: tvision lives only under `src/ui/`; ratatui/tui_* must not appear anywhere.

- [ ] **Step 2: Update `CLAUDE.md`**

Line 3 reads:
```
A schema-driven TUI LDAP editor (Rust + ratatui). Read this before touching the repo.
```
Change `Rust + ratatui` → `Rust + tvision-rs`. (Grep `CLAUDE.md` for any other `ratatui` / `edaptor-tv` / "two UIs" mention and fix; there should be none beyond line 3.)

- [ ] **Step 3: Update `README.md`**

- Line ~5: `built in Rust with [ratatui](https://ratatui.rs/).` → reference tvision-rs (`built in Rust with [tvision-rs](https://github.com/oetiker/tvision-rs).`).
- Line ~26: `implemented on a three-pane ratatui` → `three-pane tvision`.
- Grep the rest of `README.md` for `ratatui` / `edaptor-tv` / "migration" / "preview" and correct any remaining dual-UI / preview language.

- [ ] **Step 4: Update the stale `workflows` doc-comments**

These now describe a completed deletion, not a deferred copy:
- `src/workflows/pick_state.rs:3` — `//! Framework-free parity copy of the pure logic in src/ui/picker.rs.` and line ~7 `//! Deduplication with ui::picker is deferred to M5.` → reword: it is now the sole picker-state implementation (the ratatui `ui::picker` was removed at the M5b cutover).
- `src/workflows/edit_form.rs:4` — `"there is NO TextState here (cf. the ratatui ui::edit_form, deleted at M5)"` and line ~44 `"matching the ratatui ui::edit_form baseline (dedup at M5)"` → reword to past tense (the ratatui `ui::edit_form` was deleted at the M5b cutover; this is the sole edit-form model).

After editing, gate (these are `//!`/`//` comments, so clippy/tests are unaffected, but confirm):
```bash
cargo fmt --check && cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -3
```

- [ ] **Step 5: Add the `CHANGES.md` entry**

Under the unreleased section, add (matching existing style):
```markdown
- **The TUI is now Turbo-Vision–based.** The user interface was migrated from
  ratatui to [tvision-rs](https://github.com/oetiker/tvision-rs): a frameless
  full-screen three-pane browser/editor with modal rich-field editors. The old
  ratatui implementation and the transitional `edaptor-tv` dev binary were
  removed, along with the `ratatui` / `tui-tree-widget` / `tui-prompts` /
  `crossterm` dependencies.
```

- [ ] **Step 6: Scan the mdBook for migration/preview language**

```bash
grep -rni "ratatui\|edaptor-tv\|tvision preview\|preview\|migration" docs/src/ | grep -v widgets.md
```
Correct any text that describes a dual-UI / preview state to the single-UI reality. **Leave `docs/src/configuration/widgets.md` X-ORDERED wording AS-IS** (its "stripped for display and reconstructed on save" claim is deliberately deferred to M5c per spec §4 — do not edit it in this milestone). Rebuild the book to confirm it still builds:
```bash
make docs 2>&1 | tail -5
```
Expected: the book builds without error.

- [ ] **Step 7: Update the HANDOVER banner (M5b done → M5c)**

In `docs/HANDOVER.md`, update the `▶ NEXT ACTION` banner and the "What's done" / git-topology lines: M5b (cutover) is DONE; the next milestone is **M5c — the three reconciliations** (X-ORDERED editing, schema-aware last-member pre-validation, live sambaDomain discovery; see the M5b spec §4 and the M5a spec §9 for the load-bearing facts). Note the single-UI reality (no more `src/tui`, no `edaptor-tv`; the live tmux recipe now uses the `edaptor` binary).

- [ ] **Step 8: Full gate + facade guards + live acceptance**

```bash
cargo fmt --check
cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -3
cargo test -j4 2>&1 | grep "test result" | tail -3
# single-UI facade guards (must print nothing):
! grep -rl "use tvision_rs" src | grep -vE "^src/ui/"
! grep -rl "use ratatui\|use tui_" src
# gated live tests vs the demo server:
scripts/test-ldap.sh start
EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389 EDAPTOR_TEST_ADMIN_PW=adminpassword cargo test -j4 --test tv_membership --test tv_picker 2>&1 | grep -E "test result|running" | tail -6
```
Expected: fmt/clippy/tests clean; both guards empty; the gated live tests pass vs the demo server.

Live tmux acceptance of the shipping binary (the picker path + the main UI):
```bash
cargo build -j4 --bin edaptor
PICKDIR=/tmp/claude-1003/-home-oetiker-checkouts-edaptor/65e6bbae-ad5a-48c1-ad91-59fe1c2a3693/scratchpad/edcut
mkdir -p "$PICKDIR/cfg/edaptor"
{ printf '[meta]\nname = "demo-one"\ndescription = "first"\n\n'; cat examples/demo-config.toml; } > "$PICKDIR/cfg/edaptor/one.toml"
{ printf '[meta]\nname = "demo-two"\ndescription = "second"\n\n'; cat examples/demo-config.toml; } > "$PICKDIR/cfg/edaptor/two.toml"
tmux kill-session -t edcut 2>/dev/null
tmux new-session -d -s edcut -x 210 -y 50
tmux send-keys -t edcut "export EDAPTOR_TEST_ADMIN_PW=adminpassword XDG_CONFIG_HOME=$PICKDIR/cfg" Enter
tmux send-keys -t edcut '/home/oetiker/scratch/cargo-target/debug/edaptor' Enter
sleep 3
tmux capture-pane -t edcut -p | sed -n '1,18p'   # expect the tvision "Select configuration" picker (two configs)
tmux send-keys -t edcut Enter ; sleep 4
tmux capture-pane -t edcut -p | sed -n '1,6p'     # main three-pane tvision UI from the chosen config
tmux send-keys -t edcut M-x ; sleep 0.5
tmux kill-session -t edcut 2>/dev/null
```
Expected: the picker appears (driven by the real `edaptor` binary), Enter loads the main tvision UI, Alt-X exits. Paste both captures into the report.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -F - <<'EOF'
docs(cutover): single-UI facade guards + docs; close M5b

Rewrite the facade guards (tvision only under src/ui; zero ratatui/tui_*
anywhere), update CLAUDE.md/README/CHANGES/HANDOVER and the mdBook to the
single-UI reality, and correct the now-stale "parity copy / dedup at M5"
doc-comments in workflows::{pick_state,edit_form}. Live-verified: the
edaptor binary launches the tvision UI + config picker.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

## Final gate (whole milestone)

- [ ] `make check` green (fmt + clippy `-D warnings` + tests).
- [ ] Single-UI facade guards print nothing; `src/tui/` is gone; `src/ui/` is the tvision tree.
- [ ] No `ratatui` / `tui-tree-widget` / `tui-prompts` / `crossterm` in `Cargo.toml`; no `use ratatui` / `use tui_*` in `src`.
- [ ] `edaptor-tv` gone; the `edaptor` binary launches the tvision UI (live tmux verified, incl. the config picker).
- [ ] Gated live `tv_*` tests pass vs the demo server.
- [ ] `CLAUDE.md`, `README.md`, `CHANGES.md`, `HANDOVER.md`, mdBook reflect the single UI; `widgets.md` X-ORDERED wording intentionally left for M5c.

## Self-review notes (coverage check against the spec)

- Spec §2 Stage 1 (rewire main.rs): Task 1. ✓
- Spec §2 Stage 2 (delete tree + bin + deps, incl. crossterm): Task 2. ✓
- Spec §2 Stage 3 (rename + reference updates + neutral doc-comments): Task 3. ✓
- Spec §2 Stage 4 (facade guards + CLAUDE/README/CHANGES/HANDOVER/mdBook + dedup doc-comments): Task 4. ✓
- Spec §3 (parity-copy dedup = automatic + doc-comment cleanup): Task 2 (deletion) + Task 4 (comments). ✓
- Spec §4 (X-ORDERED widgets.md left as-is): Task 4 Step 6 explicitly skips it. ✓
- Spec §5 (gate + gated live + live tmux of `edaptor`): Task 1 Step 4 + Task 4 Step 8. ✓
- Spec §6 risks (rename breakage backstop = `cargo build --all-targets`): Tasks 2-3 Step 5. ✓
- Spec §7 acceptance: Final gate. ✓
