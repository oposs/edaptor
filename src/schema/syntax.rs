//! Classify an attribute's LDAP syntax (RFC 4517) into a coarse field kind.
//! M3 maps each FieldKind to a concrete TUI widget.

use oid::ObjectIdentifier;

/// Coarse semantic classification of an attribute value, from its syntax OID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    Boolean,
    Integer,
    DistinguishedName,
    GeneralizedTime,
    Binary,
}

// Well-known RFC 4517 syntax OIDs we special-case. Everything else → Text.
const OID_BOOLEAN: &str = "1.3.6.1.4.1.1466.115.121.1.7";
const OID_INTEGER: &str = "1.3.6.1.4.1.1466.115.121.1.27";
const OID_DN: &str = "1.3.6.1.4.1.1466.115.121.1.12";
const OID_GENERALIZED_TIME: &str = "1.3.6.1.4.1.1466.115.121.1.24";
const OID_OCTET_STRING: &str = "1.3.6.1.4.1.1466.115.121.1.40";
const OID_BINARY: &str = "1.3.6.1.4.1.1466.115.121.1.5";
const OID_JPEG: &str = "1.3.6.1.4.1.1466.115.121.1.28";

/// Classify a syntax OID. Unknown syntaxes default to Text.
pub fn classify_syntax(syntax: &ObjectIdentifier) -> FieldKind {
    let is = |s: &str| {
        ObjectIdentifier::try_from(s)
            .map(|o| &o == syntax)
            .unwrap_or(false)
    };
    if is(OID_BOOLEAN) {
        FieldKind::Boolean
    } else if is(OID_INTEGER) {
        FieldKind::Integer
    } else if is(OID_DN) {
        FieldKind::DistinguishedName
    } else if is(OID_GENERALIZED_TIME) {
        FieldKind::GeneralizedTime
    } else if is(OID_OCTET_STRING) || is(OID_BINARY) || is(OID_JPEG) {
        FieldKind::Binary
    } else {
        FieldKind::Text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oid::ObjectIdentifier;

    fn oid(s: &str) -> ObjectIdentifier {
        ObjectIdentifier::try_from(s).unwrap()
    }

    #[test]
    fn classifies_known_syntaxes() {
        assert_eq!(
            classify_syntax(&oid("1.3.6.1.4.1.1466.115.121.1.7")),
            FieldKind::Boolean
        );
        assert_eq!(
            classify_syntax(&oid("1.3.6.1.4.1.1466.115.121.1.27")),
            FieldKind::Integer
        );
        assert_eq!(
            classify_syntax(&oid("1.3.6.1.4.1.1466.115.121.1.12")),
            FieldKind::DistinguishedName
        );
        assert_eq!(
            classify_syntax(&oid("1.3.6.1.4.1.1466.115.121.1.24")),
            FieldKind::GeneralizedTime
        );
        assert_eq!(
            classify_syntax(&oid("1.3.6.1.4.1.1466.115.121.1.40")),
            FieldKind::Binary
        );
    }

    #[test]
    fn unknown_syntax_defaults_to_text() {
        // DirectoryString and an arbitrary OID both fall through to Text.
        assert_eq!(
            classify_syntax(&oid("1.3.6.1.4.1.1466.115.121.1.15")),
            FieldKind::Text
        );
        assert_eq!(classify_syntax(&oid("1.2.3.4.5.6.7.8")), FieldKind::Text);
    }
}
