//! Configuration: connection properties + auth. (Entry profiles arrive in M4.)

pub mod builtin;
pub mod defaults;
pub mod discovery;
pub mod label;
pub mod password;
pub mod relation;
pub mod resolver;
pub mod tree_label;
pub mod widget;
pub use password::PasswordSource;

use crate::config::defaults::ProfileDefaults;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Deserialize)]
pub struct MetaConfig {
    pub name: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(try_from = "RawConfig")]
pub struct Config {
    pub meta: MetaConfig,
    pub server: ServerConfig,
    pub auth: AuthConfig,
    /// The config's own complete profiles (blocks with `object_classes`, not
    /// disabled), in file order: what eDAPtor uses when detection is off.
    pub profiles: Vec<EntryProfile>,
    /// Every `[[profile]]` block as written (input to the merge).
    pub overrides: Vec<ProfileOverride>,
    /// The `[detect]` table.
    pub detect: DetectConfig,
    /// Samba lifecycle fallback settings. Used only when no `sambaDomain` entry
    /// is discovered in the directory (spec section 9). Absent `[samba]` table is fine.
    pub samba: SambaConfig,
    /// Configurable DIT-tree (pane 1) branch labels. Absent `[tree]` is fine.
    pub tree: TreeConfig,
}

#[derive(Deserialize)]
struct RawConfig {
    #[serde(default)]
    meta: MetaConfig,
    server: ServerConfig,
    auth: AuthConfig,
    #[serde(default, rename = "profile")]
    profiles: Vec<ProfileOverride>,
    #[serde(default)]
    detect: DetectConfig,
    #[serde(default)]
    samba: SambaConfig,
    #[serde(default)]
    tree: TreeConfig,
}

impl TryFrom<RawConfig> for Config {
    type Error = String;

    fn try_from(raw: RawConfig) -> Result<Self, String> {
        let mut profiles = Vec::new();
        for o in &raw.profiles {
            if o.object_class.is_some() {
                return Err(format!(
                    "profile \"{}\": `object_class` is not supported, use object_classes = [\"…\"]",
                    o.name
                ));
            }
            if o.object_classes.is_none() && !raw.detect.enabled {
                return Err(format!(
                    "profile \"{}\": missing field `object_classes` (required when [detect] enabled = false)",
                    o.name
                ));
            }
            if !o.is_enabled() {
                continue;
            }
            if let Some(p) = o.to_entry_profile() {
                profiles.push(p);
            }
        }
        Ok(Config {
            meta: raw.meta,
            server: raw.server,
            auth: raw.auth,
            profiles,
            overrides: raw.profiles,
            detect: raw.detect,
            samba: raw.samba,
            tree: raw.tree,
        })
    }
}

/// Where a profile's `New` is offered: `Boundary` = the container, its ancestors
/// and descendants (config and matched profiles); `Exact` = only the container
/// itself (detected-only profiles, spec 2A "Container scope").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ContainerScope {
    #[default]
    Boundary,
    Exact,
}

/// The `[detect]` table.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DetectConfig {
    /// `false` = no sampling at all; profiles come from the config only.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for DetectConfig {
    fn default() -> Self {
        DetectConfig { enabled: true }
    }
}

/// One `[[profile]]` block as written: every key optional, so "not set" differs
/// from "empty" and a block may only name a detected profile to change it.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProfileOverride {
    pub name: String,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub suppress: Vec<String>,
    #[serde(default)]
    pub object_classes: Option<Vec<String>>,
    #[serde(default)]
    pub rdn_attr: Option<String>,
    #[serde(default)]
    pub search_base: Option<String>,
    #[serde(default)]
    pub show: Option<Vec<String>>,
    #[serde(default)]
    pub search_attrs: Option<Vec<String>>,
    #[serde(default)]
    pub defaults: Option<ProfileDefaults>,
    #[serde(default, rename = "widget")]
    pub widgets: Option<std::collections::BTreeMap<String, WidgetSpecCfg>>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub companion: Option<CompanionSpec>,
    /// The removed single-string key; present only to reject it with a hint.
    #[serde(default)]
    object_class: Option<toml::Value>,
}

impl ProfileOverride {
    pub fn is_enabled(&self) -> bool {
        self.enabled != Some(false)
    }

    /// The block as a stand-alone profile (today's semantics); `None` without
    /// `object_classes`.
    pub fn to_entry_profile(&self) -> Option<EntryProfile> {
        Some(EntryProfile {
            name: self.name.clone(),
            object_classes: self.object_classes.clone()?,
            rdn_attr: self.rdn_attr.clone().unwrap_or_default(),
            search_base: self.search_base.clone().unwrap_or_default(),
            show: self.show.clone().unwrap_or_default(),
            search_attrs: self.search_attrs.clone().unwrap_or_default(),
            defaults: self.defaults.clone().unwrap_or_default(),
            widgets: self.widgets.clone().unwrap_or_default(),
            label: self.label.clone(),
            companion: self.companion.clone(),
            scope: ContainerScope::Boundary,
        })
    }
}

/// The optional `[tree]` table: ordered, presence-keyed labelling rules for the
/// DIT navigation tree (pane 1) branch nodes. Absent table → empty list →
/// compile substitutes the built-in default rule set.
#[derive(Debug, Default, Deserialize)]
pub struct TreeConfig {
    #[serde(default)]
    pub label: Vec<TreeLabelRule>,
}

/// One `[[tree.label]]` rule. The first rule whose `when` attributes are all
/// present (non-empty first value) wins; a rule with an empty/omitted `when` is
/// the unconditional fallback.
#[derive(Debug, Deserialize)]
pub struct TreeLabelRule {
    #[serde(default)]
    pub when: Vec<String>,
    pub template: String,
}

