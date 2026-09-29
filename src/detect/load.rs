//! One entry point for every command that needs profiles (spec §1.4): fetch the
//! schema, sample, detect, merge, validate.

use anyhow::{anyhow, Result};

use crate::config::widget::{resolve_widgets, ResolvedWidget};
use crate::config::{Config, EntryProfile, ProfileOverride};
use crate::detect::assume::merge_with_assumptions;
use crate::detect::merge::{validate, Provenance};
use crate::detect::model::{DetectedProfile, Sample};
use crate::detect::sample::{sample, Budget, WorkerSearcher};
use crate::detect::DETECT_BUDGET;
use crate::ldap::worker::{Request, Response, WorkerHandle};
use crate::schema::SchemaModel;

/// What `load_profiles` needs from the config (captured before the config
/// moves into the worker).
#[derive(Debug, Clone)]
pub struct ProfileInputs {
    pub base_dn: String,
    pub detect_enabled: bool,
    pub overrides: Vec<ProfileOverride>,
    pub config_profiles: Vec<EntryProfile>,
}

impl ProfileInputs {
    pub fn from_config(c: &Config) -> Self {
        ProfileInputs {
            base_dn: c.server.base_dn.clone(),
            detect_enabled: c.detect.enabled,
            overrides: c.overrides.clone(),
            config_profiles: c.profiles.clone(),
        }
    }
}

pub struct LoadedProfiles {
    pub schema: SchemaModel,
    pub profiles: Vec<EntryProfile>,
    /// `resolve_widgets(&profiles)`, computed once here so callers reuse it.
    pub widgets: Vec<ResolvedWidget>,
    /// Parallel to `profiles`.
    pub provenance: Vec<Provenance>,
    /// Names of profiles removed by `enabled = false`.
    pub disabled: Vec<String>,
    pub detected: Vec<DetectedProfile>,
    pub containers_sampled: usize,
    /// Startup warnings (sampling, detection, merge): shown before the TUI starts.
    pub notes: Vec<String>,
    /// Detected parts dropped by validation (also in `notes`).
    pub dropped: Vec<String>,
    pub detection_error: Option<String>,
}

impl LoadedProfiles {
    /// The TUI status line after startup, when detection has something to say.
    pub fn status_line(&self) -> Option<String> {
        if let Some(e) = &self.detection_error {
            return Some(format!("Profile detection failed: {e}"));
        }
        (!self.dropped.is_empty()).then(|| {
            format!(
                "Profile detection dropped {} detected part(s); run `edaptor profiles` for details",
                self.dropped.len()
            )
        })
    }
}

fn widgets_of(profiles: &[EntryProfile]) -> Result<Vec<ResolvedWidget>> {
    resolve_widgets(profiles).map_err(|e| anyhow!("widget config error: {e}"))
}

/// Pure assembly. `sampled`: `None` = detection disabled, which yields exactly
/// the config's own profiles.
pub fn assemble(
    schema: SchemaModel,
    inputs: &ProfileInputs,
    sampled: Option<Result<Sample, String>>,
) -> Result<LoadedProfiles> {
    let Some(sampled) = sampled else {
        let profiles = inputs.config_profiles.clone();
        let widgets = widgets_of(&profiles)?;
        let provenance = profiles.iter().map(Provenance::config).collect();
        return Ok(LoadedProfiles {
            schema,
            profiles,
            widgets,
            provenance,
            disabled: Vec::new(),
            detected: Vec::new(),
            containers_sampled: 0,
            notes: Vec::new(),
            dropped: Vec::new(),
            detection_error: None,
        });
    };
    let (detected, containers_sampled, mut notes, detection_error, group_ou) = match sampled {
        Ok(s) => {
            let d = crate::detect::infer::detect(&schema, &s);
            let mut notes = s.notes;
            notes.extend(d.notes);
            (d.profiles, s.containers.len(), notes, None, s.group_ou)
        }
        Err(e) => (Vec::new(), 0, Vec::new(), Some(e), None),
    };
    // Rule D runs whenever detection is enabled, also after a failed sample.
    let mut merged = merge_with_assumptions(
        &schema,
        &detected,
        &inputs.overrides,
        group_ou.as_deref(),
        detection_error.is_some(),
    );
    validate(&mut merged).map_err(|e| anyhow!("profile config error: {e}"))?;
    let widgets = widgets_of(&merged.profiles)?;
    notes.extend(merged.warnings);
    Ok(LoadedProfiles {
        schema,
        profiles: merged.profiles,
        widgets,
        provenance: merged.provenance,
        disabled: merged.disabled,
        detected,
        containers_sampled,
        notes,
        dropped: merged.dropped,
        detection_error,
    })
}

