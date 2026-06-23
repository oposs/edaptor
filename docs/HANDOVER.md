# edaptor — Session Handover

Carries the **current concerns** into the next session. Not a project history —
for that see git log, the specs under `docs/superpowers/specs/`, and project
memory (`…/memory/MEMORY.md`).

**Date:** 2026-06-23 · **Purpose of next session: start the FULL UI migration
from ratatui to `tvision-rs`.** The de-risking spike is **complete and is a GO**
(all spec success criteria met, including live interactive confirmation).

`edaptor` is a Rust TUI for administering an OpenLDAP directory. It introspects
live schema (`cn=subschema`) and generates edit forms from `objectClass`
definitions; a TOML config declares connection settings plus *entry profiles* and
a **widget palette** (`[profile.widget.<attr>]` kinds: `choice` / `password` /
`picker` / `membership`). The shipping UI is **ratatui** (`src/ui/*`, ~10k LOC).

---

## Git topology (read this before you branch)

- **`origin/main` = `2185b2e`** (Release v0.4.0). Local `main` is **unpushed**.
- **Local `main` = `dec196b`** — 3 unpushed doc commits ahead of origin: the spike
  **design spec** and **implementation plan** live here (no code).
- **Branch `spike/tvision-rs` = `b520cf5`** — 18 commits ahead of `main`. Holds the
  **spike code** (`src/bin/spike-tv.rs`, `tests/spike_tv_umlaut.rs`, the Cargo
  feature) **and the findings doc**. Kept as-is (not merged) by user choice.
- Current `Cargo.toml` version is `0.4.0`.

**The findings doc currently exists only on `spike/tvision-rs`.** It is the most
important reference for the migration — see "First steps" for how to keep it handy.

---

## What the spike proved (the GO)

The full report — read it first — is on `spike/tvision-rs` at
[`docs/superpowers/research/2026-06-22-tvision-rs-spike-findings.md`](superpowers/research/2026-06-22-tvision-rs-spike-findings.md).
Headlines:

1. **The original blocker is gone.** `tvision-rs 0.1` `InputLine` is grapheme-correct
   for German text — proven by automated test (`tests/spike_tv_umlaut.rs`: type
   `"Zü"`, backspace → `"Z"`, no panic, no byte-split). This was the explicit
   pre-condition (edaptor *left* Turbo Vision because the old third-party
   `turbo-vision` crate byte-sliced UTF-8 and panicked on an umlaut — a DIFFERENT
   crate; `tvision-rs` is our own independent port).
2. **The off-thread LDAP worker bridges into tvision-rs views with NO library
   change** (spec §5 resolved): a zero-area `PumpView` + `Context::set_timer(50ms)`
   periodic `Event::Timer` → drain `worker.poll()` → `ctx.broadcast(REFRESH)`.
3. **The needed widgets exist** on the published `0.1` crate: `Splitter` (resizable
   panes), `Outline` (DIT tree), `ListBox`, `Group`+`InputLine`+`Label`+`Button`,
   `Menu`/`StatusLine`/`Dialog`/colour picker.
4. **The domain layer ports cleanly.** The spike drives the **unchanged** edaptor
   domain layer (`config`, `ldap::worker`, `schema`, `workflows::{structure,
   read_flow}`) from a separate binary — confirming the migration is "rewrite
   `src/ui` against tvision-rs; everything else untouched."
5. **Live-confirmed by user (2026-06-23):** three-pane render, branch→leaf→form
   navigation against real LDAP, and splitter resize all work.

Use the published crate **`tvision-rs = "0.1"`** from crates.io. Do NOT use a
path/git dependency. If a needed API is ever missing from a release, the fallback
is a git pin on `https://github.com/oetiker/tvision-rs` (upstream, unmodified) —
never a local path. (User directive: fixes to tvision-rs go via a separate clone +
upstream PR.)

---

## Load-bearing tvision-rs facts (so you don't rediscover them)

All verified against the `0.1.0` source during the spike; details + file:line in
the findings doc.

- **App skeleton:** `Program::new(backend, clock, theme, init_desktop,
  init_status_line, init_menu_bar)` then `program.run_app(|prog, cmd| { … })`. The
  three `init_*` factories are `impl FnOnce(Rect) -> Option<Box<dyn View>>` — they
  **accept capturing closures**, so app state (an `Rc<RefCell<…>>`) reaches the
  view tree without a `thread_local` hack.
- **`Outline` REQUIRES `tv::ov_update(&mut outline, ctx)` once after
  construction/insert.** Skip it and `limit.y == 0`, so arrow-down clamps focus to
  `-1` and the selection vanishes. Seed it once via a wrapper view's first
  `handle_event` (same idiom as `ListBox::new_list`). **This is undocumented
  upstream — easy to miss.**
