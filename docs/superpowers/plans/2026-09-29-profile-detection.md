# Profile Detection (config by exception) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** eDAPtor derives its entry profiles from the directory (sampling containers, grouping by structural class, recognising known patterns, detecting number ranges at create time), merges `[[profile]]` blocks over them as overrides, and shows the result with `edaptor profiles`.

**Architecture:** A new `src/detect/` module with three pure stages — *detect* (`infer.rs` + `patterns/*.rs`, fixture-testable), *range* (rule C, run at allocation time), *merge* (`merge.rs`, config over detected, provenance for the dump) — fed by an LDAP *sample* stage (`sample.rs`, behind a `Searcher` trait so it is unit-testable) and glued together by `load.rs::load_profiles`, which every profile-consuming command calls once at startup. Everything downstream keeps consuming the existing `EntryProfile` type, extended by one `scope` field.

**Tech Stack:** Rust 2021, ldap3 0.12.1 (`SearchOptions::typesonly/timelimit`, `with_timeout`, streaming search), ldap-types 0.7.2 (`ObjectClassType::Structural`, `sup`), serde + toml 1.1 (`#[serde(try_from)]`, `toml::Value` display), tvision-rs 0.16 (UI unchanged apart from the chooser list).

**Spec:** `docs/superpowers/specs/2026-09-29-profile-detection-design.md` (approved; do not change its decisions). Read it next to this plan; section references (§1.4, §2B2, …) point into it.

## Global Constraints

- `SAMPLE_SIZE = 200` entries per container search (client size limit, applied per search).
- `MAX_CONTAINERS = 100` containers sampled, in server order; the rest are listed in a note.
- `DETECT_BUDGET = 10 s` for the whole sampling step, enforced per search: server time limit in whole seconds (at least 1) **and** client timeout, both set to the budget still left.
- A value-inferring rule (B1–B3, C) applies when **more than half** of the entries follow it **and** at least **3** entries were sampled (B1–B3) / scanned (C). Grouping, pattern guards and names apply regardless of size.
- Cross-container lookups are batched at **50** keys per filter.
- Sampled attributes: `objectClass uid cn sn givenName displayName gecos mail uidNumber gidNumber homeDirectory loginShell memberUid member uniqueMember sambaSID description`. **Never** `userPassword` or any other secret; attribute presence is read with a **types-only** `*` search.
- Number blocks split where neighbours are **more than 1000** apart; `MIN` = block low rounded down to a multiple of 1000; `MAX` = next block's `MIN − 1`, else `max(60000, MIN + 9999)`.
- Detected profile names are always `<base>-<container RDN value>` (`user-people`, `posixgroup-groups`); slug = lowercase, runs outside `[a-z0-9]` → one `-`, trimmed; collisions add parent RDN values. Names compare **case-insensitively everywhere**.
- Detection never stops eDAPtor: only the user's own config can cause a load error. `[detect] enabled = false` restores today's behaviour exactly.
- Detection is on by default, also for existing configs, and adds detected parts to matched hand-written profiles.
- Too little data (§2D, rule D): an empty number space allocates from `{next:10000-60000}` (LDAP starts at **10000** because client machines give 1000+ to local users); 1–2 numbers use the normal block rule (the 3-entry threshold counts only for exceptions); private groups are assumed for every final `posixAccount` profile — config-only ones included — unless sampled users contradict it. Assumptions run after the merge, never override config or detected values, have provenance `assumed`, are suppressible, and are off with `[detect] enabled = false`.
- Gates: `CARGO_BUILD_JOBS=4 make check` (fmt + clippy `-D warnings` + tests). Every cargo invocation uses `-j4` (shared 128-core machine, max 4 cores).
- Live tests run against the podman demo server (`scripts/test-ldap.sh start`, `ldap://localhost:11389`, base `dc=example,dc=org`, admin `cn=admin,dc=example,dc=org`, `EDAPTOR_TEST_ADMIN_PW=adminpassword`, gate env `EDAPTOR_TEST_LDAP_URI`). Tests that read a whole directory (number scans, sampling) run under `systemd-run --user --scope -p MemoryMax=2G -- cargo test …`.
- Comments, identifiers, docs in English. Commit messages end with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- User-visible changes: `CHANGES.md` (under `## Unreleased`, for users/admins, lead with the observable effect, max 3 sentences, no implementation names), mdBook `docs/src/` (canonical), `README.md` (short skeleton only), `examples/config.toml` ≡ the TOML block in `docs/src/configuration/full-example.md`.
- Never store scratch data in `/tmp`; use `/scratch/oetiker/claude-tmp/…`.

## Review Focus

1. **DNs with escaped commas or multi-valued RDNs** (`cn=Smith\, John,ou=people,…`, `cn=a+uid=b,…`): a person expects the entry to land in `ou=people`, not in a phantom ` John,ou=people` container, and names/parents to be computed on unescaped commas only. Pinned in Task 1 (`dn_components`, `SampleEntry::parent`) and Task 2 (naming).
2. **Mixed-case attribute and class names from the server** (`objectclass: posixaccount`, `UIDNumber`): detection must behave exactly as for canonical spelling. Pinned in Task 1 (`SampleEntry` accessors) and Task 6 (lower-case fixture still yields `user-…` with private groups).
3. **Garbage in number attributes** (`uidNumber: abc`, empty, multi-valued, negative): the range rule must ignore unparsable values, never panic, and still compute a range from the rest. Pinned in Task 4.
4. **An ACL-restricted or anonymous bind that sees no containers at all**: a person expects the config profiles and no error or status-line alarm, not "Profile detection failed". Pinned in Task 10 (sampler returns an empty `Sample`) and Task 11 (`assemble` yields config profiles, no `detection_error`).
5. **A config block that matches a detected profile by name but sets its own `search_base`**: the merged profile must create in the config's container, and its detected number range must count values in that container. Pinned in Task 8.

---

## File Structure

New files:

| File | Responsibility |
|---|---|
| `src/detect/mod.rs` | constants, threshold helpers, DN helpers (`dn_components`, `parent_dn`, `normalize_dn`, `dn_eq`), `most_common`, `is_infrastructure` |
| `src/detect/model.rs` | `Evidence`, `Detected<T>`, `SampleEntry`, `ContainerSample`, `Sample`, `DetectedProfile` (+ `to_entry_profile`) |
| `src/detect/fixtures.rs` | `#[cfg(test)]` schema + argus-like and demo-like samples |
| `src/detect/names.rs` | slug, base name, collision-free naming (§2A name rules) |
| `src/detect/infer.rs` | `detect(schema, sample) -> Detection`: grouping by structural class and the §2A fields |
| `src/detect/private.rs` | private-group predicate (§2B2) and `PrivateIndex` |
| `src/detect/patterns/mod.rs` | pattern orchestration |
| `src/detect/patterns/templates.rs` | B1 templated/literal defaults + cycle rule |
| `src/detect/patterns/posix.rs` | B2 user-private group, B3 shared primary group, group classification |
| `src/detect/patterns/samba.rs` | B4 |
| `src/detect/patterns/pickers.rs` | B5 |
| `src/detect/patterns/ranges.rs` | emits `DefaultValue::DetectedRange` defaults |
| `src/detect/range.rs` | rule C: `RangeSpec`, `detect_range`, `allocate`, scan filter/attrs |
| `src/detect/merge.rs` | matching, rename map, merge rules, order, `suppress`, `enabled`, provenance, per-origin validation |
| `src/detect/sample.rs` | `Searcher` trait, `WorkerSearcher`, `Budget`, `sample()` |
| `src/detect/load.rs` | `ProfileInputs`, `LoadedProfiles`, `assemble` (pure), `load_profiles` |
| `src/detect/dump.rs` | TOML dump with provenance comments, `compute_ranges` |
| `src/detect/testdata/argus-profiles.toml` | offline golden dump |
| `tests/live_profile_detection.rs` | live test against the demo server |
| `tests/golden/profiles-demo.toml` | live golden dump |
| `docs/src/configuration/detection.md` | mdBook page |

Modified: `src/lib.rs`, `src/main.rs`, `src/passwd.rs`, `src/schema/model.rs`, `src/config/mod.rs`, `src/config/defaults.rs`, `src/config/widget.rs`, `src/config/resolver.rs`, `src/ldap/worker.rs`, `src/workflows/create.rs`, `src/workflows/alloc_flow.rs`, `src/workflows/save.rs`, `src/ui/mod.rs`, `src/ui/app.rs`, `src/ui/state.rs`, every test file that builds a `Config` or `EntryProfile` literal, `CHANGES.md`, `README.md`, `docs/src/SUMMARY.md`, `docs/src/configuration/overview.md`, `docs/src/configuration/full-example.md`, `examples/config.toml`.

Task order and dependencies: 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 9 → 10 → 11 → 12/13/14 (independent of each other, all need 11) → 15 → 16. Each task ends green (`CARGO_BUILD_JOBS=4 make check`).

---

### Task 1: Schema helpers, DN helpers and the detection data model

**Files:**
- Modify: `src/schema/model.rs` (add three methods after `is_readonly_attr`, ~line 201)
- Modify: `src/lib.rs:4-12` (add `pub mod detect;`)
- Create: `src/detect/mod.rs`, `src/detect/model.rs`, `src/detect/fixtures.rs`

**Interfaces:**
- Consumes: `SchemaModel::object_class`, `crate::config::defaults::DefaultValue`, `crate::config::{WidgetSpecCfg, CompanionSpec}`, `crate::ldap::worker::LdapEntry`.
- Produces (used by every later task):
  - `SchemaModel::is_structural(&self, name: &str) -> bool`
  - `SchemaModel::superclasses(&self, name: &str) -> HashSet<String>` (lowercased)
  - `SchemaModel::structural_class(&self, ocs: &[String]) -> Option<String>`
  - `detect::{SAMPLE_SIZE: i32, MAX_CONTAINERS: usize, DETECT_BUDGET: Duration, MIN_SAMPLE: usize, LOOKUP_BATCH: usize, SAMPLE_ATTRS: &[&str], INFRASTRUCTURE_CLASSES: &[&str]}`
  - `detect::more_than_half(matched: usize, of: usize) -> bool`, `detect::rule_applies(matched: usize, sampled: usize) -> bool`
  - `detect::dn_components(dn: &str) -> Vec<&str>`, `detect::parent_dn(dn: &str) -> Option<&str>`, `detect::normalize_dn(dn: &str) -> String`, `detect::dn_eq(a: &str, b: &str) -> bool`
  - `detect::most_common<'a>(items: impl IntoIterator<Item = &'a str>) -> Option<(String, usize)>`
  - `detect::model::{Evidence, Detected<T>, SampleEntry, ContainerSample, Sample, DetectedProfile}` exactly as in Step 3.
  - `detect::fixtures::{schema, e, container, argus_sample, demo_sample}` (`#[cfg(test)]`).

- [ ] **Step 1: Write the failing schema test**

Append to the `tests` module of `src/schema/model.rs`:

```rust
    fn structural_raw() -> RawSubschema {
        RawSubschema {
            object_classes: vec![
                "( 2.5.6.0 NAME 'top' ABSTRACT MUST objectClass )".to_string(),
                "( 2.5.6.6 NAME 'person' SUP top STRUCTURAL MUST ( sn $ cn ) )".to_string(),
                "( 2.5.6.7 NAME 'organizationalPerson' SUP person STRUCTURAL )".to_string(),
                "( 2.16.840.1.113730.3.2.2 NAME 'inetOrgPerson' SUP organizationalPerson STRUCTURAL )"
                    .to_string(),
                "( 1.3.6.1.1.1.2.0 NAME 'posixAccount' SUP top AUXILIARY )".to_string(),
                "( 2.5.6.9 NAME 'groupOfNames' SUP top STRUCTURAL )".to_string(),
            ],
            attribute_types: vec![],
            ldap_syntaxes: vec![],
        }
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn structural_class_picks_the_most_specific_structural() {
        let m = SchemaModel::from_raw(&structural_raw());
        assert!(m.is_structural("inetorgperson"));
        assert!(!m.is_structural("posixAccount"));
        assert!(!m.is_structural("top"));
        assert!(m.superclasses("inetOrgPerson").contains("person"));
        assert_eq!(
            m.structural_class(&strings(&[
                "top",
                "person",
                "organizationalPerson",
                "inetOrgPerson",
                "posixAccount"
            ])),
            Some("inetOrgPerson".to_string())
        );
        // Server spelling differs: the schema's primary name is returned.
        assert_eq!(
            m.structural_class(&strings(&["INETORGPERSON", "person"])),
            Some("inetOrgPerson".to_string())
        );
        assert_eq!(m.structural_class(&strings(&["posixAccount"])), None);
        // Two unrelated structural classes: the first in entry order wins.
        assert_eq!(
            m.structural_class(&strings(&["groupOfNames", "person"])),
            Some("groupOfNames".to_string())
        );
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -j4 --lib schema::model::tests::structural_class_picks_the_most_specific_structural`
Expected: FAIL to compile — `no method named is_structural found for struct SchemaModel`.

- [ ] **Step 3: Implement the schema helpers**

In `src/schema/model.rs` change the `ldap_types` import to

```rust
use ldap_types::schema::{
    attribute_type_parser, object_class_parser, AttributeType, ObjectClass, ObjectClassType,
};
```

and add after `is_readonly_attr`:

```rust
    /// Whether `name` is a STRUCTURAL object class. Unknown classes → `false`.
    pub fn is_structural(&self, name: &str) -> bool {
        self.object_class(name)
            .map(|oc| oc.object_class_type == ObjectClassType::Structural)
            .unwrap_or(false)
    }

    /// Every (transitive) superclass of `name`, as lowercased names (primary names
    /// and aliases, plus the spelling the SUP reference used). Excludes `name`
    /// itself. Guarded against SUP cycles.
    pub fn superclasses(&self, name: &str) -> HashSet<String> {
        let mut out = HashSet::new();
        let mut stack: Vec<String> = self
            .object_class(name)
            .map(|oc| oc.sup.iter().map(|s| s.to_string()).collect())
            .unwrap_or_default();
        while let Some(s) = stack.pop() {
            if !out.insert(s.to_lowercase()) {
                continue;
            }
            if let Some(oc) = self.object_class(&s) {
                for n in &oc.name {
                    out.insert(n.to_string().to_lowercase());
                }
                stack.extend(oc.sup.iter().map(|x| x.to_string()));
            }
        }
        out
    }

    /// The structural class of an entry with object classes `ocs`: among the
    /// STRUCTURAL ones, the most specific (the one that is no other's superclass
    /// along the SUP chain). Unrelated structural classes: the first in `ocs`
    /// order. Returns the schema's primary name. `None` when no class is known
    /// to be structural.
    pub fn structural_class(&self, ocs: &[String]) -> Option<String> {
        let structurals: Vec<&String> = ocs.iter().filter(|o| self.is_structural(o)).collect();
        let supers: HashSet<String> = structurals
            .iter()
            .flat_map(|o| self.superclasses(o))
            .collect();
        let pick = structurals
            .into_iter()
            .find(|o| !supers.contains(&o.to_lowercase()))?;
        Some(
            self.object_class(pick)
                .and_then(|oc| oc.name.first())
                .map(|n| n.to_string())
                .unwrap_or_else(|| pick.clone()),
        )
    }
```

- [ ] **Step 4: Run it to verify it passes**

Run: `cargo test -j4 --lib schema::model::tests::structural_class_picks_the_most_specific_structural`
Expected: PASS.

- [ ] **Step 5: Write the failing detect-module tests**

Create `src/detect/mod.rs`:

```rust
//! Profile detection (spec 2026-09-29): sample the directory, detect profiles,
//! merge the config over them. `infer`, `patterns`, `range`, `merge` and `dump`
//! are pure; `sample` and `load` talk to the LDAP worker.

pub mod model;
#[cfg(test)]
pub(crate) mod fixtures;

use std::time::Duration;

/// Client size limit of every container sample search.
pub const SAMPLE_SIZE: i32 = 200;
/// At most this many containers are sampled (server order).
pub const MAX_CONTAINERS: usize = 100;
/// Time budget of the whole sampling step.
pub const DETECT_BUDGET: Duration = Duration::from_secs(10);
/// A value-inferring rule needs at least this many entries.
pub const MIN_SAMPLE: usize = 3;
/// Keys per cross-container lookup filter.
pub const LOOKUP_BATCH: usize = 50;
/// The attributes a container sample fetches (values). Never secrets.
pub const SAMPLE_ATTRS: &[&str] = &[
    "objectClass",
    "uid",
    "cn",
    "sn",
    "givenName",
    "displayName",
    "gecos",
    "mail",
    "uidNumber",
    "gidNumber",
    "homeDirectory",
    "loginShell",
    "memberUid",
    "member",
    "uniqueMember",
    "sambaSID",
    "description",
];
/// Structural classes whose detected profiles are hidden from the all-profiles
/// chooser unless the current container is exactly theirs.
pub const INFRASTRUCTURE_CLASSES: &[&str] = &[
    "organizationalUnit",
    "organization",
    "domain",
    "dcObject",
    "sambaDomain",
    "pwdPolicy",
];

/// `matched` is more than half of `of` (and `of` is not zero).
pub fn more_than_half(matched: usize, of: usize) -> bool {
    of > 0 && matched * 2 > of
}

/// A value-inferring rule applies: more than half AND at least `MIN_SAMPLE`.
pub fn rule_applies(matched: usize, sampled: usize) -> bool {
    sampled >= MIN_SAMPLE && more_than_half(matched, sampled)
}

/// Split a DN into its RDN components at unescaped commas (`\,` stays inside
/// its component). Components are returned untrimmed.
pub fn dn_components(dn: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut escaped = false;
    for (i, ch) in dn.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            ',' => {
                out.push(&dn[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if !dn.is_empty() {
        out.push(&dn[start..]);
    }
    out
}

/// The parent DN (everything after the first unescaped comma), trimmed.
pub fn parent_dn(dn: &str) -> Option<&str> {
    let first = dn_components(dn).into_iter().next()?;
    let rest = dn.get(first.len() + 1..)?;
    let rest = rest.trim_start();
    (!rest.is_empty()).then_some(rest)
}

/// Canonical comparison form of a DN: components trimmed, whitespace around `=`
/// removed, lowercased.
pub fn normalize_dn(dn: &str) -> String {
    dn_components(dn)
        .iter()
        .map(|c| match c.split_once('=') {
            Some((a, v)) => format!("{}={}", a.trim(), v.trim()),
            None => c.trim().to_string(),
        })
        .collect::<Vec<_>>()
        .join(",")
        .to_lowercase()
}

/// DN equality after [`normalize_dn`].
pub fn dn_eq(a: &str, b: &str) -> bool {
    normalize_dn(a) == normalize_dn(b)
}

/// The most frequent item (compared case-insensitively; the first spelling seen
/// is kept) and its count. Ties: the lexicographically smallest lowercased item.
pub fn most_common<'a>(items: impl IntoIterator<Item = &'a str>) -> Option<(String, usize)> {
    let mut counts: std::collections::BTreeMap<String, (String, usize)> =
        std::collections::BTreeMap::new();
    for it in items {
        let e = counts
            .entry(it.to_lowercase())
            .or_insert_with(|| (it.to_string(), 0));
        e.1 += 1;
    }
    // BTreeMap iterates in ascending key order; `max_by_key` keeps the LAST max,
    // so iterate in reverse to keep the smallest key among equal counts.
    counts
        .into_iter()
        .rev()
        .max_by_key(|(_, (_, n))| *n)
        .map(|(_, v)| v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_needs_three_and_a_majority() {
        assert!(rule_applies(2, 3));
        assert!(!rule_applies(2, 2));
        assert!(!rule_applies(1, 2));
        assert!(!rule_applies(6, 12));
        assert!(rule_applies(7, 12));
        assert!(more_than_half(2, 2));
        assert!(!more_than_half(0, 0));
    }

    #[test]
    fn dn_components_respect_escaped_commas() {
        assert_eq!(
            dn_components(r"cn=Smith\, John,ou=people,dc=x"),
            vec![r"cn=Smith\, John", "ou=people", "dc=x"]
        );
        assert_eq!(
            parent_dn(r"cn=Smith\, John,ou=people,dc=x"),
            Some("ou=people,dc=x")
        );
        assert_eq!(parent_dn("dc=x"), None);
        assert_eq!(parent_dn("cn=a+uid=b, ou=people,dc=x"), Some("ou=people,dc=x"));
    }

    #[test]
    fn dn_eq_ignores_case_and_spacing() {
        assert!(dn_eq("OU=People, DC=Example,dc=org", "ou=people,dc=example,dc=org"));
        assert!(!dn_eq("ou=people2,dc=x", "ou=people,dc=x"));
    }

    #[test]
    fn most_common_is_case_insensitive_and_stable() {
        assert_eq!(
            most_common(["cn", "CN", "uid"]),
            Some(("cn".to_string(), 2))
        );
        assert_eq!(most_common(["b", "a"]), Some(("a".to_string(), 1)));
        assert_eq!(most_common(Vec::<&str>::new()), None);
    }
}
```

Add `pub mod detect;` to `src/lib.rs` (alphabetical, after `pub mod config;`).

Create `src/detect/model.rs`:

```rust
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
```

Create `src/detect/fixtures.rs` (test-only fixtures reused by Tasks 2–13):

```rust
//! `#[cfg(test)]` fixtures: a schema with the classes the rules know, plus an
//! argus-like and a demo-like sample (spec §5.1).

use std::collections::BTreeMap;

use crate::detect::model::{ContainerSample, Sample, SampleEntry};
use crate::ldap::worker::RawSubschema;
use crate::schema::SchemaModel;

const DSTR: &str = "1.3.6.1.4.1.1466.115.121.1.15";
const IA5: &str = "1.3.6.1.4.1.1466.115.121.1.26";
const INT: &str = "1.3.6.1.4.1.1466.115.121.1.27";

pub(crate) fn schema() -> SchemaModel {
    let at = |oid: &str, name: &str, syntax: &str, extra: &str| {
        format!("( {oid} NAME '{name}' SYNTAX {syntax}{extra} )")
    };
    let raw = RawSubschema {
        object_classes: vec![
            "( 2.5.6.0 NAME 'top' ABSTRACT MUST objectClass )".into(),
            "( 2.5.6.6 NAME 'person' SUP top STRUCTURAL MUST ( sn $ cn ) MAY ( userPassword $ description ) )".into(),
            "( 2.5.6.7 NAME 'organizationalPerson' SUP person STRUCTURAL MAY ou )".into(),
            "( 2.16.840.1.113730.3.2.2 NAME 'inetOrgPerson' SUP organizationalPerson STRUCTURAL MAY ( uid $ mail $ givenName $ displayName $ jpegPhoto $ entryCSN ) )".into(),
            "( 1.3.6.1.1.1.2.0 NAME 'posixAccount' SUP top AUXILIARY MUST ( cn $ uid $ uidNumber $ gidNumber $ homeDirectory ) MAY ( userPassword $ loginShell $ gecos $ description ) )".into(),
            "( 1.3.6.1.1.1.2.1 NAME 'shadowAccount' SUP top AUXILIARY MUST uid MAY shadowLastChange )".into(),
            "( 1.3.6.1.1.1.2.2 NAME 'posixGroup' SUP top STRUCTURAL MUST ( cn $ gidNumber ) MAY ( userPassword $ memberUid $ description ) )".into(),
            "( 1.3.6.1.4.1.7165.2.2.6 NAME 'sambaSamAccount' SUP top AUXILIARY MUST ( uid $ sambaSID ) MAY sambaAcctFlags )".into(),
            "( 2.5.6.9 NAME 'groupOfNames' SUP top STRUCTURAL MUST ( member $ cn ) MAY description )".into(),
            "( 2.5.6.5 NAME 'organizationalUnit' SUP top STRUCTURAL MUST ou MAY description )".into(),
            "( 1.3.6.1.4.1.7165.2.2.5 NAME 'sambaDomain' SUP top STRUCTURAL MUST ( sambaDomainName $ sambaSID ) )".into(),
        ],
        attribute_types: vec![
            at("2.5.4.0", "objectClass", "1.3.6.1.4.1.1466.115.121.1.38", ""),
            at("2.5.4.3", "cn", DSTR, ""),
            at("2.5.4.4", "sn", DSTR, ""),
            at("2.5.4.11", "ou", DSTR, ""),
            at("2.5.4.13", "description", DSTR, ""),
            at("0.9.2342.19200300.100.1.1", "uid", DSTR, ""),
            at("0.9.2342.19200300.100.1.3", "mail", IA5, ""),
            at("2.5.4.42", "givenName", DSTR, ""),
            at("2.16.840.1.113730.3.1.241", "displayName", DSTR, " SINGLE-VALUE"),
            at("0.9.2342.19200300.100.1.60", "jpegPhoto", "1.3.6.1.4.1.1466.115.121.1.28", ""),
            at("2.5.4.35", "userPassword", "1.3.6.1.4.1.1466.115.121.1.40", ""),
            at("1.3.6.1.1.1.1.0", "uidNumber", INT, " SINGLE-VALUE"),
            at("1.3.6.1.1.1.1.1", "gidNumber", INT, " SINGLE-VALUE"),
            at("1.3.6.1.1.1.1.2", "gecos", IA5, " SINGLE-VALUE"),
            at("1.3.6.1.1.1.1.3", "homeDirectory", IA5, " SINGLE-VALUE"),
            at("1.3.6.1.1.1.1.4", "loginShell", IA5, " SINGLE-VALUE"),
            at("1.3.6.1.1.1.1.12", "memberUid", IA5, ""),
            at("2.5.4.31", "member", "1.3.6.1.4.1.1466.115.121.1.12", ""),
            at("1.3.6.1.4.1.7165.2.1.20", "sambaSID", IA5, " SINGLE-VALUE"),
            at("1.3.6.1.4.1.7165.2.1.4", "sambaAcctFlags", IA5, " SINGLE-VALUE"),
            at("1.3.6.1.4.1.7165.2.1.47", "sambaDomainName", DSTR, " SINGLE-VALUE"),
            at("1.3.6.1.1.1.1.5", "shadowLastChange", INT, " SINGLE-VALUE"),
            at("1.3.6.1.4.1.4203.666.1.7", "entryCSN", DSTR, " SINGLE-VALUE NO-USER-MODIFICATION USAGE directoryOperation"),
        ],
        ldap_syntaxes: vec![],
    };
    SchemaModel::from_raw(&raw)
}

/// Build an entry from `(attr, values)` pairs.
pub(crate) fn e(dn: &str, pairs: &[(&str, &[&str])]) -> SampleEntry {
    let attrs: BTreeMap<String, Vec<String>> = pairs
        .iter()
        .map(|(k, vs)| (k.to_string(), vs.iter().map(|v| v.to_string()).collect()))
        .collect();
    SampleEntry {
        dn: dn.to_string(),
        attrs,
    }
}

/// A container sample whose presence map is derived from the entries' keys.
pub(crate) fn container(dn: &str, entries: Vec<SampleEntry>) -> ContainerSample {
    ContainerSample {
        dn: dn.to_string(),
        entries,
        present: BTreeMap::new(),
        partial: false,
    }
}

fn ou(name: &str, base: &str) -> SampleEntry {
    e(
        &format!("ou={name},{base}"),
        &[("objectClass", &["top", "organizationalUnit"]), ("ou", &[name])],
    )
}

/// services-argus: `cn` RDN, `uid = cn`, gid = uid private groups, one shared
/// group (`staff`, 5020) sitting in the user block, shared groups at 8000+,
/// one user with a different shell (`u12`, /bin/tcsh).
pub(crate) fn argus_sample() -> Sample {
    let base = "dc=argus,dc=ch";
    let people = format!("ou=people,{base}");
    let groups = format!("ou=groups,{base}");
    let mut users = Vec::new();
    let mut group_entries = Vec::new();
    for i in 1..=12u64 {
        let name = format!("u{i:02}");
        let num = (5000 + i - 1).to_string();
        let shell = if i == 12 { "/bin/tcsh" } else { "/bin/bash" };
        let home = format!("/home/{name}");
        let given = format!("Given{i}");
        users.push(e(
            &format!("cn={name},{people}"),
            &[
                ("objectClass", &["top", "inetOrgPerson", "posixAccount"]),
                ("cn", &[name.as_str()]),
                ("uid", &[name.as_str()]),
                ("sn", &["Argus"]),
                ("givenName", &[given.as_str()]),
                ("uidNumber", &[num.as_str()]),
                ("gidNumber", &[num.as_str()]),
                ("homeDirectory", &[home.as_str()]),
                ("loginShell", &[shell]),
            ],
        ));
        group_entries.push(e(
            &format!("cn={name},{groups}"),
            &[
                ("objectClass", &["top", "posixGroup"]),
                ("cn", &[name.as_str()]),
                ("gidNumber", &[num.as_str()]),
                ("memberUid", &[name.as_str()]),
            ],
        ));
    }
    for (name, gid) in [("staff", "5020"), ("dev", "8000"), ("ops", "8001"), ("web", "8002")] {
        group_entries.push(e(
            &format!("cn={name},{groups}"),
            &[
                ("objectClass", &["top", "posixGroup"]),
                ("cn", &[name]),
                ("gidNumber", &[gid]),
                ("memberUid", &["u01"]),
            ],
        ));
    }
    Sample {
        base_dn: base.to_string(),
        containers: vec![
            container(base, vec![ou("people", base), ou("groups", base)]),
            container(&people, users),
            container(&groups, group_entries),
        ],
        ..Default::default()
    }
}

/// The podman demo, reduced: users in two containers (`ou=people` with Samba,
/// shared primary group 100; `ou=users` with gid = uid but no private groups),
/// `groupOfNames` + `posixGroup` in `ou=groups`, a `sambaDomain` at the base.
pub(crate) fn demo_sample() -> Sample {
    let base = "dc=example,dc=org";
    let people = format!("ou=people,{base}");
    let users_c = format!("ou=users,{base}");
    let groups = format!("ou=groups,{base}");
    let mut people_entries = Vec::new();
    for i in 1..=5u64 {
        let uid = format!("p{i}");
        let given = format!("P{i}");
        let cn = format!("P{i} Person");
        let num = (10000 + i - 1).to_string();
        let home = format!("/home/{uid}");
        let mail = format!("{uid}@example.org");
        let sid = format!("S-1-5-21-1-2-3-{}", 3000 + i);
        people_entries.push(e(
            &format!("uid={uid},{people}"),
            &[
                (
                    "objectClass",
                    &["top", "inetOrgPerson", "posixAccount", "shadowAccount", "sambaSamAccount"],
                ),
                ("uid", &[uid.as_str()]),
                ("cn", &[cn.as_str()]),
                ("givenName", &[given.as_str()]),
                ("sn", &["Person"]),
                ("mail", &[mail.as_str()]),
                ("uidNumber", &[num.as_str()]),
                ("gidNumber", &["100"]),
                ("homeDirectory", &[home.as_str()]),
                ("loginShell", &["/bin/bash"]),
                ("sambaSID", &[sid.as_str()]),
            ],
        ));
    }
    let mut users_entries = Vec::new();
    for i in 1..=3u64 {
        let uid = format!("user0{i}");
        let num = (1000 + i - 1).to_string();
        let home = format!("/home/{uid}");
        users_entries.push(e(
            &format!("uid={uid},{users_c}"),
            &[
                ("objectClass", &["top", "inetOrgPerson", "posixAccount", "shadowAccount"]),
                ("uid", &[uid.as_str()]),
                ("cn", &[uid.as_str()]),
                ("sn", &["User"]),
                ("uidNumber", &[num.as_str()]),
                ("gidNumber", &[num.as_str()]),
                ("homeDirectory", &[home.as_str()]),
            ],
        ));
    }
    let mut group_entries = Vec::new();
    for i in 1..=3u64 {
        let cn = format!("g{i}");
        let member = format!("uid=p{i},{people}");
        group_entries.push(e(
            &format!("cn={cn},{groups}"),
            &[
                ("objectClass", &["top", "groupOfNames"]),
                ("cn", &[cn.as_str()]),
                ("member", &[member.as_str()]),
            ],
        ));
        let pg = format!("pg{i}");
        let gid = (500 + i - 1).to_string();
        group_entries.push(e(
            &format!("cn={pg},{groups}"),
            &[
                ("objectClass", &["top", "posixGroup"]),
                ("cn", &[pg.as_str()]),
                ("gidNumber", &[gid.as_str()]),
                ("memberUid", &["p1"]),
            ],
        ));
    }
    let base_entries = vec![
        ou("people", base),
        ou("users", base),
        ou("groups", base),
        e(
            &format!("sambaDomainName=EXAMPLE,{base}"),
            &[
                ("objectClass", &["top", "sambaDomain"]),
                ("sambaDomainName", &["EXAMPLE"]),
                ("sambaSID", &["S-1-5-21-1-2-3"]),
            ],
        ),
    ];
    Sample {
        base_dn: base.to_string(),
        containers: vec![
            container(base, base_entries),
            container(&people, people_entries),
            container(&users_c, users_entries),
            container(&groups, group_entries),
        ],
        ..Default::default()
    }
}

