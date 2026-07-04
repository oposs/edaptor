//! Shared `HelpCtx` constants for edaptor field value views.
//!
//! Both the form pane (which stamps `help_ctx` on each value view) and the
//! status line (which maps a context to a hint string) reference these constants,
//! so a rename or typo in one place cannot silently break the mapping.
//!
//! All constants use `HelpCtx::custom` with the `edaptor.field.*` prefix.

use tvision_rs::HelpCtx;

/// A plain single-value text field — editable inline.
pub(crate) const FIELD_TEXT: HelpCtx = HelpCtx::custom("edaptor.field.text");

/// A multi-value list field — inline editor, unordered.
pub(crate) const FIELD_LIST: HelpCtx = HelpCtx::custom("edaptor.field.list");

/// A multi-value list field — inline editor, ordered (`XOrdered`).
pub(crate) const FIELD_LIST_ORDERED: HelpCtx = HelpCtx::custom("edaptor.field.list.ordered");

/// The reorder handle column of an ordered list field (`≡` gutter).
pub(crate) const FIELD_LIST_HANDLE: HelpCtx = HelpCtx::custom("edaptor.field.list.handle");

/// A modal-launch field that opens a picker (objectClass, choice, picker, …).
pub(crate) const FIELD_LAUNCH_PICKER: HelpCtx = HelpCtx::custom("edaptor.field.launch.picker");

/// A modal-launch field that opens a password editor.
pub(crate) const FIELD_LAUNCH_PASSWORD: HelpCtx = HelpCtx::custom("edaptor.field.launch.password");

/// Map a `HelpCtx` to the hint string shown in the status line footer.
/// Returns `None` for any unknown context (no hint drawn).
///
/// This is a pure function so it can be unit-tested without constructing a
/// `StatusLine`.
pub(crate) fn hint_for(ctx: HelpCtx) -> Option<String> {
    Some(
        match ctx.name() {
            "edaptor.field.text" => "\u{2191}\u{2193} move \u{00b7} Enter next field",
            "edaptor.field.list" => {
                "Enter add \u{00b7} Ctrl-Enter newline \u{00b7} Backspace empties\u{2192}removes \u{00b7} \u{2191}\u{2193} move"
            }
            "edaptor.field.list.ordered" => {
                "Enter add \u{00b7} Ctrl-Enter newline \u{00b7} Ctrl-\u{2191}\u{2193} or \u{2190} handle to reorder"
            }
            "edaptor.field.list.handle" => "\u{2191}\u{2193} reorder \u{00b7} \u{2192} back to text",
            "edaptor.field.launch.picker" => "any key: open picker \u{00b7} \u{2191}\u{2193} move",
            "edaptor.field.launch.password" => "any key: edit password",
            _ => return None,
        }
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hint_for_returns_correct_string_per_constant() {
        // Each constant must produce a non-None, non-empty hint.
        let cases = [
            (FIELD_TEXT, "↑↓ move · Enter next field"),
            (
                FIELD_LIST,
                "Enter add · Ctrl-Enter newline · Backspace empties→removes · ↑↓ move",
            ),
            (
                FIELD_LIST_ORDERED,
                "Enter add · Ctrl-Enter newline · Ctrl-↑↓ or ← handle to reorder",
            ),
            (FIELD_LIST_HANDLE, "↑↓ reorder · → back to text"),
            (FIELD_LAUNCH_PICKER, "any key: open picker · ↑↓ move"),
            (FIELD_LAUNCH_PASSWORD, "any key: edit password"),
        ];
        for (ctx, expected) in cases {
            let hint =
                hint_for(ctx).unwrap_or_else(|| panic!("hint_for({}) returned None", ctx.name()));
            assert_eq!(hint, expected, "wrong hint for context {}", ctx.name());
        }
    }

    #[test]
    fn hint_for_returns_none_for_unknown_context() {
        let unknown = HelpCtx::custom("unknown.context");
        assert_eq!(hint_for(unknown), None, "unknown context must return None");
    }

    #[test]
    fn hint_for_returns_none_for_no_context() {
        assert_eq!(
            hint_for(HelpCtx::NO_CONTEXT),
            None,
            "NO_CONTEXT must return None"
        );
    }

    #[test]
    fn constants_have_expected_names() {
        // Guard: if a constant's name is changed, the hint mapping breaks.
        // This test catches both.
        assert_eq!(FIELD_TEXT.name(), "edaptor.field.text");
        assert_eq!(FIELD_LIST.name(), "edaptor.field.list");
        assert_eq!(FIELD_LIST_ORDERED.name(), "edaptor.field.list.ordered");
        assert_eq!(FIELD_LIST_HANDLE.name(), "edaptor.field.list.handle");
        assert_eq!(FIELD_LAUNCH_PICKER.name(), "edaptor.field.launch.picker");
        assert_eq!(
            FIELD_LAUNCH_PASSWORD.name(),
            "edaptor.field.launch.password"
        );
    }
}
