//! Detection data model: what the sampler hands the detector, and what the
//! detector hands the merge. Pure data; no LDAP, no UI.

use std::collections::{BTreeMap, BTreeSet};

use crate::config::defaults::DefaultValue;
use crate::config::{CompanionSpec, WidgetSpecCfg};

/// Why a detected value was chosen: how many entries followed the rule, out of
/// how many, which entries broke it, and an optional free-text note.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Evidence {
    pub matched: usize,
    pub sampled: usize,
    pub exceptions: Vec<String>,
    pub note: Option<String>,
}

impl Evidence {
    pub fn new(matched: usize, sampled: usize) -> Self {
        Evidence {
            matched,
            sampled,
            exceptions: Vec::new(),
            note: None,
        }
    }

    pub fn with_exceptions(mut self, exceptions: Vec<String>) -> Self {
        self.exceptions = exceptions;
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// `"12/12"`.
    pub fn ratio(&self) -> String {
        format!("{}/{}", self.matched, self.sampled)
    }
}

/// A detected value with its evidence.
#[derive(Debug, Clone, PartialEq)]
pub struct Detected<T> {
    pub value: T,
    pub evidence: Evidence,
}

impl<T> Detected<T> {
    pub fn new(value: T, evidence: Evidence) -> Self {
        Detected { value, evidence }
    }
}

/// One sampled entry: DN plus string attribute values (as the server spelled
/// the attribute names). All accessors are case-insensitive.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SampleEntry {
    pub dn: String,
    pub attrs: BTreeMap<String, Vec<String>>,
}

impl SampleEntry {
    /// All values of `attr` (case-insensitive name), or an empty slice.
    pub fn values(&self, attr: &str) -> &[String] {
        self.attrs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(attr))
            .map(|(_, v)| v.as_slice())
            .unwrap_or(&[])
    }

    /// The first non-blank value of `attr`, trimmed.
    pub fn first(&self, attr: &str) -> Option<&str> {
        self.values(attr)
            .iter()
            .map(|v| v.trim())
            .find(|v| !v.is_empty())
    }

    /// Whether the entry lists object class `oc` (case-insensitive).
    pub fn has_class(&self, oc: &str) -> bool {
        self.values("objectClass")
            .iter()
            .any(|c| c.trim().eq_ignore_ascii_case(oc))
    }

    /// The attribute name of the DN's first RDN (`uid` in `uid=a,ou=x`). For a
    /// multi-valued RDN (`cn=a+uid=b`) the first assertion's attribute.
    pub fn rdn_attr(&self) -> Option<&str> {
        let first = crate::detect::dn_components(&self.dn).into_iter().next()?;
        let (attr, _) = first.split_once('=')?;
        let attr = attr.trim();
        (!attr.is_empty()).then_some(attr)
    }

    /// The parent DN, honouring escaped commas.
    pub fn parent(&self) -> Option<&str> {
        crate::detect::parent_dn(&self.dn)
    }
}

impl From<&crate::ldap::worker::LdapEntry> for SampleEntry {
    fn from(e: &crate::ldap::worker::LdapEntry) -> Self {
        SampleEntry {
            dn: e.dn.clone(),
            attrs: e.attrs.clone(),
        }
    }
}

/// The one-level sample of one container.
#[derive(Debug, Clone, Default)]
pub struct ContainerSample {
    pub dn: String,
    pub entries: Vec<SampleEntry>,
    /// Attribute names present per entry (lowercased DN → lowercased names), from
    /// the types-only search. Missing entry → fall back to the sampled values.
    pub present: BTreeMap<String, BTreeSet<String>>,
    /// A client/server size or time limit cut this sample short.
    pub partial: bool,
}

impl ContainerSample {
    /// Lowercased names of the attributes `e` carries.
    pub fn present_attrs(&self, e: &SampleEntry) -> BTreeSet<String> {
        match self.present.get(&e.dn.to_lowercase()) {
            Some(set) => set.clone(),
            None => e
                .attrs
                .iter()
                .filter(|(_, v)| !v.is_empty())
                .map(|(k, _)| k.to_lowercase())
                .collect(),
        }
    }
}

