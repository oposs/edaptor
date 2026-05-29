//! Typed LDAP schema model built from the raw subschema (headless: no network/UI).

pub mod model;
pub mod syntax;

pub use model::{ResolvedAttributes, SchemaModel};
pub use syntax::{classify_syntax, FieldKind};