/// Fallback Samba domain settings (spec §9). The live `sambaDomain` entry takes
/// precedence; these values are used only when that entry is absent.
#[derive(Debug, Deserialize)]
pub struct SambaConfig {
    #[serde(default)]
    pub domain_sid: Option<String>,
    #[serde(default = "default_rid_base")]
    pub algorithmic_rid_base: u32,
}

fn default_rid_base() -> u32 {
    1000
}

// Manual Default so an absent [samba] table yields algorithmic_rid_base = 1000.
impl Default for SambaConfig {
    fn default() -> Self {
        SambaConfig {
            domain_sid: None,
            algorithmic_rid_base: default_rid_base(),
        }
    }
}

fn default_store() -> String {
    "dn".to_string()
}

fn default_select() -> String {
    "auto".to_string()
}

/// One option in a `choice` widget: the stored token and its UI label.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
pub struct ChoiceOption {
    /// The token stored in the encoded value (a samba letter, a shell path, …).
    pub value: String,
    /// The human-facing label shown in the checklist and the summary.
    pub label: String,
}

/// A candidate source for a `picker`/`membership` widget: either the name of a
/// declared `[[profile]]` (whose search scope is reused) or an inline scope.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(untagged)]
pub enum CandidateRef {
    /// Name of a `[[profile]]` whose scope (base/object_classes/search_attrs/label) is reused.
    Profile(String),
    /// An inline candidate scope (pick from entries that have no managed profile).
    Inline(InlineScope),
}

/// An inline `candidate = { … }` table for a picker/membership widget.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
pub struct InlineScope {
    pub base: String,
    pub object_classes: Vec<String>,
    #[serde(default)]
    pub search_attrs: Vec<String>,
    #[serde(default)]
    pub label: Option<String>,
}

/// A `[profile.widget.<attr>]` binding. `kind`-tagged so future widget kinds add
/// variants without breaking existing config.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum WidgetSpecCfg {
    /// Pick from a fixed vocabulary; (de)serialise a single attribute string.
    Choice {
        /// `"single"` or `"multi"`.
        select: String,
        /// `"plain"` | `"bracketed"` (now); `"bitmask"` | `"delimited"` (reserved).
        format: String,
        /// The selectable options (non-empty; validated at resolve time).
        options: Vec<ChoiceOption>,
    },
    /// Inline password widget: renders a masked field with a set-password popup.
    /// When `samba` is true, also syncs `sambaNTPassword` / `sambaPwdLastSet`.
    Password {
        /// When true, also write Samba NT-hash attributes alongside the LDAP password.
        #[serde(default)]
        samba: bool,
    },
    /// Pick candidate value(s) and store them in *this* entry's attribute
    /// (covers value-lookup like `gidNumber` and DN/scalar lists like `member`).
    Picker {
        candidate: CandidateRef,
        /// The sentinel `"dn"` (default), or a candidate attribute name to store.
        #[serde(default = "default_store")]
        store: String,
        /// `"single"` | `"multi"` | `"auto"` (default; derive from schema arity).
        #[serde(default = "default_select")]
        select: String,
    },
    /// Fan *this* entry's DN out into a back-ref attr (`via`) on each picked
    /// candidate (covers `memberOf`). Always multi-select; no `store`/`select`.
    Membership {
        candidate: CandidateRef,
        /// The back-ref attribute written on each picked candidate (e.g. `member`).
        via: String,
    },
    /// Scalar value with a friendly-name popup: type a number freely OR filter a
    /// candidate list and pick one. The form shows `<value> (<name>)` by resolving
    /// `store == value` against the candidate. `store` is required (it is both the
    /// stored scalar and the reverse-lookup match key). `label` is the candidate's
    /// display template; defaults to the candidate profile's `label`, else `{cn}`.
    Lookup {
        candidate: CandidateRef,
        store: String,
        #[serde(default)]
        label: Option<String>,
    },
    /// Display-only; the attribute is excluded from the changeset.
    Readonly,
    /// OpenLDAP X-ORDERED attribute: strips/regenerates `{n}` ordering prefixes.
    #[serde(rename = "x_ordered")]
    XOrdered,
    /// Generates the Samba SID from `uidNumber` + domain SID when Samba is
    /// configured. Has no effect when no Samba domain is available.
    #[serde(rename = "samba_sid")]
    SambaSid,
}

/// A minimal entry profile (M3 slice). Richer metadata (password/membership/
/// Samba/labels/search_attributes) arrives in M4.
#[derive(Debug, Deserialize, Default, Clone)]
pub struct EntryProfile {
    pub name: String,
    /// One or more object classes for this profile. A single `object_class` key
    /// (old String form) is a hard parse error — use `object_classes = ["..."]`.
    pub object_classes: Vec<String>,
    #[serde(default)]
    pub rdn_attr: String,
    #[serde(default)]
    pub search_base: String,
    #[serde(default)]
    pub show: Vec<String>,
    /// Attributes the picker substring-search matches on. Falls back to `show`,
    /// then to `["cn"]` (see [`EntryProfile::search_attributes`]).
    #[serde(default)]
    pub search_attrs: Vec<String>,
    /// Per-attribute default values for newly-created entries (`[profile.defaults]`).
    #[serde(default)]
    pub defaults: ProfileDefaults,
    /// Per-attribute rich-widget bindings (`[profile.widget.<attr>]`).
    #[serde(default, rename = "widget")]
    pub widgets: std::collections::BTreeMap<String, WidgetSpecCfg>,
    /// Optional display-label template (`label = "{cn} ({uid})"`). When set, the
    /// membership picker renders entries of this profile via the template; `None`
    /// keeps the default behavior. The raw string is parsed into segments in
    /// [`crate::config::relation::CandidateScope`].
    #[serde(default)]
    pub label: Option<String>,
    /// Optional companion entry created atomically with the primary (`[profile.companion]`).
    #[serde(default)]
    pub companion: Option<CompanionSpec>,
    /// Container scope for `New`; never read from TOML.
    #[serde(skip)]
    pub scope: ContainerScope,
}

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