- **Reading selection is inconsistent between widgets:** `ListBox` implements
  `View::value() -> Some(FieldValue::Int(focused))`; `Outline` does **not** — read
  `outline.ov().foc` via the `OutlineViewer` trait (import it).
- **Dynamic content after insert:** `ListBox::new_list(items, ctx)` (needs a live
  `Context`; seed on first event). `Outline` tree replacement: replace the `pub
  root` field and call `ov_update` (documented at `outline.rs:1385`).
- **Mutating form fields:** `Group::child_mut(ViewId) -> Option<&mut dyn View>`
  exists; `InputLine::set_value(FieldValue::Text(s))` works.
- **`Event::Broadcast` (from `ctx.broadcast`) is DEFERRED**, not synchronous — it
  is queued and delivered on a later loop pass. So a `RefCell` borrow in one pane's
  `handle_event` can never collide with another pane re-entering on the broadcast.
  Still follow the rule: **collect into locals, drop the borrow, then call**
  `broadcast`/`new_list`/`set_value`/`worker.submit`.
- **Headless view testing works (Path A):** build a `Context` directly —
  `tvision_rs::view::Context::new(&mut out, &mut timers, 0, &mut deferred)` with
  `tvision_rs::timer::TimerQueue::new()` and `tvision_rs::view::Deferred` (all
  `pub`). A standalone `InputLine` needs `il.state.state.selected = true` to
  receive keys. (`Deferred` is NOT re-exported at the crate root — use the
  `view::` path.)

---

## The migration: scope, approach, estimate

**Approach (proven by the spike): rewrite `src/ui/*` against tvision-rs; leave the
domain layer (`form`, `workflows`, `config`, `schema`, `ldap`, `samba`) untouched.**
The facade boundary flips from ratatui to tvision-rs (see Conventions).

**Bootstrap sequence to reuse** (verbatim from the spike, all public APIs):
`Config::load(path)` → `WorkerHandle::spawn(config, password)` →
`worker.request(Request::FetchSubschema)` → `SchemaModel::from_raw` →
`worker.request(Request::LoadStructure{..})` → `Structure::build` →
`ReadFlow::new(schema)`. Branch→leaf is synchronous from `Structure::leaves_of`;
leaf→form needs the async worker (use the `Request::Search{scope: Base}` /
`ReadFlow::request_entry` path + the timer-pump pattern).

