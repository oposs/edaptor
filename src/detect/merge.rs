//! Merge config `[[profile]]` blocks over detected profiles (spec §3). Pure.

use std::collections::{BTreeMap, HashMap};

use crate::config::defaults::DefaultValue;
use crate::config::{CandidateRef, ContainerScope, EntryProfile, ProfileOverride, WidgetSpecCfg};
use crate::detect::dn_eq;
use crate::detect::model::{DetectedProfile, Evidence};
use crate::schema::SchemaModel;

#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    Config,
    Detected(Evidence),
    /// Filled by rule D (§2D); the string says why.
    Assumed(String),
    ConfigOverDetected {
        detected: String,
        evidence: Evidence,
    },
    /// A config default replaced a detected number range.
    ConfigOverDetectedRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    Config,
    Detected {
        container: String,
        sampled: usize,
        partial: bool,
    },
    Merged {
        detected: String,
        container: String,
        sampled: usize,
        partial: bool,
    },
}

#[derive(Debug, Clone)]
pub struct Provenance {
    pub name: String,
    pub origin: Origin,
    pub fields: BTreeMap<String, Source>,
    pub suppressed: Vec<String>,
    /// `suppress` paths that matched nothing yet. Rule D (Task 9) may still add
    /// the part; `flush_pending` retries them and warns about the rest.
    pub pending_suppress: Vec<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Default)]
pub struct Merged {
    pub profiles: Vec<EntryProfile>,
    /// Parallel to `profiles`.
    pub provenance: Vec<Provenance>,
    /// Names of profiles removed by `enabled = false`.
    pub disabled: Vec<String>,
    pub warnings: Vec<String>,
    /// Detected parts dropped by `validate` (also in `warnings`).
    pub dropped: Vec<String>,
    /// Config widgets dropped by `validate` because their candidate was not
    /// detected by an incomplete detection (also in `warnings`).
    pub config_dropped: Vec<String>,
}

impl Provenance {
    fn new(
        name: &str,
        origin: Origin,
        fields: BTreeMap<String, Source>,
        notes: Vec<String>,
    ) -> Self {
        Provenance {
            name: name.to_string(),
            origin,
            fields,
            suppressed: Vec::new(),
            pending_suppress: Vec::new(),
            notes,
        }
    }

    fn detected(p: &EntryProfile, d: &DetectedProfile) -> Self {
        let origin = Origin::Detected {
            container: d.container.clone(),
            sampled: d.sampled,
            partial: d.partial,
        };
        Provenance::new(&p.name, origin, detected_fields(d), d.notes.clone())
    }

    fn merged(p: &EntryProfile, d: &DetectedProfile, fields: BTreeMap<String, Source>) -> Self {
        let origin = Origin::Merged {
            detected: d.name.clone(),
            container: d.container.clone(),
            sampled: d.sampled,
            partial: d.partial,
        };
        Provenance::new(&p.name, origin, fields, d.notes.clone())
    }

    pub(crate) fn config(p: &EntryProfile) -> Self {
        Provenance::new(&p.name, Origin::Config, config_fields(p), Vec::new())
    }
}

fn candidate_mut(spec: &mut WidgetSpecCfg) -> Option<&mut CandidateRef> {
    match spec {
        WidgetSpecCfg::Picker { candidate, .. }
        | WidgetSpecCfg::Membership { candidate, .. }
        | WidgetSpecCfg::Lookup { candidate, .. } => Some(candidate),
        _ => None,
    }
}

fn candidate_name(spec: &WidgetSpecCfg) -> Option<&str> {
    match spec {
        WidgetSpecCfg::Picker {
            candidate: CandidateRef::Profile(n),
            ..
        }
        | WidgetSpecCfg::Membership {
            candidate: CandidateRef::Profile(n),
            ..
        }
        | WidgetSpecCfg::Lookup {
            candidate: CandidateRef::Profile(n),
            ..
        } => Some(n),
        _ => None,
    }
}

fn is_sentinel(name: &str) -> bool {
    name.len() > 1 && name.starts_with('_') && name.ends_with('_')
}

