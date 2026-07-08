# Lookup Widget Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a new `lookup` widget kind: a scalar field (e.g. `gidNumber`) shown in the form as `<value> (<name>)`, edited via an editable-combobox popup where you type a number freely or filter a candidate list and pick one.

**Architecture:** A new `WidgetKind::Lookup(LookupBinding)` resolved from `[profile.widget.<attr>]`. The always-visible form resolves the number → friendly name with a dedicated async flow (`ResolveFlow`, id range 4_000_000+) whose results land in a `UiState` cache the form pane reads. The edit popup (`LookupDialog`) is a value-in-input combobox: the input's leading integer is the committed value; a candidate list below filters as you type and, when picked, writes `<value> (<name>)` back into the input.

**Tech Stack:** Rust, tvision-rs 0.9 (`src/ui/`), serde/TOML config, the existing worker/`SearchFlow` async plumbing.

## Global Constraints

- **Cap build/test parallelism at 4 cores** — shared machine. Use `cargo test -j4`, `cargo clippy -j4`.
- **Facade boundary:** only `src/ui/**` may `use tvision_rs`. `src/config/**` and `src/workflows/**` (incl. the new `resolve_flow.rs`) stay UI-agnostic — no `tvision_rs`, no `crate::ui`.
- **Borrow discipline:** never hold a `RefCell`/`UiState` borrow across `ctx.broadcast`/`ctx.post`/`exec_view`/`worker.submit`/`new_list`/`child_mut`/`set_value`. Collect into locals → drop the borrow → call.
- **Strict TDD**, atomic commits, crate compiles + `cargo fmt` + `cargo clippy --all-targets -- -D warnings` clean before each commit.
- **English** for all identifiers, comments, keys. User-facing docs may use other languages (not needed here).
- **Commit trailer:** `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. Use `git commit -F` for messages with backticks.
- **Docs one-home:** config detail → `docs/src/`; `CHANGES.md` for the user-visible change; keep `README.md` orientation-only.
- **Spec:** `docs/superpowers/specs/2026-07-05-lookup-widget-design.md`.

---

## File Structure

- `src/config/mod.rs` — add the `WidgetSpecCfg::Lookup { candidate, store, label }` deserialize variant. *(Task 1)*
- `src/config/relation.rs` — add `LookupBinding` + `scope_id()`. *(Task 1)*
- `src/config/widget.rs` — add `WidgetKind::Lookup(LookupBinding)` and its `resolve_widgets` arm. *(Task 1)*
- `src/config/resolver.rs` — surface `WidgetKind::Lookup` from layer-3 profile config. *(Task 1)*
- `src/workflows/resolve_flow.rs` *(new)* — async reverse-name resolution: `LookupKey`, `ResolveFlow`, `ResolveOutcome`, `build_equality_filter`. *(Task 2)*
- `src/workflows/mod.rs` — register `resolve_flow`. *(Task 2)*
- `src/ui/state.rs` — `lookup_cache`, `resolve_flow` field, `resolve_lookup()`, `apply_resolve_outcome()`, pump wiring. *(Task 3)*
- `src/ui/lookup.rs` *(new)* — pure `lookup_model` fns *(Task 4)*, then `LookupWidget`/`LookupEditor`/`LookupDialog` *(Task 5)*.
- `src/ui/mod.rs` — register `lookup`. *(Task 4)*
- `src/ui/widget.rs` — route `Lookup` in `widget_for` + `is_modal_field`. *(Task 5)*
- `src/ui/panes/form.rs` — render `<value> (<name>)` from the cache + trigger resolve. *(Task 6)*
- `docs/src/configuration/widgets.md`, `CHANGES.md`, `examples/config.toml`, `examples/demo-config.toml`, `README.md`. *(Task 7)*

---

## Task 1: Config — `WidgetKind::Lookup` binding

**Files:**
- Modify: `src/config/mod.rs` (add variant to `WidgetSpecCfg`, ~line 175)
- Modify: `src/config/relation.rs` (add `LookupBinding`)
- Modify: `src/config/widget.rs` (add `WidgetKind::Lookup`, `resolve_widgets` arm)
- Modify: `src/config/resolver.rs` (surface `Lookup` — verify layer-3 passes it through)

**Interfaces:**
- Produces: `crate::config::relation::LookupBinding { attr: String, scope: CandidateScope, store: String, label_template: Vec<LabelSeg> }` with `fn scope_id(&self) -> String`.
- Produces: `crate::config::widget::WidgetKind::Lookup(LookupBinding)`.
- Consumes: existing `resolve_candidate`, `CandidateScope`, `crate::config::label::{parse_label_template, LabelSeg}`.

- [ ] **Step 1: Write the failing config-parse test**

In `src/config/mod.rs` tests module (near the existing `WidgetSpecCfg::Picker` test around line 554), add:

```rust
#[test]
fn lookup_widget_parses_with_candidate_store_and_label() {
    let toml = r#"
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
        WidgetSpecCfg::Lookup { candidate, store, label } => {
            assert!(matches!(candidate, CandidateRef::Profile(n) if n == "posixgroup"));
            assert_eq!(store, "gidNumber");
            assert_eq!(label.as_deref(), Some("{cn}"));
        }
        other => panic!("expected Lookup, got {other:?}"),
    }
}
```

(Reuse the existing test imports; `CandidateRef` and `WidgetSpecCfg` are already imported in that module — check the top of the `#[cfg(test)] mod` block and add `use super::CandidateRef;` only if missing.)

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -j4 --lib config::lookup_widget_parses_with_candidate_store_and_label`
Expected: FAIL to compile — `no variant named Lookup`.

- [ ] **Step 3: Add the `WidgetSpecCfg::Lookup` variant**

In `src/config/mod.rs`, inside `enum WidgetSpecCfg` (after the `Membership { .. }` variant, before `Readonly`):

```rust
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
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -j4 --lib config::lookup_widget_parses_with_candidate_store_and_label`
Expected: PASS.

- [ ] **Step 5: Write the failing resolve test**

In `src/config/widget.rs` tests module (after the existing picker resolve tests, ~line 356), add:

```rust
#[test]
fn resolve_lookup_builds_binding_with_default_label() {
    use crate::config::{CandidateRef, EntryProfile, WidgetSpecCfg};
    let group = EntryProfile {
        name: "posixgroup".into(),
        object_classes: vec!["posixGroup".into()],
        search_base: "ou=groups,dc=x".into(),
        search_attrs: vec!["cn".into()],
        ..Default::default()
    };
    let mut user = EntryProfile {
        name: "user".into(),
        object_classes: vec!["posixAccount".into()],
        ..Default::default()
    };
    user.widgets.insert(
        "gidNumber".into(),
        WidgetSpecCfg::Lookup {
            candidate: CandidateRef::Profile("posixgroup".into()),
            store: "gidNumber".into(),
            label: None,
        },
    );
    let widgets = resolve_widgets(&[group, user]).expect("resolve");
    let w = widgets.iter().find(|w| w.attr == "gidNumber").expect("gidNumber widget");
    match &w.kind {
        WidgetKind::Lookup(b) => {
            assert_eq!(b.store, "gidNumber");
            assert_eq!(b.scope.base, "ou=groups,dc=x");
            assert_eq!(b.scope.object_classes, vec!["posixGroup".to_string()]);
            // Default label template is {cn}.
            assert_eq!(
                b.label_template,
                crate::config::label::parse_label_template("{cn}")
            );
            assert_eq!(b.scope_id(), "ou=groups,dc=x|posixGroup|gidNumber");
        }
        other => panic!("expected Lookup, got {other:?}"),
    }
}
```

- [ ] **Step 6: Run it to verify it fails**

Run: `cargo test -j4 --lib config::widget::resolve_lookup_builds_binding_with_default_label`
Expected: FAIL to compile — `LookupBinding` / `WidgetKind::Lookup` do not exist.

- [ ] **Step 7: Add `LookupBinding` to `relation.rs`**

In `src/config/relation.rs`, after the `PickerBinding` impl (~line 61):

```rust
/// A `[profile.widget.<attr>]` `kind = "lookup"` binding resolved against the
/// profile list. The stored value is a scalar (`store`); the same attribute is the
/// reverse-lookup match key used to resolve the friendly name shown in the form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LookupBinding {
    /// The attribute this binds (e.g. `gidNumber`).
    pub attr: String,
    /// Resolved candidate search scope (where the named entries live).
    pub scope: CandidateScope,
    /// The candidate attribute matched on (== the stored scalar), e.g. `gidNumber`.
    pub store: String,
    /// Parsed display-label template for the resolved candidate (e.g. `{cn}`).
    pub label_template: Vec<crate::config::label::LabelSeg>,
}

impl LookupBinding {
    /// The candidate's first (structural) object class, or `""` when none.
    pub fn object_class(&self) -> &str {
        self.scope.object_classes.first().map(String::as_str).unwrap_or("")
    }

