# Picker & Membership Widgets Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fold the `[profile.picker.<attr>]` system into the `[profile.widget.<attr>]` palette as two new kinds — `kind = "picker"` (store picked value(s) in this entry) and `kind = "membership"` (fan this entry's DN into a back-ref attr on each picked candidate) — and delete the old parallel picker config namespace.

**Architecture:** Both new kinds resolve into the **existing, unchanged** `PickerBinding`/`CandidateScope` engine; only the config front-end and the field-tagging/read path move. We add the config model + resolution first (additive, green), then rewire the form/save read sites onto `EditField.widget_binding`'s new `WidgetKind::Picker` arm while the old picker config layer sits **dormant** (green), then **delete** the dormant layer (`PickerSpec`, `resolve_pickers`, `picker_for`, `tag_picker_fields`, `ResolvedPicker`, `EntryProfile.pickers`, `App.pickers`, `EditField.picker`) (green). The live candidate-search engine (`service_picker_search`, the `Response::Entries` intercept, `ValueEditor`, `PickerState`, `membership_fanout`, combined-save) is **behavior-preserving** throughout.

**Tech Stack:** Rust, serde/toml, ratatui TUI. Spec: `docs/superpowers/specs/2026-06-08-picker-widget-design.md`.

**Conventions (from `docs/HANDOVER.md`):** Cap parallelism at 4 cores (`-j4`). Cargo target dir is `/home/oetiker/scratch/cargo-target`. Strict TDD, atomic commits, crate compiles after every commit, `cargo fmt` before every commit, clippy clean `--all-targets`. Commit trailer: `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. Work on a branch (e.g. `feat-picker-widget`). No back-compat — clean removal, no aliases.

**Per-commit gate (run before every commit):**
```bash
cargo fmt
cargo build -j4 --all-targets
cargo test -j4 -p edaptor          # live_* SKIP without EDAPTOR_TEST_LDAP_URI
cargo clippy -j4 --all-targets -- -D warnings
```

---

## File map

| File | Responsibility | Tasks |
|---|---|---|
| `src/config/mod.rs` | serde model: add `CandidateRef`, `InlineScope`, `WidgetSpecCfg::{Picker,Membership}`; later delete `PickerSpec`, `EntryProfile.pickers` | T1, T4 |
| `src/config/widget.rs` | resolution: add `WidgetKind::Picker(PickerBinding)`, `resolve_candidate`, Picker/Membership arms in `resolve_widgets` | T2 |
| `src/config/relation.rs` | make `scope_of` reachable; later delete `resolve_pickers`/`picker_for`/`ResolvedPicker` + their tests (keep the engine types) | T2, T4 |
| `src/ui/edit_form.rs` | `tag_widget_fields` Picker arm; repoint `fanout_labels`/`to_edit_entry`/`order_fields`; later delete `EditField.picker` + `tag_picker_fields` | T2, T3, T4 |
| `src/ui/app/value_editor.rs` | `open_value_editor` Picker arm; later drop `field.picker` reads in tests | T3, T4 |
| `src/ui/app/save.rs` | fan-out save reads `widget_binding` Picker arm instead of `f.picker` | T3 |
| `src/ui/app/mod.rs`, `src/ui/app/action.rs`, `src/ui/app/create.rs` | construction wiring: drop `resolve_pickers`/`tag_picker_fields`/`App.pickers` | T4 |
| `examples/config.toml`, `examples/demo-config.toml` | migrate `[profile.picker.*]` → `[profile.widget.*]` | T3 |
| `tests/live_templates.rs` | migrate 4 `resolve_pickers` sites → `resolve_widgets` | T4 |
| `docs/src/configuration/widgets.md` + siblings | fold `pickers.md` into `widgets.md` | T5 |
| `CHANGES.md` | Unreleased changelog | T6 |

## Test ownership (each test lands on exactly one task — wrong side = a non-green commit)

| Test | File:line | Task | Why |
|---|---|---|---|
| `fanout_labels_come_from_picker_binding` | `edit_form.rs:532` | **T3** | production read path (`fanout_labels`) repoints; helper must set `widget_binding` |
| `open_value_editor_opens_picker_for_single_value_lookup_field` | `value_editor.rs:661` | **T3** | production read path (`open_value_editor`) repoints |
| `open_value_editor_arms_initial_search`, `lookup_alt_s_commits_scalar_to_field_editor`, other users of the two T3 helpers | `value_editor.rs` | **T3** | share the repointed helpers |
| `demo_config_parses_with_pickers` | `mod.rs:478` | **T3** | examples migrate in T3 |
| `demo_config_widgets_resolve` | `mod.rs:748` | **T3** | extend to assert memberOf→membership, gidNumber→picker |
| `tag_picker_fields_tags_by_binding_and_forces_fanout_editable` | `edit_form.rs:839` | **T4** | tests `tag_picker_fields`, deleted in T4 |
| `resolves_picker_dn_store_defaults`, `resolves_picker_scalar_store_and_select`, `resolves_picker_fanout`, `picker_for_matches_owner_oc_and_attr`, `resolve_pickers_store_and_select_are_case_insensitive` | `relation.rs:223–340` | **T4** | test `resolve_pickers`/`picker_for`, deleted in T4 (coverage migrates to `widget.rs`) |
| `unknown_picker_candidate_is_dropped` | `relation.rs:292` | **T4** | **inverts**: new resolver *errors* on unknown candidate — rewrite to assert the error in `widget.rs` |
| `parses_profile_picker_block` | `mod.rs:830` | **T4** | tests `PickerSpec` parsing, deleted in T4 |
| live `picker_*` tests (4× `resolve_pickers`) | `tests/live_templates.rs` | **T4** | switch construction to `resolve_widgets` |

---

## Task 1: Config model — `CandidateRef`, `InlineScope`, and the `Picker`/`Membership` widget variants

**Files:**
- Modify: `src/config/mod.rs` (add types near `WidgetSpecCfg` at `:116`; reuse existing `default_store`/`default_select` at `:77`/`:81`)
- Test: `src/config/mod.rs` `#[cfg(test)] mod tests`

