# edaptor — Session Handover

Carries the **current concern** into the next session. Not a project history — for
that see `git log`, the specs under `docs/superpowers/specs/`, the SDD ledger
(`.superpowers/sdd/progress.md`), and project memory (`…/memory/MEMORY.md`).

**Date:** 2026-06-30 · **Current concern: the `Shuttle` widget — PHASE 2
(consumer migration).** Phase 1 (the widget itself) is DONE and committed on
branch `feat/shuttle-widget`; `make check` green. The next session migrates the
two consumers onto it and deletes the old widget.

---

## CURRENT CONCERN — `Shuttle` widget, phase 2

### Why this exists

`src/ui/dual_list.rs` was an *opportunistic* helper: a controller that reaches
into the host's `Dialog`, inserts children at hardcoded `Rect`s, pokes their
`ViewId`s, and hands back a bespoke `DualEvent` enum. It is **not** a tvision
`View`. We are reworking it into `src/ui/shuttle.rs` — a proper, self-contained
`View` — with the end goal of **contributing it upstream to tvision-rs** (likely
renamed `Shuttle` / `TransferList`; `DualList` named the layout, not the job).

### Where phase 1 left it (DONE — branch `feat/shuttle-widget`, `main` at `8f46b66`)

`src/ui/shuttle.rs` is a complete, `make check`-clean `View`: `ShuttleModel` holds
the pure column logic (move / dedup / lock); `Shuttle` embeds an owned `Group`,
forwards the trait via `#[delegate(to = group, skip(handle_event, as_any_mut,
value, set_value, set_value_ctx))]`, and hand-writes only the move logic + data
exchange. 18 tests via a headless `Context` harness. **It is not yet wired into
the app** — no consumer constructs it (hence the temporary module
`#![allow(dead_code)]`). That wiring is phase 2, below.

### The `Shuttle` public API (what the consumers call)

```rust
// Construct (owns its own Group; the HOST inserts the Shuttle as a child):
Shuttle::new(area: Rect, left_title: &str, right_title: &str,
             with_search: bool, selected_on_left: bool) -> Shuttle

// Publish rows (each needs &mut Context):
sh.set_available(rows: Vec<ShuttleRow>, ctx)   // Available column (plain labels)
sh.set_selected(rows: Vec<ShuttleRow>, ctx)    // Selected column (locked rows get "* ")

// Read state (the owner reads these AFTER a broadcast):
sh.selected()    -> &[ShuttleRow]
sh.search_text() -> &str
sh.search_id()   -> Option<ViewId>   // focus it so typing searches immediately

// Row type — NOTE the field rename vs DualRow:
pub struct ShuttleRow { pub key: String, pub label: String, pub locked: bool }
//   DualRow.removable  ===  !ShuttleRow.locked   <-- inverted; convert on migration

// Notifications — broadcast with the Shuttle's own ViewId as `source`:
pub(crate) const CMD_SHUTTLE_CHANGED  // a move changed the Selected set
pub(crate) const CMD_SHUTTLE_SEARCH   // the search box text changed
```

The owner handles them in its own `handle_event`, after delegating to its dialog:

```rust
self.dlg.handle_event(ev, ctx);
if let Event::Broadcast { command, source } = ev {
    if *source == Some(self.shuttle_id) {
        if *command == CMD_SHUTTLE_CHANGED {
            self.refresh_available(ctx);   // reads sh.selected()
            self.update_staged();          // reads sh.selected()
        } else if *command == CMD_SHUTTLE_SEARCH {
            self.last_search = sh.search_text().into();
            self.refresh_available(ctx);   // membership: async submit; oc: local filter
        }
    }
}
```

The broadcasts carry **no payload** — the owner re-reads `selected()` /
`search_text()`. Both current consumers already recompute from the *whole*
Selected set, so this is a clean fit.

### Locked design decisions (do not relitigate)

