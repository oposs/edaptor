# TV 1.2.0 API verification for 3-pane layout

Crate source root (all `file:line` references below are relative to it):
`/home/oetiker/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/turbo-vision-1.2.0/`

All signatures below are quoted verbatim from the crate source. Where something
the plan assumes does **not** exist, it is flagged in the section and repeated in
BLOCKERS / SURPRISES.

---

## A. THE `View` TRAIT CONTRACT

Trait declared at `src/views/view.rs:63`. There are exactly **6 methods with NO
default** (must be implemented by every custom `View`), one of which
(`get_palette`) is easy to miss because it sits far down the trait body:

Required (no default body):
```rust
fn bounds(&self) -> Rect;                                 // view.rs:64
fn set_bounds(&mut self, bounds: Rect);                   // view.rs:65
fn draw(&mut self, terminal: &mut Terminal);             // view.rs:66
fn handle_event(&mut self, event: &mut Event);          // view.rs:67
fn get_palette(&self) -> Option<crate::core::palette::Palette>;  // view.rs:380
```
Note: only those are truly required. `get_palette` returning `None` is the
"transparent / inherit from parent" choice (see Group, view.rs:717-721).

Everything else has a default and is overridden only as needed. Full list with
signatures and line numbers:

```rust
fn can_focus(&self) -> bool { false }                                   // view.rs:68  (override -> true for focusable panes)
fn set_focus(&mut self, focused: bool) { self.set_state_flag(SF_FOCUSED, focused); } // view.rs:74
fn is_focused(&self) -> bool { self.get_state_flag(SF_FOCUSED) }        // view.rs:79
fn options(&self) -> u16 { 0 }                                          // view.rs:84
fn set_options(&mut self, _options: u16) {}                            // view.rs:89
fn state(&self) -> StateFlags { 0 }                                     // view.rs:92  (override + a `state` field to hold SF_DRAGGING etc.)
fn set_state(&mut self, _state: StateFlags) {}                         // view.rs:97  (override)
fn set_state_flag(&mut self, flag: StateFlags, enable: bool) { ... }    // view.rs:102 (derived from state/set_state)
fn get_state_flag(&self, flag: StateFlags) -> bool { ... }             // view.rs:113
fn has_shadow(&self) -> bool { ... }                                    // view.rs:118
fn shadow_bounds(&self) -> Rect { ... }                                 // view.rs:123
fn update_cursor(&self, _terminal: &mut Terminal) {}                    // view.rs:135 (override to show a cursor when focused)
fn zoom(&mut self, _max_bounds: Rect) {}                               // view.rs:142
fn valid(&mut self, _command: CommandId) -> bool { true }              // view.rs:159
fn as_any(&self) -> &dyn std::any::Any { panic!(...) }                 // view.rs:165 (DEFAULT PANICS — override if you ever downcast)
fn as_any_mut(&mut self) -> &mut dyn std::any::Any { panic!(...) }     // view.rs:171 (DEFAULT PANICS)
fn dump_to_file(&self, terminal: &Terminal, path: &str) -> io::Result<()> { ... } // view.rs:176
fn is_default_button(&self) -> bool { false }                          // view.rs:189
fn button_command(&self) -> Option<u16> { None }                      // view.rs:196
fn set_list_selection(&mut self, _index: usize) {}                     // view.rs:202
fn get_list_selection(&self) -> usize { 0 }                           // view.rs:208
fn get_redraw_union(&self) -> Option<Rect> { None }                   // view.rs:216
fn clear_move_tracking(&mut self) {}                                   // view.rs:222
fn get_end_state(&self) -> CommandId { 0 }                            // view.rs:229
fn set_end_state(&mut self, _command: CommandId) {}                   // view.rs:235
fn make_global(&self, local_x: i16, local_y: i16) -> (i16, i16) { ... }// view.rs:252
fn make_local(&self, global_x: i16, global_y: i16) -> (i16, i16) { ... }// view.rs:269
fn draw_shadow(&self, terminal: &mut Terminal) { ... }                 // view.rs:278
fn label_link(&self) -> Option<ViewId> { None }                      // view.rs:333
fn init_after_add(&mut self) {}                                        // view.rs:341
fn constrain_to_parent_bounds(&mut self) {}                           // view.rs:348
fn set_palette_chain(&mut self, _node: Option<PaletteChainNode>) {}    // view.rs:354 (override + store the node — needed for child color mapping)
fn get_palette_chain(&self) -> Option<&PaletteChainNode> { None }     // view.rs:360 (override)
fn set_parent_bounds(&mut self, _bounds: Rect) {}                     // view.rs:366
fn map_color(&self, color_index: u8) -> Attr { ... }                  // view.rs:393 (do NOT override; it walks the palette chain)
```

