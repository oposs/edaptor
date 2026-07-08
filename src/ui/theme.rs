//! The single source of truth for eDAPtor's colors. We clone tvision's
//! `classic_blue` and override roles to a fresh light palette: white content
//! cards on a cool-grey desktop, with the focused pane tinted a soft "post-it"
//! yellow. No TOML surface — tune here; could be lifted to config later.

use tvision_rs::{Color, Role, Style, Theme};

/// Palette. Surfaces run from the grey desktop "table" up through white content
/// cards to the pale-yellow active pane; blue is the accent, red the hotkey.
const INK: Color = Color::Rgb(0x1f, 0x29, 0x33); // body text (crisp near-black slate)
const MUTED: Color = Color::Rgb(0x9a, 0xa5, 0xb1); // secondary / disabled / passive frames / shadow
const CANVAS: Color = Color::Rgb(0xe5, 0xe7, 0xeb); // desktop backdrop, splitter gutters, menu + status chrome
const SURFACE: Color = Color::Rgb(0xff, 0xff, 0xff); // inactive/content panes, dialog surface
const ACTIVE: Color = Color::Rgb(0xfe, 0xf9, 0xc3); // active pane (soft post-it yellow)
const INPUT: Color = Color::Rgb(0xef, 0xf6, 0xff); // focused editable field well (faint blue)
const ACCENT: Color = Color::Rgb(0x25, 0x63, 0xeb); // accent / current item bg (fresh blue)
const STAGED: Color = Color::Rgb(0xbf, 0xdb, 0xfe); // multi-selected (staged) bg (light blue)
const HOTKEY: Color = Color::Rgb(0xdc, 0x26, 0x26); // hotkey highlight accent (red)

