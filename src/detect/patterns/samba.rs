//! Rule B4 (spec §2B4): sambaSamAccount profiles compute their SID.

use crate::config::defaults::{ComputedKind, DefaultValue};
use crate::detect::model::{Detected, DetectedProfile, Evidence};

pub fn apply(p: &mut DetectedProfile) {
    let samba = p
        .object_classes
        .value
        .iter()
        .any(|c| c.eq_ignore_ascii_case("sambaSamAccount"));
    if samba
        && !p
            .defaults
            .keys()
            .any(|k| k.eq_ignore_ascii_case("sambaSID"))
    {
        p.defaults.insert(
            "sambaSID".to_string(),
            Detected::new(
                DefaultValue::Computed(ComputedKind::SambaSid),
                Evidence::new(p.sampled, p.sampled).with_note("sambaSamAccount profile"),
            ),
        );
    }
}
