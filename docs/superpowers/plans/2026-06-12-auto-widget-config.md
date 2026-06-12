# Auto-configured Widget System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace hardcoded attribute-name checks (`is_secret_attr`, `is_x_ordered`, the `memberOf` guard, the `sambaSID` auto-tag) with a three-layer widget resolver backed by baked-in objectClass bundles and live schema introspection, so all editing smarts are reachable via the widget config system.

**Architecture:** A `WidgetResolver` struct holds references to the live `SchemaModel`, the compiled-in `BuiltinSchema`, and the profile's resolved widgets; its `resolve_kind(attr, entry_ocs)` method merges them (schema hint < baked-in bundle < explicit profile config). Baked-in defaults are stored in `src/config/builtin_schema.toml` (compiled via `include_str!`) using the same `WidgetSpecCfg` TOML format as user config. Two new widget kinds (`Readonly`, `XOrdered`) expose the previously-hardcoded behaviours to both the baked-in bundles and user config.

**Tech Stack:** Rust, `toml` crate (already used), `serde`, `ldap_types::schema::AttributeType` (already parsed into `SchemaModel`).

---

## File map

| Action | Path | Responsibility |
|---|---|---|
| Create | `src/config/builtin_schema.toml` | Baked-in objectClass→attr→widget mappings |
| Create | `src/config/builtin.rs` | Loads/parses the TOML; exposes `builtin_schema()` |
| Create | `src/config/resolver.rs` | `WidgetResolver` + three-layer `resolve_kind()` |
| Modify | `src/config/mod.rs` | Declare `builtin` + `resolver` modules; add `Readonly`, `XOrdered`, `SambaSid` to `WidgetSpecCfg` |
| Modify | `src/config/widget.rs` | Add `Readonly`, `XOrdered`, `SambaSid` to `WidgetKind`; handle in `resolve_widgets()` |
| Modify | `src/schema/model.rs` | Add `is_readonly_attr()` |
| Modify | `src/ui/edit_form.rs` | Use `WidgetResolver`; remove `memberOf` guard + `tag_samba_sid_field` hardcode |
| Modify | `src/form/changeset.rs` | Remove `is_secret_attr()` + `is_x_ordered()`; add `x_ordered_attrs` param to `diff()` |
| Modify | `src/workflows/save.rs` | Remove `is_secret_attr` import; derive sets from form fields |
| Modify | `docs/src/configuration/widgets.md` | Document new kinds |
| Modify | `CHANGES.md` | Changelog entry |

---

### Task 1: Extend WidgetKind and WidgetSpecCfg with Readonly, XOrdered, SambaSid

**Files:**
- Modify: `src/config/widget.rs`
- Modify: `src/config/mod.rs`

- [ ] **Step 1: Add three variants to `WidgetKind` in `src/config/widget.rs`**

Insert after the `NextNumber` variant (around line 57):

```rust
    /// The attribute is displayed but excluded from the changeset. Used for
    /// overlay-maintained back-references and NO-USER-MODIFICATION attributes.
    Readonly,
    /// OpenLDAP X-ORDERED multi-value attribute. The `{n}` ordering prefix is
    /// stripped for display and reconstructed on save.
    XOrdered,
```

`SambaSid` already exists in `WidgetKind` — no change needed there.

- [ ] **Step 2: Add the same variants to `WidgetSpecCfg` in `src/config/mod.rs`**

Insert after the `Membership` variant (around line 163). Keep the `#[serde(tag = "kind", rename_all = "lowercase")]` on the enum. Add:

```rust
    /// Display-only; the attribute is excluded from the changeset.
    Readonly,
    /// OpenLDAP X-ORDERED attribute: strips/regenerates `{n}` ordering prefixes.
    #[serde(rename = "x_ordered")]
    XOrdered,
    /// Generates the Samba SID from `uidNumber` + domain SID when Samba is
    /// configured. Has no effect when no Samba domain is available.
    #[serde(rename = "samba_sid")]
    SambaSid,
```

- [ ] **Step 3: Handle new variants in `resolve_widgets()` in `src/config/widget.rs`**

In the `match widget_spec` arm inside `resolve_widgets()`, add (before the closing brace):

```rust
WidgetSpecCfg::Readonly => resolved.push(ResolvedWidget {
    owner_object_classes: p.object_classes.clone(),
    attr: attr.clone(),
    kind: WidgetKind::Readonly,
}),
WidgetSpecCfg::XOrdered => resolved.push(ResolvedWidget {
    owner_object_classes: p.object_classes.clone(),
    attr: attr.clone(),
    kind: WidgetKind::XOrdered,
}),
WidgetSpecCfg::SambaSid => resolved.push(ResolvedWidget {
    owner_object_classes: p.object_classes.clone(),
    attr: attr.clone(),
    kind: WidgetKind::SambaSid,
}),
```

- [ ] **Step 4: Write the failing test**

Add inside the `#[cfg(test)]` module in `src/config/mod.rs` (or create one):