fn widget_kind(spec: &WidgetSpecCfg) -> &'static str {
    match spec {
        WidgetSpecCfg::Choice { .. } => "choice",
        WidgetSpecCfg::Password { .. } => "password",
        WidgetSpecCfg::Picker { .. } => "picker",
        WidgetSpecCfg::Membership { .. } => "membership",
        WidgetSpecCfg::Lookup { .. } => "lookup",
        WidgetSpecCfg::Readonly => "readonly",
        WidgetSpecCfg::XOrdered => "x_ordered",
        WidgetSpecCfg::SambaSid => "samba_sid",
    }
}

fn rewrite_candidates(
    widgets: &mut BTreeMap<String, WidgetSpecCfg>,
    rename: &HashMap<String, String>,
) {
    for spec in widgets.values_mut() {
        if let Some(CandidateRef::Profile(name)) = candidate_mut(spec) {
            if let Some(new) = rename.get(&name.to_lowercase()) {
                *name = new.clone();
            }
        }
    }
}

fn detected_fields(d: &DetectedProfile) -> BTreeMap<String, Source> {
    let whole = || Source::Detected(Evidence::new(d.sampled, d.sampled));
    let mut f = BTreeMap::new();
    f.insert(
        "object_classes".to_string(),
        Source::Detected(d.object_classes.evidence.clone()),
    );
    f.insert(
        "rdn_attr".to_string(),
        Source::Detected(d.rdn_attr.evidence.clone()),
    );
    f.insert("search_base".to_string(), whole());
    if !d.show.is_empty() {
        f.insert("show".to_string(), whole());
    }
    if !d.search_attrs.is_empty() {
        f.insert("search_attrs".to_string(), whole());
    }
    if d.label.is_some() {
        f.insert("label".to_string(), whole());
    }
    for (k, v) in &d.defaults {
        f.insert(
            format!("defaults.{k}"),
            Source::Detected(v.evidence.clone()),
        );
    }
    for (k, v) in &d.widgets {
        f.insert(format!("widget.{k}"), Source::Detected(v.evidence.clone()));
    }
    if let Some(c) = &d.companion {
        f.insert(
            "companion".to_string(),
            Source::Detected(c.evidence.clone()),
        );
    }
    f
}

fn config_fields(p: &EntryProfile) -> BTreeMap<String, Source> {
    let mut f = BTreeMap::new();
    for (k, set) in [
        ("object_classes", !p.object_classes.is_empty()),
        ("rdn_attr", !p.rdn_attr.is_empty()),
        ("search_base", !p.search_base.is_empty()),
        ("show", !p.show.is_empty()),
        ("search_attrs", !p.search_attrs.is_empty()),
        ("label", p.label.is_some()),
        ("companion", p.companion.is_some()),
    ] {
        if set {
            f.insert(k.to_string(), Source::Config);
        }
    }
    for k in p.defaults.entries.keys() {
        f.insert(format!("defaults.{k}"), Source::Config);
    }
    for k in p.widgets.keys() {
        f.insert(format!("widget.{k}"), Source::Config);
    }
    f
}

/// Record that `key` now comes from the config; remember the detected value.
fn mark_config(fields: &mut BTreeMap<String, Source>, key: &str, detected: Option<String>) {
    let src = match (fields.remove(key), detected) {
        (Some(Source::Detected(evidence)), Some(detected)) => {
            Source::ConfigOverDetected { detected, evidence }
        }
        _ => Source::Config,
    };
    fields.insert(key.to_string(), src);
}

fn take_ci<V>(map: &mut BTreeMap<String, V>, key: &str) -> Option<(String, V)> {
    let k = map.keys().find(|k| k.eq_ignore_ascii_case(key))?.clone();
    map.remove(&k).map(|v| (k, v))
}

