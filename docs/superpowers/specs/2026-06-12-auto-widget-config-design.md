# Auto-configured widget system

**Date:** 2026-06-12
**Status:** Approved

## Goal

eDAPtor currently requires explicit `[profile.widget.<attr>]` entries for every attribute that
needs special editing behaviour, and has several hardcoded attribute-name checks scattered
through the source (`is_secret_attr`, `is_x_ordered`, the `memberOf` non-editable guard, the
`sambaSID` auto-tagger). The goal is to:

1. Move all hardcoded attribute logic into the widget system.
2. Ship baked-in widget configurations for all standard OpenLDAP schemas so that a typical
   deployment needs zero widget config for the attributes it covers.
3. Add runtime schema introspection as a lowest-priority hint layer so that even unknown
   attributes on custom schemas get reasonable behaviour automatically.
4. Keep full user override capability — explicit `[profile.widget]` config still wins over
   everything.

## New widget kinds

### `readonly`

Attribute is displayed but excluded from the changeset. Used for:
- Overlay-maintained back-references (`memberOf`)
- Server-computed operational attributes (`NO-USER-MODIFICATION` in subschema)

Available in user config:
```toml
[profile.widget.myOverlayAttr]
kind = "readonly"
```

### `x_ordered`

Multi-value attribute with OpenLDAP `{n}` ordering prefixes (e.g. `olcAccess`,
`olcDbIndex`). Behaviour:
- Strips `{n}` prefix for display and editing
- Reconstructs ordered `{0}value`, `{1}value`, … prefixes on save
- Reorder UI (drag / move-up / move-down) deferred to a later iteration

Available in user config:
```toml
[profile.widget.myOrderedAttr]
kind = "x_ordered"
```

## Three-layer resolution

Widget resolution runs at **form-build time** (per entry opened), not at config-load time.
A `WidgetResolver` struct holds all three layers and exposes a single method:

```
resolve(attr: &str, entry_objectclasses: &[String]) -> EffectiveWidget
```

Priority (weakest → strongest):

### Layer 1 — Live schema introspection

Fetched from `cn=subschema` at connection time (subschema fetch already exists; this extends
it). Per-attribute hints extracted:

| Subschema field | Hint applied |
|---|---|
| `NO-USER-MODIFICATION` | `readonly` |
| Syntax OID `1.3.6.1.4.1.1466.115.121.1.7` (Boolean) | `choice` with options `["TRUE", "FALSE"]` |
| `SINGLE-VALUE` | suppress "add another value" in form UI |

DN-syntax attributes are **not** auto-converted to pickers — a picker needs a candidate source
that cannot be inferred from syntax alone.

### Layer 2 — Baked-in objectClass bundles

A TOML file `src/config/builtin_schema.toml` compiled into the binary via `include_str!`.
Uses the same `WidgetSpecCfg` format as user config. Keyed by objectClass name.

At form-build time all of the entry's objectClasses are walked in **alphabetically sorted
order** (sorted for determinism; LDAP objectClass list ordering has no semantic meaning);
later matches override earlier ones. Explicit profile config always wins over all bundles.
Conflicts between bundles are rare in practice because attribute names are typically unique
per schema.

Planned bundles:

```toml
[posixAccount]
loginShell = { kind = "choice", options = ["/bin/bash","/bin/sh","/bin/zsh","/bin/false","/sbin/nologin"] }
gidNumber  = { kind = "picker", candidate = "_posix_group_", store = "gidNumber" }

[posixGroup]
memberUid  = { kind = "picker", candidate = "_posix_account_", store = "uid" }

[shadowAccount]
shadowPassword = { kind = "password" }

[sambaSamAccount]
userPassword    = { kind = "password", samba = true }
sambaNTPassword = { kind = "readonly" }
sambaLMPassword = { kind = "readonly" }
sambaSID        = { kind = "samba_sid" }

[groupOfNames]
member       = { kind = "picker", candidate = "_any_", store = "dn" }

[groupOfUniqueNames]
uniqueMember = { kind = "picker", candidate = "_any_", store = "dn" }

# memberOf is primarily caught by Layer 1 (NO-USER-MODIFICATION) on servers with the
# memberOf overlay. These entries are belt-and-suspenders for non-overlay deployments.
[inetOrgPerson]
memberOf = { kind = "readonly" }

[posixAccount]
memberOf = { kind = "readonly" }

[olcGlobal]
olcAccess = { kind = "x_ordered" }

[olcDatabaseConfig]
olcDbIndex  = { kind = "x_ordered" }
olcSuffix   = { kind = "x_ordered" }
olcSyncrepl = { kind = "x_ordered" }
olcLimits   = { kind = "x_ordered" }
olcRootDN   = { kind = "x_ordered" }
```

