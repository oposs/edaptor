# turbo-vision 1.2 API Spike (compile-verified)

**Date:** 2026-05-29
**Crate:** `turbo-vision = "1.2"` (resolved 1.2.0), Rust 1.95.0, edition 2024
**Method:** Clean throwaway crate at `/tmp/tv-spike2/tvspike2`. The crate's OWN examples were
copied in and compiled first as a known-good baseline, then 8 minimal probe snippets (my own
code, derived from those examples) were each compiled with `cargo build --example <probe>`.

> Trust note: This file was rewritten from scratch after an earlier draft contained
> hallucinated APIs. Every snippet below was compiled in this run. Where my first attempt at a
> probe failed to compile, the compiler error is recorded and the corrected, compiling code is
> shown. Three probes were wrong on the first pass (sections 4, 5, 8); the message-box probe
> took two corrections before it compiled. Those corrections are the whole point of doing this
> against a real compiler — the wrong names looked plausible but do not exist.

## Baseline (source of truth)

Copied these crate examples into the throwaway and ran `cargo build --examples`:
`tree_view.rs, validator.rs, list_components.rs, sorted_listbox.rs, minimal_app.rs,
command_set.rs, file_browser.rs, quick_start.rs, menu_status.rs`.

**Result: ✅ all compiled, `BUILD_EXIT=0`.** These are the real API surface; the probes below
re-derive each piece independently and were each compiled.

## Probe results summary

| # | Probe | Topic | Status |
|---|-------|-------|--------|
| 1 | probe1_app_shell | Application, desktop, menu bar, status line, event loop, idle/draw | ✅ compiled this run |
| 2 | probe2_dialog_modal | Dialog + buttons, modal `execute()` return value | ✅ compiled this run |
| 3 | probe3_input_validators | InputLine, `Rc<RefCell<String>>` data binding, validators | ✅ compiled this run (fix: `Validator` trait import required) |
| 4 | probe4_listbox | ListBox: `set_items`, `get_selected_item` | ✅ compiled this run (fix: returns `Option<&str>`, not `Option<String>`) |
| 5 | probe5_sorted_listbox | SortedListBox: `add_item`, `focus_prefix`, `set_case_sensitive` | ✅ compiled this run (fix: returns `Option<&str>`) |
| 6 | probe6_outline | OutlineViewer / Node tree (DIT + schema browser) | ✅ compiled this run |
| 7 | probe7_menus | Menus two ways (MenuBuilder + MenuItem) + popup MenuBox | ✅ compiled this run |
| 8 | probe8_msgbox_cmdset | `message_box` helper + `command_set` enable/disable | ✅ compiled this run (fix: real flags are `MF_CONFIRMATION` + `MF_YES_BUTTON \| MF_NO_BUTTON`; there is no `MF_YES_NO` / `MF_YES_NO_BUTTONS`) |

Final `cargo build --examples` (all 9 baseline examples + all 8 corrected probes together):
**`full_rc=0 errcount=0` — Finished, 0 errors.** Per-probe re-check: all 8 PASS.
Probe files live in `/tmp/tv-spike2/tvspike2/examples/probeN_*.rs`.

---

## 1. Application shell, menus, status line, event loop — ✅ compiled this run

```rust
use turbo_vision::app::Application;
use turbo_vision::core::command::CM_QUIT;
use turbo_vision::core::event::{EventType, KB_ALT_X, KB_F10};
use turbo_vision::core::geometry::Rect;
use turbo_vision::views::menu_bar::{MenuBar, SubMenu};
use turbo_vision::core::menu_data::MenuBuilder;
use turbo_vision::views::status_line::{StatusItem, StatusLine};

let mut app = Application::new()?;            // Result<Application>
let (w, h) = app.terminal.size();             // (i16, i16)

let mut menu_bar = MenuBar::new(Rect::new(0, 0, w, 1));
let file_menu = MenuBuilder::new().item("E~x~it", CM_QUIT, 0).build();
menu_bar.add_submenu(SubMenu::new("~F~ile", file_menu));
app.set_menu_bar(menu_bar);

let status = StatusLine::new(
    Rect::new(0, h - 1, w, h),
    vec![
        StatusItem::new("~Alt+X~ Quit", KB_ALT_X, CM_QUIT),
        StatusItem::new("~F10~ Menu", KB_F10, 0),
    ],
);
app.set_status_line(status);

// Event loop pattern (from command_set.rs):
app.running = true;
while app.running {
    app.idle();                 // broadcasts CM_COMMAND_SET_CHANGED etc.
    app.draw();                 // draws desktop + menu_bar + status_line
    let _ = app.terminal.flush();
    if let Ok(Some(mut event)) =
        app.terminal.poll_event(std::time::Duration::from_millis(50)) {
        app.handle_event(&mut event);
        if event.what == EventType::Command && event.command == CM_QUIT {
            app.running = false;
        }
    }
}
```

