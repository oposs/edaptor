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
    /// Sampling and detection notes: routine, printed as `note:`.
    pub notes: Vec<String>,
    /// Config-caused messages (merge, `suppress`, dropped parts): `warning:`.
    pub warnings: Vec<String>,
    /// Detected parts dropped by validation (also in `warnings`).
    pub dropped: Vec<String>,
    /// Config widgets disabled because an incomplete detection missed their
    /// candidate profile (also in `warnings`).
    pub config_dropped: Vec<String>,
    pub detection_error: Option<String>,
}

impl LoadedProfiles {
    /// What the TUI prints on stderr before it starts: every warning, then one
    /// line summing up the routine notes (`edaptor profiles` lists them).
    pub fn startup_lines(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .warnings
            .iter()
            .map(|w| format!("warning: {w}"))
            .collect();
        match self.notes.len() {
            0 => {}
            1 => out.push("note: 1 detection note; run `edaptor profiles` to see it".into()),
            n => out.push(format!(
                "note: {n} detection notes; run `edaptor profiles` to see them"
            )),
        }
        out
    }

    /// The TUI status line after startup, when detection has something to say.
    pub fn status_line(&self) -> Option<String> {
        let disabled = match self.config_dropped.len() {
            0 => String::new(),
            1 => "; 1 config widget disabled, see the startup warnings".to_string(),
            n => format!("; {n} config widgets disabled, see the startup warnings"),
        };
        if let Some(e) = &self.detection_error {
            return Some(format!("Profile detection failed: {e}{disabled}"));
        }
        if !disabled.is_empty() {
            return Some(format!("Profile detection incomplete{disabled}"));
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
            warnings: Vec::new(),
            dropped: Vec::new(),
            config_dropped: Vec::new(),
            detection_error: None,
        });
    };
    let (detected, containers_sampled, notes, detection_error, group_ou, incomplete) = match sampled
    {
        Ok(s) => {
            let d = crate::detect::infer::detect(&schema, &s);
            let mut notes = s.notes;
            notes.extend(d.notes);
            let n = s.containers.len();
            (d.profiles, n, notes, None, s.group_ou, s.incomplete)
        }
        Err(e) => (Vec::new(), 0, Vec::new(), Some(e), None, false),
    };
    // Rule D runs whenever detection is enabled, also after a failed sample.
    let mut merged = merge_with_assumptions(
        &schema,
        &detected,
        &inputs.overrides,
        group_ou.as_deref(),
        detection_error.is_some(),
    );
    let why = match &detection_error {
        Some(e) => Some(format!("profile detection failed: {e}")),
        None => incomplete.then(|| "profile detection incomplete".to_string()),
    };
    validate(&mut merged, why.as_deref()).map_err(|e| anyhow!("profile config error: {e}"))?;
    let widgets = widgets_of(&merged.profiles)?;
    Ok(LoadedProfiles {
        schema,
        profiles: merged.profiles,
        widgets,
        provenance: merged.provenance,
        disabled: merged.disabled,
        detected,
        containers_sampled,
        notes,
        warnings: merged.warnings,
        dropped: merged.dropped,
        config_dropped: merged.config_dropped,
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

    const NEEDS_DETECTED: &str = "[[profile]]\nname = \"grp\"\nobject_classes = [\"posixGroup\"]\nsearch_base = \"ou=groups,dc=example,dc=org\"\n[profile.widget.memberUid]\nkind = \"picker\"\ncandidate = \"user-users\"\n";

    /// A config that names a detected profile starts after a failed detection:
    /// the widget is dropped and the status line names the failure.
    #[test]
    fn a_failed_detection_disables_a_widget_naming_a_detected_profile() {
        assert!(assemble(
            schema(),
            &inputs(NEEDS_DETECTED, true),
            Some(Ok(demo_sample()))
        )
        .is_ok());
        let l = assemble(
            schema(),
            &inputs(NEEDS_DETECTED, true),
            Some(Err("timeout".into())),
        )
        .expect("detection failure never stops eDAPtor");
        assert!(!l.profiles[0].widgets.contains_key("memberUid"));
        assert_eq!(
            l.status_line().as_deref(),
            Some("Profile detection failed: timeout; 1 config widget disabled, see the startup warnings")
        );
    }

    /// Containers only truncated at SAMPLE_SIZE still yield their profiles: a
    /// mistyped candidate stays fatal.
    #[test]
    fn a_truncated_but_complete_sample_keeps_an_unknown_candidate_fatal() {
        let mut sample = demo_sample();
        for c in &mut sample.containers {
            c.partial = true;
        }
        let bad = NEEDS_DETECTED.replace("user-users", "user-usrs");
        let err = assemble(schema(), &inputs(&bad, true), Some(Ok(sample)))
            .err()
            .expect("a mistyped candidate is a load error");
        assert!(
            err.to_string().contains("unknown candidate profile"),
            "{err}"
        );
    }

    /// The same after a sample that skipped containers.
    #[test]
    fn a_partial_sample_disables_a_widget_naming_a_missing_profile() {
        let sample = Sample {
            incomplete: true,
            ..Default::default()
        };
        let l = assemble(schema(), &inputs(NEEDS_DETECTED, true), Some(Ok(sample))).unwrap();
        assert!(!l.profiles[0].widgets.contains_key("memberUid"));
        assert_eq!(
            l.status_line().as_deref(),
            Some(
                "Profile detection incomplete; 1 config widget disabled, see the startup warnings"
            )
        );
    }

    /// Routine sampling notes are `note:` (one summary line in the TUI start);
    /// config-caused messages stay `warning:`.
    #[test]
    fn sampling_notes_and_config_warnings_are_kept_apart() {
        let ghost = "[[profile]]\nname = \"ghost\"\nsearch_base = \"ou=nowhere,dc=example,dc=org\"\nobject_classes = [\"inetOrgPerson\"]\nsuppress = [\"companion\"]\n";
        let sample = Sample {
            notes: vec!["nothing visible under the base (LDAP 32)".into()],
            ..Default::default()
        };
        let l = assemble(schema(), &inputs(ghost, true), Some(Ok(sample))).unwrap();
        assert_eq!(l.notes, vec!["nothing visible under the base (LDAP 32)"]);
        assert!(!l.warnings.is_empty(), "the bad suppress is config-caused");
        let lines = l.startup_lines();
        assert_eq!(
            lines.last().map(String::as_str),
            Some("note: 1 detection note; run `edaptor profiles` to see it")
        );
        assert!(lines[..lines.len() - 1]
            .iter()
            .all(|l| l.starts_with("warning: ")));
    }

    #[test]
    fn a_config_widget_error_is_still_a_load_error() {
        let bad =
            format!("{USER}[profile.widget.member]\nkind = \"picker\"\ncandidate = \"ghost\"\n");
        assert!(assemble(schema(), &inputs(&bad, true), Some(Ok(demo_sample()))).is_err());
    }
}