```rust
#[test]
fn deserialize_readonly_x_ordered_samba_sid() {
    let s = r#"
[a]
kind = "readonly"
[b]
kind = "x_ordered"
[c]
kind = "samba_sid"
"#;
    let m: std::collections::HashMap<String, WidgetSpecCfg> =
        toml::from_str(s).unwrap();
    assert!(matches!(m["a"], WidgetSpecCfg::Readonly));
    assert!(matches!(m["b"], WidgetSpecCfg::XOrdered));
    assert!(matches!(m["c"], WidgetSpecCfg::SambaSid));
}
```

- [ ] **Step 5: Run the test to confirm it fails**

```bash
cargo test -j4 deserialize_readonly_x_ordered_samba_sid 2>&1 | tail -5
```

Expected: compile error or test failure (variants don't exist yet if running before step 2).

- [ ] **Step 6: Run tests to confirm they pass**

```bash
cargo test -j4 deserialize_readonly_x_ordered_samba_sid 2>&1 | tail -5
```

Expected: `test ... ok`

- [ ] **Step 7: Commit**

```bash
git add src/config/mod.rs src/config/widget.rs
git commit -m "feat(config): add Readonly, XOrdered, SambaSid widget kinds"
```

---

### Task 2: Add `is_readonly_attr` to `SchemaModel`

**Files:**
- Modify: `src/schema/model.rs`

- [ ] **Step 1: Write the failing test**

Add inside the existing `#[cfg(test)]` module in `src/schema/model.rs`:

```rust
#[test]
fn is_readonly_attr_no_user_modification() {
    let raw = RawSubschema {
        object_classes: vec![],
        attribute_types: vec![
            "( 2.5.18.1 NAME 'createTimestamp' \
             SYNTAX 1.3.6.1.4.1.1466.115.121.1.24 \
             SINGLE-VALUE NO-USER-MODIFICATION USAGE directoryOperation )".into(),
        ],
        ldap_syntaxes: vec![],
    };
    let m = SchemaModel::from_raw(&raw);
    assert!(m.is_readonly_attr("createTimestamp"));
    assert!(!m.is_readonly_attr("cn"));   // unknown → false
}
```

- [ ] **Step 2: Run the test to confirm it fails**

```bash
cargo test -j4 is_readonly_attr_no_user_modification 2>&1 | tail -5
```

Expected: compile error (`method not found`).

- [ ] **Step 3: Implement `is_readonly_attr`**

Add this method to the `impl SchemaModel` block (after `is_single_value`, around line 195):

```rust
/// Returns `true` if the server marks this attribute type `NO-USER-MODIFICATION`
/// (e.g. operational attributes maintained by overlays). Unknown attrs → `false`.
pub fn is_readonly_attr(&self, attr_name: &str) -> bool {
    self.attribute_type(attr_name)
        .map(|at| at.no_user_modification)
        .unwrap_or(false)
}
```

- [ ] **Step 4: Run the test to confirm it passes**

```bash
cargo test -j4 is_readonly_attr_no_user_modification 2>&1 | tail -5
```

Expected: `test ... ok`

- [ ] **Step 5: Commit**

```bash
git add src/schema/model.rs
git commit -m "feat(schema): add is_readonly_attr for NO-USER-MODIFICATION detection"
```

---

### Task 3: Create the baked-in objectClass bundle (TOML + loader)

**Files:**
- Create: `src/config/builtin_schema.toml`
- Create: `src/config/builtin.rs`
- Modify: `src/config/mod.rs`

- [ ] **Step 1: Write `src/config/builtin_schema.toml`**

```toml
# posixAccount (RFC 2307)
[posixAccount.loginShell]
kind = "choice"
select = "single"
format = "plain"
options = [
  { value = "/bin/bash",     label = "Bash" },
  { value = "/bin/sh",       label = "POSIX sh" },
  { value = "/bin/zsh",      label = "Zsh" },
  { value = "/bin/false",    label = "Disabled (false)" },
  { value = "/sbin/nologin", label = "Disabled (nologin)" },
]

[posixAccount.gidNumber]
kind = "picker"
candidate = "_posix_group_"
store = "gidNumber"
select = "single"

[posixAccount.memberOf]
kind = "readonly"

# posixGroup (RFC 2307)
[posixGroup.memberUid]
kind = "picker"
candidate = "_posix_account_"
store = "uid"
select = "multi"

[posixGroup.memberOf]
kind = "readonly"

# shadowAccount (RFC 2307)
[shadowAccount.shadowPassword]
kind = "password"

# sambaSamAccount (Samba schema)
[sambaSamAccount.userPassword]
kind = "password"
samba = true

[sambaSamAccount.sambaNTPassword]
kind = "readonly"

[sambaSamAccount.sambaLMPassword]
kind = "readonly"

[sambaSamAccount.sambaSID]
kind = "samba_sid"

# groupOfNames (RFC 2256)
[groupOfNames.member]
kind = "picker"
candidate = "_any_"
store = "dn"
select = "multi"

[groupOfNames.memberOf]
kind = "readonly"

# groupOfUniqueNames (RFC 2256)
[groupOfUniqueNames.uniqueMember]
kind = "picker"
candidate = "_any_"
store = "dn"
select = "multi"

[groupOfUniqueNames.memberOf]
kind = "readonly"

# inetOrgPerson (RFC 2798)
# memberOf is caught by Layer 1 (NO-USER-MODIFICATION) on servers with the
# memberOf overlay. These entries are belt-and-suspenders for other cases.
[inetOrgPerson.memberOf]
kind = "readonly"

# OpenLDAP cn=config
[olcGlobal.olcAccess]
kind = "x_ordered"

[olcDatabaseConfig.olcDbIndex]
kind = "x_ordered"

[olcDatabaseConfig.olcSuffix]
kind = "x_ordered"

[olcDatabaseConfig.olcRootDN]
kind = "x_ordered"

[olcDatabaseConfig.olcLimits]
kind = "x_ordered"

[olcDatabaseConfig.olcSyncrepl]
kind = "x_ordered"
```

- [ ] **Step 2: Write `src/config/builtin.rs`**

```rust
//! Baked-in objectClass → attribute → widget-spec defaults, compiled into the
//! binary via `include_str!`. Loaded once at first access.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::config::WidgetSpecCfg;

/// objectClass name (lower-cased) → attribute name (lower-cased) → spec.
pub type BuiltinSchema = HashMap<String, HashMap<String, WidgetSpecCfg>>;

static BUILTIN: OnceLock<BuiltinSchema> = OnceLock::new();

/// Returns the singleton baked-in schema. Panics on a malformed bundled TOML
/// (a compile-time invariant, not a runtime error).
pub fn builtin_schema() -> &'static BuiltinSchema {
    BUILTIN.get_or_init(|| {
        let raw: HashMap<String, HashMap<String, WidgetSpecCfg>> =
            toml::from_str(include_str!("builtin_schema.toml"))
                .expect("builtin_schema.toml is always valid");
        // Lower-case all keys for case-insensitive lookup.
        raw.into_iter()
            .map(|(oc, attrs)| {
                (
                    oc.to_lowercase(),
                    attrs
                        .into_iter()
                        .map(|(a, w)| (a.to_lowercase(), w))
                        .collect(),
                )
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WidgetSpecCfg;

    #[test]
    fn loginshell_is_choice() {
        let bs = builtin_schema();
        assert!(matches!(
            bs["posixaccount"]["loginshell"],
            WidgetSpecCfg::Choice { .. }
        ));
    }

    #[test]
    fn userpassword_samba_is_password_with_samba() {
        let bs = builtin_schema();
        assert!(matches!(
            bs["sambasamaccount"]["userpassword"],
            WidgetSpecCfg::Password { samba: true }
        ));
    }

    #[test]
    fn memberof_is_readonly() {
        let bs = builtin_schema();
        assert!(matches!(
            bs["inetorgperson"]["memberof"],
            WidgetSpecCfg::Readonly
        ));
    }

    #[test]
    fn olcaccess_is_x_ordered() {
        let bs = builtin_schema();
        assert!(matches!(
            bs["olcglobal"]["olcaccess"],
            WidgetSpecCfg::XOrdered
        ));
    }
}
```

- [ ] **Step 3: Declare the new module in `src/config/mod.rs`**

Add after the existing `pub mod widget;` line:

```rust
pub mod builtin;
pub mod resolver;
```

(`resolver` is created in Task 4; adding the declaration here avoids a second mod.rs edit.)

- [ ] **Step 4: Run the tests to confirm they pass**

```bash
cargo test -j4 builtin:: 2>&1 | tail -10
```

Expected: all four `builtin::tests::*` tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/config/builtin_schema.toml src/config/builtin.rs src/config/mod.rs
git commit -m "feat(config): add baked-in objectClass widget bundle"
```

---

### Task 4: Implement `WidgetResolver`

**Files:**
- Create: `src/config/resolver.rs`

The resolver merges three layers for a single `(attr, entry_objectclasses)` query:
1. **Schema introspection** (weakest) — `NO-USER-MODIFICATION` → Readonly; Boolean syntax → Choice
2. **Baked-in bundles** — walk sorted objectClasses, last match wins
3. **Explicit profile config** (strongest) — the profile's resolved widgets

Sentinel candidates (`_posix_group_`, `_posix_account_`, `_any_`) are resolved against the list of configured profiles.

- [ ] **Step 1: Write `src/config/resolver.rs`**

`CandidateScope` fields (from `src/config/relation.rs`): `base`, `object_classes`,
`search_attrs`, `label_template`. Use the existing `scope_of(p)` helper to build
a scope from a profile. `PickerBinding` fields: `attr`, `scope`, `store: StoreKey`,
`select: Option<Cardinality>`, `fanout_attr`. `StoreKey::Dn` for DN storage;
`StoreKey::Attr(name)` for scalar attr storage.

```rust
//! Three-layer widget resolver: schema introspection < baked-in bundles <
//! explicit profile config. Constructed at form-build time; cheap to throw away.

use crate::config::builtin::builtin_schema;
use crate::config::relation::{scope_of, Cardinality, StoreKey, PickerBinding};
use crate::config::widget::{ChoiceWidget, ChoiceFormat, WidgetKind, PasswordWidget, widget_for};
use crate::config::{ChoiceOption, EntryProfile, WidgetSpecCfg};
use crate::schema::model::SchemaModel;
use crate::schema::syntax::FieldKind;

/// Resolves the effective widget kind for an attribute in an entry, merging the
/// three layers. Construct once per form-open; the resolver borrows its inputs.
pub struct WidgetResolver<'a> {
    schema: &'a SchemaModel,
    profiles: &'a [EntryProfile],
    profile_widgets: &'a [crate::config::widget::ResolvedWidget],
    samba_enabled: bool,
}

