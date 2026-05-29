//! Turbo Vision facade — the ONLY module in the crate that may `use turbo_vision`.
//!
//! Boundary rule (spec §8 / §14): every other module talks to the TUI
//! exclusively through plain domain types (`MenuDef`, `BrowserNode`, …). No
//! `turbo_vision` type may leak past this file. Keeping the dependency confined
//! here makes the backend swappable and keeps the rest of the crate testable
//! without a terminal.
//!
//! Tty boundary (spec §11): `Shell::new`/`run_loop`, `build_menu_bar`,
//! `build_status_line`, `build_outline`, and `attach_children` require a real
//! terminal and are NOT unit-tested. The logic they consume lives below the
//! facade in pure, tested functions (`crate::app::build_menu_defs`,
//! `crate::workflows::browser::*`).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use turbo_vision::app::Application;
use turbo_vision::core::command::CM_QUIT;
use turbo_vision::core::event::{EventType, KB_ALT_X, KB_F10};
use turbo_vision::core::geometry::Rect;
use turbo_vision::core::menu_data::MenuBuilder;
use turbo_vision::views::menu_bar::{MenuBar, SubMenu};
use turbo_vision::views::outline::{Node, OutlineViewer};
use turbo_vision::views::status_line::{StatusItem, StatusLine};

use crate::app::MenuDef;
use crate::workflows::browser::{BrowserNode, ExpandableNode};

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

/// Build the bottom status line (spike §1). Not tty-testable.
pub fn build_status_line(size_w: i16, size_h: i16) -> StatusLine {
    StatusLine::new(
        Rect::new(0, size_h - 1, size_w, size_h),
        vec![
            StatusItem::new("~Alt+X~ Quit", KB_ALT_X, CM_QUIT),
            StatusItem::new("~F10~ Menu", KB_F10, 0),
        ],
    )
}

/// The application shell: owns the Turbo Vision [`Application`] and drives the
/// manual event loop. Construction requires a real terminal.
pub struct Shell {
    app: Application,
}

impl Shell {
    /// Build the application, install the profile-derived menu bar and the
    /// status line. Requires a tty (`Application::new()` puts the terminal into
    /// raw mode). Not tty-testable.
    pub fn new(defs: &[MenuDef]) -> anyhow::Result<Shell> {
        let mut app = Application::new()?;
        let (w, h) = app.terminal.size();
        app.set_menu_bar(build_menu_bar(w, defs));
        app.set_status_line(build_status_line(w, h));
        Ok(Shell { app })
    }

    /// Run the manual event loop (spike §1/§9). Each iteration: `idle()` →
    /// `on_idle` (where Task 6 drains the worker channel) → `draw()` → flush →
    /// `poll_event(50ms)`. `CM_QUIT` (menu Quit / Alt-X) ends the loop. Not
    /// tty-testable.
    pub fn run_loop(&mut self, mut on_idle: impl FnMut(&mut Application)) {
        self.app.running = true;
        while self.app.running {
            self.app.idle();
            on_idle(&mut self.app);
            self.app.draw();
            let _ = self.app.terminal.flush();
            if let Ok(Some(mut ev)) = self.app.terminal.poll_event(Duration::from_millis(50)) {
                self.app.handle_event(&mut ev);
                if ev.what == EventType::Command && ev.command == CM_QUIT {
                    self.app.running = false;
                }
            }
        }
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
/// `Node::add_child(Rc<RefCell<Node<T>>>)`). Not tty-testable.
pub fn attach_children(parent: &BrowserNodeRef, kids: Vec<BrowserNode>) {
    for child in kids {
        parent.borrow_mut().add_child(new_node(child));
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
    fn cm_quit_matches_app() {
        // The app-layer mirror constant must equal the real Turbo Vision value
        // so command dispatch in non-facade code lines up.
        assert_eq!(tv_cm_quit(), crate::app::CM_QUIT);
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
    fn attach_children_adds_payloads() {
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
        assert_eq!(parent.dn(), "dc=example,dc=org");
    }
}
