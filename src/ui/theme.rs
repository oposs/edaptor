//! The single source of truth for eDAPtor's colors. We clone tvision's
//! `classic_blue` and override roles to a light (Solarized Light) palette.
//! No TOML surface — tune here; could be lifted to config later.

use tvision_rs::{Color, Role, Style, Theme};

/// Solarized Light reference colors used across the theme.
const BASE3: Color = Color::Rgb(0xfd, 0xf6, 0xe3); // brightest surface (active pane)
const BASE2: Color = Color::Rgb(0xee, 0xe8, 0xd5); // inactive pane surface
const BASE1: Color = Color::Rgb(0x93, 0xa1, 0xa1); // secondary / frames / disabled
const BASE01: Color = Color::Rgb(0x58, 0x6e, 0x75); // body text
const BLUE: Color = Color::Rgb(0x26, 0x8b, 0xd2); // accent / current item bg
const INPUT_BG: Color = Color::Rgb(0xff, 0xfd, 0xf3); // editable field bg
const DESKTOP: Color = Color::Rgb(0xe3, 0xdd, 0xc8); // desktop behind panes
const SEL_BG: Color = Color::Rgb(0xb5, 0xcd, 0xd8); // multi-selected (staged) bg

/// Build eDAPtor's light theme.
pub(crate) fn edaptor_theme() -> Theme {
    let mut t = Theme::classic_blue();
    // Panel surfaces: kill the cyan ListBox background; everything shares base2/base3.
    t.set_style(Role::Background, Style::new(BASE01, DESKTOP));
    t.set_style(Role::Normal, Style::new(BASE01, BASE2));
    t.set_style(Role::ListNormalInactive, Style::new(BASE01, BASE2));
    t.set_style(Role::ListNormalActive, Style::new(BASE01, BASE3));
    t.set_style(Role::OutlineNormal, Style::new(BASE01, BASE2));
    // Current item: same accent everywhere (list, outline, form).
    t.set_style(Role::ListFocused, Style::new(BASE3, BLUE));
    t.set_style(Role::OutlineFocused, Style::new(BASE3, BLUE));
    t.set_style(Role::Focused, Style::new(BASE3, BLUE));
    // Multi-selected / staged rows.
    t.set_style(Role::ListSelected, Style::new(BASE01, SEL_BG));
    t.set_style(Role::OutlineSelected, Style::new(BASE01, SEL_BG));
    // Editable fields: brightest, signals "type here".
    t.set_style(Role::InputNormal, Style::new(BASE01, INPUT_BG));
    t.set_style(Role::InputSelected, Style::new(BASE3, BLUE));
    // Secondary chrome.
    t.set_style(Role::Disabled, Style::new(BASE1, BASE2));
    t.set_style(Role::ScrollBarPage, Style::new(BASE1, BASE2));
    t.set_style(Role::ScrollBarControls, Style::new(BASE01, BASE2));
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bg(t: &Theme, role: Role) -> Color {
        // Round-trips through the public draw path: classic_blue stores Style by
        // role; we read it back via a fresh DrawCtx-free accessor.
        t.style(role).bg
    }

    #[test]
    fn panels_share_one_background_family() {
        let t = edaptor_theme();
        // The leaf ListBox no longer paints cyan: inactive list bg == base2.
        assert_eq!(bg(&t, Role::ListNormalInactive), BASE2);
        assert_eq!(bg(&t, Role::OutlineNormal), BASE2);
        // Active pane list is the brightest surface.
        assert_eq!(bg(&t, Role::ListNormalActive), BASE3);
    }

    #[test]
    fn current_item_uses_accent_everywhere() {
        let t = edaptor_theme();
        assert_eq!(bg(&t, Role::ListFocused), BLUE);
        assert_eq!(bg(&t, Role::OutlineFocused), BLUE);
        assert_eq!(bg(&t, Role::Focused), BLUE);
    }
}
