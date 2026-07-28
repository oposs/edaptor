# Every field selectable: read-only fields become navigable, scrollable, copyable

**Date:** 2026-07-28
**Status:** design, approved in conversation
**Scope:** panel 3 (the entry form pane) + a new `read_only` mode in tvision-rs

## Goal

Today a form field is either editable (focusable, scrollable, copyable) or
read-only (`state.disabled` — invisible to focus, so its value can never be
scrolled, inspected or copied). A value wider than its cell is simply
unreachable: the operator sees `cn=admin,dc=example,dc=or►` and has no way to
read the rest short of widening the terminal.

**Every field in the form becomes selectable, regardless of whether it can be
edited.** Read-only fields take a `↑↓` stop, scroll horizontally, and support
selection + `Ctrl+C`. Attempting to *change* one pops a dialog explaining that
the field is read-only. `PageUp`/`PageDown` still page the whole viewport, so
a long form stays fast to traverse.

This subsumes three separate complaints: long DNs cut at the cell edge, long
member lists that cannot be inspected, and read-only values that cannot be
copied out.

## Non-goals

- No change to what is *editable*. Read-only stays read-only; this is about
  reaching and reading values, not writing them.
- No new configuration. Applies uniformly to every form field.
- The `dn` title on the pane's rule is not a field and stays as it is (cut with
  a `…`).

## Decisions taken

| Question | Decision |
|---|---|
| Feedback on an edit attempt | **Modal popup**, dismissed with Enter/Esc |
| Where the behaviour lives | **Upstream in tvision-rs** (`InputLine::read_only`) |
| Selection + clipboard | **Yes** — select and `Ctrl+C` work on read-only fields |

**A note on the popup.** It fires on any mutating keystroke, so scanning a form
with a stray character key will interrupt. That is the accepted trade for
"impossible to miss". If it turns out to nag in daily use, the natural follow-up
is to pop it only on the first attempt per form and fall back to a status-line
message — the mechanism below supports that without redesign, since the pane
decides what to do with the signal.

## Part 1 — `InputLine::read_only` (upstream, in `../rstv`)

### Why upstream and not in edaptor

The alternative was to keep the cell enabled and drop mutating keys in `FormPane`
before they reach the widget. Every mutation inside `InputLine` funnels through a
small set of places — `apply_input_command` (`BACK_SPACE`, `DEL_CHAR`,
`DEL_WORD`, `DEL_WORD_LEFT`, `DEL_LINE`, `CUT`, `PASTE`), the `Key::Char` insert
arm, `Event::Paste` (bracketed paste), and the deferred `paste_text` the pump
calls after an async clipboard read. Replicating that list from outside means
re-deriving it every time the widget grows a new editing path; the async paste
in particular arrives *after* the keystroke, where an interceptor no longer sees
a key at all. The widget is the only place that knows what mutates.

### The mode

`InputLine` gains `read_only: bool` (default `false`) with
`set_read_only(bool)` / `is_read_only()`. It is orthogonal to `disabled`:

| | `disabled` | `read_only` | normal |
|---|---|---|---|
| Takes focus | no | **yes** | yes |
| Caret / horizontal scroll | — | **yes** | yes |
| Select + copy | — | **yes** | yes |
| Cut / paste / type / delete | — | **rejected** | yes |

When read-only, the widget:

- **allows** `CHAR_LEFT/RIGHT`, `WORD_LEFT/RIGHT`, `LINE_START/END`,
  `SELECT_ALL`, `COPY`, mouse positioning, drag-select, double-click select-all,
  and all scrolling;
- **rejects** `BACK_SPACE`, `DEL_CHAR`, `DEL_WORD`, `DEL_WORD_LEFT`, `DEL_LINE`,
  `CUT`, `PASTE`, the `Key::Char` insert, `Event::Paste`, and `paste_text`;
- on each rejection, consumes the event and calls
  `ctx.broadcast(READ_ONLY_REJECTED, Some(self_id))`.

`READ_ONLY_REJECTED` is a new command constant. Broadcast (not a polled flag)
because it carries the *source view id*, so the owner can name the field in its
message, and because the deferred-paste rejection happens outside any keystroke
the owner could poll around.

