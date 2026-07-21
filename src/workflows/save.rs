//! Pure save-flow domain logic: validate + diff a single-entry save, mask
//! preview secrets, compute membership fan-out, decide number allocation, and
//! compose renamed DNs. No terminal, no network, no UI types.

use crate::form::changeset::{diff, ChangeSet, EditEntry, ModOp};
use crate::form::validate::{plan_save, validate, SavePlan, ValidationError};
use crate::ldap::ldif::{render_changeset, render_changesets};
use crate::schema::SchemaModel;

/// The outcome of preparing a form save.
#[derive(Debug)]
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

/// A planned combined membership save: the user entry's own changes plus the
/// per-holder fan-out writes that maintain group membership.
///
/// Membership is stored on each GROUP's holder attribute (e.g. `member`), never
/// on the user's own `memberOf` (that back-ref is overlay-maintained and stripped
/// from the own-entry diff). One combined save therefore touches several entries:
/// the user (`own_mods`) and every group whose membership changed (`fanout`).
#[derive(Debug)]
pub struct CombinedSave {
    /// The user (own) entry's DN.
    pub own_dn: String,
    /// The user entry's own attribute changes (back-ref attrs stripped from BOTH
    /// the baseline and the edited side, so a `memberOf` change never lands here).
    pub own_mods: Vec<ModOp>,
    /// One `(group_dn, Add/Delete member=user_dn)` per touched group.
    pub fanout: Vec<(String, ModOp)>,
    /// Combined preview: the own changeset (secrets masked) followed by one stanza
    /// per touched group.
    pub ldif: String,
}

/// Outcome of [`plan_combined_save`].
#[derive(Debug)]
pub enum PlanCombined {
    /// Client-side validation of the own entry failed.
    Invalid(Vec<ValidationError>),
    /// The own-entry diff could not be computed (e.g. multi-valued RDN).
    DiffError(String),
    /// Neither the own entry nor any membership changed — nothing to do.
    NoChanges,
    /// The own entry is being renamed (DN/RDN change). A rename combined with a
    /// membership change is not supported in v1 — the caller must split them into
    /// separate saves. This planner is membership-scoped: a plain rename with no
    /// membership change should go through [`prepare_save`] instead.
    RenameWithMembershipUnsupported,
    /// A ready combined save.
    Ready(CombinedSave),
}