/// Lay config attributes over detected ones (attribute names compare
/// case-insensitively; the config spelling wins), recording each as config
/// provenance under `<prefix>.<attr>`.
fn merge_attrs<V: Clone>(
    detected: &mut BTreeMap<String, V>,
    config: &BTreeMap<String, V>,
    fields: &mut BTreeMap<String, Source>,
    prefix: &str,
    describe: impl Fn(&V) -> String,
) {
    for (k, v) in config {
        let old = take_ci(detected, k);
        let key = format!("{prefix}.{k}");
        if let Some((old_key, _)) = &old {
            if let Some(s) = fields.remove(&format!("{prefix}.{old_key}")) {
                fields.insert(key.clone(), s);
            }
        }
        mark_config(fields, &key, old.map(|(_, old)| describe(&old)));
        detected.insert(k.clone(), v.clone());
    }
}

/// Apply every `suppress` path of `o`; the ones matching nothing (yet) stay pending.
fn apply_suppress(p: &mut EntryProfile, prov: &mut Provenance, o: &ProfileOverride) {
    for path in &o.suppress {
        if suppress(p, prov, path).is_err() {
            prov.pending_suppress.push(path.clone());
        }
    }
}

fn merge_one(
    d: &DetectedProfile,
    o: &ProfileOverride,
    rename: &HashMap<String, String>,
) -> (EntryProfile, Provenance) {
    let mut p = d.to_entry_profile();
    rewrite_candidates(&mut p.widgets, rename);
    let mut fields = detected_fields(d);
    p.name = o.name.clone();
    p.scope = ContainerScope::Boundary;
    if let Some(v) = &o.object_classes {
        mark_config(
            &mut fields,
            "object_classes",
            Some(format!("{:?}", p.object_classes)),
        );
        p.object_classes = v.clone();
    }
    if let Some(v) = &o.rdn_attr {
        mark_config(&mut fields, "rdn_attr", Some(format!("{:?}", p.rdn_attr)));
        p.rdn_attr = v.clone();
    }
    if let Some(v) = &o.search_base {
        mark_config(
            &mut fields,
            "search_base",
            Some(format!("{:?}", p.search_base)),
        );
        p.search_base = v.clone();
    }
    if let Some(v) = &o.show {
        mark_config(&mut fields, "show", Some(format!("{:?}", p.show)));
        p.show = v.clone();
    }
    if let Some(v) = &o.search_attrs {
        mark_config(
            &mut fields,
            "search_attrs",
            Some(format!("{:?}", p.search_attrs)),
        );
        p.search_attrs = v.clone();
    }
    if let Some(v) = &o.label {
        mark_config(
            &mut fields,
            "label",
            p.label.as_ref().map(|l| format!("{l:?}")),
        );
        p.label = Some(v.clone());
    }
    if let Some(defs) = &o.defaults {
        let ranges: Vec<String> = p
            .defaults
            .entries
            .iter()
            .filter(|(_, dv)| matches!(dv, DefaultValue::DetectedRange(_)))
            .map(|(k, _)| k.to_lowercase())
            .collect();
        merge_attrs(
            &mut p.defaults.entries,
            &defs.entries,
            &mut fields,
            "defaults",
            |dv| format!("{:?}", dv.to_config_string()),
        );
        // A detected range has no config spelling worth quoting.
        for (k, src) in fields.iter_mut() {
            let over_range = k
                .strip_prefix("defaults.")
                .is_some_and(|a| ranges.contains(&a.to_lowercase()));
            if over_range && matches!(src, Source::ConfigOverDetected { .. }) {
                *src = Source::ConfigOverDetectedRange;
            }
        }
    }
    if let Some(ws) = &o.widgets {
        merge_attrs(&mut p.widgets, ws, &mut fields, "widget", |w| {
            format!("kind {:?}", widget_kind(w))
        });
    }
    if let Some(c) = &o.companion {
        mark_config(
            &mut fields,
            "companion",
            p.companion.as_ref().map(|_| "a companion".to_string()),
        );
        p.companion = Some(c.clone());
    }
    let base = p.search_base.clone();
    for dv in p.defaults.entries.values_mut() {
        if let DefaultValue::DetectedRange(spec) = dv {
            spec.container = base.clone();
        }
    }
    let mut prov = Provenance::merged(&p, d, fields);
    apply_suppress(&mut p, &mut prov, o);
    (p, prov)
}