Cut/copy command graying (`update_commands`) additionally disables `cmCut` and
`cmPaste` while read-only, leaving `cmCopy` driven by the selection as now — so
a menu route cannot bypass the mode.

### Rendering

A read-only field paints the same non-focused surface it does today, so the form
does not suddenly grow a field of white wells. When it *holds* focus it paints
the focused well like any other field — the operator must be able to see where
the caret is. Distinguishing read-only visually (a dimmer well) is deliberately
left out of this pass: the popup and the status line carry the meaning, and the
theme work is a separate decision.

### Release

Develop in `../rstv` behind a `path` dependency, then tag a release and bump the
`tvision-rs` requirement in `Cargo.toml` back to the published crate — the
repo's standard upstreaming loop.

## Part 2 — edaptor: every field a stop

- `cell_focusable` disappears in its current role. Every `Text` cell is built
  focusable; the ones that were disabled become `read_only` instead. The
  `disabled` flag stays only for cells that are genuinely inert.
- `focusable_value_ids` returns **all** value ids, so `↑↓` reaches every row.
  `PageUp`/`PageDown` are unchanged (they page the viewport, not the focus).
- `FormPane::handle_event` gains a `READ_ONLY_REJECTED` broadcast arm: resolve
  the source id to a field index, and pop a dialog naming the attribute and why
  it cannot be edited — server-maintained (the audit block), schema
  `NO-USER-MODIFICATION`, or a fan-out/computed field. The reason text comes
  from the field, so the dialog is specific rather than generic.
- **A test inverts.** `meta_rows_are_not_tab_stops` (added with the audit block)
  asserts the opposite of the new rule; it becomes
  `meta_rows_are_stops_but_reject_edits`.

## Part 3 — the list and launch blocks

`ListValueView` (inline multi-value editor) and `LaunchValueView` (the read-only
bullet blocks: members, objectClass, password) paint their lines with `put_str`,
which cuts silently at the right edge. A long member DN is therefore unreadable
even once the block has focus — the same defect as Part 1, in edaptor's own
views.

Each gains a horizontal offset, moved by `←`/`→` when the block holds focus and
its widest line exceeds the cell, with `►`/`◄` markers matching `InputLine`'s.
The offset resets to 0 when focus leaves the block, so a field never sits
scrolled when the operator returns to it.

This is the part that "solves the whole scrolling over long lists of members
thing", and it is independent of Parts 1–2: it can land before or after them.

## Sequencing

1. **Part 1** in `../rstv` (+ its own unit tests), released and consumed.
2. **Part 2** in edaptor — the payoff for read-only text fields.
3. **Part 3** in edaptor — the list/launch blocks.

Parts 1+2 must land together (Part 2 needs the mode). Part 3 stands alone.

## Testing

Upstream (`../rstv`), unit:

- read-only rejects each mutating command and each paste route, and the data is
  unchanged after every one;
- read-only allows every navigation command, selection, and copy;
- a rejection broadcasts `READ_ONLY_REJECTED` carrying the view id;
- `cmCut`/`cmPaste` are grayed while read-only;
- a read-only field is focusable (contrast: a disabled one is not).

edaptor, unit:

- every field — including the audit block and `<not set>` rows — appears in
  `focusable_value_ids`;
- `↑↓` steps onto a read-only field, and the caret homes there like anywhere
  else;
- a `READ_ONLY_REJECTED` broadcast pops the dialog with the attribute named;
- `PageUp`/`PageDown` still page the viewport with every row focusable;
- list/launch blocks scroll horizontally and reset the offset on focus loss.

Live, against the podman demo server: step through a person entry with `↓` only,
scroll a long `creatorsName` DN to its end and copy it, attempt to type into it
and dismiss the dialog, then scroll a group's member list sideways.

## Documentation

- `CHANGES.md` — one entry, framed as "every field is now reachable".
- `docs/src/usage/three-pane.md` — the entry-form section: all fields take a
  stop, read-only ones scroll and copy but refuse edits.
- The audit-block paragraph's "they take no Tab stop" sentence becomes wrong and
  must be rewritten.
