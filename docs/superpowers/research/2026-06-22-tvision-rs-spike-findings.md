# tvision-rs Migration Spike — Findings (2026-06-22)

Spike branch: `spike/tvision-rs`. Three deliverable streams from the design spec
(`docs/superpowers/specs/2026-06-22-tvision-rs-migration-spike-design.md`): worker
pumping, documentation gaps, and framework feature gaps. Plus a migration effort
estimate and a go/no-go.

---

## 1. Worker → View Pumping (spec §5)

**Outcome chosen: periodic `Context::set_timer` attached to a zero-area `PumpView`.**

### Mechanism

A `PumpView` holds a `ViewState` with a zero-area `Rect::new(0,0,0,0)` and is
inserted into the window's child list after the splitter. On its first event it
calls:

```rust
ctx.set_timer(Duration::from_millis(50), Some(Duration::from_millis(50)));
```

That arms a recurring ~20 Hz periodic timer. On every subsequent `Event::Timer(_)`
the view calls `state.borrow_mut().pump_worker()`, drops the borrow, and broadcasts
`REFRESH` (a `Command::custom("spike.refresh")`) if anything changed.

**Why `Event::Timer` reaches a zero-area child:** `group.rs:1350` routes
`Event::Broadcast { .. } | Event::Timer(_)` by iterating every child via
`deliver()`, which only blocks views for mouse/key events when `blocked()`.
`Event::Timer` passes all visibility and geometry gates unconditionally. A
zero-area `PumpView` that is never drawn and never hit-tested still receives every
timer tick. (Source: `src/view/group.rs:1341–1355` in the tvision-rs 0.1.0 crate.)

`pump_worker()` drains `worker.poll()` in a non-blocking loop and matches
`Response::Entries` by correlation id. The REFRESH broadcast then causes every
interested pane (`LeafPane`, `FormPane`) to pull fresh data from the shared
`Rc<RefCell<SpikeState>>`.

### No tvision-rs change was needed

The timer mechanism already existed and was discoverable from the crate's own
`examples/snake/` source. The zero-area delivery guarantee was confirmed by reading
`group.rs`. No fork, patch, or upstream issue is required for this mechanism.

### Alternative: `Program::set_on_idle`

`Program::set_on_idle(f: impl FnMut(&mut Program) + 'static)` (program.rs:699)
fires on every event-loop pass when no input is pending. It is the documented
external-data hook for program-level drains. It requires `&mut Program` access and
is therefore best suited to program-owned state; the timer approach integrates more
naturally when the drain logic lives inside a view with shared state.

### Observed latency

Up to one ~50 ms timer period between the LDAP response arriving and the next
`pump_worker` call. Interactive confirmation — leaf selected, form fills within
~50–100 ms — still requires a human at a terminal (automated env has no TTY:
`ENXIO`).

---

## 2. tvision-rs Documentation Gaps (spec §2.2)

Items discovered during the spike that a new consumer needs but cannot easily find.
All are candidates for issues or doc-patches against tvision-rs.

### 2.1 `ov_update` is mandatory after construction (undocumented in the overview)

**What happens without it:** `Outline` constructs with `limit.y == 0`, so
`adjust_focus` clamps the focus to -1 even on a non-empty tree. Arrow-down appears
to have no effect; selection vanishes. This takes minutes to diagnose.