fn nothing(who: &str, path: &str) -> String {
    format!("profile \"{who}\": suppress \"{path}\" matches nothing detected")
}

/// Remove one detected part. Config parts are never removed.
fn suppress(p: &mut EntryProfile, prov: &mut Provenance, path: &str) -> Result<(), String> {
    let who = p.name.clone();
    let key = match path {
        "companion" | "label" | "show" | "search_attrs" => path.to_string(),
        _ => match path.split_once('.') {
            Some(("defaults", attr)) => match p.defaults.entries.keys().find(|k| k.eq_ignore_ascii_case(attr)) {
                Some(k) => format!("defaults.{k}"),
                None => return Err(nothing(&who, path)),
            },
            Some(("widget", attr)) => match p.widgets.keys().find(|k| k.eq_ignore_ascii_case(attr)) {
                Some(k) => format!("widget.{k}"),
                None => return Err(nothing(&who, path)),
            },
            _ => {
                return Err(format!(
                    "profile \"{who}\": unknown suppress path \"{path}\" (use companion, label, show, search_attrs, defaults.<attr> or widget.<attr>)"
                ))
            }
        },
    };
    if !matches!(
        prov.fields.get(&key),
        Some(Source::Detected(_) | Source::Assumed(_))
    ) {
        return Err(nothing(&who, path));
    }
    match key.split_once('.') {
        Some(("defaults", attr)) => {
            p.defaults.entries.remove(attr);
        }
        Some(("widget", attr)) => {
            p.widgets.remove(attr);
        }
        _ => match key.as_str() {
            "companion" => p.companion = None,
            "label" => p.label = None,
            "show" => p.show.clear(),
            "search_attrs" => p.search_attrs.clear(),
            _ => {}
        },
    }
    prov.fields.remove(&key);
    prov.suppressed.push(path.to_string());
    Ok(())
}

/// Merge (spec §3 "Matching", "Merge rules", §2A "Order"). Suppress paths that
/// matched nothing become warnings.
pub fn merge(
    schema: &SchemaModel,
    detected: &[DetectedProfile],
    overrides: &[ProfileOverride],
) -> Merged {
    let mut m = merge_core(schema, detected, overrides);
    flush_pending(&mut m);
    m
}

/// Retry every pending suppress path; the ones that still match nothing (or name
/// an unknown path) become warnings.
pub(crate) fn flush_pending(m: &mut Merged) {
    for (p, prov) in m.profiles.iter_mut().zip(m.provenance.iter_mut()) {
        for path in std::mem::take(&mut prov.pending_suppress) {
            if let Err(w) = suppress(p, prov, &path) {
                m.warnings.push(w);
            }
        }
    }
}

