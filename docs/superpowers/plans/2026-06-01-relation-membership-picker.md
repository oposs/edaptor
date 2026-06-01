# Relation Membership Picker — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Strict TDD per task: write a failing test → run it to confirm the failure → implement → run `cargo test` + `cargo clippy --all-targets -- -D warnings` + `cargo fmt` → commit. **The crate MUST compile after every task's commit.** Commit with:
> ```
> git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -m "$(printf '<subject>\n\nCo-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>')"
> ```
> Preserve the facade boundary: no module other than `src/ui/*` may `use ratatui`/`use tui_*`. Live tests are gated by `EDAPTOR_TEST_LDAP_URI` and SKIP when unset.

**Goal:** Add symmetric group↔user membership editing by giving the existing multi-value popup (`ValueEditor`) a **picker mode** — a live, size-capped, searchable candidate list with always-visible current selection — driven by a single `[[relation]]` config block, editing a group's `member` directly (forward) and fanning `member` MODIFYs across affected groups when edited from a user's `memberOf` (reverse).

**Architecture:** Five phases. P1–P2 are pure/headless (config resolution, picker state machine). P3 is a one-field worker tweak. P4 wires the **holder side** (group's `member`) end-to-end through the existing single-entry save — it ships on its own. P5 adds the **back-reference side** (user's `memberOf`): a relation role on the field, exclusion from the single-entry diff on *both* sides, and a **synchronous** combined save (own-entry MODIFY + per-group fan-out MODIFYs) with last-member pre-validation, a combined LDIF preview, and a partial-failure report. Synchronous fan-out mirrors the existing `refresh_structure`/startup pattern (`worker.request(...)` blocks on its own reply channel, independent of the async poll channel).

**Tech Stack:** Rust 2021; ratatui 0.30; tui-prompts 0.6 (`TextState`); ldap3 0.12.1 (`SearchOptions::new().sizelimit(i32)` + `LdapConn::with_search_options` — verified in the worktree). Pure logic unit-tested; UI smoke-only; live paths gated by `EDAPTOR_TEST_LDAP_URI`.

**Source-of-truth references (read before coding):**
- Spec: `docs/superpowers/specs/2026-06-01-relation-membership-picker-design.md`
- Existing value editor: `src/ui/edit_form.rs` (`ValueEditor`, `EditField`, `build_edit_form`)
- Existing save path: `src/ui/app.rs` (`handle_action` FormSave ~709, `prepare_save` ~1195, `submit_prepared` ~1224, `execute_pending` ~1022, `handle_worker_response` ~363, `value_editor_key` ~634, `open_value_editor` ~618, `reconcile` ~1102)
- Worker protocol: `src/ldap/worker.rs` (`Request::Search` ~78, `run_search` ~551)
- Changeset/LDIF: `src/form/changeset.rs` (`ModOp` ~47, `ChangeSet` ~86, `diff` ~164), `src/ldap/ldif.rs` (`render_changeset`)

---

## File Structure

- **Create** `src/config/relation.rs` — `Relation` (raw TOML), `CandidateScope`, `RelationRole`, `ResolvedRelation`, `resolve_relations`, `holder_lookup`, `backref_lookup`. Pure.
- **Modify** `src/config/mod.rs` — add `pub mod relation;`, a `relations: Vec<Relation>` field on `Config`, and a `search_attrs` field + `search_attributes()` method on `EntryProfile`.
- **Create** `src/ui/picker.rs` — `Candidate`, `PickerState`, `VisibleRow`, the selection/search/toggle state machine, plus `build_member_filter` / `escape_filter` / `candidate_label`. Pure (no ratatui).
- **Modify** `src/ui/edit_form.rs` — `FieldRelation` on `EditField`; picker fields on `ValueEditor`; `ValueEditor::open_picker`; `EditForm::backref_labels`; thread `relations` into `build_edit_form`; force BackRef fields editable.
- **Modify** `src/ui/view.rs` — picker branch in `render_value_editor`.
- **Modify** `src/ui/app.rs` — `App.relations` / `App.picker_search_id` / `App.picker_last_query`; picker open + key handling; `service_picker_search`; picker-`Entries` interception; `PendingAction::CombinedSave` + synchronous executor; `membership_fanout` + `would_empty` wiring.
- **Modify** `src/ldap/worker.rs` — `size_limit: Option<i32>` on `Request::Search`; apply it in `run_search`.
- **Modify** `src/ldap/ldif.rs` — `render_changesets(&[ChangeSet]) -> String`.
- **Modify** `src/lib.rs`, `src/workflows/read_flow.rs` — add `size_limit: None` to existing `Request::Search` constructors.
- **Test** `tests/live_membership.rs` — gated integration (forward + reverse + last-member).

---

## Phase 1 — Config: the `[[relation]]` block (pure)

### Task 1.1: `EntryProfile` search attributes

**Files:**
- Modify: `src/config/mod.rs:51-61` (`EntryProfile`)

- [ ] **Step 1: Write the failing test** — append to the `tests` module in `src/config/mod.rs`:

```rust
#[test]
fn search_attributes_falls_back_to_show_then_cn() {
    let p = EntryProfile { name: "u".into(), object_class: "inetOrgPerson".into(),
        rdn_attr: "uid".into(), search_base: "ou=people".into(),
        show: vec!["uid".into(), "cn".into()], search_attrs: vec![] };
    assert_eq!(p.search_attributes(), vec!["uid".to_string(), "cn".to_string()]);

    let p2 = EntryProfile { search_attrs: vec!["mail".into()], ..p.clone() };
    assert_eq!(p2.search_attributes(), vec!["mail".to_string()]);

    let p3 = EntryProfile { show: vec![], search_attrs: vec![], ..p };
    assert_eq!(p3.search_attributes(), vec!["cn".to_string()]);
}
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p edaptor search_attributes_falls_back` → FAIL (no field `search_attrs`, no method).

- [ ] **Step 3: Implement** — add the field to `EntryProfile` (after `show`) and the method:

```rust
    #[serde(default)]
    pub show: Vec<String>,
    /// Attributes the picker substring-search matches on. Falls back to `show`,
    /// then to `["cn"]` (see [`EntryProfile::search_attributes`]).
    #[serde(default)]
    pub search_attrs: Vec<String>,
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
```

- [ ] **Step 4: Run to verify it passes** — `cargo test -p edaptor search_attributes_falls_back` → PASS.

- [ ] **Step 5: Commit**

```bash
git add src/config/mod.rs
git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -m "$(printf 'feat(config): EntryProfile::search_attributes with show/cn fallback\n\nCo-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>')"
```

### Task 1.2: `Relation` parsing + `Config.relations`

**Files:**
- Create: `src/config/relation.rs`
- Modify: `src/config/mod.rs` (add `pub mod relation;` near other `pub mod`; add field to `Config`)
- Test: in `src/config/relation.rs`

- [ ] **Step 1: Write the failing test** — create `src/config/relation.rs` with ONLY the test (types come in Step 3):

```rust
//! Membership relations: one `[[relation]]` declares both ends of a symmetric
//! holder↔candidate link (e.g. group.member ↔ user.memberOf). Pure; resolved
//! against the configured [`EntryProfile`]s into directional [`ResolvedRelation`]s.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn parses_relation_block() {
        let cfg: Config = toml::from_str(
            r#"
            [server]
            uri = "ldaps://x"
            base_dn = "dc=x"
            [auth]
            [[relation]]
            name = "group-membership"
            holder = "group"
            holder_attr = "member"
            candidate = "user"
            back_attr = "memberOf"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.relations.len(), 1);
        assert_eq!(cfg.relations[0].holder_attr, "member");
        assert_eq!(cfg.relations[0].back_attr, "memberOf");
    }
}
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p edaptor parses_relation_block` → FAIL (no `relations` field, no module).

- [ ] **Step 3: Implement** — at the TOP of `src/config/relation.rs` (above the test module) add:

```rust
use serde::Deserialize;

/// A symmetric membership relation as declared in `[[relation]]`. Template names
/// (`holder`, `candidate`) reference `[[profile]]` `name`s.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
pub struct Relation {
    pub name: String,
    /// Template whose entry OWNS the link attribute.
    pub holder: String,
    /// The real, writable attribute on the holder (e.g. `member`).
    pub holder_attr: String,
    /// Template that scopes the picker's candidate search (e.g. `user`).
    pub candidate: String,
    /// Virtual back-reference field shown on the candidate side (e.g. `memberOf`).
    pub back_attr: String,
}
```

In `src/config/mod.rs`: add `pub mod relation;` beside `pub mod password;`, then `use relation::Relation;` and add to `Config` (after `profiles`):

```rust
    /// Membership relations (`[[relation]]`); empty when absent.
    #[serde(default, rename = "relation")]
    pub relations: Vec<Relation>,
```

- [ ] **Step 4: Run to verify it passes** — `cargo test -p edaptor parses_relation_block` → PASS.

- [ ] **Step 5: Commit**

```bash
git add src/config/mod.rs src/config/relation.rs
git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -m "$(printf 'feat(config): [[relation]] block parsing\n\nCo-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>')"
```

### Task 1.3: Resolve relations + directional lookups