impl<'a> WidgetResolver<'a> {
    pub fn new(
        schema: &'a SchemaModel,
        profiles: &'a [EntryProfile],
        profile_widgets: &'a [crate::config::widget::ResolvedWidget],
        samba_enabled: bool,
    ) -> Self {
        WidgetResolver { schema, profiles, profile_widgets, samba_enabled }
    }

    /// Resolve the effective widget kind for `attr` given the entry's object classes.
    /// Returns `None` for plain-text fields.
    pub fn resolve_kind(&self, attr: &str, entry_ocs: &[String]) -> Option<WidgetKind> {
        // Layer 1: schema introspection hints (weakest).
        let mut result: Option<WidgetKind> = None;
        if self.schema.is_readonly_attr(attr) {
            result = Some(WidgetKind::Readonly);
        } else if self.schema.field_kind(attr) == FieldKind::Boolean {
            result = Some(WidgetKind::Choice(ChoiceWidget {
                select: Cardinality::Single,
                format: ChoiceFormat::Plain,
                options: vec![
                    ChoiceOption { value: "TRUE".into(),  label: "TRUE".into() },
                    ChoiceOption { value: "FALSE".into(), label: "FALSE".into() },
                ],
            }));
        }

        // Layer 2: baked-in objectClass bundles.
        // Walk alphabetically sorted objectClasses for determinism; last match wins.
        let bs = builtin_schema();
        let mut sorted_ocs: Vec<&str> = entry_ocs.iter().map(String::as_str).collect();
        sorted_ocs.sort_unstable();
        for oc in &sorted_ocs {
            if let Some(attr_map) = bs.get(&oc.to_lowercase()) {
                if let Some(spec) = attr_map.get(&attr.to_lowercase()) {
                    if let Some(kind) = self.spec_to_kind(spec, attr) {
                        result = Some(kind);
                    }
                }
            }
        }

        // Layer 3: explicit profile config (strongest).
        if let Some(kind) = widget_for(self.profile_widgets, entry_ocs, attr) {
            result = Some(kind.clone());
        }

        result
    }