/// The merge without the final suppress flush (rule D runs in between).
pub(crate) fn merge_core(
    schema: &SchemaModel,
    detected: &[DetectedProfile],
    overrides: &[ProfileOverride],
) -> Merged {
    let mut out = Merged::default();
    let mut taken: Vec<Option<usize>> = vec![None; overrides.len()];
    let mut owner: Vec<Option<usize>> = vec![None; detected.len()];
    // Pass 1: names.
    for (oi, o) in overrides.iter().enumerate() {
        if let Some(di) = detected
            .iter()
            .position(|d| d.name.eq_ignore_ascii_case(&o.name))
        {
            match owner[di] {
                None => {
                    owner[di] = Some(oi);
                    taken[oi] = Some(di);
                }
                Some(prev) => out.warnings.push(format!(
                    "profile \"{}\" also matches detected \"{}\", already merged into \"{}\"",
                    o.name, detected[di].name, overrides[prev].name
                )),
            }
        }
    }
    // Pass 2: search_base + structural class, among the free detected profiles.
    for (oi, o) in overrides.iter().enumerate() {
        if taken[oi].is_some() {
            continue;
        }
        let (Some(base), Some(ocs)) = (&o.search_base, &o.object_classes) else {
            continue;
        };
        let Some(structural) = schema.structural_class(ocs) else {
            continue;
        };
        let cands: Vec<usize> = detected
            .iter()
            .enumerate()
            .filter(|(_, d)| {
                dn_eq(&d.container, base) && d.structural.eq_ignore_ascii_case(&structural)
            })
            .map(|(i, _)| i)
            .collect();
        match cands.iter().find(|di| owner[**di].is_none()) {
            Some(&di) => {
                owner[di] = Some(oi);
                taken[oi] = Some(di);
            }
            None => {
                if let Some(&di) = cands.first() {
                    let prev = owner[di]
                        .map(|p| overrides[p].name.clone())
                        .unwrap_or_default();
                    out.warnings.push(format!(
                        "profile \"{}\" also matches detected \"{}\", already merged into \"{prev}\"",
                        o.name, detected[di].name
                    ));
                }
            }
        }
    }
    let rename: HashMap<String, String> = overrides
        .iter()
        .zip(&taken)
        .filter_map(|(o, t)| t.map(|di| (detected[di].name.to_lowercase(), o.name.clone())))
        .collect();
    // Config profiles, file order.
    for (oi, o) in overrides.iter().enumerate() {
        if !o.is_enabled() {
            out.disabled.push(o.name.clone());
            continue;
        }
        match taken[oi] {
            Some(di) => {
                let (p, prov) = merge_one(&detected[di], o, &rename);
                out.profiles.push(p);
                out.provenance.push(prov);
            }
            None => match o.to_entry_profile() {
                Some(mut p) => {
                    let mut prov = Provenance::config(&p);
                    apply_suppress(&mut p, &mut prov, o);
                    out.profiles.push(p);
                    out.provenance.push(prov);
                }
                None => out.warnings.push(format!(
                    "profile \"{}\" matches no detected profile",
                    o.name
                )),
            },
        }
    }
    // Detected-only: more object classes first, then name.
    let mut rest: Vec<usize> = (0..detected.len())
        .filter(|di| owner[*di].is_none())
        .collect();
    rest.sort_by(|a, b| {
        let (da, db) = (&detected[*a], &detected[*b]);
        db.object_classes
            .value
            .len()
            .cmp(&da.object_classes.value.len())
            .then_with(|| da.name.cmp(&db.name))
    });
    for di in rest {
        let d = &detected[di];
        let mut p = d.to_entry_profile();
        rewrite_candidates(&mut p.widgets, &rename);
        out.provenance.push(Provenance::detected(&p, d));
        out.profiles.push(p);
    }
    out
}

