# Task: report the by-value-argument miscompilation to rust-lang/rust

**Status:** not started. Self-contained — you do not need any other document,
the edaptor tree, or a running LDAP server to do this. Everything needed to
reproduce is inline below.

**Why this exists:** while chasing edaptor's release-only "footer one row too
high" bug (2026-07-27) we found the cause was not in edaptor or in tvision-rs but
in rustc: safe code with no `unsafe` anywhere produces different results at
`opt-level=0` versus `opt-level>=1`. edaptor has been *worked around* (commit
`ce26ef7` / `fa76018`, `src/ui/app.rs`) but the compiler bug itself is unreported.
Background on how it was found: `docs/superpowers/research/2026-07-27-rustc-factory-rect-miscompile.md`.

## 1. The bug in one paragraph

When the same by-value struct is passed to several generic
`impl FnOnce(T) -> …` parameters, and the callee mutates its own copy, rustc
does not re-materialise the argument for the later calls. The second and third
callee receive whatever the *previous* callee wrote into its copy instead of the
caller's unchanged value. The caller's own variable is intact — only what
arrives at the callee is wrong.

## 2. Reproducer

A single file, no dependencies. `cargo new rectbug`, drop this in `src/main.rs`:

```rust
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Point { pub x: i32, pub y: i32 }
impl Point {
    pub fn new(x: i32, y: i32) -> Self { Point { x, y } }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Rect { pub a: Point, pub b: Point }
impl Rect {
    pub fn new(ax: i32, ay: i32, bx: i32, by: i32) -> Self {
        Rect { a: Point::new(ax, ay), b: Point::new(bx, by) }
    }
}

pub trait View { fn bounds(&self) -> Rect; }

struct Leaf(Rect, #[allow(dead_code)] String);
impl View for Leaf {
    fn bounds(&self) -> Rect { self.0 }
}

type Shared = Rc<RefCell<Vec<String>>>;

#[inline(never)]
fn size() -> (u16, u16) { (80, 25) }

fn init_desktop(r: Rect, state: Shared) -> Option<Box<dyn View>> {
    let mut r = r;
    r.a.y += 1;
    r.b.y -= 1;
    state.borrow_mut().push("desktop".into());
    Some(Box::new(Leaf(r, "desktop".into())))
}

fn init_status_line(r: Rect, state: Shared) -> Option<Box<dyn View>> {
    println!("  callee init_status_line received r={r:?}");
    let mut r = r;
    r.a.y = r.b.y - 1;
    state.borrow_mut().push("status".into());
    Some(Box::new(Leaf(r, "status".into())))
}

fn init_menu_bar(r: Rect) -> Option<Box<dyn View>> {
    let mut r = r;
    r.b.y = r.a.y + 1;
    Some(Box::new(Leaf(r, "menu".into())))
}

pub struct Program { pub children: Vec<Rect> }

impl Program {
    pub fn new(
        create_desktop: impl FnOnce(Rect) -> Option<Box<dyn View>>,
        create_status_line: impl FnOnce(Rect) -> Option<Box<dyn View>>,
        create_menu_bar: impl FnOnce(Rect) -> Option<Box<dyn View>>,
    ) -> Self {
        let (w, h) = size();
        let extent = Rect::new(0, 0, w as i32, h as i32);
        let mut children = Vec::new();
        if let Some(v) = create_desktop(extent) { children.push(v.bounds()); }
        if let Some(v) = create_status_line(extent) { children.push(v.bounds()); }
        if let Some(v) = create_menu_bar(extent) { children.push(v.bounds()); }
        Program { children }
    }
}

fn main() {
    let state: Shared = Rc::new(RefCell::new(Vec::new()));
    let s = state.clone();
    let status_state = state;
    let p = Program::new(
        move |r| init_desktop(r, s.clone()),
        move |r| init_status_line(r, status_state.clone()),
        init_menu_bar,
    );
    for (i, b) in p.children.iter().enumerate() {
        println!("child {i}: {b:?}");
    }
    let expect = [
        Rect::new(0, 1, 80, 24),
        Rect::new(0, 24, 80, 25),
        Rect::new(0, 0, 80, 1),
    ];
    if p.children == expect { println!("OK"); }
    else { println!("MISCOMPILED (expected {expect:?})"); }
}
```

Run it:

```bash
cargo run --release --config 'profile.release.opt-level=0'   # prints OK
cargo run --release --config 'profile.release.opt-level=1'   # prints MISCOMPILED
```

