# edaptor — Session Handover

Carries the **current concern** into the next session. Not a project history — for
that see `git log`, the specs under `docs/superpowers/specs/`, the SDD ledger
(`.superpowers/sdd/progress.md`), and project memory (`…/memory/MEMORY.md`).

**Date:** 2026-06-30 · **Current concern: upstream the `Shuttle` widget to
tvision-rs (phase 3).** Phases 1 (the widget) and 2 (consumer migration + delete
`DualList`) are **DONE** on branch `feat/shuttle-widget`; `make check` green,
facade guards clean, both editors live-verified (objectClass) / test-covered
(membership). The branch is ready to merge to `main`.

---

## CURRENT CONCERN — graduate `Shuttle` upstream (phase 3, not started)

`src/ui/shuttle.rs` is a self-contained, generic two-list transfer `View` with no
edaptor domain types — explicitly built to be **contributed upstream to tvision-rs**
(`https://github.com/oetiker/tvision-rs`), likely renamed `Shuttle` / `TransferList`.

Directive (unchanged): work in a SEPARATE clone of tvision-rs, one focused PR;
edaptor depends on the PUBLISHED crate (git pin only as a fallback until a release
ships). When it lands upstream, delete `src/ui/shuttle.rs` and re-point the two
consumers at the crate's type. Carry across the load-bearing facts learnt here:

