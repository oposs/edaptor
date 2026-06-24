//! Program assembly: desktop, menu bar, status line, three-pane splitter, pump.

use tvision_rs::{
    self as tv, alt, Command, Constraints, Desktop, Program, Rect, Splitter, StatusDef, StatusLine,
    SystemClock, Theme, View, Window,
};

use crate::tui::panes::{
    form::FormPane,
    leaf::LeafPane,
    tree::{build_branch_nodes, TreePane},
};
use crate::tui::pump::PumpView;
use crate::tui::Shared;

fn init_status_line(r: Rect) -> Option<Box<dyn View>> {
    let mut r = r;
    r.a.y = r.b.y - 1;
    let defs = StatusDef::list()
        .def_all(|d| d.item("~Alt-X~ Exit", alt('x'), Command::QUIT))
        .build();
    Some(Box::new(StatusLine::new(r, defs)))
}

fn init_menu_bar(r: Rect) -> Option<Box<dyn View>> {
    let mut r = r;
    r.b.y = r.a.y + 1;
    let menu = tv::Menu::builder()
        .submenu("~F~ile", alt('f'), |m| {
            m.command_key("E~x~it", Command::QUIT, alt('x'), "Alt-X")
        })
        .build();
    Some(Box::new(tv::MenuBar::new(r, menu)))
}

fn init_desktop(r: Rect, state: Shared) -> Option<Box<dyn View>> {
    let mut r = r;
    r.a.y += 1; // below menu bar
    r.b.y -= 1; // above status line
    let mut desktop = Desktop::new(r, |br| Some(Desktop::init_background(br)));

    let win_rect = Rect::new(r.a.x + 1, r.a.y, r.b.x - 1, r.b.y);
    let mut win = Window::new(win_rect, Some("edaptor".to_string()), 1);
    let ext = win.state().get_extent();
    let interior = Rect::new(1, 1, ext.b.x - 1, ext.b.y - 1);
    let width = (interior.b.x - interior.a.x).max(8) as usize;

    // Build the branch tree and record the DFS DN map.
    // Take the immutable borrow, destructure the result, DROP borrow, then mutate.
    let (root, dn_map) = {
        let st = state.borrow();
        build_branch_nodes(&st, width / 3)
    }; // borrow dropped here
    state.borrow_mut().branch_dns = dn_map;

    let tree: Box<dyn View> = Box::new(TreePane::new(interior, root, state.clone()));
    let leaf: Box<dyn View> = Box::new(LeafPane::new(interior, state.clone()));
    let form: Box<dyn View> = Box::new(FormPane::new(interior, state.clone()));

    let split = Splitter::cols()
        .pane(tree, Constraints::flex().min(16))
        .pane(leaf, Constraints::flex().min(16))
        .pane(form, Constraints::flex().min(20))
        .joined();

    let split_id = win.insert_child(Box::new(split));
    if let Some(v) = win.child_mut(split_id) {
        v.change_bounds(interior);
    }
    win.insert_child(Box::new(PumpView::new(state.clone())));

    desktop.insert_view(Box::new(win));
    Some(Box::new(desktop))
}

pub(crate) fn build_program(backend: Box<dyn tv::Backend>, state: Shared) -> Program {
    let s = state.clone();
    Program::new(
        backend,
        Box::new(SystemClock::new()),
        Theme::classic_blue(),
        move |r| init_desktop(r, s.clone()),
        init_status_line,
        init_menu_bar,
    )
}
