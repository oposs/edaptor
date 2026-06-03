# Unified Configurable Picker — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Collapse the three forked field-population mechanisms — `[[relation]]` (DN membership + `memberOf` fan-out) and `[profile.lookup.<attr>]` (scalar lookup), plus the unbuilt multi-scalar case — into **one** `[profile.picker.<attr>]` binding consumed by a single engine.

**Architecture:** A per-`(profile, attribute)` `PickerSpec` (raw config) resolves to one internal `PickerBinding` carrying a `CandidateScope`, a `StoreKey` (`Dn` or `Attr(name)`), an optional `Cardinality`, and an optional `fanout_attr`. `EditField` carries `Option<PickerBinding>`; `PickerState` keys candidates by their **store value** (not always-DN); one `ValueEditor::open`, one search-param builder, one Enter handler, one overlay-commit, and one form-save fan-out branch (on `fanout_attr`) replace the three forked paths. Clean cut — no back-compat for `[[relation]]`/`[profile.lookup]`.

**Tech Stack:** Rust, ratatui 0.30 (UI facade in `src/ui/*` only), `serde`/`toml` config, strict TDD with `cargo test -p edaptor`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt`.

**Spec:** `docs/superpowers/specs/2026-06-03-unified-picker-design.md`

---

## Staging strategy (read before starting)

The project requires **the crate compiles + tests pass after every commit** and uses **atomic commits**. A clean-cut replacement is staged as a **dual-path** sequence: new types/fields are added *alongside* the old ones (Tasks 1–6), then the old ones are deleted together with their tests in one cleanup commit (Task 7).

Key facts that make this safe (confirmed against the codebase):

- **Dead-code rule under `--all-targets`:** a `pub` type stays "live" as long as its own `#[cfg(test)]` tests reference it. So an old type left unused by `app.rs` does **not** trip `clippy -D warnings` until you also delete its tests. Therefore: **delete each old type together with its tests, in Task 7 — never earlier.** Do **not** add `#[allow(dead_code)]` shims.
- **`App.pickers` and `App.relations` coexist fine** — Tasks 5 and 6 stay separate. Fallback only if a dead-code warning actually bites: fold 5+6+7 into one in-session commit.
- **app.rs is 4812 lines** — a subagent-context hazard. Tasks 5, 6, 7 give exact functions + line ranges; a subagent must edit only those, not free-range the file.

The "2–5 min" granularity ideal applies to **steps**, not commits — a refactor's atomic commits are legitimately chunkier.

### File structure after this plan

| File | Responsibility after change |
|---|---|
| `src/config/relation.rs` | **Repurposed in place** (keep filename to minimise churn): keeps `CandidateScope` + `scope_of`; gains `PickerSpec` raw type is in `mod.rs`, and `PickerBinding`/`StoreKey`/`Cardinality`/`resolve_pickers`/`picker_for` live here. Old `Relation`/`ResolvedRelation`/`RelationRole`/`resolve_relations`/`holder_lookup`/`backref_lookup` deleted in Task 7. |
| `src/config/mod.rs` | `EntryProfile` gains `pickers: BTreeMap<String, PickerSpec>` (rename `picker`); loses `lookups` + `Config.relations` + `LookupSpec` in Task 7. `PickerSpec` raw type defined here next to `LookupSpec`. |
| `src/ui/picker.rs` | `Candidate` gains `store_value`; `PickerState` keys by store value (`key_ci` flag), `selected_values()`. `value`/`selected_dns`/`same_dn` removed in Task 7. |
| `src/ui/edit_form.rs` | `EditField` gains `picker: Option<PickerBinding>`; one `ValueEditor::open`; binding-driven tagging; fan-out fields force-editable. `relation`/`lookup`, `FieldRelation`, `tag_lookup_fields`, old ctors removed in Task 7. |
| `src/ui/app.rs` | `App.pickers`; one dispatch / search / result-map / Enter / overlay-commit / form-save fan-out, all binding-driven. `App.relations` + old branches removed in Task 7. |
| `examples/demo-config.toml`, `README.md` | Rewritten to `[profile.picker.*]` + a `posixgroup` profile (Task 8). |

---

## Task 1: Raw `PickerSpec` config type + `EntryProfile.pickers`

**Files:**
- Modify: `src/config/mod.rs` (add `PickerSpec` next to `LookupSpec` ~line 92; add `pickers` field to `EntryProfile` ~line 123)

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `src/config/mod.rs`:

```rust
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
    // member: defaults — store "dn", select "auto", no fanout.
    let member = p.pickers.get("member").expect("member picker");
    assert_eq!(member.candidate, "user");
    assert_eq!(member.store, "dn");
    assert_eq!(member.select, "auto");
    assert_eq!(member.fanout_attr, None);
    // memberOf: explicit fanout.
    let mof = p.pickers.get("memberOf").expect("memberOf picker");
    assert_eq!(mof.fanout_attr.as_deref(), Some("member"));
    // gidNumber: scalar store + single select.
    let gid = p.pickers.get("gidNumber").expect("gidNumber picker");
    assert_eq!(gid.store, "gidNumber");
    assert_eq!(gid.select, "single");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p edaptor parses_profile_picker_block`
Expected: FAIL — `no field 'pickers' on EntryProfile` / `PickerSpec` not found (compile error).

- [ ] **Step 3: Add the `PickerSpec` type and the `pickers` field**

In `src/config/mod.rs`, immediately after the `LookupSpec` struct (after line 92), add:

```rust
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
```

Then add the `pickers` field to `EntryProfile` (after the `lookups` field, before `label`, ~line 123):

```rust
    /// Per-attribute picker bindings (`[profile.picker.<attr>]`). Each declares how
    /// the named attribute's field is populated from a candidate search. Supersedes
    /// `[[relation]]` and `[profile.lookup.*]`.
    #[serde(default, rename = "picker")]
    pub pickers: std::collections::BTreeMap<String, PickerSpec>,
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p edaptor parses_profile_picker_block`
Expected: PASS.

- [ ] **Step 5: Fix the `relation.rs` test fixture that constructs `EntryProfile` literally**

`src/config/relation.rs:167` (`fn profile`) builds `EntryProfile { … }` with all fields named. Add `pickers: Default::default(),` to that literal (next to `lookups: Default::default(),` at line 178) so it still compiles.

Run: `cargo test -p edaptor && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: all green (fmt may need `cargo fmt`; run it then re-check).

- [ ] **Step 6: Commit**

```bash
git add src/config/mod.rs src/config/relation.rs
git commit -m "feat(config): add [profile.picker.<attr>] raw spec + EntryProfile.pickers

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Resolved `PickerBinding` + `resolve_pickers` + `picker_for`

**Files:**
- Modify: `src/config/relation.rs` (add new types + resolution near the top, after `CandidateScope`; reuse `scope_of`)

This task adds the resolved binding the UI consumes. It reuses `CandidateScope` and `scope_of` (already in `relation.rs`). It does not touch the existing relation types.

- [ ] **Step 1: Write the failing tests**

Add to `src/config/relation.rs`'s `#[cfg(test)] mod tests`:

