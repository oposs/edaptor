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

/// Per-target live-template latch (see the live-templated-defaults spec). `segs`
/// is the parsed template; `auto` is true while the target still belongs to the
/// template; `last_written` is the value we last wrote, used to tell our own
/// writes apart from operator edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveTemplateState {
    pub segs: Vec<Seg>,
    pub auto: bool,
    pub last_written: String,
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

/// Helper: is the attr currently empty (no non-blank value)?
fn is_empty(current: &BTreeMap<String, Vec<String>>, attr: &str) -> bool {
    current
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(attr))
        .map(|(_, v)| v.iter().all(|s| s.trim().is_empty()))
        .unwrap_or(true)
}

/// Resolve a template against current field values; `None` if any `{field}` is empty.
fn resolve_template(segs: &[Seg], current: &BTreeMap<String, Vec<String>>) -> Option<String> {
    let mut out = String::new();
    for seg in segs {
        match seg {
            Seg::Lit(s) => out.push_str(s),
            Seg::Field(name) => {
                let v = current
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(name))
                    .and_then(|(_, v)| v.first())
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())?;
                out.push_str(v);
            }
        }
    }
    Some(out)
}

/// Plan which EMPTY fields to fill. Operator-entered values are never overwritten.
pub fn plan_defaults(
    d: &ProfileDefaults,
    current: &BTreeMap<String, Vec<String>>,
) -> Vec<Resolution> {
    let mut out = Vec::new();
    for (attr, dv) in &d.entries {
        if !is_empty(current, attr) {
            continue;
        }
        match dv {
            DefaultValue::Literal(s) => out.push(Resolution::Fill {
                attr: attr.clone(),
                value: s.clone(),
            }),
            DefaultValue::Template(segs) => {
                if let Some(v) = resolve_template(segs, current) {
                    out.push(Resolution::Fill {
                        attr: attr.clone(),
                        value: v,
                    });
                }
            }
            DefaultValue::AutoNumber { min, max } => out.push(Resolution::NeedsAutonumber {
                attr: attr.clone(),
                min: *min,
                max: *max,
            }),
        }
    }
    out
}

/// Build the initial live-template latches from a profile's `[profile.defaults]`:
/// one entry per Template default (literals and autonumbers are skipped). Each
/// starts `auto = true`, `last_written = ""`.
pub fn live_templates(d: &ProfileDefaults) -> BTreeMap<String, LiveTemplateState> {
    d.entries
        .iter()
        .filter_map(|(attr, dv)| match dv {
            DefaultValue::Template(segs) => Some((
                attr.clone(),
                LiveTemplateState {
                    segs: segs.clone(),
                    auto: true,
                    last_written: String::new(),
                },
            )),
            _ => None,
        })
        .collect()
}

/// The first value of `attr` in `current` (case-insensitive key match), or "".
/// Unlike `resolve_template`'s `Field` arm, this does NOT trim: it reads the
/// target's *raw* value to compare against `last_written` (also stored raw), so
/// the own-write/operator-edit disambiguation in `recompute_live` stays exact.
fn first_value(current: &BTreeMap<String, Vec<String>>, attr: &str) -> String {
    current
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(attr))
        .and_then(|(_, v)| v.first())
        .cloned()
        .unwrap_or_default()
}

