# edaptor — API Spike Findings (2026-05-29)

Verified-against-primary-sources notes that the implementation plans depend on.
Produced by a parallel research fan-out before writing the M1 plan. Each section
ends with the **decision** baked into the plans.

> Re-verify any **medium**-confidence item against the live server/crate before
> relying on it in code.

---

## 1. ldap3 — synchronous core, TLS, binds  *(confidence: high)*

- **Crate:** `ldap3 = "0.12"` (latest 0.12.1). Synchronous type is `LdapConn`.
- **Default features:** `["sync", "tls"]`, where `tls` = `tls-native` (native-tls
  backend). `sync` and native-tls TLS are therefore **on by default**.
- **rustls** is opt-in and **mutually exclusive** with native-tls — requires
  `default-features = false` + `tls-rustls-ring` (or `-aws-lc-rs`). The exact
  rustls settings method (`set_config`) was only single-sourced (medium
  confidence).
- **Connect with custom TLS:** `LdapConn::with_settings(settings, url)`.
- **Custom CA / client cert:** ldap3 exposes **no** CA helper — build a
  `native_tls::TlsConnector` yourself and pass it via
  `LdapConnSettings::set_connector(connector)`. Add `native-tls = "0.2"` as a
  **direct** dependency.
- **StartTLS:** use an `ldap://` URL + `LdapConnSettings::set_starttls(true)`
  (do **not** use `ldaps://` for StartTLS).
- **Disable verification (TEST ONLY):** `set_no_tls_verify(true)`.
- **Binds:** `simple_bind(dn, pw) -> Result<LdapResult>`,
  `sasl_external_bind()` (client-cert identity, no password),
  `sasl_gssapi_bind(server_fqdn)` (feature `gssapi`),
  `sasl_gssapi_cred_bind(cred, fqdn)`. Always chain `.success()?` — a returned
  `Ok(LdapResult)` with non-zero `rc` still means the bind was **rejected**.
- `LdapConnSettings` setters consume `self` (builder); chain them.

```rust
use std::fs;
use ldap3::{LdapConn, LdapConnSettings, LdapResult, result::Result};
use native_tls::{Certificate, TlsConnector};

fn connect(uri: &str, ca_pem_path: &str, dn: &str, pw: &str) -> Result<LdapConn> {
    let ca = Certificate::from_pem(&fs::read(ca_pem_path).unwrap()).unwrap();
    let connector = TlsConnector::builder().add_root_certificate(ca).build().unwrap();
    let settings = LdapConnSettings::new().set_connector(connector);
    let mut ldap = LdapConn::with_settings(settings, uri)?; // ldaps://host:636
    let _res: LdapResult = ldap.simple_bind(dn, pw)?.success()?;
    Ok(ldap)
}
```

**Decision (M1):** native-tls backend (default), simple bind. External/GSSAPI
binds are parsed in config but implemented in later milestones (GSSAPI last,
behind `#[cfg(feature = "gssapi")]`).

## 2. Paged results (RFC 2696)  *(confidence: high)*

- High-level: `ldap3::adapters::PagedResults::new(size)` + `EntriesOnly` via
  `LdapConn::streaming_search_with(adapters, base, scope, filter, attrs)` —
  hides cookie handling. **Preferred** for edaptor lists.
- Manual: `ldap.with_controls(PagedResults { size, cookie }).search(...)`, then
  read the returned cookie from `res.ctrls` (`Control(Some(ControlType::PagedResults), raw)`
  → `raw.parse::<PagedResults>()`). **Empty returned cookie = done.**
- **Footguns:** `with_controls` applies to the **next op only** (re-apply each
  loop iteration); the response `size` is a server *estimate*, not the page
  count — never loop on it.

**Decision:** use the `adapters` path for large-container lists (M4+).

## 3. Schema parsing (RFC 4512)  *(confidence: high on existence, medium on hardening)*

- **Take a dependency:** `ldap-types = "0.7"` (0.7.2, Feb 2026). Its `schema`
  module parses objectClass/attributeType/ldapSyntax/matchingRule description
  strings into typed structs (chumsky-based). ldap3 itself does **not** parse
  schema.