**Files:**
- Modify: `src/config/relation.rs`

- [ ] **Step 1: Write the failing test** — add to the `tests` module in `src/config/relation.rs`:

```rust
    fn profile(name: &str, oc: &str, base: &str, search: &[&str]) -> crate::config::EntryProfile {
        crate::config::EntryProfile {
            name: name.into(), object_class: oc.into(), rdn_attr: "x".into(),
            search_base: base.into(), show: vec![],
            search_attrs: search.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn fixture() -> Vec<ResolvedRelation> {
        let profiles = vec![
            profile("group", "groupOfNames", "ou=groups,dc=x", &["cn"]),
            profile("user", "inetOrgPerson", "ou=people,dc=x", &["uid", "cn"]),
        ];
        let rels = vec![Relation {
            name: "m".into(), holder: "group".into(), holder_attr: "member".into(),
            candidate: "user".into(), back_attr: "memberOf".into(),
        }];
        resolve_relations(&profiles, &rels)
    }

    #[test]
    fn resolves_both_directions_with_correct_scopes() {
        let r = fixture();
        assert_eq!(r.len(), 1);
        // Holder side (editing group.member) searches CANDIDATES = users.
        assert_eq!(r[0].candidate_scope.base, "ou=people,dc=x");
        assert_eq!(r[0].candidate_scope.object_class, "inetOrgPerson");
        // Back-ref side (editing user.memberOf) searches HOLDERS = groups.
        assert_eq!(r[0].holder_scope.base, "ou=groups,dc=x");
        assert_eq!(r[0].holder_scope.object_class, "groupOfNames");
    }

    #[test]
    fn holder_lookup_matches_holder_oc_and_attr() {
        let r = fixture();
        let ocs = vec!["top".to_string(), "groupOfNames".to_string()];
        // group's `member` → Holder, candidate scope = users.
        let h = holder_lookup(&r, &ocs, "member").unwrap();
        assert_eq!(h.candidate_scope.object_class, "inetOrgPerson");
        // a user's `member` is NOT a holder match (wrong objectClass).
        assert!(holder_lookup(&r, &["inetOrgPerson".to_string()], "member").is_none());
    }

    #[test]
    fn backref_lookup_matches_candidate_oc_and_back_attr() {
        let r = fixture();
        let ocs = vec!["inetOrgPerson".to_string()];
        let b = backref_lookup(&r, &ocs, "memberOf").unwrap();
        assert_eq!(b.holder_scope.object_class, "groupOfNames"); // searches groups
        assert!(backref_lookup(&r, &["groupOfNames".to_string()], "memberOf").is_none());
    }

    #[test]
    fn unknown_template_is_dropped() {
        let profiles = vec![profile("user", "inetOrgPerson", "ou=people", &["uid"])];
        let rels = vec![Relation { name: "m".into(), holder: "group".into(),
            holder_attr: "member".into(), candidate: "user".into(), back_attr: "memberOf".into() }];
        assert!(resolve_relations(&profiles, &rels).is_empty()); // `group` profile missing
    }
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p edaptor -- config::relation` → FAIL (types/fns missing).

- [ ] **Step 3: Implement** — add below `Relation` in `src/config/relation.rs`:

```rust
use crate::config::EntryProfile;

/// Which side of a relation a field plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationRole {
    /// The entry owns the link attribute (e.g. group.member) — written directly.
    Holder,
    /// A virtual back-reference (e.g. user.memberOf) — writes fan out to holders.
    BackRef,
}

/// The scope for a live candidate search: where to look and what to match on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateScope {
    pub base: String,
    pub object_class: String,
    pub search_attrs: Vec<String>,
}

/// A relation resolved against the configured profiles: the concrete objectClass
/// for each end plus the search scope used from each direction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRelation {
    pub name: String,
    pub holder_oc: String,
    pub holder_attr: String,
    pub candidate_oc: String,
    pub back_attr: String,
    /// Used on the HOLDER form (editing `holder_attr`) — searches candidates.
    pub candidate_scope: CandidateScope,
    /// Used on the CANDIDATE form (editing `back_attr`) — searches holders.
    pub holder_scope: CandidateScope,
}

fn scope_of(p: &EntryProfile) -> CandidateScope {
    CandidateScope {
        base: p.search_base.clone(),
        object_class: p.object_class.clone(),
        search_attrs: p.search_attributes(),
    }
}

/// Resolve each `[[relation]]` against `profiles`. Relations referencing an
/// unknown template are dropped (caller may warn).
pub fn resolve_relations(profiles: &[EntryProfile], relations: &[Relation]) -> Vec<ResolvedRelation> {
    let find = |name: &str| profiles.iter().find(|p| p.name == name);
    relations
        .iter()
        .filter_map(|r| {
            let holder = find(&r.holder)?;
            let candidate = find(&r.candidate)?;
            Some(ResolvedRelation {
                name: r.name.clone(),
                holder_oc: holder.object_class.clone(),
                holder_attr: r.holder_attr.clone(),
                candidate_oc: candidate.object_class.clone(),
                back_attr: r.back_attr.clone(),
                candidate_scope: scope_of(candidate),
                holder_scope: scope_of(holder),
            })
        })
        .collect()
}

fn has_oc(ocs: &[String], oc: &str) -> bool {
    ocs.iter().any(|o| o.eq_ignore_ascii_case(oc))
}

/// The relation where `(ocs, attr)` is the HOLDER side (e.g. group.member).
pub fn holder_lookup<'a>(rels: &'a [ResolvedRelation], ocs: &[String], attr: &str) -> Option<&'a ResolvedRelation> {
    rels.iter().find(|r| has_oc(ocs, &r.holder_oc) && r.holder_attr.eq_ignore_ascii_case(attr))
}

/// The relation where `(ocs, attr)` is the BACK-REF side (e.g. user.memberOf).
pub fn backref_lookup<'a>(rels: &'a [ResolvedRelation], ocs: &[String], attr: &str) -> Option<&'a ResolvedRelation> {
    rels.iter().find(|r| has_oc(ocs, &r.candidate_oc) && r.back_attr.eq_ignore_ascii_case(attr))
}
```

- [ ] **Step 4: Run to verify it passes** — `cargo test -p edaptor -- config::relation` → PASS. Then `cargo clippy --all-targets -- -D warnings` and `cargo fmt`.

- [ ] **Step 5: Commit**

```bash
git add src/config/relation.rs
git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -m "$(printf 'feat(config): resolve relations into directional scopes + lookups\n\nCo-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>')"
```

---

## Phase 2 — Pure picker state machine

### Task 2.1: `PickerState` selection / visible / toggle

**Files:**
- Create: `src/ui/picker.rs`
- Modify: `src/ui/mod.rs` (add `pub mod picker;`)

- [ ] **Step 1: Write the failing test** — create `src/ui/picker.rs` with the test only:

```rust
//! Pure state for the value-editor's picker mode: a current selection that is
//! ALWAYS shown, merged with the latest (size-capped) search results. No ratatui.

#[cfg(test)]
mod tests {
    use super::*;

    fn c(dn: &str) -> Candidate { Candidate { dn: dn.into(), label: dn.into() } }

    #[test]
    fn selected_stays_visible_when_results_exclude_it() {
        // Seed selection = [A]; a search returns only [B] (A does not match).
        let mut p = PickerState::new(vec![c("A")]);
        p.set_results(vec![c("B")]);
        let dns: Vec<_> = p.visible().iter().map(|r| r.candidate.dn.clone()).collect();
        assert_eq!(dns, vec!["A".to_string(), "B".to_string()]); // A still present
        assert!(p.visible()[0].selected);
        assert!(!p.visible()[1].selected);
    }

    #[test]
    fn results_already_selected_are_not_duplicated() {
        let mut p = PickerState::new(vec![c("A")]);
        p.set_results(vec![c("A"), c("B")]);
        let dns: Vec<_> = p.visible().iter().map(|r| r.candidate.dn.clone()).collect();
        assert_eq!(dns, vec!["A".to_string(), "B".to_string()]);
    }

    #[test]
    fn toggle_adds_and_removes() {
        let mut p = PickerState::new(vec![]);
        p.set_results(vec![c("A"), c("B")]);
        p.cursor = 0; // A
        p.toggle_cursor();
        assert_eq!(p.selected_dns(), vec!["A".to_string()]);
        // A now sorts into the selected block at index 0; toggle it off again.
        p.cursor = 0;
        p.toggle_cursor();
        assert!(p.selected_dns().is_empty());
    }

    #[test]
    fn cursor_clamps() {
        let mut p = PickerState::new(vec![c("A")]);
        p.move_cursor(5);
        assert_eq!(p.cursor, 0); // only one visible row
        p.move_cursor(-5);
        assert_eq!(p.cursor, 0);
    }
}
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p edaptor -- ui::picker` → FAIL.

- [ ] **Step 3: Implement** — add above the test module:

