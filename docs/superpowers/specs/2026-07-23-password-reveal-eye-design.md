# InputLine masking + reveal-eye — design

Supersedes the earlier "build a masked field around InputLine" approach. The
capability is added to **tvision-rs** (`../rstv`) so edaptor can delete its
fragile masking mirror and every tvision app gains a real password field.

## Why in the framework, not around it

edaptor's `MaskedInputLine` masks by keeping the password **twice** — bullets in
an inner `InputLine`, cleartext in a parallel `real` string — and syncing them on
every edit. Both password bugs we just fixed were the two halves of one
**mirror desync** (paste wrote cleartext into the bullet buffer and left `real`
empty; backspace then indexed `real` with the bullet caret and panicked). Native
masking stores the password **once** and masks only at draw: caret, selection,
scroll, paste, backspace are all InputLine's own tested code, so the desync bug
class cannot exist. It is also reusable — the same pattern edaptor already
upstreams (Shuttle, ListViewer, InputLine bracketed paste).

## Terminal constraint

crossterm (no Kitty keyboard protocol) delivers **only `KeyDown`** — there is no
key-release. So keyboard "hold to reveal, release to hide" is impossible; the
keyboard peek is time-boxed instead. Mouse press/release *is* available
(`MouseDown`/`MouseUp` with `MouseTrackCapture`, the `widgets::button` pattern),
so the mouse behaviour is a true hold.

## Glyphs

Single-width (ambiguous-width → 1 col in Western terminals, same class as `•`):

- **Revealed:** `◉` U+25C9 FISHEYE
- **Hidden:** `⊝` U+229D CIRCLED DASH

Both configurable (see `RevealEyeConfig`).

---

## Layer 1 — tvision-rs (`../rstv`)

### `InputLine` — masking (pure enhancement)

- `mask: Option<char>` + `set_mask(Option<char>)`. When `Some(ch)`, `draw`
  paints `ch` per grapheme of the visible window; the stored `data` / `value()`
  stay the real text.
- `reveal: bool` + `set_reveal(bool)`. Transient; when true, `draw` paints the
  real glyphs despite `mask`.
- Clipboard safety: while masked **and not** revealed, Cut/Copy emit nothing
  (no cleartext to the clipboard); Paste still inserts into the real data.
- Default (`mask = None`) leaves InputLine byte-for-byte unchanged.

### `RevealEye` — a focusable one-cell view (new)

Its own Tab stop; drives a paired field's `set_reveal`.

- Draws `◉` when the paired field is revealed, else `⊝`.
- **Mouse:** `MouseDown` → reveal + `MouseTrackCapture`; `MouseUp` (routed back)
  → hide. A true momentary peek.
- **Space** (while focused):
  - default (non-sticky): a **timed peek** — reveal, arm a one-shot timer
    (`ctx.set_timer(duration, None)`, default 1 s), hide on `Event::Timer`.
    Pressing Space again re-arms.
  - sticky mode (configured): **toggle** a latched reveal on/off; no timer.
- `RevealEyeConfig { hidden_glyph: '⊝', revealed_glyph: '◉', peek: Duration(1s),
  sticky: bool }`.
- Tab / arrows / other keys pass through so the owner keeps field navigation.

### `MaskedInput` — composite (new)

The consumer-facing widget: an `InputLine` (with `mask` set) plus a `RevealEye`
in its **last column**, bundled as a group and pre-wired.

- Layout: the input occupies width − 1; the eye sits in the last column.
- Tab order within the group: input caret → eye → (exit). This is how the eye
  is "its own Tab stop" without giving InputLine an internal sub-focus.
- Internal wiring: the eye's reveal intent drives the input's `set_reveal` — no
  broadcast needed, they share a parent.
- `value()` / cleartext delegate to the inner input.
- Optional: hide the eye unless the group (input or eye) is focused, so only the
  **active** row shows one.
- Builder passes through `RevealEyeConfig` + the mask char.

Release `../rstv` as **0.14.0** (new feature → minor bump).

---

## Layer 2 — edaptor

- Replace `MaskedInputLine` with `MaskedInput` (mask `•`, non-sticky → Space =
  1 s peek). **Delete the mirror** and everything it forced: `real`,
  `insert_char_masked`, `mutate_real`, the per-event masking, the CUT/COPY/PASTE
  swallow, and the backspace bounds-guard (all now native / unnecessary).
- `PasswordDialog` reads each field's cleartext via `value()`; the New==Confirm
  `valid()` gate we just added **stays**.
- Active-line only: the eye shows on the focused field's row (via `MaskedInput`'s
  focus-gated eye).
- During development edaptor points `tvision-rs` at `path = "../rstv"`; once
  0.14.0 is published, pin the crates.io version and drop the path.

## Testing

**tvision (`../rstv`):**
- `InputLine`: masked draw paints the echo char while `value()` returns real
  text; `set_reveal(true)` paints real; caret/selection/scroll unaffected;
  masked Cut/Copy emit nothing, Paste inserts.
- `RevealEye`: `MouseDown` reveals + `MouseUp` hides; Space non-sticky arms a
  timed reveal and `Event::Timer` hides it; Space sticky toggles on/off; glyph
  tracks state.
- `MaskedInput`: Tab visits input then eye; eye reveals the paired input; eye
  reserved to the last column; focus-gated eye visibility.

**edaptor:**
- New/Confirm fields mask and reveal via the widget; `PasswordDialog::value`
  reads cleartext; the OK match-gate still vetoes a mismatch; no mirror remains.

## Implementation order (two plans)

1. tvision-rs: InputLine masking + `RevealEye` + `MaskedInput`, tests, 0.14.0.
2. edaptor: adopt `MaskedInput`, delete the mirror, wire the dialog.

## Out of scope

- Reveal on the read-only `‹set›/‹unset›` cell in the form (editor dialog only).
- Any change to how passwords are hashed/staged.
