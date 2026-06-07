//! Resolved `[profile.widget.<attr>]` choice widgets + the pure token logic
//! (parse/serialise/commit/summary). Mirrors `config::relation` for pickers.

use std::collections::BTreeSet;

use crate::config::relation::Cardinality;
use crate::config::{ChoiceOption, EntryProfile, WidgetSpecCfg};

/// How a choice widget's value string is encoded. `Bitmask`/`Delimited` are
/// reserved — they parse in config but error at resolve time until wired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChoiceFormat {
    /// Single token; the value *is* the chosen option (e.g. `loginShell`).
    Plain,
    /// Samba `sambaAcctFlags`-style bracketed letters (owned by `samba::account`).
    Bracketed,
}

/// A resolved, ready-to-use choice widget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceWidget {
    pub select: Cardinality,
    pub format: ChoiceFormat,
    pub options: Vec<ChoiceOption>,
}

/// A resolved password widget: the primary cleartext attr plus the derived
/// attrs written alongside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordWidget {
    pub primary: String,
    pub derived: Vec<String>,
    pub samba: bool,
}

/// A resolved widget of any palette kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WidgetKind {
    Choice(ChoiceWidget),
    Password(PasswordWidget),
}

/// A resolved widget bound to its owning profile's object classes (for matching).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedWidget {
    pub owner_object_classes: Vec<String>,
    pub attr: String,
    pub kind: WidgetKind,
}

/// Resolve every `[profile.widget.*]`. Returns `Err(msg)` on an invalid binding
/// (empty options, unknown select/format, or a reserved-but-unwired format) so
/// the operator sees a loud config error rather than a silent no-op.
pub fn resolve_widgets(profiles: &[EntryProfile]) -> Result<Vec<ResolvedWidget>, String> {
    let mut out = Vec::new();
    for owner in profiles {
        for (attr, spec) in &owner.widgets {
            let kind = match spec {
                WidgetSpecCfg::Choice {
                    select,
                    format,
                    options,
                } => {
                    if options.is_empty() {
                        return Err(format!(
                            "[profile.widget.{attr}]: options must not be empty"
                        ));
                    }
                    let select = match select.to_ascii_lowercase().as_str() {
                        "single" => Cardinality::Single,
                        "multi" => Cardinality::Multi,
                        other => {
                            return Err(format!("[profile.widget.{attr}]: bad select \"{other}\""))
                        }
                    };
                    let format = match format.to_ascii_lowercase().as_str() {
                        "plain" => ChoiceFormat::Plain,
                        "bracketed" => ChoiceFormat::Bracketed,
                        "bitmask" | "delimited" => {
                            return Err(format!(
                                "[profile.widget.{attr}]: format \"{format}\" not yet implemented"
                            ))
                        }
                        other => {
                            return Err(format!("[profile.widget.{attr}]: bad format \"{other}\""))
                        }
                    };
                    if format == ChoiceFormat::Plain && select == Cardinality::Multi {
                        return Err(format!(
                            "[profile.widget.{attr}]: format \"plain\" requires select = \"single\""
                        ));
                    }
                    WidgetKind::Choice(ChoiceWidget {
                        select,
                        format,
                        options: options.clone(),
                    })
                }
                WidgetSpecCfg::Password { samba } => {
                    let derived = if *samba {
                        vec!["sambaNTPassword".to_string(), "sambaPwdLastSet".to_string()]
                    } else {
                        Vec::new()
                    };
                    WidgetKind::Password(PasswordWidget {
                        primary: attr.clone(),
                        derived,
                        samba: *samba,
                    })
                }
            };
            out.push(ResolvedWidget {
                owner_object_classes: owner.object_classes.clone(),
                attr: attr.clone(),
                kind,
            });
        }
    }
    Ok(out)
}

/// The choice widget for `(entry object classes, attr)`, if any. `.any()` owner
/// objectClass overlap, matching `picker_for`.
pub fn widget_for<'a>(
    widgets: &'a [ResolvedWidget],
    ocs: &[String],
    attr: &str,
) -> Option<&'a WidgetKind> {
    widgets
        .iter()
        .find(|w| {
            w.attr.eq_ignore_ascii_case(attr)
                && w.owner_object_classes
                    .iter()
                    .any(|oc| ocs.iter().any(|e| e.eq_ignore_ascii_case(oc)))
        })
        .map(|w| &w.kind)
}

