//! Turbo Vision facade — the ONLY module in the crate that may `use turbo_vision`.
//!
//! Boundary rule (spec §8 / §14): every other module talks to the TUI
//! exclusively through plain domain types (`MenuDef`, `WidgetSpec`, `FormModel`,
//! `BrowserNode`, …). No `turbo_vision` type may leak past this file. Keeping the
//! dependency confined here makes the backend swappable and keeps the rest of
//! the crate testable without a terminal.
//!
//! Tty boundary (spec §11): `Shell::new`/`run_loop`, `build_menu_bar`,
//! `build_status_line`, `build_outline`, `attach_children`, `build_entry_dialog`,
//! and `confirm_error` all require a real terminal and are NOT unit-tested. The
//! logic they consume lives below the facade in pure, tested functions
//! (`crate::app::build_menu_defs`, `crate::ui::form::*`,
//! `crate::workflows::browser::*`, `crate::workflows::read_flow::*`).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use turbo_vision::app::Application;
use turbo_vision::core::command::{CommandId, CM_CANCEL, CM_OK, CM_QUIT, CM_YES};
use turbo_vision::core::draw::DrawBuffer;
use turbo_vision::core::event::{
    Event, EventType, KB_ALT_X, KB_F10, KB_F2, KB_F3, KB_F6, KB_PGDN, KB_PGUP,
};
use turbo_vision::core::geometry::Rect;
use turbo_vision::core::menu_data::MenuBuilder;
use turbo_vision::core::palette::{palettes, Attr, Palette};
use turbo_vision::core::palette_chain::PaletteChainNode;
use turbo_vision::core::state::{StateFlags, SF_DRAGGING, SF_FOCUSED};
use turbo_vision::helpers::msgbox::{
    message_box, MF_CONFIRMATION, MF_ERROR, MF_INFORMATION, MF_NO_BUTTON, MF_OK_BUTTON,
    MF_YES_BUTTON,
};
use turbo_vision::terminal::Terminal;
use turbo_vision::views::button::Button;
use turbo_vision::views::dialog::Dialog;
use turbo_vision::views::group::Group;
use turbo_vision::views::input_line::InputLine;
use turbo_vision::views::listbox::ListBox;
use turbo_vision::views::menu_bar::{MenuBar, SubMenu};
use turbo_vision::views::outline::{Node, OutlineViewer};
use turbo_vision::views::static_text::StaticText;
use turbo_vision::views::status_line::{StatusItem, StatusLine};
use turbo_vision::views::view::write_line_to_terminal;
use turbo_vision::views::window::Window;
use turbo_vision::views::View;

use crate::app::{menu_action, LoopEvent, MenuDef, UiAction};
use crate::form::changeset::{diff, EditEntry};
use crate::form::validate::ValidationError;
use crate::ldap::ldif::render_changeset;
use crate::ui::form::{FormModel, WidgetSpec};
use crate::ui::form_state::GuardChoice;
use crate::workflows::browser::{BrowserNode, ExpandableNode};
use crate::workflows::structure::Structure;

/// App-local command emitted by [`DitOutline`] when the user activates (clicks) a
/// tree node. Chosen above Turbo Vision's standard `CM_*` ids and distinct from
/// the app-layer menu command ids in [`crate::app`]. The loop reads the published
/// selection on every tick, so this command mainly serves as a wakeup; the
/// shared-`Rc` selection handle is the source of truth (spike §10.3/§10.4).
const CM_DIT_ACTIVATE: CommandId = 2100;

/// App-local broadcast id the facade fires after the idle loop attaches freshly
/// fetched children to a node. [`DitOutline`] matches it and calls
/// `OutlineViewer::set_roots` to trigger the (private) `rebuild_display`, so the
/// new children appear on the next `app.draw()` (the pre-solved lazy-expand
/// refresh, spike §10.4/§10.5).
const CM_DIT_REFRESH: CommandId = 2101;

/// Compile-time proof that the crate links against Turbo Vision.
///
/// Keeps the dependency genuinely exercised at link time and gives a tty-free
/// thing to assert on.
pub fn tv_available() -> bool {
    // `Rect` construction needs no terminal, so referencing it here proves the
    // crate is linked without requiring a tty.
    let _ = Rect::new(0, 0, 1, 1);
    true
}

/// The real Turbo Vision quit command id, exposed so non-facade modules can keep
/// their own mirror constant ([`crate::app::CM_QUIT`]) without importing
/// `turbo_vision`. The `cm_quit_matches_app` test pins the two together.
pub fn tv_cm_quit() -> u16 {
    CM_QUIT
}

/// Minimum width any pane may shrink to.
const MIN_PANE_W: i16 = 8;
/// Columns a divider occupies.
const DIVIDER_W: i16 = 1;

/// Clamp two divider x-positions so every pane keeps `MIN_PANE_W` and the dividers
/// stay ordered, within the absolute interior `[left, right)`.
fn clamp_dividers(left: i16, right: i16, mut d0: i16, mut d1: i16) -> (i16, i16) {
    d0 = d0
        .max(left + MIN_PANE_W)
        .min(right - 2 * MIN_PANE_W - DIVIDER_W);
    d1 = d1.max(d0 + DIVIDER_W + MIN_PANE_W).min(right - MIN_PANE_W);
    (d0, d1)
}

/// The three absolute pane rects for a SplitContainer of bounds `b` with dividers
/// at `d0`/`d1` (already clamped).
fn pane_rects(b: Rect, d0: i16, d1: i16) -> [Rect; 3] {
    let (top, bottom) = (b.a.y, b.b.y);
    [
        Rect::new(b.a.x, top, d0, bottom),
        Rect::new(d0 + DIVIDER_W, top, d1, bottom),
        Rect::new(d1 + DIVIDER_W, top, b.b.x, bottom),
    ]
}

/// The initial (one-third / two-thirds) divider x-positions for a container of
/// `bounds`, clamped. Shared by [`SplitContainer::new`] and [`Shell::mount_split`]
/// so the panes are *built* at the same column rects the container lays them out
/// to (otherwise a pane's child widgets, built for placeholder bounds, would be
/// mangled by the crude `Group::set_bounds` offset+grow).
fn initial_dividers(bounds: Rect) -> (i16, i16) {
    let w = bounds.b.x - bounds.a.x;
    clamp_dividers(
        bounds.a.x,
        bounds.b.x,
        bounds.a.x + w / 3,
        bounds.a.x + (2 * w) / 3,
    )
}

/// Fill `bounds` with spaces in `attr`, giving a frameless pane a solid backdrop
/// (the stock dialog widgets it hosts are transparent outside their text, so
/// without this the desktop pattern shows through). Mirrors how a `Dialog`/`Window`
/// interior is filled.
fn fill_pane_background(terminal: &mut Terminal, bounds: Rect, attr: Attr) {
    let w = (bounds.b.x - bounds.a.x).max(0) as usize;
    if w == 0 {
        return;
    }
    for y in bounds.a.y..bounds.b.y {
        let mut buf = DrawBuffer::new(w);
        buf.move_char(0, ' ', attr, w);
        write_line_to_terminal(terminal, bounds.a.x, y, &buf);
    }
}

/// The gray-dialog palette every leaf/form pane provides, so the stock
/// `StaticText` / `InputLine` / `ListBox` / `Button` widgets it hosts resolve their
/// colors exactly as they would inside a real `Dialog` (which is the environment
/// they were designed for). Without it they map against the desktop palette and
/// render invisibly.
fn dialog_palette() -> Palette {
    Palette::from_slice(palettes::CP_GRAY_DIALOG)
}

/// Clamp a desired vertical scroll `delta` to `[0, max(0, content_h - viewport_h)]`.
fn clamp_scroll(delta: i16, content_h: i16, viewport_h: i16) -> i16 {
    let max = (content_h - viewport_h).max(0);
    delta.max(0).min(max)
}

/// Build the menu bar from backend-agnostic [`MenuDef`]s (spike §1/§7).
///
/// All entries live under a single `~E~daptor` submenu. Key code `0` is the
/// no-shortcut sentinel used throughout the spike examples. `MenuBuilder::item`
/// consumes and returns `self` (crate source core/menu_data.rs:294), so it is
/// chained via reassignment. Not tty-testable.
pub fn build_menu_bar(size_w: i16, defs: &[MenuDef]) -> MenuBar {
    let mut mb = MenuBar::new(Rect::new(0, 0, size_w, 1));
    let mut builder = MenuBuilder::new();
    for d in defs {
        builder = builder.item(&d.label, d.command, 0);
    }
    mb.add_submenu(SubMenu::new("~E~daptor", builder.build()));
    mb
}

/// Build the bottom status line (spike §1). The F2/F3 Save/Cancel hints are
/// omitted in `read_only` mode (the form has no Save/Cancel there). Not
/// tty-testable.
pub fn build_status_line(size_w: i16, size_h: i16, read_only: bool) -> StatusLine {
    let mut items = vec![
        StatusItem::new("~Alt+X~ Quit", KB_ALT_X, CM_QUIT),
        StatusItem::new("~F6~ Pane", KB_F6, 0),
    ];
    if !read_only {
        items.push(StatusItem::new("~F2~ Save", KB_F2, CM_FORM_SAVE));
        items.push(StatusItem::new("~F3~ Cancel", KB_F3, CM_FORM_CANCEL));
    }
    items.push(StatusItem::new("~F10~ Menu", KB_F10, 0));
    StatusLine::new(Rect::new(0, size_h - 1, size_w, size_h), items)
}