    /// A stable identity for this binding's candidate scope, independent of the
    /// looked-up value. Used to key the `UiState` resolution cache so two entries
    /// sharing a `gidNumber` share one resolved name.
    pub fn scope_id(&self) -> String {
        format!("{}|{}|{}", self.scope.base, self.object_class(), self.store)
    }
}
```

- [ ] **Step 8: Add `WidgetKind::Lookup` and the resolve arm**

In `src/config/widget.rs`, add the variant to `enum WidgetKind` (after `Picker(...)`, ~line 43):

```rust
    /// A scalar value with a friendly-name popup (see `config::relation::LookupBinding`).
    Lookup(crate::config::relation::LookupBinding),
```

In `resolve_widgets`, add a match arm (after the `WidgetSpecCfg::Membership { .. }` arm, ~line 192):

```rust
                WidgetSpecCfg::Lookup {
                    candidate,
                    store,
                    label,
                } => {
                    let scope = resolve_candidate(candidate, profiles)?;
                    // Explicit widget `label` wins; else the candidate profile's own
                    // label; else the bare `{cn}` default.
                    let label_template = match label {
                        Some(l) => crate::config::label::parse_label_template(l),
                        None => scope
                            .label_template
                            .clone()
                            .unwrap_or_else(|| crate::config::label::parse_label_template("{cn}")),
                    };
                    WidgetKind::Lookup(crate::config::relation::LookupBinding {
                        attr: attr.clone(),
                        scope,
                        store: store.clone(),
                        label_template,
                    })
                }
```

- [ ] **Step 9: Confirm the resolver passes `Lookup` through**

Open `src/config/resolver.rs` and read `resolve_kind`'s layer-3 branch (the one that consults `profile_widgets` / `widget_for`). It returns the `WidgetKind` from the matched `ResolvedWidget` verbatim, so `Lookup` flows through with no code change. Add this regression test at the end of `resolver.rs`'s test module:

```rust
#[test]
fn resolver_surfaces_lookup_from_profile_config() {
    use crate::config::relation::{CandidateScope, LookupBinding};
    use crate::config::widget::{ResolvedWidget, WidgetKind};
    let schema = crate::schema::model::SchemaModel::from_raw(&crate::ldap::worker::RawSubschema::default());
    let profiles: Vec<crate::config::EntryProfile> = vec![];
    let widgets = vec![ResolvedWidget {
        owner_object_classes: vec!["posixAccount".into()],
        attr: "gidNumber".into(),
        kind: WidgetKind::Lookup(LookupBinding {
            attr: "gidNumber".into(),
            scope: CandidateScope {
                base: "ou=groups,dc=x".into(),
                object_classes: vec!["posixGroup".into()],
                search_attrs: vec!["cn".into()],
                label_template: None,
            },
            store: "gidNumber".into(),
            label_template: crate::config::label::parse_label_template("{cn}"),
        }),
    }];
    let r = WidgetResolver::new(&schema, &profiles, &widgets, false);
    assert!(matches!(
        r.resolve_kind("gidNumber", &["posixAccount".into()]),
        Some(WidgetKind::Lookup(_))
    ));
}
```

- [ ] **Step 10: Run the full config suite**

Run: `cargo test -j4 --lib config`
Expected: PASS (all config tests, including the three new ones).

- [ ] **Step 11: fmt + clippy + commit**

```bash
cargo fmt
cargo clippy -j4 --all-targets -- -D warnings
git add src/config/mod.rs src/config/relation.rs src/config/widget.rs src/config/resolver.rs
git commit -F - <<'EOF'
feat(config): add the lookup widget kind (scalar + friendly-name binding)

WidgetSpecCfg::Lookup { candidate, store, label } resolves to
WidgetKind::Lookup(LookupBinding), reusing the picker candidate scope.
`store` doubles as the reverse-lookup match key; `scope_id()` keys the
resolution cache added in a later task.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

## Task 2: `ResolveFlow` — async reverse-name resolution

**Files:**
- Create: `src/workflows/resolve_flow.rs`
- Modify: `src/workflows/mod.rs` (add `pub mod resolve_flow;`)
- Test: inline `#[cfg(test)] mod tests` in `resolve_flow.rs`

**Interfaces:**
- Produces: `LookupKey { scope_id: String, value: String }` (`Clone, PartialEq, Eq, Hash, Debug`).
- Produces: `enum ResolveOutcome { Resolved { key: LookupKey, name: String }, NotFound { key: LookupKey }, Ignored }`.
- Produces: `struct ResolveFlow` with `fn new()`, `fn request(&mut self, worker, base, oc, store_attr, value, attrs, template) -> Result<u64>`, `fn on_response(&mut self, resp) -> ResolveOutcome`, `fn is_pending(&self, key: &LookupKey) -> bool`, and `#[cfg(test)] fn force_pending(&mut self, id: u64, key: LookupKey, template: Vec<LabelSeg>)`.
- Produces: `fn build_equality_filter(oc: &str, attr: &str, value: &str) -> String`.
- Consumes: `crate::ldap::worker::{Request, Response, SearchScope, WorkerHandle, LdapEntry}`, `crate::config::label::{LabelSeg, render_label}`.

- [ ] **Step 1: Write the failing filter test**

Create `src/workflows/resolve_flow.rs` with only the test first:

```rust
//! Async reverse name-resolution for the `lookup` widget. Given a stored scalar
//! (e.g. `gidNumber = 5000`), resolve the friendly name of the candidate whose
//! `store` attribute equals that value, so the form can show `5000 (staff)`.
//!
//! Id range 4_000_000+ keeps responses disjoint from ReadFlow (1) / WriteFlow
//! (1_000_000) / AllocFlow (2_000_000) / SearchFlow (3_000_000). Unlike
//! SearchFlow (which tracks only the latest term), ResolveFlow tracks EVERY
//! in-flight request so many distinct values resolve concurrently.
//!
//! No tvision_rs, no crate::ui — pure domain logic.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equality_filter_escapes_value() {
        assert_eq!(
            build_equality_filter("posixGroup", "gidNumber", "5000"),
            "(&(objectClass=posixGroup)(gidNumber=5000))"
        );
        // RFC-4515 metacharacters in the value are escaped.
        assert_eq!(
            build_equality_filter("posixGroup", "cn", "a*b"),
            "(&(objectClass=posixGroup)(cn=a\\2ab))"
        );
    }
}
```

- [ ] **Step 2: Register the module and run the test to see it fail**

Add to `src/workflows/mod.rs` (keep the list alphabetical — after `read_flow`):

```rust
pub mod resolve_flow;
```

Run: `cargo test -j4 --lib workflows::resolve_flow::tests::equality_filter_escapes_value`
Expected: FAIL to compile — `build_equality_filter` not found.

- [ ] **Step 3: Implement `build_equality_filter`**

Add above the test module in `resolve_flow.rs`:

```rust
use anyhow::Result;
use std::collections::HashMap;

use crate::config::label::{render_label, LabelSeg};
use crate::ldap::worker::{Request, Response, SearchScope, WorkerHandle};

/// RFC-4515-escape a filter assertion value: `* ( ) \ NUL` become `\HH`.
fn escape_filter_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'*' => out.push_str("\\2a"),
            b'(' => out.push_str("\\28"),
            b')' => out.push_str("\\29"),
            b'\\' => out.push_str("\\5c"),
            0 => out.push_str("\\00"),
            _ => out.push(b as char),
        }
    }
    out
}

/// Build an exact-match filter `(&(objectClass=<oc>)(<attr>=<value>))` with the
/// value RFC-4515-escaped. Used to find the single candidate whose `store`
/// attribute equals the field's stored value.
pub fn build_equality_filter(oc: &str, attr: &str, value: &str) -> String {
    format!(
        "(&(objectClass={})({}={}))",
        oc,
        attr,
        escape_filter_value(value)
    )
}
```