The one untried serde path is an **untagged** `CandidateRef` nested inside the **internally-tagged** `WidgetSpecCfg`. It should work (the outer `kind` tag is proven by `Choice`/`Password`; the inner untagged enum deserializes from buffered content via `deserialize_any`), but we make the **inline-table** case the first failing test so any surprise lands here, before the rewiring.

- [ ] **Step 1: Write the failing test (inline-table candidate first)**

Add to `src/config/mod.rs` test module:

```rust
#[test]
fn widget_picker_parses_inline_candidate_scope() {
    // The risky path: an untagged CandidateRef (inline table) nested in the
    // internally-tagged WidgetSpecCfg. Must parse to Inline, not error.
    let toml = r#"
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
        WidgetSpecCfg::Picker { candidate, store, select } => {
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
        WidgetSpecCfg::Picker { candidate, store, select } => {
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
fn widget_picker_store_and_select_default() {
    let toml = r#"
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
            assert_eq!(store, "dn");    // default_store
            assert_eq!(select, "auto"); // default_select
        }
        other => panic!("expected Picker, got {other:?}"),
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -j4 -p edaptor --lib widget_picker 2>&1 | tail -20`
Expected: compile error (`CandidateRef`, `WidgetSpecCfg::Picker`, `WidgetSpecCfg::Membership` not found).

- [ ] **Step 3: Add `CandidateRef` and `InlineScope`**

In `src/config/mod.rs`, immediately above `pub enum WidgetSpecCfg` (`:116`):

```rust
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
```

- [ ] **Step 4: Add the `Picker` and `Membership` variants to `WidgetSpecCfg`**

In `src/config/mod.rs`, inside `pub enum WidgetSpecCfg` (`:116-135`), after the `Password { … }` variant, add:

```rust
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
```

Note: `default_store` (`"dn"`) and `default_select` (`"auto"`) already exist at `src/config/mod.rs:77`/`:81` — reuse them. Do **not** add `#[serde(deny_unknown_fields)]` (it conflicts with internally-tagged enums; a stray `select` on `membership` is silently ignored by design).

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -j4 -p edaptor --lib widget_picker 2>&1 | tail -20`
Expected: all three PASS.

- [ ] **Step 6: Full gate + commit**

```bash
cargo fmt && cargo build -j4 --all-targets && cargo test -j4 -p edaptor && cargo clippy -j4 --all-targets -- -D warnings
git add src/config/mod.rs
git commit -m "feat(config): add CandidateRef + picker/membership widget variants

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 2: Resolution — `WidgetKind::Picker` and `resolve_widgets` arms

**Files:**
- Modify: `src/config/relation.rs` (make `scope_of` reachable: `:110`)
- Modify: `src/config/widget.rs` (add `WidgetKind::Picker`, `resolve_candidate`, two `resolve_widgets` arms; `:38`, `:54`)
- Modify: `src/ui/edit_form.rs` (`tag_widget_fields` match at `:449` — add a **stub** `Picker` arm to keep the exhaustive match compiling; full handling lands in T3)
- Test: `src/config/widget.rs` `#[cfg(test)] mod tests`

`resolve_widgets` already takes `&[EntryProfile]` and returns `Result<_, String>`, so candidate lookup + erroring on an unknown profile fit naturally. Parsing logic is copied **verbatim** from `resolve_pickers` (`relation.rs:61-92`) so behavior is identical. `select = "auto"` must stay `None` (arity derivation remains downstream in `ValueEditor::open`, untouched — `resolve_widgets` has no schema).

- [ ] **Step 1: Write the failing tests (resolution parity + inline + unknown-error)**

Add to `src/config/widget.rs` test module:

```rust
#[test]
fn resolve_widget_picker_value_lookup() {
    use crate::config::relation::{Cardinality, StoreKey};
    let profiles = vec![
        EntryProfile { name: "user".into(), object_classes: vec!["inetOrgPerson".into()],
            widgets: [("gidNumber".to_string(), WidgetSpecCfg::Picker {
                candidate: CandidateRef::Profile("posixgroup".into()),
                store: "gidNumber".into(), select: "single".into() })].into_iter().collect(),
            ..Default::default() },
        EntryProfile { name: "posixgroup".into(), object_classes: vec!["posixGroup".into()],
            search_base: "ou=groups,dc=x".into(), ..Default::default() },
    ];
    let out = resolve_widgets(&profiles).expect("resolves");
    let rw = out.iter().find(|w| w.attr == "gidNumber").expect("gidNumber widget");
    match &rw.kind {
        WidgetKind::Picker(b) => {
            assert_eq!(b.store, StoreKey::Attr("gidNumber".into()));
            assert_eq!(b.select, Some(Cardinality::Single));
            assert_eq!(b.fanout_attr, None);
            assert_eq!(b.scope.base, "ou=groups,dc=x");
            assert_eq!(b.scope.object_classes, vec!["posixGroup".to_string()]);
        }
        other => panic!("expected Picker, got {other:?}"),
    }
}

#[test]
fn resolve_widget_membership_fans_out() {
    use crate::config::relation::{Cardinality, StoreKey};
    let profiles = vec![
        EntryProfile { name: "user".into(), object_classes: vec!["inetOrgPerson".into()],
            widgets: [("memberOf".to_string(), WidgetSpecCfg::Membership {
                candidate: CandidateRef::Profile("group".into()), via: "member".into() })]
                .into_iter().collect(), ..Default::default() },
        EntryProfile { name: "group".into(), object_classes: vec!["groupOfNames".into()],
            search_base: "ou=groups,dc=x".into(), ..Default::default() },
    ];
    let out = resolve_widgets(&profiles).expect("resolves");
    let rw = out.iter().find(|w| w.attr == "memberOf").expect("memberOf widget");
    match &rw.kind {
        WidgetKind::Picker(b) => {
            assert_eq!(b.fanout_attr.as_deref(), Some("member"));
            assert_eq!(b.select, Some(Cardinality::Multi));
            assert_eq!(b.store, StoreKey::Dn);
        }
        other => panic!("expected Picker, got {other:?}"),
    }
}

#[test]
fn resolve_widget_picker_inline_scope() {
    let profiles = vec![EntryProfile {
        name: "user".into(), object_classes: vec!["inetOrgPerson".into()],
        widgets: [("secretary".to_string(), WidgetSpecCfg::Picker {
            candidate: CandidateRef::Inline(crate::config::InlineScope {
                base: "ou=people,dc=x".into(), object_classes: vec!["inetOrgPerson".into()],
                search_attrs: vec!["cn".into()], label: Some("{cn}".into()) }),
            store: "dn".into(), select: "auto".into() })].into_iter().collect(),
        ..Default::default() }];
    let out = resolve_widgets(&profiles).expect("resolves");
    let rw = out.iter().find(|w| w.attr == "secretary").unwrap();
    match &rw.kind {
        WidgetKind::Picker(b) => {
            assert_eq!(b.scope.base, "ou=people,dc=x");
            assert_eq!(b.select, None); // "auto" → None (arity derived downstream)
            assert!(b.scope.label_template.is_some());
        }
        other => panic!("expected Picker, got {other:?}"),
    }
}

#[test]
fn resolve_widget_picker_unknown_candidate_errors() {
    let profiles = vec![EntryProfile {
        name: "user".into(), object_classes: vec!["inetOrgPerson".into()],
        widgets: [("gidNumber".to_string(), WidgetSpecCfg::Picker {
            candidate: CandidateRef::Profile("nope".into()),
            store: "dn".into(), select: "auto".into() })].into_iter().collect(),
        ..Default::default() }];
    let err = resolve_widgets(&profiles).expect_err("unknown candidate profile errors");
    assert!(err.contains("nope"), "error names the missing profile: {err}");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -j4 -p edaptor --lib resolve_widget_ 2>&1 | tail -20`
Expected: compile error — `WidgetKind::Picker` not found.

- [ ] **Step 3: Make `scope_of` reachable from `config::widget`**

In `src/config/relation.rs:110`, change:

```rust
fn scope_of(p: &EntryProfile) -> CandidateScope {
```
to:
```rust
pub(crate) fn scope_of(p: &EntryProfile) -> CandidateScope {
```

- [ ] **Step 4: Add the `Picker` arm to `WidgetKind`**

In `src/config/widget.rs:38-41`:

```rust
pub enum WidgetKind {
    Choice(ChoiceWidget),
    Password(PasswordWidget),
    /// A unified candidate picker (covers `kind = "picker"` and `"membership"`).
    /// `fanout_attr = Some(_)` marks a membership/fan-out binding.
    Picker(crate::config::relation::PickerBinding),
}
```

- [ ] **Step 5: Add `resolve_candidate` and the two `resolve_widgets` arms**

In `src/config/widget.rs`, add a free helper (above `resolve_widgets`):

```rust
/// Resolve a `CandidateRef` to a live-search `CandidateScope`: reuse a named
/// profile's scope, or build one from an inline table.
fn resolve_candidate(
    c: &crate::config::CandidateRef,
    profiles: &[EntryProfile],
) -> Result<crate::config::relation::CandidateScope, String> {
    use crate::config::CandidateRef;
    match c {
        CandidateRef::Profile(name) => profiles
            .iter()
            .find(|p| &p.name == name)
            .map(crate::config::relation::scope_of)
            .ok_or_else(|| format!("unknown candidate profile \"{name}\"")),
        CandidateRef::Inline(s) => Ok(crate::config::relation::CandidateScope {
            base: s.base.clone(),
            object_classes: s.object_classes.clone(),
            search_attrs: s.search_attrs.clone(),
            label_template: s
                .label
                .as_ref()
                .map(|l| crate::config::label::parse_label_template(l)),
        }),
    }
}
```