```rust
#[test]
fn resolves_picker_dn_store_defaults() {
    use crate::config::PickerSpec;
    let mut group = profile("group", "groupOfNames", "ou=groups,dc=x", &["cn"]);
    group.pickers.insert(
        "member".to_string(),
        PickerSpec {
            candidate: "user".into(),
            store: "dn".into(),
            select: "auto".into(),
            fanout_attr: None,
        },
    );
    let user = profile("user", "inetOrgPerson", "ou=people,dc=x", &["uid", "cn"]);
    let resolved = resolve_pickers(&[group, user]);
    assert_eq!(resolved.len(), 1);
    let b = &resolved[0].binding;
    assert_eq!(b.attr, "member");
    assert_eq!(b.scope.base, "ou=people,dc=x"); // candidate = user
    assert_eq!(b.store, StoreKey::Dn);
    assert_eq!(b.select, None); // "auto"
    assert_eq!(b.fanout_attr, None);
    // Owner object classes (the group profile) drive entry matching.
    assert_eq!(resolved[0].owner_object_classes, vec!["groupOfNames".to_string()]);
}

#[test]
fn resolves_picker_scalar_store_and_select() {
    use crate::config::PickerSpec;
    let mut user = profile("user", "inetOrgPerson", "ou=people,dc=x", &["uid"]);
    user.pickers.insert(
        "gidNumber".to_string(),
        PickerSpec {
            candidate: "posixgroup".into(),
            store: "gidNumber".into(),
            select: "single".into(),
            fanout_attr: None,
        },
    );
    let pg = profile("posixgroup", "posixGroup", "ou=groups,dc=x", &["cn"]);
    let resolved = resolve_pickers(&[user, pg]);
    let b = &resolved[0].binding;
    assert_eq!(b.store, StoreKey::Attr("gidNumber".to_string()));
    assert_eq!(b.select, Some(Cardinality::Single));
}

#[test]
fn resolves_picker_fanout() {
    use crate::config::PickerSpec;
    let mut user = profile("user", "inetOrgPerson", "ou=people,dc=x", &["uid"]);
    user.pickers.insert(
        "memberOf".to_string(),
        PickerSpec {
            candidate: "group".into(),
            store: "dn".into(),
            select: "multi".into(),
            fanout_attr: Some("member".into()),
        },
    );
    let group = profile("group", "groupOfNames", "ou=groups,dc=x", &["cn"]);
    let resolved = resolve_pickers(&[user, group]);
    let b = &resolved[0].binding;
    assert_eq!(b.fanout_attr.as_deref(), Some("member"));
    assert_eq!(b.select, Some(Cardinality::Multi));
    assert_eq!(b.scope.base, "ou=groups,dc=x"); // candidate = group
}

#[test]
fn unknown_picker_candidate_is_dropped() {
    use crate::config::PickerSpec;
    let mut group = profile("group", "groupOfNames", "ou=groups,dc=x", &["cn"]);
    group.pickers.insert(
        "member".to_string(),
        PickerSpec {
            candidate: "nope".into(),
            store: "dn".into(),
            select: "auto".into(),
            fanout_attr: None,
        },
    );
    assert!(resolve_pickers(&[group]).is_empty());
}

#[test]
fn picker_for_matches_owner_oc_and_attr() {
    use crate::config::PickerSpec;
    let mut group = profile("group", "groupOfNames", "ou=groups,dc=x", &["cn"]);
    group.pickers.insert(
        "member".to_string(),
        PickerSpec {
            candidate: "user".into(),
            store: "dn".into(),
            select: "auto".into(),
            fanout_attr: None,
        },
    );
    let user = profile("user", "inetOrgPerson", "ou=people,dc=x", &["uid"]);
    let resolved = resolve_pickers(&[group, user]);
    let ocs = vec!["top".to_string(), "groupOfNames".to_string()];
    assert!(picker_for(&resolved, &ocs, "member").is_some());
    // wrong objectClass → no match
    assert!(picker_for(&resolved, &["inetOrgPerson".to_string()], "member").is_none());
    // wrong attr → no match
    assert!(picker_for(&resolved, &ocs, "owner").is_none());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p edaptor -- resolves_picker unknown_picker picker_for_matches`
Expected: FAIL — `StoreKey`/`Cardinality`/`PickerBinding`/`resolve_pickers`/`picker_for` not found.

- [ ] **Step 3: Add the resolved types + resolution**

In `src/config/relation.rs`, after the `CandidateScope` struct (after line 42), add:

```rust
/// Picker cardinality: how many candidates may be selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cardinality {
    Single,
    Multi,
}

/// What a pick stores into the field — and the identity key for dedupe/toggle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreKey {
    /// Store the candidate's DN; key compared case-insensitively.
    Dn,
    /// Store this scalar attribute of the candidate; key compared exactly.
    Attr(String),
}

/// A `[profile.picker.<attr>]` binding resolved against the profile list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerBinding {
    /// The attribute this binds (e.g. `memberUid`).
    pub attr: String,
    /// Resolved candidate search scope (from the `candidate` profile).
    pub scope: CandidateScope,
    /// What each pick contributes, and the identity key.
    pub store: StoreKey,
    /// Cardinality; `None` = derive from the field's schema arity (`select = "auto"`).
    pub select: Option<Cardinality>,
    /// `Some` ⇒ synthetic back-ref: write this attr on each picked candidate's
    /// entry (this entry's DN), and do not write the field to the server.
    pub fanout_attr: Option<String>,
}

/// A resolved picker bound to its owning profile's object classes (for entry
/// matching) — the picker analogue of `ResolvedRelation`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPicker {
    /// Object classes of the profile that DECLARES this picker (the field owner).
    pub owner_object_classes: Vec<String>,
    pub binding: PickerBinding,
}

/// Resolve every `[profile.picker.*]` across all profiles. A picker whose
/// `candidate` names an unknown profile is dropped (caller may warn).
pub fn resolve_pickers(profiles: &[EntryProfile]) -> Vec<ResolvedPicker> {
    let find = |name: &str| profiles.iter().find(|p| p.name == name);
    let mut out = Vec::new();
    for owner in profiles {
        for (attr, spec) in &owner.pickers {
            let Some(cand) = find(&spec.candidate) else {
                continue; // unknown candidate profile → drop
            };
            let store = if spec.store.eq_ignore_ascii_case("dn") {
                StoreKey::Dn
            } else {
                StoreKey::Attr(spec.store.clone())
            };
            let select = match spec.select.as_str() {
                "single" => Some(Cardinality::Single),
                "multi" => Some(Cardinality::Multi),
                _ => None, // "auto" (or anything else) → derive from schema arity
            };
            out.push(ResolvedPicker {
                owner_object_classes: owner.object_classes.clone(),
                binding: PickerBinding {
                    attr: attr.clone(),
                    scope: scope_of(cand),
                    store,
                    select,
                    fanout_attr: spec.fanout_attr.clone(),
                },
            });
        }
    }
    out
}

/// The picker binding for `(entry object classes, attr)`, if any: the entry must
/// carry one of the picker's owner object classes and the attr must match.
pub fn picker_for<'a>(
    pickers: &'a [ResolvedPicker],
    ocs: &[String],
    attr: &str,
) -> Option<&'a PickerBinding> {
    pickers
        .iter()
        .find(|p| {
            p.binding.attr.eq_ignore_ascii_case(attr)
                && p.owner_object_classes
                    .iter()
                    .any(|oc| has_oc(ocs, oc))
        })
        .map(|p| &p.binding)
}
```

