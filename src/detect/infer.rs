//! Detect profiles from a sample (spec §2A): group each container's entries by
//! structural class; derive object classes, RDN attribute, show, search
//! attributes and label; assign names. Pure.

use std::collections::BTreeMap;

use crate::config::WidgetSpecCfg;
use crate::detect::model::{
    ContainerSample, Detected, DetectedProfile, Evidence, Sample, SampleEntry,
};
use crate::detect::{more_than_half, most_common, names};
use crate::schema::{FieldKind, SchemaModel};

/// The detector's output.
#[derive(Debug, Clone, Default)]
pub struct Detection {
    pub profiles: Vec<DetectedProfile>,
    pub notes: Vec<String>,
}

/// Detect profiles from `sample` (see module docs).
pub fn detect(schema: &SchemaModel, sample: &Sample) -> Detection {
    let mut notes = Vec::new();
    let mut profiles = Vec::new();
    for c in &sample.containers {
        profiles.extend(group_container(schema, c, &mut notes));
    }
    names::assign_names(&mut profiles);
    crate::detect::patterns::apply(schema, sample, &mut profiles, &mut notes);
    Detection { profiles, notes }
}

/// One profile per structural class present in container `c`.
fn group_container(
    schema: &SchemaModel,
    c: &ContainerSample,
    notes: &mut Vec<String>,
) -> Vec<DetectedProfile> {
    let mut groups: BTreeMap<String, (String, Vec<&SampleEntry>)> = BTreeMap::new();
    let mut unclassified = 0usize;
    for e in &c.entries {
        match schema.structural_class(e.values("objectClass")) {
            Some(s) => groups
                .entry(s.to_lowercase())
                .or_insert_with(|| (s.clone(), Vec::new()))
                .1
                .push(e),
            None => unclassified += 1,
        }
    }
    if unclassified > 0 {
        notes.push(format!(
            "{}: skipped {unclassified} entr{} without a known structural object class",
            c.dn,
            if unclassified == 1 { "y" } else { "ies" }
        ));
    }
    groups
        .into_values()
        .map(|(structural, entries)| build_profile(schema, c, &structural, &entries))
        .collect()
}

fn build_profile(
    schema: &SchemaModel,
    c: &ContainerSample,
    structural: &str,
    entries: &[&SampleEntry],
) -> DetectedProfile {
    let n = entries.len();
    let object_classes = majority_classes(structural, entries);
    let rdn_attr = common_rdn_attr(entries);
    let presence = |attr: &str| {
        let key = attr.to_lowercase();
        entries
            .iter()
            .filter(|e| c.present_attrs(e).contains(&key))
            .count()
    };
    let show = show_list(schema, &object_classes.value, &rdn_attr.value, n, &presence);
    let mut search_attrs = vec![rdn_attr.value.clone()];
    for a in ["cn", "uid", "sn", "mail", "description"] {
        if more_than_half(presence(a), n) && !search_attrs.iter().any(|s| s.eq_ignore_ascii_case(a))
        {
            search_attrs.push(a.to_string());
        }
    }
    let differ = entries
        .iter()
        .filter(|e| match (e.first("cn"), e.first("uid")) {
            (Some(cn), Some(uid)) => cn != uid,
            _ => false,
        })
        .count();
    let label = if more_than_half(differ, n) {
        "{cn} ({uid})".to_string()
    } else {
        format!("{{{}}}", rdn_attr.value)
    };
    DetectedProfile {
        name: String::new(),
        base_name: names::base_name(structural, &object_classes.value),
        structural: structural.to_string(),
        container: c.dn.clone(),
        sampled: n,
        partial: c.partial,
        object_classes,
        rdn_attr,
        show,
        search_attrs,
        label: Some(label),
        defaults: BTreeMap::new(),
        widgets: BTreeMap::new(),
        companion: None,
        assumed: BTreeMap::new(),
        private_groups: false,
        users_without_private_group: None,
        notes: Vec::new(),
        entries: entries.iter().map(|e| (*e).clone()).collect(),
    }
}

