//! The entry being edited vanished from the directory: Re-create / Discard /
//! Keep editing over a form whose DN no longer exists.

use tvision_rs::dialog::{ButtonLayout, ButtonRowAlign};
use tvision_rs::{ButtonFlags, Command, Dialog, Rect, StaticText, View, ViewId};

const DLG_W: i32 = 64;
const DLG_H: i32 = 9;

/// Build the vanished-entry dialog. Returns the view and the "Keep editing"
/// button id to focus on open, so Enter takes the only non-destructive choice.
/// The dialog returns `Command::YES` (Re-create), `Command::NO` (Discard), or
/// `Command::CANCEL` (Keep editing).
pub fn build(dn: &str) -> (Box<dyn View>, ViewId) {
    let mut dlg = Dialog::new(
        Rect::new(0, 0, DLG_W, DLG_H),
        Some("Entry removed".to_string()),
    );
    dlg.state_mut().options.center_x = true;
    dlg.state_mut().options.center_y = true;
    // Long labels ("Keep editing" is 12 cols) need faces wider than the classic
    // 10, or they render against the drop shadow. Uniform sizes them to fit.
    dlg.set_button_layout(ButtonLayout::Uniform);
    dlg.insert_child(Box::new(StaticText::new(
        Rect::new(2, 2, 62, 5),
        format!("{dn}\nis no longer in the directory. You have unsaved changes."),
    )));
    let ids = dlg.button_row(
        &[
            ("~R~e-create", Command::YES, ButtonFlags::new()),
            ("~D~iscard", Command::NO, ButtonFlags::new()),
            (
                "~K~eep editing",
                Command::CANCEL,
                ButtonFlags {
                    default: true,
                    ..ButtonFlags::new()
                },
            ),
        ],
        ButtonRowAlign::Right,
    );
    (Box::new(dlg), ids[2])
}