impl EntryProfile {
    /// Effective search attributes: `search_attrs`, else `show`, else `["cn"]`.
    pub fn search_attributes(&self) -> Vec<String> {
        if !self.search_attrs.is_empty() {
            self.search_attrs.clone()
        } else if !self.show.is_empty() {
            self.show.clone()
        } else {
            vec!["cn".to_string()]
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    pub uri: String,
    pub base_dn: String,
    #[serde(default)]
    pub start_tls: bool,
    /// Global read-only mode. When true (or when the bind is anonymous), the TUI
    /// hides Save/Cancel and create/delete actions (spec §5.8).
    #[serde(default)]
    pub read_only: bool,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub tls: TlsConfig,
}

fn default_timeout() -> u64 {
    10
}

#[derive(Debug, Deserialize)]
pub struct TlsConfig {
    #[serde(default)]
    pub ca_cert: Option<PathBuf>,
    #[serde(default)]
    pub client_cert: Option<PathBuf>,
    #[serde(default)]
    pub client_key: Option<PathBuf>,
    #[serde(default = "default_true")]
    pub verify: bool,
}

fn default_true() -> bool {
    true
}

// Manual Default so an absent [server.tls] table yields verify = true.
impl Default for TlsConfig {
    fn default() -> Self {
        TlsConfig {
            ca_cert: None,
            client_cert: None,
            client_key: None,
            verify: true,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct AuthConfig {
    #[serde(default)]
    pub method: AuthMethod,
    #[serde(default)]
    pub bind_dn: Option<String>,
    #[serde(default)]
    pub password_source: PasswordSource,
}

#[derive(Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthMethod {
    #[default]
    Simple,
    External,
    Gssapi,
}

impl AuthConfig {
    /// True when the auth is an anonymous simple bind (no bind DN, no SASL method).
    /// SASL methods (External, Gssapi) are never anonymous — the identity comes
    /// from the transport credential, not a bind DN.
    pub fn is_anonymous(&self) -> bool {
        self.method == AuthMethod::Simple
            && self
                .bind_dn
                .as_deref()
                .map(str::trim)
                .unwrap_or("")
                .is_empty()
    }

    /// True when the auth method requires a password to be resolved at startup.
    pub fn needs_password(&self) -> bool {
        self.method == AuthMethod::Simple
    }
}

/// Validate every profile's optional `[profile.companion]`. Each declared companion
/// must have non-empty `object_classes`, `rdn_attr`, and `search_base`; `rdn_attr`
/// must appear as an `attributes` key (so the RDN has a value source); and every
/// attribute value must parse as a literal / `{attr}` template — a `{next:…}`
/// autonumber is rejected (companions carry no independent allocation).
fn validate_companions(overrides: &[ProfileOverride]) -> Result<()> {
    for o in overrides {
        if let Some(c) = &o.companion {
            check_companion(&format!("profile '{}' companion", o.name), c)
                .map_err(|e| anyhow::anyhow!(e))?;
        }
    }
    Ok(())
}

/// Offline companion checks (see `validate_companions`). `who` prefixes messages.
pub fn check_companion(who: &str, c: &CompanionSpec) -> Result<(), String> {
    use crate::config::defaults::{parse_default_value, DefaultValue};
    if c.object_classes.is_empty() {
        return Err(format!("{who}: object_classes must not be empty"));
    }
    if c.rdn_attr.trim().is_empty() {
        return Err(format!("{who}: rdn_attr must not be empty"));
    }
    if c.search_base.trim().is_empty() {
        return Err(format!("{who}: search_base must not be empty"));
    }
    if !c
        .attributes
        .keys()
        .any(|k| k.eq_ignore_ascii_case(&c.rdn_attr))
    {
        return Err(format!(
            "{who}: rdn_attr '{}' must be one of the companion attributes",
            c.rdn_attr
        ));
    }
    for (attr, tmpl) in &c.attributes {
        if let DefaultValue::AutoNumber { .. } =
            parse_default_value(tmpl).map_err(|e| format!("{who} attribute '{attr}': {e}"))?
        {
            return Err(format!(
                "{who} attribute '{attr}': {{next:…}} autonumber is not supported for companions"
            ));
        }
    }
    Ok(())
}

impl Config {
    pub fn load(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        Self::from_toml_str(&text, &path.display().to_string())
    }

    /// Parse and check config text; `origin` names it in a parse error.
    pub fn from_toml_str(text: &str, origin: &str) -> Result<Config> {
        let config: Config =
            toml::from_str(text).with_context(|| format!("parsing config {origin}"))?;
        validate_companions(&config.overrides)?;
        Ok(config)
    }

    /// Global read-only: the explicit flag OR an anonymous bind (spec §5.8).
    pub fn is_read_only(&self) -> bool {
        self.server.read_only || self.auth.is_anonymous()
    }

    /// Whether the LDAP connection is safe for sending passwords in clear
    /// (LDAPS, StartTLS, or a local Unix-domain socket `ldapi://`).
    /// `userPassword` is sent in cleartext for the server to hash, so we
    /// refuse the operation unless the channel cannot be intercepted.
    pub fn is_encrypted(&self) -> bool {
        let uri = self.server.uri.to_ascii_lowercase();
        self.server.start_tls || uri.starts_with("ldaps://") || uri.starts_with("ldapi://")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONN: &str =
        "[server]\nuri = \"ldap://x\"\nbase_dn = \"dc=x\"\n[auth]\nbind_dn = \"cn=a,dc=x\"\n";

    #[test]
    fn override_with_only_name_and_enabled_parses() {
        let cfg: Config = toml::from_str(&format!(
            "{CONN}[[profile]]\nname = \"user-people\"\nenabled = false\n"
        ))
        .unwrap();
        assert!(cfg.profiles.is_empty());
        assert_eq!(cfg.overrides.len(), 1);
        assert_eq!(cfg.overrides[0].enabled, Some(false));
        assert!(cfg.detect.enabled);
    }

    #[test]
    fn detection_off_requires_object_classes() {
        let err = toml::from_str::<Config>(&format!(
            "{CONN}[detect]\nenabled = false\n[[profile]]\nname = \"user\"\nsuppress = [\"companion\"]\n"
        ))
        .unwrap_err();
        assert!(err.to_string().contains("object_classes"), "{err}");
    }

    #[test]
    fn config_profiles_keep_file_order_and_boundary_scope() {
        let cfg: Config = toml::from_str(&format!(
            "{CONN}[[profile]]\nname = \"a\"\nobject_classes = [\"x\"]\n[[profile]]\nname = \"b\"\n[[profile]]\nname = \"c\"\nobject_classes = [\"y\"]\nenabled = false\n"
        ))
        .unwrap();
        let names: Vec<&str> = cfg.profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["a"]);
        assert_eq!(cfg.profiles[0].scope, ContainerScope::Boundary);
        assert_eq!(cfg.overrides.len(), 3);
    }

    #[test]
    fn parses_minimal_config() {
        let toml = r#"
            [server]
            uri = "ldaps://ldap.example.com:636"
            base_dn = "dc=example,dc=com"

            [auth]
            method = "simple"
            bind_dn = "cn=ldapmanager,dc=example,dc=com"
            password_source = "prompt"
        "#;
        let cfg: Config = toml::from_str(toml).expect("should parse");
        assert_eq!(cfg.server.uri, "ldaps://ldap.example.com:636");
        assert_eq!(cfg.server.base_dn, "dc=example,dc=com");
        assert_eq!(cfg.server.timeout_secs, 10); // default
        assert!(cfg.server.tls.verify); // default true
        assert_eq!(cfg.auth.method, AuthMethod::Simple);
        assert_eq!(
            cfg.auth.bind_dn.as_deref(),
            Some("cn=ldapmanager,dc=example,dc=com")
        );
    }

    #[test]
    fn tls_defaults_to_verify_true_when_table_absent() {
        let toml = r#"
            [server]
            uri = "ldap://ldap.example.com:389"
            base_dn = "dc=example,dc=com"
            [auth]
            bind_dn = "cn=admin,dc=example,dc=com"
        "#;
        let cfg: Config = toml::from_str(toml).expect("should parse");
        assert!(cfg.server.tls.verify);
        assert!(!cfg.server.start_tls); // default false
        assert_eq!(cfg.auth.method, AuthMethod::Simple); // default
    }

    #[test]
    fn parses_profiles() {
        let toml = r#"
            [server]
            uri = "ldap://ldap.example.com:389"
            base_dn = "dc=example,dc=com"
            [auth]
            bind_dn = "cn=admin,dc=example,dc=com"

            [[profile]]
            name = "Users"
            object_classes = ["inetOrgPerson"]
            rdn_attr = "uid"
            search_base = "ou=people,dc=example,dc=com"
            show = ["uid", "cn", "mail"]

            [[profile]]
            name = "Groups"
            object_classes = ["groupOfNames"]
        "#;
        let cfg: Config = toml::from_str(toml).expect("should parse");
        assert_eq!(cfg.profiles.len(), 2);
        assert_eq!(cfg.profiles[0].name, "Users");
        assert_eq!(cfg.profiles[0].object_classes, vec!["inetOrgPerson"]);
        assert_eq!(cfg.profiles[0].rdn_attr, "uid");
        assert_eq!(cfg.profiles[0].show, vec!["uid", "cn", "mail"]);
        assert_eq!(cfg.profiles[1].name, "Groups");
        assert_eq!(cfg.profiles[1].object_classes, vec!["groupOfNames"]);
    }

    #[test]
    fn parses_object_classes_list() {
        let toml = r#"
            [server]
            uri = "ldap://x"
            base_dn = "dc=example,dc=org"
            [auth]
            bind_dn = "cn=admin,dc=example,dc=org"
            [[profile]]
            name = "user"
            object_classes = ["inetOrgPerson", "posixAccount", "shadowAccount"]
            rdn_attr = "uid"
        "#;
        let cfg: Config = toml::from_str(toml).expect("parse");
        assert_eq!(
            cfg.profiles[0].object_classes,
            vec!["inetOrgPerson", "posixAccount", "shadowAccount"]
        );
    }

    #[test]
    fn single_string_object_class_is_a_parse_error() {
        let toml = r#"
            [server]
            uri = "ldap://x"
            base_dn = "dc=x"
            [auth]
            bind_dn = "cn=a,dc=x"
            [[profile]]
            name = "user"
            object_class = "inetOrgPerson"
        "#;
        assert!(toml::from_str::<Config>(toml).is_err());
    }

    #[test]
    fn config_without_profiles_still_parses() {
        let toml = r#"
            [server]
            uri = "ldap://ldap.example.com:389"
            base_dn = "dc=example,dc=com"
            [auth]
            bind_dn = "cn=admin,dc=example,dc=com"
        "#;
        let cfg: Config = toml::from_str(toml).expect("should parse");
        assert!(cfg.profiles.is_empty());
    }

    #[test]
    fn config_without_samba_table_defaults_rid_base_1000() {
        let toml = r#"
            [server]
            uri = "ldap://ldap.example.com:389"
            base_dn = "dc=example,dc=com"
            [auth]
            bind_dn = "cn=admin,dc=example,dc=com"
        "#;
        let cfg: Config = toml::from_str(toml).expect("should parse");
        assert_eq!(cfg.samba.algorithmic_rid_base, 1000);
        assert!(cfg.samba.domain_sid.is_none());
    }

    #[test]
    fn config_with_samba_table_parses() {
        let toml = r#"
            [server]
            uri = "ldap://ldap.example.com:389"
            base_dn = "dc=example,dc=com"
            [auth]
            bind_dn = "cn=admin,dc=example,dc=com"

            [samba]
            domain_sid = "S-1-5-21-1-2-3"
            algorithmic_rid_base = 2000
        "#;
        let cfg: Config = toml::from_str(toml).expect("should parse");
        assert_eq!(cfg.samba.domain_sid.as_deref(), Some("S-1-5-21-1-2-3"));
        assert_eq!(cfg.samba.algorithmic_rid_base, 2000);
    }

    #[test]
    fn missing_uri_is_an_error() {
        let toml = r#"
            [server]
            base_dn = "dc=example,dc=com"
            [auth]
            bind_dn = "cn=admin,dc=example,dc=com"
        "#;
        assert!(toml::from_str::<Config>(toml).is_err());
    }

    #[test]
    fn read_only_flag_forces_read_only() {
        let toml = r#"
            [server]
            uri = "ldap://x"
            base_dn = "dc=x"
            read_only = true
            [auth]
            bind_dn = "cn=admin,dc=x"
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert!(cfg.is_read_only());
    }

    #[test]
    fn anonymous_bind_is_read_only() {
        let toml = r#"
            [server]
            uri = "ldap://x"
            base_dn = "dc=x"
            [auth]
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert!(cfg.auth.is_anonymous());
        assert!(cfg.is_read_only());
    }

    #[test]
    fn external_auth_without_bind_dn_is_not_anonymous() {
        let toml = r#"
            [server]
            uri = "ldapi://%2Fvar%2Frun%2Fslapd%2Fldapi"
            base_dn = "dc=x"
            [auth]
            method = "external"
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert!(!cfg.auth.is_anonymous());
        assert!(!cfg.is_read_only());
    }

    #[test]
    fn demo_config_parses_widget_pickers() {
        let toml = include_str!("../../examples/demo-config.toml");
        let cfg: Config = toml::from_str(toml).expect("demo config parses");
        let user = cfg
            .profiles
            .iter()
            .find(|p| p.name == "user")
            .expect("user profile");
        // memberOf migrated to a membership widget fanning out via `member`.
        match &user.widgets["memberOf"] {
            WidgetSpecCfg::Membership { via, .. } => assert_eq!(via, "member"),
            other => panic!("expected Membership for memberOf, got {other:?}"),
        }
        // gidNumber migrated to a lookup widget.
        assert!(
            matches!(&user.widgets["gidNumber"], WidgetSpecCfg::Lookup { .. }),
            "expected Lookup for gidNumber"
        );
    }

    #[test]
    fn lookup_widget_parses_with_candidate_store_and_label() {
        let toml = r#"
            [server]
            uri = "ldap://x"
            base_dn = "dc=x"
            [auth]
            bind_dn = "cn=admin,dc=x"

            [[profile]]
            name = "user"
            object_classes = ["posixAccount"]

            [profile.widget.gidNumber]
            kind = "lookup"
            candidate = "posixgroup"
            store = "gidNumber"
            label = "{cn}"
        "#;
        let cfg: super::Config = toml::from_str(toml).expect("parse");
        let user = &cfg.profiles[0];
        match &user.widgets["gidNumber"] {
            WidgetSpecCfg::Lookup {
                candidate,
                store,
                label,
            } => {
                assert!(matches!(candidate, CandidateRef::Profile(n) if n == "posixgroup"));
                assert_eq!(store, "gidNumber");
                assert_eq!(label.as_deref(), Some("{cn}"));
            }
            other => panic!("expected Lookup, got {other:?}"),
        }
    }

    #[test]
    fn bound_writable_is_not_read_only() {
        let toml = r#"
            [server]
            uri = "ldap://x"
            base_dn = "dc=x"
            [auth]
            bind_dn = "cn=admin,dc=x"
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert!(!cfg.is_read_only());
    }

    #[test]
    fn search_attributes_falls_back_to_show_then_cn() {
        let p = EntryProfile {
            name: "u".into(),
            object_classes: vec!["inetOrgPerson".into()],
            rdn_attr: "uid".into(),
            search_base: "ou=people".into(),
            show: vec!["uid".into(), "cn".into()],
            search_attrs: vec![],
            defaults: Default::default(),
            widgets: Default::default(),
            label: None,
            companion: None,
            scope: Default::default(),
        };
        assert_eq!(
            p.search_attributes(),
            vec!["uid".to_string(), "cn".to_string()]
        );

        let p2 = EntryProfile {
            search_attrs: vec!["mail".into()],
            ..p.clone()
        };
        assert_eq!(p2.search_attributes(), vec!["mail".to_string()]);

        let p3 = EntryProfile {
            show: vec![],
            search_attrs: vec![],
            ..p
        };
        assert_eq!(p3.search_attributes(), vec!["cn".to_string()]);
    }

    #[test]
    fn parses_profile_defaults_block() {
        use crate::config::defaults::DefaultValue;
        let toml = r#"
            [server]
            uri = "ldap://x"
            base_dn = "dc=example,dc=org"
            [auth]
            bind_dn = "cn=admin,dc=example,dc=org"
            [[profile]]
            name = "user"
            object_classes = ["inetOrgPerson", "posixAccount"]
            rdn_attr = "uid"
            [profile.defaults]
            loginShell = "/bin/bash"
            homeDirectory = "/home/{uid}"
            uidNumber = "{next:10000-60000}"
        "#;
        let cfg: Config = toml::from_str(toml).expect("parse");
        let d = &cfg.profiles[0].defaults;
        assert!(matches!(
            d.entries.get("loginShell"),
            Some(DefaultValue::Literal(_))
        ));
        assert!(matches!(
            d.entries.get("uidNumber"),
            Some(DefaultValue::AutoNumber { .. })
        ));
    }

    #[test]
    fn bad_default_value_fails_whole_config_parse() {
        // An invalid autonumber range must propagate to a Config parse error.
        let toml = r#"
            [server]
            uri = "ldap://x"
            base_dn = "dc=example,dc=org"
            [auth]
            bind_dn = "cn=admin,dc=example,dc=org"
            [[profile]]
            name = "user"
            object_classes = ["inetOrgPerson"]
            rdn_attr = "uid"
            [profile.defaults]
            uidNumber = "{next:60000-10000}"
        "#;
        assert!(toml::from_str::<Config>(toml).is_err());
    }

    #[test]
    fn parses_profile_label_template() {
        let toml = r#"
            [server]
            uri = "ldap://x"
            base_dn = "dc=example,dc=org"
            [auth]
            bind_dn = "cn=admin,dc=example,dc=org"
            [[profile]]
            name = "user"
            object_classes = ["inetOrgPerson"]
            rdn_attr = "uid"
            label = "{cn} ({uid})"
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert_eq!(cfg.profiles[0].label.as_deref(), Some("{cn} ({uid})"));
    }

    #[test]
    fn profile_without_label_is_none() {
        let toml = r#"
            [server]
            uri = "ldap://x"
            base_dn = "dc=example,dc=org"
            [auth]
            bind_dn = "cn=admin,dc=example,dc=org"
            [[profile]]
            name = "user"
            object_classes = ["inetOrgPerson"]
            rdn_attr = "uid"
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        assert!(cfg.profiles[0].label.is_none());
    }

    #[test]
    fn parses_tree_label_rules() {
        let toml = r#"
            [server]
            uri = "ldap://ldap.example.com:389"
            base_dn = "dc=example,dc=com"
            [auth]
            bind_dn = "cn=admin,dc=example,dc=com"

            [[tree.label]]
            when     = ["description"]
            template = "{rdn} ({description})"

            [[tree.label]]
            template = "{rdn}"
        "#;
        let cfg: Config = toml::from_str(toml).expect("parses");
        assert_eq!(cfg.tree.label.len(), 2);
        assert_eq!(cfg.tree.label[0].when, vec!["description".to_string()]);
        assert_eq!(cfg.tree.label[0].template, "{rdn} ({description})");
        assert!(cfg.tree.label[1].when.is_empty());
        assert_eq!(cfg.tree.label[1].template, "{rdn}");
    }

    #[test]
    fn tree_when_defaults_to_empty() {
        let toml = r#"
            [server]
            uri = "ldap://ldap.example.com:389"
            base_dn = "dc=example,dc=com"
            [auth]
            bind_dn = "cn=admin,dc=example,dc=com"

            [[tree.label]]
            template = "{rdn}"
        "#;
        let cfg: Config = toml::from_str(toml).expect("parses");
        assert!(cfg.tree.label[0].when.is_empty());
    }

    #[test]
    fn tree_label_without_template_is_a_parse_error() {
        let toml = r#"
            [server]
            uri = "ldap://ldap.example.com:389"
            base_dn = "dc=example,dc=com"
            [auth]
            bind_dn = "cn=admin,dc=example,dc=com"

            [[tree.label]]
            when = ["cn"]
        "#;
        assert!(toml::from_str::<Config>(toml).is_err());
    }

    #[test]
    fn config_without_tree_table_has_empty_label_list() {
        let toml = r#"
            [server]
            uri = "ldap://ldap.example.com:389"
            base_dn = "dc=example,dc=com"
            [auth]
            bind_dn = "cn=admin,dc=example,dc=com"
        "#;
        let cfg: Config = toml::from_str(toml).expect("parses");
        assert!(cfg.tree.label.is_empty());
    }

    #[test]
    fn parses_profile_widget_table() {
        let toml = r#"
[server]
uri = "ldap://x"
base_dn = "dc=x"
[auth]

[[profile]]
name = "user"
object_classes = ["inetOrgPerson"]

[profile.widget.sambaAcctFlags]
kind = "choice"
select = "multi"
format = "bracketed"
options = [ { value = "D", label = "Disabled" }, { value = "X", label = "No expire" } ]

[profile.widget.loginShell]
kind = "choice"
select = "single"
format = "plain"
options = [ { value = "/bin/bash", label = "Bash" } ]
"#;
        let cfg: Config = toml::from_str(toml).expect("parse");
        let p = &cfg.profiles[0];
        assert_eq!(p.widgets.len(), 2);
        let WidgetSpecCfg::Choice {
            select,
            format,
            options,
        } = &p.widgets["sambaAcctFlags"]
        else {
            panic!("expected Choice widget for sambaAcctFlags");
        };
        assert_eq!(select, "multi");
        assert_eq!(format, "bracketed");
        assert_eq!(options[0].value, "D");
        assert_eq!(options[0].label, "Disabled");
    }

    #[test]
    fn demo_config_widgets_resolve() {
        let toml = include_str!("../../examples/demo-config.toml");
        let cfg: Config = toml::from_str(toml).expect("demo-config.toml parses");
        let widgets = crate::config::widget::resolve_widgets(&cfg.profiles)
            .expect("demo-config widgets resolve");
        // sambaAcctFlags + loginShell choice widgets are present
        assert!(widgets
            .iter()
            .any(|w| w.attr.eq_ignore_ascii_case("sambaAcctFlags")));
        assert!(widgets
            .iter()
            .any(|w| w.attr.eq_ignore_ascii_case("loginShell")));
        // userPassword password widget is present (migrated from [profile.password])
        assert!(
            widgets
                .iter()
                .any(|w| matches!(&w.kind, crate::config::widget::WidgetKind::Password(_))),
            "expected a WidgetKind::Password in demo-config widgets"
        );
        // memberOf resolves to a membership picker fanning out via `member`.
        let mof = widgets
            .iter()
            .find(|w| w.attr.eq_ignore_ascii_case("memberOf"))
            .expect("memberOf widget");
        match &mof.kind {
            crate::config::widget::WidgetKind::Picker(b) => {
                assert_eq!(b.fanout_attr.as_deref(), Some("member"))
            }
            other => panic!("expected Picker for memberOf, got {other:?}"),
        }
        // gidNumber resolves to a lookup widget (shows friendly name in the form).
        let gid = widgets
            .iter()
            .find(|w| w.attr.eq_ignore_ascii_case("gidNumber"))
            .expect("gidNumber widget");
        assert!(
            matches!(&gid.kind, crate::config::widget::WidgetKind::Lookup(_)),
            "expected Lookup for gidNumber, got {:?}",
            gid.kind
        );
    }

    #[test]
    fn reference_config_parses() {
        let toml = include_str!("../../examples/config.toml");
        let cfg: Config = toml::from_str(toml).expect("examples/config.toml parses");
        let widgets = crate::config::widget::resolve_widgets(&cfg.profiles)
            .expect("examples/config.toml widgets resolve");
        // userPassword password widget is present in the reference config too
        assert!(
            widgets
                .iter()
                .any(|w| matches!(&w.kind, crate::config::widget::WidgetKind::Password(_))),
            "expected a WidgetKind::Password in reference config widgets"
        );
    }

    #[test]
    fn is_encrypted_reflects_ldaps_or_starttls() {
        let mk = |uri: &str, start_tls: &str| -> Config {
            let toml = format!(
                r#"
                [server]
                uri = "{uri}"
                base_dn = "dc=x"
                start_tls = {start_tls}
                [auth]
                bind_dn = "cn=admin,dc=x"
                "#
            );
            toml::from_str(&toml).expect("parse")
        };
        assert!(mk("ldaps://h:636", "false").is_encrypted());
        assert!(mk("ldap://h:389", "true").is_encrypted());
        assert!(mk("LDAPS://H", "false").is_encrypted()); // case-insensitive
        assert!(mk("ldapi:///run/slapd/ldapi", "false").is_encrypted()); // unix socket
        assert!(mk("LDAPI:///var/run/ldapi", "false").is_encrypted()); // case-insensitive
        assert!(!mk("ldap://h:389", "false").is_encrypted());
    }

    #[test]
    fn parses_password_widget() {
        let toml = r#"
[server]
uri = "ldaps://x"
base_dn = "dc=x"
[auth]

[[profile]]
name = "user"
object_classes = ["inetOrgPerson"]

[profile.widget.userPassword]
kind = "password"
samba = true
"#;
        let cfg: Config = toml::from_str(toml).expect("parse");
        let p = &cfg.profiles[0];
        match &p.widgets["userPassword"] {
            WidgetSpecCfg::Password { samba } => assert!(*samba),
            other => panic!("expected password, got {other:?}"),
        }
    }

    #[test]
    fn widget_picker_parses_inline_candidate_scope() {
        // The risky path: an untagged CandidateRef (inline table) nested in the
        // internally-tagged WidgetSpecCfg. Must parse to Inline, not error.
        let toml = r#"
        [server]
        uri = "ldap://x"
        base_dn = "dc=x"
        [auth]

        [[profile]]
        name = "user"
        object_classes = ["inetOrgPerson"]

        [profile.widget.secretary]
        kind = "picker"
        store = "dn"
        select = "single"
        candidate = { base = "ou=people,dc=example,dc=org", object_classes = ["inetOrgPerson"], search_attrs = ["cn", "uid"], label = "{cn} ({uid})" }
    "#;
        let cfg: Config = toml::from_str(toml).expect("parses inline candidate scope");
        let spec = &cfg.profiles[0].widgets["secretary"];
        match spec {
            WidgetSpecCfg::Picker {
                candidate,
                store,
                select,
            } => {
                assert_eq!(store, "dn");
                assert_eq!(select, "single");
                match candidate {
                    CandidateRef::Inline(s) => {
                        assert_eq!(s.base, "ou=people,dc=example,dc=org");
                        assert_eq!(s.object_classes, vec!["inetOrgPerson".to_string()]);
                        assert_eq!(s.search_attrs, vec!["cn".to_string(), "uid".to_string()]);
                        assert_eq!(s.label.as_deref(), Some("{cn} ({uid})"));
                    }
                    other => panic!("expected inline scope, got {other:?}"),
                }
            }
            other => panic!("expected Picker variant, got {other:?}"),
        }
    }

    #[test]
    fn widget_picker_and_membership_parse_profile_ref_candidate() {
        let toml = r#"
        [server]
        uri = "ldap://x"
        base_dn = "dc=x"
        [auth]

        [[profile]]
        name = "user"
        object_classes = ["inetOrgPerson"]

        [profile.widget.gidNumber]
        kind = "picker"
        candidate = "posixgroup"
        store = "gidNumber"
        select = "single"

        [profile.widget.memberOf]
        kind = "membership"
        candidate = "group"
        via = "member"
    "#;
        let cfg: Config = toml::from_str(toml).expect("parses");
        let w = &cfg.profiles[0].widgets;
        match &w["gidNumber"] {
            WidgetSpecCfg::Picker {
                candidate,
                store,
                select,
            } => {
                assert_eq!(candidate, &CandidateRef::Profile("posixgroup".into()));
                assert_eq!(store, "gidNumber");
                assert_eq!(select, "single");
            }
            other => panic!("expected Picker, got {other:?}"),
        }
        match &w["memberOf"] {
            WidgetSpecCfg::Membership { candidate, via } => {
                assert_eq!(candidate, &CandidateRef::Profile("group".into()));
                assert_eq!(via, "member");
            }
            other => panic!("expected Membership, got {other:?}"),
        }
    }

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
        let m: std::collections::HashMap<String, WidgetSpecCfg> = toml::from_str(s).unwrap();
        assert!(matches!(m["a"], WidgetSpecCfg::Readonly));
        assert!(matches!(m["b"], WidgetSpecCfg::XOrdered));
        assert!(matches!(m["c"], WidgetSpecCfg::SambaSid));
    }

    #[test]
    fn widget_picker_store_and_select_default() {
        let toml = r#"
        [server]
        uri = "ldap://x"
        base_dn = "dc=x"
        [auth]

        [[profile]]
        name = "user"
        object_classes = ["inetOrgPerson"]

        [profile.widget.member]
        kind = "picker"
        candidate = "user"
    "#;
        let cfg: Config = toml::from_str(toml).expect("parses");
        match &cfg.profiles[0].widgets["member"] {
            WidgetSpecCfg::Picker { store, select, .. } => {
                assert_eq!(store, "dn"); // default_store
                assert_eq!(select, "auto"); // default_select
            }
            other => panic!("expected Picker, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod meta_tests {
    use super::*;

    #[test]
    fn meta_config_parses_both_fields() {
        let cfg: Config = toml::from_str(
            r#"
            [meta]
            name        = "example-corp production"
            description = "dc=example,dc=com via ldapi"
            [server]
            uri     = "ldap://x"
            base_dn = "dc=x"
            [auth]
            method  = "simple"
            bind_dn = "cn=admin,dc=x"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.meta.name.as_deref(), Some("example-corp production"));
        assert_eq!(
            cfg.meta.description.as_deref(),
            Some("dc=example,dc=com via ldapi")
        );
    }

    #[test]
    fn meta_config_absent_gives_none_fields() {
        let cfg: Config = toml::from_str(
            r#"
            [server]
            uri     = "ldap://x"
            base_dn = "dc=x"
            [auth]
            method  = "simple"
            bind_dn = "cn=admin,dc=x"
            "#,
        )
        .unwrap();
        assert!(cfg.meta.name.is_none());
        assert!(cfg.meta.description.is_none());
    }

    #[test]
    fn meta_config_partial_fields_allowed() {
        let cfg: Config = toml::from_str(
            r#"
            [meta]
            name = "only a name"
            [server]
            uri     = "ldap://x"
            base_dn = "dc=x"
            [auth]
            method  = "simple"
            bind_dn = "cn=admin,dc=x"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.meta.name.as_deref(), Some("only a name"));
        assert!(cfg.meta.description.is_none());
    }

    fn parse_config_str(toml: &str) -> anyhow::Result<Config> {
        Config::from_toml_str(toml, "test")
    }

    #[test]
    fn check_rejects_a_next_autonumber_in_an_override_only_companion() {
        const CONN: &str =
            "[server]\nuri = \"ldap://x\"\nbase_dn = \"dc=x\"\n[auth]\nbind_dn = \"cn=a,dc=x\"\n";
        let toml = format!(
            "{CONN}[[profile]]\nname = \"user-people\"\n[profile.companion]\nobject_classes = [\"posixGroup\"]\nrdn_attr = \"cn\"\nsearch_base = \"ou=g,dc=x\"\n[profile.companion.attributes]\ncn = \"{{uid}}\"\ngidNumber = \"{{next:1-9}}\"\n"
        );
        let err = parse_config_str(&toml).unwrap_err();
        assert!(err.to_string().contains("autonumber"), "{err}");
    }

    #[test]
    fn parses_companion_block() {
        let toml = r#"
            [server]
            uri = "ldap://x"
            base_dn = "dc=x"
            [auth]
            bind_dn = "cn=admin,dc=x"
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
        let c = cfg.profiles[0]
            .companion
            .as_ref()
            .expect("companion present");
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
            [auth]
            bind_dn = "cn=admin,dc=x"
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
        assert!(
            err.contains("cn"),
            "error should name the missing rdn attribute: {err}"
        );
    }

    #[test]
    fn companion_with_empty_object_classes_is_rejected() {
        let toml = r#"
            [server]
            uri = "ldap://x"
            base_dn = "dc=x"
            [auth]
            bind_dn = "cn=admin,dc=x"
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
            [auth]
            bind_dn = "cn=admin,dc=x"
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
        assert!(
            err.to_lowercase().contains("next") || err.contains("gidNumber"),
            "autonumber in a companion must be rejected: {err}"
        );
    }
}