    /// Convert a `WidgetSpecCfg` from the baked-in bundle into a live `WidgetKind`.
    /// Returns `None` when a sentinel candidate cannot be resolved (degrades to text).
    fn spec_to_kind(&self, spec: &WidgetSpecCfg, attr: &str) -> Option<WidgetKind> {
        match spec {
            WidgetSpecCfg::Readonly => Some(WidgetKind::Readonly),
            WidgetSpecCfg::XOrdered => Some(WidgetKind::XOrdered),
            WidgetSpecCfg::SambaSid => {
                if self.samba_enabled { Some(WidgetKind::SambaSid) } else { None }
            }
            WidgetSpecCfg::Password { samba } => {
                let derived = if *samba {
                    vec!["sambaNTPassword".to_string(), "sambaPwdLastSet".to_string()]
                } else {
                    Vec::new()
                };
                Some(WidgetKind::Password(PasswordWidget {
                    primary: attr.to_string(),
                    derived,
                    samba: *samba,
                }))
            }
            WidgetSpecCfg::Choice { select, format, options } => {
                let card = match select.as_str() {
                    "multi" => Cardinality::Multi,
                    _ => Cardinality::Single,
                };
                let fmt = match format.as_str() {
                    "bracketed" => ChoiceFormat::Bracketed,
                    _ => ChoiceFormat::Plain,
                };
                Some(WidgetKind::Choice(ChoiceWidget {
                    select: card,
                    format: fmt,
                    options: options.clone(),
                }))
            }
            WidgetSpecCfg::Picker { candidate, store, select } => {
                use crate::config::CandidateRef;
                let scope = self.resolve_candidate(candidate)?;
                let store_key = if store == "dn" {
                    StoreKey::Dn
                } else {
                    StoreKey::Attr(store.clone())
                };
                let card = match select.as_str() {
                    "multi" => Some(Cardinality::Multi),
                    "single" => Some(Cardinality::Single),
                    _ => None, // "auto" — let form derive from schema arity
                };
                Some(WidgetKind::Picker(PickerBinding {
                    attr: attr.to_string(),
                    scope,
                    store: store_key,
                    select: card,
                    fanout_attr: None,
                }))
            }
            WidgetSpecCfg::Membership { candidate, via } => {
                let scope = self.resolve_candidate(candidate)?;
                Some(WidgetKind::Picker(PickerBinding {
                    attr: attr.to_string(),
                    scope,
                    store: StoreKey::Dn,
                    select: Some(Cardinality::Multi),
                    fanout_attr: Some(via.clone()),
                }))
            }
        }
    }

