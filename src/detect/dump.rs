//! `edaptor profiles` output: the merged profiles as valid TOML, each value
//! followed by a provenance comment (spec §3 "edaptor profiles"). Pure.

use std::collections::BTreeMap;

use crate::config::defaults::DefaultValue;
use crate::config::{CandidateRef, ChoiceOption, EntryProfile, WidgetSpecCfg};
use crate::detect::merge::{Origin, Provenance, Source};
use crate::detect::model::{Evidence, SampleEntry};
use crate::detect::range::{detect_range, RangeReport};

#[derive(Debug, Clone)]
pub struct RangeOutcome {
    pub result: Result<RangeReport, String>,
    /// The number scan was truncated by a server limit.
    pub uncertain: bool,
}

pub fn compute_ranges(
    profiles: &[EntryProfile],
    scan: &[SampleEntry],
    truncated: bool,
) -> BTreeMap<(String, String), RangeOutcome> {
    let mut out = BTreeMap::new();
    for p in profiles {
        for (attr, dv) in &p.defaults.entries {
            if let DefaultValue::DetectedRange(spec) = dv {
                out.insert(
                    (p.name.to_lowercase(), attr.to_lowercase()),
                    RangeOutcome {
                        result: detect_range(spec, scan),
                        uncertain: truncated,
                    },
                );
            }
        }
    }
    out
}

pub fn failed_ranges(
    profiles: &[EntryProfile],
    msg: &str,
) -> BTreeMap<(String, String), RangeOutcome> {
    let mut out = BTreeMap::new();
    for p in profiles {
        for (attr, dv) in &p.defaults.entries {
            if matches!(dv, DefaultValue::DetectedRange(_)) {
                out.insert(
                    (p.name.to_lowercase(), attr.to_lowercase()),
                    RangeOutcome {
                        result: Err(msg.to_string()),
                        uncertain: false,
                    },
                );
            }
        }
    }
    out
}

pub fn header_line(enabled: bool, containers: usize, notes: usize) -> String {
    if !enabled {
        return "# detection: disabled ([detect] enabled = false)".to_string();
    }
    let notes = if notes == 0 {
        "none".to_string()
    } else {
        format!("{notes} (printed on stderr)")
    };
    format!(
        "# detection: {containers} containers sampled (up to {} entries each); notes: {notes}",
        crate::detect::SAMPLE_SIZE
    )
}

fn q(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

fn arr(v: &[String]) -> String {
    toml::Value::Array(v.iter().map(|s| toml::Value::String(s.clone())).collect()).to_string()
}

fn key(k: &str) -> String {
    if !k.is_empty()
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        k.to_string()
    } else {
        q(k)
    }
}

fn line(out: &mut String, k: &str, v: &str, comment: &str) {
    if comment.is_empty() {
        out.push_str(&format!("{k} = {v}\n"));
    } else {
        out.push_str(&format!("{k} = {v}  {comment}\n"));
    }
}

fn ev_text(ev: &Evidence) -> String {
    match &ev.note {
        Some(n) => format!("{} {n}", ev.ratio()),
        None => ev.ratio(),
    }
}

fn src(prov: &Provenance, k: &str) -> String {
    match prov.fields.get(k) {
        None => String::new(),
        Some(Source::Config) => "# config".to_string(),
        Some(Source::Assumed(reason)) => format!("# assumed: {reason}"),
        Some(Source::Detected(ev)) => format!("# detected: {}", ev_text(ev)),
        Some(Source::ConfigOverDetected { detected, evidence }) => {
            format!("# config (detected {detected}, {})", evidence.ratio())
        }
    }
}

fn origin_comment(o: &Origin) -> String {
    let partial = |p: bool| if p { " (partial sample)" } else { "" };
    match o {
        Origin::Config => "# config".to_string(),
        Origin::Detected {
            container,
            sampled,
            partial: p,
        } => format!(
            "# detected: {sampled} entries in {container}{}",
            partial(*p)
        ),
        Origin::Merged {
            detected,
            container,
            sampled,
            partial: p,
        } => {
            format!(
                "# config, merged with detected {}: {sampled} entries in {container}{}",
                q(detected),
                partial(*p)
            )
        }
    }
}

