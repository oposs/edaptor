//! Pure save-flow domain logic: validate + diff a single-entry save, mask
//! preview secrets, compute membership fan-out, decide number allocation, and
//! compose renamed DNs. No terminal, no network, no UI types.

use crate::form::changeset::{diff, ChangeSet, EditEntry, ModOp};
use crate::form::validate::{plan_save, validate, SavePlan, ValidationError};
use crate::ldap::ldif::render_changeset;
use crate::schema::SchemaModel;

/// The outcome of preparing a form save.
pub enum PrepareSave {
    /// Client-side validation failed.
    Invalid(Vec<ValidationError>),
    /// The diff could not be computed (e.g. multi-valued RDN).
    DiffError(String),
    /// The edited entry equals the baseline — nothing to do.
    NoChanges,
    /// A ready plan, its target DN, and the LDIF preview.
    Ready {
        /// The save plan to submit.
        plan: SavePlan,
        /// The (old) DN the plan targets.
        dn: String,
        /// LDIF preview text for the confirmation overlay.
        ldif: String,
    },
}

/// A copy of `cs` with the values of any `Add`/`Replace` touching a masked
/// attribute replaced by `********`, for the confirm preview — never show a
/// cleartext password or NT hash. `sambaPwdLastSet` is not secret and is left
/// intact (it is not in `mask_attrs`). Pure.
///
/// `mask_attrs` is the password flow's set of primary + derived attributes.
/// `secret_attrs` is the set of intrinsically secret attributes derived from the
/// form's field flags — defence in depth so a secret can never appear in clear in
/// the preview no matter how it entered the changeset.
pub fn mask_changeset_secrets(
    cs: &ChangeSet,
    mask_attrs: &[String],
    secret_attrs: &[String],
) -> ChangeSet {
    let is_masked = |attr: &str| {
        mask_attrs.iter().any(|a| a.eq_ignore_ascii_case(attr))
            || secret_attrs.iter().any(|a| a.eq_ignore_ascii_case(attr))
    };
    let mut out = cs.clone();
    for m in &mut out.mods {
        match m {
            ModOp::Replace { attr, values } | ModOp::Add { attr, values } if is_masked(attr) => {
                *values = vec!["********".to_string()];
            }
            _ => {}
        }
    }
    out
}

/// Validate + diff the edited entry against the `original` (baseline) and, if
/// there is a real change, return a ready [`SavePlan`] with an LDIF preview.
///
/// `password_mods` (REPLACE ops produced by the edit-password path) are folded
/// into the changeset so one source of truth drives both the plan and the
/// preview — a password-only edit (empty attribute diff) is still a change.
/// `mask_attrs` lists the attributes whose values to mask in the preview LDIF.
/// `secret_attrs` lists attributes that are intrinsically secret (from form field
/// flags) and must also be masked in the preview regardless of `mask_attrs`.
/// `x_ordered_attrs` is the caller-supplied set of X-ORDERED attribute names
/// (derived from form field `ordered` flags).
#[allow(clippy::too_many_arguments)]
pub fn prepare_save(
    schema: &SchemaModel,
    original: &EditEntry,
    edited: &EditEntry,
    object_classes: &[String],
    password_mods: &[ModOp],
    mask_attrs: &[String],
    secret_attrs: &[String],
    orphaned_attrs: &[&str],
    x_ordered_attrs: &std::collections::HashSet<String>,
) -> PrepareSave {
    let oc_refs: Vec<&str> = object_classes.iter().map(|s| s.as_str()).collect();
    let errors = validate(edited, schema, &oc_refs, orphaned_attrs);
    if !errors.is_empty() {
        return PrepareSave::Invalid(errors);
    }
    let mut cs = match diff(original, edited, x_ordered_attrs) {
        Ok(cs) => cs,
        Err(e) => return PrepareSave::DiffError(e.to_string()),
    };
    cs.mods.extend(password_mods.iter().cloned());
    if cs.is_empty() {
        return PrepareSave::NoChanges;
    }
    let ldif = render_changeset(&mask_changeset_secrets(&cs, mask_attrs, secret_attrs));
    PrepareSave::Ready {
        plan: plan_save(cs),
        dn: original.dn.clone(),
        ldif,
    }
}