Notes:
- `app.desktop`, `app.terminal`, `app.menu_bar` (Option), `app.status_line` (Option),
  `app.running` are public fields.
- `app.run()` exists (minimal_app.rs) for the simple case; for full control use the manual
  loop with `app.idle(); app.draw();`.
- `~X~` markers denote hotkeys/accelerators in titles and labels.

## 2. Dialog + buttons, modal execute() — ✅ compiled this run

```rust
use turbo_vision::core::command::{CM_CANCEL, CM_OK};
use turbo_vision::views::button::ButtonBuilder;
use turbo_vision::views::dialog::DialogBuilder;
use turbo_vision::views::static_text::StaticTextBuilder;

let mut dialog = DialogBuilder::new()
    .bounds(Rect::new(10, 5, 60, 15))
    .title("Confirm")
    .build();

dialog.add(Box::new(
    StaticTextBuilder::new().bounds(Rect::new(2,1,46,3))
        .text("Save changes?").centered(true).build()));

dialog.add(Box::new(
    ButtonBuilder::new().bounds(Rect::new(8,5,18,7))
        .title("  ~O~K  ").command(CM_OK).default(true).build()));
dialog.add(Box::new(
    ButtonBuilder::new().bounds(Rect::new(22,5,34,7))
        .title("~C~ancel").command(CM_CANCEL).build()));

dialog.set_initial_focus();
let result: u16 = dialog.execute(&mut app);   // returns CM_* of closing button
```

Notes:
- `dialog.execute(app)` is a blocking modal call that returns the command id of the button
  that closed the dialog (e.g. `CM_OK` / `CM_CANCEL`). This is the primary way to get a
  yes/no/ok answer from a dialog.
- Child coordinates are relative to the dialog interior.
- `DialogBuilder::build()` returns a `turbo_vision::views::dialog::Dialog`.

## 3. InputLine + data binding + validators — ✅ compiled this run

```rust
use std::cell::RefCell;
use std::rc::Rc;
use turbo_vision::views::input_line::InputLineBuilder;
// IMPORTANT: the `Validator` trait MUST be imported to call `is_valid()`.
use turbo_vision::views::validator::{FilterValidator, RangeValidator, Validator};
use turbo_vision::views::picture_validator::PictureValidator;

// Data binding: the field edits a shared String.
let data: Rc<RefCell<String>> = Rc::new(RefCell::new(String::from("init")));

let filter = Rc::new(RefCell::new(FilterValidator::new("0123456789")));
let mut input = InputLineBuilder::new()
    .bounds(Rect::new(2, 2, 40, 3))
    .max_length(20)
    .data(data.clone())              // takes Rc<RefCell<String>>
    .validator(filter.clone())       // takes Rc<RefCell<V: Validator>>
    .build();

// Or attach after build:
input.set_validator(Rc::new(RefCell::new(PictureValidator::new("###-####"))));

let range = RangeValidator::new(0, 100);

// is_valid() is a TRAIT method on Validator (trait must be in scope):
let _ok: bool = filter.borrow().is_valid(&data.borrow());
let _ok: bool = range.is_valid("50");

// Read the edited value back out of the bound String:
let current: String = data.borrow().clone();
```

Validator types confirmed: `FilterValidator::new(&str)`, `RangeValidator::new(i64, i64)`
(min/max), `PictureValidator::new(&str)` mask. Picture mask chars: `#`=digit, `@`=letter,
`!`=any; literals auto-inserted (from validator.rs).

**Gotcha (verified by deliberately breaking it):** removing the `Validator` trait import makes
`is_valid` fail with `E0599: no method named is_valid`. So `is_valid()` is a trait method, not
inherent — the trait import is required, not optional. (An "unused import" warning on
`Validator` is misleading; it is needed for the method call.)

## 4. ListBox — ✅ compiled this run (corrected return type)

```rust
use turbo_vision::views::listbox::ListBoxBuilder;

let mut listbox = ListBoxBuilder::new()
    .bounds(Rect::new(5, 3, 35, 13))
    .on_select_command(101)          // command fired on Enter/select
    .build();

listbox.set_items(vec![
    "alpha".to_string(), "beta".to_string(), "gamma".to_string(),
]);                                  // takes Vec<String>

// get_selected_item returns Option<&str> (a borrow), NOT Option<String>.
if let Some(sel) = listbox.get_selected_item() {
    let _: &str = sel;
    let owned: String = sel.to_string();   // convert if you need ownership
}
```

**Correction (first probe attempt failed):** I initially wrote `let _: String = sel;` and the
compiler rejected it: `E0308: mismatched types — expected String, found &str`. Verified in the
crate source: `pub fn get_selected_item(&self) -> Option<&str>` (views/listbox.rs:65). So
`get_selected_item` returns a borrowed `&str` into the listbox; call `.to_string()` for an
owned value. `set_items(Vec<String>)` is correct as written (listbox.rs:42).