/// Shared selection handle: the DN + `loaded` flag of the currently selected tree
/// node, published by [`DitOutline`] after every event and read by `run_loop`. A
/// downcast-free, view-reference-free readback path (spike §10.4 Path B). `None`
/// when no node is selected.
type Selection = Rc<RefCell<Option<(String, bool)>>>;

/// A thin `View` wrapper that owns the real [`OutlineViewer`] and lives inside a
/// [`Window`] on the desktop, so Turbo Vision routes mouse + keyboard to it and
/// draws its frame (spike §10.3/§10.8). The wrapper:
/// * forwards every event to the inner outline (navigation / expand-collapse /
///   mouse hit-test are all the outline's own behaviour);
/// * publishes the inner outline's `selected_node()` (DN + `loaded`) into a
///   shared [`Selection`] handle the app also holds — no `as_any`, no downcast
///   (the `OutlineViewer` View impl does not override `as_any`, which would
///   panic; spike §10.4);
/// * on a left-click (`MouseDown`) emits [`CM_DIT_ACTIVATE`] so the loop wakes and
///   reacts (Enter stays a pure expand/collapse toggle — emitting on Enter would
///   double-fire on every toggle, spike §10.3 activation matrix);
/// * on the [`CM_DIT_REFRESH`] broadcast rebuilds the flattened display via
///   `set_roots`, so children attached to the shared `Rc` tree during idle become
///   visible (spike §10.4/§10.5).
///
/// All other `View` methods delegate to the inner outline so palette, focus,
/// bounds, and resize behave exactly as a bare `OutlineViewer` would.
struct DitOutline {
    inner: OutlineViewer<BrowserNode>,
    selection: Selection,
    root: BrowserNodeRef,
}

impl DitOutline {
    /// Build the wrapper over a fresh `OutlineViewer` rooted at `root`, sharing
    /// `selection` with the app. `bounds` are relative to the host window's inset
    /// interior (`0,0,w-2,h-2`); never the full window (spike §10.9).
    fn new(bounds: Rect, root: BrowserNodeRef, selection: Selection) -> Self {
        let mut inner = OutlineViewer::new(bounds, |n: &BrowserNode| n.label.clone());
        inner.add_root(root.clone());
        DitOutline {
            inner,
            selection,
            root,
        }
    }

    /// Publish the inner outline's current selection (DN + `loaded`) into the
    /// shared handle. Called after every forwarded event so the loop reads a
    /// current value (`select_item` updates focus synchronously; spike §10.4).
    fn publish_selection(&self) {
        let next = self.inner.selected_node().map(|node| {
            let n = node.borrow();
            (n.data.dn.clone(), n.data.loaded)
        });
        *self.selection.borrow_mut() = next;
    }
}

impl View for DitOutline {
    fn bounds(&self) -> Rect {
        self.inner.bounds()
    }

    fn set_bounds(&mut self, bounds: Rect) {
        self.inner.set_bounds(bounds);
    }

    fn draw(&mut self, terminal: &mut Terminal) {
        self.inner.draw(terminal);
    }

    fn handle_event(&mut self, event: &mut Event) {
        // Refresh broadcast: rebuild the flattened display from the shared Rc tree
        // so lazily attached children become visible. `set_roots` calls the
        // private `rebuild_display` (spike §10.4). Consume the broadcast.
        if event.what == EventType::Broadcast && event.command == CM_DIT_REFRESH {
            self.inner.set_roots(vec![self.root.clone()]);
            self.publish_selection();
            event.clear();
            return;
        }

        // Activation on selection CHANGE (covers both mouse click and keyboard
        // arrow navigation), so the three-pane loop re-spins pane 2 whenever the
        // highlighted branch changes. Only keyboard/mouse events can change the
        // selection — guarding on `track` ensures we never clobber a passing
        // broadcast (e.g. CM_LEAF_REFRESH) with a command, which would break the
        // broadcast fan-out to the sibling panes. Expand/collapse (Enter) keeps the
        // same node selected, so it does not re-fire (no double-trigger).
        let track = matches!(
            event.what,
            EventType::Keyboard | EventType::MouseDown | EventType::MouseMove | EventType::MouseUp
        );
        let before = if track {
            self.selection.borrow().clone()
        } else {
            None
        };
        self.inner.handle_event(event);
        self.publish_selection();
        if track && *self.selection.borrow() != before {
            // Transform the event into an app command so the loop wakes and reacts
            // to the new selection (spike §10.3 child→parent pattern). The
            // shared-Rc selection is already published above.
            *event = Event::command(CM_DIT_ACTIVATE);
        }
    }

    fn can_focus(&self) -> bool {
        true
    }

    fn state(&self) -> StateFlags {
        self.inner.state()
    }

    fn set_state(&mut self, state: StateFlags) {
        self.inner.set_state(state);
    }

    fn update_cursor(&self, terminal: &mut Terminal) {
        self.inner.update_cursor(terminal);
    }

    fn set_palette_chain(&mut self, node: Option<PaletteChainNode>) {
        self.inner.set_palette_chain(node);
    }

    fn get_palette_chain(&self) -> Option<&PaletteChainNode> {
        self.inner.get_palette_chain()
    }

    fn get_palette(&self) -> Option<Palette> {
        self.inner.get_palette()
    }
}

/// A frameless three-column container with two mouse-draggable vertical dividers.
/// Wraps a [`Group`] that owns the three pane child views (so Tab focus cycling and
/// child event routing come for free); the SplitContainer adds only divider drawing
/// and drag (TV has no splitter widget). Mounted directly on `app.desktop`.
pub struct SplitContainer {
    inner: Group,
    bounds: Rect,
    divider_x: [i16; 2],
    dragging: Option<usize>,
    state: StateFlags,
    palette_chain: Option<PaletteChainNode>,
}

impl SplitContainer {
    /// Build from three already-constructed pane views (left→right). Incoming pane
    /// bounds are ignored; `layout` assigns columns.
    pub fn new(
        bounds: Rect,
        left: Box<dyn View>,
        middle: Box<dyn View>,
        right: Box<dyn View>,
    ) -> Self {
        let (d0, d1) = initial_dividers(bounds);
        let mut inner = Group::new(bounds);
        inner.add(left);
        inner.add(middle);
        inner.add(right);
        let mut me = SplitContainer {
            inner,
            bounds,
            divider_x: [d0, d1],
            dragging: None,
            state: 0,
            palette_chain: None,
        };
        me.layout();
        me.inner.set_initial_focus();
        me
    }

    fn layout(&mut self) {
        let rects = pane_rects(self.bounds, self.divider_x[0], self.divider_x[1]);
        for (i, r) in rects.iter().enumerate() {
            self.inner.child_at_mut(i).set_bounds(*r);
        }
    }

    fn divider_at(&self, x: i16, y: i16) -> Option<usize> {
        if y < self.bounds.a.y || y >= self.bounds.b.y {
            return None;
        }
        self.divider_x.iter().position(|&dx| x == dx)
    }
}

impl View for SplitContainer {
    fn bounds(&self) -> Rect {
        self.bounds
    }

    fn set_bounds(&mut self, new: Rect) {
        let old_w = (self.bounds.b.x - self.bounds.a.x).max(1);
        let new_w = (new.b.x - new.a.x).max(1);
        for d in &mut self.divider_x {
            let frac = (*d - self.bounds.a.x) as f32 / old_w as f32;
            *d = new.a.x + (frac * new_w as f32).round() as i16;
        }
        self.bounds = new;
        self.inner.set_bounds(new);
        let (d0, d1) = clamp_dividers(new.a.x, new.b.x, self.divider_x[0], self.divider_x[1]);
        self.divider_x = [d0, d1];
        self.layout();
    }

    fn draw(&mut self, terminal: &mut Terminal) {
        self.inner.set_palette_chain(self.palette_chain.clone());
        self.inner.draw(terminal);
        let attr = self.map_color(1);
        for &x in &self.divider_x {
            for y in self.bounds.a.y..self.bounds.b.y {
                let mut buf = DrawBuffer::new(DIVIDER_W as usize);
                buf.move_char(0, '│', attr, DIVIDER_W as usize);
                write_line_to_terminal(terminal, x, y, &buf);
            }
        }
    }

