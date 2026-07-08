//! Dismissible error dialog.

use tvision_rs::{ButtonFlags, ButtonRowAlign, Command, Dialog, Rect, StaticText, View, ViewId};

/// Build the error dialog. Returns the view and the `OK` button id to focus on
/// open (so Enter dismisses). The dialog returns `Command::OK` on dismiss.
pub fn build(text: &str) -> (Box<dyn View>, ViewId) {
    let mut dlg = Dialog::new(Rect::new(0, 0, 60, 12), Some("Error".to_string()));
    dlg.state_mut().options.center_x = true;
    dlg.state_mut().options.center_y = true;
    dlg.insert_child(Box::new(StaticText::new(
        Rect::new(2, 2, 58, 9),
        text.to_string(),
    )));
    let ids = dlg.button_row(
        &[(
            "~O~K",
            Command::OK,
            ButtonFlags {
                default: true,
                ..ButtonFlags::new()
            },
        )],
        ButtonRowAlign::Center,
    );
    (Box::new(dlg), ids[0])
}
