//! Pure attribute diffing: turn an original vs. edited entry into a
//! [`ChangeSet`] of LDAP modify operations, detecting an RDN-attribute change as
//! a MODRDN (not a MODIFY) per spec §8 (rename = MODRDN).
//!
//! Self-contained types only: this module MUST NOT import `LdapEntry` from
//! `ldap::worker` (the worker imports `ModOp` from here; reusing `LdapEntry`
//! would close a module cycle). The read/save flow converts an `LdapEntry` into
//! an [`EditEntry`] at the boundary.

use std::collections::BTreeMap;

/// A pure, self-contained snapshot of an entry for diffing: its DN and its
/// string-valued attributes. Deliberately independent of `ldap::worker::LdapEntry`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EditEntry {
    /// The entry's distinguished name.
    pub dn: String,
    /// String attribute values, attribute name -> values (order-insensitive set
    /// semantics for the diff, but a `Vec` to preserve display order).
    pub attrs: BTreeMap<String, Vec<String>>,
}

/// A single LDAP MODIFY operation on one attribute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModOp {
    /// Add the given values to an attribute.
    Add {
        /// Attribute name.
        attr: String,
        /// Values to add.
        values: Vec<String>,
    },
    /// Delete the given values from an attribute; an empty `values` deletes the
    /// whole attribute.
    Delete {
        /// Attribute name.
        attr: String,
        /// Values to delete (empty = delete the entire attribute).
        values: Vec<String>,
    },
    /// Replace an attribute's values with the given set.
    Replace {
        /// Attribute name.
        attr: String,
        /// New values.
        values: Vec<String>,
    },
}

/// A MODRDN (rename) operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModRdn {
    /// The new RDN, e.g. `cn=Bob`.
    pub new_rdn: String,
    /// Whether to delete the old RDN attribute value (default `true`, spec §8).
    pub delete_old: bool,
    /// New superior (parent) DN. Always `None` in M4 (no subtree moves).
    pub new_superior: Option<String>,
}

/// The full set of changes to apply to one entry: an optional rename plus a list
/// of attribute modifications.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChangeSet {
    /// The entry's (original) DN the changes apply to.
    pub dn: String,
    /// An optional rename. When present it is applied before `mods`.
    pub modrdn: Option<ModRdn>,
    /// Per-attribute modifications.
    pub mods: Vec<ModOp>,
}

impl ChangeSet {
    /// True when there is nothing to send (no rename and no mods).
    pub fn is_empty(&self) -> bool {
        self.modrdn.is_none() && self.mods.is_empty()
    }
}

/// Error from [`diff`] when the change cannot be expressed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeSetError {
    /// The original DN has a multi-valued RDN (e.g. `cn=x+uid=y`); renaming such
    /// entries is out of scope for M4.
    MultiValuedRdnUnsupported,
}

impl std::fmt::Display for ChangeSetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChangeSetError::MultiValuedRdnUnsupported => {
                write!(f, "multi-valued RDNs are not supported")
            }
        }
    }
}

impl std::error::Error for ChangeSetError {}

/// Parse the leftmost RDN component of a DN into `(attr, value)`, e.g.
/// `cn=Alice,ou=people,dc=x` -> `("cn", "Alice")`. Returns `None` if there is no
/// `=` in the first component. Multi-valued RDNs (`cn=x+uid=y`) are detected by
/// the caller; this returns the first `attr=value` pair as-is.
pub fn rdn_component(dn: &str) -> Option<(String, String)> {
    let first = dn.split(',').next()?.trim();
    let (attr, value) = first.split_once('=')?;
    Some((attr.trim().to_string(), value.trim().to_string()))
}

/// Whether the first RDN component of `dn` is multi-valued (contains a `+`).
fn rdn_is_multivalued(dn: &str) -> bool {
    dn.split(',')
        .next()
        .map(|first| first.contains('+'))
        .unwrap_or(false)
}

/// Case-insensitive attribute lookup over an `EditEntry`'s attrs.
fn values_for<'a>(entry: &'a EditEntry, attr: &str) -> Option<&'a Vec<String>> {
    entry
        .attrs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(attr))
        .map(|(_, v)| v)
}

