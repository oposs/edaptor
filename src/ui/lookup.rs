//! The `lookup` widget: a scalar field shown as `<value> (<name>)` and edited via
//! an editable-combobox popup. This module holds the pure input model (parse /
//! validity / filter / display) plus, in a later task, the FieldWidget/editor/
//! dialog. The value in the input is authoritative: its leading integer is the
//! committed value; picking a candidate writes `<value> (<name>)` back into it.

// NOTE: the pure helpers below carry `#[allow(dead_code)]` only because their
// production callers land in Task 5 (the LookupDialog wires them in). Until then
// they are exercised solely by this module's tests, which would otherwise trip
// `dead_code` on the non-test lib build. Task 5 removes each `#[allow(dead_code)]`
// as it adds the real call site.

/// The pending value = the leading run of ASCII digits in `input`, if any.
/// `"5000"` → `Some("5000")`; `"5000 (staff)"` → `Some("5000")`; `"staff"` → `None`;
/// `""` → `None`.
#[allow(dead_code)]
pub(crate) fn leading_number(input: &str) -> Option<String> {
    let digits: String = input
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        None
    } else {
        Some(digits)
    }
}

/// OK is enabled iff the input yields a committable value.
#[allow(dead_code)]
pub(crate) fn ok_enabled(input: &str) -> bool {
    leading_number(input).is_some()
}

/// List-filter predicate: empty filter matches all; otherwise the candidate
/// matches when its label contains `filter` (case-insensitive) OR its value
/// starts with `filter` (numeric-prefix search when the user types digits).
#[allow(dead_code)]
pub(crate) fn row_matches(label: &str, value: &str, filter: &str) -> bool {
    let f = filter.trim();
    if f.is_empty() {
        return true;
    }
    label.to_ascii_lowercase().contains(&f.to_ascii_lowercase()) || value.starts_with(f)
}

/// A list row renders as `"{label} ({value})"`, e.g. `"staff (5000)"`.
#[allow(dead_code)]
pub(crate) fn row_display(value: &str, label: &str) -> String {
    format!("{label} ({value})")
}

/// Picking a row fills the input with `"{value} ({label})"`, e.g. `"5000 (staff)"`.
#[allow(dead_code)]
pub(crate) fn input_after_pick(value: &str, label: &str) -> String {
    format!("{value} ({label})")
}

/// The index of the row whose value exactly equals the input's leading number,
/// so a typed number highlights its matching group.
#[allow(dead_code)]
pub(crate) fn highlight_index(rows: &[(String, String)], input: &str) -> Option<usize> {
    let n = leading_number(input)?;
    rows.iter().position(|(value, _label)| *value == n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leading_number_extracts_prefix_digits() {
        assert_eq!(leading_number("5000"), Some("5000".into()));
        assert_eq!(leading_number("5000 (staff)"), Some("5000".into()));
        assert_eq!(leading_number("staff"), None);
        assert_eq!(leading_number(""), None);
        assert_eq!(leading_number("  42x"), Some("42".into()));
    }

    #[test]
    fn ok_enabled_requires_leading_number() {
        assert!(ok_enabled("5000"));
        assert!(ok_enabled("5000 (staff)"));
        assert!(!ok_enabled("staff"));
        assert!(!ok_enabled(""));
    }

    #[test]
    fn row_matches_by_label_substring_and_value_prefix() {
        assert!(row_matches("staff", "5000", "")); // empty → all
        assert!(row_matches("staff", "5000", "sta")); // label substring, ci
        assert!(row_matches("Staff", "5000", "aff"));
        assert!(row_matches("staff", "5000", "50")); // numeric prefix on value
        assert!(!row_matches("staff", "5000", "99"));
        assert!(!row_matches("users", "100", "xyz"));
    }

    #[test]
    fn display_helpers_use_opposite_orders() {
        assert_eq!(row_display("5000", "staff"), "staff (5000)");
        assert_eq!(input_after_pick("5000", "staff"), "5000 (staff)");
    }

    #[test]
    fn highlight_matches_exact_value() {
        let rows = vec![
            ("100".to_string(), "users".to_string()),
            ("5000".to_string(), "staff".to_string()),
        ];
        assert_eq!(highlight_index(&rows, "5000"), Some(1));
        assert_eq!(highlight_index(&rows, "5000 (staff)"), Some(1));
        assert_eq!(highlight_index(&rows, "50"), None); // prefix, not exact
        assert_eq!(highlight_index(&rows, "staff"), None); // no leading number
    }
}