Then in `resolve_widgets` (`src/config/widget.rs:57`), inside `let kind = match spec { … }`, after the `WidgetSpecCfg::Password { samba } => { … }` arm, add:

```rust
                WidgetSpecCfg::Picker {
                    candidate,
                    store,
                    select,
                } => {
                    // Parsing copied verbatim from the old resolve_pickers so
                    // behavior is identical; "auto"/unknown select → None.
                    let scope = resolve_candidate(candidate, profiles)?;
                    let store = if store.eq_ignore_ascii_case("dn") {
                        crate::config::relation::StoreKey::Dn
                    } else {
                        crate::config::relation::StoreKey::Attr(store.clone())
                    };
                    let select = match select.to_ascii_lowercase().as_str() {
                        "single" => Some(Cardinality::Single),
                        "multi" => Some(Cardinality::Multi),
                        _ => None,
                    };
                    WidgetKind::Picker(crate::config::relation::PickerBinding {
                        attr: attr.clone(),
                        scope,
                        store,
                        select,
                        fanout_attr: None,
                    })
                }
                WidgetSpecCfg::Membership { candidate, via } => {
                    let scope = resolve_candidate(candidate, profiles)?;
                    WidgetKind::Picker(crate::config::relation::PickerBinding {
                        attr: attr.clone(),
                        scope,
                        store: crate::config::relation::StoreKey::Dn,
                        select: Some(Cardinality::Multi),
                        fanout_attr: Some(via.clone()),
                    })
                }
```

`Cardinality` is already imported in `widget.rs` (used by `ChoiceWidget`). Add `use crate::config::CandidateRef;` / `InlineScope` only if the helper references them unqualified — the code above fully-qualifies, so no new `use` is required beyond what compiles.

- [ ] **Step 6: Add the stub `Picker` arm to `tag_widget_fields` (keep the exhaustive match compiling)**

In `src/ui/edit_form.rs:449`, the `match &rw.kind { … }` is exhaustive. Add a stub arm (full handling lands in T3):

```rust
            WidgetKind::Picker(_) => {
                // Picker/membership wiring lands in T3 (tag onto widget_binding).
            }
```

Note: `password_widget_for` (`widget.rs:146`) already has a `_ => None` fallthrough — no change needed there.

- [ ] **Step 7: Run the resolution tests + full gate**

Run: `cargo test -j4 -p edaptor --lib resolve_widget_ 2>&1 | tail -20`
Expected: all four PASS. The old picker path (`resolve_pickers`, `relation.rs` tests) is untouched and still green.

```bash
cargo fmt && cargo build -j4 --all-targets && cargo test -j4 -p edaptor && cargo clippy -j4 --all-targets -- -D warnings
```

- [ ] **Step 8: Commit**

```bash
git add src/config/widget.rs src/config/relation.rs src/ui/edit_form.rs
git commit -m "feat(config): resolve picker/membership widgets into PickerBinding

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 3: Rewire form/save read path onto `widget_binding::Picker`; migrate examples

The old picker config layer (`resolve_pickers`, `tag_picker_fields`, `App.pickers`, `EditField.picker`) stays present but goes **dormant** here — examples no longer declare `[profile.picker]`, so `resolve_pickers` returns empty and `tag_picker_fields` is a no-op; `EditField.picker` is written by no one (removed in T4). After this task the running app drives pickers entirely through `widget_binding`.

**Files:**
- Modify: `src/ui/edit_form.rs` — full `tag_widget_fields` Picker arm; repoint `fanout_labels`/`to_edit_entry`/`order_fields`
- Modify: `src/ui/app/value_editor.rs` — `open_value_editor` Picker arm; repoint test helpers
- Modify: `src/ui/app/save.rs` — fan-out save read site
- Modify: `examples/config.toml`, `examples/demo-config.toml`
- Modify: `src/config/mod.rs` tests (`demo_config_parses_with_pickers`, `demo_config_widgets_resolve`)
- Modify: `src/ui/edit_form.rs` test (`fanout_labels_come_from_picker_binding`)

- [ ] **Step 1: Add a `fanout_attr_of` helper and a `widget_picker` accessor in `edit_form.rs`**

Near the top of `src/ui/edit_form.rs` (module-level, after the `EditField` impl block), add:

```rust
/// The bound picker, if this field carries a `kind = "picker"`/`"membership"`
/// widget. `None` for choice/password/plain fields.
fn widget_picker(f: &EditField) -> Option<&crate::config::relation::PickerBinding> {
    match &f.widget_binding {
        Some(crate::config::widget::WidgetKind::Picker(b)) => Some(b),
        _ => None,
    }
}

/// The fan-out back-ref attr for a field (a `kind = "membership"` widget), if any.
fn fanout_attr_of(f: &EditField) -> Option<&str> {
    widget_picker(f).and_then(|b| b.fanout_attr.as_deref())
}
```

- [ ] **Step 2: Repoint `to_edit_entry`, `fanout_labels`, `order_fields`**

In `src/ui/edit_form.rs`, replace the three `f.picker…` reads.

`to_edit_entry` (`:292`):
```rust
        .filter(|f| {
            f.picker
                .as_ref()
                .and_then(|b| b.fanout_attr.as_ref())
                .is_none()
        })