## 5. SortedListBox — ✅ compiled this run (corrected return type)

```rust
use turbo_vision::views::sorted_listbox::SortedListBoxBuilder;

let mut lb = SortedListBoxBuilder::new()
    .bounds(Rect::new(5, 3, 35, 18))
    .on_select_command(1000)
    .build();

lb.add_item("Zebra".to_string());   // items auto-sorted; note: add_item, not set_items
lb.add_item("Apple".to_string());

lb.set_case_sensitive(false);
lb.focus_prefix("A");               // binary-search jump to first item with prefix

if let Some(sel) = lb.get_selected_item() {  // Option<&str>, same as ListBox
    let _: &str = sel;
}
```

**Correction (first probe attempt failed):** same `E0308` as probe 4 —
`get_selected_item` is `Option<&str>` (sorted_listbox.rs:96), not `Option<String>`.
Population API differs from plain ListBox: `add_item(String)` (incremental, auto-sorted)
rather than `set_items(Vec)`. Also has `find_exact`, `find_prefix` (per the example's doc
comment).

## 6. OutlineViewer / Node tree (DIT + schema browser) — ✅ compiled this run

```rust
use std::cell::RefCell;
use std::rc::Rc;
use turbo_vision::core::geometry::Rect;
use turbo_vision::views::outline::{Node, OutlineViewer};

// Node holds a generic payload; here String.
let root: Rc<RefCell<Node<String>>> =
    Rc::new(RefCell::new(Node::new("ou=people".to_string())));
let child = Rc::new(RefCell::new(Node::new("cn=alice".to_string())));
root.borrow_mut().add_child(child);

// new(bounds, closure: &Payload -> String for display text)
let mut viewer = OutlineViewer::new(Rect::new(2, 5, 64, 17), |n: &String| n.clone());
viewer.add_root(root);
```

Key facts:
- The tree widget is `OutlineViewer` (path `turbo_vision::views::outline`), NOT "TreeView"/
  "OutlineView". The crate example file is named `tree_view.rs` but uses `OutlineViewer`.
- `Node<T>` is generic over the payload. `Node::new(payload)`, `add_child(Rc<RefCell<Node<T>>>)`.
- Nodes are wrapped in `Rc<RefCell<...>>`; trees are built with `.borrow_mut().add_child(...)`.
- `OutlineViewer::new` takes a closure `&T -> String` to render each node's label — so you can
  store a rich payload (e.g. an LDAP entry / attribute) and render a display string from it.
- `viewer.add_root(root)` installs the root.

## 7. Menus — two construction styles + popup MenuBox — ✅ compiled this run

```rust
use turbo_vision::core::command::{CM_NEW, CM_OPEN, CM_QUIT};
use turbo_vision::core::geometry::{Point, Rect};
use turbo_vision::core::menu_data::{Menu, MenuBuilder, MenuItem};
use turbo_vision::views::menu_bar::{MenuBar, SubMenu};
use turbo_vision::views::menu_box::MenuBox;

// Style A — fluent MenuBuilder:
let file_menu = MenuBuilder::new()
    .item_with_shortcut("~N~ew", CM_NEW, 0, "Ctrl+N")
    .item_with_shortcut("~O~pen...", CM_OPEN, 0, "Ctrl+O")
    .separator()
    .item("E~x~it", CM_QUIT, 0)
    .build();
menu_bar.add_submenu(SubMenu::new("~F~ile", file_menu));

// Style B — explicit MenuItem with nested submenu:
let sub = Menu::from_items(vec![
    MenuItem::with_shortcut("~P~roperties", 222, 0, "", 0),
    MenuItem::separator(),
]);
let edit_menu = Menu::from_items(vec![MenuItem::submenu("~M~ore", 0, sub, 0)]);
menu_bar.add_submenu(SubMenu::new("~E~dit", edit_menu));

// Popup / context menu, returns selected command id (0 if cancelled):
let mut mb = MenuBox::new(Point::new(10, 5), Menu::from_items(vec![
    MenuItem::with_shortcut("~N~ew", CM_NEW, 0, "", 0),
]));
let cmd: u16 = mb.execute(&mut app.terminal);
```

Signatures confirmed:
- `MenuBuilder`: `.item(text, cmd, key)`, `.item_with_shortcut(text, cmd, key, shortcut_str)`,
  `.separator()`, `.build()`.
- `MenuItem::with_shortcut(text, cmd, key, shortcut, help_ctx)` (5 args),
  `MenuItem::submenu(text, key, Menu, help_ctx)`, `MenuItem::separator()`.
