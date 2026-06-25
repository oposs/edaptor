//! Dirty-guard dialog: Save / Discard / Stay over an unsaved form.

use tvision_rs::{ButtonFlags, ButtonRowAlign, Command, Dialog, Rect, StaticText, View};

/// Build the guard dialog. Returns `Command::YES` (Save), `Command::NO` (Discard),
/// or `Command::CANCEL` (Stay).
pub fn build() -> Box<dyn View> {
    let mut dlg = Dialog::new(Rect::new(0, 0, 56, 9), Some("Unsaved changes".to_string()));
    dlg.state_mut().options.center_x = true;
    dlg.state_mut().options.center_y = true;
    dlg.insert_child(Box::new(StaticText::new(
        Rect::new(2, 2, 54, 4),
        "This entry has unsaved changes.".to_string(),
    )));
    dlg.button_row(
        &[
            (
                "~S~ave",
                Command::YES,
                ButtonFlags {
                    default: true,
                    ..ButtonFlags::new()
                },
            ),
            ("~D~iscard", Command::NO, ButtonFlags::new()),
            ("S~t~ay", Command::CANCEL, ButtonFlags::new()),
        ],
        ButtonRowAlign::Right,
    );
    Box::new(dlg)
}
