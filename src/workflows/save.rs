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
pub fn mask_changeset_secrets(cs: &ChangeSet, mask_attrs: &[String]) -> ChangeSet {
    let is_masked = |attr: &str| mask_attrs.iter().any(|a| a.eq_ignore_ascii_case(attr));
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
pub fn prepare_save(
    schema: &SchemaModel,
    original: &EditEntry,
    edited: &EditEntry,
    object_classes: &[String],
    password_mods: &[ModOp],
    mask_attrs: &[String],
) -> PrepareSave {
    let oc_refs: Vec<&str> = object_classes.iter().map(|s| s.as_str()).collect();
    let errors = validate(edited, schema, &oc_refs);
    if !errors.is_empty() {
        return PrepareSave::Invalid(errors);
    }
    let mut cs = match diff(original, edited) {
        Ok(cs) => cs,
        Err(e) => return PrepareSave::DiffError(e.to_string()),
    };
    cs.mods.extend(password_mods.iter().cloned());
    if cs.is_empty() {
        return PrepareSave::NoChanges;
    }
    let ldif = render_changeset(&mask_changeset_secrets(&cs, mask_attrs));
    PrepareSave::Ready {
        plan: plan_save(cs),
        dn: original.dn.clone(),
        ldif,
    }
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
                &[]
            ),
            PrepareSave::NoChanges
        ));
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