/// Plan a combined membership save: the own-entry diff (each fan-out / back-ref
/// label stripped from BOTH the baseline and the edited side) plus the per-holder
/// fan-out derived from each fan-out field's baseline→selection delta.
///
/// Neutral parity port of `ui::app::save::plan_combined_save` (the
/// `edit_form`/`write_flow` precedent — no `crate::ui`, no UI framework). The
/// own-entry leg mirrors [`prepare_save`]'s contract: the same `password_mods`,
/// `mask_attrs`, `secret_attrs`, `orphaned_attrs` and `x_ordered_attrs` apply. The
/// caller stages any password change (producing `password_mods` + `mask_attrs`);
/// `mask_attrs` are also stripped from BOTH sides here so a stored hash on the
/// baseline can never diff to a spurious Delete — exactly as the single-entry path
/// does via [`stage_pending_password`].
///
/// # Caller contract — password attrs MUST always be in `mask_attrs`
///
/// [`stage_pending_password`] **always** strips the password primary and derived
/// attributes from the entry maps it receives, regardless of whether a password is
/// staged. This function mirrors that guarantee by stripping every attr in
/// `mask_attrs` from BOTH the baseline and the edited side before diffing. For the
/// guarantee to hold, callers MUST include the password primary and all derived
/// attributes in `mask_attrs` even when no password change is staged (i.e. even
/// when `password_mods` is empty). Passing only the result of
/// `stage_pending_password` as `mask_attrs` is **not sufficient** when
/// `pending_password` is `None`, because that returns an empty mask — leaving a
/// stored baseline hash visible to the diff and causing a spurious
/// `Delete userPassword`.
///
/// When no fan-out field changed the result still holds (empty `fanout`); callers
/// that know there is no membership change may use [`prepare_save`] directly.
/// A rename combined with a membership change yields
/// [`PlanCombined::RenameWithMembershipUnsupported`] (v1 simplification).
#[allow(clippy::too_many_arguments)]
pub fn plan_combined_save(
    schema: &SchemaModel,
    form: &crate::workflows::edit_form::EditForm,
    password_mods: &[ModOp],
    mask_attrs: &[String],
    secret_attrs: &[String],
    orphaned_attrs: &[&str],
    x_ordered_attrs: &std::collections::HashSet<String>,
) -> PlanCombined {
    let fanout_lbls = form.fanout_labels();

    // Own entry: baseline from each field's load-time values, edited from the
    // current values. Strip the fan-out (back-ref) labels from BOTH sides so the
    // overlay-maintained `memberOf` never diffs into the own-entry MODIFY.
    let mut original = EditEntry {
        dn: form.dn.clone(),
        attrs: form
            .fields
            .iter()
            .map(|f| (f.label.clone(), f.baseline.clone()))
            .collect(),
    };
    let mut edited = form.to_edit_entry();
    for l in &fanout_lbls {
        original.attrs.remove(l);
        edited.attrs.remove(l);
    }
    // Mirror the single-entry password strip: drop the masked (primary + derived)
    // password attrs from BOTH sides so a baseline hash never diffs to a Delete.
    for a in mask_attrs {
        original.attrs.retain(|k, _| !k.eq_ignore_ascii_case(a));
        edited.attrs.retain(|k, _| !k.eq_ignore_ascii_case(a));
    }

    // Validate the edited own entry against its objectClasses.
    let oc_refs: Vec<&str> = form.object_classes.iter().map(|s| s.as_str()).collect();
    let errors = validate(&edited, schema, &oc_refs, orphaned_attrs);
    if !errors.is_empty() {
        return PlanCombined::Invalid(errors);
    }

    // Own-entry diff.
    let mut own_cs = match diff(&original, &edited, x_ordered_attrs) {
        Ok(c) => c,
        Err(e) => return PlanCombined::DiffError(e.to_string()),
    };

    // Fan-out: one Add/Delete per touched holder, per fan-out field.
    let mut fanout_ops: Vec<(String, ModOp)> = Vec::new();
    let mut fanout_sets: Vec<ChangeSet> = Vec::new();
    for f in form
        .fields
        .iter()
        .filter(|f| fanout_lbls.contains(&f.label))
    {
        let Some(attr) = crate::workflows::edit_form::fanout_attr_of(f).map(|s| s.to_string())
        else {
            continue;
        };
        for (gdn, op) in membership_fanout(&form.dn, &f.baseline, &f.current_values(), &attr) {
            fanout_sets.push(ChangeSet {
                dn: gdn.clone(),
                modrdn: None,
                mods: vec![op.clone()],
            });
            fanout_ops.push((gdn, op));
        }
    }

    // A rename reaching this membership-scoped planner is unsupported in v1: the
    // user must do the rename and the membership change as separate saves.
    if own_cs.modrdn.is_some() {
        return PlanCombined::RenameWithMembershipUnsupported;
    }

    // Fold any staged password REPLACEs into the own-entry MODIFY (one source of
    // truth for both the apply and the preview).
    own_cs.mods.extend(password_mods.iter().cloned());

    if own_cs.mods.is_empty() && fanout_ops.is_empty() {
        return PlanCombined::NoChanges;
    }

    // Combined preview: own changeset (secrets masked) then one stanza per holder.
    let mut preview_sets: Vec<ChangeSet> = Vec::new();
    if !own_cs.mods.is_empty() {
        preview_sets.push(mask_changeset_secrets(&own_cs, mask_attrs, secret_attrs));
    }
    preview_sets.extend(fanout_sets);

    PlanCombined::Ready(CombinedSave {
        own_dn: form.dn.clone(),
        own_mods: own_cs.mods,
        fanout: fanout_ops,
        ldif: render_changesets(&preview_sets),
    })
}

/// True when removing `member` would leave the group with no members (groupOfNames
/// requires ≥1). Only fires when `member` is the SOLE current member. False for
/// empty input (the group is already empty — not our removal's fault).
pub fn would_empty(current_members: &[String], member: &str) -> bool {
    current_members.len() == 1 && current_members[0].eq_ignore_ascii_case(member)
}

/// Pre-validation for a combined membership save: scan the fan-out `Delete`
/// ops and, for any group present in `group_members` whose sole member is
/// `own_dn`, return a refusal message. Groups absent from `group_members` are
/// treated as having no known members (`would_empty` returns false) — this is
/// how MAY-membership groups are exempted: the caller only populates the map
/// for groups whose membership attribute is MUST (see
/// [`membership_attr_is_must`]). Returns `None` when nothing would be emptied.
pub fn last_member_block(
    fanout: &[(String, ModOp)],
    group_members: &std::collections::HashMap<String, Vec<String>>,
    own_dn: &str,
) -> Option<String> {
    for (group_dn, op) in fanout {
        if let ModOp::Delete { .. } = op {
            let current = group_members
                .get(group_dn)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            if would_empty(current, own_dn) {
                return Some(format!(
                    "Refusing to save: removing {own_dn} from {group_dn} would leave \
                     the group with no members (a required attribute)."
                ));
            }
        }
    }
    None
}