/// Build eDAPtor's light theme.
pub(crate) fn edaptor_theme() -> Theme {
    let mut t = Theme::classic_blue();
    // Panel surfaces: kill the cyan ListBox background. The desktop backdrop is
    // white — it is only ever visible as the splitter gutters, whose line glyphs
    // already separate the panes, so a grey "table" tone is not needed there.
    t.set_style(Role::Background, Style::new(INK, SURFACE));
    t.set_style(Role::Normal, Style::new(INK, SURFACE));
    // Active pane = pale post-it yellow; inactive panes stay white (they read as
    // cards on the grey desktop). The "you are here" cue is the yellow tint, not
    // brightness. `ListSurface` (tvision 0.9's three-surface default) is a list
    // that is a NON-focused sibling in an active group — the passive column of the
    // two-list shuttle. Pin it to white so the focused (yellow) list stands out
    // against its passive (white) column. (Single-list panes — tree, leaf — never
    // hit Surface: their sole list is focused iff its pane is.)
    t.set_style(Role::ListInactive, Style::new(INK, SURFACE));
    t.set_style(Role::ListNormal, Style::new(INK, ACTIVE));
    t.set_style(Role::ListSurface, Style::new(INK, SURFACE));
    // Outline (tree) surface is focus-aware: a focused tree tints to the active
    // yellow, an unfocused one stays white. Keep OutlineNotExpanded's background on
    // the active tint so a collapsed branch's dim label matches the focused row
    // fill. `OutlineSurface` (the non-focused-sibling case) is unreachable for
    // edaptor's single tree pane, but wire it to white for palette completeness.
    t.set_style(Role::OutlineNormal, Style::new(INK, ACTIVE));
    t.set_style(Role::OutlineInactive, Style::new(INK, SURFACE));
    t.set_style(Role::OutlineSurface, Style::new(INK, SURFACE));
    t.set_style(Role::OutlineNotExpanded, Style::new(MUTED, ACTIVE));
    t.set_style(Role::ListDivider, Style::new(INK, SURFACE));
    // Current item: same accent everywhere (list, outline, form).
    t.set_style(Role::ListFocused, Style::new(SURFACE, ACCENT));
    t.set_style(Role::OutlineFocused, Style::new(SURFACE, ACCENT));
    t.set_style(Role::Focused, Style::new(SURFACE, ACCENT));
    t.set_style(Role::Pressed, Style::new(SURFACE, ACCENT));
    // Multi-selected / staged rows.
    t.set_style(Role::ListSelected, Style::new(INK, STAGED));
    t.set_style(Role::OutlineSelected, Style::new(INK, STAGED));
    // Editable fields, three surfaces (InputLine self-focus opt-in, tvision 0.8):
    // `InputNormal` (the FOCUSED field, active pane) is the faint-blue "type here"
    // well — it stands out on both the yellow pane and a white dialog. `InputSurface`
    // (a NON-focused field in an active pane) is a plain white well: on the yellow
    // form pane the fields read as clean wells with the focused one tinted blue, and
    // in a white dialog a non-focused input blends in rather than lighting up yellow
    // (which read as "active" precisely when it was not). `InputInactive` (any field
    // in an INACTIVE pane) is likewise white so an unfocused form reads flat — the
    // framework paints these via `owner_active`, so edaptor no longer repaints value
    // cells by hand.
    t.set_style(Role::InputNormal, Style::new(INK, INPUT));
    t.set_style(Role::InputSurface, Style::new(INK, SURFACE));
    t.set_style(Role::InputInactive, Style::new(INK, SURFACE));
    t.set_style(Role::InputSelected, Style::new(SURFACE, ACCENT));
    t.set_style(Role::InputArrow, Style::new(INK, INPUT));
    // Secondary chrome.
    t.set_style(Role::Disabled, Style::new(MUTED, SURFACE));
    t.set_style(Role::ScrollBarPage, Style::new(MUTED, SURFACE));
    t.set_style(Role::ScrollBarControls, Style::new(INK, SURFACE));

    // Dialog buttons: kill the BIOS green. Normal = a raised grey chip (canvas tone
    // on the white dialog); default/selected = white-on-blue accent so the primary
    // action always stands out.
    t.set_style(Role::ButtonNormal, Style::new(INK, CANVAS));
    t.set_style(Role::ButtonDefault, Style::new(SURFACE, ACCENT));
    t.set_style(Role::ButtonSelected, Style::new(SURFACE, ACCENT));
    t.set_style(Role::ButtonDisabled, Style::new(MUTED, CANVAS));
    t.set_style(Role::ButtonNormalShortcut, Style::new(HOTKEY, CANVAS));
    t.set_style(Role::ButtonDefaultShortcut, Style::new(HOTKEY, ACCENT));
    t.set_style(Role::ButtonSelectedShortcut, Style::new(HOTKEY, ACCENT));
    t.set_style(Role::ButtonShadow, Style::new(MUTED, SURFACE));

    // Static text / labels: ink on the white dialog surface. (The form pane's own
    // field-label highlight is handled separately, via the ListNormal/ListFocused
    // roles, so LabelLight has nothing brighter than white to step up to here.)
    t.set_style(Role::StaticText, Style::new(INK, SURFACE));
    t.set_style(Role::LabelNormal, Style::new(INK, SURFACE));
    t.set_style(Role::LabelLight, Style::new(INK, SURFACE));
    t.set_style(Role::LabelNormalShortcut, Style::new(HOTKEY, SURFACE));
    t.set_style(Role::LabelLightShortcut, Style::new(HOTKEY, SURFACE));

    // Cluster items (check boxes / radio buttons): on the white dialog surface.
    t.set_style(Role::ClusterNormal, Style::new(INK, SURFACE));
    t.set_style(Role::ClusterSelected, Style::new(SURFACE, ACCENT));
    t.set_style(Role::ClusterNormalShortcut, Style::new(HOTKEY, SURFACE));
    t.set_style(Role::ClusterSelectedShortcut, Style::new(HOTKEY, ACCENT));
    t.set_style(Role::ClusterDisabled, Style::new(MUTED, SURFACE));

    // Menu bar + status line: ink on the grey canvas so the chrome frames the
    // white/yellow workspace; BLUE accent for the selected/hovered item; HOTKEY red
    // for the shortcut letter throughout.
    t.set_style(Role::MenuNormal, Style::new(INK, CANVAS));
    t.set_style(Role::MenuNormalShortcut, Style::new(HOTKEY, CANVAS));
    t.set_style(Role::MenuSelected, Style::new(SURFACE, ACCENT));
    t.set_style(Role::MenuSelectedShortcut, Style::new(HOTKEY, ACCENT));
    t.set_style(Role::MenuDisabled, Style::new(MUTED, CANVAS));
    t.set_style(Role::MenuSelectedDisabled, Style::new(MUTED, ACCENT));
    t.set_style(Role::StatusNormal, Style::new(INK, CANVAS));
    t.set_style(Role::StatusShortcut, Style::new(HOTKEY, CANVAS));
    t.set_style(Role::StatusSelect, Style::new(SURFACE, ACCENT));
    t.set_style(Role::StatusShortcutSelect, Style::new(HOTKEY, ACCENT));
    t.set_style(Role::StatusDisabled, Style::new(MUTED, CANVAS));
    t.set_style(Role::StatusSelDisabled, Style::new(MUTED, ACCENT));

    // Blue-scheme frame: the three-pane browser's splitter dividers draw their
    // line glyphs with these roles (FrameActive when the splitter is focused —
    // the normal case — FrameDragging while a divider is being moved). Paint a
    // slate line on white so the gutter merges with the surrounding panes and only
    // the divider glyphs separate them; BLUE while dragging for clear resize
    // feedback.
    t.set_style(Role::FrameActive, Style::new(INK, SURFACE));
    t.set_style(Role::FramePassive, Style::new(MUTED, SURFACE));
    t.set_style(Role::FrameDragging, Style::new(ACCENT, SURFACE));
    t.set_style(Role::FrameIcon, Style::new(INK, SURFACE));

    // Gray-scheme frame (dialogs): ink/muted line on the white dialog surface;
    // BLUE while dragging for clear visual feedback. FrameGrayIcon (close/zoom
    // glyphs) gets the same style as the active frame so icons are fully readable.
    t.set_style(Role::FrameGrayActive, Style::new(INK, SURFACE));
    t.set_style(Role::FrameGrayPassive, Style::new(MUTED, SURFACE));
    t.set_style(Role::FrameGrayDragging, Style::new(ACCENT, SURFACE));
    t.set_style(Role::FrameGrayIcon, Style::new(INK, SURFACE));

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
    fn panels_are_white_cards_with_a_yellow_active_pane() {
        let t = edaptor_theme();
        // The leaf ListBox no longer paints cyan; inactive panes are white cards,
        // the active pane tints to the post-it yellow so it is unmistakable.
        assert_eq!(bg(&t, Role::ListInactive), SURFACE);
        // ListSurface = the non-focused sibling list (passive shuttle column):
        // white, distinct from the yellow focused list.
        assert_eq!(bg(&t, Role::ListSurface), SURFACE);
        assert_ne!(bg(&t, Role::ListSurface), bg(&t, Role::ListNormal));
        // Active pane list is the yellow surface.
        assert_eq!(bg(&t, Role::ListNormal), ACTIVE);
        // The outline (tree) surface is focus-aware and mirrors the list: a focused
        // tree tints to yellow, an unfocused one stays white.
        assert_eq!(bg(&t, Role::OutlineNormal), ACTIVE);
        assert_eq!(bg(&t, Role::OutlineInactive), SURFACE);
        // Three input surfaces: the focused field (InputNormal) is the faint-blue
        // "type here" well; a non-focused field in an active pane (InputSurface) is a
        // plain white well (so a dialog's unfocused input blends into the white dialog
        // instead of lighting up yellow); a field in an inactive pane (InputInactive)
        // is white so an unfocused form reads flat.
        assert_eq!(bg(&t, Role::InputNormal), INPUT);
        assert_eq!(bg(&t, Role::InputSurface), SURFACE);
        assert_eq!(bg(&t, Role::InputInactive), SURFACE);
    }

    #[test]
    fn splitter_dividers_are_themed() {
        let t = edaptor_theme();
        // The blue-scheme Frame roles drive the three-pane splitter gutters.
        // They must be wired into the light palette (white background so the
        // gutter merges with the panes and only the divider glyphs separate them),
        // not left on classic_blue's dark defaults.
        assert_eq!(bg(&t, Role::FrameActive), SURFACE);
        assert_eq!(bg(&t, Role::FramePassive), SURFACE);
        assert_eq!(bg(&t, Role::FrameDragging), SURFACE);
        // Active divider line uses dark body-text; dragging flips to the blue accent.
        assert_eq!(fg(&t, Role::FrameActive), INK);
        assert_eq!(fg(&t, Role::FrameDragging), ACCENT);
    }

    #[test]
    fn current_item_uses_accent_everywhere() {
        let t = edaptor_theme();
        assert_eq!(bg(&t, Role::ListFocused), ACCENT);
        assert_eq!(bg(&t, Role::OutlineFocused), ACCENT);
        assert_eq!(bg(&t, Role::Focused), ACCENT);
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
        assert_eq!(bg(&t, Role::ButtonDefault), ACCENT);
        assert_eq!(bg(&t, Role::ButtonSelected), ACCENT);
        // Normal (non-default) button sits on the raised grey chip, not the accent.
        assert_eq!(bg(&t, Role::ButtonNormal), CANVAS);
    }

    #[test]
    fn static_text_and_labels_on_dialog_surface() {
        let t = edaptor_theme();
        assert_eq!(bg(&t, Role::StaticText), SURFACE);
        assert_eq!(bg(&t, Role::LabelNormal), SURFACE);
        assert_eq!(bg(&t, Role::LabelLight), SURFACE);
    }

    #[test]
    fn menu_and_status_on_canvas_with_blue_selected() {
        let t = edaptor_theme();
        assert_eq!(bg(&t, Role::MenuNormal), CANVAS);
        assert_eq!(bg(&t, Role::MenuSelected), ACCENT);
        assert_eq!(bg(&t, Role::StatusNormal), CANVAS);
        assert_eq!(bg(&t, Role::StatusSelect), ACCENT);
    }

    #[test]
    fn hotkey_accent_is_red_throughout() {
        let t = edaptor_theme();
        assert_eq!(fg(&t, Role::ButtonNormalShortcut), HOTKEY);
        assert_eq!(fg(&t, Role::ButtonDefaultShortcut), HOTKEY);
        assert_eq!(fg(&t, Role::LabelNormalShortcut), HOTKEY);
        assert_eq!(fg(&t, Role::MenuNormalShortcut), HOTKEY);
        assert_eq!(fg(&t, Role::StatusShortcut), HOTKEY);
    }

    #[test]
    fn gray_dialog_frame_on_white_surface() {
        let t = edaptor_theme();
        assert_eq!(bg(&t, Role::FrameGrayActive), SURFACE);
        assert_eq!(bg(&t, Role::FrameGrayPassive), SURFACE);
        // Active frame uses dark body-text for visible border characters.
        assert_eq!(fg(&t, Role::FrameGrayActive), INK);
        // Passive frame uses the secondary/muted color.
        assert_eq!(fg(&t, Role::FrameGrayPassive), MUTED);
    }
}
