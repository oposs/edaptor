//! Rules B2 (user-private group) and B3 (shared primary group), spec §2B2–3.

use std::collections::{BTreeMap, HashSet};

use crate::config::defaults::{parse_default_value, DefaultValue};
use crate::config::{ChoiceOption, CompanionSpec, WidgetSpecCfg};
use crate::detect::model::{Detected, DetectedProfile, Evidence, Sample, SampleEntry};
use crate::detect::private::PrivateIndex;
use crate::detect::{most_common, rule_applies, MIN_SAMPLE};

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
/// uid, gidNumber = uidNumber, the user as memberUid. Shared with rule D
/// (assumed defaults) so both build the same companion.
pub fn private_group_companion(search_base: &str) -> CompanionSpec {
    CompanionSpec {
        object_classes: vec!["posixGroup".to_string()],
        rdn_attr: "cn".to_string(),
        search_base: search_base.to_string(),
        attributes: BTreeMap::from([
            ("cn".to_string(), "{uid}".to_string()),
            ("gidNumber".to_string(), "{uidNumber}".to_string()),
            ("memberUid".to_string(), "{uid}".to_string()),
        ]),
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
    let companion = private_group_companion(&base);
    p.defaults.insert(
        "gidNumber".to_string(),
        Detected::new(template("{uidNumber}"), ev.clone()),
    );
    p.companion = Some(Detected::new(companion, ev));
    p.private_groups = true;
    true
}

/// Provenance of a `loginShell` choice no sampled entry supports.
pub const REASON_NO_SHELLS: &str = "no login shells in the sample; built-in list";

/// At most this many shells in use are added to the built-in `loginShell` options.
const MAX_EXTRA_SHELLS: usize = 10;

/// A `loginShell` value worth offering: an absolute path without blanks or
/// control characters.
fn plausible_shell(s: &str) -> bool {
    s.starts_with('/') && s.len() > 1 && !s.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// `/usr/bin/fish` → `Fish`.
fn shell_label(path: &str) -> String {
    let base = path.rsplit('/').next().unwrap_or(path);
    let mut chars = base.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => path.to_string(),
    }
}

/// The `loginShell` choice of a user profile: the built-in options plus the
/// shells in use (most frequent first, at most [`MAX_EXTRA_SHELLS`]), the
/// detected default first. Entries whose shell is left out are exceptions.
/// Without any shell in the sample the built-in list is proposed as assumed.
pub fn apply_login_shell(p: &mut DetectedProfile) {
    let Some(WidgetSpecCfg::Choice { options, .. }) = crate::config::builtin::builtin_schema()
        .get("posixaccount")
        .and_then(|m| m.get("loginshell"))
    else {
        return;
    };
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for sh in p.entries.iter().filter_map(|u| u.first("loginShell")) {
        if plausible_shell(sh) && !options.iter().any(|o| o.value == sh) {
            *counts.entry(sh).or_default() += 1;
        }
    }
    let mut extras: Vec<(&str, usize)> = counts.into_iter().collect();
    extras.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    extras.truncate(MAX_EXTRA_SHELLS);
    let mut all: Vec<ChoiceOption> = options.clone();
    all.extend(extras.iter().map(|(v, _)| ChoiceOption {
        value: v.to_string(),
        label: shell_label(v),
    }));
    if let Some(Detected {
        value: DefaultValue::Literal(d),
        ..
    }) = p.defaults.get("loginShell")
    {
        if let Some(i) = all.iter().position(|o| &o.value == d) {
            let first = all.remove(i);
            all.insert(0, first);
        }
    }
    let mut matched = 0;
    let mut exceptions = Vec::new();
    for u in &p.entries {
        if let Some(sh) = u.first("loginShell") {
            if all.iter().any(|o| o.value == sh) {
                matched += 1;
            } else {
                exceptions.push(u.dn.clone());
            }
        }
    }
    if matched == 0 && exceptions.is_empty() {
        p.assumed.insert(
            "widget.loginShell".to_string(),
            REASON_NO_SHELLS.to_string(),
        );
    }
    let ev = Evidence::new(matched, p.entries.len())
        .with_exceptions(exceptions)
        .with_note("login shells in use");
    let spec = WidgetSpecCfg::Choice {
        select: "single".to_string(),
        format: "plain".to_string(),
        options: all,
    };
    p.widgets
        .insert("loginShell".to_string(), Detected::new(spec, ev));
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
