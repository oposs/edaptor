# Rich User Templates — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make entry profiles rich enough to onboard a real posix(+Samba) user in one create flow — multi-objectClass templates, defaulted/templated/auto-numbered fields, an inline password field, a value-lookup picker, and a profile chooser — after first unifying "create" into the pane-3 edit form.

**Architecture:** The shared `EditForm` widget gains a `FormMode` (Edit | Create) so NEW is a pane-3 form whose save does an `Add` instead of a `Modify`; the modal `Overlay::CreateForm` is deleted. A new pure `config::defaults` engine parses `[profile.defaults]` values (literal / `{attr}` template / `{next:MIN-MAX}` autonumber) and plans which empty fields to fill; the autonumber scan runs through the existing synchronous `worker.request` seam and refuses to allocate on a truncated result. Password staging reuses M5's `nt_hash`. The value-lookup picker reuses the membership `PickerState`/search infrastructure in a single-select variant.

**Tech Stack:** Rust, ratatui 0.30 (facade-isolated under `src/ui/*`), `ldap3` worker thread, `toml`/`serde`, `md4` (NT hash). Strict TDD, atomic commits, `cargo fmt`/`cargo clippy -D warnings` green after every commit.

**Spec:** [`docs/superpowers/specs/2026-06-02-rich-user-templates-design.md`](../specs/2026-06-02-rich-user-templates-design.md)

---

## Shared types (defined once; used across tasks — keep names exact)

```rust
// src/ui/edit_form.rs — added in Phase 0
pub enum FormMode {
    Edit,
    Create { profile_idx: usize, container: String },
}
// EditForm gains:  pub mode: FormMode,
// EditForm gains:  pub fn is_new(&self) -> bool { matches!(self.mode, FormMode::Create { .. }) }

// src/config/defaults.rs — added in Phase 2
pub enum Seg { Lit(String), Field(String) }                 // a template segment
pub enum DefaultValue {
    Literal(String),
    Template(Vec<Seg>),
    AutoNumber { min: u64, max: u64 },
}
#[derive(Default)]
pub struct ProfileDefaults { pub entries: std::collections::BTreeMap<String, DefaultValue> }
pub enum Resolution {
    Fill { attr: String, value: String },
    NeedsAutonumber { attr: String, min: u64, max: u64 },
}
pub fn parse_default_value(s: &str) -> Result<DefaultValue, String>;
pub fn plan_defaults(d: &ProfileDefaults,
                     current: &std::collections::BTreeMap<String, Vec<String>>) -> Vec<Resolution>;
pub fn next_in_range(existing: &[u64], min: u64, max: u64) -> Result<u64, String>;

// src/config/mod.rs — Phase 1 & 4 & 5
pub struct PasswordSpec { pub ldap_attribute: String, pub samba: bool }   // default attr "userPassword"
pub struct LookupSpec {
    pub object_class: String,
    pub search_base: String,
    pub value_attr: String,
    pub label: String,
    pub search_attrs: Vec<String>,
}
// EntryProfile gains:
//   pub object_classes: Vec<String>,                 // REPLACES object_class: String
//   pub defaults: ProfileDefaults,                   // #[serde(default)]
//   pub password: Option<PasswordSpec>,              // #[serde(default)]
//   pub lookups: BTreeMap<String, LookupSpec>,       // #[serde(default, rename = "lookup")]

// src/workflows/create.rs — Phase 6
pub fn profiles_for_container(profiles: &[EntryProfile], container_dn: &str) -> Vec<usize>;

// src/ldap/worker.rs — Phase 3
// Response::Entries gains:  truncated: bool
// run_search returns:       Result<(Vec<LdapEntry>, bool)>
```

**Conventions for every task below (the TDD rhythm):** write the failing test → run it and confirm it fails for the stated reason → write the minimal code → run it and confirm pass → run `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo build --all-targets` → commit. Commit trailer on every commit: `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.

---

# Phase 0 — Create-host unification (foundational; lands green before anything else)

**Why first:** collapses the modal create popup into the pane-3 form so every later feature wires into one host. Pure refactor — no new template behaviour. After this phase the app behaves exactly as before from the user's view, but NEW renders in pane 3.

**Parity verified (do not re-derive):** single-value field editing is identical in both handlers — direct char input via `field.editor.handle_key_event(key)` guarded by `field.editable && !field.multi` (pane-3 `edit_focused_field` app.rs:657; `create_form_key` app.rs:1125). Focus nav (Up/Down) and F2/F3 semantics also match. The only delta is Enter: the pane-3 handler calls `open_value_editor`, which is a **no-op on a single-value field** — and Task 0.2 forces every editable create field to `multi=false`, so create behaviour is preserved. The unit tests below set values via `set_field_value` (bypassing key routing); **Task 0.6's tmux smoke is the hard gate** that exercises real key routing — treat it as required, not optional.

### Task 0.1: Add `FormMode` to `EditForm`

**Files:**
- Modify: `src/ui/edit_form.rs` (struct `EditForm` ~line 183; `build_edit_form` ~line 281)
- Test: `src/ui/edit_form.rs` (`#[cfg(test)]` module)

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn editform_mode_defaults_to_edit_and_reports_not_new() {
    use crate::ui::form::{FormModel, FormField};
    let model = FormModel { title: "cn=x,dc=example,dc=org".into(), fields: vec![] };
    let form = build_edit_form(&model, &empty_schema(), false, &[]);
    assert!(matches!(form.mode, FormMode::Edit));
    assert!(!form.is_new());
}
```

Add a tiny `empty_schema()` test helper if one is not already present in the module:

```rust
fn empty_schema() -> SchemaModel {
    SchemaModel::from_raw(&crate::ldap::worker::RawSubschema {
        object_classes: vec![], attribute_types: vec![], ldap_syntaxes: vec![],
    })
}
```

- [ ] **Step 2: Run test, confirm fail**

Run: `cargo test -p edaptor editform_mode_defaults_to_edit -- --nocapture`
Expected: FAIL — no field `mode` / no `FormMode` / no `is_new`.

- [ ] **Step 3: Implement**

In `src/ui/edit_form.rs` add the enum and field:

```rust
/// Whether the form edits an existing entry or composes a new one.
pub enum FormMode {
    /// Editing an entry already in the directory (diff against `baseline`).
    Edit,
    /// Composing a new entry of `profile_idx`, to be added under `container`.
    Create { profile_idx: usize, container: String },
}

// in `pub struct EditForm { ... }` add as the last field:
    /// Edit an existing entry, or compose a new one (Create → Add on save).
    pub mode: FormMode,