**Minimum a custom `SplitContainer` / scrollable form pane must implement:**
`bounds`, `set_bounds`, `draw`, `handle_event`, `get_palette`, plus —
to participate in focus and color correctly — `can_focus`, `state`,
`set_state`, `set_palette_chain`, `get_palette_chain`. This is exactly the set
`DitOutline` overrides today (`src/ui/facade.rs:165-229`), which is the proven
template. `update_cursor` should also be forwarded so the focused input shows a
cursor.

Separate trait: `IdleView { fn idle(&mut self); }` at `view.rs:472` — only for
animation/timer views; not needed here.

---

## B. `Group` AS A CONTAINER

`Group` is `src/views/group.rs:14`. It DOES implement `View` (`impl View for
Group` at group.rs:423), so a `Group` can be used as a child view inside another
container **and** mounted directly on the desktop (Desktop::add takes
`Box<dyn View>`, desktop.rs:47 — a bare Group satisfies that; no Window
required). It does not draw a frame.

Requested signatures (all verified):
```rust
pub fn new(bounds: Rect) -> Self                                  // group.rs:25
pub fn with_background(bounds: Rect, background: Attr) -> Self    // group.rs:37
pub fn add(&mut self, mut view: Box<dyn View>) -> ViewId          // group.rs:49
pub fn set_initial_focus(&mut self)                              // group.rs:67
pub fn select_next(&mut self)                                    // group.rs:368
pub fn select_previous(&mut self)                                // group.rs:392
pub fn focused_child(&self) -> Option<&dyn View>                 // group.rs:360
pub fn child_at(&self, index: usize) -> &dyn View               // group.rs:96
pub fn child_at_mut(&mut self, index: usize) -> &mut dyn View    // group.rs:100
pub fn broadcast(&mut self, event: &mut Event, owner_index: Option<usize>) // group.rs:319
pub fn draw_sub_views(&mut self, terminal: &mut Terminal, start_index: usize, clip: Rect) // group.rs:344
// View::handle_event for Group:                                    group.rs:498
```
Also useful: `set_focus_to(index)` (group.rs:104), `clear_all_focus`
(group.rs:82), `child_by_id`/`child_by_id_mut` (group.rs:215/222),
`focus_by_view_id` (group.rs:114), `len`/`is_empty` (group.rs:88/92),
`remove(index)` (group.rs:191).

### Coordinate model (critical for layout)
`add()` converts the child's bounds from **relative to the Group's origin** to
**absolute** screen coords and stores absolute bounds (group.rs:49-65). So you
build each child with `Rect::new(0,0,w,h)`-style local coords and `add()`
offsets them. `Group::set_bounds` (group.rs:428-451) then moves AND resizes all
children by the delta — i.e. moving/resizing the Group drags every child with
it, and a size delta is added to each child's `b` corner (children grow with the
group). That last behaviour matters for a SplitContainer: see SURPRISE below.

### Event routing & Tab focus (group.rs:498-660)
- Mouse (`MouseDown`/`MouseMove`/`MouseUp`): hit-tests children in **reverse
  z-order** and routes to the one whose `bounds.contains(pos)` (group.rs:504-575).
  On `MouseDown` it gives that child focus if `can_focus()`.
- **Mouse capture exists** (group.rs:510-517): for `MouseMove`/`MouseUp`, if the
  focused child has `SF_DRAGGING | SF_RESIZING` set in its `state()`, the event
  is sent to that child **even when the cursor is outside its bounds**, and the
  function returns early. This is the hook a draggable divider needs (see D).
- Keyboard/Command: three-phase (PreProcess via `OF_PRE_PROCESS`, then focused
  child, then PostProcess via `OF_POST_PROCESS`), group.rs:582-619.
- **Tab cycling is built in** (group.rs:623-632): after three-phase processing,
  if the event is still a Keyboard event, `KB_TAB` -> `select_next()` + clear,
  `KB_SHIFT_TAB` -> `select_previous()` + clear. `select_next/previous`
  skip non-focusable children and wrap around (group.rs:368-420).
