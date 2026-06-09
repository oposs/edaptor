//! Client-side validation of an edited entry before sending (spec §8: MUST /
//! single-value / syntax checks before the write), plus the pure "what request
//! does this changeset become" decision ([`plan_save`]). Pure, unit-tested; no
//! terminal, no network.

use crate::form::changeset::{ChangeSet, ModOp, ModRdn};
use crate::schema::{FieldKind, SchemaModel};

/// A single validation failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    /// A required (MUST) attribute is missing or has no non-empty value.
    MissingMust(String),
    /// A single-valued attribute was given more than one value.
    MultiValueOnSingle(String),
    /// A value does not match the attribute's syntax.
    SyntaxInvalid {
        /// Attribute name.
        attr: String,
        /// Why it was rejected.
        reason: String,
    },
}

/// The pure edit type validated here (re-exported from `changeset` for callers).
pub use crate::form::changeset::EditEntry;

/// Validate `edited` against the schema for `object_classes`.
///
/// Checks performed:
/// * every MUST attribute (from `schema.effective_attributes`) is present with at
///   least one non-empty value — else [`ValidationError::MissingMust`];
/// * single-valued attributes (`schema.is_single_value`) carry at most one value
///   — else [`ValidationError::MultiValueOnSingle`];
/// * each value matches its [`FieldKind`] syntax (Int must parse as an integer,
///   Dn must look like an RDN sequence, Time must be a generalized-time prefix);
///   Text/Boolean/Binary are always accepted — else
///   [`ValidationError::SyntaxInvalid`].
pub fn validate(
    edited: &EditEntry,
    schema: &SchemaModel,
    object_classes: &[&str],
    orphaned_attrs: &[&str],
) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    let resolved = schema.effective_attributes(object_classes);

    // MUST checks: each required attr must have a non-empty value.
    // Orphaned attrs are being deleted — skip the MUST check for them.
    for must in &resolved.must {
        if orphaned_attrs.iter().any(|a| a.eq_ignore_ascii_case(must)) {
            continue; // attribute is being deleted — not required to be filled
        }
        let has_value = edited
            .attrs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(must))
            .map(|(_, vs)| vs.iter().any(|v| !v.trim().is_empty()))
            .unwrap_or(false);
        if !has_value {
            errors.push(ValidationError::MissingMust(must.clone()));
        }
    }

    // Per-attribute single-value and syntax checks.
    for (attr, values) in &edited.attrs {
        let non_empty: Vec<&String> = values.iter().filter(|v| !v.trim().is_empty()).collect();

        if non_empty.len() > 1 && schema.is_single_value(attr) {
            errors.push(ValidationError::MultiValueOnSingle(attr.clone()));
        }

        let kind = schema.field_kind(attr);
        for v in &non_empty {
            if let Some(reason) = syntax_error(kind, v) {
                errors.push(ValidationError::SyntaxInvalid {
                    attr: attr.clone(),
                    reason,
                });
            }
        }
    }

    errors
}

/// Return a human reason if `value` does not match `kind`, else `None`. Only the
/// kinds M2 classifies are checked; Text/Boolean/Binary are always valid.
fn syntax_error(kind: FieldKind, value: &str) -> Option<String> {
    match kind {
        FieldKind::Integer => {
            if value.trim().parse::<i64>().is_ok() {
                None
            } else {
                Some(format!("'{value}' is not an integer"))
            }
        }
        FieldKind::DistinguishedName => {
            // A minimal DN check: at least one `attr=value` component.
            if value.split(',').all(|c| c.contains('=')) && value.contains('=') {
                None
            } else {
                Some(format!("'{value}' is not a valid DN"))
            }
        }
        FieldKind::GeneralizedTime => {
            // Generalized time begins with at least YYYYMMDDHH (10 digits).
            let digits = value.chars().take_while(|c| c.is_ascii_digit()).count();
            if digits >= 10 {
                None
            } else {
                Some(format!("'{value}' is not a generalized time"))
            }
        }
        FieldKind::Text | FieldKind::Boolean | FieldKind::Binary => None,
    }
}

/// The concrete worker request(s) a [`ChangeSet`] should produce (Decision D3:
/// when both apply, MODRDN runs first, then MODIFY the rest).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SavePlan {
    /// Nothing to send.
    Nothing,
    /// Only attribute modifications.
    Modify(Vec<ModOp>),
    /// A rename, then modifications.
    Rename {
        /// The rename to apply first.
        modrdn: ModRdn,
        /// The mods to apply after the rename.
        then_mods: Vec<ModOp>,
    },
    /// Only a rename.
    RenameOnly(ModRdn),
}