/// Everything the sampler found.
#[derive(Debug, Clone, Default)]
pub struct Sample {
    pub base_dn: String,
    pub containers: Vec<ContainerSample>,
    /// Forward lookup: `posixGroup` entries whose `cn` is a sampled user's `uid`.
    pub groups: Vec<SampleEntry>,
    /// Reverse lookup: `posixAccount` entries whose `uid` is a sampled group's `cn`.
    pub accounts: Vec<SampleEntry>,
    /// Set when a cross-container lookup failed; dependent rules are skipped.
    pub lookup_error: Option<String>,
    /// `ou=groups,<base_dn>` when that entry exists (rule D's fallback companion base).
    pub group_ou: Option<String>,
    pub notes: Vec<String>,
}

/// One detected profile before the merge.
#[derive(Debug, Clone)]
pub struct DetectedProfile {
    /// Final name (`user-people`); empty until `names::assign_names` runs.
    pub name: String,
    /// Pattern name or slugged structural class (`user`, `organizationalunit`).
    pub base_name: String,
    /// The group's structural class, schema spelling.
    pub structural: String,
    /// The container DN (becomes `search_base`).
    pub container: String,
    /// Sampled entries in this group.
    pub sampled: usize,
    pub partial: bool,
    pub object_classes: Detected<Vec<String>>,
    pub rdn_attr: Detected<String>,
    pub show: Vec<String>,
    pub search_attrs: Vec<String>,
    pub label: Option<String>,
    pub defaults: BTreeMap<String, Detected<DefaultValue>>,
    pub widgets: BTreeMap<String, Detected<WidgetSpecCfg>>,
    pub companion: Option<Detected<CompanionSpec>>,
    /// Rule B2 applied: users here have user-private groups.
    pub private_groups: bool,
    /// posixAccount profiles: how many sampled users have no private group
    /// (§2B2 predicate). `None` = not evaluated (not a user profile, or the
    /// private-group lookup failed). Contrary evidence for rule D.
    pub users_without_private_group: Option<usize>,
    pub notes: Vec<String>,
    /// The sampled entries of this group (input to the pattern rules; never printed).
    pub entries: Vec<SampleEntry>,
}

impl DetectedProfile {
    /// This profile as an `EntryProfile` (detected-only: `Exact` scope).
    pub fn to_entry_profile(&self) -> crate::config::EntryProfile {
        crate::config::EntryProfile {
            name: self.name.clone(),
            object_classes: self.object_classes.value.clone(),
            rdn_attr: self.rdn_attr.value.clone(),
            search_base: self.container.clone(),
            show: self.show.clone(),
            search_attrs: self.search_attrs.clone(),
            defaults: crate::config::defaults::ProfileDefaults {
                entries: self
                    .defaults
                    .iter()
                    .map(|(k, d)| (k.clone(), d.value.clone()))
                    .collect(),
            },
            widgets: self
                .widgets
                .iter()
                .map(|(k, d)| (k.clone(), d.value.clone()))
                .collect(),
            label: self.label.clone(),
            companion: self.companion.as_ref().map(|c| c.value.clone()),
            scope: crate::config::ContainerScope::Exact,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> SampleEntry {
        let mut attrs = BTreeMap::new();
        attrs.insert("objectclass".to_string(), vec!["posixaccount".to_string()]);
        attrs.insert("UIDNumber".to_string(), vec![" 5000 ".to_string()]);
        attrs.insert("description".to_string(), vec!["  ".to_string()]);
        SampleEntry {
            dn: r"cn=Smith\, John,ou=people,dc=x".to_string(),
            attrs,
        }
    }

    #[test]
    fn accessors_are_case_insensitive_and_trim() {
        let e = entry();
        assert!(e.has_class("posixAccount"));
        assert_eq!(e.first("uidNumber"), Some("5000"));
        assert_eq!(e.first("description"), None);
        assert!(e.values("missing").is_empty());
    }

    #[test]
    fn rdn_and_parent_honour_escaped_commas() {
        let e = entry();
        assert_eq!(e.rdn_attr(), Some("cn"));
        assert_eq!(e.parent(), Some("ou=people,dc=x"));
    }

    #[test]
    fn present_attrs_prefers_the_types_only_map() {
        let e = entry();
        let mut c = ContainerSample::default();
        assert!(c.present_attrs(&e).contains("uidnumber"));
        c.present.insert(
            e.dn.to_lowercase(),
            ["jpegphoto".to_string()].into_iter().collect(),
        );
        assert_eq!(c.present_attrs(&e).len(), 1);
    }
}
