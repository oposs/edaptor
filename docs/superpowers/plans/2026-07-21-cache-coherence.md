# Cache Coherence (Spec 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every list edaptor shows reflect directory reality — writes reflow the in-memory model immediately, incremental find is answered by the server, and Alt+R rebuilds the projection from scratch.

**Architecture:** One mutation point (`Structure::upsert`) fed by every entry read, so create/rename/label-edit coherence all travel the same path. A new `LeafSearchFlow` makes the entry list's find a live one-level search, and the lookup combobox re-queries per keystroke instead of filtering a capped one-shot load. A `tree_dirty` flag gives the tree pane the rebuild path it never had, and a new `RELOAD` command re-runs the eager scan.

**Tech Stack:** Rust 2021, tvision-rs 0.12.1, ldap3 0.12.1, anyhow. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-07-21-cache-coherence-design.md`

## Global Constraints

- **Cap build/test parallelism at 4 cores**: `cargo test -j4`, `cargo clippy --all-targets -- -D warnings`. Never `-j` above 4 (shared machine).
- **`make check` (fmt + clippy `-D warnings` + tests) is the gate** — it must be green before any task is declared done.
- **Facade boundary:** only `src/ui/**` may `use tvision_rs`; `ldap3` only in `src/ldap/**`. `src/workflows/**` is pure domain logic — no tvision, no ldap3, no `crate::ui`.
- **Comments, identifiers and docs in English.**
- **Commit trailer on every commit:** `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- **Docs are part of done** (Task 9 collects them): `CHANGES.md` for every user-visible change, mdBook under `docs/src/` for behaviour detail, README stays an overview.
- **Existing id ranges** (do not collide): ReadFlow 1+, WriteFlow 1_000_000+, AllocFlow 2_000_000+, SearchFlow 3_000_000+, ResolveFlow 4_000_000+. This plan adds LeafSearchFlow at **5_000_000+**.
- Work on branch `feat/cache-coherence` in a worktree under `/scratch/oetiker/claude-worktrees/`.

---

### Task 1: `Structure::upsert` — the single mutation point

**Files:**
- Modify: `src/workflows/structure.rs:167-202` (replace `add_child`, keep `remove`)
- Test: `src/workflows/structure.rs` (inline `mod tests`)

**Interfaces:**
- Consumes: nothing (pure model, no dependencies on other tasks).
- Produces: `Structure::upsert(&mut self, input: StructureInput) -> bool` — `true` means "the tree pane must rebuild". `Structure::remove(&mut self, dn: &str) -> bool` is unchanged and stays.
- Removes: `Structure::add_child` (no production callers; `upsert` subsumes it).

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `src/workflows/structure.rs` (the existing `input()` and `fixture()` helpers are already there):

```rust
    #[test]
    fn upsert_preserves_existing_children() {
        // Upserting a CONTAINER must never orphan its subtree.
        let mut s = fixture();
        let changed = s.upsert(input("ou=users,dc=example,dc=org", Some("Users"), None));
        assert!(!changed, "updating an existing branch's label is not a flip");
        let node = s.get("ou=users,dc=example,dc=org").unwrap();
        assert_eq!(node.label, "Users", "label refreshed from the new input");
        assert_eq!(
            node.children,
            vec!["uid=jane,ou=users,dc=example,dc=org".to_string()],
            "children survive the upsert"
        );
    }

    #[test]
    fn upsert_links_new_node_under_known_parent() {
        let mut s = fixture();
        let changed = s.upsert(input(
            "uid=bob,ou=users,dc=example,dc=org",
            Some("Bob"),
            None,
        ));
        assert!(!changed, "parent was already a branch — no tree flip");
        let leaves: Vec<&str> = s
            .leaves_of("ou=users,dc=example,dc=org")
            .iter()
            .map(|n| n.dn.as_str())
            .collect();
        assert!(leaves.contains(&"uid=bob,ou=users,dc=example,dc=org"));
    }

    #[test]
    fn upsert_promoting_parent_leaf_to_branch_returns_true() {
        let mut s = fixture();
        // ou=empty has no children yet: the first child flips it leaf->branch.
        let changed = s.upsert(input("cn=x,ou=empty,dc=example,dc=org", Some("X"), None));
        assert!(changed, "leaf->branch flip must request a tree rebuild");
        assert!(s.get("ou=empty,dc=example,dc=org").unwrap().is_branch());
    }

    #[test]
    fn upsert_changing_a_branch_attr_returns_true() {
        // The tree pane renders branch labels from `attrs`, so an attr change on a
        // BRANCH must request a rebuild.
        let mut s = fixture();
        let mut inp = input("ou=users,dc=example,dc=org", None, None);
        inp.attrs
            .insert("description".to_string(), vec!["Staff".to_string()]);
        assert!(s.upsert(inp), "branch attrs changed → rebuild");
    }

    #[test]
    fn upsert_unchanged_leaf_returns_false() {
        let mut s = fixture();
        assert!(
            !s.upsert(input(
                "uid=jane,ou=users,dc=example,dc=org",
                Some("Jane Doe"),
                None
            )),
            "re-upserting an unchanged leaf is not a tree change"
        );
    }

    #[test]
    fn upsert_with_unknown_parent_inserts_unlinked() {
        // An entry outside the loaded base: inserted, but not reachable as a leaf.
        let mut s = fixture();
        s.upsert(input("uid=zoe,ou=other,dc=elsewhere,dc=org", Some("Zoe"), None));
        assert!(s.get("uid=zoe,ou=other,dc=elsewhere,dc=org").is_some());
        assert!(s.leaves_of("ou=other,dc=elsewhere,dc=org").is_empty());
    }

    #[test]
    fn rename_modelled_as_remove_then_upsert_leaves_no_stale_node() {
        let mut s = fixture();
        s.remove("uid=jane,ou=users,dc=example,dc=org");
        s.upsert(input(
            "uid=jane2,ou=users,dc=example,dc=org",
            Some("Jane Doe"),
            None,
        ));
        assert!(s.get("uid=jane,ou=users,dc=example,dc=org").is_none());
        let leaves: Vec<&str> = s
            .leaves_of("ou=users,dc=example,dc=org")
            .iter()
            .map(|n| n.dn.as_str())
            .collect();
        assert_eq!(leaves, vec!["uid=jane2,ou=users,dc=example,dc=org"]);
    }
```

Also **delete** the now-obsolete test `promote_marks_parent_as_branch_on_first_child` (it calls `add_child`, which this task removes; `upsert_promoting_parent_leaf_to_branch_returns_true` replaces it).

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -j4 --lib workflows::structure`
Expected: FAIL — `no method named 'upsert' found for struct 'Structure'`.

- [ ] **Step 3: Replace `add_child` with `upsert`**

In `src/workflows/structure.rs`, delete the whole `add_child` method (lines 167-186, including its doc comment) and insert in its place:

```rust
    /// Insert or update the node for `input.dn`, preserving any children the node
    /// already has, and link it under its parent when that parent is a known node.
    ///
    /// This is the single production mutation point for the structure: it is fed by
    /// every entry read (navigation and post-write re-read alike), so a create, a
    /// rename's new DN, and a label-changing edit all reflow through one path. An
    /// entry whose parent lies outside the loaded base is inserted but stays
    /// unlinked — hence invisible — exactly as [`Structure::build`] treats it.
    ///
    /// Returns `true` when the **tree pane** must rebuild: either the parent flipped
    /// leaf→branch, or an existing branch's attributes changed (the tree renders
    /// branch labels from `attrs`).
    pub fn upsert(&mut self, input: StructureInput) -> bool {
        let dn = input.dn.clone();
        let parent = parent_of(&dn).map(str::to_string);
        let parent_was_branch = parent
            .as_ref()
            .and_then(|p| self.nodes.get(p))
            .map(|n| n.is_branch())
            .unwrap_or(false);

        // Preserve the existing subtree links; note whether this node is itself a
        // branch whose rendered attributes change.
        let (children, was_branch, attrs_changed) = match self.nodes.get(&dn) {
            Some(n) => (n.children.clone(), n.is_branch(), n.attrs != input.attrs),
            None => (Vec::new(), false, false),
        };

        self.nodes.insert(
            dn.clone(),
            StructureNode {
                dn: dn.clone(),
                label: label_for(&input),
                object_classes: input.object_classes,
                attrs: input.attrs,
                children,
            },
        );
        if let Some(p) = parent.as_ref().and_then(|p| self.nodes.get_mut(p)) {
            if !p.children.iter().any(|c| c.eq_ignore_ascii_case(&dn)) {
                p.children.push(dn.clone());
            }
        }

        let parent_is_branch = parent
            .as_ref()
            .and_then(|p| self.nodes.get(p))
            .map(|n| n.is_branch())
            .unwrap_or(false);
        (!parent_was_branch && parent_is_branch) || (was_branch && attrs_changed)
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -j4 --lib workflows::structure`
Expected: PASS (all tests in the module, including the untouched `remove`/`build` tests).

- [ ] **Step 5: Verify no caller of `add_child` remains**

Run: `grep -rn "add_child" --include=*.rs src/ tests/`
Expected: no output.

- [ ] **Step 6: Commit**

```bash
git add src/workflows/structure.rs
git commit -m "feat(structure): upsert as the single mutation point, replacing add_child

Preserves children, links to a known parent, and reports whether the tree
pane must rebuild (parent leaf->branch flip, or a branch's attrs changed).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Every entry read feeds the structure

**Files:**
- Modify: `src/workflows/read_flow.rs:19-35` (add `dn` + `attrs` to `ReadOutcome::Form`), `:93-98` (populate them), add a test seam
- Modify: `src/ui/state.rs:107-226` (add `scan_attrs`, `tree_dirty` fields), `:243-292` (`new_for_test`), `:316-344` (wire the upsert), `:1218-1266` (`bootstrap`)
- Test: `src/workflows/read_flow.rs` (inline tests), `src/ui/state.rs` (inline tests)

**Interfaces:**
- Consumes: `Structure::upsert(StructureInput) -> bool` (Task 1).
- Produces:
  - `ReadOutcome::Form { model, object_classes, baseline_csn, dn: String, attrs: BTreeMap<String, Vec<String>> }`
  - `ReadFlow::insert_pending_for_test(&mut self, id: u64, show: Vec<String>)` (test seam)
  - `UiState::scan_attrs: Vec<String>` — the label/tree template attributes fetched by the eager scan; Tasks 5/6/8 reuse it.
  - `UiState::tree_dirty: bool` — set here, consumed by the tree pane in Task 4.
  - `UiState::upsert_from_read(&mut self, dn: &str, attrs: &BTreeMap<String, Vec<String>>)` — `pub(crate)`, reused by Task 6.

- [ ] **Step 1: Write the failing tests**

In `src/workflows/read_flow.rs`, add to `mod tests`:

```rust
    #[test]
    fn form_outcome_carries_dn_and_raw_attrs() {
        let mut flow = ReadFlow::new(schema());
        flow.insert_pending_for_test(7, Vec::new());
        let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        attrs.insert("cn".to_string(), vec!["Jane".to_string()]);
        attrs.insert("objectClass".to_string(), vec!["person".to_string()]);
        let resp = Response::Entries {
            id: 7,
            entries: vec![LdapEntry {
                dn: "uid=jane,ou=users,dc=x".to_string(),
                attrs,
                bin_attrs: Default::default(),
            }],
            truncated: false,
        };
        match flow.on_response(&resp) {
            ReadOutcome::Form { dn, attrs, .. } => {
                assert_eq!(dn, "uid=jane,ou=users,dc=x");
                assert_eq!(attrs.get("cn").unwrap(), &vec!["Jane".to_string()]);
            }
            _ => panic!("expected Form"),
        }
    }
```

In `src/ui/state.rs`, add to `mod tests`:

```rust
    #[test]
    fn upsert_from_read_projects_scan_attrs_and_marks_dirty() {
        let structure = Structure::build("dc=x", vec![si("dc=x", None), si("ou=p,dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.scan_attrs = vec!["cn".to_string()];
        let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        attrs.insert("cn".to_string(), vec!["Bob".to_string()]);
        attrs.insert("objectClass".to_string(), vec!["person".to_string()]);
        // `sn` is NOT in scan_attrs and must not be stored on the node.
        attrs.insert("sn".to_string(), vec!["Baker".to_string()]);

        st.upsert_from_read("uid=bob,ou=p,dc=x", &attrs);

        let node = st.structure.get("uid=bob,ou=p,dc=x").expect("node inserted");
        assert_eq!(node.label, "Bob", "label rendered from cn");
        assert_eq!(node.object_classes, vec!["person".to_string()]);
        assert!(node.attrs.contains_key("cn"));
        assert!(
            !node.attrs.contains_key("sn"),
            "only scan_attrs are projected onto the node"
        );
        assert!(st.list_dirty, "the leaf list must rebuild");
        assert!(
            st.tree_dirty,
            "ou=p flipped leaf->branch, so the tree must rebuild too"
        );
    }

    #[test]
    fn upsert_from_read_snaps_the_highlight_to_the_shown_entry() {
        let structure = Structure::build("dc=x", vec![si("dc=x", None), si("ou=p,dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.scan_attrs = vec!["cn".to_string()];
        st.current_branch = Some("ou=p,dc=x".into());
        st.current_leaf = Some("uid=bob,ou=p,dc=x".into());
        let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        attrs.insert("cn".to_string(), vec!["Bob".to_string()]);

        st.upsert_from_read("uid=bob,ou=p,dc=x", &attrs);

        // Rows are [‹self› ou=p, Bob] → the new entry is row 1.
        assert_eq!(st.set_leaf_row, Some(1));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -j4 --lib read_flow::tests::form_outcome_carries_dn_and_raw_attrs upsert_from_read`
Expected: FAIL — no `insert_pending_for_test`, no `scan_attrs`, no `tree_dirty`, no `upsert_from_read`.

- [ ] **Step 3: Extend `ReadOutcome::Form` and add the test seam**

In `src/workflows/read_flow.rs`, replace the `Form` variant (lines 22-30) with:

```rust
    Form {
        /// The schema-driven form model.
        model: FormModel,
        /// The entry's objectClass values.
        object_classes: Vec<String>,
        /// The entry's `entryCSN` at read time (version token for optimistic
        /// concurrency). `None` if the server did not return it.
        baseline_csn: Option<String>,
        /// The entry's DN, so the controller can refresh its structure node.
        dn: String,
        /// The entry's raw string attributes, so the controller can project the
        /// label/tree attributes onto the structure node without a second read.
        attrs: std::collections::BTreeMap<String, Vec<String>>,
    },
```

In `on_response` (line 93), replace the `ReadOutcome::Form { … }` construction with:

```rust
                ReadOutcome::Form {
                    model: self.form_for(entry, &show),
                    object_classes: object_classes_of(entry),
                    baseline_csn: entry_csn_of(entry),
                    dn: entry.dn.clone(),
                    attrs: entry.attrs.clone(),
                }
```

Add the test seam directly after `request_entry` (before `on_response`):

```rust
    /// Test-only: register a pending read id without a live [`WorkerHandle`], so
    /// `on_response` can be driven with hand-built responses.
    #[cfg(test)]
    pub(crate) fn insert_pending_for_test(&mut self, id: u64, show: Vec<String>) {
        self.pending.insert(id, show);
    }
```

- [ ] **Step 4: Add the state fields and the upsert bridge**

In `src/ui/state.rs`, add to the `UiState` struct after `tree_rules` (line 115):

```rust
    /// The attribute names the label/tree templates reference — what the eager
    /// scan fetches, and what a per-entry read projects onto its structure node.
    pub scan_attrs: Vec<String>,
```

and after `list_dirty` (line 166):

```rust
    /// True when the DIT tree pane must rebuild its node set (a branch appeared,
    /// disappeared, or changed its rendered label). The tree pane clears it.
    pub tree_dirty: bool,
```

Add `scan_attrs: Vec::new(),` and `tree_dirty: false,` to `new_for_test` (next to `label_rules` / `list_dirty` respectively), and to `bootstrap`'s `UiState { … }` literal add `scan_attrs: scan_attrs.clone(),` and `tree_dirty: false,`. In `bootstrap`, the `LoadStructure` request currently moves `scan_attrs`; change that line to `attrs: scan_attrs.clone(),` so the field can keep the list.

Add this method in the `impl UiState` block that holds `leaf_rows` (after `current_leaf_row`, `src/ui/state.rs:1093`):

```rust
    /// Refresh the structure node for a freshly-read entry.
    ///
    /// Projects the raw attributes onto the label/tree template attributes
    /// (`scan_attrs`) plus `objectClass`, so a node never carries the entry's whole
    /// attribute set, then upserts it. Marks the leaf list dirty and — when the
    /// upsert reports a branch-level change — the tree too. When the refreshed entry
    /// is the one on screen, the leaf highlight is snapped to its row, which is what
    /// makes a newly created entry both appear AND become selected.
    ///
    /// Called for every entry read: navigation clicks and post-write re-reads alike,
    /// so any entry the operator visits self-heals from live data.
    pub(crate) fn upsert_from_read(
        &mut self,
        dn: &str,
        attrs: &std::collections::BTreeMap<String, Vec<String>>,
    ) {
        let first = |name: &str| {
            attrs
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .and_then(|(_, v)| v.first().cloned())
        };
        let mut kept: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        for want in &self.scan_attrs {
            if let Some((k, v)) = attrs.iter().find(|(k, _)| k.eq_ignore_ascii_case(want)) {
                kept.insert(k.clone(), v.clone());
            }
        }
        let object_classes = attrs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("objectClass"))
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        let input = StructureInput {
            dn: dn.to_string(),
            cn: first("cn"),
            description: first("description"),
            object_classes,
            attrs: kept,
        };
        if self.structure.upsert(input) {
            self.tree_dirty = true;
        }
        self.list_dirty = true;
        if self
            .current_leaf
            .as_deref()
            .map(|c| c.eq_ignore_ascii_case(dn))
            .unwrap_or(false)
        {
            self.set_leaf_row = self.current_leaf_row();
        }
    }
```

`StructureInput` is already imported at `src/ui/state.rs:19`.

- [ ] **Step 5: Wire it into the response pump**

In `process_responses` (`src/ui/state.rs:316`), change the `ReadOutcome::Form` arm's pattern and add the call as its first statement:

```rust
                ReadOutcome::Form {
                    model,
                    object_classes,
                    baseline_csn,
                    dn,
                    attrs,
                } => {
                    // Refresh this entry's structure node from the live read before
                    // installing the form, so the list/tree agree with what is shown.
                    self.upsert_from_read(&dn, &attrs);
                    let mut form = build_edit_form(&model, self.read_flow.schema(), self.read_only);
```

(the rest of the arm is unchanged).

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -j4 --lib read_flow upsert_from_read`
Expected: PASS.

- [ ] **Step 7: Run the full check**

Run: `make check`
Expected: green. Any other `ReadOutcome::Form { … }` match sites (`src/workflows/read_flow.rs:195,228` in tests) must be updated with `..` or the new fields — clippy/rustc will point at them.

- [ ] **Step 8: Commit**

```bash
git add src/workflows/read_flow.rs src/ui/state.rs
git commit -m "feat(state): refresh the structure node from every entry read

ReadOutcome::Form carries the entry dn + raw attrs; UiState projects them
onto the label/tree scan attributes and upserts the node, marking the leaf
list (and, on a branch change, the tree) dirty and snapping the highlight
to the entry on screen.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Write-side reflow — rename, create, lookup-cache

**Files:**
- Modify: `src/ui/state.rs:615-724` (`apply_write_outcome`: `Saved`, `CombinedSaved`, `Created` arms)
- Test: `src/ui/state.rs` (inline tests)

**Interfaces:**
- Consumes: `Structure::remove` (existing), `UiState::upsert_from_read` (Task 2), `UiState::tree_dirty` (Task 2).
- Produces: no new API — behaviour only.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `src/ui/state.rs`:

```rust
    #[test]
    fn saved_under_a_new_dn_drops_the_stale_node() {
        // A rename (MODRDN) makes the server echo a different DN than the form was
        // loaded with; the old node must not linger in the entry list.
        let structure = Structure::build(
            "dc=x",
            vec![si("dc=x", None), si("ou=p,dc=x", None), si("uid=old,ou=p,dc=x", Some("Old"))],
        );
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.current_branch = Some("ou=p,dc=x".into());
        st.current_leaf = Some("uid=old,ou=p,dc=x".into());

        st.apply_write_outcome(WriteOutcome::Saved {
            reread_dn: "uid=new,ou=p,dc=x".into(),
            quit_after: false,
        });

        assert!(
            st.structure.get("uid=old,ou=p,dc=x").is_none(),
            "the pre-rename node must be removed"
        );
        assert!(st.list_dirty);
    }

    #[test]
    fn saved_under_the_same_dn_keeps_the_node() {
        let structure = Structure::build(
            "dc=x",
            vec![si("dc=x", None), si("ou=p,dc=x", None), si("uid=a,ou=p,dc=x", Some("A"))],
        );
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.current_leaf = Some("uid=a,ou=p,dc=x".into());

        st.apply_write_outcome(WriteOutcome::Saved {
            reread_dn: "uid=a,ou=p,dc=x".into(),
            quit_after: false,
        });

        assert!(st.structure.get("uid=a,ou=p,dc=x").is_some());
    }

    #[test]
    fn a_write_clears_the_lookup_cache() {
        use crate::workflows::resolve_flow::LookupKey;
        let structure = Structure::build("dc=x", vec![si("dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        let key = LookupKey {
            scope_id: "dc=x|posixGroup|gidNumber".into(),
            value: "5000".into(),
        };
        st.lookup_cache.insert(key.clone(), Some("staff".into()));
        st.current_leaf = Some("uid=a,dc=x".into());

        st.apply_write_outcome(WriteOutcome::Saved {
            reread_dn: "uid=a,dc=x".into(),
            quit_after: false,
        });

        assert!(
            st.lookup_cache.is_empty(),
            "our own write may have changed any label — drop the whole cache"
        );
    }

    #[test]
    fn created_clears_a_stale_find_query() {
        let structure = Structure::build("dc=x", vec![si("dc=x", None), si("ou=p,dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.current_branch = Some("ou=p,dc=x".into());
        st.search = "zzz".into();

        st.apply_write_outcome(WriteOutcome::Created {
            dn: "uid=bob,ou=p,dc=x".into(),
            quit_after: false,
        });

        assert!(
            st.search.is_empty(),
            "a stale query must not hide the entry just created"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -j4 --lib saved_under_a_new_dn saved_under_the_same_dn a_write_clears_the_lookup_cache created_clears_a_stale_find_query`
Expected: FAIL — the stale node survives, the cache is not cleared, `search` still holds `"zzz"`.

- [ ] **Step 3: Implement the `Saved` arm changes**

In `src/ui/state.rs`, inside `WriteOutcome::Saved { reread_dn, quit_after }` (line 616), insert immediately after `self.status = "Saved.".to_string();`:

```rust
                // Our own write may have changed any label we cached (including via a
                // rename), and the cache stores negatives too — drop it wholesale and
                // let the visible fields re-resolve lazily.
                self.lookup_cache.clear();
                // A rename (MODRDN) echoes a DIFFERENT dn than the form was loaded
                // with: drop the pre-rename node here; the re-read upserts the new one.
                if let Some(old) = self.current_leaf.clone() {
                    if !old.eq_ignore_ascii_case(&reread_dn) {
                        if self.structure.remove(&old) {
                            self.tree_dirty = true;
                        }
                        self.list_dirty = true;
                    }
                }
```

- [ ] **Step 4: Implement the `CombinedSaved` and `Created` arm changes**

In `WriteOutcome::CombinedSaved { .. }` (line 655), insert after `self.status = "Saved.".to_string();`:

```rust
                self.lookup_cache.clear();
```

(A combined membership save cannot carry a rename — `PlanCombined::RenameWithMembershipUnsupported` rejects that pairing — so no node removal is needed here.)

In `WriteOutcome::Created { dn, quit_after }` (line 704), insert before `self.current_leaf = Some(dn.clone());`:

```rust
                // A leftover incremental-find query would hide the new row; the
                // cached labels may be stale for the same reason as on Saved.
                self.search.clear();
                self.lookup_cache.clear();
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -j4 --lib --  state::tests`
Expected: PASS, including the pre-existing `Created` test that asserts `list_dirty` and `current_leaf`.

- [ ] **Step 6: Commit**

```bash
git add src/ui/state.rs
git commit -m "feat(state): reflow the model on write — rename, create, lookup cache

Saved drops the pre-rename node when the echoed dn differs; Created clears a
stale find query; every write clears lookup_cache (it caches negatives too, so
a label we just changed would otherwise stay wrong all session).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: The tree pane learns to rebuild

**Files:**
- Modify: `src/ui/pump.rs:90-93` (broadcast on `tree_dirty` too)
- Modify: `src/ui/panes/tree.rs:1-10` (imports), `:97-216` (add `rebuild`), `:224-258` (`handle_event`)
- Test: `src/ui/panes/tree.rs` (inline tests)

**Interfaces:**
- Consumes: `UiState::tree_dirty` (Task 2), `build_branch_nodes(&UiState, usize) -> (Option<Box<tv::Node>>, Vec<String>)` (existing, `src/ui/panes/tree.rs:15`).
- Produces: no new API — the pane consumes and clears `tree_dirty`.

- [ ] **Step 1: Write the failing test**

Add to `mod tests` in `src/ui/panes/tree.rs`:

```rust
    /// A structure change that promotes a leaf to a branch must reach the outline:
    /// on REFRESH with `tree_dirty` set the pane rebuilds its node set, refreshes
    /// `branch_dns`, and keeps the highlight on the same DN (row indices shift).
    #[test]
    fn refresh_with_tree_dirty_rebuilds_and_keeps_the_selected_dn() {
        let inputs = vec![
            si("dc=x"),
            si("ou=a,dc=x"),
            si("ou=b,dc=x"),
            si("cn=1,ou=b,dc=x"),
        ];
        let structure = Structure::build("dc=x", inputs);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st = UiState::new_for_test(
            structure,
            schema,
            "dc=x".into(),
            Vec::new(),
            compile_tree_rules(&TreeConfig::default()),
        );
        let (root, dns) = build_branch_nodes(&st, 40);
        // Only dc=x and ou=b are branches at build time; ou=a is a childless leaf.
        assert_eq!(dns, vec!["dc=x".to_string(), "ou=b,dc=x".to_string()]);
        st.branch_dns = dns;
        st.current_branch = Some("ou=b,dc=x".into());
        let shared: std::rc::Rc<std::cell::RefCell<UiState>> =
            std::rc::Rc::new(std::cell::RefCell::new(st));
        let mut pane = TreePane::new(Rect::new(0, 0, 30, 10), root, shared.clone());

        // A new entry lands under ou=a → it becomes a branch → the tree must change.
        {
            let mut st = shared.borrow_mut();
            st.structure.upsert(si("cn=2,ou=a,dc=x"));
            st.tree_dirty = true;
        }

        let mut out = std::collections::VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<tv::Deferred> = Vec::new();
        let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut ev, &mut ctx);

        let st = shared.borrow();
        assert_eq!(
            st.branch_dns,
            vec![
                "dc=x".to_string(),
                "ou=a,dc=x".to_string(),
                "ou=b,dc=x".to_string()
            ],
            "the promoted container must appear in the DFS index"
        );
        assert!(!st.tree_dirty, "the pane clears the flag it consumed");
        assert_eq!(
            st.requested_branch.as_deref(),
            None,
            "restoring the highlight by DN must not look like a user navigation"
        );
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -j4 --lib refresh_with_tree_dirty_rebuilds`
Expected: FAIL — `branch_dns` still holds the two original DNs (no rebuild path exists).

- [ ] **Step 3: Broadcast REFRESH when the tree is dirty**

In `src/ui/pump.rs`, replace lines 90-93 with:

```rust
            // A clean branch switch (reconcile_branch) or a post-guard branch switch
            // sets list_dirty without any worker activity (r.changed = false). Broadcast
            // REFRESH so the leaf pane rebuilds. The leaf clears list_dirty on rebuild,
            // so this is a single idempotent refresh per dirty-marking, not a loop.
            // `tree_dirty` is the same contract for the DIT tree pane.
            let (list_dirty, tree_dirty) = {
                let st = self.state.borrow();
                (st.list_dirty, st.tree_dirty)
            };
            if r.changed || list_dirty || tree_dirty {
                ctx.broadcast(REFRESH, None);
            }
```

- [ ] **Step 4: Add the rebuild path to `TreePane`**

In `src/ui/panes/tree.rs`, change the import line 8-9 to also bring in `REFRESH`:

```rust
use crate::ui::state::UiState;
use crate::ui::{Shared, REFRESH};
```

(the test module's `use crate::ui::REFRESH;` then becomes redundant — delete that line from `mod tests`.)

Add this method to `impl TreePane`, right after `sync_scrollbar` (line 206):

```rust
    /// Rebuild the outline's node set from the current structure.
    ///
    /// Called when `tree_dirty` is set — a branch appeared, disappeared, or changed
    /// its rendered label. The highlight is restored **by DN**, never by row index:
    /// a rebuild shifts every index below the change. `last_sel` is resynced so the
    /// restored position is not reported back as a fresh user navigation.
    fn rebuild(&mut self, ctx: &mut Context) {
        let width = (self.group.state().get_extent().b.x).max(4) as usize;
        let (root, dns, selected) = {
            let st = self.state.borrow();
            let (root, dns) = build_branch_nodes(&st, width);
            (root, dns, st.current_branch.clone())
        };
        let row = selected
            .and_then(|dn| dns.iter().position(|d| d.eq_ignore_ascii_case(&dn)))
            .map(|i| i as i32);
        self.state.borrow_mut().branch_dns = dns;
        if let Some(outline) = self.outline_mut() {
            outline.root = root;
            tv::widgets::outline::ov_update(outline, ctx);
        }
        if let Some(row) = row {
            if let Some(outline) = self.outline_mut() {
                tv::widgets::outline::adjust_focus(outline, row, ctx);
            }
            self.last_sel = row;
        }
    }
```

- [ ] **Step 5: Consume the flag in `handle_event`**

In `src/ui/panes/tree.rs`, insert at the top of `handle_event`, immediately after the wheel guard (line 230) and **before** the snap-back block:

```rust
        // A structure change (create, rename, delete, refresh) marked the tree stale:
        // rebuild before this event is processed, so the DFS index the selection
        // logic below reads is the current one.
        let needs_rebuild =
            matches!(ev, Event::Broadcast { command, .. } if *command == REFRESH)
                && self.state.borrow().tree_dirty;
        if needs_rebuild {
            self.rebuild(ctx);
            self.state.borrow_mut().tree_dirty = false;
        }
```

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo test -j4 --lib panes::tree`
Expected: PASS (all tree tests, including the existing selector-contract ones).

- [ ] **Step 7: Run the full check**

Run: `make check`
Expected: green.

- [ ] **Step 8: Commit**

```bash
git add src/ui/pump.rs src/ui/panes/tree.rs
git commit -m "feat(tree): rebuild the outline when the structure changes

The tree pane built its node set once at construction and never again. It now
consumes tree_dirty on REFRESH, swaps Outline::root, refreshes branch_dns and
restores the highlight by DN; the pump broadcasts REFRESH for tree_dirty too.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: `LeafSearchFlow` — the live one-level search

**Files:**
- Create: `src/workflows/leaf_search.rs`
- Modify: `src/workflows/mod.rs` (register the module)
- Test: `src/workflows/leaf_search.rs` (inline tests)

**Interfaces:**
- Consumes: `pick_state::escape_filter` (existing, `src/workflows/pick_state.rs:9`), `ldap::worker::{LdapEntry, Request, Response, SearchScope, WorkerHandle}`.
- Produces:
  - `pub const LEAF_SEARCH_CAP: i32 = 500;`
  - `pub fn build_leaf_filter(attrs: &[String], term: &str) -> String`
  - `pub enum LeafSearchOutcome { Results { entries: Vec<LdapEntry>, truncated: bool }, Failed(String), Ignored }`
  - `pub struct LeafSearchFlow` with `new()`, `request(&mut self, worker: &WorkerHandle, branch_dn: &str, term: &str, filter_attrs: &[String], fetch_attrs: &[String]) -> Result<u64>`, `on_response(&mut self, resp: &Response) -> LeafSearchOutcome`, and `#[cfg(test)] force_latest(&mut self, id: u64)`.

- [ ] **Step 1: Write the failing tests**

Create `src/workflows/leaf_search.rs` containing ONLY the test module for now (the implementation lands in Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldap::worker::LdapEntry;
    use std::collections::BTreeMap;

    #[test]
    fn filter_single_attr_is_a_bare_substring_match() {
        assert_eq!(build_leaf_filter(&["cn".to_string()], "ann"), "(cn=*ann*)");
    }

    #[test]
    fn filter_multiple_attrs_are_ored() {
        assert_eq!(
            build_leaf_filter(&["cn".to_string(), "uid".to_string()], "ann"),
            "(|(cn=*ann*)(uid=*ann*))"
        );
    }

    #[test]
    fn filter_escapes_rfc4515_specials() {
        assert_eq!(
            build_leaf_filter(&["cn".to_string()], "a*b"),
            "(cn=*a\\2ab*)"
        );
    }

    #[test]
    fn filter_falls_back_to_cn_uid_without_configured_attrs() {
        // No label rules configured → never emit an empty "(|)" (invalid filter).
        assert_eq!(
            build_leaf_filter(&[], "ann"),
            "(|(cn=*ann*)(uid=*ann*))"
        );
    }

    #[test]
    fn filter_empty_term_matches_everything() {
        assert_eq!(build_leaf_filter(&["cn".to_string()], ""), "(objectClass=*)");
    }

    #[test]
    fn stale_response_is_ignored() {
        let mut f = LeafSearchFlow::new();
        f.force_latest(5_000_001);
        let resp = Response::Entries {
            id: 5_000_000,
            entries: vec![],
            truncated: false,
        };
        assert!(matches!(f.on_response(&resp), LeafSearchOutcome::Ignored));
    }

    #[test]
    fn latest_response_yields_entries_and_truncation() {
        let mut f = LeafSearchFlow::new();
        f.force_latest(5_000_007);
        let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        attrs.insert("cn".to_string(), vec!["Ann".to_string()]);
        let resp = Response::Entries {
            id: 5_000_007,
            entries: vec![LdapEntry {
                dn: "uid=ann,ou=p,dc=x".to_string(),
                attrs,
                bin_attrs: Default::default(),
            }],
            truncated: true,
        };
        match f.on_response(&resp) {
            LeafSearchOutcome::Results { entries, truncated } => {
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].dn, "uid=ann,ou=p,dc=x");
                assert!(truncated);
            }
            other => panic!("expected Results, got {other:?}"),
        }
    }

    #[test]
    fn search_error_for_latest_is_failed() {
        let mut f = LeafSearchFlow::new();
        f.force_latest(5_000_009);
        let resp = Response::SearchError {
            id: 5_000_009,
            msg: "Operations error".to_string(),
        };
        assert!(
            matches!(f.on_response(&resp), LeafSearchOutcome::Failed(m) if m == "Operations error")
        );
    }

    #[test]
    fn fresh_flow_ignores_everything() {
        let mut f = LeafSearchFlow::new();
        let resp = Response::Entries {
            id: 5_000_000,
            entries: vec![],
            truncated: false,
        };
        assert!(matches!(f.on_response(&resp), LeafSearchOutcome::Ignored));
    }
}
```

Register the module in `src/workflows/mod.rs` (alphabetical order, next to `labels`):

```rust
pub mod leaf_search;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -j4 --lib workflows::leaf_search`
Expected: FAIL to compile — `build_leaf_filter`, `LeafSearchFlow`, `LeafSearchOutcome` are undefined.

- [ ] **Step 3: Write the implementation**

Prepend to `src/workflows/leaf_search.rs` (above the test module):

```rust
//! Async one-level search backing the entry list's incremental find.
//!
//! Pane 2 lists the leaf children of the selected container. Its find used to
//! filter the cached [`crate::workflows::structure::Structure`] projection, so an
//! entry another client created was invisible until restart. This flow answers the
//! find from the directory instead: one `SearchScope::OneLevel` query under the
//! selected branch per keystroke, superseded by the next.
//!
//! Id range 5_000_000+ keeps responses disjoint from ReadFlow (1) / WriteFlow
//! (1_000_000) / AllocFlow (2_000_000) / SearchFlow (3_000_000) / ResolveFlow
//! (4_000_000). Only the *latest* id is tracked; a superseded response is dropped,
//! so the list always shows the newest query's answer.
//!
//! No tvision_rs, no crate::ui — pure domain logic.

use anyhow::Result;

use crate::ldap::worker::{LdapEntry, Request, Response, SearchScope, WorkerHandle};
use crate::workflows::pick_state::escape_filter;

/// Result cap for one find. Generous compared with `PICKER_SEARCH_CAP` because
/// this list is the operator's primary navigation surface, not a picker popup.
pub const LEAF_SEARCH_CAP: i32 = 500;

/// Build the RFC-4515 filter for a find over `attrs`.
///
/// - Empty `term` → `(objectClass=*)` (everything in the container).
/// - One attribute → `(cn=*term*)`.
/// - Several → `(|(cn=*term*)(uid=*term*))`.
/// - No attributes configured → falls back to `cn` + `uid`, so the filter can
///   never degenerate into an invalid empty `(|)`.
///
/// `term` is RFC-4515-escaped, so `*`, `(`, `)`, `\` and NUL are literal.
pub fn build_leaf_filter(attrs: &[String], term: &str) -> String {
    if term.is_empty() {
        return "(objectClass=*)".to_string();
    }
    let fallback = ["cn".to_string(), "uid".to_string()];
    let dims: &[String] = if attrs.is_empty() { &fallback } else { attrs };
    let esc = escape_filter(term);
    let parts: Vec<String> = dims
        .iter()
        .map(|a| format!("({a}=*{esc}*)"))
        .collect();
    if parts.len() == 1 {
        parts.into_iter().next().unwrap_or_default()
    } else {
        format!("(|{})", parts.join(""))
    }
}

/// The result of correlating one response against the latest find.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeafSearchOutcome {
    /// The latest find returned these entries (capped at [`LEAF_SEARCH_CAP`]).
    Results {
        entries: Vec<LdapEntry>,
        truncated: bool,
    },
    /// The latest find failed; the caller falls back to the cached projection.
    Failed(String),
    /// The response belongs to a superseded find (or another flow).
    Ignored,
}

/// One-level container search, superseded on every keystroke.
pub struct LeafSearchFlow {
    next_id: u64,
    latest: Option<u64>,
}

impl Default for LeafSearchFlow {
    fn default() -> Self {
        Self::new()
    }
}

impl LeafSearchFlow {
    /// Create a new flow. The first allocated id is 5_000_000.
    pub fn new() -> Self {
        LeafSearchFlow {
            next_id: 5_000_000,
            latest: None,
        }
    }

    fn alloc(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Submit a one-level find for `term` under `branch_dn`.
    ///
    /// `filter_attrs` are the dimensions matched (the column-2 label attributes —
    /// what the operator actually sees); `fetch_attrs` are the attributes returned
    /// (the wider label+tree scan set, so an upserted node carries what the tree
    /// pane needs). Records the id as `latest` and returns it.
    pub fn request(
        &mut self,
        worker: &WorkerHandle,
        branch_dn: &str,
        term: &str,
        filter_attrs: &[String],
        fetch_attrs: &[String],
    ) -> Result<u64> {
        let id = self.alloc();
        let mut attrs = fetch_attrs.to_vec();
        for want in ["cn", "description", "objectClass"] {
            if !attrs.iter().any(|a| a.eq_ignore_ascii_case(want)) {
                attrs.push(want.to_string());
            }
        }
        worker.submit(Request::Search {
            id,
            base: branch_dn.to_string(),
            scope: SearchScope::OneLevel,
            filter: build_leaf_filter(filter_attrs, term),
            attrs,
            size_limit: Some(LEAF_SEARCH_CAP),
        })?;
        self.latest = Some(id);
        Ok(id)
    }

    /// Correlate one worker response. Pure; a non-latest id yields `Ignored`.
    pub fn on_response(&mut self, resp: &Response) -> LeafSearchOutcome {
        match resp {
            Response::Entries {
                id,
                entries,
                truncated,
            } => {
                if Some(*id) != self.latest {
                    return LeafSearchOutcome::Ignored;
                }
                LeafSearchOutcome::Results {
                    entries: entries.clone(),
                    truncated: *truncated,
                }
            }
            Response::SearchError { id, msg } => {
                if Some(*id) != self.latest {
                    return LeafSearchOutcome::Ignored;
                }
                LeafSearchOutcome::Failed(msg.clone())
            }
            _ => LeafSearchOutcome::Ignored,
        }
    }

    /// Test-only: set `latest` without submitting, so `on_response` can be driven
    /// with hand-built responses.
    #[cfg(test)]
    pub(crate) fn force_latest(&mut self, id: u64) {
        self.latest = Some(id);
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -j4 --lib workflows::leaf_search`
Expected: PASS (9 tests).

- [ ] **Step 5: Commit**

```bash
git add src/workflows/leaf_search.rs src/workflows/mod.rs
git commit -m "feat(workflows): LeafSearchFlow — live one-level find for the entry list

Supersede-on-keystroke correlation in the 5_000_000+ id range, RFC-4515
escaped substring filter over the configured label attributes (cn+uid
fallback so the filter can never degenerate to an empty OR).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: Wire the live find into the entry list

**Files:**
- Modify: `src/workflows/labels.rs:86-113` (add `compute_rows_for_dns`)
- Modify: `src/ui/state.rs` (fields, `set_leaf_search`, response correlation, `leaf_rows`, `commit_branch`)
- Modify: `src/ui/panes/leaf.rs:275-279` (submit instead of assigning `search`)
- Test: `src/workflows/labels.rs`, `src/ui/state.rs` (inline tests)

**Interfaces:**
- Consumes: `LeafSearchFlow`, `LeafSearchOutcome`, `LEAF_SEARCH_CAP` (Task 5); `UiState::upsert_from_read` (Task 2); `labels::label_rule_attrs` (existing).
- Produces:
  - `labels::compute_rows_for_dns(structure: &Structure, branch: &str, search: &str, rules: &[LabelRule], dns: &[String]) -> Vec<(String, String)>`
  - `UiState::leaf_search_rows: Option<Vec<String>>`, `UiState::leaf_search_truncated: bool`, `UiState::leaf_search: LeafSearchFlow`
  - `UiState::set_leaf_search(&mut self, query: String)` — the leaf pane's single entry point for a find edit.

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `src/workflows/labels.rs`:

```rust
    #[test]
    fn compute_rows_for_dns_renders_the_given_dns_and_keeps_the_self_row() {
        let s = structure();
        let rules = vec![LabelRule {
            object_classes: vec!["inetOrgPerson".into()],
            template: crate::config::label::parse_label_template("{cn} ({uid})"),
        }];
        let dns = vec!["uid=jane,ou=users,dc=example,dc=org".to_string()];
        let rows = compute_rows_for_dns(&s, "ou=users,dc=example,dc=org", "jane", &rules, &dns);
        // The ‹self› row is filtered by the query exactly as in compute_rows: it
        // does not contain "jane", so only the live hit survives.
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "Jane (jane)");
        assert_eq!(rows[0].1, "uid=jane,ou=users,dc=example,dc=org");
    }

    #[test]
    fn compute_rows_for_dns_skips_branches_and_unknown_dns() {
        let s = structure();
        let dns = vec![
            "ou=users,dc=example,dc=org".to_string(), // a branch: pane 2 shows leaves
            "uid=ghost,ou=users,dc=example,dc=org".to_string(), // not in the model
        ];
        let rows = compute_rows_for_dns(&s, "dc=example,dc=org", "", &[], &dns);
        // Only the ‹self› row for the container remains.
        assert_eq!(rows.len(), 1);
        assert!(rows[0].0.starts_with("‹self›"));
    }
```

Add to `mod tests` in `src/ui/state.rs`:

```rust
    #[test]
    fn leaf_rows_uses_live_results_when_a_query_is_active() {
        let structure = Structure::build(
            "dc=x",
            vec![si("dc=x", None), si("ou=p,dc=x", None), si("uid=a,ou=p,dc=x", Some("A"))],
        );
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.current_branch = Some("ou=p,dc=x".into());

        // No query → the structure projection.
        assert_eq!(st.leaf_rows().len(), 2, "‹self› + uid=a");

        // Query with live results in hand → the live rows.
        st.scan_attrs = vec!["cn".to_string()];
        let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        attrs.insert("cn".to_string(), vec!["Bee".to_string()]);
        st.upsert_from_read("uid=b,ou=p,dc=x", &attrs);
        st.search = "bee".into();
        st.leaf_search_rows = Some(vec!["uid=b,ou=p,dc=x".to_string()]);
        let rows = st.leaf_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, "uid=b,ou=p,dc=x");

        // Query with NO results yet (in flight or failed) → cached filter fallback.
        st.leaf_search_rows = None;
        st.search = "a".into();
        assert_eq!(
            st.leaf_rows().len(),
            1,
            "falls back to filtering the cached projection"
        );
    }

    #[test]
    fn switching_branch_drops_live_results() {
        let structure = Structure::build("dc=x", vec![si("dc=x", None), si("ou=p,dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.search = "ann".into();
        st.leaf_search_rows = Some(vec!["uid=ann,ou=q,dc=x".to_string()]);

        st.commit_branch("ou=p,dc=x".into());

        assert!(st.search.is_empty());
        assert!(
            st.leaf_search_rows.is_none(),
            "another branch's hits must not leak into this one"
        );
    }

    #[test]
    fn empty_query_clears_live_results_without_a_search() {
        let structure = Structure::build("dc=x", vec![si("dc=x", None), si("ou=p,dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.current_branch = Some("ou=p,dc=x".into());
        st.leaf_search_rows = Some(vec!["uid=ann,ou=p,dc=x".to_string()]);

        st.set_leaf_search(String::new());

        assert!(st.leaf_search_rows.is_none());
        assert!(st.list_dirty);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -j4 --lib compute_rows_for_dns leaf_rows_uses_live_results switching_branch_drops_live_results empty_query_clears_live_results`
Expected: FAIL — `compute_rows_for_dns`, `leaf_search_rows`, `set_leaf_search` are undefined.

- [ ] **Step 3: Add `compute_rows_for_dns`**

Append to `src/workflows/labels.rs` after `compute_rows` (line 113):

```rust
/// Rows for a live find: the container's ‹self› row (filtered by `search`, exactly
/// as [`compute_rows`] does) followed by the given `dns` rendered through the label
/// rules and sorted by label.
///
/// DNs the model does not know, and DNs that are branches (pane 2 lists leaves),
/// are skipped — a one-level search returns containers too.
pub(crate) fn compute_rows_for_dns(
    structure: &Structure,
    branch: &str,
    search: &str,
    rules: &[LabelRule],
    dns: &[String],
) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    let q = search.to_lowercase();
    if let Some(node) = structure.get(branch) {
        let self_label = format!("‹self› {}", node.label);
        if q.is_empty() || self_label.to_lowercase().contains(&q) {
            rows.push((self_label, branch.to_string()));
        }
    }
    let mut hits: Vec<(String, String)> = dns
        .iter()
        .filter_map(|dn| structure.get(dn))
        .filter(|n| !n.is_branch())
        .map(|n| {
            (
                render_node_label(rules, &n.object_classes, &n.attrs, &n.label),
                n.dn.clone(),
            )
        })
        .collect();
    hits.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    rows.extend(hits);
    rows
}
```

- [ ] **Step 4: Add the state fields and the submit path**

In `src/ui/state.rs`, import the flow next to the other workflow imports at the top of the file:

```rust
use crate::workflows::leaf_search::{LeafSearchFlow, LeafSearchOutcome};
```

Add to the `UiState` struct after `search` (line 120):

```rust
    /// Live one-level find backing the entry list (supersedes on every keystroke).
    pub leaf_search: LeafSearchFlow,
    /// DNs returned by the newest find, or `None` when no find is active / none has
    /// landed yet. `leaf_rows` falls back to filtering the cached projection then.
    pub leaf_search_rows: Option<Vec<String>>,
    /// True when the newest find hit `LEAF_SEARCH_CAP`.
    pub leaf_search_truncated: bool,
```

Add the initialisers to BOTH `new_for_test` and `bootstrap`:

```rust
            leaf_search: LeafSearchFlow::new(),
            leaf_search_rows: None,
            leaf_search_truncated: false,
```

Add these methods in the `impl UiState` block containing `commit_branch` (after `commit_branch`, `src/ui/state.rs:1065`):

```rust
    /// The entry list's find changed: mirror the query and answer it from the
    /// directory. An empty query drops the live rows and returns pane 2 to the
    /// container listing; a non-empty one submits a fresh one-level search whose
    /// predecessor (if any) is superseded. No-op without a worker or a branch.
    pub fn set_leaf_search(&mut self, query: String) {
        self.search = query;
        self.list_dirty = true;
        if self.search.is_empty() {
            self.leaf_search_rows = None;
            self.leaf_search_truncated = false;
            return;
        }
        let Some(branch) = self.current_branch.clone() else {
            return;
        };
        let filter_attrs = crate::workflows::labels::label_rule_attrs(&self.label_rules);
        let Self {
            worker,
            leaf_search,
            scan_attrs,
            search,
            ..
        } = self;
        if let Some(w) = worker.as_ref() {
            let _ = leaf_search.request(w, &branch, search, &filter_attrs, scan_attrs);
        }
    }

    /// Apply one non-ignored find outcome.
    ///
    /// `Results`: upsert every hit into the structure (so entries other clients
    /// created become permanent local nodes, not transient rows), then keep their
    /// DNs as the list's row source. `Failed`: surface the error and drop back to
    /// the cached projection so the pane is never blank over a transient failure.
    pub fn apply_leaf_search_outcome(&mut self, out: LeafSearchOutcome) {
        match out {
            LeafSearchOutcome::Results { entries, truncated } => {
                let mut dns = Vec::with_capacity(entries.len());
                for e in &entries {
                    self.upsert_from_read(&e.dn, &e.attrs);
                    dns.push(e.dn.clone());
                }
                self.leaf_search_rows = Some(dns);
                self.leaf_search_truncated = truncated;
                if truncated {
                    self.status = format!(
                        "Showing the first {} matches — narrow the search.",
                        crate::workflows::leaf_search::LEAF_SEARCH_CAP
                    );
                }
                self.list_dirty = true;
            }
            LeafSearchOutcome::Failed(msg) => {
                self.status = format!("Search failed: {msg}");
                self.leaf_search_rows = None;
                self.leaf_search_truncated = false;
                self.list_dirty = true;
            }
            LeafSearchOutcome::Ignored => {}
        }
    }
```

- [ ] **Step 5: Correlate the responses and switch the row source**

In `process_responses` (`src/ui/state.rs`), insert a new correlation block immediately **before** the candidate-search block (line 358, `let s_out = self.search_flow.on_response(resp);`):

```rust
            // Entry-list find: Entries/SearchError with leaf-search ids (5_000_000+).
            let l_out = self.leaf_search.on_response(resp);
            if !matches!(l_out, LeafSearchOutcome::Ignored) {
                self.apply_leaf_search_outcome(l_out);
                out.changed = true;
                continue;
            }
```

Replace `leaf_rows` (`src/ui/state.rs:1071-1081`) with:

```rust
    /// (label, dn) rows for the current branch, using the configured column-2 label
    /// rules. Empty when no branch is selected.
    ///
    /// | State | Source |
    /// |---|---|
    /// | no query | the structure projection |
    /// | query + live results | those results, rendered and sorted by label |
    /// | query, none landed yet or the find failed | the cached projection, filtered |
    ///
    /// This is the single row source for the pane: the list's selection index maps
    /// 1:1 onto it, so the selection→DN mapping stays correct in every state.
    pub fn leaf_rows(&self) -> Vec<(String, String)> {
        let Some(branch) = self.current_branch.as_deref() else {
            return Vec::new();
        };
        match (self.search.is_empty(), self.leaf_search_rows.as_deref()) {
            (false, Some(dns)) => crate::workflows::labels::compute_rows_for_dns(
                &self.structure,
                branch,
                &self.search,
                &self.label_rules,
                dns,
            ),
            _ => crate::workflows::labels::compute_rows(
                &self.structure,
                branch,
                &self.search,
                &self.label_rules,
            ),
        }
    }
```

In `commit_branch` (line 1061), add after `self.search = String::new();`:

```rust
        // Another container's live hits must not leak into this one.
        self.leaf_search_rows = None;
        self.leaf_search_truncated = false;
```

- [ ] **Step 6: Point the leaf pane at the new entry point**

In `src/ui/panes/leaf.rs`, replace lines 275-279 with:

```rust
        if cur != self.last_search {
            self.last_search = cur.clone();
            // Answer the find from the directory (superseding any in-flight one),
            // never from the cached projection: an entry another client created must
            // be findable without a restart.
            self.state.borrow_mut().set_leaf_search(cur);
            self.repopulate(ctx);
        }
```

Also update the module doc at `src/ui/panes/leaf.rs:2` and the comment block at lines 262-267 to say the rows come from a live one-level search while a query is active (the `Highlight` mode contract — the pane supplies already-filtered rows — is unchanged).

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -j4 --lib labels leaf_rows switching_branch empty_query`
Expected: PASS.

- [ ] **Step 8: Run the full check**

Run: `make check`
Expected: green.

- [ ] **Step 9: Commit**

```bash
git add src/workflows/labels.rs src/ui/state.rs src/ui/panes/leaf.rs
git commit -m "feat(leaf): answer the entry list's find from the directory

A find now submits a one-level search under the selected container; hits are
upserted into the structure and become the row source. In-flight or failed
searches fall back to the cached projection, so the pane is never blank.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: The lookup combobox re-queries per keystroke

**Files:**
- Modify: `src/ui/lookup.rs:1-11` (module doc), `:112-126` (struct doc), `:164` (find mode), `:202-212` (submit), `:375-390` (typed-change branch)
- Test: `src/ui/lookup.rs` (inline `mod dialog_tests`)

**Interfaces:**
- Consumes: `UiState::submit_search` (existing, `src/ui/state.rs:512`).
- Produces: no new API — behaviour only.

- [ ] **Step 1: Write the failing test**

Add to `mod dialog_tests` in `src/ui/lookup.rs` (it already has `shared_with_candidates`, and the shared state records submitted requests only through the worker, which is `None` in tests — so assert on the *find mode contract* and the query plumbing instead):

```rust
    /// The candidate list must NOT narrow itself: with the list server-backed, the
    /// dialog owns which rows exist and the list only highlights matches. A local
    /// `FindMode::Filter` would hide rows the server just returned.
    #[test]
    fn candidate_list_highlights_rather_than_filters() {
        let shared = shared_with_candidates(vec![("100", "users"), ("5000", "staff")]);
        let binding = test_binding();
        let mut dlg = LookupDialog::new(binding, "5000 (staff)".into(), shared);

        let mut out = std::collections::VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<tv::Deferred> = Vec::new();
        let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        dlg.handle_event(&mut ev, &mut ctx);

        // Type a query that matches only one candidate; the list keeps BOTH rows
        // (the server, not the list, decides the row set) and highlights the match.
        dlg.set_input("staff", &mut ctx);
        let mut ev = Event::KeyDown(tv::KeyEvent::from_key(Key::Char('f')));
        dlg.handle_event(&mut ev, &mut ctx);
        assert_eq!(
            dlg.list_text_for_test().len(),
            2,
            "rows come from the server's answer, not from local narrowing"
        );
    }
```

Add the test helper `list_text_for_test` to `impl LookupDialog` (mirroring `LeafPane::list_text_for_test`):

```rust
    #[cfg(test)]
    pub(crate) fn list_text_for_test(&mut self) -> Vec<String> {
        self.dlg
            .child_mut(self.list_id)
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<ListBox>())
            .map(|lb| lb.list().to_vec())
            .unwrap_or_default()
    }
```

If `test_binding()` does not already exist in `mod dialog_tests`, reuse the binding construction from the existing tests there (`LookupBinding { … }` with `CandidateScope`), extracted into a helper.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -j4 --lib lookup::dialog_tests::candidate_list_highlights_rather_than_filters`
Expected: FAIL — with `FindMode::Filter` the list narrows to 1 row.

- [ ] **Step 3: Switch the list to `Highlight` and re-query on typing**

In `src/ui/lookup.rs:164`, change:

```rust
        let list = ListBox::new(Rect::new(2, 4, 62, 18), 1, None, None).with_find(FindMode::Filter);
```

to:

```rust
        // The SERVER decides which candidates exist: `FindMode::Highlight` marks the
        // matched substring without hiding rows. Narrowing locally would cap the
        // reachable candidates at whatever the last query returned.
        let list =
            ListBox::new(Rect::new(2, 4, 62, 18), 1, None, None).with_find(FindMode::Highlight);
```

Generalise `submit_load` (line 202) into a term-taking submit:

```rust
    /// Submit a candidate search for `term` (empty = load all, capped by the
    /// server-side `PICKER_SEARCH_CAP`). Called on open and on every typed change,
    /// so a candidate ranked past the cap is still reachable by typing.
    fn submit_query(&self, term: &str) {
        let attrs = self.label_attrs();
        self.shared.borrow_mut().submit_search(
            &self.binding.scope.base,
            self.binding.object_class(),
            term,
            &attrs,
            Some(&self.binding.store),
        );
    }
```

Replace both `self.submit_load();` calls (in `reset_current` and `handle_event`, lines 321 and 330) with `self.submit_query("");`.

In the typed-change branch (lines 378-390), add the re-query:

```rust
        let cur = self.current_input();
        if cur != self.last_input {
            self.last_input = cur.clone();
            // Ask the SERVER for candidates matching the typed text. `mirror_focused`
            // already syncs `last_input` when it writes a picked row back, so a pick
            // never round-trips as a query.
            self.submit_query(&cur);
            if let Some(lb) = self
                .dlg
                .child_mut(self.list_id)
                .and_then(|v| v.as_any_mut())
                .and_then(|a| a.downcast_mut::<ListBox>())
            {
                lb.set_find_query(&cur, ctx);
            }
            self.sync_ok(ctx);
        }
```

- [ ] **Step 4: Update the docs in the file**

Change the module doc (lines 7-11) and the `LookupDialog` struct doc (lines 112-114) so both say: the input drives a **server-side** candidate query per keystroke, and the list highlights matches within the returned rows rather than narrowing a one-shot load.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -j4 --lib lookup`
Expected: PASS (the new test plus the existing dialog tests — the pick/mirror tests must stay green, since `mirror_focused`'s `last_input` sync is what keeps a pick from re-querying).

- [ ] **Step 6: Commit**

```bash
git add src/ui/lookup.rs
git commit -m "feat(lookup): re-query the server as the operator types

The combobox loaded candidates once with an empty term, capped at
PICKER_SEARCH_CAP (100), and narrowed that set locally — so a candidate past
the cap was unreachable and one created later never appeared. Typing now
drives a fresh search; the list highlights instead of filtering.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 8: Alt+R reload

**Files:**
- Modify: `src/ui/mod.rs:41-53` (new `RELOAD` command)
- Modify: `src/ui/app.rs:37-48` (menu entry), `:115+` (dispatch arm)
- Modify: `src/ui/state.rs` (add `reload_structure`)
- Delete: `src/app.rs` (dead `UiAction` vocabulary); modify `src/lib.rs:4`
- Test: `src/ui/state.rs` (inline tests)

**Interfaces:**
- Consumes: `UiState::scan_attrs` (Task 2), `UiState::tree_dirty` (Task 2), `leaf_search_rows` (Task 6).
- Produces: `pub const RELOAD: tv::Command`, `UiState::reload_structure(&mut self)`, and `UiState::adopt_structure(&mut self, structure: Structure)` (the pure half, unit-tested without a worker).

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `src/ui/state.rs`:

```rust
    #[test]
    fn adopt_structure_keeps_a_still_existing_branch_and_leaf() {
        let old = Structure::build(
            "dc=x",
            vec![si("dc=x", None), si("ou=p,dc=x", None), si("uid=a,ou=p,dc=x", Some("A"))],
        );
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st = UiState::new_for_test(old, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.current_branch = Some("ou=p,dc=x".into());
        st.current_leaf = Some("uid=a,ou=p,dc=x".into());
        st.search = "zzz".into();
        st.leaf_search_rows = Some(vec!["uid=a,ou=p,dc=x".to_string()]);
        st.lookup_cache.insert(
            crate::workflows::resolve_flow::member_key("uid=a,ou=p,dc=x"),
            Some("A".into()),
        );

        let fresh = Structure::build(
            "dc=x",
            vec![si("dc=x", None), si("ou=p,dc=x", None), si("uid=a,ou=p,dc=x", Some("A"))],
        );
        st.adopt_structure(fresh);

        assert_eq!(st.current_branch.as_deref(), Some("ou=p,dc=x"));
        assert_eq!(st.current_leaf.as_deref(), Some("uid=a,ou=p,dc=x"));
        assert!(st.search.is_empty());
        assert!(st.leaf_search_rows.is_none());
        assert!(st.lookup_cache.is_empty());
        assert!(st.list_dirty);
        assert!(st.tree_dirty);
    }

    #[test]
    fn adopt_structure_falls_back_when_the_branch_is_gone() {
        let old = Structure::build("dc=x", vec![si("dc=x", None), si("ou=p,dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st = UiState::new_for_test(old, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.current_branch = Some("ou=gone,dc=x".into());
        st.current_leaf = Some("uid=ghost,ou=gone,dc=x".into());

        let fresh = Structure::build("dc=x", vec![si("dc=x", None), si("ou=p,dc=x", None)]);
        st.adopt_structure(fresh);

        assert_eq!(
            st.current_branch.as_deref(),
            Some("dc=x"),
            "a vanished container falls back to the base DN"
        );
        assert_eq!(st.current_leaf, None);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -j4 --lib adopt_structure`
Expected: FAIL — `adopt_structure` is undefined.

- [ ] **Step 3: Implement `adopt_structure` + `reload_structure`**

Add to `src/ui/state.rs`, in the `impl UiState` block that holds `upsert_from_read`:

```rust
    /// Install a freshly scanned structure, keeping the operator's place.
    ///
    /// The current container and entry are preserved **by DN** when they still
    /// exist; a vanished container falls back to the base DN and a vanished entry to
    /// no selection. Every projection derived from the old scan is dropped: the find
    /// query, its live rows, and the reverse-label cache (which caches negatives, so
    /// a stale miss would otherwise outlive the refresh). Pure — no I/O — so the
    /// place-keeping rules are unit-testable.
    pub fn adopt_structure(&mut self, structure: Structure) {
        self.structure = structure;
        if let Some(branch) = self.current_branch.clone() {
            if self.structure.get(&branch).is_none() {
                self.current_branch = Some(self.base_dn.clone());
            }
        }
        if let Some(leaf) = self.current_leaf.clone() {
            if self.structure.get(&leaf).is_none() {
                self.current_leaf = None;
            }
        }
        self.search.clear();
        self.leaf_search_rows = None;
        self.leaf_search_truncated = false;
        self.lookup_cache.clear();
        self.list_dirty = true;
        self.tree_dirty = true;
    }

    /// Re-run the eager structure scan and adopt the result (Alt+R).
    ///
    /// Blocking, like the bootstrap scan it repeats: the TUI is unresponsive for its
    /// duration, which is acceptable for an explicit, operator-initiated action. The
    /// open edit form is deliberately left untouched, so unsaved work is never at
    /// risk and no dirty-form guard is needed. On failure the previous structure is
    /// kept and the error is surfaced in the status line.
    pub fn reload_structure(&mut self) {
        let Some(worker) = self.worker.as_ref() else {
            return;
        };
        let resp = worker.request(Request::LoadStructure {
            id: 0,
            base: self.base_dn.clone(),
            page_size: 500,
            attrs: self.scan_attrs.clone(),
        });
        match resp {
            Ok(Response::StructureEntries { nodes, .. }) => {
                let count = nodes.len();
                let structure = Structure::build(
                    &self.base_dn,
                    crate::workflows::labels::structure_inputs(nodes),
                );
                self.adopt_structure(structure);
                self.status = format!("Reloaded {count} entries.");
            }
            Ok(Response::StructureError { msg, .. }) => {
                self.status = format!("Reload failed: {msg}");
            }
            Ok(other) => {
                self.status = format!("Reload failed: unexpected {other:?}");
            }
            Err(e) => {
                self.status = format!("Reload failed: {e}");
            }
        }
    }
```

(`Response::StructureError { id, msg, truncated }` is the real shape — `src/ldap/worker.rs:229`; the `truncated` flag is deliberately ignored here because the message already carries the limit text.)

- [ ] **Step 4: Add the command, the menu entry and the dispatch arm**

In `src/ui/mod.rs`, next to the other command constants (after `STARTUP`, line 53):

```rust
/// Re-run the eager structure scan (Alt+R) — the escape hatch for structure
/// staleness that no local reflow can see (another client created a container).
pub const RELOAD: tv::Command = tv::Command::custom("edaptor.reload");
```

In `src/ui/app.rs`, extend the File menu (line 42-44):

```rust
            m.command_key("~N~ew", CREATE, alt('n'), "Alt-N")
                .command_key("~S~ave", SAVE, alt('s'), "Alt-S")
                .command_key("~R~eload", RELOAD, alt('r'), "Alt-R")
                .command_key("E~x~it", REQUEST_QUIT, alt('x'), "Alt-X")
```

and import `RELOAD` alongside the other commands at the top of the file.

Add the dispatch arm in `dispatch` (after the `SAVE` arm, `src/ui/app.rs:126`):

```rust
    } else if cmd == RELOAD {
        // Blocking rescan; the pump broadcasts REFRESH for the list/tree because
        // adopt_structure marks both dirty.
        state.borrow_mut().reload_structure();
```

- [ ] **Step 5: Delete the dead `UiAction` vocabulary**

`src/app.rs` declares a `UiAction` enum (including a `Refresh` variant documented as Alt+R) that has **no** producers or consumers — the real mechanism is the command/dispatch pair added above. Leaving it would document a keybinding twice, in one case falsely.

```bash
git rm src/app.rs
```

Then remove `pub mod app;` from `src/lib.rs:4`.

Run: `grep -rn "UiAction\|crate::app::" --include=*.rs src/ tests/`
Expected: no output.

- [ ] **Step 6: Run the tests and the full check**

Run: `cargo test -j4 --lib adopt_structure && make check`
Expected: PASS, then green.

- [ ] **Step 7: Manual smoke (tmux harness)**

```bash
scripts/test-ldap.sh start
export EDAPTOR_TEST_ADMIN_PW=adminpassword
tmux new-session -d -s ed -x 200 -y 50
tmux send-keys -t ed "cargo run -- --config examples/demo-config.toml" Enter
# create an entry from a second shell with ldapadd, then in the TUI:
tmux send-keys -t ed M-r
tmux capture-pane -t ed -p | head -30
```
Expected: the status line reports `Reloaded N entries.` and the out-of-band entry is listed.

- [ ] **Step 8: Commit**

```bash
git add -A src/ui/mod.rs src/ui/app.rs src/ui/state.rs src/lib.rs src/app.rs
git commit -m "feat(ui): Alt+R reloads the DIT scan; drop the dead UiAction enum

Blocking rescan that keeps the operator's place by DN, clears the derived
caches and leaves the edit form untouched. src/app.rs declared a UiAction
vocabulary with no producers or consumers, including a Refresh variant that
documented a keybinding that did not exist.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 9: Live test and documentation

**Files:**
- Create: `tests/live_search.rs`
- Create: `docs/src/concepts/live-data.md`
- Modify: `docs/src/SUMMARY.md`, `CHANGES.md`, `README.md` (only if it describes list/search behaviour)
- Test: `tests/live_search.rs`

**Interfaces:**
- Consumes: `LeafSearchFlow` (Task 5), the worker `Request::Add` path (existing).
- Produces: nothing consumed by later tasks (final task).

- [ ] **Step 1: Write the gated live test**

Create `tests/live_search.rs`, following the harness style of `tests/live_write.rs` (copy its `test_config` and `poll_for_id` helpers verbatim — they are per-file helpers in this suite):

```rust
//! Live test for the entry list's server-backed incremental find.
//!
//! Enable by setting EDAPTOR_TEST_LDAP_URI (e.g. ldap://localhost:1389).
//! Start a server with: scripts/test-ldap.sh start
//! When the env var is unset the test prints SKIP and passes.

// … test_config() and poll_for_id() copied from tests/live_write.rs …

#[test]
fn find_sees_an_entry_created_out_of_band() {
    let Ok(uri) = std::env::var("EDAPTOR_TEST_LDAP_URI") else {
        println!("SKIP: EDAPTOR_TEST_LDAP_URI unset");
        return;
    };
    let (config, password) = test_config(uri);
    let worker = WorkerHandle::spawn(config, password).expect("worker");

    // The "other client": add an entry the running edaptor never saw.
    let dn = "uid=coherence-probe,ou=people,dc=example,dc=org";
    let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    attrs.insert(
        "objectClass".into(),
        vec!["inetOrgPerson".into(), "organizationalPerson".into(), "person".into(), "top".into()],
    );
    attrs.insert("cn".into(), vec!["Coherence Probe".into()]);
    attrs.insert("sn".into(), vec!["Probe".into()]);
    attrs.insert("uid".into(), vec!["coherence-probe".into()]);
    let add_id = 900_001;
    worker
        .submit(Request::Add { id: add_id, dn: dn.to_string(), attrs })
        .expect("submit add");
    let resp = poll_for_id(&worker, add_id, Duration::from_secs(10)).expect("add reply");
    assert!(matches!(resp, Response::WriteOk { .. }), "add failed: {resp:?}");

    // The find must see it without any structure rescan.
    let mut flow = LeafSearchFlow::new();
    let id = flow
        .request(
            &worker,
            "ou=people,dc=example,dc=org",
            "coherence-probe",
            &["cn".to_string(), "uid".to_string()],
            &["cn".to_string(), "uid".to_string()],
        )
        .expect("submit find");
    let resp = poll_for_id(&worker, id, Duration::from_secs(10)).expect("find reply");
    match flow.on_response(&resp) {
        LeafSearchOutcome::Results { entries, .. } => {
            assert!(
                entries.iter().any(|e| e.dn.eq_ignore_ascii_case(dn)),
                "the out-of-band entry must be findable: {entries:?}"
            );
        }
        other => panic!("expected Results, got {other:?}"),
    }

    // Clean up so repeated runs stay idempotent.
    let del_id = 900_002;
    worker
        .submit(Request::Delete { id: del_id, dn: dn.to_string(), assert_csn: None })
        .expect("submit delete");
    let _ = poll_for_id(&worker, del_id, Duration::from_secs(10));
}
```

(Verified field lists: `Request::Add { id, dn, attrs }` at `src/ldap/worker.rs:146`, `Request::Delete { id, dn, assert_csn }` at `:176`.)

- [ ] **Step 2: Run the live test both ways**

Run without a server: `cargo test -j4 --test live_search`
Expected: PASS with `SKIP: EDAPTOR_TEST_LDAP_URI unset` printed.

Run against the demo server:
```bash
scripts/test-ldap.sh start
export EDAPTOR_TEST_ADMIN_PW=adminpassword
EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389 cargo test -j4 --test live_search -- --nocapture
```
Expected: PASS.

- [ ] **Step 3: Write the mdBook page**

Create `docs/src/concepts/live-data.md`:

```markdown
# Live data, find and reload

eDAPtor loads the directory tree once at startup and keeps that model in step
with the directory as you work. Three mechanisms do it.

## Searching always asks the server

Typing in the entry list runs a one-level search under the selected container,
matching the attributes your `label` template renders. An entry another
administrator created seconds ago is therefore findable immediately — the find
is never answered from the copy loaded at startup. Matches are folded into the
local model, so they stay listed after you clear the query.

The lookup field's candidate list works the same way: every keystroke asks the
server, so candidates beyond the first page are reachable by typing.

At most 500 matches are returned per find; when that limit is hit the status
line says so and asks you to narrow the query.

## Writes update the list immediately

Creating an entry adds it to the list and selects it. Renaming one moves it.
Editing an attribute that appears in a label re-renders that label in the tree
and the entry list. This happens without a rescan: every entry eDAPtor reads —
including the read that follows a save — refreshes that entry in the model.

## Alt+R reloads the tree

Structural changes made by other clients (a new container, a deleted subtree)
cannot be observed locally. **Alt+R** re-runs the full scan, keeping your place:
the selected container and entry are restored when they still exist. The scan
blocks briefly on large directories. Your open edit form is left untouched, so
unsaved changes are never at risk.
```

Register it in `docs/src/SUMMARY.md` under the same Concepts group that holds `optimistic-concurrency.md`, matching that file's indentation exactly.

- [ ] **Step 4: Update `CHANGES.md`**

Add under the current unreleased section:

```markdown
- Newly created entries now appear in the entry list immediately (and are
  selected) instead of only after a restart; renamed entries move rather than
  leaving a stale row behind.
- The entry list's incremental find is answered by the server (one-level search
  under the selected container), so entries created by other clients are found
  without a restart. Capped at 500 matches with a status-line notice.
- The `lookup` field's candidate list re-queries the server as you type instead
  of filtering a one-shot, capped load — candidates past the first 100 are now
  reachable.
- New **Alt+R** (File → Reload) re-runs the directory scan, keeping the selected
  container and entry; the open edit form is left untouched.
- Editing an attribute used in a tree or list label now refreshes that label in
  place, and the cached reverse-label lookups are dropped after every write
  (they previously kept showing values eDAPtor itself had changed).
```

- [ ] **Step 5: Check the README**

Run: `grep -n "search\|find\|refresh\|reload" README.md`
If the README describes list/search behaviour, update those sentences to match (live find, Alt+R) and link to `concepts/live-data.md`. If it does not, leave it alone — the README is an overview and must not restate the reference docs.

- [ ] **Step 6: Build the docs and run the full check**

Run: `make docs && make check`
Expected: mdBook builds with no warnings about the new page; `make check` green.

- [ ] **Step 7: Commit**

```bash
git add tests/live_search.rs docs/src/concepts/live-data.md docs/src/SUMMARY.md CHANGES.md README.md
git commit -m "docs+test: live-data page, changelog, and a gated find live test

Proves an entry created out-of-band is findable through LeafSearchFlow without
a rescan, and documents live find, after-write reflow and Alt+R.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Final verification

- [ ] `make check` green (fmt, clippy `-D warnings`, all tests).
- [ ] `cargo test -j4` with the demo server up and `EDAPTOR_TEST_LDAP_URI` exported — the gated live tests pass, **except** the known pre-existing failure `tests/live_templates.rs::picker_gidnumber_scalar_store_resolves_group_gidnumber`, which also fails on `main` and is unrelated to this branch.
- [ ] Manual TUI smoke against the demo server, covering the four user-visible behaviours:
  1. Create an entry → it appears in the list and is selected, with no stale find query hiding it.
  2. Rename an entry (edit its RDN attribute) → the row moves; no stale row remains.
  3. From a second shell, `ldapadd` an entry into the shown container → type its name in the entry list → it is found.
  4. Open a `lookup` field, type a group name that sorts past the first 100 candidates → it appears.
  5. Alt+R → status reports the entry count, the selected container and entry are still selected.