/// True when `attr` is a MUST (required) attribute for any of `object_classes`
/// per `schema` (case-insensitive). Gates last-member pre-validation: only block
/// removing a group's final member when its membership attribute is MUST (e.g.
/// `member` in `groupOfNames`), never for MAY (`memberUid` in `posixGroup`,
/// where an empty group is legal).
pub fn membership_attr_is_must(schema: &SchemaModel, object_classes: &[&str], attr: &str) -> bool {
    schema
        .effective_attributes(object_classes)
        .must
        .iter()
        .any(|m| m.eq_ignore_ascii_case(attr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflows::test_fixtures::user_schema;
    use std::collections::BTreeMap;

    fn group_schema() -> SchemaModel {
        use crate::ldap::worker::RawSubschema;
        SchemaModel::from_raw(&RawSubschema {
            object_classes: vec![
                "( 2.5.6.0 NAME 'top' ABSTRACT MUST objectClass )".to_string(),
                "( 2.5.6.9 NAME 'groupOfNames' SUP top STRUCTURAL MUST ( member $ cn ) \
                  MAY ( description $ owner ) )"
                    .to_string(),
                "( 2.5.6.17 NAME 'groupOfUniqueNames' SUP top STRUCTURAL \
                  MUST ( uniqueMember $ cn ) MAY ( description ) )"
                    .to_string(),
                "( 1.3.6.1.1.1.2.2 NAME 'posixGroup' SUP top STRUCTURAL \
                  MUST ( cn $ gidNumber ) MAY ( userPassword $ memberUid $ description ) )"
                    .to_string(),
            ],
            attribute_types: vec![],
            ldap_syntaxes: vec![],
        })
    }

    #[test]
    fn member_is_must_for_group_of_names() {
        let s = group_schema();
        assert!(membership_attr_is_must(&s, &["groupOfNames"], "member"));
        assert!(membership_attr_is_must(&s, &["groupOfNames"], "MEMBER")); // case-insensitive
    }

    #[test]
    fn unique_member_is_must_for_group_of_unique_names() {
        let s = group_schema();
        assert!(membership_attr_is_must(
            &s,
            &["groupOfUniqueNames"],
            "uniqueMember"
        ));
    }

    #[test]
    fn member_uid_is_may_for_posix_group() {
        let s = group_schema();
        assert!(!membership_attr_is_must(&s, &["posixGroup"], "memberUid"));
    }

    #[test]
    fn unknown_class_or_attr_is_not_must() {
        let s = group_schema();
        assert!(!membership_attr_is_must(&s, &["doesNotExist"], "member"));
        assert!(!membership_attr_is_must(
            &s,
            &["groupOfNames"],
            "noSuchAttr"
        ));
    }

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

    // --- plan_combined_save (membership fan-out) ---------------------------

    use crate::config::relation::{CandidateScope, PickerBinding, StoreKey};
    use crate::config::widget::WidgetKind;
    use crate::schema::FieldKind;
    use crate::workflows::edit_form::{EditField, EditForm, FormMode};
    use crate::workflows::form_model::WidgetSpec;

    fn memberof_field(values: Vec<&str>, baseline: Vec<&str>) -> EditField {
        let scope = CandidateScope {
            base: "ou=groups,dc=x".into(),
            object_classes: vec!["groupOfNames".into()],
            search_attrs: vec!["cn".into()],
            label_template: None,
        };
        EditField {
            label: "memberOf".into(),
            must: false,
            editable: true,
            multi: true,
            secret: false,
            ordered: false,
            orphaned: false,
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: Some(WidgetKind::Picker(PickerBinding {
                attr: "memberOf".into(),
                scope,
                store: StoreKey::Dn,
                select: None,
                fanout_attr: Some("member".into()),
            })),
            values: values.into_iter().map(|s| s.to_string()).collect(),
            baseline: baseline.into_iter().map(|s| s.to_string()).collect(),
        }
    }

    fn plain_field(label: &str, values: Vec<&str>, baseline: Vec<&str>) -> EditField {
        EditField {
            label: label.into(),
            must: label == "uid",
            editable: true,
            multi: label != "uid",
            secret: false,
            ordered: false,
            orphaned: false,
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: None,
            values: values.into_iter().map(|s| s.to_string()).collect(),
            baseline: baseline.into_iter().map(|s| s.to_string()).collect(),
        }
    }

    /// User form: description changes old→new, memberOf changes g1→g2.
    fn user_form_own_and_memberof() -> EditForm {
        EditForm {
            dn: "uid=ann,ou=people,dc=x".into(),
            mode: FormMode::Edit,
            object_classes: vec!["testUser".into()],
            fields: vec![
                plain_field("uid", vec!["ann"], vec!["ann"]),
                plain_field("description", vec!["new desc"], vec!["old desc"]),
                memberof_field(vec!["cn=g2,ou=groups,dc=x"], vec!["cn=g1,ou=groups,dc=x"]),
            ],
            baseline_csn: None,
        }
    }

    /// User form: uid (RDN attr) changes ann→bob AND memberOf changes g1→g2.
    fn user_form_rename_and_memberof() -> EditForm {
        EditForm {
            dn: "uid=ann,ou=people,dc=x".into(),
            mode: FormMode::Edit,
            object_classes: vec!["testUser".into()],
            fields: vec![
                plain_field("uid", vec!["bob"], vec!["ann"]),
                memberof_field(vec!["cn=g2,ou=groups,dc=x"], vec!["cn=g1,ou=groups,dc=x"]),
            ],
            baseline_csn: None,
        }
    }

    #[test]
    fn combined_splits_own_diff_and_membership_fanout() {
        let form = user_form_own_and_memberof();
        let plan = plan_combined_save(
            &user_schema(),
            &form,
            &[],
            &[],
            &[],
            &[],
            &Default::default(),
        );
        let cs = match plan {
            PlanCombined::Ready(cs) => cs,
            other => panic!("expected Ready, got {other:?}"),
        };
        // own_mods carry the description change and NEVER the back-ref memberOf.
        assert!(
            cs.own_mods.iter().all(|m| {
                let attr = match m {
                    ModOp::Add { attr, .. }
                    | ModOp::Delete { attr, .. }
                    | ModOp::Replace { attr, .. } => attr,
                };
                !attr.eq_ignore_ascii_case("memberOf")
            }),
            "own_mods must not contain memberOf (back-ref stripped both sides); got {:?}",
            cs.own_mods
        );
        assert!(
            cs.own_mods.iter().any(|m| matches!(
                m,
                ModOp::Replace { attr, .. } if attr.eq_ignore_ascii_case("description")
            )),
            "own_mods must carry the description change; got {:?}",
            cs.own_mods
        );
        // fanout: g2 gains the user (Add), g1 loses the user (Delete).
        assert_eq!(
            cs.fanout.len(),
            2,
            "expected add g2 + delete g1; got {:?}",
            cs.fanout
        );
        assert!(cs.fanout.iter().any(|(dn, op)| dn == "cn=g2,ou=groups,dc=x"
            && matches!(op, ModOp::Add { attr, values }
                if attr == "member" && values == &["uid=ann,ou=people,dc=x".to_string()])));
        assert!(cs.fanout.iter().any(|(dn, op)| dn == "cn=g1,ou=groups,dc=x"
            && matches!(op, ModOp::Delete { attr, values }
                if attr == "member" && values == &["uid=ann,ou=people,dc=x".to_string()])));
        // combined LDIF mentions the touched groups.
        assert!(cs.ldif.contains("cn=g2,ou=groups,dc=x"));
        assert!(cs.ldif.contains("cn=g1,ou=groups,dc=x"));
        assert_eq!(cs.own_dn, "uid=ann,ou=people,dc=x");
    }

    #[test]
    fn combined_rename_plus_membership_is_unsupported() {
        let form = user_form_rename_and_memberof();
        assert!(
            matches!(
                plan_combined_save(
                    &user_schema(),
                    &form,
                    &[],
                    &[],
                    &[],
                    &[],
                    &Default::default()
                ),
                PlanCombined::RenameWithMembershipUnsupported
            ),
            "rename combined with a membership change must be unsupported in v1"
        );
    }

    /// A user form whose memberOf field changes (g1→g2, fan-out) AND whose
    /// userPassword field carries the directory's stored hash on the baseline but
    /// no value on the edited side — the typical state when the password widget
    /// hides the hash from the form. Used to test that no-pending-password saves
    /// do not emit a spurious Delete for userPassword.
    fn user_form_with_password_baseline_and_memberof_change() -> EditForm {
        EditForm {
            dn: "uid=ann,ou=people,dc=x".into(),
            mode: FormMode::Edit,
            object_classes: vec!["testUser".into()],
            fields: vec![
                plain_field("uid", vec!["ann"], vec!["ann"]),
                // Password field: the directory stores a hash on the baseline but the
                // password widget does not surface the hash value to the form editor.
                // values = [] simulates the hidden-hash state; baseline = ["{SSHA}…"]
                // is what was loaded from the directory. Without proper stripping, the
                // diff would produce a spurious Delete for userPassword.
                plain_field("userPassword", vec![], vec!["{SSHA}oldhash"]),
                // Fan-out: g1 → g2.
                memberof_field(vec!["cn=g2,ou=groups,dc=x"], vec!["cn=g1,ou=groups,dc=x"]),
            ],
            baseline_csn: None,
        }
    }

    /// Regression: when a combined (membership fan-out) save happens and there is
    /// NO staged password change, a password attribute whose baseline holds the
    /// stored directory hash must NOT produce a spurious `Delete userPassword` in
    /// `own_mods`.
    ///
    /// The property is guaranteed by stripping `mask_attrs` from BOTH the baseline
    /// and the edited entry before diffing. Per the caller contract on
    /// [`plan_combined_save`], the caller MUST include the password primary (and
    /// any derived attrs) in `mask_attrs` unconditionally — not only when a
    /// password change is staged.
    #[test]
    fn combined_no_pending_password_does_not_clobber_baseline_hash() {
        let form = user_form_with_password_baseline_and_memberof_change();
        // Correct caller behavior: always include the password primary in mask_attrs
        // even when no password is staged (password_mods is empty).
        let password_primary = "userPassword".to_string();
        match plan_combined_save(
            &user_schema(),
            &form,
            &[],                 // no staged password mods
            &[password_primary], // primary always in mask_attrs (caller contract)
            &[],
            &[],
            &Default::default(),
        ) {
            PlanCombined::Ready(cs) => {
                let touches_password = cs.own_mods.iter().any(|m| {
                    let attr = match m {
                        ModOp::Add { attr, .. }
                        | ModOp::Delete { attr, .. }
                        | ModOp::Replace { attr, .. } => attr,
                    };
                    attr.eq_ignore_ascii_case("userPassword")
                });
                assert!(
                    !touches_password,
                    "no staged password must not emit a userPassword mod (clobber!); \
                     got own_mods: {:?}",
                    cs.own_mods
                );
                // Sanity: the fan-out change IS captured.
                assert_eq!(
                    cs.fanout.len(),
                    2,
                    "expected g1 Delete + g2 Add in fanout; got {:?}",
                    cs.fanout
                );
            }
            other => panic!("expected PlanCombined::Ready, got {other:?}"),
        }
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

    #[test]
    fn last_member_block_fires_only_for_known_sole_member() {
        use std::collections::HashMap;
        let fanout = vec![(
            "cn=admins,ou=groups".to_string(),
            ModOp::Delete {
                attr: "member".into(),
                values: vec!["uid=ann,ou=people".into()],
            },
        )];
        // Group known with ann as the sole member → blocked.
        let mut gm: HashMap<String, Vec<String>> = HashMap::new();
        gm.insert(
            "cn=admins,ou=groups".into(),
            vec!["uid=ann,ou=people".into()],
        );
        assert!(last_member_block(&fanout, &gm, "uid=ann,ou=people").is_some());

        // Group not in the map (e.g. MAY membership, never fetched) → no block.
        let empty: HashMap<String, Vec<String>> = HashMap::new();
        assert!(last_member_block(&fanout, &empty, "uid=ann,ou=people").is_none());

        // Group with another member too → no block.
        let mut gm2: HashMap<String, Vec<String>> = HashMap::new();
        gm2.insert(
            "cn=admins,ou=groups".into(),
            vec!["uid=ann,ou=people".into(), "uid=bob,ou=people".into()],
        );
        assert!(last_member_block(&fanout, &gm2, "uid=ann,ou=people").is_none());
    }

    #[test]
    fn last_member_block_ignores_add_ops() {
        use std::collections::HashMap;
        let fanout = vec![(
            "cn=admins,ou=groups".to_string(),
            ModOp::Add {
                attr: "member".into(),
                values: vec!["uid=ann,ou=people".into()],
            },
        )];
        let mut gm: HashMap<String, Vec<String>> = HashMap::new();
        gm.insert(
            "cn=admins,ou=groups".into(),
            vec!["uid=ann,ou=people".into()],
        );
        assert!(last_member_block(&fanout, &gm, "uid=ann,ou=people").is_none());
    }
}
