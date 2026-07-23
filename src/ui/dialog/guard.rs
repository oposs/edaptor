//! Dirty-guard dialog: Save / Discard / Stay over an unsaved form.

use tvision_rs::dialog::{ButtonLayout, ButtonRowAlign};
use tvision_rs::{ButtonFlags, Command, Dialog, Rect, StaticText, View, ViewId};

/// Dialog canvas size (must match the `Dialog::new` rect below).
const DLG_W: i32 = 64;
const DLG_H: i32 = 9;

/// Build the guard dialog. Returns the view and the `Save` button id to focus on
/// open (so Enter saves, not the firstMatch-focused Stay). The dialog returns
/// `Command::YES` (Save), `Command::NO` (Discard), or `Command::CANCEL` (Stay).
pub fn build() -> (Box<dyn View>, ViewId) {
    let mut dlg = Dialog::new(
        Rect::new(0, 0, DLG_W, DLG_H),
        Some("Unsaved changes".to_string()),
    );
    dlg.state_mut().options.center_x = true;
    dlg.state_mut().options.center_y = true;
    dlg.insert_child(Box::new(StaticText::new(
        Rect::new(2, 2, 62, 4),
        "This entry has unsaved changes.".to_string(),
    )));

    // "Discard" (7 glyphs) overruns the classic 10-column face and clips against
    // the drop shadow; Uniform widens every face to fit the longest label.
    dlg.set_button_layout(ButtonLayout::Uniform);
    let ids = dlg.button_row(
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
    (Box::new(dlg), ids[0])
}