    /// Resolve a `CandidateRef` — sentinel names (`_posix_group_`, `_posix_account_`,
    /// `_any_`) or a regular profile name — to a `CandidateScope`.
    /// Returns `None` when no matching profile exists (field degrades to plain text).
    fn resolve_candidate(
        &self,
        candidate: &crate::config::CandidateRef,
    ) -> Option<crate::config::relation::CandidateScope> {
        use crate::config::CandidateRef;
        match candidate {
            CandidateRef::Inline(inline) => {
                // Build a minimal scope from the inline table.  Parse label template
                // the same way scope_of() does.
                let label_template = inline.label.as_ref().map(|s| {
                    crate::config::label::parse_label_template(s)
                });
                Some(crate::config::relation::CandidateScope {
                    base: inline.base.clone(),
                    object_classes: inline.object_classes.clone(),
                    search_attrs: inline.search_attrs.clone(),
                    label_template,
                })
            }
            CandidateRef::Profile(name) => {
                let target_oc: Option<&str> = match name.as_str() {
                    "_posix_group_"   => Some("posixGroup"),
                    "_posix_account_" => Some("posixAccount"),
                    "_any_"           => None,
                    other => {
                        return self.profiles.iter()
                            .find(|p| p.name == other)
                            .map(scope_of);
                    }
                };
                match target_oc {
                    None => self.profiles.first().map(scope_of),
                    Some(oc) => self.profiles.iter()
                        .find(|p| p.object_classes.iter()
                            .any(|o| o.eq_ignore_ascii_case(oc)))
                        .map(scope_of),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::widget::ResolvedWidget;
    use crate::ldap::worker::RawSubschema;
    use crate::schema::model::SchemaModel;

    fn empty_schema() -> SchemaModel {
        SchemaModel::from_raw(&RawSubschema {
            object_classes: vec![],
            attribute_types: vec![],
            ldap_syntaxes: vec![],
        })
    }

    fn no_widgets() -> Vec<ResolvedWidget> { vec![] }
    fn no_profiles() -> Vec<EntryProfile> { vec![] }

    #[test]
    fn builtin_loginshell_is_choice_for_posixaccount() {
        let schema = empty_schema();
        let resolver = WidgetResolver::new(&schema, &no_profiles(), &no_widgets(), false);
        let kind = resolver.resolve_kind("loginShell", &["posixAccount".into()]);
        assert!(matches!(kind, Some(WidgetKind::Choice(_))), "got {kind:?}");
    }

    #[test]
    fn schema_no_user_modification_wins_over_nothing() {
        let raw = RawSubschema {
            object_classes: vec![],
            attribute_types: vec![
                "( 1.1 NAME 'opAttr' NO-USER-MODIFICATION USAGE directoryOperation )".into(),
            ],
            ldap_syntaxes: vec![],
        };
        let schema = SchemaModel::from_raw(&raw);
        let resolver = WidgetResolver::new(&schema, &no_profiles(), &no_widgets(), false);
        let kind = resolver.resolve_kind("opAttr", &["top".into()]);
        assert!(matches!(kind, Some(WidgetKind::Readonly)), "got {kind:?}");
    }

    #[test]
    fn explicit_profile_widget_overrides_builtin() {
        use crate::config::widget::{ChoiceWidget, ChoiceFormat};
        use crate::config::relation::Cardinality;
        let schema = empty_schema();
        // Explicit profile widget for loginShell with different options.
        let explicit = vec![ResolvedWidget {
            owner_object_classes: vec!["posixAccount".into()],
            attr: "loginShell".into(),
            kind: WidgetKind::Choice(ChoiceWidget {
                select: Cardinality::Single,
                format: ChoiceFormat::Plain,
                options: vec![crate::config::ChoiceOption {
                    value: "/bin/custom".into(),
                    label: "Custom shell".into(),
                }],
            }),
        }];
        let resolver = WidgetResolver::new(&schema, &no_profiles(), &explicit, false);
        let kind = resolver.resolve_kind("loginShell", &["posixAccount".into()]);
        if let Some(WidgetKind::Choice(w)) = kind {
            assert_eq!(w.options[0].value, "/bin/custom");
        } else {
            panic!("expected Choice, got {kind:?}");
        }
    }

    #[test]
    fn samba_sid_requires_samba_enabled() {
        let schema = empty_schema();
        let disabled = WidgetResolver::new(&schema, &no_profiles(), &no_widgets(), false);
        assert!(disabled.resolve_kind("sambaSID", &["sambaSamAccount".into()]).is_none());

        let enabled = WidgetResolver::new(&schema, &no_profiles(), &no_widgets(), true);
        assert!(matches!(
            enabled.resolve_kind("sambaSID", &["sambaSamAccount".into()]),
            Some(WidgetKind::SambaSid)
        ));
    }
}
```

- [ ] **Step 2: Check what `CandidateScope` and `PickerBinding` look like**

Run:
```bash
grep -n "pub struct CandidateScope\|pub struct PickerBinding\|pub fn " \
  src/config/relation.rs | head -20
```

Adjust the imports and field names in `resolver.rs` if they differ from what's shown above.

- [ ] **Step 3: Run the tests to confirm they pass**

```bash
cargo test -j4 resolver:: 2>&1 | tail -15
```

Expected: all four `resolver::tests::*` tests pass.

- [ ] **Step 4: Fix any compile errors and re-run**

```bash
cargo build 2>&1 | head -30
```

Common issues: missing `pub` on `ChoiceFormat`, wrong field names on `PickerBinding`. Check `src/config/relation.rs` and `src/config/widget.rs` for exact names and adjust.

- [ ] **Step 5: Commit**

```bash
git add src/config/resolver.rs src/config/mod.rs
git commit -m "feat(config): implement three-layer WidgetResolver"
```

---

### Task 5: Wire `WidgetResolver` into the form builder

**Files:**
- Modify: `src/ui/edit_form.rs`

The form builder currently:
1. Sets `secret` and `ordered` flags on each `EditField` using `is_secret_attr()` and `is_x_ordered()`.
2. Calls `tag_widget_fields()` to bind widgets.
3. Checks `memberOf` by name in `field_is_editable()`.
4. Calls `tag_samba_sid_field(form, enabled)` after building.

After this task, the `WidgetResolver` drives all of the above except `NextNumber` and `ObjectClassPicker` (those stay as-is).

- [ ] **Step 1: Add `WidgetResolver` parameter to `build_edit_form`**

Find the signature of `build_edit_form` (around line 420). It currently takes `(model: FormModel, schema: &SchemaModel, read_only: bool)` or similar. Add a `resolver: &WidgetResolver` parameter.

Check exact signature:
```bash
grep -n "pub fn build_edit_form\|fn build_edit_form" src/ui/edit_form.rs
```

Then update the signature to include:
```rust
resolver: &crate::config::resolver::WidgetResolver<'_>,
```

- [ ] **Step 2: Replace `is_secret_attr` and `is_x_ordered` calls in `build_edit_form`**

Find lines 449–450 and 517–518 (the `secret:` and `ordered:` assignments). Replace the `is_secret_attr`/`is_x_ordered` calls with resolver queries:

```rust
// Before:
secret: crate::form::changeset::is_secret_attr(attr),
ordered: crate::form::changeset::is_x_ordered(attr),

// After:
secret: matches!(resolver.resolve_kind(attr, object_classes), Some(WidgetKind::Password(_))),
ordered: matches!(resolver.resolve_kind(attr, object_classes), Some(WidgetKind::XOrdered)),
```

Where `object_classes` is the entry's OC list available at that call site. (If it isn't available at both call sites, pass it through from the caller.)

- [ ] **Step 3: Remove the `memberOf` hardcode from `field_is_editable`**

Find lines 692–695 (the `memberOf` guard in `field_is_editable`). The function currently returns `false` for `memberOf` by name. Replace with a widget-kind check:

```rust
// Before:
if field.label.eq_ignore_ascii_case("memberOf") {
    return false;
}

// After: removed. Readonly is now signalled via widget_binding.
```

Instead, add a `Readonly` check alongside the existing `BinaryNote`/`DisabledCheckBox` check in `field_is_editable`:

```rust
if matches!(field.widget_binding, Some(WidgetKind::Readonly)) {
    return false;
}
```

- [ ] **Step 4: Inject `Readonly` widget binding from resolver**

In `tag_widget_fields` (around line 552), after the existing widget-binding logic, add a pass that injects `Readonly` for fields whose resolver result is `Readonly` (and that don't already have a binding from explicit profile config):

```rust
// After all existing tag_widget_fields logic:
for f in &mut form.fields {
    if f.widget_binding.is_none() {
        if let Some(WidgetKind::Readonly) =
            resolver.resolve_kind(&f.label, object_classes)
        {
            f.widget_binding = Some(WidgetKind::Readonly);
            f.editable = false;
        }
    }
}
```

Pass `resolver` and `object_classes` into `tag_widget_fields` (update its signature).

- [ ] **Step 5: Remove `tag_samba_sid_field` direct call; resolver handles it**

Find where `tag_samba_sid_field(form, enabled)` is called (in the form-build flow). Remove the call. In Step 4's resolver-injection loop, `SambaSid` is already handled: when `resolver.resolve_kind("sambaSID", ocs)` returns `Some(SambaSid)`, it will be injected via the same loop. Add the `SambaSid` injection:

```rust
if f.widget_binding.is_none() {
    match resolver.resolve_kind(&f.label, object_classes) {
        Some(WidgetKind::Readonly) => {
            f.widget_binding = Some(WidgetKind::Readonly);
            f.editable = false;
        }
        Some(WidgetKind::SambaSid) => {
            f.widget_binding = Some(WidgetKind::SambaSid);
        }
        Some(WidgetKind::XOrdered) => {
            f.ordered = true;
        }
        _ => {}
    }
}
```

- [ ] **Step 6: Update all callers of `build_edit_form` / `tag_widget_fields`**

Run:
```bash
grep -rn "build_edit_form\|tag_widget_fields\|tag_samba_sid_field" src/ | grep -v "\.rs:#\|test"
```

For each call site, construct a `WidgetResolver` from the available schema, profiles, resolved_widgets, and samba_enabled flag, and pass it in.

- [ ] **Step 7: Run the full test suite**

```bash
cargo test -j4 2>&1 | tail -20
```

Fix any failures. The existing tests in `edit_form.rs` that assert `userPassword.secret == true` and `cn.secret == false` should still pass (now driven by resolver).

- [ ] **Step 8: Commit**

```bash
git add src/ui/edit_form.rs
git commit -m "feat(ui): drive secret/ordered/readonly from WidgetResolver"
```

---

### Task 6: Remove `is_secret_attr` and `is_x_ordered` from changeset; update `diff`

**Files:**
- Modify: `src/form/changeset.rs`
- Modify: `src/workflows/save.rs`

`diff()` currently calls `is_x_ordered()` internally. After this task it receives an explicit set from the caller. `is_secret_attr()` is used in `save.rs` for preview masking; that will be derived from form field flags instead.

- [ ] **Step 1: Write a failing test for the new `diff` signature**

Add inside the `#[cfg(test)]` module in `src/form/changeset.rs`:

```rust
#[test]
fn diff_x_ordered_replace_when_in_set() {
    use std::collections::HashSet;
    let mut orig = EditEntry::default();
    orig.dn = "cn=config".into();
    orig.attrs.insert(
        "olcAccess".into(),
        vec!["{0}to * by * read".into(), "{1}to dn.base='' by * read".into()],
    );
    let mut edited = orig.clone();
    edited.attrs.insert(
        "olcAccess".into(),
        vec!["{0}to dn.base='' by * read".into(), "{1}to * by * read".into()],
    );
    let x_ordered: HashSet<String> = ["olcAccess".into()].into();
    let cs = diff(&orig, &edited, &x_ordered).unwrap();
    // A reorder must produce a Replace (not Add+Delete).
    assert_eq!(cs.mods.len(), 1);
    assert!(matches!(&cs.mods[0], crate::form::changeset::ModOp::Replace { attr, .. } if attr == "olcAccess"));
}
```

- [ ] **Step 2: Run the test to confirm it fails**

```bash
cargo test -j4 diff_x_ordered_replace_when_in_set 2>&1 | tail -5
```

Expected: compile error (`diff` doesn't accept a third argument yet).

- [ ] **Step 3: Update `diff()` signature in `src/form/changeset.rs`**

Change the function signature from:
```rust
pub fn diff(original: &EditEntry, edited: &EditEntry) -> Result<ChangeSet, ChangeSetError>
```
to:
```rust
pub fn diff(
    original: &EditEntry,
    edited: &EditEntry,
    x_ordered_attrs: &std::collections::HashSet<String>,
) -> Result<ChangeSet, ChangeSetError>
```

Replace the internal `is_x_ordered(attr)` call (around line 243) with:
```rust
if x_ordered_attrs.iter().any(|a| a.eq_ignore_ascii_case(attr)) && !orig.is_empty() && !new.is_empty() {
```

- [ ] **Step 4: Remove `is_x_ordered()` and `is_secret_attr()` from changeset.rs**

Delete both functions (lines 33–53). They are replaced by the resolver and form-field flags.

- [ ] **Step 5: Update `save.rs` to stop importing `is_secret_attr`**

In `src/workflows/save.rs` line 5, remove `is_secret_attr` from the import:
```rust
// Before:
use crate::form::changeset::{diff, is_secret_attr, ChangeSet, EditEntry, ModOp};
// After:
use crate::form::changeset::{diff, ChangeSet, EditEntry, ModOp};
```

Derive the secret-attr set and x-ordered-attr set from the edit form's fields.

Find the call site of `diff()` in `save.rs`. Before the call, add:

```rust
// Collect x-ordered and secret attribute names from the form's field flags.
let x_ordered_attrs: std::collections::HashSet<String> = form
    .fields
    .iter()
    .filter(|f| f.ordered)
    .map(|f| f.label.clone())
    .collect();
```

Pass `&x_ordered_attrs` as the third argument to `diff()`.

For preview masking, replace `is_secret_attr(attr)` with:
```rust
form.fields.iter().any(|f| f.label.eq_ignore_ascii_case(attr) && f.secret)
    || mask_attrs.iter().any(|a| a.eq_ignore_ascii_case(attr))
```

(If `form` isn't available at the masking call site, thread it through or collect a `secret_attrs: HashSet<String>` the same way as `x_ordered_attrs`.)

- [ ] **Step 6: Update any remaining `diff()` call sites**

```bash
grep -rn "changeset::diff\|form::changeset::diff\b\|diff(" src/ | grep -v test | grep -v "\.rs:#"
```

Add `&x_ordered_attrs` (or `&Default::default()` for call sites that don't have a form — like tests) to every `diff()` call.

- [ ] **Step 7: Fix `is_x_ordered` / `is_secret_attr` remaining usages**

```bash
grep -rn "is_x_ordered\|is_secret_attr" src/
```

Should return zero results. Fix any stragglers.

- [ ] **Step 8: Run the full test suite**

```bash
cargo test -j4 2>&1 | tail -20
```

Expected: all tests pass.

- [ ] **Step 9: Commit**

```bash
git add src/form/changeset.rs src/workflows/save.rs
git commit -m "refactor(changeset): remove is_secret_attr/is_x_ordered hardcodes"
```

---

### Task 7: Full check and clippy clean

- [ ] **Step 1: Run `make check`**

```bash
make check 2>&1 | tail -30
```

Expected: no warnings, no test failures. Fix any `clippy` warnings before proceeding.

- [ ] **Step 2: Smoke-test with the demo server (optional but recommended)**

```bash
scripts/test-ldap.sh start
export EDAPTOR_TEST_ADMIN_PW=adminpassword
cargo run -- --config examples/demo-config.toml
```

Open a posixAccount entry: confirm `loginShell` has a choice widget, `gidNumber` has a picker. Open a user with `sambaSamAccount`: confirm `userPassword` triggers the password popup (samba=true), `sambaNTPassword` is read-only.

- [ ] **Step 3: Commit if any fixes were needed**

```bash
git add -p
git commit -m "fix: clippy + integration issues after auto-widget-config"
```

---

### Task 8: Update documentation and changelog

**Files:**
- Modify: `docs/src/configuration/widgets.md`
- Modify: `CHANGES.md`

- [ ] **Step 1: Add `readonly` and `x_ordered` sections to `docs/src/configuration/widgets.md`**

Add two new sections after the existing widget-kind descriptions:

````markdown
### `readonly`

Marks the attribute as display-only. It is rendered in the form but excluded from
the save changeset — the user cannot edit it. Use this for overlay-maintained
back-references or any attribute your schema generates automatically.

```toml
[profile.widget.myOverlayAttr]
kind = "readonly"
```

Built-in assignments: `memberOf` (all standard object classes), `sambaNTPassword`,
`sambaLMPassword`. Additionally, any attribute the server marks
`NO-USER-MODIFICATION` in the subschema is treated as readonly automatically.

### `x_ordered`

For OpenLDAP **X-ORDERED** multi-value attributes (e.g. `olcAccess`,
`olcDbIndex`). The `{n}` ordering prefix is stripped for display and
reconstructed on save. Changing the set of values or their order produces a
single `REPLACE` operation.

```toml
[profile.widget.myOrderedAttr]
kind = "x_ordered"
```

Built-in assignments: `olcAccess`, `olcDbIndex`, `olcSuffix`, `olcRootDN`,
`olcLimits`, `olcSyncrepl` (all under the `olcGlobal` / `olcDatabaseConfig`
object classes).
````

- [ ] **Step 2: Add changelog entry to `CHANGES.md`**

Under the current unreleased section:

```markdown
### Changed

- Widget configuration is now auto-applied for standard LDAP schemas
  (posixAccount, posixGroup, shadowAccount, sambaSamAccount, groupOfNames,
  groupOfUniqueNames, inetOrgPerson, OpenLDAP cn=config). A typical deployment
  no longer needs `[profile.widget]` entries for these well-known attributes.
- Attributes flagged `NO-USER-MODIFICATION` in the server's subschema are
  automatically rendered read-only, even without explicit widget config.
- New widget kind `readonly`: marks an attribute display-only (excluded from
  the changeset). Available in user config for custom schemas.
- New widget kind `x_ordered`: handles OpenLDAP X-ORDERED attributes
  (`{n}` prefix management). Available in user config for custom schemas.
- `memberOf`, `sambaNTPassword`, `sambaLMPassword` are now read-only by
  default via the built-in schema bundle (previously hardcoded).
```

- [ ] **Step 3: Commit**

```bash
git add docs/src/configuration/widgets.md CHANGES.md
git commit -m "docs: document readonly/x_ordered widget kinds and auto-config"
```
