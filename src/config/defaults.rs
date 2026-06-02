//! Pure parsing + planning for `[profile.defaults]`: literal / `{attr}` template /
//! `{next:MIN-MAX}` autonumber. No worker, no UI.

use serde::{Deserialize, Deserializer};
use std::collections::BTreeMap;

/// One segment of a template value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seg {
    Lit(String),
    Field(String),
}

/// A parsed default value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefaultValue {
    Literal(String),
    Template(Vec<Seg>),
    AutoNumber { min: u64, max: u64 },
}

/// A profile's `[profile.defaults]` table (attr -> parsed value), order-stable.
#[derive(Debug, Clone, Default)]
pub struct ProfileDefaults {
    pub entries: BTreeMap<String, DefaultValue>,
}

/// A planned action for one defaulted attribute (see `plan_defaults`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    Fill { attr: String, value: String },
    NeedsAutonumber { attr: String, min: u64, max: u64 },
}

/// Parse one config value string into a `DefaultValue`.
pub fn parse_default_value(s: &str) -> Result<DefaultValue, String> {
    let trimmed = s.trim();
    if let Some(inner) = trimmed
        .strip_prefix("{next:")
        .and_then(|r| r.strip_suffix('}'))
    {
        let (lo, hi) = inner
            .split_once('-')
            .ok_or_else(|| format!("autonumber '{s}' must be {{next:MIN-MAX}}"))?;
        let min: u64 = lo
            .trim()
            .parse()
            .map_err(|_| format!("autonumber MIN '{lo}' is not a number"))?;
        let max: u64 = hi
            .trim()
            .parse()
            .map_err(|_| format!("autonumber MAX '{hi}' is not a number"))?;
        if min > max {
            return Err(format!("autonumber range '{s}' has MIN > MAX"));
        }
        return Ok(DefaultValue::AutoNumber { min, max });
    }
    if !s.contains('{') {
        return Ok(DefaultValue::Literal(s.to_string()));
    }
    let mut segs = Vec::new();
    let mut lit = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            if !lit.is_empty() {
                segs.push(Seg::Lit(std::mem::take(&mut lit)));
            }
            let mut name = String::new();
            let mut closed = false;
            for c2 in chars.by_ref() {
                if c2 == '}' {
                    closed = true;
                    break;
                }
                name.push(c2);
            }
            if !closed {
                return Err(format!("unterminated placeholder in '{s}'"));
            }
            if name.is_empty() {
                return Err(format!("empty placeholder in '{s}'"));
            }
            segs.push(Seg::Field(name));
        } else {
            lit.push(c);
        }
    }
    if !lit.is_empty() {
        segs.push(Seg::Lit(lit));
    }
    Ok(DefaultValue::Template(segs))
}

impl<'de> Deserialize<'de> for ProfileDefaults {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw: BTreeMap<String, String> = BTreeMap::deserialize(d)?;
        let mut entries = BTreeMap::new();
        for (k, v) in raw {
            let parsed = parse_default_value(&v).map_err(serde::de::Error::custom)?;
            entries.insert(k, parsed);
        }
        Ok(ProfileDefaults { entries })
    }
}

/// Next free number: max of `existing` within `[min,max]`, plus one; `min` if none
/// in window. Errors if the pool is exhausted.
pub fn next_in_range(existing: &[u64], min: u64, max: u64) -> Result<u64, String> {
    let cur_max = existing
        .iter()
        .copied()
        .filter(|n| *n >= min && *n <= max)
        .max();
    let next = match cur_max {
        Some(m) => m + 1,
        None => min,
    };
    if next > max {
        return Err(format!("number pool {min}-{max} is exhausted"));
    }
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Task 2.1 tests — parse_default_value

    #[test]
    fn parses_literal() {
        assert!(
            matches!(parse_default_value("/bin/bash"), Ok(DefaultValue::Literal(s)) if s == "/bin/bash")
        );
    }

    #[test]
    fn parses_template_with_embedded_text() {
        match parse_default_value("/home/{uid}").unwrap() {
            DefaultValue::Template(segs) => {
                assert!(matches!(&segs[0], Seg::Lit(s) if s == "/home/"));
                assert!(matches!(&segs[1], Seg::Field(s) if s == "uid"));
            }
            _ => panic!("expected template"),
        }
    }

    #[test]
    fn parses_multi_placeholder_template() {
        match parse_default_value("{givenName}.{sn}").unwrap() {
            DefaultValue::Template(segs) => assert_eq!(segs.len(), 3), // Field, Lit("."), Field
            _ => panic!(),
        }
    }

    #[test]
    fn parses_autonumber() {
        assert!(matches!(
            parse_default_value("{next:10000-60000}"),
            Ok(DefaultValue::AutoNumber {
                min: 10000,
                max: 60000
            })
        ));
    }

    #[test]
    fn autonumber_min_gt_max_is_error() {
        assert!(parse_default_value("{next:60000-10000}").is_err());
    }

    #[test]
    fn malformed_autonumber_is_error() {
        assert!(parse_default_value("{next:abc}").is_err());
        assert!(parse_default_value("{next:10000}").is_err());
    }

    #[test]
    fn unterminated_placeholder_is_error() {
        assert!(parse_default_value("/home/{uid").is_err());
    }

    // Task 2.2 tests — next_in_range

    #[test]
    fn next_in_range_empty_returns_min() {
        assert_eq!(next_in_range(&[], 10000, 60000).unwrap(), 10000);
    }

    #[test]
    fn next_in_range_is_max_plus_one() {
        assert_eq!(
            next_in_range(&[10000, 10005, 10003], 10000, 60000).unwrap(),
            10006
        );
    }

    #[test]
    fn next_in_range_ignores_out_of_window_values() {
        assert_eq!(
            next_in_range(&[9000, 70000, 10002], 10000, 60000).unwrap(),
            10003
        );
    }

    #[test]
    fn next_in_range_exhausted_errors() {
        assert!(next_in_range(&[60000], 10000, 60000).is_err());
    }
}
