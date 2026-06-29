//! X-ORDERED multi-value editor: like the free-text multi-value editor, but it
//! owns the OpenLDAP `X-ORDERED 'VALUES'` `{n}` ordering prefix. Values are shown
//! with the `{n}` stripped; on commit the prefix is reconstructed from the current
//! row order, so reordering rows is the central operation. Staged values carry
//! `{n}`, so the neutral `form::changeset::diff` (which special-cases x-ordered
//! attrs into a single `Replace`) is unchanged. First save after editing may emit
//! one normalizing `Replace` if the server's stored indices were not `{0..n-1}`;
//! the server re-normalizes, so this is harmless. Capability: `Static`.

/// Drop a leading `{<digits>}` ordering prefix; return everything else unchanged.
/// A `{` not followed by one-or-more ASCII digits and a `}` is NOT a prefix.
#[allow(dead_code)]
pub(crate) fn strip_ordering(s: &str) -> &str {
    let bytes = s.as_bytes();
    if bytes.first() != Some(&b'{') {
        return s;
    }
    let mut i = 1;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    // Need at least one digit (i > 1) and a closing '}' right after.
    if i > 1 && bytes.get(i) == Some(&b'}') {
        &s[i + 1..]
    } else {
        s
    }
}

/// Prepend `{i}` (contiguous row index) to each row, in order.
#[allow(dead_code)]
pub(crate) fn reconstruct(rows: &[String]) -> Vec<String> {
    rows.iter()
        .enumerate()
        .map(|(i, r)| format!("{{{i}}}{r}"))
        .collect()
}

#[cfg(test)]
mod helper_tests {
    use super::*;

    #[test]
    fn strip_removes_leading_index_only() {
        assert_eq!(strip_ordering("{0}read by self"), "read by self");
        assert_eq!(strip_ordering("{12}write"), "write");
        assert_eq!(strip_ordering("{0}"), "");
    }

    #[test]
    fn strip_leaves_non_index_braces() {
        assert_eq!(strip_ordering("plain"), "plain");
        assert_eq!(strip_ordering("{}empty"), "{}empty");
        assert_eq!(strip_ordering("{a}x"), "{a}x");
        assert_eq!(strip_ordering("by group/{0}"), "by group/{0}");
        assert_eq!(strip_ordering(""), "");
    }

    #[test]
    fn reconstruct_numbers_rows_in_order() {
        assert_eq!(
            reconstruct(&["write".to_string(), "read".to_string()]),
            vec!["{0}write".to_string(), "{1}read".to_string()]
        );
    }

    #[test]
    fn strip_then_reconstruct_round_trips_order() {
        let stored = ["{0}a".to_string(), "{1}b".to_string()];
        let display: Vec<String> = stored
            .iter()
            .map(|s| strip_ordering(s).to_string())
            .collect();
        assert_eq!(
            reconstruct(&display),
            vec!["{0}a".to_string(), "{1}b".to_string()]
        );
    }
}
