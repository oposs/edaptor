# Controller Handoff — edaptor batch: MaskedInput adoption + form/browser fixes

> Starter pack for the next controller session. This handoff lives in ONE
> worktree — run `git worktree list` first and confirm this is the workstream
> you're resuming. Read this first, then `git log <handoff-commit>..HEAD` for
> everything that changed since. Detail is NOT here — it's in git + the reports
> (§6). Before you rewrite this file at your own handoff: read the previous
> version (`git show HEAD:docs/controller-handoff.md`) and carry forward any
> lesson in §4/§5 that is still true. Fresh synthesis, not blank page. On merge
> into another branch, rewrite that branch's handoff to the merged reality.

Handoff commit: 9589a21 (branch `integrate/2026-07`)   Date: 2026-07-24   Reason: milestone before rollover
Worktree / branch: main checkout `/home/oetiker/checkouts/edaptor` @ integrate/2026-07
Sibling worktrees: none. rstv (tvision-rs source) sits at `/home/oetiker/checkouts/rstv` (sibling repo, not a worktree).

## 1. Mission
edaptor is a schema-driven TUI LDAP editor on **tvision-rs**. This session did two
things: (A) shipped **tvision-rs 0.14.0** — native `InputLine` password masking +
a `RevealEye` toggle + a `MaskedInput` composite (the structural cure for a class
of password bugs), now **released to crates.io**; and (B) a batch of edaptor fixes
on top of Track A (the carried-over paste/password bugfixes), all combined into
**`integrate/2026-07`** and opened as **edaptor PR #6**.

## 2. Where we are now
- **tvision-rs 0.14.0 — DONE & RELEASED.** PR #21 merged, tag v0.14.0 published.
  edaptor pins `tvision-rs = "0.14.0"`.
- **edaptor `integrate/2026-07` — PR #6 open, green, NOT merged.** It merges Track A
  + three reviewed fixes (all green together: 890 lib tests, clippy `-D warnings`,
  fmt clean):
  1. Track A (`fix/multivalue-paste`): multi-value paste + password paste/confirm/
     backspace fixes.
  2. `feat/adopt-masked-input`: delete the hand-rolled password mirror, adopt
     native `MaskedInput` (−377 lines). Supersedes Track A's password-mirror fixes.
  3. `fix/launch-value-scroll`: read-only multi-value blocks now scroll line-by-line.
  4. `fix/container-classification`: containers classified by objectClass, empty OUs
     in the tree, sub-containers in the entry pane.
- `main` was clean/synced with origin at handoff. The four sibling branches are all
  merged into `integrate/2026-07`.
- **carbo still runs plain v1.3.0 — the live panic + silent-password bugs are STILL
  in production.** Redeploy is the last pending action (needs explicit go-ahead).

## 3. Do this next
1. **Get PR #6 merged** (oposs/edaptor#6) — user reviews/merges to `main`.
2. **Redeploy carbo** from the merged binary (recipe in §4) — it has live bugs.
   Production + ssh: confirm before running.
3. Optional follow-ups (not blocking): the two tangential shuttle bugs (§7), and
   sub-container navigate-on-Enter (§7).

## 4. Lessons & traps  ← the irreplaceable part
- **The whole password-bug cluster was ONE root cause: an unhandled `Event::Paste`
  in a custom view.** tvision delivers terminal bracketed paste as `Event::Paste`
  (routed like a KeyDown), distinct from `Event::Command(Command::PASTE)` (Ctrl+V).
  A `match` on only `KeyDown` silently drops it. Now moot in the password field
  (native `MaskedInput`), but the pattern recurs — when "paste doesn't work", look
  for a view that doesn't handle `Event::Paste`.
- **Rust method-resolution trap in `MaskedInput` (carry forward for ANY tvision
  composite):** an inherent `&mut self value()` is silently shadowed by the
  `View::value(&self)` default (which returns `None`) at the dot-call site → your
  getter returns `None` and no error. Fix: make the inherent getter `&self`, and
  **explicitly override `value`/`set_value`/`set_value_ctx` in `impl View`** so the
  `#[delegate]` macro doesn't forward to `Group`'s no-op defaults. A dedicated
  round-trip-through-`Box<dyn View>` test guards it.
- **tvision facts (0.14):** timers are **synchronous** via `Context::set_timer`/
  `kill_timer` over an injected `TimerQueue` — there is **no** `Deferred::SetTimer`.
  `InputLine::SURFACE_ROLES` is **not pub**. **Theme glyphs live in `Glyphs`**
  (`reveal_eye_hidden`/`reveal_eye_revealed`); widgets read `ctx.glyphs()` — do NOT
  bake glyphs into a widget config (that was a review finding). Masked clipboard
  guard keys on `mask.is_some()` (blocks copy even when *revealed* — a deliberate
  security choice), not on `masking()`.
