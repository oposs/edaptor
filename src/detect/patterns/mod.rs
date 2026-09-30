//! Known patterns (spec §2B) applied to detected profiles.

pub mod pickers;
pub mod posix;
pub mod ranges;
pub mod samba;
pub mod templates;

use crate::detect::model::{DetectedProfile, Sample};
use crate::detect::private::PrivateIndex;
use crate::schema::SchemaModel;

/// Run every pattern over `profiles` (names are already assigned).
pub fn apply(
    schema: &SchemaModel,
    sample: &Sample,
    profiles: &mut [DetectedProfile],
    notes: &mut Vec<String>,
) {
    let all = posix::all_posix_entries(sample);
    let index = PrivateIndex::new(all.iter());
    let lookups_ok = sample.lookup_error.is_none();
    if let Some(e) = &sample.lookup_error {
        notes.push(format!(
            "private-group rules skipped: the private-group lookup failed ({e})"
        ));
    }
    for p in profiles.iter_mut() {
        // B1 for a posixGroup profile uses its shared (non-private) groups only.
        let b1_entries = if posix::is_group_profile(p) && lookups_ok {
            let shared = posix::non_private(p, &index);
            let private = p.entries.len() - shared.len();
            if private > 0 {
                p.notes.push(format!(
                    "{private} of {} groups are user-private groups; defaults and the number range use the other {}",
                    p.entries.len(),
                    shared.len()
                ));
            }
            shared
        } else {
            p.entries.clone()
        };
        let (defaults, dropped) = templates::infer_defaults(schema, &b1_entries, &p.rdn_attr.value);
        p.notes.extend(dropped);
        p.defaults.extend(defaults);
        if posix::is_user_profile(p) {
            if lookups_ok {
                p.users_without_private_group = Some(posix::count_without_private(p, &index));
            }
            if !(lookups_ok && posix::apply_private_group(p, &index)) {
                posix::apply_shared_gid(p);
            }
            posix::apply_login_shell(p);
        }
        samba::apply(p);
    }
    pickers::apply(profiles);
    ranges::apply(profiles);
}

#[cfg(test)]
mod tests {
    use crate::config::defaults::DefaultValue;
    use crate::config::{CandidateRef, WidgetSpecCfg};
    use crate::detect::fixtures::{argus_sample, container, demo_sample, e, schema};
    use crate::detect::infer::{detect, Detection};
    use crate::detect::model::{DetectedProfile, Sample};

