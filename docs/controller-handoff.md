# Controller Handoff — edaptor: form-pane scroll rework + the release-only footer bug

> Starter pack for the next controller session. This handoff lives in ONE
> worktree — run `git worktree list` first and confirm this is the workstream
> you're resuming. Read this first, then `git log <handoff-commit>..HEAD` for
> everything that changed since. Detail is NOT here — it's in git + the PRs (§6).
> Before you rewrite this file at your own handoff: read the previous version
> (`git show HEAD:docs/controller-handoff.md`) and carry forward any lesson in
> §4/§5 that is still true. Fresh synthesis, not blank page. On merge into
> another branch, rewrite that branch's handoff to the merged reality.

Handoff commit: 130533c (branch `fix/form-block-scroll-inputs`)   Date: 2026-07-26   Reason: milestone before rollover
Worktree / branch: main checkout `/home/oetiker/checkouts/edaptor` @ `fix/form-block-scroll-inputs`
Sibling worktrees: none. tvision-rs source sits at `/home/oetiker/checkouts/rstv` (sibling repo, **0.13.1**, NOT a worktree — see §4 version-mismatch trap). `main` is at `b5c6d52 Release v1.5.1`.

## 1. Mission
edaptor is a schema-driven TUI LDAP editor on **tvision-rs 0.14.0**. This session
fixed the **entry form pane (panel 3) scrolling**, driven by live user testing.
Two shipped pieces plus one big unfinished discovery:
- **PR #7 (MERGED → Release v1.5.1):** read-only fields at the form edges scroll
  into view; the main window no longer sits a row too low overlapping the footer
  (`win_rect` was screen-absolute but is applied desktop-**local**).
- **PR #8 (OPEN):** a proper decoupled viewport-scroll model for the form — wheel,
  PageUp/Down, scrollbar drag all move the viewport directly (independent of which
  field is focused), the scrollbar thumb tracks the viewport across the full range,
  and you can always scroll back up from the bottom.
- **UNFINISHED:** the "footer one row too high / overwrites the bottom row" bug on
  carbo is a **release-build-only** rendering race in tvision-rs (§4). Not fixed.

## 2. Where we are now
- **Branch `fix/form-block-scroll-inputs` @ 130533c, PR #8 OPEN, green.** 4 commits
  on top of `Release v1.5.1`: (1) drive tall-block scroll from all inputs; (2)
  decouple viewport scrolling from focus; (3) scroll-bar thumb reaches the bottom.
  `make check` green (898 lib tests + integration, clippy `-D warnings`, fmt).
- **Working tree clean.** Local release binary rebuilt = sha `5eb6a3a9…`.
- **carbo (`ds-carbo-feh-adm`) has the PR #8 branch build installed for testing**
  (`/usr/local/bin/edaptor` = `5eb6a3a9…`, still *labeled* 1.5.1 — no `--version`).
  Backups: `edaptor.bak-2026-07-26` (the old 1.5.0), `edaptor.bak-2026-07-26-213858`
  (the released 1.5.1). Config untouched: `/etc/edaptor/ds-carbo-feh.toml`.
- **User is evaluating PR #8 scroll behaviour on carbo.** Navigation confirmed
  "much better". The footer bug (§4) is cosmetic-ish and does NOT block that eval.
- The demo LDAP podman container was left running locally (`scripts/test-ldap.sh`).

## 3. Do this next
1. **Await the user's PR #8 verdict on carbo.** If good → merge PR #8, cut **1.5.2**
   via the release workflow (bugfix), then redeploy the *released* build to carbo
   (recipe in §4). Confirm before ssh.
2. **The footer bug is the real open problem** (§4, §7). It affects EVERY release
   deployment. Next concrete move: wrap `CrosstermBackend` in a logging `Backend`
   in edaptor (`src/ui/mod.rs` `run()`) that logs `size()` on every call, run the
   RELEASE binary under tmux, and catch the transient `h-1` read. Then fix in
   tvision-rs (mind the version-mismatch trap in §4).
