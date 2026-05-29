//! The typed schema model: parses raw definitions, indexes them by name
//! (case-insensitive, alias-aware), and resolves SUP inheritance.

use std::collections::{BTreeSet, HashMap};

use chumsky::Parser;
use ldap_types::schema::{attribute_type_parser, object_class_parser, AttributeType, ObjectClass};

use crate::ldap::worker::RawSubschema;

pub struct SchemaModel {
    object_classes: Vec<ObjectClass>,
    attribute_types: Vec<AttributeType>,
    oc_by_name: HashMap<String, usize>, // lowercased name (incl. aliases) -> index
    at_by_name: HashMap<String, usize>,
    /// Definitions the server returned that we could not parse (diagnostics).
    pub warnings: Vec<String>,
}

/// Resolved required/optional attributes for a set of object classes.
/// Names are canonical (the attribute type's primary name when known).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ResolvedAttributes {
    pub must: BTreeSet<String>,
    pub may: BTreeSet<String>,
}

impl SchemaModel {
    pub fn from_raw(raw: &RawSubschema) -> SchemaModel {
        let mut warnings = Vec::new();

        let oc_parser = object_class_parser();
        let mut object_classes = Vec::new();
        for desc in &raw.object_classes {
            match oc_parser.parse(desc.as_str()).into_result() {
                Ok(oc) => object_classes.push(oc),
                Err(errs) => {
                    warnings.push(format!("objectClass parse error in {desc:?}: {errs:?}"))
                }
            }
        }

        let at_parser = attribute_type_parser();
        let mut attribute_types = Vec::new();
        for desc in &raw.attribute_types {
            match at_parser.parse(desc.as_str()).into_result() {
                Ok(at) => attribute_types.push(at),
                Err(errs) => {
                    warnings.push(format!("attributeType parse error in {desc:?}: {errs:?}"))
                }
            }
        }

        let mut oc_by_name = HashMap::new();
        for (i, oc) in object_classes.iter().enumerate() {
            for n in &oc.name {
                oc_by_name.insert(n.to_string().to_lowercase(), i);
            }
        }
        let mut at_by_name = HashMap::new();
        for (i, at) in attribute_types.iter().enumerate() {
            for n in &at.name {
                at_by_name.insert(n.to_string().to_lowercase(), i);
            }
        }

        SchemaModel {
            object_classes,
            attribute_types,
            oc_by_name,
            at_by_name,
            warnings,
        }
    }

    pub fn object_class(&self, name: &str) -> Option<&ObjectClass> {
        self.oc_by_name
            .get(&name.to_lowercase())
            .map(|&i| &self.object_classes[i])
    }

    pub fn attribute_type(&self, name: &str) -> Option<&AttributeType> {
        self.at_by_name
            .get(&name.to_lowercase())
            .map(|&i| &self.attribute_types[i])
    }

    pub fn object_class_count(&self) -> usize {
        self.object_classes.len()
    }

    pub fn attribute_type_count(&self) -> usize {
        self.attribute_types.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldap::worker::RawSubschema;

    fn raw() -> RawSubschema {
        RawSubschema {
            object_classes: vec![
                "( 2.5.6.0 NAME 'top' ABSTRACT MUST objectClass )".to_string(),
                "( 2.5.6.6 NAME 'person' SUP top STRUCTURAL MUST ( sn $ cn ) \
                  MAY ( userPassword $ description ) )"
                    .to_string(),
                "garbage not a definition".to_string(), // must be tolerated as a warning
            ],
            attribute_types: vec![
                "( 2.5.4.4 NAME ( 'sn' 'surname' ) SUP name )".to_string(),
                "( 2.5.4.3 NAME 'cn' SUP name )".to_string(),
            ],
            ldap_syntaxes: vec![],
        }
    }

    #[test]
    fn parses_and_counts_with_warnings() {
        let m = SchemaModel::from_raw(&raw());
        assert_eq!(m.object_class_count(), 2); // top + person; garbage skipped
        assert_eq!(m.attribute_type_count(), 2);
        assert_eq!(m.warnings.len(), 1); // the garbage line
    }

    #[test]
    fn lookup_is_case_insensitive_and_handles_aliases() {
        let m = SchemaModel::from_raw(&raw());
        assert!(m.object_class("PERSON").is_some());
        assert!(m.object_class("person").is_some());
        assert!(m.object_class("nope").is_none());
        // alias: 'surname' resolves to the same attribute as 'sn'
        assert!(m.attribute_type("surname").is_some());
        assert!(m.attribute_type("SN").is_some());
    }
}