#[test]
fn fixture_schema_parses_cleanly() {
    let s = schema();
    assert!(s.warnings.is_empty(), "{:?}", s.warnings);
    assert!(s.is_structural("posixGroup"));
    assert_eq!(
        s.structural_class(&["top".into(), "inetOrgPerson".into(), "posixAccount".into()]),
        Some("inetOrgPerson".into())
    );
}
```

- [ ] **Step 6: Run the new tests**

Run: `cargo test -j4 --lib detect::`
Expected: PASS (`threshold_needs_three_and_a_majority`, `dn_components_respect_escaped_commas`, `dn_eq_ignores_case_and_spacing`, `most_common_is_case_insensitive_and_stable`, the three `model` tests, `fixture_schema_parses_cleanly`). If `fixture_schema_parses_cleanly` reports parse warnings, fix the offending definition string in `fixtures.rs` (ldap-types is strict about keyword order: `NAME`, `SUP`, kind, `MUST`, `MAY` / `SYNTAX`, `SINGLE-VALUE`, `NO-USER-MODIFICATION`, `USAGE`).

- [ ] **Step 7: Gate and commit**

Run: `CARGO_BUILD_JOBS=4 make check`
Expected: `All checks passed!`. (Unused-item warnings: `detect::model` items unused outside tests are `pub`, so clippy does not flag them.)

```bash
git add src/lib.rs src/schema/model.rs src/detect/
git commit -m "feat(detect): data model, DN helpers and schema structural-class lookup

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Grouping by structural class, §2A fields and profile names

**Files:**
- Create: `src/detect/names.rs`, `src/detect/infer.rs`
- Modify: `src/detect/mod.rs` (add `pub mod infer; pub mod names;`)

**Interfaces:**
- Consumes: Task 1 (`SchemaModel::structural_class`, `SampleEntry`, `ContainerSample`, `Sample`, `DetectedProfile`, `most_common`, `more_than_half`, `dn_components`).
- Produces:
  - `names::slug(s: &str) -> String`
  - `names::base_name(structural: &str, object_classes: &[String]) -> String`
  - `names::assign_names(profiles: &mut [DetectedProfile])`
  - `infer::Detection { pub profiles: Vec<DetectedProfile>, pub notes: Vec<String> }`
  - `infer::detect(schema: &SchemaModel, sample: &Sample) -> Detection` (Task 3 and Task 6 add the pattern pass inside it)

- [ ] **Step 1: Write the failing naming tests**

Create `src/detect/names.rs` with only the tests first:

```rust
//! Detected profile names (spec §2A "Name rules"): `<base>-<container RDN value>`,
//! extended by parent RDN values until unique. Depends only on DNs, so stable.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::model::{Detected, DetectedProfile, Evidence};

    fn prof(base: &str, container: &str, structural: &str) -> DetectedProfile {
        DetectedProfile {
            name: String::new(),
            base_name: base.to_string(),
            structural: structural.to_string(),
            container: container.to_string(),
            sampled: 1,
            partial: false,
            object_classes: Detected::new(vec![structural.to_string()], Evidence::new(1, 1)),
            rdn_attr: Detected::new("cn".to_string(), Evidence::new(1, 1)),
            show: vec![],
            search_attrs: vec![],
            label: None,
            defaults: Default::default(),
            widgets: Default::default(),
            companion: None,
            private_groups: false,
            users_without_private_group: None,
            notes: vec![],
            entries: vec![],
        }
    }

    #[test]
    fn slug_lowercases_and_collapses_runs() {
        assert_eq!(slug("IT Staff"), "it-staff");
        assert_eq!(slug("  --People__2 "), "people-2");
        assert_eq!(slug("Ärger"), "rger");
        assert_eq!(slug("日本"), "");
    }

    #[test]
    fn base_name_prefers_patterns() {
        let ocs = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(base_name("inetOrgPerson", &ocs(&["inetOrgPerson", "posixAccount"])), "user");
        assert_eq!(base_name("posixGroup", &ocs(&["posixGroup"])), "posixgroup");
        assert_eq!(base_name("groupOfNames", &ocs(&["groupOfNames"])), "group");
        assert_eq!(base_name("groupOfUniqueNames", &ocs(&["groupOfUniqueNames"])), "group");
        assert_eq!(
            base_name("organizationalUnit", &ocs(&["organizationalUnit"])),
            "organizationalunit"
        );
    }

    #[test]
    fn names_carry_the_container_and_extend_on_collision() {
        let mut ps = vec![
            prof("user", "ou=people,o=a,dc=x", "inetOrgPerson"),
            prof("user", "ou=people,o=b,dc=x", "inetOrgPerson"),
            prof("user", "ou=IT Staff,dc=x", "inetOrgPerson"),
            prof("posixgroup", "ou=groups,dc=x", "posixGroup"),
        ];
        assign_names(&mut ps);
        let names: Vec<&str> = ps.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["user-people-a", "user-people-b", "user-it-staff", "posixgroup-groups"]
        );
    }

    #[test]
    fn same_container_same_base_falls_back_to_the_structural_class() {
        let mut ps = vec![
            prof("user", "ou=people,dc=x", "inetOrgPerson"),
            prof("user", "ou=people,dc=x", "account"),
        ];
        assign_names(&mut ps);
        assert_eq!(ps[0].name, "user-people-x-inetorgperson");
        assert_eq!(ps[1].name, "user-people-x-account");
    }

    #[test]
    fn escaped_comma_in_container_rdn_is_part_of_the_value() {
        let mut ps = vec![prof("user", r"ou=Smith\, Jones,dc=x", "inetOrgPerson")];
        assign_names(&mut ps);
        assert_eq!(ps[0].name, "user-smith-jones");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -j4 --lib detect::names` (after adding `pub mod names;` to `src/detect/mod.rs`)
Expected: FAIL to compile — `cannot find function slug`.

- [ ] **Step 3: Implement names**

Prepend to `src/detect/names.rs` (above the tests):

```rust
use crate::detect::model::DetectedProfile;

/// Lowercase; every run of characters outside `[a-z0-9]` becomes one `-`;
/// leading/trailing `-` dropped. May return "" (all characters dropped).
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for ch in s.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(ch);
        } else {
            pending_dash = true;
        }
    }
    out
}

/// The name base: the pattern's name (`user`, `posixgroup`, `group`, in that
/// precedence) or the slugged structural class.
pub fn base_name(structural: &str, object_classes: &[String]) -> String {
    let has = |oc: &str| {
        structural.eq_ignore_ascii_case(oc)
            || object_classes.iter().any(|c| c.eq_ignore_ascii_case(oc))
    };
    if has("posixAccount") {
        "user".to_string()
    } else if has("posixGroup") {
        "posixgroup".to_string()
    } else if has("groupOfNames") || has("groupOfUniqueNames") {
        "group".to_string()
    } else {
        slug(structural)
    }
}

/// Slugged RDN values of `dn`, leaf first. An empty slug becomes `x`.
fn rdn_slugs(dn: &str) -> Vec<String> {
    crate::detect::dn_components(dn)
        .iter()
        .map(|c| {
            let value = c.split_once('=').map(|(_, v)| v).unwrap_or(c);
            let s = slug(&value.replace('\\', ""));
            if s.is_empty() {
                "x".to_string()
            } else {
                s
            }
        })
        .collect()
}

/// Assign `name` to every profile: `<base>-<rdn value>`; while two names clash,
/// each clashing profile adds its next parent RDN value. Profiles that still
/// clash (same container, same base) get their structural class appended.
pub fn assign_names(profiles: &mut [DetectedProfile]) {
    fn clashes(names: &[String], i: usize) -> bool {
        names
            .iter()
            .enumerate()
            .any(|(j, other)| j != i && other == &names[i])
    }
    let n = profiles.len();
    let comps: Vec<Vec<String>> = profiles.iter().map(|p| rdn_slugs(&p.container)).collect();
    // Owned copies, so the closure below does not borrow `profiles`.
    let bases: Vec<String> = profiles.iter().map(|p| p.base_name.clone()).collect();
    let name_at = |i: usize, depth: usize| -> String {
        let parts = &comps[i][..depth.min(comps[i].len())];
        if parts.is_empty() {
            bases[i].clone()
        } else {
            format!("{}-{}", bases[i], parts.join("-"))
        }
    };
    let mut depth = vec![1usize; n];
    loop {
        let names: Vec<String> = (0..n).map(|i| name_at(i, depth[i])).collect();
        let mut changed = false;
        for i in 0..n {
            if clashes(&names, i) && depth[i] < comps[i].len() {
                depth[i] += 1;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut names: Vec<String> = (0..n).map(|i| name_at(i, depth[i])).collect();
    let still: Vec<bool> = (0..n).map(|i| clashes(&names, i)).collect();
    for i in 0..n {
        if still[i] {
            names[i] = format!("{}-{}", names[i], slug(&profiles[i].structural));
        }
    }
    for (p, name) in profiles.iter_mut().zip(names) {
        p.name = name;
    }
}
```

- [ ] **Step 4: Run naming tests**

Run: `cargo test -j4 --lib detect::names`
Expected: PASS (5 tests). `same_container_same_base…` expects the loop to extend to the full depth first (`people-x`) and then append the class — that is the specified fallback.

- [ ] **Step 5: Write the failing grouping tests**

Create `src/detect/infer.rs` with the tests first:

```rust
//! Detect profiles from a sample (spec §2A): group each container's entries by
//! structural class; derive object classes, RDN attribute, show, search
//! attributes and label; assign names. Pure.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::fixtures::{container, demo_sample, e, schema};
    use crate::detect::model::Sample;

    fn by_name<'a>(d: &'a Detection, name: &str) -> &'a DetectedProfile {
        d.profiles
            .iter()
            .find(|p| p.name == name)
            .unwrap_or_else(|| panic!("no profile {name}: {:?}", d.profiles.iter().map(|p| &p.name).collect::<Vec<_>>()))
    }

    #[test]
    fn demo_groups_into_the_expected_profiles() {
        let d = detect(&schema(), &demo_sample());
        let mut names: Vec<&str> = d.profiles.iter().map(|p| p.name.as_str()).collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "group-groups",
                "organizationalunit-example",
                "posixgroup-groups",
                "sambadomain-example",
                "user-people",
                "user-users"
            ]
        );
        let up = by_name(&d, "user-people");
        assert_eq!(up.structural, "inetOrgPerson");
        assert_eq!(up.container, "ou=people,dc=example,dc=org");
        assert_eq!(
            up.object_classes.value,
            vec!["inetOrgPerson", "posixAccount", "sambaSamAccount", "shadowAccount"]
        );
        assert_eq!(up.rdn_attr.value, "uid");
        assert_eq!(up.rdn_attr.evidence.ratio(), "5/5");
        assert_eq!(up.show[0], "uid");
        assert!(up.show.iter().any(|a| a == "loginShell"), "{:?}", up.show);
        assert_eq!(up.search_attrs, vec!["uid", "cn", "sn", "mail"]);
        assert_eq!(up.label.as_deref(), Some("{cn} ({uid})"));
        assert_eq!(up.sampled, 5);
        // uid == cn in ou=users → label falls back to the RDN attribute.
        assert_eq!(by_name(&d, "user-users").label.as_deref(), Some("{uid}"));
    }

    #[test]
    fn minority_classes_are_dropped_and_missing_ones_are_exceptions() {
        let base = "ou=people,dc=x";
        let mk = |i: u32, extra: &[&str]| {
            let mut ocs = vec!["top", "inetOrgPerson"];
            ocs.extend_from_slice(extra);
            let dn = format!("uid=a{i},{base}");
            let uid = format!("a{i}");
            e(&dn, &[("objectClass", &ocs), ("uid", &[uid.as_str()]), ("cn", &["A"]), ("sn", &["B"])])
        };
        let sample = Sample {
            containers: vec![container(
                base,
                vec![
                    mk(1, &["shadowAccount"]),
                    mk(2, &["shadowAccount"]),
                    mk(3, &["shadowAccount"]),
                    mk(4, &[]),
                    mk(5, &["sambaSamAccount"]),
                ],
            )],
            ..Default::default()
        };
        let d = detect(&schema(), &sample);
        let p = &d.profiles[0];
        assert_eq!(p.object_classes.value, vec!["inetOrgPerson", "shadowAccount"]);
        assert_eq!(p.object_classes.evidence.ratio(), "3/5");
        assert_eq!(
            p.object_classes.evidence.exceptions,
            vec!["uid=a4,ou=people,dc=x", "uid=a5,ou=people,dc=x"]
        );
    }

    #[test]
    fn show_excludes_binary_and_operational_attributes() {
        let base = "ou=people,dc=x";
        let entries: Vec<_> = (1..=3)
            .map(|i| {
                let dn = format!("uid=b{i},{base}");
                let uid = format!("b{i}");
                e(
                    &dn,
                    &[
                        ("objectClass", &["inetOrgPerson"]),
                        ("uid", &[uid.as_str()]),
                        ("cn", &["C"]),
                        ("sn", &["S"]),
                        ("jpegPhoto", &["x"]),
                        ("entryCSN", &["1"]),
                        ("mail", &["m@x"]),
                    ],
                )
            })
            .collect();
        let d = detect(
            &schema(),
            &Sample {
                containers: vec![container(base, entries)],
                ..Default::default()
            },
        );
        let show = &d.profiles[0].show;
        assert!(show.iter().any(|a| a == "mail"));
        assert!(!show.iter().any(|a| a.eq_ignore_ascii_case("jpegPhoto")));
        assert!(!show.iter().any(|a| a.eq_ignore_ascii_case("entryCSN")));
        assert!(!show.iter().any(|a| a.eq_ignore_ascii_case("objectClass")));
    }

    #[test]
    fn entries_without_a_structural_class_are_noted() {
        let sample = Sample {
            containers: vec![container(
                "ou=x,dc=x",
                vec![e("cn=a,ou=x,dc=x", &[("objectClass", &["posixAccount"])])],
            )],
            ..Default::default()
        };
        let d = detect(&schema(), &sample);
        assert!(d.profiles.is_empty());
        assert_eq!(d.notes.len(), 1);
        assert!(d.notes[0].contains("ou=x,dc=x"), "{}", d.notes[0]);
    }

    #[test]
    fn an_empty_sample_detects_nothing() {
        let d = detect(&schema(), &Sample::default());
        assert!(d.profiles.is_empty());
        assert!(d.notes.is_empty());
    }
}
```

- [ ] **Step 6: Run to verify failure**

Run: `cargo test -j4 --lib detect::infer` (after adding `pub mod infer;`)
Expected: FAIL to compile — `cannot find function detect`.

- [ ] **Step 7: Implement grouping**

Prepend to `src/detect/infer.rs`:

```rust
use std::collections::BTreeMap;

use crate::detect::model::{ContainerSample, Detected, DetectedProfile, Evidence, Sample, SampleEntry};
use crate::detect::{more_than_half, most_common, names};
use crate::schema::{FieldKind, SchemaModel};

/// The detector's output.
#[derive(Debug, Clone, Default)]
pub struct Detection {
    pub profiles: Vec<DetectedProfile>,
    pub notes: Vec<String>,
}

/// Detect profiles from `sample` (see module docs).
pub fn detect(schema: &SchemaModel, sample: &Sample) -> Detection {
    let mut notes = Vec::new();
    let mut profiles = Vec::new();
    for c in &sample.containers {
        profiles.extend(group_container(schema, c, &mut notes));
    }
    names::assign_names(&mut profiles);
    Detection { profiles, notes }
}

/// One profile per structural class present in container `c`.
fn group_container(
    schema: &SchemaModel,
    c: &ContainerSample,
    notes: &mut Vec<String>,
) -> Vec<DetectedProfile> {
    let mut groups: BTreeMap<String, (String, Vec<&SampleEntry>)> = BTreeMap::new();
    let mut unclassified = 0usize;
    for e in &c.entries {
        match schema.structural_class(e.values("objectClass")) {
            Some(s) => groups
                .entry(s.to_lowercase())
                .or_insert_with(|| (s.clone(), Vec::new()))
                .1
                .push(e),
            None => unclassified += 1,
        }
    }
    if unclassified > 0 {
        notes.push(format!(
            "{}: skipped {unclassified} entr{} without a known structural object class",
            c.dn,
            if unclassified == 1 { "y" } else { "ies" }
        ));
    }
    groups
        .into_values()
        .map(|(structural, entries)| build_profile(schema, c, &structural, &entries))
        .collect()
}

fn build_profile(
    schema: &SchemaModel,
    c: &ContainerSample,
    structural: &str,
    entries: &[&SampleEntry],
) -> DetectedProfile {
    let n = entries.len();
    let object_classes = majority_classes(structural, entries);
    let rdn_attr = common_rdn_attr(entries);
    let presence = |attr: &str| {
        let key = attr.to_lowercase();
        entries
            .iter()
            .filter(|e| c.present_attrs(e).contains(&key))
            .count()
    };
    let show = show_list(schema, &object_classes.value, &rdn_attr.value, n, &presence);
    let mut search_attrs = vec![rdn_attr.value.clone()];
    for a in ["cn", "uid", "sn", "mail", "description"] {
        if more_than_half(presence(a), n) && !search_attrs.iter().any(|s| s.eq_ignore_ascii_case(a)) {
            search_attrs.push(a.to_string());
        }
    }
    let differ = entries
        .iter()
        .filter(|e| match (e.first("cn"), e.first("uid")) {
            (Some(cn), Some(uid)) => cn != uid,
            _ => false,
        })
        .count();
    let label = if more_than_half(differ, n) {
        "{cn} ({uid})".to_string()
    } else {
        format!("{{{}}}", rdn_attr.value)
    };
    DetectedProfile {
        name: String::new(),
        base_name: names::base_name(structural, &object_classes.value),
        structural: structural.to_string(),
        container: c.dn.clone(),
        sampled: n,
        partial: c.partial,
        object_classes,
        rdn_attr,
        show,
        search_attrs,
        label: Some(label),
        defaults: BTreeMap::new(),
        widgets: BTreeMap::new(),
        companion: None,
        private_groups: false,
        users_without_private_group: None,
        notes: Vec::new(),
        entries: entries.iter().map(|e| (*e).clone()).collect(),
    }
}

/// Classes (except `top`) present in more than half the group; the structural
/// class first, then by frequency, then by name.
fn majority_classes(structural: &str, entries: &[&SampleEntry]) -> Detected<Vec<String>> {
    let n = entries.len();
    let mut counts: BTreeMap<String, (String, usize)> = BTreeMap::new();
    for e in entries {
        for oc in e.values("objectClass") {
            let oc = oc.trim();
            if oc.eq_ignore_ascii_case("top") || oc.is_empty() {
                continue;
            }
            counts
                .entry(oc.to_lowercase())
                .or_insert_with(|| (oc.to_string(), 0))
                .1 += 1;
        }
    }
    let mut chosen: Vec<(String, usize)> = counts
        .into_values()
        .filter(|(_, k)| more_than_half(*k, n))
        .collect();
    chosen.sort_by(|a, b| {
        let a_s = a.0.eq_ignore_ascii_case(structural);
        let b_s = b.0.eq_ignore_ascii_case(structural);
        b_s.cmp(&a_s)
            .then(b.1.cmp(&a.1))
            .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });
    let mut value: Vec<String> = chosen.into_iter().map(|(name, _)| name).collect();
    // Use the schema spelling of the structural class.
    if let Some(first) = value.first_mut() {
        if first.eq_ignore_ascii_case(structural) {
            *first = structural.to_string();
        }
    }
    let exceptions: Vec<String> = entries
        .iter()
        .filter(|e| !value.iter().all(|oc| e.has_class(oc)))
        .map(|e| e.dn.clone())
        .collect();
    let matched = n - exceptions.len();
    Detected::new(value, Evidence::new(matched, n).with_exceptions(exceptions))
}

/// The most common RDN attribute of the group.
fn common_rdn_attr(entries: &[&SampleEntry]) -> Detected<String> {
    let n = entries.len();
    let (attr, matched) =
        most_common(entries.iter().filter_map(|e| e.rdn_attr())).unwrap_or(("cn".to_string(), 0));
    let exceptions = entries
        .iter()
        .filter(|e| !e.rdn_attr().is_some_and(|a| a.eq_ignore_ascii_case(&attr)))
        .map(|e| e.dn.clone())
        .collect();
    Detected::new(attr, Evidence::new(matched, n).with_exceptions(exceptions))
}

/// `rdn_attr`, then MUST attributes, then MAY attributes present in more than
/// half the group (by frequency, then name); operational and binary excluded.
fn show_list(
    schema: &SchemaModel,
    object_classes: &[String],
    rdn_attr: &str,
    n: usize,
    presence: &dyn Fn(&str) -> usize,
) -> Vec<String> {
    let excluded = |a: &str| {
        a.eq_ignore_ascii_case("objectClass")
            || schema.is_readonly_attr(a)
            || schema.field_kind(a) == FieldKind::Binary
    };
    let mut show: Vec<String> = vec![rdn_attr.to_string()];
    let push = |show: &mut Vec<String>, a: &str| {
        if !show.iter().any(|s| s.eq_ignore_ascii_case(a)) {
            show.push(a.to_string());
        }
    };
    let refs: Vec<&str> = object_classes.iter().map(String::as_str).collect();
    let resolved = schema.effective_attributes(&refs);
    for m in &resolved.must {
        if !excluded(m) {
            push(&mut show, m);
        }
    }
    let mut may: Vec<(usize, String)> = resolved
        .may
        .iter()
        .filter(|a| !excluded(a))
        .map(|a| (presence(a), a.clone()))
        .filter(|(k, _)| more_than_half(*k, n))
        .collect();
    may.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase())));
    for (_, a) in may {
        push(&mut show, &a);
    }
    show
}
```

- [ ] **Step 8: Run grouping tests**

Run: `cargo test -j4 --lib detect::`
Expected: PASS. If `demo_groups_into_the_expected_profiles` fails only on `search_attrs` order, the implementation is wrong (order is `rdn_attr` then the fixed list `cn uid sn mail description` filtered by presence) — fix the code, not the test.

- [ ] **Step 9: Gate and commit**

Run: `CARGO_BUILD_JOBS=4 make check` → `All checks passed!`

```bash
git add src/detect/
git commit -m "feat(detect): group samples by structural class and name the profiles

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Rule B1 — templated and literal defaults

**Files:**
- Create: `src/detect/patterns/mod.rs`, `src/detect/patterns/templates.rs`
- Modify: `src/detect/mod.rs` (add `pub mod patterns;`), `src/detect/infer.rs` (`detect` calls `patterns::apply`), `src/config/defaults.rs` (add `DefaultValue::to_config_string`)

**Interfaces:**
- Consumes: Task 1/2 types; `crate::config::defaults::{parse_default_value, resolve_template, DefaultValue, Seg}`; `SchemaModel::is_single_value`.
- Produces:
  - `DefaultValue::to_config_string(&self) -> String` (inverse of `parse_default_value`; used by Task 8 and Task 14)
  - `patterns::templates::infer_defaults(schema: &SchemaModel, entries: &[SampleEntry], rdn_attr: &str) -> (BTreeMap<String, Detected<DefaultValue>>, Vec<String>)` — second value: notes about dropped templates
  - `patterns::templates::NEVER_LITERAL: &[&str]`
  - `patterns::apply(schema: &SchemaModel, sample: &Sample, profiles: &mut [DetectedProfile], notes: &mut Vec<String>)` (Task 6 replaces its body)

- [ ] **Step 1: Write the failing `to_config_string` test**

Append to the tests of `src/config/defaults.rs`:

```rust
    #[test]
    fn to_config_string_round_trips() {
        for s in ["/bin/bash", "/home/{uid}", "{givenName} {sn}", "{next:5000-7999}", "{auto:sambaSID}"] {
            let v = parse_default_value(s).unwrap();
            assert_eq!(v.to_config_string(), s, "round trip of {s}");
            assert_eq!(parse_default_value(&v.to_config_string()).unwrap(), v);
        }
    }
```

Run: `cargo test -j4 --lib config::defaults::tests::to_config_string_round_trips`
Expected: FAIL — `no method named to_config_string`.

- [ ] **Step 2: Implement it**

Add below `parse_default_value` in `src/config/defaults.rs`:

```rust
impl DefaultValue {
    /// The config spelling of this value (inverse of [`parse_default_value`]).
    pub fn to_config_string(&self) -> String {
        match self {
            DefaultValue::Literal(s) => s.clone(),
            DefaultValue::Template(segs) => segs
                .iter()
                .map(|s| match s {
                    Seg::Lit(l) => l.clone(),
                    Seg::Field(f) => format!("{{{f}}}"),
                })
                .collect(),
            DefaultValue::AutoNumber { min, max } => format!("{{next:{min}-{max}}}"),
            DefaultValue::Computed(ComputedKind::SambaSid) => "{auto:sambaSID}".to_string(),
        }
    }
}
```

Run the test again → PASS.

- [ ] **Step 3: Write the failing B1 tests**

Create `src/detect/patterns/templates.rs` with tests first:

```rust
//! Rule B1 (spec §2B1): templated and fixed defaults inferred from the sample.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::fixtures::{argus_sample, demo_sample, e, schema};

    fn entries_of(sample: &crate::detect::model::Sample, container: &str) -> Vec<SampleEntry> {
        sample
            .containers
            .iter()
            .find(|c| c.dn == container)
            .unwrap()
            .entries
            .clone()
    }

    fn cfg(d: &BTreeMap<String, Detected<DefaultValue>>, attr: &str) -> Option<String> {
        d.get(attr).map(|v| v.value.to_config_string())
    }

    #[test]
    fn argus_keeps_the_template_fed_by_the_rdn_and_drops_its_mirror() {
        let users = entries_of(&argus_sample(), "ou=people,dc=argus,dc=ch");
        let (d, dropped) = infer_defaults(&schema(), &users, "cn");
        assert_eq!(cfg(&d, "uid").as_deref(), Some("{cn}"));
        assert_eq!(d["uid"].evidence.ratio(), "12/12");
        assert!(!d.contains_key("cn"), "cn = {{uid}} must be dropped (cycle)");
        assert_eq!(dropped.len(), 1);
        assert!(dropped[0].contains("cn"), "{}", dropped[0]);
        assert_eq!(cfg(&d, "homeDirectory").as_deref(), Some("/home/{uid}"));
        assert_eq!(cfg(&d, "loginShell").as_deref(), Some("/bin/bash"));
        assert_eq!(d["loginShell"].evidence.ratio(), "11/12");
        assert_eq!(
            d["loginShell"].evidence.exceptions,
            vec!["cn=u12,ou=people,dc=argus,dc=ch"]
        );
    }

    #[test]
    fn demo_people_get_cn_from_given_name_and_sn() {
        let people = entries_of(&demo_sample(), "ou=people,dc=example,dc=org");
        let (d, _) = infer_defaults(&schema(), &people, "uid");
        assert_eq!(cfg(&d, "cn").as_deref(), Some("{givenName} {sn}"));
        assert!(!d.contains_key("uid"));
        // gidNumber is shared (100) but belongs to rule B3, never B1.
        assert!(!d.contains_key("gidNumber"));
        assert!(!d.contains_key("sambaSID"));
        assert!(!d.contains_key("mail"));
    }

    fn user(i: u32, shell: &str, home: &str) -> SampleEntry {
        let dn = format!("uid=t{i},ou=p,dc=x");
        let uid = format!("t{i}");
        e(
            &dn,
            &[
                ("objectClass", &["inetOrgPerson", "posixAccount"]),
                ("uid", &[uid.as_str()]),
                ("loginShell", &[shell]),
                ("homeDirectory", &[home]),
                ("memberUid", &["admin"]),
                ("description", &["same"]),
            ],
        )
    }

    #[test]
    fn untidy_directory_applies_with_exceptions() {
        let mut v: Vec<SampleEntry> = (1..=10)
            .map(|i| user(i, "/bin/zsh", &format!("/srv/home/t{i}")))
            .collect();
        v.push(user(11, "/bin/sh", "/tmp/x"));
        v.push(user(12, "/bin/sh", "/tmp/y"));
        let (d, _) = infer_defaults(&schema(), &v, "uid");
        assert_eq!(cfg(&d, "loginShell").as_deref(), Some("/bin/zsh"));
        assert_eq!(d["loginShell"].evidence.ratio(), "10/12");
        assert_eq!(d["loginShell"].evidence.exceptions.len(), 2);
        assert_eq!(cfg(&d, "homeDirectory").as_deref(), Some("/srv/home/{uid}"));
        // Membership and multi-valued attributes never become literals.
        assert!(!d.contains_key("memberUid"));
        assert!(!d.contains_key("description"));
    }

    #[test]
    fn below_threshold_nothing_is_inferred() {
        let v = vec![user(1, "/bin/zsh", "/home/t1"), user(2, "/bin/zsh", "/home/t2")];
        let (d, _) = infer_defaults(&schema(), &v, "uid");
        assert!(d.is_empty(), "{d:?}");
    }
}
```

Create `src/detect/patterns/mod.rs`:

```rust
//! Known patterns (spec §2B) applied to detected profiles.

pub mod templates;

use crate::detect::model::{DetectedProfile, Sample};
use crate::schema::SchemaModel;

/// Run every pattern over `profiles` (names are already assigned).
pub fn apply(
    schema: &SchemaModel,
    _sample: &Sample,
    profiles: &mut [DetectedProfile],
    _notes: &mut Vec<String>,
) {
    for p in profiles.iter_mut() {
        let (defaults, dropped) = templates::infer_defaults(schema, &p.entries, &p.rdn_attr.value);
        p.notes.extend(dropped);
        p.defaults.extend(defaults);
    }
}
```

and add `pub mod patterns;` to `src/detect/mod.rs`.

- [ ] **Step 4: Run to verify failure**

Run: `cargo test -j4 --lib detect::patterns`
Expected: FAIL to compile — `cannot find function infer_defaults`.

- [ ] **Step 5: Implement B1**

Prepend to `src/detect/patterns/templates.rs`:

```rust
use std::collections::{BTreeMap, BTreeSet};

use crate::config::defaults::{parse_default_value, resolve_template, DefaultValue, Seg};
use crate::detect::model::{Detected, Evidence, SampleEntry};
use crate::detect::{most_common, rule_applies, MIN_SAMPLE};
use crate::schema::SchemaModel;

/// Candidate templates, tried in order; the first one the majority follows wins
/// for its attribute. `homeDirectory` is handled by prefix detection.
const TEMPLATES: &[(&str, &str)] = &[
    ("uid", "{cn}"),
    ("cn", "{uid}"),
    ("cn", "{givenName} {sn}"),
    ("displayName", "{givenName} {sn}"),
    ("gecos", "{givenName} {sn}"),
];

/// Never inferred as a literal default: membership attributes, attributes
/// unique per entry, and attributes another rule owns (`gidNumber`: B2/B3).
pub const NEVER_LITERAL: &[&str] = &[
    "objectClass", "memberUid", "member", "uniqueMember", "memberOf", "uidNumber",
    "gidNumber", "mail", "sambaSID", "uid", "cn", "sn", "givenName", "displayName",
    "gecos", "userPassword",
];

/// B1 over `entries` (the profile's sampled entries, or its non-private groups).
/// Returns the defaults and notes about templates dropped by the cycle rule.
pub fn infer_defaults(
    schema: &SchemaModel,
    entries: &[SampleEntry],
    rdn_attr: &str,
) -> (BTreeMap<String, Detected<DefaultValue>>, Vec<String>) {
    let mut out: BTreeMap<String, Detected<DefaultValue>> = BTreeMap::new();
    if entries.len() < MIN_SAMPLE {
        return (out, Vec::new());
    }
    for (attr, tmpl) in TEMPLATES {
        if out.keys().any(|k| k.eq_ignore_ascii_case(attr)) {
            continue;
        }
        if let Some(d) = test_template(entries, attr, tmpl) {
            out.insert(attr.to_string(), d);
        }
    }
    if let Some(d) = home_directory(entries) {
        out.insert("homeDirectory".to_string(), d);
    }
    let dropped = break_cycles(&mut out, rdn_attr);
    for attr in literal_candidates(schema, entries, rdn_attr) {
        if out.keys().any(|k| k.eq_ignore_ascii_case(&attr)) {
            continue;
        }
        if let Some(d) = common_literal(entries, &attr) {
            out.insert(attr, d);
        }
    }
    (out, dropped)
}

/// `Some` when the majority of `entries` hold exactly the value `tmpl` yields.
fn test_template(entries: &[SampleEntry], attr: &str, tmpl: &str) -> Option<Detected<DefaultValue>> {
    let Ok(DefaultValue::Template(segs)) = parse_default_value(tmpl) else {
        return None;
    };
    let exceptions: Vec<String> = entries
        .iter()
        .filter(|e| {
            let want = e.first(attr);
            want.is_none() || resolve_template(&segs, &e.attrs).as_deref() != want
        })
        .map(|e| e.dn.clone())
        .collect();
    let matched = entries.len() - exceptions.len();
    rule_applies(matched, entries.len()).then(|| {
        Detected::new(
            DefaultValue::Template(segs),
            Evidence::new(matched, entries.len()).with_exceptions(exceptions),
        )
    })
}

/// `homeDirectory = "<P>{uid}"` for the most common fixed prefix `P`.
fn home_directory(entries: &[SampleEntry]) -> Option<Detected<DefaultValue>> {
    let prefixes: Vec<&str> = entries
        .iter()
        .filter_map(|e| {
            let (h, u) = (e.first("homeDirectory")?, e.first("uid")?);
            h.strip_suffix(u)
        })
        .collect();
    let (prefix, _) = most_common(prefixes)?;
    if prefix.is_empty() || prefix.contains('{') {
        return None;
    }
    test_template(entries, "homeDirectory", &format!("{prefix}{{uid}}"))
}