    fn handle_event(&mut self, event: &mut Event) {
        // F6 cycles focus between the three panes. Intercept it here (before
        // delegating to the inner Group) because each pane's own inner Group
        // consumes Tab to cycle its widgets, which would otherwise trap keyboard
        // focus inside a single pane (mouse-click still switches panes freely).
        if event.what == EventType::Keyboard && event.key_code == KB_F6 {
            self.inner.select_next();
            event.clear();
            return;
        }
        match event.what {
            EventType::MouseDown => {
                if let Some(i) = self.divider_at(event.mouse.pos.x, event.mouse.pos.y) {
                    self.dragging = Some(i);
                    self.state |= SF_DRAGGING;
                    event.clear();
                    return;
                }
            }
            EventType::MouseMove => {
                if let Some(i) = self.dragging {
                    let x = event.mouse.pos.x;
                    self.divider_x[i] = x;
                    let (d0, d1) = clamp_dividers(
                        self.bounds.a.x,
                        self.bounds.b.x,
                        self.divider_x[0],
                        self.divider_x[1],
                    );
                    self.divider_x = [d0, d1];
                    self.layout();
                    event.clear();
                    return;
                }
            }
            EventType::MouseUp if self.dragging.is_some() => {
                self.dragging = None;
                self.state &= !SF_DRAGGING;
                event.clear();
                return;
            }
            EventType::MouseWheelUp | EventType::MouseWheelDown => {
                // Route the wheel to the pane UNDER the cursor (positional), so
                // scrolling the form works without focusing it first. A Group
                // otherwise sends wheel events only to its focused child.
                let pos = event.mouse.pos;
                for i in 0..self.inner.len() {
                    if self.inner.child_at(i).bounds().contains(pos) {
                        self.inner.child_at_mut(i).handle_event(event);
                        return;
                    }
                }
            }
            _ => {}
        }
        self.inner.handle_event(event);
    }

    fn can_focus(&self) -> bool {
        true
    }

    fn state(&self) -> StateFlags {
        self.state
    }

    fn set_state(&mut self, state: StateFlags) {
        self.state = state;
    }

    fn set_focus(&mut self, focused: bool) {
        self.set_state_flag(SF_FOCUSED, focused);
        if focused {
            self.inner.set_initial_focus();
        }
    }

    fn update_cursor(&self, terminal: &mut Terminal) {
        if let Some(child) = self.inner.focused_child() {
            child.update_cursor(terminal);
        }
    }

    fn set_palette_chain(&mut self, node: Option<PaletteChainNode>) {
        self.palette_chain = node;
    }

    fn get_palette_chain(&self) -> Option<&PaletteChainNode> {
        self.palette_chain.as_ref()
    }

    fn get_palette(&self) -> Option<Palette> {
        None
    }
}

/// Local command ids emitted by [`FormPane`] when its buttons fire. Distinct from
/// the DIT outline ids (2100/2101). Surfaced by [`Shell::run_loop`] as
/// [`UiAction::FormSave`]/[`UiAction::FormCancel`].
const CM_FORM_SAVE: CommandId = 2200;
const CM_FORM_CANCEL: CommandId = 2201;

/// Broadcast id the run-loop fires after writing a new model into
/// [`FormHandles::model`]; [`FormPane`] matches it and (re)builds its rows (or
/// clears when the model is `None`). Distinct from the leaf-refresh id (2500).
const CM_FORM_REFRESH: CommandId = 2501;

/// Shared handles the run-loop uses to drive — and read back — the [`FormPane`]
/// without reaching through the desktop view tree (the broadcast-push pattern,
/// mirroring [`DitOutline`]'s `selection`). `model` is the loop→pane channel
/// (push a model, broadcast [`CM_FORM_REFRESH`]); `dirty`/`edit`/`dn` are the
/// pane→loop channel (republished after every event the pane handles).
#[derive(Clone, Default)]
pub struct FormHandles {
    /// Loop→pane: the model to display, or `None` to clear.
    pub model: Rc<RefCell<Option<FormModel>>>,
    /// Pane→loop: whether any editable binding differs from its baseline.
    pub dirty: Rc<RefCell<bool>>,
    /// Pane→loop: the live edited entry (`None` when no entry is shown).
    pub edit: Rc<RefCell<Option<EditEntry>>>,
    /// Pane→loop: the DN currently shown (empty when cleared).
    pub dn: Rc<RefCell<String>>,
}

/// One editable row's attribute name + the shared String its InputLine mutates.
type RowBinding = (String, Rc<RefCell<String>>);

/// The live, scrollable entry-edit pane (pane 3). Holds an inner [`Group`] of
/// label+editor rows translated by a manual scroll `delta`, plus a Save/Cancel bar
/// (omitted in read-only mode). Dirty = any binding differs from its baseline.
pub struct FormPane {
    bounds: Rect,
    read_only: bool,
    inner: Group,
    bindings: Vec<RowBinding>,
    baseline: std::collections::BTreeMap<String, Vec<String>>,
    dn: String,
    content_h: i16,
    scroll: i16,
    state: StateFlags,
    palette_chain: Option<PaletteChainNode>,
    handles: FormHandles,
}

impl FormPane {
    /// Build an empty pane covering `bounds`, wired to the shared `handles`. In
    /// `read_only` mode the pane shows no Save/Cancel bar and renders every field
    /// as static text.
    pub fn new(bounds: Rect, read_only: bool, handles: FormHandles) -> Self {
        FormPane {
            bounds,
            read_only,
            inner: Group::new(bounds),
            bindings: Vec::new(),
            baseline: std::collections::BTreeMap::new(),
            dn: String::new(),
            content_h: 0,
            scroll: 0,
            state: 0,
            palette_chain: None,
            handles,
        }
    }

    /// Republish the pane's state (dirty / live edit / DN) into the shared handles
    /// so the run-loop reads a current value each tick. Called after every event
    /// the pane handles and after every (re)build.
    fn publish(&self) {
        *self.handles.dirty.borrow_mut() = self.is_dirty();
        *self.handles.edit.borrow_mut() = self.take_edit();
        *self.handles.dn.borrow_mut() = self.dn.clone();
    }

    /// React to a [`CM_FORM_REFRESH`] broadcast: load the model the loop pushed
    /// into [`FormHandles::model`], or clear the pane when it is `None`.
    fn apply_refresh(&mut self) {
        let model = self.handles.model.borrow().clone();
        match model {
            Some(m) => self.set_model(&m),
            None => self.clear(),
        }
    }

    /// Rebuild the pane's rows from `model`: a label (with a `*` suffix for MUST
    /// attributes) plus an editor (an [`InputLine`] bound to a shared String for
    /// editable fields, static text otherwise). Records every field's values as the
    /// dirty baseline and resets the scroll position.
    pub fn set_model(&mut self, model: &FormModel) {
        self.dn = model.title.clone();
        self.baseline.clear();
        for f in &model.fields {
            self.baseline.insert(f.label.clone(), f.values.clone());
        }
        self.inner = Group::new(self.bounds);
        self.bindings.clear();
        // Row bounds are RELATIVE to the group origin — `Group::add` offsets them by
        // `self.bounds.a` to absolute. (Scrolling later translates the whole group.)
        let width = self.bounds.b.x - self.bounds.a.x;
        let mut y: i16 = 0;
        for field in &model.fields {
            let label = if field.is_must {
                format!("{} *", field.label)
            } else {
                field.label.clone()
            };
            self.inner.add(Box::new(StaticText::new(
                Rect::new(0, y, 18, y + 1),
                &label,
            )));
            if !self.read_only && field_is_editable(field) {
                let seed = field.values.join("\n");
                let data = Rc::new(RefCell::new(seed));
                let input = InputLine::new(Rect::new(19, y, width - 1, y + 1), 1024, data.clone());
                self.inner.add(Box::new(input));
                self.bindings.push((field.label.clone(), data));
            } else {
                let value = field_display(&field.widget, &field.values);
                self.inner.add(Box::new(StaticText::new(
                    Rect::new(19, y, width - 1, y + 1),
                    &value,
                )));
            }
            y += 1;
        }
        if !self.read_only {
            // Buttons MUST be ≥2 rows tall — Button::draw bails on height < 2.
            self.inner.add(Box::new(Button::new(
                Rect::new(0, y + 1, 10, y + 3),
                "~S~ave",
                CM_FORM_SAVE,
                true,
            )));
            self.inner.add(Box::new(Button::new(
                Rect::new(12, y + 1, 22, y + 3),
                "~C~ancel",
                CM_FORM_CANCEL,
                false,
            )));
            y += 3;
        }
        self.content_h = y;
        self.scroll = 0;
        self.inner.set_initial_focus();
        self.publish();
    }

    /// Reset the pane to empty (no entry selected).
    pub fn clear(&mut self) {
        self.dn.clear();
        self.bindings.clear();
        self.baseline.clear();
        self.inner = Group::new(self.bounds);
        self.content_h = 0;
        self.scroll = 0;
        self.publish();
    }

    /// The DN of the entry currently shown (empty when cleared).
    #[allow(dead_code)] // pane→loop DN is read via the shared handle
    pub fn dn(&self) -> &str {
        &self.dn
    }

    /// True when any editable binding's current value set differs from its
    /// baseline. Mirrors the modal dialog's value-collection semantics (newline
    /// split, trimmed, non-empty).
    pub fn is_dirty(&self) -> bool {
        for (label, data) in &self.bindings {
            let current: Vec<String> = data
                .borrow()
                .split('\n')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            let base = self.baseline.get(label).cloned().unwrap_or_default();
            if current != base {
                return true;
            }
        }
        false
    }