### Expected vs actual (opt-level >= 1)

`extent` is `(0,0)-(80,25)` and is never mutated by `Program::new`, so all three
factories must see it. Instead:

```
  callee init_status_line received r=Rect { a: (0,1), b: (80,24) }   <-- init_desktop's mutated copy
child 0: Rect { a: (0,1),  b: (80,24) }   correct
child 1: Rect { a: (0,23), b: (80,24) }   WRONG, expected (0,24)-(80,25)
child 2: Rect { a: (0,1),  b: (80,2)  }   WRONG, expected (0,0)-(80,1)
```

The `println!` inside the callee is the key evidence: the wrong value is already
there on entry, before the callee touches anything.

## 3. Affected versions

Reproduced on `x86_64-unknown-linux-gnu`:

| rustc | LLVM | opt-level 0 | opt-level 1/2/3 |
|---|---|---|---|
| 1.93.1 | 21.1.8 | OK | MISCOMPILED |
| 1.95.0 | 22.1.2 | OK | MISCOMPILED |
| 1.96.0 | 22.1.2 | OK | MISCOMPILED |

Spanning two LLVM majors suggests a rustc/MIR-level problem (MIR inlining is the
obvious suspect — it turns on at `opt-level=1`, which is exactly where the bug
starts) rather than an LLVM pass. **This has not been verified** — see §5.

## 4. What narrows it down

Each row is a one-change variant of the reproducer above. These were run; the
results are measured, not guessed.

| Variant | Result |
|---|---|
| baseline as written | **MISCOMPILED** |
| `impl Fn(Rect)` instead of `impl FnOnce(Rect)` | **MISCOMPILED** |
| callee builds a fresh `Rect` instead of `let mut r = r` | OK |
| factories take `&Rect`, callee does `let mut r = *r` | OK |
| caller wraps each argument: `create_x(std::hint::black_box(extent))` | OK |
| direct calls to the three `init_*` fns — no closures, no generics | OK |
| caller passes a freshly built `Rect::new(0,0,w,h)` at each call site | **MISCOMPILED** |

That last row matters: writing a *new* value at each call site does **not** help,
only forcing it through `black_box` does. So the caller's store is being elided
or hoisted, not merely reused.

Ingredients that appear necessary: a by-value struct argument, reached through a
generic closure parameter, mutated by the callee, with several call sites sharing
the value. Plain `fn f(mut x: T)` called directly is unaffected, which is why this
has not blown up everywhere.

## 5. What to do

1. **Confirm it still reproduces** on current stable and on nightly. If nightly is
   clean, find the fix and check whether a backport is warranted.
2. **Try to attribute it.** On nightly, `-Zinline-mir=no` will confirm or refute
   the MIR-inlining hypothesis in one run. `-Zmir-opt-level=0` is the blunter
   version. If MIR inlining is the culprit, `-Zdump-mir` around
   `Program::new` should show the missing re-initialisation directly.
3. **Reduce further if cheap.** The `Rc<RefCell<…>>` state, the `println!`, the
   `Box<dyn View>` return and the third factory are probably all removable. Do not
   over-invest — the current form is already small and self-contained, and
   rust-lang triage is used to reductions of this size.
4. **File at <https://github.com/rust-lang/rust/issues>.** Search first for an
   existing report (terms: "miscompile", "by-value argument", "FnOnce", "argument
   not re-materialised", "stale argument"). Label-wise this is
   `I-unsound`/`C-bug`; the issue template asks for the code, the command, the
   expected and actual output, and `rustc -vV` — all of which are above. Mention
   that it reproduces back to 1.93.1, so it is not a recent regression.
5. **Record the issue number** back in
   `docs/superpowers/research/2026-07-27-rustc-factory-rect-miscompile.md` (its
   closing section says "Not yet filed") and in `CHANGES.md` if a release is
   pending.

## 6. Related decision, still open

tvision-rs's `Program::new` is what invites this pattern: it hands each factory a
`Rect` by value and expects the factory to shrink it. Changing those three
parameters to `&Rect` would make the framework immune regardless of what rustc
does. That is a **breaking API change** for tvision-rs (source at
`/home/oetiker/checkouts/rstv`, but note it is at 0.13.1 while edaptor pins
0.14.0 — read the real 0.14.0 from the cargo registry cache) and needs a release
plus a bump in edaptor. Not required to keep edaptor correct; edaptor's own fix
already holds. oetiker's call.