/// Derive the password mods for an edit save from a staged `pending` cleartext.
///
/// Always strips `primary` and every `derived` attribute from BOTH `original`
/// and `edited` (case-insensitive) so the plain attribute diff can never
/// double-write them — the directory's stored hash on the baseline would
/// otherwise diff to a spurious Delete, and a derived value present on both
/// sides would shadow the REPLACE. When `pending` is `Some`, returns the REPLACE
/// mods produced by [`crate::samba::password::password_add_attrs`] together with
/// the attrs to mask in the preview (`primary` + `derived`). When `pending` is
/// `None`, only the strip happens and both returned vecs are empty. `now_secs` is
/// injected for testability. Pure (no clock, no I/O).
pub fn stage_pending_password(
    pending: Option<&str>,
    primary: &str,
    derived: &[String],
    samba: bool,
    now_secs: u64,
    original: &mut std::collections::BTreeMap<String, Vec<String>>,
    edited: &mut std::collections::BTreeMap<String, Vec<String>>,
) -> (Vec<ModOp>, Vec<String>) {
    let strip = |m: &mut std::collections::BTreeMap<String, Vec<String>>| {
        m.retain(|k, _| {
            !k.eq_ignore_ascii_case(primary) && !derived.iter().any(|d| d.eq_ignore_ascii_case(k))
        });
    };
    strip(original);
    strip(edited);
    let Some(pw) = pending else {
        return (Vec::new(), Vec::new());
    };
    let mods: Vec<ModOp> = crate::samba::password::password_add_attrs(pw, primary, samba, now_secs)
        .into_iter()
        .map(|(attr, values)| ModOp::Replace { attr, values })
        .collect();
    let mut mask = vec![primary.to_string()];
    mask.extend(derived.iter().cloned());
    (mods, mask)
}

/// The parent DN (everything after the first comma), or `None` at the top.
pub fn parent_dn(dn: &str) -> Option<&str> {
    dn.split_once(',').map(|(_, rest)| rest)
}

/// Compose the post-rename DN: `<new_rdn>,<parent of old_dn>`.
pub fn compose_renamed_dn(old_dn: &str, new_rdn: &str) -> String {
    match parent_dn(old_dn) {
        Some(container) => format!("{new_rdn},{container}"),
        None => new_rdn.to_string(),
    }
}

/// Decide an allocation from a (possibly truncated) directory scan. Refuses when
/// the scan was truncated by a server limit — never allocates over a partial set
/// (a silent duplicate would be worse than a constraint violation).
pub fn decide_allocation(
    values: &[u64],
    truncated: bool,
    min: u64,
    max: u64,
) -> Result<u64, String> {
    if truncated {
        return Err(
            "refusing to allocate: the number scan hit a server size limit \
             (bind with a higher-limit identity or configure a counter)"
                .to_string(),
        );
    }
    crate::config::defaults::next_in_range(values, min, max)
}

/// Per-holder MODIFYs for a membership change on the candidate's back-ref field.
/// `entry_dn` is the candidate (user) DN written into each holder's `holder_attr`.
/// Added groups get an Add; removed groups get a Delete. Order: adds, then deletes.
pub fn membership_fanout(
    entry_dn: &str,
    baseline: &[String],
    selected: &[String],
    holder_attr: &str,
) -> Vec<(String, ModOp)> {
    let has = |set: &[String], dn: &str| set.iter().any(|x| x.eq_ignore_ascii_case(dn));
    let mut out = Vec::new();
    for g in selected {
        if !has(baseline, g) {
            out.push((
                g.clone(),
                ModOp::Add {
                    attr: holder_attr.to_string(),
                    values: vec![entry_dn.to_string()],
                },
            ));
        }
    }
    for g in baseline {
        if !has(selected, g) {
            out.push((
                g.clone(),
                ModOp::Delete {
                    attr: holder_attr.to_string(),
                    values: vec![entry_dn.to_string()],
                },
            ));
        }
    }
    out
}