```rust
/// One candidate entry: the DN that is stored, and the human label that is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub dn: String,
    pub label: String,
}

/// A row as displayed in the picker: a candidate plus whether it is selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleRow {
    pub candidate: Candidate,
    pub selected: bool,
}

/// Picker state: the current selection (always shown) and the latest results.
#[derive(Debug, Clone, Default)]
pub struct PickerState {
    pub selected: Vec<Candidate>,
    pub results: Vec<Candidate>,
    pub cursor: usize,
}

fn same_dn(a: &str, b: &str) -> bool { a.eq_ignore_ascii_case(b) }

impl PickerState {
    pub fn new(selected: Vec<Candidate>) -> Self {
        PickerState { selected, results: Vec::new(), cursor: 0 }
    }

    /// Visible rows: every selected candidate (marked), then each result not
    /// already selected (unmarked). This is why a selected entry never vanishes
    /// while filtering — selection is independent of the search results.
    pub fn visible(&self) -> Vec<VisibleRow> {
        let mut rows: Vec<VisibleRow> = self
            .selected
            .iter()
            .map(|c| VisibleRow { candidate: c.clone(), selected: true })
            .collect();
        for r in &self.results {
            if !self.selected.iter().any(|s| same_dn(&s.dn, &r.dn)) {
                rows.push(VisibleRow { candidate: r.clone(), selected: false });
            }
        }
        rows
    }

    pub fn set_results(&mut self, results: Vec<Candidate>) {
        self.results = results;
        let n = self.visible().len();
        if self.cursor >= n {
            self.cursor = n.saturating_sub(1);
        }
    }

    pub fn move_cursor(&mut self, delta: i32) {
        let n = self.visible().len();
        if n == 0 {
            self.cursor = 0;
            return;
        }
        let next = (self.cursor as i32 + delta).clamp(0, n as i32 - 1);
        self.cursor = next as usize;
    }

    /// Toggle the cursor row's membership in the selection.
    pub fn toggle_cursor(&mut self) {
        let rows = self.visible();
        let Some(row) = rows.get(self.cursor) else { return };
        let dn = row.candidate.dn.clone();
        if let Some(pos) = self.selected.iter().position(|s| same_dn(&s.dn, &dn)) {
            self.selected.remove(pos);
        } else {
            self.selected.push(row.candidate.clone());
        }
        let n = self.visible().len();
        if self.cursor >= n {
            self.cursor = n.saturating_sub(1);
        }
    }

    pub fn selected_dns(&self) -> Vec<String> {
        self.selected.iter().map(|c| c.dn.clone()).collect()
    }
}
```

Add `pub mod picker;` to `src/ui/mod.rs`.

- [ ] **Step 4: Run to verify it passes** — `cargo test -p edaptor -- ui::picker` → PASS.

- [ ] **Step 5: Commit**

```bash
git add src/ui/picker.rs src/ui/mod.rs
git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -m "$(printf 'feat(ui): pure PickerState (selection always visible, toggle, cursor)\n\nCo-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>')"
```

### Task 2.2: Filter builder, escaping, and candidate label

**Files:**
- Modify: `src/ui/picker.rs`

- [ ] **Step 1: Write the failing test** — add to the `tests` module:

```rust
    #[test]
    fn escapes_filter_specials() {
        assert_eq!(escape_filter("a*b(c)\\d"), r"a\2ab\28c\29\5cd");
    }

    #[test]
    fn builds_or_filter_with_objectclass_and_term() {
        let f = build_member_filter("inetOrgPerson", &["uid".into(), "cn".into()], "ann");
        assert_eq!(f, "(&(objectClass=inetOrgPerson)(|(uid=*ann*)(cn=*ann*)))");
    }

    #[test]
    fn empty_term_filters_objectclass_only() {
        let f = build_member_filter("groupOfNames", &["cn".into()], "");
        assert_eq!(f, "(objectClass=groupOfNames)");
    }

    #[test]
    fn label_prefers_cn_then_dn() {
        use std::collections::BTreeMap;
        let mut attrs = BTreeMap::new();
        attrs.insert("cn".to_string(), vec!["Ann Smith".to_string()]);
        assert_eq!(candidate_label("uid=ann,ou=people", &attrs), "Ann Smith");
        assert_eq!(candidate_label("uid=bob,ou=people", &BTreeMap::new()), "uid=bob,ou=people");
    }
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p edaptor -- ui::picker` → FAIL.

- [ ] **Step 3: Implement** — add to `src/ui/picker.rs` (above the tests):

```rust
use std::collections::BTreeMap;

/// RFC 4515 filter-value escaping for the four special bytes.
pub fn escape_filter(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '*' => out.push_str(r"\2a"),
            '(' => out.push_str(r"\28"),
            ')' => out.push_str(r"\29"),
            '\\' => out.push_str(r"\5c"),
            '\0' => out.push_str(r"\00"),
            _ => out.push(ch),
        }
    }
    out
}

/// Build the candidate search filter. Empty `term` → objectClass only; otherwise
/// AND the objectClass with an OR of `attr=*term*` over each search attribute.
pub fn build_member_filter(object_class: &str, search_attrs: &[String], term: &str) -> String {
    let oc = format!("(objectClass={})", object_class);
    if term.is_empty() {
        return oc;
    }
    let esc = escape_filter(term);
    let ors: String = search_attrs.iter().map(|a| format!("({a}=*{esc}*)")).collect();
    format!("(&{oc}(|{ors}))")
}

/// A candidate's display label: first `cn` value, else the raw DN.
pub fn candidate_label(dn: &str, attrs: &BTreeMap<String, Vec<String>>) -> String {
    attrs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("cn"))
        .and_then(|(_, v)| v.first().cloned())
        .unwrap_or_else(|| dn.to_string())
}
```

- [ ] **Step 4: Run to verify it passes** — `cargo test -p edaptor -- ui::picker` → PASS. Then clippy + fmt.

- [ ] **Step 5: Commit**

```bash
git add src/ui/picker.rs
git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -m "$(printf 'feat(ui): picker filter builder, RFC4515 escape, candidate label\n\nCo-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>')"
```

---

## Phase 3 — Worker: size-limited search

### Task 3.1: `size_limit` on `Request::Search`

**Files:**
- Modify: `src/ldap/worker.rs` (`Request::Search` ~78; the match arm ~356; `run_search` ~551)
- Modify: `src/lib.rs:168`, `src/lib.rs:217`, `src/workflows/read_flow.rs:65` (constructors)

- [ ] **Step 1: Write the failing test** — add to the `tests` module in `src/ldap/worker.rs` (gated like the other live tests; if there is no live test in this file, add a pure compile-guard test instead):

```rust
#[test]
fn search_request_has_size_limit_field() {
    // Compile-level guarantee that the field exists and defaults are explicit.
    let r = Request::Search {
        id: 1, base: "dc=x".into(), scope: SearchScope::OneLevel,
        filter: "(objectClass=*)".into(), attrs: vec!["cn".into()], size_limit: Some(20),
    };
    match r { Request::Search { size_limit, .. } => assert_eq!(size_limit, Some(20)), _ => panic!() }
}
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p edaptor search_request_has_size_limit_field` → FAIL (no field `size_limit`).

- [ ] **Step 3: Implement**

In `Request::Search` add the field:

```rust
        /// Attributes to request (`"*"` for all user attributes).
        attrs: Vec<String>,
        /// Optional server-side size limit (picker type-ahead caps at ~20).
        size_limit: Option<i32>,
    },
```

In the worker match arm (~356), pass it to `run_search`:

```rust
            Request::Search { id, base, scope, filter, attrs, size_limit } => {
                let resp = match run_search(conn, &base, scope, &filter, attrs, size_limit) {
                    Ok(entries) => Response::Entries { id, entries },
                    Err(e) => Response::SearchError { id, msg: e.to_string() },
                };
                // ...existing send...
            }
```

Update `run_search` (~551) — add `import` `use ldap3::SearchOptions;` at the top of the file, then:

```rust
fn run_search(
    conn: &mut LdapConn,
    base: &str,
    scope: SearchScope,
    filter: &str,
    attrs: Vec<String>,
    size_limit: Option<i32>,
) -> Result<Vec<LdapEntry>> {
    if let Some(n) = size_limit {
        // `with_search_options` applies to the next search on this conn.
        conn.with_search_options(SearchOptions::new().sizelimit(n));
    }
    let (entries, _res) = conn
        .search(base, scope_to_ldap3(scope), filter, attrs)?
        .success()
        .with_context(|| format!("searching {base}"))?;
    Ok(entries
        .into_iter()
        .map(SearchEntry::construct)
        .map(to_ldap_entry)
        .collect())
}
```

> **[verify-at-task-start]** Confirm `SearchOptions::new().sizelimit(i32)` and `conn.with_search_options(opts)` against the worktree's ldap3 0.12.1 (`~/.cargo/registry/src/index.crates.io-*/ldap3-0.12.1/src/{search,sync}.rs` — both verified present during planning). If the API differs, fall back to a client-side truncate: `let mut v = run_search(...)?; v.truncate(n as usize);`.

Add `size_limit: None` to the three existing constructors: `src/lib.rs:168`, `src/lib.rs:217`, `src/workflows/read_flow.rs:65`.

- [ ] **Step 4: Run to verify it passes** — `cargo test -p edaptor search_request_has_size_limit_field` → PASS; then `cargo build` (catches the three constructor sites) → clean.

- [ ] **Step 5: Commit**