- Parsers: `schema::object_class_parser()`, `attribute_type_parser()`,
  `ldap_syntax_parser()`, `ldap_schema_parser()`. Call
  `.parse(input).into_result()` (chumsky returns `ParseResult`, not `Result`).
- `struct ObjectClass { oid, name: Vec<KeyString>, sup: Vec<KeyStringOrOID>,
  desc, object_class_type: ObjectClassType, must: Vec<..>, may: Vec<..>,
  obsolete }` — `Vec`s cover `( a $ b $ c )` and `NAME ( 'cn' 'commonName' )`.
- **Caveat:** default features pull in `ldap3` (version-conflict risk), `serde`,
  `diff`, `ariadne`. Use `ldap-types = { version = "0.7", default-features =
  false, features = ["chumsky"] }`.
- **Caveat:** modest adoption (~274 dl/month). Write **golden-file tests**
  against the real OpenLDAP `cn=subschema`. Fallback if too thin: hand-roll with
  `chumsky`/`winnow` (~200 lines; grammar in RFC 4512 §4.1).

**Decision (M2):** depend on `ldap-types` (chumsky-only features); golden-file
test against the live `oposs.openldap` schema.

### 3a. ldap-types — verified by compilation (2026-05-29, for M2)  *(confidence: high)*

A throwaway crate was built against `ldap-types` 0.7.2 to confirm the exact API:

