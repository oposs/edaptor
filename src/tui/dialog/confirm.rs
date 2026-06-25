//! Save-confirmation dialog: shows the (secret-masked) LDIF preview with Save/Cancel.

use tvision_rs::{ButtonFlags, ButtonRowAlign, Command, Dialog, Rect, StaticText, View};

/// Build the confirm dialog. Returns `Command::OK` (Save) or `Command::CANCEL`.
pub fn build(ldif: &str) -> Box<dyn View> {
    let mut dlg = Dialog::new(Rect::new(0, 0, 70, 20), Some("Confirm save".to_string()));
    dlg.state_mut().options.center_x = true;
    dlg.state_mut().options.center_y = true;
    dlg.insert_child(Box::new(StaticText::new(
        Rect::new(2, 2, 68, 16),
        ldif.to_string(),
    )));
    dlg.button_row(
        &[
            (
                "~S~ave",
                Command::OK,
                ButtonFlags {
                    default: true,
                    ..ButtonFlags::new()
                },
            ),
            ("~C~ancel", Command::CANCEL, ButtonFlags::new()),
        ],
        ButtonRowAlign::Right,
    );
    Box::new(dlg)
}
