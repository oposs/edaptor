//! Rule B1 (spec §2B1): templated and fixed defaults inferred from the sample.

use std::collections::{BTreeMap, BTreeSet};

use crate::config::defaults::{parse_default_value, resolve_template, DefaultValue, Seg};
use crate::detect::model::{Detected, Evidence, SampleEntry};
use crate::detect::{most_common, rule_applies, MIN_SAMPLE};
use crate::schema::SchemaModel;

/// Candidate templates, tried in order; the first one the majority follows wins
/// for its attribute. `homeDirectory` is handled by prefix detection.
const TEMPLATES: &[(&str, &str)] = &[
    ("uid", "{cn}"),
    ("cn", "{uid}"),
    ("cn", "{givenName} {sn}"),
    ("displayName", "{givenName} {sn}"),
    ("gecos", "{givenName} {sn}"),
];

/// Never inferred as a literal default: membership attributes, attributes
/// unique per entry, and attributes another rule owns (`gidNumber`: B2/B3).
pub const NEVER_LITERAL: &[&str] = &[
    "objectClass",
    "memberUid",
    "member",
    "uniqueMember",
    "memberOf",
    "uidNumber",
    "gidNumber",
    "mail",
    "sambaSID",
    "uid",
    "cn",
    "sn",
    "givenName",
    "displayName",
    "gecos",
    "userPassword",
];

/// B1 over `entries` (the profile's sampled entries, or its non-private groups).
/// Returns the defaults and notes about templates dropped by the cycle rule.
pub fn infer_defaults(
    schema: &SchemaModel,
    entries: &[SampleEntry],
    rdn_attr: &str,
) -> (BTreeMap<String, Detected<DefaultValue>>, Vec<String>) {
    let mut out: BTreeMap<String, Detected<DefaultValue>> = BTreeMap::new();
    if entries.len() < MIN_SAMPLE {
        return (out, Vec::new());
    }
    for (attr, tmpl) in TEMPLATES {
        if out.keys().any(|k| k.eq_ignore_ascii_case(attr)) {
            continue;
        }
        if let Some(d) = test_template(entries, attr, tmpl) {
            out.insert(attr.to_string(), d);
        }
    }
    if let Some(d) = home_directory(entries) {
        out.insert("homeDirectory".to_string(), d);
    }
    let dropped = break_cycles(&mut out, rdn_attr);
    for attr in literal_candidates(schema, entries, rdn_attr) {
        if out.keys().any(|k| k.eq_ignore_ascii_case(&attr)) {
            continue;
        }
        if let Some(d) = common_literal(entries, &attr) {
            out.insert(attr, d);
        }
    }
    (out, dropped)
}

/// `Some` when the majority of `entries` hold exactly the value `tmpl` yields.
fn test_template(
    entries: &[SampleEntry],
    attr: &str,
    tmpl: &str,
) -> Option<Detected<DefaultValue>> {
    let Ok(DefaultValue::Template(segs)) = parse_default_value(tmpl) else {
        return None;
    };
    let exceptions: Vec<String> = entries
        .iter()
        .filter(|e| {
            let want = e.first(attr);
            want.is_none() || resolve_template(&segs, &e.attrs).as_deref() != want
        })
        .map(|e| e.dn.clone())
        .collect();
    let matched = entries.len() - exceptions.len();
    rule_applies(matched, entries.len()).then(|| {
        Detected::new(
            DefaultValue::Template(segs),
            Evidence::new(matched, entries.len()).with_exceptions(exceptions),
        )
    })
}

/// `homeDirectory = "<P>{uid}"` for the most common fixed prefix `P`.
fn home_directory(entries: &[SampleEntry]) -> Option<Detected<DefaultValue>> {
    let prefixes: Vec<&str> = entries
        .iter()
        .filter_map(|e| {
            let (h, u) = (e.first("homeDirectory")?, e.first("uid")?);
            h.strip_suffix(u)
        })
        .collect();
    let (prefix, _) = most_common(prefixes)?;
    if prefix.is_empty() || prefix.contains('{') {
        return None;
    }
    test_template(entries, "homeDirectory", &format!("{prefix}{{uid}}"))
}

