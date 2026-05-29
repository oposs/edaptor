# edaptor M2 — Schema Model & Introspection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the raw subschema strings M1 fetches into a typed `SchemaModel` that resolves an object class's effective MUST/MAY attributes across SUP inheritance and classifies each attribute's syntax into a `FieldKind` — exposed via an `edaptor schema <objectClass>` CLI subcommand.

**Architecture:** A new headless `schema` module consumes `RawSubschema` (no network, no UI). It uses the `ldap-types` chumsky parsers to parse each definition, indexes classes/attributes by name (case-insensitive), and walks SUP chains itself (the crate does not resolve inheritance). Builds on M1's worker/`fetch` pipeline.

**Tech Stack:** Rust 2021; `ldap-types` 0.7 (chumsky feature only), `chumsky` 0.12, `oid` 0.3 — all three verified to compile together (see `docs/superpowers/research/2026-05-29-api-spike-findings.md` §3a). Existing: `ldap3`, `clap`, `serde`/`toml`, `anyhow`.

---

## Context from M1 (already on `main`)

- `edaptor::ldap::worker::RawSubschema { object_classes: Vec<String>, attribute_types: Vec<String>, ldap_syntaxes: Vec<String> }` — raw RFC 4512 description strings.
- `edaptor::run_check(config, password) -> Result<CheckSummary>` spawns the worker and fetches the subschema.
- `src/lib.rs` declares `pub mod config; pub mod ldap;`.
- `src/main.rs` is a single-action CLI (`--config`, prints the M1 summary).

## Verified `ldap-types` facts (from the spike — do not re-derive)

- Deps: `ldap-types = { version = "0.7", default-features = false, features = ["chumsky"] }`, `chumsky = "0.12"`, `oid = "0.3"`.
- `use chumsky::Parser;` then `object_class_parser().parse(s).into_result()` / `attribute_type_parser()`. Build the parser ONCE and reuse it across the loop (`.parse` takes `&self`).
- `KeyString` and `KeyStringOrOID` implement `Display` → use `.to_string()` for the name. (No `Deref` to `str`.)
- `ObjectClass { name: Vec<KeyString>, sup: Vec<KeyStringOrOID>, object_class_type: ObjectClassType (Structural/Abstract/Auxiliary), must: Vec<KeyStringOrOID>, may: Vec<KeyStringOrOID>, obsolete: bool, .. }`.
- `AttributeType { name: Vec<KeyString>, sup: Option<KeyStringOrOID>, syntax: Option<OIDWithLength{ oid: oid::ObjectIdentifier, length: Option<u32> }>, single_value: bool, .. }`.
- `ObjectIdentifier` has NO `Display`; classify via `oid == ObjectIdentifier::try_from("1.3.6...").unwrap()` (PartialEq verified).
- `into_result()` returns `Result<T, Vec<chumsky::error::Rich<char>>>` (the error type is `Debug`).

---

## M2 File Structure

```
src/
├── lib.rs                    # MODIFY: add `pub mod schema;`, run_schema, SchemaReport, fetch_raw refactor
├── main.rs                   # MODIFY: clap subcommands (Check default + Schema { object_class })
└── schema/
    ├── mod.rs                # CREATE: `pub mod model; pub mod syntax;` + re-exports
    ├── syntax.rs             # CREATE: FieldKind enum + classify_syntax(&ObjectIdentifier)
    └── model.rs              # CREATE: SchemaModel (parse/index/lookups/inheritance/field_kind), ResolvedAttributes
```

---

## Task 1: Dependencies + syntax classification

**Files:**
- Modify: `Cargo.toml`
- Create: `src/schema/syntax.rs`, `src/schema/mod.rs`
- Modify: `src/lib.rs` (declare the module)
- Test: inline `#[cfg(test)]` in `src/schema/syntax.rs`

- [ ] **Step 1: Add dependencies**

Run (in the crate root):
```bash
cargo add ldap-types@0.7 --no-default-features --features chumsky
cargo add chumsky@0.12
cargo add oid@0.3
```
Expected: all three resolve (ldap-types pulls neither ldap3 nor serde with these features).

- [ ] **Step 2: Create the module skeleton**

