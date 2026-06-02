//! Pure label templates: `"{cn} ({uid})"` → literal + {field} segments, rendered
//! against an entry's attributes. No worker, no UI. Used by the membership picker
//! to display candidates as e.g. "Bob Baker (bob)".

use std::collections::BTreeMap;

/// One segment of a label template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LabelSeg {
    Lit(String),
    Field(String),
}

/// Parse a label template into segments. Lenient/infallible: `{name}` is a Field,
/// everything else is literal text. An unterminated `{` (no closing `}`) is kept
/// as literal text (so a stray brace never panics or drops input).
pub fn parse_label_template(s: &str) -> Vec<LabelSeg> {
    let mut segs = Vec::new();
    let mut lit = String::new();
    let mut chars = s.char_indices().peekable();
    while let Some((idx, c)) = chars.next() {
        if c == '{' {
            // Look for the matching closing brace from here.
            if let Some(close) = s[idx + 1..].find('}') {
                // Flush any pending literal before the field.
                if !lit.is_empty() {
                    segs.push(LabelSeg::Lit(std::mem::take(&mut lit)));
                }
                let name = &s[idx + 1..idx + 1 + close];
                segs.push(LabelSeg::Field(name.to_string()));
                // Advance the iterator past the closing brace.
                while let Some(&(j, _)) = chars.peek() {
                    if j <= idx + 1 + close {
                        chars.next();
                    } else {
                        break;
                    }
                }
            } else {
                // Unterminated `{`: keep the rest as literal text.
                lit.push(c);
            }
        } else {
            lit.push(c);
        }
    }
    if !lit.is_empty() {
        segs.push(LabelSeg::Lit(lit));
    }
    segs
}

/// Resolve a template against an entry's attributes (case-insensitive attr match,
/// first value). A missing/empty field substitutes the EMPTY string — never drop
/// the whole label (do NOT use all-or-nothing semantics).
pub fn render_label(segs: &[LabelSeg], attrs: &BTreeMap<String, Vec<String>>) -> String {
    let mut out = String::new();
    for seg in segs {
        match seg {
            LabelSeg::Lit(s) => out.push_str(s),
            LabelSeg::Field(name) => {
                let value = attrs
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(name))
                    .and_then(|(_, v)| v.first())
                    .map(String::as_str)
                    .unwrap_or("");
                out.push_str(value);
            }
        }
    }
    out
}

/// The distinct attribute names a template references (the `{field}` segments),
/// case-preserved, de-duplicated. Used to decide which attrs the picker fetches.
pub fn template_attrs(segs: &[LabelSeg]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for seg in segs {
        if let LabelSeg::Field(name) = seg {
            if !out.iter().any(|n| n == name) {
                out.push(name.clone());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
        pairs
            .iter()
            .map(|(k, vs)| (k.to_string(), vs.iter().map(|s| s.to_string()).collect()))
            .collect()
    }

    #[test]
    fn parses_fields_and_literals() {
        assert_eq!(
            parse_label_template("{cn} ({uid})"),
            vec![
                LabelSeg::Field("cn".into()),
                LabelSeg::Lit(" (".into()),
                LabelSeg::Field("uid".into()),
                LabelSeg::Lit(")".into()),
            ]
        );
    }

    #[test]
    fn no_braces_is_literal() {
        assert_eq!(parse_label_template("cn"), vec![LabelSeg::Lit("cn".into())]);
    }

    #[test]
    fn unterminated_brace_is_literal() {
        assert_eq!(
            parse_label_template("/home/{uid"),
            vec![LabelSeg::Lit("/home/{uid".into())]
        );
    }

    #[test]
    fn renders_all_fields_present() {
        let segs = parse_label_template("{cn} ({uid})");
        let attrs = map(&[("cn", &["Bob Baker"]), ("uid", &["bob"])]);
        assert_eq!(render_label(&segs, &attrs), "Bob Baker (bob)");
    }

    #[test]
    fn renders_missing_field_as_empty() {
        let segs = parse_label_template("{cn} ({uid})");
        let attrs = map(&[("cn", &["Bob Baker"])]);
        assert_eq!(render_label(&segs, &attrs), "Bob Baker ()");
    }

    #[test]
    fn render_is_case_insensitive() {
        let segs = parse_label_template("{CN}");
        let attrs = map(&[("cn", &["Bob Baker"])]);
        assert_eq!(render_label(&segs, &attrs), "Bob Baker");
    }

    #[test]
    fn template_attrs_dedup_and_preserve_case() {
        assert_eq!(
            template_attrs(&parse_label_template("{cn} ({uid})")),
            vec!["cn".to_string(), "uid".to_string()]
        );
        assert_eq!(
            template_attrs(&parse_label_template("{cn}-{cn}")),
            vec!["cn".to_string()]
        );
    }
}