/// Per-origin validation (spec §3 "Validation"): a failing detected part is
/// dropped and noted; a failing config part is an error. `incomplete` names why
/// detection may have missed profiles (failed, cut short); then a config widget
/// whose candidate is unknown is dropped with a warning instead, because the
/// same config is valid whenever detection completes.
pub fn validate(m: &mut Merged, incomplete: Option<&str>) -> Result<(), String> {
    let names: Vec<String> = m.profiles.iter().map(|p| p.name.to_lowercase()).collect();
    for (p, prov) in m.profiles.iter_mut().zip(m.provenance.iter_mut()) {
        let mut drop: Vec<(String, String)> = Vec::new();
        let mut config_drop: Vec<(String, String)> = Vec::new();
        for (attr, spec) in &p.widgets {
            let Some(name) = candidate_name(spec) else {
                continue;
            };
            if is_sentinel(name) || names.contains(&name.to_lowercase()) {
                continue;
            }
            if matches!(
                prov.fields.get(&format!("widget.{attr}")),
                Some(Source::Detected(_) | Source::Assumed(_))
            ) {
                drop.push((
                    attr.clone(),
                    format!("profile \"{}\": dropped detected widget.{attr}: unknown candidate profile \"{name}\"", p.name),
                ));
            } else if let Some(why) = incomplete {
                config_drop.push((
                    attr.clone(),
                    format!("profile \"{}\": disabled [profile.widget.{attr}]: unknown candidate profile \"{name}\" ({why})", p.name),
                ));
            } else {
                return Err(format!(
                    "profile \"{}\" [profile.widget.{attr}]: unknown candidate profile \"{name}\"",
                    p.name
                ));
            }
        }
        for (attr, note) in drop {
            p.widgets.remove(&attr);
            prov.fields.remove(&format!("widget.{attr}"));
            prov.notes.push(note.clone());
            m.warnings.push(note.clone());
            m.dropped.push(note);
        }
        for (attr, note) in config_drop {
            p.widgets.remove(&attr);
            prov.fields.remove(&format!("widget.{attr}"));
            prov.notes.push(note.clone());
            m.warnings.push(note.clone());
            m.config_dropped.push(note);
        }
        if let Some(c) = &p.companion {
            let who = format!("profile '{}' companion", p.name);
            if let Err(e) = crate::config::check_companion(&who, c) {
                if matches!(
                    prov.fields.get("companion"),
                    Some(Source::Detected(_) | Source::Assumed(_))
                ) {
                    let note = format!("dropped detected companion: {e}");
                    p.companion = None;
                    prov.fields.remove("companion");
                    prov.notes.push(note.clone());
                    m.warnings.push(note.clone());
                    m.dropped.push(note);
                } else {
                    return Err(e);
                }
            }
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::fixtures::{argus_sample, demo_sample, overrides, schema};
    use crate::detect::infer::detect;

    fn demo() -> Vec<DetectedProfile> {
        detect(&schema(), &demo_sample()).profiles
    }
    fn names(m: &Merged) -> Vec<&str> {
        m.profiles.iter().map(|p| p.name.as_str()).collect()
    }
    fn get<'a>(m: &'a Merged, name: &str) -> (&'a EntryProfile, &'a Provenance) {
        let i = m
            .profiles
            .iter()
            .position(|p| p.name == name)
            .unwrap_or_else(|| panic!("no {name}"));
        (&m.profiles[i], &m.provenance[i])
    }

    #[test]
    fn detected_only_order_and_scope() {
        let m = merge(&schema(), &demo(), &[]);
        let n = names(&m);
        let pos = |x: &str| n.iter().position(|y| *y == x).unwrap();
        assert!(pos("user-people") < pos("user-users"), "{n:?}");
        assert!(m.profiles.iter().all(|p| p.scope == ContainerScope::Exact));
        assert!(matches!(m.provenance[0].origin, Origin::Detected { .. }));
    }

    #[test]
    fn match_by_name_is_case_insensitive_and_keeps_the_config_name() {
        let m = merge(&schema(), &demo(), &overrides("[[profile]]\nname = \"USER-People\"\n[profile.defaults]\nloginShell = \"/bin/sh\"\n"));
        assert_eq!(m.profiles[0].name, "USER-People");
        assert_eq!(m.profiles[0].scope, ContainerScope::Boundary);
        assert!(!names(&m).contains(&"user-people"));
        let (p, prov) = get(&m, "USER-People");
        assert_eq!(
            p.defaults.entries["loginShell"].to_config_string(),
            "/bin/sh"
        );
        assert!(matches!(
            prov.fields["defaults.loginShell"],
            Source::ConfigOverDetected { .. }
        ));
        assert!(matches!(
            prov.fields["defaults.homeDirectory"],
            Source::Detected(_)
        ));
    }

    #[test]
    fn match_by_search_base_and_structural_class() {
        let cfg: crate::config::Config =
            toml::from_str(include_str!("../../examples/demo-config.toml")).unwrap();
        let m = merge(&schema(), &demo(), &cfg.overrides);
        let n = names(&m);
        assert_eq!(&n[..3], &["user", "group", "posixgroup"]);
        assert!(
            !n.contains(&"user-people")
                && !n.contains(&"group-groups")
                && !n.contains(&"posixgroup-groups")
        );
        let (user, _) = get(&m, "user");
        assert_eq!(
            user.defaults.entries["uidNumber"].to_config_string(),
            "{next:10000-60000}"
        );
        assert_eq!(
            user.defaults.entries["sambaSID"].to_config_string(),
            "{auto:sambaSID}"
        );
        assert!(matches!(
            user.widgets["gidNumber"],
            WidgetSpecCfg::Lookup { .. }
        ));
    }

    #[test]
    fn rename_rewrites_detected_candidates() {
        let mut m = merge(&schema(), &demo(), &overrides(
            "[[profile]]\nname = \"people\"\nobject_classes = [\"inetOrgPerson\", \"posixAccount\"]\nsearch_base = \"ou=people,dc=example,dc=org\"\n",
        ));
        let (g, _) = get(&m, "posixgroup-groups");
        match &g.widgets["memberUid"] {
            WidgetSpecCfg::Picker {
                candidate: CandidateRef::Profile(n),
                ..
            } => assert_eq!(n, "people"),
            other => panic!("{other:?}"),
        }
        validate(&mut m, None).unwrap();
        crate::config::widget::resolve_widgets(&m.profiles).expect("no widget config error");
    }

    #[test]
    fn companion_is_replaced_as_a_whole() {
        let d = detect(&schema(), &argus_sample()).profiles;
        let m = merge(&schema(), &d, &overrides(
            "[[profile]]\nname = \"user-people\"\n[profile.companion]\nobject_classes = [\"posixGroup\"]\nrdn_attr = \"cn\"\nsearch_base = \"ou=other,dc=argus,dc=ch\"\n[profile.companion.attributes]\ncn = \"{uid}\"\n",
        ));
        let (p, prov) = get(&m, "user-people");
        let c = p.companion.as_ref().unwrap();
        assert_eq!(c.search_base, "ou=other,dc=argus,dc=ch");
        assert!(!c.attributes.contains_key("gidNumber"));
        assert!(matches!(
            prov.fields["companion"],
            Source::ConfigOverDetected { .. }
        ));
    }

    #[test]
    fn suppress_each_path_kind() {
        let d = detect(&schema(), &argus_sample()).profiles;
        let m = merge(&schema(), &d, &overrides(
            "[[profile]]\nname = \"user-people\"\nsuppress = [\"companion\", \"defaults.loginShell\", \"widget.gidNumber\", \"label\", \"show\", \"search_attrs\", \"defaults.nope\", \"bogus\"]\n",
        ));
        let (p, prov) = get(&m, "user-people");
        assert!(p.companion.is_none());
        assert!(!p.defaults.entries.contains_key("loginShell"));
        assert!(!p.widgets.contains_key("gidNumber"));
        assert!(p.label.is_none() && p.show.is_empty() && p.search_attrs.is_empty());
        assert_eq!(prov.suppressed.len(), 6);
        assert!(m
            .warnings
            .iter()
            .any(|w| w.contains("\"defaults.nope\" matches nothing detected")));
        assert!(m
            .warnings
            .iter()
            .any(|w| w.contains("unknown suppress path \"bogus\"")));
    }

    #[test]
    fn suppress_never_removes_config_parts() {
        let m = merge(&schema(), &demo(), &overrides(
            "[[profile]]\nname = \"user-people\"\nsuppress = [\"defaults.loginShell\"]\n[profile.defaults]\nloginShell = \"/bin/sh\"\n",
        ));
        let (p, _) = get(&m, "user-people");
        assert_eq!(
            p.defaults.entries["loginShell"].to_config_string(),
            "/bin/sh"
        );
        assert!(m
            .warnings
            .iter()
            .any(|w| w.contains("matches nothing detected")));
    }

    #[test]
    fn enabled_false_drops_the_profile() {
        let m = merge(
            &schema(),
            &demo(),
            &overrides("[[profile]]\nname = \"user-users\"\nenabled = false\n"),
        );
        assert!(!names(&m).contains(&"user-users"));
        assert_eq!(m.disabled, vec!["user-users"]);
    }

    #[test]
    fn unmatched_block_without_object_classes_is_dropped_with_a_warning() {
        let m = merge(
            &schema(),
            &demo(),
            &overrides("[[profile]]\nname = \"ghost\"\n"),
        );
        assert!(!names(&m).contains(&"ghost"));
        assert!(m
            .warnings
            .iter()
            .any(|w| w == "profile \"ghost\" matches no detected profile"));
    }

    #[test]
    fn two_config_blocks_for_one_detected_profile() {
        let m = merge(&schema(), &demo(), &overrides(
            "[[profile]]\nname = \"staff\"\nobject_classes = [\"inetOrgPerson\"]\nsearch_base = \"ou=people,dc=example,dc=org\"\n[[profile]]\nname = \"samba-staff\"\nobject_classes = [\"inetOrgPerson\", \"sambaSamAccount\"]\nsearch_base = \"ou=people,dc=example,dc=org\"\n",
        ));
        assert!(names(&m).starts_with(&["staff", "samba-staff"]));
        assert!(matches!(get(&m, "samba-staff").1.origin, Origin::Config));
        assert!(m.warnings.iter().any(|w| w.contains(
            "\"samba-staff\" also matches detected \"user-people\", already merged into \"staff\""
        )));
    }

    #[test]
    fn config_search_base_wins_and_the_range_follows_it() {
        let m = merge(&schema(), &demo(), &overrides("[[profile]]\nname = \"user-people\"\nsearch_base = \"ou=staff,dc=example,dc=org\"\n"));
        let (p, _) = get(&m, "user-people");
        assert_eq!(p.search_base, "ou=staff,dc=example,dc=org");
        match &p.defaults.entries["uidNumber"] {
            DefaultValue::DetectedRange(s) => assert_eq!(s.container, "ou=staff,dc=example,dc=org"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn validate_drops_detected_parts_but_rejects_config_parts() {
        let mut d = demo();
        let i = d
            .iter()
            .position(|p| p.name == "posixgroup-groups")
            .unwrap();
        d[i].widgets.insert(
            "memberUid".into(),
            crate::detect::model::Detected::new(
                WidgetSpecCfg::Picker {
                    candidate: CandidateRef::Profile("ghost".into()),
                    store: "uid".into(),
                    select: "multi".into(),
                },
                Evidence::new(1, 1),
            ),
        );
        let mut m = merge(&schema(), &d, &[]);
        validate(&mut m, None).unwrap();
        assert!(!get(&m, "posixgroup-groups")
            .0
            .widgets
            .contains_key("memberUid"));
        assert_eq!(m.dropped.len(), 1);
        let mut bad = merge(&schema(), &demo(), &overrides(
            "[[profile]]\nname = \"x\"\nobject_classes = [\"posixGroup\"]\n[profile.widget.memberUid]\nkind = \"picker\"\ncandidate = \"ghost\"\n",
        ));
        assert!(validate(&mut bad, None)
            .unwrap_err()
            .contains("unknown candidate profile \"ghost\""));
    }

    /// After an incomplete detection, a config widget naming a profile that was
    /// not detected this time is dropped with a warning naming the cause, not a
    /// fatal error: the same config works when detection completes.
    #[test]
    fn an_incomplete_detection_drops_an_unknown_config_candidate() {
        let mut m = merge(&schema(), &[], &overrides(
            "[[profile]]\nname = \"x\"\nobject_classes = [\"posixGroup\"]\n[profile.widget.memberUid]\nkind = \"picker\"\ncandidate = \"user-people\"\n",
        ));
        validate(&mut m, Some("profile detection failed: timeout")).unwrap();
        let (p, _) = get(&m, "x");
        assert!(!p.widgets.contains_key("memberUid"));
        assert_eq!(m.config_dropped.len(), 1);
        let w = &m.config_dropped[0];
        assert!(
            w.contains("[profile.widget.memberUid]")
                && w.contains("\"user-people\"")
                && w.contains("profile detection failed: timeout"),
            "{w}"
        );
        assert!(m.warnings.contains(w));
    }
}
