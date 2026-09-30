//! Rule D (spec §2D): when there is too little data, assume what Ubuntu's
//! useradd does (numbers from a fixed start, user-private groups), with LDAP
//! numbers starting at 10000. Runs after the merge; fills only what is missing.

use crate::config::defaults::{parse_default_value, DefaultValue};
use crate::config::{EntryProfile, ProfileOverride};
use crate::detect::merge::{flush_pending, merge_core, Merged, Origin, Provenance, Source};
use crate::detect::model::DetectedProfile;
use crate::detect::patterns::posix::private_group_companion;
use crate::detect::range::RangeSpec;
use crate::detect::MIN_SAMPLE;
use crate::schema::SchemaModel;

pub const NO_GROUP_CONTAINER: &str = "no group container for private groups";
pub const REASON_NO_USERS: &str = "no users yet; useradd-style private group";
pub const REASON_FEW_USERS: &str =
    "fewer than 3 users, all with a private group; useradd-style private group";

/// Merge, apply rule D, then resolve `suppress` paths (which may name assumed parts).
pub fn merge_with_assumptions(
    schema: &SchemaModel,
    detected: &[DetectedProfile],
    overrides: &[ProfileOverride],
    group_ou: Option<&str>,
    detection_failed: bool,
) -> Merged {
    let mut m = merge_core(schema, detected, overrides);
    apply(schema, detected, group_ou, detection_failed, &mut m);
    flush_pending(&mut m);
    m
}

fn has_class(p: &EntryProfile, oc: &str) -> bool {
    p.object_classes.iter().any(|c| c.eq_ignore_ascii_case(oc))
}

fn has_default(p: &EntryProfile, attr: &str) -> bool {
    p.defaults
        .entries
        .keys()
        .any(|k| k.eq_ignore_ascii_case(attr))
}

fn gid_follows_uid(p: &EntryProfile) -> bool {
    p.defaults.entries.iter().any(|(k, v)| {
        k.eq_ignore_ascii_case("gidNumber")
            && v.to_config_string().eq_ignore_ascii_case("{uidNumber}")
    })
}

/// Mark an existing detected range on `attr` as sharing one number space.
fn mark_unified(p: &mut EntryProfile, attr: &str) {
    for (k, v) in p.defaults.entries.iter_mut() {
        if let (true, DefaultValue::DetectedRange(s)) = (k.eq_ignore_ascii_case(attr), v) {
            s.unified = true;
        }
    }
}

fn sampled_of(prov: &Provenance) -> usize {
    match &prov.origin {
        Origin::Detected { sampled, .. } | Origin::Merged { sampled, .. } => *sampled,
        Origin::Config => 0,
    }
}

fn detected_of<'a>(
    prov: &Provenance,
    detected: &'a [DetectedProfile],
) -> Option<&'a DetectedProfile> {
    let name = match &prov.origin {
        Origin::Detected { .. } => prov.name.as_str(),
        Origin::Merged { detected, .. } => detected.as_str(),
        Origin::Config => return None,
    };
    detected.iter().find(|d| d.name.eq_ignore_ascii_case(name))
}

/// The detected user profiles sampled at or below `base`: the evidence rule D
/// weighs for a config-only profile. An empty `base` covers everything.
fn users_under<'a>(base: &str, detected: &'a [DetectedProfile]) -> Vec<&'a DetectedProfile> {
    detected
        .iter()
        .filter(|d| {
            d.object_classes
                .value
                .iter()
                .any(|c| c.eq_ignore_ascii_case("posixAccount"))
                && (base.trim().is_empty() || crate::detect::dn_within(&d.container, base))
        })
        .collect()
}

fn structural(schema: &SchemaModel, p: &EntryProfile) -> String {
    schema
        .structural_class(&p.object_classes)
        .or_else(|| p.object_classes.first().cloned())
        .unwrap_or_default()
}

fn range_reason(attr: &str) -> String {
    format!("no {attr} range configured or detected; useradd-style numbering")
}