- **Three direct deps required** (the `Parser` trait and `ObjectIdentifier` type
  are not usably re-exported, and versions must match ldap-types' locked tree):
  ```toml
  ldap-types = { version = "0.7", default-features = false, features = ["chumsky"] }
  chumsky = "0.12"   # locked tree has 0.12.0 — needed for the Parser trait
  oid = "0.3"        # locked tree has 0.3.0 — ObjectIdentifier type + try_from
  ```
  `default-features = false, features = ["chumsky"]` pulls in NEITHER ldap3 nor
  serde nor diff (confirmed) — so there is no ldap3 version conflict with our 0.12.
- **Parsing:** `use chumsky::Parser;` then
  `object_class_parser().parse(s).into_result()` (and `attribute_type_parser()`).
  chumsky's `.parse()` returns a `ParseResult`, so `.into_result()` is required to
  get a `Result`.
- **Name extraction:** `KeyString` and `KeyStringOrOID` both implement `Display`,
  so `.to_string()` yields the plain name. (`KeyString` does NOT `Deref` to `str`.)
- **`ObjectClass` fields:** `oid`, `name: Vec<KeyString>`, `sup: Vec<KeyStringOrOID>`,
  `desc: Option<String>`, `object_class_type: ObjectClassType` (variants
  `Structural` / `Abstract` / `Auxiliary`), `must: Vec<KeyStringOrOID>`,
  `may: Vec<KeyStringOrOID>`, `obsolete: bool`.
- **`AttributeType` fields:** `oid`, `name: Vec<KeyString>`,
  `sup: Option<KeyStringOrOID>`, `desc`, `syntax: Option<OIDWithLength>`
  (where `OIDWithLength { oid: oid::ObjectIdentifier, length: Option<u32> }`),
  `single_value: bool`, `equality: Option<KeyString>`, `substr`, etc.
- **Syntax classification:** `oid::ObjectIdentifier` has **no** `Display`/`ToString`.
  Compare against known syntaxes via `PartialEq`:
  `syntax.oid == ObjectIdentifier::try_from("1.3.6.1.4.1.1466.115.121.1.7").unwrap()`
  (verified `== true` for the Boolean syntax). Define known syntax OIDs as `&str`
  constants and parse them once.
- **Robustness note:** parse each description string individually and collect
  failures rather than aborting — OpenLDAP may emit definitions with quirks.
  Inheritance (SUP) is NOT resolved by the crate; edaptor walks SUP chains itself.

## 4. turbo-vision 1.2 — widgets & the dual-pane editor  *(confidence: medium-high)*

- **Verdict:** a single modal `Dialog` can hold two side-by-side `ListBox`es,
  two `InputLine`s, and add/remove `Button`s; the inner `Group` auto-routes
  keyboard/command events and Tab focus among children.
- **Live per-pane filtering is NOT turn-key.** `SortedListBox::focus_prefix`
  only *jumps* to a match. To *shrink* a list you rebuild it with
  `ListBox::set_items(Vec<String>)` on each keystroke. Read the query from an
  `InputLine` built with `.data(Rc<RefCell<String>>)` (there is no text getter)
  and implement a small custom `View` that re-filters on `EventType::Keyboard`.
- **Widgets present:** `ListBox`/`SortedListBox`, `InputLine`, `Button`,
  `CheckBox`, `RadioButton`, `MenuBar`, `StatusLine`, `Dialog`, and
  `OutlineViewer` + `Node` (a real tree/outline widget → DIT browser).
- **No feature flags needed** (only `ssh`, `test-util` exist). No hosted docs —
  the authoritative reference is the `src/views/` and `examples/` trees.
- Useful examples: `list_components.rs`, `sorted_listbox.rs`, `validator.rs`
  (InputLine `.data`), `file_browser.rs` (manual dual-pane fallback),
  `tree_view.rs` (OutlineViewer), `showcase.rs`, `command_set.rs`.

**Decision (M3/M4):** wrap turbo-vision behind `src/ui/facade.rs`; implement a
custom `FilteredList` view (InputLine query → `set_items`) for both membership
panes and all searchable lists; use `OutlineViewer` for the DIT tree.

## 5. Samba password & SID logic  *(confidence: high)*

- **NT hash** (`sambaNTPassword`) = MD4(UTF-16LE(password)) as **32 uppercase**
  hex chars. Crate `md4 = "0.11"` (0.11.0 stable; avoid 0.11.0-rc.*).
- `encode_utf16()` yields host-order `u16`; you **must** `to_le_bytes()` each.
- `sambaPwdLastSet` = unix epoch seconds.
- **RID (Samba 3.x algorithmic):** `userRID = 2*uidNumber + base` (even),
  `groupRID = 2*gidNumber + base + 1` (odd); default base = **1000**.
  `sambaSID = <domainSID>-<RID>`.
- **Discover from directory:** objectClass `sambaDomain` holds `sambaSID` and
  (optional) `sambaAlgorithmicRidBase` → read it; fall back to config + 1000.
- `sambaSamAccount` (AUXILIARY) MUST `uid`, `sambaSID`. `sambaGroupMapping`
  (AUXILIARY) MUST `gidNumber`, `sambaSID`, `sambaGroupType` (2 = domain group,
  4 = alias). `sambaAcctFlags` = 16-char bracket string, e.g. `[U          ]`.
- MD4 is cryptographically broken — used only for Windows compatibility.

```rust
use md4::{Md4, Digest};
fn samba_nt_password(password: &str) -> String {
    let utf16le: Vec<u8> = password.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let mut h = Md4::new();
    h.update(&utf16le);
    h.finalize().iter().map(|b| format!("{:02X}", b)).collect()
}
```

**Decision (M5):** `md4` 0.11; discover domain SID from `sambaDomain`; config
`[samba]` is fallback only.

## 6. SASL EXTERNAL & GSSAPI  *(confidence: high)*

- `sasl_external_bind()` — no args; identity from the TLS **client certificate**
  (configure the cert in `LdapConnSettings`). Needs `authz-regexp` mapping on
  the server.
- `sasl_gssapi_bind(server_fqdn)` — feature `gssapi` (off by default). Build
  deps: `clang`/`libclang-dev` (bindgen) + `libkrb5-dev`; runtime needs a valid
  Kerberos ccache. FFI to C → most fragile path.
- No `sasl_spnego_bind`. `sasl_ntlm_bind` exists behind feature `ntlm`.

**Decision:** simple bind first (M1); external mid-stream; **GSSAPI last**,
feature-gated, so the core build stays dependency-light.
