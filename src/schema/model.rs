//! The typed schema model: parses raw definitions, indexes them by name
//! (case-insensitive, alias-aware), and resolves SUP inheritance.

use std::collections::{BTreeSet, HashMap, HashSet};

use chumsky::Parser;
use ldap_types::schema::{attribute_type_parser, object_class_parser, AttributeType, ObjectClass};

use crate::ldap::worker::RawSubschema;
use crate::schema::syntax::{classify_syntax, FieldKind};

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

    /// The canonical (primary) name of an attribute, or the referenced name if
    /// the attribute type is unknown. Lets set operations dedup consistently.
    fn canonical_attr(&self, referenced: &str) -> String {
        self.attribute_type(referenced)
            .and_then(|at| at.name.first())
            .map(|n| n.to_string())
            .unwrap_or_else(|| referenced.to_string())
    }

    /// Resolve the effective MUST/MAY attributes for a set of object classes,
    /// walking SUP inheritance. An attribute required by any class is MUST and
    /// is excluded from MAY.
    pub fn effective_attributes(&self, object_classes: &[&str]) -> ResolvedAttributes {
        let mut must = BTreeSet::new();
        let mut may = BTreeSet::new();
        let mut visited = HashSet::new();
        for &name in object_classes {
            self.collect_class(name, &mut must, &mut may, &mut visited);
        }
        // An attribute required by any class is MUST; drop it from MAY. Compare
        // case-insensitively so an undefined attribute referenced with differing
        // case across classes is still deduplicated (display case is preserved).
        may.retain(|m| !must.iter().any(|r| r.eq_ignore_ascii_case(m)));
        ResolvedAttributes { must, may }
    }

    fn collect_class(
        &self,
        name: &str,
        must: &mut BTreeSet<String>,
        may: &mut BTreeSet<String>,
        visited: &mut HashSet<String>,
    ) {
        if !visited.insert(name.to_lowercase()) {
            return; // already processed (also guards against SUP cycles)
        }
        let Some(oc) = self.object_class(name) else {
            return;
        };
        // Clone the referenced names out before recursing (avoids borrow conflicts).
        let must_names: Vec<String> = oc.must.iter().map(|a| a.to_string()).collect();
        let may_names: Vec<String> = oc.may.iter().map(|a| a.to_string()).collect();
        let sups: Vec<String> = oc.sup.iter().map(|s| s.to_string()).collect();
        for a in must_names {
            let c = self.canonical_attr(&a);
            must.insert(c);
        }
        for a in may_names {
            let c = self.canonical_attr(&a);
            may.insert(c);
        }
        for sup in sups {
            self.collect_class(&sup, must, may, visited);
        }
    }

    /// The FieldKind of an attribute, following the SUP chain to find the first
    /// declared SYNTAX. Defaults to Text when no syntax is found.
    pub fn field_kind(&self, attr_name: &str) -> FieldKind {
        let mut current = self.attribute_type(attr_name);
        for _ in 0..64 {
            // bounded against malformed SUP cycles
            let Some(at) = current else {
                break;
            };
            if let Some(syntax) = &at.syntax {
                return classify_syntax(&syntax.oid);
            }
            current = at
                .sup
                .as_ref()
                .and_then(|s| self.attribute_type(&s.to_string()));
        }
        FieldKind::Text
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

    fn inheritance_raw() -> RawSubschema {
        RawSubschema {
            object_classes: vec![
                "( 2.5.6.0 NAME 'top' ABSTRACT MUST objectClass )".to_string(),
                "( 2.5.6.6 NAME 'person' SUP top STRUCTURAL MUST ( sn $ cn ) \
                  MAY ( userPassword $ description ) )"
                    .to_string(),
                "( 2.5.6.7 NAME 'organizationalPerson' SUP person STRUCTURAL \
                  MAY ( title $ ou ) )"
                    .to_string(),
                "( 2.16.840.1.113730.3.2.2 NAME 'inetOrgPerson' SUP organizationalPerson \
                  STRUCTURAL MAY ( mail $ givenName ) )"
                    .to_string(),
            ],
            attribute_types: vec![],
            ldap_syntaxes: vec![],
        }
    }

    #[test]
    fn effective_attributes_walk_the_sup_chain() {
        let m = SchemaModel::from_raw(&inheritance_raw());
        let r = m.effective_attributes(&["inetOrgPerson"]);
        // MUST inherited from person (and objectClass from top):
        assert!(r.must.contains("sn"), "must={:?}", r.must);
        assert!(r.must.contains("cn"));
        assert!(r.must.contains("objectClass"));
        // MAY from the chain:
        assert!(r.may.contains("mail"));
        assert!(r.may.contains("title"));
        assert!(r.may.contains("description"));
        // An attribute that is MUST anywhere must NOT also appear in MAY:
        assert!(!r.may.contains("sn"));
    }

    #[test]
    fn unknown_object_class_yields_empty() {
        let m = SchemaModel::from_raw(&inheritance_raw());
        assert_eq!(
            m.effective_attributes(&["doesNotExist"]),
            ResolvedAttributes::default()
        );
    }

    fn syntax_raw() -> RawSubschema {
        RawSubschema {
            object_classes: vec![],
            attribute_types: vec![
                // 'name' carries the DirectoryString syntax → Text.
                "( 2.5.4.41 NAME 'name' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15{32768} )".to_string(),
                // 'sn' has NO syntax of its own; it must inherit from SUP name → Text.
                "( 2.5.4.4 NAME ( 'sn' 'surname' ) SUP name )".to_string(),
                // a boolean attribute, single-valued.
                "( 2.5.4.100 NAME 'flag' SYNTAX 1.3.6.1.4.1.1466.115.121.1.7 SINGLE-VALUE )"
                    .to_string(),
                // a DN-valued attribute.
                "( 2.5.4.49 NAME 'member' SYNTAX 1.3.6.1.4.1.1466.115.121.1.12 )".to_string(),
            ],
            ldap_syntaxes: vec![],
        }
    }

    #[test]
    fn field_kind_follows_syntax_and_sup_chain() {
        let m = SchemaModel::from_raw(&syntax_raw());
        assert_eq!(m.field_kind("name"), FieldKind::Text);
        assert_eq!(m.field_kind("sn"), FieldKind::Text); // inherited from name
        assert_eq!(m.field_kind("flag"), FieldKind::Boolean);
        assert_eq!(m.field_kind("member"), FieldKind::DistinguishedName);
        assert_eq!(m.field_kind("unknownAttr"), FieldKind::Text); // default
    }

    #[test]
    fn undefined_attr_with_mixed_case_does_not_appear_in_both_sets() {
        // 'Foo'/'foo' are referenced but never defined as attribute types.
        // Canonicalization must still dedup them so MUST wins over MAY.
        let raw = RawSubschema {
            object_classes: vec![
                "( 1.1.1 NAME 'a' STRUCTURAL MUST Foo )".to_string(),
                "( 1.1.2 NAME 'b' SUP a STRUCTURAL MAY foo )".to_string(),
            ],
            attribute_types: vec![],
            ldap_syntaxes: vec![],
        };
        let m = SchemaModel::from_raw(&raw);
        let r = m.effective_attributes(&["b"]);
        assert!(
            r.must.iter().any(|a| a.eq_ignore_ascii_case("foo")),
            "must={:?}",
            r.must
        );
        assert!(
            !r.may.iter().any(|a| a.eq_ignore_ascii_case("foo")),
            "undefined attr in both sets; may={:?}",
            r.may
        );
    }
}