- Broadcast: sent to all children (group.rs:647-652).

### Recommendation for the 3-pane SplitContainer
**Embed a `Group` and add divider draw + drag on top — recommended, with one
caveat.** A `Group` already gives you: child ownership, the relative→absolute
coordinate conversion on `add`, Tab/Shift-Tab focus cycling across the 3 panes
for free, mouse routing with hit-test, and the SF_DRAGGING mouse-capture path.

Concretely: `SplitContainer { inner: Group, divider_x: [i16;2], state: StateFlags, ... }`.
Delegate `bounds/set_bounds/draw/handle_event/...` to `inner` (the `DitOutline`
delegation pattern, facade.rs:165-229), then:
- In `draw`, call `inner.draw(terminal)` then paint the two vertical divider
  columns yourself.
- In `handle_event`, intercept `MouseDown` on a divider column FIRST (before
  delegating): set `self.state |= SF_DRAGGING` and record which divider; on
  `MouseMove` while dragging, recompute pane widths and call
  `child_at_mut(i).set_bounds(...)` for the affected panes (absolute coords);
  on `MouseUp` clear SF_DRAGGING. For non-divider events, delegate to
  `inner.handle_event`.

Caveat / why you can't be 100% hands-off: divider drag means you must reposition
panes by directly calling `set_bounds` on the children with **absolute** rects
(remember Group stores absolute bounds). You cannot lean on `Group::set_bounds`
for repartitioning because it moves *all* children uniformly; per-pane resize is
your job. That is a small amount of code and does not fight the Group — the Group
just owns/focuses/draws/routes; you own the partition geometry. No part of
Group's layout actively gets in the way.

