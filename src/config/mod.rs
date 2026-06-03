//! Configuration: connection properties + auth. (Entry profiles arrive in M4.)

pub mod defaults;
pub mod label;
pub mod password;
pub mod relation;
pub use password::PasswordSource;

use crate::config::defaults::ProfileDefaults;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub auth: AuthConfig,
    /// Entry profiles drive the menu and read-form ordering. A minimal slice is
    /// pulled forward into M3 (the rich profile metadata stays in M4). `[[profile]]`
    /// TOML blocks parse here; absent profiles default to empty.
    #[serde(default, rename = "profile")]
    pub profiles: Vec<EntryProfile>,
    /// Samba lifecycle fallback settings. Used only when no `sambaDomain` entry
    /// is discovered in the directory (spec §9). Absent `[samba]` table is fine.
    #[serde(default)]
    pub samba: SambaConfig,
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

/// Password configuration for an entry profile. Controls which LDAP attribute
/// holds the password and whether to maintain Samba NT-hash attributes.
#[derive(Debug, Deserialize, Clone)]
pub struct PasswordSpec {
    /// LDAP attribute to store the password. Defaults to `userPassword`.
    #[serde(default = "default_pw_attr")]
    pub ldap_attribute: String,
    /// When true, also write `sambaNTPassword` and `sambaPwdLastSet` on
    /// create/modify so Samba credentials stay in sync with the Unix password.
    #[serde(default)]
    pub samba: bool,
}

fn default_pw_attr() -> String {
    "userPassword".to_string()
}

fn default_store() -> String {
    "dn".to_string()
}

fn default_select() -> String {
    "auto".to_string()
}

/// Raw `[profile.picker.<attr>]` binding: how an attribute's field is populated
/// from a live candidate search. Resolves (against the profile list) to a
/// [`crate::config::relation::PickerBinding`].
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
pub struct PickerSpec {
    /// `[[profile]]` name supplying the candidate search scope.
    pub candidate: String,
    /// What to store per pick: the sentinel `"dn"` (default) or an attribute name.
    #[serde(default = "default_store")]
    pub store: String,
    /// Cardinality: `"auto"` (from the attribute's schema arity), `"single"`, `"multi"`.
    #[serde(default = "default_select")]
    pub select: String,
    /// Present ⇒ synthetic back-ref: the field is not written to the server; this
    /// entry's DN is added/removed in `fanout_attr` on each picked candidate
    /// (e.g. `memberOf` → write `member` on each picked group).
    #[serde(default)]
    pub fanout_attr: Option<String>,
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
    /// Optional password field configuration. When present, the create/edit form
    /// will show an inline password field with the given attribute and Samba settings.
    #[serde(default)]
    pub password: Option<PasswordSpec>,
    /// Per-attribute picker bindings (`[profile.picker.<attr>]`). Each declares how
    /// the named attribute's field is populated from a candidate search.
    #[serde(default, rename = "picker")]
    pub pickers: std::collections::BTreeMap<String, PickerSpec>,
    /// Optional display-label template (`label = "{cn} ({uid})"`). When set, the
    /// membership picker renders entries of this profile via the template; `None`
    /// keeps the default behavior. The raw string is parsed into segments in
    /// [`crate::config::relation::CandidateScope`].
    #[serde(default)]
    pub label: Option<String>,
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
    /// True when no bind DN is configured (anonymous bind).
    pub fn is_anonymous(&self) -> bool {
        self.bind_dn
            .as_deref()
            .map(str::trim)
            .unwrap_or("")
            .is_empty()
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let config: Config =
            toml::from_str(&text).with_context(|| format!("parsing config {}", path.display()))?;
        Ok(config)
    }

    /// Global read-only: the explicit flag OR an anonymous bind (spec §5.8).
    pub fn is_read_only(&self) -> bool {
        self.server.read_only || self.auth.is_anonymous()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn demo_config_parses_with_pickers() {
        let toml = include_str!("../../examples/demo-config.toml");
        let cfg: Config = toml::from_str(toml).expect("demo config parses");
        let pickers = crate::config::relation::resolve_pickers(&cfg.profiles);
        // member (group) + memberOf, gidNumber (user) + memberUid (posixgroup) = 4.
        assert_eq!(pickers.len(), 4);
        // Spot-check the fan-out and scalar-store bindings resolved correctly.
        let mof = pickers
            .iter()
            .find(|p| p.binding.attr == "memberOf")
            .expect("memberOf picker");
        assert_eq!(mof.binding.fanout_attr.as_deref(), Some("member"));
        let gid = pickers
            .iter()
            .find(|p| p.binding.attr == "gidNumber")
            .expect("gidNumber picker");
        assert_eq!(
            gid.binding.store,
            crate::config::relation::StoreKey::Attr("gidNumber".to_string())
        );
        let muid = pickers
            .iter()
            .find(|p| p.binding.attr == "memberUid")
            .expect("memberUid picker");
        assert_eq!(
            muid.binding.store,
            crate::config::relation::StoreKey::Attr("uid".to_string())
        );
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
            password: None,
            pickers: Default::default(),
            label: None,
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
    fn parses_profile_password_block() {
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
            [profile.password]
            samba = true
        "#;
        let cfg: Config = toml::from_str(toml).unwrap();
        let p = cfg.profiles[0].password.as_ref().unwrap();
        assert_eq!(p.ldap_attribute, "userPassword"); // default
        assert!(p.samba);
    }

    #[test]
    fn profile_without_password_block_is_none() {
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
        assert!(cfg.profiles[0].password.is_none());
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
    fn parses_profile_picker_block() {
        let cfg: Config = toml::from_str(
            r#"
        [server]
        uri = "ldaps://x"
        base_dn = "dc=x"
        [auth]
        [[profile]]
        name = "group"
        object_classes = ["groupOfNames"]
        [profile.picker.member]
        candidate = "user"
        [profile.picker.memberOf]
        candidate = "group"
        store = "dn"
        fanout_attr = "member"
        [profile.picker.gidNumber]
        candidate = "posixgroup"
        store = "gidNumber"
        select = "single"
        "#,
        )
        .unwrap();
        let p = &cfg.profiles[0];
        let member = p.pickers.get("member").expect("member picker");
        assert_eq!(member.candidate, "user");
        assert_eq!(member.store, "dn");
        assert_eq!(member.select, "auto");
        assert_eq!(member.fanout_attr, None);
        let mof = p.pickers.get("memberOf").expect("memberOf picker");
        assert_eq!(mof.fanout_attr.as_deref(), Some("member"));
        assert_eq!(mof.candidate, "group");
        assert_eq!(mof.store, "dn");
        assert_eq!(mof.select, "auto");
        let gid = p.pickers.get("gidNumber").expect("gidNumber picker");
        assert_eq!(gid.store, "gidNumber");
        assert_eq!(gid.select, "single");
        assert_eq!(gid.candidate, "posixgroup");
        assert_eq!(gid.fanout_attr, None);
    }
}