    /// Build an [`EditEntry`] from the baseline overlaid with the live bindings, or
    /// `None` when no entry is shown. Non-editable fields keep their baseline
    /// values.
    pub fn take_edit(&self) -> Option<EditEntry> {
        if self.dn.is_empty() {
            return None;
        }
        let mut attrs = self.baseline.clone();
        for (label, data) in &self.bindings {
            let values: Vec<String> = data
                .borrow()
                .split('\n')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            attrs.insert(label.clone(), values);
        }
        Some(EditEntry {
            dn: self.dn.clone(),
            attrs,
        })
    }

    /// Restore every editable binding to its baseline value. (Cancel is driven by
    /// re-pushing the model through the shared handle, so this is kept only for
    /// completeness / direct use.)
    #[allow(dead_code)] // Cancel re-pushes the model rather than reverting in place
    pub fn revert(&mut self) {
        for (label, data) in &self.bindings {
            let base = self.baseline.get(label).cloned().unwrap_or_default();
            *data.borrow_mut() = base.join("\n");
        }
    }

    fn viewport_h(&self) -> i16 {
        self.bounds.b.y - self.bounds.a.y
    }

    /// Scroll the form rows to the clamped `new_scroll` by translating each row
    /// view up/down by the delta. The inner [`Group`]'s own bounds stay anchored to
    /// the pane: `Group::set_bounds` would move the clip region with the group, so
    /// scrolling by moving the group collapses the visible window to wherever the
    /// group still overlaps the pane. Moving the children individually keeps the
    /// clip fixed to the viewport while the content slides under it.
    fn apply_scroll(&mut self, new_scroll: i16) {
        let clamped = clamp_scroll(new_scroll, self.content_h, self.viewport_h());
        let dy = clamped - self.scroll;
        if dy != 0 {
            for i in 0..self.inner.len() {
                let cb = self.inner.child_at(i).bounds();
                self.inner.child_at_mut(i).set_bounds(Rect::new(
                    cb.a.x,
                    cb.a.y - dy,
                    cb.b.x,
                    cb.b.y - dy,
                ));
            }
            self.scroll = clamped;
        }
    }
}

impl View for FormPane {
    fn bounds(&self) -> Rect {
        self.bounds
    }

    fn set_bounds(&mut self, b: Rect) {
        self.bounds = b;
        self.inner.set_bounds(b);
    }

    fn draw(&mut self, terminal: &mut Terminal) {
        fill_pane_background(terminal, self.bounds, self.map_color(1));
        // Propagate a chain node carrying this pane's dialog palette to the rows.
        let chain = PaletteChainNode::new(self.get_palette(), self.palette_chain.clone());
        self.inner.set_palette_chain(Some(chain));
        self.inner.draw(terminal);
    }

    fn handle_event(&mut self, event: &mut Event) {
        // Refresh broadcast: (re)build the form from the model the loop pushed.
        // Consume it so it does not propagate further.
        if event.what == EventType::Broadcast && event.command == CM_FORM_REFRESH {
            self.apply_refresh();
            event.clear();
            return;
        }
        // F2 = Save, F3 = Cancel: emit the same command the (possibly scrolled
        // off-screen) Save/Cancel buttons would, so the form is reliably saveable
        // from the keyboard regardless of scroll position or which field has focus.
        if !self.read_only && event.what == EventType::Keyboard {
            if event.key_code == KB_F2 {
                *event = Event::command(CM_FORM_SAVE);
                return;
            }
            if event.key_code == KB_F3 {
                *event = Event::command(CM_FORM_CANCEL);
                return;
            }
        }
        match event.what {
            EventType::MouseWheelDown => {
                self.apply_scroll(self.scroll + 1);
                event.clear();
                return;
            }
            EventType::MouseWheelUp => {
                self.apply_scroll(self.scroll - 1);
                event.clear();
                return;
            }
            EventType::Keyboard => {
                if event.key_code == KB_PGDN {
                    self.apply_scroll(self.scroll + self.viewport_h());
                    event.clear();
                    return;
                }
                if event.key_code == KB_PGUP {
                    self.apply_scroll(self.scroll - self.viewport_h());
                    event.clear();
                    return;
                }
            }
            _ => {}
        }
        self.inner.handle_event(event);
        // Keep the dirty/edit handles current as the user types into the bound
        // InputLines (the run-loop polls `dirty` for the navigation guard).
        self.publish();
    }

    fn can_focus(&self) -> bool {
        true
    }

    fn state(&self) -> StateFlags {
        self.state
    }

    fn set_state(&mut self, s: StateFlags) {
        self.state = s;
    }

    fn set_focus(&mut self, focused: bool) {
        self.set_state_flag(SF_FOCUSED, focused);
        if focused {
            self.inner.set_initial_focus();
        }
    }

    fn update_cursor(&self, terminal: &mut Terminal) {
        if let Some(c) = self.inner.focused_child() {
            c.update_cursor(terminal);
        }
    }

    fn set_palette_chain(&mut self, n: Option<PaletteChainNode>) {
        self.palette_chain = n;
    }

    fn get_palette_chain(&self) -> Option<&PaletteChainNode> {
        self.palette_chain.as_ref()
    }

    fn get_palette(&self) -> Option<Palette> {
        Some(dialog_palette())
    }
}

/// Command id the [`ListBox`] is built with (lists emit no selection-change event,
/// so the pane publishes its selection by polling instead). Distinct from the DIT
/// outline ids (2100/2101) and the FormPane ids (2200/2201/2501).
const CM_LEAF_SELECT: CommandId = 2301;

/// Broadcast id the run-loop fires after writing fresh rows into
/// [`LeafHandles::rows`]; [`LeafListPane`] matches it and rebuilds its [`ListBox`].
const CM_LEAF_REFRESH: CommandId = 2500;

/// Shared handles the run-loop uses to drive — and read back — the
/// [`LeafListPane`] without reaching through the desktop view tree. `rows` is the
/// loop→pane channel (push rows, broadcast [`CM_LEAF_REFRESH`]); `search` is bound
/// directly to the filter [`InputLine`] (the loop reads the live text); `selected`
/// is the pane→loop channel (the highlighted row's DN, republished after every
/// event).
#[derive(Clone, Default)]
pub struct LeafHandles {
    /// Loop→pane: the visible rows as `(display label, dn)`.
    pub rows: Rc<RefCell<Vec<(String, String)>>>,
    /// Pane→loop (live): the incremental-search box text.
    pub search: Rc<RefCell<String>>,
    /// Pane→loop: the DN of the highlighted row, if any.
    pub selected: Rc<RefCell<Option<String>>>,
}

/// Pane 2: an incremental-search [`InputLine`] over a [`ListBox`] of the current
/// branch's leaves (plus a `‹self›` row for the branch entry itself). The pane is
/// passive: the run-loop recomputes rows from the
/// [`crate::workflows::structure::Structure`] whenever the branch selection or the
/// search text changes, pushes them through [`LeafHandles::rows`], and broadcasts
/// [`CM_LEAF_REFRESH`]. The pane publishes the highlighted row's DN into
/// [`LeafHandles::selected`] after every event (lists emit no change event).
pub struct LeafListPane {
    bounds: Rect,
    inner: Group,
    handles: LeafHandles,
    /// Parallel to the ListBox items: the DN for each visible row.
    row_dns: Vec<String>,
    state: StateFlags,
    palette_chain: Option<PaletteChainNode>,
}

impl LeafListPane {
    /// Build an empty pane covering `bounds`, wired to the shared `handles`: a
    /// `Search:` label + [`InputLine`] (bound to `handles.search`) on the top row,
    /// and a [`ListBox`] filling the rest. The list starts empty; the run-loop
    /// populates it by pushing rows and broadcasting [`CM_LEAF_REFRESH`].
    pub fn new(bounds: Rect, handles: LeafHandles) -> Self {
        let mut inner = Group::new(bounds);
        // Child bounds are RELATIVE to the group origin — `Group::add` offsets them
        // by `bounds.a` to absolute. Building them absolute here would double-offset.
        let width = bounds.b.x - bounds.a.x;
        let height = bounds.b.y - bounds.a.y;
        inner.add(Box::new(StaticText::new(Rect::new(0, 0, 8, 1), "Search:")));
        inner.add(Box::new(InputLine::new(
            Rect::new(8, 0, width, 1),
            256,
            handles.search.clone(),
        )));
        inner.add(Box::new(ListBox::new(
            Rect::new(0, 1, width, height),
            CM_LEAF_SELECT,
        )));
        // Focus the ListBox (child 2), not the search box, so arrow keys browse
        // leaves immediately; Tab/click reaches the search box to filter.
        inner.set_focus_to(2);
        let mut me = LeafListPane {
            bounds,
            inner,
            handles,
            row_dns: Vec::new(),
            state: 0,
            palette_chain: None,
        };
        // Seed the list from any rows the loop pre-populated before mounting, so
        // pane 2 is non-empty on the first frame (no refresh broadcast needed yet).
        me.rebuild_rows();
        me
    }