```bash
git add src/ldap/worker.rs src/lib.rs src/workflows/read_flow.rs
git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -m "$(printf 'feat(ldap): optional size_limit on Search (picker type-ahead cap)\n\nCo-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>')"
```

---

## Phase 4 — Wire the holder side end-to-end (ships on its own)

This phase makes editing a **group's `member`** open the picker, search live, and save through the existing single-entry path. Shared infra (relations on the form, picker open, the tick-based search service, picker-`Entries` interception, render) is introduced here.

### Task 4.1: `FieldRelation` on `EditField`; thread `relations` into `build_edit_form`

**Files:**
- Modify: `src/ui/edit_form.rs` (`EditField`, `build_edit_form` ~182, signature + callers)
- Modify: `src/ui/app.rs:454`, `src/ui/app.rs:752` (callers)

- [ ] **Step 1: Write the failing test** — add to `src/ui/edit_form.rs` tests:

```rust
    #[test]
    fn member_field_on_group_gets_holder_relation() {
        use crate::config::relation::{resolve_relations, Relation};
        use crate::config::EntryProfile;
        let profiles = vec![
            EntryProfile { name: "group".into(), object_class: "groupOfNames".into(),
                rdn_attr: "cn".into(), search_base: "ou=groups".into(), show: vec![], search_attrs: vec!["cn".into()] },
            EntryProfile { name: "user".into(), object_class: "inetOrgPerson".into(),
                rdn_attr: "uid".into(), search_base: "ou=people".into(), show: vec![], search_attrs: vec!["uid".into()] },
        ];
        let rels = resolve_relations(&profiles, &[Relation { name: "m".into(),
            holder: "group".into(), holder_attr: "member".into(),
            candidate: "user".into(), back_attr: "memberOf".into() }]);
        // A form for a group entry: objectClass=groupOfNames, fields include `member`.
        let model = group_model_with_member(); // helper below
        let form = build_edit_form(&model, &schema_with_member(), false, &rels);
        let f = form.fields.iter().find(|f| f.label == "member").unwrap();
        let rel = f.relation.as_ref().expect("member is a relation field");
        assert!(matches!(rel.role, crate::config::relation::RelationRole::Holder));
        assert_eq!(rel.scope.object_class, "inetOrgPerson"); // searches users
    }
```

Add the two small helpers near the existing test `schema()` helper (mirror its style): `group_model_with_member()` returns a `FormModel` whose `title` is a group DN, whose `fields` include a `member` field (`FormField` with `label="member"`, `kind`/`widget` as for a DN-list — reuse the pattern at `edit_form.rs:248-274`), and whose object-class source yields `groupOfNames`; `schema_with_member()` returns a `SchemaModel` parsed from a minimal subschema where `member` is multi-valued. (Follow the existing `schema()` test helper at `edit_form.rs:246`.)

- [ ] **Step 2: Run to verify it fails** — `cargo test -p edaptor member_field_on_group_gets_holder_relation` → FAIL.

- [ ] **Step 3: Implement**

Add the struct + field. In `src/ui/edit_form.rs`:

```rust
use crate::config::relation::{backref_lookup, holder_lookup, CandidateScope, RelationRole, ResolvedRelation};

/// Relation metadata attached to a picker-enabled field.
#[derive(Clone)]
pub struct FieldRelation {
    pub role: RelationRole,
    /// Scope for the candidate search opened from THIS field.
    pub scope: CandidateScope,
}
```

Add to `EditField` (after `editor`):

```rust
    /// `Some` when this field is a membership relation (opens the picker).
    pub relation: Option<FieldRelation>,
```

Change `build_edit_form` signature to take `relations: &[ResolvedRelation]` and set the field. The entry's objectClasses come from the `objectClass` field values in the model (same source `object_classes_of` uses). Compute them once:

```rust
pub fn build_edit_form(
    model: &FormModel,
    schema: &SchemaModel,
    read_only: bool,
    relations: &[ResolvedRelation],
) -> EditForm {
    let object_classes: Vec<String> = model
        .fields
        .iter()
        .find(|f| f.label.eq_ignore_ascii_case("objectClass"))
        .map(|f| f.values.clone())
        .unwrap_or_default();

    let fields: Vec<EditField> = model
        .fields
        .iter()
        .map(|f| {
            let relation = holder_lookup(relations, &object_classes, &f.label)
                .map(|r| FieldRelation { role: RelationRole::Holder, scope: r.candidate_scope.clone() })
                .or_else(|| backref_lookup(relations, &object_classes, &f.label)
                    .map(|r| FieldRelation { role: RelationRole::BackRef, scope: r.holder_scope.clone() }));
            // BackRef fields (e.g. memberOf) are normally non-editable; the picker
            // makes them editable. (P5 wires the fan-out save.)
            let editable = match &relation {
                Some(FieldRelation { role: RelationRole::BackRef, .. }) => !read_only,
                _ => !read_only && field_is_editable(f),
            };
            let seed = f.values.first().cloned().unwrap_or_default();
            EditField {
                label: f.label.clone(),
                must: f.is_must,
                editable,
                multi: !schema.is_single_value(&f.label),
                secret: is_secret_attr(&f.label),
                ordered: is_x_ordered(&f.label),
                values: f.values.clone(),
                kind: f.kind,
                widget: f.widget.clone(),
                editor: TextState::new().with_value(seed),
                relation,
            }
        })
        .collect();
    // ...baseline + EditForm construction unchanged...
}
```

Update callers:
- `src/ui/edit_form.rs` test calls (4): add `&[]` as the 4th arg.
- `src/ui/app.rs:454`: `build_edit_form(&model, read_flow.schema(), app.read_only, &app.relations)`.
- `src/ui/app.rs:752` (create): `build_edit_form(&model, read_flow.schema(), false, &[])` (membership-on-create is out of scope; see spec §9).

(`App.relations` is added in Task 4.2; if implementing strictly task-by-task, temporarily pass `&[]` at line 454 and switch to `&app.relations` in 4.2.)

- [ ] **Step 4: Run to verify it passes** — `cargo test -p edaptor member_field_on_group_gets_holder_relation` → PASS; `cargo build` clean.

- [ ] **Step 5: Commit**

```bash
git add src/ui/edit_form.rs src/ui/app.rs
git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -m "$(printf 'feat(ui): attach FieldRelation to picker-enabled form fields\n\nCo-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>')"
```

### Task 4.2: `App` picker state + resolve relations at startup

**Files:**
- Modify: `src/ui/app.rs` (`App` struct ~152; `run` ~211 where the App is built; the `config.profiles.clone()` area ~214)

- [ ] **Step 1:** No new unit test (struct wiring). Verification is `cargo build`.

- [ ] **Step 2: Implement** — add to `App` (after `menu_defs`):

```rust
    /// Resolved membership relations (built once from config).
    pub relations: Vec<ResolvedRelation>,
    /// Correlation id of the latest in-flight picker search (stale ids ignored).
    pub picker_search_id: Option<u64>,
    /// The picker search term last submitted (delta detection in the loop).
    pub picker_last_query: String,
```

Add `use crate::config::relation::{resolve_relations, ResolvedRelation, RelationRole};` at the top.

In `run` (~212), after `let profiles = config.profiles.clone();` add:

```rust
    let relations = resolve_relations(&config.profiles, &config.relations);
```

Pass `relations` into the `App { .. }` constructor (set the three new fields: `relations`, `picker_search_id: None`, `picker_last_query: String::new()`). Switch the line-454 `build_edit_form` call to `&app.relations`.

- [ ] **Step 3: Run** — `cargo build` → clean.

- [ ] **Step 4: Commit**

```bash
git add src/ui/app.rs
git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -m "$(printf 'feat(ui): hold resolved relations + picker search id on App\n\nCo-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>')"
```

### Task 4.3: `ValueEditor` picker fields + open in picker mode

**Files:**
- Modify: `src/ui/edit_form.rs` (`ValueEditor` ~73; new `open_picker`)
- Modify: `src/ui/app.rs` (`open_value_editor` ~618 and its caller ~594)

- [ ] **Step 1: Write the failing test** — add to `src/ui/edit_form.rs` tests:

```rust
    #[test]
    fn open_picker_seeds_selection_from_field_values() {
        use crate::config::relation::{CandidateScope, RelationRole};
        let scope = CandidateScope { base: "ou=people".into(), object_class: "inetOrgPerson".into(),
            search_attrs: vec!["uid".into()] };
        let mut field = EditField {
            label: "member".into(), must: false, editable: true, multi: true, secret: false,
            ordered: false, values: vec!["uid=a,ou=people".into(), "uid=b,ou=people".into()],
            kind: FieldKind::Text, widget: WidgetSpec::ReadOnlyText,
            editor: TextState::new(),
            relation: Some(FieldRelation { role: RelationRole::Holder, scope: scope.clone() }),
        };
        // labels resolved via a closure (DN→label); here identity.
        let ve = ValueEditor::open_picker(0, &mut field, |dn| dn.to_string());
        let picker = ve.picker.expect("picker mode");
        assert_eq!(picker.selected_dns(), vec!["uid=a,ou=people".to_string(), "uid=b,ou=people".to_string()]);
        assert_eq!(ve.scope.unwrap().object_class, "inetOrgPerson");
    }
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p edaptor open_picker_seeds_selection` → FAIL.

