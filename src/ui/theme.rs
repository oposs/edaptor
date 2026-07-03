//! The single source of truth for eDAPtor's colors. We clone tvision's
//! `classic_blue` and override roles to a light (Solarized Light) palette.
//! No TOML surface — tune here; could be lifted to config later.

use tvision_rs::{Color, Role, Style, Theme};

/// Solarized Light reference colors used across the theme.
const BASE3: Color = Color::Rgb(0xfd, 0xf6, 0xe3); // brightest surface (active pane)
const BASE2: Color = Color::Rgb(0xee, 0xe8, 0xd5); // inactive pane surface / dialog bg
const BASE1: Color = Color::Rgb(0x93, 0xa1, 0xa1); // secondary / frames / disabled
const BASE01: Color = Color::Rgb(0x58, 0x6e, 0x75); // body text
const BLUE: Color = Color::Rgb(0x26, 0x8b, 0xd2); // accent / current item bg
const INPUT_BG: Color = Color::Rgb(0xff, 0xfd, 0xf3); // editable field bg
const DESKTOP: Color = Color::Rgb(0xe3, 0xdd, 0xc8); // desktop behind panes
const SEL_BG: Color = Color::Rgb(0xb5, 0xcd, 0xd8); // multi-selected (staged) bg
const ACCENT_RED: Color = Color::Rgb(0xdc, 0x32, 0x2f); // Solarized red — hotkey highlight accent