fn list(dns: &[String]) -> String {
    let mut s = dns.iter().take(5).cloned().collect::<Vec<_>>().join("; ");
    if dns.len() > 5 {
        s.push_str(&format!(" (+{} more)", dns.len() - 5));
    }
    s
}

fn options(opts: &[ChoiceOption]) -> String {
    let items: Vec<String> = opts
        .iter()
        .map(|o| format!("{{ value = {}, label = {} }}", q(&o.value), q(&o.label)))
        .collect();
    format!("[{}]", items.join(", "))
}

fn candidate(c: &CandidateRef) -> String {
    match c {
        CandidateRef::Profile(n) => q(n),
        CandidateRef::Inline(s) => {
            let label = s
                .label
                .as_ref()
                .map(|l| format!(", label = {}", q(l)))
                .unwrap_or_default();
            format!(
                "{{ base = {}, object_classes = {}, search_attrs = {}{label} }}",
                q(&s.base),
                arr(&s.object_classes),
                arr(&s.search_attrs)
            )
        }
    }
}

/// `(key, TOML value)` lines of one `[profile.widget.<attr>]` table.
pub fn widget_lines(spec: &WidgetSpecCfg) -> Vec<(String, String)> {
    let kv = |k: &str, v: String| (k.to_string(), v);
    match spec {
        WidgetSpecCfg::Choice {
            select,
            format,
            options: o,
        } => vec![
            kv("kind", q("choice")),
            kv("select", q(select)),
            kv("format", q(format)),
            kv("options", options(o)),
        ],
        WidgetSpecCfg::Password { samba } => {
            let mut v = vec![kv("kind", q("password"))];
            if *samba {
                v.push(kv("samba", "true".to_string()));
            }
            v
        }
        WidgetSpecCfg::Picker {
            candidate: c,
            store,
            select,
        } => vec![
            kv("kind", q("picker")),
            kv("candidate", candidate(c)),
            kv("store", q(store)),
            kv("select", q(select)),
        ],
        WidgetSpecCfg::Membership { candidate: c, via } => vec![
            kv("kind", q("membership")),
            kv("candidate", candidate(c)),
            kv("via", q(via)),
        ],
        WidgetSpecCfg::Lookup {
            candidate: c,
            store,
            label,
        } => {
            let mut v = vec![
                kv("kind", q("lookup")),
                kv("candidate", candidate(c)),
                kv("store", q(store)),
            ];
            if let Some(l) = label {
                v.push(kv("label", q(l)));
            }
            v
        }
        WidgetSpecCfg::Readonly => vec![kv("kind", q("readonly"))],
        WidgetSpecCfg::XOrdered => vec![kv("kind", q("x_ordered"))],
        WidgetSpecCfg::SambaSid => vec![kv("kind", q("samba_sid"))],
    }
}