- [ ] **Step 3: Implement** — add picker fields to `ValueEditor`:

```rust
use crate::ui::picker::{Candidate, PickerState};
use crate::config::relation::{CandidateScope, RelationRole};

pub struct ValueEditor {
    pub field: usize,
    pub label: String,
    pub ordered: bool,
    pub secret: bool,
    pub rows: Vec<TextState<'static>>,
    pub sel: usize,
    /// `Some` in picker mode (relation fields); `None` for the free-text editor.
    pub picker: Option<PickerState>,
    /// The picker's incremental-search box (Unicode-correct edit engine).
    pub search: TextState<'static>,
    /// Candidate search scope (picker mode only).
    pub scope: Option<CandidateScope>,
    /// The relation role being edited (picker mode only).
    pub role: Option<RelationRole>,
}
```

Update the existing `ValueEditor::open` (~90) to set the new fields to defaults (`picker: None, search: TextState::new(), scope: None, role: None`). Add the picker constructor:

```rust
impl ValueEditor {
    /// Open in PICKER mode over a relation `field`. `label_of` resolves a DN to a
    /// display label (caller passes a lookup over the loaded structure).
    pub fn open_picker(
        field_idx: usize,
        field: &EditField,
        label_of: impl Fn(&str) -> String,
    ) -> Self {
        let rel = field.relation.as_ref().expect("open_picker on a relation field");
        let selected: Vec<Candidate> = field
            .values
            .iter()
            .map(|dn| Candidate { dn: dn.clone(), label: label_of(dn) })
            .collect();
        ValueEditor {
            field: field_idx,
            label: field.label.clone(),
            ordered: field.ordered,
            secret: field.secret,
            rows: Vec::new(),
            sel: 0,
            picker: Some(PickerState::new(selected)),
            search: TextState::new(),
            scope: Some(rel.scope.clone()),
            role: Some(rel.role),
        }
    }
}
```

In `src/ui/app.rs`, change `open_value_editor` to branch on the relation and to receive the structure (for labels). Update its caller at ~594.

```rust
fn open_value_editor(app: &mut App, structure: &Structure) {
    let focus = app.form_focus;
    let Some(form) = app.form.as_ref() else { return };
    let Some(field) = form.fields.get(focus) else { return };
    if field.relation.is_some() && field.editable {
        // Picker mode: label DNs from the loaded structure (fallback = the DN).
        let label_of = |dn: &str| {
            structure.get(dn).map(|n| n.label.clone()).unwrap_or_else(|| dn.to_string())
        };
        let ve = ValueEditor::open_picker(focus, field, label_of);
        app.overlay = Some(Overlay::ValueEditor(ve));
        app.picker_last_query.clear();
        app.picker_search_id = None;
    } else if field.multi && field.editable {
        app.overlay = Some(Overlay::ValueEditor(ValueEditor::open(focus, field)));
    }
}
```

> **[verify-at-task-start]** Confirm `Structure::get(dn)` returns a node exposing a `label` (see `src/workflows/structure.rs`). If the label accessor differs (e.g. `node.label()` or a `(label, dn)` row), adapt `label_of`. The fallback to the raw DN keeps this correct regardless.

The caller at ~594 (`KeyCode::Enter => open_value_editor(app)`) becomes `open_value_editor(app, structure)`. Ensure `structure` is in scope there; the key handler is reached from the event loop which owns `structure`. Thread `structure: &Structure` through the form-key handler that contains line 594 if not already present.

- [ ] **Step 4: Run to verify it passes** — `cargo test -p edaptor open_picker_seeds_selection` → PASS; `cargo build` clean.

- [ ] **Step 5: Commit**

```bash
git add src/ui/edit_form.rs src/ui/app.rs
git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -m "$(printf 'feat(ui): open ValueEditor in picker mode for relation fields\n\nCo-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>')"
```

### Task 4.4: Picker key handling + tick-based candidate search + `Entries` interception

**Files:**
- Modify: `src/ui/app.rs` (`value_editor_key` ~634; the event loop in `run` where `reconcile` is called; `handle_worker_response` ~363)

- [ ] **Step 1: Write the failing test** — add to `src/ui/app.rs` tests (mirror the existing `ValueEditor` test at ~1412):

```rust
    #[test]
    fn picker_space_toggles_and_f2_commits_dns() {
        use crate::ui::picker::{Candidate, PickerState};
        use crate::config::relation::{CandidateScope, RelationRole};
        let mut app = test_app_with_form_field_member(); // helper: form whose field 0 is `member`, multi, editable, Holder relation
        let mut ve = /* open_picker over field 0 with empty selection */ make_picker_ve(0);
        ve.picker.as_mut().unwrap().set_results(vec![Candidate { dn: "uid=a,ou=people".into(), label: "a".into() }]);
        app.overlay = Some(Overlay::ValueEditor(ve));
        // Space toggles the cursor row (a) into the selection.
        value_editor_key(&mut app, key(KeyCode::Char(' ')));
        // F2 commits the selected DNs into the field.
        value_editor_key(&mut app, key(KeyCode::F(2)));
        let f = &app.form.as_ref().unwrap().fields[0];
        assert_eq!(f.values, vec!["uid=a,ou=people".to_string()]);
        assert!(app.overlay.is_none());
    }
```

Add the test helpers `test_app_with_form_field_member()` and `make_picker_ve(idx)` near the existing app test helpers (reuse whatever `App`/`EditForm` constructor the existing test at ~1412 uses; build a one-field `member` form with a `Holder` `FieldRelation`). `key(code)` mirrors the existing test key constructor.

- [ ] **Step 2: Run to verify it fails** — `cargo test -p edaptor picker_space_toggles_and_f2_commits_dns` → FAIL.

- [ ] **Step 3: Implement**

In `value_editor_key` (~634), branch to picker handling when the overlay is in picker mode. At the very top of the function:

```rust
    // Picker mode has its own key map (search box + selection toggle).
    if matches!(&app.overlay, Some(Overlay::ValueEditor(ve)) if ve.picker.is_some()) {
        picker_editor_key(app, key);
        return;
    }
```

Add the picker key handler:

```rust
/// Keys inside the picker: Esc/F3 cancel; F2 commit selected DNs to the field;
/// ↑↓ move; Space toggle; any other key edits the search box (the tick-based
/// `service_picker_search` turns a changed query into a live candidate search).
fn picker_editor_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc | KeyCode::F(3) => {
            app.overlay = None;
            app.picker_search_id = None;
        }
        KeyCode::F(2) => {
            if let Some(Overlay::ValueEditor(ve)) = app.overlay.take() {
                if let Some(picker) = &ve.picker {
                    if let Some(field) = app.form.as_mut().and_then(|f| f.fields.get_mut(ve.field)) {
                        field.values = picker.selected_dns();
                    }
                }
            }
            app.picker_search_id = None;
        }
        KeyCode::Up => if let Some(Overlay::ValueEditor(ve)) = app.overlay.as_mut() {
            if let Some(p) = ve.picker.as_mut() { p.move_cursor(-1); }
        },
        KeyCode::Down => if let Some(Overlay::ValueEditor(ve)) = app.overlay.as_mut() {
            if let Some(p) = ve.picker.as_mut() { p.move_cursor(1); }
        },
        KeyCode::Char(' ') => if let Some(Overlay::ValueEditor(ve)) = app.overlay.as_mut() {
            if let Some(p) = ve.picker.as_mut() { p.toggle_cursor(); }
        },
        _ => if let Some(Overlay::ValueEditor(ve)) = app.overlay.as_mut() {
            ve.search.handle_key_event(key);
        },
    }
}
```

> Note: Space is reserved for toggle, so a literal space cannot be typed into the search term. That is acceptable — search terms for `cn`/`uid` do not need spaces, and substring match still works on each word.

Add the tick-based search service and call it from the event loop right after the existing `reconcile(...)` call in `run`:

```rust
/// When a picker is open and its search term changed, submit a fresh size-capped
/// candidate search (stale ids are discarded in `handle_worker_response`). Empty
/// term → clear results (selection-only view). Mirrors the leaf incremental search.
fn service_picker_search(app: &mut App, worker: &WorkerHandle) {
    let Some(Overlay::ValueEditor(ve)) = app.overlay.as_ref() else { return };
    if ve.picker.is_none() { return; }
    let query = ve.search.value().to_string();
    if query == app.picker_last_query { return; }
    app.picker_last_query = query.clone();
    let Some(scope) = ve.scope.clone() else { return };

    if query.is_empty() {
        if let Some(Overlay::ValueEditor(ve)) = app.overlay.as_mut() {
            if let Some(p) = ve.picker.as_mut() { p.set_results(Vec::new()); }
        }
        app.picker_search_id = None;
        return;
    }
    let id = next_id();
    app.picker_search_id = Some(id);
    let filter = crate::ui::picker::build_member_filter(&scope.object_class, &scope.search_attrs, &query);
    let _ = worker.submit(Request::Search {
        id,
        base: scope.base,
        scope: SearchScope::Subtree,
        filter,
        attrs: vec!["cn".to_string()],
        size_limit: Some(20),
    });
}
```

