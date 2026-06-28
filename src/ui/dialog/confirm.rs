//! Save-confirmation dialog: shows the (secret-masked) LDIF preview with Save/Cancel.

use tvision_rs::{ButtonFlags, ButtonRowAlign, Command, Dialog, Rect, StaticText, View, ViewId};

/// Minimum dialog height (enough for the frame + buttons + a few text rows).
const MIN_DIALOG_H: i32 = 8;
/// Maximum dialog height — stays within typical terminal heights (50+ rows)
/// while leaving a couple of rows for centering margins.
const MAX_DIALOG_H: i32 = 44;
/// Rows consumed by dialog chrome: 2 top (border + blank) + 4 bottom
/// (blank + buttons + blank + border).
const CHROME_H: i32 = 6;

/// Compute the dialog height needed for `ldif`, capped at `MAX_DIALOG_H`.
///
/// The StaticText area height = `dialog_height - 4` (border+blank top, blank+border
/// bottom around the button row).
pub(crate) fn dialog_height_for(ldif: &str) -> i32 {
    let lines = ldif.lines().count() as i32;
    (lines + CHROME_H).clamp(MIN_DIALOG_H, MAX_DIALOG_H)
}

/// Build the confirm dialog. Returns the view and the `Save` button id to focus on
/// open (so Enter confirms, not the firstMatch-focused Cancel). The dialog returns
/// `Command::OK` (Save) or `Command::CANCEL`.
///
/// The dialog height is sized to the content so that multi-stanza combined LDIF
/// previews (own entry + group stanzas) are fully visible up to `MAX_DIALOG_H` rows.
/// Single-entry saves render identically to before for typical LDIF sizes.
pub fn build(ldif: &str) -> (Box<dyn View>, ViewId) {
    let h: i32 = dialog_height_for(ldif);
    let text_end_y = h - 4; // 4 = 1 blank + 1 button row + 1 blank + 1 border
    let mut dlg = Dialog::new(Rect::new(0, 0, 70, h), Some("Confirm save".to_string()));
    dlg.state_mut().options.center_x = true;
    dlg.state_mut().options.center_y = true;
    dlg.insert_child(Box::new(StaticText::new(
        Rect::new(2, 2, 68, text_end_y),
        ldif.to_string(),
    )));
    let ids = dlg.button_row(
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
    (Box::new(dlg), ids[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_ldif_clamps_to_min_height() {
        // An empty or very short LDIF must not produce a tiny unusable dialog.
        assert_eq!(dialog_height_for(""), MIN_DIALOG_H);
        assert_eq!(
            dialog_height_for("dn: uid=x,dc=example,dc=com"),
            MIN_DIALOG_H
        );
    }

    #[test]
    fn typical_single_entry_ldif_fits() {
        // A 14-line single-entry LDIF → height 14 + 6 = 20 (same as the old fixed value).
        let ldif = (0..14)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(dialog_height_for(&ldif), 20);
    }

    #[test]
    fn combined_ldif_grows_beyond_old_fixed_height() {
        // A 30-line combined LDIF (user entry + a few group stanzas) → height 30 + 6 = 36.
        let ldif = (0..30)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        // 36 > 20: taller than the old fixed dialog height.
        assert_eq!(dialog_height_for(&ldif), 36);
    }

    #[test]
    fn very_long_ldif_caps_at_max_height() {
        // A 100-line LDIF must be capped, not overflow the screen.
        let ldif = (0..100)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(dialog_height_for(&ldif), MAX_DIALOG_H);
    }

    #[test]
    fn more_lines_produce_taller_dialog() {
        let short = (0..5)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let long = (0..25)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            dialog_height_for(&long) > dialog_height_for(&short),
            "longer LDIF must produce a taller dialog"
        );
    }
}