/// Fetch the subschema, sample (when enabled), detect and merge.
pub fn load_profiles(worker: &WorkerHandle, inputs: &ProfileInputs) -> Result<LoadedProfiles> {
    let raw = match worker.request(Request::FetchSubschema)? {
        Response::Subschema(raw) => raw,
        other => return Err(anyhow!("FetchSubschema: unexpected {other:?}")),
    };
    let schema = SchemaModel::from_raw(&raw);
    let sampled = inputs.detect_enabled.then(|| {
        eprintln!("detecting profiles…");
        sample(
            &mut WorkerSearcher(worker),
            &inputs.base_dn,
            &Budget::new(DETECT_BUDGET),
        )
    });
    assemble(schema, inputs, sampled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::fixtures::{demo_sample, schema};

    fn inputs(toml_profiles: &str, enabled: bool) -> ProfileInputs {
        let cfg: Config = toml::from_str(&format!(
            "[server]\nuri = \"ldap://x\"\nbase_dn = \"dc=example,dc=org\"\n[auth]\nbind_dn = \"cn=a\"\n[detect]\nenabled = {enabled}\n{toml_profiles}"
        ))
        .unwrap();
        ProfileInputs::from_config(&cfg)
    }
    const USER: &str = "[[profile]]\nname = \"user\"\nobject_classes = [\"inetOrgPerson\"]\nsearch_base = \"ou=people,dc=example,dc=org\"\n";

    #[test]
    fn detection_off_yields_exactly_the_config_profiles() {
        let l = assemble(schema(), &inputs(USER, false), None).unwrap();
        assert_eq!(l.profiles.len(), 1);
        assert_eq!(l.profiles[0].name, "user");
        assert!(l.profiles[0].defaults.entries.is_empty());
        assert!(l.detected.is_empty() && l.notes.is_empty() && l.status_line().is_none());
    }

    #[test]
    fn detection_on_merges() {
        let l = assemble(schema(), &inputs(USER, true), Some(Ok(demo_sample()))).unwrap();
        assert_eq!(l.profiles[0].name, "user");
        assert!(l.profiles.iter().any(|p| p.name == "user-users"));
        assert_eq!(l.containers_sampled, 4);
    }

    #[test]
    fn a_failed_detection_falls_back_to_config_profiles() {
        let l = assemble(
            schema(),
            &inputs(USER, true),
            Some(Err("connection reset".into())),
        )
        .unwrap();
        assert_eq!(l.profiles.len(), 1);
        assert_eq!(
            l.status_line().as_deref(),
            Some("Profile detection failed: connection reset")
        );
    }

    #[test]
    fn nothing_visible_is_not_an_error() {
        let l = assemble(schema(), &inputs(USER, true), Some(Ok(Sample::default()))).unwrap();
        assert_eq!(l.profiles.len(), 1);
        assert!(l.detection_error.is_none());
        assert!(l.status_line().is_none());
    }

    #[test]
    fn an_empty_directory_gets_useradd_style_assumptions() {
        let posix = "[[profile]]\nname = \"user\"\nobject_classes = [\"inetOrgPerson\", \"posixAccount\"]\nsearch_base = \"ou=people,dc=example,dc=org\"\n";
        let sample = Sample {
            group_ou: Some("ou=groups,dc=example,dc=org".into()),
            ..Default::default()
        };
        let l = assemble(schema(), &inputs(posix, true), Some(Ok(sample))).unwrap();
        let p = &l.profiles[0];
        assert!(p.companion.is_some());
        assert!(matches!(
            p.defaults.entries["uidNumber"],
            crate::config::defaults::DefaultValue::DetectedRange(_)
        ));
        // Detection off: no assumptions.
        let off = assemble(schema(), &inputs(posix, false), None).unwrap();
        assert!(off.profiles[0].companion.is_none() && off.profiles[0].defaults.entries.is_empty());
    }

    /// Spec §4: a detection that failed as a whole yields the config profiles
    /// only; rule D must not add a private group it has no evidence for.
    #[test]
    fn a_failed_detection_adds_no_private_group() {
        let posix = "[[profile]]\nname = \"user\"\nobject_classes = [\"inetOrgPerson\", \"posixAccount\"]\nsearch_base = \"ou=people,dc=example,dc=org\"\n";
        let l = assemble(schema(), &inputs(posix, true), Some(Err("timeout".into()))).unwrap();
        let p = &l.profiles[0];
        assert!(p.companion.is_none());
        assert!(!p.defaults.entries.contains_key("gidNumber"));
    }

    #[test]
    fn a_config_widget_error_is_still_a_load_error() {
        let bad =
            format!("{USER}[profile.widget.member]\nkind = \"picker\"\ncandidate = \"ghost\"\n");
        assert!(assemble(schema(), &inputs(&bad, true), Some(Ok(demo_sample()))).is_err());
    }
}