/// Recompute every auto target against `current` field values, mutating the
/// latches, and return the `(attr, new_value)` changes to apply to the form.
/// Pure. Implements the per-pass rule from the spec:
/// 1. if the target's current value differs from `last_written`, ownership is
///    re-evaluated: `auto = value.is_empty()` (empty ⇒ re-arm, else operator owns);
/// 2. while `auto`, mirror the template: `Some(out)` ⇒ write `out` if it differs;
///    `None` (a source empty) ⇒ clear the target if non-empty.
pub fn recompute_live(
    states: &mut BTreeMap<String, LiveTemplateState>,
    current: &BTreeMap<String, Vec<String>>,
) -> Vec<(String, String)> {
    let mut changes = Vec::new();
    for (attr, st) in states.iter_mut() {
        let value = first_value(current, attr);
        if value != st.last_written {
            st.auto = value.is_empty();
        }
        if !st.auto {
            continue;
        }
        match resolve_template(&st.segs, current) {
            Some(out) => {
                if out != value {
                    st.last_written = out.clone();
                    changes.push((attr.clone(), out));
                }
            }
            None => {
                if !value.is_empty() {
                    st.last_written = String::new();
                    changes.push((attr.clone(), String::new()));
                }
            }
        }
    }
    changes
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

    #[test]
    fn empty_placeholder_is_error() {
        assert!(parse_default_value("/home/{}").is_err());
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

    // Task 2.3 tests — plan_defaults

    fn cur(pairs: &[(&str, &str)]) -> BTreeMap<String, Vec<String>> {
        pairs
            .iter()
            .map(|(k, v)| {
                (
                    k.to_string(),
                    if v.is_empty() {
                        vec![]
                    } else {
                        vec![v.to_string()]
                    },
                )
            })
            .collect()
    }

    #[test]
    fn fills_only_empty_fields() {
        let mut d = ProfileDefaults::default();
        d.entries.insert(
            "loginShell".into(),
            DefaultValue::Literal("/bin/bash".into()),
        );
        assert!(plan_defaults(&d, &cur(&[("loginShell", "/bin/zsh")])).is_empty());
        assert_eq!(
            plan_defaults(&d, &cur(&[("loginShell", "")])),
            vec![Resolution::Fill {
                attr: "loginShell".into(),
                value: "/bin/bash".into()
            }]
        );
    }

    #[test]
    fn resolves_template_against_current_values() {
        let mut d = ProfileDefaults::default();
        d.entries.insert(
            "homeDirectory".into(),
            parse_default_value("/home/{uid}").unwrap(),
        );
        assert_eq!(
            plan_defaults(&d, &cur(&[("uid", "alice"), ("homeDirectory", "")])),
            vec![Resolution::Fill {
                attr: "homeDirectory".into(),
                value: "/home/alice".into()
            }]
        );
    }

    #[test]
    fn template_with_empty_source_yields_no_fill() {
        let mut d = ProfileDefaults::default();
        d.entries.insert(
            "homeDirectory".into(),
            parse_default_value("/home/{uid}").unwrap(),
        );
        assert!(plan_defaults(&d, &cur(&[("uid", ""), ("homeDirectory", "")])).is_empty());
    }

    #[test]
    fn autonumber_surfaces_as_needs_autonumber() {
        let mut d = ProfileDefaults::default();
        d.entries.insert(
            "uidNumber".into(),
            parse_default_value("{next:10000-60000}").unwrap(),
        );
        assert_eq!(
            plan_defaults(&d, &cur(&[("uidNumber", "")])),
            vec![Resolution::NeedsAutonumber {
                attr: "uidNumber".into(),
                min: 10000,
                max: 60000
            }]
        );
    }

    // --- live templated defaults ---

    fn defs(pairs: &[(&str, &str)]) -> ProfileDefaults {
        let mut d = ProfileDefaults::default();
        for (k, v) in pairs {
            d.entries
                .insert(k.to_string(), parse_default_value(v).unwrap());
        }
        d
    }

    #[test]
    fn live_templates_picks_only_templates() {
        let d = defs(&[
            ("cn", "{givenName} {sn}"),
            ("loginShell", "/bin/bash"),       // literal → excluded
            ("uidNumber", "{next:1000-2000}"), // autonumber → excluded
        ]);
        let states = live_templates(&d);
        assert_eq!(states.keys().collect::<Vec<_>>(), vec!["cn"]);
        let s = &states["cn"];
        assert!(s.auto);
        assert_eq!(s.last_written, "");
    }

    #[test]
    fn recompute_fills_when_sources_present() {
        let mut states = live_templates(&defs(&[("cn", "{givenName} {sn}")]));
        let changes = recompute_live(&mut states, &cur(&[("givenName", "John"), ("sn", "Doe")]));
        assert_eq!(changes, vec![("cn".to_string(), "John Doe".to_string())]);
        assert_eq!(states["cn"].last_written, "John Doe");
        assert!(states["cn"].auto);
    }

    #[test]
    fn recompute_incomplete_source_clears_target() {
        let mut states = live_templates(&defs(&[("cn", "{givenName} {sn}")]));
        // First fill, then remove sn: the auto target must clear.
        recompute_live(&mut states, &cur(&[("givenName", "John"), ("sn", "Doe")]));
        let changes = recompute_live(
            &mut states,
            &cur(&[("givenName", "John"), ("sn", ""), ("cn", "John Doe")]),
        );
        assert_eq!(changes, vec![("cn".to_string(), "".to_string())]);
        assert!(states["cn"].auto);
        assert_eq!(states["cn"].last_written, "");
    }

    #[test]
    fn recompute_stops_when_operator_overrides() {
        let mut states = live_templates(&defs(&[("cn", "{givenName} {sn}")]));
        recompute_live(&mut states, &cur(&[("givenName", "John"), ("sn", "Doe")])); // cn = "John Doe"
                                                                                    // Operator edits cn to something else, then changes a source.
        let changes = recompute_live(
            &mut states,
            &cur(&[("givenName", "Jon"), ("sn", "Doe"), ("cn", "Johnny")]),
        );
        assert!(changes.is_empty(), "operator-owned field is not rewritten");
        assert!(!states["cn"].auto);
        // A further source change is still ignored.
        let changes = recompute_live(
            &mut states,
            &cur(&[("givenName", "Jonathan"), ("sn", "Doe"), ("cn", "Johnny")]),
        );
        assert!(changes.is_empty());
        assert!(!states["cn"].auto);
    }

    #[test]
    fn recompute_rearms_when_target_cleared() {
        let mut states = live_templates(&defs(&[("cn", "{givenName} {sn}")]));
        recompute_live(&mut states, &cur(&[("givenName", "John"), ("sn", "Doe")]));
        recompute_live(
            &mut states,
            &cur(&[("givenName", "John"), ("sn", "Doe"), ("cn", "Johnny")]),
        ); // owned
        assert!(!states["cn"].auto);
        // Operator clears cn → re-arm and refill.
        let changes = recompute_live(
            &mut states,
            &cur(&[("givenName", "John"), ("sn", "Doe"), ("cn", "")]),
        );
        assert_eq!(changes, vec![("cn".to_string(), "John Doe".to_string())]);
        assert!(states["cn"].auto);
    }

    #[test]
    fn recompute_our_write_is_not_read_as_override() {
        // Two passes with unchanged sources: the second must NOT flip auto off just
        // because the target now holds our written value.
        let mut states = live_templates(&defs(&[("cn", "{givenName} {sn}")]));
        recompute_live(&mut states, &cur(&[("givenName", "John"), ("sn", "Doe")]));
        let changes = recompute_live(
            &mut states,
            &cur(&[("givenName", "John"), ("sn", "Doe"), ("cn", "John Doe")]),
        );
        assert!(
            changes.is_empty(),
            "no change: target already equals output"
        );
        assert!(
            states["cn"].auto,
            "still auto after our own write is read back"
        );
    }
}