fn template_sources(dv: &DefaultValue) -> Vec<String> {
    match dv {
        DefaultValue::Template(segs) => segs
            .iter()
            .filter_map(|s| match s {
                Seg::Field(f) => Some(f.to_lowercase()),
                Seg::Lit(_) => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Templates must not feed each other: of such a pair keep only the one whose
/// source is `rdn_attr` (neither → drop both). Returns one note per drop.
fn break_cycles(out: &mut BTreeMap<String, Detected<DefaultValue>>, rdn_attr: &str) -> Vec<String> {
    let keys: Vec<String> = out.keys().cloned().collect();
    let mut drop: BTreeSet<String> = BTreeSet::new();
    for a in &keys {
        for b in &keys {
            if a >= b {
                continue;
            }
            let (sa, sb) = (template_sources(&out[a].value), template_sources(&out[b].value));
            if sa.contains(&b.to_lowercase()) && sb.contains(&a.to_lowercase()) {
                let rdn = rdn_attr.to_lowercase();
                if !sa.contains(&rdn) {
                    drop.insert(a.clone());
                }
                if !sb.contains(&rdn) {
                    drop.insert(b.clone());
                }
            }
        }
    }
    drop.into_iter()
        .filter_map(|k| {
            let d = out.remove(&k)?;
            Some(format!(
                "dropped detected default {k} = \"{}\": it and another detected template feed each other",
                d.value.to_config_string()
            ))
        })
        .collect()
}

/// Sampled single-valued attributes that may carry a literal default.
fn literal_candidates(schema: &SchemaModel, entries: &[SampleEntry], rdn_attr: &str) -> Vec<String> {
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    for e in entries {
        for k in e.attrs.keys() {
            seen.entry(k.to_lowercase()).or_insert_with(|| k.clone());
        }
    }
    seen.into_values()
        .filter(|a| !a.eq_ignore_ascii_case(rdn_attr))
        .filter(|a| !NEVER_LITERAL.iter().any(|n| n.eq_ignore_ascii_case(a)))
        .filter(|a| schema.is_single_value(a))
        .collect()
}

fn common_literal(entries: &[SampleEntry], attr: &str) -> Option<Detected<DefaultValue>> {
    let (value, matched) = most_common(entries.iter().filter_map(|e| e.first(attr)))?;
    if !rule_applies(matched, entries.len()) {
        return None;
    }
    let exceptions = entries
        .iter()
        .filter(|e| e.first(attr) != Some(value.as_str()))
        .map(|e| e.dn.clone())
        .collect();
    Some(Detected::new(
        DefaultValue::Literal(value),
        Evidence::new(matched, entries.len()).with_exceptions(exceptions),
    ))
}
```

Note: `most_common` compares case-insensitively; a literal whose spellings differ only in case (`/bin/Bash` vs `/bin/bash`) counts as one value and keeps the first spelling, while `common_literal`'s exceptions compare exactly. That is acceptable for shells and paths; do not "fix" it by lowercasing values.

Wire the pass into `detect` in `src/detect/infer.rs`:

```rust
    names::assign_names(&mut profiles);
    crate::detect::patterns::apply(schema, sample, &mut profiles, &mut notes);
    Detection { profiles, notes }
```

- [ ] **Step 6: Run the tests**

Run: `cargo test -j4 --lib detect:: config::defaults`
Expected: PASS (all Task 1–3 tests).

- [ ] **Step 7: Gate and commit**

Run: `CARGO_BUILD_JOBS=4 make check` → `All checks passed!`

```bash
git add src/config/defaults.rs src/detect/
git commit -m "feat(detect): infer templated and literal defaults (rule B1)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 4: Private-group predicate and rule C (pure range function)

**Files:**
- Create: `src/detect/private.rs`, `src/detect/range.rs`
- Modify: `src/detect/mod.rs` (add `pub mod private; pub mod range;`)

**Interfaces:**
- Consumes: `SampleEntry`, `Evidence`, `dn_eq`, `MIN_SAMPLE`.
- Produces:
  - `private::is_private_group(group: &SampleEntry, user: &SampleEntry) -> bool`
  - `private::PrivateIndex<'a>` with `new(entries: impl IntoIterator<Item = &'a SampleEntry>) -> Self`, `private_group_of(&self, user: &SampleEntry) -> Option<&'a SampleEntry>`, `is_private(&self, group: &SampleEntry) -> bool`
  - `range::RangeSpec { attr, container, structural: String, unified, exclude_private: bool }` (`Debug, Clone, PartialEq, Eq`)
  - `range::RangeReport { min, max, next: u64, in_use: Option<(u64, u64)>, next_block: Option<u64>, exhausted: bool, evidence: Evidence }` with `template(&self) -> String` and `describe(&self) -> String` (`in_use = None`: no numbers in the space, rule D start)
  - `range::{ASSUMED_MIN: u64 = 10000, OPEN_END: u64 = 60000}`
  - `range::detect_range(spec: &RangeSpec, entries: &[SampleEntry]) -> Result<RangeReport, String>`
  - `range::allocate(spec: &RangeSpec, entries: &[SampleEntry]) -> Result<(u64, Evidence), String>`
  - `range::SCAN_FILTER: &str = "(|(uidNumber=*)(gidNumber=*))"`, `range::SCAN_ATTRS: &[&str] = &["objectClass", "cn", "uid", "uidNumber", "gidNumber"]`

- [ ] **Step 1: Write the failing tests**

`src/detect/private.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::fixtures::e;

    fn user(uid: &str, cn: &str, num: &str, gid: &str) -> SampleEntry {
        e(
            &format!("cn={cn},ou=people,dc=x"),
            &[("objectClass", &["posixAccount"]), ("uid", &[uid]), ("cn", &[cn]),
              ("uidNumber", &[num]), ("gidNumber", &[gid])],
        )
    }
    fn group(cn: &str, gid: &str) -> SampleEntry {
        e(&format!("cn={cn},ou=groups,dc=x"), &[("objectClass", &["posixGroup"]), ("cn", &[cn]), ("gidNumber", &[gid])])
    }

    #[test]
    fn predicate_needs_name_and_all_three_numbers() {
        assert!(is_private_group(&group("alice", "7000"), &user("alice", "Alice Smith", "7000", "7000")));
        assert!(is_private_group(&group("ALICE", "7000"), &user("alice", "x", "7000", "7000")));
        assert!(!is_private_group(&group("alice", "7000"), &user("alice", "x", "7000", "100")));
        assert!(!is_private_group(&group("alice", "7001"), &user("alice", "x", "7000", "7000")));
        assert!(!is_private_group(&group("bob", "7000"), &user("alice", "x", "7000", "7000")));
    }

    #[test]
    fn index_answers_both_directions() {
        let all = vec![user("alice", "Alice", "7000", "7000"), group("alice", "7000"), group("staff", "100")];
        let idx = PrivateIndex::new(all.iter());
        assert_eq!(idx.private_group_of(&all[0]).map(|g| g.dn.as_str()), Some("cn=alice,ou=groups,dc=x"));
        assert!(idx.is_private(&all[1]));
        assert!(!idx.is_private(&all[2]));
    }
}
```

`src/detect/range.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::fixtures::{argus_sample, e};

    fn all_entries(s: &crate::detect::model::Sample) -> Vec<SampleEntry> {
        s.containers.iter().flat_map(|c| c.entries.clone()).collect()
    }
    fn spec(attr: &str, container: &str, structural: &str, unified: bool, excl: bool) -> RangeSpec {
        RangeSpec { attr: attr.into(), container: container.into(), structural: structural.into(), unified, exclude_private: excl }
    }
    fn acct(i: u64, num: &str) -> SampleEntry {
        e(&format!("uid=a{i},ou=p,dc=x"), &[("objectClass", &["inetOrgPerson", "posixAccount"]), ("uid", &[&format!("a{i}")]), ("uidNumber", &[num]), ("gidNumber", &["100"])])
    }

    #[test]
    fn argus_users_and_shared_groups() {
        let all = all_entries(&argus_sample());
        let users = detect_range(&spec("uidNumber", "ou=people,dc=argus,dc=ch", "inetOrgPerson", true, false), &all).unwrap();
        assert_eq!((users.min, users.max), (5000, 7999));
        assert_eq!(users.next, 5021, "staff (5020) is in the user block");
        assert_eq!(users.next_block, Some(8000));
        assert_eq!(users.template(), "{next:5000-7999}");
        let groups = detect_range(&spec("gidNumber", "ou=groups,dc=argus,dc=ch", "posixGroup", true, true), &all).unwrap();
        assert_eq!((groups.min, groups.max, groups.next), (8000, 60000, 8003));
        assert_eq!(groups.evidence.ratio(), "3/4");
        assert_eq!(groups.evidence.exceptions, vec!["cn=staff,ou=groups,dc=argus,dc=ch"]);
    }

    #[test]
    fn neighbouring_block_caps_max() {
        let v = vec![acct(1, "5000"), acct(2, "5001"), acct(3, "5003"), acct(4, "6200")];
        let r = detect_range(&spec("uidNumber", "ou=p,dc=x", "inetOrgPerson", false, false), &v).unwrap();
        assert_eq!((r.min, r.max, r.next), (5000, 5999, 5004));
    }

    #[test]
    fn a_block_at_65534_yields_a_valid_range() {
        let v = vec![acct(1, "65532"), acct(2, "65533"), acct(3, "65534")];
        let r = detect_range(&spec("uidNumber", "ou=p,dc=x", "inetOrgPerson", false, false), &v).unwrap();
        assert!(r.min <= r.max);
        assert_eq!((r.min, r.max, r.next), (65000, 74999, 65535));
    }

    #[test]
    fn exhausted_pool_is_reported_and_allocation_refuses() {
        let v: Vec<SampleEntry> = (0..=20).map(|i| acct(i, &(50000 + i * 500).to_string())).collect();
        let s = spec("uidNumber", "ou=p,dc=x", "inetOrgPerson", false, false);
        let r = detect_range(&s, &v).unwrap();
        assert_eq!((r.min, r.max, r.next), (50000, 60000, 60001));
        assert!(r.exhausted);
        assert!(r.describe().contains("pool exhausted"));
        assert_eq!(allocate(&s, &v).unwrap_err(), "number pool 50000-60000 is exhausted");
    }

    #[test]
    fn non_unified_uid_space_ignores_account_gids() {
        let v = vec![acct(1, "10000"), acct(2, "10001"), acct(3, "10002")];
        let (n, ev) = allocate(&spec("uidNumber", "ou=p,dc=x", "inetOrgPerson", false, false), &v).unwrap();
        assert_eq!(n, 10003);
        assert_eq!(ev.ratio(), "3/3");
    }

    #[test]
    fn private_group_found_when_uid_differs_from_cn() {
        let v = vec![
            e("cn=Alice Smith,ou=p,dc=x", &[("objectClass", &["posixAccount"]), ("uid", &["alice"]), ("cn", &["Alice Smith"]), ("uidNumber", &["7000"]), ("gidNumber", &["7000"])]),
            e("cn=alice,ou=g,dc=x", &[("objectClass", &["posixGroup"]), ("cn", &["alice"]), ("gidNumber", &["7000"])]),
            e("cn=a,ou=g,dc=x", &[("objectClass", &["posixGroup"]), ("cn", &["a"]), ("gidNumber", &["9000"])]),
            e("cn=b,ou=g,dc=x", &[("objectClass", &["posixGroup"]), ("cn", &["b"]), ("gidNumber", &["9001"])]),
            e("cn=c,ou=g,dc=x", &[("objectClass", &["posixGroup"]), ("cn", &["c"]), ("gidNumber", &["9002"])]),
        ];
        let r = detect_range(&spec("gidNumber", "ou=g,dc=x", "posixGroup", true, true), &v).unwrap();
        assert_eq!(r.evidence.ratio(), "3/3", "the private group is not a value of the group profile");
        assert_eq!((r.min, r.next), (9000, 9003));
    }

    #[test]
    fn an_empty_space_starts_useradd_style_at_10000() {
        let r = detect_range(&spec("uidNumber", "ou=p,dc=x", "inetOrgPerson", true, false), &[]).unwrap();
        assert_eq!((r.min, r.max, r.next), (10000, 60000, 10000));
        assert_eq!(r.in_use, None);
        assert_eq!(r.template(), "{next:10000-60000}");
        assert!(r.describe().contains("no numbers in use"), "{}", r.describe());
        assert_eq!(allocate(&spec("gidNumber", "ou=g,dc=x", "posixGroup", true, true), &[]).unwrap().0, 10000);
    }

    #[test]
    fn one_or_two_values_continue_their_block() {
        // argus-like start: one user at 5000 → 5001, not 10000.
        let r = detect_range(&spec("uidNumber", "ou=p,dc=x", "inetOrgPerson", false, false), &[acct(1, "5000")]).unwrap();
        assert_eq!((r.min, r.max, r.next), (5000, 60000, 5001));
        // Two values in different blocks: no exceptions below the 3-entry threshold.
        let v = vec![acct(1, "5000"), acct(2, "9000")];
        let r = detect_range(&spec("uidNumber", "ou=p,dc=x", "inetOrgPerson", false, false), &v).unwrap();
        assert_eq!((r.min, r.next), (5000, 5001));
        assert!(r.evidence.exceptions.is_empty());
    }

    #[test]
    fn a_profile_without_own_values_uses_the_fullest_block() {
        // New group profile; the unified space already holds user numbers 5000, 5001.
        let v = vec![acct(1, "5000"), acct(2, "5001")];
        let r = detect_range(&spec("gidNumber", "ou=g,dc=x", "posixGroup", true, true), &v).unwrap();
        assert_eq!((r.min, r.next), (5000, 5002), "100 is the accounts' gid, counted in a unified space");
    }

    #[test]
    fn garbage_numbers_are_ignored() {
        let mut v = vec![acct(1, "10000"), acct(2, "10001"), acct(3, "10002")];
        v.push(acct(4, "abc"));
        v.push(acct(5, "-7"));
        v.push(acct(6, ""));
        v.push(e("uid=a7,ou=p,dc=x", &[("objectClass", &["inetOrgPerson", "posixAccount"]), ("uidNumber", &["10003", "99999"])]));
        let r = detect_range(&spec("uidNumber", "ou=p,dc=x", "inetOrgPerson", false, false), &v).unwrap();
        assert_eq!(r.min, 10000);
        assert!(r.next >= 10004);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -j4 --lib detect::private detect::range`
Expected: FAIL to compile (`cannot find function is_private_group` / `detect_range`).

- [ ] **Step 3: Implement**

Prepend to `src/detect/private.rs`:

```rust
//! The private-group predicate (spec §2B2): posixGroup G is the private group
//! of posixAccount U when G.cn = U.uid and G.gidNumber = U.gidNumber = U.uidNumber.

use std::collections::HashMap;

use crate::detect::model::SampleEntry;

fn num(e: &SampleEntry, attr: &str) -> Option<u64> {
    e.first(attr)?.parse().ok()
}

pub fn is_private_group(group: &SampleEntry, user: &SampleEntry) -> bool {
    if !group.has_class("posixGroup") || !user.has_class("posixAccount") {
        return false;
    }
    let (Some(cn), Some(uid)) = (group.first("cn"), user.first("uid")) else {
        return false;
    };
    if !cn.eq_ignore_ascii_case(uid) {
        return false;
    }
    match (num(group, "gidNumber"), num(user, "gidNumber"), num(user, "uidNumber")) {
        (Some(g), Some(ug), Some(uu)) => g == ug && ug == uu,
        _ => false,
    }
}

/// Lookup tables for the predicate in both directions.
pub struct PrivateIndex<'a> {
    users_by_uid: HashMap<String, Vec<&'a SampleEntry>>,
    groups_by_cn: HashMap<String, Vec<&'a SampleEntry>>,
}

impl<'a> PrivateIndex<'a> {
    pub fn new(entries: impl IntoIterator<Item = &'a SampleEntry>) -> Self {
        let mut users_by_uid: HashMap<String, Vec<&'a SampleEntry>> = HashMap::new();
        let mut groups_by_cn: HashMap<String, Vec<&'a SampleEntry>> = HashMap::new();
        for e in entries {
            if e.has_class("posixAccount") {
                if let Some(uid) = e.first("uid") {
                    users_by_uid.entry(uid.to_lowercase()).or_default().push(e);
                }
            }
            if e.has_class("posixGroup") {
                if let Some(cn) = e.first("cn") {
                    groups_by_cn.entry(cn.to_lowercase()).or_default().push(e);
                }
            }
        }
        PrivateIndex { users_by_uid, groups_by_cn }
    }

    pub fn private_group_of(&self, user: &SampleEntry) -> Option<&'a SampleEntry> {
        let uid = user.first("uid")?.to_lowercase();
        self.groups_by_cn
            .get(&uid)?
            .iter()
            .copied()
            .find(|g| is_private_group(g, user))
    }

    pub fn is_private(&self, group: &SampleEntry) -> bool {
        let Some(cn) = group.first("cn") else { return false };
        self.users_by_uid
            .get(&cn.to_lowercase())
            .is_some_and(|us| us.iter().any(|u| is_private_group(group, u)))
    }
}
```

Prepend to `src/detect/range.rs`:

```rust
//! Rule C (spec §2C): number ranges, computed from a full number scan at create
//! time (and eagerly by `edaptor profiles`). Pure.

use crate::detect::model::{Evidence, SampleEntry};
use crate::detect::private::PrivateIndex;
use crate::detect::{dn_eq, MIN_SAMPLE};

/// Neighbouring values more than this apart start a new block.
pub const BLOCK_GAP: u64 = 1000;
/// Rule D (§2D): first number when the space is empty. Client machines hand out
/// 1000 and up to local users, so LDAP numbers start higher.
pub const ASSUMED_MIN: u64 = 10000;
/// Upper end of the highest block: `max(OPEN_END, MIN + 9999)`.
pub const OPEN_END: u64 = 60000;
/// The allocation scan (subtree under `base_dn`, no size limit).
pub const SCAN_FILTER: &str = "(|(uidNumber=*)(gidNumber=*))";
pub const SCAN_ATTRS: &[&str] = &["objectClass", "cn", "uid", "uidNumber", "gidNumber"];

/// What a detected `{next:…}` allocates and which entries are "its" values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeSpec {
    /// `uidNumber` or `gidNumber`.
    pub attr: String,
    /// The profile's `search_base`.
    pub container: String,
    /// The profile's structural class.
    pub structural: String,
    /// B2 applied: uidNumber and gidNumber form one number space.
    pub unified: bool,
    /// posixGroup profile: private groups are not its values.
    pub exclude_private: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeReport {
    pub min: u64,
    pub max: u64,
    pub next: u64,
    /// Lowest and highest number in use in the chosen block; `None` when the
    /// space is empty (rule D start).
    pub in_use: Option<(u64, u64)>,
    pub next_block: Option<u64>,
    pub exhausted: bool,
    pub evidence: Evidence,
}

impl RangeReport {
    /// `{next:MIN-MAX}`.
    pub fn template(&self) -> String {
        format!("{{next:{}-{}}}", self.min, self.max)
    }

    /// `in use 5000-5016; next block at 8000[; pool exhausted]`.
    pub fn describe(&self) -> String {
        let Some((lo, hi)) = self.in_use else {
            return format!("no numbers in use; useradd-style start at {ASSUMED_MIN}");
        };
        let mut s = format!("in use {lo}-{hi}");
        match self.next_block {
            Some(b) => s.push_str(&format!("; next block at {b}")),
            None => s.push_str("; no higher block"),
        }
        if self.exhausted {
            s.push_str("; pool exhausted");
        }
        s
    }
}

fn nums(e: &SampleEntry, attr: &str) -> Vec<u64> {
    e.values(attr).iter().filter_map(|v| v.trim().parse().ok()).collect()
}

fn split_blocks(sorted: &[u64]) -> Vec<(u64, u64)> {
    let mut out: Vec<(u64, u64)> = Vec::new();
    for &v in sorted {
        match out.last_mut() {
            Some(b) if v - b.1 <= BLOCK_GAP => b.1 = v,
            _ => out.push((v, v)),
        }
    }
    out
}

/// Apply rule C to a full scan.
pub fn detect_range(spec: &RangeSpec, entries: &[SampleEntry]) -> Result<RangeReport, String> {
    let index = PrivateIndex::new(entries.iter());
    let mut space: Vec<u64> = Vec::new();
    for e in entries {
        let is_account = e.has_class("posixAccount");
        if spec.unified {
            if is_account {
                space.extend(nums(e, "uidNumber"));
            }
            space.extend(nums(e, "gidNumber"));
        } else if spec.attr.eq_ignore_ascii_case("uidNumber") {
            space.extend(nums(e, "uidNumber"));
        } else if !is_account {
            space.extend(nums(e, &spec.attr));
        }
    }
    space.sort_unstable();
    space.dedup();
    let mine: Vec<(u64, &str)> = entries
        .iter()
        .filter(|e| e.parent().is_some_and(|p| dn_eq(p, &spec.container)))
        .filter(|e| e.has_class(&spec.structural))
        .filter(|e| !(spec.exclude_private && index.is_private(e)))
        .flat_map(|e| nums(e, &spec.attr).into_iter().map(move |n| (n, e.dn.as_str())))
        .collect();
    if space.is_empty() {
        // Rule D: nothing in use yet.
        return Ok(RangeReport {
            min: ASSUMED_MIN,
            max: OPEN_END,
            next: ASSUMED_MIN,
            in_use: None,
            next_block: None,
            exhausted: false,
            evidence: Evidence::new(0, 0).with_note("no numbers in use; useradd-style start"),
        });
    }
    let blocks = split_blocks(&space);
    // The profile's block holds most of its own values; a profile without values
    // yet takes the block holding most numbers of the space. Ties: the lower block.
    let in_block = |vals: &mut dyn Iterator<Item = u64>, b: &(u64, u64)| vals.filter(|n| *n >= b.0 && *n <= b.1).count();
    let count = |b: &(u64, u64)| {
        if mine.is_empty() {
            in_block(&mut space.iter().copied(), b)
        } else {
            in_block(&mut mine.iter().map(|(n, _)| *n), b)
        }
    };
    let (bi, block) = blocks
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| count(a).cmp(&count(b)).then_with(|| b.0.cmp(&a.0)))
        .map(|(i, b)| (i, *b))
        .expect("the space is not empty, so there is a block");
    // The 3-entry threshold counts only for exceptions (§2D).
    let exceptions: Vec<String> = if mine.len() >= MIN_SAMPLE {
        mine.iter()
            .filter(|(n, _)| *n < block.0 || *n > block.1)
            .map(|(_, dn)| dn.to_string())
            .collect()
    } else {
        Vec::new()
    };
    let matched = mine.iter().filter(|(n, _)| *n >= block.0 && *n <= block.1).count();
    let min = block.0 / 1000 * 1000;
    let next_block = blocks.get(bi + 1).map(|b| b.0);
    let max = match next_block {
        Some(lo) => lo / 1000 * 1000 - 1,
        None => OPEN_END.max(min + 9999),
    };
    let next = block.1 + 1;
    Ok(RangeReport {
        min,
        max,
        next,
        in_use: Some(block),
        next_block,
        exhausted: next > max,
        evidence: Evidence::new(matched, mine.len()).with_exceptions(exceptions),
    })
}

/// The number to allocate: `max(in use in block) + 1`; refuses on an exhausted pool
/// with the same message `{next:MIN-MAX}` uses.
pub fn allocate(spec: &RangeSpec, entries: &[SampleEntry]) -> Result<(u64, Evidence), String> {
    let r = detect_range(spec, entries)?;
    if r.exhausted {
        return Err(format!("number pool {}-{} is exhausted", r.min, r.max));
    }
    Ok((r.next, r.evidence))
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -j4 --lib detect::private detect::range`
Expected: PASS (2 + 10 tests).

- [ ] **Step 5: Gate and commit**

Run: `CARGO_BUILD_JOBS=4 make check` → `All checks passed!`

```bash
git add src/detect/
git commit -m "feat(detect): private-group predicate and number-range rule C

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: `DefaultValue::DetectedRange` and create-time allocation

**Files:**
- Modify: `src/config/defaults.rs` (enum `DefaultValue`, enum `Resolution`, `plan_defaults`, `to_config_string`)
- Modify: `src/workflows/create.rs:84-102` (`plan_companion` arm), `:244-257` (`apply_static_defaults`), `:411-487` (`build_create_form`), tests at `:944-1023`
- Modify: `src/workflows/save.rs:159-173` (message constant)
- Modify: `src/workflows/alloc_flow.rs` (second request kind)
- Modify: `src/ui/app.rs:530-600` (`open_create`)

**Interfaces:**
- Consumes: `range::{RangeSpec, allocate, SCAN_FILTER, SCAN_ATTRS}`, `SampleEntry: From<&LdapEntry>`.
- Produces:
  - `DefaultValue::DetectedRange(RangeSpec)`; `Resolution::NeedsDetectedRange { attr: String, spec: RangeSpec }`
  - `create::AllocRequest { Range { attr: String, min: u64, max: u64 }, Detected { attr: String, spec: RangeSpec } }` with `attr(&self) -> &str`
  - `create::apply_static_defaults(..) -> Vec<AllocRequest>`; `create::build_create_form(..) -> (EditForm, Vec<AllocRequest>)`
  - `save::TRUNCATED_SCAN_MSG: &str`
  - `AllocFlow::request_detected(&mut self, worker: &WorkerHandle, base: &str, attr: &str, spec: RangeSpec) -> Result<u64>`; test seam `AllocFlow::insert_detected_for_test(&mut self, id: u64, attr: String, spec: RangeSpec)`

- [ ] **Step 1: Write the failing tests**

In `src/config/defaults.rs` tests:

```rust
    #[test]
    fn detected_range_surfaces_as_needs_detected_range() {
        let spec = crate::detect::range::RangeSpec {
            attr: "uidNumber".into(), container: "ou=p,dc=x".into(), structural: "inetOrgPerson".into(),
            unified: false, exclude_private: false,
        };
        let mut d = ProfileDefaults::default();
        d.entries.insert("uidNumber".into(), DefaultValue::DetectedRange(spec.clone()));
        assert_eq!(
            plan_defaults(&d, &cur(&[("uidNumber", "")])),
            vec![Resolution::NeedsDetectedRange { attr: "uidNumber".into(), spec }]
        );
        assert!(plan_defaults(&d, &cur(&[("uidNumber", "12")])).is_empty());
    }
```

In `src/workflows/alloc_flow.rs` tests:

```rust
    fn scan_entry(dn: &str, ocs: &[&str], pairs: &[(&str, &str)]) -> crate::ldap::worker::LdapEntry {
        let mut attrs: std::collections::BTreeMap<String, Vec<String>> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), vec![v.to_string()]))
            .collect();
        attrs.insert("objectClass".into(), ocs.iter().map(|s| s.to_string()).collect());
        crate::ldap::worker::LdapEntry { dn: dn.into(), attrs, bin_attrs: Default::default() }
    }

    fn user_spec() -> crate::detect::range::RangeSpec {
        crate::detect::range::RangeSpec {
            attr: "uidNumber".into(), container: "ou=people,dc=x".into(), structural: "inetOrgPerson".into(),
            unified: false, exclude_private: false,
        }
    }

    #[test]
    fn detected_range_allocation_fills_from_the_scan() {
        let mut af = AllocFlow::new();
        let id = af.alloc_for_test();
        af.insert_detected_for_test(id, "uidNumber".into(), user_spec());
        let entries = (0..3)
            .map(|i| scan_entry(&format!("uid=u{i},ou=people,dc=x"), &["inetOrgPerson", "posixAccount"],
                &[("uid", &format!("u{i}")), ("uidNumber", &(5000 + i).to_string()), ("gidNumber", "100")]))
            .collect();
        let out = af.on_response(&Response::Entries { id, entries, truncated: false });
        assert_eq!(out, AllocOutcome::Filled { attr: "uidNumber".into(), value: "5003".into() });
    }

    #[test]
    fn detected_range_refuses_a_truncated_scan_like_today() {
        let mut af = AllocFlow::new();
        let id = af.alloc_for_test();
        af.insert_detected_for_test(id, "uidNumber".into(), user_spec());
        let out = af.on_response(&Response::Entries { id, entries: vec![], truncated: true });
        assert_eq!(
            out,
            AllocOutcome::Failed { attr: "uidNumber".into(), msg: crate::workflows::save::TRUNCATED_SCAN_MSG.into() }
        );
    }

    #[test]
    fn request_detected_posts_the_number_scan() {
        let (worker, rx) = WorkerHandle::recording();
        let mut af = AllocFlow::new();
        af.request_detected(&worker, "dc=x", "uidNumber", user_spec()).unwrap();
        let (req, _) = rx.try_recv().expect("a request was submitted");
        match req {
            Request::Search { base, scope, filter, attrs, size_limit, .. } => {
                assert_eq!(base, "dc=x");
                assert_eq!(scope, SearchScope::Subtree);
                assert_eq!(filter, "(|(uidNumber=*)(gidNumber=*))");
                assert!(attrs.iter().any(|a| a == "uid"));
                assert_eq!(size_limit, None);
            }
            other => panic!("unexpected {other:?}"),
        }
    }
```

In `src/workflows/create.rs` tests, change the last assertion block of `apply_static_defaults_fills_literals_templates_and_surfaces_autonumber` to

```rust
        assert_eq!(
            autonum,
            vec![AllocRequest::Range { attr: "uidNumber".to_string(), min: 10000, max: 60000 }]
        );
```

and add

```rust
    #[test]
    fn apply_static_defaults_surfaces_detected_ranges() {
        use crate::config::defaults::{DefaultValue, ProfileDefaults};
        let spec = crate::detect::range::RangeSpec {
            attr: "gidNumber".into(), container: "ou=g,dc=x".into(), structural: "posixGroup".into(),
            unified: true, exclude_private: true,
        };
        let mut d = ProfileDefaults::default();
        d.entries.insert("gidNumber".into(), DefaultValue::DetectedRange(spec.clone()));
        let mut attrs = BTreeMap::new();
        let reqs = apply_static_defaults(&d, &mut attrs);
        assert_eq!(reqs, vec![AllocRequest::Detected { attr: "gidNumber".into(), spec }]);
        assert_eq!(reqs[0].attr(), "gidNumber");
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -j4 --lib config::defaults workflows::alloc_flow workflows::create`
Expected: FAIL to compile (`no variant DetectedRange`, `no function insert_detected_for_test`, `cannot find type AllocRequest`).

- [ ] **Step 3: Implement**

`src/config/defaults.rs`:

```rust
    /// A number range detected from the directory (spec §2C); resolved at create
    /// time by a full number scan. Never written in a config file.
    DetectedRange(crate::detect::range::RangeSpec),
```

added to `DefaultValue`; add to `Resolution`:

```rust
    NeedsDetectedRange { attr: String, spec: crate::detect::range::RangeSpec },
```

add the `plan_defaults` arm:

```rust
            DefaultValue::DetectedRange(spec) => out.push(Resolution::NeedsDetectedRange {
                attr: attr.clone(),
                spec: spec.clone(),
            }),
```

and the `to_config_string` arm (only ever used inside comments; not parseable on purpose):

```rust
            DefaultValue::DetectedRange(s) => format!("(detected {} range)", s.attr),
```

`src/workflows/save.rs` — replace the literal in `decide_allocation` by a constant:

```rust
/// Why an allocation refuses a truncated number scan (shared by every allocator).
pub const TRUNCATED_SCAN_MSG: &str = "refusing to allocate: the number scan hit a server size limit \
     (bind with a higher-limit identity or configure a counter)";
```

with `return Err(TRUNCATED_SCAN_MSG.to_string());` in `decide_allocation`.

`src/workflows/create.rs` — add above `apply_static_defaults`:

```rust
/// A number the create form still needs from a directory scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllocRequest {
    /// `{next:MIN-MAX}` from the config.
    Range { attr: String, min: u64, max: u64 },
    /// A detected range (rule C), resolved from a full number scan.
    Detected { attr: String, spec: crate::detect::range::RangeSpec },
}

impl AllocRequest {
    pub fn attr(&self) -> &str {
        match self {
            AllocRequest::Range { attr, .. } | AllocRequest::Detected { attr, .. } => attr,
        }
    }
}
```

change `apply_static_defaults` to return `Vec<AllocRequest>`:

```rust
            Resolution::NeedsAutonumber { attr, min, max } => {
                autonum.push(AllocRequest::Range { attr, min, max })
            }
            Resolution::NeedsDetectedRange { attr, spec } => {
                autonum.push(AllocRequest::Detected { attr, spec })
            }
```

change `build_create_form`'s return type to `(crate::workflows::edit_form::EditForm, Vec<AllocRequest>)` and its doc comment ("the allocation requests that still need a directory scan"). In `plan_companion` add the arm

```rust
            DefaultValue::DetectedRange(_) => {
                return Err(format!(
                    "companion attribute '{attr}' uses a detected number range, which is unsupported"
                ))
            }
```

`src/workflows/alloc_flow.rs` — replace the pending map value by an enum:

```rust
use crate::detect::model::SampleEntry;
use crate::detect::range::{allocate, RangeSpec, SCAN_ATTRS, SCAN_FILTER};
use crate::workflows::save::TRUNCATED_SCAN_MSG;

enum Pending {
    Range { attr: String, min: u64, max: u64 },
    Detected { attr: String, spec: RangeSpec },
}
```

`pending: HashMap<u64, Pending>`; `request` inserts `Pending::Range { .. }`; add

```rust
    /// Post the full number scan for a detected range; returns the request id.
    pub fn request_detected(
        &mut self,
        worker: &WorkerHandle,
        base: &str,
        attr: &str,
        spec: RangeSpec,
    ) -> Result<u64> {
        let id = self.alloc();
        worker.submit(Request::Search {
            id,
            base: base.to_string(),
            scope: SearchScope::Subtree,
            filter: SCAN_FILTER.to_string(),
            attrs: SCAN_ATTRS.iter().map(|s| s.to_string()).collect(),
            size_limit: None,
        })?;
        self.pending.insert(id, Pending::Detected { attr: attr.to_string(), spec });
        Ok(id)
    }
```

and in `on_response` for `Response::Entries`:

```rust
                let Some(pending) = self.pending.remove(id) else {
                    return AllocOutcome::Ignored;
                };
                match pending {
                    Pending::Range { attr, min, max } => {
                        // (existing body: collect `values` for `attr`, then decide_allocation)
                    }
                    Pending::Detected { attr, spec } => {
                        if *truncated {
                            return AllocOutcome::Failed { attr, msg: TRUNCATED_SCAN_MSG.to_string() };
                        }
                        let scan: Vec<SampleEntry> = entries.iter().map(SampleEntry::from).collect();
                        match allocate(&spec, &scan) {
                            Ok((n, _)) => AllocOutcome::Filled { attr, value: n.to_string() },
                            Err(msg) => AllocOutcome::Failed { attr, msg },
                        }
                    }
                }
```

For `Response::SearchError`, take the attr from either variant (`Pending::Range { attr, .. } | Pending::Detected { attr, .. }`). Keep `insert_for_test(id, attr, min, max)` (inserts `Pending::Range`) and add

```rust
    #[cfg(test)]
    pub(crate) fn insert_detected_for_test(&mut self, id: u64, attr: String, spec: RangeSpec) {
        self.pending.insert(id, Pending::Detected { attr, spec });
    }
```

The `alloc_flow` test module needs `use crate::ldap::worker::{Request, Response, SearchScope, WorkerHandle};` — `WorkerHandle::recording()` is `pub(crate)` and `#[cfg(test)]` already.

`src/ui/app.rs` `open_create` — the placeholder loop becomes `for req in &autonum { … .find(|f| f.label.eq_ignore_ascii_case(req.attr())) … }` and the scan loop becomes

```rust
            for req in &autonum {
                let _ = match req {
                    crate::workflows::create::AllocRequest::Range { attr, min, max } => {
                        alloc_flow.request(w, &base_dn, attr, *min, *max)
                    }
                    crate::workflows::create::AllocRequest::Detected { attr, spec } => {
                        alloc_flow.request_detected(w, &base_dn, attr, spec.clone())
                    }
                };
            }
```

Fix every remaining compile error the new enum variants cause (the compiler lists them; `computed_defaults` and `live_templates` use `_ =>` and need no change).

- [ ] **Step 4: Run tests**

Run: `cargo test -j4 --lib`
Expected: PASS, including the new `detected_range_*`, `request_detected_posts_the_number_scan`, `apply_static_defaults_surfaces_detected_ranges`, and the unchanged `alloc_fills_next_free_number` / `alloc_refuses_truncated_scan`.

- [ ] **Step 5: Gate and commit**

Run: `CARGO_BUILD_JOBS=4 make check` → `All checks passed!`

```bash
git add src/config/defaults.rs src/workflows/ src/ui/app.rs
git commit -m "feat(create): allocate detected number ranges at create time

A detected {next:…} range is resolved from one full scan of uidNumber and
gidNumber, so private groups count as used numbers. A truncated scan
refuses with the existing message.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Rules B2–B5 and range defaults

**Files:**
- Create: `src/detect/patterns/posix.rs`, `src/detect/patterns/samba.rs`, `src/detect/patterns/pickers.rs`, `src/detect/patterns/ranges.rs`
- Modify: `src/detect/patterns/mod.rs` (full orchestration)

**Interfaces:**
- Consumes: Tasks 1–5 (`PrivateIndex`, `templates::infer_defaults`, `RangeSpec`, `DefaultValue::{Template, Literal, Computed, DetectedRange}`, `WidgetSpecCfg::{Picker, Lookup}`, `CandidateRef::Profile`).
- Produces:
  - `posix::all_posix_entries(sample: &Sample) -> Vec<SampleEntry>`
  - `posix::is_user_profile(p: &DetectedProfile) -> bool`, `posix::is_group_profile(p: &DetectedProfile) -> bool`
  - `posix::non_private(p: &DetectedProfile, index: &PrivateIndex) -> Vec<SampleEntry>`
  - `posix::apply_private_group(p: &mut DetectedProfile, index: &PrivateIndex) -> bool` (B2), `posix::apply_shared_gid(p: &mut DetectedProfile)` (B3 + the "no private groups" note)
  - `posix::count_without_private(p: &DetectedProfile, index: &PrivateIndex) -> usize`; the orchestrator stores it in `p.users_without_private_group` for every user profile (any size) when the lookups succeeded — rule D (Task 9) reads it
  - `samba::apply(p: &mut DetectedProfile)`, `pickers::apply(profiles: &mut [DetectedProfile])`, `ranges::apply(profiles: &mut [DetectedProfile])`

- [ ] **Step 1: Write the failing tests**

Add to `src/detect/patterns/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use crate::config::defaults::DefaultValue;
    use crate::config::{CandidateRef, WidgetSpecCfg};
    use crate::detect::fixtures::{argus_sample, container, demo_sample, e, schema};
    use crate::detect::infer::{detect, Detection};
    use crate::detect::model::{DetectedProfile, Sample};

    fn p<'a>(d: &'a Detection, name: &str) -> &'a DetectedProfile {
        d.profiles.iter().find(|p| p.name == name).unwrap_or_else(|| panic!("no {name}"))
    }
    fn dflt(p: &DetectedProfile, attr: &str) -> Option<String> {
        p.defaults.get(attr).map(|d| d.value.to_config_string())
    }
    fn cand(w: &WidgetSpecCfg) -> &str {
        match w {
            WidgetSpecCfg::Picker { candidate: CandidateRef::Profile(n), .. }
            | WidgetSpecCfg::Lookup { candidate: CandidateRef::Profile(n), .. } => n,
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn argus_users_get_private_groups_and_ranges() {
        let d = detect(&schema(), &argus_sample());
        let u = p(&d, "user-people");
        assert!(u.private_groups);
        assert_eq!(dflt(u, "gidNumber").as_deref(), Some("{uidNumber}"));
        assert_eq!(u.defaults["gidNumber"].evidence.ratio(), "12/12");
        let c = &u.companion.as_ref().expect("companion").value;
        assert_eq!(c.object_classes, vec!["posixGroup"]);
        assert_eq!(c.rdn_attr, "cn");
        assert_eq!(c.search_base, "ou=groups,dc=argus,dc=ch");
        assert_eq!(c.attributes["cn"], "{uid}");
        assert_eq!(c.attributes["gidNumber"], "{uidNumber}");
        assert_eq!(c.attributes["memberUid"], "{uid}");
        match &u.defaults["uidNumber"].value {
            DefaultValue::DetectedRange(s) => assert!(s.unified && !s.exclude_private),
            other => panic!("{other:?}"),
        }
        assert_eq!(cand(&u.widgets["gidNumber"].value), "posixgroup-groups");
        let g = p(&d, "posixgroup-groups");
        match &g.defaults["gidNumber"].value {
            DefaultValue::DetectedRange(s) => assert!(s.unified && s.exclude_private),
            other => panic!("{other:?}"),
        }
        assert_eq!(cand(&g.widgets["memberUid"].value), "user-people");
        // B1 over the 4 shared groups only: memberUid is never a default.
        assert!(!g.defaults.contains_key("memberUid"));
    }

    #[test]
    fn demo_shared_primary_group_samba_and_member_picker() {
        let d = detect(&schema(), &demo_sample());
        let up = p(&d, "user-people");
        assert!(!up.private_groups);
        assert_eq!(dflt(up, "gidNumber").as_deref(), Some("100"));
        assert_eq!(dflt(up, "sambaSID").as_deref(), Some("{auto:sambaSID}"));
        let uu = p(&d, "user-users");
        assert!(!uu.defaults.contains_key("gidNumber"));
        assert!(uu.notes.iter().any(|n| n.contains("gidNumber = uidNumber for 3/3, but no private groups found")), "{:?}", uu.notes);
        assert_eq!(cand(&p(&d, "group-groups").widgets["member"].value), "user-people");
        // Posix-user profile = the one with the most sampled entries.
        assert_eq!(cand(&p(&d, "posixgroup-groups").widgets["memberUid"].value), "user-people");
    }

    #[test]
    fn reverse_lookup_classifies_a_group_whose_user_was_not_sampled() {
        let mut s = argus_sample();
        // Drop the users container; the accounts arrive through the reverse lookup.
        let users = s.containers.remove(1).entries;
        s.accounts = users;
        let d = detect(&schema(), &s);
        let g = p(&d, "posixgroup-groups");
        let all = super::posix::all_posix_entries(&s);
        let idx = crate::detect::private::PrivateIndex::new(all.iter());
        assert_eq!(super::posix::non_private(g, &idx).len(), 4);
    }

    #[test]
    fn contrary_evidence_is_counted_for_every_user_profile() {
        let d = detect(&schema(), &argus_sample());
        assert_eq!(p(&d, "user-people").users_without_private_group, Some(0));
        let d = detect(&schema(), &demo_sample());
        assert_eq!(p(&d, "user-users").users_without_private_group, Some(3));
        assert_eq!(p(&d, "posixgroup-groups").users_without_private_group, None);
    }

    #[test]
    fn a_failed_lookup_skips_b2_with_a_note() {
        let mut s = argus_sample();
        s.lookup_error = Some("timeout".into());
        let d = detect(&schema(), &s);
        assert!(p(&d, "user-people").companion.is_none());
        assert!(d.notes.iter().any(|n| n.contains("timeout")));
    }

    #[test]
    fn small_groups_keep_names_and_widgets_but_infer_no_values() {
        let base = "ou=few,dc=x";
        let mk = |u: &str, n: &str| e(&format!("uid={u},{base}"), &[
            ("objectClass", &["inetOrgPerson", "posixAccount"]), ("uid", &[u]), ("cn", &[u]), ("sn", &["s"]),
            ("uidNumber", &[n]), ("gidNumber", &["100"]), ("loginShell", &["/bin/bash"])]);
        let grp = e("cn=g,ou=grp,dc=x", &[("objectClass", &["posixGroup"]), ("cn", &["g"]), ("gidNumber", &["100"])]);
        let s = Sample {
            containers: vec![container(base, vec![mk("a", "1"), mk("b", "2")]), container("ou=grp,dc=x", vec![grp])],
            ..Default::default()
        };
        let d = detect(&schema(), &s);
        let u = p(&d, "user-few");
        assert!(!u.defaults.contains_key("loginShell"));
        assert!(!u.defaults.contains_key("gidNumber"));
        assert_eq!(cand(&u.widgets["gidNumber"].value), "posixgroup-grp");
    }

    #[test]
    fn lower_case_server_spelling_still_detects_private_groups() {
        let mut s = argus_sample();
        for c in &mut s.containers {
            for en in &mut c.entries {
                let ocs = en.attrs.remove("objectClass").unwrap();
                en.attrs.insert("objectclass".into(), ocs.iter().map(|o| o.to_lowercase()).collect());
                if let Some(v) = en.attrs.remove("uidNumber") {
                    en.attrs.insert("UIDNUMBER".into(), v);
                }
            }
        }
        let d = detect(&schema(), &s);
        assert!(p(&d, "user-people").private_groups);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -j4 --lib detect::patterns`
Expected: FAIL (compile errors: `posix` module missing) — the argus/demo assertions describe behaviour B2–B5 add.

- [ ] **Step 3: Implement**

`src/detect/patterns/posix.rs`:

```rust
//! Rules B2 (user-private group) and B3 (shared primary group), spec §2B2–3.

use std::collections::{BTreeMap, HashSet};

use crate::config::defaults::{parse_default_value, DefaultValue};
use crate::config::CompanionSpec;
use crate::detect::model::{Detected, DetectedProfile, Evidence, Sample, SampleEntry};
use crate::detect::private::PrivateIndex;
use crate::detect::{more_than_half, most_common, rule_applies, MIN_SAMPLE};

fn has_class(p: &DetectedProfile, oc: &str) -> bool {
    p.object_classes.value.iter().any(|c| c.eq_ignore_ascii_case(oc))
}
pub fn is_user_profile(p: &DetectedProfile) -> bool {
    has_class(p, "posixAccount")
}
pub fn is_group_profile(p: &DetectedProfile) -> bool {
    has_class(p, "posixGroup")
}

/// Every posixAccount / posixGroup entry known: sampled plus looked up, deduped by DN.
pub fn all_posix_entries(sample: &Sample) -> Vec<SampleEntry> {
    let mut seen = HashSet::new();
    sample
        .containers
        .iter()
        .flat_map(|c| c.entries.iter())
        .chain(sample.groups.iter())
        .chain(sample.accounts.iter())
        .filter(|e| e.has_class("posixAccount") || e.has_class("posixGroup"))
        .filter(|e| seen.insert(e.dn.to_lowercase()))
        .cloned()
        .collect()
}

/// Sampled users of `p` without a private group (evaluated at any sample size).
pub fn count_without_private(p: &DetectedProfile, index: &PrivateIndex) -> usize {
    p.entries.iter().filter(|u| index.private_group_of(u).is_none()).count()
}

/// The profile's groups that are nobody's private group.
pub fn non_private(p: &DetectedProfile, index: &PrivateIndex) -> Vec<SampleEntry> {
    p.entries.iter().filter(|g| !index.is_private(g)).cloned().collect()
}

fn template(s: &str) -> DefaultValue {
    parse_default_value(s).expect("built-in template parses")
}

/// B2. Returns true when it applied.
pub fn apply_private_group(p: &mut DetectedProfile, index: &PrivateIndex) -> bool {
    let n = p.entries.len();
    if n < MIN_SAMPLE {
        return false;
    }
    let mut pairs: Vec<(SampleEntry, SampleEntry)> = Vec::new();
    let mut without: Vec<String> = Vec::new();
    for u in &p.entries {
        match index.private_group_of(u) {
            Some(g) => pairs.push((u.clone(), g.clone())),
            None => without.push(u.dn.clone()),
        }
    }
    if !rule_applies(pairs.len(), n) {
        return false;
    }
    let ev = Evidence::new(pairs.len(), n)
        .with_exceptions(without)
        .with_note("have a private group");
    let base = most_common(pairs.iter().filter_map(|(_, g)| g.parent()))
        .map(|(b, _)| b)
        .unwrap_or_default();
    let mut attributes = BTreeMap::from([
        ("cn".to_string(), "{uid}".to_string()),
        ("gidNumber".to_string(), "{uidNumber}".to_string()),
    ]);
    let with_member = pairs
        .iter()
        .filter(|(u, g)| {
            u.first("uid")
                .is_some_and(|uid| g.values("memberUid").iter().any(|m| m.trim().eq_ignore_ascii_case(uid)))
        })
        .count();
    if more_than_half(with_member, pairs.len()) {
        attributes.insert("memberUid".to_string(), "{uid}".to_string());
    }
    p.defaults.insert("gidNumber".to_string(), Detected::new(template("{uidNumber}"), ev.clone()));
    p.companion = Some(Detected::new(
        CompanionSpec {
            object_classes: vec!["posixGroup".to_string()],
            rdn_attr: "cn".to_string(),
            search_base: base,
            attributes,
        },
        ev,
    ));
    p.private_groups = true;
    true
}

/// B3 (a shared `gidNumber` becomes a literal), else the gid = uid note.
pub fn apply_shared_gid(p: &mut DetectedProfile) {
    let n = p.entries.len();
    if n < MIN_SAMPLE {
        return;
    }
    if let Some((gid, k)) = most_common(p.entries.iter().filter_map(|u| u.first("gidNumber"))) {
        if rule_applies(k, n) {
            let exceptions = p
                .entries
                .iter()
                .filter(|u| u.first("gidNumber") != Some(gid.as_str()))
                .map(|u| u.dn.clone())
                .collect();
            p.defaults.insert(
                "gidNumber".to_string(),
                Detected::new(DefaultValue::Literal(gid), Evidence::new(k, n).with_exceptions(exceptions)),
            );
            return;
        }
    }
    let same = p
        .entries
        .iter()
        .filter(|u| u.first("gidNumber").is_some() && u.first("gidNumber") == u.first("uidNumber"))
        .count();
    if rule_applies(same, n) {
        p.notes.push(format!("gidNumber = uidNumber for {same}/{n}, but no private groups found"));
    }
}
```

`src/detect/patterns/samba.rs`:

```rust
//! Rule B4 (spec §2B4): sambaSamAccount profiles compute their SID.

use crate::config::defaults::{ComputedKind, DefaultValue};
use crate::detect::model::{Detected, DetectedProfile, Evidence};

pub fn apply(p: &mut DetectedProfile) {
    let samba = p.object_classes.value.iter().any(|c| c.eq_ignore_ascii_case("sambaSamAccount"));
    if samba && !p.defaults.keys().any(|k| k.eq_ignore_ascii_case("sambaSID")) {
        p.defaults.insert(
            "sambaSID".to_string(),
            Detected::new(
                DefaultValue::Computed(ComputedKind::SambaSid),
                Evidence::new(p.sampled, p.sampled).with_note("sambaSamAccount profile"),
            ),
        );
    }
}
```

`src/detect/patterns/pickers.rs`:

```rust
//! Rule B5 (spec §2B5): picker and lookup targets.

use crate::config::{CandidateRef, WidgetSpecCfg};
use crate::detect::model::{Detected, DetectedProfile, Evidence};
use crate::detect::patterns::posix::{is_group_profile, is_user_profile};
use crate::detect::{dn_eq, more_than_half, most_common};

/// The name of the matching profile with the most sampled entries (ties: name).
fn largest(profiles: &[DetectedProfile], pred: impl Fn(&DetectedProfile) -> bool) -> Option<String> {
    profiles
        .iter()
        .filter(|p| pred(p))
        .max_by(|a, b| a.sampled.cmp(&b.sampled).then_with(|| b.name.cmp(&a.name)))
        .map(|p| p.name.clone())
}

fn picker(candidate: &str, store: &str) -> WidgetSpecCfg {
    WidgetSpecCfg::Picker {
        candidate: CandidateRef::Profile(candidate.to_string()),
        store: store.to_string(),
        select: "multi".to_string(),
    }
}

pub fn apply(profiles: &mut [DetectedProfile]) {
    let user = largest(profiles, is_user_profile);
    let group = largest(profiles, is_group_profile);
    let containers: Vec<(String, String, usize)> = profiles
        .iter()
        .map(|p| (p.container.clone(), p.name.clone(), p.sampled))
        .collect();
    let target_in = |container: &str| -> Option<String> {
        containers
            .iter()
            .filter(|(c, _, _)| dn_eq(c, container))
            .max_by(|a, b| a.2.cmp(&b.2).then_with(|| b.1.cmp(&a.1)))
            .map(|(_, n, _)| n.clone())
    };
    for p in profiles.iter_mut() {
        let has = |oc: &str| p.object_classes.value.iter().any(|c| c.eq_ignore_ascii_case(oc));
        let guard = Evidence::new(p.sampled, p.sampled);
        let mut add: Vec<(String, Detected<WidgetSpecCfg>)> = Vec::new();
        if has("posixGroup") {
            if let Some(u) = &user {
                add.push(("memberUid".into(), Detected::new(picker(u, "uid"), guard.clone().with_note("posix-user profile"))));
            }
        }
        if has("posixAccount") {
            if let Some(g) = &group {
                add.push((
                    "gidNumber".into(),
                    Detected::new(
                        WidgetSpecCfg::Lookup {
                            candidate: CandidateRef::Profile(g.clone()),
                            store: "gidNumber".into(),
                            label: Some("{cn}".into()),
                        },
                        guard.clone().with_note("posix-group profile"),
                    ),
                ));
            }
        }
        for (attr, oc) in [("member", "groupOfNames"), ("uniqueMember", "groupOfUniqueNames")] {
            if !has(oc) {
                continue;
            }
            let dns: Vec<&str> = p
                .entries
                .iter()
                .flat_map(|e| e.values(attr).iter().map(String::as_str))
                .collect();
            let parents: Vec<&str> = dns.iter().filter_map(|d| crate::detect::parent_dn(d)).collect();
            if let Some((container, k)) = most_common(parents) {
                if more_than_half(k, dns.len()) {
                    if let Some(target) = target_in(&container) {
                        add.push((attr.into(), Detected::new(picker(&target, "dn"), Evidence::new(k, dns.len()).with_note(format!("member DNs in {container}")))));
                    }
                }
            }
        }
        for (attr, w) in add {
            p.widgets.entry(attr).or_insert(w);
        }
    }
}
```

`src/detect/patterns/ranges.rs`:

```rust
//! Emit rule-C defaults: posix users get a detected `uidNumber` range, posix
//! groups a detected `gidNumber` range (resolved at create time).

use crate::config::defaults::DefaultValue;
use crate::detect::model::{Detected, DetectedProfile, Evidence};
use crate::detect::patterns::posix::{is_group_profile, is_user_profile};
use crate::detect::range::RangeSpec;

pub fn apply(profiles: &mut [DetectedProfile]) {
    let unified_any = profiles.iter().any(|p| p.private_groups);
    for p in profiles.iter_mut() {
        let (attr, unified, exclude_private) = if is_user_profile(p) {
            ("uidNumber", p.private_groups, false)
        } else if is_group_profile(p) {
            ("gidNumber", unified_any, true)
        } else {
            continue;
        };
        if p.defaults.keys().any(|k| k.eq_ignore_ascii_case(attr)) {
            continue;
        }
        let spec = RangeSpec {
            attr: attr.to_string(),
            container: p.container.clone(),
            structural: p.structural.clone(),
            unified,
            exclude_private,
        };
        p.defaults.insert(
            attr.to_string(),
            Detected::new(DefaultValue::DetectedRange(spec), Evidence::new(0, 0).with_note("range detected at create time")),
        );
    }
}
```

Replace `src/detect/patterns/mod.rs` (keep its tests):

```rust
//! Known patterns (spec §2B) applied to detected profiles.

pub mod pickers;
pub mod posix;
pub mod ranges;
pub mod samba;
pub mod templates;

use crate::detect::model::{DetectedProfile, Sample};
use crate::detect::private::PrivateIndex;
use crate::schema::SchemaModel;

/// Run every pattern over `profiles` (names are already assigned).
pub fn apply(schema: &SchemaModel, sample: &Sample, profiles: &mut [DetectedProfile], notes: &mut Vec<String>) {
    let all = posix::all_posix_entries(sample);
    let index = PrivateIndex::new(all.iter());
    let lookups_ok = sample.lookup_error.is_none();
    if let Some(e) = &sample.lookup_error {
        notes.push(format!("private-group rules skipped: the private-group lookup failed ({e})"));
    }
    for p in profiles.iter_mut() {
        // B1 for a posixGroup profile uses its shared (non-private) groups only.
        let b1_entries = if posix::is_group_profile(p) && lookups_ok {
            let shared = posix::non_private(p, &index);
            let private = p.entries.len() - shared.len();
            if private > 0 {
                p.notes.push(format!(
                    "{private} of {} groups are user-private groups; defaults and the number range use the other {}",
                    p.entries.len(),
                    shared.len()
                ));
            }
            shared
        } else {
            p.entries.clone()
        };
        let (defaults, dropped) = templates::infer_defaults(schema, &b1_entries, &p.rdn_attr.value);
        p.notes.extend(dropped);
        p.defaults.extend(defaults);
        if posix::is_user_profile(p) {
            if lookups_ok {
                p.users_without_private_group = Some(posix::count_without_private(p, &index));
            }
            if !(lookups_ok && posix::apply_private_group(p, &index)) {
                posix::apply_shared_gid(p);
            }
        }
        samba::apply(p);
    }
    pickers::apply(profiles);
    ranges::apply(profiles);
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -j4 --lib detect::`
Expected: PASS (all detect tests, including Task 3's, which still hold because B1 runs first and never touches `gidNumber`).

- [ ] **Step 5: Gate and commit**

Run: `CARGO_BUILD_JOBS=4 make check` → `All checks passed!`

```bash
git add src/detect/
git commit -m "feat(detect): private groups, shared gid, Samba SID, pickers and ranges (B2-B5, C)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---
### Task 7: Config — `ProfileOverride`, `[detect]`, container scope, offline validation

**Files:**
- Modify: `src/config/mod.rs` (`Config` ~26-44, `EntryProfile` ~191-221, `validate_companions` ~344-381, `Config::load`)
- Modify: `src/workflows/create.rs:265-283` (`profiles_for_container`)
- Modify (mechanical): every `Config { … }` literal (19 in `tests/*.rs`) and every full `EntryProfile { … companion: None, }` literal (`src/config/mod.rs:692`, `src/config/relation.rs:135`, `src/ui/state.rs:1830`, `src/workflows/test_fixtures.rs:20,52`, `src/workflows/create.rs:563,970`, `tests/tv_create.rs:220`)

**Interfaces:**
- Produces:
  - `config::ContainerScope { Boundary (default), Exact }`; `EntryProfile.scope: ContainerScope` (`#[serde(skip)]`)
  - `config::DetectConfig { pub enabled: bool }` (default `true`)
  - `config::ProfileOverride { name: String, enabled: Option<bool>, suppress: Vec<String>, object_classes: Option<Vec<String>>, rdn_attr: Option<String>, search_base: Option<String>, show: Option<Vec<String>>, search_attrs: Option<Vec<String>>, defaults: Option<ProfileDefaults>, widgets: Option<BTreeMap<String, WidgetSpecCfg>>, label: Option<String>, companion: Option<CompanionSpec> }` with `is_enabled()` and `to_entry_profile() -> Option<EntryProfile>`
  - `Config.profiles: Vec<EntryProfile>` = the config-only profiles exactly as before (blocks with `object_classes`, not disabled); `Config.overrides: Vec<ProfileOverride>` = every block; `Config.detect: DetectConfig`
  - `config::check_companion(who: &str, c: &CompanionSpec) -> Result<(), String>`

- [ ] **Step 1: Write the failing tests** (append to `src/config/mod.rs` tests)

```rust
    const CONN: &str = "[server]\nuri = \"ldap://x\"\nbase_dn = \"dc=x\"\n[auth]\nbind_dn = \"cn=a,dc=x\"\n";

    #[test]
    fn override_with_only_name_and_enabled_parses() {
        let cfg: Config = toml::from_str(&format!("{CONN}[[profile]]\nname = \"user-people\"\nenabled = false\n")).unwrap();
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
    fn check_rejects_a_next_autonumber_in_an_override_only_companion() {
        let toml = format!(
            "{CONN}[[profile]]\nname = \"user-people\"\n[profile.companion]\nobject_classes = [\"posixGroup\"]\nrdn_attr = \"cn\"\nsearch_base = \"ou=g,dc=x\"\n[profile.companion.attributes]\ncn = \"{{uid}}\"\ngidNumber = \"{{next:1-9}}\"\n"
        );
        let err = parse_config_str(&toml).unwrap_err();
        assert!(err.to_string().contains("autonumber"), "{err}");
    }
```

In `src/workflows/create.rs` tests:

```rust
    #[test]
    fn exact_scope_matches_only_its_own_container() {
        let mut p = prof("dc=example,dc=org");
        p.scope = crate::config::ContainerScope::Exact;
        let ps = vec![p];
        assert_eq!(profiles_for_container(&ps, "DC=Example, dc=org"), vec![0]);
        assert!(profiles_for_container(&ps, "ou=people,dc=example,dc=org").is_empty());
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -j4 --lib config:: workflows::create`
Expected: FAIL to compile (`no field overrides`, `ContainerScope` unknown).

- [ ] **Step 3: Implement**

In `src/config/mod.rs`:

```rust
/// Where a profile's `New` is offered: `Boundary` = the container, its ancestors
/// and descendants (config and matched profiles); `Exact` = only the container
/// itself (detected-only profiles, spec §2A "Container scope").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ContainerScope {
    #[default]
    Boundary,
    Exact,
}
```

Add to `EntryProfile` (last field):

```rust
    /// Container scope for `New`; never read from TOML.
    #[serde(skip)]
    pub scope: ContainerScope,
```

Add:

```rust
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
```

Turn `Config` into a `try_from` type — keep the doc comments, drop the field-level serde attributes, add the two fields:

```rust
#[derive(Debug, Deserialize)]
#[serde(try_from = "RawConfig")]
pub struct Config {
    pub meta: MetaConfig,
    pub server: ServerConfig,
    pub auth: AuthConfig,
    /// The config's own complete profiles (blocks with `object_classes`, not
    /// disabled), in file order — what eDAPtor uses when detection is off.
    pub profiles: Vec<EntryProfile>,
    /// Every `[[profile]]` block as written (input to the merge).
    pub overrides: Vec<ProfileOverride>,
    pub detect: DetectConfig,
    pub samba: SambaConfig,
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
```

Split `validate_companions` so the merge can reuse the per-companion check; keep the messages byte-identical:

```rust
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
    if !c.attributes.keys().any(|k| k.eq_ignore_ascii_case(&c.rdn_attr)) {
        return Err(format!("{who}: rdn_attr '{}' must be one of the companion attributes", c.rdn_attr));
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

/// Validate every block's companion, including override-only blocks, so
/// `edaptor check` (which does not detect) still reports them.
fn validate_companions(overrides: &[ProfileOverride]) -> Result<()> {
    for o in overrides {
        if let Some(c) = &o.companion {
            check_companion(&format!("profile '{}' companion", o.name), c).map_err(|e| anyhow::anyhow!(e))?;
        }
    }
    Ok(())
}
```

`Config::load` calls `validate_companions(&config.overrides)?;`.

`src/workflows/create.rs` `profiles_for_container`:

```rust
        .filter(|(_, p)| {
            !p.search_base.is_empty()
                && match p.scope {
                    crate::config::ContainerScope::Boundary => dn_boundary_match(&p.search_base, container_dn),
                    crate::config::ContainerScope::Exact => crate::detect::dn_eq(&p.search_base, container_dn),
                }
        })
```

(update its doc comment: "…`Exact`-scoped profiles only match their own container").

Mechanical literal updates:

```bash
sed -i 's/^\(\s*\)profiles: Vec::new(),$/\1profiles: Vec::new(),\n\1overrides: Vec::new(),\n\1detect: Default::default(),/' tests/*.rs
sed -i 's/^\(\s*\)companion: None,$/\1companion: None,\n\1scope: Default::default(),/' \
  src/config/mod.rs src/config/relation.rs src/ui/state.rs src/workflows/test_fixtures.rs src/workflows/create.rs tests/tv_create.rs
cargo build -j4 --all-targets 2>&1 | grep -E '^error' | head
```

The `companion: None,` sed must not touch `ProfileOverride`/`CompanionSpec` code (it only matches literal lines ending in `companion: None,`); if the build reports a duplicate `scope` field (a literal already using `..Default::default()` after `companion: None,` is fine; a duplicate is not), remove the duplicate. Fix any remaining `Config` literal the build reports the same way.

- [ ] **Step 4: Run tests**

Run: `cargo test -j4 --lib config:: workflows::create`
Expected: PASS, including the existing `single_string_object_class_is_a_parse_error`, `bad_default_value_fails_whole_config_parse`, `companion_*_is_rejected`, `demo_config_widgets_resolve`, `reference_config_parses`.

- [ ] **Step 5: Gate and commit**

Run: `CARGO_BUILD_JOBS=4 make check` → `All checks passed!`

```bash
git add -u
git commit -m "feat(config): optional-field profile overrides, [detect] table, container scope

[[profile]] blocks now parse into ProfileOverride; Config.profiles keeps the
old config-only list, so behaviour with detection off is unchanged.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Merge, suppression, provenance and per-origin validation

**Files:**
- Create: `src/detect/merge.rs`
- Modify: `src/detect/model.rs` (add `DetectedProfile::to_entry_profile`), `src/detect/mod.rs` (`pub mod merge;`)
- Modify: `src/config/widget.rs:90` and `src/config/resolver.rs:196` (case-insensitive candidate names)

**Interfaces:**
- Consumes: Task 7 (`ProfileOverride`, `ContainerScope`, `check_companion`), `DefaultValue::to_config_string`, `SchemaModel::structural_class`.
- Produces:
  - `DetectedProfile::to_entry_profile(&self) -> EntryProfile` (scope `Exact`)
  - `merge::Source { Config, Detected(Evidence), ConfigOverDetected { detected: String, evidence: Evidence } }`
  - `merge::Origin { Config, Detected { container, sampled, partial }, Merged { detected, container, sampled, partial } }`
  - `merge::Provenance { name, origin, fields: BTreeMap<String, Source>, suppressed: Vec<String>, pending_suppress: Vec<String>, notes: Vec<String> }`
  - `merge::merge_core(..) -> Merged` (pending suppress paths kept) and `merge::flush_pending(m: &mut Merged)`, both `pub(crate)`; `merge` = `merge_core` + `flush_pending`
  - `merge::Merged { profiles: Vec<EntryProfile>, provenance: Vec<Provenance>, disabled: Vec<String>, warnings: Vec<String>, dropped: Vec<String> }`
  - `merge::merge(schema: &SchemaModel, detected: &[DetectedProfile], overrides: &[ProfileOverride]) -> Merged`
  - `merge::validate(m: &mut Merged) -> Result<(), String>`
  - Field keys in `Provenance.fields`: `object_classes rdn_attr search_base show search_attrs label companion defaults.<attr> widget.<attr>`

- [ ] **Step 1: Write the failing tests** (in `src/detect/merge.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::fixtures::{argus_sample, demo_sample, schema};
    use crate::detect::infer::detect;

    fn overrides(toml: &str) -> Vec<ProfileOverride> {
        #[derive(serde::Deserialize)]
        struct W {
            #[serde(default)]
            profile: Vec<ProfileOverride>,
        }
        toml::from_str::<W>(toml).expect("overrides parse").profile
    }
    fn demo() -> Vec<DetectedProfile> {
        detect(&schema(), &demo_sample()).profiles
    }
    fn names(m: &Merged) -> Vec<&str> {
        m.profiles.iter().map(|p| p.name.as_str()).collect()
    }
    fn get<'a>(m: &'a Merged, name: &str) -> (&'a EntryProfile, &'a Provenance) {
        let i = m.profiles.iter().position(|p| p.name == name).unwrap_or_else(|| panic!("no {name}"));
        (&m.profiles[i], &m.provenance[i])
    }

    #[test]
    fn detected_only_order_and_scope() {
        let m = merge(&schema(), &demo(), &[]);
        let n = names(&m);
        let pos = |x: &str| n.iter().position(|y| *y == x).unwrap();
        assert!(pos("user-people") < pos("user-users"), "{n:?}");
        assert!(m.profiles.iter().all(|p| p.scope == ContainerScope::Exact));
        assert!(matches!(m.provenance[0].origin, Origin::Detected { .. }));
    }

    #[test]
    fn match_by_name_is_case_insensitive_and_keeps_the_config_name() {
        let m = merge(&schema(), &demo(), &overrides("[[profile]]\nname = \"USER-People\"\n[profile.defaults]\nloginShell = \"/bin/sh\"\n"));
        assert_eq!(m.profiles[0].name, "USER-People");
        assert_eq!(m.profiles[0].scope, ContainerScope::Boundary);
        assert!(!names(&m).contains(&"user-people"));
        let (p, prov) = get(&m, "USER-People");
        assert_eq!(p.defaults.entries["loginShell"].to_config_string(), "/bin/sh");
        assert!(matches!(prov.fields["defaults.loginShell"], Source::ConfigOverDetected { .. }));
        assert!(matches!(prov.fields["defaults.homeDirectory"], Source::Detected(_)));
    }

    #[test]
    fn match_by_search_base_and_structural_class() {
        let cfg: crate::config::Config = toml::from_str(include_str!("../../examples/demo-config.toml")).unwrap();
        let m = merge(&schema(), &demo(), &cfg.overrides);
        let n = names(&m);
        assert_eq!(&n[..3], &["user", "group", "posixgroup"]);
        assert!(!n.contains(&"user-people") && !n.contains(&"group-groups") && !n.contains(&"posixgroup-groups"));
        let (user, _) = get(&m, "user");
        assert_eq!(user.defaults.entries["uidNumber"].to_config_string(), "{next:10000-60000}");
        assert_eq!(user.defaults.entries["sambaSID"].to_config_string(), "{auto:sambaSID}");
        assert!(matches!(user.widgets["gidNumber"], WidgetSpecCfg::Lookup { .. }));
    }

    #[test]
    fn rename_rewrites_detected_candidates() {
        let mut m = merge(&schema(), &demo(), &overrides(
            "[[profile]]\nname = \"people\"\nobject_classes = [\"inetOrgPerson\", \"posixAccount\"]\nsearch_base = \"ou=people,dc=example,dc=org\"\n",
        ));
        let (g, _) = get(&m, "posixgroup-groups");
        match &g.widgets["memberUid"] {
            WidgetSpecCfg::Picker { candidate: CandidateRef::Profile(n), .. } => assert_eq!(n, "people"),
            other => panic!("{other:?}"),
        }
        validate(&mut m).unwrap();
        crate::config::widget::resolve_widgets(&m.profiles).expect("no widget config error");
    }

    #[test]
    fn companion_is_replaced_as_a_whole() {
        let d = detect(&schema(), &argus_sample()).profiles;
        let m = merge(&schema(), &d, &overrides(
            "[[profile]]\nname = \"user-people\"\n[profile.companion]\nobject_classes = [\"posixGroup\"]\nrdn_attr = \"cn\"\nsearch_base = \"ou=other,dc=argus,dc=ch\"\n[profile.companion.attributes]\ncn = \"{uid}\"\n",
        ));
        let (p, prov) = get(&m, "user-people");
        let c = p.companion.as_ref().unwrap();
        assert_eq!(c.search_base, "ou=other,dc=argus,dc=ch");
        assert!(!c.attributes.contains_key("gidNumber"));
        assert!(matches!(prov.fields["companion"], Source::ConfigOverDetected { .. }));
    }

    #[test]
    fn suppress_each_path_kind() {
        let d = detect(&schema(), &argus_sample()).profiles;
        let m = merge(&schema(), &d, &overrides(
            "[[profile]]\nname = \"user-people\"\nsuppress = [\"companion\", \"defaults.loginShell\", \"widget.gidNumber\", \"label\", \"show\", \"search_attrs\", \"defaults.nope\", \"bogus\"]\n",
        ));
        let (p, prov) = get(&m, "user-people");
        assert!(p.companion.is_none());
        assert!(!p.defaults.entries.contains_key("loginShell"));
        assert!(!p.widgets.contains_key("gidNumber"));
        assert!(p.label.is_none() && p.show.is_empty() && p.search_attrs.is_empty());
        assert_eq!(prov.suppressed.len(), 6);
        assert!(m.warnings.iter().any(|w| w.contains("\"defaults.nope\" matches nothing detected")));
        assert!(m.warnings.iter().any(|w| w.contains("unknown suppress path \"bogus\"")));
    }

    #[test]
    fn suppress_never_removes_config_parts() {
        let m = merge(&schema(), &demo(), &overrides(
            "[[profile]]\nname = \"user-people\"\nsuppress = [\"defaults.loginShell\"]\n[profile.defaults]\nloginShell = \"/bin/sh\"\n",
        ));
        let (p, _) = get(&m, "user-people");
        assert_eq!(p.defaults.entries["loginShell"].to_config_string(), "/bin/sh");
        assert!(m.warnings.iter().any(|w| w.contains("matches nothing detected")));
    }

    #[test]
    fn enabled_false_drops_the_profile() {
        let m = merge(&schema(), &demo(), &overrides("[[profile]]\nname = \"user-users\"\nenabled = false\n"));
        assert!(!names(&m).contains(&"user-users"));
        assert_eq!(m.disabled, vec!["user-users"]);
    }

    #[test]
    fn unmatched_block_without_object_classes_is_dropped_with_a_warning() {
        let m = merge(&schema(), &demo(), &overrides("[[profile]]\nname = \"ghost\"\n"));
        assert!(!names(&m).contains(&"ghost"));
        assert!(m.warnings.iter().any(|w| w == "profile \"ghost\" matches no detected profile"));
    }

    #[test]
    fn two_config_blocks_for_one_detected_profile() {
        let m = merge(&schema(), &demo(), &overrides(
            "[[profile]]\nname = \"staff\"\nobject_classes = [\"inetOrgPerson\"]\nsearch_base = \"ou=people,dc=example,dc=org\"\n[[profile]]\nname = \"samba-staff\"\nobject_classes = [\"inetOrgPerson\", \"sambaSamAccount\"]\nsearch_base = \"ou=people,dc=example,dc=org\"\n",
        ));
        assert!(names(&m).starts_with(&["staff", "samba-staff"]));
        assert!(matches!(get(&m, "samba-staff").1.origin, Origin::Config));
        assert!(m.warnings.iter().any(|w| w.contains("\"samba-staff\" also matches detected \"user-people\", already merged into \"staff\"")));
    }

    #[test]
    fn config_search_base_wins_and_the_range_follows_it() {
        let m = merge(&schema(), &demo(), &overrides("[[profile]]\nname = \"user-people\"\nsearch_base = \"ou=staff,dc=example,dc=org\"\n"));
        let (p, _) = get(&m, "user-people");
        assert_eq!(p.search_base, "ou=staff,dc=example,dc=org");
        match &p.defaults.entries["uidNumber"] {
            DefaultValue::DetectedRange(s) => assert_eq!(s.container, "ou=staff,dc=example,dc=org"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn validate_drops_detected_parts_but_rejects_config_parts() {
        let mut d = demo();
        let i = d.iter().position(|p| p.name == "posixgroup-groups").unwrap();
        d[i].widgets.insert("memberUid".into(), crate::detect::model::Detected::new(
            WidgetSpecCfg::Picker { candidate: CandidateRef::Profile("ghost".into()), store: "uid".into(), select: "multi".into() },
            Evidence::new(1, 1),
        ));
        let mut m = merge(&schema(), &d, &[]);
        validate(&mut m).unwrap();
        assert!(!get(&m, "posixgroup-groups").0.widgets.contains_key("memberUid"));
        assert_eq!(m.dropped.len(), 1);
        let mut bad = merge(&schema(), &demo(), &overrides(
            "[[profile]]\nname = \"x\"\nobject_classes = [\"posixGroup\"]\n[profile.widget.memberUid]\nkind = \"picker\"\ncandidate = \"ghost\"\n",
        ));
        assert!(validate(&mut bad).unwrap_err().contains("unknown candidate profile \"ghost\""));
    }
}
```

In `src/config/widget.rs` tests add:

```rust
    #[test]
    fn candidate_profile_names_are_case_insensitive() {
        let mut target = crate::workflows::test_fixtures::bare_profile("posixgroup");
        target.search_base = "ou=g,dc=x".into();
        let mut owner = crate::workflows::test_fixtures::bare_profile("user");
        owner.object_classes = vec!["posixAccount".into()];
        owner.widgets.insert("gidNumber".into(), crate::config::WidgetSpecCfg::Lookup {
            candidate: crate::config::CandidateRef::Profile("PosixGroup".into()),
            store: "gidNumber".into(),
            label: None,
        });
        assert!(resolve_widgets(&[owner, target]).is_ok());
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -j4 --lib detect::merge config::widget`
Expected: FAIL to compile (`cannot find function merge`), and after that the widget test fails with `unknown candidate profile "PosixGroup"`.

- [ ] **Step 3: Implement**

`src/config/widget.rs:90`: `.find(|p| p.name.eq_ignore_ascii_case(name))`. `src/config/resolver.rs:196`: `self.profiles.iter().find(|p| p.name.eq_ignore_ascii_case(other)).map(scope_of)`.

`src/detect/model.rs` — add:

```rust
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
                entries: self.defaults.iter().map(|(k, d)| (k.clone(), d.value.clone())).collect(),
            },
            widgets: self.widgets.iter().map(|(k, d)| (k.clone(), d.value.clone())).collect(),
            label: self.label.clone(),
            companion: self.companion.as_ref().map(|c| c.value.clone()),
            scope: crate::config::ContainerScope::Exact,
        }
    }
}
```

Create `src/detect/merge.rs` (tests from Step 1 at the bottom):

```rust
//! Merge config `[[profile]]` blocks over detected profiles (spec §3). Pure.

use std::collections::{BTreeMap, HashMap};

use crate::config::defaults::DefaultValue;
use crate::config::{CandidateRef, ContainerScope, EntryProfile, ProfileOverride, WidgetSpecCfg};
use crate::detect::dn_eq;
use crate::detect::model::{DetectedProfile, Evidence};
use crate::schema::SchemaModel;

#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    Config,
    Detected(Evidence),
    ConfigOverDetected { detected: String, evidence: Evidence },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    Config,
    Detected { container: String, sampled: usize, partial: bool },
    Merged { detected: String, container: String, sampled: usize, partial: bool },
}

#[derive(Debug, Clone)]
pub struct Provenance {
    pub name: String,
    pub origin: Origin,
    pub fields: BTreeMap<String, Source>,
    pub suppressed: Vec<String>,
    /// `suppress` paths that matched nothing yet. Rule D (Task 9) may still add
    /// the part; `flush_pending` retries them and warns about the rest.
    pub pending_suppress: Vec<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Default)]