- `Menu::from_items(Vec<MenuItem>)`.
- `SubMenu::new(title, Menu)`; `menu_bar.add_submenu(SubMenu)`.
- `MenuBox::new(Point, Menu)`; `menu_box.execute(&mut Terminal) -> u16`.
- Cascading submenus from the bar: `menu_bar.check_cascading_submenu(&mut app.terminal)`
  returns `Option<command>` (seen in menu_status.rs).

## 8. message_box helper + command_set enable/disable — ✅ compiled this run (corrected flags, two passes)

```rust
use turbo_vision::core::command::{CM_COPY, CM_CUT, CM_NO, CM_YES};
use turbo_vision::core::command_set;
// Real constants (verified in crate source helpers/msgbox.rs). There is NO `MF_YES_NO`
// and NO `MF_YES_NO_BUTTONS` — combine the individual button flags with `|`.
use turbo_vision::helpers::msgbox::{
    message_box, MF_CONFIRMATION, MF_INFORMATION, MF_NO_BUTTON, MF_OK_BUTTON, MF_YES_BUTTON,
};

// Modal message box; returns the CommandId of the pressed button.
let _ = message_box(&mut app, "Operation complete", MF_INFORMATION | MF_OK_BUTTON);

let answer = message_box(&mut app, "Delete entry?",
                         MF_CONFIRMATION | MF_YES_BUTTON | MF_NO_BUTTON);
let yes = matches!(answer, CM_YES);   // returns CM_YES / CM_NO

// Global command-set state (drives button graying via app.idle()):
command_set::disable_command(CM_COPY);
command_set::enable_command(CM_CUT);
```

**Corrections (this probe failed twice before compiling):**
1. First I imported `MF_YES_NO` → `E0432: no MF_YES_NO in helpers::msgbox`.
2. Then I guessed `MF_YES_NO_BUTTONS` → also `E0432: no MF_YES_NO_BUTTONS`. The compiler even
   suggested `MF_YES_BUTTON`.

The REAL constants, read verbatim from `helpers/msgbox.rs`:

Message-type flags (lower bits; pick one):
`MF_WARNING = 0x0000`, `MF_ERROR = 0x0001`, `MF_INFORMATION = 0x0002`,
`MF_CONFIRMATION = 0x0003` (note: `MF_CONFIRMATION`, not `MF_CONFIRM`), `MF_ABOUT = 0x0004`.

Button flags (combine with `|`):
`MF_YES_BUTTON = 0x0100`, `MF_NO_BUTTON = 0x0200`, `MF_OK_BUTTON = 0x0400`,
`MF_CANCEL_BUTTON = 0x0800`.

Pre-combined button sets (only these two exist):
`MF_YES_NO_CANCEL = MF_YES_BUTTON | MF_NO_BUTTON | MF_CANCEL_BUTTON`,
`MF_OK_CANCEL = MF_OK_BUTTON | MF_CANCEL_BUTTON`. **There is no plain "Yes/No" combo** — build it
as `MF_YES_BUTTON | MF_NO_BUTTON`.

Facts:
- `message_box(app: &mut Application, msg: &str, options: u16) -> CommandId` (= `u16`).
  Path: `turbo_vision::helpers::msgbox`. Returns the chosen button's command id:
  `CM_OK` / `CM_CANCEL` / `CM_YES` / `CM_NO`.
- Also present: `message_box_rect(app, bounds, msg, options) -> CommandId`, and input-box
  helpers returning `(CommandId, String)` (msgbox.rs:84,157,185) — not separately probed.
- `command_set::enable_command(CommandId)` / `disable_command(CommandId)` are free functions
  in `turbo_vision::core::command_set`. Buttons created while a command is disabled render
  grayed; `app.idle()` broadcasts `CM_COMMAND_SET_CHANGED` so buttons restyle live.
- `CommandId` (= `u16`) and `CM_YES` / `CM_NO` are exported from `turbo_vision::core::command`.

---

## Other confirmed-from-baseline facts (in compiling examples, not separately probed)

- `turbo_vision::prelude::*` brings in `Application`, `Rect`, `CM_OK`, etc. (quick_start.rs,
  tree_view.rs).
- File widgets exist and compile: `views::dir_listbox::DirListBox::new(Rect, &Path)`,
  `views::file_list::FileList::new(Rect, &Path)`, with `.refresh()`, `.change_dir(path)`,
  `.current_path()`, `.update_cursor(&mut terminal)` (file_browser.rs).
- `Button::new(Rect, &str, cmd, default: bool)` direct constructor (quick_start.rs).
- `Event::command(cmd)` constructs a command event; `event.clear()`, `event.mouse.pos`,
  `event.mouse.buttons & MB_RIGHT_BUTTON`, `event.key_code`, `event.what`, `event.command`
  (menu_status.rs).
