//! Single-row, non-interactive text cells for the entry form: the DN header
//! (a `dn` label + the DN value styled as a title) and the per-field labels.
//!
//! The form's *values* are real `InputLine`s, but its title and labels are not —
//! a disabled `InputLine` always paints the input surface and left-aligns. A
//! `FieldLabel` instead paints the *pane* surface (so it brightens/dims with
//! focus like the tree and leaf panes), right-aligns field labels in a column
//! sized to the longest name, renders labels a shade lighter than the values,
//! and gives the *selected* field's label the blue current-row highlight — the
//! same treatment the tree and leaf panes give their current row.
//!
//! Colours come from theme roles (never hard-coded) so the widget tracks the
//! palette: the surface is `ListNormal{Active,Inactive}`, the selected-row chip is
//! `ListFocused` / `ListSelected`, the lighter label text is `Disabled`'s
//! foreground, and the value/title text is body text (`ListNormal` fg).

use tvision_rs::{DrawCtx, FieldValue, Rect, Role, View, ViewState};
use unicode_width::UnicodeWidthStr;

/// Columns of empty space kept to the right of a right-aligned field label, so
/// the label never butts directly against its value editor.
const LABEL_GAP: i32 = 1;

/// Which flavour of cell this is — decides alignment and emphasis.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum LabelKind {
    /// The DN value in the header: left-aligned in the value column, bold.
    Title,
    /// A field label (or the header's `dn` caption): right-aligned, a shade
    /// lighter than the value text, and highlighted when its field is active.
    Label,
}

/// A read-only, single-row label cell. Not selectable, so it never takes focus
/// or a Tab stop; the owning [`FormPane`](super::form::FormPane) drives its text
/// via [`View::set_value`] and its `active` flag. Focus (bright vs. dim) comes
/// from the framework at draw time via [`DrawCtx::owner_active`] — the owning
/// pane group's focus, fanned by `Group::draw` — so the pane need not push it.
pub(crate) struct FieldLabel {
    pub state: ViewState,
    text: String,
    kind: LabelKind,
    /// Whether this label's field is the current/selected one (highlight chip).
    active: bool,
}

impl FieldLabel {
    fn new(bounds: Rect, kind: LabelKind) -> Self {
        FieldLabel {
            state: ViewState::new(bounds),
            text: String::new(),
            kind,
            active: false,
        }
    }

    /// A left-aligned, bold DN-value title cell.
    pub(crate) fn title(bounds: Rect) -> Self {
        Self::new(bounds, LabelKind::Title)
    }

    /// A right-aligned label cell.
    pub(crate) fn label(bounds: Rect) -> Self {
        Self::new(bounds, LabelKind::Label)
    }

    /// Mark this label's field as the active (selected) one, so it is highlighted.
    pub(crate) fn set_active(&mut self, active: bool) {
        self.active = active;
    }
}