pub struct Merged {
    pub profiles: Vec<EntryProfile>,
    /// Parallel to `profiles`.
    pub provenance: Vec<Provenance>,
    /// Names of profiles removed by `enabled = false`.
    pub disabled: Vec<String>,
    pub warnings: Vec<String>,
    /// Detected parts dropped by `validate` (also in `warnings`).
    pub dropped: Vec<String>,
}

fn candidate_mut(spec: &mut WidgetSpecCfg) -> Option<&mut CandidateRef> {
    match spec {
        WidgetSpecCfg::Picker { candidate, .. }
        | WidgetSpecCfg::Membership { candidate, .. }
        | WidgetSpecCfg::Lookup { candidate, .. } => Some(candidate),
        _ => None,
    }
}

fn candidate_name(spec: &WidgetSpecCfg) -> Option<&str> {
    match spec {
        WidgetSpecCfg::Picker { candidate: CandidateRef::Profile(n), .. }
        | WidgetSpecCfg::Membership { candidate: CandidateRef::Profile(n), .. }
        | WidgetSpecCfg::Lookup { candidate: CandidateRef::Profile(n), .. } => Some(n),
        _ => None,
    }
}

fn is_sentinel(name: &str) -> bool {
    name.len() > 1 && name.starts_with('_') && name.ends_with('_')
}

