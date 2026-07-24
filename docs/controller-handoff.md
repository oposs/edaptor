# Controller Handoff — edaptor paste bugfixes + tvision password-field feature

> Starter pack for the next controller session. Read this first, then
> `git log <handoff-commit>..HEAD` for everything that changed since.
> Detail is NOT here — it's in git + the superpowers plan/spec (§6).
> Before you rewrite this file at your own handoff: read the previous
> version (`git show HEAD:docs/controller-handoff.md`) and carry forward
> any lesson in §4/§5 that is still true. Fresh synthesis, not blank page.

Handoff commit: 5b9dc8a (branch `fix/multivalue-paste`)   Date: 2026-07-24   Reason: milestone before rollover

Two repos in play:
- **edaptor**: `/home/oetiker/checkouts/edaptor` (main checkout; the feat/cache-coherence worktree was removed early this session).
- **rstv** = tvision-rs source: `/home/oetiker/checkouts/rstv` (sibling; the framework edaptor upstreams to — now noted in edaptor `CLAUDE.md`).

## 1. Mission

Two tracks. **(A) Ship paste bugfixes** found on the deployed v1.3.0: paste
didn't work in multi-value fields, pasting into the password field showed
cleartext *and* staged nothing, backspacing a pasted password crashed the app,
and the password dialog's OK accepted mismatched New/Confirm silently. All four
are fixed and committed. **(B) The structural cure**: edaptor masks passwords
with a fragile *mirror* (bullets in an inner InputLine + a parallel `real`
string). Every password bug was a mirror desync. So we're adding **native
masking + a reveal-eye to tvision-rs InputLine** (spec + plan done), after which
edaptor deletes the mirror entirely. The reveal-eye (mouse-hold peek, Space
timed/sticky, `◉`/`⊝`) is being upstreamed too.

## 2. Where we are now

- **Track A — DONE, committed on `fix/multivalue-paste`, `make check` green, NOT pushed/merged/redeployed.** Commits `e17ab87`, `0c829c2`, `b9eb793`, `63986b1` (see `git log v1.3.0..HEAD`). carbo still runs plain v1.3.0 → the panic and the silent password no-op are **live in production**.
- **Track B — spec + tvision plan written.** Spec pivoted to "enhance InputLine, not extend the mirror" (`5b9dc8a`). tvision plan (7 TDD tasks) written to `rstv/docs/superpowers/plans/2026-07-24-inputline-masking-reveal-eye.md` — **uncommitted in rstv**. Nothing implemented yet.
- rstv is on `main` @ `635a907`, clean-ish (an unrelated modified spec + an untracked PDF). No masking branch yet.

## 3. Do this next

1. **Resolve Track A logistics (asked, not answered):** (a) split the 4 bugfix commits off `fix/multivalue-paste` so they can merge/redeploy independently of the feature-planning commits; (b) **redeploy to carbo** — it has live bugs. Deploy recipe in §4.
2. **Start Track B execution** in `../rstv`: branch `feat/inputline-masking` off `main`, commit the plan, then run it task-by-task (subagent-driven-development recommended). Execution mode was asked, not answered.
3. After rstv cuts `0.14.0`, **write the edaptor adoption plan** (delete the mirror, swap in `MaskedInput`, keep the `valid()` OK-gate) against the real API.

## 4. Lessons & traps  ← the irreplaceable part