impl ChoiceWidget {
    /// Parse `value` into the present-token set (format-specific).
    fn parse(&self, value: &str) -> BTreeSet<String> {
        match self.format {
            ChoiceFormat::Plain => {
                if value.trim().is_empty() {
                    BTreeSet::new()
                } else {
                    [value.trim().to_string()].into_iter().collect()
                }
            }
            ChoiceFormat::Bracketed => crate::samba::account::parse_bracketed(value)
                .into_iter()
                .map(|c| c.to_string())
                .collect(),
        }
    }

    /// Serialise a present-token set back to the encoded value.
    fn serialize(&self, set: &BTreeSet<String>) -> String {
        match self.format {
            ChoiceFormat::Plain => set.iter().next().cloned().unwrap_or_default(),
            ChoiceFormat::Bracketed => {
                let chars: BTreeSet<char> = set.iter().filter_map(|s| s.chars().next()).collect();
                crate::samba::account::serialize_bracketed(&chars)
            }
        }
    }

    /// Which option `value`s should be pre-checked when opening the editor over
    /// `current` (the option values whose token is present).
    pub fn seed_checked(&self, current: &str) -> Vec<String> {
        let present = self.parse(current);
        self.options
            .iter()
            .map(|o| o.value.clone())
            .filter(|v| present.contains(v))
            .collect()
    }

    /// Assemble the new encoded value: seed from `current` (lossless — preserves
    /// tokens the UI never surfaced), then set/clear only the configured options
    /// per `checked`. For single-select, `checked` holds at most one value.
    pub fn commit_value(&self, current: &str, checked: &[String]) -> String {
        let mut set = self.parse(current);
        if matches!(self.select, Cardinality::Single) {
            for o in &self.options {
                set.remove(&o.value);
            }
        }
        for o in &self.options {
            if checked.iter().any(|c| c == &o.value) {
                set.insert(o.value.clone());
            } else {
                set.remove(&o.value);
            }
        }
        self.serialize(&set)
    }