```
→
```rust
        .filter(|f| fanout_attr_of(f).is_none())
```

`fanout_labels` (`:311`):
```rust
        .filter(|f| {
            f.picker
                .as_ref()
                .and_then(|b| b.fanout_attr.as_ref())
                .is_some()
        })
```
→
```rust
        .filter(|f| fanout_attr_of(f).is_some())
```

`order_fields` bucket fn (`:481`):
```rust
        } else if !f.current_values().is_empty() || f.secret || f.picker.is_some() {
```
→
```rust
        } else if !f.current_values().is_empty() || f.secret || widget_picker(f).is_some() {
```

- [ ] **Step 3: Replace the stub `tag_widget_fields` Picker arm with full handling**

In `src/ui/edit_form.rs`, replace the **entire** `tag_widget_fields` fn (`:431-472`, currently `if read_only { return; }` then the match) with this version, which folds in the old `tag_picker_fields` read-only semantics (fan-out fields forced editable but honoring global read-only; non-fan-out tagged only when already editable; picker tags apply even in read-only so field ordering is preserved):

```rust
/// Attach a `[profile.widget.<attr>]` widget (choice / password / picker /
/// membership) to each matching field. Choice fields stay editable (Enter opens
/// the choice overlay). Password fields stay read-only inline; Enter opens the
/// password popup. Picker fields open the candidate picker; a membership
/// (fan-out) binding forces the field editable (its value fans out, it is never
/// written to the field itself), honoring global read-only. `.any()` objectClass
/// matching, mirroring `picker_for`/`widget_for`.
pub fn tag_widget_fields(
    form: &mut EditForm,
    widgets: &[crate::config::widget::ResolvedWidget],
    object_classes: &[String],
    read_only: bool,
) {
    use crate::config::widget::WidgetKind;
    let has_oc = |ocs: &[String]| {
        ocs.iter()
            .any(|oc| object_classes.iter().any(|e| e.eq_ignore_ascii_case(oc)))
    };
    for rw in widgets {
        if !has_oc(&rw.owner_object_classes) {
            continue;
        }
        match &rw.kind {
            WidgetKind::Picker(binding) => {
                if let Some(f) = form
                    .fields
                    .iter_mut()
                    .find(|f| f.label.eq_ignore_ascii_case(&rw.attr))
                {
                    if binding.fanout_attr.is_some() {
                        f.editable = !read_only;
                        f.widget_binding = Some(rw.kind.clone());
                    } else if f.editable {
                        f.widget_binding = Some(rw.kind.clone());
                    }
                }
            }
            WidgetKind::Choice(_) => {
                if read_only {
                    continue;
                }
                if let Some(f) = form
                    .fields
                    .iter_mut()
                    .find(|f| f.label.eq_ignore_ascii_case(&rw.attr))
                {
                    f.widget_binding = Some(rw.kind.clone());
                    f.editable = true;
                }
            }
            WidgetKind::Password(pw) => {
                if read_only {
                    continue;
                }
                let mut targets = vec![pw.primary.clone()];
                targets.extend(pw.derived.iter().cloned());
                for f in form.fields.iter_mut() {
                    if targets.iter().any(|t| t.eq_ignore_ascii_case(&f.label)) {
                        f.widget_binding = Some(rw.kind.clone());
                    }
                }
            }
        }
    }
}
```

- [ ] **Step 4: Repoint `open_value_editor` (`value_editor.rs`)**

In `src/ui/app/value_editor.rs:51-57`, replace the `else if let Some(binding) = field.picker.clone()…` branch with a `widget_binding` Picker match:

```rust
    } else if let Some(crate::config::widget::WidgetKind::Picker(binding)) =
        field.widget_binding.clone().filter(|_| field.editable)
    {
        // Unified picker: open from the resolved binding. Labels and real DNs are
        // upgraded from search results in the `Response::Entries` intercept.
        let ve = ValueEditor::open(focus, field, &binding);
        app.overlay = Some(Overlay::ValueEditor(ve));
        app.picker_last_query = PICKER_INIT_QUERY.to_string();
        app.picker_search_id = None;
    } else if field.multi && field.editable {
```

(`ValueEditor::open` takes `&PickerBinding`; `binding` is owned from the cloned `widget_binding`, so `&binding` is correct. The engine — `service_picker_search`, the `Response::Entries` intercept, `ve.binding`, `ve.picker` — is unchanged.)

- [ ] **Step 5: Repoint the fan-out save read site (`save.rs`)**

In `src/ui/app/save.rs:260`:
```rust
        let Some(attr) = f.picker.as_ref().and_then(|b| b.fanout_attr.clone()) else {
            continue;
        };
```
→
```rust
        let Some(attr) = (match &f.widget_binding {
            Some(crate::config::widget::WidgetKind::Picker(b)) => b.fanout_attr.clone(),
            _ => None,
        }) else {
            continue;
        };
```

- [ ] **Step 6: Migrate `examples/config.toml`**

In `examples/config.toml`, replace every `[profile.picker.*]` block with `[profile.widget.*]`:

```toml
# user profile: was [profile.picker.gidNumber] / [profile.picker.memberOf]
[profile.widget.gidNumber]
kind      = "picker"
candidate = "posixgroup"
store     = "gidNumber"
select    = "single"

[profile.widget.memberOf]
kind      = "membership"
candidate = "group"
via       = "member"

# group profile: was [profile.picker.member]
[profile.widget.member]
kind      = "picker"
candidate = "user"

# posixgroup profile: was [profile.picker.memberUid]
[profile.widget.memberUid]
kind      = "picker"
candidate = "user"
store     = "uid"
```

(Keep each block under its existing `[[profile]]`. The old `[profile.picker.member]` had no `store`/`select` → defaults `dn`/`auto`; preserve by omitting them.)

- [ ] **Step 7: Migrate `examples/demo-config.toml`**

Apply the identical replacement in `examples/demo-config.toml` (same four blocks, same profiles). Leave the existing `[profile.widget.userPassword]`, `[profile.widget.sambaAcctFlags]`, `[profile.widget.loginShell]` blocks untouched.

- [ ] **Step 8: Update the two production-read-path tests + the demo-config parse tests**

(a) `src/ui/edit_form.rs` `fanout_labels_come_from_picker_binding` (`:532`): in the `mk` closure's `EditField`, change `picker: Some(PickerBinding { … })` to `picker: None` and set `widget_binding: Some(crate::config::widget::WidgetKind::Picker(PickerBinding { … }))` (move the same binding literal into the `Picker(...)`). The assertion (`form.fanout_labels()` non-empty when `fanout` is `Some`) is unchanged.

(b) `src/ui/app/value_editor.rs` helpers — update the two helpers so every dependent test follows:
- `app_with_lookup_field` (`:634`): `picker: Some(gid_picker_binding())` → `picker: None`, and add/replace `widget_binding: Some(crate::config::widget::WidgetKind::Picker(gid_picker_binding()))`.
- the `member`-field helper used by `open_value_editor_arms_initial_search` (find it via `test_app_with_form_field_member`): same move — bind the member `PickerBinding` through `widget_binding: Some(WidgetKind::Picker(...))`, set `picker: None`.

(c) `src/config/mod.rs` `demo_config_parses_with_pickers` (`:478`): examples no longer have `[profile.picker]`. Repurpose it to assert the migrated widgets parse — replace its body to load `examples/demo-config.toml`, find the `user` profile, and assert `widgets["memberOf"]` is `WidgetSpecCfg::Membership { via, .. }` with `via == "member"` and `widgets["gidNumber"]` is `WidgetSpecCfg::Picker { .. }`. Rename it `demo_config_parses_widget_pickers`.

(d) `src/config/mod.rs` `demo_config_widgets_resolve` (`:748`): extend it to also assert, after `resolve_widgets`, that the `memberOf` resolved widget is `WidgetKind::Picker(b)` with `b.fanout_attr == Some("member")` and the `gidNumber` widget is `WidgetKind::Picker(b)` with `b.fanout_attr == None`.

- [ ] **Step 9: Run the full gate**

```bash
cargo fmt && cargo build -j4 --all-targets && cargo test -j4 -p edaptor && cargo clippy -j4 --all-targets -- -D warnings
```
Expected: green. `tag_picker_fields`/`resolve_pickers` are now dormant (examples carry no `[profile.picker]`); their unit tests (`relation.rs`, `edit_form.rs:839`) still pass against inline TOML and are deleted in T4.

- [ ] **Step 10: Commit**

```bash
git add src/ui/edit_form.rs src/ui/app/value_editor.rs src/ui/app/save.rs examples/config.toml examples/demo-config.toml src/config/mod.rs
git commit -m "refactor(ui): drive pickers through widget_binding; migrate examples

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 4: Delete the dormant picker config layer

Pure deletion. The kept engine (`PickerBinding`, `CandidateScope`, `StoreKey`, `Cardinality`, `scope_of`, `PickerState`, `service_picker_search`, `membership_fanout`, `ValueEditor`, combined-save) is **untouched**. Removing `EditField.picker` is compiler-guided: delete the struct field, then fix every literal the compiler flags.

**Files:** `src/config/mod.rs`, `src/config/relation.rs`, `src/ui/edit_form.rs`, `src/ui/app/mod.rs`, `src/ui/app/action.rs`, `src/ui/app/create.rs`, `src/ui/app/save.rs`, plus every file with an `EditField { … }` literal (compiler lists them), and `tests/live_templates.rs`.

- [ ] **Step 1: Delete the config-parsing layer**

- `src/config/mod.rs`: delete `pub struct PickerSpec` (`:88-103`) and the `pickers: BTreeMap<String, PickerSpec>` field on `EntryProfile` (`:160-162`, the `#[serde(default, rename = "picker")]` one). Delete the `parses_profile_picker_block` test (`:830`).
- `src/config/relation.rs`: delete `pub struct ResolvedPicker` (`:52-57`), `pub fn resolve_pickers` (`:61-92`), `pub fn picker_for` (`:96-108`). Keep `CandidateScope`, `Cardinality`, `StoreKey`, `PickerBinding`, `scope_of` (now `pub(crate)`), and `has_oc` if still used elsewhere — check `cargo build` and remove `has_oc` only if it becomes dead.
- `src/config/relation.rs` tests: delete `resolves_picker_dn_store_defaults`, `resolves_picker_scalar_store_and_select`, `resolves_picker_fanout`, `picker_for_matches_owner_oc_and_attr`, `resolve_pickers_store_and_select_are_case_insensitive` (`:223-340`). Their coverage now lives in `widget.rs` (T2). Do **not** carry `unknown_picker_candidate_is_dropped` forward unchanged — its inverse (errors on unknown candidate) is already covered by `resolve_widget_picker_unknown_candidate_errors` in T2; delete it.

- [ ] **Step 2: Remove `tag_picker_fields` and `EditField.picker`**

- `src/ui/edit_form.rs`: delete `pub fn tag_picker_fields` (`:401-425`) and its test `tag_picker_fields_tags_by_binding_and_forces_fanout_editable` (`:839`).
- `src/ui/edit_form.rs`: delete the `picker: Option<…>` field from `pub struct EditField` (`:46-47`, the `/// `Some` when this field is bound to a `[profile.picker.<attr>]` picker.` doc + field).

- [ ] **Step 3: Fix every `EditField` literal the compiler flags**

Run `cargo build -j4 --all-targets 2>&1 | grep -A2 "missing field \`picker\`"` and remove the `picker: …,` line from each flagged `EditField { … }` literal. Known sites (≈30, mostly tests): `src/ui/edit_form.rs` (368, 536, 760, 809, 843, 887, 915, 956, 1007), `src/ui/view.rs` (703, 929, 1143, 1387), `src/ui/app/test_support.rs:50`, `src/ui/app/input.rs` (520, 536), `src/ui/app/value_editor.rs` (498, 634 — now uses `widget_binding`, just drop the `picker:` line, 974), `src/ui/app/save.rs` (545, 560, 575, 623, 638, 760, 774), `src/ui/app/password_editor.rs:141`. Let the compiler be the source of truth.

- [ ] **Step 4: Remove `App.pickers` and the construction wiring**

- `src/ui/app/mod.rs`: delete the `pub pickers: Vec<…ResolvedPicker>` field (`:116`), the `use crate::config::relation::resolve_pickers;` import (`:22`), the `let pickers = resolve_pickers(&config.profiles);` line (`:137`), and the `pickers,` struct-init line (`:202`). Drop `&app.pickers` from the `build_loaded_form` call (`:485`).
- `src/ui/app/action.rs`: delete the `tag_picker_fields(&mut form, pickers, &ocs, read_only);` call (`:501`) and the `pickers: &[ResolvedPicker]` parameter from `build_loaded_form`; update all `build_loaded_form` callers to drop the pickers arg (`mod.rs:485`, `save.rs:344`).
- `src/ui/app/create.rs`: drop `&app.pickers` from `build_new_entry_form` (`:151`) and remove that fn's pickers parameter + the `tag_picker_fields` call inside it if present.
- `src/ui/app/save.rs`: drop `&app.pickers` from the `build_loaded_form` call (`:344`).

(`build_loaded_form` / `build_new_entry_form` already receive `&app.widgets` and call `tag_widget_fields`, which now handles pickers — so deleting the pickers arg + `tag_picker_fields` call is sufficient.)

- [ ] **Step 5: Migrate the live tests**

In `tests/live_templates.rs`, replace the 4 `resolve_pickers` sites (`:509`, `:593`, `:694`, `:854`) and the import (`:24`). Pattern for each:

```rust
// before:
use edaptor::config::relation::{resolve_pickers, StoreKey};
let pickers = resolve_pickers(&cfg.profiles);
let binding = picker_for(&pickers, &ocs, "memberOf").expect("...");

// after:
use edaptor::config::relation::StoreKey;
use edaptor::config::widget::{resolve_widgets, widget_for, WidgetKind};
let widgets = resolve_widgets(&cfg.profiles).expect("resolve widgets");
let WidgetKind::Picker(binding) = widget_for(&widgets, &ocs, "memberOf").expect("memberOf widget")
else { panic!("expected a picker widget") };
```

Adjust each of the 4 tests to pull its binding (`memberOf`/`member`/`gidNumber`/`memberUid`) from `widget_for(...).Picker(...)`. These are compiled even though gated; `cargo build --all-targets` must pass.

- [ ] **Step 6: Full gate + done-gate grep**

```bash
cargo fmt && cargo build -j4 --all-targets && cargo test -j4 -p edaptor && cargo clippy -j4 --all-targets -- -D warnings
# done-gate: these names must be GONE (the kept engine has none of them):
grep -rn "resolve_pickers\|picker_for\|tag_picker_fields\|ResolvedPicker\|PickerSpec\|\.pickers\b" src tests
```
Expected: gate green; grep returns nothing.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "refactor: delete the old [profile.picker] config layer

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 5: Docs — fold `pickers.md` into `widgets.md`

**Files:** `docs/src/configuration/widgets.md`, delete `docs/src/configuration/pickers.md`, `docs/src/SUMMARY.md`, `docs/src/configuration/overview.md`, `docs/src/usage/membership.md`.

- [ ] **Step 1: Extend the widgets.md kinds table**

In `docs/src/configuration/widgets.md`, replace the "Two kinds are available today" table with a four-row table adding `picker` and `membership`:

```markdown
| `kind` | Editor | Use it for |
|---|---|---|
| [`choice`](#the-choice-kind) | a checklist (multi) / radio list (single) over a fixed set of options | enumerated or flag attributes — `loginShell`, `sambaAcctFlags` |
| [`password`](#the-password-kind) | a masked **New + Confirm** set-password popup | password / hash attributes — `userPassword`, with optional Samba sync |
| [`picker`](#the-picker-kind) | a live candidate search; stores the picked value(s) in this entry | value lookup (`gidNumber`) and DN/scalar lists (`member`, `memberUid`) |
| [`membership`](#the-membership-kind) | a live candidate search; fans this entry's DN into a back-ref attr on each pick | back-reference views (`memberOf`) |
```

(Drop any "the only implemented kind is choice" style line if present.)

- [ ] **Step 2: Add the `picker` and `membership` kind sections**

Append two sections to `widgets.md`, ported from `pickers.md` content but using the new config. Include: the `candidate` field (profile-name string **or** inline `{ base, object_classes, search_attrs?, label? }` table), `store` (`"dn"` sentinel or attr name) and `select` (`single`/`multi`/`auto`) for `picker`; `via` for `membership` (always multi). Use the worked examples from the spec's "Config schema" section (`gidNumber`, `member`, `memberOf`, inline `secretary`).

- [ ] **Step 3: Delete `pickers.md` and repoint links**

- `git rm docs/src/configuration/pickers.md`.
- `docs/src/SUMMARY.md`: delete the `- [Pickers](configuration/pickers.md)` line (`:16`).
- `docs/src/configuration/overview.md`: delete the Pickers row (`:52`); update the Widgets row (`:51`) to mention picker/membership too.
- `docs/src/usage/membership.md`: repoint `../configuration/pickers.md` links (`:11`, `:23`, `:53`) to `../configuration/widgets.md` (and reword "a [picker]" / "[Pickers]" to point at the widget kinds).

- [ ] **Step 4: Build the book (no broken links) + commit**

```bash
( cd docs && mdbook build )   # clean = no broken links
git add -A
git commit -m "docs: fold pickers into the widget palette

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 6: Changelog

**Files:** `CHANGES.md`

- [ ] **Step 1: Add picker/membership to the Unreleased section**

Under `## Unreleased` → `### New` (after the choice/password bullets), add:

```markdown
  - `kind = "picker"` — populate an attribute from a live candidate search and
    store the picked value(s) in this entry (value lookup like `gidNumber`, or a
    DN/scalar list like `member`/`memberUid`). `candidate` is a `[[profile]]`
    name or an inline `{ base, object_classes, … }` scope.
  - `kind = "membership"` — fan this entry's DN into a back-reference attribute
    (`via`) on each picked candidate (e.g. `memberOf` writes `member` on each
    chosen group).
```

Under `### Changed`, add:

```markdown
- **Pickers are now configured with `[profile.widget.<attr>] kind = "picker"` /
  `"membership"`** instead of `[profile.picker.<attr>]`, which has been removed.
```

- [ ] **Step 2: Commit**

```bash
git add CHANGES.md
git commit -m "docs: changelog for picker/membership widgets

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 7: Live smoke (manual, against the podman server)

**Not automatable in CI — gated on the test LDAP. Run once before declaring done.**

- [ ] **Step 1: Start the server and launch the TUI**

```bash
scripts/test-ldap.sh start
EDAPTOR_TEST_ADMIN_PW=adminpassword \
  cargo run -j4 --bin edaptor -- --config examples/demo-config.toml
```

- [ ] **Step 2: Verify the membership (fan-out) path**

Open a user, focus `memberOf`, Enter → the candidate picker opens; toggle a group, Alt+S, save. Confirm the chosen group's `member` now contains the user's DN (fan-out worked).

- [ ] **Step 3: Verify the value-lookup picker**

Open a user, focus `gidNumber`, Enter → picker opens over posixGroups; pick one, commit, save. Confirm the group's `gidNumber` is stored on the user.

- [ ] **Step 4: Stop the server**

```bash
scripts/test-ldap.sh stop   # do NOT pkill -f edaptor — it matches the container
```

(Quit the TUI via **Alt+X**, per `docs/HANDOVER.md` / memory `edaptor-tui-debug-gotchas`.)

---

## Self-review notes

- **Spec coverage:** kinds (T1/T2), `candidate` profile-ref + inline (T1/T2), removal of the old layer (T4), engine unchanged (no engine file touched), example migration (T3), docs fold (T5), tests per the spec's Testing section (T1–T4), live smoke (T7). All present.
- **`select="auto"` → `None`:** preserved in T2 (verbatim from `resolve_pickers`); arity derivation stays downstream in `ValueEditor::open` (untouched).
- **Read-only ordering parity:** T3's `tag_widget_fields` runs the Picker arm even when `read_only` (Choice/Password skip), matching the old `tag_picker_fields`.
- **Match exhaustiveness:** only `tag_widget_fields` (`edit_form.rs:449`) needs the new `Picker` arm; `password_widget_for` already has `_ => None`. `open_value_editor`/`view.rs`/`password_editor.rs` use `matches!`/`if let`, so they don't break on the added variant.
- **Type-name consistency:** `CandidateRef`, `InlineScope`, `WidgetSpecCfg::{Picker,Membership}`, `WidgetKind::Picker`, `resolve_candidate`, `fanout_attr_of`, `widget_picker`, `widget_for` used consistently across tasks.
