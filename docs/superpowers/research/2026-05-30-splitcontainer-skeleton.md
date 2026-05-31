// SplitContainer — three vertical, mouse-resizable columns for turbo-vision-4-rust.
//
// DESIGN SKELETON (not a compiled/tested build). Layout 1: tree | list | form.
//
// Why this is a custom view rather than framework glue:
//   * turbo-vision-4-rust has NO splitter/pane widget (confirmed: no `splitter`
//     symbol anywhere in the crate).
//   * `Group::set_bounds` applies a UNIFORM dw/dh to every child's bottom-right
//     corner (Borland's per-view growMode flags were not ported), so a plain
//     Group cannot give "tree keeps width, form absorbs slack" behavior.
//   * Window "resizable" only resizes the OUTER frame, not internal divisions.
// So we own the three children and the two divider positions ourselves, lay them
// out, draw the dividers, and adjust the splits on mouse-drag.
//
// Mount it as the window's single interior child:
//     let mut win = WindowBuilder::new().bounds(...).title("Panels").build();
//     win.add(Box::new(SplitContainer::new(interior_bounds, tree, list, form)));
//
// where:
//     tree = Box::new(OutlineViewer::new(...))            // hierarchical list, part (a)
//     list = Box::new(ListBox::new(bounds, on_select))    // ordinary flat list
//     form = Box::new(build_form_group(bounds))           // Group of Label/InputLine/Button

use crate::core::draw::DrawBuffer;
use crate::core::event::{Event, EventType, KB_TAB, KB_SHIFT_TAB};
use crate::core::geometry::{Point, Rect};
use crate::core::palette::Palette;
use crate::core::palette_chain::PaletteChainNode;
use crate::core::state::{StateFlags, SF_FOCUSED, SF_RESIZING};
use crate::terminal::Terminal;
use crate::views::view::{View, write_line_to_terminal};

/// Minimum interior width (in columns) any pane is allowed to shrink to.
const MIN_PANE_W: i16 = 8;
/// Columns occupied by a divider bar.
const DIVIDER_W: i16 = 1;

pub struct SplitContainer {
    bounds: Rect,
    state: StateFlags,
    /// [0] = tree (OutlineViewer), [1] = list (ListBox), [2] = form (Group)
    panes: [Box<dyn View>; 3],
    /// Which pane currently owns keyboard focus (0..=2).
    focused_pane: usize,
    /// Absolute screen x of divider 0 (between pane 0 and 1) and divider 1 (1 and 2).
    split_x: [i16; 2],
    /// Index (0 or 1) of the divider currently being dragged, if any.
    dragging: Option<usize>,
    palette_chain: Option<PaletteChainNode>,
}

impl SplitContainer {
    /// `bounds` is the absolute interior rect (e.g. window interior, frame already excluded).
    /// Children are positioned by `layout()`, so their incoming bounds are ignored.
    pub fn new(bounds: Rect, tree: Box<dyn View>, list: Box<dyn View>, form: Box<dyn View>) -> Self {
        // Initial split: roughly thirds.
        let w = bounds.width();
        let s0 = bounds.a.x + w / 3;
        let s1 = bounds.a.x + (2 * w) / 3;
        let mut me = Self {
            bounds,
            state: 0,
            panes: [tree, list, form],
            focused_pane: 0,
            split_x: [s0, s1],
            dragging: None,
            palette_chain: None,
        };
        me.clamp_splits();
        me.layout();
        me
    }

    /// Keep dividers ordered and each pane >= MIN_PANE_W. Call after any split/bounds change.
    fn clamp_splits(&mut self) {
        let left = self.bounds.a.x;
        let right = self.bounds.b.x;
        // divider 0 must leave room for pane 0 on its left and a minimal middle to its right
        self.split_x[0] = self.split_x[0]
            .max(left + MIN_PANE_W)
            .min(right - 2 * MIN_PANE_W - DIVIDER_W);
        // divider 1 must sit at least MIN_PANE_W past divider 0, and leave pane 2 room
        self.split_x[1] = self.split_x[1]
            .max(self.split_x[0] + DIVIDER_W + MIN_PANE_W)
            .min(right - MIN_PANE_W);
    }

    /// Assign each pane its column rect. Dividers occupy split_x[0] and split_x[1].
    fn layout(&mut self) {
        let (top, bottom) = (self.bounds.a.y, self.bounds.b.y);
        let left = self.bounds.a.x;
        let right = self.bounds.b.x;
        let (d0, d1) = (self.split_x[0], self.split_x[1]);

        self.panes[0].set_bounds(Rect::new(left,            top, d0,    bottom));
        self.panes[1].set_bounds(Rect::new(d0 + DIVIDER_W,  top, d1,    bottom));
        self.panes[2].set_bounds(Rect::new(d1 + DIVIDER_W,  top, right, bottom));
    }

    /// If `p` lands on a divider column, return its index (0 or 1).
    fn divider_at(&self, p: Point) -> Option<usize> {
        if p.y < self.bounds.a.y || p.y >= self.bounds.b.y {
            return None;
        }
        for (i, &x) in self.split_x.iter().enumerate() {
            if p.x == x {
                return Some(i);
            }
        }
        None
    }

    /// Which pane (0..=2) contains point `p`, if any.
    fn pane_at(&self, p: Point) -> Option<usize> {
        self.panes.iter().position(|pane| pane.bounds().contains(p))
    }

    fn set_focus_to(&mut self, pane: usize) {
        for (i, p) in self.panes.iter_mut().enumerate() {
            p.set_focus(i == pane);
        }
        self.focused_pane = pane;
    }

