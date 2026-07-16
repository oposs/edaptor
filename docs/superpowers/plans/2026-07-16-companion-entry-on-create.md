# Companion Entry on Create Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a profile declare one companion entry (e.g. a `posixGroup` mirroring a
user) that is created alongside the primary — atomically via LDAP transactions
(RFC 5805) when the server supports them, with a sequential companion-first fallback.

**Architecture:** A declarative `[profile.companion]` is parsed and validated at config
load. A pure `plan_companion` composes the companion `Add` from the primary's final
attributes using the existing `{attr}` template engine. The worker gains an `AddAtomic`
request (`StartTxn` → both Adds under the txn control → `EndTxn`) and a root-DSE
capability read; `UiState.server_supports_txn` records support. `do_create` plans both
entries, previews both LDIF stanzas in one confirm, and dispatches to the atomic or the
sequential write path.

**Tech Stack:** Rust, ldap3 0.12 (its `StartTxn`/`EndTxn` exops + `TxnSpec` control),
tvision-rs 0.12, serde/toml, anyhow. Tests are `#[test]` units run via `cargo test -j4`.

## Global Constraints

- **Cap build/test parallelism at 4 cores** (shared box): `cargo test -j4`,
  `cargo clippy --all-targets -- -D warnings`. Gate = `make check` (fmt + clippy
  `-D warnings` + tests), green after every task. **Run `cargo fmt` before every
  commit** and confirm `cargo fmt --check` is clean — plan code blocks are not always
  rustfmt-shaped.
- **Facade boundary:** only `src/ui/**` may `use tvision_rs`. New pure code in
  `src/config/**` and `src/workflows/create.rs` must not reference tvision. `ldap3` is
  confined to `src/ldap/**` (the worker) — it must not leak into `workflows`/`ui`.
- **English** identifiers/comments/doc-comments.
- **RFC 5805 OIDs:** StartTransaction `1.3.6.1.1.21.1`, EndTransaction `1.3.6.1.1.21.3`.
- **ldap3 API (confirmed):** `ldap3::exop::{StartTxn, StartTxnResp, EndTxn}`,
  `ldap3::controls::{TxnSpec, RawControl}` (`RawControl: From<TxnSpec>`),
  `ldap3::result::ExopResult(pub Exop, pub LdapResult)`; `conn.extended(exop) ->
  Result<ExopResult>`; `conn.with_controls(vec![RawControl]).add(dn, entry)`.
- **Docs one-home:** config detail → mdBook (`docs/src/`); `CHANGES.md` for user-visible
  changes; README orientation-only.
- **Commit trailer:** `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

---

## File Structure

- `src/config/mod.rs` — **modify.** Add `CompanionSpec`, `EntryProfile.companion`, and a
  `validate_companions` load-time check + tests.
- `src/config/defaults.rs` — **modify.** Make `resolve_template` `pub` (reused by
  `plan_companion`).
- `src/workflows/create.rs` — **modify.** Add `CompanionAdd` + `plan_companion` + tests.
- `src/ldap/worker.rs` — **modify.** Add `Request::FetchRootDse`/`Response::RootDse` +
  `fetch_root_dse`, the pure `txn_supported`, and `Request::AddAtomic` + `run_add_atomic`.
- `src/ui/state.rs` — **modify.** Add `server_supports_txn` (both ctors + bootstrap read),
  and the `WriteOutcome::NeedFollowupCreate` arm in `apply_write_outcome`.
- `src/workflows/write_flow.rs` — **modify.** `submit_create_atomic`, the sequential
  `CompanionThenPrimary` intent + `NeedFollowupCreate` outcome + `submit_create_with_companion`
  + `submit_followup_create` + `on_response` arms + tests.
- `src/ui/app.rs` — **modify.** `do_create`: plan companion, two-stanza preview, dispatch.
- `docs/src/configuration/companion.md`, `docs/src/SUMMARY.md`, `examples/config.toml`,
  `CHANGES.md` — **modify/create.** Docs.

---

## Task 1: Config — `CompanionSpec` + validation

**Files:**
- Modify: `src/config/mod.rs` (add struct near `EntryProfile` ~line 190; field on
  `EntryProfile` after `label`; a `validate_companions` fn; call it in `Config::load`
  before its final `Ok(config)`)
- Test: `src/config/mod.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Produces: `pub struct CompanionSpec { pub object_classes: Vec<String>, pub rdn_attr:
  String, pub search_base: String, pub attributes: BTreeMap<String, String> }` and
  `EntryProfile.companion: Option<CompanionSpec>`.

- [ ] **Step 1: Write failing tests**

Add to the `tests` module in `src/config/mod.rs`. Use the existing `load_str`/parse
helper the other tests use; if the module parses via a helper like `Config::load` from a
temp file, mirror the nearest existing config-parse test (e.g. `parses_profiles`) for the
harness. Tests:

```rust
#[test]
fn parses_companion_block() {
    let toml = r#"
        [server]
        uri = "ldap://x"
        base_dn = "dc=x"
        [[profile]]
        name = "Users"
        object_classes = ["inetOrgPerson","posixAccount"]
        rdn_attr = "uid"
        search_base = "ou=people,dc=x"
        [profile.companion]
        object_classes = ["posixGroup"]
        rdn_attr = "cn"
        search_base = "ou=groups,dc=x"
        [profile.companion.attributes]
        cn = "{uid}"
        gidNumber = "{gidNumber}"
    "#;
    let cfg = parse_config_str(toml).expect("parses");
    let c = cfg.profiles[0].companion.as_ref().expect("companion present");
    assert_eq!(c.object_classes, vec!["posixGroup"]);
    assert_eq!(c.rdn_attr, "cn");
    assert_eq!(c.search_base, "ou=groups,dc=x");
    assert_eq!(c.attributes.get("cn"), Some(&"{uid}".to_string()));
}

#[test]
fn companion_without_rdn_attr_in_attributes_is_rejected() {
    let toml = r#"
        [server]
        uri = "ldap://x"
        base_dn = "dc=x"
        [[profile]]
        name = "Users"
        object_classes = ["inetOrgPerson"]
        rdn_attr = "uid"
        search_base = "ou=people,dc=x"
        [profile.companion]
        object_classes = ["posixGroup"]
        rdn_attr = "cn"
        search_base = "ou=groups,dc=x"
        [profile.companion.attributes]
        gidNumber = "{gidNumber}"
    "#;
    let err = parse_config_str(toml).unwrap_err().to_string();
    assert!(err.contains("cn"), "error should name the missing rdn attribute: {err}");
}

#[test]
fn companion_with_empty_object_classes_is_rejected() {
    let toml = r#"
        [server]
        uri = "ldap://x"
        base_dn = "dc=x"
        [[profile]]
        name = "Users"
        object_classes = ["inetOrgPerson"]
        rdn_attr = "uid"
        search_base = "ou=people,dc=x"
        [profile.companion]
        object_classes = []
        rdn_attr = "cn"
        search_base = "ou=groups,dc=x"
        [profile.companion.attributes]
        cn = "{uid}"
    "#;
    assert!(parse_config_str(toml).is_err());
}

#[test]
fn companion_autonumber_attribute_is_rejected() {
    let toml = r#"
        [server]
        uri = "ldap://x"
        base_dn = "dc=x"
        [[profile]]
        name = "Users"
        object_classes = ["inetOrgPerson"]
        rdn_attr = "uid"
        search_base = "ou=people,dc=x"
        [profile.companion]
        object_classes = ["posixGroup"]
        rdn_attr = "cn"
        search_base = "ou=groups,dc=x"
        [profile.companion.attributes]
        cn = "{uid}"
        gidNumber = "{next:10000-20000}"
    "#;
    let err = parse_config_str(toml).unwrap_err().to_string();
    assert!(err.to_lowercase().contains("next") || err.contains("gidNumber"),
        "autonumber in a companion must be rejected: {err}");
}
```