pub fn apply(
    schema: &SchemaModel,
    detected: &[DetectedProfile],
    group_ou: Option<&str>,
    detection_failed: bool,
    m: &mut Merged,
) {
    // §2B5 "posix-group profile", over the merged profiles.
    let group_base: Option<String> = m
        .profiles
        .iter()
        .zip(&m.provenance)
        .filter(|(p, _)| has_class(p, "posixGroup") && !p.search_base.is_empty())
        .max_by(|(a, pa), (b, pb)| {
            sampled_of(pa)
                .cmp(&sampled_of(pb))
                .then_with(|| b.name.cmp(&a.name))
        })
        .map(|(p, _)| p.search_base.clone())
        .or_else(|| group_ou.map(str::to_string));
    // Users first: group ranges need to know whether any user space is unified.
    for i in 0..m.profiles.len() {
        if !has_class(&m.profiles[i], "posixAccount") {
            continue;
        }
        let evidence: Vec<&DetectedProfile> = match detected_of(&m.provenance[i], detected) {
            Some(d) => vec![d],
            None => users_under(&m.profiles[i].search_base, detected),
        };
        let sampled: usize = evidence.iter().map(|d| d.sampled).sum();
        let contrary = sampled >= MIN_SAMPLE
            || evidence
                .iter()
                .any(|d| d.users_without_private_group != Some(0));
        let reason = if sampled == 0 {
            REASON_NO_USERS
        } else {
            REASON_FEW_USERS
        };
        let st = structural(schema, &m.profiles[i]);
        let (p, prov) = (&mut m.profiles[i], &mut m.provenance[i]);
        let mut assumed_private = false;
        // A failed detection saw nothing, so it cannot tell whether users here
        // have private groups: assume none rather than flip on a timeout.
        if !detection_failed && !contrary && !has_default(p, "gidNumber") && p.companion.is_none() {
            match &group_base {
                Some(base) => {
                    p.defaults.entries.insert(
                        "gidNumber".into(),
                        parse_default_value("{uidNumber}").expect("template"),
                    );
                    p.companion = Some(private_group_companion(base));
                    prov.fields
                        .insert("defaults.gidNumber".into(), Source::Assumed(reason.into()));
                    prov.fields
                        .insert("companion".into(), Source::Assumed(reason.into()));
                    assumed_private = true;
                }
                None => prov.notes.push(NO_GROUP_CONTAINER.to_string()),
            }
        }
        if !has_default(p, "uidNumber") {
            let spec = RangeSpec {
                attr: "uidNumber".into(),
                container: p.search_base.clone(),
                structural: st,
                unified: gid_follows_uid(p),
                exclude_private: false,
            };
            p.defaults
                .entries
                .insert("uidNumber".into(), DefaultValue::DetectedRange(spec));
            prov.fields.insert(
                "defaults.uidNumber".into(),
                Source::Assumed(range_reason("uidNumber")),
            );
        } else if assumed_private {
            mark_unified(p, "uidNumber");
        }
    }
    let unified_any = m
        .profiles
        .iter()
        .any(|p| has_class(p, "posixAccount") && gid_follows_uid(p));
    for (p, prov) in m.profiles.iter_mut().zip(m.provenance.iter_mut()) {
        if !has_class(p, "posixGroup") {
            continue;
        }
        if has_default(p, "gidNumber") {
            // A detected group range joins a space that assumed private groups unified.
            if unified_any {
                mark_unified(p, "gidNumber");
            }
            continue;
        }
        let spec = RangeSpec {
            attr: "gidNumber".into(),
            container: p.search_base.clone(),
            structural: structural(schema, p),
            unified: unified_any,
            exclude_private: true,
        };
        p.defaults
            .entries
            .insert("gidNumber".into(), DefaultValue::DetectedRange(spec));
        prov.fields.insert(
            "defaults.gidNumber".into(),
            Source::Assumed(range_reason("gidNumber")),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::fixtures::{container, e, overrides, schema};
    use crate::detect::infer::detect;
    use crate::detect::model::Sample;

    const CFG_USER: &str = "[[profile]]\nname = \"user\"\nobject_classes = [\"inetOrgPerson\", \"posixAccount\"]\nrdn_attr = \"uid\"\nsearch_base = \"ou=people,dc=x\"\n";
    fn get<'a>(m: &'a Merged, name: &str) -> (&'a EntryProfile, &'a Provenance) {
        let i = m
            .profiles
            .iter()
            .position(|p| p.name == name)
            .unwrap_or_else(|| panic!("no {name}"));
        (&m.profiles[i], &m.provenance[i])
    }
    fn dflt(p: &EntryProfile, attr: &str) -> Option<String> {
        p.defaults.entries.get(attr).map(|d| d.to_config_string())
    }

    #[test]
    fn empty_directory_config_user_gets_private_groups_and_a_range() {
        let m = merge_with_assumptions(
            &schema(),
            &[],
            &overrides(CFG_USER),
            Some("ou=groups,dc=x"),
            false,
        );
        let (p, prov) = get(&m, "user");
        assert_eq!(dflt(p, "gidNumber").as_deref(), Some("{uidNumber}"));
        let c = p.companion.as_ref().unwrap();
        assert_eq!(c.search_base, "ou=groups,dc=x");
        assert_eq!(c.attributes["memberUid"], "{uid}");
        match &p.defaults.entries["uidNumber"] {
            DefaultValue::DetectedRange(s) => assert!(s.unified && s.container == "ou=people,dc=x"),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            prov.fields["companion"],
            Source::Assumed(REASON_NO_USERS.into())
        );
        assert!(matches!(
            prov.fields["defaults.uidNumber"],
            Source::Assumed(_)
        ));
        assert!(m.warnings.is_empty(), "{:?}", m.warnings);
    }

    #[test]
    fn companion_base_prefers_the_posix_group_profile() {
        let o = format!("{CFG_USER}[[profile]]\nname = \"grp\"\nobject_classes = [\"posixGroup\"]\nsearch_base = \"ou=unix,dc=x\"\n");
        let m = merge_with_assumptions(
            &schema(),
            &[],
            &overrides(&o),
            Some("ou=groups,dc=x"),
            false,
        );
        assert_eq!(
            get(&m, "user").0.companion.as_ref().unwrap().search_base,
            "ou=unix,dc=x"
        );
        match &get(&m, "grp").0.defaults.entries["gidNumber"] {
            DefaultValue::DetectedRange(s) => assert!(s.unified && s.exclude_private),
            other => panic!("{other:?}"),
        }
    }

    fn two_users(private: bool) -> Sample {
        let users = (1..=2u64)
            .map(|i| {
                let uid = format!("u{i}");
                let num = (5000 + i).to_string();
                let gid = if private {
                    num.clone()
                } else {
                    "100".to_string()
                };
                e(
                    &format!("uid={uid},ou=people,dc=x"),
                    &[
                        ("objectClass", &["inetOrgPerson", "posixAccount"]),
                        ("uid", &[uid.as_str()]),
                        ("cn", &[uid.as_str()]),
                        ("sn", &["s"]),
                        ("uidNumber", &[num.as_str()]),
                        ("gidNumber", &[gid.as_str()]),
                    ],
                )
            })
            .collect();
        let groups = if private {
            (1..=2u64)
                .map(|i| {
                    let cn = format!("u{i}");
                    let num = (5000 + i).to_string();
                    e(
                        &format!("cn={cn},ou=groups,dc=x"),
                        &[
                            ("objectClass", &["posixGroup"]),
                            ("cn", &[cn.as_str()]),
                            ("gidNumber", &[num.as_str()]),
                        ],
                    )
                })
                .collect()
        } else {
            vec![e(
                "cn=staff,ou=groups,dc=x",
                &[
                    ("objectClass", &["posixGroup"]),
                    ("cn", &["staff"]),
                    ("gidNumber", &["100"]),
                ],
            )]
        };
        Sample {
            containers: vec![
                container("ou=people,dc=x", users),
                container("ou=groups,dc=x", groups),
            ],
            group_ou: Some("ou=groups,dc=x".into()),
            ..Default::default()
        }
    }

    #[test]
    fn two_users_with_private_groups_are_assumed() {
        let d = detect(&schema(), &two_users(true)).profiles;
        let m = merge_with_assumptions(&schema(), &d, &[], Some("ou=groups,dc=x"), false);
        let (p, prov) = get(&m, "user-people");
        assert_eq!(dflt(p, "gidNumber").as_deref(), Some("{uidNumber}"));
        assert_eq!(p.companion.as_ref().unwrap().search_base, "ou=groups,dc=x");
        assert_eq!(
            prov.fields["companion"],
            Source::Assumed(REASON_FEW_USERS.into())
        );
        match &p.defaults.entries["uidNumber"] {
            DefaultValue::DetectedRange(s) => {
                assert!(s.unified, "assumed private groups unify the space")
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn two_users_sharing_gid_100_block_the_assumption() {
        let d = detect(&schema(), &two_users(false)).profiles;
        let m = merge_with_assumptions(&schema(), &d, &[], Some("ou=groups,dc=x"), false);
        let (p, _) = get(&m, "user-people");
        assert!(p.companion.is_none());
        assert!(
            !p.defaults.entries.contains_key("gidNumber"),
            "B3 needs 3 users"
        );
    }

    /// A config-only user profile whose base covers sampled users sharing gid
    /// 100 must not gain a private group: those users are contrary evidence.
    #[test]
    fn config_only_profile_takes_contrary_evidence_from_users_under_its_base() {
        let d = detect(&schema(), &two_users(false)).profiles;
        // Structural class differs from the detected one, so pass 2 cannot match.
        let o = "[[profile]]\nname = \"user\"\nobject_classes = [\"person\", \"posixAccount\"]\nsearch_base = \"dc=x\"\n";
        let m = merge_with_assumptions(&schema(), &d, &overrides(o), Some("ou=groups,dc=x"), false);
        let (p, _) = get(&m, "user");
        assert!(p.companion.is_none(), "users under dc=x share gid 100");
        assert!(!p.defaults.entries.contains_key("gidNumber"));
    }

    /// Two sampled users under the base, both with private groups: too few to
    /// contradict, so the assumption holds with the "few users" reason.
    #[test]
    fn config_only_profile_over_few_private_group_users_is_assumed() {
        let d = detect(&schema(), &two_users(true)).profiles;
        let o = "[[profile]]\nname = \"user\"\nobject_classes = [\"person\", \"posixAccount\"]\nsearch_base = \"dc=x\"\n";
        let m = merge_with_assumptions(&schema(), &d, &overrides(o), Some("ou=groups,dc=x"), false);
        let (p, prov) = get(&m, "user");
        assert!(p.companion.is_some());
        assert_eq!(
            prov.fields["companion"],
            Source::Assumed(REASON_FEW_USERS.into())
        );
    }

    /// Users sampled in a sibling container are not evidence for this base.
    #[test]
    fn users_outside_the_base_are_not_evidence() {
        let d = detect(&schema(), &two_users(false)).profiles;
        let o = "[[profile]]\nname = \"user\"\nobject_classes = [\"person\", \"posixAccount\"]\nsearch_base = \"ou=people2,dc=x\"\n";
        let m = merge_with_assumptions(&schema(), &d, &overrides(o), Some("ou=groups,dc=x"), false);
        let (p, prov) = get(&m, "user");
        assert!(p.companion.is_some());
        assert_eq!(
            prov.fields["companion"],
            Source::Assumed(REASON_NO_USERS.into())
        );
    }

    /// Detection failed as a whole: no evidence either way, so no private group
    /// is assumed. The number range is still assumed (it scans at create time).
    #[test]
    fn a_failed_detection_assumes_no_private_group() {
        let m = merge_with_assumptions(
            &schema(),
            &[],
            &overrides(CFG_USER),
            Some("ou=groups,dc=x"),
            true,
        );
        let (p, _) = get(&m, "user");
        assert!(p.companion.is_none());
        assert!(!p.defaults.entries.contains_key("gidNumber"));
        assert!(matches!(
            p.defaults.entries["uidNumber"],
            DefaultValue::DetectedRange(_)
        ));
    }

    #[test]
    fn no_group_container_is_noted() {
        let m = merge_with_assumptions(&schema(), &[], &overrides(CFG_USER), None, false);
        let (p, prov) = get(&m, "user");
        assert!(p.companion.is_none() && !p.defaults.entries.contains_key("gidNumber"));
        assert!(prov.notes.iter().any(|n| n == NO_GROUP_CONTAINER));
        match &p.defaults.entries["uidNumber"] {
            DefaultValue::DetectedRange(s) => assert!(!s.unified),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_assumed_companion_can_be_suppressed() {
        let o = format!("{CFG_USER}suppress = [\"companion\"]\n");
        let m = merge_with_assumptions(
            &schema(),
            &[],
            &overrides(&o),
            Some("ou=groups,dc=x"),
            false,
        );
        let (p, prov) = get(&m, "user");
        assert!(p.companion.is_none());
        assert_eq!(prov.suppressed, vec!["companion"]);
        assert!(m.warnings.is_empty(), "{:?}", m.warnings);
    }

    #[test]
    fn one_attribute_of_an_assumed_companion_can_be_suppressed() {
        let o = format!("{CFG_USER}suppress = [\"companion.memberUid\"]\n");
        let m = merge_with_assumptions(
            &schema(),
            &[],
            &overrides(&o),
            Some("ou=groups,dc=x"),
            false,
        );
        let (p, prov) = get(&m, "user");
        let c = p.companion.as_ref().expect("the companion stays");
        assert!(!c.attributes.contains_key("memberUid"));
        assert_eq!(c.attributes["cn"], "{uid}");
        assert!(matches!(prov.fields["companion"], Source::Assumed(_)));
        assert_eq!(prov.suppressed, vec!["companion.memberUid"]);
        assert!(m.warnings.is_empty(), "{:?}", m.warnings);
    }

    #[test]
    fn config_values_are_never_overridden() {
        let o = format!("{CFG_USER}[profile.defaults]\ngidNumber = \"100\"\nuidNumber = \"{{next:2000-2999}}\"\n");
        let m = merge_with_assumptions(
            &schema(),
            &[],
            &overrides(&o),
            Some("ou=groups,dc=x"),
            false,
        );
        let (p, _) = get(&m, "user");
        assert_eq!(dflt(p, "gidNumber").as_deref(), Some("100"));
        assert_eq!(dflt(p, "uidNumber").as_deref(), Some("{next:2000-2999}"));
        assert!(p.companion.is_none());
    }
}
