//! The concurrent-modification dialog: shown when a save is refused because the
//! entry changed on the server since it was read (rc 122) and the change overlaps
//! the attributes we are writing. Offers Reload (discard our edit and re-read),
//! Overwrite (re-assert against the new version, keeping our values), or Cancel.

use tvision_rs::{ButtonFlags, ButtonRowAlign, Command, Dialog, Rect, StaticText, View, ViewId};

/// Custom command returned when the operator chooses to overwrite (re-apply our
/// edit on top of the other client's version).
pub const OVERWRITE: Command = Command::custom("edaptor.conflict.overwrite");

/// Build the conflict dialog. Returns the view and the button id to focus (Reload,
/// the safe default). Reload → `Command::CANCEL`; Overwrite → [`OVERWRITE`]; Cancel
/// → `Command::custom("edaptor.conflict.keep")` (any non-matching answer = keep
/// editing).
pub fn build(text: &str) -> (Box<dyn View>, ViewId) {
    let mut dlg = Dialog::new(Rect::new(0, 0, 64, 13), Some("Entry changed".to_string()));
    dlg.state_mut().options.center_x = true;
    dlg.state_mut().options.center_y = true;
    dlg.insert_child(Box::new(StaticText::new(
        Rect::new(2, 2, 62, 9),
        text.to_string(),
    )));
    let ids = dlg.button_row(
        &[
            (
                "~R~eload",
                Command::CANCEL,
                ButtonFlags {
                    default: true,
                    ..ButtonFlags::new()
                },
            ),
            ("~O~verwrite", OVERWRITE, ButtonFlags::new()),
            (
                "~C~ancel",
                Command::custom("edaptor.conflict.keep"),
                ButtonFlags::new(),
            ),
        ],
        ButtonRowAlign::Center,
    );
    (Box::new(dlg), ids[0])
}
