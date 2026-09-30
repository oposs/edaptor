//! Rule B5 (spec §2B5): picker and lookup targets.

use crate::config::{CandidateRef, WidgetSpecCfg};
use crate::detect::model::{Detected, DetectedProfile, Evidence};
use crate::detect::patterns::posix::{is_group_profile, is_user_profile};
use crate::detect::{dn_eq, more_than_half, most_common};

/// The name of the matching profile with the most sampled entries (ties: name).
fn largest(
    profiles: &[DetectedProfile],
    pred: impl Fn(&DetectedProfile) -> bool,
) -> Option<String> {
    profiles
        .iter()
        .filter(|p| pred(p))
        .max_by(|a, b| a.sampled.cmp(&b.sampled).then_with(|| b.name.cmp(&a.name)))
        .map(|p| p.name.clone())
}

fn picker(candidate: &str, store: &str) -> WidgetSpecCfg {
    WidgetSpecCfg::Picker {
        candidate: CandidateRef::Profile(candidate.to_string()),
        store: store.to_string(),
        select: "multi".to_string(),
    }
}

pub fn apply(profiles: &mut [DetectedProfile]) {
    let user = largest(profiles, is_user_profile);
    let group = largest(profiles, is_group_profile);
    let containers: Vec<(String, String, usize)> = profiles
        .iter()
        .map(|p| (p.container.clone(), p.name.clone(), p.sampled))
        .collect();
    let target_in = |container: &str| -> Option<String> {
        containers
            .iter()
            .filter(|(c, _, _)| dn_eq(c, container))
            .max_by(|a, b| a.2.cmp(&b.2).then_with(|| b.1.cmp(&a.1)))
            .map(|(_, n, _)| n.clone())
    };
    for p in profiles.iter_mut() {
        let has = |oc: &str| {
            p.object_classes
                .value
                .iter()
                .any(|c| c.eq_ignore_ascii_case(oc))
        };
        let guard = Evidence::new(p.sampled, p.sampled);
        let mut add: Vec<(String, Detected<WidgetSpecCfg>)> = Vec::new();
        if has("posixGroup") {
            if let Some(u) = &user {
                add.push((
                    "memberUid".into(),
                    Detected::new(
                        picker(u, "uid"),
                        guard.clone().with_note("posix-user profile"),
                    ),
                ));
            }
        }
        if has("posixAccount") {
            if let Some(g) = &group {
                add.push((
                    "gidNumber".into(),
                    Detected::new(
                        WidgetSpecCfg::Lookup {
                            candidate: CandidateRef::Profile(g.clone()),
                            store: "gidNumber".into(),
                            label: Some("{cn}".into()),
                        },
                        guard.clone().with_note("posix-group profile"),
                    ),
                ));
            }
        }
        for (attr, oc) in [
            ("member", "groupOfNames"),
            ("uniqueMember", "groupOfUniqueNames"),
        ] {
            if !has(oc) {
                continue;
            }
            let dns: Vec<&str> = p
                .entries
                .iter()
                .flat_map(|e| e.values(attr).iter().map(String::as_str))
                .collect();
            let parents: Vec<&str> = dns
                .iter()
                .filter_map(|d| crate::detect::parent_dn(d))
                .collect();
            if let Some((container, k)) = most_common(parents) {
                if more_than_half(k, dns.len()) {
                    if let Some(target) = target_in(&container) {
                        add.push((
                            attr.into(),
                            Detected::new(
                                picker(&target, "dn"),
                                Evidence::new(k, dns.len())
                                    .with_note(format!("member DNs in {container}")),
                            ),
                        ));
                    }
                }
            }
        }
        for (attr, w) in add {
            p.widgets.entry(attr).or_insert(w);
        }
    }
}