pub fn render(
    profiles: &[EntryProfile],
    provenance: &[Provenance],
    disabled: &[String],
    header: &str,
    ranges: &BTreeMap<(String, String), RangeOutcome>,
) -> String {
    let mut out = String::new();
    out.push_str(header);
    out.push('\n');
    for name in disabled {
        out.push_str(&format!(
            "# profile {} disabled by config (enabled = false)\n",
            q(name)
        ));
    }
    for (p, prov) in profiles.iter().zip(provenance) {
        out.push_str("\n[[profile]]\n");
        line(&mut out, "name", &q(&p.name), &origin_comment(&prov.origin));
        line(
            &mut out,
            "object_classes",
            &arr(&p.object_classes),
            &src(prov, "object_classes"),
        );
        if !p.rdn_attr.is_empty() {
            line(
                &mut out,
                "rdn_attr",
                &q(&p.rdn_attr),
                &src(prov, "rdn_attr"),
            );
        }
        if !p.search_base.is_empty() {
            line(
                &mut out,
                "search_base",
                &q(&p.search_base),
                &src(prov, "search_base"),
            );
        }
        if !p.show.is_empty() {
            line(&mut out, "show", &arr(&p.show), &src(prov, "show"));
        }
        if !p.search_attrs.is_empty() {
            line(
                &mut out,
                "search_attrs",
                &arr(&p.search_attrs),
                &src(prov, "search_attrs"),
            );
        }
        if let Some(l) = &p.label {
            line(&mut out, "label", &q(l), &src(prov, "label"));
        }
        let mut trailer: Vec<String> = Vec::new();
        if !p.defaults.entries.is_empty() {
            out.push_str("[profile.defaults]\n");
            for (attr, dv) in &p.defaults.entries {
                let comment = src(prov, &format!("defaults.{attr}"));
                match dv {
                    DefaultValue::DetectedRange(_) => {
                        match ranges.get(&(p.name.to_lowercase(), attr.to_lowercase())) {
                            Some(RangeOutcome {
                                result: Ok(r),
                                uncertain,
                            }) => {
                                let unc = if *uncertain {
                                    "; uncertain (the number scan hit a server limit)"
                                } else {
                                    ""
                                };
                                let lead = match prov.fields.get(&format!("defaults.{attr}")) {
                                    Some(Source::Assumed(reason)) => {
                                        format!("# assumed: {reason}; at dump time: ")
                                    }
                                    _ => "# detected at dump time: ".to_string(),
                                };
                                line(
                                    &mut out,
                                    &key(attr),
                                    &q(&r.template()),
                                    &format!("{lead}{}{unc}", r.describe()),
                                );
                                if !r.evidence.exceptions.is_empty() {
                                    trailer.push(format!(
                                        "# exceptions (defaults.{attr} range): {}",
                                        list(&r.evidence.exceptions)
                                    ));
                                }
                            }
                            Some(RangeOutcome { result: Err(e), .. }) => {
                                out.push_str(&format!("# {attr} = (no range detected: {e})\n"))
                            }
                            None => out
                                .push_str(&format!("# {attr} = (range detected at create time)\n")),
                        }
                    }
                    other => line(
                        &mut out,
                        &key(attr),
                        &q(&other.to_config_string()),
                        &comment,
                    ),
                }
            }
        }
        for (attr, spec) in &p.widgets {
            out.push_str(&format!("[profile.widget.{}]\n", key(attr)));
            for (i, (k, v)) in widget_lines(spec).into_iter().enumerate() {
                let c = if i == 0 {
                    src(prov, &format!("widget.{attr}"))
                } else {
                    String::new()
                };
                line(&mut out, &k, &v, &c);
            }
        }
        if let Some(c) = &p.companion {
            out.push_str("[profile.companion]\n");
            line(
                &mut out,
                "object_classes",
                &arr(&c.object_classes),
                &src(prov, "companion"),
            );
            line(&mut out, "rdn_attr", &q(&c.rdn_attr), "");
            line(&mut out, "search_base", &q(&c.search_base), "");
            if !c.attributes.is_empty() {
                out.push_str("[profile.companion.attributes]\n");
                for (k, v) in &c.attributes {
                    line(&mut out, &key(k), &q(v), "");
                }
            }
        }
        for (field, s) in &prov.fields {
            let ev = match s {
                Source::Detected(ev) | Source::ConfigOverDetected { evidence: ev, .. } => ev,
                Source::Config | Source::Assumed(_) => continue,
            };
            if !ev.exceptions.is_empty() {
                out.push_str(&format!(
                    "# exceptions ({field}): {}\n",
                    list(&ev.exceptions)
                ));
            }
        }
        for t in trailer {
            out.push_str(&t);
            out.push('\n');
        }
        for s in &prov.suppressed {
            out.push_str(&format!("# suppressed by config: {s}\n"));
        }
        for n in &prov.notes {
            out.push_str(&format!("# note: {n}\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::fixtures::{argus_sample, overrides, schema};

    fn argus_dump() -> String {
        let s = argus_sample();
        let d = crate::detect::infer::detect(&schema(), &s);
        let mut m = crate::detect::merge::merge(&schema(), &d.profiles, &[]);
        crate::detect::merge::validate(&mut m, None).unwrap();
        let scan: Vec<SampleEntry> = s
            .containers
            .iter()
            .flat_map(|c| c.entries.clone())
            .collect();
        let ranges = compute_ranges(&m.profiles, &scan, false);
        render(
            &m.profiles,
            &m.provenance,
            &m.disabled,
            &header_line(true, s.containers.len(), 0),
            &ranges,
        )
    }

    #[test]
    fn dump_contains_values_and_provenance() {
        let t = argus_dump();
        assert!(
            t.starts_with(
                "# detection: 3 containers sampled (up to 200 entries each); notes: none\n"
            ),
            "{t}"
        );
        assert!(t.contains(
            "name = \"user-people\"  # detected: 12 entries in ou=people,dc=argus,dc=ch"
        ));
        assert!(t.contains("uid = \"{cn}\"  # detected: 12/12"));
        assert!(t.contains("gidNumber = \"{uidNumber}\"  # detected: 12/12 have a private group"));
        assert!(t.contains("uidNumber = \"{next:5000-7999}\"  # detected at dump time: in use 5000-5020; next block at 8000"));
        assert!(t.contains("gidNumber = \"{next:8000-60000}\""));
        assert!(t.contains("# exceptions (defaults.loginShell): cn=u12,ou=people,dc=argus,dc=ch"));
    }

    #[test]
    fn dump_never_contains_secrets() {
        let t = argus_dump().to_lowercase();
        assert!(!t.contains("userpassword"), "{t}");
        assert!(!t.contains("{ssha}") && !t.contains("ntpassword"), "{t}");
    }

    #[test]
    fn dump_is_valid_toml_that_parses_back_as_overrides() {
        let t = argus_dump();
        let cfg: crate::config::Config = toml::from_str(&format!(
            "[server]\nuri = \"ldap://x\"\nbase_dn = \"dc=argus,dc=ch\"\n[auth]\nbind_dn = \"cn=a\"\n{t}"
        ))
        .expect("the dump must be pasteable");
        assert!(cfg.overrides.iter().any(|o| o.name == "user-people"));
    }

    #[test]
    fn suppressed_parts_and_disabled_profiles_are_commented() {
        let s = argus_sample();
        let d = crate::detect::infer::detect(&schema(), &s);
        let o = overrides("[[profile]]\nname = \"user-people\"\nsuppress = [\"widget.gidNumber\"]\n[profile.defaults]\nloginShell = \"/bin/sh\"\n[[profile]]\nname = \"posixgroup-groups\"\nenabled = false\n");
        let m = crate::detect::merge::merge(&schema(), &d.profiles, &o);
        let t = render(
            &m.profiles,
            &m.provenance,
            &m.disabled,
            "# h",
            &BTreeMap::new(),
        );
        assert!(t.contains("# suppressed by config: widget.gidNumber"));
        assert!(t.contains("loginShell = \"/bin/sh\"  # config (detected \"/bin/bash\", 11/12)"));
        assert!(t.contains("# profile \"posixgroup-groups\" disabled by config (enabled = false)"));
    }

    #[test]
    fn assumed_values_carry_their_reason() {
        let o = overrides("[[profile]]\nname = \"user\"\nobject_classes = [\"inetOrgPerson\", \"posixAccount\"]\nsearch_base = \"ou=people,dc=x\"\n");
        let m = crate::detect::assume::merge_with_assumptions(
            &schema(),
            &[],
            &o,
            Some("ou=groups,dc=x"),
            false,
        );
        let ranges = compute_ranges(&m.profiles, &[], false);
        let t = render(
            &m.profiles,
            &m.provenance,
            &m.disabled,
            &header_line(true, 0, 0),
            &ranges,
        );
        assert!(
            t.contains(
                "gidNumber = \"{uidNumber}\"  # assumed: no users yet; useradd-style private group"
            ),
            "{t}"
        );
        assert!(t.contains("uidNumber = \"{next:10000-60000}\"  # assumed: no uidNumber range configured or detected; useradd-style numbering; at dump time: no numbers in use; useradd-style start at 10000"), "{t}");
        assert!(t.contains("object_classes = [\"posixGroup\"]  # assumed: no users yet; useradd-style private group"), "{t}");
    }

    #[test]
    fn argus_golden() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/detect/testdata/argus-profiles.toml"
        );
        let t = argus_dump();
        if std::env::var_os("EDAPTOR_UPDATE_GOLDEN").is_some() {
            std::fs::write(path, &t).unwrap();
        }
        let golden = std::fs::read_to_string(path).expect("run once with EDAPTOR_UPDATE_GOLDEN=1");
        assert_eq!(
            t, golden,
            "dump format changed; review and regenerate with EDAPTOR_UPDATE_GOLDEN=1"
        );
    }
}