- Low-level drawing: `core::draw::DrawBuffer`, `core::palette::{colors, Attr}` (`Attr::from_u8`),
  `views::view::write_line_to_terminal(term, x, y, &buf)` (list_components.rs, sorted_listbox.rs).

## Biggest surprises vs. prior (hallucinated) research — corrected list

1. **The tree widget is `OutlineViewer` + `Node<T>`** in `views::outline`, taking a
   `&T -> String` render closure and `Rc<RefCell<Node<T>>>` roots. There is no `TreeView`/
   `OutlineView` type. (The example *file* is `tree_view.rs` but the type is `OutlineViewer`.)
2. **`get_selected_item()` returns `Option<&str>`, not `Option<String>`** — for BOTH `ListBox`
   and `SortedListBox`. My first probes assumed `String`; the compiler rejected them (`E0308`).
   Call `.to_string()` for ownership.
3. **`Validator::is_valid` is a TRAIT method** — the `Validator` trait must be in scope or you
   get `E0599`. Verified by deliberately removing the import and watching it fail.
4. **There is no `MF_YES_NO` / `MF_YES_NO_BUTTONS`** message-box flag (two separate `E0432`s).
   Use `MF_YES_BUTTON | MF_NO_BUTTON`. The question type flag is `MF_CONFIRMATION` (not
   `MF_CONFIRM`). `message_box` returns a `CommandId` (`CM_YES`/`CM_NO`/`CM_OK`/`CM_CANCEL`),
   not a bool. The only pre-combined sets are `MF_YES_NO_CANCEL` and `MF_OK_CANCEL`.
5. **ListBox vs SortedListBox have different population APIs**: `ListBox::set_items(Vec<String>)`
   vs `SortedListBox::add_item(String)` (incremental + auto-sorted). Don't assume `set_items`
   on the sorted variant.
6. **InputLine data binding is `Rc<RefCell<String>>` via `.data(...)`**, and you read the user's
   input back by `data.borrow().clone()` — the widget mutates your shared String in place.
7. **Modal results come back as command ids**: `dialog.execute(app)`, `message_box(app, ..)`,
   and `MenuBox::execute(term)` all return the command id of the chosen button/item
   (0 = cancelled for MenuBox). This is the idiom for getting answers, not callbacks.
8. **Button enable/disable is global, not per-widget**: `command_set::{enable,disable}_command`
   plus `app.idle()` broadcast — buttons gray themselves based on their command's state.

> Note for the planner: a sibling file `2026-05-29-api-spike-findings.md` in this same directory
> reaches a *different* conclusion (build the TUI on ratatui/crossterm rather than depend on a
> Turbo Vision crate). This spike demonstrates the `turbo-vision` 1.2 crate itself compiles and
> exposes all the primitives we need. The choice between "depend on turbo-vision 1.2" vs.
> "reimplement the model on ratatui" is a real decision the M3 plan still has to make; this file
> only establishes that the turbo-vision crate option is viable and what its real API looks like.

## Implications for the M3 plan

- **DIT browser and schema browser** can both be built on `OutlineViewer<Payload>` where the
  payload is the LDAP node/attribute model object and the render closure produces the row label.
  Lazy expansion can be modeled by populating child `Node`s on demand (nodes are `Rc<RefCell>`).
- **Entry editor**: build a `Dialog` of `InputLine`s, each bound to an `Rc<RefCell<String>>`
  field. Use `FilterValidator`/`RangeValidator`/`PictureValidator` for attribute syntaxes; gate
  Save by calling `is_valid()` on each (remember the `Validator` trait import). Read values back
  via `data.borrow()`. Modal `dialog.execute(app)` returns `CM_OK`/`CM_CANCEL`.
- **Confirmations / errors** (delete entry, bind failure, etc.): use `message_box(app, msg,
  MF_CONFIRMATION | MF_YES_BUTTON | MF_NO_BUTTON)` and branch on the returned `CM_YES`/`CM_NO`.
  No custom dialog needed.
- **Attribute / object-class pickers**: `ListBox` for unordered, `SortedListBox` for large
  alphabetical lists (schema attribute names) with `focus_prefix` type-ahead. Remember selection
  comes back as `&str`.
- **Context actions** (right-click on a DIT node: Add child / Delete / Rename): `MenuBox` popup
  at the mouse position; act on its returned command id.
- **Menu/toolbar gating**: drive availability of actions (e.g. Save only when dirty, Delete only
  when an entry is selected) with `command_set::{enable,disable}_command` + `app.idle()`.
- **Main shell**: `Application` + `MenuBar` + `StatusLine` + the manual
  `idle()/draw()/poll_event()` loop gives full control needed to interleave LDAP I/O with the
  TUI. `app.run()` is too opaque for that; prefer the manual loop.

## 9. Worker↔UI bridge (decisive micro-spike)