- **The whole bug cluster is ONE root cause per widget: an unhandled `Event::Paste`.** tvision delivers *two* pastes: `Event::Paste(text)` (terminal **bracketed paste** = the external clipboard, routed to the focused view like a KeyDown) and `Event::Command(Command::PASTE)` (Ctrl+V/Shift+Insert = tvision's *internal* clipboard broker, editor-only). Custom views that `match` only `KeyDown` silently drop `Event::Paste`. That was both the multi-value `ListValueView` bug and the password `MaskedInputLine` bug. When a "paste doesn't work" report comes in, look for a view that doesn't handle `Event::Paste`.
- **The password cleartext-on-paste and the backspace panic were the same mirror desync.** `MaskedInputLine` forwarded `Event::Paste` to its inner InputLine → cleartext landed in the *bullet* buffer, `real` stayed empty → (1) visible password, (2) nothing staged, (3) next Backspace indexed `real` (len 0/3) with the *bullet* caret (18/277) → `remove` out-of-bounds panic. This is the entire justification for going native: **store the secret once, mask at draw** kills the class.
- **No key-release exists** (crossterm, no Kitty protocol negotiated) → keyboard "hold to reveal, release to hide" is impossible → we use a **timed 1 s peek** (or a sticky toggle). **Mouse** hold *is* possible: `MouseDown`/`MouseUp` with `ctx.start_mouse_track(...)` (the `widgets::button` pattern; MouseUp is always delivered to the capturer).
- **Enter cannot be a reveal key** — the Dialog turns Enter into a `Command::DEFAULT` broadcast that fires OK globally. Space only.
- **tvision modal veto pattern** (how the OK-match gate works): implement `fn valid(&mut self, cmd, ctx) -> bool` on the Dialog; `Program::validate_modal_close` calls it before ending the modal and **keeps it open on `false`**; `ctx.request_message_box(text, MessageBoxKind::Error, MessageBoxButtons::ok(), None, None)` shows the error inline. Cancel/Esc must always return `true`.
- **Build/deploy facts:** cargo target dir is redirected to `/home/oetiker/scratch/cargo-target` (NOT `./target`) — use `cargo metadata` to find binaries. edaptor has **no `--version`** flag. **carbo deploy recipe:** build release locally (this box glibc 2.39 → runs on carbo's Ubuntu 26.04 glibc 2.43; older→newer is safe), `scp` to `ds-carbo-feh-adm:/tmp`, then `ssh -t ds-carbo-feh-adm` (passwordless sudo) → `sudo cp -a /usr/local/bin/edaptor <dated .bak>` then `sudo install -m0755 /tmp/... /usr/local/bin/edaptor`. Config: `/etc/edaptor/ds-carbo-feh.toml`.
- **Stale Cargo.lock in the v1.3.0 release** (said `edaptor 1.2.1` while Cargo.toml was 1.3.0); fixed in `0c829c2`.
- **tvision masking caveat baked into the plan:** InputLine scroll/cursor math is *display-column* based, so masking echoes one char per `char` and **assumes width-1 graphemes** (fine for passwords) and **skips the selection-highlight repaint while masked**. The paint hook is `input_line.rs:848` (`put_str_part`). Clipboard writes go via `ctx.set_clipboard` → `Deferred::SetClipboard` (guard these when masked).

## 5. Don'ts & constraints

- **≤ 4 cores** for all compiles/tests (shared 128-core box). `podman` not docker; `pnpm` not npm.
- **Confirm before ssh.** carbo redeploy replaces a *production* binary — always back up the old one first; it's outward-facing.
- **Don't push/merge/redeploy without the user's explicit go-ahead.**
- **Settled decisions — do not relitigate:** enhance tvision InputLine natively (not extend the mirror); **tvision-first** sequencing; upstream the reveal-eye too; glyphs `◉` revealed / `⊝` hidden; eye = its own **Tab stop**, shown **active-line only**, **inside the field's last column**; **Space = 1 s peek by default**, **sticky is a per-field config** (then Space toggles); **mouse hold = momentary**.
- The reveal-eye spec's *earlier* mirror-based version is superseded — build on native masking, not the mirror.
- Don't reintroduce the removed `[profile.picker.<attr>]` / `[profile.password]` config layers.

## 6. Where the detail lives

- Change history (bugfixes): `git log v1.3.0..fix/multivalue-paste` in edaptor.
- Spec: `docs/superpowers/specs/2026-07-23-password-reveal-eye-design.md` (edaptor).
- **tvision plan:** `docs/superpowers/plans/2026-07-24-inputline-masking-reveal-eye.md` in **rstv** (`../rstv`). Contains the full rstv research (exact line numbers/signatures) in-line.
- rstv checkout location + upstream workflow: edaptor `CLAUDE.md` "Upstream framework" section.
- Key edaptor files: `src/ui/pw_editor.rs` (password dialog + the mirror to delete; `valid()` gate lives here), `src/ui/panes/list_view.rs` (multi-value paste fix), `src/ui/app.rs:242` (modal OK dispatch), `src/ui/panes/form.rs` (list-block relayout on paste).

## 7. Open questions / pending decisions

- Split bugfix commits from feature-planning commits on `fix/multivalue-paste`? (asked)
- Redeploy fixed binary to carbo now? (live bugs — recommended yes)
- Execution mode for the rstv plan: subagent-driven vs inline? (asked)
- Publish tvision `0.14.0` to crates.io, or keep edaptor on a `path = "../rstv"` dep during adoption?

## 8. Staleness watch

- The branch commit list grows — re-check `git log v1.3.0..HEAD`.
- carbo was on **plain v1.3.0** at this handoff — verify before assuming the fixes are/aren't deployed.
- The rstv plan is **uncommitted** and rstv is on **`main`** (not a masking branch) at this handoff.
- The plan flags a few unverified crate spellings (`Deferred::SetTimer` variant, `MouseEvent` field order, whether `InputLine::SURFACE_ROLES` is `pub`) — the implementer confirms these via the compiler on the first build of each task; don't trust them as gospel.
- HANDOVER cleanup pending: edaptor `CLAUDE.md` still has a stale "Start here: read the handover" section pointing at `docs/HANDOVER.md`, and `MEMORY.md` references HANDOVER — per the new global rule these should be replaced by this controller-handoff convention.