- **Nested-group focus (the phase-2 gotcha worth upstreaming as docs/example):** a
  host that embeds the Shuttle as a `Dialog` child must pass the **Shuttle's own
  `ViewId`** to `exec_view_focused` (a direct child → `focus_descendant` calls
  `focus_child` → `dialog.current = Shuttle`), and must run the **Shuttle's
  `reset_current`** during the host's `reset_current` so the Shuttle's internal
  currency is its search box before the dialog focuses it. Passing the *search
  box* id (DualList's old trick) only focuses the leaf and leaves the dialog
  routing keys to a button — the modal opens keyboard-dead. The Shuttle now owns a
  `reset_current` that focuses its search box (or the Available list).
- **Host layout:** the Shuttle owns Add/Remove at its local `y1-3` (left); the host
  adds OK/Cancel **right-aligned** (`ButtonRowAlign::Right`) on the same row —
  disjoint columns, verified no overlap live.
- **API:** `Shuttle::new(area, left_title, right_title, with_search,
  selected_on_left)`; `set_available`/`set_selected(rows, ctx)`; `selected()`,
  `search_text()`, `search_id()`; notifies via `CMD_SHUTTLE_CHANGED` /
  `CMD_SHUTTLE_SEARCH` broadcasts (source = the Shuttle's id, no payload — the
  owner re-reads `selected()`/`search_text()`). `ShuttleRow { key, label, locked }`.

### What phase 2 delivered (committed on `feat/shuttle-widget`)

- `ObjectClassPicker` and `MembershipDialog` now embed a `Shuttle` child and react
  to its broadcasts in `handle_event` (after delegating). `oc_picker`:
  `CMD_SHUTTLE_CHANGED` → `refresh_available` + `update_staged`; `CMD_SHUTTLE_SEARCH`
  → local filter. `membership`: `CMD_SHUTTLE_SEARCH` → async `submit_search`
  (results land on the pump's `REFRESH` → `sync_results`); `CMD_SHUTTLE_CHANGED` →
  re-filter Available + restage `SetValues`. Membership now **filters
  already-selected candidates out of Available** (the old ✓ marker is gone).
- `src/ui/dual_list.rs` deleted; `#![allow(dead_code)]` removed from `shuttle.rs`.
- User-visible: **→/← no longer move** (focus traversal only); moves are
  Insert/Delete, [Add]/[Remove] (Alt-A/Alt-R), or **Enter while a list holds
  focus**. Documented in `CHANGES.md`, `docs/src/usage/crud.md`,
  `docs/src/configuration/widgets.md`.
- ⚠ **Membership could not be live-driven in the demo:** `memberOf` is
  overlay-maintained (operational) and is **not a reachable form field** in the
  demo config — so the membership Shuttle is covered by its unit tests + the
  `tv_membership` write-flow integration test + the identical shared Shuttle code
  verified live via objectClass, not by tmux. (Pre-existing demo reality, not a
  regression.) If a future session wants a live membership drive, configure a
  profile whose `show` includes a fan-out attribute that *is* a real form field.

---

## Project state (history is elsewhere)

The **tvision-rs UI migration (M1–M5c) is COMPLETE and merged to `main`.** edaptor
is a Rust TUI for administering OpenLDAP: it introspects live schema
(`cn=subschema`) and generates edit forms from `objectClass` definitions; a TOML
config declares connection settings plus *entry profiles* and a **widget palette**
(`[profile.widget.<attr>]` kinds: `choice` / `password` / `picker` / `membership`).
The UI is **tvision-rs** (`src/ui/`); the `edaptor` binary is the sole binary. For
the full migration narrative and load-bearing M3/M4/M5 facts, see `git log`, the
specs/plans under `docs/superpowers/`, and `.superpowers/sdd/progress.md`.

`Cargo.toml` version `0.4.0`; dependency **`tvision-rs = "0.3"`** (crates.io
release, no pin).

---

## Build / test / run

**⚠ Cap parallelism at 4 cores** (shared box). Target dir
`/home/oetiker/scratch/cargo-target` (NOT `./target`).

```bash
cargo build -j4                                   # the edaptor binary
CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 cargo test -j4 --lib shuttle   # the new widget's tests
cargo clippy -j4 --all-targets -- -D warnings
cargo fmt --check
make check                                        # fmt + clippy + tests (the gate)

# Live LDAP demo (podman): ~600 users / ~25 groups, ldap://localhost:1389
scripts/test-ldap.sh start
export EDAPTOR_TEST_ADMIN_PW=adminpassword
cargo run -- --config examples/demo-config.toml
# gated live tests (skip without the env):
EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389 EDAPTOR_TEST_ADMIN_PW=adminpassword \
    cargo test -j4 --test tv_membership --test tv_picker --test tv_objectclass
```

Facade guards (must print nothing — only `src/ui/**` may use tvision_rs):
```bash
! grep -rl "use tvision_rs" src | grep -vE "^src/ui/"
! grep -rl "use ratatui\|use tui_" src
```

---

## ⭐ Live-driving the TUI from an agent session (tmux)

You can drive the real TUI yourself over a PTY (no human needed) — use this to
accept the migrated editors in phase 2:

```bash
scripts/test-ldap.sh start                       # podman demo server (idempotent)
tmux kill-session -t edtv 2>/dev/null
tmux new-session -d -s edtv -x 210 -y 50         # wide enough for 3 panes
tmux send-keys -t edtv 'export EDAPTOR_TEST_ADMIN_PW=adminpassword' Enter
tmux send-keys -t edtv '/home/oetiker/scratch/cargo-target/debug/edaptor --config examples/demo-config.toml' Enter
sleep 4
tmux send-keys -t edtv Down                       # keys: Down/Up/Tab/Enter, or literals
sleep 0.4
tmux capture-pane -t edtv -p | sed -n '2,14p'    # read the screen
tmux kill-session -t edtv                         # clean up (the run holds an LDAP bind)
```

Build first (`cargo build -j4 --bin edaptor`). For modals send the button hotkey
or arrows+Enter. Do NOT trigger destructive saves against demo data carelessly —
prefer edit-then-Discard. Insert `sleep` between keystrokes (async reads land via
the 50ms pump). Focus probes: `tmux display-message -p '#{cursor_x}'` locates the
focused widget by column; `tmux capture-pane -e` renders the focused element bg
bright-green `(0,170,0)`.

---

## Conventions (follow these)

- **Facade boundary:** only `src/ui/**` may `use tvision_rs`. The domain layer
  (`config`, `form`, `ldap`, `schema`, `samba`, `workflows`) imports neither
  tvision_rs nor any tui crate and stays UI-agnostic.
- **Borrow discipline:** never hold a `RefCell`/`UiState` borrow across
  `ctx.broadcast`/`ctx.post`/`Program::exec_view`/`worker.submit`/`new_list`/
  `child_mut`/`set_value`. Collect into locals → drop the borrow → call. (A
  `FieldEditor` must NOT `borrow_mut()` during `into_view` — `dispatch` holds
  `state.borrow()` to pass the schema in; stage in `reset_current` / on events.)
- **Strict TDD**, atomic commits, crate compiles after every commit, `cargo fmt`
  before each commit, clippy `--all-targets -D warnings` clean.
- **Live tests gated** by `EDAPTOR_TEST_LDAP_URI` (skip when unset). Base
  `dc=example,dc=org`, password env `EDAPTOR_TEST_ADMIN_PW=adminpassword`.
- **Docs one-home:** config detail → mdBook (`docs/src/`); README orientation only;
  `CHANGES.md` for every user-visible change. Process/design → `docs/superpowers/`.
- **Commit trailer:** `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
  ⚠ Use `git commit -F` (a file/heredoc) for messages with backticks.

---

## Load-bearing tvision-rs facts (0.3.0 — so you don't rediscover them)

Directly relevant to phase 2:

- **`#[delegate(to = group, skip(...))]`** (from `tvision-rs-macros`) forwards the
  `View` impl to an embedded field; `skip(...)` lists the methods you hand-write.
  This is the idiom `Window`/`Dialog`/`Shuttle` use. A composite view embeds a
  `Group` and inserts id-addressed children — it does NOT "extend" Group.
- **`Group`:** `Group::new(bounds)`, `group.insert(Box<dyn View>) -> ViewId`,
  `group.child_mut(id) -> Option<&mut dyn View>`, `group.current() -> Option<ViewId>`
  (the focused child), `group.focus_child(id, ctx)`.
- **`ListBox::new_list` does NOT sort** (stores verbatim); only `SortedListBox`
  sorts case-insensitively. Both expose `new(bounds, num_cols, h, v)`, `new_list`,
  `list() -> &[String]`, and `value() -> Some(FieldValue::Int(focused_idx))`.
- **Notification:** no owner pointers. `ctx.broadcast(command, source: Option<ViewId>)`
  queues an `Event::Broadcast { command, source }`; a listener filters by matching
  `source` to a stored child id. `ScrollBar`/`Button` notify this way.
- **Gather/scatter:** `View::value()/set_value()` are the dialog data channel;
  `set_value_ctx` defaults to calling `set_value` (override it to render on scatter).
  `FieldValue::List(Vec<FieldValue>)`.
- **`reset_current` is THE modal-open init hook** (runs before first draw/event),
  NOT `on_bounds_changed` (dead for modal inserts — `Group::insert` calls
  `set_bounds` directly). Seed lists / stage state there.
- **Headless view tests:** `Context::new(&mut out, &mut timers, 0, &mut deferred)`
  with `tv::timer::TimerQueue::new()` and `Vec<tv::Deferred>`. Events are
  `Event::KeyDown(KeyEvent::from(Key::…))`. The `shuttle.rs` test module's
  `Harness` is the template (incl. `broadcast_seen`).
- **Worker → views:** zero-area `PumpView` + 50ms `Event::Timer` → drain
  `worker.poll()` → correlate (`SearchFlow`, ids 3M+) → `ctx.broadcast(REFRESH)`.
  Membership's async Available column rides this.

---

## Upstream tvision-rs (oetiker's repo) — working with it

edaptor is tvision-rs's first real consumer; the `Shuttle` is explicitly intended
to graduate upstream once proven here. Directive: work in a SEPARATE clone of
`https://github.com/oetiker/tvision-rs`, one focused PR per change; edaptor depends
on the PUBLISHED crate (a git pin is the only fallback, until a release ships).
The `#[delegate(to=group)]` macro forwards `View` methods via a manual list in
`tvision-rs-macros/src/specs.rs` — a NEW `View` method must be added there to be
forwarded.