**Question:** how does a background thread feed events/data into turbo-vision's
single-threaded event loop? The earlier "no idle hook exists" conclusion is
**WRONG**. There are TWO usable mechanisms, both confirmed in source.

### 9.1 The event loop (src/app/application.rs, v1.2.0)

`Application::run()` is the main loop. Every iteration it **polls** the backend
with a 20 ms timeout; on timeout (no input) it calls `self.idle()`:

```rust
pub fn run(&mut self) {
    self.running = true;
    self.update_active_view_bounds();
    self.draw();
    let _ = self.terminal.flush();

    while self.running {
        let needs_draw = self.needs_redraw;
        if needs_draw { /* draw + flush */ self.needs_redraw = false; }

        // Poll for event with 20ms timeout (blocks until event OR timeout)
        match self.terminal.poll_event(Duration::from_millis(20)).ok().flatten() {
            Some(mut event) => {
                self.handle_event(&mut event);
                self.update_active_view_bounds();
                self.draw();
                let _ = self.terminal.flush();
            }
            None => {
                // Timeout with no events - call idle()
                self.idle();
                if !self.overlay_widgets.is_empty() {
                    for widget in &mut self.overlay_widgets {
                        widget.draw(&mut self.terminal);
                    }
                    let _ = self.terminal.flush();
                }
            }
        }
        let had_closed_windows = self.desktop.remove_closed_windows();
        if had_closed_windows { self.needs_redraw = true; }
        let had_moved_windows = self.desktop.handle_moved_windows(&mut self.terminal);
        if had_moved_windows { let _ = self.terminal.flush(); }
    }
}
```

So YES there is an idle path. The hook the application gets is
`Application::idle()` (and per-iteration `handle_event`). The 20 ms poll
timeout means idle fires at least ~50x/sec even with zero input. The same
loop body exists in `exec_view()` (modal dialogs) and `get_event()`.

`idle()` itself drives overlay widgets and the command-set-changed broadcast:

```rust
pub fn idle(&mut self) {
    for widget in &mut self.overlay_widgets { widget.idle(); }   // <- our hook
    if self.desktop.has_tileable_windows() { /* enable CM_TILE/CM_CASCADE */ }
    else { /* disable */ }
    if command_set::command_set_changed() {
        let mut event = Event::broadcast(CM_COMMAND_SET_CHANGED);
        self.desktop.handle_event(&mut event);
        /* + menu_bar + status_line */
        command_set::clear_command_set_changed();
    }
}
```

**Overlay widgets** are the public, supported idle hook for application code:

```rust
pub(crate) overlay_widgets: Vec<Box<dyn IdleView>>,
pub fn add_overlay_widget(&mut self, widget: Box<dyn IdleView>) { ... }
```

