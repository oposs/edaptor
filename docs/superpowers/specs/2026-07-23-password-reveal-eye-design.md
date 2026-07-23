# Password reveal-eye — design

## Goal

Add a "reveal password" affordance to the Set-password dialog's masked fields
(`MaskedInputLine`): a small eye control at the right end of the **active**
(focused) field that momentarily shows the typed password in the clear.

- **Mouse:** press-and-hold the eye → cleartext while held; release → back to
  bullets.
- **Keyboard:** with the eye focused, **Space** reveals for **1 second**, then
  auto-reverts.

## Why a timed keyboard reveal

The terminal (crossterm, no Kitty keyboard protocol negotiated) delivers **only
`KeyDown`** — there is no key-release event. So keyboard "hold to reveal, release
to hide" cannot detect the release. A one-shot 1 s timer (`ctx.set_timer(dur,
None)` → `Event::Timer`) gives a keyboard reveal that needs no release event.

Mouse press/release *is* available: `Event::MouseDown` / `Event::MouseUp`, with
`MouseTrackCapture` routing the `MouseUp` back to the presser (the exact pattern
`widgets::button` uses). So the mouse behaviour matches the request precisely.

**Enter is deliberately not a reveal key:** Enter broadcasts `Command::DEFAULT`,
which fires the dialog's OK button globally regardless of focus, so binding Enter
to reveal would trip the OK / password-match flow. Space only.

## Glyphs

Single-width (ambiguous-width, rendered 1 col in Western terminals — same class
as the existing `•` bullet), from the geometric/math blocks:

- **Revealed:** `◉` U+25C9 FISHEYE
- **Hidden:** `⊝` U+229D CIRCLED DASH

## Components

### `EyeToggle` (new focusable view)

- Single-cell, selectable view drawn as `◉` (revealed) / `⊝` (hidden).
- Holds only its *reveal intent*: `Off`, `Held` (mouse down, cleared on
  `MouseUp`), or `Timed` (Space pressed; the dialog owns the timer that clears
  it).
- `MouseDown` → intent `Held`, start a `MouseTrackCapture` (so `MouseUp` returns
  here), and broadcast `CMD_PW_REVEAL_CHANGED { source: self.id }`. `MouseUp` →
  intent `Off`, broadcast again.
- `Space` (`KeyDown`) → intent `Timed`, broadcast; the dialog arms the timer.
  All other keys pass through (Tab, arrows for field navigation).
- Never edits or holds any password text.

### `MaskedInputLine` (extend)

- Add `revealed: bool` and `set_revealed(bool)`.
- The stored model is unchanged: inner field holds bullets, `real` mirrors the
  cleartext 1:1 (this is what the existing masking/paste/backspace logic relies
  on).
- **Reveal rendering:** when `revealed`, `draw` shows `real` instead of the
  bullets by briefly substituting the inner field's display buffer with `real`
  — caret and selection remapped from their char index (already available via
  `caret_char` / `selection_chars`) to the corresponding byte offsets in `real`
  — drawing through the inner field so its scroll/caret/selection rendering is
  reused, then restoring the bullet buffer. Char counts are equal by construction,
  so single-width columns line up 1:1 (the mirror already assumes this mapping).
- Reserve the **last column** of the field for the eye: the editable width is
  the field width minus one, and the eye is drawn/hit in that last column.

### `PasswordDialog` (coordinator — already mediates keys / `valid` / staging)

- Insert an `EyeToggle` per masked field, positioned over each field's last
  column. Keep a `field_id → eye_id` (and reverse) mapping.
- Tab order: New → New-eye → Confirm → Confirm-eye → OK → Cancel.
- **Active-line only:** an eye is drawn only while its paired field or the eye
  itself is focused; otherwise it is hidden. So exactly the focused row shows an
  eye.
- On `CMD_PW_REVEAL_CHANGED` (checked after delegating, like `CMD_SHUTTLE_CHANGED`):
  read the source eye's intent and apply it to the paired field via
  `set_revealed`. For a `Timed` intent, arm a one-shot 1 s timer (`ctx.set_timer`),
  remembering `(timer_id → field_id)`; pressing Space again kills the pending
  timer and re-arms.
- On `Event::Timer(id)`: if `id` is a pending reveal timer, `set_revealed(false)`
  on its field and drop the mapping.
- Cancelling/closing the dialog leaves nothing revealed (fields are cleared on
  close as today).

## Security

Cleartext is shown only during a deliberate peek and always reverts — on mouse
release or after 1 s. No change to staging, and nothing secret is logged. The
reveal is transient view state, never persisted.

## Testing

- `EyeToggle`: `MouseDown` sets `Held` + broadcasts; `MouseUp` clears + rebroadcasts;
  `Space` sets `Timed` + broadcasts; Tab/arrows pass through; draw shows the right
  glyph per intent.
- `MaskedInputLine`: `set_revealed(true)` draw shows `real`; `false` shows bullets;
  caret position preserved across a reveal/hide cycle; the eye column is reserved
  (editable width = field width − 1).
- `PasswordDialog`: a reveal-changed broadcast from an eye toggles the *paired*
  field only; a `Timed` reveal arms a timer and the matching `Event::Timer` hides
  it; a second Space re-arms (kills the old timer); an eye is drawn only on the
  focused row.

## Out of scope

- No reveal on the read-only `‹set›/‹unset›` cell in the form (only inside the
  editor dialog).
- No config option to default-reveal or to disable masking.
- No change to the mirror architecture (a larger "store real, mask at draw"
  refactor that would also remove the desync bug class is noted but deferred).