    /// The ListBox is child index 2 (label, input, listbox).
    fn listbox_mut(&mut self) -> &mut ListBox {
        self.inner
            .child_at_mut(2)
            .as_any_mut()
            .downcast_mut::<ListBox>()
            .expect("child 2 is the ListBox")
    }
    fn listbox(&self) -> &ListBox {
        self.inner
            .child_at(2)
            .as_any()
            .downcast_ref::<ListBox>()
            .expect("child 2 is the ListBox")
    }

    /// Rebuild the visible rows from [`LeafHandles::rows`] (loop→pane), resetting
    /// selection to row 0, then republish the (new) selection.
    fn rebuild_rows(&mut self) {
        let rows = self.handles.rows.borrow().clone();
        let labels: Vec<String> = rows.iter().map(|(l, _)| l.clone()).collect();
        self.row_dns = rows.into_iter().map(|(_, d)| d).collect();
        self.listbox_mut().set_items(labels);
        self.publish_selection();
    }

    /// Publish the highlighted row's DN into [`LeafHandles::selected`].
    fn publish_selection(&self) {
        let sel = self
            .listbox()
            .get_selection()
            .and_then(|i| self.row_dns.get(i).cloned());
        *self.handles.selected.borrow_mut() = sel;
    }
}

impl View for LeafListPane {
    fn bounds(&self) -> Rect {
        self.bounds
    }
    fn set_bounds(&mut self, b: Rect) {
        self.bounds = b;
        self.inner.set_bounds(b);
    }
    fn draw(&mut self, terminal: &mut Terminal) {
        fill_pane_background(terminal, self.bounds, self.map_color(1));
        let chain = PaletteChainNode::new(self.get_palette(), self.palette_chain.clone());
        self.inner.set_palette_chain(Some(chain));
        self.inner.draw(terminal);
    }
    fn handle_event(&mut self, event: &mut Event) {
        // Refresh broadcast: rebuild rows from the handle the loop pushed. Consume
        // it so it does not propagate further.
        if event.what == EventType::Broadcast && event.command == CM_LEAF_REFRESH {
            self.rebuild_rows();
            event.clear();
            return;
        }
        self.inner.handle_event(event);
        // Republish the highlighted DN so the loop sees selection changes (arrow
        // keys / clicks in the ListBox emit no change event).
        self.publish_selection();
    }
    fn can_focus(&self) -> bool {
        true
    }
    fn state(&self) -> StateFlags {
        self.state
    }
    fn set_state(&mut self, s: StateFlags) {
        self.state = s;
    }
    fn set_focus(&mut self, focused: bool) {
        self.set_state_flag(SF_FOCUSED, focused);
        if focused {
            // Land on the ListBox so arrows browse leaves immediately.
            self.inner.set_focus_to(2);
        }
    }
    fn update_cursor(&self, terminal: &mut Terminal) {
        if let Some(c) = self.inner.focused_child() {
            c.update_cursor(terminal);
        }
    }
    fn set_palette_chain(&mut self, n: Option<PaletteChainNode>) {
        self.palette_chain = n;
    }
    fn get_palette_chain(&self) -> Option<&PaletteChainNode> {
        self.palette_chain.as_ref()
    }
    fn get_palette(&self) -> Option<Palette> {
        Some(dialog_palette())
    }
}

/// The application shell: owns the Turbo Vision [`Application`] and the shared
/// handles needed to drive the DIT tree, and runs the manual event loop.
/// Construction requires a real terminal.
///
/// Design note (M4.1 rebuild, spike §10): the DIT outline is wrapped in a
/// [`DitOutline`] view and inserted into a real [`Window`] on `app.desktop`, so
/// `app.draw()` renders it (with a frame) and Turbo Vision routes mouse +
/// keyboard to it through the normal desktop→window→interior hierarchy. The Shell
/// no longer holds the `OutlineViewer`; it keeps only the shared [`Selection`]
/// handle (read each loop tick) and the `root` `Rc` (used to resolve nodes by DN
/// on expansion and to drive the refresh broadcast). This fixes the prior
/// bolted-on approach's dead mouse and missing frame (both caused by hand-drawing
/// outside the view hierarchy; spike §10.0/§10.9).
pub struct Shell {
    app: Application,
    /// The currently selected node (DN + `loaded`), published by the windowed
    /// [`DitOutline`] and read by `run_loop`. `None` until a node is selected.
    selection: Selection,
}

impl Shell {
    /// Build the application, install the profile-derived menu bar and the
    /// status line. Requires a tty (`Application::new()` puts the terminal into
    /// raw mode). Not tty-testable.
    pub fn new(defs: &[MenuDef], read_only: bool) -> anyhow::Result<Shell> {
        let mut app = Application::new()?;
        let (w, h) = app.terminal.size();
        app.set_menu_bar(build_menu_bar(w, defs));
        app.set_status_line(build_status_line(w, h, read_only));
        Ok(Shell {
            app,
            selection: Rc::new(RefCell::new(None)),
        })
    }

    /// Build the DIT outline rooted at `root`, wrap it in a [`DitOutline`] view
    /// inside a "DIT" [`Window`], and insert that window into `app.desktop` so the
    /// tree is a real, framed, mouse-driven view (spike §10.1/§10.8). The window
    /// occupies the left pane; the interior child is inset (`0,0,w-2,h-2`) so the
    /// frame survives (spike §10.9). The outline shares the node `Rc` tree with
    /// `root`, and the Shell keeps the shared selection handle. Not tty-testable.
    pub fn mount_outline(&mut self, root: BrowserNodeRef) {
        // Size to the desktop (already inset for menu/status), left half.
        let db = self.app.desktop.get_bounds();
        let dw = db.width();
        let dh = db.height();
        let win_w = (dw / 2).max(20);
        let win_h = dh.max(5);
        let mut win = Window::new(Rect::new(0, 0, win_w, win_h), "DIT");
        // Interior child bounds are relative to the inset interior (spike §10.9).
        let inner_bounds = Rect::new(0, 0, win_w - 2, win_h - 2);
        win.add(Box::new(DitOutline::new(
            inner_bounds,
            root,
            self.selection.clone(),
        )));
        win.set_initial_focus();
        self.app.desktop.add(Box::new(win));
    }

    /// Mount the frameless three-pane [`SplitContainer`] as the desktop's content
    /// (replacing [`mount_outline`](Self::mount_outline) for the M6 redesign).
    /// Pane 1 is the branch [`DitOutline`] (sharing the Shell's `selection` handle
    /// and the `tree_root` `Rc` so refreshes work), pane 2 the [`LeafListPane`],
    /// pane 3 the [`FormPane`]. The run-loop drives panes 2/3 through the shared
    /// `leaf`/`form` handles + the refresh broadcasts; it reads the tree selection
    /// through `selection` exactly as before. Not tty-testable.
    pub fn mount_split(
        &mut self,
        tree_root: BrowserNodeRef,
        read_only: bool,
        leaf: LeafHandles,
        form: FormHandles,
    ) {
        let db = self.app.desktop.get_bounds();
        // Build each pane at its real column rect (NOT a placeholder), so its child
        // widgets are laid out correctly; SplitContainer::new then lays them out to
        // the same rects (a no-op offset) and only takes over on later resizes.
        let (d0, d1) = initial_dividers(db);
        let rects = pane_rects(db, d0, d1);
        let tree = Box::new(DitOutline::new(rects[0], tree_root, self.selection.clone()));
        let leaves = Box::new(LeafListPane::new(rects[1], leaf));
        let form_pane = Box::new(FormPane::new(rects[2], read_only, form));
        let split = SplitContainer::new(db, tree, leaves, form_pane);
        self.app.desktop.add(Box::new(split));
    }

    /// The DN and `loaded` flag of the currently selected tree node, read from the
    /// shared [`Selection`] handle the windowed [`DitOutline`] publishes. `None`
    /// when no node is selected. No view reference, no downcast (spike §10.4).
    fn selected(&self) -> Option<(String, bool)> {
        self.selection.borrow().clone()
    }

