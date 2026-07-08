# Spike: edaptor core 3-pane on tvision-rs

- **Date:** 2026-06-22
- **Status:** Design approved, spec under review
- **Type:** De-risking spike (throwaway code on a branch) ahead of a possible full UI migration
- **Author:** brainstormed with Claude

## 1. Context & motivation

edaptor's UI is ~10k lines of [ratatui](https://ratatui.rs) across 19 files in
`src/ui` — roughly half the codebase. The rest of the crate (LDAP worker,
`schema`, `samba`, `config`, `form` changeset/validate, `workflows`) is pure
domain logic with no terminal dependency: `src/ui/mod.rs` enforces the rule that
`ratatui`/`crossterm` are imported *only* inside `src/ui`.

The proposal is to migrate the UI onto [`tvision-rs`](https://github.com/oetiker/tvision-rs)
— our own pure-Rust port of magiblot's Turbo Vision (v0.1.0). The motivation is
twofold:

- **Usability.** tvision-rs ships a multi-pane resizable `Splitter` and an
  `Outline` tree (plus `ListBox`, `InputLine` with validators, `Menu`,
  `StatusLine`, `Dialog`, scrollbars, a colour picker). The `splitter.rs`
  example is almost exactly edaptor's layout: a three-column split with a tree
  over a list on the left, a form in the middle, list-over-form on the right,
  with mouse-draggable dividers and a keyboard resize mode. Resizable panes are
  a concrete UX win we do not have today.
- **Dogfooding.** tvision-rs is v0.1.0 with zero external consumers. edaptor
  would be its first real-world application; the migration is how the library
  gets battle-hardened.

### The round-trip we are knowingly reversing

edaptor *was* on Turbo Vision and deliberately migrated **away** to ratatui (see
`research/2026-05-29-turbo-vision-spike.md`, `plans/2026-06-01-ratatui-migration.md`,
`handoff-2026-06-01-ratatui-migration.md`). The recorded trigger, verbatim from
`src/ui/view.rs`:

> "…the whole reason for leaving turbo-vision (its `InputLine` byte-sliced UTF-8
> and panicked on an umlaut at the cut)."

Crucial distinction: **that bug was in the third-party `turbo-vision` 1.3.1
crate**, not in `tvision-rs`. tvision-rs is a separate, independent port that
uses `unicode-segmentation` for grapheme clustering and is reported to handle
UTF-8 correctly. The spike must *prove* this for edaptor's edit fields before any
broader commitment — umlauts are first-class for a tool with German users and
DN/CN values.

The other thing ratatui bought was immediate-mode state simplicity: plain-data
`App`, re-render every frame, no `Rc<RefCell>` pane handles, no `CM_*` refresh
broadcasts. tvision-rs is retained-mode (persistent views, `ViewId` handles,
command dispatch via `Program::run_app`). The spike must confirm this paradigm
fits edaptor's worker-driven data flow without reintroducing the complexity we
shed.

## 2. Goals

This spike has **three** deliverable streams. The edaptor port is the vehicle;
the other two are first-class outcomes, because doing the port is the best way to
discover them.

### 2.1 Edaptor port (the vehicle)

Prove edaptor's core navigation + display UX runs on tvision-rs against the real
demo LDAP server, and produce a concrete gap-list and effort estimate for the
full migration.

### 2.2 tvision-rs documentation improvements

Having just navigated the library cold, capture what a coding agent (or a new
human consumer) needs to get productive fast and is missing today. Concretely,
record while the friction is fresh:

- What was hard to find, and where you eventually found it (so the index/README
  can point there).
- Gaps between the C++ Turbo Vision mental model and the Rust API surface — the
  porting-guide deltas that were not obvious.
- The "how do I stand up a minimal app with my own data source" path — the
  examples are demo-shaped (files, snake); a *bring-your-own-state* recipe was
  missing.
- Any place where the doc *exists* but did not surface when searched for the
  obvious term.

Output: a markdown findings doc, suitable to file as issues/PRs against
tvision-rs.

### 2.3 Framework features that would make the port simpler

Record features that, if present, would have made the edaptor port materially
easier — **but with strict discipline:**

> **Search hard before declaring anything missing.** tvision-rs is a port of a
> *vast* framework. A capability may exist under its classic Turbo Vision name
> (e.g. `T`-prefixed concepts, `cm*`/`ev*`/`hc*` command/event/help families,
> owner-draw hooks, `valid()`/`dataSize()`/`getData()`/`setData()` data
> protocols, `TValidator` subclasses, broadcast messaging) rather than the
> obvious modern term. Before writing "tvision-rs lacks X", grep the crate
> source, the `docs/`, the `PORTING-GUIDE`, the `CHANGELOG`, and the examples for
> the classic name. Only after that search comes up empty is it a genuine gap.

Output: a section in the same findings doc, split into "exists, found under name
Y" vs "genuinely absent".

## 3. Scope

### In

- The three-pane `Splitter` layout: `Outline` (DIT tree) │ `ListBox` (leaves +
  search `InputLine`) │ form `Group` (a `Label` + `InputLine` per attribute).
- Wiring to the **existing** domain layer unchanged: `WorkerHandle`, `ReadFlow`,
  `Structure`. Expand a branch → list populates → select a leaf → form fills.
- Resizable splitter dividers (mouse drag + keyboard resize mode).
- Read-only navigation + display; typing into `InputLine`s (display only, not
  persisted) to exercise grapheme editing.
- Runs against `scripts/test-ldap.sh` (podman OpenLDAP, ~600 users / ~25 groups).

### Out (stubbed or skipped — these belong to the *full* migration, not the spike)

