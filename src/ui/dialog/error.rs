//! Dismissible error dialog.

use tvision_rs::{ButtonFlags, ButtonRowAlign, Command, Dialog, Rect, StaticText, View, ViewId};

/// Widest text column the dialog lays out at.
const MAX_TEXT_W: i32 = 68;
/// Narrowest text column, so a one-word error still looks like a dialog.
const MIN_TEXT_W: i32 = 34;
/// Most text rows shown; a longer message is clipped rather than run off-screen.
const MAX_TEXT_H: i32 = 18;
/// Columns the frame + margins take on top of the text column.
const CHROME_W: i32 = 4;
/// Rows the title, margins and button row take on top of the text rows.
const CHROME_H: i32 = 5;

/// Break `text` into lines of at most `width` columns.
///
/// Honours explicit `\n`, wraps on spaces, and hard-breaks a token longer than
/// `width` (a DN has no spaces to wrap at, and an unbroken DN would otherwise
/// run past the frame). Pre-wrapping here — rather than leaving it to
/// [`StaticText`] — is what lets [`build`] size the dialog to the message.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut line = String::new();
        for word in para.split(' ') {
            let mut word = word;
            // A token wider than the whole column: chop it into full lines.
            while word.chars().count() > width {
                if !line.is_empty() {
                    out.push(std::mem::take(&mut line));
                }
                let head: String = word.chars().take(width).collect();
                let consumed = head.len();
                out.push(head);
                word = &word[consumed..];
            }
            let need = if line.is_empty() {
                word.chars().count()
            } else {
                line.chars().count() + 1 + word.chars().count()
            };
            if need > width && !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        out.push(line);
    }
    out
}

/// Build the error dialog. Returns the view and the `OK` button id to focus on
/// open (so Enter dismisses). The dialog returns `Command::OK` on dismiss.
///
/// The dialog sizes itself to the message: write-failure messages name the DN
/// and the server's reason, which does not fit a fixed 60x12 box.
pub fn build(text: &str) -> (Box<dyn View>, ViewId) {
    let lines = wrap(text, MAX_TEXT_W as usize);
    let text_w = lines
        .iter()
        .map(|l| l.chars().count() as i32)
        .max()
        .unwrap_or(0)
        .clamp(MIN_TEXT_W, MAX_TEXT_W);
    let text_h = (lines.len() as i32).clamp(1, MAX_TEXT_H);
    let body = lines
        .into_iter()
        .take(text_h as usize)
        .collect::<Vec<_>>()
        .join("\n");

    let mut dlg = Dialog::new(
        Rect::new(0, 0, text_w + CHROME_W, text_h + CHROME_H),
        Some("Error".to_string()),
    );
    dlg.state_mut().options.center_x = true;
    dlg.state_mut().options.center_y = true;
    dlg.insert_child(Box::new(StaticText::new(
        Rect::new(2, 2, 2 + text_w, 2 + text_h),
        body,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_on_spaces() {
        assert_eq!(
            wrap("the quick brown fox jumps", 10),
            vec!["the quick", "brown fox", "jumps"]
        );
    }

    #[test]
    fn honours_explicit_newlines() {
        assert_eq!(wrap("a\n\nb", 10), vec!["a", "", "b"]);
    }

    /// A DN has no spaces: it must be chopped, not allowed to overflow.
    #[test]
    fn hard_breaks_a_token_wider_than_the_column() {
        let dn = "cn=cedric,ou=users,ou=groups,dc=carbo-link,dc=com";
        let lines = wrap(dn, 20);
        assert!(
            lines.iter().all(|l| l.chars().count() <= 20),
            "no line may exceed the column: {lines:?}"
        );
        assert_eq!(lines.concat(), dn, "chopping must not lose characters");
    }

    #[test]
    fn no_line_exceeds_the_column_for_a_real_write_failure() {
        let msg = "Constraint violation (LDAP 19)\n\nThe server rejected the whole \
                   change and wrote nothing.\n\nWhile adding:\n  \
                   cn=cedric,ou=users,ou=groups,dc=carbo-link,dc=com\n\nReason: \
                   non-unique attributes found with (|(gidNumber=1211))";
        let lines = wrap(msg, MAX_TEXT_W as usize);
        assert!(lines
            .iter()
            .all(|l| l.chars().count() <= MAX_TEXT_W as usize));
    }
}