Create `src/schema/mod.rs`:
```rust
//! Typed LDAP schema model built from the raw subschema (headless: no network/UI).

pub mod model;
pub mod syntax;

pub use model::{ResolvedAttributes, SchemaModel};
pub use syntax::{classify_syntax, FieldKind};
```

Add to `src/lib.rs` (after `pub mod ldap;`):
```rust
pub mod schema;
```

(`model` is created in Task 2; the crate will not compile until then. To run Task 1's tests in isolation, temporarily comment out `pub mod model;` and the `model::` re-export, then restore them in Task 2. Verify the final `mod.rs` re-exports both.)

- [ ] **Step 3: Write the failing tests**

Create `src/schema/syntax.rs` with the test module:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use oid::ObjectIdentifier;

    fn oid(s: &str) -> ObjectIdentifier {
        ObjectIdentifier::try_from(s).unwrap()
    }

    #[test]
    fn classifies_known_syntaxes() {
        assert_eq!(classify_syntax(&oid("1.3.6.1.4.1.1466.115.121.1.7")), FieldKind::Boolean);
        assert_eq!(classify_syntax(&oid("1.3.6.1.4.1.1466.115.121.1.27")), FieldKind::Integer);
        assert_eq!(classify_syntax(&oid("1.3.6.1.4.1.1466.115.121.1.12")), FieldKind::DistinguishedName);
        assert_eq!(classify_syntax(&oid("1.3.6.1.4.1.1466.115.121.1.24")), FieldKind::GeneralizedTime);
        assert_eq!(classify_syntax(&oid("1.3.6.1.4.1.1466.115.121.1.40")), FieldKind::Binary);
    }

    #[test]
    fn unknown_syntax_defaults_to_text() {
        // DirectoryString and an arbitrary OID both fall through to Text.
        assert_eq!(classify_syntax(&oid("1.3.6.1.4.1.1466.115.121.1.15")), FieldKind::Text);
        assert_eq!(classify_syntax(&oid("1.2.3.4.5.6.7.8")), FieldKind::Text);
    }
}
```

- [ ] **Step 4: Run tests to verify they fail**

Run: `cargo test --lib schema::syntax`
Expected: FAIL — `classify_syntax` / `FieldKind` not defined.

- [ ] **Step 5: Write the implementation (above the test module)**

Prepend to `src/schema/syntax.rs`:
```rust
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
    let is = |s: &str| ObjectIdentifier::try_from(s).map(|o| &o == syntax).unwrap_or(false);
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
```

- [ ] **Step 6: Run tests, clippy, fmt**

Run: `cargo test --lib schema::syntax` → PASS (2 tests, after Task 2 makes the crate compile; or with `model` temporarily commented out).
Run: `cargo clippy --all-targets` → clean. Run `cargo fmt`.

- [ ] **Step 7: Commit (after Task 2 compiles)**

Defer to end of Task 2, since `schema/mod.rs` references `model`.

---

## Task 2: SchemaModel — parse, index, lookups

**Files:**
- Create: `src/schema/model.rs`
- Test: inline `#[cfg(test)]` in `src/schema/model.rs`

- [ ] **Step 1: Write the failing tests**

