//! Shared `#[cfg(test)]` domain fixtures (no UI/ratatui types), reused by the
//! workflows and ui test modules so they aren't duplicated.
#![cfg(test)]

use crate::config::{EntryProfile, PasswordSpec};
use crate::ldap::worker::RawSubschema;
use crate::schema::SchemaModel;
use std::collections::BTreeMap;

pub(crate) fn bare_profile(name: &str) -> EntryProfile {
    EntryProfile {
        name: name.into(),
        object_classes: vec![],
        rdn_attr: String::new(),
        search_base: String::new(),
        show: vec![],
        search_attrs: vec![],
        defaults: Default::default(),
        password: None,
        pickers: Default::default(),
        label: None,
    }
}

pub(crate) fn attr_map(pairs: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
    pairs
        .iter()
        .map(|(k, vs)| (k.to_string(), vs.iter().map(|s| s.to_string()).collect()))
        .collect()
}

/// Minimal schema for user (inetOrgPerson-like) with uid, description, memberOf.
pub(crate) fn user_schema() -> SchemaModel {
    let raw = RawSubschema {
        object_classes: vec![
            // No SUP top so validate does not require objectClass in the entry.
            "( 1.2.3.4 NAME 'testUser' STRUCTURAL MUST uid MAY ( description $ memberOf ) )".to_string(),
        ],
        attribute_types: vec![
            "( 0.9.2342.19200300.100.1.1 NAME 'uid' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 SINGLE-VALUE )".to_string(),
            "( 2.5.4.13 NAME 'description' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".to_string(),
            "( 1.2.840.113556.1.2.102 NAME 'memberOf' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".to_string(),
        ],
        ldap_syntaxes: vec![],
    };
    SchemaModel::from_raw(&raw)
}

pub(crate) fn create_user_profile() -> EntryProfile {
    EntryProfile {
        name: "User".into(),
        object_classes: vec!["testUser".into()],
        rdn_attr: "uid".into(),
        search_base: "ou=people,dc=example,dc=org".into(),
        show: vec!["uid".into()],
        search_attrs: vec![],
        defaults: Default::default(),
        password: None,
        pickers: Default::default(),
        label: None,
    }
}

pub(crate) fn pw_spec(samba: bool) -> PasswordSpec {
    PasswordSpec {
        ldap_attribute: "userPassword".into(),
        samba,
    }
}