- **rstv release hygiene:** `.github/workflows/release.yml` OWNS version bumps +
  CHANGELOG rollover. A feature branch must ONLY add notes under `## Unreleased`
  and must NOT bump `Cargo.toml`/tag. (We tripped this and had to revert.)
- **The "form scroll" bug was NOT the shuttle** (the shuttle works). It was the
  form's read-only **`LaunchValueView`** launch block: single focus stop, passes
  Up/Down straight through, no internal scroll → tall blocks unreachable. Fix is
  **geometry-driven in the form** (`ScrollGroup::scroll_block_edge`) — the block
  exposes **no cursor** (a caret would wrongly imply editability). Don't add a caret
  to a read-only block.
- **Container classification:** a node is a container if its objectClass ∈
  {organizationalUnit, organization, dcObject, domain, container} (case-insensitive)
  OR it has children. **`is_branch` (pure has-children) is STILL load-bearing for the
  promote/demote child-count reflow — do NOT replace it.** Only tree membership
  (`branch_dns`), panel-2 exclusion, AND the `upsert` **tree-dirty signal** moved to
  container terms. The upsert-signal gap (still keyed on `is_branch` after the rest
  moved) was a real review catch: a live-created/relabelled empty OU didn't refresh
  panel 1 until a rescan. Snapshot `was_container` before mutating, like `was_branch`.
- **`.superpowers/` is now gitignored in edaptor** (it was NOT — a subagent report
  file leaked into a commit; cleaned in the integration merge). Reports are scratch.
- **carbo deploy recipe:** cargo target dir is redirected to
  `/home/oetiker/scratch/cargo-target` (NOT `./target`) — use `cargo metadata` to
  find the binary. edaptor has **no `--version`**. Build release locally (this box
  glibc 2.39 → carbo Ubuntu 26.04 glibc 2.43; older→newer is safe), `scp` to
  `ds-carbo-feh-adm:/tmp`, `ssh -t ds-carbo-feh-adm` (passwordless sudo) → back up
  `/usr/local/bin/edaptor` to a dated `.bak`, then `install -m0755`. Config:
  `/etc/edaptor/ds-carbo-feh.toml`.

## 5. Don'ts & constraints
- **≤ 4 cores** for all compiles/tests (shared 128-core box). Never two building
  subagents at once. `podman` not docker; `pnpm` not npm.
- **Confirm before ssh / carbo redeploy** — production, outward-facing; back up the
  old binary first.
- **Don't push/merge/redeploy without the user's explicit go-ahead.**
- **Settled — do not relitigate:** container rule = objectClass set OR has-children;
  panel 2 shows sub-containers (▸) + leaves; a sub-container row **opens its entry**
  (navigate-on-Enter is a *future option*, not a bug); `MaskedInput` reveal is
  **non-sticky** (Space = 1 s peek); eye glyphs are **theme-controlled**; the masked
  clipboard block covers the revealed state too.
- Don't reintroduce the removed `[profile.picker.<attr>]` / `[profile.password]`
  config layers.

## 6. Where the detail lives
- Change history: `git log 70eb910..HEAD` on `integrate/2026-07` (the whole batch).
- **edaptor PR #6** (oposs/edaptor#6); **tvision PR #21** (oposs... `oetiker/tvision-rs#21`, MERGED).
- Subagent reports (gitignored scratch): `.superpowers/adopt-masked-input-report.md`,
  `.superpowers/launch-scroll-report.md`, `.superpowers/container-classification-report.md`.
- Key files: `src/ui/pw_editor.rs` (MaskedInput adoption), `src/ui/panes/form.rs` +
  `src/ui/scroll_group.rs` (`scroll_block_edge` launch-block scroll),
  `src/workflows/structure.rs` (`is_container`, `upsert` tree-dirty signal),
  `src/ui/panes/leaf.rs` + `src/workflows/labels.rs` + `leaf_search.rs` (panel-2
  children incl. sub-containers).

## 7. Open questions / pending decisions
- Merge PR #6 + redeploy carbo — pending user.
- **Sub-container navigate-on-Enter**: selecting a sub-container row currently opens
  its entry (like `‹self›`). Navigate-the-tree-into-it was considered and left out
  (ambiguous UX, not requested). Revisit if the user wants folder-style nav.
- **Two tangential shuttle bugs** found during investigation, deliberately NOT fixed:
  (1) list scrollbar step goes stale after a dialog resize (framework: `Group`/
  `Window::on_bounds_changed` doesn't cascade to children); (2) the Available list
  resets to the top on every refresh/move (`new_list` always `reset_focus=true`).

## 8. Staleness watch
- PR #6 may already be merged — check `gh pr view 6`; if merged, `integrate/2026-07`
  and the four sibling branches may be deleted.
- carbo may already be redeployed — verify (no `--version`; check the `.bak` date
  on `ds-carbo-feh-adm`).
- `main` was clean at handoff; re-check `git log main` and `git status`.
- This file was written on the integration branch; after PR #6 merges, `main`'s
  handoff IS this file — keep it honest or rewrite to the merged reality.
