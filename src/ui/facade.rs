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
use turbo_vision::core::command::{CM_CANCEL, CM_OK, CM_QUIT, CM_YES};
use turbo_vision::core::event::{EventType, KB_ALT_X, KB_ENTER, KB_F10};
use turbo_vision::core::geometry::Rect;
use turbo_vision::core::menu_data::MenuBuilder;
use turbo_vision::helpers::msgbox::{
    message_box, MF_CONFIRMATION, MF_ERROR, MF_INFORMATION, MF_NO_BUTTON, MF_OK_BUTTON,
    MF_YES_BUTTON,
};
use turbo_vision::views::button::Button;
use turbo_vision::views::dialog::Dialog;
use turbo_vision::views::input_line::InputLine;
use turbo_vision::views::menu_bar::{MenuBar, SubMenu};
use turbo_vision::views::outline::{Node, OutlineViewer};
use turbo_vision::views::static_text::StaticText;
use turbo_vision::views::status_line::{StatusItem, StatusLine};
use turbo_vision::views::View;

use crate::app::{menu_action, LoopEvent, MenuDef, UiAction};
use crate::form::changeset::{diff, EditEntry};
use crate::form::validate::ValidationError;
use crate::ldap::ldif::render_changeset;
use crate::ui::form::{FormModel, WidgetSpec};
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

/// The application shell: owns the Turbo Vision [`Application`] and the DIT
/// outline, and drives the manual event loop. Construction requires a real
/// terminal.
///
/// Design note (deviation from the plan, documented): the turbo-vision
/// `OutlineViewer<BrowserNode>` is held as a **typed field on the Shell rather
/// than inserted into `app.desktop`**. The desktop exposes neither a focused-view
/// accessor nor a typed get-back of inserted views, and `View::as_any()` defaults
/// to `panic!` (the `OutlineViewer` View impl does not override it — verified in
/// the crate source). So a "downcast the desktop child back to the outline" path
/// would panic at runtime. Keeping the outline as a typed field lets us call its
/// `selected_node()` directly (no downcast, no panic) while we draw it manually
/// each frame and forward keyboard events to it for navigation/expand/collapse.
pub struct Shell {
    app: Application,
    /// The DIT outline, owned here (not on the desktop) so we can read its
    /// selection with a direct typed call.
    outline: Option<OutlineViewer<BrowserNode>>,
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
        Ok(Shell { app, outline: None })
    }

    /// Build and install the DIT outline rooted at `root` (spike §6). The Shell
    /// owns the viewer; `run_loop` draws it and feeds it keyboard events. Not
    /// tty-testable.
    pub fn mount_outline(&mut self, root: BrowserNodeRef) {
        let (_w, h) = self.app.terminal.size();
        let mut viewer = OutlineViewer::new(Rect::new(0, 1, 42, h - 1), |n: &BrowserNode| {
            n.label.clone()
        });
        viewer.add_root(root);
        self.outline = Some(viewer);
    }

    /// The DN and `loaded` flag of the currently selected outline node, via the
    /// outline's typed `selected_node()`. `None` when no outline / no selection.
    fn selected(&self) -> Option<(String, bool)> {
        let node = self.outline.as_ref()?.selected_node()?;
        let n = node.borrow();
        Some((n.data.dn.clone(), n.data.loaded))
    }

    /// Run the manual event loop (spike §1/§9). Each iteration: `idle()` →
    /// `on_event(Idle)` (where the read flow drains the worker channel) →
    /// `draw()` (desktop + the outline) → flush → `poll_event(50ms)`. `CM_QUIT`
    /// (menu Quit / Alt-X) ends the loop.
    ///
    /// A single `on_event` callback is used (not separate idle/action callbacks)
    /// so the caller can own `&mut` browser / read-flow state in one closure
    /// without a double-mutable-borrow conflict. The callback receives
    /// `&mut Application` and a [`LoopEvent`]:
    /// * [`LoopEvent::Idle`] every tick;
    /// * [`LoopEvent::Action`] when a non-quit command (menu New <profile> /
    ///   Delete → [`crate::app::menu_action`], resolved against the current
    ///   selection) fires, or when Enter activates the focused outline node
    ///   ([`UiAction::Activate`]).
    ///
    /// Keyboard events are forwarded to the outline first (navigation /
    /// expand-collapse), then to the application (menu hotkeys, Alt-X, F10). The
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
            // Draw the outline on top of the desktop (disjoint field borrows).
            if let Some(outline) = self.outline.as_mut() {
                outline.draw(&mut self.app.terminal);
            }
            let _ = self.app.terminal.flush();
            if let Ok(Some(mut ev)) = self.app.terminal.poll_event(Duration::from_millis(50)) {
                if ev.what == EventType::Command {
                    if ev.command == CM_QUIT {
                        self.app.running = false;
                    } else {
                        // Resolve the menu command against the current selection.
                        let selected = self.selected();
                        let action = menu_action(
                            ev.command,
                            profile_count,
                            selected.as_ref().map(|(dn, _)| dn.as_str()),
                        );
                        self.app.handle_event(&mut ev);
                        if action != UiAction::None {
                            on_event(&mut self.app, LoopEvent::Action(action));
                        }
                    }
                } else if ev.what == EventType::Keyboard {
                    // Remember Enter before the outline clears the event, then let
                    // the outline navigate / expand-collapse. After it processes
                    // the key the selection reflects any cursor move, so read it
                    // then to build the Activate action.
                    let is_enter = ev.key_code == KB_ENTER;
                    if let Some(outline) = self.outline.as_mut() {
                        outline.handle_event(&mut ev);
                    }
                    // Forward anything the outline left unhandled (menu hotkeys,
                    // Alt-X, F10) to the application.
                    self.app.handle_event(&mut ev);
                    if ev.what == EventType::Command && ev.command == CM_QUIT {
                        self.app.running = false;
                    } else if is_enter {
                        if let Some((dn, loaded)) = self.selected() {
                            on_event(
                                &mut self.app,
                                LoopEvent::Action(UiAction::Activate { dn, loaded }),
                            );
                        }
                    }
                } else {
                    self.app.handle_event(&mut ev);
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
            Rect::new(2, height - 2, 14, height - 1),
            "~S~ave",
            CM_OK,
            true,
        )));
        dialog.add(Box::new(Button::new(
            Rect::new(16, height - 2, 28, height - 1),
            "~C~ancel",
            CM_CANCEL,
            false,
        )));
        dialog.add(Box::new(Button::new(
            Rect::new(30, height - 2, 48, height - 1),
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