/// Classes (except `top`) present in more than half the group; the structural
/// class first, then by frequency, then by name.
fn majority_classes(structural: &str, entries: &[&SampleEntry]) -> Detected<Vec<String>> {
    let n = entries.len();
    let mut counts: BTreeMap<String, (String, usize)> = BTreeMap::new();
    for e in entries {
        for oc in e.values("objectClass") {
            let oc = oc.trim();
            if oc.eq_ignore_ascii_case("top") || oc.is_empty() {
                continue;
            }
            counts
                .entry(oc.to_lowercase())
                .or_insert_with(|| (oc.to_string(), 0))
                .1 += 1;
        }
    }
    let mut chosen: Vec<(String, usize)> = counts
        .into_values()
        .filter(|(_, k)| more_than_half(*k, n))
        .collect();
    chosen.sort_by(|a, b| {
        let a_s = a.0.eq_ignore_ascii_case(structural);
        let b_s = b.0.eq_ignore_ascii_case(structural);
        b_s.cmp(&a_s)
            .then(b.1.cmp(&a.1))
            .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });
    let mut value: Vec<String> = chosen.into_iter().map(|(name, _)| name).collect();
    // Use the schema spelling of the structural class.
    if let Some(first) = value.first_mut() {
        if first.eq_ignore_ascii_case(structural) {
            *first = structural.to_string();
        }
    }
    let exceptions: Vec<String> = entries
        .iter()
        .filter(|e| !value.iter().all(|oc| e.has_class(oc)))
        .map(|e| e.dn.clone())
        .collect();
    let matched = n - exceptions.len();
    Detected::new(value, Evidence::new(matched, n).with_exceptions(exceptions))
}

/// The most common RDN attribute of the group.
fn common_rdn_attr(entries: &[&SampleEntry]) -> Detected<String> {
    let n = entries.len();
    let (attr, matched) =
        most_common(entries.iter().filter_map(|e| e.rdn_attr())).unwrap_or(("cn".to_string(), 0));
    let exceptions = entries
        .iter()
        .filter(|e| !e.rdn_attr().is_some_and(|a| a.eq_ignore_ascii_case(&attr)))
        .map(|e| e.dn.clone())
        .collect();
    Detected::new(attr, Evidence::new(matched, n).with_exceptions(exceptions))
}

/// Identity attributes a password field follows in `show` (with `rdn_attr`).
const IDENTITY_ATTRS: &[&str] = &["uid", "cn", "sn", "givenName", "displayName"];

/// Classes whose profiles get the built-in password field: accounts, not
/// every `person` (a password policy entry is one too).
const ACCOUNT_CLASSES: &[&str] = &["posixAccount", "inetOrgPerson", "sambaSamAccount"];

/// Lowercased attributes the effective widgets turn into a password field: the
/// built-in bundle for account classes (classes in name order, the last match
/// wins, as in the resolver), overlaid by the profile's own `widgets`.
pub(crate) fn password_fields<'a>(
    object_classes: &[String],
    widgets: impl IntoIterator<Item = (&'a String, &'a WidgetSpecCfg)>,
) -> Vec<String> {
    let mut kinds: BTreeMap<String, bool> = BTreeMap::new();
    let account = object_classes
        .iter()
        .any(|c| ACCOUNT_CLASSES.iter().any(|a| a.eq_ignore_ascii_case(c)));
    if account {
        let mut classes: Vec<String> = object_classes.iter().map(|c| c.to_lowercase()).collect();
        classes.sort();
        let bundle = crate::config::builtin::builtin_schema();
        for oc in &classes {
            for (attr, spec) in bundle.get(oc).into_iter().flatten() {
                kinds.insert(attr.clone(), matches!(spec, WidgetSpecCfg::Password { .. }));
            }
        }
    }
    for (attr, spec) in widgets {
        kinds.insert(
            attr.to_lowercase(),
            matches!(spec, WidgetSpecCfg::Password { .. }),
        );
    }
    kinds
        .into_iter()
        .filter(|(_, pw)| *pw)
        .map(|(a, _)| a)
        .collect()
}

/// Put every attribute of `passwords` (lowercased) the classes allow right
/// after the last identity attribute of `show` (else after the first
/// entry), moving it there when `show` already lists it.
pub(crate) fn place_password_fields(
    schema: &SchemaModel,
    show: &mut Vec<String>,
    object_classes: &[String],
    rdn_attr: &str,
    passwords: &[String],
) {
    show.retain(|a| !passwords.contains(&a.to_lowercase()));
    let mut at = show
        .iter()
        .rposition(|a| {
            a.eq_ignore_ascii_case(rdn_attr)
                || IDENTITY_ATTRS.iter().any(|n| n.eq_ignore_ascii_case(a))
        })
        .map_or(show.len().min(1), |i| i + 1);
    let refs: Vec<&str> = object_classes.iter().map(String::as_str).collect();
    let resolved = schema.effective_attributes(&refs);
    for a in resolved.must.iter().chain(&resolved.may) {
        let is_password = passwords.contains(&a.to_lowercase());
        if is_password && !show.iter().any(|s| s.eq_ignore_ascii_case(a)) {
            show.insert(at, a.clone());
            at += 1;
        }
    }
}