fn widget_kind(spec: &WidgetSpecCfg) -> &'static str {
    match spec {
        WidgetSpecCfg::Choice { .. } => "choice",
        WidgetSpecCfg::Password { .. } => "password",
        WidgetSpecCfg::Picker { .. } => "picker",
        WidgetSpecCfg::Membership { .. } => "membership",
        WidgetSpecCfg::Lookup { .. } => "lookup",
        WidgetSpecCfg::Readonly => "readonly",
        WidgetSpecCfg::XOrdered => "x_ordered",
        WidgetSpecCfg::SambaSid => "samba_sid",
    }
}

fn rewrite_candidates(widgets: &mut BTreeMap<String, WidgetSpecCfg>, rename: &HashMap<String, String>) {
    for spec in widgets.values_mut() {
        if let Some(CandidateRef::Profile(name)) = candidate_mut(spec) {
            if let Some(new) = rename.get(&name.to_lowercase()) {
                *name = new.clone();
            }
        }
    }
}

fn detected_fields(d: &DetectedProfile) -> BTreeMap<String, Source> {
    let whole = || Source::Detected(Evidence::new(d.sampled, d.sampled));
    let mut f = BTreeMap::new();
    f.insert("object_classes".to_string(), Source::Detected(d.object_classes.evidence.clone()));
    f.insert("rdn_attr".to_string(), Source::Detected(d.rdn_attr.evidence.clone()));
    f.insert("search_base".to_string(), whole());
    if !d.show.is_empty() {
        f.insert("show".to_string(), whole());
    }
    if !d.search_attrs.is_empty() {
        f.insert("search_attrs".to_string(), whole());
    }
    if d.label.is_some() {
        f.insert("label".to_string(), whole());
    }
    for (k, v) in &d.defaults {
        f.insert(format!("defaults.{k}"), Source::Detected(v.evidence.clone()));
    }
    for (k, v) in &d.widgets {
        f.insert(format!("widget.{k}"), Source::Detected(v.evidence.clone()));
    }
    if let Some(c) = &d.companion {
        f.insert("companion".to_string(), Source::Detected(c.evidence.clone()));
    }
    f
}

fn config_fields(p: &EntryProfile) -> BTreeMap<String, Source> {
    let mut f = BTreeMap::new();
    for (k, set) in [
        ("object_classes", !p.object_classes.is_empty()),
        ("rdn_attr", !p.rdn_attr.is_empty()),
        ("search_base", !p.search_base.is_empty()),
        ("show", !p.show.is_empty()),
        ("search_attrs", !p.search_attrs.is_empty()),
        ("label", p.label.is_some()),
        ("companion", p.companion.is_some()),
    ] {
        if set {
            f.insert(k.to_string(), Source::Config);
        }
    }
    for k in p.defaults.entries.keys() {
        f.insert(format!("defaults.{k}"), Source::Config);
    }
    for k in p.widgets.keys() {
        f.insert(format!("widget.{k}"), Source::Config);
    }
    f
}

/// Record that `key` now comes from the config; remember the detected value.
fn mark_config(fields: &mut BTreeMap<String, Source>, key: &str, detected: Option<String>) {
    let src = match (fields.remove(key), detected) {
        (Some(Source::Detected(evidence)), Some(detected)) => Source::ConfigOverDetected { detected, evidence },
        _ => Source::Config,
    };
    fields.insert(key.to_string(), src);
}

fn take_ci<V>(map: &mut BTreeMap<String, V>, key: &str) -> Option<(String, V)> {
    let k = map.keys().find(|k| k.eq_ignore_ascii_case(key))?.clone();
    map.remove(&k).map(|v| (k, v))
}

fn merge_one(d: &DetectedProfile, o: &ProfileOverride, rename: &HashMap<String, String>) -> (EntryProfile, Provenance) {
    let mut p = d.to_entry_profile();
    rewrite_candidates(&mut p.widgets, rename);
    let mut fields = detected_fields(d);
    p.name = o.name.clone();
    p.scope = ContainerScope::Boundary;
    if let Some(v) = &o.object_classes {
        mark_config(&mut fields, "object_classes", Some(format!("{:?}", p.object_classes)));
        p.object_classes = v.clone();
    }
    if let Some(v) = &o.rdn_attr {
        mark_config(&mut fields, "rdn_attr", Some(format!("{:?}", p.rdn_attr)));
        p.rdn_attr = v.clone();
    }
    if let Some(v) = &o.search_base {
        mark_config(&mut fields, "search_base", Some(format!("{:?}", p.search_base)));
        p.search_base = v.clone();
    }
    if let Some(v) = &o.show {
        mark_config(&mut fields, "show", Some(format!("{:?}", p.show)));
        p.show = v.clone();
    }
    if let Some(v) = &o.search_attrs {
        mark_config(&mut fields, "search_attrs", Some(format!("{:?}", p.search_attrs)));
        p.search_attrs = v.clone();
    }
    if let Some(v) = &o.label {
        mark_config(&mut fields, "label", p.label.as_ref().map(|l| format!("{l:?}")));
        p.label = Some(v.clone());
    }
    if let Some(defs) = &o.defaults {
        for (k, v) in &defs.entries {
            let old = take_ci(&mut p.defaults.entries, k);
            let key = format!("defaults.{k}");
            if let Some((old_key, _)) = &old {
                if let Some(s) = fields.remove(&format!("defaults.{old_key}")) {
                    fields.insert(key.clone(), s);
                }
            }
            mark_config(&mut fields, &key, old.map(|(_, dv)| format!("{:?}", dv.to_config_string())));
            p.defaults.entries.insert(k.clone(), v.clone());
        }
    }
    if let Some(ws) = &o.widgets {
        for (k, v) in ws {
            let old = take_ci(&mut p.widgets, k);
            let key = format!("widget.{k}");
            if let Some((old_key, _)) = &old {
                if let Some(s) = fields.remove(&format!("widget.{old_key}")) {
                    fields.insert(key.clone(), s);
                }
            }
            mark_config(&mut fields, &key, old.map(|(_, w)| format!("kind {:?}", widget_kind(&w))));
            p.widgets.insert(k.clone(), v.clone());
        }
    }
    if let Some(c) = &o.companion {
        mark_config(&mut fields, "companion", p.companion.as_ref().map(|_| "a companion".to_string()));
        p.companion = Some(c.clone());
    }
    let base = p.search_base.clone();
    for dv in p.defaults.entries.values_mut() {
        if let DefaultValue::DetectedRange(spec) = dv {
            spec.container = base.clone();
        }
    }
    let mut prov = Provenance {
        name: p.name.clone(),
        origin: Origin::Merged {
            detected: d.name.clone(),
            container: d.container.clone(),
            sampled: d.sampled,
            partial: d.partial,
        },
        fields,
        suppressed: Vec::new(),
        pending_suppress: Vec::new(),
        notes: d.notes.clone(),
    };
    for path in &o.suppress {
        if suppress(&mut p, &mut prov, path).is_err() {
            prov.pending_suppress.push(path.clone());
        }
    }
    (p, prov)
}

fn nothing(who: &str, path: &str) -> String {
    format!("profile \"{who}\": suppress \"{path}\" matches nothing detected")
}

/// Remove one detected part. Config parts are never removed.
fn suppress(p: &mut EntryProfile, prov: &mut Provenance, path: &str) -> Result<(), String> {
    let who = p.name.clone();
    let key = match path {
        "companion" | "label" | "show" | "search_attrs" => path.to_string(),
        _ => match path.split_once('.') {
            Some(("defaults", attr)) => match p.defaults.entries.keys().find(|k| k.eq_ignore_ascii_case(attr)) {
                Some(k) => format!("defaults.{k}"),
                None => return Err(nothing(&who, path)),
            },
            Some(("widget", attr)) => match p.widgets.keys().find(|k| k.eq_ignore_ascii_case(attr)) {
                Some(k) => format!("widget.{k}"),
                None => return Err(nothing(&who, path)),
            },
            _ => {
                return Err(format!(
                    "profile \"{who}\": unknown suppress path \"{path}\" (use companion, label, show, search_attrs, defaults.<attr> or widget.<attr>)"
                ))
            }
        },
    };
    if !matches!(prov.fields.get(&key), Some(Source::Detected(_))) {
        return Err(nothing(&who, path));
    }
    match key.split_once('.') {
        Some(("defaults", attr)) => {
            p.defaults.entries.remove(attr);
        }
        Some(("widget", attr)) => {
            p.widgets.remove(attr);
        }
        _ => match key.as_str() {
            "companion" => p.companion = None,
            "label" => p.label = None,
            "show" => p.show.clear(),
            "search_attrs" => p.search_attrs.clear(),
            _ => {}
        },
    }
    prov.fields.remove(&key);
    prov.suppressed.push(path.to_string());
    Ok(())
}

/// Merge (spec §3 "Matching", "Merge rules", §2A "Order"). Suppress paths that
/// matched nothing become warnings.
pub fn merge(schema: &SchemaModel, detected: &[DetectedProfile], overrides: &[ProfileOverride]) -> Merged {
    let mut m = merge_core(schema, detected, overrides);
    flush_pending(&mut m);
    m
}

/// Retry every pending suppress path; the ones that still match nothing (or name
/// an unknown path) become warnings.
pub(crate) fn flush_pending(m: &mut Merged) {
    for (p, prov) in m.profiles.iter_mut().zip(m.provenance.iter_mut()) {
        for path in std::mem::take(&mut prov.pending_suppress) {
            if let Err(w) = suppress(p, prov, &path) {
                m.warnings.push(w);
            }
        }
    }
}

/// The merge without the final suppress flush (rule D runs in between).
pub(crate) fn merge_core(schema: &SchemaModel, detected: &[DetectedProfile], overrides: &[ProfileOverride]) -> Merged {
    let mut out = Merged::default();
    let mut taken: Vec<Option<usize>> = vec![None; overrides.len()];
    let mut owner: Vec<Option<usize>> = vec![None; detected.len()];
    // Pass 1: names.
    for (oi, o) in overrides.iter().enumerate() {
        if let Some(di) = detected.iter().position(|d| d.name.eq_ignore_ascii_case(&o.name)) {
            match owner[di] {
                None => {
                    owner[di] = Some(oi);
                    taken[oi] = Some(di);
                }
                Some(prev) => out.warnings.push(format!(
                    "profile \"{}\" also matches detected \"{}\", already merged into \"{}\"",
                    o.name, detected[di].name, overrides[prev].name
                )),
            }
        }
    }
    // Pass 2: search_base + structural class, among the free detected profiles.
    for (oi, o) in overrides.iter().enumerate() {
        if taken[oi].is_some() {
            continue;
        }
        let (Some(base), Some(ocs)) = (&o.search_base, &o.object_classes) else { continue };
        let Some(structural) = schema.structural_class(ocs) else { continue };
        let cands: Vec<usize> = detected
            .iter()
            .enumerate()
            .filter(|(_, d)| dn_eq(&d.container, base) && d.structural.eq_ignore_ascii_case(&structural))
            .map(|(i, _)| i)
            .collect();
        match cands.iter().find(|di| owner[**di].is_none()) {
            Some(&di) => {
                owner[di] = Some(oi);
                taken[oi] = Some(di);
            }
            None => {
                if let Some(&di) = cands.first() {
                    let prev = owner[di].map(|p| overrides[p].name.clone()).unwrap_or_default();
                    out.warnings.push(format!(
                        "profile \"{}\" also matches detected \"{}\", already merged into \"{prev}\"",
                        o.name, detected[di].name
                    ));
                }
            }
        }
    }
    let rename: HashMap<String, String> = overrides
        .iter()
        .zip(&taken)
        .filter_map(|(o, t)| t.map(|di| (detected[di].name.to_lowercase(), o.name.clone())))
        .collect();
    // Config profiles, file order.
    for (oi, o) in overrides.iter().enumerate() {
        if !o.is_enabled() {
            out.disabled.push(o.name.clone());
            continue;
        }
        match taken[oi] {
            Some(di) => {
                let (p, prov) = merge_one(&detected[di], o, &rename);
                out.profiles.push(p);
                out.provenance.push(prov);
            }
            None => match o.to_entry_profile() {
                Some(p) => {
                    let mut prov = Provenance {
                        name: p.name.clone(),
                        origin: Origin::Config,
                        fields: config_fields(&p),
                        suppressed: Vec::new(),
                        pending_suppress: Vec::new(),
                        notes: Vec::new(),
                    };
                    let mut p = p;
                    for path in &o.suppress {
                        if suppress(&mut p, &mut prov, path).is_err() {
                            prov.pending_suppress.push(path.clone());
                        }
                    }
                    out.profiles.push(p);
                    out.provenance.push(prov);
                }
                None => out.warnings.push(format!("profile \"{}\" matches no detected profile", o.name)),
            },
        }
    }
    // Detected-only: more object classes first, then name.
    let mut rest: Vec<usize> = (0..detected.len()).filter(|di| owner[*di].is_none()).collect();
    rest.sort_by(|a, b| {
        let (da, db) = (&detected[*a], &detected[*b]);
        db.object_classes.value.len().cmp(&da.object_classes.value.len()).then_with(|| da.name.cmp(&db.name))
    });
    for di in rest {
        let d = &detected[di];
        let mut p = d.to_entry_profile();
        rewrite_candidates(&mut p.widgets, &rename);
        out.provenance.push(Provenance {
            name: p.name.clone(),
            origin: Origin::Detected { container: d.container.clone(), sampled: d.sampled, partial: d.partial },
            fields: detected_fields(d),
            suppressed: Vec::new(),
            pending_suppress: Vec::new(),
            notes: d.notes.clone(),
        });
        out.profiles.push(p);
    }
    out
}