3. Keep `CHANGES.md` Unreleased in sync (PR #8 already added its entry).

## 4. Lessons & traps  ← the irreplaceable part
- **THE BIG ONE — the carbo "footer one row too high" bug is RELEASE-vs-DEBUG.**
  Same terminal, same pane, same source: `cargo run` (debug) renders the footer on
  the last row (correct); the **release** binary renders it one row too high, last
  row unused. This is why it "works locally" (you run `cargo run` = debug) but is
  broken on carbo (release binary). The earlier "window one row too high" carbo
  report was THIS same bug — I wrongly wrote it off as a stale session and "fixed"
  an unrelated `win_rect` issue. edaptor's layout is provably correct (headless is
  right at ALL heights; debug build is right). `backend.size()` reads correctly
  (`(80,25)`) at build, but in the release build the perceived height drops to `h-1`
  after alt-screen/raw-mode entry (a startup timing race), so the status line ends
  at `h-2`. **The bug is in the tvision-rs backend/pump/render layer, NOT edaptor
  layout and NOT the scroll work.** Not yet root-caused to a line.
- **Headless probes DO NOT catch release-render-timing bugs.** `HeadlessBackend` is
  correct at every height and for resize; the footer bug only shows in a real
  terminal release build. **Verify TUI layout with the actual RELEASE binary in a
  real terminal (tmux), not just headless snapshots.** I burned a lot of time
  trusting headless "correct" while the release binary was wrong.
- **tvision-rs VERSION MISMATCH:** edaptor pins published **0.14.0**; the local
  `../rstv` checkout is **0.13.1** (OLDER). Do NOT assume `../rstv` == what edaptor
  builds. Read the real 0.14.0 source from the cargo cache:
  `~/.cargo/registry/src/index.crates.io-*/tvision-rs-0.14.0/`. For a framework fix
  you must bring `../rstv` up to 0.14.0 first (or work a path-dep carefully), then
  release + bump — see rstv release hygiene below.
- **The form scroll model was REWORKED to a decoupled viewport scroll** (`ScrollGroup`):
  a `scroll_locked` flag (set by `scroll_viewport`, cleared by `focus_child`)
  suppresses the per-event `ensure_focused_visible` so a deliberate scroll isn't
  snapped back to the focused field. Wheel + PageUp/Down call `scroll_viewport`
  (checked BEFORE the List-field branch so a focused inline List can't swallow the
  page keys — that was the "no getting back up" bug). Arrows still navigate fields /
  scroll a focused tall block via `scroll_focus_region_edge` → `scroll_edge` (which
  uses `focus_region`, extending the focused field's keep-visible span across a
  read-only head/tail). The old launch-only `scroll_block_edge` special case was
  DELETED. **This supersedes the prior handoff's "don't add a caret to a read-only
  block" note** — still true, but the whole scroll path is different now.
- **Scroll-bar is VIEWPORT semantics now, not listbox.** `publish_bar` value = `top`,
  and **max MUST be `max_top` (= content_height − viewport_h), NOT content_height−1**
  — else the thumb stops ~2/3 down at the bottom (the thumb position is
  `value/max`). Drag (`apply_scroll_sync`) treats the value as a `top` and scrolls
  there directly + locks.