(`scope_of` and `has_oc` already exist in this file.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p edaptor -- resolves_picker unknown_picker picker_for_matches`
Expected: PASS.

- [ ] **Step 5: Full check + commit**

```bash
cargo test -p edaptor && cargo clippy --all-targets -- -D warnings && cargo fmt --check
git add src/config/relation.rs
git commit -m "feat(config): resolve PickerSpec to PickerBinding (resolve_pickers/picker_for)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: `PickerState` keys by store value; `Candidate.store_value`

**Files:**
- Modify: `src/ui/picker.rs`

Generalise `PickerState` from always-DN identity to **store-value** identity. The `Candidate` keeps `dn` (the real entry DN — fan-out target, and the key when `store = dn`) and gains `store_value` (the scalar committed; equals the DN for `store = dn`). A `key_ci: bool` on `PickerState` selects case-insensitive (DN) vs exact (scalar) key comparison. `saved` becomes a list of **store values**, not DNs. `value: Option<String>` is replaced by `store_value: String`.

> **Critical (per design review):** the `Response::Entries` label-upgrade (Task 5) must match seeded selections to results by **store value**, not DN — for scalar stores the seeded DN is a placeholder. This task makes `store_value` the identity that Task 5 relies on.

- [ ] **Step 1: Write the failing tests**

Replace the `fn c(dn)` helper in `picker.rs`'s test module and add store-value tests. First, update the existing helper (it currently sets `value: None`):

```rust
fn c(dn: &str) -> Candidate {
    Candidate {
        dn: dn.into(),
        label: dn.into(),
        store_value: dn.into(), // store = dn: store_value == dn
    }
}
```

Add these new tests:

```rust
#[test]
fn scalar_store_keys_by_value_exact() {
    // store = uid: identity is the scalar, compared exactly (case-sensitive).
    let mut p = PickerState::new(
        vec![Candidate {
            dn: "alice".into(), // placeholder for scalar store
            label: "alice".into(),
            store_value: "alice".into(),
        }],
        false, // key_ci = false (scalar, exact)
    );
    // A result with a different-cased value is NOT the same key.
    p.set_results(vec![Candidate {
        dn: "uid=Alice,ou=people".into(),
        label: "Alice".into(),
        store_value: "Alice".into(),
    }]);
    let dns: Vec<_> = p.visible().iter().map(|r| r.candidate.store_value.clone()).collect();
    assert_eq!(dns, vec!["alice".to_string(), "Alice".to_string()]); // distinct
}

#[test]
fn dn_store_keys_case_insensitively() {
    let mut p = PickerState::new(vec![c("UID=Bob,OU=people")], true); // key_ci = true
    p.set_results(vec![c("uid=bob,ou=people")]); // same DN, different case
    // Same key → not duplicated; the seeded selection stays.
    assert_eq!(p.visible().len(), 1);
}

#[test]
fn selected_values_returns_store_values() {
    let mut p = PickerState::new(vec![], false);
    p.set_results(vec![
        Candidate { dn: "uid=a,o=x".into(), label: "A".into(), store_value: "1001".into() },
        Candidate { dn: "uid=b,o=x".into(), label: "B".into(), store_value: "1002".into() },
    ]);
    p.cursor = 0;
    p.toggle_cursor();
    p.cursor = 1;
    p.toggle_cursor();
    let mut vals = p.selected_values();
    vals.sort();
    assert_eq!(vals, vec!["1001".to_string(), "1002".to_string()]);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p edaptor -p edaptor --lib picker`
Expected: FAIL — `PickerState::new` takes 1 arg not 2; `store_value`/`selected_values` not found.

- [ ] **Step 3: Update `Candidate`, `PickerState`, and the keying**

In `src/ui/picker.rs`:

1. Replace the `Candidate` struct (lines 65–72):

```rust
/// One candidate: the real entry `dn` (fan-out target; also the key/store value
/// when `store = dn`), the human `label` shown, and `store_value` — the scalar
/// committed into the field and the identity key for dedupe/toggle/selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub dn: String,
    pub label: String,
    /// The value committed for this pick (the DN for `store = dn`, else the
    /// chosen `store` attribute). Also the identity key.
    pub store_value: String,
}
```

2. Add a `key_ci` field to `PickerState` (after `truncated`, line 125):

```rust
    /// True ⇒ keys (store values) compare case-insensitively (DN store); false ⇒
    /// exact (scalar store). Set at construction from the binding's `StoreKey`.
    pub key_ci: bool,
```

3. Replace `same_dn` (lines 128–130) with a key comparator that respects `key_ci`:

```rust
impl PickerState {
    fn same_key(&self, a: &str, b: &str) -> bool {
        if self.key_ci {
            a.eq_ignore_ascii_case(b)
        } else {
            a == b
        }
    }
}
```

4. Update `PickerState::new` (lines 133–144) to take `key_ci` and seed `saved` from store values:

```rust
    pub fn new(selected: Vec<Candidate>, key_ci: bool) -> Self {
        let saved = selected.iter().map(|c| c.store_value.clone()).collect();
        PickerState {
            selected,
            results: Vec::new(),
            saved,
            cursor: 0,
            scroll: 0,
            search_active: false,
            truncated: false,
            key_ci,
        }
    }
```

5. In `visible()` (lines 159–222), replace every `same_dn(...)` and `&c.dn`/`&r.dn`/`&s.dn` key usage with `self.same_key(...)` over `store_value`. Concretely, the three closures become:

```rust
        let is_saved = |sv: &str| self.saved.iter().any(|d| self.same_key(d, sv));
        let is_selected = |sv: &str| self.selected.iter().any(|s| self.same_key(&s.store_value, sv));
        let in_results = |sv: &str| self.results.iter().any(|r| self.same_key(&r.store_value, sv));
```

and every call site passes `&r.store_value` / `&c.store_value` instead of `.dn`. The synthesized "saved, will be removed" row (lines 206–219) builds its `Candidate` from the saved **store value**:

```rust
        for sv in &self.saved {
            let in_selected = self.selected.iter().any(|s| self.same_key(&s.store_value, sv));
            let in_results = self.results.iter().any(|r| self.same_key(&r.store_value, sv));
            if !in_selected && !in_results {
                rows.push(VisibleRow {
                    candidate: Candidate {
                        dn: sv.clone(),
                        label: sv.clone(),
                        store_value: sv.clone(),
                    },
                    selected: false,
                    saved: true,
                });
            }
        }
```

> Note: closures borrowing `self` immutably while building `rows` is fine; if the borrow checker objects to `self.same_key` inside a closure that also needs `&self.saved`, snapshot `let key_ci = self.key_ci;` and inline the comparison, or make `same_key` a free `fn same_key(ci: bool, a, b)`. Prefer the free-fn form if borrow errors appear.

6. Update `toggle_cursor` (lines 243–258) to key on `store_value`:

```rust
    pub fn toggle_cursor(&mut self) {
        let rows = self.visible();
        let Some(row) = rows.get(self.cursor) else { return; };
        let sv = row.candidate.store_value.clone();
        if let Some(pos) = self.selected.iter().position(|s| self.same_key(&s.store_value, &sv)) {
            self.selected.remove(pos);
        } else {
            self.selected.push(row.candidate.clone());
        }
        let n = self.visible().len();
        if self.cursor >= n {
            self.cursor = n.saturating_sub(1);
        }
    }
```

7. Replace `selected_dns` (lines 260–262) with `selected_values` (returns store values) **and** keep a `selected_dns` returning `dn`s (still needed by the fan-out, which uses `store = dn` so `dn == store_value`):

```rust
    /// Store values of the current selection — what a direct-write commit writes.
    pub fn selected_values(&self) -> Vec<String> {
        self.selected.iter().map(|c| c.store_value.clone()).collect()
    }

    /// Real entry DNs of the current selection — fan-out targets (`store = dn`).
    pub fn selected_dns(&self) -> Vec<String> {
        self.selected.iter().map(|c| c.dn.clone()).collect()
    }
```

8. Add `key_ci: false` (or appropriate) wherever `PickerState { ... }` is built literally other than `new` — there is none besides `Default`. For `Default`, derive still works because `bool: Default` → `false`. Leave `#[derive(Default)]`.

9. Fix `pick_value`'s doc comment referencing `value` if needed (no code change).

- [ ] **Step 4: Fix all in-file test call sites**

Every `PickerState::new(...)` in `picker.rs` tests now needs a second arg. The membership/DN tests use `true`; add it. Replace `.value: None` / `value: None` literals in tests with `store_value: <dn>`. Replace `selected_dns()` assertions that expect DNs — they still work for DN-store (`true`). For the new scalar tests use `selected_values()`.

Specifically update: `selected_stays_visible_when_results_exclude_it`, `results_already_selected_are_not_duplicated`, `toggle_adds_and_removes`, `cursor_clamps`, `move_cursor_advances_and_stops_at_last`, `new_marks_seeded_selection_as_saved`, `visible_flags_saved_rows`, `set_results_resets_cursor_and_scroll_to_top`, `search_active_puts_matches_first_and_selected_after`, `no_search_keeps_selected_first`, `toggling_off_a_saved_member_keeps_saved_true` — all pass `true` to `new(...)`. `new_marks_seeded_selection_as_saved` asserts `state.saved == vec!["uid=bob,ou=people"]` which still holds (store_value == dn).

- [ ] **Step 5: Run picker tests to verify they pass**

Run: `cargo test -p edaptor --lib picker`
Expected: PASS (note: full crate won't compile yet — `edit_form.rs`/`app.rs` still call `PickerState::new` with one arg and read `.value`; those are fixed in Tasks 4–6. To keep this commit green, this task must also patch the *minimal* call sites in edit_form/app to compile. See Step 6.)

- [ ] **Step 6: Make the crate compile (minimal call-site patches)**

The old `ValueEditor::open_picker`/`open_lookup` (edit_form.rs:153,192) call `PickerState::new(selected)` and build `Candidate { …, value: … }`; `app.rs` reads `candidate.value` (lines 819–828, 435–438) and calls `selected_dns()`. To keep *this* commit compiling without doing the Task 4–6 rewrite yet, apply mechanical patches:

- In `edit_form.rs` `open_picker`: `PickerState::new(selected, true)`; build `Candidate { dn, label, store_value: dn.clone() }` (drop `value`).
- In `edit_form.rs` `open_lookup`: `PickerState::new(Vec::new(), false)`.
- In `app.rs` Response::Entries map (lines 434–449): set `store_value` instead of `value` — for the lookup arm `store_value: pick_value(&e.attrs, &spec.value_attr).unwrap_or_default()`, for the membership arm `store_value: e.dn.clone()`.
- In `app.rs` lookup Alt+S (lines 819–828): replace `c.value.clone()` / `row.candidate.value.clone()` reads with `Some(c.store_value.clone())` / `Some(row.candidate.store_value.clone())` (filtering empties: `.filter(|s| !s.is_empty())`).
- In `app.rs` Enter single-select (line 872) and lookup label upgrade (line 454): change `.dn` matching to `.store_value` where it compares seeded vs results **only for the lookup arm** — but since the lookup arm now has placeholder dn, match on `store_value`. (This is a temporary bridge; Task 5 rewrites this region cleanly.)

> These are throwaway bridge edits — Task 5 replaces this whole region. Keep them minimal; the goal is only "compiles + green this commit."

- [ ] **Step 7: Full check + commit**

```bash
cargo test -p edaptor && cargo clippy --all-targets -- -D warnings && cargo fmt --check
git add src/ui/picker.rs src/ui/edit_form.rs src/ui/app.rs
git commit -m "refactor(picker): key PickerState by store value; Candidate.store_value

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: `EditField.picker` + unified `ValueEditor::open` + binding-driven tagging

**Files:**
- Modify: `src/ui/edit_form.rs`

Add `picker: Option<PickerBinding>` to `EditField` **alongside** the existing `relation`/`lookup` (removed in Task 7). Add one `ValueEditor::open(field_idx, field, binding)` constructor. Tag fields from resolved pickers in a new pass. Fan-out fields are force-editable.

- [ ] **Step 1: Write the failing tests**

Add to `edit_form.rs` test module:

```rust
#[test]
fn tag_picker_fields_tags_by_binding_and_forces_fanout_editable() {
    use crate::config::relation::{
        resolve_pickers, CandidateScope, PickerBinding, ResolvedPicker, StoreKey,
    };
    // Build a form with a memberOf field that schema would mark read-only.
    let mut form = EditForm {
        dn: "uid=bob,ou=people,dc=x".into(),
        fields: vec![
            EditField {
                label: "memberOf".into(),
                must: false,
                editable: false, // operational, read-only by default
                multi: true,
                secret: false,
                ordered: false,
                values: vec!["cn=admins,ou=groups,dc=x".into()],
                kind: FieldKind::Dn,
                widget: WidgetSpec::ReadOnlyText,
                editor: TextState::new(),
                relation: None,
                lookup: None,
                picker: None,
            },
        ],
        baseline: Default::default(),
        mode: FormMode::Edit,
    };
    let pickers = vec![ResolvedPicker {
        owner_object_classes: vec!["inetOrgPerson".into()],
        binding: PickerBinding {
            attr: "memberOf".into(),
            scope: CandidateScope {
                base: "ou=groups,dc=x".into(),
                object_classes: vec!["groupOfNames".into()],
                search_attrs: vec!["cn".into()],
                label_template: None,
            },
            store: StoreKey::Dn,
            select: None,
            fanout_attr: Some("member".into()),
        },
    }];
    tag_picker_fields(&mut form, &pickers, &["inetOrgPerson".to_string()]);
    let f = &form.fields[0];
    assert!(f.picker.is_some(), "memberOf gets a picker binding");
    assert!(f.editable, "fan-out field forced editable despite operational read-only");
}

#[test]
fn value_editor_open_seeds_from_field_values_with_store_value_key() {
    use crate::config::relation::{CandidateScope, PickerBinding, StoreKey};
    let field = EditField {
        label: "gidNumber".into(),
        must: false,
        editable: true,
        multi: false,
        secret: false,
        ordered: false,
        values: vec!["1001".into()],
        kind: FieldKind::Text,
        widget: WidgetSpec::ReadOnlyText,
        editor: TextState::new().with_value("1001"),
        relation: None,
        lookup: None,
        picker: None,
    };
    let binding = PickerBinding {
        attr: "gidNumber".into(),
        scope: CandidateScope {
            base: "ou=groups,dc=x".into(),
            object_classes: vec!["posixGroup".into()],
            search_attrs: vec!["cn".into()],
            label_template: None,
        },
        store: StoreKey::Attr("gidNumber".into()),
        select: Some(crate::config::relation::Cardinality::Single),
        fanout_attr: None,
    };
    let ve = ValueEditor::open(0, &field, &binding);
    let p = ve.picker.as_ref().expect("picker present");
    // Seeded selection carries the current value as the store value (scalar).
    assert_eq!(p.selected.len(), 1);
    assert_eq!(p.selected[0].store_value, "1001");
    assert!(!p.key_ci, "scalar store → exact key compare");
    // The binding is carried on the editor for search/commit.
    assert_eq!(ve.binding.as_ref().unwrap().attr, "gidNumber");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p edaptor --lib edit_form -- tag_picker_fields value_editor_open_seeds`
Expected: FAIL — `picker` field, `tag_picker_fields`, `ValueEditor::open(.. ,binding)`, `ve.binding` not found.

- [ ] **Step 3: Add `picker` to `EditField` and `binding` to `ValueEditor`**

1. Add to `EditField` (after `lookup`, line 61):

```rust
    /// `Some` when this field is bound to a `[profile.picker.<attr>]` picker —
    /// the unified replacement for `relation`/`lookup`.
    pub picker: Option<crate::config::relation::PickerBinding>,
```

2. Add `picker: None` to every `EditField { … }` literal in the crate that doesn't set it: `build_edit_form` (line ~399, next to `lookup: None`), `inject_password_fields` `mk` (line ~448), and every `EditField { … }` in `edit_form.rs` tests, `app.rs` tests. Grep: `rg -n "relation: None," src/ui/edit_form.rs src/ui/app.rs` finds the literals to extend.

3. Add a `binding` field to the `ValueEditor` struct (next to `lookup`, ~line 120):

```rust
    /// The resolved picker binding driving this editor's search/commit (unified
    /// path). `None` for the plain free-text multi-value editor.
    pub binding: Option<crate::config::relation::PickerBinding>,
```

Set `binding: None` in the existing `open`/`open_picker`/`open_lookup` ctors (they are removed in Task 7; this just keeps them compiling).

- [ ] **Step 4: Add the unified `ValueEditor::open` and `tag_picker_fields`**

> The current `ValueEditor::open(field_idx, field)` (plain free-text, line 128) must be **renamed** to avoid a signature clash. Rename it to `open_plain` and update its one caller (`app.rs:776`, the `field.multi && field.editable` branch). Then add the unified picker `open`:

```rust
    /// Open the picker for a `[profile.picker.<attr>]`-bound field. Seeds the
    /// selection from the field's current values (each becomes a `Candidate`
    /// whose `store_value`/key is that value; `dn` equals the value, upgraded to
    /// the real entry DN when a search result matches the store value). Key
    /// comparison is case-insensitive iff `store = dn`.
    pub fn open(field_idx: usize, field: &EditField, binding: &PickerBinding) -> Self {
        let key_ci = matches!(binding.store, StoreKey::Dn);
        let selected: Vec<Candidate> = field
            .values
            .iter()
            .map(|v| Candidate {
                dn: v.clone(),         // placeholder for scalar stores; real DN for store=dn
                label: v.clone(),      // upgraded when a result matches (by store value)
                store_value: v.clone(),
            })
            .collect();
        ValueEditor {
            field: field_idx,
            label: field.label.clone(),
            ordered: field.ordered,
            secret: field.secret,
            rows: Vec::new(),
            sel: 0,
            scroll: 0,
            picker: Some(PickerState::new(selected, key_ci)),
            search: TextState::new(),
            scope: None,          // legacy fields, unused by the binding path
            role: None,
            lookup: None,
            base: String::new(),
            binding: Some(binding.clone()),
        }
    }
```

Add the imports at the top of `edit_form.rs`:

```rust
use crate::config::relation::{PickerBinding, StoreKey};
```

(Extend the existing `use crate::config::relation::{ … }` line 16.)

Then add the tagging pass (after `tag_lookup_fields`, ~line 470):

```rust
/// Tag each field whose attribute matches a resolved `[profile.picker.<attr>]`
/// binding for the entry's object classes. Fan-out fields (`fanout_attr` set) are
/// forced editable — their value is never written to the field itself, it fans
/// out. Non-fan-out fields keep their normal editable state and are tagged only
/// when already editable.
pub fn tag_picker_fields(
    form: &mut EditForm,
    pickers: &[crate::config::relation::ResolvedPicker],
    object_classes: &[String],
) {
    for field in &mut form.fields {
        let Some(binding) = crate::config::relation::picker_for(pickers, object_classes, &field.label)
        else {
            continue;
        };
        if binding.fanout_attr.is_some() {
            field.editable = true; // override operational read-only
            field.picker = Some(binding.clone());
        } else if field.editable {
            field.picker = Some(binding.clone());
        }
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p edaptor --lib edit_form -- tag_picker_fields value_editor_open_seeds`
Expected: PASS.

- [ ] **Step 6: Full check + commit**

```bash
cargo test -p edaptor && cargo clippy --all-targets -- -D warnings && cargo fmt --check
git add src/ui/edit_form.rs src/ui/app.rs
git commit -m "feat(edit_form): EditField.picker, unified ValueEditor::open, tag_picker_fields

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: app.rs cutover #1 — dispatch / search / results / Enter / overlay commit

**Files:**
- Modify: `src/ui/app.rs` — **only these functions/regions** (line numbers from the current map; re-grep before editing):
  - `App` struct fields (~189–237): add `pickers`
  - `run` init (~244, ~306): build/assign `pickers`
  - `open_value_editor` (~742–779): unified dispatch
  - `picker_editor_key` Alt+S commit (~806–859) and Enter (~861–879)
  - `service_picker_search` (~977–1042): binding-driven params
  - `Response::Entries` intercept (~421–460): store-value mapping + label upgrade

> **Subagent scope guard:** edit ONLY the functions above. Do not restructure the file. The old `App.relations`, `plan_combined_save`, and relation imports stay until Task 6/7.

> **Wiring correction (discovered during execution):** the rewritten `open_value_editor` reads ONLY `field.picker`. Nothing populates `field.picker` unless `tag_picker_fields` runs during form-building. So Task 5 MUST wire `tag_picker_fields` into BOTH form-building seams (keeping the old `relation`/`lookup` tagging in place — dual tagging — so Task 6's fan-out still works off `field.relation` until it's switched):
> - `build_loaded_form` (app.rs:1693) — the edit seam, called at app.rs:580 and app.rs:2117. Add a `pickers: &[ResolvedPicker]` param; after the password/lookup tagging, before `order_fields`, compute `let ocs = object_classes_of(&form);` (move it out of the `if !read_only` block or recompute) and call `crate::ui::edit_form::tag_picker_fields(&mut form, pickers, &ocs, read_only);`. Thread `&app.pickers` at both call sites (they already pass `&app.relations`).
> - `build_new_entry_form` (app.rs:2832) — the create seam, called by `open_create_form` (app.rs:2864, has `app` in scope). Add a `pickers: &[ResolvedPicker]` param; after `tag_lookup_fields`, before `order_fields`, compute `let ocs = object_classes_of(&form);` and call `tag_picker_fields(&mut form, pickers, &ocs, false)`. Pass `&app.pickers` from `open_create_form`. (NOTE: create forces editable fields to single-value first, so `member` becomes a single-select picker on create — acceptable; migrate any test that asserted a plain editor.)

- [ ] **Step 1: Add `App.pickers` and initialise it (compile-only, then build a test around behavior)**

1. Add to `App` struct (after `relations`, ~line 230):

```rust
    /// Resolved picker bindings (built once from config profiles). Supersedes
    /// `relations`/`lookups` for field population.
    pub pickers: Vec<crate::config::relation::ResolvedPicker>,
```

2. In `run` (near line 244 where `relations` is built):

```rust
    let pickers = crate::config::relation::resolve_pickers(&config.profiles);
```

and in the `App { … }` literal (~line 306) add `pickers,`.

3. Import: extend the relation `use` (app.rs:23) — add `resolve_pickers, picker_for, PickerBinding, StoreKey, Cardinality` as needed (some used in later steps).

- [ ] **Step 2: Write a render/behaviour test for the unified dispatch**

The existing app.rs tests construct `App` and drive keys. Add a test mirroring the existing membership-picker open test but asserting it works via `pickers` (search the file for an existing `open_value_editor`/picker test to copy the harness). Minimal example asserting dispatch opens a picker for a `field.picker` field:

```rust
#[test]
fn enter_on_picker_field_opens_picker_overlay() {
    // Build a form with one picker-bound multi field, focus it, press Enter.
    // (Reuse the test App constructor used by existing picker tests.)
    // Assert: app.overlay is Some(Overlay::ValueEditor(ve)) with ve.picker.is_some()
    //         and ve.binding.is_some().
    // See the existing membership-open test for the App/Structure scaffolding.
}
```

> Fill the body using the existing test scaffolding in `app.rs` (there are tests that build a `Structure` + `App` and call `open_value_editor`). Keep it concrete — no placeholder.

- [ ] **Step 3: Rewrite `open_value_editor` to the unified dispatch**

Replace the body (lines ~742–779) with:

```rust
fn open_value_editor(app: &mut App, structure: &Structure) {
    let focus = app.form_focus;
    let Some(form) = app.form.as_ref() else { return; };
    let Some(field) = form.fields.get(focus) else { return; };

    if let Some(binding) = field.picker.clone().filter(|_| field.editable) {
        let ve = ValueEditor::open(focus, field, &binding);
        app.overlay = Some(Overlay::ValueEditor(ve));
        app.picker_last_query = PICKER_INIT_QUERY.to_string();
        app.picker_search_id = None;
    } else if field.multi && field.editable {
        let ve = ValueEditor::open_plain(focus, field);
        app.overlay = Some(Overlay::ValueEditor(ve));
    }
}
```

(The seeding label-of for DN candidates is no longer needed at open time — labels upgrade from search results by store value, Step 5.)

- [ ] **Step 4: Rewrite `service_picker_search` param resolution (lines ~977–1042)**

Replace the lookup-vs-membership param branch with one binding-driven builder:

```rust
fn service_picker_search(app: &mut App, worker: &WorkerHandle) {
    let Some(Overlay::ValueEditor(ve)) = app.overlay.as_ref() else { return; };
    let Some(binding) = ve.binding.as_ref() else { return; };
    if ve.picker.is_none() { return; }

    let query = ve.search.value().to_string();
    if query == app.picker_last_query { return; }
    app.picker_last_query = query.clone();

    let scope = &binding.scope;
    let filter = crate::ui::picker::build_member_filter(
        &scope.object_classes,
        &scope.search_attrs,
        &query,
    );
    // Request the store attribute (when scalar) + label-template attrs + cn.
    let mut attrs: Vec<String> = scope
        .label_template
        .as_deref()
        .map(crate::config::label::template_attrs)
        .unwrap_or_default();
    attrs.push("cn".to_string());
    if let crate::config::relation::StoreKey::Attr(a) = &binding.store {
        attrs.push(a.clone());
    }
    dedupe_ci(&mut attrs);

    let id = next_id();
    app.picker_search_id = Some(id);
    let _ = worker.submit(Request::Search {
        id,
        base: scope.base.clone(),
        scope: SearchScope::Subtree,
        filter,
        attrs,
        size_limit: Some(PICKER_SEARCH_CAP),
    });
}
```

- [ ] **Step 5: Rewrite the `Response::Entries` intercept (lines ~421–460) — store-value mapping + key-based label upgrade**

```rust
Response::Entries { id, entries, .. } if app.picker_search_id == Some(*id) => {
    if let Some(Overlay::ValueEditor(ve)) = app.overlay.as_mut() {
        let binding = ve.binding.clone();
        if let (Some(binding), Some(p)) = (binding, ve.picker.as_mut()) {
            let label_template = binding.scope.label_template.clone();
            let results: Vec<crate::ui::picker::Candidate> = entries
                .iter()
                .filter_map(|e| {
                    let store_value = match &binding.store {
                        crate::config::relation::StoreKey::Dn => e.dn.clone(),
                        crate::config::relation::StoreKey::Attr(a) => {
                            crate::ui::picker::pick_value(&e.attrs, a)?
                        }
                    };
                    Some(crate::ui::picker::Candidate {
                        dn: e.dn.clone(),
                        label: membership_candidate_label(label_template.as_deref(), &e.dn, &e.attrs),
                        store_value,
                    })
                })
                .collect();
            // Upgrade seeded selection labels (and real DNs for scalar stores)
            // by matching on the STORE VALUE, not the DN.
            let ci = p.key_ci;
            for sel in p.selected.iter_mut() {
                if let Some(r) = results.iter().find(|r| {
                    if ci { r.store_value.eq_ignore_ascii_case(&sel.store_value) }
                    else { r.store_value == sel.store_value }
                }) {
                    sel.label = r.label.clone();
                    sel.dn = r.dn.clone(); // upgrade placeholder DN for scalar stores
                }
            }
            p.set_results(results);
        }
    }
}
```

> `pick_value` returning `None` (hit lacks the store attr) drops the candidate via `filter_map` — matches the spec ("hits lacking the store value are skipped").
> `membership_candidate_label` already exists (label from template/cn/dn). `candidate_label_for_lookup` is no longer used here — leave it for Task 7 deletion.

- [ ] **Step 6: Rewrite the Enter single-vs-toggle branch (lines ~861–879)**

Cardinality from the binding (or schema arity). Compute `single` from `ve.binding`:

```rust
KeyCode::Enter => {
    if let Some(Overlay::ValueEditor(ve)) = app.overlay.as_mut() {
        let single = match ve.binding.as_ref().and_then(|b| b.select) {
            Some(crate::config::relation::Cardinality::Single) => true,
            Some(crate::config::relation::Cardinality::Multi) => false,
            None => {
                // "auto": derive from the field's schema arity.
                app.form
                    .as_ref()
                    .and_then(|f| f.fields.get(ve.field))
                    .map(|f| !f.multi)
                    .unwrap_or(false)
            }
        };
        if let Some(p) = ve.picker.as_mut() {
            if single {
                let chosen = p.visible().get(p.cursor).map(|row| row.candidate.clone());
                if let Some(c) = chosen {
                    p.selected = vec![c];
                }
            } else {
                p.toggle_cursor();
            }
        }
    }
}
```

- [ ] **Step 7: Rewrite the overlay Alt+S commit (lines ~806–859)**

Single unified commit: branch on `fanout_attr`. For both, write `field.values` from the selected store values (the fan-out field's in-memory values then drive Task 6's diff; it is excluded from the server changeset there). Single → exactly one (also seed `editor`).

```rust
KeyCode::Char('s') | KeyCode::Char('S') if alt => {
    if let Some(Overlay::ValueEditor(ve)) = app.overlay.take() {
        if let (Some(binding), Some(picker)) = (ve.binding.as_ref(), ve.picker.as_ref()) {
            let single = matches!(binding.select, Some(crate::config::relation::Cardinality::Single))
                || (binding.select.is_none()
                    && app.form.as_ref().and_then(|f| f.fields.get(ve.field)).map(|f| !f.multi).unwrap_or(false));
            let values = picker.selected_values();
            if let Some(field) = app.form.as_mut().and_then(|f| f.fields.get_mut(ve.field)) {
                if single {
                    let v = values.into_iter().next().unwrap_or_default();
                    field.editor = TextState::new().with_value(v.clone());
                    field.values = if v.is_empty() { vec![] } else { vec![v] };
                } else {
                    field.values = values;
                }
            }
        }
        app.picker_search_id = None;
        app.picker_last_query.clear();
    }
}
```

- [ ] **Step 8: Run the full suite + checks**

Run: `cargo test -p edaptor && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: PASS. (`App.relations`/`plan_combined_save` still compile via the old path; the form-save fan-out still uses `backref_lookup` until Task 6 — membership *save* may still route through the relation path here, which is fine: this commit only cuts over the picker *overlay* interaction.)

> If a test that drove the old lookup/relation overlay path now fails because the overlay reads `ve.binding`, update that test to set up a `field.picker` binding (the behaviour is equivalent). Do not weaken assertions.

- [ ] **Step 9: Commit**

```bash
git add src/ui/app.rs
git commit -m "refactor(app): drive picker open/search/results/commit from PickerBinding

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: app.rs cutover #2 — form-save fan-out via `fanout_attr`

**Files:**
- Modify: `src/ui/app.rs` — `plan_combined_save` (~1948–2052) and its call sites (~1134, ~1462)
- Modify: `src/ui/edit_form.rs` — `backref_labels` (~293–307) and `to_edit_entry` (~270–289)

Switch the own-entry exclusion + fan-out from `backref_lookup`/`RelationRole::BackRef` to the picker binding's `fanout_attr`. The field carrying a `picker` with `fanout_attr = Some(X)` is excluded from the own-entry diff and fans out X on each picked candidate.

- [ ] **Step 1: Write the failing tests**

`backref_labels` should now key on `field.picker.fanout_attr`. Add to `edit_form.rs` tests:

```rust
#[test]
fn fanout_labels_come_from_picker_binding() {
    use crate::config::relation::{CandidateScope, PickerBinding, StoreKey};
    let mut form = EditForm {
        dn: "uid=bob,ou=people,dc=x".into(),
        fields: vec![EditField {
            label: "memberOf".into(),
            must: false, editable: true, multi: true, secret: false, ordered: false,
            values: vec![], kind: FieldKind::Dn, widget: WidgetSpec::ReadOnlyText,
            editor: TextState::new(), relation: None, lookup: None,
            picker: Some(PickerBinding {
                attr: "memberOf".into(),
                scope: CandidateScope { base: "".into(), object_classes: vec![], search_attrs: vec![], label_template: None },
                store: StoreKey::Dn,
                select: None,
                fanout_attr: Some("member".into()),
            }),
        }],
        baseline: Default::default(),
        mode: FormMode::Edit,
    };
    assert_eq!(form.fanout_labels(), vec!["memberOf".to_string()]);
    // Non-fanout picker field is NOT a fanout label.
    form.fields[0].picker.as_mut().unwrap().fanout_attr = None;
    assert!(form.fanout_labels().is_empty());
}
```

Add an app.rs unit test for the fan-out planning (reuse the existing `membership_fanout`/`plan_combined_save` test scaffolding — search for `membership_fanout` tests) asserting that a picker-bound `memberOf` change produces the right per-group `member` Add/Delete ops via the binding.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p edaptor -- fanout_labels_come_from_picker_binding`
Expected: FAIL — `fanout_labels` not found.

- [ ] **Step 3: Add `fanout_labels` + per-field fan-out attr to `edit_form.rs`**

Add a method returning (label, fanout_attr) and a label-only convenience. Keep `backref_labels` until Task 7 (it still compiles). Add:

```rust
    /// Labels of fields whose picker binding fans out (excluded from the own-entry
    /// diff; their change drives the per-candidate fan-out save).
    pub fn fanout_labels(&self) -> Vec<String> {
        self.fields
            .iter()
            .filter(|f| f.picker.as_ref().and_then(|b| b.fanout_attr.as_ref()).is_some())
            .map(|f| f.label.clone())
            .collect()
    }

    /// `(field label, fanout attr)` for each fan-out field — the attr to modify on
    /// each picked candidate.
    pub fn fanout_bindings(&self) -> Vec<(String, String)> {
        self.fields
            .iter()
            .filter_map(|f| {
                let b = f.picker.as_ref()?;
                let attr = b.fanout_attr.as_ref()?;
                Some((f.label.clone(), attr.clone()))
            })
            .collect()
    }
```

Update `to_edit_entry` (lines 270–289) to exclude fan-out picker fields instead of (or in addition to — keep both until Task 7) BackRef:

```rust
            .filter(|f| {
                let is_backref_relation = matches!(
                    &f.relation,
                    Some(FieldRelation { role: RelationRole::BackRef, .. })
                );
                let is_fanout_picker =
                    f.picker.as_ref().and_then(|b| b.fanout_attr.as_ref()).is_some();
                !is_backref_relation && !is_fanout_picker
            })
```

- [ ] **Step 4: Rewrite `plan_combined_save` fan-out (app.rs ~1948–2052)**

Replace the `backref` detection + the `backref_lookup`-driven fan-out loop with the picker bindings:

- Change `let backref = form.backref_labels();` → `let fanout = form.fanout_labels();` and the early-return + "changed" detection to use `fanout`.
- In the fan-out generation loop, replace:

```rust
        for f in form.fields.iter().filter(|f| fanout.contains(&f.label)) {
            let Some(attr) = f
                .picker
                .as_ref()
                .and_then(|b| b.fanout_attr.clone())
            else {
                continue;
            };
            let base = form.baseline.get(&f.label).cloned().unwrap_or_default();
            let ops = membership_fanout(&form.dn, &base, &f.current_values(), &attr);
            for (gdn, op) in ops {
                fanout_ops.push((gdn, op));
            }
        }
```

(rename the local `fanout: Vec<(String, ModOp)>` accumulator to `fanout_ops` to avoid the name clash with the labels `fanout`.)

- `plan_combined_save` no longer needs the `relations: &[ResolvedRelation]` param — change its signature to drop it (and drop it at the two call sites ~1134, ~1462 and in `combined_save_overlay`). `profiles` is still passed (used for password staging).

- [ ] **Step 5: Run tests + checks**

Run: `cargo test -p edaptor && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: PASS. Live membership behaviour unchanged (verified by gated tests in Task 9).

- [ ] **Step 6: Commit**

```bash
git add src/ui/app.rs src/ui/edit_form.rs
git commit -m "refactor(app): fan-out save keyed on picker binding fanout_attr

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Clean cut — delete `[[relation]]` + `[profile.lookup]` machinery

**Files:**
- Modify: `src/config/mod.rs`, `src/config/relation.rs`, `src/ui/edit_form.rs`, `src/ui/app.rs`

Delete the superseded types **and their tests** in one commit (the dead-code rule: an old `pub` type stays live only while its tests reference it, so delete both together).

- [ ] **Step 1: Delete config types**

In `src/config/mod.rs`:
- Remove `LookupSpec` struct (lines 74–92) and the `lookups` field on `EntryProfile` (lines 119–123).
- Remove `Config.relations` field (lines 28–30) and the `use relation::Relation;` (line 8).

In `src/config/relation.rs`:
- Remove `Relation`, `RelationRole`, `ResolvedRelation`, `resolve_relations`, `holder_lookup`, `backref_lookup` and **their tests** (`parses_relation_block`, `fixture`, `resolves_both_directions_with_correct_scopes`, `holder_lookup_matches_holder_oc_and_attr`, `backref_lookup_matches_candidate_oc_and_back_attr`, `candidate_scope_carries_parsed_label_template`* , `scope_search_attrs_gain_label_template_attrs_not_already_listed`*, `unknown_template_is_dropped`).
  - *Keep the label-template / search-attrs coverage by re-pointing those two tests at `resolve_pickers` (they exercise `scope_of`, which survives). Rewrite them to build a `PickerSpec` instead of a `Relation` (mirror the Task 2 tests). Do not lose `scope_of` label-template coverage.
- Keep: `CandidateScope`, `scope_of`, `has_oc`, and all Task 2 picker types/tests. Update the module doc comment (lines 1–3) to describe pickers.
- Update the test `fn profile(...)` literal: it currently sets `lookups: Default::default(),` — remove that line (the field is gone) and keep `pickers: Default::default(),`.

- [ ] **Step 2: Delete edit_form types/fns**

In `src/ui/edit_form.rs`:
- Remove `EditField.relation` and `EditField.lookup` fields (lines 56–61) and every `relation: None,` / `lookup: None,` literal across the crate (grep `rg -n "relation: None|lookup: None"`).
- Remove `FieldRelation` struct (lines 24–30).
- Remove `ValueEditor` legacy fields `scope`, `role`, `lookup`, `base` and the `open_picker`/`open_lookup` constructors (lines 153–212). Keep `open_plain` and the unified `open`.
- Remove `tag_lookup_fields` (lines 457+) and its test.
- Remove `backref_labels` (now `fanout_labels`) and the `RelationRole::BackRef` arm in `to_edit_entry` (leave only the `is_fanout_picker` check).
- Fix `build_edit_form`: drop the `relations` param and the `holder_lookup`/`backref_lookup` tagging block (lines 366–385); set `relation`/`lookup` no longer exist. The editable computation reverts to `let editable = !read_only && field_is_editable(f);`. Picker tagging now happens via `tag_picker_fields` called by the form-build caller.
- Update the relation `use` (line 16) to import only surviving items (`CandidateScope`, `PickerBinding`, `StoreKey`, `picker_for`, `ResolvedPicker`, `Cardinality` as needed).
- Update `field_is_relevant`/`field_is_editable` references to `f.relation` (line 483) → `f.picker.is_some()`.
- Fix the tests that built `FieldRelation` fields (lines 599+, 799+, 841+, 957+) — convert to `picker` bindings or delete if redundant with Task 4/6 tests.

- [ ] **Step 3: Delete app.rs relation usage**

In `src/ui/app.rs`:
- Remove `App.relations` field (line 230), its build (`resolve_relations`, line 244), and the `relations,` in the `App { … }` literal (line 306).
- Update the `use crate::config::relation::{ … }` (line 23) to drop `backref_lookup, resolve_relations, ResolvedRelation` and keep `resolve_pickers, picker_for, PickerBinding, StoreKey, Cardinality, ResolvedPicker`.
- `build_edit_form(...)` call sites: drop the `relations` arg; add a `tag_picker_fields(&mut form, &app.pickers, &object_classes)` call right after each `build_edit_form` (the caller must compute `object_classes` from the form's objectClass field — mirror what `build_edit_form` did internally). Search for `build_edit_form(` call sites and update each.
- Delete `candidate_label_for_lookup` (no longer referenced) and any `effective_search_attrs` helper that only served lookups (verify with `rg`).
- Fix any remaining test imports of `CandidateScope`/`RelationRole`/`FieldRelation` (map noted at lines 3484, 3523, 4043, 4103, 4129, 4202) — convert to picker equivalents or delete.

- [ ] **Step 4: Compile, iterate on the long tail**

Run: `cargo build --all-targets 2>&1 | head -50`
Fix each reference the compiler flags (this is the bulk of the task — work through them mechanically). Re-run until clean.

- [ ] **Step 5: Full check**

Run: `cargo test -p edaptor && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: all green. Verify the facade boundary is intact:
`! grep -rl "use ratatui\|use tui_" src | grep -v "^src/ui/"`

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor: remove [[relation]] + [profile.lookup] machinery (superseded by picker)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Rewrite demo config + README

**Files:**
- Modify: `examples/demo-config.toml`, `README.md`

- [ ] **Step 1: Rewrite `examples/demo-config.toml`**

Replace the `[profile.lookup.gidNumber]` block (lines 40–45) and the `[[relation]]` block (lines 55–60) with `[profile.picker.*]` declarations, and add a `posixgroup` profile so `memberUid` becomes a multi-select user picker. The `user` profile gains `member` is on `group`; `memberOf` (fan-out) and `gidNumber` (scalar) on `user`; `memberUid` on `posixgroup`:

```toml
# user profile — gidNumber resolves a posixGroup (scalar store), memberOf fans
# out to group.member (synthetic back-ref).
[profile.picker.gidNumber]
candidate = "posixgroup"
store     = "gidNumber"
select    = "single"

[profile.picker.memberOf]
candidate   = "group"
store       = "dn"
fanout_attr = "member"

# group profile — member is a multi-select DN picker over users.
[profile.picker.member]
candidate = "user"

# posixgroup profile (posixGroup) — memberUid stores each picked user's uid.
[[profile]]
name           = "posixgroup"
object_classes = ["posixGroup"]
rdn_attr       = "cn"
search_base    = "ou=groups,dc=example,dc=org"
show           = ["cn", "gidNumber", "memberUid", "description"]
label          = "{cn}"

[profile.picker.memberUid]
candidate = "user"
store     = "uid"
```

> Place each `[profile.picker.*]` block under the right `[[profile]]` (TOML sub-tables attach to the most recent `[[profile]]`). `gidNumber`/`memberOf` under `user`; `member` under `group`; `memberUid` under `posixgroup`. Verify by parsing (Step 3).

- [ ] **Step 2: Update the README config example**

In `README.md`, replace the `[[relation]]`/`[profile.lookup]` example in the `## Configuration` section with the `[profile.picker.<attr>]` shape (mirror the demo config); document the four knobs (`candidate`, `store`, `select`, `fanout_attr`) in one short table. (The stale "Status"/"Turbo Vision" blurb is a separate known gap — leave it unless trivially adjacent.)

- [ ] **Step 3: Validate the config parses + round-trips**

Run a quick parse check (a throwaway test or `cargo run -- --config examples/demo-config.toml` against a running test server is overkill here — prefer a unit test):

Add a test in `src/config/mod.rs`:

```rust
#[test]
fn demo_config_parses_with_pickers() {
    let toml = include_str!("../../examples/demo-config.toml");
    let cfg: Config = toml::from_str(toml).expect("demo config parses");
    let pickers = crate::config::relation::resolve_pickers(&cfg.profiles);
    // member, memberOf, gidNumber (user/group) + memberUid (posixgroup) = 4.
    assert_eq!(pickers.len(), 4);
}
```

Run: `cargo test -p edaptor demo_config_parses_with_pickers`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add examples/demo-config.toml README.md src/config/mod.rs
git commit -m "docs(config): rewrite demo config + README for [profile.picker.<attr>]

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: Live (gated) tests for the four binding shapes

**Files:**
- Modify: `tests/live_membership.rs`, `tests/live_templates.rs`

Live tests are gated by `EDAPTOR_TEST_LDAP_URI` (skip when unset). DN base `dc=example,dc=org`. Bring up the server: `scripts/test-ldap.sh start`; `export EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389`; `export EDAPTOR_TEST_ADMIN_PW=adminpassword`.

- [ ] **Step 1: Extend `tests/live_membership.rs`**

Add (or adapt the existing member round-trip test to) assertions that exercise the binding path end-to-end against the server:

- `member` (group, store=dn, multi): pick two users → group's `member` holds both DNs.
- `memberOf` (user, fanout_attr=member): tick a group on a user → that group's `member` gains the user DN; untick → removed. Last-member removal is blocked.

Mirror the structure of the existing membership live test (`rg -n "fn " tests/live_membership.rs` to find the pattern + helpers). Gate with the same `EDAPTOR_TEST_LDAP_URI` guard the file already uses.

- [ ] **Step 2: Extend `tests/live_templates.rs`**

- `gidNumber` (user, store=gidNumber, single): single-pick a posixGroup → the user's `gidNumber` is the chosen group's `gidNumber` scalar (not a DN).
- `memberUid` (posixgroup, store=uid, multi): multi-pick users → the posixGroup's `memberUid` holds the picked users' `uid`s.

- [ ] **Step 3: Run the gated suite**

```bash
scripts/test-ldap.sh start
export EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389
export EDAPTOR_TEST_ADMIN_PW=adminpassword
cargo test -p edaptor
scripts/test-ldap.sh stop
```
Expected: all live tests pass (member/memberOf/gidNumber/memberUid). Without the env var the same `cargo test -p edaptor` SKIPs them and the lib suite stays green.

- [ ] **Step 4: Commit**

```bash
git add tests/live_membership.rs tests/live_templates.rs
git commit -m "test(live): cover unified picker (member/memberOf/gidNumber/memberUid)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Final verification

- [ ] `cargo build --all-targets` — clean
- [ ] `cargo test -p edaptor` — all lib tests pass; live SKIP without env var
- [ ] `cargo clippy --all-targets -- -D warnings` — clean
- [ ] `cargo fmt --check` — clean
- [ ] Facade boundary: `! grep -rl "use ratatui\|use tui_" src | grep -v "^src/ui/"` — no output
- [ ] Gated live run (server up): member / memberOf fan-out / gidNumber scalar / memberUid multi-scalar all round-trip
- [ ] Manual TUI smoke (`cargo run -- --config examples/demo-config.toml`): open a group → Enter on `member` → search/toggle → Alt+S; a user → Enter on `gidNumber` → single-pick; a user → Enter on `memberOf` → toggle group → save fans out; a posixGroup → Enter on `memberUid` → multi-pick users.
- [ ] Update `docs/HANDOVER.md`: move "Unified configurable picker" to ✅, replace the "Architecture: the picker today" section with the unified design.

---

## Spec-coverage self-check

| Spec requirement | Task |
|---|---|
| `[profile.picker.<attr>]` parse (`candidate`/`store`/`select`/`fanout_attr`, defaults) | 1 |
| `PickerSpec` → `PickerBinding`, `StoreKey`, `Cardinality`, `resolve_pickers`, unknown-candidate drop | 2 |
| `PickerState` keyed by store value; `Candidate.store_value`; DN ci vs scalar exact | 3 |
| Scalar-store label upgrade by store key (review blind spot) | 3 (state) + 5 (intercept) |
| `EditField.picker`; one `ValueEditor::open`; fan-out force-editable; binding tagging | 4 |
| One open dispatch / search-param builder / Enter handler / overlay commit | 5 |
| Commit branches on `fanout_attr`; direct single (replace) / multi (set) / fan-out diff | 6 |
| `memberOf` editable despite operational read-only | 4 (tag) + 6 (exclude from own diff) |
| Remove `[[relation]]`/`[profile.lookup]` types + config (clean cut) | 7 |
| Demo config + `posixgroup` profile + README | 8 |
| Live: member / gidNumber / memberUid / memberUid | 9 |
| Out of scope: server schema, picker popup UI, reorder semantics, password/default features | (untouched) |