fn template_sources(dv: &DefaultValue) -> Vec<String> {
    match dv {
        DefaultValue::Template(segs) => segs
            .iter()
            .filter_map(|s| match s {
                Seg::Field(f) => Some(f.to_lowercase()),
                Seg::Lit(_) => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Templates must not feed each other: of such a pair keep only the one whose
/// source is `rdn_attr` (neither → drop both). Returns one note per drop.
fn break_cycles(out: &mut BTreeMap<String, Detected<DefaultValue>>, rdn_attr: &str) -> Vec<String> {
    let keys: Vec<String> = out.keys().cloned().collect();
    let mut drop: BTreeSet<String> = BTreeSet::new();
    for a in &keys {
        for b in &keys {
            if a >= b {
                continue;
            }
            let (sa, sb) = (
                template_sources(&out[a].value),
                template_sources(&out[b].value),
            );
            if sa.contains(&b.to_lowercase()) && sb.contains(&a.to_lowercase()) {
                let rdn = rdn_attr.to_lowercase();
                if !sa.contains(&rdn) {
                    drop.insert(a.clone());
                }
                if !sb.contains(&rdn) {
                    drop.insert(b.clone());
                }
            }
        }
    }
    drop.into_iter()
        .filter_map(|k| {
            let d = out.remove(&k)?;
            Some(format!(
                "dropped detected default {k} = \"{}\": it and another detected template feed each other",
                d.value.to_config_string()
            ))
        })
        .collect()
}

/// Sampled single-valued attributes that may carry a literal default.
fn literal_candidates(
    schema: &SchemaModel,
    entries: &[SampleEntry],
    rdn_attr: &str,
) -> Vec<String> {
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    for e in entries {
        for k in e.attrs.keys() {
            seen.entry(k.to_lowercase()).or_insert_with(|| k.clone());
        }
    }
    seen.into_values()
        .filter(|a| !a.eq_ignore_ascii_case(rdn_attr))
        .filter(|a| !NEVER_LITERAL.iter().any(|n| n.eq_ignore_ascii_case(a)))
        .filter(|a| schema.is_single_value(a))
        .collect()
}

fn common_literal(entries: &[SampleEntry], attr: &str) -> Option<Detected<DefaultValue>> {
    let (value, matched) = most_common(entries.iter().filter_map(|e| e.first(attr)))?;
    if !rule_applies(matched, entries.len()) {
        return None;
    }
    let exceptions = entries
        .iter()
        .filter(|e| e.first(attr) != Some(value.as_str()))
        .map(|e| e.dn.clone())
        .collect();
    Some(Detected::new(
        DefaultValue::Literal(value),
        Evidence::new(matched, entries.len()).with_exceptions(exceptions),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::fixtures::{argus_sample, demo_sample, e, schema};

    fn entries_of(sample: &crate::detect::model::Sample, container: &str) -> Vec<SampleEntry> {
        sample
            .containers
            .iter()
            .find(|c| c.dn == container)
            .unwrap()
            .entries
            .clone()
    }

    fn cfg(d: &BTreeMap<String, Detected<DefaultValue>>, attr: &str) -> Option<String> {
        d.get(attr).map(|v| v.value.to_config_string())
    }

    #[test]
    fn argus_keeps_the_template_fed_by_the_rdn_and_drops_its_mirror() {
        let users = entries_of(&argus_sample(), "ou=people,dc=argus,dc=ch");
        let (d, dropped) = infer_defaults(&schema(), &users, "cn");
        assert_eq!(cfg(&d, "uid").as_deref(), Some("{cn}"));
        assert_eq!(d["uid"].evidence.ratio(), "12/12");
        assert!(
            !d.contains_key("cn"),
            "cn = {{uid}} must be dropped (cycle)"
        );
        assert_eq!(dropped.len(), 1);
        assert!(dropped[0].contains("cn"), "{}", dropped[0]);
        assert_eq!(cfg(&d, "homeDirectory").as_deref(), Some("/home/{uid}"));
        assert_eq!(cfg(&d, "loginShell").as_deref(), Some("/bin/bash"));
        assert_eq!(d["loginShell"].evidence.ratio(), "11/12");
        assert_eq!(
            d["loginShell"].evidence.exceptions,
            vec!["cn=u12,ou=people,dc=argus,dc=ch"]
        );
    }

    #[test]
    fn demo_people_get_cn_from_given_name_and_sn() {
        let people = entries_of(&demo_sample(), "ou=people,dc=example,dc=org");
        let (d, _) = infer_defaults(&schema(), &people, "uid");
        assert_eq!(cfg(&d, "cn").as_deref(), Some("{givenName} {sn}"));
        assert!(!d.contains_key("uid"));
        // gidNumber is shared (100) but belongs to rule B3, never B1.
        assert!(!d.contains_key("gidNumber"));
        assert!(!d.contains_key("sambaSID"));
        assert!(!d.contains_key("mail"));
    }

    fn user(i: u32, shell: &str, home: &str) -> SampleEntry {
        let dn = format!("uid=t{i},ou=p,dc=x");
        let uid = format!("t{i}");
        e(
            &dn,
            &[
                ("objectClass", &["inetOrgPerson", "posixAccount"]),
                ("uid", &[uid.as_str()]),
                ("loginShell", &[shell]),
                ("homeDirectory", &[home]),
                ("memberUid", &["admin"]),
                ("description", &["same"]),
            ],
        )
    }

    #[test]
    fn untidy_directory_applies_with_exceptions() {
        let mut v: Vec<SampleEntry> = (1..=10)
            .map(|i| user(i, "/bin/zsh", &format!("/srv/home/t{i}")))
            .collect();
        v.push(user(11, "/bin/sh", "/tmp/x"));
        v.push(user(12, "/bin/sh", "/tmp/y"));
        let (d, _) = infer_defaults(&schema(), &v, "uid");
        assert_eq!(cfg(&d, "loginShell").as_deref(), Some("/bin/zsh"));
        assert_eq!(d["loginShell"].evidence.ratio(), "10/12");
        assert_eq!(d["loginShell"].evidence.exceptions.len(), 2);
        assert_eq!(cfg(&d, "homeDirectory").as_deref(), Some("/srv/home/{uid}"));
        // Membership and multi-valued attributes never become literals.
        assert!(!d.contains_key("memberUid"));
        assert!(!d.contains_key("description"));
    }

    #[test]
    fn below_threshold_nothing_is_inferred() {
        let v = vec![
            user(1, "/bin/zsh", "/home/t1"),
            user(2, "/bin/zsh", "/home/t2"),
        ];
        let (d, _) = infer_defaults(&schema(), &v, "uid");
        assert!(d.is_empty(), "{d:?}");
    }
}