/// Diff `original` against `edited`, producing a [`ChangeSet`].
///
/// Per-attribute semantics (set-based, value order ignored):
/// * attribute present in both with the same value set -> no op;
/// * attribute gained values -> `Add` of the new values;
/// * attribute lost values -> `Delete` of the removed values;
/// * attribute present only in `edited` -> `Add` of all its values;
/// * attribute present only in `original` (or cleared to empty) -> `Delete` whole;
/// * a single-valued change (one value -> a different single value) -> `Replace`.
///
/// RDN handling (spec §8): if the edited value of the original RDN attribute
/// differs (case-sensitively, per LDAP value semantics) from the original RDN
/// value, a [`ModRdn`] is emitted with `new_rdn = "<attr>=<newvalue>"` and the
/// RDN attribute is excluded from `mods` (OpenLDAP updates it as part of MODRDN).
/// Multi-valued original RDNs are refused.
pub fn diff(original: &EditEntry, edited: &EditEntry) -> Result<ChangeSet, ChangeSetError> {
    if rdn_is_multivalued(&original.dn) {
        return Err(ChangeSetError::MultiValuedRdnUnsupported);
    }

    // Detect an RDN change. The RDN attribute is matched case-insensitively
    // (LDAP), but the value comparison is case-sensitive (value semantics).
    let rdn = rdn_component(&original.dn);
    let mut modrdn: Option<ModRdn> = None;
    let mut rdn_attr_excluded: Option<String> = None;

    if let Some((rdn_attr, rdn_value)) = &rdn {
        if let Some(new_values) = values_for(edited, rdn_attr) {
            // The new RDN value is the edited attribute's first value.
            if let Some(new_value) = new_values.first() {
                if new_value != rdn_value {
                    modrdn = Some(ModRdn {
                        new_rdn: format!("{rdn_attr}={new_value}"),
                        delete_old: true,
                        new_superior: None,
                    });
                    rdn_attr_excluded = Some(rdn_attr.clone());
                }
            }
        }
    }

    let mut mods: Vec<ModOp> = Vec::new();

    // Collect the union of attribute names (case-insensitive), preferring the
    // edited entry's display case where available.
    let mut seen: Vec<String> = Vec::new();
    let push_name = |name: &str, seen: &mut Vec<String>| {
        if !seen.iter().any(|n| n.eq_ignore_ascii_case(name)) {
            seen.push(name.to_string());
        }
    };
    for k in edited.attrs.keys() {
        push_name(k, &mut seen);
    }
    for k in original.attrs.keys() {
        push_name(k, &mut seen);
    }

    for attr in &seen {
        // Skip the RDN attribute when a MODRDN already covers it.
        if let Some(excluded) = &rdn_attr_excluded {
            if attr.eq_ignore_ascii_case(excluded) {
                continue;
            }
        }

        let orig = values_for(original, attr).cloned().unwrap_or_default();
        let new = values_for(edited, attr).cloned().unwrap_or_default();

        if value_set_eq(&orig, &new) {
            continue;
        }

        match (orig.is_empty(), new.is_empty()) {
            // Attribute removed entirely.
            (false, true) => mods.push(ModOp::Delete {
                attr: attr.clone(),
                values: Vec::new(),
            }),
            // Brand-new attribute.
            (true, false) => mods.push(ModOp::Add {
                attr: attr.clone(),
                values: new.clone(),
            }),
            // Both non-empty and different.
            (false, false) => {
                if orig.len() == 1 && new.len() == 1 {
                    // Single value changed -> Replace.
                    mods.push(ModOp::Replace {
                        attr: attr.clone(),
                        values: new.clone(),
                    });
                } else {
                    // Multi-valued: emit Add for gained, Delete for lost.
                    let added: Vec<String> = new
                        .iter()
                        .filter(|v| !orig.iter().any(|o| o == *v))
                        .cloned()
                        .collect();
                    let removed: Vec<String> = orig
                        .iter()
                        .filter(|v| !new.iter().any(|n| n == *v))
                        .cloned()
                        .collect();
                    if !removed.is_empty() {
                        mods.push(ModOp::Delete {
                            attr: attr.clone(),
                            values: removed,
                        });
                    }
                    if !added.is_empty() {
                        mods.push(ModOp::Add {
                            attr: attr.clone(),
                            values: added,
                        });
                    }
                }
            }
            // Both empty: handled by value_set_eq above.
            (true, true) => {}
        }
    }

    Ok(ChangeSet {
        dn: original.dn.clone(),
        modrdn,
        mods,
    })
}