    fn p<'a>(d: &'a Detection, name: &str) -> &'a DetectedProfile {
        d.profiles
            .iter()
            .find(|p| p.name == name)
            .unwrap_or_else(|| panic!("no {name}"))
    }
    fn dflt(p: &DetectedProfile, attr: &str) -> Option<String> {
        p.defaults.get(attr).map(|d| d.value.to_config_string())
    }
    fn cand(w: &WidgetSpecCfg) -> &str {
        match w {
            WidgetSpecCfg::Picker {
                candidate: CandidateRef::Profile(n),
                ..
            }
            | WidgetSpecCfg::Lookup {
                candidate: CandidateRef::Profile(n),
                ..
            } => n,
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn argus_users_get_private_groups_and_ranges() {
        let d = detect(&schema(), &argus_sample());
        let u = p(&d, "user-people");
        assert!(u.private_groups);
        assert_eq!(dflt(u, "gidNumber").as_deref(), Some("{uidNumber}"));
        assert_eq!(u.defaults["gidNumber"].evidence.ratio(), "12/12");
        let c = &u.companion.as_ref().expect("companion").value;
        assert_eq!(c.object_classes, vec!["posixGroup"]);
        assert_eq!(c.rdn_attr, "cn");
        assert_eq!(c.search_base, "ou=groups,dc=argus,dc=ch");
        assert_eq!(c.attributes["cn"], "{uid}");
        assert_eq!(c.attributes["gidNumber"], "{uidNumber}");
        assert_eq!(c.attributes["memberUid"], "{uid}");
        match &u.defaults["uidNumber"].value {
            DefaultValue::DetectedRange(s) => assert!(s.unified && !s.exclude_private),
            other => panic!("{other:?}"),
        }
        assert_eq!(cand(&u.widgets["gidNumber"].value), "posixgroup-groups");
        let g = p(&d, "posixgroup-groups");
        match &g.defaults["gidNumber"].value {
            DefaultValue::DetectedRange(s) => assert!(s.unified && s.exclude_private),
            other => panic!("{other:?}"),
        }
        assert_eq!(cand(&g.widgets["memberUid"].value), "user-people");
        // B1 over the 4 shared groups only: memberUid is never a default.
        assert!(!g.defaults.contains_key("memberUid"));
    }

    #[test]
    fn demo_shared_primary_group_samba_and_member_picker() {
        let d = detect(&schema(), &demo_sample());
        let up = p(&d, "user-people");
        assert!(!up.private_groups);
        assert_eq!(dflt(up, "gidNumber").as_deref(), Some("100"));
        assert_eq!(dflt(up, "sambaSID").as_deref(), Some("{auto:sambaSID}"));
        let uu = p(&d, "user-users");
        assert!(!uu.defaults.contains_key("gidNumber"));
        assert!(
            uu.notes
                .iter()
                .any(|n| n.contains("gidNumber = uidNumber for 3/3, but no private groups found")),
            "{:?}",
            uu.notes
        );
        assert_eq!(
            cand(&p(&d, "group-groups").widgets["member"].value),
            "user-people"
        );
        // Posix-user profile = the one with the most sampled entries.
        assert_eq!(
            cand(&p(&d, "posixgroup-groups").widgets["memberUid"].value),
            "user-people"
        );
    }

    #[test]
    fn reverse_lookup_classifies_a_group_whose_user_was_not_sampled() {
        let mut s = argus_sample();
        // Drop the users container; the accounts arrive through the reverse lookup.
        let users = s.containers.remove(1).entries;
        s.accounts = users;
        let d = detect(&schema(), &s);
        let g = p(&d, "posixgroup-groups");
        let all = super::posix::all_posix_entries(&s);
        let idx = crate::detect::private::PrivateIndex::new(all.iter());
        assert_eq!(super::posix::non_private(g, &idx).len(), 4);
        assert!(
            g.notes
                .iter()
                .any(|n| n.contains("12 of 16 groups are user-private groups")),
            "{:?}",
            g.notes
        );
    }

    #[test]
    fn companion_builder_is_shared() {
        let c = super::posix::private_group_companion("ou=groups,dc=x");
        assert_eq!(c.object_classes, vec!["posixGroup"]);
        assert_eq!(c.rdn_attr, "cn");
        assert_eq!(c.search_base, "ou=groups,dc=x");
        assert_eq!(c.attributes["memberUid"], "{uid}");
    }

    /// Private groups without their user as `memberUid` still get a companion
    /// that writes it: the hand-written configs all do.
    #[test]
    fn detected_companion_writes_member_uid_by_default() {
        let mut s = argus_sample();
        for c in &mut s.containers {
            for en in &mut c.entries {
                en.attrs.remove("memberUid");
            }
        }
        let d = detect(&schema(), &s);
        let c = &p(&d, "user-people")
            .companion
            .as_ref()
            .expect("companion")
            .value;
        assert_eq!(c.attributes["memberUid"], "{uid}");
    }

    #[test]
    fn contrary_evidence_is_counted_for_every_user_profile() {
        let d = detect(&schema(), &argus_sample());
        assert_eq!(p(&d, "user-people").users_without_private_group, Some(0));
        let d = detect(&schema(), &demo_sample());
        assert_eq!(p(&d, "user-users").users_without_private_group, Some(3));
        assert_eq!(p(&d, "posixgroup-groups").users_without_private_group, None);
    }

    #[test]
    fn a_failed_lookup_skips_b2_with_a_note() {
        let mut s = argus_sample();
        s.lookup_error = Some("timeout".into());
        let d = detect(&schema(), &s);
        assert!(p(&d, "user-people").companion.is_none());
        assert!(d.notes.iter().any(|n| n.contains("timeout")));
    }

    #[test]
    fn small_groups_keep_names_and_widgets_but_infer_no_values() {
        let base = "ou=few,dc=x";
        let mk = |u: &str, n: &str| {
            e(
                &format!("uid={u},{base}"),
                &[
                    ("objectClass", &["inetOrgPerson", "posixAccount"]),
                    ("uid", &[u]),
                    ("cn", &[u]),
                    ("sn", &["s"]),
                    ("uidNumber", &[n]),
                    ("gidNumber", &["100"]),
                    ("loginShell", &["/bin/bash"]),
                ],
            )
        };
        let grp = e(
            "cn=g,ou=grp,dc=x",
            &[
                ("objectClass", &["posixGroup"]),
                ("cn", &["g"]),
                ("gidNumber", &["100"]),
            ],
        );
        let s = Sample {
            containers: vec![
                container(base, vec![mk("a", "1"), mk("b", "2")]),
                container("ou=grp,dc=x", vec![grp]),
            ],
            ..Default::default()
        };
        let d = detect(&schema(), &s);
        let u = p(&d, "user-few");
        assert!(!u.defaults.contains_key("loginShell"));
        assert!(!u.defaults.contains_key("gidNumber"));
        assert_eq!(cand(&u.widgets["gidNumber"].value), "posixgroup-grp");
    }

    fn shell_options(p: &DetectedProfile) -> Vec<(String, String)> {
        match &p.widgets["loginShell"].value {
            WidgetSpecCfg::Choice {
                select,
                format,
                options,
            } => {
                assert_eq!((select.as_str(), format.as_str()), ("single", "plain"));
                options
                    .iter()
                    .map(|o| (o.value.clone(), o.label.clone()))
                    .collect()
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn user_profiles_get_a_login_shell_choice_with_the_shells_in_use() {
        let d = detect(&schema(), &argus_sample());
        let u = p(&d, "user-people");
        let opts = shell_options(u);
        let values: Vec<&str> = opts.iter().map(|(v, _)| v.as_str()).collect();
        assert_eq!(
            values,
            vec![
                "/bin/bash",
                "/bin/sh",
                "/bin/zsh",
                "/bin/false",
                "/sbin/nologin",
                "/bin/tcsh"
            ]
        );
        assert_eq!(opts[0].1, "Bash");
        assert_eq!(opts[5].1, "Tcsh");
        let ev = &u.widgets["loginShell"].evidence;
        assert_eq!(ev.ratio(), "12/12");
        assert!(ev.exceptions.is_empty());
        assert!(!p(&d, "posixgroup-groups")
            .widgets
            .contains_key("loginShell"));
    }

    #[test]
    fn login_shell_choice_puts_the_default_first_and_caps_extras() {
        let base = "ou=people,dc=x";
        let mut shells: Vec<String> = vec!["/bin/zsh".into(); 30];
        shells.extend(["/usr/bin/fish".into(), "/usr/bin/fish".into()]);
        shells.extend((1..=11).map(|i| format!("/opt/sh{i:02}")));
        shells.extend(["".into(), "bash".into(), "/bin/b ash".into()]);
        let entries = shells
            .iter()
            .enumerate()
            .map(|(i, sh)| {
                let uid = format!("u{i}");
                let num = (10000 + i).to_string();
                e(
                    &format!("uid={uid},{base}"),
                    &[
                        ("objectClass", &["inetOrgPerson", "posixAccount"]),
                        ("uid", &[uid.as_str()]),
                        ("cn", &[uid.as_str()]),
                        ("sn", &["s"]),
                        ("uidNumber", &[num.as_str()]),
                        ("gidNumber", &["100"]),
                        ("loginShell", &[sh.as_str()]),
                    ],
                )
            })
            .collect();
        let s = Sample {
            containers: vec![container(base, entries)],
            ..Default::default()
        };
        let d = detect(&schema(), &s);
        let u = p(&d, "user-people");
        assert_eq!(dflt(u, "loginShell").as_deref(), Some("/bin/zsh"));
        let opts = shell_options(u);
        let values: Vec<&str> = opts.iter().map(|(v, _)| v.as_str()).collect();
        let mut want = vec![
            "/bin/zsh",
            "/bin/bash",
            "/bin/sh",
            "/bin/false",
            "/sbin/nologin",
            "/usr/bin/fish",
        ];
        let extra: Vec<String> = (1..=9).map(|i| format!("/opt/sh{i:02}")).collect();
        want.extend(extra.iter().map(String::as_str));
        assert_eq!(values, want);
        assert_eq!(opts[0].1, "Zsh");
        assert_eq!(opts[5].1, "Fish");
        let ev = &u.widgets["loginShell"].evidence;
        // 30 zsh + 2 fish + 9 kept extras; sh10, sh11 and two garbage values
        // are exceptions; the empty value counts as no shell.
        assert_eq!(ev.ratio(), format!("41/{}", shells.len()));
        assert_eq!(ev.exceptions.len(), 4, "{:?}", ev.exceptions);
    }

    /// Extras whose basename label repeats another option's label carry the
    /// full path; built-in labels stay.
    #[test]
    fn colliding_login_shell_labels_show_the_path() {
        let base = "ou=people,dc=x";
        let shells = [
            "/bin/tcsh",
            "/bin/tcsh",
            "/usr/bin/tcsh",
            "/usr/bin/zsh",
            "/usr/bin/fish",
        ];
        let entries = shells
            .iter()
            .enumerate()
            .map(|(i, sh)| {
                let uid = format!("u{i}");
                e(
                    &format!("uid={uid},{base}"),
                    &[
                        ("objectClass", &["inetOrgPerson", "posixAccount"]),
                        ("uid", &[uid.as_str()]),
                        ("cn", &[uid.as_str()]),
                        ("sn", &["s"]),
                        ("loginShell", &[sh]),
                    ],
                )
            })
            .collect();
        let s = Sample {
            containers: vec![container(base, entries)],
            ..Default::default()
        };
        let d = detect(&schema(), &s);
        let opts = shell_options(p(&d, "user-people"));
        let label = |v: &str| {
            opts.iter()
                .find(|(ov, _)| ov == v)
                .map(|(_, l)| l.clone())
                .unwrap()
        };
        assert_eq!(label("/bin/tcsh"), "Tcsh (/bin/tcsh)");
        assert_eq!(label("/usr/bin/tcsh"), "Tcsh (/usr/bin/tcsh)");
        assert_eq!(label("/usr/bin/zsh"), "Zsh (/usr/bin/zsh)");
        assert_eq!(label("/bin/zsh"), "Zsh");
        assert_eq!(label("/usr/bin/fish"), "Fish");
    }

    #[test]
    fn lower_case_server_spelling_still_detects_private_groups() {
        let mut s = argus_sample();
        for c in &mut s.containers {
            for en in &mut c.entries {
                let ocs = en.attrs.remove("objectClass").unwrap();
                en.attrs.insert(
                    "objectclass".into(),
                    ocs.iter().map(|o| o.to_lowercase()).collect(),
                );
                if let Some(v) = en.attrs.remove("uidNumber") {
                    en.attrs.insert("UIDNUMBER".into(), v);
                }
            }
        }
        let d = detect(&schema(), &s);
        assert!(p(&d, "user-people").private_groups);
    }
}