- **`win_rect` must be desktop-LOCAL** (`src/ui/app.rs` `init_desktop`): the window
  is a desktop child, so `(0, 0, width, height)` — reusing the screen-absolute
  `r.a.y`/`r.b.y` shifts it one row down. Real fix (PR #7), but it does NOT cure the
  carbo footer bug (that's the release-timing thing above).
- **carbo deploy recipe:** cargo target dir is redirected to
  `/home/oetiker/scratch/cargo-target` (NOT `./target`) — `sha256sum` it. edaptor has
  **no `--version`** (confirm deploys by sha + `.bak` timestamp). Build release
  locally (this box glibc 2.39 → carbo Ubuntu glibc 2.43; older→newer safe), `scp`
  to `ds-carbo-feh-adm:/tmp/edaptor`, then `ssh ds-carbo-feh-adm` (passwordless
  sudo): back up `/usr/local/bin/edaptor` to a **timestamped** `.bak-$(date +%F-%H%M%S)`
  (a plain dated `.bak` clobbers same-day backups), `install -m0755`, verify sha.
- **rstv release hygiene:** `.github/workflows/release.yml` OWNS version bumps +
  CHANGELOG rollover. A feature branch adds notes only under `## Unreleased`, never
  bumps `Cargo.toml`/tag. edaptor's release is the same (`workflow_dispatch`,
  bugfix/feature/major). We (correctly this time) did NOT bump versions on the
  branches; PR #7 became 1.5.1 via the release commit on `main`.
- **tvision facts (0.14):** timers are synchronous via `Context::set_timer`; the
  pump fires them on IDLE passes only, so a headless test must idle-pump (~300ms real
  time, not an in-loop capture spin) to trigger the one-shot fullscreen flip
  (`PumpView` posts `Command::FULLSCREEN`). `apply_fullscreen` re-fits menu/desktop/
  window on resize but does NOT touch the status line (it relies on the status line's
  grow_mode `lo_y+hi_y` to stick to the bottom — which IS correct).
- **tmux live-testing gotchas:** `cargo run` = DEBUG build (masks the footer bug!).
  `tmux set -t <s> status off` for clean row counts. SGR mouse click:
  `tmux send-keys -t <s> -l $'\033[<0;COL;ROWM\033[<0;COL;ROWm'` (1-based col/row).
  Wheel: SGR button 65 = down, 64 = up (`$'\033[<65;COL;ROWM'`). Real time must pass
  between launch and capture — do it across SEPARATE tool calls, not an in-shell loop
  of `capture-pane` (those are instant, no real delay), or clicks land before the app
  is ready and echo as literal `^[[<…` text.

## 5. Don'ts & constraints
- **≤ 4 cores** for all compiles/tests (shared 128-core box). `podman` not docker;
  `pnpm` not npm.
- **Confirm before ssh / carbo redeploy** — production, outward-facing; back up the
  old binary first (timestamped `.bak`).
- **Don't push/merge/redeploy without the user's explicit go-ahead.**
- **oetiker rejects half-done scroll features** — a scroll container must drive
  scrollbar + wheel + PageUp/Down + arrows + drag uniformly, not just arrows. He
  pushed back three times ("took some short cuts") before the decoupled model landed.
- **Settled — do not relitigate:** the form scroll model is decoupled viewport scroll
  (§4); the scroll bar is viewport semantics; `win_rect` is desktop-local.
- **Config model (unchanged):** rich editors are `[profile.widget.<attr>]` with a
  `kind`; do NOT reintroduce `[profile.picker.<attr>]` / `[profile.password]`.

## 6. Where the detail lives
- Change history: `git log b5c6d52..HEAD` (PR #8, 3 commits) and `git show f6d0ca8`
  (PR #7 merge / the 1.5.1 window+edge-scroll fixes).
- **PRs:** #7 (MERGED, `fix/form-scroll-and-window-layout`) → Release v1.5.1;
  **#8 OPEN** (`fix/form-block-scroll-inputs`). `gh pr view 8`.
- Key files: `src/ui/scroll_group.rs` (the whole scroll engine — `scroll_viewport`,
  `scroll_locked`, `focus_region`, `scroll_edge`, `ensure_focused_visible`,
  `publish_bar` viewport semantics, `apply_scroll_sync` drag); `src/ui/panes/form.rs`
  (`handle_event` routing — wheel/page/arrow branches, `scroll_region_or_focus`,
  `WHEEL_STEP`); `src/ui/app.rs` (`init_desktop` win_rect, `init_status_line`,
  `build_program`, `main_window_is_not_shifted_onto_the_footer` test); `src/ui/mod.rs`
  `run()` (where to add a size-logging backend wrapper for §7).
- Memory: `~/.claude/projects/-home-oetiker-checkouts-edaptor/memory/` — see
  `scroll-must-be-complete.md` (the "no shortcuts" feedback).

## 7. Open questions / pending decisions
- **The footer/last-row release-only bug (§4).** Root cause not pinned. Hypothesis:
  a transient `h-1` terminal-size read during alt-screen/raw-mode entry that the fast
  release build latches and doesn't recover from. Confirm with a logging backend
  wrapper, then fix in tvision-rs (version-mismatch trap applies). It affects every
  release deployment — decide whether to fix before or after shipping PR #8's scroll
  work as 1.5.2.
- **PR #8 → 1.5.2 → redeploy** pending the user's carbo eval.
- Should the local demo LDAP container be stopped? Left running.

## 8. Staleness watch
- **PR #8 may already be merged / 1.5.2 cut** — check `gh pr view 8`, `git log main`,
  `git tag`.
- **carbo's `/usr/local/bin/edaptor`** may have been redeployed — verify by sha
  (`5eb6a3a9…` = the PR #8 branch build as of this handoff) and the newest `.bak`.
- **The footer bug may have been root-caused/fixed** since — check `../rstv` status,
  edaptor's `tvision-rs` pin in `Cargo.toml`, and any new backend wrapper in
  `src/ui/mod.rs`.
- This file reflects commit 130533c, not now. `git status` + `git log 130533c..HEAD`
  before trusting.