/// Set equality over two value lists (order-insensitive, duplicates ignored).
fn value_set_eq(a: &[String], b: &[String]) -> bool {
    a.len() == b.len() && a.iter().all(|v| b.iter().any(|w| w == v))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn diff_no_change_is_empty() {
        let e = entry("cn=Alice,dc=x", &[("cn", &["Alice"]), ("sn", &["Adams"])]);
        let cs = diff(&e, &e).unwrap();
        assert!(cs.is_empty(), "cs={cs:?}");
    }

    #[test]
    fn diff_added_value_emits_add() {
        let orig = entry("uid=a,dc=x", &[("uid", &["a"]), ("mail", &["a@x"])]);
        let edited = entry("uid=a,dc=x", &[("uid", &["a"]), ("mail", &["a@x", "a2@x"])]);
        let cs = diff(&orig, &edited).unwrap();
        assert_eq!(
            cs.mods,
            vec![ModOp::Add {
                attr: "mail".to_string(),
                values: vec!["a2@x".to_string()]
            }]
        );
    }

    #[test]
    fn diff_removed_value_emits_delete() {
        let orig = entry("uid=a,dc=x", &[("uid", &["a"]), ("mail", &["a@x", "a2@x"])]);
        let edited = entry("uid=a,dc=x", &[("uid", &["a"]), ("mail", &["a@x"])]);
        let cs = diff(&orig, &edited).unwrap();
        assert_eq!(
            cs.mods,
            vec![ModOp::Delete {
                attr: "mail".to_string(),
                values: vec!["a2@x".to_string()]
            }]
        );
    }

    #[test]
    fn diff_new_attr_emits_add() {
        let orig = entry("uid=a,dc=x", &[("uid", &["a"])]);
        let edited = entry("uid=a,dc=x", &[("uid", &["a"]), ("description", &["hi"])]);
        let cs = diff(&orig, &edited).unwrap();
        assert_eq!(
            cs.mods,
            vec![ModOp::Add {
                attr: "description".to_string(),
                values: vec!["hi".to_string()]
            }]
        );
    }

    #[test]
    fn diff_cleared_attr_emits_delete_whole() {
        let orig = entry("uid=a,dc=x", &[("uid", &["a"]), ("description", &["hi"])]);
        let edited = entry("uid=a,dc=x", &[("uid", &["a"]), ("description", &[])]);
        let cs = diff(&orig, &edited).unwrap();
        assert_eq!(
            cs.mods,
            vec![ModOp::Delete {
                attr: "description".to_string(),
                values: vec![]
            }]
        );
    }

    #[test]
    fn diff_changed_single_value_emits_replace() {
        let orig = entry("uid=a,dc=x", &[("uid", &["a"]), ("sn", &["Adams"])]);
        let edited = entry("uid=a,dc=x", &[("uid", &["a"]), ("sn", &["Brown"])]);
        let cs = diff(&orig, &edited).unwrap();
        assert_eq!(
            cs.mods,
            vec![ModOp::Replace {
                attr: "sn".to_string(),
                values: vec!["Brown".to_string()]
            }]
        );
    }

    #[test]
    fn rdn_component_parses_simple() {
        assert_eq!(
            rdn_component("cn=Alice,ou=people,dc=x"),
            Some(("cn".to_string(), "Alice".to_string()))
        );
    }

    #[test]
    fn diff_rdn_change_emits_modrdn_not_modify() {
        // cn is the RDN; editing it from Alice to Bob must produce a MODRDN and
        // NO modify op for cn.
        let orig = entry("cn=Alice,dc=x", &[("cn", &["Alice"]), ("sn", &["Adams"])]);
        let edited = entry("cn=Alice,dc=x", &[("cn", &["Bob"]), ("sn", &["Adams"])]);
        let cs = diff(&orig, &edited).unwrap();
        assert_eq!(
            cs.modrdn,
            Some(ModRdn {
                new_rdn: "cn=Bob".to_string(),
                delete_old: true,
                new_superior: None,
            })
        );
        assert!(
            !cs.mods.iter().any(|m| match m {
                ModOp::Add { attr, .. }
                | ModOp::Delete { attr, .. }
                | ModOp::Replace { attr, .. } => attr.eq_ignore_ascii_case("cn"),
            }),
            "cn must not also appear in mods; mods={:?}",
            cs.mods
        );
    }

    #[test]
    fn diff_rdn_unchanged_no_modrdn() {
        // cn edited elsewhere (sn changes) but the RDN value is identical.
        let orig = entry("cn=Alice,dc=x", &[("cn", &["Alice"]), ("sn", &["Adams"])]);
        let edited = entry("cn=Alice,dc=x", &[("cn", &["Alice"]), ("sn", &["Brown"])]);
        let cs = diff(&orig, &edited).unwrap();
        assert!(cs.modrdn.is_none(), "no rename expected; cs={cs:?}");
        assert_eq!(
            cs.mods,
            vec![ModOp::Replace {
                attr: "sn".to_string(),
                values: vec!["Brown".to_string()]
            }]
        );
    }

    #[test]
    fn diff_multivalued_rdn_is_refused() {
        let orig = entry("cn=x+uid=y,dc=x", &[("cn", &["x"]), ("uid", &["y"])]);
        let edited = entry("cn=x+uid=y,dc=x", &[("cn", &["z"]), ("uid", &["y"])]);
        assert_eq!(
            diff(&orig, &edited),
            Err(ChangeSetError::MultiValuedRdnUnsupported)
        );
    }

    #[test]
    fn changeset_is_empty_default() {
        assert!(ChangeSet::default().is_empty());
    }
}
