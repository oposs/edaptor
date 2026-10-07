//! Emit rule-C defaults: posix users get a detected `uidNumber` range, posix
//! groups a detected `gidNumber` range (resolved at create time).

use crate::config::defaults::DefaultValue;
use crate::detect::model::{Detected, DetectedProfile, Evidence};
use crate::detect::patterns::posix::{is_group_profile, is_user_profile};
use crate::detect::range::RangeSpec;

pub fn apply(profiles: &mut [DetectedProfile]) {
    let unified_any = profiles.iter().any(|p| p.private_groups);
    for p in profiles.iter_mut() {
        let (attr, unified, exclude_private) = if is_user_profile(p) {
            ("uidNumber", p.private_groups, false)
        } else if is_group_profile(p) {
            ("gidNumber", unified_any, true)
        } else {
            continue;
        };
        if p.defaults.keys().any(|k| k.eq_ignore_ascii_case(attr)) {
            continue;
        }
        let spec = RangeSpec {
            attr: attr.to_string(),
            container: p.container.clone(),
            structural: p.structural.clone(),
            unified,
            exclude_private,
        };
        p.defaults.insert(
            attr.to_string(),
            Detected::new(
                DefaultValue::DetectedRange(spec),
                Evidence::new(0, 0).with_note("range detected at create time"),
            ),
        );
    }
}