/// `rdn_attr`, then MUST attributes, then MAY attributes present in more than
/// half the group (by frequency, then name); operational, binary and secret
/// attributes excluded. The built-in password field of an account profile
/// follows the identity attributes, whether or not entries reveal it.
fn show_list(
    schema: &SchemaModel,
    object_classes: &[String],
    rdn_attr: &str,
    n: usize,
    presence: &dyn Fn(&str) -> usize,
) -> Vec<String> {
    let excluded = |a: &str| {
        a.eq_ignore_ascii_case("objectClass")
            || crate::detect::is_secret_attr(a)
            || schema.is_readonly_attr(a)
            || schema.field_kind(a) == FieldKind::Binary
    };
    let mut show: Vec<String> = vec![rdn_attr.to_string()];
    let push = |show: &mut Vec<String>, a: &str| {
        if !show.iter().any(|s| s.eq_ignore_ascii_case(a)) {
            show.push(a.to_string());
        }
    };
    let refs: Vec<&str> = object_classes.iter().map(String::as_str).collect();
    let resolved = schema.effective_attributes(&refs);
    for m in &resolved.must {
        if !excluded(m) {
            push(&mut show, m);
        }
    }
    let mut may: Vec<(usize, String)> = resolved
        .may
        .iter()
        .filter(|a| !excluded(a))
        .map(|a| (presence(a), a.clone()))
        .filter(|(k, _)| more_than_half(*k, n))
        .collect();
    may.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
    });
    for (_, a) in may {
        push(&mut show, &a);
    }
    let passwords = password_fields(object_classes, []);
    place_password_fields(schema, &mut show, object_classes, rdn_attr, &passwords);
    show
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::fixtures::{container, demo_sample, e, schema};
    use crate::detect::model::Sample;

    fn by_name<'a>(d: &'a Detection, name: &str) -> &'a DetectedProfile {
        d.profiles
            .iter()
            .find(|p| p.name == name)
            .unwrap_or_else(|| {
                panic!(
                    "no profile {name}: {:?}",
                    d.profiles.iter().map(|p| &p.name).collect::<Vec<_>>()
                )
            })
    }

    /// A password field is proposed for `show` right after the naming
    /// attributes, even when no sampled entry reveals one; password hashes
    /// are never proposed, however common.
    #[test]
    fn show_lists_the_password_field_but_never_hashes() {
        let ocs: Vec<String> = ["inetOrgPerson", "posixAccount", "sambaSamAccount"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let presence = |a: &str| {
            if a.eq_ignore_ascii_case("userPassword") {
                0
            } else {
                10
            }
        };
        let show = show_list(&schema(), &ocs, "uid", 10, &presence);
        assert!(
            !show
                .iter()
                .any(|a| a.eq_ignore_ascii_case("sambaNTPassword")),
            "{show:?}"
        );
        assert!(show.iter().any(|a| a == "sambaAcctFlags"), "{show:?}");
        let pw = show.iter().position(|a| a == "userPassword");
        let last_name = show
            .iter()
            .rposition(|a| ["uid", "cn", "sn", "givenName", "displayName"].contains(&a.as_str()))
            .unwrap();
        assert_eq!(pw, Some(last_name + 1), "{show:?}");
        assert_eq!(show.iter().filter(|a| *a == "userPassword").count(), 1);
        // After the identity attributes, not after mail or loginShell.
        let mail = show.iter().position(|a| a == "mail").unwrap();
        assert!(pw.unwrap() < mail, "{show:?}");
    }

    /// Only account profiles get the built-in password field: a `person`
    /// entry such as a password policy does not.
    #[test]
    fn show_has_no_password_field_for_a_person_only_profile() {
        let ocs = vec!["person".to_string()];
        let show = show_list(&schema(), &ocs, "cn", 10, &|_| 10);
        assert!(!show.iter().any(|a| a == "userPassword"), "{show:?}");
    }

    /// Only a password widget brings a password attribute into `show`: a class
    /// that allows `userPassword` without one (posixGroup) gets none.
    #[test]
    fn show_has_no_password_field_without_a_password_widget() {
        let ocs = vec!["posixGroup".to_string()];
        let show = show_list(&schema(), &ocs, "cn", 10, &|_| 10);
        assert!(!show.iter().any(|a| a == "userPassword"), "{show:?}");
    }

    #[test]
    fn demo_groups_into_the_expected_profiles() {
        let d = detect(&schema(), &demo_sample());
        let mut names: Vec<&str> = d.profiles.iter().map(|p| p.name.as_str()).collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "group-groups",
                "organizationalunit-example",
                "posixgroup-groups",
                "sambadomain-example",
                "user-people",
                "user-users"
            ]
        );
        let up = by_name(&d, "user-people");
        assert_eq!(up.structural, "inetOrgPerson");
        assert_eq!(up.container, "ou=people,dc=example,dc=org");
        assert_eq!(
            up.object_classes.value,
            vec![
                "inetOrgPerson",
                "posixAccount",
                "sambaSamAccount",
                "shadowAccount"
            ]
        );
        assert_eq!(up.rdn_attr.value, "uid");
        assert_eq!(up.rdn_attr.evidence.ratio(), "5/5");
        assert_eq!(up.show[0], "uid");
        assert!(up.show.iter().any(|a| a == "loginShell"), "{:?}", up.show);
        assert_eq!(up.search_attrs, vec!["uid", "cn", "sn", "mail"]);
        assert_eq!(up.label.as_deref(), Some("{cn} ({uid})"));
        assert_eq!(up.sampled, 5);
        // uid == cn in ou=users → label falls back to the RDN attribute.
        assert_eq!(by_name(&d, "user-users").label.as_deref(), Some("{uid}"));
    }

    #[test]
    fn minority_classes_are_dropped_and_missing_ones_are_exceptions() {
        let base = "ou=people,dc=x";
        let mk = |i: u32, extra: &[&str]| {
            let mut ocs = vec!["top", "inetOrgPerson"];
            ocs.extend_from_slice(extra);
            let dn = format!("uid=a{i},{base}");
            let uid = format!("a{i}");
            e(
                &dn,
                &[
                    ("objectClass", &ocs),
                    ("uid", &[uid.as_str()]),
                    ("cn", &["A"]),
                    ("sn", &["B"]),
                ],
            )
        };
        let sample = Sample {
            containers: vec![container(
                base,
                vec![
                    mk(1, &["shadowAccount"]),
                    mk(2, &["shadowAccount"]),
                    mk(3, &["shadowAccount"]),
                    mk(4, &[]),
                    mk(5, &["sambaSamAccount"]),
                ],
            )],
            ..Default::default()
        };
        let d = detect(&schema(), &sample);
        let p = &d.profiles[0];
        assert_eq!(
            p.object_classes.value,
            vec!["inetOrgPerson", "shadowAccount"]
        );
        assert_eq!(p.object_classes.evidence.ratio(), "3/5");
        assert_eq!(
            p.object_classes.evidence.exceptions,
            vec!["uid=a4,ou=people,dc=x", "uid=a5,ou=people,dc=x"]
        );
    }

    #[test]
    fn show_excludes_binary_and_operational_attributes() {
        let base = "ou=people,dc=x";
        let entries: Vec<_> = (1..=3)
            .map(|i| {
                let dn = format!("uid=b{i},{base}");
                let uid = format!("b{i}");
                e(
                    &dn,
                    &[
                        ("objectClass", &["inetOrgPerson"]),
                        ("uid", &[uid.as_str()]),
                        ("cn", &["C"]),
                        ("sn", &["S"]),
                        ("jpegPhoto", &["x"]),
                        ("entryCSN", &["1"]),
                        ("mail", &["m@x"]),
                    ],
                )
            })
            .collect();
        let d = detect(
            &schema(),
            &Sample {
                containers: vec![container(base, entries)],
                ..Default::default()
            },
        );
        let show = &d.profiles[0].show;
        assert!(show.iter().any(|a| a == "mail"));
        assert!(!show.iter().any(|a| a.eq_ignore_ascii_case("jpegPhoto")));
        assert!(!show.iter().any(|a| a.eq_ignore_ascii_case("entryCSN")));
        assert!(!show.iter().any(|a| a.eq_ignore_ascii_case("objectClass")));
    }

    #[test]
    fn entries_without_a_structural_class_are_noted() {
        let sample = Sample {
            containers: vec![container(
                "ou=x,dc=x",
                vec![e("cn=a,ou=x,dc=x", &[("objectClass", &["posixAccount"])])],
            )],
            ..Default::default()
        };
        let d = detect(&schema(), &sample);
        assert!(d.profiles.is_empty());
        assert_eq!(d.notes.len(), 1);
        assert!(d.notes[0].contains("ou=x,dc=x"), "{}", d.notes[0]);
    }

    #[test]
    fn an_empty_sample_detects_nothing() {
        let d = detect(&schema(), &Sample::default());
        assert!(d.profiles.is_empty());
        assert!(d.notes.is_empty());
    }
}