If no `parse_config_str(&str) -> Result<Config>` test helper exists, add this thin one to
the `tests` module (write to a temp file and call `Config::load`, matching how the other
parse tests obtain a `Config`):

```rust
fn parse_config_str(toml: &str) -> anyhow::Result<Config> {
    let dir = std::env::temp_dir();
    let path = dir.join(format!("edaptor-cfg-test-{}.toml", toml.len()));
    std::fs::write(&path, toml)?;
    let cfg = Config::load(&path);
    let _ = std::fs::remove_file(&path);
    cfg
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -j4 -p edaptor companion 2>&1 | tail -20`
Expected: FAIL to compile — `companion` field / `CompanionSpec` unresolved.

- [ ] **Step 3: Add `CompanionSpec` and the field**

In `src/config/mod.rs`, add (near `EntryProfile`, ~line 190):

```rust
/// A declarative companion entry created alongside the primary on `New`
/// (e.g. a `posixGroup` mirroring a POSIX user). `attributes` values use the same
/// literal / `{attr}` template syntax as `[profile.defaults]`, resolved against the
/// primary's final attributes; `{next:…}` autonumbers are not allowed here (rejected
/// at load). `objectClass` is fixed by `object_classes`, not an `attributes` key.
#[derive(Debug, Deserialize, Default, Clone)]
pub struct CompanionSpec {
    pub object_classes: Vec<String>,
    #[serde(default)]
    pub rdn_attr: String,
    #[serde(default)]
    pub search_base: String,
    #[serde(default)]
    pub attributes: std::collections::BTreeMap<String, String>,
}
```

Add to `EntryProfile` (immediately after the `label` field, ~line 216):

```rust
    /// Optional companion entry created atomically with the primary (`[profile.companion]`).
    #[serde(default)]
    pub companion: Option<CompanionSpec>,
```

- [ ] **Step 4: Add the load-time validation**

Add this free function to `src/config/mod.rs` (near `Config::load`):

```rust
/// Validate every profile's optional `[profile.companion]`. Each declared companion
/// must have non-empty `object_classes`, `rdn_attr`, and `search_base`; `rdn_attr`
/// must appear as an `attributes` key (so the RDN has a value source); and every
/// attribute value must parse as a literal / `{attr}` template — a `{next:…}`
/// autonumber is rejected (companions carry no independent allocation).
fn validate_companions(profiles: &[EntryProfile]) -> Result<()> {
    use crate::config::defaults::{parse_default_value, DefaultValue};
    for p in profiles {
        let Some(c) = &p.companion else { continue };
        let who = format!("profile '{}' companion", p.name);
        if c.object_classes.is_empty() {
            anyhow::bail!("{who}: object_classes must not be empty");
        }
        if c.rdn_attr.trim().is_empty() {
            anyhow::bail!("{who}: rdn_attr must not be empty");
        }
        if c.search_base.trim().is_empty() {
            anyhow::bail!("{who}: search_base must not be empty");
        }
        if !c.attributes.keys().any(|k| k.eq_ignore_ascii_case(&c.rdn_attr)) {
            anyhow::bail!(
                "{who}: rdn_attr '{}' must be one of the companion attributes",
                c.rdn_attr
            );
        }
        for (attr, tmpl) in &c.attributes {
            match parse_default_value(tmpl).map_err(|e| anyhow::anyhow!("{who} attribute '{attr}': {e}"))? {
                DefaultValue::AutoNumber { .. } => {
                    anyhow::bail!("{who} attribute '{attr}': {{next:…}} autonumber is not supported for companions");
                }
                _ => {}
            }
        }
    }
    Ok(())
}
```

In `Config::load`, immediately before it returns `Ok(config)` (the final expression),
insert:

```rust
    validate_companions(&config.profiles)?;
```

- [ ] **Step 5: Run to verify they pass**

Run: `cargo test -j4 -p edaptor companion 2>&1 | tail -20`
Expected: PASS — 4 tests.

- [ ] **Step 6: Format, gate, commit**