    /// Run the manual event loop (spike §1/§9/§10). Each iteration: `idle()` →
    /// `on_event(Idle)` (where the read flow drains the worker channel) →
    /// `draw()` (desktop renders the menu, the DIT window+frame, and the status
    /// line) → flush → `poll_event(50ms)` → `app.handle_event` (Turbo Vision
    /// routes mouse/keyboard to the focused window's outline, and menu/status
    /// hotkeys). `CM_QUIT` (menu Quit / Alt-X) ends the loop.
    ///
    /// A single `on_event` callback is used (not separate idle/action callbacks)
    /// so the caller can own `&mut` browser / read-flow state in one closure
    /// without a double-mutable-borrow conflict. The callback receives
    /// `&mut Application` and a [`LoopEvent`]:
    /// * [`LoopEvent::Idle`] every tick;
    /// * [`LoopEvent::Action`] when a non-quit menu command (New <profile> /
    ///   Delete → [`crate::app::menu_action`], resolved against the current
    ///   selection) fires, or when the tree's [`CM_DIT_ACTIVATE`] command fires
    ///   (the user clicked a node) — surfaced as [`UiAction::Activate`] built from
    ///   the shared selection.
    ///
    /// All event routing to the tree is Turbo Vision's job now (the window is in
    /// the desktop hierarchy); the loop only translates the resulting app commands
    /// and the published selection into backend-agnostic [`UiAction`]s. The
    /// callback never sees a `turbo_vision` type, keeping callers backend-free.
    /// Not tty-testable.
    pub fn run_loop(
        &mut self,
        mut on_event: impl FnMut(&mut Application, LoopEvent),
        profile_count: usize,
    ) {
        self.app.running = true;
        while self.app.running {
            self.app.idle();
            on_event(&mut self.app, LoopEvent::Idle);
            self.app.draw();
            position_cursor(&mut self.app);
            let _ = self.app.terminal.flush();
            if let Ok(Some(mut ev)) = self.app.terminal.poll_event(Duration::from_millis(50)) {
                // Resolve a menu command against the current selection BEFORE the
                // app consumes the event (menu New <profile> / Delete).
                let menu_cmd = if ev.what == EventType::Command {
                    let selected = self.selected();
                    Some(menu_action(
                        ev.command,
                        profile_count,
                        selected.as_ref().map(|(dn, _)| dn.as_str()),
                    ))
                } else {
                    None
                };

                if ev.what == EventType::Command && ev.command == CM_QUIT {
                    self.app.running = false;
                    continue;
                }

                // Let Turbo Vision route the event: menu hotkeys, Alt-X, F10, and
                // (crucially) mouse/keyboard to the focused DIT window's outline.
                self.app.handle_event(&mut ev);

                if !self.app.running {
                    // Alt-X is handled inside app.handle_event and sets running=false.
                    continue;
                }

                // The FormPane's Save/Cancel buttons rewrite the in-flight event to
                // their command (button.rs sets `*event = Event::command(..)`); it
                // bubbles intact through both Group layers (verified against the
                // crate source). Surface them as backend-agnostic actions.
                if ev.what == EventType::Command && ev.command == CM_FORM_SAVE {
                    on_event(&mut self.app, LoopEvent::Action(UiAction::FormSave));
                    continue;
                }
                if ev.what == EventType::Command && ev.command == CM_FORM_CANCEL {
                    on_event(&mut self.app, LoopEvent::Action(UiAction::FormCancel));
                    continue;
                }

                // A tree click surfaces as CM_DIT_ACTIVATE (the wrapper emitted it;
                // the app left the unknown command intact, spike §10.1). Build the
                // Activate action from the freshly published selection.
                if ev.what == EventType::Command && ev.command == CM_DIT_ACTIVATE {
                    if let Some((dn, loaded)) = self.selected() {
                        on_event(
                            &mut self.app,
                            LoopEvent::Action(UiAction::Activate { dn, loaded }),
                        );
                    }
                    continue;
                }

                // A resolved non-quit menu command (New / Delete).
                if let Some(action) = menu_cmd {
                    if action != UiAction::None {
                        on_event(&mut self.app, LoopEvent::Action(action));
                    }
                }
            }
        }
    }

    /// Broadcast [`CM_DIT_REFRESH`] so the windowed [`DitOutline`] rebuilds its
    /// flattened display from the shared `Rc` tree (showing children attached
    /// during idle). Routed through `app.handle_event`, which dispatches the
    /// broadcast to the desktop and on to every window child — no handle to the
    /// inserted view and no downcast needed (spike §10.4). Not tty-testable.
    fn refresh(app: &mut Application) {
        let mut ev = Event::broadcast(CM_DIT_REFRESH);
        app.handle_event(&mut ev);
    }
}

/// Trigger a DIT tree refresh after children have been attached to the shared
/// node tree during idle (lazy expand, re-read after a write). Keeps `main.rs`
/// turbo-vision-free: the idle closure calls this, the facade broadcasts
/// [`CM_DIT_REFRESH`] to the windowed outline. Not tty-testable.
pub fn refresh_tree(app: &mut Application) {
    Shell::refresh(app);
}

/// Position the hardware text cursor for the focused view. turbo-vision 1.2's
/// `Desktop::update_cursor` is a no-op (it does not override the trait default),
/// so `app.draw()` never positions the cursor in the non-modal main loop — the
/// form's focused `InputLine` would show no cursor. Route the update to the
/// topmost desktop child (our [`SplitContainer`]); its `update_cursor` delegates
/// down through the focused pane to the focused widget, which calls
/// `terminal.show_cursor` (or hides it if the focused widget has no cursor). The
/// borrows of `app.desktop` and `app.terminal` are disjoint fields, so this is
/// sound. Not tty-testable.
fn position_cursor(app: &mut Application) {
    let n = app.desktop.child_count();
    if n > 0 {
        let child = app.desktop.child_at(n - 1);
        child.update_cursor(&mut app.terminal);
    }
}

/// Broadcast [`CM_LEAF_REFRESH`] so the [`LeafListPane`] rebuilds its rows from the
/// shared handle the run-loop just wrote. Keeps `main.rs` turbo-vision-free. Not
/// tty-testable.
pub fn refresh_leaf(app: &mut Application) {
    let mut ev = Event::broadcast(CM_LEAF_REFRESH);
    app.handle_event(&mut ev);
}

/// Broadcast [`CM_FORM_REFRESH`] so the [`FormPane`] (re)builds from the model the
/// run-loop just wrote into its shared handle (or clears when it is `None`). Keeps
/// `main.rs` turbo-vision-free. Not tty-testable.
pub fn refresh_form(app: &mut Application) {
    let mut ev = Event::broadcast(CM_FORM_REFRESH);
    app.handle_event(&mut ev);
}

/// Build the pane-1 branch tree from the eager [`Structure`]: a node per *branch*
/// (leaves live in pane 2), nested by parent links and fully expanded, rooted at
/// `structure.root_dn()`. Every node is marked `loaded` (the whole structure is
/// already in memory). The returned `Rc` tree is handed to `mount_split` and also
/// kept by the loop so it can be rebuilt in place after a reflow. Not unit-tested
/// (operates on the concrete `Node<BrowserNode>` behind the facade).
pub fn build_structure_tree(structure: &Structure) -> BrowserNodeRef {
    fn build(structure: &Structure, dn: &str) -> BrowserNodeRef {
        let (label, object_classes) = match structure.get(dn) {
            Some(n) => (n.label.clone(), n.object_classes.clone()),
            None => (
                dn.split(',').next().unwrap_or(dn).trim().to_string(),
                Vec::new(),
            ),
        };
        let node = new_node(BrowserNode {
            dn: dn.to_string(),
            label,
            loaded: true,
            object_classes,
        });
        if let Some(n) = structure.get(dn) {
            for child_dn in &n.children {
                if structure
                    .get(child_dn)
                    .map(|c| c.is_branch())
                    .unwrap_or(false)
                {
                    node.borrow_mut().add_child(build(structure, child_dn));
                }
            }
        }
        node.borrow_mut().expanded = true;
        node
    }
    build(structure, structure.root_dn())
}

/// Rebuild the branch tree under the existing `root` `Rc` in place from a (mutated)
/// [`Structure`], so the [`DitOutline`] that shares `root` reflects create/delete
/// reflows after the caller broadcasts [`refresh_tree`]. Replaces `root`'s label
/// and children rather than the node identity (the outline holds the same `Rc`).
/// Not unit-tested (touches the concrete node behind the facade).
pub fn rebuild_structure_tree(root: &BrowserNodeRef, structure: &Structure) {
    let fresh = build_structure_tree(structure);
    let new_children = fresh.borrow().children.clone();
    let new_label = fresh.borrow().data.label.clone();
    let mut r = root.borrow_mut();
    r.data.label = new_label;
    r.children = new_children;
    r.expanded = true;
}

/// Modal Save / Discard / Stay dialog for the dirty-form navigation guard (spec
/// §5.6). Returns the user's [`GuardChoice`]; closing the dialog any other way is
/// treated as `Stay` (the safe, non-destructive default). Not tty-testable.
pub fn confirm_guard(app: &mut Application) -> GuardChoice {
    // Use standard modal-end command ids so `Dialog::execute`'s loop actually
    // terminates when a button fires: the crate only ends the modal for
    // `CM_OK`/`CM_CANCEL`/`CM_YES`/`CM_NO` (and other ids < 1000). Custom ids
    // >= 1000 are treated as internal view commands and never close the dialog.
    // Save → CM_OK, Discard → CM_YES, Stay → CM_CANCEL (also Esc-Esc).
    const CM_SAVE: CommandId = CM_OK;
    const CM_DISCARD: CommandId = CM_YES;
    const CM_STAY: CommandId = CM_CANCEL;
    let mut d = Dialog::new(Rect::new(0, 0, 46, 9), "Unsaved changes");
    d.add(Box::new(StaticText::new(
        Rect::new(2, 1, 44, 3),
        "This entry has unsaved changes.",
    )));
    // Buttons MUST be at least 2 rows tall — Button::draw bails on height < 2.
    d.add(Box::new(Button::new(
        Rect::new(2, 4, 14, 6),
        "~S~ave",
        CM_SAVE,
        true,
    )));
    d.add(Box::new(Button::new(
        Rect::new(16, 4, 30, 6),
        "~D~iscard",
        CM_DISCARD,
        false,
    )));
    d.add(Box::new(Button::new(
        Rect::new(32, 4, 44, 6),
        "S~t~ay",
        CM_STAY,
        false,
    )));
    d.set_initial_focus();
    match d.execute(app) {
        x if x == CM_SAVE => GuardChoice::Save,
        x if x == CM_DISCARD => GuardChoice::Discard,
        _ => GuardChoice::Stay,
    }
}

