//! Dirty-guard dialog: Save / Discard / Stay over an unsaved form.

use tvision_rs::{Button, ButtonFlags, Command, Dialog, Rect, StaticText, View, ViewId};

/// Dialog canvas size (must match the `Dialog::new` rect below).
const DLG_W: i32 = 64;
const DLG_H: i32 = 9;
/// Button face width. Wider than the standard 10-column button: `Dialog::button_row`
/// forces every button to 10 columns, but "Discard" (7 glyphs) overruns that face
/// and clips against the drop shadow, leaving no space after the text. An 11-column
/// face keeps a padding column after every label.
const BTN_W: i32 = 11;
const BTN_H: i32 = 2;
/// Cells between adjacent buttons / from the right frame (classic TV metrics).
const BTN_GAP: i32 = 2;
const MARGIN_RIGHT: i32 = 2;
/// Button-row top edge = `dialog_height - BUTTON_ROW_FROM_BOTTOM`.
const BUTTON_ROW_FROM_BOTTOM: i32 = 3;

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

    // Lay the button row out by hand (rather than `Dialog::button_row`) so each
    // button gets the wider `BTN_W` face — see the constant's note on "Discard".
    let specs: [(&str, Command, bool); 3] = [
        ("~S~ave", Command::YES, true),
        ("~D~iscard", Command::NO, false),
        ("S~t~ay", Command::CANCEL, false),
    ];
    let n = specs.len() as i32;
    let span = n * BTN_W + (n - 1) * BTN_GAP;
    let mut x = DLG_W - MARGIN_RIGHT - span;
    let top = DLG_H - BUTTON_ROW_FROM_BOTTOM;
    let mut ids = Vec::with_capacity(specs.len());
    for (title, command, default) in specs {
        let flags = ButtonFlags {
            default,
            ..ButtonFlags::new()
        };
        let b = Button::new(
            Rect::new(x, top, x + BTN_W, top + BTN_H),
            title,
            command,
            flags,
        );
        ids.push(dlg.insert_child(Box::new(b)));
        x += BTN_W + BTN_GAP;
    }
    (Box::new(dlg), ids[0])
}