- Save / changeset / validate / write path.
- All modal overlays → dialogs: `Confirm` (LDIF preview), `Error`, `Guard`
  (save/discard/stay), the Alt+N profile chooser, multi-value `ValueEditor`.
- The rich field widgets: `Choice`, `Password` (samba), `Picker`, `Membership`,
  `ObjectClassPicker`.
- Config-driven column-2 label rules and DIT tree-label rules (use a minimal
  hardcoded mapping just enough to navigate).
- The config-discovery / config-picker startup flow.

## 4. Architecture

Reuse everything outside `src/ui` untouched. Build the spike as a **separate
binary** (`src/bin/spike-tv.rs`, mirroring the existing `src/bin/gen-testdata.rs`
pattern) so the ratatui UI in `src/ui` stays fully intact and runnable on `main`
as the fallback. The spike depends on `tvision-rs` (aliased `tv` per its house
style: `tv = { package = "tvision-rs" }`) behind an optional feature or dev-dep so
the default build is unaffected.

The binary constructs a `tv::Program` with the splitter layout and bridges the
LDAP worker on the same poll-drain pattern edaptor already uses.

### Pane → widget mapping

| edaptor pane / element        | current (ratatui)              | tvision-rs            |
| ----------------------------- | ------------------------------ | --------------------- |
| Pane 1 — branch tree          | `TreeState` + `tui-tree-widget` | `Outline`             |
| Pane 2 — leaf list            | `rows: Vec<(label, dn)>`        | `ListBox`             |
| Pane 2 — incremental search   | `search: TextState`            | `InputLine`           |
| Pane 3 — edit form            | `EditForm` fields              | `Group` of `Label` + `InputLine` |
| Modals (full migration only)  | `Overlay` enum                 | `Dialog`              |
| Pane resize (new capability)  | fixed `Layout` percentages     | `Splitter` dividers   |

### The current data-flow loop (what we must reproduce)

edaptor's ratatui loop runs at a 50 ms tick:

1. draw the frame;
2. **drain all pending worker responses** (non-blocking `worker.poll()`);
3. poll input with a 50 ms timeout;
4. reconcile UI deltas (skipped while a modal holds keys);
5. service picker type-ahead search.

The LDAP worker is a **separate thread** reached over a request/response channel.
The loop never blocks on it — it drains whatever is ready each tick.

## 5. Primary risk — worker → view event pumping

This is the central question the spike exists to answer, and it directly shapes
the full-migration plan:

> tvision-rs's `Program::run_app(|prog, cmd| …)` is a **command-dispatch** loop.
> Can it interleave **non-blocking draining of the worker channel** with input
> handling — i.e. is there an idle/tick hook, a timeout on the input poll, or an
> event-injection path that lets external (off-thread) data flow into views
> between keystrokes?

Possible outcomes, in order of preference:

1. tvision-rs already exposes an idle/timer/tick hook or a timeout-poll
   (**search hard** — Turbo Vision has an `idle()` slot on `TProgram` and an
   event-queue `putEvent`/`getEvent` mechanism; these may be present under those
   names). → bridge the worker there, no library change.
2. It exposes a way to inject synthetic events / a custom command from another
   thread. → push worker responses as events.
3. Neither exists. → the spike's top finding becomes a concrete tvision-rs
   feature request (an idle callback or an event-injection API), and we evaluate
   the smallest addition that unblocks edaptor.

A secondary risk: **dynamic repopulation** of `Outline`/`ListBox` on navigation.
The `splitter.rs` example seeds a `ListBox` once on first event via a small
`#[delegate]` wrapper; edaptor needs to *re-seed* on every branch/leaf change.
Confirm the idiomatic way to mutate a view's contents post-insert (likely via
`Context` + the view's own setter, or a wrapper that owns the model).

## 6. Deliverables

1. **`src/bin/spike-tv.rs`** — the runnable spike, navigable against the demo
   server, with resizable panes.
2. **Findings doc** (`docs/superpowers/research/2026-06-22-tvision-rs-spike-findings.md`)
   containing:
   - the worker-pumping resolution (which outcome from §5, and how);
   - the tvision-rs **documentation** gap-list (§2.2);
   - the tvision-rs **framework feature** gap-list, split into "found under name
     Y" vs "genuinely absent" (§2.3);
   - a rough **effort estimate** for the full migration, broached as: panes (done
     in spike), then overlays→dialogs, then each rich widget, then save/validate,
     then config-driven labels/tree and startup.
3. **An explicit UTF-8 / umlaut test** — typing `Müller` / `Zürich` into an
   `InputLine` and the search box edits by grapheme with no panic and no
   byte-slice corruption. This is the regression that drove the last migration;
   it gets a named test, not just a manual check.

## 7. Success criteria

The spike succeeds when **all** of these hold:

1. The three-pane `Splitter` layout renders with `Outline` │ `ListBox`+search │
   form `Group`.
2. End-to-end navigation works against **real** demo data via the unchanged
   `worker → read_flow → structure` layer: expand branch → list populates →
   select leaf → form fills.
3. Splitter dividers resize (mouse drag + keyboard resize mode).
4. Umlaut/grapheme editing in `InputLine` and search is correct (criterion #3 of
   §6, with its test).
5. The findings doc exists with all three streams populated (§6.2).

If criteria 1–4 hold, the full migration is greenlit and gets its own plan via
the writing-plans flow. If the worker-pumping question (§5) lands on outcome 3,
the full migration is gated on the agreed tvision-rs addition first.

## 8. Non-goals

- No change to any module outside `src/ui` for the spike.
- No removal or modification of the existing ratatui UI — it remains the shipping
  UI and the fallback until the full migration lands.
- No production-quality polish in the spike binary — it is a throwaway probe.