In `handle_worker_response` (~363), intercept picker results BEFORE the read-flow routing. Add as the first arm inside the `match resp`:

```rust
        Response::Entries { id, entries } if app.picker_search_id == Some(*id) => {
            if let Some(Overlay::ValueEditor(ve)) = app.overlay.as_mut() {
                if let Some(p) = ve.picker.as_mut() {
                    let results = entries.iter().map(|e| crate::ui::picker::Candidate {
                        dn: e.dn.clone(),
                        label: crate::ui::picker::candidate_label(&e.dn, &e.attrs),
                    }).collect();
                    p.set_results(results);
                }
            }
            return;
        }
```

- [ ] **Step 4: Run to verify it passes** — `cargo test -p edaptor picker_space_toggles_and_f2_commits_dns` → PASS; `cargo build` clean.

- [ ] **Step 5: Commit**

```bash
git add src/ui/app.rs
git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -m "$(printf 'feat(ui): picker keys, tick-based candidate search, Entries intercept\n\nCo-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>')"
```

### Task 4.5: Render the picker overlay

**Files:**
- Modify: `src/ui/view.rs` (`render_value_editor` ~396)

- [ ] **Step 1:** No unit test (rendering). Verify via `cargo build` + a tmux smoke run (manual; see Task 5.5 verification).

- [ ] **Step 2: Implement** — at the top of `render_value_editor`, branch when `ve.picker.is_some()` to a picker layout: a one-line search box (show `ve.search.value()`), then the visible rows from `ve.picker.visible()`, each `"[x] <label>"` / `"[ ] <label>"`, the row at `picker.cursor` highlighted, plus a footer hint `"Space toggle · F2 save · Esc cancel · type to search (cap 20)"`. Mirror the existing overlay framing (`centered(...)`, `pane_block(...)`) used by the row-mode branch. Keep DNs dimmed/secondary only if space allows; label is primary.

```rust
fn render_value_editor(f: &mut Frame, ve: &ValueEditor, area: Rect) {
    if let Some(picker) = &ve.picker {
        let rect = centered(70, 20, area);
        f.render_widget(Clear, rect);
        let block = pane_block(&format!(" {} ", ve.label), true);
        // search line + rows; highlight picker.cursor; footer hint.
        // ... build Vec<Line> from picker.visible(): "[x]"/"[ ]" + candidate.label ...
        // (follow the row-mode branch's Paragraph/List construction below)
        return;
    }
    // ...existing row-mode rendering unchanged...
}
```

- [ ] **Step 3: Run** — `cargo build` → clean; `cargo clippy --all-targets -- -D warnings`; `cargo fmt`.

- [ ] **Step 4: Commit**

```bash
git add src/ui/view.rs
git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -m "$(printf 'feat(ui): render picker overlay (search + marked candidate rows)\n\nCo-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>')"
```

### Task 4.6: Live integration — forward (group.member) edit

**Files:**
- Create: `tests/live_membership.rs`

- [ ] **Step 1: Write the test** (gated; SKIPs when `EDAPTOR_TEST_LDAP_URI` unset — mirror `tests/live_write.rs` setup):

```rust
// Gated live test: edit a group's `member` and confirm the write round-trips.
#[test]
fn forward_member_edit_round_trips() {
    let Some(uri) = std::env::var("EDAPTOR_TEST_LDAP_URI").ok() else { return };
    // 1. bind; ensure a group + two users exist (seed like live_write.rs).
    // 2. Modify group: set member = [userA, userB] via Request::Modify(Replace).
    // 3. Search the group (Base, attrs=["member"]); assert both DNs present.
    let _ = uri;
}
```

Fill in the body following the exact bind/seed/teardown pattern in `tests/live_write.rs` (reuse its helper for connecting via `WorkerHandle` and its DN constants).

- [ ] **Step 2: Run** — `cargo test -p edaptor --test live_membership` (SKIPs without the env var → PASS-as-skip). With a seeded podman slapd + `EDAPTOR_TEST_LDAP_URI` set, it exercises the real write.

- [ ] **Step 3: Commit**

```bash
git add tests/live_membership.rs
git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -m "$(printf 'test(live): forward group.member edit round-trips (gated)\n\nCo-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>')"
```

**P4 ships here:** editing a group's `member` opens the picker, searches users live (cap 20), keeps the current members pinned, and saves via the existing single-entry path. The reverse direction is P5.

---

## Phase 5 — Back-reference side: synchronous fan-out save

### Task 5.1: Exclude BackRef fields from the single-entry diff (BOTH sides)

**Files:**
- Modify: `src/ui/edit_form.rs` (`EditForm::to_edit_entry` ~134; add `backref_labels`)

This is the sharpest correctness point (advisor): if `edited` drops `memberOf` but `original` (from `baseline`) keeps it, `diff` emits a spurious `Delete memberOf` against the user. Strip from **both**.

- [ ] **Step 1: Write the failing test** — add to `src/ui/edit_form.rs` tests:

```rust
    #[test]
    fn backref_field_excluded_from_own_entry_diff() {
        use crate::form::changeset::{diff, EditEntry};
        // Build a user form with a BackRef `memberOf` field whose selection changed.
        let form = user_form_with_memberof(
            /* baseline */ vec!["cn=g1,ou=groups".into()],
            /* edited    */ vec!["cn=g2,ou=groups".into()],
        ); // helper: sets field.relation = BackRef, field.values = edited, baseline has the original
        let labels = form.backref_labels();
        assert_eq!(labels, vec!["memberOf".to_string()]);

        // Own-entry diff with backref labels stripped from BOTH sides → no mods.
        let mut original = EditEntry { dn: form.dn.clone(), attrs: form.baseline.clone() };
        let mut edited = form.to_edit_entry();
        for l in &labels { original.attrs.remove(l); edited.attrs.remove(l); }
        let cs = diff(&original, &edited).unwrap();
        assert!(cs.mods.is_empty(), "memberOf-only change must produce zero own-entry mods");

        // And to_edit_entry already omits backref fields.
        assert!(!edited.attrs.contains_key("memberOf"));
    }
```

Add `user_form_with_memberof(baseline, edited)` helper near the other test helpers.

- [ ] **Step 2: Run to verify it fails** — `cargo test -p edaptor backref_field_excluded` → FAIL.

- [ ] **Step 3: Implement** — in `to_edit_entry`, skip BackRef fields:

```rust
    pub fn to_edit_entry(&self) -> EditEntry {
        let attrs = self
            .fields
            .iter()
            .filter(|f| !matches!(&f.relation,
                Some(FieldRelation { role: RelationRole::BackRef, .. })))
            .map(|f| (f.label.clone(), f.current_values()))
            .collect();
        EditEntry { dn: self.dn.clone(), attrs }
    }

    /// Labels of BackRef relation fields (excluded from the own-entry diff; their
    /// change drives the fan-out). Used to strip them from the baseline too.
    pub fn backref_labels(&self) -> Vec<String> {
        self.fields
            .iter()
            .filter(|f| matches!(&f.relation,
                Some(FieldRelation { role: RelationRole::BackRef, .. })))
            .map(|f| f.label.clone())
            .collect()
    }
```