```bash
cargo fmt && cargo fmt --check && cargo clippy --all-targets -- -D warnings 2>&1 | tail -3
cargo test -j4 2>&1 | tail -5
git add src/config/mod.rs
git commit -m "$(printf 'feat(config): [profile.companion] spec + load validation\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 2: `plan_companion` pure planner

**Files:**
- Modify: `src/config/defaults.rs` (make `resolve_template` `pub`)
- Modify: `src/workflows/create.rs` (add `CompanionAdd` + `plan_companion` + tests)

**Interfaces:**
- Consumes: `CompanionSpec` (Task 1); `config::defaults::{parse_default_value,
  resolve_template, DefaultValue}`.
- Produces: `pub struct CompanionAdd { pub dn: String, pub attrs: BTreeMap<String,
  Vec<String>>, pub ldif: String }` and `pub fn plan_companion(spec: &CompanionSpec,
  primary_attrs: &BTreeMap<String, Vec<String>>, schema: &SchemaModel) -> Result<CompanionAdd, String>`.

- [ ] **Step 1: Make `resolve_template` public**

In `src/config/defaults.rs`, change `fn resolve_template(` (line ~146) to
`pub fn resolve_template(`. Update its doc-comment's first line to note it is reused by
companion planning:

```rust
/// Resolve a template against a values map; `None` if any `{field}` is empty. Pure.
/// Reused by `plan_defaults` (create-form defaults) and `create::plan_companion`.
pub fn resolve_template(segs: &[Seg], current: &BTreeMap<String, Vec<String>>) -> Option<String> {
```

- [ ] **Step 2: Write failing tests**

Add to `src/workflows/create.rs` `tests` module. Reuse the module's existing schema
helpers; add a posixGroup schema so validation has a MUST `gidNumber`:

```rust
fn group_schema() -> SchemaModel {
    let raw = crate::ldap::worker::RawSubschema {
        object_classes: vec![
            "( 2.5.6.0 NAME 'top' ABSTRACT MUST objectClass )".into(),
            "( 1.3.6.1.1.1.2.2 NAME 'posixGroup' SUP top STRUCTURAL \
              MUST ( cn $ gidNumber ) MAY memberUid )".into(),
        ],
        attribute_types: vec![
            "( 2.5.4.3 NAME 'cn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".into(),
            "( 1.3.6.1.1.1.1.1 NAME 'gidNumber' SYNTAX 1.3.6.1.4.1.1466.115.121.1.27 SINGLE-VALUE )".into(),
            "( 1.3.6.1.1.1.1.12 NAME 'memberUid' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".into(),
        ],
        ldap_syntaxes: vec![],
    };
    SchemaModel::from_raw(&raw)
}

fn companion_spec() -> crate::config::CompanionSpec {
    let mut attributes = std::collections::BTreeMap::new();
    attributes.insert("cn".to_string(), "{uid}".to_string());
    attributes.insert("gidNumber".to_string(), "{gidNumber}".to_string());
    attributes.insert("memberUid".to_string(), "{uid}".to_string());
    crate::config::CompanionSpec {
        object_classes: vec!["posixGroup".into()],
        rdn_attr: "cn".into(),
        search_base: "ou=groups,dc=example,dc=org".into(),
        attributes,
    }
}

fn primary_attrs_with(uid: &str, gid: Option<&str>) -> BTreeMap<String, Vec<String>> {
    let mut m: BTreeMap<String, Vec<String>> = BTreeMap::new();
    m.insert("uid".into(), vec![uid.into()]);
    if let Some(g) = gid {
        m.insert("gidNumber".into(), vec![g.into()]);
    }
    m
}

#[test]
fn plan_companion_composes_dn_attrs_and_objectclass() {
    let add = plan_companion(&companion_spec(), &primary_attrs_with("alice", Some("10001")), &group_schema())
        .expect("plans");
    assert_eq!(add.dn, "cn=alice,ou=groups,dc=example,dc=org");
    assert_eq!(add.attrs.get("cn"), Some(&vec!["alice".to_string()]));
    assert_eq!(add.attrs.get("gidNumber"), Some(&vec!["10001".to_string()]));
    assert_eq!(add.attrs.get("memberUid"), Some(&vec!["alice".to_string()]));
    let oc = add.attrs.get("objectClass").expect("objectClass");
    assert_eq!(oc[0], "top");
    assert!(oc.iter().any(|v| v.eq_ignore_ascii_case("posixGroup")));
    assert!(add.ldif.contains("cn=alice,ou=groups,dc=example,dc=org"));
}

#[test]
fn plan_companion_errors_when_rdn_resolves_empty() {
    // No uid on the primary → cn template resolves empty → RDN empty.
    let err = plan_companion(&companion_spec(), &primary_attrs_with("", Some("10001")), &group_schema())
        .unwrap_err();
    assert!(err.to_lowercase().contains("cn") || err.to_lowercase().contains("rdn"));
}

#[test]
fn plan_companion_errors_when_must_attr_missing() {
    // No gidNumber on the primary → posixGroup MUST gidNumber missing → validation error.
    let err = plan_companion(&companion_spec(), &primary_attrs_with("alice", None), &group_schema())
        .unwrap_err();
    assert!(err.to_lowercase().contains("gidnumber"));
}
```

- [ ] **Step 3: Run to verify they fail**

Run: `cargo test -j4 -p edaptor plan_companion 2>&1 | tail -20`
Expected: FAIL — `plan_companion` / `CompanionAdd` unresolved.

- [ ] **Step 4: Implement**

Add to `src/workflows/create.rs` (after `plan_create`, near the top of the file's
function section):

```rust
/// A planned companion `Add` (see [`plan_companion`]).
pub struct CompanionAdd {
    pub dn: String,
    pub attrs: BTreeMap<String, Vec<String>>,
    pub ldif: String,
}

/// Plan the companion entry for a create, resolving its `attributes` templates against
/// the primary's **final** attributes (`primary_attrs` = the map `plan_create` returns).
/// Composes `objectClass` (`["top"] + object_classes`, deduped), the DN
/// (`<rdn_attr>=<resolved rdn>,<search_base>`), and validates against `schema`. Pure.
/// Errors on an empty RDN, a `{next:…}` template (unsupported), or a schema-validation
/// failure — surfaced before any write.
pub fn plan_companion(
    spec: &crate::config::CompanionSpec,
    primary_attrs: &BTreeMap<String, Vec<String>>,
    schema: &SchemaModel,
) -> Result<CompanionAdd, String> {
    use crate::config::defaults::{parse_default_value, resolve_template, DefaultValue};

    let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (attr, tmpl) in &spec.attributes {
        let resolved: Option<String> = match parse_default_value(tmpl)? {
            DefaultValue::Literal(s) => {
                let t = s.trim();
                (!t.is_empty()).then(|| t.to_string())
            }
            DefaultValue::Template(segs) => resolve_template(&segs, primary_attrs),
            DefaultValue::AutoNumber { .. } => {
                return Err(format!(
                    "companion attribute '{attr}' uses a {{next:…}} autonumber, which is unsupported"
                ))
            }
        };
        if let Some(v) = resolved {
            if !v.is_empty() {
                attrs.insert(attr.clone(), vec![v]);
            }
        }
    }

    // objectClass: "top" first, then each class, deduped case-insensitively.
    let mut oc: Vec<String> = vec!["top".to_string()];
    for c in &spec.object_classes {
        if !oc.iter().any(|x| x.eq_ignore_ascii_case(c)) {
            oc.push(c.clone());
        }
    }
    attrs.insert("objectClass".to_string(), oc);

    // RDN from the (already-resolved) rdn attribute.
    let rdn_value = attrs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(&spec.rdn_attr))
        .and_then(|(_, v)| v.first().cloned())
        .unwrap_or_default();
    if rdn_value.trim().is_empty() {
        return Err(format!(
            "companion RDN attribute '{}' resolved to an empty value",
            spec.rdn_attr
        ));
    }
    let dn = format!("{}={},{}", spec.rdn_attr, rdn_value.trim(), spec.search_base);

    let oc_refs: Vec<&str> = spec.object_classes.iter().map(String::as_str).collect();
    let full = EditEntry {
        dn: dn.clone(),
        attrs: attrs.clone(),
    };
    let errors = validate(&full, schema, &oc_refs, &[]);
    if !errors.is_empty() {
        return Err(format_validation_errors(&errors));
    }
    let ldif = render_add(&dn, &attrs);
    Ok(CompanionAdd { dn, attrs, ldif })
}
```

(`EditEntry`, `validate`, `format_validation_errors`, `render_add`, `SchemaModel`,
`BTreeMap` are already imported at the top of `create.rs`.)

- [ ] **Step 5: Run to verify they pass**

Run: `cargo test -j4 -p edaptor plan_companion 2>&1 | tail -20`
Expected: PASS — 3 tests.

- [ ] **Step 6: Format, gate, commit**

```bash
cargo fmt && cargo fmt --check && cargo clippy --all-targets -- -D warnings 2>&1 | tail -3
cargo test -j4 2>&1 | tail -5
git add src/config/defaults.rs src/workflows/create.rs
git commit -m "$(printf 'feat(create): pure plan_companion planner\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 3: Transaction capability detection

**Files:**
- Modify: `src/ldap/worker.rs` (`Request::FetchRootDse`, `Response::RootDse`,
  `fetch_root_dse`, worker-loop arm, pure `txn_supported` + tests)
- Modify: `src/ui/state.rs` (`server_supports_txn` field in both ctors + `bootstrap` read)

**Interfaces:**
- Produces: `Request::FetchRootDse`, `Response::RootDse { supported_extensions: Vec<String> }`,
  `pub fn txn_supported(exts: &[String]) -> bool`, and `UiState.server_supports_txn: bool`.

- [ ] **Step 1: Write failing test for the pure helper**

Add to `src/ldap/worker.rs` `#[cfg(test)] mod tests`:

```rust
#[test]
fn txn_supported_requires_both_oids() {
    let both = vec!["1.3.6.1.1.21.1".to_string(), "1.3.6.1.1.21.3".to_string(), "1.3.6.1.1.8".to_string()];
    assert!(txn_supported(&both));
    assert!(!txn_supported(&["1.3.6.1.1.21.1".to_string()]));
    assert!(!txn_supported(&["1.3.6.1.1.21.3".to_string()]));
    assert!(!txn_supported(&[]));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -j4 -p edaptor txn_supported 2>&1 | tail -20`
Expected: FAIL — `txn_supported` unresolved.

- [ ] **Step 3: Add the OIDs + pure helper + request/response + fetch**

In `src/ldap/worker.rs`:

Add near the top (after the existing use-statements/consts):

```rust
/// RFC 5805 transaction extended-operation OIDs.
pub const TXN_START_OID: &str = "1.3.6.1.1.21.1";
pub const TXN_END_OID: &str = "1.3.6.1.1.21.3";

/// True iff the server's `supportedExtension` advertises BOTH the Start- and
/// End-Transaction OIDs (RFC 5805). Pure.
pub fn txn_supported(exts: &[String]) -> bool {
    exts.iter().any(|e| e == TXN_START_OID) && exts.iter().any(|e| e == TXN_END_OID)
}
```

Add to `enum Request` (a variant with no id, like `FetchSubschema`):

```rust
    /// Read the root DSE `supportedExtension` list (capability probe).
    FetchRootDse,
```

Add to `enum Response`:

```rust
    /// Root DSE `supportedExtension` values (reply to [`Request::FetchRootDse`]).
    RootDse { supported_extensions: Vec<String> },
```

Add the worker-loop arm (in `worker_loop`'s `match req`, beside `FetchSubschema`):

```rust
            Request::FetchRootDse => {
                let resp = match fetch_root_dse(conn) {
                    Ok(supported_extensions) => Response::RootDse { supported_extensions },
                    Err(e) => Response::Error(e.to_string()),
                };
                let _ = reply.send(resp);
            }
```

Add the fetch function (near `fetch_subschema`):

```rust
/// Read the root DSE (`""`, base scope) and return its `supportedExtension` values.
fn fetch_root_dse(conn: &mut LdapConn) -> Result<Vec<String>> {
    let (entries, _res) = conn
        .search("", Scope::Base, "(objectClass=*)", vec!["supportedExtension"])
        .context("reading root DSE")?
        .success()
        .context("root DSE search failed")?;
    let exts = entries
        .into_iter()
        .flat_map(|e| {
            SearchEntry::construct(e)
                .attrs
                .remove("supportedExtension")
                .unwrap_or_default()
        })
        .collect();
    Ok(exts)
}
```

(Match the exact `search`/`SearchEntry::construct` idiom already used by `fetch_subschema`
/ `run_search` in this file; adjust the call shape to whatever those use.)

- [ ] **Step 4: Run to verify the helper test passes**

Run: `cargo test -j4 -p edaptor txn_supported 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Add `server_supports_txn` to `UiState` + bootstrap read**

In `src/ui/state.rs`, add the field to `UiState` (after `samba_domain`, the last field):

```rust
    /// True when the server advertises RFC 5805 transactions; drives the atomic
    /// companion-create path (vs. the sequential fallback). Set in `bootstrap`.
    pub server_supports_txn: bool,
```

Initialise it in **both** constructors — `new_for_test` (add `server_supports_txn: false,`
after `samba_domain: None,`) and `bootstrap`'s returned struct.

In `bootstrap`, after the `FetchSubschema` block that builds `schema`, add a tolerant
capability probe (a failed/absent root DSE just means "no txn"):

```rust
    let server_supports_txn = match worker.request(Request::FetchRootDse) {
        Ok(Response::RootDse { supported_extensions }) => {
            crate::ldap::worker::txn_supported(&supported_extensions)
        }
        _ => false,
    };
```

Then set `server_supports_txn,` in the returned `UiState { … }`.

- [ ] **Step 6: Format, gate, commit**

```bash
cargo fmt && cargo fmt --check && cargo clippy --all-targets -- -D warnings 2>&1 | tail -3
cargo test -j4 2>&1 | tail -5
git add src/ldap/worker.rs src/ui/state.rs
git commit -m "$(printf 'feat(ldap): detect RFC 5805 txn support via root DSE\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 4: Worker `Request::AddAtomic` (LDAP transaction)

**Files:**
- Modify: `src/ldap/worker.rs` (`Request::AddAtomic`, worker-loop arm, `run_add_atomic`,
  ldap3 txn imports)

**Interfaces:**
- Consumes: `run_add` / `run_add_atomic` share the attrs→entry conversion.
- Produces: `Request::AddAtomic { id: u64, entries: Vec<(String, BTreeMap<String, Vec<String>>)> }`
  → one `Response::WriteOk { id, dn }` (all created) or `Response::WriteError { id, msg }`
  (rolled back, nothing written).

There is **no pure unit test** for the live-transaction path (it needs a server); it is
verified by compile + the Task 6 integration run against the demo LDAP (which advertises
txn). Keep the function small.

- [ ] **Step 1: Add ldap3 txn imports**

At the top of `src/ldap/worker.rs`, add to the `ldap3` imports:

```rust
use ldap3::controls::{RawControl, TxnSpec};
use ldap3::exop::{EndTxn, StartTxn, StartTxnResp};
```

- [ ] **Step 2: Add the request variant + worker-loop arm**

Add to `enum Request`:

```rust
    /// Create several entries in one atomic RFC 5805 transaction. All succeed or the
    /// transaction is rolled back and nothing is written. `id` echoes in the reply.
    AddAtomic {
        id: u64,
        entries: Vec<(String, BTreeMap<String, Vec<String>>)>,
    },
```

Add the arm in `worker_loop` (beside `Request::Add`):

```rust
            Request::AddAtomic { id, entries } => {
                let _ = reply.send(run_add_atomic(conn, id, &entries));
            }
```

- [ ] **Step 3: Implement `run_add_atomic`**

Add near `run_add`:

```rust
/// Create every entry in `entries` inside one RFC 5805 transaction: StartTxn → each
/// Add under the transaction control → EndTxn(commit). Any Add failure (or an
/// EndTxn/commit failure) aborts the transaction and returns [`Response::WriteError`]
/// with nothing written. On success yields [`Response::WriteOk`] carrying the LAST
/// entry's DN (the primary, submitted last). Confined to the worker (ldap3-only).
fn run_add_atomic(
    conn: &mut LdapConn,
    id: u64,
    entries: &[(String, BTreeMap<String, Vec<String>>)],
) -> Response {
    // StartTransaction.
    let txn_id = match conn.extended(StartTxn) {
        Ok(ex) if ex.1.rc == 0 => ex.0.parse::<StartTxnResp>().txn_id,
        Ok(ex) => {
            return Response::WriteError {
                id,
                msg: result_code_message(ex.1.rc, &ex.1.text),
            }
        }
        Err(e) => return Response::WriteError { id, msg: format!("StartTransaction: {e}") },
    };

    let mut last_dn = String::new();
    for (dn, attrs) in entries {
        let entry: Vec<(String, HashSet<String>)> = attrs
            .iter()
            .map(|(k, vs)| (k.clone(), vs.iter().cloned().collect::<HashSet<String>>()))
            .collect();
        let ctrl = RawControl::from(TxnSpec { txn_id: &txn_id });
        match conn.with_controls(vec![ctrl]).add(dn, entry) {
            Ok(r) if r.rc == 0 => last_dn = dn.clone(),
            Ok(r) => {
                let _ = conn.extended(EndTxn { txn_id: &txn_id, commit: false });
                return Response::WriteError { id, msg: result_code_message(r.rc, &r.text) };
            }
            Err(e) => {
                let _ = conn.extended(EndTxn { txn_id: &txn_id, commit: false });
                return Response::WriteError { id, msg: format!("{e}") };
            }
        }
    }

    // EndTransaction(commit).
    match conn.extended(EndTxn { txn_id: &txn_id, commit: true }) {
        Ok(ex) if ex.1.rc == 0 => Response::WriteOk { id, dn: last_dn },
        Ok(ex) => Response::WriteError { id, msg: result_code_message(ex.1.rc, &ex.1.text) },
        Err(e) => Response::WriteError { id, msg: format!("EndTransaction commit: {e}") },
    }
}
```

(`HashSet`, `result_code_message`, `LdapConn` are already in scope in this file — mirror
`run_add`'s entry construction exactly.)

- [ ] **Step 4: Verify it builds**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -8`
Expected: clean (no unused imports; the txn types resolve). If a `with_controls` /
`extended` signature differs from the plan, adjust to the compiler's guidance — the ldap3
0.12 API is confirmed present (`ldap3::exop::{StartTxn,StartTxnResp,EndTxn}`,
`ldap3::controls::{RawControl,TxnSpec}`, `ExopResult(pub Exop, pub LdapResult)`).

- [ ] **Step 5: Format, gate, commit**

```bash
cargo fmt && cargo fmt --check && cargo test -j4 2>&1 | tail -5
git add src/ldap/worker.rs
git commit -m "$(printf 'feat(ldap): AddAtomic — create entries in one RFC 5805 txn\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 5: Write flow — atomic path + sequential fallback

**Files:**
- Modify: `src/workflows/write_flow.rs` (`WriteIntent::CompanionThenPrimary`,
  `WriteOutcome::NeedFollowupCreate`, `submit_create_atomic`, `submit_create_with_companion`,
  `submit_followup_create`, `on_response` arms, tests)
- Modify: `src/ui/state.rs` (`apply_write_outcome`: `NeedFollowupCreate` arm)

**Interfaces:**
- Consumes: `Request::AddAtomic` (Task 4), existing `WriteIntent::Create` /
  `WriteOutcome::Created`.
- Produces: `submit_create_atomic(worker, entries, reread_dn, quit_after)`,
  `submit_create_with_companion(worker, companion_dn, companion_attrs, primary_dn,
  primary_attrs, quit_after)`, `submit_followup_create(worker, dn, attrs, quit_after)`,
  `WriteOutcome::NeedFollowupCreate { dn, attrs, quit_after }`.

- [ ] **Step 1: Write failing tests**

Add to `src/workflows/write_flow.rs` `tests` module (mirror the existing
`submit`/`on_response` tests that build a `WriteFlow` and feed synthetic `Response`s;
reuse the module's existing `WriteFlow`/worker test scaffolding):

```rust
#[test]
fn atomic_create_yields_created() {
    let mut wf = WriteFlow::new();
    // Register an atomic submit's intent without a live worker: use the same id path
    // the submit uses. Simplest: drive on_response after manually inserting a Create
    // intent via submit_create_atomic against a test worker handle if the module has
    // one; otherwise assert via the sequential helpers below and cover atomic by
    // on_response mapping (AddAtomic WriteOk carries a plain Create intent).
    // Insert a Create intent under id 7 (submit_create_atomic uses WriteIntent::Create).
    wf.insert_create_intent_for_test(7, "uid=alice,ou=people,dc=x", true);
    match wf.on_response(&Response::WriteOk { id: 7, dn: "uid=alice,ou=people,dc=x".into() }) {
        WriteOutcome::Created { dn, quit_after } => {
            assert_eq!(dn, "uid=alice,ou=people,dc=x");
            assert!(quit_after);
        }
        other => panic!("expected Created, got {other:?}"),
    }
}

#[test]
fn companion_ok_yields_needfollowupcreate() {
    let mut wf = WriteFlow::new();
    let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    attrs.insert("uid".into(), vec!["alice".into()]);
    wf.insert_companion_intent_for_test(3, "uid=alice,ou=people,dc=x", attrs.clone(), false);
    match wf.on_response(&Response::WriteOk { id: 3, dn: "cn=alice,ou=groups,dc=x".into() }) {
        WriteOutcome::NeedFollowupCreate { dn, attrs: got, companion_dn, quit_after } => {
            assert_eq!(dn, "uid=alice,ou=people,dc=x");
            assert_eq!(companion_dn, "cn=alice,ou=groups,dc=x");
            assert_eq!(got.get("uid"), Some(&vec!["alice".to_string()]));
            assert!(!quit_after);
        }
        other => panic!("expected NeedFollowupCreate, got {other:?}"),
    }
}

#[test]
fn companion_error_yields_error_and_no_followup() {
    let mut wf = WriteFlow::new();
    let attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    wf.insert_companion_intent_for_test(4, "uid=alice,ou=people,dc=x", attrs, false);
    match wf.on_response(&Response::WriteError { id: 4, msg: "already exists".into() }) {
        WriteOutcome::Error(msg) => assert!(msg.contains("already exists")),
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn primary_after_companion_error_names_orphan() {
    let mut wf = WriteFlow::new();
    wf.insert_primary_after_companion_for_test(
        5, "uid=alice,ou=people,dc=x", "cn=alice,ou=groups,dc=x", false,
    );
    match wf.on_response(&Response::WriteError { id: 5, msg: "boom".into() }) {
        WriteOutcome::Error(m) => {
            assert!(m.contains("cn=alice,ou=groups,dc=x"), "names the orphan: {m}");
            assert!(m.contains("boom"));
        }
        other => panic!("expected Error, got {other:?}"),
    }
}
```

Add these two `#[cfg(test)]`-only seam methods to `impl WriteFlow` (they mirror what the
submit functions insert; used so the pure `on_response` can be tested without a worker):

```rust
    #[cfg(test)]
    pub(crate) fn insert_create_intent_for_test(&mut self, id: u64, dn: &str, quit_after: bool) {
        self.pending.insert(id, WriteIntent::Create { dn: dn.to_string(), quit_after });
    }
    #[cfg(test)]
    pub(crate) fn insert_companion_intent_for_test(
        &mut self,
        id: u64,
        primary_dn: &str,
        primary_attrs: BTreeMap<String, Vec<String>>,
        quit_after: bool,
    ) {
        self.pending.insert(id, WriteIntent::CompanionThenPrimary {
            primary_dn: primary_dn.to_string(),
            primary_attrs,
            quit_after,
        });
    }
    #[cfg(test)]
    pub(crate) fn insert_primary_after_companion_for_test(
        &mut self,
        id: u64,
        primary_dn: &str,
        companion_dn: &str,
        quit_after: bool,
    ) {
        self.pending.insert(id, WriteIntent::PrimaryAfterCompanion {
            primary_dn: primary_dn.to_string(),
            companion_dn: companion_dn.to_string(),
            quit_after,
        });
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -j4 -p edaptor -- companion_ok_yields companion_error primary_after_companion atomic_create_yields 2>&1 | tail -20`
Expected: FAIL — `CompanionThenPrimary` / `PrimaryAfterCompanion` / `NeedFollowupCreate` / seam methods unresolved.

- [ ] **Step 3: Add the intent + outcome**

In `src/workflows/write_flow.rs`, add to `enum WriteIntent`:

```rust
    /// Sequential fallback (no server txn): the companion ADD was submitted; on its
    /// success submit the primary ADD carried here. See [`WriteFlow::submit_create_with_companion`].
    CompanionThenPrimary {
        primary_dn: String,
        primary_attrs: BTreeMap<String, Vec<String>>,
        quit_after: bool,
    },
```

Add to `enum WriteOutcome`:

```rust
    /// The companion ADD landed; the caller must now submit the primary ADD via
    /// [`WriteFlow::submit_followup_create`]. Sequential fallback only. `companion_dn`
    /// is the just-created companion, carried so a later primary failure can name the
    /// orphan.
    NeedFollowupCreate {
        dn: String,
        attrs: BTreeMap<String, Vec<String>>,
        companion_dn: String,
        quit_after: bool,
    },
```

Also add a second intent for the sequential primary leg (so its failure can name the
orphan companion):

```rust
    /// Sequential fallback: the primary ADD, submitted after the companion succeeded.
    /// On success → [`WriteOutcome::Created`]; on failure → an error naming the orphaned
    /// `companion_dn` that was already created.
    PrimaryAfterCompanion {
        primary_dn: String,
        companion_dn: String,
        quit_after: bool,
    },
```

- [ ] **Step 4: Add the `on_response` arms**

The outer `Response::WriteOk` match currently binds `{ id, .. }`. Change it to bind the
response DN — `Response::WriteOk { id, dn: resp_dn }` — so the companion arm can carry it.
Then add these arms in the `WriteOk` match (beside the `Create` arm):

```rust
                Some(WriteIntent::CompanionThenPrimary {
                    primary_dn,
                    primary_attrs,
                    quit_after,
                }) => WriteOutcome::NeedFollowupCreate {
                    dn: primary_dn,
                    attrs: primary_attrs,
                    companion_dn: resp_dn, // the companion just created
                    quit_after,
                },
                Some(WriteIntent::PrimaryAfterCompanion {
                    primary_dn,
                    quit_after,
                    ..
                }) => WriteOutcome::Created { dn: primary_dn, quit_after },
```

In the `Response::WriteError` match, add BEFORE the generic `Some(_) =>
WriteOutcome::Error(msg.clone())` arm:

```rust
                Some(WriteIntent::PrimaryAfterCompanion { companion_dn, .. }) => {
                    WriteOutcome::Error(format!(
                        "The primary entry failed to create ({msg}). Its companion \
                         {companion_dn} was already created — remove it or retry."
                    ))
                }
```

A companion failure (the `CompanionThenPrimary` intent's own `WriteError`) still falls
into the generic `Some(_) => Error(msg)` arm, so no primary is submitted — correct.

- [ ] **Step 5: Add the submit functions**

Add to `impl WriteFlow` (near `submit_create`):

```rust
    /// Atomic path: create `entries` (companion first, primary last) in one RFC 5805
    /// transaction. `reread_dn` is the primary DN to re-read after success. One
    /// `WriteOk` → [`WriteOutcome::Created`]; one `WriteError` → [`WriteOutcome::Error`]
    /// (nothing written).
    pub fn submit_create_atomic(
        &mut self,
        worker: &WorkerHandle,
        entries: Vec<(String, BTreeMap<String, Vec<String>>)>,
        reread_dn: &str,
        quit_after: bool,
    ) -> Result<()> {
        let id = self.alloc();
        worker.submit(Request::AddAtomic { id, entries })?;
        self.pending.insert(
            id,
            WriteIntent::Create { dn: reread_dn.to_string(), quit_after },
        );
        Ok(())
    }

    /// Sequential fallback: submit the companion ADD first, carrying the primary ADD to
    /// submit on the companion's success (via [`WriteOutcome::NeedFollowupCreate`] →
    /// [`submit_followup_create`](Self::submit_followup_create)).
    pub fn submit_create_with_companion(
        &mut self,
        worker: &WorkerHandle,
        companion_dn: &str,
        companion_attrs: BTreeMap<String, Vec<String>>,
        primary_dn: &str,
        primary_attrs: BTreeMap<String, Vec<String>>,
        quit_after: bool,
    ) -> Result<()> {
        let id = self.alloc();
        worker.submit(Request::Add {
            id,
            dn: companion_dn.to_string(),
            attrs: companion_attrs,
        })?;
        self.pending.insert(
            id,
            WriteIntent::CompanionThenPrimary {
                primary_dn: primary_dn.to_string(),
                primary_attrs,
                quit_after,
            },
        );
        Ok(())
    }

    /// Sequential fallback second phase: submit the primary ADD after the companion
    /// (`companion_dn`) landed. Tracked as [`WriteIntent::PrimaryAfterCompanion`], so its
    /// success → [`WriteOutcome::Created`] and its failure → an error naming the orphaned
    /// companion.
    pub fn submit_followup_create(
        &mut self,
        worker: &WorkerHandle,
        dn: &str,
        attrs: BTreeMap<String, Vec<String>>,
        companion_dn: &str,
        quit_after: bool,
    ) -> Result<()> {
        let id = self.alloc();
        worker.submit(Request::Add { id, dn: dn.to_string(), attrs })?;
        self.pending.insert(
            id,
            WriteIntent::PrimaryAfterCompanion {
                primary_dn: dn.to_string(),
                companion_dn: companion_dn.to_string(),
                quit_after,
            },
        );
        Ok(())
    }
```

- [ ] **Step 6: Handle `NeedFollowupCreate` in `state.rs`**

In `src/ui/state.rs` `apply_write_outcome`, add (beside the `NeedFollowupModify` arm ~line
463):

```rust
            WriteOutcome::NeedFollowupCreate { dn, attrs, companion_dn, quit_after } => {
                if let Some(w) = self.worker.as_ref() {
                    let _ = self
                        .write_flow
                        .submit_followup_create(w, &dn, attrs, &companion_dn, quit_after);
                }
            }
```

- [ ] **Step 7: Run to verify the tests pass**

Run: `cargo test -j4 -p edaptor -- companion_ok_yields companion_error primary_after_companion atomic_create_yields 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 8: Format, gate, commit**

```bash
cargo fmt && cargo fmt --check && cargo clippy --all-targets -- -D warnings 2>&1 | tail -3
cargo test -j4 2>&1 | tail -5
git add src/workflows/write_flow.rs src/ui/state.rs
git commit -m "$(printf 'feat(write): atomic + sequential companion create paths\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 6: `do_create` — plan companion, preview both, dispatch

**Files:**
- Modify: `src/ui/app.rs` (`do_create`, ~lines 700-742)

**Interfaces:**
- Consumes: `create::plan_companion` + `CompanionAdd` (Task 2), `state.server_supports_txn`
  (Task 3), `write_flow.submit_create_atomic` / `submit_create_with_companion` (Task 5).

No new unit test (UI wiring; the planners/write paths are unit-tested in Tasks 2 & 5).
Verified by compile + the integration run below.

- [ ] **Step 1: Plan the companion alongside the primary**

In `do_create`, the first borrow block computes `prep` (the primary plan). Extend that
block to also capture the profile's companion spec and the schema so the companion can be
planned after the primary attrs are known. Add to the tuple returned from that block: the
`Option<CompanionSpec>` (cloned) and keep the `schema` reachable. Concretely, inside the
`let st = state.borrow();` block, after `let profile = &st.profiles[*profile_idx];`, add:

```rust
        let companion_spec = profile.companion.clone();
```

and include `companion_spec` in the block's returned tuple (extend the existing
destructuring `let (prep, pending, pending_pw_attrs, resolved_widgets) = { … };` to
`let (prep, pending, pending_pw_attrs, resolved_widgets, companion_spec) = { … };`,
returning `(prep, pending, pending_pw_attrs, resolved_widgets, companion_spec)`).

- [ ] **Step 2: In the `Confirm` arm, plan the companion and build the two-stanza preview**

In the `CreatePrep::Confirm { dn, mut attrs, ldif, .. }` arm, AFTER the password fold that
produces the final `ldif` (the `let ldif = masked.unwrap_or(ldif);` line) and BEFORE the
confirm dialog is built, insert:

```rust
            // Plan the companion (if declared) against the primary's final attrs.
            let companion = match &companion_spec {
                Some(spec) => {
                    let planned = {
                        let st = state.borrow();
                        crate::workflows::create::plan_companion(spec, &attrs, st.read_flow.schema())
                    };
                    match planned {
                        Ok(c) => Some(c),
                        Err(msg) => {
                            let (view, ok) = crate::ui::dialog::error::build(&msg);
                            prog.exec_view_focused(view, ok);
                            return; // companion invalid → abort the whole create
                        }
                    }
                }
                None => None,
            };

            // Preview: primary stanza, then the companion stanza when present.
            let preview = match &companion {
                Some(c) => format!("# New entry\n{ldif}\n# Companion entry\n{}", c.ldif),
                None => ldif,
            };
            let (view, save) = crate::ui::dialog::confirm::build(&preview);
```

Replace the existing `let (view, save) = crate::ui::dialog::confirm::build(&ldif);` line
with the block above (it ends by building the confirm view from `preview`).

- [ ] **Step 3: Dispatch to the right write path on OK**

Replace the existing submit block (the `let mut st = state.borrow_mut(); … submit_create(w, &dn, attrs, false);`
tail of the `Confirm` arm) with:

```rust
            let mut st = state.borrow_mut();
            st.pending_password = None; // cleartext consumed
            let supports_txn = st.server_supports_txn;
            let crate::ui::state::UiState { worker, write_flow, .. } = &mut *st;
            if let Some(w) = worker.as_ref() {
                match companion {
                    Some(c) if supports_txn => {
                        // Atomic: companion first, primary last; re-read the primary.
                        let _ = write_flow.submit_create_atomic(
                            w,
                            vec![(c.dn, c.attrs), (dn.clone(), attrs)],
                            &dn,
                            false,
                        );
                    }
                    Some(c) => {
                        // Sequential fallback: companion first, then primary.
                        let _ = write_flow.submit_create_with_companion(
                            w, &c.dn, c.attrs, &dn, attrs, false,
                        );
                    }
                    None => {
                        let _ = write_flow.submit_create(w, &dn, attrs, false);
                    }
                }
            }
```

- [ ] **Step 4: Build, gate**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5 && cargo test -j4 2>&1 | tail -8`
Expected: clean; all tests pass.

- [ ] **Step 5: Integration check (record the result)**

Add a companion to a profile in the demo config and create a user through it against the
demo LDAP (which advertises txn):

```bash
scripts/test-ldap.sh start
export EDAPTOR_TEST_ADMIN_PW=adminpassword
cargo run -- --config examples/demo-config.toml   # (or tui-create the user profile)
```

Create a user under the companion-declaring profile; confirm the preview shows BOTH
stanzas and, after OK, BOTH the user and the `cn=<uid>` group exist under the groups OU
(re-browse or `ldapsearch`). Note the outcome in the report. (The sequential fallback is
exercised only against a non-txn server; note it as not-locally-verified if no such server
is available.)

- [ ] **Step 6: Format, commit**

```bash
cargo fmt && cargo fmt --check
git add src/ui/app.rs
git commit -m "$(printf 'feat(ui): create the companion entry alongside the primary\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Task 7: Docs + `CHANGES.md` + example

**Files:**
- Create: `docs/src/configuration/companion.md`
- Modify: `docs/src/SUMMARY.md`, `examples/config.toml`, `CHANGES.md`

- [ ] **Step 1: mdBook page**

Create `docs/src/configuration/companion.md`:

```markdown
# Companion Entries

A profile may declare **one companion entry** that eDAPtor creates alongside the primary
whenever you create through that profile. The classic use is a **user-private group**: a
`posixGroup` whose `cn` is the user's `uid` and whose `gidNumber` mirrors the user's.

```toml
[profile.companion]
object_classes = ["posixGroup"]
rdn_attr       = "cn"
search_base    = "ou=groups,dc=example,dc=org"

[profile.companion.attributes]
cn        = "{cn}"          # templates resolve against the primary's final attributes
gidNumber = "{gidNumber}"   # mirrors the user's already-allocated gid
memberUid = "{uid}"
```

- `attributes` values use the same literal / `{attr}` template syntax as
  [Defaults](defaults.md); they resolve against the **primary's** composed attributes
  (including its RDN, defaults, and allocated autonumbers). `{next:…}` autonumbers are
  **not** allowed in a companion.
- `objectClass` comes from `object_classes` (with `top` added); `rdn_attr` must be one of
  the `attributes` keys.

## Atomicity

When the server advertises **LDAP transactions (RFC 5805)**, the primary and the
companion are created in **one atomic transaction** — either both are created or neither
is. Against a server without transaction support, eDAPtor falls back to creating the
**companion first, then the primary**; if the companion fails, the primary is not created.
Both entries are shown in the create confirmation before anything is written.
```

- [ ] **Step 2: SUMMARY.md**

In `docs/src/SUMMARY.md`, add under the Configuration section (after `Defaults` or
`Widgets`):

```markdown
- [Companion Entries](configuration/companion.md)
```

- [ ] **Step 3: examples/config.toml**

Add a `[profile.companion]` block to the user profile in `examples/config.toml`
(consistent with `docs/src/configuration/full-example.md`), e.g.:

```toml
[profile.companion]
object_classes = ["posixGroup"]
rdn_attr       = "cn"
search_base    = "ou=groups,dc=example,dc=org"

[profile.companion.attributes]
cn        = "{cn}"
gidNumber = "{gidNumber}"
memberUid = "{uid}"
```

Verify the example config still parses: `cargo run -- --config examples/config.toml check`
is not required (no server), but `cargo test -j4` must stay green.

- [ ] **Step 4: CHANGES.md**

Under `## Unreleased` → `### New`, add:

```markdown
- **Companion entries on create.** A profile can declare `[profile.companion]` (e.g. a
  `posixGroup` mirroring a POSIX user); creating through the profile creates both
  entries — atomically via LDAP transactions (RFC 5805) when the server supports them,
  otherwise companion-first with the primary aborted on failure. The create confirmation
  previews both entries. See
  [Configuration → Companion Entries](https://oposs.github.io/edaptor/configuration/companion.html).
```

- [ ] **Step 5: Build docs, gate, commit**

```bash
make docs 2>&1 | tail -8
cargo test -j4 2>&1 | tail -5
git add docs/src/configuration/companion.md docs/src/SUMMARY.md examples/config.toml CHANGES.md
git commit -m "$(printf 'docs: companion entries page + example + changelog\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Final verification

- [ ] **Full gate:** `make check 2>&1 | tail -20` → fmt clean, clippy `-D warnings` clean,
  all tests pass.
- [ ] **Confirm the Task 6 integration outcome was recorded** (both entries created
  atomically against the demo LDAP; fallback noted).