**Layers to build (rough estimate ~5–8 weeks; see findings §4):**
1. Three-pane core (Splitter + Outline + ListBox + form Group) — **already
   prototyped in the spike**; productionize it (use `ReadFlow`/`FormModel` instead
   of the spike's raw `LdapEntry` display, and dynamic per-attribute widgets).
2. Overlays → tvision `Dialog`s: Confirm (LDIF preview), Error, Guard
   (save/discard/stay), Alt+N profile chooser, multi-value `ValueEditor`.
3. The rich widgets: Choice, Password (samba, TLS-gated), Picker, Membership,
   **ObjectClassPicker — flagged the RISKIEST piece** (schema-driven dynamic field
   regeneration; the spike did not touch it).
4. Save / validate / changeset wiring (the `form::{changeset,validate}` domain
   layer already exists and is UI-agnostic — wire the tvision dialogs to it).
5. Config-driven column-2 label rules + DIT tree-label rules.
6. Config-discovery / config-picker startup flow.

**The spike binary is REFERENCE ONLY — throwaway.** Do not grow the real UI inside
`src/bin/spike-tv.rs`. Build the real thing in `src/ui/*`. When the tvision UI
ships, delete `src/bin/spike-tv.rs`, `tests/spike_tv_umlaut.rs` (fold its umlaut
assertions into the real edit-field tests), and the `spike-tv` Cargo feature.

**Upstream tvision-rs PRs worth doing first** (small, make the migration smoother;
via a separate clone + PR per user directive): document that `ov_update` is
mandatory after `Outline` construction; implement `Outline::value()` for parity
with `ListBox`; re-export `Deferred` at the crate root; add a "bring-your-own-state
/ external data source" example.

---

## First steps for the migration session

1. **Pull / sync** (this repo commits directly to `main`): `git pull --ff-only`.
   Note local `main` has unpushed spike spec+plan commits.
2. **Read the findings doc** (on `spike/tvision-rs`). To keep it on trunk so it
   survives, consider cherry-picking just that doc onto `main`, or `git show
   spike/tvision-rs:docs/superpowers/research/2026-06-22-tvision-rs-spike-findings.md`.
   Also re-read the spec & plan (already on `main`).
3. **Run the spike once** for orientation (see Build/run) — it shows the target UX.
4. **Brainstorm → write the full-migration plan** (use the brainstorming →
   writing-plans flow). The spike's spec/plan cover only the spike; the full
   migration needs its own spec+plan. Decompose by the 6 layers above; consider
   shipping the three-pane core behind a feature flag first, ratatui as fallback,
   then swap the default once parity is reached.
5. **Execute subagent-driven** (fresh subagent per task + spec-then-quality review),
   as the spike was. ⚠ One subagent edits the tree at a time — do not run a side
   agent on the same working tree concurrently (it caused a commit-bundling
   collision during the spike).

---

## Build / test / run

**⚠ Cap parallelism at 4 cores** (shared 128-core box). Cargo target dir is
`/home/oetiker/scratch/cargo-target` (binary NOT under `./target`).

```bash
# Default build/tests — spike is feature-gated OUT (must stay clean):
cargo build -j4
cargo test -j4
cargo clippy -j4 --all-targets -- -D warnings
cargo fmt --check

# The spike (only on branch spike/tvision-rs):
cargo test  -j4 --features spike-tv --test spike_tv_umlaut       # 2 grapheme tests
cargo clippy -j4 --features spike-tv --all-targets -- -D warnings

# Live demo server (podman) + run the spike TUI in a REAL terminal:
scripts/test-ldap.sh start              # ~600 users / ~25 groups, ldap://localhost:1389
export EDAPTOR_TEST_ADMIN_PW=adminpassword
cargo run -j4 --features spike-tv --bin spike-tv     # select branch → leaf → form
scripts/test-ldap.sh stop
```
Verify feature isolation: `cargo tree -i tvision-rs` must FAIL without
`--features spike-tv` (proves the spike never touches the default build).
TUI smoke: quit via **Alt+X**; do NOT `pkill -f edaptor` (matches the LDAP
container). No TTY in agent sessions → `CrosstermBackend::new()` returns ENXIO;
interactive checks need a human at a terminal.

---

## Conventions (follow these)

- **Facade boundary — this is the migration's core rule.** Today: only `src/ui/*`
  may `use ratatui` / `use tui_*`. During the migration the SAME rule applies to
  `tvision_rs`: only `src/ui/*` (and, transitionally, `src/bin/spike-tv.rs`) may
  `use tvision_rs`. The domain layer must stay UI-framework-agnostic. Verify:
  `! grep -rl "use ratatui\|use tui_\|use tvision_rs" src | grep -vE "^src/ui/|^src/bin/spike-tv.rs"`.
- **`form` is the pure domain layer** (`changeset`, `validate`, `is_secret_attr`):
  must NOT import `ui`. `ui` and `workflows` depend on `form`. Reuse it for the
  tvision save/validate path unchanged.
- **Widget palette = the one config-driven "rich field" home.** New per-attribute
  behavior is a `[profile.widget.<attr>]` `kind`, resolved in `config::widget`
  into a `WidgetKind`. The tvision form must honor `EditField.widget_binding`.
- **Strict TDD**, atomic commits; crate compiles after every commit; `cargo fmt`
  before every commit; clippy clean (`--all-targets`).
- **Dependency:** published `tvision-rs = "0.1"`; alias as `tv` per its house style
  (`tv = { package = "tvision-rs" }`) if convenient. tvision-rs is edition 2024;
  edaptor is edition 2021 — fine (edition is per-crate).
- **Live tests gated** by `EDAPTOR_TEST_LDAP_URI` (skip when unset). DN base
  `dc=example,dc=org`. Demo password env `EDAPTOR_TEST_ADMIN_PW=adminpassword`.
- **Docs are one-home:** config detail → mdBook (`docs/src/`), surfaced at
  <https://oposs.github.io/edaptor>; `README.md` is orientation only; `CHANGES.md`
  gets every user-visible change. Process/design notes → `docs/superpowers/`.
- **No back-compat constraints** — no userbase; remove/replace cleanly.
- **Commit trailer:** `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- **Execution style:** subagent-driven (fresh subagent per task + spec-then-quality
  review). One editor per working tree at a time.

---

## Open gaps / decisions to make

1. **Where the findings doc + handover live.** Both are currently on
   `spike/tvision-rs` (handover) / branch-only (findings). Decide whether to land
   them on `main` so they survive if the spike branch is deleted.
2. **Migration branch strategy:** new branch off `main`; keep the ratatui UI as a
   runtime fallback (feature flag) until tvision reaches parity, or hard-swap.
3. **`ReadFlow`/`FormModel` vs raw `LdapEntry`:** the spike displayed raw attrs;
   the real form must use `ReadFlow` + the schema-driven `FormModel`/widgets and
   the changeset/validate write path.
4. **`main` is local-only / unpushed** (origin behind at `2185b2e`); CI, docs
   deploy, and releases only take effect once pushed + Pages enabled.
5. **ObjectClassPicker** is the riskiest widget — prototype it early in the
   migration rather than last.