impl View for FieldLabel {
    fn state(&self) -> &ViewState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut ViewState {
        &mut self.state
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    /// The owning pane pushes label/title text through `set_value(Text)`,
    /// mirroring the `InputLine` value cells it sits beside.
    fn set_value(&mut self, v: FieldValue) {
        if let FieldValue::Text(s) = v {
            self.text = s;
        }
    }

    fn value(&self) -> Option<FieldValue> {
        Some(FieldValue::Text(self.text.clone()))
    }

    fn draw(&mut self, ctx: &mut DrawCtx) {
        let size = self.state.size;

        // The selected field's label gets the "current row" chip — full blue when
        // the pane is focused, faded blue when it is not — exactly like the tree
        // and leaf panes highlight their current row. Everything else sits on the
        // pane surface (which itself dims when the pane loses focus).
        let focused = ctx.owner_active();
        let chip = self.active && self.kind == LabelKind::Label;
        let fill = if chip {
            ctx.style(if focused {
                Role::ListFocused
            } else {
                Role::ListSelected
            })
        } else {
            ctx.style(if focused {
                Role::ListNormal
            } else {
                Role::ListInactive
            })
        };
        ctx.fill(Rect::new(0, 0, size.x, size.y), ' ', fill);
        if self.text.is_empty() {
            return;
        }

        // Text colour + weight (all pulled from theme roles):
        //   selected label       → the chip's foreground (bright-on-blue / faded)
        //   unfocused pane        → dim (Disabled fg)
        //   title (DN value)      → body text, bold
        //   normal field label    → a shade lighter than the value text
        let mut style = fill;
        if chip {
            // fg already carries the chip's on-blue foreground.
        } else if !focused {
            style.fg = ctx.style(Role::Disabled).fg;
        } else if self.kind == LabelKind::Title {
            style.modifiers.bold = true;
        } else {
            style.fg = ctx.style(Role::Disabled).fg;
        }

        let x = match self.kind {
            LabelKind::Title => 0,
            LabelKind::Label => {
                let text_w = UnicodeWidthStr::width(self.text.as_str()) as i32;
                (size.x - text_w - LABEL_GAP).max(0)
            }
        };
        ctx.put_str(x, 0, &self.text, style);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::edaptor_theme;
    use tvision_rs::{Buffer, Color, Point};

    /// Render a single label and return (symbol, fg, bg) of each cell in row 0.
    /// `owner_active` is the owning pane's focus, which `FieldLabel::draw` reads
    /// via `ctx.owner_active()` for its bright/dim surface — the real signal the
    /// framework fans from the form pane's `Group::draw`.
    fn render_row(fl: &mut FieldLabel, w: u16, owner_active: bool) -> Vec<(String, Color, Color)> {
        let theme = edaptor_theme();
        let mut buf = Buffer::new(w, 1);
        {
            let mut ctx = DrawCtx::new(
                &mut buf,
                &theme,
                Rect::new(0, 0, w as i32, 1),
                Point::new(0, 0),
            );
            ctx.set_owner_active(owner_active);
            fl.draw(&mut ctx);
        }
        (0..w)
            .map(|x| {
                let c = buf.get(x, 0);
                (c.symbol().to_string(), c.style().fg, c.style().bg)
            })
            .collect()
    }

    #[test]
    fn label_is_right_aligned() {
        let mut fl = FieldLabel::label(Rect::new(0, 0, 10, 1));
        fl.set_value(FieldValue::Text("cn".into()));
        let row = render_row(&mut fl, 10, true);
        // Width 10, label "cn" (2 cols) with a 1-col right gap → text at cols 7,8.
        assert_eq!(row[7].0, "c");
        assert_eq!(row[8].0, "n");
        assert_eq!(row[9].0, " ", "the trailing gap column stays blank");
    }

    #[test]
    fn selected_label_gets_the_blue_chip() {
        let theme = edaptor_theme();
        let chip_bg = theme.style(Role::ListFocused).bg; // BLUE
        let faded_bg = theme.style(Role::ListSelected).bg; // faded selection
        let surface = theme.style(Role::ListNormal).bg;

        let mut fl = FieldLabel::label(Rect::new(0, 0, 6, 1));
        fl.set_value(FieldValue::Text("cn".into()));

        // Focused pane + active → full blue chip fills the label cell.
        fl.set_active(true);
        assert_eq!(
            render_row(&mut fl, 6, true)[0].2,
            chip_bg,
            "active label → blue chip"
        );

        // Focused pane + inactive → plain pane surface, no chip.
        fl.set_active(false);
        assert_eq!(
            render_row(&mut fl, 6, true)[0].2,
            surface,
            "inactive label → surface"
        );

        // Unfocused pane + active → faded selection chip (matches the other panes).
        fl.set_active(true);
        assert_eq!(
            render_row(&mut fl, 6, false)[0].2,
            faded_bg,
            "unfocused active → faded chip"
        );
    }

    #[test]
    fn label_text_is_lighter_than_value_text() {
        let theme = edaptor_theme();
        let value_fg = theme.style(Role::ListNormal).fg; // body text (values)
        let light_fg = theme.style(Role::Disabled).fg; // lighter label text
        assert_ne!(value_fg, light_fg, "test premise: the two tones differ");

        let mut fl = FieldLabel::label(Rect::new(0, 0, 6, 1));
        fl.set_value(FieldValue::Text("cn".into()));
        // "cn" is right-aligned to cols 3,4 in a width-6 cell (1-col gap).
        assert_eq!(
            render_row(&mut fl, 6, true)[3].1,
            light_fg,
            "label text uses the lighter tone"
        );
    }

    #[test]
    fn title_is_bold_body_text_and_left_aligned() {
        let theme = edaptor_theme();
        let body = theme.style(Role::ListNormal).fg;
        let mut fl = FieldLabel::title(Rect::new(0, 0, 20, 1));
        fl.set_value(FieldValue::Text("cn=a,dc=x".into()));
        let row = render_row(&mut fl, 20, true);
        assert_eq!(row[0].0, "c", "title is left-aligned in the value column");
        assert_eq!(row[0].1, body, "title uses body text colour");
    }
}
