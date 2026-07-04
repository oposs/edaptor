//! Shared formatting of a field's values into display rows (bulleted list,
//! `<not set>`, `*****`). Used by the read-only launch block and the inline list
//! editor so both look identical.

#[allow(dead_code)]
pub(crate) const NOT_SET: &str = "<not set>";

#[allow(dead_code)]
pub(crate) fn bullet_lines(values: &[String], strip_ordering: bool) -> Vec<String> {
    let cleaned: Vec<String> = values
        .iter()
        .map(|v| {
            if strip_ordering {
                crate::ui::ordered::strip_ordering(v).to_string()
            } else {
                v.clone()
            }
        })
        .filter(|v| !v.trim().is_empty())
        .collect();
    if cleaned.is_empty() {
        return vec![NOT_SET.to_string()];
    }
    let mut out = Vec::new();
    for v in &cleaned {
        for (i, line) in v.split('\n').enumerate() {
            out.push(if i == 0 { format!("- {line}") } else { format!("  {line}") });
        }
    }
    out
}

#[allow(dead_code)]
pub(crate) fn masked_line() -> Vec<String> {
    vec!["*****".to_string()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_not_set() {
        assert_eq!(bullet_lines(&[], false), vec![NOT_SET.to_string()]);
        assert_eq!(bullet_lines(&["   ".into()], false), vec![NOT_SET.to_string()]);
    }

    #[test]
    fn values_render_as_bullets() {
        let v = vec!["a".to_string(), "b".to_string()];
        assert_eq!(bullet_lines(&v, false), vec!["- a".to_string(), "- b".to_string()]);
    }

    #[test]
    fn newline_becomes_indented_continuation() {
        let v = vec!["b\ncont".to_string()];
        assert_eq!(bullet_lines(&v, false), vec!["- b".to_string(), "  cont".to_string()]);
    }

    #[test]
    fn ordering_prefix_stripped_when_requested() {
        let v = vec!["{0}read".to_string(), "{1}write".to_string()];
        assert_eq!(bullet_lines(&v, true), vec!["- read".to_string(), "- write".to_string()]);
    }
}