// ---------------------------------------------------------------------------
// DIT browser outline (spike §6). The concrete tree-node type `Node<BrowserNode>`
// lives here so it never leaks past the facade; `crate::workflows::browser`
// drives expansion generically over the `ExpandableNode` trait, which we
// implement for the real node handle below.
// ---------------------------------------------------------------------------

/// The concrete reference-counted outline node carrying a [`BrowserNode`].
/// Other modules treat this as opaque via the `ExpandableNode` trait.
pub type BrowserNodeRef = Rc<RefCell<Node<BrowserNode>>>;

impl ExpandableNode for BrowserNodeRef {
    fn dn(&self) -> String {
        // `Node::data` is a public field (crate source views/outline.rs:27).
        self.borrow().data.dn.clone()
    }
    fn mark_loaded(&self) {
        self.borrow_mut().data.loaded = true;
    }
}

/// Wrap a [`BrowserNode`] payload in a fresh outline node (spike §6:
/// `Node::new(payload)` — one arg).
pub fn new_node(payload: BrowserNode) -> BrowserNodeRef {
    Rc::new(RefCell::new(Node::new(payload)))
}

/// Build the outline viewer rooted at `root`, rendering each node by its label
/// (labels-everywhere, spec §7). Not tty-testable.
pub fn build_outline(root: BrowserNodeRef) -> OutlineViewer<BrowserNode> {
    let mut v = OutlineViewer::new(Rect::new(1, 1, 40, 20), |n: &BrowserNode| n.label.clone());
    v.add_root(root);
    v
}

/// Attach freshly fetched child payloads under `parent` (spike §6:
/// `Node::add_child(Rc<RefCell<Node<T>>>)`). The parent is marked expanded so the
/// new children are visible after the next `set_roots`/`rebuild_display` (the
/// refresh broadcast); without this the freshly loaded subtree would stay folded.
/// Not tty-testable.
pub fn attach_children(parent: &BrowserNodeRef, kids: Vec<BrowserNode>) {
    let mut p = parent.borrow_mut();
    for child in kids {
        p.add_child(new_node(child));
    }
    p.expanded = true;
}

/// Find the node with the given DN by depth-first walk of the shared Rc tree
/// rooted at `root`. Returns the node handle so the browser can request its
/// children (lazy expand). DN comparison is case-insensitive (LDAP). Returns
/// `None` if no node matches. Not unit-tested (operates on the concrete
/// `Node<BrowserNode>` behind the facade), but pure of any tty.
pub fn find_node(root: &BrowserNodeRef, dn: &str) -> Option<BrowserNodeRef> {
    if root.borrow().data.dn.eq_ignore_ascii_case(dn) {
        return Some(root.clone());
    }
    let children: Vec<BrowserNodeRef> = root.borrow().children.to_vec();
    for child in &children {
        if let Some(found) = find_node(child, dn) {
            return Some(found);
        }
    }
    None
}

/// Clear a node's children and mark it unloaded so a subsequent
/// `BrowserState::request_children` re-fetches a fresh one-level listing (used
/// after add/delete/rename to reflect the tree change — Decision D4, re-read).
/// Touches the concrete `Node<BrowserNode>`, so it lives in the facade and is not
/// unit-tested; covered by the live test (Task 7).
pub fn clear_children(parent: &BrowserNodeRef) {
    let mut p = parent.borrow_mut();
    p.children.clear();
    p.data.loaded = false;
}

/// The DN of the tree root node (the base DN), used as the default container for
/// a new entry when its profile has no `search_base`. Reads the concrete node, so
/// it lives in the facade; trivially correct, not unit-tested.
pub fn root_dn(root: &BrowserNodeRef) -> String {
    root.borrow().data.dn.clone()
}

// ---------------------------------------------------------------------------
// Read-only entry form dialog (spike §2) + error message box (spike §8).
//
// Deviation (documented): the spike verified the `DialogBuilder`/`ButtonBuilder`/
// `StaticTextBuilder` fluent path, but the crate also exposes the simpler direct
// constructors `Dialog::new`/`StaticText::new`/`Button::new` (verified in the
// crate source: views/dialog.rs, static_text.rs, button.rs). We use those —
// fewer moving parts for a read-only form. Booleans render as a static `[x]`/
// `[ ]` glyph rather than a disabled checkbox cluster: the crate's checkbox
// primitive (`CheckBoxes::new(Rect, Vec<String>)`) is an editable cluster, which
// would imply an input affordance; a static glyph is unambiguously read-only and
// keeps the write path firmly out of M3.
// ---------------------------------------------------------------------------

/// Render a field's value as the read-only display string shown in the dialog.
fn field_display(widget: &WidgetSpec, values: &[String]) -> String {
    match widget {
        WidgetSpec::DisabledCheckBox(b) => {
            if *b {
                "[x]".to_string()
            } else {
                "[ ]".to_string()
            }
        }
        WidgetSpec::BinaryNote(n) => format!("<{n} bytes>"),
        // Text / Int / Dn / Time: join multi-values for display only.
        _ => values.join(", "),
    }
}

/// Build the modal read-only entry dialog from a [`FormModel`] (spike §2).
///
/// One row per field: a label (with a `*` suffix for MUST attributes) and the
/// field's read-only value rendering; a single Close button shown via
/// `dialog.execute(&mut app)`. Not tty-testable.
pub fn build_entry_dialog(model: &FormModel) -> Dialog {
    let width: i16 = 70;
    let rows = model.fields.len() as i16;
    let height = (rows + 5).clamp(7, 24);
    let mut dialog = Dialog::new(Rect::new(0, 0, width, height), &model.title);

    let mut y: i16 = 1;
    for field in &model.fields {
        let label = if field.is_must {
            format!("{} *", field.label)
        } else {
            field.label.clone()
        };
        dialog.add(Box::new(StaticText::new(
            Rect::new(2, y, 28, y + 1),
            &label,
        )));
        let value = field_display(&field.widget, &field.values);
        dialog.add(Box::new(StaticText::new(
            Rect::new(30, y, width - 2, y + 1),
            &value,
        )));
        y += 1;
        if y >= height - 3 {
            break; // keep the close button visible; paging is M4+.
        }
    }

    dialog.add(Box::new(Button::new(
        Rect::new(width / 2 - 6, height - 2, width / 2 + 6, height - 1),
        "~C~lose",
        CM_CANCEL,
        true,
    )));
    dialog.set_initial_focus();
    dialog
}

/// Show a modal read-only entry dialog and block until the user closes it.
/// Not tty-testable.
pub fn show_entry_dialog(app: &mut Application, model: &FormModel) {
    let mut dialog = build_entry_dialog(model);
    let _ = dialog.execute(app);
}

/// Show a modal error message box (spike §8). Used to surface `Response::Error`
/// / `Response::SearchError` to the operator. Not tty-testable.
pub fn confirm_error(app: &mut Application, msg: &str) {
    let _ = message_box(app, msg, MF_ERROR | MF_OK_BUTTON);
}

/// Show a Yes/No confirmation (spike §8) and return `true` for Yes. Keeps the
/// `CM_YES`/`CM_NO` branching inside the facade so callers stay turbo-vision-free.
/// Not tty-testable.
pub fn confirm(app: &mut Application, msg: &str) -> bool {
    message_box(app, msg, MF_CONFIRMATION | MF_YES_BUTTON | MF_NO_BUTTON) == CM_YES
}

/// Show an informational message box with an OK button (spike §8). Used for the
/// post-write success notice ("no silent success", spec §10). Not tty-testable.
pub fn info(app: &mut Application, msg: &str) {
    let _ = message_box(app, msg, MF_INFORMATION | MF_OK_BUTTON);
}

/// Local command id for the "Preview LDIF" button inside the edit dialog. Chosen
/// not to collide with the standard `CM_OK`/`CM_CANCEL` ids.
const CM_PREVIEW: u16 = 2001;

/// Whether a form field is editable in the entry dialog. Read-only kinds (binary
/// notes, disabled checkboxes) and the never-writable `memberOf` (spec §8) render
/// as static text; everything else becomes an editable [`InputLine`]. Pure, so
/// it is unit-tested.
fn field_is_editable(field: &crate::ui::form::FormField) -> bool {
    if field.label.eq_ignore_ascii_case("memberOf") {
        return false;
    }
    !matches!(
        field.widget,
        WidgetSpec::BinaryNote(_) | WidgetSpec::DisabledCheckBox(_)
    )
}