Note on the divider's own drag capture: Group's capture check looks at
`self.children[self.focused].state()`. Your divider is NOT a child — it's drawn
by the SplitContainer. So the SplitContainer itself must track drag state and
handle MouseMove/MouseUp directly (it receives them because IT is the focused
view in the desktop/window above it, and the desktop/Group above it applies the
same capture rule to the SplitContainer's own `state()`). Set `SF_DRAGGING` on
the SplitContainer's `state()` during a divider drag so the parent keeps feeding
it MouseMove after the cursor leaves the divider column.

---

## C. `Scroller` FOR THE FORM PANE

`Scroller` is `src/views/scroller.rs:14`. Full API:
```rust
pub fn new(bounds: Rect,
           h_scrollbar: Option<Box<ScrollBar>>,
           v_scrollbar: Option<Box<ScrollBar>>) -> Self        // scroller.rs:24
pub fn scroll_to(&mut self, x: i16, y: i16)                     // scroller.rs:38  (clamps to limit)
pub fn set_limit(&mut self, x: i16, y: i16)                     // scroller.rs:45  (content size; clamps delta)
pub fn get_delta(&self) -> Point                               // scroller.rs:57
pub fn get_limit(&self) -> Point                               // scroller.rs:62
pub fn draw_scrollbars(&mut self, terminal: &mut Terminal)      // scroller.rs:90
pub fn handle_scrollbar_events(&mut self, event: &mut Event)    // scroller.rs:101
// builder: ScrollerBuilder::new().bounds(..).v_scrollbar(..).h_scrollbar(..).build()  // scroller.rs:216-266
```
`update_scrollbars` is private (scroller.rs:67). `View` impl: scroller.rs:122;
its `draw` only draws the scrollbars (scroller.rs:154-158) and `handle_event`
only forwards to scrollbars (scroller.rs:160-162). `get_palette` returns
`CP_SCROLLER` (scroller.rs:172).

### How Scroller is actually used in this crate — KEY FINDING
**Nothing in the crate hosts child views inside a Scroller and offsets them by
the delta. `Scroller` holds NO children at all** — it has no `Vec<children>`,
no `add()`. Its struct is just `bounds, delta, limit, h_scrollbar,
v_scrollbar, palette_chain` (scroller.rs:14-21). It is a *base to embed*: a
concrete view stores a `Scroller` (or just a `delta`) and draws content shifted
by `delta` itself.

Searched the whole crate: the only references to `Scroller` are the module
declaration (`mod.rs:71`), `palette.rs` (the CP_SCROLLER palette), and a
**comment** in `chdir_dialog.rs:150`. `editor.rs`, `memo.rs`, and
`text_viewer.rs` do **not** use `Scroller` at all. Instead each one:
- keeps its own `delta: Point` field (editor.rs:101, memo.rs:29,
  text_viewer.rs:19),
- owns its own `Option<Box<ScrollBar>>` directly (text_viewer.rs:21-22),
- in `draw` reads lines/cols at `delta.y + y` / `delta.x` and paints them
  (editor.rs:1262/1269, text_viewer.rs:241),
- clamps `delta` against content size by hand and calls `ScrollBar::set_params`
  itself (text_viewer.rs:133-150).

So the established crate pattern for "scrollable content" is: **own a `delta`,
draw content offset by it, drive a `ScrollBar` directly.** Child *views* are
never repositioned by a scroll delta anywhere in the codebase.

### Decision: vertically-scrolling FORM of label+InputLine rows
Three candidate approaches were on the table:
1. Scroller + manually reposition child InputLine bounds by delta each draw.
2. Wrap children in a Group and translate.
3. Hand-roll scrolling entirely.

**Recommendation: a hybrid of (1)+(2) — own a `Group` of the rows plus a manual
vertical `delta`/`ScrollBar`, and on each scroll change reposition the rows by
calling `Group::set_bounds` (which shifts ALL children uniformly).** This is the
lowest-risk path that still reuses the InputLine focus/editing machinery:

- The rows (label `StaticText` + `InputLine`) live in an inner `Group`. The
  Group gives you Tab cycling between InputLines and the InputLine cursor/edit
  behaviour for free (same as the edit dialog, facade.rs:699).
- Keep your own `v_scroll_delta: i16` and a `Box<ScrollBar>`.
- The visible viewport is the pane bounds. On a scroll event, set the Group's
  bounds origin to `pane.a.y - v_scroll_delta` (i.e. move the whole row block
  up). Because `Group::set_bounds` moves every child by the same delta
  (group.rs:441-450), one call repositions all rows. Then push a clip rect for
  the viewport in `draw` (Group::draw already pushes a clip of its own bounds,
  group.rs:471-476 — but its bounds will extend above/below the viewport, so the
  SplitContainer/host should push the *viewport* clip before calling the inner
  group's draw, or set the group bounds to the viewport and rely on children
  being drawn outside being clipped by the terminal). Clamp delta to
  `total_rows_height - viewport_height`.

Why not pure (1): manually re-`set_bounds`-ing each InputLine every draw is more
code and more error-prone (absolute-coord bookkeeping) than letting
`Group::set_bounds` shift the block once per scroll change.

Why not pure (3): hand-rolling loses InputLine editing/cursor and Tab focus,
which the Group provides.

**Caution — the SF_RESIZING grow-with-parent caveat:** because
`Group::set_bounds` also adds the *size* delta to each child's `b` corner
(group.rs:443-448), only change the Group's **origin** (`a`) when scrolling, and
keep its width/height equal to the content block — otherwise rows will stretch.
Concretely: scroll by constructing a new Rect that moves `a.y` and moves `b.y`
by the same amount (pure translation, zero size delta). Verify with a quick
unit-style check that `dw == 0 && dh == 0` for the scroll rect.

Exact calls for the scroll step:
```rust
let dy = new_delta - self.v_scroll_delta;          // signed row delta
let gb = self.rows.bounds();                        // current Group bounds (absolute)
self.rows.set_bounds(Rect::new(gb.a.x, gb.a.y - dy, gb.b.x, gb.b.y - dy)); // pure translate, dw=dh=0
self.v_scroll_delta = new_delta;
self.vbar.set_params(new_delta as i32, 0, max_delta as i32, viewport_h as i32, 1); // scrollbar.rs:87
```
Plus clip the viewport in `draw` so rows scrolled off-pane don't paint over the
divider/other panes.

---

## D. DIVIDER DRAG MECHANICS

### Event types & mouse field path (`src/core/event.rs`)
`EventType` enum (event.rs:121-132):
```rust
pub enum EventType {
    Nothing, Keyboard,
    MouseDown, MouseUp, MouseMove, MouseAuto,
    MouseWheelUp, MouseWheelDown,
    Command, Broadcast,
}
```
Mouse-down/move/up are `EventType::MouseDown`, `EventType::MouseMove`,
`EventType::MouseUp`. There is also `MouseAuto` (auto-repeat while held) and
wheel variants.

Mouse data struct (event.rs:153-159) and its place on `Event` (event.rs:182-189):
```rust
pub struct MouseEvent {
    pub pos: Point,        // <-- position
    pub buttons: u8,       // bit flags, MB_LEFT_BUTTON=0x01 (event.rs:149)
    pub double_click: bool,
}
pub struct Event {
    pub what: EventType,
    pub key_code: KeyCode,
    pub key_modifiers: KeyModifiers,
    pub mouse: MouseEvent,
    pub command: CommandId,
}
```
**Field path to the cursor position: `event.mouse.pos.x` / `event.mouse.pos.y`**
(`Point.x`, `Point.y` are `i16`, geometry.rs:23-24). Button test:
`event.mouse.buttons & MB_LEFT_BUTTON != 0` (MB_LEFT_BUTTON=0x01, event.rs:149).
Event constructors: `Event::mouse(event_type, pos, buttons, double_click)`
(event.rs:231), `Event::command(cmd)` (event.rs:215), `Event::broadcast(cmd)`
(event.rs:223), `Event::keyboard(key_code)` (event.rs:206), `event.clear()`
sets `what = Nothing` (event.rs:250).

Key codes confirmed: `KB_TAB=0x0F09` (event.rs:18), `KB_SHIFT_TAB=0x0F00`
(event.rs:19), `KB_PGUP=0x4900` (event.rs:44), `KB_PGDN=0x5100` (event.rs:45),
`KB_UP/DOWN/LEFT/RIGHT` (event.rs:37-40), `KB_HOME/END` (event.rs:42-43).

### `SF_RESIZING` and the mouse-capture mechanism (`src/core/state.rs`)
**`SF_RESIZING` exists**: `pub const SF_RESIZING: StateFlags = 0x2000;`
(state.rs:24, commented "Window is being resized (Rust-specific)"). Companion
`SF_DRAGGING: StateFlags = 0x080;` (state.rs:18). `StateFlags = u16`
(state.rs:8). Other relevant flags: `SF_FOCUSED=0x040` (state.rs:17),
`SF_VISIBLE=0x001`, `SF_SELECTED=0x020`, `SF_CLOSED=0x1000`.

**Mouse capture is real and is implemented in `Group::handle_event`**
(group.rs:510-517), quoted:
```rust
if (event.what == EventType::MouseMove || event.what == EventType::MouseUp)
    && self.focused < self.children.len() {
    let child_state = self.children[self.focused].state();
    if (child_state & (crate::core::state::SF_DRAGGING | crate::core::state::SF_RESIZING)) != 0 {
        self.children[self.focused].handle_event(event);
        return;
    }
}
```
So: a child that sets `SF_DRAGGING` (or `SF_RESIZING`) in its own `state()`
keeps receiving `MouseMove`/`MouseUp` from its parent Group **even when the
cursor leaves its bounds** — until it clears the flag. This is the capture you
need for a divider drag.

### The real resize/drag pattern (Window + Frame)
This is the template to copy. The flag lives on the view; the parent Group
honours it; the view clears it on MouseUp.

`Frame::handle_event` (frame.rs:194-253) — starts the gesture on MouseDown:
```rust
// resize corner hit:
self.state |= SF_RESIZING;   // frame.rs:206  (does NOT clear event; lets Window read it)
// title-bar hit (not close button):
self.state |= SF_DRAGGING;   // frame.rs:224
...
} else if event.what == EventType::MouseUp {
    if (self.state & SF_DRAGGING) != 0 { self.state &= !SF_DRAGGING; event.clear(); }   // frame.rs:245-247
    else if (self.state & SF_RESIZING) != 0 { self.state &= !SF_RESIZING; event.clear(); } // frame.rs:248-250
}
```
`Window::handle_event` (window.rs:473-612) — drives the gesture: it forwards to
the frame, then notices `frame.state() & SF_DRAGGING/RESIZING`, on the first
MouseDown/MouseMove records an offset (`self.drag_offset` / `resize_start_size`)
and **also sets `self.state |= SF_DRAGGING`** (window.rs:486) so its OWN parent
keeps capturing; on subsequent MouseMove it recomputes bounds and calls
`set_bounds` on frame + interior (window.rs:506-557 drag, 560-600 resize); when
the frame's flag clears it tears down (window.rs:603-611).

The takeaway for the divider: the SplitContainer plays Window's role. On
`MouseDown` over a divider column set `self.state |= SF_DRAGGING` and stash which
divider + the click offset; on `MouseMove` while `SF_DRAGGING` recompute pane
widths and `set_bounds` the affected child panes; on `MouseUp` clear
`SF_DRAGGING` and `event.clear()`. The parent (desktop or host Window's interior
Group) will keep delivering MouseMove because of the capture rule above — *as
long as the SplitContainer is the focused child of that parent*.

---

## E. INPUT_LINE + LISTBOX

### InputLine (`src/views/input_line.rs`)
```rust
pub fn new(bounds: Rect, max_length: usize, data: Rc<RefCell<String>>) -> Self  // input_line.rs:39
pub fn with_validator(bounds, max_length, data, validator: ValidatorRef) -> Self // input_line.rs:57
pub fn set_text(&mut self, text: String)   // input_line.rs:83
pub fn get_text(&self) -> String           // input_line.rs:91  (clones the bound String)
```
**Data binding type is `Rc<RefCell<String>>`** (input_line.rs:27). The bound
value is read either via `get_text()` (input_line.rs:91-93) or directly from the
shared `Rc<RefCell<String>>` the caller still holds — `borrow().clone()`. This
is exactly how the edit dialog reads edited values (facade.rs:637-638:
`data.borrow().clone()`), and how it constructs inputs
(facade.rs:699: `InputLine::new(Rect::new(30, y, width - 2, y + 1), 1024, data.clone())`).
`InputLine::can_focus() -> true` (input_line.rs:410); it stores `SF_FOCUSED` in
its own `state` field and shows a cursor in `update_cursor` (input_line.rs:425).
Builder: `InputLineBuilder::new().bounds(..).data(..).max_length(..).build()`
(input_line.rs:481-547; default max_length 255).

### ListBox (`src/views/listbox.rs`)
```rust
pub fn new(bounds: Rect, on_select_command: CommandId) -> Self  // listbox.rs:30
pub fn set_items(&mut self, items: Vec<String>)                 // listbox.rs:42 (resets range)
pub fn add_item(&mut self, item: String)                        // listbox.rs:48
pub fn clear(&mut self)                                         // listbox.rs:54
pub fn get_selection(&self) -> Option<usize>                   // listbox.rs:60 (selected index)
pub fn get_selected_item(&self) -> Option<&str>                // listbox.rs:65
pub fn set_selection(&mut self, index: usize)                  // listbox.rs:72
pub fn item_count(&self) -> usize                              // listbox.rs:80
pub fn select_prev/next/first/last/page_up/page_down(&mut self)// listbox.rs:88-121
```
Construct: `ListBox::new(bounds, on_select_command)` then `set_items(vec)`.
After `set_items`/`add_item`, `set_range` is called and selection defaults to
item 0 (tests at listbox.rs:357). Selected index: `get_selection()`; selected
text: `get_selected_item()`.

**Reacting to selection-change:** ListBox does NOT emit an event on mere
selection movement (arrows). It only emits `Event::command(on_select_command)`
on **Enter** (listbox.rs:215-218) and on **double-click** (listbox.rs:185-202).
Single-click/arrows update selection silently via the embedded `ListViewer`
(`handle_list_event`, list_viewer.rs:287). So to react to every selection change
you poll `get_selection()` after handling the event (the same poll-after-event
pattern the facade uses for the outline, facade.rs:192-193), not by waiting for a
command. `ListBox` overrides `as_any`/`as_any_mut` (listbox.rs:275-281), so it is
safe to downcast — unlike most views.

### SortedListBox (`src/views/sorted_listbox.rs`) differences
- Items kept sorted automatically; `add_item` inserts at the binary-search point
  (sorted_listbox.rs:77), `set_items` sorts (sorted_listbox.rs:70).
- Case-insensitive by default; `set_case_sensitive(bool)` (sorted_listbox.rs:62).
- Adds search: `find_exact(&str) -> Option<usize>` (sorted_listbox.rs:116),
  `find_prefix(&str) -> Option<usize>` (sorted_listbox.rs:130),
  `focus_prefix(&str) -> bool` (sorted_listbox.rs:206).
- **It does NOT override `as_any`** (no downcast), and its `handle_event` only
  calls `handle_list_event` — there is an explicit `// TODO: Add incremental
  search on keyboard input` (sorted_listbox.rs:297). So SortedListBox gives you
  the *search primitives* but no built-in type-to-filter; and it does not emit an
  on-select command (its `_on_select_command` is stored but unused,
  sorted_listbox.rs:42).

### Which for an incrementally-filtered, re-populated-on-every-keystroke leaf list?
**Plain `ListBox`.** Reasons, from the source:
- You are filtering and **re-populating the whole list on each keystroke**, so
  you already compute the filtered subset yourself; `ListBox::set_items(vec)`
  (listbox.rs:42) replaces the contents in one call. You don't need
  SortedListBox's auto-sort or binary search — sort the filtered Vec yourself
  before `set_items` if you want order.
- The keystrokes belong to a **separate filter `InputLine`**, not to the list.
  ListBox's own `handle_event` would consume printable keys for navigation only
  (it ignores them), so a dedicated filter InputLine + `ListBox::set_items` is
  the clean split.
- `ListBox` supports `as_any`/downcast (listbox.rs:275) and exposes
  `get_selection`/`get_selected_item`, which you need to act on the chosen leaf.
- SortedListBox's `find_prefix` would only help if you kept the FULL list loaded
  and jumped to a prefix instead of filtering — that's a different UX than
  "re-populate on every keystroke", and SortedListBox can't react-on-select
  anyway.

Pattern: a small `Group` (or the SplitContainer pane) holding a filter
`InputLine` (bound `Rc<RefCell<String>>`) on top and a `ListBox` below; on each
event, read the filter string (`data.borrow()`), recompute the matching leaf
names, `listbox.set_items(filtered)`, then read `listbox.get_selection()` to
know the current leaf.

---

## BLOCKERS / SURPRISES

1. **`Scroller` hosts no children and is used by nothing in the crate.** It is a
   delta/limit + scrollbar holder only (scroller.rs:14-21); editor/memo/
   text_viewer all roll their own `delta` and own their `ScrollBar`s directly
   and never touch `Scroller`. The plan's "Scroller-based scrolling form" must be
   reframed: there is no crate-supported way to put `InputLine`s "inside" a
   Scroller. Use a `Group` of rows + a manual `delta`/`ScrollBar`, translating
   the Group's origin on scroll (Section C). Risk is real but contained.

2. **`Group::set_bounds` resizes children, not just moves them**
   (group.rs:443-448 adds the width/height delta to each child's `b`). For
   scroll translation you must keep `dw == dh == 0` (move `a` and `b` by the same
   amount) or your InputLines will stretch. Easy to get wrong.

3. **Divider drag is not free from `Group`.** The Group's mouse-capture only
   inspects `children[focused].state()` (group.rs:512). The divider is drawn by
   the SplitContainer, not a child, so the SplitContainer itself must set
   `SF_DRAGGING` on its *own* `state()` and handle MouseMove/MouseUp directly;
   it then relies on *its* parent (desktop / host Window interior Group) applying
   the same capture rule to it. This works only while the SplitContainer is the
   focused child of that parent. Confirmed viable (it's exactly how Window rides
   Frame's flag, window.rs:478-486), but it's a two-level dance, not a one-liner.

4. **`as_any()` default panics** (view.rs:165-166). Any custom view you might
   later downcast (or that the framework downcasts) must override `as_any`/
   `as_any_mut`. The proven `DitOutline` wrapper deliberately avoids downcasting
   for this reason (facade.rs comments at 124-128). `ListBox` overrides it;
   `SortedListBox` and `OutlineViewer` do not.

5. **ListBox/SortedListBox do not emit "selection changed" events.** Selection
   movement is silent; only Enter/double-click emit a command (listbox.rs:185-218),
   and SortedListBox emits nothing. Plan must poll `get_selection()` after each
   event (facade's documented poll-after-event pattern), not subscribe to a
   change event. No type-to-filter exists in SortedListBox (explicit TODO,
   sorted_listbox.rs:297).

6. **A frameless `Group` CAN be mounted directly** (Desktop::add /
   Group-as-child both accept `Box<dyn View>`; Group impls View at group.rs:423),
   so the "frameless Group-based SplitContainer" is supported. But Group draws no
   border and is "transparent" to the palette (`get_palette` -> None,
   group.rs:717-721); panes that need a background must paint it themselves or
   use `Group::with_background` (group.rs:37).

7. **`get_palette` is a required method that's easy to overlook** (view.rs:380,
   no default) sitting amid ~40 defaulted methods. Returning `None` (inherit) is
   fine, but forgetting it is a compile error — flagging so it isn't mistaken for
   optional.
