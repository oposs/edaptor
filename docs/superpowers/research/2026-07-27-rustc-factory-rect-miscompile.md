# The release-only "footer one row too high" bug is a rustc miscompilation

*2026-07-27 — root cause found, fixed in `src/ui/app.rs`.*

## Symptom

Deployed (release) edaptor binaries laid the whole UI out one row short: the
status line drew on the second-to-last terminal row and the bottom row stayed
dead; the entry form lost its last line. `cargo run` (debug) was always correct,
at every terminal height, which is why the bug only ever appeared on installed
binaries and resisted local reproduction for several sessions.

## What it is not

Ruled out with measurements, not reasoning:

- **Not a bad terminal-size read.** A `Backend` decorator logging every `size()`
  call recorded 80x25 on all 41 calls of a run that exhibited the bug — and 20k
  calls in an earlier one. The size is read correctly and continuously.
- **Not a startup race** at alt-screen entry (sleeps of 0–100 ms change nothing).
- **Not clipping or a short render buffer.** `Buffer` is built from the same
  correct `size()`.
- **Not the resize path.** A resize preserves the offset exactly, because
  `change_bounds` applies a *delta* — the error is baked in at construction.
- **Not a heisenbug.** The earlier "adding `note()` calls makes it vanish"
  finding was an artefact of a broken test harness (see below).

## Root cause

`tvision_rs::Program::new` builds its three children from injected factories:

```rust
let extent = Rect::new(0, 0, w as i32, h as i32);
if let Some(view) = create_desktop(extent)     { ... }
if let Some(view) = create_status_line(extent) { ... }
if let Some(view) = create_menu_bar(extent)    { ... }
```

Each factory is reached through a separate `impl FnOnce(Rect) -> Option<Box<dyn View>>`
generic parameter, and edaptor's factories used to shrink their own copy in
place (`let mut r = r; r.a.y += 1;`).

**Under `opt-level >= 1` rustc does not re-materialise the by-value `Rect`
argument for the second and third call.** Each factory receives whatever the
*previous* factory wrote into its copy. Instrumenting both sides showed the
caller's `extent` reading `(0,0)-(80,25)` while the callee received the
desktop's mutated `(0,1)-(80,24)`:

```
before create_status   extent=Rect { a: (0,0), b: (80,25) }
init_status_line CALLEE received r=Rect { a: (0,1), b: (80,24) }
status factory -> Rect { a: (0,23), b: (80,24) }      <-- should be (0,24)-(80,25)
```

`init_status_line` computes `a.y = b.y - 1`, so a `b.y` of 24 instead of 25 puts
the status line one row too high — exactly the reported symptom.

## Minimal reproduction

~100 lines of safe Rust, no dependencies, kept at `/home/oetiker/scratch/rectbug`
(`src/main.rs` is the baseline; `src/bin/*.rs` are the variants). It prints `OK`
at `opt-level=0` and `MISCOMPILED` at 1, 2 and 3, on rustc **1.93.1** (LLVM 21.1.8),
**1.95.0** and **1.96.0** (LLVM 22.1.2) — so it is not a recent regression and
spans two LLVM majors, which points at a rustc/MIR-level problem rather than an
LLVM pass.

Ingredients that matter:

| Variant | Result |
|---|---|
| baseline: `impl FnOnce(Rect)` closures, callee mutates its copy | **MISCOMPILED** |
| `impl Fn(Rect)` instead of `FnOnce` | **MISCOMPILED** |
| callee builds a fresh `Rect` instead of mutating | OK |
| factories take `&Rect` | OK |
| caller wraps each argument in `std::hint::black_box` | OK |
| direct calls — no closures, no generics | OK |

So the trigger is *a by-value struct argument that the callee mutates, passed
through a generic closure parameter, at several call sites sharing one value*.
Plain `fn f(mut x: T)` is unaffected, which keeps the blast radius small.

## The fix

`init_desktop`, `init_status_line` and `init_menu_bar` in `src/ui/app.rs` now
derive a fresh `Rect` instead of assigning through their parameter. That leaves
the shared argument slot pristine, so the missing re-materialisation is harmless.
The long comment on `init_desktop` explains why the obvious "simplification"
back to `let mut r = r` must not be made.

## Regression guard

The existing headless test `main_window_is_not_shifted_onto_the_footer` **does**
catch this — but only when the test binary itself is optimised:

```bash
cargo test --release --lib main_window_is_not_shifted_onto_the_footer
```

It failed on the unfixed tree and passes now. The default debug `cargo test`
(and therefore `make check`) cannot catch this class of bug. This corrects the
earlier belief that headless probes are blind to it — they are not, they were
just always built in debug.

## Harness note (cost several sessions)

Binaries copied into the agent scratchpad under `/tmp/claude-<uid>/…` **hang
during bootstrap** — they start but open no LDAP connection and never paint.
The identical binary run from `/home/oetiker/scratch/…` or the cargo target dir
works. Earlier sessions' "every binary hangs" and "the bug vanishes when I add
logging" conclusions came from that trap. Run test binaries from
`/home/oetiker/scratch/`.

## Upstream

Worth reporting to rust-lang/rust: safe code, no `unsafe` anywhere, differing
results between `opt-level=0` and `opt-level>=1`. The reproducer above is
self-contained. Not yet filed.