**Sentinel candidates** — resolved at runtime against configured profiles:

| Sentinel | Resolves to |
|---|---|
| `_posix_group_` | first profile whose `object_classes` includes `posixGroup` |
| `_posix_account_` | first profile whose `object_classes` includes `posixAccount` |
| `_any_` | first profile regardless of objectClass |

If no matching profile is found the picker degrades to a plain text field rather than
failing.

### Layer 3 — Explicit user profile config (unchanged)

`[profile.widget.<attr>]` in the user's config file. Semantics unchanged; wins over
everything else.

## Data structures

### `BuiltinSchema` (`src/config/builtin.rs`)

Parsed once at startup from the compiled-in TOML (`OnceLock`):

```rust
pub struct BuiltinSchema(HashMap<String, HashMap<String, WidgetSpecCfg>>);

impl BuiltinSchema {
    pub fn get(&self, object_class: &str, attr: &str) -> Option<&WidgetSpecCfg>;
}
```

### `SchemaHints` (`src/ldap/schema_hints.rs`)

Built from the subschema fetch result:

```rust
pub struct AttributeHint {
    pub readonly: bool,
    pub single_value: bool,
    pub syntax_hint: Option<SyntaxHint>,
}

pub enum SyntaxHint { Boolean }

pub struct SchemaHints(HashMap<String, AttributeHint>);

impl SchemaHints {
    pub fn get(&self, attr: &str) -> Option<&AttributeHint>;
}
```

### `WidgetResolver`

Constructed at form-build time, holds references to all three layers, exposes `resolve()`.

## Code changes

### New files

| File | Purpose |
|---|---|
| `src/config/builtin_schema.toml` | Baked-in objectClass → attribute → widget mappings |
| `src/config/builtin.rs` | Parses builtin_schema.toml, exposes `BuiltinSchema` |
| `src/ldap/schema_hints.rs` | Extracts `AttributeHint` from live subschema data |

### Modified files

| File | Change |
|---|---|
| `src/config/widget.rs` | Add `Readonly` and `XOrdered` variants to `WidgetKind` / `WidgetSpecCfg` |
| `src/config/mod.rs` | Integrate `BuiltinSchema`; construct and expose `WidgetResolver` |
| `src/ldap/worker.rs` | Extend subschema fetch to populate `SchemaHints` |
| `src/ui/edit_form.rs` | Use `WidgetResolver`; remove `memberOf` guard and `sambaSID` auto-tag |
| `src/form/changeset.rs` | Remove `is_secret_attr()` and `is_x_ordered()`; use widget kind for exclusion and prefix handling |

## Hardcodes removed

| Location | Removed | Replaced by |
|---|---|---|
| `changeset.rs::is_secret_attr()` | hardcoded password attr list | baked-in `password` widgets on `userPassword`, `sambaNTPassword`, `sambaLMPassword`, `shadowPassword` |
| `edit_form.rs` memberOf guard | `memberOf` non-editable check | baked-in `readonly` widget under `inetOrgPerson` |
| `changeset.rs::is_x_ordered()` | hardcoded cn=config attr list | baked-in `x_ordered` widgets under `olcGlobal`, `olcDatabaseConfig` |
| `edit_form.rs` sambaSID tag | `sambaSID` auto-tagger | baked-in `samba_sid` widget under `sambaSamAccount` |

## What schema authors gain

Custom schema maintainers can now use `readonly` and `x_ordered` in their profile widget
config, giving them the same editing behaviour as the baked-in standard schemas.

## Out of scope

- Reorder UI for `x_ordered` attributes (move-up / move-down / drag) — future iteration
- Auto-conversion of DN-syntax attributes to pickers (needs candidate source)
- `jpegPhoto` binary display — separate feature