/// Decide what to send for a [`ChangeSet`].
pub fn plan_save(cs: ChangeSet) -> SavePlan {
    match (cs.modrdn, cs.mods.is_empty()) {
        (None, true) => SavePlan::Nothing,
        (None, false) => SavePlan::Modify(cs.mods),
        (Some(modrdn), true) => SavePlan::RenameOnly(modrdn),
        (Some(modrdn), false) => SavePlan::Rename {
            modrdn,
            then_mods: cs.mods,
        },
    }
}

/// Format a list of [`ValidationError`]s as one multi-line message.
pub fn format_validation_errors(errors: &[ValidationError]) -> String {
    let mut out = String::from("Cannot save — please fix:");
    for e in errors {
        let line = match e {
            ValidationError::MissingMust(a) => format!("missing required attribute: {a}"),
            ValidationError::MultiValueOnSingle(a) => format!("attribute is single-valued: {a}"),
            ValidationError::SyntaxInvalid { attr, reason } => format!("{attr}: {reason}"),
        };
        out.push_str("\n- ");
        out.push_str(&line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldap::worker::RawSubschema;
    use std::collections::BTreeMap;

    fn schema() -> SchemaModel {
        let raw = RawSubschema {
            object_classes: vec![
                "( 2.5.6.0 NAME 'top' ABSTRACT MUST objectClass )".to_string(),
                "( 2.5.6.6 NAME 'person' SUP top STRUCTURAL MUST ( sn $ cn ) \
                  MAY ( description $ seeAlso ) )"
                    .to_string(),
                "( 1.2.3 NAME 'demoPerson' SUP person STRUCTURAL \
                  MAY ( employeeNumber $ manager $ uidFlag ) )"
                    .to_string(),
            ],
            attribute_types: vec![
                "( 2.5.4.3 NAME 'cn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".to_string(),
                "( 2.5.4.4 NAME 'sn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".to_string(),
                "( 2.5.4.13 NAME 'description' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".to_string(),
                // integer, single-valued
                "( 1.1.1 NAME 'employeeNumber' SYNTAX 1.3.6.1.4.1.1466.115.121.1.27 \
                  SINGLE-VALUE )"
                    .to_string(),
                // DN-valued
                "( 1.1.2 NAME 'manager' SYNTAX 1.3.6.1.4.1.1466.115.121.1.12 )".to_string(),
            ],
            ldap_syntaxes: vec![],
        };
        SchemaModel::from_raw(&raw)
    }

    fn entry(dn: &str, attrs: &[(&str, &[&str])]) -> EditEntry {
        let mut map = BTreeMap::new();
        for (k, vs) in attrs {
            map.insert(
                k.to_string(),
                vs.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            );
        }
        EditEntry {
            dn: dn.to_string(),
            attrs: map,
        }
    }

    #[test]
    fn missing_must_attr_flagged() {
        // sn is MUST but absent.
        let e = entry("cn=A,dc=x", &[("cn", &["A"]), ("objectClass", &["person"])]);
        let errs = validate(&e, &schema(), &["person"], &[]);
        assert!(errs.contains(&ValidationError::MissingMust("sn".to_string())));
    }

    #[test]
    fn empty_must_attr_flagged() {
        // sn present but empty.
        let e = entry(
            "cn=A,dc=x",
            &[("cn", &["A"]), ("sn", &[""]), ("objectClass", &["person"])],
        );
        let errs = validate(&e, &schema(), &["person"], &[]);
        assert!(errs.contains(&ValidationError::MissingMust("sn".to_string())));
    }

    #[test]
    fn second_value_on_single_valued_flagged() {
        let e = entry(
            "cn=A,dc=x",
            &[
                ("cn", &["A"]),
                ("sn", &["Adams"]),
                ("employeeNumber", &["1", "2"]),
                ("objectClass", &["demoPerson"]),
            ],
        );
        let errs = validate(&e, &schema(), &["demoPerson"], &[]);
        assert!(errs.contains(&ValidationError::MultiValueOnSingle(
            "employeeNumber".to_string()
        )));
    }

    #[test]
    fn integer_syntax_rejects_non_numeric() {
        let e = entry(
            "cn=A,dc=x",
            &[
                ("cn", &["A"]),
                ("sn", &["Adams"]),
                ("employeeNumber", &["not-a-number"]),
                ("objectClass", &["demoPerson"]),
            ],
        );
        let errs = validate(&e, &schema(), &["demoPerson"], &[]);
        assert!(errs.iter().any(|err| matches!(
            err,
            ValidationError::SyntaxInvalid { attr, .. } if attr == "employeeNumber"
        )));
    }

    #[test]
    fn dn_syntax_rejects_garbage() {
        let e = entry(
            "cn=A,dc=x",
            &[
                ("cn", &["A"]),
                ("sn", &["Adams"]),
                ("manager", &["not a dn"]),
                ("objectClass", &["demoPerson"]),
            ],
        );
        let errs = validate(&e, &schema(), &["demoPerson"], &[]);
        assert!(errs.iter().any(|err| matches!(
            err,
            ValidationError::SyntaxInvalid { attr, .. } if attr == "manager"
        )));
    }

    #[test]
    fn valid_entry_has_no_errors() {
        let e = entry(
            "cn=A,dc=x",
            &[
                ("cn", &["A"]),
                ("sn", &["Adams"]),
                ("employeeNumber", &["42"]),
                ("manager", &["cn=boss,dc=x"]),
                ("objectClass", &["demoPerson"]),
            ],
        );
        let errs = validate(&e, &schema(), &["demoPerson"], &[]);
        assert!(errs.is_empty(), "errs={errs:?}");
    }

    // --- plan_save ---

    fn modrdn() -> ModRdn {
        ModRdn {
            new_rdn: "cn=Bob".to_string(),
            delete_old: true,
            new_superior: None,
        }
    }

    fn a_mod() -> ModOp {
        ModOp::Replace {
            attr: "sn".to_string(),
            values: vec!["Brown".to_string()],
        }
    }

    #[test]
    fn empty_changeset_is_nothing() {
        let cs = ChangeSet {
            dn: "cn=A,dc=x".to_string(),
            modrdn: None,
            mods: vec![],
        };
        assert_eq!(plan_save(cs), SavePlan::Nothing);
    }

    #[test]
    fn mods_only_is_modify() {
        let cs = ChangeSet {
            dn: "cn=A,dc=x".to_string(),
            modrdn: None,
            mods: vec![a_mod()],
        };
        assert_eq!(plan_save(cs), SavePlan::Modify(vec![a_mod()]));
    }

    #[test]
    fn rdn_only_is_rename_only() {
        let cs = ChangeSet {
            dn: "cn=A,dc=x".to_string(),
            modrdn: Some(modrdn()),
            mods: vec![],
        };
        assert_eq!(plan_save(cs), SavePlan::RenameOnly(modrdn()));
    }

    #[test]
    fn rdn_plus_mods_is_rename_then_modify() {
        let cs = ChangeSet {
            dn: "cn=A,dc=x".to_string(),
            modrdn: Some(modrdn()),
            mods: vec![a_mod()],
        };
        assert_eq!(
            plan_save(cs),
            SavePlan::Rename {
                modrdn: modrdn(),
                then_mods: vec![a_mod()],
            }
        );
    }

    #[test]
    fn validation_errors_format_as_bullets() {
        let errs = vec![
            ValidationError::MissingMust("sn".into()),
            ValidationError::MultiValueOnSingle("cn".into()),
        ];
        let out = format_validation_errors(&errs);
        assert!(out.contains("missing required attribute: sn"));
        assert!(out.contains("attribute is single-valued: cn"));
    }

    #[test]
    fn orphaned_must_attr_is_not_flagged() {
        // sn is MUST for person, but it is orphaned (will be deleted).
        // validate() must skip the MUST check for orphaned attrs.
        let e = entry(
            "cn=A,dc=x",
            &[("cn", &["A"]), ("objectClass", &["person"])],
            // sn is absent — but it is in orphaned_attrs
        );
        let errs = validate(&e, &schema(), &["person"], &["sn"]);
        assert!(
            !errs
                .iter()
                .any(|err| matches!(err, ValidationError::MissingMust(a) if a == "sn")),
            "orphaned MUST attr must not be flagged as missing"
        );
    }

    #[test]
    fn non_orphaned_must_attr_still_flagged() {
        let e = entry("cn=A,dc=x", &[("objectClass", &["person"])]);
        // cn and sn are both MUST, neither is orphaned
        let errs = validate(&e, &schema(), &["person"], &[]);
        assert!(errs
            .iter()
            .any(|err| matches!(err, ValidationError::MissingMust(a) if a == "sn")));
    }
}
