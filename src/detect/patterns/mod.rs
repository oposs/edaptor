//! Known patterns (spec §2B) applied to detected profiles.

pub mod templates;

use crate::detect::model::{DetectedProfile, Sample};
use crate::schema::SchemaModel;

/// Run every pattern over `profiles` (names are already assigned).
pub fn apply(
    schema: &SchemaModel,
    _sample: &Sample,
    profiles: &mut [DetectedProfile],
    _notes: &mut Vec<String>,
) {
    for p in profiles.iter_mut() {
        let (defaults, dropped) = templates::infer_defaults(schema, &p.entries, &p.rdn_attr.value);
        p.notes.extend(dropped);
        p.defaults.extend(defaults);
    }
}
