//! Detected profile names (spec §2A "Name rules"): `<base>-<container RDN value>`,
//! extended by parent RDN values until unique. Depends only on DNs, so stable.

use crate::detect::model::DetectedProfile;

/// Lowercase; every run of characters outside `[a-z0-9]` becomes one `-`;
/// leading/trailing `-` dropped. May return "" (all characters dropped).
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for ch in s.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(ch);
        } else {
            pending_dash = true;
        }
    }
    out
}

/// The name base: the pattern's name (`user`, `posixgroup`, `group`, in that
/// precedence) or the slugged structural class.
pub fn base_name(structural: &str, object_classes: &[String]) -> String {
    let has = |oc: &str| {
        structural.eq_ignore_ascii_case(oc)
            || object_classes.iter().any(|c| c.eq_ignore_ascii_case(oc))
    };
    if has("posixAccount") {
        "user".to_string()
    } else if has("posixGroup") {
        "posixgroup".to_string()
    } else if has("groupOfNames") || has("groupOfUniqueNames") {
        "group".to_string()
    } else {
        slug(structural)
    }
}

/// Slugged RDN values of `dn`, leaf first. An empty slug becomes `x`.
fn rdn_slugs(dn: &str) -> Vec<String> {
    crate::detect::dn_components(dn)
        .iter()
        .map(|c| {
            let value = c.split_once('=').map(|(_, v)| v).unwrap_or(c);
            let s = slug(&value.replace('\\', ""));
            if s.is_empty() {
                "x".to_string()
            } else {
                s
            }
        })
        .collect()
}

/// Assign `name` to every profile: `<base>-<rdn value>`; while two names clash,
/// each clashing profile adds its next parent RDN value. Profiles that still
/// clash (same container, same base) get their structural class appended.
pub fn assign_names(profiles: &mut [DetectedProfile]) {
    fn clashes(names: &[String], i: usize) -> bool {
        names
            .iter()
            .enumerate()
            .any(|(j, other)| j != i && other == &names[i])
    }
    let n = profiles.len();
    let comps: Vec<Vec<String>> = profiles.iter().map(|p| rdn_slugs(&p.container)).collect();
    // Owned copies, so the closure below does not borrow `profiles`.
    let bases: Vec<String> = profiles.iter().map(|p| p.base_name.clone()).collect();
    let name_at = |i: usize, depth: usize| -> String {
        let parts = &comps[i][..depth.min(comps[i].len())];
        if parts.is_empty() {
            bases[i].clone()
        } else {
            format!("{}-{}", bases[i], parts.join("-"))
        }
    };
    let mut depth = vec![1usize; n];
    loop {
        let names: Vec<String> = (0..n).map(|i| name_at(i, depth[i])).collect();
        let mut changed = false;
        for i in 0..n {
            if clashes(&names, i) && depth[i] < comps[i].len() {
                depth[i] += 1;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut names: Vec<String> = (0..n).map(|i| name_at(i, depth[i])).collect();
    let still: Vec<bool> = (0..n).map(|i| clashes(&names, i)).collect();
    for i in 0..n {
        if still[i] {
            names[i] = format!("{}-{}", names[i], slug(&profiles[i].structural));
        }
    }
    for (p, name) in profiles.iter_mut().zip(names) {
        p.name = name;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::model::{Detected, DetectedProfile, Evidence};

    fn prof(base: &str, container: &str, structural: &str) -> DetectedProfile {
        DetectedProfile {
            name: String::new(),
            base_name: base.to_string(),
            structural: structural.to_string(),
            container: container.to_string(),
            sampled: 1,
            partial: false,
            object_classes: Detected::new(vec![structural.to_string()], Evidence::new(1, 1)),
            rdn_attr: Detected::new("cn".to_string(), Evidence::new(1, 1)),
            show: vec![],
            search_attrs: vec![],
            label: None,
            defaults: Default::default(),
            widgets: Default::default(),
            companion: None,
            assumed: Default::default(),
            private_groups: false,
            users_without_private_group: None,
            notes: vec![],
            entries: vec![],
        }
    }

    #[test]
    fn slug_lowercases_and_collapses_runs() {
        assert_eq!(slug("IT Staff"), "it-staff");
        assert_eq!(slug("  --People__2 "), "people-2");
        assert_eq!(slug("Ärger"), "rger");
        assert_eq!(slug("日本"), "");
    }

    #[test]
    fn base_name_prefers_patterns() {
        let ocs = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            base_name("inetOrgPerson", &ocs(&["inetOrgPerson", "posixAccount"])),
            "user"
        );
        assert_eq!(base_name("posixGroup", &ocs(&["posixGroup"])), "posixgroup");
        assert_eq!(base_name("groupOfNames", &ocs(&["groupOfNames"])), "group");
        assert_eq!(
            base_name("groupOfUniqueNames", &ocs(&["groupOfUniqueNames"])),
            "group"
        );
        assert_eq!(
            base_name("organizationalUnit", &ocs(&["organizationalUnit"])),
            "organizationalunit"
        );
    }

    #[test]
    fn names_carry_the_container_and_extend_on_collision() {
        let mut ps = vec![
            prof("user", "ou=people,o=a,dc=x", "inetOrgPerson"),
            prof("user", "ou=people,o=b,dc=x", "inetOrgPerson"),
            prof("user", "ou=IT Staff,dc=x", "inetOrgPerson"),
            prof("posixgroup", "ou=groups,dc=x", "posixGroup"),
        ];
        assign_names(&mut ps);
        let names: Vec<&str> = ps.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "user-people-a",
                "user-people-b",
                "user-it-staff",
                "posixgroup-groups"
            ]
        );
    }

    #[test]
    fn same_container_same_base_falls_back_to_the_structural_class() {
        let mut ps = vec![
            prof("user", "ou=people,dc=x", "inetOrgPerson"),
            prof("user", "ou=people,dc=x", "account"),
        ];
        assign_names(&mut ps);
        assert_eq!(ps[0].name, "user-people-x-inetorgperson");
        assert_eq!(ps[1].name, "user-people-x-account");
    }

    #[test]
    fn escaped_comma_in_container_rdn_is_part_of_the_value() {
        let mut ps = vec![prof("user", r"ou=Smith\, Jones,dc=x", "inetOrgPerson")];
        assign_names(&mut ps);
        assert_eq!(ps[0].name, "user-smith-jones");
    }
}