**Where it is now documented:** The doc comment on `Outline::new` (added in the
crate's own source) reads:

> After calling `new`, insert the widget into a group and then call `ov_update`
> with the resulting `Context`. That second step is mandatory: publishing the
> scrollbar range and page parameters requires a `Context`, which is unavailable
> at construction time.

That comment exists at the field/method level. It does not appear in the crate's
top-level README or in any "getting started" example. A new consumer following
only the `splitter.rs` example (which seeds static data via a `#[delegate]`
wrapper) will not see it.

**Pattern required:**
```rust
// After inserting the Outline into the group, in the first handle_event:
if !self.seeded {
    tv::ov_update(&mut self.outline, ctx);
    self.seeded = true;
}
```

### 2.2 No "bring-your-own-state / external data source" recipe

The bundled examples (`splitter.rs`, `snake/`, `editor/`) are demo-shaped: they
own their data directly in the view or seed it once from a static source. There is
no recipe for the pattern edaptor needs: a view that holds a reference to shared
mutable application state and re-renders when that state changes.

We had to invent the full pattern:
- A type alias `type Shared = Rc<RefCell<SpikeState>>` passed by clone to each
  pane factory closure.
- A custom `Command::custom("spike.refresh")` constant broadcast on change.
- `LeafPane` / `FormPane` checking for that broadcast in `handle_event` and
  pulling data on receipt.
- `PumpView` arming a timer and calling the worker drain.
- Borrow discipline: never hold a `borrow()` or `borrow_mut()` across a call that
  could re-enter (e.g. across `ctx.broadcast`, `new_list`, `child_mut`). All
  borrows are dropped explicitly before those calls.

This is the core pattern for any real application; it should be a first-class
example in the tvision-rs repository.

### 2.3 `Program::new` factories accept capturing closures (pleasant surprise)

The C++ Turbo Vision porting guide's mental model suggests factory callbacks
might require bare function pointers (which cannot capture state). They do not:
`Program::new` takes `impl FnOnce(Rect) -> Option<Box<dyn View>>`, which
accepts move-closures that capture `Shared` clones directly. No `thread_local!`
workaround is needed. This is a positive delta from the C++ model but is not
explicitly called out in the porting guide.

### 2.4 Standalone `InputLine` requires manual `state.state.selected = true`

An `InputLine` constructed outside a `Program` (e.g. in a unit test) silently
ignores all key events unless `il.state.state.selected` is set to `true` before
driving it. The fields involved (`InputLine::state: ViewState`, `ViewState::state:
State`, `State::selected: bool`) are all `pub`, so the workaround is
straightforward once discovered. The crate's own private test helper `field()` does
this internally but it is not a public API and the requirement is not documented
for external consumers.

### 2.5 `Deferred` is not re-exported at the crate root

`Context::new` requires a `Vec<Deferred>` argument. `Deferred` is `pub` at
`tvision_rs::view::Deferred` (re-exported in `view/mod.rs`) but is NOT at
`tvision_rs::Deferred`. A consumer building a headless test context must use the
two-segment path `tvision_rs::view::Deferred`. Minor ergonomics issue; a simple
`pub use view::Deferred` in `lib.rs` would fix it.

---

## 3. Framework Feature Gaps (spec §2.3)

Split as required: features that exist under a non-obvious name, vs features that
are genuinely absent. For each candidate gap, the search performed is stated before
the classification.

### 3.1 EXISTS — Idle/tick hook

**Search:** `set_on_idle`, `idle`, `timer`, `putEvent`, `on_idle` in
`src/app/program.rs` and `src/view/context.rs`.

**Found:** `Program::set_on_idle(f: impl FnMut(&mut Program) + 'static)`
(program.rs:699) and `Context::set_timer` / `Context::set_timer_abs`
(context.rs:1102). Both are present in tvision-rs 0.1.0. No feature request needed.

### 3.2 EXISTS — Cross-view messaging / broadcast

**Search:** `broadcast`, `putEvent`, `broadcast_command`, `Context::broadcast` in
`src/view/context.rs` and examples.

**Found:** `Context::broadcast(command: Command, source: Option<ViewId>)`
(context.rs:1095). Sends a `Command` as an `Event::Broadcast` to all views in the
group hierarchy. Used in the spike for `REFRESH`. No feature request needed.

### 3.3 EXISTS — `ListBox` selection accessor

**Search:** `value`, `fn value`, `focused`, `lv.focused` in
`src/widgets/list_box.rs`.

**Found:** `ListBox` implements `View::value() -> Some(FieldValue::Int(self.lv.focused))`
(list_box.rs:181). Reading the focused row index needs no downcast. The spike uses
`if let Some(FieldValue::Int(sel)) = self.list.value()` in `LeafPane::handle_event`.

### 3.4 EXISTS — `Group` child mutation after insert

**Search:** `child_mut`, `get_child_mut`, `child_by_id`, `View::downcast` in
`src/view/group.rs`.

**Found:** `Group::child_mut(id: ViewId) -> Option<&mut dyn View>` (group.rs:236).
Confirmed by reading that `Window::child_mut` is a thin wrapper: `self.group.child_mut(id)`.
`InputLine::set_value(FieldValue::Text(s))` (input_line.rs:1143) loads the string
into the buffer and selects all. Used in `FormPane::handle_event` to fill rows.

### 3.5 EXISTS — Headless view testing (Path A)

**Search:** `Context::new`, `TimerQueue::new`, `Deferred` in `src/view/context.rs`
and `src/timer.rs`; also `lib.rs` for re-exports.

**Found:** `Context::new`, `TimerQueue::new`, and `Deferred` are all `pub`. A
consumer can construct a headless context without a `Program`:

```rust
let mut out: VecDeque<Event> = VecDeque::new();
let mut timers = TimerQueue::new();
let mut deferred: Vec<tvision_rs::view::Deferred> = Vec::new();
let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);
widget.handle_event(ev, &mut ctx);
```

This is exactly what `tests/spike_tv_umlaut.rs` does. The umlaut regression test
(two tests, both PASS) was built and run against this API.

Minor gap: `Deferred` not at crate root (see §2.5 above).

### 3.6 EXISTS (via field write + `ov_update`) — Replacing `Outline`'s node tree at runtime

**Search:** `set_root`, `replace_root`, `load_tree`, `reset`, `fn.*root` in
`src/widgets/outline.rs`.

**Found:** No dedicated `set_root` or `replace_root` method exists. However,
`Outline::root` is a `pub` field with an explicit doc comment (outline.rs:1385–1386)
stating verbatim:

> To swap the tree at runtime, replace this field and call [`ov_update`]
> so the scrollbar limits stay consistent with the new content.

So the mechanism IS documented and supported — it just requires a direct field
write followed by `ov_update(ctx)`, not a method call. The pattern:

```rust
outline.root = Some(new_root_node);
tv::ov_update(&mut outline, ctx);
```

Classification: **exists via pub field + ov_update**. Not a missing feature, but
the method-call ergonomics are rougher than `ListBox::new_list`. A wrapper method
`Outline::set_root(root, ctx)` would improve consistency.

### 3.7 ABSENT — `Outline` lacks `View::value()`

**Search:** `fn value` in the `impl View for Outline` block and throughout
`src/widgets/outline.rs`.

**Confirmed absent:** `Outline`'s `View` impl (outline.rs:1528) does not override
`value()`, which means it returns the default `None`. The selection must be read
via `OutlineViewer::ov().foc` (a `pub i32` field on `OutlineViewerState`). This
API inconsistency — `ListBox` has `value()`, `Outline` does not — is a genuine gap.
A `View::value()` implementation returning `FieldValue::Int(self.ov.foc)` would
make the two tree-like widgets consistent.

### 3.8 ABSENT — Blessed shared application-state pattern

**Search:** `Rc<RefCell`, `AppState`, `shared`, `application_state`, `app_state`
in all example and docs source files in the crate; also `PORTING-GUIDE.md` and
`CHANGELOG.md`.

**Confirmed absent:** The crate ships no example or documented recipe for
multi-pane applications where views share mutable state. The `Rc<RefCell<T>>` +
broadcast + timer-pump pattern the spike invented is the natural Rust idiom, but it
took non-trivial discovery time. A "shared state" example in the repository would
substantially lower the barrier for new consumers.

---

## 4. Migration Effort Estimate

The spike proves panes work. Here is a rough T-shirt sizing for the remaining
layers, based on what we now know about the tvision-rs API surface.

| Layer | Status | Size | Notes |
|---|---|---|---|
| Three-pane splitter + `Outline` + `ListBox` + form `Group` | **DONE in spike** | — | Three-pane render, navigation, leaf→form, and splitter resize all confirmed live (2026-06-23) |
| Modal overlays → `Dialog`s: Confirm (LDIF), Error, Guard, profile-chooser | Not started | M | tvision-rs ships `Dialog`, `Button`, `Program::exec_view`; no unknown APIs |
| `ValueEditor` multi-value overlay | Not started | M | Own scroll + editing; more complex than Confirm |
| `Choice` widget (radio/checkbox over `ListBox`) | Not started | S | `ListBox` + label already proven |
| `Password` widget (samba NTLM/LM toggle) | Not started | S | `InputLine` + mask; straightforward |
| `Picker` widget (DN picker dialog) | Not started | M | A dialog wrapping the tree+list pair; pattern is now known |
| `Membership` widget (group membership editor) | Not started | M | Two-column `ListBox` + move buttons |
| `ObjectClassPicker` | Not started | M–L | May need custom multi-select `ListBox` logic |
| Save / validate / changeset wiring | Not started | M | Domain layer (`form`, `workflows`) is unchanged; wiring only |
| Config-driven column-2 label rules + DIT tree-label rules | Not started | S | Reading from `EntryProfile`; pattern is clear |
| Config-discovery / config-picker startup flow | Not started | S | A startup `Dialog` before `Program::run_app` |
| Worker-pump (the spec's primary risk) | **RESOLVED** | — | `PumpView` + timer; no tvision-rs change |

**Total remaining estimate: ~5–8 person-weeks** at spike pace (exploratory + TDD),
~3–5 weeks at full-migration pace (patterns now known). The bulk is the rich
widgets and the save/validate wiring.

**Riskiest piece: `ObjectClassPicker`.** edaptor's existing implementation uses a
custom multi-select list with profile-bundle grouping. tvision-rs has no multi-select
`ListBox` out of the box; this either requires subclassing `ListViewer` or
hand-rolling the selection state on top of a plain `ListBox`. The spike did not
exercise this path.

---

## 5. Go / No-Go Against Spec §7 Success Criteria

| Criterion | Status | Evidence |
|---|---|---|
| 1. Three-pane `Splitter` renders with `Outline` │ `ListBox`+search │ form `Group` | **CONFIRMED LIVE** | Confirmed by user at a terminal (2026-06-23): the three-pane layout renders against the demo server. |
| 2. End-to-end navigation via real `worker → read_flow → structure` layer: expand → list → form | **CONFIRMED LIVE** | Confirmed by user (2026-06-23): tree navigation and the leaf→form chain both work end-to-end against real LDAP data. |
| 3. Splitter dividers resize (mouse drag + `Ctrl-F5` keyboard mode) | **CONFIRMED LIVE** | Confirmed by user (2026-06-23) in the interactive run. |
| 4. Umlaut/grapheme editing in `InputLine` and search box correct | **PROVEN by automated test** | `tests/spike_tv_umlaut.rs`: 2 tests, both PASS. Types `"Müller Zürich"`, backspaces onto `'ü'`, no panic, no byte-split. Green light. |
| 5. Findings doc with all three streams | **DONE** | This document. |

### Recommendation: **GO** — all success criteria met

The central blocker that caused the original migration away from Turbo Vision —
the `InputLine` UTF-8 panic — is **provably gone** in tvision-rs 0.1.0. That was
the explicit pre-condition for any re-engagement with the framework.

The worker-pump question (spec §5, outcome 1 or 2 required) resolved as **outcome
1**: tvision-rs already has an idle/timer hook; no library change is needed.

All five §7 success criteria are now satisfied: criterion 4 (umlaut/grapheme) is
proven by automated test, and criteria 1–3 (three-pane render, end-to-end
navigation incl. the leaf→form chain, and splitter resize) were **confirmed live
by the user at a terminal on 2026-06-23**. No criterion remains outstanding.

The full migration should proceed with its own plan via the writing-plans flow.

The crate's documentation gaps (§2) and the two minor feature gaps (§3.6, §3.7,
§3.8) are improvement candidates for upstream tvision-rs but are not blockers for
the migration.