```

Add to `impl EditForm`:

```rust
/// True when this form composes a not-yet-saved new entry.
pub fn is_new(&self) -> bool {
    matches!(self.mode, FormMode::Create { .. })
}
```

In `build_edit_form`, set `mode: FormMode::Edit` in the returned `EditForm { .. }`. Search the crate for every other `EditForm { ` struct-literal (tests in `view.rs` ~line 646/802, `app.rs` tests) and add `mode: FormMode::Edit,`.

- [ ] **Step 4: Run, confirm pass**

Run: `cargo test -p edaptor editform_mode_defaults_to_edit`
Expected: PASS. Then `cargo build --all-targets` to surface every struct-literal that now needs `mode:` and fix each.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "refactor(ui): add FormMode to EditForm (Edit default)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

### Task 0.2: `build_create_form` — build a Create-mode pane-3 form

**Files:**
- Modify: `src/workflows/create.rs` (has `empty_form_for_profile`)
- Modify: `src/ui/edit_form.rs` (a thin constructor) OR `src/ui/app.rs`
- Test: `src/ui/app.rs` tests

**Context:** Today `UiAction::NewEntry(i)` (app.rs ~914) builds an `Overlay::CreateForm`. We replace that with an `app.form` carrying `FormMode::Create`. The form is built exactly as today: `empty_form_for_profile` → `build_edit_form(model, schema, false, &[])` → force every editable field to single-value (`field.multi = false`).

- [ ] **Step 1: Write the failing test** (in `src/ui/app.rs` tests)

```rust
#[test]
fn new_entry_installs_create_mode_form_in_pane3() {
    let mut app = test_app();                 // existing test helper; if absent, see Task 0.5 note
    let profiles = vec![test_user_profile()]; // object_classes=["inetOrgPerson"], rdn_attr="uid", search_base="ou=people,dc=example,dc=org"
    let mut read_flow = test_read_flow();
    let mut structure = test_structure();
    let worker = test_worker();
    handle_ui_action(&mut app, UiAction::NewEntry(0), &worker, &mut read_flow,
                     &mut structure, &profiles, "dc=example,dc=org");
    let form = app.form.as_ref().expect("create form installed in pane 3");
    assert!(form.is_new());
    assert!(app.overlay.is_none(), "no modal overlay for create anymore");
    match &form.mode {
        FormMode::Create { profile_idx, container } => {
            assert_eq!(*profile_idx, 0);
            assert_eq!(container, "ou=people,dc=example,dc=org");
        }
        _ => panic!("expected Create mode"),
    }
}
```

> If `test_app()`/`test_user_profile()` helpers don't exist, reuse the patterns already in `src/ui/app.rs` tests (search the test module for how `App`, `ReadFlow`, `Structure`, and `EntryProfile` are constructed) and add minimal local helpers. Do **not** invent fields — mirror an existing test's construction.

- [ ] **Step 2: Run, confirm fail** — `cargo test -p edaptor new_entry_installs_create_mode_form` → FAIL (still opens overlay).

- [ ] **Step 3: Implement** — replace the body of the `UiAction::NewEntry(i)` arm (app.rs ~914-943) with:

```rust
UiAction::NewEntry(i) => {
    if let Some(profile) = profiles.get(i) {
        let container = if profile.search_base.is_empty() {
            structure.root_dn().to_string()
        } else {
            profile.search_base.clone()
        };
        let model = empty_form_for_profile(read_flow.schema(), profile);
        // Pure-refactor parity: today's NewEntry builds with NO relations (`&[]`),
        // so create has no pickers. Keep that exactly — enabling member pickers on
        // create is a separate, out-of-scope behaviour change, not part of §5.0.
        let mut form = build_edit_form(&model, read_flow.schema(), false, &[]);
        for field in &mut form.fields {
            if field.editable { field.multi = false; }
        }
        form.mode = FormMode::Create { profile_idx: i, container };
        app.form = Some(form);
        app.form_focus = 0;
        app.form_scroll = 0;
        app.overlay = None;
        app.status = format!("New {} — fill fields, F2 to create, Esc to cancel.", profile.name);
    }
}
```

- [ ] **Step 4: Run, confirm pass** — `cargo test -p edaptor new_entry_installs_create_mode_form` → PASS.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(ui): NEW installs a Create-mode form in pane 3 (no modal)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

### Task 0.3: `FormSave` branches on mode → create-save pipeline

**Files:**
- Modify: `src/ui/app.rs` — `UiAction::FormSave` (~868); move `commit_create` logic (~1136-1198) into a `prepare_create` reachable from `FormSave`.
- Test: `src/ui/app.rs` tests

**Context:** `commit_create` already builds the DN, validates, renders LDIF and sets `Overlay::Confirm { action: PendingAction::Create { .. } }`. We call that path from `FormSave` when `form.is_new()`, reading `profile_idx`/`container` from `form.mode` instead of the old overlay.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn formsave_on_create_form_opens_create_confirm() {
    let mut app = test_app();
    let profiles = vec![test_user_profile()];
    let mut read_flow = test_read_flow_with_person_schema();
    let mut structure = test_structure();
    let worker = test_worker();
    handle_ui_action(&mut app, UiAction::NewEntry(0), &worker, &mut read_flow,
                     &mut structure, &profiles, "dc=example,dc=org");
    // type a uid into the RDN field
    set_field_value(app.form.as_mut().unwrap(), "uid", "alice");
    set_field_value(app.form.as_mut().unwrap(), "cn", "Alice");
    set_field_value(app.form.as_mut().unwrap(), "sn", "Adams");
    handle_ui_action(&mut app, UiAction::FormSave, &worker, &mut read_flow,
                     &mut structure, &profiles, "dc=example,dc=org");
    match &app.overlay {
        Some(Overlay::Confirm { action: PendingAction::Create { dn, .. }, .. }) =>
            assert_eq!(dn, "uid=alice,ou=people,dc=example,dc=org"),
        other => panic!("expected Create confirm, got {other:?}"),
    }
}
```

> `set_field_value(form, attr, val)` test helper: find the field by `label`, set its `editor` via `TextState::new().with_value(val.into())`. Add it to the test module.

- [ ] **Step 2: Run, confirm fail** — FAIL (FormSave takes the edit/diff path; with empty baseline it likely yields `NoChanges` or a diff confirm, not a Create confirm).

- [ ] **Step 3: Implement** — at the top of the `UiAction::FormSave` arm, before the membership/diff logic, insert:

```rust
UiAction::FormSave => {
    let Some(form) = app.form.as_ref() else { return; };
    if form.is_new() {
        prepare_create(app, &worker, read_flow, profiles);
        return;
    }
    // ... existing edit-save body unchanged ...
```

Add `prepare_create` (adapted from `commit_create`, now reading the mode and taking `worker` for Phase 3 autonumber — in Phase 0 it does NOT yet apply defaults):

```rust
/// Validate a Create-mode pane-3 form and open the create LDIF confirm.
fn prepare_create(
    app: &mut App,
    _worker: &WorkerHandle,         // used from Phase 3 for autonumber
    read_flow: &mut ReadFlow,
    profiles: &[EntryProfile],
) {
    let Some(form) = app.form.as_ref() else { return; };
    let (profile_idx, container) = match &form.mode {
        FormMode::Create { profile_idx, container } => (*profile_idx, container.clone()),
        FormMode::Edit => return,
    };
    let Some(profile) = profiles.get(profile_idx) else { return; };
    let edited = form.to_edit_entry();

    let rdn_value = edited.attrs.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(&profile.rdn_attr))
        .and_then(|(_, v)| v.first().cloned())
        .unwrap_or_default();
    if rdn_value.trim().is_empty() {
        app.overlay = Some(Overlay::Error { text: "The RDN attribute must have a value.".into() });
        return;
    }

    let (dn, attrs) = build_add_entry(profile, &container, rdn_value.trim(), &edited);
    let oc_refs: Vec<&str> = profile.object_classes.iter().map(String::as_str).collect();
    let full_entry = EditEntry { dn: dn.clone(), attrs: attrs.clone() };
    let errors = validate(&full_entry, read_flow.schema(), &oc_refs);
    if !errors.is_empty() {
        app.overlay = Some(Overlay::Error { text: format_validation_errors(&errors) });
        return;
    }
    let ldif = render_add(&dn, &attrs);
    app.overlay = Some(Overlay::Confirm {
        title: "Create this entry?".into(),
        body: ldif,
        action: PendingAction::Create { dn, attrs, parent: container },
    });
}
```

> Note `oc_refs` is now `Vec<&str>` over `object_classes` — this anticipates Phase 1. In Phase 0, `object_class` is still a `String`; temporarily write `let oc_refs = [profile.object_class.as_str()];` and switch to the `Vec` form in Phase 1 Task 1.3. Keep whichever compiles now.

- [ ] **Step 4: Run, confirm pass** — `cargo test -p edaptor formsave_on_create_form_opens_create_confirm` → PASS.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(ui): FormSave routes Create-mode forms through prepare_create

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

### Task 0.4: Cancel + clobber guard + navigation interaction

**Files:**
- Modify: `src/ui/app.rs` — base-read install guard (~506); `revert_form` (~997) / `FormCancel`; the navigation/guard path.
- Test: `src/ui/app.rs` tests

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn late_base_read_does_not_clobber_create_form() {
    let mut app = test_app();
    let profiles = vec![test_user_profile()];
    let (mut read_flow, mut structure, worker) = (test_read_flow_with_person_schema(), test_structure(), test_worker());
    app.last_seen_leaf = Some("uid=bob,ou=people,dc=example,dc=org".into()); // prior selection
    handle_ui_action(&mut app, UiAction::NewEntry(0), &worker, &mut read_flow,
                     &mut structure, &profiles, "dc=example,dc=org");
    // a base-read for the prior selection arrives
    let resp = Response::form_for("uid=bob,ou=people,dc=example,dc=org"); // test helper building a ReadOutcome::Form-producing response
    handle_worker_response(&mut app, resp, &worker, &mut read_flow, &mut HashMap::new(), &mut HashMap::new());
    assert!(app.form.as_ref().unwrap().is_new(), "create form must survive the late base-read");
}

#[test]
fn cancel_on_create_form_clears_it() {
    let mut app = test_app();
    let profiles = vec![test_user_profile()];
    let (mut read_flow, mut structure, worker) = (test_read_flow_with_person_schema(), test_structure(), test_worker());
    handle_ui_action(&mut app, UiAction::NewEntry(0), &worker, &mut read_flow,
                     &mut structure, &profiles, "dc=example,dc=org");
    handle_ui_action(&mut app, UiAction::FormCancel, &worker, &mut read_flow,
                     &mut structure, &profiles, "dc=example,dc=org");
    assert!(app.form.is_none());
}
```

> If building a `Response` that yields `ReadOutcome::Form` is awkward in a unit test, assert the guard at the predicate level instead: extract the install condition into `fn should_install_form(app, title) -> bool { app.last_seen_leaf.as_deref().map_or(false, |d| d.eq_ignore_ascii_case(title)) && app.overlay.is_none() && !app.form.as_ref().map_or(false, |f| f.is_new()) }` and unit-test that function directly.

- [ ] **Step 2: Run, confirm fail.**

- [ ] **Step 3: Implement.**

(a) Clobber guard — at app.rs ~506 change:

```rust
if current && app.overlay.is_none() && !app.form.as_ref().map_or(false, |f| f.is_new()) {
```

(b) `FormCancel` — `revert_form` must discard when the form is new. In `revert_form` (~997) add at the top:

```rust
if app.form.as_ref().map_or(false, |f| f.is_new()) {
    app.form = None;
    app.form_focus = 0;
    app.form_scroll = 0;
    app.status.clear();
    return;
}
```

(c) Navigation interaction — in the tree-navigation guard (where moving while `app.form` is dirty raises a `GuardIntent`), add: when `app.form.is_new()` and `!is_dirty()`, clear the create form before processing the move (an untouched new entry is discarded silently); when `is_new()` and dirty, the existing dirty-guard prompt applies unchanged. Locate the guard predicate (search for `is_dirty(` near the navigation/`GuardIntent` construction) and adjust.

- [ ] **Step 4: Run, confirm pass.**

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(ui): protect & cancel Create-mode form (clobber guard, discard on cancel)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

### Task 0.5: Delete the modal create path; render Create-mode in pane 3

**Files:**
- Modify: `src/ui/app.rs` — remove `Overlay::CreateForm` variant (~78), `create_form_key` (~1096), `commit_create` (now superseded by `prepare_create`), and the `Overlay::CreateForm` dispatch arm (~1054).
- Modify: `src/ui/view.rs` — remove `render_create_form` (~485) and its `Overlay::CreateForm` render arm (~329); ensure `render_form` (~188) titles a Create-mode form as `New <profile>`.
- Test: existing tests must stay green; add a render smoke test.

- [ ] **Step 1: Write/adjust the test** — `render_form` title for a create form:

```rust
#[test]
fn render_form_titles_create_mode_as_new() {
    // build a Create-mode EditForm with one field; render into a TestBackend buffer
    // (mirror the existing render_form test at view.rs ~690) and assert the buffer
    // contains "New" in the pane-3 title row.
}
```

For the pane-3 title, `render_form` currently derives the title from `app.form` (view.rs ~190). Make it show `New <name>` when `form.is_new()`. Since the profile name isn't on the form, store the display title on the form or pass profiles in. Simplest: set `app.status`/title from the `New <name>` already in Task 0.2, and in `render_form` use a constant prefix: when `form.is_new()`, title = format!("New entry: {}", form.dn_or_placeholder()). Add `EditForm::display_title()` returning `"New entry"` when new (DN not yet composed) else `self.dn.clone()`.

- [ ] **Step 2: Run, confirm fail/compile-error** after deleting the modal arms.

- [ ] **Step 3: Implement** — delete the four modal sites and the render arm; wire `render_form` to `display_title()`. `build_edit_form` sets `dn: String::new()` for create (DN composed at save), so `display_title()` returns `"New entry"` when `is_new()`.

- [ ] **Step 4: Run** — `cargo test -p edaptor` (full suite) → all green; `cargo build --all-targets` clean; `cargo clippy --all-targets -- -D warnings` clean (no dead code from the removed modal).

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "refactor(ui): delete modal create path; render NEW in pane 3

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

### Task 0.6: tmux smoke (manual checkpoint)

- [ ] Run the app against the test LDAP (`scripts/test-ldap.sh start`, export the two env vars, `cargo run -- --config <test.toml>`); F7 → fill uid/cn/sn → F2 → Confirm → entry appears and is selected; F7 → Esc cancels; F7 → type → navigate away → dirty-guard prompts. Record result in the task checkbox. **Phase 0 done — app behaves as before, create now lives in pane 3.**

---

# Phase 1 — `object_classes` list

**Goal:** `EntryProfile.object_class: String` → `object_classes: Vec<String>`; create uses all classes (posixAccount/shadowAccount MUST/MAY now appear and are validated); the picker filter ANDs all classes. Breaking config change (D1).

### Task 1.1: `build_member_filter` takes a slice and ANDs classes

**Files:**
- Modify: `src/ui/picker.rs` — `build_member_filter` (~24)
- Test: `src/ui/picker.rs` tests

- [ ] **Step 1: Failing test**

```rust
#[test]
fn member_filter_ands_multiple_object_classes() {
    let f = build_member_filter(&["posixAccount".into(), "inetOrgPerson".into()],
                                &["cn".into(), "uid".into()], "ali");
    // (&(objectClass=posixAccount)(objectClass=inetOrgPerson)(|(cn=*ali*)(uid=*ali*)))
    assert!(f.starts_with("(&(objectClass=posixAccount)(objectClass=inetOrgPerson)"));
    assert!(f.contains("(cn=*ali*)"));
    assert!(f.contains("(uid=*ali*)"));
}

#[test]
fn member_filter_single_class_unchanged_shape() {
    let f = build_member_filter(&["inetOrgPerson".into()], &["cn".into()], "bob");
    assert_eq!(f, "(&(objectClass=inetOrgPerson)(|(cn=*bob*)))");
}
```

- [ ] **Step 2: Run, confirm fail** (signature is `&str`, not `&[String]`).

- [ ] **Step 3: Implement** — change the signature to `build_member_filter(object_classes: &[String], search_attrs: &[String], term: &str) -> String`; emit one `(objectClass=<oc>)` per class (each escaped via the existing `escape_filter`) inside the outer `(&…)`. Keep the search-term `(|…)` group exactly as today.

- [ ] **Step 4: Run, confirm pass.**

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(picker): build_member_filter ANDs multiple objectClasses

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

### Task 1.2: `CandidateScope` holds `object_classes`

**Files:**
- Modify: `src/config/relation.rs` — `CandidateScope` (~37), `resolve_relations` (~59), holder/backref scope construction (~79-81)
- Modify: callers `src/ui/app.rs:844` (`build_member_filter(&scope.object_class, …)`), `src/ui/edit_form.rs` test scopes
- Test: `src/config/relation.rs` tests

- [ ] **Step 1: Failing test** — update the relation tests (~176-198) to assert `candidate_scope.object_classes == vec!["inetOrgPerson"]` etc.

- [ ] **Step 2: Run, confirm fail.**

- [ ] **Step 3: Implement** — rename the field to `pub object_classes: Vec<String>`; in `resolve_relations` set it from the resolved profile's `object_classes` (Task 1.3 makes that a `Vec`; until then wrap the single string: `vec![p.object_class.clone()]`). Update `app.rs:844` to `build_member_filter(&scope.object_classes, &scope.search_attrs, &query)`.

- [ ] **Step 4: Run, confirm pass.**

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "refactor(config): CandidateScope.object_classes is a list

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

### Task 1.3: `EntryProfile.object_classes` + create uses all classes

**Files:**
- Modify: `src/config/mod.rs` — `EntryProfile` (~57); tests (~222-249, ~348-374)
- Modify: `src/workflows/create.rs` — `build_add_entry` (~41-44), `empty_form_for_profile` (~70)
- Modify: `src/ui/app.rs` — `prepare_create` `oc_refs` (Task 0.3 note); `app.rs:1174` already inside `prepare_create` now
- Modify: `src/config/relation.rs` — `resolve_relations` now reads `p.object_classes`
- Test: `src/workflows/create.rs` tests (~157-168, ~213-233), `src/config/mod.rs` tests

- [ ] **Step 1: Failing tests**

```rust
// config/mod.rs
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
    assert_eq!(cfg.profiles[0].object_classes,
               vec!["inetOrgPerson", "posixAccount", "shadowAccount"]);
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
```

```rust
// workflows/create.rs — multi-OC objectClass set
#[test]
fn build_add_includes_all_object_classes_top_first_deduped() {
    let p = EntryProfile { object_classes:
        vec!["inetOrgPerson".into(), "posixAccount".into(), "top".into()], ..profile() };
    let (_, attrs) = build_add_entry(&p, "ou=people,dc=example,dc=org", "alice", &edited());
    let oc = attrs.get("objectClass").unwrap();
    assert_eq!(oc[0], "top");
    assert!(oc.contains(&"inetOrgPerson".to_string()));
    assert!(oc.contains(&"posixAccount".to_string()));
    assert_eq!(oc.iter().filter(|v| v.eq_ignore_ascii_case("top")).count(), 1);
}
```

Update the `profile()` test helper in `create.rs` (~123) to use `object_classes: vec!["inetOrgPerson".into()]`.

- [ ] **Step 2: Run, confirm fail.**

- [ ] **Step 3: Implement**

In `EntryProfile`: replace `pub object_class: String` with `pub object_classes: Vec<String>`. (No serde alias — D1.)

In `build_add_entry` (create.rs ~41) replace the fixed objectClass insert:

```rust
// Canonical objectClass set: "top" first, then the profile's classes, deduped
// (case-insensitive), preserving declared order.
let mut oc: Vec<String> = vec!["top".to_string()];
for c in &profile.object_classes {
    if !oc.iter().any(|x| x.eq_ignore_ascii_case(c)) {
        oc.push(c.clone());
    }
}
attrs.insert("objectClass".to_string(), oc);
```

In `empty_form_for_profile` (create.rs ~70):

```rust
let oc_refs: Vec<&str> = profile.object_classes.iter().map(String::as_str).collect();
let resolved = schema.effective_attributes(&oc_refs);
```

In `prepare_create` use `let oc_refs: Vec<&str> = profile.object_classes.iter().map(String::as_str).collect();` then `validate(&full_entry, read_flow.schema(), &oc_refs)`.

In `resolve_relations` (relation.rs) set `object_classes: p.object_classes.clone()`.

Fix every other construction of `EntryProfile { .. object_class: .. }` across the crate (config tests, app.rs tests ~2412+, edit_form.rs tests ~439+, relation.rs tests) to `object_classes: vec![..]`. `cargo build --all-targets` enumerates them.

- [ ] **Step 4: Run** — `cargo test -p edaptor` green.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(config): EntryProfile.object_classes list; create uses all classes

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

### Task 1.4: Update README + sample/fixture configs

**Files:** Modify `README.md` `## Configuration` example and any `*.toml` sample/fixtures to `object_classes = [...]`.

- [ ] Update each, `cargo test -p edaptor` (fixtures parse), commit:

```bash
git add -A && git commit -m "docs: object_classes list in README + sample configs

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

# Phase 2 — `config::defaults` pure engine

**Goal:** a self-contained, fully unit-tested module that parses `[profile.defaults]` values and plans which empty fields to fill. No worker, no UI.

### Task 2.1: `parse_default_value`

**Files:** Create `src/config/defaults.rs`; register `pub mod defaults;` in `src/config/mod.rs`.

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn parses_literal() {
    assert!(matches!(parse_default_value("/bin/bash"), Ok(DefaultValue::Literal(s)) if s == "/bin/bash"));
}
#[test]
fn parses_template_with_embedded_text() {
    match parse_default_value("/home/{uid}").unwrap() {
        DefaultValue::Template(segs) => {
            assert!(matches!(&segs[0], Seg::Lit(s) if s == "/home/"));
            assert!(matches!(&segs[1], Seg::Field(s) if s == "uid"));
        }
        _ => panic!("expected template"),
    }
}
#[test]
fn parses_multi_placeholder_template() {
    match parse_default_value("{givenName}.{sn}").unwrap() {
        DefaultValue::Template(segs) => assert_eq!(segs.len(), 3), // Field, Lit("."), Field
        _ => panic!(),
    }
}
#[test]
fn parses_autonumber() {
    assert!(matches!(parse_default_value("{next:10000-60000}"),
                     Ok(DefaultValue::AutoNumber { min: 10000, max: 60000 })));
}
#[test]
fn autonumber_min_gt_max_is_error() {
    assert!(parse_default_value("{next:60000-10000}").is_err());
}
#[test]
fn malformed_autonumber_is_error() {
    assert!(parse_default_value("{next:abc}").is_err());
    assert!(parse_default_value("{next:10000}").is_err());
}
#[test]
fn unterminated_placeholder_is_error() {
    assert!(parse_default_value("/home/{uid").is_err());
}
```

- [ ] **Step 2: Run, confirm fail** (module/functions absent).

- [ ] **Step 3: Implement** `src/config/defaults.rs`:

```rust
//! Pure parsing + planning for `[profile.defaults]`: literal / `{attr}` template /
//! `{next:MIN-MAX}` autonumber. No worker, no UI.

use std::collections::BTreeMap;
use serde::{Deserialize, Deserializer};

/// One segment of a template value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seg { Lit(String), Field(String) }

/// A parsed default value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefaultValue {
    Literal(String),
    Template(Vec<Seg>),
    AutoNumber { min: u64, max: u64 },
}

/// A profile's `[profile.defaults]` table (attr → parsed value), order-stable.
#[derive(Debug, Clone, Default)]
pub struct ProfileDefaults { pub entries: BTreeMap<String, DefaultValue> }

/// A planned action for one defaulted attribute (see `plan_defaults`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    Fill { attr: String, value: String },
    NeedsAutonumber { attr: String, min: u64, max: u64 },
}

/// Parse one config value string into a `DefaultValue`.
pub fn parse_default_value(s: &str) -> Result<DefaultValue, String> {
    let trimmed = s.trim();
    if let Some(inner) = trimmed.strip_prefix("{next:").and_then(|r| r.strip_suffix('}')) {
        let (lo, hi) = inner.split_once('-')
            .ok_or_else(|| format!("autonumber '{s}' must be {{next:MIN-MAX}}"))?;
        let min: u64 = lo.trim().parse().map_err(|_| format!("autonumber MIN '{lo}' is not a number"))?;
        let max: u64 = hi.trim().parse().map_err(|_| format!("autonumber MAX '{hi}' is not a number"))?;
        if min > max { return Err(format!("autonumber range '{s}' has MIN > MAX")); }
        return Ok(DefaultValue::AutoNumber { min, max });
    }
    if !s.contains('{') {
        return Ok(DefaultValue::Literal(s.to_string()));
    }
    // Template: split into Lit / Field segments. '{' opens a placeholder, '}' closes.
    let mut segs = Vec::new();
    let mut lit = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            if !lit.is_empty() { segs.push(Seg::Lit(std::mem::take(&mut lit))); }
            let mut name = String::new();
            let mut closed = false;
            for c2 in chars.by_ref() {
                if c2 == '}' { closed = true; break; }
                name.push(c2);
            }
            if !closed { return Err(format!("unterminated placeholder in '{s}'")); }
            if name.is_empty() { return Err(format!("empty placeholder in '{s}'")); }
            segs.push(Seg::Field(name));
        } else {
            lit.push(c);
        }
    }
    if !lit.is_empty() { segs.push(Seg::Lit(lit)); }
    Ok(DefaultValue::Template(segs))
}

impl<'de> Deserialize<'de> for ProfileDefaults {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw: BTreeMap<String, String> = BTreeMap::deserialize(d)?;
        let mut entries = BTreeMap::new();
        for (k, v) in raw {
            let parsed = parse_default_value(&v).map_err(serde::de::Error::custom)?;
            entries.insert(k, parsed);
        }
        Ok(ProfileDefaults { entries })
    }
}
```

- [ ] **Step 4: Run, confirm pass.**

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(config): defaults value parser (literal/template/autonumber)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

### Task 2.2: `next_in_range`

**Files:** `src/config/defaults.rs`

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn next_in_range_empty_returns_min() {
    assert_eq!(next_in_range(&[], 10000, 60000).unwrap(), 10000);
}
#[test]
fn next_in_range_is_max_plus_one() {
    assert_eq!(next_in_range(&[10000, 10005, 10003], 10000, 60000).unwrap(), 10006);
}
#[test]
fn next_in_range_ignores_out_of_window_values() {
    assert_eq!(next_in_range(&[9000, 70000, 10002], 10000, 60000).unwrap(), 10003);
}
#[test]
fn next_in_range_exhausted_errors() {
    assert!(next_in_range(&[60000], 10000, 60000).is_err());
}
```

- [ ] **Step 2: Run, confirm fail.**

- [ ] **Step 3: Implement**

```rust
/// Next free number: max of `existing` within `[min,max]`, plus one; `min` if none
/// in window. Errors if the pool is exhausted.
pub fn next_in_range(existing: &[u64], min: u64, max: u64) -> Result<u64, String> {
    let cur_max = existing.iter().copied().filter(|n| *n >= min && *n <= max).max();
    let next = match cur_max { Some(m) => m + 1, None => min };
    if next > max { return Err(format!("number pool {min}-{max} is exhausted")); }
    Ok(next)
}
```

- [ ] **Step 4-5: Run, pass, commit.**

```bash
git add -A && git commit -m "feat(config): next_in_range autonumber allocator (pure)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

### Task 2.3: `plan_defaults` (empty-only fill, template resolution)

**Files:** `src/config/defaults.rs`

- [ ] **Step 1: Failing tests**

```rust
fn cur(pairs: &[(&str, &str)]) -> BTreeMap<String, Vec<String>> {
    pairs.iter().map(|(k, v)| (k.to_string(),
        if v.is_empty() { vec![] } else { vec![v.to_string()] })).collect()
}
#[test]
fn fills_only_empty_fields() {
    let mut d = ProfileDefaults::default();
    d.entries.insert("loginShell".into(), DefaultValue::Literal("/bin/bash".into()));
    // operator already typed a shell → no fill
    let r = plan_defaults(&d, &cur(&[("loginShell", "/bin/zsh")]));
    assert!(r.is_empty());
    // empty → fill
    let r = plan_defaults(&d, &cur(&[("loginShell", "")]));
    assert_eq!(r, vec![Resolution::Fill { attr: "loginShell".into(), value: "/bin/bash".into() }]);
}
#[test]
fn resolves_template_against_current_values() {
    let mut d = ProfileDefaults::default();
    d.entries.insert("homeDirectory".into(),
        parse_default_value("/home/{uid}").unwrap());
    let r = plan_defaults(&d, &cur(&[("uid", "alice"), ("homeDirectory", "")]));
    assert_eq!(r, vec![Resolution::Fill { attr: "homeDirectory".into(), value: "/home/alice".into() }]);
}
#[test]
fn template_with_empty_source_yields_no_fill() {
    let mut d = ProfileDefaults::default();
    d.entries.insert("homeDirectory".into(), parse_default_value("/home/{uid}").unwrap());
    let r = plan_defaults(&d, &cur(&[("uid", ""), ("homeDirectory", "")]));
    assert!(r.is_empty(), "unresolved template must not fill");
}
#[test]
fn autonumber_surfaces_as_needs_autonumber() {
    let mut d = ProfileDefaults::default();
    d.entries.insert("uidNumber".into(), parse_default_value("{next:10000-60000}").unwrap());
    let r = plan_defaults(&d, &cur(&[("uidNumber", "")]));
    assert_eq!(r, vec![Resolution::NeedsAutonumber { attr: "uidNumber".into(), min: 10000, max: 60000 }]);
}
```

- [ ] **Step 2: Run, confirm fail.**

- [ ] **Step 3: Implement**

```rust
/// Helper: is the attr currently empty (no non-blank value)?
fn is_empty(current: &BTreeMap<String, Vec<String>>, attr: &str) -> bool {
    current.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(attr))
        .map(|(_, v)| v.iter().all(|s| s.trim().is_empty()))
        .unwrap_or(true)
}

/// Resolve a template against current field values; `None` if any `{field}` is empty.
fn resolve_template(segs: &[Seg], current: &BTreeMap<String, Vec<String>>) -> Option<String> {
    let mut out = String::new();
    for seg in segs {
        match seg {
            Seg::Lit(s) => out.push_str(s),
            Seg::Field(name) => {
                let v = current.iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(name))
                    .and_then(|(_, v)| v.first())
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())?;
                out.push_str(v);
            }
        }
    }
    Some(out)
}

/// Plan which EMPTY fields to fill. Operator-entered values are never overwritten.
pub fn plan_defaults(d: &ProfileDefaults,
                     current: &BTreeMap<String, Vec<String>>) -> Vec<Resolution> {
    let mut out = Vec::new();
    for (attr, dv) in &d.entries {
        if !is_empty(current, attr) { continue; }
        match dv {
            DefaultValue::Literal(s) =>
                out.push(Resolution::Fill { attr: attr.clone(), value: s.clone() }),
            DefaultValue::Template(segs) => {
                if let Some(v) = resolve_template(segs, current) {
                    out.push(Resolution::Fill { attr: attr.clone(), value: v });
                }
            }
            DefaultValue::AutoNumber { min, max } =>
                out.push(Resolution::NeedsAutonumber { attr: attr.clone(), min: *min, max: *max }),
        }
    }
    out
}
```

- [ ] **Step 4-5: Run, pass, commit.**

```bash
git add -A && git commit -m "feat(config): plan_defaults (empty-only fill, template resolution)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

### Task 2.4: wire `defaults` into `EntryProfile` parsing

**Files:** `src/config/mod.rs`

- [ ] **Step 1: Failing test**

```rust
#[test]
fn parses_profile_defaults_block() {
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
    assert!(matches!(d.entries.get("loginShell"), Some(DefaultValue::Literal(_))));
    assert!(matches!(d.entries.get("uidNumber"), Some(DefaultValue::AutoNumber { .. })));
}
```

- [ ] **Step 2: Run, confirm fail.**

- [ ] **Step 3: Implement** — add to `EntryProfile`:

```rust
use crate::config::defaults::ProfileDefaults;
// ...
    #[serde(default)]
    pub defaults: ProfileDefaults,
```

Add `Default` impl note: `EntryProfile` derives `Default`; `ProfileDefaults` derives `Default` — fine. Update every `EntryProfile { .. }` test literal to include `defaults: Default::default()` (or use `..Default::default()` where possible).

- [ ] **Step 4-5: Run, pass, commit.**

```bash
git add -A && git commit -m "feat(config): parse [profile.defaults] onto EntryProfile

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

# Phase 3 — Truncation-safe autonumber, wired into create-save

**Goal:** surface search truncation from the worker (D6), add a synchronous `allocate_number`, and apply defaults+autonumber inside `prepare_create` before building the Add.

### Task 3.1: `Response::Entries` carries `truncated`

**Files:**
- Modify: `src/ldap/worker.rs` — `run_search` (~554), `Response::Entries` (~156), the Search dispatch (~366), worker test (~702)
- Modify: consumers using exact `{ id, entries }`: `src/ui/app.rs:399`
- Test: `src/ldap/worker.rs` tests

- [ ] **Step 1: Failing test**

```rust
#[test]
fn run_search_reports_truncation_flag() {
    // Unit-test the flag mapping directly: is_limit_rc drives `truncated`.
    assert!(truncated_from_rc(4));   // sizeLimitExceeded
    assert!(!truncated_from_rc(0));  // clean
}
```

(Extract `fn truncated_from_rc(rc: u32) -> bool { is_limit_rc(rc) }` so the mapping is unit-testable without a live conn.)

- [ ] **Step 2: Run, confirm fail.**

- [ ] **Step 3: Implement**

`run_search` → `Result<(Vec<LdapEntry>, bool)>`; compute `let truncated = is_limit_rc(res.rc);` and return `Ok((entries, truncated))`.

`Response::Entries { id, entries }` → add `truncated: bool`.

Search dispatch (~366): `Ok((entries, truncated)) => Response::Entries { id, entries, truncated }`.

Update the worker test constructor (~702) and `app.rs:399` (`Response::Entries { id, entries }` → `{ id, entries, .. }`). Other consumers already use `..`.

- [ ] **Step 4: Run** — `cargo test -p edaptor`, `cargo build --all-targets` green.

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(ldap): Response::Entries exposes truncation flag

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

### Task 3.2: `allocate_number` (synchronous, refuses on truncation)

**Files:** `src/ui/app.rs` (near `read_group_members` ~1847)

- [ ] **Step 1: Failing test** — test the pure decision, not the live scan. Add a helper:

```rust
/// Decide an allocation from a (possibly truncated) scan result.
fn decide_allocation(values: &[u64], truncated: bool, min: u64, max: u64) -> Result<u64, String> {
    if truncated {
        return Err("uidNumber scan was truncated by a server limit; refusing to allocate".into());
    }
    crate::config::defaults::next_in_range(values, min, max)
}

#[test]
fn allocation_refuses_on_truncation() {
    assert!(decide_allocation(&[10000], true, 10000, 60000).is_err());
    assert_eq!(decide_allocation(&[10000], false, 10000, 60000).unwrap(), 10001);
}
```

- [ ] **Step 2: Run, confirm fail.**

- [ ] **Step 3: Implement** `allocate_number` using the synchronous `worker.request` seam (mirror `read_group_members`):

```rust
/// Allocate the next free numeric `attr` in `[min,max]` by scanning the whole
/// directory subtree from `base_dn`. Refuses (errors) if the scan was truncated
/// by a server limit — never allocates over a partial set (spec D6).
fn allocate_number(worker: &WorkerHandle, base_dn: &str, attr: &str,
                   min: u64, max: u64) -> Result<u64, String> {
    let resp = worker.request(Request::Search {
        id: next_id(),
        base: base_dn.to_string(),
        scope: SearchScope::Subtree,
        filter: format!("({attr}=*)"),
        attrs: vec![attr.to_string()],
        size_limit: None,
    }).map_err(|e| e.to_string())?;
    let (entries, truncated) = match resp {
        Response::Entries { entries, truncated, .. } => (entries, truncated),
        Response::SearchError { msg, .. } => return Err(msg),
        _ => return Err("unexpected response while allocating".into()),
    };
    let mut values: Vec<u64> = Vec::new();
    for e in &entries {
        if let Some((_, vs)) = e.attrs.iter().find(|(k, _)| k.eq_ignore_ascii_case(attr)) {
            for v in vs { if let Ok(n) = v.trim().parse::<u64>() { values.push(n); } }
        }
    }
    decide_allocation(&values, truncated, min, max)
}
```

> Confirm `SearchScope::Subtree` is the correct variant name (worker.rs ~36). Confirm `LdapEntry.attrs` shape (`Vec<(String, Vec<String>)>`) — mirror `read_group_members`'s access pattern exactly.
>
> **Server-sizelimit ceiling (known limitation):** `size_limit: None` removes only the *client* cap; slapd still enforces its own `sizelimit` (default ~500) unless the bind identity is rootdn / high-limit. In a directory with more `uidNumber`-bearing entries than that limit, the `(uidNumber=*)` subtree scan returns `truncated=true` and allocation refuses **every time**. Failing closed is correct (never duplicate), but it means auto-allocation effectively requires an admin/high-limit bind on large directories. Real fixes (paged scan, or a dedicated counter entry with compare-and-set) are a follow-up — do **not** silently widen the scan. Surface this in the error text: "…refusing to allocate (scan hit the server size limit — bind with a higher-limit identity or configure a counter)."

- [ ] **Step 4-5: Run, pass, commit.**

```bash
git add -A && git commit -m "feat(ui): allocate_number scan with truncation refusal

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

### Task 3.3: apply defaults + autonumber in `prepare_create`

**Files:** `src/ui/app.rs` — `prepare_create` (Task 0.3)

- [ ] **Step 1: Failing test**

```rust
#[test]
fn prepare_create_fills_literal_and_template_defaults() {
    let mut app = test_app();
    let mut profile = test_user_profile();   // object_classes=[inetOrgPerson,posixAccount]
    profile.defaults.entries.insert("loginShell".into(),
        crate::config::defaults::DefaultValue::Literal("/bin/bash".into()));
    profile.defaults.entries.insert("homeDirectory".into(),
        crate::config::defaults::parse_default_value("/home/{uid}").unwrap());
    let profiles = vec![profile];
    let (mut read_flow, mut structure, worker) =
        (test_read_flow_with_posix_schema(), test_structure(), test_worker());
    handle_ui_action(&mut app, UiAction::NewEntry(0), &worker, &mut read_flow, &mut structure, &profiles, "dc=example,dc=org");
    set_field_value(app.form.as_mut().unwrap(), "uid", "alice");
    set_field_value(app.form.as_mut().unwrap(), "cn", "Alice");
    set_field_value(app.form.as_mut().unwrap(), "sn", "Adams");
    handle_ui_action(&mut app, UiAction::FormSave, &worker, &mut read_flow, &mut structure, &profiles, "dc=example,dc=org");
    if let Some(Overlay::Confirm { action: PendingAction::Create { attrs, .. }, .. }) = &app.overlay {
        assert_eq!(attrs.get("loginShell"), Some(&vec!["/bin/bash".to_string()]));
        assert_eq!(attrs.get("homeDirectory"), Some(&vec!["/home/alice".to_string()]));
    } else { panic!("expected create confirm"); }
}
```

> Autonumber needs a worker that answers a `Search` with seeded `uidNumber`s; if the test worker can't, cover autonumber in the gated live test (Task 3.4) and keep this unit test to literal+template only.

- [ ] **Step 2: Run, confirm fail.**

- [ ] **Step 3: Implement** — in `prepare_create`, between computing `edited` and `build_add_entry`, insert the defaults pass:

```rust
use crate::config::defaults::{plan_defaults, Resolution};

let mut edited = edited; // make mutable
let plan = plan_defaults(&profile.defaults, &edited.attrs);
for res in plan {
    match res {
        Resolution::Fill { attr, value } => {
            // plan_defaults already guarantees the field is empty; just set it.
            edited.attrs.insert(attr, vec![value]);
        }
        Resolution::NeedsAutonumber { attr, min, max } => {
            match allocate_number(_worker, base_dn_of(app), &attr, min, max) {
                Ok(n) => { edited.attrs.insert(attr, vec![n.to_string()]); }
                Err(e) => { app.overlay = Some(Overlay::Error { text: e }); return; }
            }
        }
    }
}
```

> Case is consistent because `plan_defaults` echoes the config attr name and `build_add_entry`/`validate` compare case-insensitively. Resolve `base_dn`: `prepare_create` must receive `base_dn: &str` — thread it through from `handle_ui_action` (which already has it) and update the `FormSave` call site. The autonumber scan uses `base_dn` (whole-directory uniqueness), not `container`.

- [ ] **Step 4-5: Run, pass, commit.**

```bash
git add -A && git commit -m "feat(ui): apply defaults + autonumber in create-save

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

### Task 3.4: gated live test — autonumber + truncation + multi-OC create

**Files:** Create `tests/live_templates.rs` (mirror `tests/live_write.rs` gating on `EDAPTOR_TEST_LDAP_URI`).

- [ ] Write gated tests: (a) seed two posixAccounts with uidNumbers, allocate → expect max+1; seed a gap → expect gap not reused (max+1); (b) a posixAccount/shadowAccount user created with defaults+autonumber supplying MUST passes server `Add`; (c) `decide_allocation`-level truncation already unit-tested — add a live assertion only if a tight server sizelimit can be forced, else note it as covered by the unit test. Commit:

```bash
git add -A && git commit -m "test(live): autonumber allocation + multi-OC create (gated)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

# Phase 4 — Inline password field (create + edit)

**Goal:** when `[profile.password]` is declared, the `ldap_attribute` field becomes a masked, confirm-twice "set password" field; the schema-generated `userPassword` field is suppressed (D8); on save the cleartext goes to the directory but the LDIF preview shows `********` (D7); `samba=true` + `sambaSamAccount` also sets `sambaNTPassword`.

### Task 4.1: `PasswordSpec` config

**Files:** `src/config/mod.rs`

- [ ] **Step 1: Failing test**

```rust
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
```

- [ ] **Step 2-3: Implement**

```rust
#[derive(Debug, Deserialize, Clone)]
pub struct PasswordSpec {
    #[serde(default = "default_pw_attr")]
    pub ldap_attribute: String,
    #[serde(default)]
    pub samba: bool,
}
fn default_pw_attr() -> String { "userPassword".to_string() }
```

Add to `EntryProfile`: `#[serde(default)] pub password: Option<PasswordSpec>,`.

- [ ] **Step 4-5: Run, pass, commit.**

```bash
git add -A && git commit -m "feat(config): [profile.password] (ldap_attribute + samba)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

### Task 4.2: password Add-attrs helper (reuse M5)

**Files:** `src/samba/password.rs`

- [ ] **Step 1: Failing test**

```rust
#[test]
fn password_add_attrs_userpassword_only_when_not_samba() {
    let a = password_add_attrs("hunter2", "userPassword", false, 1_700_000_000);
    assert_eq!(a, vec![("userPassword".to_string(), vec!["hunter2".to_string()])]);
}
#[test]
fn password_add_attrs_includes_nt_hash_when_samba() {
    let a = password_add_attrs("hunter2", "userPassword", true, 1_700_000_000);
    let nt = a.iter().find(|(k, _)| k == "sambaNTPassword").unwrap();
    assert_eq!(nt.1[0], crate::samba::nthash::nt_hash("hunter2"));
    assert!(a.iter().any(|(k, _)| k == "sambaPwdLastSet"));
}
```

- [ ] **Step 2-3: Implement** (mirror `build_password_mods` but emit `(attr, values)` for an Add):

```rust
/// Attribute (name, values) pairs to inject into an `Add` for a new entry's
/// password. `userPassword` is sent cleartext (slapd hashes over TLS); when
/// `samba`, also `sambaNTPassword` (NT hash) and `sambaPwdLastSet` (epoch secs).
pub fn password_add_attrs(password: &str, ldap_attribute: &str, samba: bool,
                          now_secs: u64) -> Vec<(String, Vec<String>)> {
    let mut out = vec![(ldap_attribute.to_string(), vec![password.to_string()])];
    if samba {
        out.push(("sambaNTPassword".to_string(), vec![crate::samba::nthash::nt_hash(password)]));
        out.push(("sambaPwdLastSet".to_string(), vec![now_secs.to_string()]));
    }
    out
}
```

- [ ] **Step 4-5: Run, pass, commit.**

```bash
git add -A && git commit -m "feat(samba): password_add_attrs for create-time password

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

### Task 4.3: password field in the form (suppress schema userPassword, masked confirm)

**Files:**
- Modify: `src/workflows/create.rs` `empty_form_for_profile` + `src/ui/edit_form.rs` `build_edit_form` (mark a password field), or tag in `app.rs` post-build.
- Modify: `src/ui/edit_form.rs` — `EditField` gains `pub password: bool` and a confirm buffer, or a dedicated `EditField.confirm: Option<TextState>`.
- Modify: `src/ui/view.rs` `render_form` — render two masked rows (password + confirm) for a password field.
- Test: form-construction test.

**Design:** A password field is a normal single-value `EditField` with `secret = true` plus a new `password: bool` flag and a `confirm: TextState<'static>`. When `[profile.password]` is declared on the profile, after building the form: (1) remove any field whose label == `ldap_attribute`; (2) append a synthetic field labelled `ldap_attribute` with `password=true, secret=true, editable=true, must=false`.

- [ ] **Step 1: Failing test**

```rust
#[test]
fn password_block_injects_one_masked_field_no_duplicate() {
    let mut profile = test_user_profile(); // inetOrgPerson → userPassword is a MAY
    profile.password = Some(PasswordSpec { ldap_attribute: "userPassword".into(), samba: false });
    let form = build_create_form(&schema_with_userpassword(), &profile); // helper wrapping empty_form_for_profile+build_edit_form+password injection
    let pw: Vec<_> = form.fields.iter().filter(|f| f.label.eq_ignore_ascii_case("userPassword")).collect();
    assert_eq!(pw.len(), 1, "exactly one userPassword field");
    assert!(pw[0].password && pw[0].secret);
}
```

- [ ] **Step 2-3: Implement** — add `password: bool` and `confirm: TextState<'static>` to `EditField` (default `false`/empty in every constructor — `build_edit_form`'s map and the test literals). Add a `fn inject_password_field(form: &mut EditForm, spec: &PasswordSpec)` in `edit_form.rs`; call it from the create-form builder (and from the edit path in Task 4.5). Render: in `render_form`, when `field.password`, draw the value row masked and a second "Confirm:" row masked from `field.confirm`.

> Keep it minimal: reuse the existing secret masking used for `secret` fields; the confirm row is the only new render element.

- [ ] **Step 4-5: Run, pass, commit.**

```bash
git add -A && git commit -m "feat(ui): inline masked password field (suppress schema userPassword)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

### Task 4.4: password staging on create-save (mask in preview, cleartext in Add)

**Files:** `src/ui/app.rs` `prepare_create`; `src/ldap/ldif.rs` render (mask) — or mask before calling `render_add`.

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn create_confirm_masks_password_but_add_carries_cleartext() {
    let mut app = test_app();
    let mut profile = test_user_profile();
    profile.password = Some(PasswordSpec { ldap_attribute: "userPassword".into(), samba: false });
    let profiles = vec![profile];
    let (mut rf, mut st, w) = (test_read_flow_with_person_schema(), test_structure(), test_worker());
    handle_ui_action(&mut app, UiAction::NewEntry(0), &w, &mut rf, &mut st, &profiles, "dc=example,dc=org");
    set_field_value(app.form.as_mut().unwrap(), "uid", "alice");
    set_field_value(app.form.as_mut().unwrap(), "cn", "Alice");
    set_field_value(app.form.as_mut().unwrap(), "sn", "Adams");
    set_password(app.form.as_mut().unwrap(), "userPassword", "hunter2", "hunter2"); // value + confirm
    handle_ui_action(&mut app, UiAction::FormSave, &w, &mut rf, &mut st, &profiles, "dc=example,dc=org");
    if let Some(Overlay::Confirm { body, action: PendingAction::Create { attrs, .. }, .. }) = &app.overlay {
        assert!(body.contains("userPassword: ********"));
        assert!(!body.contains("hunter2"));
        assert_eq!(attrs.get("userPassword"), Some(&vec!["hunter2".to_string()]));
    } else { panic!() }
}
#[test]
fn mismatched_confirm_blocks_with_error() {
    // ... set_password(.., "a", "b") then FormSave → Overlay::Error, no Confirm
}
```

- [ ] **Step 2-3: Implement** — in `prepare_create`, after defaults and before `build_add_entry`:
  1. Find the password field. If present and its value non-empty: require `value == confirm` else `Overlay::Error { "Passwords do not match." }` and return. Remove the password attr from `edited.attrs` (so it isn't double-written), and stash the cleartext.
  2. `build_add_entry` as usual → `attrs`.
  3. Determine samba: `spec.samba && attrs["objectClass"] contains "sambaSamAccount" (case-insensitive)`.
  4. `for (k, vs) in password_add_attrs(&clear, &spec.ldap_attribute, samba, now_secs()) { attrs.insert(k, vs); }` where `now_secs()` reads `SystemTime` (mirror M5 — this is the one impure call, acceptable in the app layer).
  5. Build a **masked copy** of `attrs` for the preview: clone, replace `ldap_attribute` and `sambaNTPassword` values with `["********"]`, then `render_add(&dn, &masked)`. The `PendingAction::Create` carries the real `attrs`.

- [ ] **Step 4-5: Run, pass, commit.**

```bash
git add -A && git commit -m "feat(ui): stage password on create (cleartext in Add, masked preview)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

### Task 4.5: password on edit (reuse build_password_mods)

**Files:** `src/ui/app.rs` `FormSave` edit branch; `inject_password_field` also on the edit form (`build_edit_form` when the entry's profile declares password).

**Design:** When editing an entry of a password-profile, the `ldap_attribute` field is the masked-confirm field, shown **empty**. On save: empty → no password mods (don't diff it). Non-empty → validate confirm, then inject `build_password_mods(...)` output into the prepared `ChangeSet` (and mask the password lines in the edit LDIF preview).

- [ ] **Step 1: Failing tests** — (a) empty password field on edit emits no `userPassword`/`sambaNTPassword` mod; (b) set password on edit emits the M5 mods and the preview masks them.

> Edit needs the entry→profile mapping. Determine the profile by matching the loaded entry's objectClasses / container against `profiles` (a small helper `profile_for_entry(profiles, dn, object_classes) -> Option<&EntryProfile>`). Keep it minimal — match on objectClass membership.

- [ ] **Step 2-3: Implement** — exclude the password field from the normal diff (like `backref_labels` are excluded): add it to the set stripped from both `edited` and `baseline`. After `prepare_save` yields a `ChangeSet`, if the password field is non-empty append `build_password_mods(...)` mods; mask those lines in the preview body.

- [ ] **Step 4-5: Run, pass, commit.**

```bash
git add -A && git commit -m "feat(ui): set/change password on edit (reuse build_password_mods)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

### Task 4.6: gated live test — password set

**Files:** `tests/live_templates.rs`

- [ ] Create a user with a password → bind as the new DN with that password succeeds. With `samba=true` on a sambaSamAccount → `sambaNTPassword` equals `nt_hash`. Commit.

---

# Phase 5 — Value-lookup picker (gidNumber from a group)

**Goal:** a field declared in `[profile.lookup.<attr>]` opens a single-select picker over `object_class`/`search_base`; selecting a candidate writes its `value_attr` scalar into the field (no DN).

### Task 5.1: `LookupSpec` config

**Files:** `src/config/mod.rs`

- [ ] **Step 1: Failing test** — parse `[profile.lookup.gidNumber]` into `profile.lookups["gidNumber"]` with `object_class`, `value_attr`, `label`, `search_attrs`.

- [ ] **Step 2-3: Implement**

```rust
#[derive(Debug, Deserialize, Clone)]
pub struct LookupSpec {
    pub object_class: String,
    #[serde(default)]
    pub search_base: String,
    pub value_attr: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub search_attrs: Vec<String>,
}
// EntryProfile:
    #[serde(default, rename = "lookup")]
    pub lookups: std::collections::BTreeMap<String, LookupSpec>,
```

- [ ] **Step 4-5: Run, pass, commit.**

```bash
git add -A && git commit -m "feat(config): [profile.lookup.<attr>] value-lookup spec

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

### Task 5.2: tag lookup fields + single-select picker open

**Files:** `src/ui/edit_form.rs` (`EditField.lookup: Option<LookupSpec>`; a `ValueEditor` single-select mode), `src/ui/app.rs` (open on Enter), `src/ui/picker.rs` (`PickerState` single-select).

**Design:** A lookup field is single-value. Enter opens a `ValueEditor` in picker mode with a `single_select: true` flag (or reuse `PickerState` but commit on Enter rather than toggle). The candidate search uses `build_member_filter(&[spec.object_class.clone()], &spec.search_attrs, term)`; the search must also request `spec.value_attr` and the `label` attrs. On Enter over a candidate, read its `value_attr` and write it as the field's single value; close.

- [ ] **Step 1: Failing tests** — (a) `build_edit_form` tags a field with a matching `[profile.lookup]` (thread `&profile.lookups` into the create/edit form builder); (b) a pure `pick_value(candidate_attrs, value_attr) -> Option<String>` returns the scalar.

- [ ] **Step 2-3: Implement** the pure pieces first (`pick_value`, the single-select commit producing `Vec<String>` of length ≤1), then the app wiring. Reuse `service_picker_search` and the `picker_search_id` intercept; branch the commit on single-select to call `pick_value` against the chosen candidate's attributes.

> This is the heaviest app.rs task. Scope tightly: land the pure `pick_value` + config tagging first (separate commit), then the app wiring in a second commit. If subagent context is tight, resolve the app-wiring commit in-session.

- [ ] **Step 4-5: Run, pass, commit (two commits as above).**

```bash
git add -A && git commit -m "feat(ui): value-lookup picker pulls an attribute from a chosen entry

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

### Task 5.3: gated live test — lookup yields gidNumber

**Files:** `tests/live_templates.rs` — seed a posixGroup; `pick_value` against a fetched candidate returns its `gidNumber`. Commit.

---

# Phase 6 — Context-filtered profile chooser

**Goal:** F7 opens a chooser over profiles whose `search_base` matches the current container (DN-component boundary); 0 → all, 1 → create directly, >1 → overlay.

### Task 6.1: `profiles_for_container` (pure)

**Files:** `src/workflows/create.rs`

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn matches_exact_and_descendant_container() {
    let ps = vec![
        prof("user", "ou=people,dc=example,dc=org"),
        prof("group", "ou=groups,dc=example,dc=org"),
    ];
    assert_eq!(profiles_for_container(&ps, "ou=people,dc=example,dc=org"), vec![0]);
    // cursor on a parent container offers children whose search_base is under it
    assert_eq!(profiles_for_container(&ps, "dc=example,dc=org"), vec![0, 1]);
}
#[test]
fn rejects_non_boundary_prefix() {
    let ps = vec![prof("user", "ou=people,dc=example,dc=org")];
    assert!(profiles_for_container(&ps, "ou=people2,dc=example,dc=org").is_empty());
}
#[test]
fn matching_is_case_insensitive() {
    let ps = vec![prof("user", "OU=People,DC=Example,DC=Org")];
    assert_eq!(profiles_for_container(&ps, "ou=people,dc=example,dc=org"), vec![0]);
}
```

`fn prof(name, base)` builds an `EntryProfile` with that `search_base`.

- [ ] **Step 2-3: Implement**

```rust
/// Indices of profiles whose `search_base` matches `container_dn` at a DN-component
/// boundary: equal, or one is a proper suffix of the other (case-insensitive).
pub fn profiles_for_container(profiles: &[EntryProfile], container_dn: &str) -> Vec<usize> {
    profiles.iter().enumerate()
        .filter(|(_, p)| !p.search_base.is_empty()
            && dn_boundary_match(&p.search_base, container_dn))
        .map(|(i, _)| i)
        .collect()
}

/// True when `a` == `b` or one ends with `,<other>` (case-insensitive), i.e. they
/// match at a DN-component boundary (so "ou=people2,…" ≠ "ou=people,…").
fn dn_boundary_match(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim().to_lowercase(), b.trim().to_lowercase());
    if a == b { return true; }
    a.ends_with(&format!(",{b}")) || b.ends_with(&format!(",{a}"))
}
```

- [ ] **Step 4-5: Run, pass, commit.**

```bash
git add -A && git commit -m "feat(create): profiles_for_container DN-boundary matcher

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

### Task 6.2: F7 chooser overlay + wiring

**Files:** `src/ui/app.rs` (F7 handler ~584 → compute matches; `Overlay::ChooseProfile { indices, sel }`; key handling; on Enter → `NewEntry(idx)`), `src/ui/view.rs` (render the select list).

- [ ] **Step 1: Failing tests** — (a) F7 with two matching profiles opens `Overlay::ChooseProfile` with both indices; (b) F7 with exactly one match installs a create form directly (no overlay); (c) F7 with zero matches falls back to all profiles.

> F7 currently returns `UiAction::NewEntry(0)` from `dispatch_key` (pure, no `structure`/profiles access there). Move the chooser decision to where `profiles`/`structure` are available: have F7 return a new `UiAction::NewEntryChoose` and compute matches in `handle_ui_action` (which has `profiles`, `structure`, `base_dn`). The "current container" = selected node's DN, or its parent if it's a leaf (use the structure's selection APIs already used elsewhere).

- [ ] **Step 2-3: Implement** — add `UiAction::NewEntryChoose`; in `handle_ui_action` compute `container`, `let mut m = profiles_for_container(profiles, &container); if m.is_empty() { m = (0..profiles.len()).collect(); }`; then `match m.len() { 0 => {}, 1 => self-dispatch NewEntry(m[0]), _ => app.overlay = Some(Overlay::ChooseProfile { indices: m, sel: 0 }) }`. Add the overlay variant, its key handler (↑↓/Enter/Esc) → on Enter dispatch `NewEntry(indices[sel])`, and `render` (a simple bordered list of `profiles[idx].name`).

- [ ] **Step 4-5: Run, pass, commit.**

```bash
git add -A && git commit -m "feat(ui): context-filtered profile chooser on F7

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

### Task 6.3: README example + final tmux smoke

- [ ] Add a multi-profile config example (user/group + defaults + password + lookup) to `README.md`. Manual smoke: F7 in different containers offers the right profiles; create a posix user end-to-end (defaults fill, uidNumber allocates, password sets, gidNumber via lookup). Update `docs/HANDOVER.md` milestone table. Commit.

```bash
git add -A && git commit -m "docs: rich-templates config example + handover update

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Self-Review (run before handing off to execution)

**Spec coverage** — each spec item maps to tasks:
- §5.0 unification → Phase 0 (0.1–0.6). ✓
- §4/§D1 `object_classes` → Phase 1. ✓
- §5.2 defaults engine → Phase 2; wiring → Phase 3. ✓
- §D6 truncation refusal → Task 3.1 + 3.2 (`Response::Entries.truncated`, `decide_allocation`). ✓
- §D7/§D8 password (create+edit, mask, suppress dup) → Phase 4. ✓
- §5.3 value-lookup picker → Phase 5. ✓
- §5.4/§D10 profile chooser → Phase 6. ✓
- §6 tests: pure tests inline per task; gated `tests/live_templates.rs` (3.4, 4.6, 5.3). ✓

**Type consistency** — `FormMode`, `DefaultValue`, `Resolution`, `ProfileDefaults`, `PasswordSpec`, `LookupSpec`, `profiles_for_container`, `allocate_number`, `decide_allocation`, `password_add_attrs`, `pick_value` are used with the same signatures wherever referenced. `Response::Entries` gains `truncated` (consumers updated in 3.1).

**Known risk (call out at execution):** `app.rs` is large; Tasks 0.3–0.5, 3.3, 4.4–4.5, 5.2, 6.2 edit it. Each is a separate commit and the heaviest (0.5 modal removal, 5.2 lookup wiring) should be resolved in-session if a subagent's context runs short. The exact test-helper names (`test_app`, `test_read_flow_*`, `set_field_value`, `test_user_profile`) must be reconciled with the actual `src/ui/app.rs` test module on first use — mirror existing constructions, don't invent fields.

---

## Execution Handoff

Per the plan header, implement with **superpowers:subagent-driven-development** (fresh subagent per task + two-stage review) — phases are sequential; Phase 0 must land green before Phase 1+. App.rs-heavy tasks are flagged for in-session resolution.