Note: `escape_filter_value` treats input as ASCII bytes rendered as `char`; lookup values are numeric/`cn` text, so this is safe. (Non-ASCII bytes would pass through as-is, matching the picker's tolerance.)

- [ ] **Step 4: Run the filter test**

Run: `cargo test -j4 --lib workflows::resolve_flow::tests::equality_filter_escapes_value`
Expected: PASS.

- [ ] **Step 5: Write the failing `LookupKey`/`on_response` tests**

Add to the `tests` module in `resolve_flow.rs`:

```rust
use crate::config::label::parse_label_template;
use crate::ldap::worker::LdapEntry;
use std::collections::BTreeMap;

fn key(v: &str) -> LookupKey {
    LookupKey { scope_id: "ou=groups,dc=x|posixGroup|gidNumber".into(), value: v.into() }
}

#[test]
fn resolved_renders_label_from_first_entry() {
    let mut rf = ResolveFlow::new();
    rf.force_pending(4_000_000, key("5000"), parse_label_template("{cn}"));
    let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    attrs.insert("cn".into(), vec!["staff".into()]);
    let resp = Response::Entries {
        id: 4_000_000,
        entries: vec![LdapEntry { dn: "cn=staff,ou=groups,dc=x".into(), attrs, bin_attrs: Default::default() }],
        truncated: false,
    };
    assert_eq!(
        rf.on_response(&resp),
        ResolveOutcome::Resolved { key: key("5000"), name: "staff".into() }
    );
    // The id is consumed: a second identical response is Ignored.
    assert_eq!(rf.on_response(&resp), ResolveOutcome::Ignored);
}

#[test]
fn no_entries_is_not_found() {
    let mut rf = ResolveFlow::new();
    rf.force_pending(4_000_001, key("9999"), parse_label_template("{cn}"));
    let resp = Response::Entries { id: 4_000_001, entries: vec![], truncated: false };
    assert_eq!(rf.on_response(&resp), ResolveOutcome::NotFound { key: key("9999") });
}

#[test]
fn search_error_is_not_found() {
    let mut rf = ResolveFlow::new();
    rf.force_pending(4_000_002, key("1"), parse_label_template("{cn}"));
    let resp = Response::SearchError { id: 4_000_002, msg: "boom".into() };
    assert_eq!(rf.on_response(&resp), ResolveOutcome::NotFound { key: key("1") });
}

#[test]
fn unknown_id_is_ignored_and_is_pending_tracks_keys() {
    let mut rf = ResolveFlow::new();
    rf.force_pending(4_000_003, key("42"), parse_label_template("{cn}"));
    assert!(rf.is_pending(&key("42")));
    assert!(!rf.is_pending(&key("43")));
    let resp = Response::Entries { id: 999, entries: vec![], truncated: false };
    assert_eq!(rf.on_response(&resp), ResolveOutcome::Ignored);
    assert!(rf.is_pending(&key("42")), "unrelated response must not clear pending");
}
```

- [ ] **Step 6: Run to verify they fail**

Run: `cargo test -j4 --lib workflows::resolve_flow`
Expected: FAIL to compile — `LookupKey`, `ResolveOutcome`, `ResolveFlow` missing.

- [ ] **Step 7: Implement `LookupKey`, `ResolveOutcome`, `ResolveFlow`**

Add above the test module in `resolve_flow.rs`:

```rust
/// Identity of one reverse-lookup: a scope (base|objectClass|store attr) plus the
/// stored value. Keys the `UiState` resolution cache.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LookupKey {
    pub scope_id: String,
    pub value: String,
}

/// The result of correlating one worker response against the in-flight resolves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveOutcome {
    /// The candidate was found; `name` is the rendered label.
    Resolved { key: LookupKey, name: String },
    /// No candidate matched (empty result or search error): show the bare value.
    NotFound { key: LookupKey },
    /// The response id did not match any in-flight resolve; discard it.
    Ignored,
}

/// One in-flight resolve: the key it will produce and the label template to render.
struct Pending {
    key: LookupKey,
    template: Vec<LabelSeg>,
}

/// Async reverse name-resolution. Tracks every in-flight request id → its
/// `Pending`, so concurrent resolves for different values all complete.
pub struct ResolveFlow {
    next_id: u64,
    inflight: HashMap<u64, Pending>,
}

impl Default for ResolveFlow {
    fn default() -> Self {
        Self::new()
    }
}

impl ResolveFlow {
    /// First allocated id is 4_000_000 (disjoint from the other flows).
    pub fn new() -> Self {
        ResolveFlow { next_id: 4_000_000, inflight: HashMap::new() }
    }

    fn alloc(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Submit an exact-match search for the candidate whose `store_attr == value`.
    /// `attrs` are the label-template attributes to fetch; `template` is rendered
    /// against the first matching entry in `on_response`. Records the id as pending.
    pub fn request(
        &mut self,
        worker: &WorkerHandle,
        base: &str,
        oc: &str,
        store_attr: &str,
        value: &str,
        attrs: &[String],
        template: Vec<LabelSeg>,
    ) -> Result<u64> {
        let id = self.alloc();
        worker.submit(Request::Search {
            id,
            base: base.to_string(),
            scope: SearchScope::Subtree,
            filter: build_equality_filter(oc, store_attr, value),
            attrs: attrs.to_vec(),
            size_limit: Some(2),
        })?;
        let key = LookupKey { scope_id: format!("{base}|{oc}|{store_attr}"), value: value.to_string() };
        self.inflight.insert(id, Pending { key, template });
        Ok(id)
    }

    /// Whether a resolve for `key` is currently in flight.
    pub fn is_pending(&self, key: &LookupKey) -> bool {
        self.inflight.values().any(|p| &p.key == key)
    }

    /// Correlate one worker response. Removes the matched id from `inflight`.
    pub fn on_response(&mut self, resp: &Response) -> ResolveOutcome {
        match resp {
            Response::Entries { id, entries, .. } => {
                let Some(p) = self.inflight.remove(id) else {
                    return ResolveOutcome::Ignored;
                };
                match entries.first() {
                    Some(e) => ResolveOutcome::Resolved {
                        key: p.key,
                        name: render_label(&p.template, &e.attrs),
                    },
                    None => ResolveOutcome::NotFound { key: p.key },
                }
            }
            Response::SearchError { id, .. } => match self.inflight.remove(id) {
                Some(p) => ResolveOutcome::NotFound { key: p.key },
                None => ResolveOutcome::Ignored,
            },
            _ => ResolveOutcome::Ignored,
        }
    }

    /// Test-only: register an in-flight resolve without a live worker.
    #[cfg(test)]
    pub(crate) fn force_pending(&mut self, id: u64, key: LookupKey, template: Vec<LabelSeg>) {
        self.inflight.insert(id, Pending { key, template });
    }
}
```

Note: `LookupKey::scope_id` here (`base|oc|store_attr`) MUST match `LookupBinding::scope_id()` from Task 1 (`scope.base | object_class | store`). They are the same three parts in the same order — keep them identical.

- [ ] **Step 8: Run the resolve suite**

Run: `cargo test -j4 --lib workflows::resolve_flow`
Expected: PASS (all five tests).

- [ ] **Step 9: fmt + clippy + commit**

```bash
cargo fmt
cargo clippy -j4 --all-targets -- -D warnings
git add src/workflows/resolve_flow.rs src/workflows/mod.rs
git commit -F - <<'EOF'
feat(workflows): ResolveFlow for async reverse name-resolution

Given a stored scalar, resolve the candidate whose `store` attribute
equals it and render its label. Id range 4_000_000+; tracks every
in-flight request so many values resolve concurrently. Pure domain logic.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

## Task 3: `UiState` — resolution cache + pump wiring

**Files:**
- Modify: `src/ui/state.rs` (struct fields, constructors, `resolve_lookup`, `apply_resolve_outcome`, `process_responses`)
- Test: `src/ui/state.rs` tests module

**Interfaces:**
- Consumes: `crate::workflows::resolve_flow::{LookupKey, ResolveFlow, ResolveOutcome}`, `crate::config::label::LabelSeg`.
- Produces on `UiState`:
  - `pub lookup_cache: HashMap<LookupKey, Option<String>>` (`Some(name)` = resolved, `None` = resolved-but-not-found).
  - `pub resolve_flow: ResolveFlow`.
  - `pub fn resolve_lookup(&mut self, key: LookupKey, base: &str, oc: &str, store_attr: &str, value: &str, attrs: &[String], template: &[LabelSeg])`.
  - `pub fn apply_resolve_outcome(&mut self, out: ResolveOutcome)`.

- [ ] **Step 1: Write the failing state tests**

In `src/ui/state.rs` tests module, add:

```rust
#[test]
fn resolve_lookup_dedups_by_cache_and_pending() {
    use crate::config::label::parse_label_template;
    use crate::workflows::resolve_flow::LookupKey;
    let mut st = super::UiState::new_for_test(
        crate::workflows::structure::Structure::build("dc=x", vec![]),
        crate::schema::model::SchemaModel::from_raw(&crate::ldap::worker::RawSubschema::default()),
        "dc=x".into(),
        Vec::new(),
        Vec::new(),
    );
    let key = LookupKey { scope_id: "ou=groups,dc=x|posixGroup|gidNumber".into(), value: "5000".into() };
    let tmpl = parse_label_template("{cn}");
    let attrs = vec!["cn".to_string()];
    // No worker → request() is a no-op, but the pending/cache guards are pure.
    // First: cache already has it → is_pending stays false, no attempt to submit.
    st.lookup_cache.insert(key.clone(), Some("staff".into()));
    st.resolve_lookup(key.clone(), "ou=groups,dc=x", "posixGroup", "gidNumber", "5000", &attrs, &tmpl);
    assert!(!st.resolve_flow.is_pending(&key), "cached key must not be resubmitted");
}

#[test]
fn apply_resolve_outcome_fills_cache_and_flags_render() {
    use crate::workflows::resolve_flow::{LookupKey, ResolveOutcome};
    let mut st = super::UiState::new_for_test(
        crate::workflows::structure::Structure::build("dc=x", vec![]),
        crate::schema::model::SchemaModel::from_raw(&crate::ldap::worker::RawSubschema::default()),
        "dc=x".into(),
        Vec::new(),
        Vec::new(),
    );
    let key = LookupKey { scope_id: "s".into(), value: "5000".into() };
    st.form_needs_render = false;
    st.apply_resolve_outcome(ResolveOutcome::Resolved { key: key.clone(), name: "staff".into() });
    assert_eq!(st.lookup_cache.get(&key), Some(&Some("staff".to_string())));
    assert!(st.form_needs_render);

    let key2 = LookupKey { scope_id: "s".into(), value: "9999".into() };
    st.apply_resolve_outcome(ResolveOutcome::NotFound { key: key2.clone() });
    assert_eq!(st.lookup_cache.get(&key2), Some(&None));
}

#[test]
fn process_responses_routes_resolve_entries_into_cache() {
    use crate::config::label::parse_label_template;
    use crate::workflows::resolve_flow::LookupKey;
    let mut st = super::UiState::new_for_test(
        crate::workflows::structure::Structure::build("dc=x", vec![]),
        crate::schema::model::SchemaModel::from_raw(&crate::ldap::worker::RawSubschema::default()),
        "dc=x".into(),
        Vec::new(),
        Vec::new(),
    );
    // Register an in-flight resolve for id 4_000_000, then feed its response.
    let key = LookupKey { scope_id: "dc=x|posixGroup|gidNumber".into(), value: "5000".into() };
    st.resolve_flow.force_pending(4_000_000, key.clone(), parse_label_template("{cn}"));
    let mut attrs = std::collections::BTreeMap::new();
    attrs.insert("cn".to_string(), vec!["staff".to_string()]);
    let resp = crate::ldap::worker::Response::Entries {
        id: 4_000_000,
        entries: vec![crate::ldap::worker::LdapEntry { dn: "cn=staff,dc=x".into(), attrs, bin_attrs: Default::default() }],
        truncated: false,
    };
    st.pump_responses_for_test(&[resp]);
    assert_eq!(st.lookup_cache.get(&key), Some(&Some("staff".to_string())));
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -j4 --lib ui::state::tests::resolve_lookup_dedups_by_cache_and_pending`
Expected: FAIL to compile — `lookup_cache` / `resolve_flow` / methods missing.

- [ ] **Step 3: Add the fields**

In `src/ui/state.rs`: add the import near the other workflow imports (top of file, next to `use crate::workflows::alloc_flow::...`):

```rust
use crate::workflows::resolve_flow::{LookupKey, ResolveFlow, ResolveOutcome};
```

Add fields to `struct UiState` (near `search_results` / `alloc_flow`, ~line 58-65):

```rust
    /// Async reverse name-resolution for `lookup` widgets.
    pub resolve_flow: ResolveFlow,
    /// Resolved friendly names for `lookup` fields, keyed by scope+value.
    /// `Some(name)` = resolved; `None` = resolved but no candidate matched.
    pub lookup_cache: std::collections::HashMap<LookupKey, Option<String>>,
```

Initialize them in EVERY `UiState` constructor. There is the real constructor (`bootstrap`/`new`) and `new_for_test`. Search the file for existing initializers (`alloc_flow: AllocFlow::new(),` appears in each) and add alongside each:

```rust
            resolve_flow: ResolveFlow::new(),
            lookup_cache: std::collections::HashMap::new(),
```

(Run `grep -n "alloc_flow:" src/ui/state.rs` to find every site — add the two lines at each.)

- [ ] **Step 4: Add `resolve_lookup` and `apply_resolve_outcome`**

In `src/ui/state.rs`, next to `submit_search` (~line 314) and `apply_search_results`:

```rust
    /// Kick off a reverse name-resolution for a `lookup` field's value, unless it
    /// is already cached or in flight. No-op without a live worker.
    ///
    /// Borrow-safe: a single atomic `&mut self`. Call as
    /// `shared.borrow_mut().resolve_lookup(...)` — never while holding another borrow.
    #[allow(clippy::too_many_arguments)]
    pub fn resolve_lookup(
        &mut self,
        key: LookupKey,
        base: &str,
        oc: &str,
        store_attr: &str,
        value: &str,
        attrs: &[String],
        template: &[crate::config::label::LabelSeg],
    ) {
        if self.lookup_cache.contains_key(&key) || self.resolve_flow.is_pending(&key) {
            return;
        }
        if let Some(w) = self.worker.as_ref() {
            let _ = self
                .resolve_flow
                .request(w, base, oc, store_attr, value, attrs, template.to_vec());
        }
    }

    /// Apply one non-ignored resolve outcome: cache the name (or `None` when not
    /// found) and flag a re-render so the form repaints `<value> (<name>)`.
    pub fn apply_resolve_outcome(&mut self, out: ResolveOutcome) {
        match out {
            ResolveOutcome::Resolved { key, name } => {
                self.lookup_cache.insert(key, Some(name));
                self.form_needs_render = true;
            }
            ResolveOutcome::NotFound { key } => {
                self.lookup_cache.insert(key, None);
                self.form_needs_render = true;
            }
            ResolveOutcome::Ignored => {}
        }
    }
```

- [ ] **Step 5: Wire into `process_responses`**

In `process_responses` (~line 238), after the candidate-search branch and before the writes branch, add:

```rust
            // Reverse name-resolution: Entries/SearchError with resolve-range ids (4_000_000+).
            let r_out = self.resolve_flow.on_response(resp);
            if !matches!(r_out, ResolveOutcome::Ignored) {
                self.apply_resolve_outcome(r_out);
                out.changed = true;
                continue;
            }
```

- [ ] **Step 6: Run the state tests**

Run: `cargo test -j4 --lib ui::state`
Expected: PASS (existing + three new).

- [ ] **Step 7: fmt + clippy + commit**

```bash
cargo fmt
cargo clippy -j4 --all-targets -- -D warnings
git add src/ui/state.rs
git commit -F - <<'EOF'
feat(ui): UiState resolution cache + ResolveFlow pump wiring

lookup_cache stores resolved friendly names keyed by scope+value;
resolve_lookup() submits (deduped by cache/pending) and process_responses
routes ResolveFlow results into the cache, flagging a re-render.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

## Task 4: Lookup popup — pure input model

**Files:**
- Create: `src/ui/lookup.rs` (module doc + `lookup_model` fns + tests)
- Modify: `src/ui/mod.rs` (add `pub(crate) mod lookup;`)

**Interfaces:**
- Produces (all pure, in `src/ui/lookup.rs`):
  - `fn leading_number(input: &str) -> Option<String>` — the pending value (leading ASCII digit run, else `None`).
  - `fn ok_enabled(input: &str) -> bool` — `leading_number(input).is_some()`.
  - `fn row_matches(label: &str, value: &str, filter: &str) -> bool` — list filter predicate.
  - `fn row_display(value: &str, label: &str) -> String` — `"{label} ({value})"` (list row).
  - `fn input_after_pick(value: &str, label: &str) -> String` — `"{value} ({label})"` (input text).
  - `fn highlight_index(rows: &[(String /*value*/, String /*label*/)], input: &str) -> Option<usize>`.
- Consumes: nothing (std only).

- [ ] **Step 1: Write the failing model tests**

Create `src/ui/lookup.rs`:

```rust
//! The `lookup` widget: a scalar field shown as `<value> (<name>)` and edited via
//! an editable-combobox popup. This module holds the pure input model (parse /
//! validity / filter / display) plus, in a later task, the FieldWidget/editor/
//! dialog. The value in the input is authoritative: its leading integer is the
//! committed value; picking a candidate writes `<value> (<name>)` back into it.

/// The pending value = the leading run of ASCII digits in `input`, if any.
/// `"5000"` → `Some("5000")`; `"5000 (staff)"` → `Some("5000")`; `"staff"` → `None`;
/// `""` → `None`.
pub(crate) fn leading_number(input: &str) -> Option<String> {
    let digits: String = input.trim_start().chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() { None } else { Some(digits) }
}

/// OK is enabled iff the input yields a committable value.
pub(crate) fn ok_enabled(input: &str) -> bool {
    leading_number(input).is_some()
}

/// List-filter predicate: empty filter matches all; otherwise the candidate
/// matches when its label contains `filter` (case-insensitive) OR its value
/// starts with `filter` (numeric-prefix search when the user types digits).
pub(crate) fn row_matches(label: &str, value: &str, filter: &str) -> bool {
    let f = filter.trim();
    if f.is_empty() {
        return true;
    }
    label.to_ascii_lowercase().contains(&f.to_ascii_lowercase()) || value.starts_with(f)
}

/// A list row renders as `"{label} ({value})"`, e.g. `"staff (5000)"`.
pub(crate) fn row_display(value: &str, label: &str) -> String {
    format!("{label} ({value})")
}

/// Picking a row fills the input with `"{value} ({label})"`, e.g. `"5000 (staff)"`.
pub(crate) fn input_after_pick(value: &str, label: &str) -> String {
    format!("{value} ({label})")
}

/// The index of the row whose value exactly equals the input's leading number,
/// so a typed number highlights its matching group.
pub(crate) fn highlight_index(rows: &[(String, String)], input: &str) -> Option<usize> {
    let n = leading_number(input)?;
    rows.iter().position(|(value, _label)| *value == n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leading_number_extracts_prefix_digits() {
        assert_eq!(leading_number("5000"), Some("5000".into()));
        assert_eq!(leading_number("5000 (staff)"), Some("5000".into()));
        assert_eq!(leading_number("staff"), None);
        assert_eq!(leading_number(""), None);
        assert_eq!(leading_number("  42x"), Some("42".into()));
    }

    #[test]
    fn ok_enabled_requires_leading_number() {
        assert!(ok_enabled("5000"));
        assert!(ok_enabled("5000 (staff)"));
        assert!(!ok_enabled("staff"));
        assert!(!ok_enabled(""));
    }

    #[test]
    fn row_matches_by_label_substring_and_value_prefix() {
        assert!(row_matches("staff", "5000", "")); // empty → all
        assert!(row_matches("staff", "5000", "sta")); // label substring, ci
        assert!(row_matches("Staff", "5000", "aff"));
        assert!(row_matches("staff", "5000", "50")); // numeric prefix on value
        assert!(!row_matches("staff", "5000", "99"));
        assert!(!row_matches("users", "100", "xyz"));
    }

    #[test]
    fn display_helpers_use_opposite_orders() {
        assert_eq!(row_display("5000", "staff"), "staff (5000)");
        assert_eq!(input_after_pick("5000", "staff"), "5000 (staff)");
    }

    #[test]
    fn highlight_matches_exact_value() {
        let rows = vec![("100".to_string(), "users".to_string()), ("5000".to_string(), "staff".to_string())];
        assert_eq!(highlight_index(&rows, "5000"), Some(1));
        assert_eq!(highlight_index(&rows, "5000 (staff)"), Some(1));
        assert_eq!(highlight_index(&rows, "50"), None); // prefix, not exact
        assert_eq!(highlight_index(&rows, "staff"), None); // no leading number
    }
}
```

- [ ] **Step 2: Register the module**

Add to `src/ui/mod.rs` (after `mod help_ctx;` group, keep near `picker`):

```rust
pub(crate) mod lookup;
```

- [ ] **Step 3: Run the model tests**

Run: `cargo test -j4 --lib ui::lookup::tests`
Expected: PASS (five tests). (They compile immediately since the fns are defined; if TDD-purists want a red first, comment out a fn body to see the failure, then restore.)

- [ ] **Step 4: fmt + clippy + commit**

The fns are `pub(crate)` and not yet used outside tests; clippy would flag dead code. Guard the not-yet-used fns is unnecessary — they ARE used by the Task-4 tests, so clippy is satisfied. Verify:

```bash
cargo fmt
cargo clippy -j4 --all-targets -- -D warnings
git add src/ui/lookup.rs src/ui/mod.rs
git commit -F - <<'EOF'
feat(ui): lookup popup pure input model

Parse/validity/filter/display helpers for the lookup combobox: the input's
leading integer is the committed value; list rows show `name (value)` while
the input shows `value (name)`.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

## Task 5: Lookup popup — widget, editor, dialog + routing

**Files:**
- Modify: `src/ui/lookup.rs` (add `LookupWidget`, `LookupEditor`, `LookupDialog`)
- Modify: `src/ui/widget.rs` (`widget_for` + `is_modal_field` add `Lookup`)
- Test: `src/ui/lookup.rs` tests + `src/ui/widget.rs` tests

**Interfaces:**
- Consumes: `crate::config::widget::WidgetKind::Lookup`, `crate::config::relation::LookupBinding`, `crate::ui::widget::{Activation, Capability, CommitOutcome, FieldEditor, FieldWidget}`, `crate::ui::{Shared, REFRESH}`, `crate::workflows::pick_state::Candidate`, the Task-4 model fns.
- Produces: `LookupWidget` (`FieldWidget`), `LookupEditor` (`FieldEditor`), `LookupDialog` (`View`). Commit is `CommitOutcome::SetValues(vec![leading_number])`.

Pattern reference: `src/ui/picker.rs` (`PickerWidget`/`PickerEditor`/`PickerDialog`) — same three-type shape, same `submit_search`/`search_results`/`REFRESH` plumbing, same `staged_commit` commit path. Differences: buttons sit on the input row; candidates filter locally; OK grays via command enable/disable.

- [ ] **Step 1: Write the failing routing tests**

In `src/ui/widget.rs` tests module, add:

```rust
#[test]
fn lookup_field_routes_to_lookup_widget_and_is_modal() {
    use crate::config::relation::{CandidateScope, LookupBinding};
    use crate::config::widget::WidgetKind;
    let mut f = field(&["5000"], WidgetSpec::ReadOnlyText);
    f.label = "gidNumber".into();
    f.widget_binding = Some(WidgetKind::Lookup(LookupBinding {
        attr: "gidNumber".into(),
        scope: CandidateScope {
            base: "ou=groups,dc=x".into(),
            object_classes: vec!["posixGroup".into()],
            search_attrs: vec!["cn".into()],
            label_template: None,
        },
        store: "gidNumber".into(),
        label_template: crate::config::label::parse_label_template("{cn}"),
    }));
    assert!(is_modal_field(&f), "lookup fields open a modal");
    // widget_for returns a LookupWidget whose activate() yields a Modal editor.
    assert!(matches!(widget_for(&f).activate(&f), Activation::Modal(_)));
}
```

(`field(...)` is the existing test helper in that module; `Activation` is imported there.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -j4 --lib ui::widget::tests::lookup_field_routes_to_lookup_widget_and_is_modal`
Expected: FAIL — `Lookup` not handled; `LookupWidget` missing.

- [ ] **Step 3: Add the `LookupWidget`/`LookupEditor`/`LookupDialog` implementation**

Append to `src/ui/lookup.rs` (below the model fns, above the `tests` module):

```rust
use tvision_rs::{
    self as tv, delegate, ButtonFlags, Command, Context, Dialog, Event, FieldValue, InputLine, Key,
    ListBox, Rect, View,
};

use crate::config::relation::LookupBinding;
use crate::config::widget::WidgetKind;
use crate::schema::SchemaModel;
use crate::ui::widget::{Activation, Capability, CommitOutcome, FieldEditor, FieldWidget};
use crate::ui::{Shared, REFRESH};
use crate::workflows::edit_form::EditField;
use crate::workflows::pick_state::Candidate;

/// FieldWidget plugin for `WidgetKind::Lookup`. `present` returns the bare stored
/// value (the form pane enriches it to `<value> (<name>)` from the resolution
/// cache); `activate` opens a `LookupDialog`.
pub(crate) struct LookupWidget;

impl FieldWidget for LookupWidget {
    fn capability(&self) -> Capability {
        Capability::NeedsWorkerSearch
    }

    fn present(&self, field: &EditField) -> String {
        field.values.first().cloned().unwrap_or_default()
    }

    fn activate(&self, field: &EditField) -> Activation {
        match &field.widget_binding {
            Some(WidgetKind::Lookup(b)) => Activation::Modal(Box::new(LookupEditor {
                binding: b.clone(),
                current: field.values.first().cloned().unwrap_or_default(),
            })),
            _ => Activation::Inline,
        }
    }
}

/// Carries the binding + current value into the dialog builder.
pub(crate) struct LookupEditor {
    binding: LookupBinding,
    current: String,
}

impl FieldEditor for LookupEditor {
    fn into_view(self: Box<Self>, _schema: &SchemaModel, shared: Shared) -> (Box<dyn View>, tv::ViewId) {
        let LookupEditor { binding, current } = *self;
        let dlg = LookupDialog::new(binding, current, shared);
        let focus = dlg.input_id;
        (Box::new(dlg), focus)
    }
}

/// The interactive combobox: an input + OK/Cancel on row 1, a filtered candidate
/// list below. Candidates load once (empty-term search) and filter locally.
pub(crate) struct LookupDialog {
    dlg: Dialog,
    input_id: tv::ViewId,
    list_id: tv::ViewId,
    shared: Shared,
    binding: LookupBinding,
    /// All loaded candidates (value, label), unfiltered.
    all: Vec<(String, String)>,
    /// Current filtered view (indices into `all`), parallel to the ListBox rows.
    filtered: Vec<usize>,
    last_input: String,
    /// Set true right after a programmatic pick so the input-change detector does
    /// not immediately re-filter from the auto-filled `<value> (<name>)` text.
    suppress_filter: bool,
    seeded: bool,
}

impl LookupDialog {
    fn new(binding: LookupBinding, current: String, shared: Shared) -> Self {
        let title = format!("Select {}", binding.attr);
        let mut dlg = Dialog::new(Rect::new(0, 0, 64, 20), Some(title));
        dlg.state_mut().options.center_x = true;
        dlg.state_mut().options.center_y = true;
        // Row 1: input on the left, OK/Cancel to its right (same row).
        let input = InputLine::with_limit(Rect::new(2, 1, 40, 2), 128);
        let input_id = dlg.insert_child(Box::new(input));
        let ok = tv::Button::new(Rect::new(41, 1, 51, 3), "~O~K", Command::OK, ButtonFlags { default: true, ..ButtonFlags::new() });
        dlg.insert_child(Box::new(ok));
        let cancel = tv::Button::new(Rect::new(51, 1, 62, 3), "~C~ancel", Command::CANCEL, ButtonFlags::new());
        dlg.insert_child(Box::new(cancel));
        // Rows 3..: the list spans the full inner width (input + buttons).
        let list = ListBox::new(Rect::new(2, 3, 62, 18), 1, None, None);
        let list_id = dlg.insert_child(Box::new(list));

        LookupDialog {
            dlg,
            input_id,
            list_id,
            shared,
            binding,
            all: Vec::new(),
            filtered: Vec::new(),
            last_input: current.clone(),
            suppress_filter: false,
            seeded: false,
        }
    }

    /// The label-template attributes the candidate search must fetch.
    fn label_attrs(&self) -> Vec<String> {
        let mut attrs = crate::config::label::template_attrs(&self.binding.label_template);
        if !attrs.iter().any(|a| a.eq_ignore_ascii_case(&self.binding.store)) {
            attrs.push(self.binding.store.clone());
        }
        if !attrs.iter().any(|a| a.eq_ignore_ascii_case("cn")) {
            attrs.push("cn".into());
        }
        attrs
    }

    /// Submit the one-shot candidate load (empty term = all candidates).
    fn submit_load(&self) {
        let attrs = self.label_attrs();
        self.shared.borrow_mut().submit_search(
            &self.binding.scope.base,
            self.binding.object_class(),
            "",
            &attrs,
            Some(&self.binding.store),
        );
    }

    /// Copy the pump-delivered candidates into `all`, rendering labels via the
    /// binding's template, then refilter.
    fn sync_candidates(&mut self, ctx: &mut Context) {
        let results: Vec<Candidate> = self.shared.borrow().search_results.clone();
        self.all = results
            .into_iter()
            .map(|c| (c.store_value, c.label))
            .collect();
        self.apply_filter(ctx);
    }

    /// Rebuild the ListBox from `all` filtered by the current input text.
    fn apply_filter(&mut self, ctx: &mut Context) {
        let filter = self.current_input();
        self.filtered = self
            .all
            .iter()
            .enumerate()
            .filter(|(_, (value, label))| row_matches(label, value, &filter))
            .map(|(i, _)| i)
            .collect();
        let rows: Vec<String> = self
            .filtered
            .iter()
            .map(|&i| {
                let (value, label) = &self.all[i];
                row_display(value, label)
            })
            .collect();
        if let Some(lb) = self
            .dlg
            .child_mut(self.list_id)
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<ListBox>())
        {
            lb.new_list(rows, ctx);
        }
        // Highlight the exact numeric match, if any.
        let rows_ref: Vec<(String, String)> =
            self.filtered.iter().map(|&i| self.all[i].clone()).collect();
        if let Some(idx) = highlight_index(&rows_ref, &filter) {
            if let Some(list) = self.dlg.child_mut(self.list_id) {
                list.set_value_ctx(FieldValue::Int(idx as i32), ctx);
            }
        }
    }

    fn current_input(&mut self) -> String {
        match self.dlg.child_mut(self.input_id).and_then(|v| v.value()) {
            Some(FieldValue::Text(s)) => s,
            _ => String::new(),
        }
    }

    fn set_input(&mut self, text: &str, ctx: &mut Context) {
        if let Some(v) = self.dlg.child_mut(self.input_id) {
            v.set_value_ctx(FieldValue::Text(text.to_string()), ctx);
        }
    }

    /// Reflect validity into the OK command and stage the commit.
    fn sync_ok(&mut self, ctx: &mut Context) {
        let input = self.current_input();
        if let Some(value) = leading_number(&input) {
            ctx.enable_command(Command::OK);
            self.shared.borrow_mut().staged_commit = Some(CommitOutcome::SetValues(vec![value]));
        } else {
            ctx.disable_command(Command::OK);
            self.shared.borrow_mut().staged_commit = None;
        }
    }

    /// Pick the highlighted row: write `<value> (<name>)` into the input.
    fn pick_highlighted(&mut self, ctx: &mut Context) {
        let idx = match self.dlg.child_mut(self.list_id).and_then(|v| v.value()) {
            Some(FieldValue::Int(i)) if i >= 0 => i as usize,
            _ => return,
        };
        let Some(&ai) = self.filtered.get(idx) else { return };
        let (value, label) = self.all[ai].clone();
        let text = input_after_pick(&value, &label);
        self.set_input(&text, ctx);
        self.last_input = text;
        self.suppress_filter = true;
        self.sync_ok(ctx);
    }
}

#[delegate(to = dlg)]
impl View for LookupDialog {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    fn reset_current(&mut self, ctx: &mut Context) {
        self.dlg.reset_current(ctx);
        if !self.seeded {
            self.seeded = true;
            self.sync_candidates(ctx);
            self.submit_load();
            self.sync_ok(ctx);
        }
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        if !self.seeded {
            self.seeded = true;
            self.sync_candidates(ctx);
            self.submit_load();
            self.sync_ok(ctx);
        }

        // Pump-delivered candidate results.
        if matches!(ev, Event::Broadcast { command, .. } if *command == REFRESH) {
            self.sync_candidates(ctx);
            self.sync_ok(ctx);
            self.dlg.handle_event(ev, ctx);
            return;
        }

        // Enter on the list picks the highlighted row; nav keys go to the list.
        let enter_on_list = matches!(ev, Event::KeyDown(k) if k.key == Key::Enter)
            && self.dlg.state().state.focused // dialog focused
            && self.dlg.current() == Some(self.list_id);
        let nav = matches!(ev, Event::KeyDown(k) if matches!(k.key, Key::Up | Key::Down | Key::PageUp | Key::PageDown));

        if enter_on_list {
            self.pick_highlighted(ctx);
            ev.clear();
        } else if nav && self.dlg.current() == Some(self.list_id) {
            if let Some(list) = self.dlg.child_mut(self.list_id) {
                list.handle_event(ev, ctx);
            }
        } else {
            self.dlg.handle_event(ev, ctx);
        }

        // Detect input changes → refilter (unless a pick just set the text) + re-stage.
        let cur = self.current_input();
        if cur != self.last_input {
            self.last_input = cur;
            if self.suppress_filter {
                self.suppress_filter = false;
            } else {
                self.apply_filter(ctx);
            }
            self.sync_ok(ctx);
        }
    }
}
```

(Adjust `tv::Button::new` / `insert_child` signatures to the exact ones `picker.rs` and `oc_picker.rs` use — check those files; if the crate uses `dlg.button_row(...)` only, place OK/Cancel via the same `Button` constructor those modules import. The key requirement: OK/Cancel land on row 1 to the right of the input, and the list spans rows 3.. full width.)

- [ ] **Step 4: Route `Lookup` in `widget.rs`**

In `src/ui/widget.rs` `widget_for` (before the final `else { Box::new(PlainWidget) }`, ~line 145):

```rust
    } else if matches!(field.widget_binding, Some(WidgetKind::Lookup(_))) {
        Box::new(crate::ui::lookup::LookupWidget)
```

In `is_modal_field` (~line 161), add the `Lookup` arm:

```rust
        || matches!(field.widget_binding, Some(WidgetKind::Lookup(_)))
```

- [ ] **Step 5: Run the routing test**

Run: `cargo test -j4 --lib ui::widget::tests::lookup_field_routes_to_lookup_widget_and_is_modal`
Expected: PASS.

- [ ] **Step 6: Write a headless dialog test**

In `src/ui/lookup.rs` tests module, add (model template: `PickerDialog` tests in `picker.rs`, ~line 378+):

```rust
mod dialog {
    use super::super::*;
    use crate::config::relation::{CandidateScope, LookupBinding};
    use crate::ui::widget::CommitOutcome;
    use crate::workflows::pick_state::Candidate;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;
    use tvision_rs::{timer::TimerQueue, Command, Deferred, Event, View};

    fn shared_with_candidates(cands: Vec<(&str, &str)>) -> crate::ui::Shared {
        let mut st = crate::ui::state::UiState::new_for_test(
            crate::workflows::structure::Structure::build("dc=x", vec![]),
            crate::schema::model::SchemaModel::from_raw(&crate::ldap::worker::RawSubschema::default()),
            "dc=x".into(),
            Vec::new(),
            Vec::new(),
        );
        st.search_results = cands
            .into_iter()
            .map(|(v, l)| Candidate { dn: format!("cn={l},dc=x"), label: l.into(), store_value: v.into() })
            .collect();
        Rc::new(RefCell::new(st))
    }

    fn binding() -> LookupBinding {
        LookupBinding {
            attr: "gidNumber".into(),
            scope: CandidateScope { base: "ou=groups,dc=x".into(), object_classes: vec!["posixGroup".into()], search_attrs: vec!["cn".into()], label_template: None },
            store: "gidNumber".into(),
            label_template: crate::config::label::parse_label_template("{cn}"),
        }
    }

    #[test]
    fn seeded_numeric_input_stages_commit_and_enables_ok() {
        let shared = shared_with_candidates(vec![("100", "users"), ("5000", "staff")]);
        let mut dlg = LookupDialog::new(binding(), "5000".into(), shared.clone());
        let mut out = VecDeque::new();
        let mut timers = TimerQueue::new();
        let mut deferred: Vec<Deferred> = Vec::new();
        let mut ctx = tvision_rs::Context::new(&mut out, &mut timers, 0, &mut deferred);
        // First event seeds candidates + stages from the seeded "5000".
        let mut ev = Event::Broadcast { command: crate::ui::REFRESH, source: None };
        dlg.handle_event(&mut ev, &mut ctx);
        assert_eq!(
            shared.borrow().staged_commit,
            Some(CommitOutcome::SetValues(vec!["5000".to_string()])),
            "seeded numeric input stages its value"
        );
    }
}
```

- [ ] **Step 7: Run the dialog test**

Run: `cargo test -j4 --lib ui::lookup`
Expected: PASS. If the `tv::Button`/`insert_child`/`Context::new`/`enable_command` signatures differ from what compiled elsewhere, align them with `picker.rs`/`oc_picker.rs` (same crate version) until green — do NOT change behavior, only the API surface.

- [ ] **Step 8: fmt + clippy + commit**

```bash
cargo fmt
cargo clippy -j4 --all-targets -- -D warnings
git add src/ui/lookup.rs src/ui/widget.rs
git commit -F - <<'EOF'
feat(ui): lookup combobox dialog + widget routing

LookupWidget/LookupEditor/LookupDialog: input on row 1 with OK/Cancel to
its right, filtered candidate list below. Candidates load once and filter
locally; OK grays unless the input has a leading number; picking a row
writes `<value> (<name>)` into the input. widget_for/is_modal_field route
WidgetKind::Lookup.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

## Task 6: Form pane — `<value> (<name>)` display + resolve trigger

**Files:**
- Modify: `src/ui/panes/form.rs` (`render` — lookup display + resolve trigger)
- Test: `src/ui/panes/form.rs` tests module

**Interfaces:**
- Consumes: `crate::config::widget::WidgetKind::Lookup`, `crate::workflows::resolve_flow::LookupKey`, `crate::config::label::template_attrs`, the `UiState::{lookup_cache, resolve_lookup}` from Task 3.
- No new public surface.

Design: a `lookup` field is already `ValueKind::Launch` (via `is_modal_field`, Task 5). Its bullet block is one line. In `render`, before the ScrollGroup child loop, precompute a per-field `Vec<Option<Vec<String>>>` of lookup display lines from the cache, and collect the not-yet-cached values to resolve; after the loop (borrow released) trigger the resolves. In the loop's `Launch` arm, prefer the precomputed lines.

- [ ] **Step 1: Write the failing display tests**

In `src/ui/panes/form.rs` tests module, add a helper + tests. The helper builds a form with a single `gidNumber` lookup field and a pre-seeded cache:

```rust
fn lookup_binding_for_test() -> crate::config::widget::WidgetKind {
    use crate::config::relation::{CandidateScope, LookupBinding};
    crate::config::widget::WidgetKind::Lookup(LookupBinding {
        attr: "gidNumber".into(),
        scope: CandidateScope {
            base: "ou=groups,dc=x".into(),
            object_classes: vec!["posixGroup".into()],
            search_attrs: vec!["cn".into()],
            label_template: None,
        },
        store: "gidNumber".into(),
        label_template: crate::config::label::parse_label_template("{cn}"),
    })
}

/// Build a pane whose single field is a `gidNumber` lookup with value `5000`,
/// and optionally pre-seed the resolution cache. Returns the rendered line for
/// that field's value view.
fn lookup_line_after_render(cache: Option<Option<String>>) -> String {
    use crate::workflows::resolve_flow::LookupKey;
    let mut field = ef("gidNumber", "5000", false);
    field.widget_binding = Some(lookup_binding_for_test());
    let (shared, mut pane) = build_pane_with_form(vec![field]);
    if let Some(entry) = cache {
        let key = LookupKey { scope_id: "ou=groups,dc=x|posixGroup|gidNumber".into(), value: "5000".into() };
        shared.borrow_mut().lookup_cache.insert(key, entry);
    }
    let mut out = VecDeque::new();
    let mut timers = tv::timer::TimerQueue::new();
    let mut deferred = Vec::new();
    let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
    let mut ev = Event::Broadcast { command: REFRESH, source: None };
    pane.handle_event(&mut ev, &mut ctx);
    pane.launch_line_for_test(0)
}

#[test]
fn lookup_resolved_shows_value_and_name() {
    assert_eq!(lookup_line_after_render(Some(Some("staff".into()))), "5000 (staff)");
}

#[test]
fn lookup_unresolved_not_found_shows_bare_value() {
    assert_eq!(lookup_line_after_render(Some(None)), "5000");
}

#[test]
fn lookup_uncached_shows_ellipsis_placeholder() {
    assert_eq!(lookup_line_after_render(None), "5000 (…)");
}
```

This needs a test seam `launch_line_for_test(i)` returning the first line the value view at field `i` shows. Add it near the other `#[cfg(test)]` seams (~line 320):

```rust
    /// Test seam: the first display line of field `i`'s LaunchValueView.
    #[cfg(test)]
    pub(crate) fn launch_line_for_test(&mut self, i: usize) -> String {
        let vid = self.value_ids[i];
        self.scroll_mut()
            .and_then(|sg| sg.child_mut(vid))
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<LaunchValueView>())
            .and_then(|lv| lv.first_line_for_test())
            .unwrap_or_default()
    }
```

If `LaunchValueView` has no `first_line_for_test`, add a trivial one to `src/ui/panes/launch_view.rs` returning `self.lines.first().cloned()` (check the field name holding its lines).

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -j4 --lib ui::panes::form::tests::lookup_resolved_shows_value_and_name`
Expected: FAIL — placeholder/name not rendered (currently shows the bare value from `present_field`).

- [ ] **Step 3: Implement the lookup display + trigger in `render`**

In `src/ui/panes/form.rs` `render`, right after the `fields` clone (~line 565), add the precompute + trigger:

```rust
        // Lookup fields: render `<value> (<name>)` from the resolution cache and
        // kick off resolves for values not yet cached. Collect under one short
        // borrow, then trigger (borrow-free) so we never submit while borrowed.
        use crate::config::widget::WidgetKind;
        use crate::workflows::resolve_flow::LookupKey;
        let mut lookup_lines: Vec<Option<Vec<String>>> = vec![None; fields.len()];
        struct ToResolve {
            key: LookupKey,
            base: String,
            oc: String,
            store: String,
            value: String,
            attrs: Vec<String>,
            template: Vec<crate::config::label::LabelSeg>,
        }
        let mut to_resolve: Vec<ToResolve> = Vec::new();
        {
            let st = self.state.borrow();
            for (i, f) in fields.iter().enumerate() {
                let Some(WidgetKind::Lookup(b)) = &f.widget_binding else { continue };
                let value = f.values.first().map(|s| s.trim().to_string()).unwrap_or_default();
                if value.is_empty() {
                    lookup_lines[i] = Some(vec![NOT_SET.to_string()]);
                    continue;
                }
                let key = LookupKey { scope_id: b.scope_id(), value: value.clone() };
                match st.lookup_cache.get(&key) {
                    Some(Some(name)) => lookup_lines[i] = Some(vec![format!("{value} ({name})")]),
                    Some(None) => lookup_lines[i] = Some(vec![value.clone()]),
                    None => {
                        lookup_lines[i] = Some(vec![format!("{value} (\u{2026})")]); // 5000 (…)
                        to_resolve.push(ToResolve {
                            key,
                            base: b.scope.base.clone(),
                            oc: b.object_class().to_string(),
                            store: b.store.clone(),
                            value,
                            attrs: {
                                let mut a = crate::config::label::template_attrs(&b.label_template);
                                if !a.iter().any(|x| x.eq_ignore_ascii_case(&b.store)) { a.push(b.store.clone()); }
                                if !a.iter().any(|x| x.eq_ignore_ascii_case("cn")) { a.push("cn".into()); }
                                a
                            },
                            template: b.label_template.clone(),
                        });
                    }
                }
            }
        } // state borrow dropped
        for r in to_resolve {
            self.state.borrow_mut().resolve_lookup(
                r.key, &r.base, &r.oc, &r.store, &r.value, &r.attrs, &r.template,
            );
        }
```

Then in the ScrollGroup child loop's `ValueKind::Launch` arm (~line 613), prefer the precomputed lines:

```rust
                            ValueKind::Launch => {
                                if let Some(lv) = v
                                    .as_any_mut()
                                    .and_then(|a| a.downcast_mut::<LaunchValueView>())
                                {
                                    let lines = lookup_lines
                                        .get(i)
                                        .and_then(|o| o.clone())
                                        .unwrap_or_else(|| launch_lines(field));
                                    lv.set_lines(lines);
                                }
                            }
```

- [ ] **Step 4: Run the display tests**

Run: `cargo test -j4 --lib ui::panes::form::tests`
Expected: PASS (the three lookup tests + existing form tests).

- [ ] **Step 5: Verify the resolve trigger fires (no worker → no panic, cache untouched)**

The `lookup_uncached_shows_ellipsis_placeholder` test already exercises the trigger path with a worker-less `Shared` (resolve_lookup is a no-op without a worker but must not panic). Confirm it is green from Step 4.

- [ ] **Step 6: fmt + clippy + full test run + commit**

```bash
cargo fmt
cargo clippy -j4 --all-targets -- -D warnings
cargo test -j4
git add src/ui/panes/form.rs src/ui/panes/launch_view.rs
git commit -F - <<'EOF'
feat(ui): render lookup fields as `<value> (<name>)` in the form

The form pane resolves a lookup field's value to its friendly name from
the UiState cache, showing `5000 (…)` while a background resolve is in
flight, `5000 (staff)` once resolved, and `5000` when no candidate matches.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

## Task 7: Docs, changelog, examples

**Files:**
- Modify: `docs/src/configuration/widgets.md` (new `## The lookup kind` section; update the kinds table; switch the `gidNumber` example)
- Modify: `CHANGES.md` (unreleased entry)
- Modify: `examples/config.toml`, `examples/demo-config.toml` (point `gidNumber` at `lookup`)
- Verify: `README.md` (no contradiction; widget-kind list mentions `lookup`)

- [ ] **Step 1: Add the `lookup` kind to the widgets table**

In `docs/src/configuration/widgets.md`, add a row to the kinds table (after the `membership` row, ~line 16):

```markdown
| [`lookup`](#the-lookup-kind) | an editable-combobox popup: type a number or filter a candidate list and pick one | scalar values shown with a friendly name (`gidNumber` → group name) |
```

- [ ] **Step 2: Write the `## The lookup kind` section**

Add after the `## The picker kind` section (before `## The membership kind`, ~line 256):

````markdown
## The `lookup` kind

The `lookup` kind turns a **scalar attribute into a value shown with its friendly
name**. In the form the field renders as `<value> (<name>)` — e.g. `5000 (staff)`
for a `gidNumber` — by resolving the value against a candidate profile. Pressing
Enter opens an **editable combobox**: type a number freely, or filter a list of
candidates by name and pick one.

```toml
[profile.widget.gidNumber]
kind      = "lookup"
candidate = "posixgroup"
store     = "gidNumber"
label     = "{cn}"
```

### Options

- **`kind`** *(required)* — must be `"lookup"`.
- **`candidate`** *(required)* — the source of candidates. Same as
  [`picker`](#the-picker-kind): a `[[profile]]` name string, or an inline scope
  table.
- **`store`** *(required)* — the scalar attribute stored in this entry's field. It
  is **also the match key**: the friendly name is resolved by searching the
  candidate for an entry whose `store` attribute equals the current value.
- **`label`** *(optional, default `{cn}`)* — a label template rendered against the
  resolved candidate to produce the friendly name (e.g. `{cn}`, `{cn} ({description})`).
  Defaults to the candidate profile's own `label`, else `{cn}`.

### The edit popup

The popup is an **editable combobox**:

- The **input** is authoritative. Its leading integer is the value that will be
  stored — you can type any number, even one with no matching candidate.
- Typing filters the **list** below (by name, or by numeric prefix). Rows show
  `<name> (<value>)`, e.g. `staff (5000)`.
- Picking a row (Enter/click) fills the input with `<value> (<name>)`.
- **OK** is enabled only when the input has a leading number; it stores that
  number.

The always-visible form shows `<value> (<name>)` — a brief `<value> (…)` while the
name resolves in the background, and just `<value>` if no candidate matches.

### `lookup` vs `picker`

Both can store a group's `gidNumber`. Use `picker` when you only ever pick an
existing group and want a search-over-radio-list. Use `lookup` when you also want
to **type an arbitrary number** and to **see the group name in the form** without
opening the editor.
````

- [ ] **Step 3: Switch the `gidNumber` worked example**

Find the `#### gidNumber — single-select, stores a scalar` worked example under the picker section (~line 213) and add a one-line pointer under it:

```markdown
> For `gidNumber` you will usually prefer the [`lookup` kind](#the-lookup-kind),
> which additionally shows the group name in the form and lets you type a bare
> number. The `picker` form above remains valid for pick-only behavior.
```

- [ ] **Step 4: Add the CHANGES.md entry**

In `CHANGES.md`, under the current unreleased section, add:

```markdown
- **New `lookup` widget kind.** A scalar attribute (e.g. `gidNumber`) is shown in
  the form as `<value> (<name>)` and edited via an editable-combobox popup: type a
  number freely or filter a candidate list and pick one. See
  [Widgets → The lookup kind](https://oposs.github.io/edaptor/configuration/widgets.html#the-lookup-kind).
```

- [ ] **Step 5: Point the example configs at `lookup`**

In `examples/config.toml` and `examples/demo-config.toml`, find the `[profile.widget.gidNumber]` block (currently `kind = "picker"`, `store = "gidNumber"`) and replace it with:

```toml
[profile.widget.gidNumber]
kind      = "lookup"
candidate = "posixgroup"
store     = "gidNumber"
label     = "{cn}"
```

Match the candidate profile name used in each file (`posixgroup` or whatever that file names the group profile — check the `[[profile]]` names in the same file and keep it consistent).

- [ ] **Step 6: Verify README + build docs**

```bash
grep -n "picker\|lookup\|widget" README.md   # ensure no statement contradicts the new kind
make docs                                     # mdBook build must succeed
```

If `README.md` enumerates widget kinds, add `lookup` to that list (orientation only — no reference detail).

- [ ] **Step 7: Manual smoke check against the demo server**

```bash
scripts/test-ldap.sh start
export EDAPTOR_TEST_ADMIN_PW=adminpassword
cargo run -j4 -- --config examples/demo-config.toml
```

Navigate to a user with `gidNumber`, confirm the form shows `<number> (<group>)`, open the field (Enter), type a number and a name-filter, pick a row, OK, and confirm the value updates. (Use Discard, not Save, against demo data.)

- [ ] **Step 8: Commit**

```bash
git add docs/src/configuration/widgets.md CHANGES.md examples/config.toml examples/demo-config.toml README.md
git commit -F - <<'EOF'
docs: document the lookup widget kind; point gidNumber examples at it

New `## The lookup kind` section, kinds-table row, CHANGES entry, and the
example configs now use `kind = "lookup"` for gidNumber.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

---

## Final verification

- [ ] `make check` (fmt + clippy -D warnings + tests) is green.
- [ ] Facade guards print nothing:
  ```bash
  ! grep -rl "use tvision_rs" src | grep -vE "^src/ui/"
  ! grep -rl "use ratatui\|use tui_" src
  ```
- [ ] `cargo test -j4` green end-to-end.
- [ ] The manual smoke check (Task 7 Step 7) behaved as specified.

## Self-review notes (addressed)

- **Spec coverage:** config surface → T1; live form resolution → T2/T3/T6; value-in-input popup → T4/T5; docs/examples → T7. Every spec section maps to a task.
- **Type consistency:** `LookupKey.scope_id` is built identically in `LookupBinding::scope_id()` (T1), `ResolveFlow::request` (T2), and `form.rs` render (T6) — `base|objectClass|store`. `CommitOutcome::SetValues(vec![leading_number])` is the single commit shape (T5). `lookup_cache: HashMap<LookupKey, Option<String>>` semantics (`Some`=name, `None`=not found) are consistent across T3/T6.
- **Open item from the spec — the `resolved_label` seam on `EditField`:** dropped. Carrying the resolved name in the `UiState` cache (read by the form pane) avoids editing all `EditField` constructors and keeps the resolved name out of the dirty/baseline comparison. `present()` stays pure; the form pane owns the `<value> (<name>)` formatting.
- **Dropped spec item — reject `lookup` on a multi-valued attribute at config-load:** not feasible without schema at config time; the widget operates on the first value. Not a task.
````