/// Format a list of [`ValidationError`]s as a single multi-line message for a
/// message box. Pure, unit-tested.
pub fn format_validation_errors(errors: &[ValidationError]) -> String {
    let mut out = String::from("Cannot save — please fix:");
    for e in errors {
        let line = match e {
            ValidationError::MissingMust(a) => format!("missing required attribute: {a}"),
            ValidationError::MultiValueOnSingle(a) => {
                format!("attribute is single-valued: {a}")
            }
            ValidationError::SyntaxInvalid { attr, reason } => format!("{attr}: {reason}"),
        };
        out.push_str("\n- ");
        out.push_str(&line);
    }
    out
}

/// An editable field's attribute name plus the `Rc<RefCell<String>>` the bound
/// `InputLine` mutates in place (spike §3).
type FieldBinding = (String, Rc<RefCell<String>>);

/// Build an [`EditEntry`] from a `FormModel`'s DN plus the live bindings of its
/// editable fields. Non-editable fields keep their original values. Pure given
/// the bindings, so it is unit-tested via the binding map.
fn collect_edit_entry(dn: &str, model: &FormModel, bindings: &[FieldBinding]) -> EditEntry {
    let mut attrs = std::collections::BTreeMap::new();
    // Start from the original values of every field.
    for field in &model.fields {
        attrs.insert(field.label.clone(), field.values.clone());
    }
    // Overlay edited values (one binding per editable field). A binding holds a
    // newline-separated multi-value string so several values can be edited.
    for (label, data) in bindings {
        let raw = data.borrow().clone();
        let values: Vec<String> = raw
            .split('\n')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        attrs.insert(label.clone(), values);
    }
    EditEntry {
        dn: dn.to_string(),
        attrs,
    }
}

/// Show the LDIF preview of the pending changes in a modal box (spec §12 F1:
/// "show exactly what will be sent"). Not tty-testable.
pub fn show_ldif_preview(app: &mut Application, ldif: &str) {
    let body = if ldif.trim().is_empty() {
        "(no changes)".to_string()
    } else {
        ldif.to_string()
    };
    let _ = message_box(app, &body, MF_INFORMATION | MF_OK_BUTTON);
}

/// Build + run a modal editable entry dialog and return the resulting
/// [`EditEntry`] on Save, or `None` on Cancel. The DN comes from `model.title`.
///
/// Each editable field is an [`InputLine`] bound to an `Rc<RefCell<String>>`
/// seeded from the field's values (joined by newline for multi-valued attrs);
/// read-only / `memberOf` fields render as static text. Buttons: `~S~ave`
/// (`CM_OK`), `~C~ancel` (`CM_CANCEL`), `~P~review LDIF` (`CM_PREVIEW`). On
/// `CM_PREVIEW` the current bindings are diffed against the original and the LDIF
/// shown, then the dialog re-runs. Not tty-testable.
pub fn edit_entry_dialog(app: &mut Application, model: &FormModel) -> Option<EditEntry> {
    let dn = model.title.clone();
    let original = entry_to_edit(&dn, model);

    loop {
        let width: i16 = 72;
        let rows = model.fields.len() as i16;
        let height = (rows + 5).clamp(8, 24);
        let mut dialog = Dialog::new(Rect::new(0, 0, width, height), &dn);

        let mut bindings: Vec<(String, Rc<RefCell<String>>)> = Vec::new();
        let mut y: i16 = 1;
        for field in &model.fields {
            let label = if field.is_must {
                format!("{} *", field.label)
            } else {
                field.label.clone()
            };
            dialog.add(Box::new(StaticText::new(
                Rect::new(2, y, 28, y + 1),
                &label,
            )));

            if field_is_editable(field) {
                let seed = field.values.join("\n");
                let data = Rc::new(RefCell::new(seed));
                // `InputLine::new(bounds, max_length, data)` binds the shared
                // String directly (spike §3, verified signature).
                let input = InputLine::new(Rect::new(30, y, width - 2, y + 1), 1024, data.clone());
                dialog.add(Box::new(input));
                bindings.push((field.label.clone(), data));
            } else {
                let value = field_display(&field.widget, &field.values);
                dialog.add(Box::new(StaticText::new(
                    Rect::new(30, y, width - 2, y + 1),
                    &value,
                )));
            }
            y += 1;
            if y >= height - 3 {
                break;
            }
        }

        dialog.add(Box::new(Button::new(
            Rect::new(2, height - 3, 14, height - 1),
            "~S~ave",
            CM_OK,
            true,
        )));
        dialog.add(Box::new(Button::new(
            Rect::new(16, height - 3, 28, height - 1),
            "~C~ancel",
            CM_CANCEL,
            false,
        )));
        dialog.add(Box::new(Button::new(
            Rect::new(30, height - 3, 48, height - 1),
            "~P~review LDIF",
            CM_PREVIEW,
            false,
        )));
        dialog.set_initial_focus();

        match dialog.execute(app) {
            x if x == CM_OK => {
                return Some(collect_edit_entry(&dn, model, &bindings));
            }
            x if x == CM_PREVIEW => {
                let edited = collect_edit_entry(&dn, model, &bindings);
                let ldif = match diff(&original, &edited) {
                    Ok(cs) => render_changeset(&cs),
                    Err(e) => format!("# cannot render: {e}"),
                };
                show_ldif_preview(app, &ldif);
                // Re-loop to re-present the dialog (a fresh build re-seeds from the
                // original values; acceptable for M4 preview).
                continue;
            }
            _ => return None,
        }
    }
}

/// Build the `EditEntry` of the *original* values from a `FormModel` (used as the
/// diff baseline). Pure, unit-tested.
fn entry_to_edit(dn: &str, model: &FormModel) -> EditEntry {
    let mut attrs = std::collections::BTreeMap::new();
    for field in &model.fields {
        attrs.insert(field.label.clone(), field.values.clone());
    }
    EditEntry {
        dn: dn.to_string(),
        attrs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facade_boundary_compiles() {
        assert!(tv_available());
    }

    #[test]
    fn dividers_clamp_and_panes_tile() {
        let (d0, d1) = clamp_dividers(0, 60, -100, 1000);
        assert!(d0 >= 8 && d1 > d0 && d1 <= 52);
        let panes = pane_rects(Rect::new(0, 0, 60, 10), d0, d1);
        assert_eq!(panes[0].a.x, 0);
        assert_eq!(panes[1].a.x, d0 + 1);
        assert_eq!(panes[2].b.x, 60);
        assert!(panes[0].b.x <= panes[1].a.x);
        assert!(panes[1].b.x <= panes[2].a.x);
    }

    #[test]
    fn scroll_clamps_to_content() {
        assert_eq!(clamp_scroll(-5, 20, 10), 0);
        assert_eq!(clamp_scroll(100, 20, 10), 10);
        assert_eq!(clamp_scroll(3, 20, 10), 3);
        assert_eq!(clamp_scroll(5, 8, 10), 0);
    }

    #[test]
    fn cm_quit_matches_app() {
        // The app-layer mirror constant must equal the real Turbo Vision value
        // so command dispatch in non-facade code lines up.
        assert_eq!(tv_cm_quit(), crate::app::CM_QUIT);
    }

    #[test]
    fn field_display_renders_read_only_values() {
        // The dialog construction needs a tty, but this value-rendering helper
        // does not, so we lock its read-only output down here.
        assert_eq!(
            field_display(&WidgetSpec::DisabledCheckBox(true), &[]),
            "[x]"
        );
        assert_eq!(
            field_display(&WidgetSpec::DisabledCheckBox(false), &[]),
            "[ ]"
        );
        assert_eq!(
            field_display(&WidgetSpec::BinaryNote(12), &[]),
            "<12 bytes>"
        );
        assert_eq!(
            field_display(
                &WidgetSpec::ReadOnlyText,
                &["a".to_string(), "b".to_string()]
            ),
            "a, b"
        );
    }

    #[test]
    fn node_ref_exposes_dn_and_marks_loaded() {
        let n = new_node(BrowserNode {
            dn: "ou=people,dc=example,dc=org".to_string(),
            label: "people".to_string(),
            loaded: false,
            object_classes: vec![],
        });
        assert_eq!(n.dn(), "ou=people,dc=example,dc=org");
        assert!(!n.borrow().data.loaded);
        n.mark_loaded();
        assert!(n.borrow().data.loaded);
    }

    #[test]
    fn attach_children_adds_payloads_and_expands() {
        let parent = new_node(BrowserNode {
            dn: "dc=example,dc=org".to_string(),
            label: "root".to_string(),
            loaded: false,
            object_classes: vec![],
        });
        attach_children(
            &parent,
            vec![BrowserNode {
                dn: "ou=people,dc=example,dc=org".to_string(),
                label: "people".to_string(),
                loaded: false,
                object_classes: vec![],
            }],
        );
        assert_eq!(parent.borrow().children.len(), 1);
        // Newly loaded subtree is expanded so the refresh shows it.
        assert!(parent.borrow().expanded);
        assert_eq!(parent.dn(), "dc=example,dc=org");
    }
}