`is_dirty` (~151) needs no change: a BackRef field stays `multi`, its `current_values()` returns `values` (the picker's selection), compared set-wise to `baseline` — so a membership change marks the form dirty for free.

- [ ] **Step 4: Run to verify it passes** — `cargo test -p edaptor backref_field_excluded` → PASS.

- [ ] **Step 5: Commit**

```bash
git add src/ui/edit_form.rs
git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -m "$(printf 'feat(ui): exclude BackRef fields from the single-entry diff (both sides)\n\nCo-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>')"
```

### Task 5.2: Fan-out builder + last-member predicate + multi-entry LDIF

**Files:**
- Modify: `src/ui/app.rs` (new pure fns `membership_fanout`, `would_empty`)
- Modify: `src/ldap/ldif.rs` (`render_changesets`)

- [ ] **Step 1: Write the failing tests**

In `src/ui/app.rs` tests:

```rust
    #[test]
    fn fanout_adds_and_removes_per_group() {
        let out = membership_fanout(
            "uid=ann,ou=people",
            &["cn=g1,ou=groups".into(), "cn=g2,ou=groups".into()], // baseline groups
            &["cn=g2,ou=groups".into(), "cn=g3,ou=groups".into()], // new selection
            "member",
        );
        // g3 gains ann; g1 loses ann; g2 unchanged.
        assert_eq!(out, vec![
            ("cn=g3,ou=groups".to_string(), ModOp::Add { attr: "member".into(), values: vec!["uid=ann,ou=people".into()] }),
            ("cn=g1,ou=groups".to_string(), ModOp::Delete { attr: "member".into(), values: vec!["uid=ann,ou=people".into()] }),
        ]);
    }

    #[test]
    fn would_empty_only_when_sole_member() {
        assert!(would_empty(&["uid=ann,ou=people".into()], "uid=ann,ou=people"));
        assert!(!would_empty(&["uid=ann,ou=people".into(), "uid=bob,ou=people".into()], "uid=ann,ou=people"));
        assert!(!would_empty(&[], "uid=ann,ou=people")); // already empty: not our removal's fault
    }
```

In `src/ldap/ldif.rs` tests:

```rust
    #[test]
    fn renders_multiple_changesets_with_separators() {
        let a = ChangeSet { dn: "cn=g1,ou=groups".into(), modrdn: None,
            mods: vec![ModOp::Delete { attr: "member".into(), values: vec!["uid=ann".into()] }] };
        let b = ChangeSet { dn: "cn=g3,ou=groups".into(), modrdn: None,
            mods: vec![ModOp::Add { attr: "member".into(), values: vec!["uid=ann".into()] }] };
        let out = render_changesets(&[a, b]);
        assert!(out.contains("dn: cn=g1,ou=groups"));
        assert!(out.contains("dn: cn=g3,ou=groups"));
        assert!(out.contains("\n\n")); // blank line between entries
    }
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p edaptor -- fanout would_empty renders_multiple_changesets` → FAIL.

- [ ] **Step 3: Implement**

In `src/ui/app.rs` (near the other pure helpers; import `ModOp` is already in scope via `form::changeset`):

```rust
/// Per-holder MODIFYs for a membership change on the candidate's back-ref field.
/// `entry_dn` is the candidate (user) DN written into each holder's `holder_attr`.
/// Added groups get an Add; removed groups get a Delete. Order: adds, then deletes.
fn membership_fanout(
    entry_dn: &str,
    baseline: &[String],
    selected: &[String],
    holder_attr: &str,
) -> Vec<(String, ModOp)> {
    let has = |set: &[String], dn: &str| set.iter().any(|x| x.eq_ignore_ascii_case(dn));
    let mut out = Vec::new();
    for g in selected {
        if !has(baseline, g) {
            out.push((g.clone(), ModOp::Add { attr: holder_attr.to_string(), values: vec![entry_dn.to_string()] }));
        }
    }
    for g in baseline {
        if !has(selected, g) {
            out.push((g.clone(), ModOp::Delete { attr: holder_attr.to_string(), values: vec![entry_dn.to_string()] }));
        }
    }
    out
}

/// True when removing `member` would leave the group with no members (groupOfNames
/// requires ≥1). Only fires when `member` is the SOLE current member.
fn would_empty(current_members: &[String], member: &str) -> bool {
    current_members.len() == 1 && current_members[0].eq_ignore_ascii_case(member)
}
```

In `src/ldap/ldif.rs`:

```rust
/// Render several change sets as one LDIF preview, separated by a blank line.
pub fn render_changesets(sets: &[ChangeSet]) -> String {
    sets.iter()
        .filter(|cs| !cs.is_empty())
        .map(render_changeset)
        .collect::<Vec<_>>()
        .join("\n\n")
}
```

- [ ] **Step 4: Run to verify it passes** — `cargo test -p edaptor -- fanout would_empty renders_multiple_changesets` → PASS.

- [ ] **Step 5: Commit**

```bash
git add src/ui/app.rs src/ldap/ldif.rs
git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -m "$(printf 'feat: membership fan-out builder, last-member predicate, multi-entry LDIF\n\nCo-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>')"
```

### Task 5.3: `PendingAction::CombinedSave` + synchronous executor

**Files:**
- Modify: `src/ui/app.rs` (`PendingAction` ~89; `handle_action` FormSave ~709; `SaveThenNavigate` ~1049; `execute_pending` ~1022)

- [ ] **Step 1: Write the failing test** — a pure test of the planning step that turns a dirty user form into a `CombinedSave` (the synchronous worker apply itself is covered live in Task 5.4):

```rust
    #[test]
    fn plan_combined_save_splits_own_and_fanout() {
        // user form: own change (description) + memberOf change (g1→g2).
        let form = user_form_own_and_memberof_change();
        let plan = plan_combined_save(&form, schema_for(&form), &relations_for(&form));
        let cs = match plan { CombinedPlan::Ready { own_mods, fanout, entry_dn, .. } => (own_mods, fanout, entry_dn), _ => panic!() };
        // own_mods touches description, NOT memberOf.
        assert!(cs.0.iter().all(|m| !matches!(m, ModOp::Add{attr,..}|ModOp::Delete{attr,..}|ModOp::Replace{attr,..} if attr.eq_ignore_ascii_case("memberOf"))));
        // fanout: g2 gains the user, g1 loses the user.
        assert_eq!(cs.1.len(), 2);
    }

    #[test]
    fn rename_plus_membership_is_blocked() {
        let form = user_form_rename_and_memberof_change(); // RDN attr changed + memberOf changed
        assert!(matches!(plan_combined_save(&form, schema_for(&form), &relations_for(&form)), CombinedPlan::Blocked(_)));
    }
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p edaptor -- plan_combined_save rename_plus_membership` → FAIL.

- [ ] **Step 3: Implement**

Add the plan type + planner (pure):

```rust
/// Outcome of planning a save for a form that has BackRef (membership) changes.
enum CombinedPlan {
    /// No membership change → caller uses the normal single-entry path.
    NoMembershipChange,
    /// Own-entry mods + per-holder fan-out, with the combined LDIF preview.
    Ready {
        entry_dn: String,
        own_mods: Vec<ModOp>,
        fanout: Vec<(String, ModOp)>,
        ldif: String,
    },
    Blocked(String),
    Invalid(Vec<ValidationError>),
    DiffError(String),
}

/// Plan a combined save: own-entry diff (backref stripped from BOTH sides) plus
/// the fan-out from each BackRef field's baseline→selection delta. Blocks a
/// rename combined with a membership change (v1 simplification, spec §6.3).
fn plan_combined_save(form: &EditForm, schema: &SchemaModel, relations: &[ResolvedRelation]) -> CombinedPlan {
    let backref = form.backref_labels();
    if backref.is_empty() {
        return CombinedPlan::NoMembershipChange;
    }
    // Did any backref field actually change?
    let changed = form.fields.iter().any(|f| backref.contains(&f.label) && {
        let base = form.baseline.get(&f.label).cloned().unwrap_or_default();
        !value_set_eq_pub(&f.current_values(), &base) // a small pub wrapper or inline set compare
    });
    if !changed {
        return CombinedPlan::NoMembershipChange;
    }

    // Own-entry: strip backref labels from both sides, validate + diff.
    let object_classes = object_classes_of(form);
    let oc_refs: Vec<&str> = object_classes.iter().map(|s| s.as_str()).collect();
    let mut original = EditEntry { dn: form.dn.clone(), attrs: form.baseline.clone() };
    let mut edited = form.to_edit_entry(); // already omits backref
    for l in &backref { original.attrs.remove(l); edited.attrs.remove(l); }

    let errors = validate(&edited, schema, &oc_refs);
    if !errors.is_empty() { return CombinedPlan::Invalid(errors); }
    let own_cs = match diff(&original, &edited) { Ok(c) => c, Err(e) => return CombinedPlan::DiffError(e.to_string()) };
    if own_cs.modrdn.is_some() {
        return CombinedPlan::Blocked(
            "Rename and membership changes can't be saved together — do them in separate saves.".into());
    }

    // Fan-out from each backref field.
    let mut fanout = Vec::new();
    let mut preview_sets: Vec<ChangeSet> = Vec::new();
    if !own_cs.is_empty() { preview_sets.push(own_cs.clone()); }
    for f in form.fields.iter().filter(|f| backref.contains(&f.label)) {
        // Which relation? backref_lookup by this form's objectClasses + the field label.
        let Some(rel) = crate::config::relation::backref_lookup(relations, &object_classes, &f.label) else { continue };
        let base = form.baseline.get(&f.label).cloned().unwrap_or_default();
        let ops = membership_fanout(&form.dn, &base, &f.current_values(), &rel.holder_attr);
        for (gdn, op) in ops {
            preview_sets.push(ChangeSet { dn: gdn.clone(), modrdn: None, mods: vec![op.clone()] });
            fanout.push((gdn, op));
        }
    }

    CombinedPlan::Ready {
        entry_dn: form.dn.clone(),
        own_mods: own_cs.mods,
        fanout,
        ldif: render_changesets(&preview_sets),
    }
}
```

> `value_set_eq_pub`: either make `edit_form::value_set_eq` `pub(crate)` and import it, or inline a 3-line set compare here. Prefer making the existing one `pub(crate)` (single source of truth).

Add the `PendingAction` variant:

```rust
    /// A combined membership save: own-entry MODIFY + per-holder fan-out MODIFYs,
    /// applied synchronously (spec §6.3). `reread_dn` is the edited entry.
    CombinedSave {
        entry_dn: String,
        own_mods: Vec<ModOp>,
        fanout: Vec<(String, ModOp)>,
    },
```

In `handle_action` `FormSave` (~709), call `plan_combined_save` FIRST; only fall through to the existing `prepare_save` path when there is no membership change:

```rust
        UiAction::FormSave => {
            let Some(form) = app.form.as_ref() else { return };
            match plan_combined_save(form, read_flow.schema(), &app.relations) {
                CombinedPlan::Ready { entry_dn, own_mods, fanout, ldif } => {
                    app.overlay = Some(Overlay::Confirm {
                        title: "Apply these changes?".to_string(),
                        body: ldif,
                        action: PendingAction::CombinedSave { entry_dn, own_mods, fanout },
                    });
                }
                CombinedPlan::Blocked(msg) => app.overlay = Some(Overlay::Error { text: msg }),
                CombinedPlan::Invalid(errs) => app.overlay = Some(Overlay::Error { text: format_validation_errors(&errs) }),
                CombinedPlan::DiffError(e) => app.overlay = Some(Overlay::Error { text: e }),
                CombinedPlan::NoMembershipChange => {
                    // ...existing prepare_save(...) path, unchanged...
                }
            }
        }
```

Apply the same `NoMembershipChange`-guarded branch in `SaveThenNavigate` (~1049): if `plan_combined_save` is `Ready/Blocked/Invalid/DiffError`, handle as above (membership save does not also navigate in v1 — show the confirm; navigation resumes after); only the `NoMembershipChange` case runs the existing prepare_save+navigate logic.

Add the synchronous executor arm to `execute_pending` (~1030):

```rust
        PendingAction::CombinedSave { entry_dn, own_mods, fanout } => {
            apply_combined_save(app, worker, read_flow, &entry_dn, own_mods, fanout);
        }
```

```rust
/// Apply a combined membership save SYNCHRONOUSLY (mirrors refresh_structure):
/// pre-validate last-member on every removal, abort the whole batch if any would
/// empty a group, then apply own-entry mods + each fan-out MODIFY, collecting a
/// partial-failure report, and finally re-read the edited entry (async).
fn apply_combined_save(
    app: &mut App,
    worker: &WorkerHandle,
    read_flow: &mut ReadFlow,
    entry_dn: &str,
    own_mods: Vec<ModOp>,
    fanout: Vec<(String, ModOp)>,
) {
    // 1. Pre-validate: for each Delete (removal), Base-read the group's members.
    let mut blocked: Vec<String> = Vec::new();
    for (gdn, op) in &fanout {
        if let ModOp::Delete { values, .. } = op {
            let members = read_group_members(worker, gdn); // Base search, attrs=[holder_attr]
            if let Some(member) = values.first() {
                if would_empty(&members, member) {
                    blocked.push(gdn.clone());
                }
            }
        }
    }
    if !blocked.is_empty() {
        app.overlay = Some(Overlay::Error {
            text: format!("Can't remove the last member of: {}", blocked.join(", ")),
        });
        return;
    }

    // 2. Apply own-entry mods, then each fan-out MODIFY; collect failures.
    let mut failures: Vec<String> = Vec::new();
    if !own_mods.is_empty() {
        if let Some(msg) = apply_one_modify(worker, entry_dn, own_mods) { failures.push(format!("{entry_dn}: {msg}")); }
    }
    for (gdn, op) in fanout {
        if let Some(msg) = apply_one_modify(worker, &gdn, vec![op]) { failures.push(format!("{gdn}: {msg}")); }
    }

    // 3. Report + re-read the edited entry (its memberOf refreshes via the overlay).
    if failures.is_empty() {
        app.status = "Saved.".to_string();
    } else {
        app.overlay = Some(Overlay::Error {
            text: format!("Some changes did not apply:\n- {}", failures.join("\n- ")),
        });
    }
    rebind_selection(app, entry_dn);
    let _ = read_flow.request_entry(worker, entry_dn, None);
}

/// Base-read a group's current `member` values (sync). Empty on error.
fn read_group_members(worker: &WorkerHandle, group_dn: &str) -> Vec<String> {
    match worker.request(Request::Search {
        id: next_id(), base: group_dn.to_string(), scope: SearchScope::Base,
        filter: "(objectClass=*)".to_string(), attrs: vec!["member".to_string()], size_limit: None,
    }) {
        Ok(Response::Entries { entries, .. }) => entries.into_iter().next()
            .and_then(|e| e.attrs.into_iter().find(|(k, _)| k.eq_ignore_ascii_case("member")).map(|(_, v)| v))
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Apply one MODIFY synchronously; return Some(human message) on failure.
fn apply_one_modify(worker: &WorkerHandle, dn: &str, changes: Vec<ModOp>) -> Option<String> {
    match worker.request(Request::Modify { id: next_id(), dn: dn.to_string(), changes }) {
        Ok(Response::WriteOk { .. }) => None,
        Ok(Response::WriteError { msg, .. }) => Some(msg),
        Ok(_) => Some("unexpected response".to_string()),
        Err(e) => Some(e.to_string()),
    }
}
```

> **[verify-at-task-start]** `read_group_members` hardcodes `"member"`; if a relation uses a different `holder_attr`, pass it in. For v1 (single group-membership relation) `member` is correct; thread `holder_attr` through if a second relation with a different attr is configured.

- [ ] **Step 4: Run to verify it passes** — `cargo test -p edaptor -- plan_combined_save rename_plus_membership` → PASS; `cargo build` + clippy + fmt clean.

- [ ] **Step 5: Commit**

```bash
git add src/ui/app.rs
git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -m "$(printf 'feat(ui): synchronous combined membership save + last-member pre-validate\n\nCo-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>')"
```

### Task 5.4: Live integration — reverse (user.memberOf) edit + last-member block

**Files:**
- Modify: `tests/live_membership.rs`

- [ ] **Step 1: Write the tests** (gated):

```rust
#[test]
fn reverse_memberof_edit_writes_group_member() {
    let Some(_uri) = std::env::var("EDAPTOR_TEST_LDAP_URI").ok() else { return };
    // Seed: userA, group g (member: someoneElse so it never goes empty).
    // Apply a fan-out: Add member=userA to g (Request::Modify Add).
    // Re-read g; assert userA in member. Re-read userA; assert memberOf contains g (overlay).
}

#[test]
fn removing_last_member_is_rejected_by_server() {
    let Some(_uri) = std::env::var("EDAPTOR_TEST_LDAP_URI").ok() else { return };
    // Seed: group g with exactly one member userA.
    // Attempt Delete member=userA → expect WriteError (objectClassViolation),
    // confirming the server enforces ≥1 even if client pre-check is bypassed.
}
```

Fill bodies using the `tests/live_write.rs` connection + seed/teardown helpers.

- [ ] **Step 2: Run** — `cargo test -p edaptor --test live_membership` (SKIPs without env). With slapd + env set, exercises the reverse path and the last-member guard.

- [ ] **Step 3: Commit**

```bash
git add tests/live_membership.rs
git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -m "$(printf 'test(live): reverse memberOf fan-out + last-member rejection (gated)\n\nCo-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>')"
```

### Task 5.5: Manual tmux smoke + example config

**Files:**
- Modify: an example/config doc (e.g. `README.md` or `docs/` example) to show a `[[relation]]` block and `search_attrs`.

- [ ] **Step 1:** Add a documented `[[relation]]` example (the §3 block) plus `search_attrs` to the example config, so an operator can enable membership editing.

- [ ] **Step 2: Manual smoke** (per project memory `edaptor-m4-handoff` run instructions): launch against the podman slapd, open a group → Enter on `member` → type to search users (≤20 results, current members pinned) → Space toggles → F2 → Save → confirm the LDIF preview spans the group → verify the write. Then open a user → Enter on `memberOf` → add/remove a group → Save → confirm the combined LDIF spans the affected groups → verify. Attempt removing a group's last member → expect the clear block message.

- [ ] **Step 3: Commit**

```bash
git add README.md
git -c user.name='oetiker' -c user.email='oetiker@gmail.com' commit -m "$(printf 'docs: document [[relation]] membership config + search_attrs\n\nCo-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>')"
```

---

## Self-Review (completed during planning)

- **Spec coverage:** §2 picker mode → P2/P4; §3 `[[relation]]` → P1; §4 picker UX (search, selected-always-visible, labels, DN storage) → P2.1/P2.2/P4.3-4.5; §5 worker (size_limit, debounce-via-stale-id, fan-out via Modify, re-read) → P3 + P4.4 + P5.3; §6 save semantics (holder direct, backref exclusion both sides, combined flow) → P4 + P5.1/P5.3; §7 errors (last-member, partial-failure report, size cap hint) → P5.2/P5.3 + P4.5 footer; §8 testing → tests throughout + `tests/live_membership.rs`; §9 out-of-scope (membership-on-create passes `&[]`; rename+membership blocked) → P4.1/P5.3.
- **Deviation from spec (flag to user):** debounce is implemented as fire-on-change + stale-id discard (no timer) — simpler, matches the existing incremental-search pattern, satisfies "snappy + bounded (cap 20)". Space is the toggle key so search terms can't contain spaces (acceptable for cn/uid). Re-read collapses to the edited entry only (groups are confirmed by their per-op WriteOk and aren't displayed).
- **Type consistency:** `ResolvedRelation`/`CandidateScope`/`RelationRole` (config::relation) used identically in edit_form + app; `Candidate`/`PickerState` (ui::picker) used in edit_form + app; `ModOp`/`ChangeSet` reused unchanged; `Request::Search.size_limit: Option<i32>` matches all call sites.
- **Placeholder scan:** the two `[verify-at-task-start]` notes (ldap3 sizelimit API — verified present; `Structure::get` label accessor; `holder_attr` in `read_group_members`) are explicit verification steps with fallbacks, not vague gaps.
