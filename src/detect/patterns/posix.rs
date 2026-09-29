//! Rules B2 (user-private group) and B3 (shared primary group), spec §2B2–3.

use std::collections::{BTreeMap, HashSet};

use crate::config::defaults::{parse_default_value, DefaultValue};
use crate::config::CompanionSpec;
use crate::detect::model::{Detected, DetectedProfile, Evidence, Sample, SampleEntry};
use crate::detect::private::PrivateIndex;
use crate::detect::{more_than_half, most_common, rule_applies, MIN_SAMPLE};

fn has_class(p: &DetectedProfile, oc: &str) -> bool {
    p.object_classes
        .value
        .iter()
        .any(|c| c.eq_ignore_ascii_case(oc))
}
pub fn is_user_profile(p: &DetectedProfile) -> bool {
    has_class(p, "posixAccount")
}
pub fn is_group_profile(p: &DetectedProfile) -> bool {
    has_class(p, "posixGroup")
}

/// Every posixAccount / posixGroup entry known: sampled plus looked up, deduped by DN.
pub fn all_posix_entries(sample: &Sample) -> Vec<SampleEntry> {
    let mut seen = HashSet::new();
    sample
        .containers
        .iter()
        .flat_map(|c| c.entries.iter())
        .chain(sample.groups.iter())
        .chain(sample.accounts.iter())
        .filter(|e| e.has_class("posixAccount") || e.has_class("posixGroup"))
        .filter(|e| seen.insert(e.dn.to_lowercase()))
        .cloned()
        .collect()
}

/// Sampled users of `p` without a private group (evaluated at any sample size).
pub fn count_without_private(p: &DetectedProfile, index: &PrivateIndex) -> usize {
    p.entries
        .iter()
        .filter(|u| index.private_group_of(u).is_none())
        .count()
}

/// The profile's groups that are nobody's private group.
pub fn non_private(p: &DetectedProfile, index: &PrivateIndex) -> Vec<SampleEntry> {
    p.entries
        .iter()
        .filter(|g| !index.is_private(g))
        .cloned()
        .collect()
}

fn template(s: &str) -> DefaultValue {
    parse_default_value(s).expect("built-in template parses")
}

/// The posixGroup companion a user profile with private groups creates: cn =
/// uid, gidNumber = uidNumber, optionally the user as memberUid. Shared with
/// rule D (assumed defaults) so both build the same companion.
pub fn private_group_companion(search_base: &str, member_uid: bool) -> CompanionSpec {
    let mut attributes = BTreeMap::from([
        ("cn".to_string(), "{uid}".to_string()),
        ("gidNumber".to_string(), "{uidNumber}".to_string()),
    ]);
    if member_uid {
        attributes.insert("memberUid".to_string(), "{uid}".to_string());
    }
    CompanionSpec {
        object_classes: vec!["posixGroup".to_string()],
        rdn_attr: "cn".to_string(),
        search_base: search_base.to_string(),
        attributes,
    }
}

/// B2. Returns true when it applied.
pub fn apply_private_group(p: &mut DetectedProfile, index: &PrivateIndex) -> bool {
    let n = p.entries.len();
    if n < MIN_SAMPLE {
        return false;
    }
    let mut pairs: Vec<(SampleEntry, SampleEntry)> = Vec::new();
    let mut without: Vec<String> = Vec::new();
    for u in &p.entries {
        match index.private_group_of(u) {
            Some(g) => pairs.push((u.clone(), g.clone())),
            None => without.push(u.dn.clone()),
        }
    }
    if !rule_applies(pairs.len(), n) {
        return false;
    }
    let ev = Evidence::new(pairs.len(), n)
        .with_exceptions(without)
        .with_note("have a private group");
    let base = most_common(pairs.iter().filter_map(|(_, g)| g.parent()))
        .map(|(b, _)| b)
        .unwrap_or_default();
    let with_member = pairs
        .iter()
        .filter(|(u, g)| {
            u.first("uid").is_some_and(|uid| {
                g.values("memberUid")
                    .iter()
                    .any(|m| m.trim().eq_ignore_ascii_case(uid))
            })
        })
        .count();
    let companion = private_group_companion(&base, more_than_half(with_member, pairs.len()));
    p.defaults.insert(
        "gidNumber".to_string(),
        Detected::new(template("{uidNumber}"), ev.clone()),
    );
    p.companion = Some(Detected::new(companion, ev));
    p.private_groups = true;
    true
}

/// B3 (a shared `gidNumber` becomes a literal), else the gid = uid note.
pub fn apply_shared_gid(p: &mut DetectedProfile) {
    let n = p.entries.len();
    if n < MIN_SAMPLE {
        return;
    }
    if let Some((gid, k)) = most_common(p.entries.iter().filter_map(|u| u.first("gidNumber"))) {
        if rule_applies(k, n) {
            let exceptions = p
                .entries
                .iter()
                .filter(|u| u.first("gidNumber") != Some(gid.as_str()))
                .map(|u| u.dn.clone())
                .collect();
            p.defaults.insert(
                "gidNumber".to_string(),
                Detected::new(
                    DefaultValue::Literal(gid),
                    Evidence::new(k, n).with_exceptions(exceptions),
                ),
            );
            return;
        }
    }
    let same = p
        .entries
        .iter()
        .filter(|u| u.first("gidNumber").is_some() && u.first("gidNumber") == u.first("uidNumber"))
        .count();
    if rule_applies(same, n) {
        p.notes.push(format!(
            "gidNumber = uidNumber for {same}/{n}, but no private groups found"
        ));
    }
}