/// Per-origin validation (spec §3 "Validation"): a failing detected part is
/// dropped and noted; a failing config part is an error.
pub fn validate(m: &mut Merged) -> Result<(), String> {
    let names: Vec<String> = m.profiles.iter().map(|p| p.name.to_lowercase()).collect();
    for (p, prov) in m.profiles.iter_mut().zip(m.provenance.iter_mut()) {
        let mut drop: Vec<(String, String)> = Vec::new();
        for (attr, spec) in &p.widgets {
            let Some(name) = candidate_name(spec) else { continue };
            if is_sentinel(name) || names.contains(&name.to_lowercase()) {
                continue;
            }
            if matches!(prov.fields.get(&format!("widget.{attr}")), Some(Source::Detected(_))) {
                drop.push((
                    attr.clone(),
                    format!("profile \"{}\": dropped detected widget.{attr}: unknown candidate profile \"{name}\"", p.name),
                ));
            } else {
                return Err(format!("profile \"{}\" [profile.widget.{attr}]: unknown candidate profile \"{name}\"", p.name));
            }
        }
        for (attr, note) in drop {
            p.widgets.remove(&attr);
            prov.fields.remove(&format!("widget.{attr}"));
            prov.notes.push(note.clone());
            m.warnings.push(note.clone());
            m.dropped.push(note);
        }
        if let Some(c) = &p.companion {
            let who = format!("profile '{}' companion", p.name);
            if let Err(e) = crate::config::check_companion(&who, c) {
                if matches!(prov.fields.get("companion"), Some(Source::Detected(_))) {
                    let note = format!("dropped detected companion: {e}");
                    p.companion = None;
                    prov.fields.remove("companion");
                    prov.notes.push(note.clone());
                    m.warnings.push(note.clone());
                    m.dropped.push(note);
                } else {
                    return Err(e);
                }
            }
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -j4 --lib detect:: config::`
Expected: PASS. In `match_by_search_base_and_structural_class` the demo config's `user` block carries `object_classes = [inetOrgPerson, …]` + `search_base = ou=people…` → matches `user-people`; `group` (groupOfNames) → `group-groups`; `posixgroup` → `posixgroup-groups`.

- [ ] **Step 5: Gate and commit**

Run: `CARGO_BUILD_JOBS=4 make check` → `All checks passed!`

```bash
git add src/detect/ src/config/widget.rs src/config/resolver.rs
git commit -m "feat(detect): merge config profiles over detected ones, with suppress and provenance

Profile names now compare case-insensitively in widget candidates.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: Rule D — useradd-style assumptions after the merge

**Files:**
- Create: `src/detect/assume.rs`; Modify: `src/detect/mod.rs` (`pub mod assume;`)
- Modify: `src/detect/merge.rs` (`Source::Assumed`, `suppress` and `validate` accept assumed parts)

**Interfaces:**
- Consumes: Task 8 (`merge_core`, `flush_pending`, `Merged`, `Provenance`, `Origin`, `Source`), Task 6 (`DetectedProfile::users_without_private_group`), Task 4 (`RangeSpec`), `SchemaModel::structural_class`.
- Produces:
  - `merge::Source::Assumed(String)` — the reason, printed by the dump as `# assumed: <reason>`
  - `assume::merge_with_assumptions(schema: &SchemaModel, detected: &[DetectedProfile], overrides: &[ProfileOverride], group_ou: Option<&str>) -> Merged` (used by `load::assemble`, Task 11)
  - `assume::apply(schema: &SchemaModel, detected: &[DetectedProfile], group_ou: Option<&str>, m: &mut Merged)`
  - `assume::{NO_GROUP_CONTAINER, REASON_NO_USERS, REASON_FEW_USERS}: &str`

Rule (spec §2D), over every final profile whose `object_classes` include `posixAccount`:
- **Contrary evidence** = the profile came from a detected group with ≥ 3 sampled users (B2/B3 decided), or with `users_without_private_group != Some(0)` (a user lacks a private group, or the lookup failed so it is unknown). A config-only profile (no detected group) has none.
- **Private groups:** without contrary evidence, and only when the profile has **neither** a `gidNumber` default **nor** a companion: add `gidNumber = "{uidNumber}"` and the companion `{ posixGroup, rdn cn, cn = "{uid}", gidNumber = "{uidNumber}", memberUid = "{uid}" }`. Companion base: the `search_base` of the posix-group profile among the merged profiles (most sampled entries, ties by name), else `group_ou`, else skip with the note `no group container for private groups`.
- **Ranges:** a user profile without a `uidNumber` default gets `DetectedRange { uidNumber, unified = gidNumber default is "{uidNumber}" }`; an existing detected user range becomes unified when private groups were assumed. A posix-group profile without a `gidNumber` default gets `DetectedRange { gidNumber, exclude_private: true, unified = any user profile has gidNumber = "{uidNumber}" }`.
- Assumed parts get `Source::Assumed(reason)`; pending `suppress` paths are retried afterwards (`flush_pending`), so `suppress = ["companion"]` removes an assumed companion without a warning.

- [ ] **Step 1: Write the failing tests** (bottom of `src/detect/assume.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::fixtures::{container, e, schema};
    use crate::detect::infer::detect;
    use crate::detect::model::Sample;

    fn overrides(toml: &str) -> Vec<ProfileOverride> {
        #[derive(serde::Deserialize)]
        struct W {
            #[serde(default)]
            profile: Vec<ProfileOverride>,
        }
        toml::from_str::<W>(toml).unwrap().profile
    }
    const CFG_USER: &str = "[[profile]]\nname = \"user\"\nobject_classes = [\"inetOrgPerson\", \"posixAccount\"]\nrdn_attr = \"uid\"\nsearch_base = \"ou=people,dc=x\"\n";
    fn get<'a>(m: &'a Merged, name: &str) -> (&'a EntryProfile, &'a Provenance) {
        let i = m.profiles.iter().position(|p| p.name == name).unwrap_or_else(|| panic!("no {name}"));
        (&m.profiles[i], &m.provenance[i])
    }
    fn dflt(p: &EntryProfile, attr: &str) -> Option<String> {
        p.defaults.entries.get(attr).map(|d| d.to_config_string())
    }

    #[test]
    fn empty_directory_config_user_gets_private_groups_and_a_range() {
        let m = merge_with_assumptions(&schema(), &[], &overrides(CFG_USER), Some("ou=groups,dc=x"));
        let (p, prov) = get(&m, "user");
        assert_eq!(dflt(p, "gidNumber").as_deref(), Some("{uidNumber}"));
        let c = p.companion.as_ref().unwrap();
        assert_eq!(c.search_base, "ou=groups,dc=x");
        assert_eq!(c.attributes["memberUid"], "{uid}");
        match &p.defaults.entries["uidNumber"] {
            DefaultValue::DetectedRange(s) => assert!(s.unified && s.container == "ou=people,dc=x"),
            other => panic!("{other:?}"),
        }
        assert_eq!(prov.fields["companion"], Source::Assumed(REASON_NO_USERS.into()));
        assert!(matches!(prov.fields["defaults.uidNumber"], Source::Assumed(_)));
        assert!(m.warnings.is_empty(), "{:?}", m.warnings);
    }

    #[test]
    fn companion_base_prefers_the_posix_group_profile() {
        let o = format!("{CFG_USER}[[profile]]\nname = \"grp\"\nobject_classes = [\"posixGroup\"]\nsearch_base = \"ou=unix,dc=x\"\n");
        let m = merge_with_assumptions(&schema(), &[], &overrides(&o), Some("ou=groups,dc=x"));
        assert_eq!(get(&m, "user").0.companion.as_ref().unwrap().search_base, "ou=unix,dc=x");
        match &get(&m, "grp").0.defaults.entries["gidNumber"] {
            DefaultValue::DetectedRange(s) => assert!(s.unified && s.exclude_private),
            other => panic!("{other:?}"),
        }
    }

    fn two_users(private: bool) -> Sample {
        let users = (1..=2u64)
            .map(|i| {
                let uid = format!("u{i}");
                let num = (5000 + i).to_string();
                let gid = if private { num.clone() } else { "100".to_string() };
                e(&format!("uid={uid},ou=people,dc=x"), &[
                    ("objectClass", &["inetOrgPerson", "posixAccount"]), ("uid", &[uid.as_str()]), ("cn", &[uid.as_str()]),
                    ("sn", &["s"]), ("uidNumber", &[num.as_str()]), ("gidNumber", &[gid.as_str()])])
            })
            .collect();
        let groups = if private {
            (1..=2u64)
                .map(|i| {
                    let cn = format!("u{i}");
                    let num = (5000 + i).to_string();
                    e(&format!("cn={cn},ou=groups,dc=x"), &[("objectClass", &["posixGroup"]), ("cn", &[cn.as_str()]), ("gidNumber", &[num.as_str()])])
                })
                .collect()
        } else {
            vec![e("cn=staff,ou=groups,dc=x", &[("objectClass", &["posixGroup"]), ("cn", &["staff"]), ("gidNumber", &["100"])])]
        };
        Sample {
            containers: vec![container("ou=people,dc=x", users), container("ou=groups,dc=x", groups)],
            group_ou: Some("ou=groups,dc=x".into()),
            ..Default::default()
        }
    }

    #[test]
    fn two_users_with_private_groups_are_assumed() {
        let d = detect(&schema(), &two_users(true)).profiles;
        let m = merge_with_assumptions(&schema(), &d, &[], Some("ou=groups,dc=x"));
        let (p, prov) = get(&m, "user-people");
        assert_eq!(dflt(p, "gidNumber").as_deref(), Some("{uidNumber}"));
        assert_eq!(p.companion.as_ref().unwrap().search_base, "ou=groups,dc=x");
        assert_eq!(prov.fields["companion"], Source::Assumed(REASON_FEW_USERS.into()));
        match &p.defaults.entries["uidNumber"] {
            DefaultValue::DetectedRange(s) => assert!(s.unified, "assumed private groups unify the space"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn two_users_sharing_gid_100_block_the_assumption() {
        let d = detect(&schema(), &two_users(false)).profiles;
        let m = merge_with_assumptions(&schema(), &d, &[], Some("ou=groups,dc=x"));
        let (p, _) = get(&m, "user-people");
        assert!(p.companion.is_none());
        assert!(!p.defaults.entries.contains_key("gidNumber"), "B3 needs 3 users");
    }

    #[test]
    fn no_group_container_is_noted() {
        let m = merge_with_assumptions(&schema(), &[], &overrides(CFG_USER), None);
        let (p, prov) = get(&m, "user");
        assert!(p.companion.is_none() && !p.defaults.entries.contains_key("gidNumber"));
        assert!(prov.notes.iter().any(|n| n == NO_GROUP_CONTAINER));
        match &p.defaults.entries["uidNumber"] {
            DefaultValue::DetectedRange(s) => assert!(!s.unified),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_assumed_companion_can_be_suppressed() {
        let o = format!("{CFG_USER}suppress = [\"companion\"]\n");
        let m = merge_with_assumptions(&schema(), &[], &overrides(&o), Some("ou=groups,dc=x"));
        let (p, prov) = get(&m, "user");
        assert!(p.companion.is_none());
        assert_eq!(prov.suppressed, vec!["companion"]);
        assert!(m.warnings.is_empty(), "{:?}", m.warnings);
    }

    #[test]
    fn config_values_are_never_overridden() {
        let o = format!("{CFG_USER}[profile.defaults]\ngidNumber = \"100\"\nuidNumber = \"{{next:2000-2999}}\"\n");
        let m = merge_with_assumptions(&schema(), &[], &overrides(&o), Some("ou=groups,dc=x"));
        let (p, _) = get(&m, "user");
        assert_eq!(dflt(p, "gidNumber").as_deref(), Some("100"));
        assert_eq!(dflt(p, "uidNumber").as_deref(), Some("{next:2000-2999}"));
        assert!(p.companion.is_none());
    }
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -j4 --lib detect::assume` → FAIL to compile (`merge_with_assumptions` / `Source::Assumed` missing).

- [ ] **Step 3: Implement**

In `src/detect/merge.rs`: add the variant

```rust
    /// Filled by rule D (§2D); the string says why.
    Assumed(String),
```

to `Source`; in `suppress` change the check to

```rust
    if !matches!(prov.fields.get(&key), Some(Source::Detected(_) | Source::Assumed(_))) {
```

and in `validate` both origin checks to `Some(Source::Detected(_) | Source::Assumed(_))`.

Prepend to `src/detect/assume.rs`:

```rust
//! Rule D (spec §2D): when there is too little data, assume what Ubuntu's
//! useradd does (numbers from a fixed start, user-private groups), with LDAP
//! numbers starting at 10000. Runs after the merge; fills only what is missing.

use crate::config::defaults::{parse_default_value, DefaultValue};
use crate::config::{CompanionSpec, EntryProfile, ProfileOverride};
use crate::detect::merge::{flush_pending, merge_core, Merged, Origin, Provenance, Source};
use crate::detect::model::DetectedProfile;
use crate::detect::range::RangeSpec;
use crate::detect::MIN_SAMPLE;
use crate::schema::SchemaModel;

pub const NO_GROUP_CONTAINER: &str = "no group container for private groups";
pub const REASON_NO_USERS: &str = "no users yet; useradd-style private group";
pub const REASON_FEW_USERS: &str = "fewer than 3 users, all with a private group; useradd-style private group";

/// Merge, apply rule D, then resolve `suppress` paths (which may name assumed parts).
pub fn merge_with_assumptions(
    schema: &SchemaModel,
    detected: &[DetectedProfile],
    overrides: &[ProfileOverride],
    group_ou: Option<&str>,
) -> Merged {
    let mut m = merge_core(schema, detected, overrides);
    apply(schema, detected, group_ou, &mut m);
    flush_pending(&mut m);
    m
}

fn has_class(p: &EntryProfile, oc: &str) -> bool {
    p.object_classes.iter().any(|c| c.eq_ignore_ascii_case(oc))
}

fn has_default(p: &EntryProfile, attr: &str) -> bool {
    p.defaults.entries.keys().any(|k| k.eq_ignore_ascii_case(attr))
}

fn gid_follows_uid(p: &EntryProfile) -> bool {
    p.defaults
        .entries
        .iter()
        .any(|(k, v)| k.eq_ignore_ascii_case("gidNumber") && v.to_config_string().eq_ignore_ascii_case("{uidNumber}"))
}

fn sampled_of(prov: &Provenance) -> usize {
    match &prov.origin {
        Origin::Detected { sampled, .. } | Origin::Merged { sampled, .. } => *sampled,
        Origin::Config => 0,
    }
}

fn detected_of<'a>(prov: &Provenance, detected: &'a [DetectedProfile]) -> Option<&'a DetectedProfile> {
    let name = match &prov.origin {
        Origin::Detected { .. } => prov.name.as_str(),
        Origin::Merged { detected, .. } => detected.as_str(),
        Origin::Config => return None,
    };
    detected.iter().find(|d| d.name.eq_ignore_ascii_case(name))
}

fn structural(schema: &SchemaModel, p: &EntryProfile) -> String {
    schema
        .structural_class(&p.object_classes)
        .or_else(|| p.object_classes.first().cloned())
        .unwrap_or_default()
}

fn range_reason(attr: &str) -> String {
    format!("no {attr} range configured or detected; useradd-style numbering")
}

pub fn apply(schema: &SchemaModel, detected: &[DetectedProfile], group_ou: Option<&str>, m: &mut Merged) {
    // §2B5 "posix-group profile", over the merged profiles.
    let group_base: Option<String> = m
        .profiles
        .iter()
        .zip(&m.provenance)
        .filter(|(p, _)| has_class(p, "posixGroup") && !p.search_base.is_empty())
        .max_by(|(a, pa), (b, pb)| sampled_of(pa).cmp(&sampled_of(pb)).then_with(|| b.name.cmp(&a.name)))
        .map(|(p, _)| p.search_base.clone())
        .or_else(|| group_ou.map(str::to_string));
    // Users first: group ranges need to know whether any user space is unified.
    for i in 0..m.profiles.len() {
        if !has_class(&m.profiles[i], "posixAccount") {
            continue;
        }
        let d = detected_of(&m.provenance[i], detected);
        let contrary = d.is_some_and(|d| d.entries.len() >= MIN_SAMPLE || d.users_without_private_group != Some(0));
        let reason = if d.is_none() { REASON_NO_USERS } else { REASON_FEW_USERS };
        let (p, prov) = (&mut m.profiles[i], &mut m.provenance[i]);
        let mut assumed_private = false;
        if !contrary && !has_default(p, "gidNumber") && p.companion.is_none() {
            match &group_base {
                Some(base) => {
                    p.defaults.entries.insert("gidNumber".into(), parse_default_value("{uidNumber}").expect("template"));
                    p.companion = Some(CompanionSpec {
                        object_classes: vec!["posixGroup".into()],
                        rdn_attr: "cn".into(),
                        search_base: base.clone(),
                        attributes: [("cn", "{uid}"), ("gidNumber", "{uidNumber}"), ("memberUid", "{uid}")]
                            .into_iter()
                            .map(|(k, v)| (k.to_string(), v.to_string()))
                            .collect(),
                    });
                    prov.fields.insert("defaults.gidNumber".into(), Source::Assumed(reason.into()));
                    prov.fields.insert("companion".into(), Source::Assumed(reason.into()));
                    assumed_private = true;
                }
                None => prov.notes.push(NO_GROUP_CONTAINER.to_string()),
            }
        }
        let unified = gid_follows_uid(p);
        if !has_default(p, "uidNumber") {
            let spec = RangeSpec {
                attr: "uidNumber".into(),
                container: p.search_base.clone(),
                structural: structural(schema, p),
                unified,
                exclude_private: false,
            };
            p.defaults.entries.insert("uidNumber".into(), DefaultValue::DetectedRange(spec));
            prov.fields.insert("defaults.uidNumber".into(), Source::Assumed(range_reason("uidNumber")));
        } else if assumed_private {
            for (k, v) in p.defaults.entries.iter_mut() {
                if let (true, DefaultValue::DetectedRange(s)) = (k.eq_ignore_ascii_case("uidNumber"), v) {
                    s.unified = true;
                }
            }
        }
    }
    let unified_any = m.profiles.iter().any(|p| has_class(p, "posixAccount") && gid_follows_uid(p));
    for (p, prov) in m.profiles.iter_mut().zip(m.provenance.iter_mut()) {
        if !has_class(p, "posixGroup") {
            continue;
        }
        if has_default(p, "gidNumber") {
            // A detected group range joins a space that assumed private groups unified.
            for (k, v) in p.defaults.entries.iter_mut() {
                if let (true, DefaultValue::DetectedRange(s)) = (k.eq_ignore_ascii_case("gidNumber"), v) {
                    s.unified |= unified_any;
                }
            }
            continue;
        }
        let spec = RangeSpec {
            attr: "gidNumber".into(),
            container: p.search_base.clone(),
            structural: structural(schema, p),
            unified: unified_any,
            exclude_private: true,
        };
        p.defaults.entries.insert("gidNumber".into(), DefaultValue::DetectedRange(spec));
        prov.fields.insert("defaults.gidNumber".into(), Source::Assumed(range_reason("gidNumber")));
    }
}
```

Note: `structural(schema, p)` borrows `p` immutably while `p` is `&mut` — compute it into a local before building `RangeSpec` if the borrow checker complains (`let st = structural(schema, p);`).

- [ ] **Step 4: Run tests** — `cargo test -j4 --lib detect::` → PASS (7 new tests; Task 8's tests unchanged because `merge` does not assume).

- [ ] **Step 5: Gate and commit** — `CARGO_BUILD_JOBS=4 make check` → `All checks passed!`

```bash
git add src/detect/
git commit -m "feat(detect): useradd-style assumptions when there is too little data (rule D)

Every posixAccount profile without contrary evidence gets private groups and
a number range starting at 10000; assumed parts are marked and suppressible.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: Sampling — worker request and sampler

**Files:**
- Modify: `src/ldap/worker.rs` (new `SampleParams`, `Request::SampleSearch`, worker-loop arm, `run_sample_search`)
- Create: `src/detect/sample.rs`; Modify: `src/detect/mod.rs` (`pub mod sample;`)

**Interfaces:**
- Produces:
  - `worker::SampleParams { base: String, scope: SearchScope, filter: String, attrs: Vec<String>, size_limit: Option<i32>, types_only: bool, time_limit: Duration }` (`Debug, Clone, PartialEq, Eq`)
  - `Request::SampleSearch { id: u64, params: SampleParams }` → `Response::Entries { truncated }` (truncated = size/time/admin limit **or** client timeout) / `Response::SearchError`
  - `worker::time_limit_secs(d: Duration) -> i32` (whole seconds, at least 1)
  - `sample::Searcher` trait: `fn search(&mut self, q: &SampleParams) -> Result<(Vec<SampleEntry>, bool), String>`
  - `sample::WorkerSearcher<'a>(pub &'a WorkerHandle)`
  - `sample::Budget { new(total: Duration) -> Self, remaining(&self) -> Option<Duration> }`
  - `sample::sample(s: &mut dyn Searcher, base_dn: &str, budget: &Budget) -> Result<Sample, String>` — `Err` only when no container list could be obtained at all
  - `sample::{HAS_SUBORDINATES_FILTER, FALLBACK_FILTER}`
  - `Sample.group_ou` is filled: `Some("ou=groups,<base_dn>")` when that entry exists (seen in a sample, or confirmed by one base-scope search) — rule D's fallback companion base

- [ ] **Step 1: Write the failing tests**

`src/ldap/worker.rs` tests:

```rust
    #[test]
    fn time_limit_is_whole_seconds_at_least_one() {
        assert_eq!(time_limit_secs(std::time::Duration::from_millis(300)), 1);
        assert_eq!(time_limit_secs(std::time::Duration::from_millis(2900)), 2);
        assert_eq!(time_limit_secs(std::time::Duration::from_secs(10)), 10);
    }
```

`src/detect/sample.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::fixtures::e;

    type Reply = Result<(Vec<SampleEntry>, bool), String>;
    struct Fake {
        calls: Vec<SampleParams>,
        reply: Box<dyn FnMut(&SampleParams) -> Reply>,
    }
    impl Searcher for Fake {
        fn search(&mut self, q: &SampleParams) -> Reply {
            self.calls.push(q.clone());
            (self.reply)(q)
        }
    }
    fn fake(reply: impl FnMut(&SampleParams) -> Reply + 'static) -> Fake {
        Fake { calls: Vec::new(), reply: Box::new(reply) }
    }
    fn budget() -> Budget {
        Budget::new(std::time::Duration::from_secs(30))
    }
    fn user(i: usize) -> SampleEntry {
        e(&format!("uid=u{i},ou=p,dc=x"), &[("objectClass", &["posixAccount"]), ("uid", &[&format!("u{i}")])])
    }

    #[test]
    fn samples_each_container_with_values_and_types_only() {
        let mut f = fake(|q| match q.filter.as_str() {
            HAS_SUBORDINATES_FILTER => Ok((vec![e("ou=p,dc=x", &[])], false)),
            "(objectClass=*)" if q.types_only => Ok((vec![e("uid=u1,ou=p,dc=x", &[("jpegPhoto", &[]), ("uid", &[])])], false)),
            "(objectClass=*)" => Ok((vec![user(1)], true)),
            _ => Ok((vec![], false)),
        });
        let s = sample(&mut f, "dc=x", &budget()).unwrap();
        assert_eq!(s.containers.len(), 1);
        let c = &s.containers[0];
        assert!(c.partial, "size-limited sample is partial");
        assert!(c.present["uid=u1,ou=p,dc=x"].contains("jpegphoto"));
        let values = f.calls.iter().find(|q| q.base == "ou=p,dc=x" && !q.types_only).unwrap();
        assert_eq!(values.scope, SearchScope::OneLevel);
        assert_eq!(values.size_limit, Some(SAMPLE_SIZE));
        assert!(!values.attrs.iter().any(|a| a.eq_ignore_ascii_case("userPassword")));
        let types = f.calls.iter().find(|q| q.types_only).unwrap();
        assert_eq!(types.attrs, vec!["*"]);
    }

    #[test]
    fn rejected_container_filter_falls_back_with_a_note() {
        let mut f = fake(|q| match q.filter.as_str() {
            HAS_SUBORDINATES_FILTER => Err("unwilling to perform".into()),
            FALLBACK_FILTER => Ok((vec![e("ou=p,dc=x", &[])], false)),
            _ => Ok((vec![], false)),
        });
        let s = sample(&mut f, "dc=x", &budget()).unwrap();
        assert_eq!(s.containers.len(), 1);
        assert!(s.notes.iter().any(|n| n.contains("fallback")));
    }

    #[test]
    fn nothing_visible_is_an_empty_sample_not_an_error() {
        let mut f = fake(|_| Ok((vec![], false)));
        let s = sample(&mut f, "dc=x", &budget()).unwrap();
        assert!(s.containers.is_empty());
        assert!(s.lookup_error.is_none());
    }

    #[test]
    fn container_cap_is_noted() {
        let mut f = fake(|q| match q.filter.as_str() {
            HAS_SUBORDINATES_FILTER => Ok(((0..105).map(|i| e(&format!("ou=c{i},dc=x"), &[])).collect(), false)),
            _ => Ok((vec![], false)),
        });
        let s = sample(&mut f, "dc=x", &budget()).unwrap();
        assert_eq!(s.containers.len(), MAX_CONTAINERS);
        assert!(s.notes.iter().any(|n| n.contains("5 more containers")));
    }

    #[test]
    fn exhausted_budget_fails_before_the_first_search() {
        let mut f = fake(|_| Ok((vec![], false)));
        assert!(sample(&mut f, "dc=x", &Budget::new(std::time::Duration::ZERO)).is_err());
        assert!(f.calls.is_empty());
    }

    #[test]
    fn an_empty_groups_ou_is_found_by_a_base_read() {
        let mut f = fake(|q| match (q.filter.as_str(), q.scope) {
            (HAS_SUBORDINATES_FILTER, _) => Ok((vec![], false)),
            (_, SearchScope::Base) if q.base == "ou=groups,dc=x" => Ok((vec![e("ou=groups,dc=x", &[])], false)),
            _ => Ok((vec![], false)),
        });
        assert_eq!(sample(&mut f, "dc=x", &budget()).unwrap().group_ou.as_deref(), Some("ou=groups,dc=x"));
        let mut none = fake(|_| Ok((vec![], false)));
        assert_eq!(sample(&mut none, "dc=x", &budget()).unwrap().group_ou, None);
    }

    #[test]
    fn lookups_are_batched_and_escaped() {
        let mut f = fake(|q| match q.filter.as_str() {
            HAS_SUBORDINATES_FILTER => Ok((vec![e("ou=p,dc=x", &[])], false)),
            "(objectClass=*)" if !q.types_only => Ok(((0..120).map(user).chain([e("uid=x,ou=p,dc=x", &[("objectClass", &["posixAccount"]), ("uid", &["a*b"])])]).collect(), false)),
            _ => Ok((vec![], false)),
        });
        sample(&mut f, "dc=x", &budget()).unwrap();
        let lookups: Vec<&SampleParams> = f.calls.iter().filter(|q| q.filter.starts_with("(&(objectClass=posixGroup)")).collect();
        assert_eq!(lookups.len(), 3, "121 uids → 3 batches of ≤ 50");
        assert!(lookups.iter().any(|q| q.filter.contains(r"(cn=a\2ab)")));
        assert!(lookups.iter().all(|q| q.base == "dc=x" && q.scope == SearchScope::Subtree));
    }

    #[test]
    fn a_failed_lookup_is_recorded() {
        let mut f = fake(|q| match q.filter.as_str() {
            HAS_SUBORDINATES_FILTER => Ok((vec![e("ou=p,dc=x", &[])], false)),
            "(objectClass=*)" if !q.types_only => Ok(((0..3).map(user).collect(), false)),
            f if f.starts_with("(&") => Err("insufficient access".into()),
            _ => Ok((vec![], false)),
        });
        let s = sample(&mut f, "dc=x", &budget()).unwrap();
        assert!(s.lookup_error.as_deref().unwrap().contains("insufficient access"));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -j4 --lib ldap::worker::tests::time_limit detect::sample`
Expected: FAIL to compile.

- [ ] **Step 3: Implement the worker side** (`src/ldap/worker.rs`)

```rust
/// A detection sample search: like `Search`, plus types-only and a time limit
/// (server `timelimit` in whole seconds and a client timeout, both `time_limit`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampleParams {
    pub base: String,
    pub scope: SearchScope,
    pub filter: String,
    pub attrs: Vec<String>,
    pub size_limit: Option<i32>,
    pub types_only: bool,
    pub time_limit: std::time::Duration,
}

/// Whole seconds for the server time limit, at least 1.
pub fn time_limit_secs(d: std::time::Duration) -> i32 {
    d.as_secs().clamp(1, i32::MAX as u64) as i32
}
```

Add to `Request`: `SampleSearch { id: u64, params: SampleParams },` and to `worker_loop`:

```rust
            Request::SampleSearch { id, params } => {
                let resp = match run_sample_search(conn, &params) {
                    Ok((entries, truncated)) => Response::Entries { id, entries, truncated },
                    Err(e) => Response::SearchError { id, msg: format!("{e:#}") },
                };
                let _ = reply.send(resp);
            }
```

```rust
/// Streaming search that keeps what arrived before a client timeout: a limit
/// (rc 3/4/11) or a timeout returns the entries so far with `truncated = true`.
fn run_sample_search(conn: &mut LdapConn, p: &SampleParams) -> Result<(Vec<LdapEntry>, bool)> {
    let mut opts = SearchOptions::new()
        .typesonly(p.types_only)
        .timelimit(time_limit_secs(p.time_limit));
    if let Some(n) = p.size_limit {
        opts = opts.sizelimit(n);
    }
    conn.with_search_options(opts)
        .with_timeout(p.time_limit.max(std::time::Duration::from_secs(1)));
    let adapters: Vec<Box<dyn Adapter<_, _>>> = vec![Box::new(EntriesOnly::new())];
    let mut stream = conn
        .streaming_search_with(adapters, &p.base, scope_to_ldap3(p.scope), &p.filter, p.attrs.clone())
        .with_context(|| format!("searching {}", p.base))?;
    let mut out = Vec::new();
    loop {
        match stream.next() {
            Ok(Some(re)) => out.push(to_ldap_entry(SearchEntry::construct(re))),
            Ok(None) => break,
            Err(ldap3::LdapError::Timeout { .. }) => return Ok((out, true)),
            Err(e) => return Err(anyhow!(e)).with_context(|| format!("searching {}", p.base)),
        }
    }
    let res = stream.result();
    if res.rc != 0 && !is_limit_rc(res.rc) {
        return Err(anyhow!(result_code_message(res.rc, &res.text))).with_context(|| format!("searching {}", p.base));
    }
    Ok((out, is_limit_rc(res.rc)))
}
```

(`lib.rs::describe_response` needs no change — `Response` is unchanged.)

- [ ] **Step 4: Implement the sampler** — prepend to `src/detect/sample.rs`:

```rust
//! Sampling (spec §1.1): containers, one-level samples with values and
//! types-only presence, cross-container private-group lookups. Talks to LDAP
//! through `Searcher`, so the logic is unit-tested with a fake.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use crate::detect::model::{ContainerSample, Sample, SampleEntry};
use crate::detect::{LOOKUP_BATCH, MAX_CONTAINERS, SAMPLE_ATTRS, SAMPLE_SIZE};
use crate::ldap::worker::{Request, Response, SampleParams, SearchScope, WorkerHandle};
use crate::workflows::pick_state::escape_filter;

pub const HAS_SUBORDINATES_FILTER: &str = "(hasSubordinates=TRUE)";
pub const FALLBACK_FILTER: &str =
    "(|(objectClass=organizationalUnit)(objectClass=organization)(objectClass=domain)(objectClass=container))";

pub trait Searcher {
    /// Entries plus `partial` (a limit or timeout cut the result short).
    fn search(&mut self, q: &SampleParams) -> Result<(Vec<SampleEntry>, bool), String>;
}

pub struct WorkerSearcher<'a>(pub &'a WorkerHandle);

impl Searcher for WorkerSearcher<'_> {
    fn search(&mut self, q: &SampleParams) -> Result<(Vec<SampleEntry>, bool), String> {
        match self.0.request(Request::SampleSearch { id: 0, params: q.clone() }) {
            Ok(Response::Entries { entries, truncated, .. }) => {
                Ok((entries.iter().map(SampleEntry::from).collect(), truncated))
            }
            Ok(Response::SearchError { msg, .. }) => Err(msg),
            Ok(other) => Err(format!("unexpected worker response {other:?}")),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// The time left of the detection budget.
pub struct Budget {
    deadline: Instant,
}

impl Budget {
    pub fn new(total: Duration) -> Self {
        Budget { deadline: Instant::now() + total }
    }

    /// `None` once the budget is used up.
    pub fn remaining(&self) -> Option<Duration> {
        let r = self.deadline.saturating_duration_since(Instant::now());
        (!r.is_zero()).then_some(r)
    }
}

fn params(base: &str, scope: SearchScope, filter: &str, attrs: Vec<String>, size: Option<i32>, types_only: bool, t: Duration) -> SampleParams {
    SampleParams { base: base.to_string(), scope, filter: filter.to_string(), attrs, size_limit: size, types_only, time_limit: t }
}

pub fn sample(s: &mut dyn Searcher, base_dn: &str, budget: &Budget) -> Result<Sample, String> {
    let mut out = Sample { base_dn: base_dn.to_string(), ..Default::default() };
    let t = budget.remaining().ok_or("the detection budget was used up before the container search")?;
    let one = vec!["1.1".to_string()];
    let (found, partial) = match s.search(&params(base_dn, SearchScope::Subtree, HAS_SUBORDINATES_FILTER, one.clone(), None, false, t)) {
        Ok(r) => r,
        Err(e) => {
            out.notes.push(format!("container search {HAS_SUBORDINATES_FILTER} failed ({e}); used the objectClass fallback"));
            let t = budget.remaining().ok_or("the detection budget was used up before the container search")?;
            s.search(&params(base_dn, SearchScope::Subtree, FALLBACK_FILTER, one, None, false, t))
                .map_err(|e| format!("container search failed: {e}"))?
        }
    };
    if partial {
        out.notes.push("the container search hit a limit; some containers may be missing".to_string());
    }
    let mut dns: Vec<String> = Vec::new();
    for c in found {
        if !dns.iter().any(|d| crate::detect::dn_eq(d, &c.dn)) {
            dns.push(c.dn);
        }
    }
    if dns.len() > MAX_CONTAINERS {
        out.notes.push(format!("sampled the first {MAX_CONTAINERS} containers; {} more containers skipped", dns.len() - MAX_CONTAINERS));
        dns.truncate(MAX_CONTAINERS);
    }
    let attrs: Vec<String> = SAMPLE_ATTRS.iter().map(|a| a.to_string()).collect();
    for (i, dn) in dns.iter().enumerate() {
        let Some(t) = budget.remaining() else {
            out.notes.push(format!("detection budget used up; skipped {} containers (partial)", dns.len() - i));
            break;
        };
        let (entries, mut partial) =
            match s.search(&params(dn, SearchScope::OneLevel, "(objectClass=*)", attrs.clone(), Some(SAMPLE_SIZE), false, t)) {
                Ok(r) => r,
                Err(e) => {
                    out.notes.push(format!("sampling {dn} failed: {e}"));
                    continue;
                }
            };
        let mut present: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        match budget.remaining() {
            Some(t) => match s.search(&params(dn, SearchScope::OneLevel, "(objectClass=*)", vec!["*".to_string()], Some(SAMPLE_SIZE), true, t)) {
                Ok((typed, p)) => {
                    partial |= p;
                    for te in typed {
                        present.insert(te.dn.to_lowercase(), te.attrs.keys().map(|k| k.to_lowercase()).collect());
                    }
                }
                Err(e) => out.notes.push(format!("attribute presence for {dn} failed: {e}")),
            },
            None => partial = true,
        }
        out.containers.push(ContainerSample { dn: dn.clone(), entries, present, partial });
    }
    out.group_ou = find_group_ou(s, &out, base_dn, budget);
    lookups(s, &mut out, budget);
    Ok(out)
}

/// `ou=groups` directly under the base, if it exists. An empty OU has no
/// subordinates, so it is not a sampled container; look for it among the
/// sampled entries first, then with one base-scope read.
fn find_group_ou(s: &mut dyn Searcher, out: &Sample, base_dn: &str, budget: &Budget) -> Option<String> {
    let want = format!("ou=groups,{base_dn}");
    let seen = out.containers.iter().any(|c| crate::detect::dn_eq(&c.dn, &want))
        || out.containers.iter().flat_map(|c| c.entries.iter()).any(|e| crate::detect::dn_eq(&e.dn, &want));
    if seen {
        return Some(want);
    }
    let t = budget.remaining()?;
    match s.search(&params(&want, SearchScope::Base, "(objectClass=*)", vec!["1.1".to_string()], None, false, t)) {
        Ok((found, _)) if !found.is_empty() => Some(want),
        _ => None,
    }
}

/// Forward (`posixGroup` by sampled `uid`) and reverse (`posixAccount` by
/// sampled group `cn`) lookups, batched at `LOOKUP_BATCH`.
fn lookups(s: &mut dyn Searcher, out: &mut Sample, budget: &Budget) {
    let mut uids: Vec<String> = Vec::new();
    let mut cns: Vec<String> = Vec::new();
    for e in out.containers.iter().flat_map(|c| c.entries.iter()) {
        if e.has_class("posixAccount") {
            if let Some(u) = e.first("uid") {
                uids.push(u.to_string());
            }
        }
        if e.has_class("posixGroup") {
            if let Some(c) = e.first("cn") {
                cns.push(c.to_string());
            }
        }
    }
    let jobs = [
        ("posixGroup", "cn", uids, vec!["objectClass", "cn", "gidNumber", "memberUid"]),
        ("posixAccount", "uid", cns, vec!["objectClass", "uid", "uidNumber", "gidNumber"]),
    ];
    for (class, key, values, attrs) in jobs {
        for chunk in values.chunks(LOOKUP_BATCH) {
            let Some(t) = budget.remaining() else {
                out.lookup_error = Some("the detection budget was used up".to_string());
                return;
            };
            let ors: String = chunk.iter().map(|v| format!("({key}={})", escape_filter(v))).collect();
            let filter = format!("(&(objectClass={class})(|{ors}))");
            let attrs = attrs.iter().map(|a| a.to_string()).collect();
            match s.search(&params(&out.base_dn.clone(), SearchScope::Subtree, &filter, attrs, None, false, t)) {
                Ok((found, false)) => {
                    if class == "posixGroup" {
                        out.groups.extend(found);
                    } else {
                        out.accounts.extend(found);
                    }
                }
                Ok((_, true)) => {
                    out.lookup_error = Some(format!("the {class} lookup hit a server limit"));
                    return;
                }
                Err(e) => {
                    out.lookup_error = Some(format!("the {class} lookup failed: {e}"));
                    return;
                }
            }
        }
    }
}
```

- [ ] **Step 5: Run tests** — `cargo test -j4 --lib ldap::worker detect::sample` → PASS (1 + 8).

- [ ] **Step 6: Gate and commit** — `CARGO_BUILD_JOBS=4 make check` → `All checks passed!`

```bash
git add src/ldap/worker.rs src/detect/
git commit -m "feat(detect): sample containers with size, time and types-only limits

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: `load_profiles`, bootstrap reorder, Samba trigger

**Files:**
- Create: `src/detect/load.rs`; Modify: `src/detect/mod.rs` (`pub mod load;`)
- Modify: `src/ui/state.rs:1623-1628` (add `samba_needed`), `:1671-1767` (`bootstrap`)

**Interfaces:**
- Produces:
  - `load::ProfileInputs { base_dn: String, detect_enabled: bool, overrides: Vec<ProfileOverride>, config_profiles: Vec<EntryProfile> }` + `from_config(c: &Config) -> Self`
  - `load::LoadedProfiles { schema: SchemaModel, profiles: Vec<EntryProfile>, provenance: Vec<Provenance>, disabled: Vec<String>, detected: Vec<DetectedProfile>, containers_sampled: usize, notes: Vec<String>, dropped: Vec<String>, detection_error: Option<String> }` + `status_line(&self) -> Option<String>`
  - `load::assemble(schema: SchemaModel, inputs: &ProfileInputs, sampled: Option<Result<Sample, String>>) -> anyhow::Result<LoadedProfiles>` (pure; `None` = detection disabled)
  - `load::load_profiles(worker: &WorkerHandle, inputs: &ProfileInputs) -> anyhow::Result<LoadedProfiles>`
  - `ui::state::samba_needed(profiles: &[EntryProfile], widgets: &[ResolvedWidget]) -> bool`

- [ ] **Step 1: Write the failing tests**

`src/detect/load.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::fixtures::{demo_sample, schema};

    fn inputs(toml_profiles: &str, enabled: bool) -> ProfileInputs {
        let cfg: Config = toml::from_str(&format!(
            "[server]\nuri = \"ldap://x\"\nbase_dn = \"dc=example,dc=org\"\n[auth]\nbind_dn = \"cn=a\"\n[detect]\nenabled = {enabled}\n{toml_profiles}"
        ))
        .unwrap();
        ProfileInputs::from_config(&cfg)
    }
    const USER: &str = "[[profile]]\nname = \"user\"\nobject_classes = [\"inetOrgPerson\"]\nsearch_base = \"ou=people,dc=example,dc=org\"\n";

    #[test]
    fn detection_off_yields_exactly_the_config_profiles() {
        let l = assemble(schema(), &inputs(USER, false), None).unwrap();
        assert_eq!(l.profiles.len(), 1);
        assert_eq!(l.profiles[0].name, "user");
        assert!(l.profiles[0].defaults.entries.is_empty());
        assert!(l.detected.is_empty() && l.notes.is_empty() && l.status_line().is_none());
    }

    #[test]
    fn detection_on_merges() {
        let l = assemble(schema(), &inputs(USER, true), Some(Ok(demo_sample()))).unwrap();
        assert_eq!(l.profiles[0].name, "user");
        assert!(l.profiles.iter().any(|p| p.name == "user-users"));
        assert_eq!(l.containers_sampled, 4);
    }

    #[test]
    fn a_failed_detection_falls_back_to_config_profiles() {
        let l = assemble(schema(), &inputs(USER, true), Some(Err("connection reset".into()))).unwrap();
        assert_eq!(l.profiles.len(), 1);
        assert_eq!(l.status_line().as_deref(), Some("Profile detection failed: connection reset"));
    }

    #[test]
    fn nothing_visible_is_not_an_error() {
        let l = assemble(schema(), &inputs(USER, true), Some(Ok(Sample::default()))).unwrap();
        assert_eq!(l.profiles.len(), 1);
        assert!(l.detection_error.is_none());
        assert!(l.status_line().is_none());
    }

    #[test]
    fn an_empty_directory_gets_useradd_style_assumptions() {
        let posix = "[[profile]]\nname = \"user\"\nobject_classes = [\"inetOrgPerson\", \"posixAccount\"]\nsearch_base = \"ou=people,dc=example,dc=org\"\n";
        let sample = Sample { group_ou: Some("ou=groups,dc=example,dc=org".into()), ..Default::default() };
        let l = assemble(schema(), &inputs(posix, true), Some(Ok(sample))).unwrap();
        let p = &l.profiles[0];
        assert!(p.companion.is_some());
        assert!(matches!(p.defaults.entries["uidNumber"], crate::config::defaults::DefaultValue::DetectedRange(_)));
        // Detection off: no assumptions.
        let off = assemble(schema(), &inputs(posix, false), None).unwrap();
        assert!(off.profiles[0].companion.is_none() && off.profiles[0].defaults.entries.is_empty());
    }

    #[test]
    fn a_config_widget_error_is_still_a_load_error() {
        let bad = format!("{USER}[profile.widget.member]\nkind = \"picker\"\ncandidate = \"ghost\"\n");
        assert!(assemble(schema(), &inputs(&bad, true), Some(Ok(demo_sample()))).is_err());
    }
}
```

`src/ui/state.rs` tests:

```rust
    #[test]
    fn samba_lookup_runs_for_a_samba_profile_without_widget_or_default() {
        let mut p = crate::workflows::test_fixtures::bare_profile("user");
        p.object_classes = vec!["inetOrgPerson".into(), "sambaSamAccount".into()];
        assert!(super::samba_needed(&[p], &[]));
        let q = crate::workflows::test_fixtures::bare_profile("group");
        assert!(!super::samba_needed(&[q], &[]));
    }
```

- [ ] **Step 2: Run to verify failure** — `cargo test -j4 --lib detect::load ui::state::tests::samba_lookup` → FAIL to compile.

- [ ] **Step 3: Implement**

Prepend to `src/detect/load.rs`:

```rust
//! One entry point for every command that needs profiles (spec §1.4): fetch the
//! schema, sample, detect, merge, validate.

use anyhow::{anyhow, Result};

use crate::config::{Config, EntryProfile, ProfileOverride};
use crate::detect::assume::merge_with_assumptions;
use crate::detect::merge::{validate, Origin, Provenance};
use crate::detect::model::{DetectedProfile, Sample};
use crate::detect::sample::{sample, Budget, WorkerSearcher};
use crate::detect::DETECT_BUDGET;
use crate::ldap::worker::{Request, Response, WorkerHandle};
use crate::schema::SchemaModel;

/// What `load_profiles` needs from the config (captured before the config
/// moves into the worker).
#[derive(Debug, Clone)]
pub struct ProfileInputs {
    pub base_dn: String,
    pub detect_enabled: bool,
    pub overrides: Vec<ProfileOverride>,
    pub config_profiles: Vec<EntryProfile>,
}

impl ProfileInputs {
    pub fn from_config(c: &Config) -> Self {
        ProfileInputs {
            base_dn: c.server.base_dn.clone(),
            detect_enabled: c.detect.enabled,
            overrides: c.overrides.clone(),
            config_profiles: c.profiles.clone(),
        }
    }
}

pub struct LoadedProfiles {
    pub schema: SchemaModel,
    pub profiles: Vec<EntryProfile>,
    pub provenance: Vec<Provenance>,
    pub disabled: Vec<String>,
    pub detected: Vec<DetectedProfile>,
    pub containers_sampled: usize,
    pub notes: Vec<String>,
    pub dropped: Vec<String>,
    pub detection_error: Option<String>,
}

impl LoadedProfiles {
    /// The TUI status line after startup, when detection has something to say.
    pub fn status_line(&self) -> Option<String> {
        if let Some(e) = &self.detection_error {
            return Some(format!("Profile detection failed: {e}"));
        }
        (!self.dropped.is_empty()).then(|| {
            format!(
                "Profile detection dropped {} detected part(s); run `edaptor profiles` for details",
                self.dropped.len()
            )
        })
    }
}

/// Pure assembly. `sampled`: `None` = detection disabled.
pub fn assemble(schema: SchemaModel, inputs: &ProfileInputs, sampled: Option<Result<Sample, String>>) -> Result<LoadedProfiles> {
    let Some(sampled) = sampled else {
        let provenance = inputs
            .config_profiles
            .iter()
            .map(|p| Provenance { name: p.name.clone(), origin: Origin::Config, fields: Default::default(), suppressed: vec![], pending_suppress: vec![], notes: vec![] })
            .collect();
        crate::config::widget::resolve_widgets(&inputs.config_profiles).map_err(|e| anyhow!("widget config error: {e}"))?;
        return Ok(LoadedProfiles {
            schema,
            profiles: inputs.config_profiles.clone(),
            provenance,
            disabled: vec![],
            detected: vec![],
            containers_sampled: 0,
            notes: vec![],
            dropped: vec![],
            detection_error: None,
        });
    };
    let (detected, containers_sampled, mut notes, detection_error, group_ou) = match sampled {
        Ok(s) => {
            let d = crate::detect::infer::detect(&schema, &s);
            let mut notes = s.notes.clone();
            notes.extend(d.notes);
            (d.profiles, s.containers.len(), notes, None, s.group_ou.clone())
        }
        Err(e) => (Vec::new(), 0, Vec::new(), Some(e), None),
    };
    // Rule D runs whenever detection is enabled, also after a failed sample.
    let mut merged = merge_with_assumptions(&schema, &detected, &inputs.overrides, group_ou.as_deref());
    validate(&mut merged).map_err(|e| anyhow!("profile config error: {e}"))?;
    crate::config::widget::resolve_widgets(&merged.profiles).map_err(|e| anyhow!("widget config error: {e}"))?;
    notes.extend(merged.warnings.iter().cloned());
    Ok(LoadedProfiles {
        schema,
        profiles: merged.profiles,
        provenance: merged.provenance,
        disabled: merged.disabled,
        detected,
        containers_sampled,
        notes,
        dropped: merged.dropped,
        detection_error,
    })
}

/// Fetch the subschema, sample (when enabled), detect and merge.
pub fn load_profiles(worker: &WorkerHandle, inputs: &ProfileInputs) -> Result<LoadedProfiles> {
    let raw = match worker.request(Request::FetchSubschema)? {
        Response::Subschema(raw) => raw,
        Response::Error(e) => return Err(anyhow!(e)),
        other => return Err(anyhow!("FetchSubschema: unexpected {other:?}")),
    };
    let schema = SchemaModel::from_raw(&raw);
    let sampled = inputs.detect_enabled.then(|| {
        eprintln!("detecting profiles…");
        sample(&mut WorkerSearcher(worker), &inputs.base_dn, &Budget::new(DETECT_BUDGET))
    });
    assemble(schema, inputs, sampled)
}
```

`src/ui/state.rs` — add next to `samba_in_use`:

```rust
/// Whether startup must look up the Samba domain: a `sambaSID` widget, an
/// `{auto:sambaSID}` default, or any profile whose object classes include
/// `sambaSamAccount` (the built-in bundle then gives `sambaSID` a SID widget).
pub(crate) fn samba_needed(profiles: &[EntryProfile], widgets: &[crate::config::widget::ResolvedWidget]) -> bool {
    samba_in_use(widgets)
        || profiles.iter().any(|p| {
            crate::config::defaults::uses_computed_samba_sid(&p.defaults)
                || p.object_classes.iter().any(|oc| oc.eq_ignore_ascii_case("sambaSamAccount"))
        })
}
```

Rewrite the head of `bootstrap` (everything before `// Tolerant capability probe`) to:

```rust
    use crate::workflows::labels::{label_rules, structure_inputs, structure_scan_attrs};
    let base_dn = config.server.base_dn.clone();
    let inputs = crate::detect::load::ProfileInputs::from_config(&config);
    let tree_rules = crate::config::tree_label::compile_tree_rules(&config.tree);
    let connection_encrypted = config.is_encrypted();
    let samba_from_config = samba_info_from_config(&config);
    let worker = WorkerHandle::spawn(config, password)?;
    // Schema first: detection needs it, and every derived table below is
    // computed once from the merged profiles.
    let loaded = crate::detect::load::load_profiles(&worker, &inputs)?;
    let status = loaded.status_line().unwrap_or_default();
    let crate::detect::load::LoadedProfiles { schema, profiles, .. } = loaded;
    let resolved_widgets = crate::config::widget::resolve_widgets(&profiles)
        .map_err(|e| anyhow!("widget config error: {e}"))?;
    let label_rules = label_rules(&profiles);
    let scan_attrs = structure_scan_attrs(&label_rules, &tree_rules);
    let samba_domain = if samba_needed(&profiles, &resolved_widgets) {
        discover_samba_domain(&worker, &base_dn).or(samba_from_config)
    } else {
        samba_from_config
    };
```

delete the old `FetchSubschema` block, and set `status,` (instead of `status: String::new(),`) in the returned `UiState`.

- [ ] **Step 4: Run tests** — `cargo test -j4 --lib` → PASS.

- [ ] **Step 5: Smoke-test the TUI against the demo server**

```bash
scripts/test-ldap.sh start
export EDAPTOR_TEST_ADMIN_PW=adminpassword
cargo build -j4 && timeout 20 target/debug/edaptor --config examples/demo-config.toml
```

Expected: `detecting profiles…` on stderr, then the TUI; quit with Alt-X. The status line is empty (nothing dropped).

- [ ] **Step 6: Gate and commit** — `CARGO_BUILD_JOBS=4 make check` → `All checks passed!`

```bash
git add src/detect/ src/ui/state.rs
git commit -m "feat(detect): load profiles at startup before anything is derived from them

The TUI now fetches the schema first, detects and merges profiles, and only
then resolves widgets, label rules and the Samba lookup. A profile with
sambaSamAccount now triggers the Samba domain lookup on its own.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 12: `tui-create` by name and chooser filtering

**Files:**
- Modify: `src/ui/mod.rs:61-95` (`StartupRequest`, `resolve_startup`, `run`)
- Modify: `src/main.rs:98-155` and its tests
- Modify: `src/ui/app.rs:399-427` (ChooseThenCreate)
- Modify: `src/workflows/create.rs` (add `chooser_profiles`), `src/detect/mod.rs` (add `is_infrastructure`)

**Interfaces:**
- Produces:
  - `ui::StartupRequest { Create { profile: String, container: Option<String> }, Choose { container: Option<String> } }`
  - `ui::resolve_startup(profiles: &[EntryProfile], req: StartupRequest) -> Result<StartupAction, String>`
  - `ui::run(config: Config, password: String, startup: Option<StartupRequest>) -> Result<()>`
  - `detect::is_infrastructure(p: &EntryProfile) -> bool`
  - `create::chooser_profiles(profiles: &[EntryProfile], container: Option<&str>) -> Vec<usize>`
  - `StartupAction` stays `Create { profile_idx, container } | ChooseThenCreate { container }` (internal, resolved form).

- [ ] **Step 1: Write the failing tests**

In `src/ui/mod.rs` add a test module (move the profile-resolution tests out of `main.rs`):

```rust
#[cfg(test)]
mod startup_tests {
    use super::*;
    use crate::config::EntryProfile;

    fn profiles() -> Vec<EntryProfile> {
        vec![
            EntryProfile { name: "user".into(), search_base: "ou=people,dc=example,dc=org".into(), ..Default::default() },
            EntryProfile { name: "user-people".into(), search_base: "ou=people,dc=example,dc=org".into(), ..Default::default() },
            EntryProfile { name: "NoBase".into(), ..Default::default() },
        ]
    }

    #[test]
    fn create_resolves_a_merged_name_to_its_index() {
        match resolve_startup(&profiles(), StartupRequest::Create { profile: "USER-PEOPLE".into(), container: None }).unwrap() {
            StartupAction::Create { profile_idx, container } => {
                assert_eq!(profile_idx, 1);
                assert_eq!(container, "ou=people,dc=example,dc=org");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn container_override_wins() {
        let a = resolve_startup(&profiles(), StartupRequest::Create { profile: "user".into(), container: Some("ou=x,dc=example,dc=org".into()) }).unwrap();
        assert!(matches!(a, StartupAction::Create { container, .. } if container == "ou=x,dc=example,dc=org"));
    }

    #[test]
    fn unknown_profile_lists_valid_names() {
        let e = resolve_startup(&profiles(), StartupRequest::Create { profile: "Admins".into(), container: None }).unwrap_err();
        assert!(e.contains("Admins") && e.contains("user-people"), "{e}");
    }

    #[test]
    fn empty_search_base_without_container_errors() {
        let e = resolve_startup(&profiles(), StartupRequest::Create { profile: "nobase".into(), container: None }).unwrap_err();
        assert!(e.contains("search_base"));
    }

    #[test]
    fn choose_passes_through() {
        assert!(matches!(
            resolve_startup(&profiles(), StartupRequest::Choose { container: None }).unwrap(),
            StartupAction::ChooseThenCreate { container: None }
        ));
    }
}
```

In `src/workflows/create.rs` tests:

```rust
    #[test]
    fn chooser_hides_detected_infrastructure_outside_its_container() {
        let mut ou = prof("dc=example,dc=org");
        ou.name = "organizationalunit-example".into();
        ou.object_classes = vec!["organizationalUnit".into()];
        ou.scope = crate::config::ContainerScope::Exact;
        let mut cfg_ou = ou.clone();
        cfg_ou.scope = crate::config::ContainerScope::Boundary;
        let user = prof("ou=people,dc=example,dc=org");
        let ps = vec![ou, cfg_ou, user];
        assert_eq!(chooser_profiles(&ps, None), vec![1, 2]);
        assert_eq!(chooser_profiles(&ps, Some("ou=people,dc=example,dc=org")), vec![1, 2]);
        assert_eq!(chooser_profiles(&ps, Some("DC=example,dc=org")), vec![0, 1, 2]);
    }
```

- [ ] **Step 2: Run to verify failure** — `cargo test -j4 --lib ui::startup_tests workflows::create::tests::chooser` → FAIL to compile.

- [ ] **Step 3: Implement**

`src/detect/mod.rs`:

```rust
/// A detected-only profile of an infrastructure class (OU, domain, …). Such a
/// profile is hidden from the all-profiles chooser outside its container.
pub fn is_infrastructure(p: &crate::config::EntryProfile) -> bool {
    p.scope == crate::config::ContainerScope::Exact
        && p.object_classes
            .first()
            .is_some_and(|oc| INFRASTRUCTURE_CLASSES.iter().any(|i| i.eq_ignore_ascii_case(oc)))
}
```

`src/workflows/create.rs`:

```rust
/// Indices of the profiles the all-profiles chooser (`tui-create` without a
/// profile) offers: everything except detected infrastructure profiles, which
/// appear only when `container` is exactly theirs. Pure.
pub fn chooser_profiles(profiles: &[EntryProfile], container: Option<&str>) -> Vec<usize> {
    profiles
        .iter()
        .enumerate()
        .filter(|(_, p)| {
            !crate::detect::is_infrastructure(p)
                || container.is_some_and(|c| crate::detect::dn_eq(c, &p.search_base))
        })
        .map(|(i, _)| i)
        .collect()
}
```

`src/ui/mod.rs`:

```rust
/// What `edaptor tui-create` asked for, before profiles exist (names are
/// resolved against the merged profiles after `bootstrap`).
#[derive(Debug, Clone)]
pub enum StartupRequest {
    Create { profile: String, container: Option<String> },
    Choose { container: Option<String> },
}

/// Resolve a startup request against the merged profiles (case-insensitive).
pub fn resolve_startup(profiles: &[crate::config::EntryProfile], req: StartupRequest) -> Result<StartupAction, String> {
    match req {
        StartupRequest::Choose { container } => Ok(StartupAction::ChooseThenCreate { container }),
        StartupRequest::Create { profile, container } => {
            let idx = crate::workflows::create::resolve_profile_arg(profiles, Some(&profile))?
                .expect("Some(name) resolves to Some(idx) or an error");
            let dn = container.unwrap_or_else(|| profiles[idx].search_base.clone());
            if dn.trim().is_empty() {
                return Err(format!("profile '{}' has no search_base; pass --container", profiles[idx].name));
            }
            Ok(StartupAction::Create { profile_idx: idx, container: dn })
        }
    }
}

pub fn run(config: Config, password: String, startup: Option<StartupRequest>) -> Result<()> {
    let mut booted = state::bootstrap(config, password)?;
    // Resolved before the screen takeover, so an unknown name is reported on the terminal.
    booted.pending_startup = startup
        .map(|r| resolve_startup(&booted.profiles, r))
        .transpose()
        .map_err(|e| anyhow::anyhow!(e))?;
    // (rest unchanged)
```

Update `resolve_profile_arg`'s error text in `create.rs` from "Configured profiles:" to "Available profiles:" (the list now includes detected names) and its test `resolve_profile_arg_unknown_lists_valid_names` accordingly.

`src/main.rs`: replace `build_startup_action` with

```rust
/// Turn the `tui-create` arguments into a [`edaptor::ui::StartupRequest`]. Only
/// the container is checked here; the profile name is resolved after the
/// profiles are loaded, still before the screen takeover.
fn build_startup_request(profile: Option<String>, container: Option<String>) -> Result<edaptor::ui::StartupRequest> {
    use edaptor::ui::StartupRequest;
    if let Some(c) = &container {
        if c.trim().is_empty() {
            return Err(anyhow::anyhow!("--container must not be empty"));
        }
    }
    Ok(match profile {
        Some(profile) => StartupRequest::Create { profile, container },
        None => StartupRequest::Choose { container },
    })
}
```

the `TuiCreate` arm becomes `let req = build_startup_request(profile, container)?; run_tui(config, password, Some(req))?;`, `run_tui` takes `Option<edaptor::ui::StartupRequest>`, and the `main.rs` tests shrink to `blank_container_errors`, `no_profile_yields_choose`, `named_profile_yields_create` against `build_startup_request`. Update the `TuiCreate` doc comment: "Profile name to create (case-insensitive; detected names such as `user-people` work too)."

`src/ui/app.rs` ChooseThenCreate:

```rust
            Some(StartupAction::ChooseThenCreate { container }) => {
                let (idxs, names): (Vec<usize>, Vec<String>) = {
                    let st = state.borrow();
                    let idxs = crate::workflows::create::chooser_profiles(&st.profiles, container.as_deref());
                    let names = idxs.iter().map(|i| st.profiles[*i].name.clone()).collect();
                    (idxs, names)
                };
                if names.is_empty() {
                    state.borrow_mut().status = "No profiles configured.".into();
                    return;
                }
                let (view, focus) = crate::ui::dialog::profile_chooser::build(names, state.clone());
                if prog.exec_view_focused(view, focus) == Command::OK {
                    let chosen = state.borrow_mut().chosen_profile.take();
                    if let Some(idx) = chosen.and_then(|rel| idxs.get(rel).copied()) {
                        // (existing body: dn from container or search_base, then open_create(state, idx, &dn))
                    }
                } else {
                    state.borrow_mut().chosen_profile = None;
                }
            }
```

- [ ] **Step 4: Run tests** — `cargo test -j4` (lib + bins) → PASS.

- [ ] **Step 5: Gate and commit** — `CARGO_BUILD_JOBS=4 make check` → `All checks passed!`

```bash
git add src/ui/mod.rs src/ui/app.rs src/main.rs src/workflows/create.rs src/detect/mod.rs
git commit -m "feat(tui-create): resolve the profile by name after detection; hide OU profiles in the chooser

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 13: `edaptor passwd` searches account profiles only

**Files:**
- Modify: `src/passwd.rs:17-27` and tests; `src/lib.rs:180-245` (`resolve_passwd_target` message, `run_passwd`)

**Interfaces:**
- Produces: `passwd::is_account_profile(p: &EntryProfile) -> bool`; `username_searches` now only yields searches for account profiles.

- [ ] **Step 1: Write the failing tests** (in `src/passwd.rs` tests; give the existing `profile(...)` helper an object-class parameter where needed)

```rust
    #[test]
    fn only_account_profiles_are_searched() {
        let mut user = profile("user", "uid", "ou=people,dc=example,dc=org");
        user.object_classes = vec!["inetOrgPerson".into()];
        let mut group = profile("group", "cn", "ou=groups,dc=example,dc=org");
        group.object_classes = vec!["posixGroup".into()];
        let mut own = profile("svc", "cn", "ou=svc,dc=example,dc=org");
        own.object_classes = vec!["applicationProcess".into()];
        own.widgets.insert("userPassword".into(), crate::config::WidgetSpecCfg::Password { samba: false });
        assert_eq!(
            username_searches(&[user, group, own], "andy"),
            vec![
                ("ou=people,dc=example,dc=org".to_string(), "(uid=andy)".to_string()),
                ("ou=svc,dc=example,dc=org".to_string(), "(cn=andy)".to_string()),
            ]
        );
    }

    #[test]
    fn argus_user_is_not_ambiguous_with_its_private_group() {
        let d = crate::detect::infer::detect(&crate::detect::fixtures::schema(), &crate::detect::fixtures::argus_sample());
        let m = crate::detect::merge::merge(&crate::detect::fixtures::schema(), &d.profiles, &[]);
        let searches = username_searches(&m.profiles, "u01");
        assert_eq!(searches, vec![("ou=people,dc=argus,dc=ch".to_string(), "(cn=u01)".to_string())]);
    }
```

Update `username_searches_one_per_profile_with_base` / `…skip_profiles_without_base_or_rdn` so their profiles carry `object_classes = ["inetOrgPerson"]` (and expect only account profiles).

- [ ] **Step 2: Run to verify failure** — `cargo test -j4 --lib passwd` → FAIL (the group profile is still searched).

- [ ] **Step 3: Implement**

```rust
/// An account profile: it carries a password widget, either its own
/// `[profile.widget.*] kind = "password"` or one of the built-in bundle's
/// (person, inetOrgPerson, posixAccount, sambaSamAccount → userPassword).
pub fn is_account_profile(p: &EntryProfile) -> bool {
    use crate::config::WidgetSpecCfg;
    p.widgets.values().any(|w| matches!(w, WidgetSpecCfg::Password { .. }))
        || p.object_classes.iter().any(|oc| {
            crate::config::builtin::builtin_schema()
                .get(&oc.to_lowercase())
                .is_some_and(|m| m.values().any(|w| matches!(w, WidgetSpecCfg::Password { .. })))
        })
}
```

and in `username_searches` add `.filter(|p| is_account_profile(p))`; update the module docs ("…every account profile's `search_base`…").

`src/lib.rs` `run_passwd`:

```rust
    let inputs = crate::detect::load::ProfileInputs::from_config(&config);
    let worker = WorkerHandle::spawn(config, bind_password)?;
    let loaded = crate::detect::load::load_profiles(&worker, &inputs)?;
    let (target_dn, object_classes) = resolve_passwd_target(&worker, &loaded.profiles, target_arg)?;
```

(remove `let profiles = config.profiles.clone();`), and the NotFound message becomes `"no entry found for username \"{arg}\" in any account profile; pass a full DN instead"`. Also move the stray `run_passwd` doc comment that currently sits above `search_object_classes` (`src/lib.rs:141-154`) back onto `run_passwd`.

- [ ] **Step 4: Run tests** — `cargo test -j4 --lib passwd` → PASS.

- [ ] **Step 5: Gate and commit** — `CARGO_BUILD_JOBS=4 make check` → `All checks passed!`

```bash
git add src/passwd.rs src/lib.rs
git commit -m "fix(passwd): search account profiles only, using detected profiles

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 14: `edaptor profiles` — TOML dump with provenance

**Files:**
- Create: `src/detect/dump.rs`, `src/detect/testdata/argus-profiles.toml` (generated); Modify: `src/detect/mod.rs` (`pub mod dump;`)
- Modify: `src/lib.rs` (`run_profiles`, `ProfilesReport`), `src/main.rs` (`Command::Profiles`)

**Interfaces:**
- Produces:
  - `dump::RangeOutcome { result: Result<RangeReport, String>, uncertain: bool }`
  - `dump::compute_ranges(profiles: &[EntryProfile], scan: &[SampleEntry], truncated: bool) -> BTreeMap<(String, String), RangeOutcome>` (key: lowercased profile name, lowercased attr)
  - `dump::failed_ranges(profiles: &[EntryProfile], msg: &str) -> BTreeMap<(String, String), RangeOutcome>`
  - `dump::header_line(enabled: bool, containers: usize, notes: usize) -> String`
  - `dump::render(profiles: &[EntryProfile], provenance: &[Provenance], disabled: &[String], header: &str, ranges: &BTreeMap<(String, String), RangeOutcome>) -> String`
  - `lib::ProfilesReport { toml: String, notes: Vec<String> }`, `lib::run_profiles(config: Config, password: String, detected_only: bool) -> Result<ProfilesReport>`

- [ ] **Step 1: Write the failing tests** (in `src/detect/dump.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::fixtures::{argus_sample, schema};

    fn argus_dump() -> String {
        let s = argus_sample();
        let d = crate::detect::infer::detect(&schema(), &s);
        let mut m = crate::detect::merge::merge(&schema(), &d.profiles, &[]);
        crate::detect::merge::validate(&mut m).unwrap();
        let scan: Vec<SampleEntry> = s.containers.iter().flat_map(|c| c.entries.clone()).collect();
        let ranges = compute_ranges(&m.profiles, &scan, false);
        render(&m.profiles, &m.provenance, &m.disabled, &header_line(true, s.containers.len(), 0), &ranges)
    }

    #[test]
    fn dump_contains_values_and_provenance() {
        let t = argus_dump();
        assert!(t.starts_with("# detection: 3 containers sampled (up to 200 entries each); notes: none\n"), "{t}");
        assert!(t.contains("name = \"user-people\"  # detected: 12 entries in ou=people,dc=argus,dc=ch"));
        assert!(t.contains("uid = \"{cn}\"  # detected: 12/12"));
        assert!(t.contains("gidNumber = \"{uidNumber}\"  # detected: 12/12 have a private group"));
        assert!(t.contains("uidNumber = \"{next:5000-7999}\"  # detected at dump time: in use 5000-5020; next block at 8000"));
        assert!(t.contains("gidNumber = \"{next:8000-60000}\""));
        assert!(t.contains("# exceptions (defaults.loginShell): cn=u12,ou=people,dc=argus,dc=ch"));
    }

    #[test]
    fn dump_is_valid_toml_that_parses_back_as_overrides() {
        let t = argus_dump();
        let cfg: crate::config::Config = toml::from_str(&format!(
            "[server]\nuri = \"ldap://x\"\nbase_dn = \"dc=argus,dc=ch\"\n[auth]\nbind_dn = \"cn=a\"\n{t}"
        ))
        .expect("the dump must be pasteable");
        assert!(cfg.overrides.iter().any(|o| o.name == "user-people"));
    }

    #[test]
    fn suppressed_parts_and_disabled_profiles_are_commented() {
        let s = argus_sample();
        let d = crate::detect::infer::detect(&schema(), &s);
        #[derive(serde::Deserialize)]
        struct W { profile: Vec<crate::config::ProfileOverride> }
        let o = toml::from_str::<W>("[[profile]]\nname = \"user-people\"\nsuppress = [\"widget.gidNumber\"]\n[profile.defaults]\nloginShell = \"/bin/sh\"\n[[profile]]\nname = \"posixgroup-groups\"\nenabled = false\n").unwrap().profile;
        let m = crate::detect::merge::merge(&schema(), &d.profiles, &o);
        let t = render(&m.profiles, &m.provenance, &m.disabled, "# h", &BTreeMap::new());
        assert!(t.contains("# suppressed by config: widget.gidNumber"));
        assert!(t.contains("loginShell = \"/bin/sh\"  # config (detected \"/bin/bash\", 11/12)"));
        assert!(t.contains("# profile \"posixgroup-groups\" disabled by config (enabled = false)"));
    }

    #[test]
    fn assumed_values_carry_their_reason() {
        #[derive(serde::Deserialize)]
        struct W { profile: Vec<crate::config::ProfileOverride> }
        let o = toml::from_str::<W>("[[profile]]\nname = \"user\"\nobject_classes = [\"inetOrgPerson\", \"posixAccount\"]\nsearch_base = \"ou=people,dc=x\"\n").unwrap().profile;
        let m = crate::detect::assume::merge_with_assumptions(&schema(), &[], &o, Some("ou=groups,dc=x"));
        let ranges = compute_ranges(&m.profiles, &[], false);
        let t = render(&m.profiles, &m.provenance, &m.disabled, &header_line(true, 0, 0), &ranges);
        assert!(t.contains("gidNumber = \"{uidNumber}\"  # assumed: no users yet; useradd-style private group"), "{t}");
        assert!(t.contains("uidNumber = \"{next:10000-60000}\"  # assumed: no uidNumber range configured or detected; useradd-style numbering; at dump time: no numbers in use; useradd-style start at 10000"), "{t}");
        assert!(t.contains("object_classes = [\"posixGroup\"]  # assumed: no users yet; useradd-style private group"), "{t}");
    }

    #[test]
    fn argus_golden() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/detect/testdata/argus-profiles.toml");
        let t = argus_dump();
        if std::env::var_os("EDAPTOR_UPDATE_GOLDEN").is_some() {
            std::fs::write(path, &t).unwrap();
        }
        let golden = std::fs::read_to_string(path).expect("run once with EDAPTOR_UPDATE_GOLDEN=1");
        assert_eq!(t, golden, "dump format changed; review and regenerate with EDAPTOR_UPDATE_GOLDEN=1");
    }
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -j4 --lib detect::dump` → FAIL to compile.

- [ ] **Step 3: Implement** — prepend to `src/detect/dump.rs`:

```rust
//! `edaptor profiles` output: the merged profiles as valid TOML, each value
//! followed by a provenance comment (spec §3 "edaptor profiles"). Pure.

use std::collections::BTreeMap;

use crate::config::defaults::DefaultValue;
use crate::config::{CandidateRef, ChoiceOption, EntryProfile, WidgetSpecCfg};
use crate::detect::merge::{Origin, Provenance, Source};
use crate::detect::model::{Evidence, SampleEntry};
use crate::detect::range::{detect_range, RangeReport};

#[derive(Debug, Clone)]
pub struct RangeOutcome {
    pub result: Result<RangeReport, String>,
    /// The number scan was truncated by a server limit.
    pub uncertain: bool,
}

pub fn compute_ranges(profiles: &[EntryProfile], scan: &[SampleEntry], truncated: bool) -> BTreeMap<(String, String), RangeOutcome> {
    let mut out = BTreeMap::new();
    for p in profiles {
        for (attr, dv) in &p.defaults.entries {
            if let DefaultValue::DetectedRange(spec) = dv {
                out.insert(
                    (p.name.to_lowercase(), attr.to_lowercase()),
                    RangeOutcome { result: detect_range(spec, scan), uncertain: truncated },
                );
            }
        }
    }
    out
}

pub fn failed_ranges(profiles: &[EntryProfile], msg: &str) -> BTreeMap<(String, String), RangeOutcome> {
    let mut out = BTreeMap::new();
    for p in profiles {
        for (attr, dv) in &p.defaults.entries {
            if matches!(dv, DefaultValue::DetectedRange(_)) {
                out.insert((p.name.to_lowercase(), attr.to_lowercase()), RangeOutcome { result: Err(msg.to_string()), uncertain: false });
            }
        }
    }
    out
}

pub fn header_line(enabled: bool, containers: usize, notes: usize) -> String {
    if !enabled {
        return "# detection: disabled ([detect] enabled = false)".to_string();
    }
    let notes = if notes == 0 { "none".to_string() } else { format!("{notes} (printed on stderr)") };
    format!("# detection: {containers} containers sampled (up to {} entries each); notes: {notes}", crate::detect::SAMPLE_SIZE)
}

fn q(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

fn arr(v: &[String]) -> String {
    toml::Value::Array(v.iter().map(|s| toml::Value::String(s.clone())).collect()).to_string()
}

fn key(k: &str) -> String {
    if !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        k.to_string()
    } else {
        q(k)
    }
}

fn line(out: &mut String, k: &str, v: &str, comment: &str) {
    if comment.is_empty() {
        out.push_str(&format!("{k} = {v}\n"));
    } else {
        out.push_str(&format!("{k} = {v}  {comment}\n"));
    }
}

fn ev_text(ev: &Evidence) -> String {
    match &ev.note {
        Some(n) => format!("{} {n}", ev.ratio()),
        None => ev.ratio(),
    }
}

fn src(prov: &Provenance, k: &str) -> String {
    match prov.fields.get(k) {
        None => String::new(),
        Some(Source::Config) => "# config".to_string(),
        Some(Source::Assumed(reason)) => format!("# assumed: {reason}"),
        Some(Source::Detected(ev)) => format!("# detected: {}", ev_text(ev)),
        Some(Source::ConfigOverDetected { detected, evidence }) => format!("# config (detected {detected}, {})", evidence.ratio()),
    }
}

fn origin_comment(o: &Origin) -> String {
    let partial = |p: bool| if p { " (partial sample)" } else { "" };
    match o {
        Origin::Config => "# config".to_string(),
        Origin::Detected { container, sampled, partial: p } => format!("# detected: {sampled} entries in {container}{}", partial(*p)),
        Origin::Merged { detected, container, sampled, partial: p } => {
            format!("# config, merged with detected {}: {sampled} entries in {container}{}", q(detected), partial(*p))
        }
    }
}

fn list(dns: &[String]) -> String {
    let mut s = dns.iter().take(5).cloned().collect::<Vec<_>>().join("; ");
    if dns.len() > 5 {
        s.push_str(&format!(" (+{} more)", dns.len() - 5));
    }
    s
}

fn options(opts: &[ChoiceOption]) -> String {
    let items: Vec<String> = opts.iter().map(|o| format!("{{ value = {}, label = {} }}", q(&o.value), q(&o.label))).collect();
    format!("[{}]", items.join(", "))
}

fn candidate(c: &CandidateRef) -> String {
    match c {
        CandidateRef::Profile(n) => q(n),
        CandidateRef::Inline(s) => {
            let label = s.label.as_ref().map(|l| format!(", label = {}", q(l))).unwrap_or_default();
            format!("{{ base = {}, object_classes = {}, search_attrs = {}{label} }}", q(&s.base), arr(&s.object_classes), arr(&s.search_attrs))
        }
    }
}

/// `(key, TOML value)` lines of one `[profile.widget.<attr>]` table.
pub fn widget_lines(spec: &WidgetSpecCfg) -> Vec<(String, String)> {
    let kv = |k: &str, v: String| (k.to_string(), v);
    match spec {
        WidgetSpecCfg::Choice { select, format, options: o } => vec![kv("kind", q("choice")), kv("select", q(select)), kv("format", q(format)), kv("options", options(o))],
        WidgetSpecCfg::Password { samba } => {
            let mut v = vec![kv("kind", q("password"))];
            if *samba {
                v.push(kv("samba", "true".to_string()));
            }
            v
        }
        WidgetSpecCfg::Picker { candidate: c, store, select } => vec![kv("kind", q("picker")), kv("candidate", candidate(c)), kv("store", q(store)), kv("select", q(select))],
        WidgetSpecCfg::Membership { candidate: c, via } => vec![kv("kind", q("membership")), kv("candidate", candidate(c)), kv("via", q(via))],
        WidgetSpecCfg::Lookup { candidate: c, store, label } => {
            let mut v = vec![kv("kind", q("lookup")), kv("candidate", candidate(c)), kv("store", q(store))];
            if let Some(l) = label {
                v.push(kv("label", q(l)));
            }
            v
        }
        WidgetSpecCfg::Readonly => vec![kv("kind", q("readonly"))],
        WidgetSpecCfg::XOrdered => vec![kv("kind", q("x_ordered"))],
        WidgetSpecCfg::SambaSid => vec![kv("kind", q("samba_sid"))],
    }
}

pub fn render(
    profiles: &[EntryProfile],
    provenance: &[Provenance],
    disabled: &[String],
    header: &str,
    ranges: &BTreeMap<(String, String), RangeOutcome>,
) -> String {
    let mut out = String::new();
    out.push_str(header);
    out.push('\n');
    for name in disabled {
        out.push_str(&format!("# profile {} disabled by config (enabled = false)\n", q(name)));
    }
    for (p, prov) in profiles.iter().zip(provenance) {
        out.push_str("\n[[profile]]\n");
        line(&mut out, "name", &q(&p.name), &origin_comment(&prov.origin));
        line(&mut out, "object_classes", &arr(&p.object_classes), &src(prov, "object_classes"));
        if !p.rdn_attr.is_empty() {
            line(&mut out, "rdn_attr", &q(&p.rdn_attr), &src(prov, "rdn_attr"));
        }
        if !p.search_base.is_empty() {
            line(&mut out, "search_base", &q(&p.search_base), &src(prov, "search_base"));
        }
        if !p.show.is_empty() {
            line(&mut out, "show", &arr(&p.show), &src(prov, "show"));
        }
        if !p.search_attrs.is_empty() {
            line(&mut out, "search_attrs", &arr(&p.search_attrs), &src(prov, "search_attrs"));
        }
        if let Some(l) = &p.label {
            line(&mut out, "label", &q(l), &src(prov, "label"));
        }
        let mut trailer: Vec<String> = Vec::new();
        if !p.defaults.entries.is_empty() {
            out.push_str("[profile.defaults]\n");
            for (attr, dv) in &p.defaults.entries {
                let comment = src(prov, &format!("defaults.{attr}"));
                match dv {
                    DefaultValue::DetectedRange(_) => match ranges.get(&(p.name.to_lowercase(), attr.to_lowercase())) {
                        Some(RangeOutcome { result: Ok(r), uncertain }) => {
                            let unc = if *uncertain { "; uncertain (the number scan hit a server limit)" } else { "" };
                            let lead = match prov.fields.get(&format!("defaults.{attr}")) {
                                Some(Source::Assumed(reason)) => format!("# assumed: {reason}; at dump time: "),
                                _ => "# detected at dump time: ".to_string(),
                            };
                            line(&mut out, &key(attr), &q(&r.template()), &format!("{lead}{}{unc}", r.describe()));
                            if !r.evidence.exceptions.is_empty() {
                                trailer.push(format!("# exceptions (defaults.{attr} range): {}", list(&r.evidence.exceptions)));
                            }
                        }
                        Some(RangeOutcome { result: Err(e), .. }) => out.push_str(&format!("# {attr} = (no range detected: {e})\n")),
                        None => out.push_str(&format!("# {attr} = (range detected at create time)\n")),
                    },
                    other => line(&mut out, &key(attr), &q(&other.to_config_string()), &comment),
                }
            }
        }
        for (attr, spec) in &p.widgets {
            out.push_str(&format!("[profile.widget.{}]\n", key(attr)));
            for (i, (k, v)) in widget_lines(spec).into_iter().enumerate() {
                let c = if i == 0 { src(prov, &format!("widget.{attr}")) } else { String::new() };
                line(&mut out, &k, &v, &c);
            }
        }
        if let Some(c) = &p.companion {
            out.push_str("[profile.companion]\n");
            line(&mut out, "object_classes", &arr(&c.object_classes), &src(prov, "companion"));
            line(&mut out, "rdn_attr", &q(&c.rdn_attr), "");
            line(&mut out, "search_base", &q(&c.search_base), "");
            if !c.attributes.is_empty() {
                out.push_str("[profile.companion.attributes]\n");
                for (k, v) in &c.attributes {
                    line(&mut out, &key(k), &q(v), "");
                }
            }
        }
        for (field, s) in &prov.fields {
            let ev = match s {
                Source::Detected(ev) | Source::ConfigOverDetected { evidence: ev, .. } => ev,
                Source::Config | Source::Assumed(_) => continue,
            };
            if !ev.exceptions.is_empty() {
                out.push_str(&format!("# exceptions ({field}): {}\n", list(&ev.exceptions)));
            }
        }
        for t in trailer {
            out.push_str(&t);
            out.push('\n');
        }
        for s in &prov.suppressed {
            out.push_str(&format!("# suppressed by config: {s}\n"));
        }
        for n in &prov.notes {
            out.push_str(&format!("# note: {n}\n"));
        }
    }
    out
}
```

Note: `ConfigOverDetected.detected` already carries its quotes (`"\"/bin/bash\""` from `{:?}`), so the comment reads `# config (detected "/bin/bash", 11/12)`.

`src/lib.rs`:

```rust
/// `edaptor profiles` output: TOML for stdout, notes for stderr.
pub struct ProfilesReport {
    pub toml: String,
    pub notes: Vec<String>,
}

/// Load (and detect) the profiles, run the number scan for detected ranges,
/// and render the dump. A failed detection is an error (non-zero exit).
pub fn run_profiles(config: Config, password: String, detected_only: bool) -> Result<ProfilesReport> {
    use crate::detect::{dump, model::SampleEntry, range};
    let inputs = crate::detect::load::ProfileInputs::from_config(&config);
    let worker = WorkerHandle::spawn(config, password)?;
    let loaded = crate::detect::load::load_profiles(&worker, &inputs)?;
    if let Some(e) = &loaded.detection_error {
        return Err(anyhow!("profile detection failed: {e}"));
    }
    let (profiles, provenance, disabled) = if detected_only {
        let m = crate::detect::merge::merge(&loaded.schema, &loaded.detected, &[]);
        (m.profiles, m.provenance, Vec::new())
    } else {
        (loaded.profiles, loaded.provenance, loaded.disabled)
    };
    let needs_scan = profiles.iter().any(|p| {
        p.defaults.entries.values().any(|d| matches!(d, crate::config::defaults::DefaultValue::DetectedRange(_)))
    });
    let ranges = if !needs_scan {
        Default::default()
    } else {
        match worker.request(Request::Search {
            id: 1,
            base: inputs.base_dn.clone(),
            scope: SearchScope::Subtree,
            filter: range::SCAN_FILTER.to_string(),
            attrs: range::SCAN_ATTRS.iter().map(|s| s.to_string()).collect(),
            size_limit: None,
        })? {
            Response::Entries { entries, truncated, .. } => {
                let scan: Vec<SampleEntry> = entries.iter().map(SampleEntry::from).collect();
                dump::compute_ranges(&profiles, &scan, truncated)
            }
            Response::SearchError { msg, .. } => dump::failed_ranges(&profiles, &msg),
            other => dump::failed_ranges(&profiles, describe_response(&other)),
        }
    };
    let header = dump::header_line(inputs.detect_enabled, loaded.containers_sampled, loaded.notes.len());
    Ok(ProfilesReport {
        toml: dump::render(&profiles, &provenance, &disabled, &header, &ranges),
        notes: loaded.notes,
    })
}
```

`src/main.rs` — add to `Command`:

```rust
    /// Print the profiles in effect as TOML: detected values with their evidence,
    /// config values, suppressed parts. Notes go to stderr.
    Profiles {
        /// Show detection before merging the config.
        #[arg(long)]
        detected_only: bool,
    },
```

and the arm:

```rust
        Some(Command::Profiles { detected_only }) => {
            let report = edaptor::run_profiles(config, password, detected_only)?;
            for n in &report.notes {
                eprintln!("note: {n}");
            }
            print!("{}", report.toml);
        }
```

- [ ] **Step 4: Generate and review the golden file**

```bash
mkdir -p src/detect/testdata
EDAPTOR_UPDATE_GOLDEN=1 cargo test -j4 --lib detect::dump::tests::argus_golden
cargo test -j4 --lib detect::dump
```

Expected: PASS. Read `src/detect/testdata/argus-profiles.toml` and check by eye: `user-people` first (3 classes → sorted before 1-class profiles), `uid = "{cn}"`, no `cn` default, `loginShell` with the `u12` exception line, the companion table, `posixgroup-groups` with `gidNumber = "{next:8000-60000}"` and the `staff` exception, the note about 12 of 16 groups being private, and `organizationalunit-argus` last.

- [ ] **Step 5: Gate and commit** — `CARGO_BUILD_JOBS=4 make check` → `All checks passed!`

```bash
git add src/detect/ src/lib.rs src/main.rs
git commit -m "feat: edaptor profiles prints the profiles in effect with their provenance

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 15: Live test against the demo server

**Files:**
- Create: `tests/live_profile_detection.rs`, `tests/golden/profiles-demo.toml` (generated)

**Interfaces:** consumes `edaptor::{run_profiles, detect::load::{load_profiles, ProfileInputs}, passwd::{username_searches, resolve_outcome, Resolution}, workflows::create::profiles_for_container, ldap::worker::*}`.

- [ ] **Step 1: Write the tests**

```rust
//! Live test (gated by EDAPTOR_TEST_LDAP_URI): profile detection against the
//! podman demo server. Start it with `scripts/test-ldap.sh start`.

use edaptor::config::Config;
use edaptor::detect::load::{load_profiles, ProfileInputs};
use edaptor::ldap::worker::{Request, Response, SampleParams, SearchScope, WorkerHandle};

fn conn_only(uri: &str) -> Config {
    toml::from_str(&format!(
        "[server]\nuri = \"{uri}\"\nbase_dn = \"dc=example,dc=org\"\n[auth]\nbind_dn = \"cn=admin,dc=example,dc=org\"\npassword_source = \"env:EDAPTOR_TEST_ADMIN_PW\"\n"
    ))
    .unwrap()
}
fn demo(uri: &str) -> Config {
    let text = include_str!("../examples/demo-config.toml").replace("ldap://localhost:11389", uri);
    toml::from_str(&text).unwrap()
}
fn pw() -> String {
    std::env::var("EDAPTOR_TEST_ADMIN_PW").unwrap_or_else(|_| "adminpassword".into())
}
macro_rules! live {
    () => {
        match std::env::var("EDAPTOR_TEST_LDAP_URI") {
            Ok(u) => u,
            Err(_) => {
                eprintln!("SKIP: EDAPTOR_TEST_LDAP_URI not set");
                return;
            }
        }
    };
}
fn load(cfg: Config) -> edaptor::detect::load::LoadedProfiles {
    let inputs = ProfileInputs::from_config(&cfg);
    let worker = WorkerHandle::spawn(cfg, pw()).expect("bind");
    load_profiles(&worker, &inputs).expect("load")
}

#[test]
fn detected_only_yields_the_expected_profiles() {
    let uri = live!();
    let r = edaptor::run_profiles(conn_only(&uri), pw(), true).expect("profiles");
    let t: toml::Table = toml::from_str(&r.toml).expect("valid TOML");
    let names: Vec<String> = t["profile"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap().to_string()).collect();
    for want in ["user-people", "user-users", "posixgroup-groups", "group-groups"] {
        assert!(names.iter().any(|n| n == want), "{want} missing from {names:?}");
    }
}

#[test]
fn merged_order_and_exact_scope() {
    let uri = live!();
    let l = load(conn_only(&uri));
    let pos = |n: &str| l.profiles.iter().position(|p| p.name == n).unwrap_or_else(|| panic!("{n}"));
    assert!(pos("user-people") < pos("user-users"));
    let here: Vec<&str> = edaptor::workflows::create::profiles_for_container(&l.profiles, "ou=people,dc=example,dc=org")
        .into_iter()
        .map(|i| l.profiles[i].name.as_str())
        .collect();
    assert!(here.contains(&"user-people"), "{here:?}");
    assert!(!here.iter().any(|n| n.starts_with("organizationalunit-") || n.starts_with("sambadomain-")), "{here:?}");
}

#[test]
fn demo_config_produces_no_duplicates() {
    let uri = live!();
    let l = load(demo(&uri));
    let mut seen = std::collections::HashSet::new();
    for p in &l.profiles {
        assert!(seen.insert(p.name.to_lowercase()), "duplicate name {}", p.name);
    }
    let mut keys = std::collections::HashSet::new();
    for p in &l.profiles {
        let st = l.schema.structural_class(&p.object_classes).unwrap_or_default().to_lowercase();
        assert!(keys.insert((edaptor::detect::normalize_dn(&p.search_base), st.clone())), "two profiles for {} / {st}", p.search_base);
    }
    assert!(l.profiles.iter().any(|p| p.name == "user"));
    assert!(!l.profiles.iter().any(|p| p.name == "user-people"));
}

#[test]
fn passwd_resolves_with_a_connection_only_config() {
    let uri = live!();
    let cfg = conn_only(&uri);
    let inputs = ProfileInputs::from_config(&cfg);
    let worker = WorkerHandle::spawn(cfg, pw()).unwrap();
    let l = load_profiles(&worker, &inputs).unwrap();
    let mut dns = Vec::new();
    for (base, filter) in edaptor::passwd::username_searches(&l.profiles, "bbrown") {
        if let Response::Entries { entries, .. } = worker
            .request(Request::Search { id: 1, base, scope: SearchScope::Subtree, filter, attrs: vec!["1.1".into()], size_limit: None })
            .unwrap()
        {
            dns.extend(entries.into_iter().map(|e| e.dn));
        }
    }
    assert_eq!(
        edaptor::passwd::resolve_outcome(dns),
        edaptor::passwd::Resolution::Unique("uid=bbrown,ou=people,dc=example,dc=org".into())
    );
}

#[test]
fn types_only_search_returns_names_without_values() {
    let uri = live!();
    let cfg = conn_only(&uri);
    let worker = WorkerHandle::spawn(cfg, pw()).unwrap();
    let resp = worker
        .request(Request::SampleSearch {
            id: 7,
            params: SampleParams {
                base: "ou=people,dc=example,dc=org".into(),
                scope: SearchScope::OneLevel,
                filter: "(objectClass=*)".into(),
                attrs: vec!["*".into()],
                size_limit: Some(5),
                types_only: true,
                time_limit: std::time::Duration::from_secs(5),
            },
        })
        .unwrap();
    let Response::Entries { entries, truncated, .. } = resp else { panic!("{resp:?}") };
    assert_eq!(entries.len(), 5);
    assert!(truncated, "600 users, size limit 5 → partial");
    for e in &entries {
        assert!(e.attrs.keys().any(|k| k.eq_ignore_ascii_case("uid")));
        assert!(e.attrs.values().all(|v| v.is_empty()), "types-only must not carry values");
        assert!(!e.attrs.keys().any(|k| k.eq_ignore_ascii_case("userPassword")) || e.attrs.values().all(|v| v.is_empty()));
    }
}

#[test]
fn profiles_dump_matches_the_golden_file() {
    let uri = live!();
    let r = edaptor::run_profiles(conn_only(&uri), pw(), false).unwrap();
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/profiles-demo.toml");
    if std::env::var_os("EDAPTOR_UPDATE_GOLDEN").is_some() {
        std::fs::write(path, &r.toml).unwrap();
    }
    let golden = std::fs::read_to_string(path).expect("regenerate with EDAPTOR_UPDATE_GOLDEN=1");
    assert_eq!(r.toml, golden, "run against a freshly started server; regenerate with EDAPTOR_UPDATE_GOLDEN=1 after reviewing");
}
```

Also check `edaptor::detect::normalize_dn` is `pub` (it is, Task 1).

- [ ] **Step 2: Run against a fresh demo server** (whole-directory scans → memory cap)

```bash
scripts/test-ldap.sh stop; scripts/test-ldap.sh start
export EDAPTOR_TEST_LDAP_URI=ldap://localhost:11389 EDAPTOR_TEST_ADMIN_PW=adminpassword
mkdir -p tests/golden
EDAPTOR_UPDATE_GOLDEN=1 systemd-run --user --scope -p MemoryMax=2G -- cargo test -j4 --test live_profile_detection -- --test-threads=1
systemd-run --user --scope -p MemoryMax=2G -- cargo test -j4 --test live_profile_detection -- --test-threads=1
```

Expected: 6 passed. Review `tests/golden/profiles-demo.toml`: `user-people` has `sambaSID = "{auto:sambaSID}"`, `gidNumber` as a literal (B3) or a note, a `uidNumber = "{next:…}"` line, `# partial sample` on `user-people` (600 users > 200), and `user-users` carries the `gidNumber = uidNumber for …, but no private groups found` note. If `types_only_search_returns_names_without_values` fails because ldap3 puts empty-valued attributes into `bin_attrs`, change `sample.rs` to also collect `bin_attrs` keys into `present` (the `SampleEntry::from` conversion drops `bin_attrs`); do not weaken the assertion on values.

- [ ] **Step 3: Run the whole live suite once** (regression check for the bootstrap reorder)

```bash
systemd-run --user --scope -p MemoryMax=2G -- cargo test -j4 -- --test-threads=1
```

Expected: all pass.

- [ ] **Step 4: Commit**

```bash
git add tests/live_profile_detection.rs tests/golden/profiles-demo.toml
git commit -m "test: live profile detection against the demo server, with a golden dump

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 16: Documentation, examples, changelog

**Files:**
- Create: `docs/src/configuration/detection.md`
- Modify: `docs/src/SUMMARY.md`, `docs/src/configuration/overview.md`, `docs/src/configuration/full-example.md`, `examples/config.toml`, `README.md:68-101`, `CHANGES.md` (`## Unreleased`)

- [ ] **Step 1: Write `docs/src/configuration/detection.md`**

```markdown
# Profile Detection

eDAPtor works out its entry profiles from the directory. At startup it samples
every container, groups the entries it finds by structural object class, and
turns each group into a profile named `<kind>-<container>`: `user-people`,
`posixgroup-groups`, `organizationalunit-example`. A config with only `[server]`
and `[auth]` can browse, edit and create.

## What is detected

| Part | Rule |
|---|---|
| object classes | classes carried by more than half of the group |
| `rdn_attr` | the most common RDN attribute |
| `show`, `search_attrs`, `label` | MUST attributes and the optional attributes most entries carry; `label = "{cn} ({uid})"` when the two differ |
| defaults | templates such as `uid = "{cn}"`, `cn = "{givenName} {sn}"`, `homeDirectory = "/home/{uid}"`, and shared values such as `loginShell = "/bin/bash"` |
| user-private groups | when users have a `posixGroup` named after them with `gidNumber = uidNumber`: `gidNumber = "{uidNumber}"` and a companion group |
| shared primary group | when most users share one `gidNumber`, that value |
| Samba | `sambaSID = "{auto:sambaSID}"` for `sambaSamAccount` profiles |
| pickers | `memberUid`, `member`, `uniqueMember` pickers and a `gidNumber` lookup, pointing at the matching detected profiles |
| number ranges | `uidNumber` / `gidNumber` ranges, computed when you create an entry (see below) |

A rule applies when **more than half** of the sampled entries follow it and at
least **3** entries were sampled. Entries that break an applied rule are listed
as exceptions by `edaptor profiles`.

### Number ranges

When you create an entry, eDAPtor reads every `uidNumber` and `gidNumber` in the
directory, splits the numbers into blocks wherever two neighbours are more than
1000 apart, takes the block that holds most of the profile's own numbers, and
uses `MIN` = that block's lowest number rounded down to a multiple of 1000 and
`MAX` = one below the next block (or `max(60000, MIN + 9999)`). The new number is
one above the highest number in use in the block. With user-private groups, user
and group numbers share one space. A `{next:MIN-MAX}` in the config replaces the
detected range.

### When there is too little data

A new or nearly empty directory gets what Ubuntu's `useradd` does, with one
change: LDAP numbers start at **10000**, because every client machine hands out
1000 and up to its own local users, and an LDAP account must not share a number
with them.

- With no numbers in use, users and groups are numbered from `{next:10000-60000}`.
  With one or two numbers in use, the block rule above continues from them.
- Every user profile — also one written only in your config — gets a private
  group (`gidNumber = "{uidNumber}"` and a companion `posixGroup` named after the
  user, in the posix-group profile's container or else in `ou=groups` directly
  under `base_dn`), unless the users already in the directory show otherwise
  (for example two users sharing group 100).

These values never replace anything from your config or from detection.
`edaptor profiles` marks them `# assumed: …`; `suppress` removes them like any
detected part, and `[detect] enabled = false` turns them off.

### Limits

Sampling reads at most 200 entries per container and 100 containers, and stops
after 10 seconds; a cut-short sample is marked `partial`. It never reads
`userPassword` or other secrets.

## Overriding detection

A `[[profile]]` block merges over the detected profile with the same `name`
(case-insensitive), or with the same `search_base` and structural class. The
merged profile keeps the config's name. Keys you set replace the detected ones;
`[profile.defaults]` and `[profile.widget.*]` merge per attribute; a
`[profile.companion]` replaces the detected one as a whole.

```toml
[[profile]]
name     = "user-people"
suppress = ["companion", "defaults.loginShell", "widget.gidNumber"]

[profile.defaults]
uidNumber = "{next:10000-19999}"

[[profile]]
name    = "organizationalunit-example"
enabled = false
```

`suppress` removes single detected parts: `companion`, `defaults.<attr>`,
`widget.<attr>`, `label`, `show`, `search_attrs`. `enabled = false` removes a
whole profile. A block that matches no detected profile and has no
`object_classes` is ignored with a warning.

Detection also adds its parts to hand-written profiles it matches, so a create
may write a companion group your config never mentioned. `edaptor profiles`
shows every such addition.

To switch detection off:

```toml
[detect]
enabled = false
```

eDAPtor then behaves exactly as before: every `[[profile]]` needs `name` and
`object_classes`.

## `edaptor profiles`

Prints the profiles in effect as TOML, ready to paste into a config. Each value
carries a comment saying where it came from (`# detected: 12/12`, `# config`,
`# config (detected "/bin/bash", 11/12)`); suppressed parts and exceptions are
listed as comments. `--detected-only` shows detection before the merge. Notes
and warnings go to stderr.
```

- [ ] **Step 2: Wire it into the book and the overview**

`docs/src/SUMMARY.md`: after `- [Overview](configuration/overview.md)` add `- [Profile Detection](configuration/detection.md)`.

`docs/src/configuration/overview.md`: replace the paragraph under "## Top-level shape" with a minimal-config section:

```markdown
## Minimal config

Connection settings are enough; eDAPtor detects the profiles
([Profile Detection](detection.md)):

    [server]
    uri     = "ldaps://ldap.example.com"
    base_dn = "dc=example,dc=com"

    [auth]
    bind_dn         = "cn=ldapmanager,dc=example,dc=com"
    password_source = "prompt"

`[[profile]]` blocks are optional overrides of what detection found; run
`edaptor profiles` to see it.
```

(keep the "Top-level shape" table; add rows `[detect]  # optional: switch profile detection off` and change the `[[profile]]` comment to `# optional overrides of detected profiles`), and add a row to the orientation map: `| [Profile Detection](detection.md) | What eDAPtor detects, how \`[[profile]]\` blocks override it, \`suppress\`, \`[detect]\`, \`edaptor profiles\`. |`.

- [ ] **Step 3: Examples and README**

In `examples/config.toml`, before the first `[[profile]]`, add:

```toml
# Profile detection (on by default). eDAPtor derives profiles from the
# directory; the [[profile]] blocks below override what it detects.
# `edaptor profiles` prints the result. `enabled = false` restores the old
# behaviour, where every [[profile]] needs name and object_classes.
[detect]
enabled = true
```

and after the last profile:

```toml
# Change a detected profile without restating it: drop single detected parts
# (companion, defaults.<attr>, widget.<attr>, label, show, search_attrs), or
# the whole profile with `enabled = false`.
[[profile]]
name     = "posixgroup-groups"
suppress = ["widget.memberUid"]
```

Copy the complete new `examples/config.toml` into the TOML block of
`docs/src/configuration/full-example.md` and verify:

```bash
diff <(awk '/^```toml$/{f=1;next} /^```$/{f=0} f' docs/src/configuration/full-example.md) examples/config.toml && echo identical
```

Expected: `identical`. Add one sentence to the prose of `full-example.md` pointing at [Profile Detection](detection.md).

`README.md` "## Configuration": the skeleton becomes `[server]` + `[auth]` only, followed by

```markdown
eDAPtor detects users, groups and their rules from the directory; `[[profile]]`
blocks only override what it got wrong, and `edaptor profiles` shows what it
detected.
```

and add a bullet to the doc-links list: `- [Profile Detection](https://oposs.github.io/edaptor/configuration/detection.html)`.

- [ ] **Step 4: CHANGES.md** (under `## Unreleased`)

Under `### New`:

```markdown
- **eDAPtor works out users, groups and their rules from the directory.** A
  config with only `[server]` and `[auth]` can now create entries: eDAPtor
  samples each container at startup and detects object classes, naming,
  defaults such as `homeDirectory = "/home/{uid}"`, user-private groups, pickers
  and free `uidNumber`/`gidNumber` ranges. Startup prints `detecting profiles…`
  and can take up to 10 seconds longer on a slow server.
- **`edaptor profiles` prints the profiles in effect** as TOML you can paste
  into a config, each value marked as detected (with how many entries follow
  it) or taken from the config; `--detected-only` shows detection alone.
```

and, as its own entry:

```markdown
- **A new, empty directory gets useradd-style defaults.** With no users yet,
  eDAPtor numbers users and groups from 10000 up (client machines use 1000 and
  up for their local users) and gives every new user a private group, placed in
  `ou=groups` when there is no group profile. `edaptor profiles` marks these
  values `# assumed`, and `suppress` removes them.
```

Under `### Changed`:

```markdown
- **Detection is on for existing configs too and adds to hand-written
  profiles.** A `[[profile]]` with the same name as a detected profile, or the
  same `search_base` and object class, receives the detected defaults, widgets
  and companion it does not set itself, so creating a user may now also create
  its private group. Remove one part with `suppress = ["companion"]`, a whole
  profile with `enabled = false`, or all detection with `[detect] enabled = false`.
- **`edaptor tui-create` accepts detected profile names** such as
  `user-people`, and the profile chooser no longer offers OU or domain entries
  outside their own container.
- **`edaptor passwd <user>` only searches user accounts**, so a user whose
  private group has the same name no longer fails with "matches multiple entries".
- **Profile names in `candidate = "…"` are case-insensitive**:
  `candidate = "PosixGroup"` now finds the `posixgroup` profile instead of
  failing with `widget config error`.
```

Under `### Fixed`:

```markdown
- **The `sambaSID` field generates the SID for every Samba profile.** It fell
  back to plain text unless the config set `[samba] domain_sid`, a `sambaSID`
  widget or an `{auto:sambaSID}` default; eDAPtor now looks up the Samba domain
  whenever a profile includes `sambaSamAccount`.
```

- [ ] **Step 5: Build the book and gate**

```bash
make docs    # skip with a note if mdbook is not installed
CARGO_BUILD_JOBS=4 make check
```

Expected: book builds; `All checks passed!` (`reference_config_parses` and `demo_config_widgets_resolve` still pass: the new override-only block has no `object_classes` and detection is enabled, so it is not a load error).

- [ ] **Step 6: Commit**

```bash
git add docs/src/ examples/config.toml README.md CHANGES.md
git commit -m "docs: profile detection page, minimal config, examples and changelog

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Self-review notes (for the executor)

- Spec coverage: §1.1 sampling → Task 10; §1.2 detect → Tasks 2, 3, 6; §1.3 merge → Task 8; §1.4 startup/consumers → Tasks 11 (TUI, samba), 12 (tui-create, chooser), 13 (passwd), 14 (profiles); `check`/`schema` unchanged but still validate companions offline → Task 7; §2A → Task 2 (+ scope Task 7, order Task 8, infrastructure Task 12); §2B1–5 → Tasks 3, 6; §2C → Tasks 4, 5, 6, 14; §2D (assumptions) → Task 4 (empty and 1–2-value ranges), Task 6 (contrary-evidence count), Task 9 (rule D), Task 10 (`ou=groups` fact), Task 11 (wiring), Task 14 (dump), Task 16 (docs); §3 → Tasks 7, 8, 14; §4 errors table → Tasks 8, 10, 11, 14; §5 tests 1–7 → spread as listed in each task; §6 docs → Task 16.
- Member-target pickers (B5) take the target container from the member DNs' parents; no extra LDAP lookup is needed for them, so Task 10 only implements the private-group lookups and the `ou=groups` existence check.
- `make check` itself runs `cargo test` without `-j`; always invoke it as `CARGO_BUILD_JOBS=4 make check`.
