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

---

## 10. Embedding the DIT tree as a real TV Window (selection events, mouse, focus)

**Date:** 2026-05-30. **Crate:** `turbo-vision = "1.2"` (1.2.0). **Method:** fresh
throwaway `/tmp/tv-embed/e` (`cargo add turbo-vision@1.2`); four probes
(`examples/probe10_window_outline.rs`, `probe10b_selection_and_asany.rs`,
`probe10c_multi_window.rs`, `probe10d_size.rs`) plus a final
`cargo build --examples`. **All compiled; final ALL_EXIT=0.** Every claim below is
either a quoted real signature from the 1.2.0 source or marked ✅ compiled /
⚠️ source-only (not runtime-observed).

> **Scope honesty:** this is a *compile + source-read* spike. It proves the APIs
> exist, type-check, and what the code paths do. It does **not** observe pixels.
> Three things are asserted-by-construction / from source, NOT runtime-observed,
> and the M4.1 implementer should sanity-check them on a real tty: (1) the window
> **frame** renders (§10.9 — the prior app's missing frame is the open symptom);
> (2) on **window resize**, `Group::set_bounds` propagates to the wrapper's
> `set_bounds` → `inner.set_bounds` (the wrapper forwards it), so the outline
> should redraw at the new size — wired correctly in source, not seen running;
> (3) selection-after-click ordering (§10.4 — argued from the dispatch order in
> source). Everything else here is `cargo`-compiled.

### 10.0 Why this section exists — and what the prior author got wrong

A prior build bolted the `OutlineViewer` on as a typed field on the app shell,
hand-drew it on the desktop every frame, and manually forwarded keystrokes.
Claimed reasons: (a) no focused-view / typed get-back from the desktop, and
(b) `View::as_any()` panics so an inserted outline can't be downcast.

The first claim is moot and the second is true-but-irrelevant. The real failure
was architectural: real TV mouse/keyboard events flow **desktop → window →
interior Group → focused child**. A view drawn *outside* that hierarchy never
receives them, so it had no working mouse — **and** a hand-draw painted over the
window frame (see §10.9), so it also had no chrome. **One root cause (drawing
outside the hierarchy) produced both symptoms.** The fix is to put the outline
*inside* a `Window` inserted into `app.desktop` and read its selection back
through its own public accessor / the shared `Rc` node tree — no downcast, no
`as_any`, no hand-draw.

**Decisive source fact (refutes "the outline can't respond to the mouse"):**
`OutlineViewer::handle_event` (src/views/outline.rs:334-359) handles arrows /
Enter / Left / Right, then falls through to `self.handle_list_event(event)`. And
`ListViewer::handle_list_event` (src/views/list_viewer.rs:331-350) has a real
`EventType::MouseDown` arm that hit-tests by bounds and selects the clicked row:

```rust
EventType::MouseDown => {
    if event.mouse.buttons & MB_LEFT_BUTTON != 0 {
        let mouse_pos = event.mouse.pos;          // ABSOLUTE screen coords
        let bounds = self.bounds();               // ABSOLUTE (Group::add converted)
        if bounds.contains(mouse_pos) {
            let relative_y = (mouse_pos.y - bounds.a.y) as usize;  // abs - abs = local row
            let clicked_item = self.list_state().top_item + relative_y;
            if clicked_item < self.item_count() {
                self.select_item(clicked_item);   // moves selection
                event.clear();
                return true;
            }
        }
    }
    false
}
```

**Coordinate identity (why the click lands on the right row):** when the outline
is an interior child of a `Window` on the desktop, `Group::add` has already
converted its bounds to **absolute** screen coordinates (group.rs:49-65), and the
`Desktop` dispatches **absolute** mouse positions. So `mouse_pos.y - bounds.a.y`
is absolute-minus-absolute = the correct local row index. The arithmetic is sound
*only* because the widget lives in the hierarchy (the same precondition that makes
the event arrive at all). `OutlineViewer` **is** mouse-capable already.

### 10.1 Q1 — Window on the desktop; mouse/keyboard auto-route — ✅ compiled

Real signatures (src/views/window.rs):
- `Window::new(bounds: Rect, title: &str) -> Window` (window.rs:65) — blue,
  resizable, draggable, closable; constructs a `Frame` and an interior `Group`
  inset by 1 (`interior_bounds = bounds; interior_bounds.grow(-1,-1)`,
  window.rs:123-127).
- `Window::add(&mut self, Box<dyn View>) -> ViewId` (window.rs:159) — adds to the
  interior `Group`; child bounds are **relative to the interior** (Group::add
  converts relative→absolute).
- `Window::set_initial_focus(&mut self)` (window.rs:205).
- `Desktop::add(&mut self, Box<dyn View>) -> ViewId` (src/views/desktop.rs:47) —
  inserts the window and focuses it (last child becomes current; desktop.rs:79-88).
- Sizing the window: `Application::desktop` is public; **two working ways** to get
  dimensions (both compiled in probe10d):
  - `app.desktop.get_bounds() -> Rect` (desktop.rs:203), then `.width()/.height()`
    — **preferred**, because the desktop is already inset for any menu bar /
    status line.
  - `app.terminal.size() -> (i16, i16)` (src/terminal/mod.rs:239) — full terminal,
    ignores menu/status insets.

```rust
let db = app.desktop.get_bounds();
let (w, h) = (db.width(), db.height());          // preferred (excludes menu/status)
let mut win = Window::new(Rect::new(1, 1, w / 2, h - 1), "DIT");
win.add(Box::new(outline));      // interior child; bounds relative to interior
win.set_initial_focus();
let _win_id = app.desktop.add(Box::new(win));    // <- the proper insert
```

**Auto-routing proof** (`Desktop`/`Window`-interior are `Group`s):
`Group::handle_event` (group.rs:498-575) dispatches `MouseDown` to the top-most
child whose `bounds.contains(mouse_pos)` (reverse z-order), focuses it, then
forwards the event; keyboard/command events use three-phase processing to the
**focused** child, with `Tab`/`Shift-Tab` cycling focus (group.rs:623-632). Once
the window is a desktop child and the outline is its focused interior child, real
mouse + keyboard reach the outline with zero manual forwarding.

**App dispatch (so your command survives back to the loop):**
`Application::handle_event` (src/app/application.rs:457) dispatches
**menu_bar → desktop → status_line**, each step guarded by
`event.what != EventType::Nothing`. Nothing in that method sets an *unrecognized*
command to `Nothing`, so a custom command that no view consumes is still present
in `event` when `handle_event` returns and the loop can match it. (Source-observed
structure.)

### 10.2 Q2 — OutlineViewer inside a Window, mouse-clickable — ✅ compiled

`probe10c` puts an `OutlineViewer<String>` inside a `Window` sized to the interior
and inserts it on the desktop; `probe10` does the same with a rich payload. Both
compile. The click path: `Desktop(Group)::handle_event` → finds the window by
bounds, brings it to front (OF_TOP_SELECT, desktop.rs:547-573) →
`Window::handle_event` → interior `Group::handle_event` → finds the outline by
bounds, focuses it, forwards the `MouseDown` → `OutlineViewer::handle_event` →
`handle_list_event` selects the clicked row (§10.0). The frame (title + borders)
is drawn by `Window::draw` → `self.frame.draw(terminal)` (window.rs:451) **before**
the interior. The crate's own `tree_view.rs` example demonstrates the identical
containment with a `Dialog` (which *is* a `Window`): `dialog.add(Box::new(
tree_view)); app.desktop.add(Box::new(dialog));`.

### 10.3 Q3 — reacting to selection/expansion the TV way — ✅ compiled

There is **no** built-in "selection changed" broadcast emitted by `OutlineViewer`
(unlike `ListBox`, which takes an `on_select_command`;
`OutlineViewer::new(bounds, format_fn)` has no command parameter). So the
event-driven hook is a **thin wrapper `View`** that embeds the `OutlineViewer`,
lets it process the event, then (a) publishes its selection and (b) emits an app
command by transforming the event — the documented TV child→parent pattern
(view.rs:38-62: "Transform event to send message upward … Event bubbles up
through Group::handle_event"; `Group::handle_event` re-dispatches a child-produced
`Command`/`Broadcast`, group.rs:558-573).

```rust
struct DitOutline { inner: OutlineViewer<DitNode>, last_selected: Rc<RefCell<Option<...>>> }

impl View for DitOutline {
    fn can_focus(&self) -> bool { true }                 // so the Group focuses it
    fn handle_event(&mut self, event: &mut Event) {
        // CLICK-ONLY activation (this is what probe10 ships). Do NOT treat Enter
        // as activate: OutlineViewer consumes Enter as an expand/collapse toggle,
        // so emitting on Enter double-fires on every toggle (see matrix below).
        let was_click = event.what == EventType::MouseDown;
        self.inner.handle_event(event);                  // real nav + mouse hit-test
        *self.last_selected.borrow_mut() = self.inner.selected_node();   // publish (no downcast)
        if was_click { *event = Event::command(CM_DIT_ACTIVATE); }
    }
    /* bounds/draw/state/palette delegate to self.inner */
}
```

**Exact activation matrix (read this — the naive wrapper has a footgun):**

| User input | What `inner` does | event after `inner` | Wrapper emits |
|---|---|---|---|
| Left-click on row | `handle_list_event` selects that row, clears event | Nothing | `CM_DIT_ACTIVATE` (select **+** activate) |
| Enter | `OutlineViewer` **toggles expand/collapse**, clears event | Nothing | `CM_DIT_ACTIVATE` — ⚠️ **fires activate on every expand/collapse** |
| ↑/↓/PgUp/PgDn/Home/End | moves `list_state.focused`, clears event | Nothing | nothing (but `last_selected` is republished) |
| ←/→ | collapse/expand, clears event | Nothing | nothing |

So "Enter = activate" double-fires with the expand toggle, and arrow-move emits no
command. **Recommendation:** decide the semantics deliberately. Either (a) gate
`CM_DIT_ACTIVATE` to clicks only (Enter stays pure expand/collapse), or (b) compare
the selected node identity before/after with **`Rc::ptr_eq`** (not a value
`PartialEq`) and emit a distinct `CM_DIT_SELECT` whenever it changed (drives a live
detail pane on arrow-move), plus `CM_DIT_ACTIVATE` only on click. There is **no
double-click event** in what the crate exposes here (only `MouseDown`), so
TV-style double-click activation is not available — single-click select+activate is
the intended choice, not an accident.

### 10.4 Q4 — getting the selection back; the `as_any` claim — ✅ compiled

The prior `as_any` claim is **literally true but irrelevant**:
- `View::as_any(&self) -> &dyn Any` defaults to `panic!("as_any() not implemented
  …")` (view.rs:165-167), and `OutlineViewer` does **not** override it (it would
  panic if called). `probe10b` type-checks a closure that *names* `as_any()`
  (proving it compiles) but never invokes it. (Note: `Window` *does* override
  `as_any` → `self`, window.rs:765 — but a downcast to `Window` still doesn't reach
  the outline.)
- **You never need it.** `OutlineViewer::selected_node(&self) ->
  Option<Rc<RefCell<Node<T>>>>` is a **public `&self` accessor** (outline.rs:233-243).
  Two compiled readback paths:

```rust
// PATH A — direct accessor (needs a &self ref to the widget):
let sel: Option<Rc<RefCell<Node<DitNode>>>> = outline.selected_node();

// PATH B (recommended) — shared-Rc handle the app keeps independently of where
// the view lives: the wrapper writes inner.selected_node() into an
// Rc<RefCell<Option<Rc<RefCell<Node<DitNode>>>>>> the app also holds; the app
// reads last_selected.borrow().clone() with NO view reference, NO downcast.
```

**Ordering guarantee (why the post-event read is current):** the wrapper publishes
`inner.selected_node()` *after* `inner.handle_event`. For a click,
`handle_list_event` → `select_item` → `ListViewerState::focus_item` sets
`self.focused = Some(item)` **synchronously** before returning (list_viewer.rs:101-106),
so the `selected_node()` read immediately after `handle_event` reflects the row
just clicked — no deferred update, no extra redraw needed.

Path B is the recommendation **and** the robust way to react: poll `last_selected`
each loop iteration regardless of whether the `CM_DIT_ACTIVATE` command path fires.
(`Application::handle_event` leaves unknown commands intact — §10.1 — so the
command path also works; the shared-`Rc` poll is just immune to any future
event-clearing.) The `Node<T>` tree being `Rc<RefCell<…>>` throughout is what makes
all of this downcast-free.

### 10.5 Q5 — worker idle-bridge composes with the windowed outline — ✅ compiled

The §9 overlay-`IdleView` bridge composes unchanged. The outline's roots are
`Rc<RefCell<Node<DitNode>>>`; the worker bridge holds clones of the **same `Rc`
nodes** and on each idle tick drains its channel and mutates the tree
(`parent.borrow_mut().add_child(...)`, set `children_loaded = true`). For lazy
load, push children **then** expand the parent — expanding calls `rebuild_display`
(outline.rs:246-255), re-flattening so the new children appear. The bridge needs
only the `Rc`, not the view, so it doesn't conflict with the outline living inside
the window. `probe10` wires `app.add_overlay_widget(Box::new(LazyLoadBridge{ rx,
.. }))` alongside the windowed outline and compiles. (Mechanism + caveats per §9:
overlay `idle()` runs ~50×/s incl. during modal dialogs; `Terminal`/`Backend`
aren't `Sync`, so the worker hands data over `mpsc` and the UI thread applies it.)

### 10.6 Q6 — standard window UX: what's free vs. what needs wiring — ✅ compiled

- **Free (inside `Window::handle_event` via mouse / `Frame`):** drag (title-bar
  `MouseDown`+`MouseMove`), resize (corner), close. The close box generates
  `CM_CLOSE`; with default `auto_close` the window marks itself `SF_CLOSED` and the
  desktop's `remove_closed_windows()` sweep removes it (window.rs:628-650).
  `Window::set_auto_close(false)` (window.rs:235) lets an owner intercept for a
  "save changes?" prompt — useful for the entry editor.
- **Needs command wiring (app/menu/key must emit the command):** zoom — the method
  `Window::zoom(max_bounds)` exists (window.rs:697) and `Desktop::zoom_top_window()`
  (desktop.rs:440) drives it, but it must be triggered by a `cmZoom`-style command;
  and window cycling `Desktop::select_next()/select_prev()` (desktop.rs:375,402) /
  `CM_NEXT`/`CM_PREV` (desktop.rs:583-624). `Tab`/`Shift-Tab` cycle focus among the
  focused window's interior children (group.rs:623-632) for free.
- `probe10c` opens a **tree window + detail window** side by side and compiles —
  the M4.1 shape. Clicking either brings it to front (OF_TOP_SELECT,
  desktop.rs:547-573).

### 10.7 Probe status

| Probe | Topic | Status |
|-------|-------|--------|
| probe10_window_outline | Window+outline wrapper, CM activation, shared-Rc selection, worker idle-bridge, full manual loop | ✅ compiled |
| probe10b_selection_and_asany | `selected_node()` (Path A), shared-`Rc` (Path B), `as_any` panic claim | ✅ compiled |
| probe10c_multi_window | two real Windows (tree + detail) + `select_next/prev` | ✅ compiled |
| probe10d_size | both `app.terminal.size()` and `app.desktop.get_bounds()` | ✅ compiled |

Final `cargo build --examples` in `/tmp/tv-embed/e`: **ALL_EXIT=0**. (Correction
to an in-progress assumption: `Terminal::size()` *does* exist — probe10d proves
`app.terminal.size()` compiles. `app.desktop.get_bounds()` is still preferred
because it accounts for menu/status insets.)

### 10.8 Verdict & recommended M4.1 design

**Real-TV, embedded, mouse-driven, lazy `OutlineViewer`-in-a-`Window` in
turbo-vision 1.2? → YES (with one ~40-line wrapper view).**

One-line mechanism: put the `OutlineViewer` inside a `Window`, insert the window
into `app.desktop`, wrap the outline in a small `View` that forwards events to it,
publishes `selected_node()` into a shared `Rc`, and emits an app `Command` on
activate.

Concrete M4.1 shape:
1. **Tree window.** `Window::new(bounds, "DIT")`; `win.add(Box::new(
   DitOutline::new(Rect::new(0,0,w-2,h-2), root, sel_handle.clone())))` — interior
   child bounds are **relative and inset** (`0,0,w-2,h-2`, *not* the full window —
   see §10.9); `win.set_initial_focus()`; `app.desktop.add(Box::new(win))`.
2. **Wrapper `DitOutline`** (only custom code, ~40 lines, compiled in probe10):
   embeds `OutlineViewer<DitNode>`; `can_focus()->true`; in `handle_event` call
   `inner.handle_event`, publish `*sel_handle.borrow_mut() = inner.selected_node()`,
   then emit a command per the chosen activation policy (§10.3). Delegate
   bounds/draw/state/palette to `inner`.
3. **React** by polling the **shared `Rc` handle** each loop iteration
   (`sel_handle.borrow().clone()`), optionally also matching `CM_DIT_ACTIVATE`
   after `app.handle_event(&mut e)` (the command survives — §10.1). Never `as_any`,
   never a `dyn View` downcast.
4. **Lazy load via the §9 idle bridge:** on activate/expand of an unloaded node,
   hand the node's `Rc` (or DN) to the LDAP worker over `mpsc`; the overlay
   `IdleView` drains results on the UI thread, `add_child`s into the shared node
   tree, then expand the parent (triggers `rebuild_display`). Same `Rc` the
   windowed outline holds → next draw shows the children.
5. **Detail editor as a second `Window`** (probe10c): drag/resize/close + Tab focus
   for free; `set_auto_close(false)` to prompt "save changes?".

**Honest support note.** Supported, but **not 100% turnkey**: the crate gives mouse
+ keyboard + frame + focus + scrolling for free once the outline is in the
hierarchy, plus a downcast-free selection accessor. It does **not** give a built-in
selection/activation command (write the wrapper), an `as_any` override on
`OutlineViewer` (read selection via the wrapper / shared `Rc`), a double-click event
(single-click activate), or automatic zoom/window-cycling (wire the commands). That
wrapper is the entire delta — small, compiled, idiomatic.

**Three biggest changes the rebuild must make vs. the bolted-on approach:**
1. **Put the outline in the hierarchy.** Build it inside a `Window` and
   `app.desktop.add(window)`; draw via the normal `app.draw()`→desktop→window path.
   Stop hand-drawing it on the desktop and stop manually forwarding keys. This fixes
   **both** the dead mouse and the missing frame (§10.9), and gives focus + chrome
   for free.
2. **Stop trying to downcast the inserted view.** Keep an
   `Rc<RefCell<Option<Rc<RefCell<Node<…>>>>>>` selection handle shared with the
   wrapper; read selection from it. `as_any` genuinely panics for `OutlineViewer`,
   but `selected_node()` + shared `Rc` make it unnecessary.
3. **React via command/shared-`Rc`, not polling-from-outside.** The wrapper emits
   `CM_DIT_ACTIVATE` (and optionally `CM_DIT_SELECT` on `Rc::ptr_eq` change); handle
   it in the loop and/or read the shared handle. The §9 worker idle-bridge stays,
   feeding the shared node tree for lazy expansion.

### 10.9 The "dialog/window has no frame" symptom — root cause (source-grounded, ⚠️ not runtime-observed)

Reported during this spike: the prior app's dialog showed **no frame** — "very odd
in TV terms." Diagnosis from source (this spike is compile-only and cannot observe
pixels, so this is a source-grounded hypothesis with a verification step):

- **A frame DOES exist structurally.** `Window::draw` draws the frame then the
  interior: shadow (if any), `self.frame.draw(terminal)` (window.rs:451), then
  `self.interior.draw(terminal)`. `Dialog` is just `Window::new_for_dialog` and
  delegates `draw` to that `Window` (`Dialog::draw` → `self.window.draw(terminal)`,
  dialog.rs:238-239; constructed via `Window::new_for_dialog`, dialog.rs:21). So
  *constructing* a `Window`/`Dialog` and drawing it through the normal path yields a
  frame. **A frameless dialog is therefore not the framework default — something is
  overpainting or mis-sizing.**
- **Most likely root cause: the bolted-on hand-draw painted over the frame.** If the
  outline was drawn directly to the terminal at the window's rect every frame
  *after* `app.draw()`/`desktop.draw()` (exactly the bolted-on pattern that also
  broke the mouse), it overwrites the frame border regardless of containment. **This
  ties the section together: the missing frame and the dead mouse are the same root
  cause — drawing outside the hierarchy.**
- **Second suspect: an interior child sized to the full window instead of the inset
  interior.** `Group::draw` does `clip_bounds = self.bounds; grow(1,1); push_clip`
  (group.rs:471-475) — the interior Group's clip is grown by 1, re-expanding to
  exactly the frame line. A child whose bounds fill the *window* (rather than
  `0,0,w-2,h-2`) can then paint onto that frame line. Always size the interior child
  to the inset interior.
- **Verification step (needs a tty, out of scope here):** run a real `Window`/dialog
  with **no** hand-draw and the outline sized to the inset interior; look for the
  box-drawing border + title bar. If the frame appears there but not in the app, the
  app is hand-drawing over it (suspect #1) or over-sizing the interior child
  (suspect #2). Fix = remove the hand-draw and size children to the inset interior;
  the frame survives via the normal `Window::draw` path.