Create `src/schema/model.rs` with the test module:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldap::worker::RawSubschema;

    fn raw() -> RawSubschema {
        RawSubschema {
            object_classes: vec![
                "( 2.5.6.0 NAME 'top' ABSTRACT MUST objectClass )".to_string(),
                "( 2.5.6.6 NAME 'person' SUP top STRUCTURAL MUST ( sn $ cn ) \
                  MAY ( userPassword $ description ) )".to_string(),
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib schema::model`
Expected: FAIL — `SchemaModel` not defined.

- [ ] **Step 3: Write the implementation (above the test module)**

Prepend to `src/schema/model.rs`:
```rust
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
                Err(errs) => warnings.push(format!("objectClass parse error in {desc:?}: {errs:?}")),
            }
        }

        let at_parser = attribute_type_parser();
        let mut attribute_types = Vec::new();
        for desc in &raw.attribute_types {
            match at_parser.parse(desc.as_str()).into_result() {
                Ok(at) => attribute_types.push(at),
                Err(errs) => warnings.push(format!("attributeType parse error in {desc:?}: {errs:?}")),
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

        SchemaModel { object_classes, attribute_types, oc_by_name, at_by_name, warnings }
    }

    pub fn object_class(&self, name: &str) -> Option<&ObjectClass> {
        self.oc_by_name.get(&name.to_lowercase()).map(|&i| &self.object_classes[i])
    }

    pub fn attribute_type(&self, name: &str) -> Option<&AttributeType> {
        self.at_by_name.get(&name.to_lowercase()).map(|&i| &self.attribute_types[i])
    }

    pub fn object_class_count(&self) -> usize {
        self.object_classes.len()
    }

    pub fn attribute_type_count(&self) -> usize {
        self.attribute_types.len()
    }
}
```

(`canonical_attr`, the inheritance methods, and the `syntax` import are added in Tasks 3–4, where they are first used, so Task 2 stays warning-free.)

- [ ] **Step 4: Restore the module declarations and run tests**

Ensure `src/schema/mod.rs` has both `pub mod model;` and `pub mod syntax;` and the re-exports (un-comment if you commented them in Task 1).

Run: `cargo test --lib schema` → PASS (Task 1's 2 syntax tests + Task 2's 2 model tests).
Run: `cargo clippy --all-targets` → clean. `cargo fmt`.

- [ ] **Step 5: Commit**

```bash
git -c user.name='oetiker' -c user.email='oetiker@gmail.com' add src/schema/ src/lib.rs Cargo.toml Cargo.lock
git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -m "$(printf 'M2: schema model parsing, indexing, and case-insensitive lookups\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 3: Effective MUST/MAY with SUP inheritance

**Files:**
- Modify: `src/schema/model.rs` (add `effective_attributes` + a test)

- [ ] **Step 1: Write the failing test (add to the existing test module)**

Add inside `#[cfg(test)] mod tests` in `src/schema/model.rs`:
```rust
    fn inheritance_raw() -> RawSubschema {
        RawSubschema {
            object_classes: vec![
                "( 2.5.6.0 NAME 'top' ABSTRACT MUST objectClass )".to_string(),
                "( 2.5.6.6 NAME 'person' SUP top STRUCTURAL MUST ( sn $ cn ) \
                  MAY ( userPassword $ description ) )".to_string(),
                "( 2.5.6.7 NAME 'organizationalPerson' SUP person STRUCTURAL \
                  MAY ( title $ ou ) )".to_string(),
                "( 2.16.840.1.113730.3.2.2 NAME 'inetOrgPerson' SUP organizationalPerson \
                  STRUCTURAL MAY ( mail $ givenName ) )".to_string(),
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
        assert_eq!(m.effective_attributes(&["doesNotExist"]), ResolvedAttributes::default());
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib schema::model::tests::effective_attributes_walk_the_sup_chain`
Expected: FAIL — `effective_attributes` not defined.

- [ ] **Step 3: Implement**

First, add `HashSet` to the collections import at the top of `src/schema/model.rs`:
```rust
use std::collections::{BTreeSet, HashMap, HashSet};
```

Then add these methods to `impl SchemaModel`:
```rust
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
        for m in &must {
            may.remove(m);
        }
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
```

- [ ] **Step 4: Run tests, clippy, fmt**

Run: `cargo test --lib schema` → PASS (now 6 tests).
Run: `cargo clippy --all-targets` → clean. `cargo fmt`.

- [ ] **Step 5: Commit**

```bash
git -c user.name='oetiker' -c user.email='oetiker@gmail.com' add src/schema/model.rs
git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -m "$(printf 'M2: resolve effective MUST/MAY attributes across SUP inheritance\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 4: field_kind with SYNTAX inheritance

**Files:**
- Modify: `src/schema/model.rs` (add the `use`, `field_kind`, and a test)

- [ ] **Step 1: Write the failing test**

Add to the test module in `src/schema/model.rs`:
```rust
    fn syntax_raw() -> RawSubschema {
        RawSubschema {
            object_classes: vec![],
            attribute_types: vec![
                // 'name' carries the DirectoryString syntax → Text.
                "( 2.5.4.41 NAME 'name' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15{32768} )".to_string(),
                // 'sn' has NO syntax of its own; it must inherit from SUP name → Text.
                "( 2.5.4.4 NAME ( 'sn' 'surname' ) SUP name )".to_string(),
                // a boolean attribute, single-valued.
                "( 2.5.4.100 NAME 'flag' SYNTAX 1.3.6.1.4.1.1466.115.121.1.7 SINGLE-VALUE )".to_string(),
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
        assert_eq!(m.field_kind("sn"), FieldKind::Text);       // inherited from name
        assert_eq!(m.field_kind("flag"), FieldKind::Boolean);
        assert_eq!(m.field_kind("member"), FieldKind::DistinguishedName);
        assert_eq!(m.field_kind("unknownAttr"), FieldKind::Text); // default
    }
```
Also add `use crate::schema::syntax::FieldKind;` to the test module's `use super::*;` context if needed — `FieldKind` is re-exported from `super` once Task 4's `use` is in place, so `super::*` covers it.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --lib schema::model::tests::field_kind_follows_syntax_and_sup_chain`
Expected: FAIL — `field_kind` not defined.

- [ ] **Step 3: Implement**

Add the import at the top of `src/schema/model.rs` (next to the other `use` lines):
```rust
use crate::schema::syntax::{classify_syntax, FieldKind};
```

Add this method to `impl SchemaModel`:
```rust
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
            current = at.sup.as_ref().and_then(|s| self.attribute_type(&s.to_string()));
        }
        FieldKind::Text
    }
```

- [ ] **Step 4: Run tests, clippy, fmt**

Run: `cargo test --lib schema` → PASS (7 tests).
Run: `cargo clippy --all-targets` → clean. `cargo fmt`.

- [ ] **Step 5: Commit**

```bash
git -c user.name='oetiker' -c user.email='oetiker@gmail.com' add src/schema/model.rs
git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -m "$(printf 'M2: classify attribute field kinds with SYNTAX inheritance\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 5: run_schema + CLI subcommands

**Files:**
- Modify: `src/lib.rs` (refactor a `fetch_raw` helper; add `run_schema`, `SchemaReport`, `SchemaAttrReport`)
- Modify: `src/main.rs` (clap subcommands)

- [ ] **Step 1: Refactor lib.rs**

Replace the body of `src/lib.rs` below the `pub mod` lines with:
```rust
use anyhow::{anyhow, Result};

use crate::config::Config;
use crate::ldap::worker::{RawSubschema, Request, Response, WorkerHandle};
use crate::schema::{FieldKind, SchemaModel};

/// Result of the M1 connectivity + schema-fetch check.
pub struct CheckSummary {
    pub uri: String,
    pub bind_dn: Option<String>,
    pub object_class_count: usize,
    pub attribute_type_count: usize,
    pub ldap_syntax_count: usize,
}

/// Connect, bind, and fetch the raw subschema. Shared by run_check / run_schema.
fn fetch_raw(config: Config, password: String) -> Result<RawSubschema> {
    let handle = WorkerHandle::spawn(config, password)?;
    match handle.request(Request::FetchSubschema)? {
        Response::Subschema(raw) => Ok(raw),
        Response::Error(e) => Err(anyhow!(e)),
        Response::Done => Err(anyhow!("unexpected Done response to FetchSubschema")),
    }
}

/// Connect, bind, fetch the raw subschema, and summarize counts.
pub fn run_check(config: Config, password: String) -> Result<CheckSummary> {
    let uri = config.server.uri.clone();
    let bind_dn = config.auth.bind_dn.clone();
    let raw = fetch_raw(config, password)?;
    Ok(CheckSummary {
        uri,
        bind_dn,
        object_class_count: raw.object_classes.len(),
        attribute_type_count: raw.attribute_types.len(),
        ldap_syntax_count: raw.ldap_syntaxes.len(),
    })
}

/// One attribute of a resolved object class.
pub struct SchemaAttrReport {
    pub name: String,
    pub required: bool,
    pub kind: FieldKind,
    pub single_value: bool,
}

/// The effective attribute set of an object class, for display.
pub struct SchemaReport {
    pub object_class: String,
    pub attributes: Vec<SchemaAttrReport>,
    pub parse_warnings: usize,
}

/// Fetch the schema and resolve the effective attributes of one object class.
pub fn run_schema(config: Config, password: String, object_class: &str) -> Result<SchemaReport> {
    let raw = fetch_raw(config, password)?;
    let model = SchemaModel::from_raw(&raw);
    if model.object_class(object_class).is_none() {
        return Err(anyhow!("object class '{object_class}' not found in the server schema"));
    }
    let resolved = model.effective_attributes(&[object_class]);

    let mut attributes = Vec::new();
    for name in &resolved.must {
        attributes.push(make_row(&model, name, true));
    }
    for name in &resolved.may {
        attributes.push(make_row(&model, name, false));
    }

    Ok(SchemaReport {
        object_class: object_class.to_string(),
        attributes,
        parse_warnings: model.warnings.len(),
    })
}

fn make_row(model: &SchemaModel, name: &str, required: bool) -> SchemaAttrReport {
    let single_value = model.attribute_type(name).map(|at| at.single_value).unwrap_or(false);
    SchemaAttrReport {
        name: name.to_string(),
        required,
        kind: model.field_kind(name),
        single_value,
    }
}
```

Keep the `pub mod config; pub mod ldap; pub mod schema;` lines at the top of the file.

- [ ] **Step 2: Rewrite main.rs with subcommands**

Replace `src/main.rs`:
```rust
//! edaptor CLI. M1/M2: connectivity check and schema introspection.
//! (The TUI replaces these in M3.)

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use edaptor::config::Config;
use edaptor::SchemaReport;

#[derive(Parser)]
#[command(name = "edaptor", about = "TUI for editing OpenLDAP directories")]
struct Cli {
    /// Path to the configuration file
    /// (default: $XDG_CONFIG_HOME/edaptor/config.toml or ~/.config/edaptor/config.toml).
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Connect, bind, and print a schema summary (the default action).
    Check,
    /// Resolve and print the effective attributes of an object class.
    Schema {
        /// Object class name, e.g. inetOrgPerson
        object_class: String,
    },
}

fn default_config_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join("edaptor/config.toml");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config/edaptor/config.toml")
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config_path = cli.config.clone().unwrap_or_else(default_config_path);
    let config = Config::load(&config_path)?;
    let password = config
        .auth
        .password_source
        .resolve()
        .context("resolving bind password")?;

    match cli.command.unwrap_or(Command::Check) {
        Command::Check => {
            let summary = edaptor::run_check(config, password)?;
            println!("Connected to {}", summary.uri);
            if let Some(dn) = &summary.bind_dn {
                println!("Bound as {dn}");
            }
            println!(
                "Subschema: {} objectClasses, {} attributeTypes, {} ldapSyntaxes",
                summary.object_class_count,
                summary.attribute_type_count,
                summary.ldap_syntax_count
            );
        }
        Command::Schema { object_class } => {
            let report: SchemaReport = edaptor::run_schema(config, password, &object_class)?;
            print_schema(&report);
        }
    }
    Ok(())
}

fn print_schema(report: &SchemaReport) {
    println!(
        "Object class '{}' — {} effective attributes ({} schema parse warnings)",
        report.object_class,
        report.attributes.len(),
        report.parse_warnings
    );
    for a in &report.attributes {
        println!(
            "  {:<28} {:<4} {:?}{}",
            a.name,
            if a.required { "MUST" } else { "MAY" },
            a.kind,
            if a.single_value { " (single-valued)" } else { "" }
        );
    }
}
```

- [ ] **Step 3: Build, lint, fmt**

Run: `cargo build` → PASS. `cargo clippy --all-targets` → clean. `cargo fmt` then `cargo fmt --check`.
Run: `cargo test --lib` → PASS (config 3 + password 5 + tls 4 + schema 7 = 19).

- [ ] **Step 4: Manual end-to-end verification against the container**

```bash
scripts/test-ldap.sh start
cat > /tmp/edaptor-test.toml <<'EOF'
[server]
uri = "ldap://localhost:1389"
base_dn = "dc=example,dc=org"

[auth]
method = "simple"
bind_dn = "cn=admin,dc=example,dc=org"
password_source = "env:EDAPTOR_PW"
EOF
EDAPTOR_PW=adminpassword cargo run -- --config /tmp/edaptor-test.toml schema inetOrgPerson
scripts/test-ldap.sh stop
rm -f /tmp/edaptor-test.toml
```
Expected: a header line plus rows; `cn` and `sn` shown as `MUST` (inherited from `person`), `mail` and `givenName` as `MAY`, `member`-like DN attributes classified `DistinguishedName`, etc. Capture the actual output. If `inetOrgPerson` is absent in the bitnami image, try `person`. Do NOT fake output; report honestly. STOP the container even on failure.

- [ ] **Step 5: Commit**

```bash
git -c user.name='oetiker' -c user.email='oetiker@gmail.com' add src/lib.rs src/main.rs
git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -m "$(printf 'M2: run_schema + edaptor schema CLI subcommand\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 6: Integration test against live OpenLDAP

**Files:**
- Modify: `tests/integration.rs` (add a schema-resolution test)

- [ ] **Step 1: Add the test**

Append to `tests/integration.rs`:
```rust
#[test]
fn resolves_inetorgperson_schema() {
    let uri = match std::env::var("EDAPTOR_TEST_LDAP_URI") {
        Ok(uri) => uri,
        Err(_) => {
            eprintln!("SKIP resolves_inetorgperson_schema: set EDAPTOR_TEST_LDAP_URI to run");
            return;
        }
    };

    let (config, password) = test_config(uri);
    let report = edaptor::run_schema(config, password, "inetOrgPerson")
        .expect("run_schema should resolve inetOrgPerson");

    let names: Vec<&str> = report.attributes.iter().map(|a| a.name.as_str()).collect();
    // cn and sn are MUST (inherited from person); mail is MAY.
    assert!(names.iter().any(|n| n.eq_ignore_ascii_case("cn")), "attrs={names:?}");
    assert!(names.iter().any(|n| n.eq_ignore_ascii_case("sn")), "attrs={names:?}");
    assert!(report
        .attributes
        .iter()
        .any(|a| a.name.eq_ignore_ascii_case("sn") && a.required));
    assert!(names.iter().any(|n| n.eq_ignore_ascii_case("mail")), "attrs={names:?}");
}
```

(`test_config` already exists in `tests/integration.rs` from M1.)

- [ ] **Step 2: Run against the container**

```bash
scripts/test-ldap.sh start
export EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389
export EDAPTOR_TEST_ADMIN_PW=adminpassword
cargo test --test integration -- --nocapture
unset EDAPTOR_TEST_LDAP_URI EDAPTOR_TEST_ADMIN_PW
cargo test --test integration -- --nocapture   # confirm all SKIP
scripts/test-ldap.sh stop
```
Expected: 3 integration tests pass live (M1's 2 + this one), and all SKIP when the env var is unset. If `inetOrgPerson` is not in the image's schema, report it; the bitnami OpenLDAP includes the standard `cosine`/`inetorgperson` schemas, so it should be present.

- [ ] **Step 3: clippy, fmt, commit**

Run: `cargo clippy --all-targets` → clean. `cargo fmt`.
```bash
git -c user.name='oetiker' -c user.email='oetiker@gmail.com' add tests/integration.rs
git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -m "$(printf 'M2: integration test for inetOrgPerson schema resolution\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## M2 Definition of Done

- [ ] `cargo test --lib` passes (expect 19: 3 config + 5 password + 4 tls + 7 schema).
- [ ] `cargo test --test integration` passes live (3 tests) and SKIPs cleanly without the env var.
- [ ] `cargo run -- --config <file> schema inetOrgPerson` prints the resolved MUST/MAY attributes with field kinds; `cargo run -- --config <file>` (or `... check`) still prints the M1 summary.
- [ ] `cargo clippy --all-targets` clean; `cargo fmt --check` clean.
- [ ] All six task commits present.

## Notes / scope

- The schema model is **headless** — no network or UI. It consumes `RawSubschema` produced by the M1 worker. M3 will map `FieldKind` → concrete turbo-vision widgets.
- Inheritance is resolved by edaptor (the crate does not). SUP cycles are guarded (visited-set for classes; bounded loop for attributes).
- `ldap_syntaxes` from the subschema are fetched but not yet parsed into the model (M2 classifies via attribute SYNTAX OIDs directly); parsing the syntax descriptions themselves is deferred until a milestone needs human-readable syntax names.