- **Notify by broadcast**, not a return enum. `DualEvent` is gone.
- **Available column = `SortedListBox`** (type-to-search); highlighted row mapped
  back to the model **by label** (`list()[idx]`). **Selected column = plain
  `ListBox`** (unsorted) → its focused index *is* the model index. This is why the
  old `highlighted_text` display-string round-trip is **absent**: `ListBox::new_list`
  does NOT sort (only `SortedListBox` does), so the old code was defending a sort
  that never happened.
- **Available rows render plain** (no ✓ marker): both consumers filter
  already-selected rows out, so the marker was vestigial. Only **Selected** rows
  carry the `* ` lock marker.
- **Move affordances:** Insert/Delete + [Add]/[Remove] buttons (Alt-A/Alt-R) +
  **Enter-on-list** (Enter on Available→in, Selected→out; Enter elsewhere passes
  through so a host dialog's default OK still fires). **No Left/Right move keys** —
  they collide with real focus traversal. ⚠ **This is a user-visible change** vs
  DualList (which moved on →/←): update `docs/src/usage/crud.md` and add a
  `CHANGES.md` note in phase 2.
- **No overflow-gated scrollbars** (the old `sync_bars` hack is gone) — accept
  tvision's stock bars-while-active.

### Phase 2 — the work to do

1. **Migrate `oc_picker.rs`** (`ObjectClassPicker`): the host is a `Dialog`; insert
   a `Shuttle` as a child (store `shuttle_id`), add OK/Cancel, react to the two
   broadcasts. Seed via `set_selected` with `locked = structural && originally_active`
   (the structural-lock rule — keep it exactly; the "session-added structural class
   stays removable" bug fix must survive). `update_staged` reads `selected()`.
   Current `DualList::new`: `area 72x22, "Active","Available", with_search=true,
   selected_on_left=true`.
2. **Migrate `membership.rs`** (`MembershipDialog`): same shape, but the Available
   column is fed by the **async candidate pump** — `CMD_SHUTTLE_SEARCH` submits a
   `SearchFlow`; results arrive on the pump's `REFRESH` broadcast → `set_available`.
   `CMD_SHUTTLE_CHANGED` → mirror Selected into `staged_commit` as `SetValues`.
   Current `DualList::new`: `area 80x22, "Available","Members", with_search=true,
   selected_on_left=false`.
3. **Host layout (the one gotcha):** the Shuttle owns Add/Remove buttons at its
   local `y1-3` (left-aligned, `x0+2..x0+26`). Insert the Shuttle covering the full
   dialog content rect and add the host's OK/Cancel **right-aligned** via
   `dlg.button_row(.., ButtonRowAlign::Right)` — same row, disjoint columns, exactly
   as DualList did. Verify no overlap live (tmux).
4. **Consumer tests:** the existing `oc_picker`/`membership` tests drive the mover
   through `dual.avail_id_for_test()` / `selected_id_for_test()` and assert on
   `DualEvent`. `Shuttle`'s equivalents are currently **private cfg(test) helpers**
   (`avail_id`, `selected_id`, `highlight`). Phase 2 will need `pub(crate)`
   cfg(test) accessors on `Shuttle` (mirror DualList's `*_for_test`) so the host
   tests can set a highlight and dispatch a key. Rewrite the assertions against
   `selected()` + the broadcast (`Event::Broadcast`) instead of `DualEvent`.
5. **Delete `dual_list.rs`** and its `pub(crate) mod dual_list;` line in
   `src/ui/mod.rs`.
6. **Remove `#![allow(dead_code)]`** from the top of `shuttle.rs` (it is only there
   because nothing constructs a `Shuttle` yet).
7. **Gate:** full `make check` (fmt + clippy `-D warnings` + tests). Then live-verify
   both editors in tmux (objectClass add/remove + structural lock; membership search
   + add/remove). Update `CHANGES.md` (the →/← removal + Enter-on-list) and
   `docs/src/usage/crud.md`.

Suggested order: oc_picker first (local filter, simpler), then membership (async).
Keep it TDD where the seam allows (the model logic is already covered; the host
reactions are the new behavior).

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