/// Build eDAPtor's light theme.
pub(crate) fn edaptor_theme() -> Theme {
    let mut t = Theme::classic_blue();
    // Panel surfaces: kill the cyan ListBox background; everything shares base2/base3.
    t.set_style(Role::Background, Style::new(BASE01, DESKTOP));
    t.set_style(Role::Normal, Style::new(BASE01, BASE2));
    // Active pane = brightest parchment (base3); inactive panes recede to the
    // desktop tone so the focused pane is unmistakable (base2 vs base3 was one
    // Solarized step apart — invisible on most terminals).
    t.set_style(Role::ListInactive, Style::new(BASE01, DESKTOP));
    t.set_style(Role::ListNormal, Style::new(BASE01, BASE3));
    // Outline (tree) surface is now focus-aware (tvision-rs 0.5): a focused tree
    // brightens to base3 like the active list, an unfocused one recedes to the
    // desktop tone. Keep OutlineNotExpanded's background on base3 so a collapsed
    // branch's dim label matches the focused row fill.
    t.set_style(Role::OutlineNormal, Style::new(BASE01, BASE3));
    t.set_style(Role::OutlineInactive, Style::new(BASE01, DESKTOP));
    t.set_style(Role::OutlineNotExpanded, Style::new(BASE1, BASE3));
    t.set_style(Role::ListDivider, Style::new(BASE01, BASE2));
    // Current item: same accent everywhere (list, outline, form).
    t.set_style(Role::ListFocused, Style::new(BASE3, BLUE));
    t.set_style(Role::OutlineFocused, Style::new(BASE3, BLUE));
    t.set_style(Role::Focused, Style::new(BASE3, BLUE));
    t.set_style(Role::Pressed, Style::new(BASE3, BLUE));
    // Multi-selected / staged rows.
    t.set_style(Role::ListSelected, Style::new(BASE01, SEL_BG));
    t.set_style(Role::OutlineSelected, Style::new(BASE01, SEL_BG));
    // Editable fields. `InputNormal` (the FOCUSED field) is the near-white "type
    // here" well that marks where input goes. `InputInactive` (every UNFOCUSED
    // field) sits on the bright pane surface (base3) so non-selected fields carry
    // no special background — they read as plain text against the pane, not as
    // recessed wells — while the one focused field stands out as the input well.
    t.set_style(Role::InputNormal, Style::new(BASE01, INPUT_BG));
    t.set_style(Role::InputInactive, Style::new(BASE01, BASE3));
    t.set_style(Role::InputSelected, Style::new(BASE3, BLUE));
    t.set_style(Role::InputArrow, Style::new(BASE01, INPUT_BG));
    // Secondary chrome.
    t.set_style(Role::Disabled, Style::new(BASE1, BASE2));
    t.set_style(Role::ScrollBarPage, Style::new(BASE1, BASE2));
    t.set_style(Role::ScrollBarControls, Style::new(BASE01, BASE2));

    // Dialog buttons: kill the BIOS green; normal = slate-on-parchment (slightly
    // raised vs the cream dialog); default/selected = cream-on-blue accent so the
    // primary action always stands out.
    t.set_style(Role::ButtonNormal, Style::new(BASE01, BASE2));
    t.set_style(Role::ButtonDefault, Style::new(BASE3, BLUE));
    t.set_style(Role::ButtonSelected, Style::new(BASE3, BLUE));
    t.set_style(Role::ButtonDisabled, Style::new(BASE1, BASE2));
    t.set_style(Role::ButtonNormalShortcut, Style::new(ACCENT_RED, BASE2));
    t.set_style(Role::ButtonDefaultShortcut, Style::new(ACCENT_RED, BLUE));
    t.set_style(Role::ButtonSelectedShortcut, Style::new(ACCENT_RED, BLUE));
    t.set_style(Role::ButtonShadow, Style::new(BASE1, BASE2));

    // Static text / labels: slate text on the dialog surface (BASE2). LabelLight
    // uses the slightly brighter BASE3 to signal that its linked control is focused.
    t.set_style(Role::StaticText, Style::new(BASE01, BASE2));
    t.set_style(Role::LabelNormal, Style::new(BASE01, BASE2));
    t.set_style(Role::LabelLight, Style::new(BASE01, BASE3));
    t.set_style(Role::LabelNormalShortcut, Style::new(ACCENT_RED, BASE2));
    t.set_style(Role::LabelLightShortcut, Style::new(ACCENT_RED, BASE3));

    // Cluster items (check boxes / radio buttons): on the dialog surface.
    t.set_style(Role::ClusterNormal, Style::new(BASE01, BASE2));
    t.set_style(Role::ClusterSelected, Style::new(BASE3, BLUE));
    t.set_style(Role::ClusterNormalShortcut, Style::new(ACCENT_RED, BASE2));
    t.set_style(Role::ClusterSelectedShortcut, Style::new(ACCENT_RED, BLUE));
    t.set_style(Role::ClusterDisabled, Style::new(BASE1, BASE2));

    // Menu bar + status line: dark slate on parchment; BLUE accent for the
    // selected/hovered item; ACCENT_RED for the hotkey letter throughout.
    t.set_style(Role::MenuNormal, Style::new(BASE01, BASE2));
    t.set_style(Role::MenuNormalShortcut, Style::new(ACCENT_RED, BASE2));
    t.set_style(Role::MenuSelected, Style::new(BASE3, BLUE));
    t.set_style(Role::MenuSelectedShortcut, Style::new(ACCENT_RED, BLUE));
    t.set_style(Role::MenuDisabled, Style::new(BASE1, BASE2));
    t.set_style(Role::MenuSelectedDisabled, Style::new(BASE1, BLUE));
    t.set_style(Role::StatusNormal, Style::new(BASE01, BASE2));
    t.set_style(Role::StatusShortcut, Style::new(ACCENT_RED, BASE2));
    t.set_style(Role::StatusSelect, Style::new(BASE3, BLUE));
    t.set_style(Role::StatusShortcutSelect, Style::new(ACCENT_RED, BLUE));
    t.set_style(Role::StatusDisabled, Style::new(BASE1, BASE2));
    t.set_style(Role::StatusSelDisabled, Style::new(BASE1, BLUE));

    // Blue-scheme frame: the three-pane browser's splitter dividers draw their
    // line glyphs with these roles (FrameActive when the splitter is focused —
    // the normal case — FrameDragging while a divider is being moved). Untheming
    // them left the gutters on classic_blue's dark defaults. Paint a slate line
    // on the desktop tone so the gutter reads as background space; BLUE while
    // dragging for clear resize feedback.
    t.set_style(Role::FrameActive, Style::new(BASE01, DESKTOP));
    t.set_style(Role::FramePassive, Style::new(BASE1, DESKTOP));
    t.set_style(Role::FrameDragging, Style::new(BLUE, DESKTOP));
    t.set_style(Role::FrameIcon, Style::new(BASE01, DESKTOP));

    // Gray-scheme frame (dialogs): slate/muted-slate on parchment; BLUE while
    // dragging for clear visual feedback. FrameGrayIcon (close/zoom glyphs) gets
    // the same style as the active frame so icons are fully readable.
    t.set_style(Role::FrameGrayActive, Style::new(BASE01, BASE2));
    t.set_style(Role::FrameGrayPassive, Style::new(BASE1, BASE2));
    t.set_style(Role::FrameGrayDragging, Style::new(BLUE, BASE2));
    t.set_style(Role::FrameGrayIcon, Style::new(BASE01, BASE2));

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

    fn fg(t: &Theme, role: Role) -> Color {
        t.style(role).fg
    }

    #[test]
    fn panels_share_one_background_family() {
        let t = edaptor_theme();
        // The leaf ListBox no longer paints cyan; inactive panes recede to the
        // desktop tone so the active pane (base3) is clearly distinguishable.
        assert_eq!(bg(&t, Role::ListInactive), DESKTOP);
        // Active pane list is the brightest surface.
        assert_eq!(bg(&t, Role::ListNormal), BASE3);
        // The outline (tree) surface is focus-aware and mirrors the list: a
        // focused tree brightens to base3, an unfocused one recedes to the
        // desktop tone. InputInactive is pinned to the input surface (never the
        // classic_blue blue default) so unfocused form fields stay on parchment.
        assert_eq!(bg(&t, Role::OutlineNormal), BASE3);
        assert_eq!(bg(&t, Role::OutlineInactive), DESKTOP);
        // The focused field (InputNormal) is the bright "type here" well; every
        // unfocused field (InputInactive) sits on the pane surface (base3) so
        // non-selected fields carry no special background — plain text on the pane,
        // not recessed wells — while only the focused field reads as an input well.
        assert_eq!(bg(&t, Role::InputNormal), INPUT_BG);
        assert_eq!(bg(&t, Role::InputInactive), BASE3);
    }

    #[test]
    fn splitter_dividers_are_themed() {
        let t = edaptor_theme();
        // The blue-scheme Frame roles drive the three-pane splitter gutters.
        // They must be wired into the light palette (desktop-tone background),
        // not left on classic_blue's dark defaults.
        assert_eq!(bg(&t, Role::FrameActive), DESKTOP);
        assert_eq!(bg(&t, Role::FramePassive), DESKTOP);
        assert_eq!(bg(&t, Role::FrameDragging), DESKTOP);
        // Active divider line uses dark body-text; dragging flips to the blue accent.
        assert_eq!(fg(&t, Role::FrameActive), BASE01);
        assert_eq!(fg(&t, Role::FrameDragging), BLUE);
    }

    #[test]
    fn current_item_uses_accent_everywhere() {
        let t = edaptor_theme();
        assert_eq!(bg(&t, Role::ListFocused), BLUE);
        assert_eq!(bg(&t, Role::OutlineFocused), BLUE);
        assert_eq!(bg(&t, Role::Focused), BLUE);
    }

    #[test]
    fn buttons_are_not_bios_green() {
        let t = edaptor_theme();
        // In classic_blue all three active button states have green as the bg (BIOS 0x2).
        // The light theme must not carry that green through.
        assert_ne!(bg(&t, Role::ButtonNormal), Color::bios_rgb(0x2));
        assert_ne!(bg(&t, Role::ButtonDefault), Color::bios_rgb(0x2));
        assert_ne!(bg(&t, Role::ButtonSelected), Color::bios_rgb(0x2));
    }

    #[test]
    fn default_button_uses_blue_accent() {
        let t = edaptor_theme();
        assert_eq!(bg(&t, Role::ButtonDefault), BLUE);
        assert_eq!(bg(&t, Role::ButtonSelected), BLUE);
        // Normal (non-default) button sits on parchment, not the accent.
        assert_eq!(bg(&t, Role::ButtonNormal), BASE2);
    }

    #[test]
    fn static_text_and_labels_on_dialog_surface() {
        let t = edaptor_theme();
        assert_eq!(bg(&t, Role::StaticText), BASE2);
        assert_eq!(bg(&t, Role::LabelNormal), BASE2);
        // LabelLight (linked control focused) steps up to the brighter surface.
        assert_eq!(bg(&t, Role::LabelLight), BASE3);
    }

    #[test]
    fn menu_and_status_on_parchment_with_blue_selected() {
        let t = edaptor_theme();
        assert_eq!(bg(&t, Role::MenuNormal), BASE2);
        assert_eq!(bg(&t, Role::MenuSelected), BLUE);
        assert_eq!(bg(&t, Role::StatusNormal), BASE2);
        assert_eq!(bg(&t, Role::StatusSelect), BLUE);
    }

    #[test]
    fn hotkey_accent_is_red_throughout() {
        let t = edaptor_theme();
        assert_eq!(fg(&t, Role::ButtonNormalShortcut), ACCENT_RED);
        assert_eq!(fg(&t, Role::ButtonDefaultShortcut), ACCENT_RED);
        assert_eq!(fg(&t, Role::LabelNormalShortcut), ACCENT_RED);
        assert_eq!(fg(&t, Role::MenuNormalShortcut), ACCENT_RED);
        assert_eq!(fg(&t, Role::StatusShortcut), ACCENT_RED);
    }

    #[test]
    fn gray_dialog_frame_on_parchment() {
        let t = edaptor_theme();
        assert_eq!(bg(&t, Role::FrameGrayActive), BASE2);
        assert_eq!(bg(&t, Role::FrameGrayPassive), BASE2);
        // Active frame uses dark body-text for visible border characters.
        assert_eq!(fg(&t, Role::FrameGrayActive), BASE01);
        // Passive frame uses the secondary/muted color.
        assert_eq!(fg(&t, Role::FrameGrayPassive), BASE1);
    }
}