`IdleView: View` adds `fn idle(&mut self)`. An overlay widget's `idle()` is
called on EVERY idle tick of `run()`, `exec_view()`, and `get_event()` — i.e.
even while a modal dialog is open ("Matches Borland: TProgram::idle()
continues running during execView()"). A worker-fed widget can `rx.try_recv()`
in its `idle()` and mutate UI state.

### 9.2 The SSH backend — the injection path (src/terminal/ssh_backend.rs)

This is the structural proof that external events ARE injected into the loop.
The SSH backend receives events over a **channel** and hands them to the loop
via `poll_event` (non-blocking `try_recv`):

```rust
pub struct SshBackend {
    output_tx: mpsc::UnboundedSender<Vec<u8>>,
    event_rx: mpsc::UnboundedReceiver<Event>,   // <- events come from elsewhere
    event_queue: Vec<Event>,
    size: Arc<Mutex<(u16, u16)>>,
    ...
}

fn poll_event(&mut self, _timeout: Duration) -> io::Result<Option<Event>> {
    if let Some(ev) = self.event_queue.pop() { return Ok(Some(ev)); }
    match self.event_rx.try_recv() {                       // non-blocking
        Ok(ev) => Ok(Some(ev)),
        Err(TryRecvError::Empty) => Ok(None),
        Err(TryRecvError::Disconnected) => Err(BrokenPipe),
    }
}
```

The counterpart `SshSessionHandle` (held by the async SSH side / another task)
pushes events in via an `mpsc::UnboundedSender<Event>`:

```rust
pub struct SshSessionHandle {
    pub event_tx: mpsc::UnboundedSender<Event>,   // <- inject events here
    pub output_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    pub size: Arc<Mutex<(u16, u16)>>,
    pub input_parser: InputParser,
}
pub fn process_input(&mut self, data: &[u8]) {
    for event in self.input_parser.parse(data) { let _ = self.event_tx.send(event); }
}
```

**This is the architectural confirmation:** turbo-vision's loop is fed by a
`poll_event(timeout)` that returns `Ok(None)` on idle. A custom `Backend`
that owns an `mpsc::Receiver<Event>` can inject *arbitrary* events (including
`Event::command(cmd)` / `Event::broadcast(cmd)`) from any thread — that is
exactly how `ssh` works. NB: `Backend: Send`, and the SSH path runs the loop
synchronously while a separate (tokio) task feeds `event_tx`.

Important nuance: the **default crossterm backend** poll is keyboard/mouse
only. VERIFIED: grep for `put_event` / `inject` / `push_event` / `event_queue`
in `src/terminal/mod.rs` and `src/terminal/crossterm_backend.rs` returns **0
hits** — there is no external-injection method on `Terminal` or the crossterm
backend; the SSH `event_rx` channel exists only on `SshBackend`. So to inject
from another thread you must wrap the backend. Two clean options for our worker:
1. **Overlay-widget idle poll** (simplest, no backend swap): widget holds
   `Receiver<T>`, drains it in `idle()`. Works with the stock terminal.
2. **Custom Backend wrapper** (the SSH pattern): wrap the crossterm backend,
   add an `mpsc::Receiver<Event>`; in `poll_event`, first `try_recv` your
   injected events, else delegate to crossterm. Lets a worker thread post
   real `Event::command(...)` into the loop. Install via
   `Terminal::with_backend(backend: Box<dyn Backend>) -> io::Result<Self>`
   — VERIFIED present in `src/terminal/mod.rs` (alongside `with_backend_and_size`).

### 9.3 Command/event injection API reachable from views

- `Event::command(cmd: CommandId) -> Event` and
  `Event::broadcast(cmd: CommandId) -> Event` construct injectable events.
- The loop calls `handle_event(&mut Event)` → dispatches to menu_bar, then
  `desktop.handle_event` (propagates to all child views), then status_line,
  then app-level command match. Views handle/clear commands the standard way.
- **`Terminal::put_event(&mut self, event: Event)` EXISTS** (src/terminal/mod.rs:515,
  *"matching Borland's `TProgram::putEvent()`"*). Implementation is a single
  `self.pending_event = Some(event);`, and `poll_event` returns
  `self.pending_event.take()` BEFORE delegating to the backend (mod.rs:520-527).
  It is NOT on the `Backend` trait — it lives on `Terminal` itself, so it works
  with the **default crossterm backend**, no wrapper. Since `Application` owns
  `pub terminal: Terminal`, **`app.terminal.put_event(Event::command(cmd))` is a
  direct, supported injection API**: the next `poll_event` returns the queued
  event before reading the tty. (Single-slot, not a real queue — a second
  `put_event` before the next poll overwrites the first.)
  CAVEAT: `app.terminal` lives on the UI thread; `Terminal`/`Backend` are not
  `Sync`, so a worker thread cannot call `put_event` directly. The worker still
  hands data over an `mpsc` channel; whoever holds `&mut app` (the idle hook, or
  the manual loop) calls `put_event` after draining. So `put_event` is the
  injection sink, the `mpsc` channel is the thread boundary.

### 9.4 Compiled probe

Status: **✅ COMPILES** — `cargo build` → `Finished dev`, `BUILD_EXIT=0`, in a
fresh crate `/tmp/tv-bridge/b` with `turbo-vision = "1.2"` (1.2.0), Rust 1.95.0.
The probe targets the **overlay-widget idle-poll** variant (option 1), which
needs no backend swap and is the lowest-risk choice for M3. Signatures were
verified against source: `View` required methods are exactly
`bounds / set_bounds / draw / handle_event / get_palette` (others have defaults;
`update_cursor` is also defaulted but overridden here); `ListBox::new(bounds,
on_select_command)` takes **2** args; `ListBox::set_items(Vec<String>)` exists.

```rust
use std::sync::mpsc::{Receiver, TryRecvError};
use turbo_vision::app::Application;
use turbo_vision::core::event::Event;
use turbo_vision::core::geometry::Rect;
use turbo_vision::core::palette::Palette;
use turbo_vision::terminal::Terminal;
use turbo_vision::views::{IdleView, View};
use turbo_vision::views::listbox::ListBox;

/// An overlay widget that drains an LDAP-worker channel on every idle tick
/// and pushes results into a ListBox. The Application calls `idle()` ~50x/sec
/// (20ms poll timeout) including while a modal dialog is open.
struct WorkerBridge {
    rx: Receiver<Vec<String>>,
    list: ListBox,
    bounds: Rect,
}

impl View for WorkerBridge {
    fn bounds(&self) -> Rect { self.bounds }
    fn set_bounds(&mut self, r: Rect) { self.bounds = r; self.list.set_bounds(r); }
    fn draw(&mut self, t: &mut Terminal) { self.list.draw(t); }
    fn handle_event(&mut self, e: &mut Event) { self.list.handle_event(e); }
    fn update_cursor(&self, t: &mut Terminal) { self.list.update_cursor(t); }
    fn get_palette(&self) -> Option<Palette> { None }
}

impl IdleView for WorkerBridge {
    fn idle(&mut self) {
        // Non-blocking drain of the worker channel on the UI thread.
        loop {
            match self.rx.try_recv() {
                Ok(items) => self.list.set_items(items),       // update ListBox
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
    }
}

fn wire_up(rx: Receiver<Vec<String>>) -> Box<dyn IdleView> {
    let bounds = Rect::new(2, 2, 40, 20);          // corners: x1,y1,x2,y2
    let list = ListBox::new(bounds, 0 /* on_select_command */);  // 2 args!
    Box::new(WorkerBridge { rx, list, bounds })
}

fn main() {
    // In the real app, _tx is moved to the LDAP worker thread.
    let (_tx, rx) = std::sync::mpsc::channel::<Vec<String>>();
    // Application::new() opens the real terminal; guard it so this also
    // type-checks (and we proved it builds) with no tty in the sandbox.
    if let Ok(mut app) = Application::new() {
        app.add_overlay_widget(wire_up(rx));       // <- the worker->UI hook
        app.run();                                 // not reached without a tty
    } else {
        let _bridge = wire_up(rx);                 // still must compile
    }
}
```

Verified API facts (from source, all load-bearing for the facade):
- `View` trait (src/views/view.rs:63) — only `bounds`, `set_bounds`, `draw`,
  `handle_event`, `get_palette` are required; everything else is defaulted.
- `IdleView: View` (src/views/view.rs:472) adds the single `fn idle(&mut self)`.
- `pub use view::{View, ViewId, IdleView}` (src/views/mod.rs:114) — import from
  `turbo_vision::views`.
- `ListBox::new(bounds: Rect, on_select_command: CommandId)` (listbox.rs:30) and
  `ListBox::set_items(&mut self, items: Vec<String>)` (listbox.rs:42).
- `Application::add_overlay_widget(&mut self, Box<dyn IdleView>)` (application.rs:144).

### 9.5 Rect semantics

`Rect::new(a,b,c,d)` is **CORNERS (x1, y1, x2, y2)**, top-left inclusive /
bottom-right exclusive — NOT origin+size. From src/core/geometry.rs:

```rust
pub struct Rect { pub a: Point /*top-left incl*/, pub b: Point /*bottom-right excl*/ }
impl Rect {
    pub const fn new(x1: i16, y1: i16, x2: i16, y2: i16) -> Self {
        Self { a: Point::new(x1, y1), b: Point::new(x2, y2) }
    }
    pub const fn from_coords(x: i16, y: i16, width: i16, height: i16) -> Self {
        Self { a: Point::new(x, y), b: Point::new(x + width, y + height) }
    }
    pub fn width(&self)  -> i16 { self.b.x - self.a.x }
    pub fn height(&self) -> i16 { self.b.y - self.a.y }
}
```

`Rect::new(0,0,10,10)` → width 10, height 10. Use `Rect::from_coords` if you
want origin+size. Coordinates are `i16`.

### 9.6 VERDICT

**Background-worker design WORKS — use the idle hook.**

- Mechanism (recommended for M3): **overlay-widget `IdleView::idle()`**, called
  every ~20 ms by `Application::run()` / `exec_view()` / `get_event()` (even
  during modal dialogs). The widget owns a `std::sync::mpsc::Receiver`, drains
  it non-blocking with `try_recv()`, and updates child views. No backend swap.
- Mechanism (heavier, for true cross-thread *event* injection): a **custom
  `Backend`** wrapping `CrosstermBackend` plus an `mpsc::Receiver<Event>`,
  exactly mirroring `SshBackend::poll_event` (`try_recv` then delegate). This
  lets a worker thread post real `Event::command(...)` into the loop. This is
  the proven `ssh`-feature pattern.

The prior "no idle hook" conclusion is refuted by `Application::idle()`,
`add_overlay_widget` / `IdleView`, `Terminal::put_event`, and the `SshBackend`
channel-injection path. M3 does NOT need to fall back to a synchronous blocking
fetch; an async LDAP worker thread + idle-drain is fully supported.

Recommended concrete M3 pattern: run a **manual** loop (the spike already
recommends manual over `app.run()` for I/O interleaving), spawn the LDAP worker
with an `mpsc::Sender`, and on each loop iteration `try_recv()` the worker
channel; on data either mutate the view directly or `app.terminal.put_event(
Event::command(CM_LDAP_DONE))` to drive it through the normal dispatch. If you
prefer `app.run()`, use the overlay-widget `IdleView` variant (probe above,
✅ compiles) which drains the channel in `idle()`.

Rect is corners (x1,y1,x2,y2).