    /// Cycle focus across the three panes (Tab forward / Shift+Tab back),
    /// skipping panes whose `can_focus()` is false.
    fn cycle_focus(&mut self, forward: bool) {
        let n = self.panes.len();
        let mut idx = self.focused_pane;
        for _ in 0..n {
            idx = if forward { (idx + 1) % n } else { (idx + n - 1) % n };
            if self.panes[idx].can_focus() {
                self.set_focus_to(idx);
                return;
            }
        }
    }
}

impl View for SplitContainer {
    fn bounds(&self) -> Rect {
        self.bounds
    }

    /// Called when the outer window is resized. We rescale the split positions
    /// proportionally so each pane keeps roughly its share of the new width,
    /// then re-clamp and re-lay-out the children.
    fn set_bounds(&mut self, new: Rect) {
        let old_w = self.bounds.width().max(1);
        let new_w = new.width().max(1);
        // Convert each divider to a fraction of the old interior, then re-anchor.
        for d in &mut self.split_x {
            let frac = (*d - self.bounds.a.x) as f32 / old_w as f32;
            *d = new.a.x + (frac * new_w as f32).round() as i16;
        }
        self.bounds = new;
        self.clamp_splits();
        self.layout();
    }

    fn draw(&mut self, terminal: &mut Terminal) {
        // Establish this view's palette-chain node for children (same pattern as Group::draw).
        let node = PaletteChainNode::new(self.get_palette(), self.palette_chain.clone());
        for pane in &mut self.panes {
            pane.set_palette_chain(Some(node.clone()));
            pane.draw(terminal);
        }

        // Draw the two vertical divider bars on top of the gap between panes.
        let attr = self.map_color(1); // TODO: pick a dedicated divider palette slot
        for &x in &self.split_x {
            let mut buf = DrawBuffer::new(DIVIDER_W as usize);
            for y in self.bounds.a.y..self.bounds.b.y {
                buf.move_char(0, '│', attr, DIVIDER_W as usize);
                write_line_to_terminal(terminal, x, y, &buf);
            }
        }
    }

    fn handle_event(&mut self, event: &mut Event) {
        match event.what {
            // --- begin / continue / end a divider drag -------------------------------
            EventType::MouseDown => {
                if let Some(i) = self.divider_at(event.mouse.pos) {
                    self.dragging = Some(i);
                    self.set_state_flag(SF_RESIZING, true); // keeps mouse capture (see Group::handle_event)
                    event.clear();
                    return;
                }
                // Click inside a pane: focus it, then forward the click.
                if let Some(p) = self.pane_at(event.mouse.pos) {
                    self.set_focus_to(p);
                    self.panes[p].handle_event(event);
                }
                return;
            }
            EventType::MouseMove => {
                if let Some(i) = self.dragging {
                    self.split_x[i] = event.mouse.pos.x;
                    self.clamp_splits();
                    self.layout();
                    event.clear();
                    return;
                }
            }
            EventType::MouseUp => {
                if self.dragging.is_some() {
                    self.dragging = None;
                    self.set_state_flag(SF_RESIZING, false);
                    event.clear();
                    return;
                }
                // Forward release to whichever pane is under the cursor.
                if let Some(p) = self.pane_at(event.mouse.pos) {
                    self.panes[p].handle_event(event);
                }
                return;
            }
            // --- keyboard: Tab cycles panes, everything else goes to the focused pane -
            EventType::Keyboard => {
                match event.key_code {
                    KB_TAB => {
                        self.cycle_focus(true);
                        event.clear();
                        return;
                    }
                    KB_SHIFT_TAB => {
                        self.cycle_focus(false);
                        event.clear();
                        return;
                    }
                    _ => {
                        self.panes[self.focused_pane].handle_event(event);
                        return;
                    }
                }
            }
            _ => {}
        }
    }

    fn can_focus(&self) -> bool {
        true
    }

    fn set_focus(&mut self, focused: bool) {
        self.set_state_flag(SF_FOCUSED, focused);
        // When the container gains focus, hand it to the active pane.
        if focused {
            let p = self.focused_pane;
            self.set_focus_to(p);
        }
    }

    fn state(&self) -> StateFlags {
        self.state
    }

    fn set_state(&mut self, state: StateFlags) {
        self.state = state;
    }

    fn set_palette_chain(&mut self, node: Option<PaletteChainNode>) {
        self.palette_chain = node;
    }

    fn get_palette_chain(&self) -> Option<&PaletteChainNode> {
        self.palette_chain.as_ref()
    }

    /// Transparent to color mapping (children carry their own palettes), like a Group.
    fn get_palette(&self) -> Option<Palette> {
        None
    }

    // update_cursor(): TODO — forward to self.panes[self.focused_pane] so the
    // focused InputLine in the form shows its caret.
}

// ----------------------------------------------------------------------------
// Outstanding work to turn this skeleton into a build (intentionally omitted):
//
//  1. build_form_group(bounds) -> Box<Group>: Label + InputLine
//     (InputLine::new(bounds, max_len, Rc<RefCell<String>>)) + Button, mirroring
//     how Dialog hosts controls — but in a plain Group so it lives inside a pane.
//  2. update_cursor passthrough to the focused pane.
//  3. A dedicated palette slot for the divider glyph instead of map_color(1).
//  4. Optional: render a different divider glyph / highlight while `dragging`.
//  5. Generalization: replace the fixed [_; 3] with a reusable two-child `Splitter`
//     you nest, if you want arbitrary pane counts later.
// ----------------------------------------------------------------------------