    /// Read-only summary: the labels of present options joined with `, `, or the
    /// raw value when nothing matches (off-list plain), or `—` when empty.
    pub fn present_summary(&self, current: &str) -> String {
        let present = self.parse(current);
        let labels: Vec<&str> = self
            .options
            .iter()
            .filter(|o| present.contains(&o.value))
            .map(|o| o.label.as_str())
            .collect();
        if !labels.is_empty() {
            labels.join(", ")
        } else if matches!(self.format, ChoiceFormat::Plain) && !current.trim().is_empty() {
            current.trim().to_string()
        } else {
            "—".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ChoiceOption, EntryProfile, WidgetSpecCfg};

    fn profile_with(attr: &str, select: &str, format: &str, opts: &[(&str, &str)]) -> EntryProfile {
        let mut p = EntryProfile {
            name: "user".into(),
            object_classes: vec!["inetOrgPerson".into()],
            ..Default::default()
        };
        p.widgets.insert(
            attr.into(),
            WidgetSpecCfg::Choice {
                select: select.into(),
                format: format.into(),
                options: opts
                    .iter()
                    .map(|(v, l)| ChoiceOption {
                        value: v.to_string(),
                        label: l.to_string(),
                    })
                    .collect(),
            },
        );
        p
    }

    #[test]
    fn resolves_bracketed_and_plain() {
        let profiles = vec![profile_with(
            "sambaAcctFlags",
            "multi",
            "bracketed",
            &[("D", "Disabled")],
        )];
        let resolved = resolve_widgets(&profiles).expect("ok");
        match widget_for(&resolved, &["inetOrgPerson".into()], "sambaacctflags").unwrap() {
            WidgetKind::Choice(w) => {
                assert_eq!(w.select, crate::config::relation::Cardinality::Multi);
                assert!(matches!(w.format, ChoiceFormat::Bracketed));
            }
            _ => panic!("expected choice widget"),
        }
    }

    #[test]
    fn resolves_password_widget_with_derived() {
        let mut p = EntryProfile {
            name: "user".into(),
            object_classes: vec!["inetOrgPerson".into()],
            ..Default::default()
        };
        p.widgets.insert(
            "userPassword".into(),
            WidgetSpecCfg::Password { samba: true },
        );
        let resolved = resolve_widgets(&[p]).expect("ok");
        match widget_for(&resolved, &["inetOrgPerson".into()], "userPassword").unwrap() {
            WidgetKind::Password(pw) => {
                assert_eq!(pw.primary, "userPassword");
                assert!(pw.samba);
                assert_eq!(
                    pw.derived,
                    vec!["sambaNTPassword".to_string(), "sambaPwdLastSet".to_string()]
                );
            }
            _ => panic!("expected password widget"),
        }
    }

    #[test]
    fn resolves_password_widget_without_samba_has_no_derived() {
        let mut p = EntryProfile {
            name: "u".into(),
            object_classes: vec!["inetOrgPerson".into()],
            ..Default::default()
        };
        p.widgets.insert(
            "userPassword".into(),
            WidgetSpecCfg::Password { samba: false },
        );
        let resolved = resolve_widgets(&[p]).unwrap();
        match widget_for(&resolved, &["inetOrgPerson".into()], "userPassword").unwrap() {
            WidgetKind::Password(pw) => assert!(pw.derived.is_empty()),
            _ => panic!("expected password widget"),
        }
    }

    #[test]
    fn rejects_empty_options_and_unknown_format() {
        let p_empty = profile_with("a", "single", "plain", &[]);
        assert!(resolve_widgets(&[p_empty]).is_err());
        let p_bad = profile_with("a", "single", "nope", &[("x", "X")]);
        assert!(resolve_widgets(&[p_bad]).is_err());
    }

    #[test]
    fn rejects_multi_plain() {
        assert!(resolve_widgets(&[profile_with("a", "multi", "plain", &[("x", "X")])]).is_err());
    }

    #[test]
    fn reserved_formats_error_until_wired() {
        let p = profile_with("a", "multi", "bitmask", &[("x", "X")]);
        assert!(resolve_widgets(&[p]).is_err());
    }

    #[test]
    fn bracketed_commit_merges_from_original_and_preserves_unmanaged() {
        let w = ChoiceWidget {
            select: crate::config::relation::Cardinality::Multi,
            format: ChoiceFormat::Bracketed,
            options: vec![
                ChoiceOption {
                    value: "D".into(),
                    label: "Disabled".into(),
                },
                ChoiceOption {
                    value: "X".into(),
                    label: "No expire".into(),
                },
            ],
        };
        let checked = w.seed_checked("[UW         ]");
        assert!(checked.is_empty(), "neither D nor X set originally");
        let v = w.commit_value("[UW         ]", &["D".to_string()]);
        assert_eq!(v, "[DUW        ]");
    }

    #[test]
    fn plain_commit_replaces_value_and_summarises() {
        let w = ChoiceWidget {
            select: crate::config::relation::Cardinality::Single,
            format: ChoiceFormat::Plain,
            options: vec![
                ChoiceOption {
                    value: "/bin/bash".into(),
                    label: "Bash".into(),
                },
                ChoiceOption {
                    value: "/bin/sh".into(),
                    label: "POSIX sh".into(),
                },
            ],
        };
        assert_eq!(w.seed_checked("/bin/sh"), vec!["/bin/sh".to_string()]);
        assert_eq!(
            w.commit_value("/bin/bash", &["/bin/sh".to_string()]),
            "/bin/sh"
        );
        assert_eq!(w.present_summary("/bin/sh"), "POSIX sh");
        assert_eq!(w.present_summary("/bin/zsh"), "/bin/zsh");
    }

    #[test]
    fn bracketed_summary_joins_set_labels() {
        let w = ChoiceWidget {
            select: crate::config::relation::Cardinality::Multi,
            format: ChoiceFormat::Bracketed,
            options: vec![
                ChoiceOption {
                    value: "D".into(),
                    label: "Disabled".into(),
                },
                ChoiceOption {
                    value: "X".into(),
                    label: "No expire".into(),
                },
            ],
        };
        assert_eq!(w.present_summary("[DU         ]"), "Disabled");
        assert_eq!(w.present_summary("[U          ]"), "—");
    }
}