/// True when removing `member` would leave the group with no members (groupOfNames
/// requires ≥1). Only fires when `member` is the SOLE current member. False for
/// empty input (the group is already empty — not our removal's fault).
pub fn would_empty(current_members: &[String], member: &str) -> bool {
    current_members.len() == 1 && current_members[0].eq_ignore_ascii_case(member)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflows::test_fixtures::user_schema;
    use std::collections::BTreeMap;

    #[test]
    fn mask_changeset_secrets_masks_secret_attrs_even_without_mask_list() {
        // Defense in depth: a secret attribute that reaches a changeset by ANY
        // path (not just the password flow) must never appear cleartext in the
        // preview — the caller passes `secret_attrs` derived from form field flags.
        let cs = ChangeSet {
            dn: "uid=jsmith,ou=people,dc=example,dc=org".into(),
            modrdn: None,
            mods: vec![
                ModOp::Replace {
                    attr: "sambaNTPassword".into(),
                    values: vec!["hunter2".into()],
                },
                ModOp::Replace {
                    attr: "cn".into(),
                    values: vec!["James".into()],
                },
            ],
        };
        let secret_attrs = vec!["sambaNTPassword".to_string()];
        let masked = mask_changeset_secrets(&cs, &[], &secret_attrs);
        let val = |attr: &str| {
            masked.mods.iter().find_map(|m| match m {
                ModOp::Replace { attr: a, values } if a == attr => Some(values.clone()),
                _ => None,
            })
        };
        assert_eq!(
            val("sambaNTPassword"),
            Some(vec!["********".to_string()]),
            "secret attr masked even with empty mask_attrs"
        );
        assert_eq!(
            val("cn"),
            Some(vec!["James".to_string()]),
            "non-secret attr untouched"
        );
    }

    #[test]
    fn compose_renamed_dn_replaces_rdn() {
        assert_eq!(
            compose_renamed_dn("cn=Alice,ou=people,dc=org", "cn=Bob"),
            "cn=Bob,ou=people,dc=org"
        );
        assert_eq!(compose_renamed_dn("dc=org", "dc=net"), "dc=net");
    }

    #[test]
    fn fanout_adds_and_removes_per_group() {
        let out = membership_fanout(
            "uid=ann,ou=people",
            &["cn=g1,ou=groups".to_string(), "cn=g2,ou=groups".to_string()], // baseline groups
            &["cn=g2,ou=groups".to_string(), "cn=g3,ou=groups".to_string()], // new selection
            "member",
        );
        // g3 gains ann; g1 loses ann; g2 unchanged.
        assert_eq!(
            out,
            vec![
                (
                    "cn=g3,ou=groups".to_string(),
                    ModOp::Add {
                        attr: "member".into(),
                        values: vec!["uid=ann,ou=people".into()]
                    }
                ),
                (
                    "cn=g1,ou=groups".to_string(),
                    ModOp::Delete {
                        attr: "member".into(),
                        values: vec!["uid=ann,ou=people".into()]
                    }
                ),
            ]
        );
    }

    #[test]
    fn fanout_is_case_insensitive_on_dns() {
        let out = membership_fanout(
            "uid=ann,ou=people",
            &["CN=G1,OU=GROUPS".into()],
            &["cn=g1,ou=groups".into()],
            "member",
        );
        assert!(
            out.is_empty(),
            "same DN in different case must not produce add/delete"
        );
    }

    #[test]
    fn would_empty_only_when_sole_member() {
        assert!(would_empty(
            &["uid=ann,ou=people".to_string()],
            "uid=ann,ou=people"
        ));
        assert!(!would_empty(
            &[
                "uid=ann,ou=people".to_string(),
                "uid=bob,ou=people".to_string()
            ],
            "uid=ann,ou=people"
        ));
        // Already empty: not our removal's fault.
        assert!(!would_empty(&[], "uid=ann,ou=people"));
    }

    #[test]
    fn prepare_save_folds_password_mods_and_masks_preview() {
        // No attribute diff (original == edited): a password-only edit is still a
        // change. The real plan carries the cleartext + hash; the preview masks them.
        let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        attrs.insert("uid".into(), vec!["alice".into()]);
        let entry = EditEntry {
            dn: "uid=alice,ou=people,dc=example,dc=org".into(),
            attrs,
        };
        let pw_mods = vec![
            ModOp::Replace {
                attr: "userPassword".into(),
                values: vec!["hunter2".into()],
            },
            ModOp::Replace {
                attr: "sambaNTPassword".into(),
                values: vec!["DEADBEEF".into()],
            },
        ];
        let mask = vec!["userPassword".to_string(), "sambaNTPassword".to_string()];
        match prepare_save(
            &user_schema(),
            &entry,
            &entry,
            &["testUser".to_string()],
            &pw_mods,
            &mask,
            &[],
            &[],
            &Default::default(),
        ) {
            PrepareSave::Ready { plan, ldif, .. } => {
                // Preview masks both secrets, never the cleartext or hash.
                assert!(ldif.contains("********"), "preview must mask secrets");
                assert!(!ldif.contains("hunter2"), "cleartext must not appear");
                assert!(!ldif.contains("DEADBEEF"), "NT hash must not appear");
                // The real plan carries the unmasked values.
                match plan {
                    SavePlan::Modify(mods) => {
                        assert!(mods.contains(&ModOp::Replace {
                            attr: "userPassword".into(),
                            values: vec!["hunter2".into()],
                        }));
                        assert!(mods.contains(&ModOp::Replace {
                            attr: "sambaNTPassword".into(),
                            values: vec!["DEADBEEF".into()],
                        }));
                    }
                    _ => panic!("expected Modify"),
                }
            }
            _ => panic!("expected Ready"),
        }
    }

    #[test]
    fn prepare_save_no_password_no_diff_is_no_changes() {
        let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        attrs.insert("uid".into(), vec!["alice".into()]);
        let entry = EditEntry {
            dn: "uid=alice,ou=people,dc=example,dc=org".into(),
            attrs,
        };
        assert!(matches!(
            prepare_save(
                &user_schema(),
                &entry,
                &entry,
                &["testUser".to_string()],
                &[],
                &[],
                &[],
                &[],
                &Default::default(),
            ),
            PrepareSave::NoChanges
        ));
    }

    #[test]
    fn stage_pending_password_derives_and_strips() {
        let mut orig = BTreeMap::from([
            ("userPassword".to_string(), vec!["{SSHA}old".to_string()]),
            ("sambaNTPassword".to_string(), vec!["OLD".to_string()]),
            ("cn".to_string(), vec!["A".to_string()]),
        ]);
        let mut edited = orig.clone();
        let derived = vec!["sambaNTPassword".to_string(), "sambaPwdLastSet".to_string()];
        let (mods, mask) = stage_pending_password(
            Some("hunter2"),
            "userPassword",
            &derived,
            true,
            1_700_000_000,
            &mut orig,
            &mut edited,
        );
        assert!(mods
            .iter()
            .any(|m| matches!(m, ModOp::Replace { attr, .. } if attr == "userPassword")));
        assert!(mods
            .iter()
            .any(|m| matches!(m, ModOp::Replace { attr, .. } if attr == "sambaNTPassword")));
        assert!(!orig.contains_key("userPassword") && !edited.contains_key("sambaNTPassword"));
        assert!(orig.contains_key("cn"));
        assert!(mask.contains(&"userPassword".to_string()));
    }

    #[test]
    fn stage_pending_password_none_only_strips() {
        let mut orig = BTreeMap::from([("userPassword".to_string(), vec!["x".to_string()])]);
        let mut edited = orig.clone();
        let (mods, mask) =
            stage_pending_password(None, "userPassword", &[], false, 0, &mut orig, &mut edited);
        assert!(mods.is_empty() && mask.is_empty());
        assert!(!orig.contains_key("userPassword"));
    }

    #[test]
    fn allocation_refuses_on_truncation() {
        assert!(decide_allocation(&[10000], true, 10000, 60000).is_err());
        assert_eq!(
            decide_allocation(&[10000], false, 10000, 60000).unwrap(),
            10001
        );
        assert_eq!(decide_allocation(&[], false, 10000, 60000).unwrap(), 10000);
    }
}
