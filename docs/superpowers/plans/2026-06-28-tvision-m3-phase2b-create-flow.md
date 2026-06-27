# M3 Phase 2b — create flow (+ autonumber + password widget) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create new LDAP entries from a profile in the tvision UI — Alt+N chooser, create-mode form (objectClass auto-injected + editable, live DN), auto-allocated next-free numbers, a TLS-gated password widget, and an ADD submit that navigates to the new entry.

**Architecture:** Controller-owned modals (like 2a): panes/pump post commands; `app::dispatch` runs the chooser/confirm/password modals and submits via the worker. Create-form composition + validation are pure (`workflows::create`); the ADD submit and autonumber scans are async via the worker + pump. The password widget reuses the 2a `FieldEditor` seam.

**Tech Stack:** Rust, tvision-rs 0.3.0, the neutral `workflows`/`config`/`schema`/`samba` layers, the existing worker (`Request::Add`/`Search`).

## Global Constraints

- **Scope:** the full create story in three sequenced blocks — **A** core create, **B** autonumber, **C** password widget. No other M4 widgets. No ratatui (`src/ui/**`) changes.
- **Facade boundary:** only `src/tui/**` + `src/bin/edaptor-tv.rs` may `use tvision_rs`; only `src/ui/**` may `use ratatui`/`use tui_*`; the neutral layers (`config`, `form`, `ldap`, `schema`, `samba`, `workflows`) import NEITHER. Guards (must print nothing): `! grep -rl "use tvision_rs" src | grep -vE "^src/(tui/|bin/edaptor-tv.rs)"` and `! grep -rl "use ratatui\|use tui_" src | grep -vE "^src/ui/"`.
- **Borrow discipline (load-bearing — the 2a panic lesson):** never hold a `RefCell`/`UiState` borrow across `exec_view*`/`ctx.post`/`ctx.broadcast`/`new_list`/`child_mut`/`set_value*`. A modal editor must NOT `borrow_mut` shared state during construction/`into_view`; stage live on events (the `reset_current` pattern from 2a `oc_picker.rs`).
- **Cap parallelism at 4 cores:** `cargo … -j4`. Package name `edaptor`. Dev binary at `/home/oetiker/scratch/cargo-target/debug/edaptor-tv`.
- **Strict TDD**, atomic commits, crate compiles + `cargo fmt` + `cargo clippy --all-targets -j4 -- -D warnings` clean after every commit.
- **Commit trailer:** `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. Use `git commit -F` (heredoc) for messages with backticks.
- **Live tests gated** by `EDAPTOR_TEST_LDAP_URI` (skip when unset); demo base `dc=example,dc=org`, `EDAPTOR_TEST_ADMIN_PW=adminpassword`. The demo is plain `ldap://` (unencrypted) — password-create is NOT live-testable there; test the password **refusal** instead.
- **Password secrecy:** cleartext lives only in `pending_password` + the editor; never written to `EditField.values` (except a masked sentinel), never rendered, masked in LDIF (`mask_password_attrs`).

## File structure

| File | Change | Responsibility |
|---|---|---|
| `src/workflows/edit_form.rs` | modify | `FormMode::Create{profile_idx,container}`; `composed_create_dn`. |
| `src/workflows/create.rs` | modify | `build_create_form` (compose the create form + autonumber requests). |
| `src/workflows/alloc_flow.rs` | **create** | `AllocFlow`: async next-free-number scan + correlation. |
| `src/workflows/write_flow.rs` | modify | `submit_create` (`Request::Add`) + `WriteIntent::Create` + `WriteOutcome::Created`. |
| `src/workflows/widget_bind.rs` | **create** | `apply_widget_bindings` (neutral port of `inject_resolver_kinds`). |
| `src/workflows/save.rs` | modify | fold a staged password into `password_mods` for the edit prepare. |
| `src/tui/state.rs` | modify | `connection_encrypted`, `alloc_flow`, `resolved_widgets`; `apply_write_outcome` Created; `apply_commit` StageSecret; bootstrap wiring. |
| `src/tui/pump.rs` | modify | route Entries → AllocFlow; fill autonumber fields. |
| `src/tui/app.rs` | modify | `CREATE` dispatch arm; `do_create`; Alt-S FormMode branch. |
| `src/tui/mod.rs` | modify | `CREATE` command; menu/status "New". |
| `src/tui/dialog/profile_chooser.rs` | **create** | profile-chooser `Dialog`. |
| `src/tui/widget.rs` | modify | `widget_for` Password routing; `is_modal_field` for password. |
| `src/tui/pw_editor.rs` | **create** | `PasswordWidget` + `PasswordEditor` (TLS-gated New+Confirm → StageSecret). |
| `tests/tv_create.rs` | **create** | gated live create + password-refusal tests. |
| `CHANGES.md` | modify | per-block entries. |

---

## Block A — core create

### Task 1: `FormMode::Create` + `composed_create_dn`

**Files:** Modify + Test: `src/workflows/edit_form.rs`

**Interfaces:**
- Produces: `FormMode::Create { profile_idx: usize, container: String }`; `pub fn composed_create_dn(rdn_attr: &str, rdn_value: &str, container: &str) -> String`.

- [ ] **Step 1: Write failing tests** (add to the `tests` module):

```rust
#[test]
fn composed_create_dn_uses_rdn_and_container() {
    assert_eq!(
        composed_create_dn("uid", "  alice ", "ou=people,dc=example,dc=org"),
        "uid=alice,ou=people,dc=example,dc=org"
    );
}

#[test]
fn composed_create_dn_placeholder_when_rdn_empty() {
    assert_eq!(
        composed_create_dn("uid", "   ", "ou=people,dc=example,dc=org"),
        "uid=…,ou=people,dc=example,dc=org"
    );
}
```

- [ ] **Step 2: Run — expect FAIL** (`composed_create_dn` undefined):
Run: `cargo test -j4 --lib edit_form 2>&1 | tail -20`

- [ ] **Step 3: Implement.** Change the `FormMode` enum:

```rust
/// Create vs edit. `Create` composes a new entry of `profile_idx` under `container`.
pub enum FormMode {
    Edit,
    Create { profile_idx: usize, container: String },
}
```

Add near the bottom (before `#[cfg(test)]`):

```rust
/// The DN a create-mode form would produce: `<rdn_attr>=<rdn_value>,<container>`,
/// with the RDN value trimmed. When the value is blank, a `…` placeholder stands in
/// (so the header reads `uid=…,ou=…`). Pure.
pub fn composed_create_dn(rdn_attr: &str, rdn_value: &str, container: &str) -> String {
    let v = rdn_value.trim();
    let shown = if v.is_empty() { "…" } else { v };
    format!("{rdn_attr}={shown},{container}")
}
```

- [ ] **Step 4: Run — expect PASS:** `cargo test -j4 --lib edit_form 2>&1 | tail -20`
- [ ] **Step 5: fmt + clippy + build both bins + commit**

```bash
cargo fmt && cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -3
cargo build -j4 2>&1 | tail -2
git add src/workflows/edit_form.rs
git commit -F - <<'MSG'
feat(edit_form): FormMode::Create + composed_create_dn

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
MSG
```

Note: adding the `Create` variant makes existing `match form.mode` sites non-exhaustive. The build will flag them; add `FormMode::Create { .. } =>` arms that fall through to the existing behavior where a match exists (e.g. none currently branch on mode in the neutral layer — only `mode: FormMode::Edit` constructors exist, which still compile). If a non-exhaustive match error appears, handle it minimally (the create-specific branches land in later tasks).

---

### Task 2: `build_create_form`

**Files:** Modify + Test: `src/workflows/create.rs`

**Interfaces:**
- Consumes: `empty_form_for_profile`, `build_edit_form`, `EditForm`, `EditField`, `EditForm::sync_schema_fields`, `apply_static_defaults`, `FormMode::Create` (Task 1).
- Produces: `pub fn build_create_form(schema: &SchemaModel, profile: &EntryProfile, profile_idx: usize, container: &str) -> (EditForm, Vec<(String, u64, u64)>)` — the create form (objectClass field seeded with `["top"]+profile.object_classes`, MUST/MAY fields present, static defaults filled) plus the autonumber requests `(attr, min, max)` still needing a scan.

- [ ] **Step 1: Write failing test:**

```rust
#[test]
fn build_create_form_injects_objectclass_and_resolves_fields() {
    // schema with person (MUST sn,cn MAY description) + organizationalPerson.
    let raw = crate::ldap::worker::RawSubschema {
        object_classes: vec![
            "( 2.5.6.0 NAME 'top' ABSTRACT MUST objectClass )".into(),
            "( 2.5.6.6 NAME 'person' SUP top STRUCTURAL MUST ( sn $ cn ) MAY description )".into(),
        ],
        attribute_types: vec![
            "( 2.5.4.3 NAME 'cn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 SINGLE-VALUE )".into(),
            "( 2.5.4.4 NAME 'sn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".into(),
            "( 2.5.4.13 NAME 'description' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".into(),
        ],
        ldap_syntaxes: vec![],
    };
    let schema = crate::schema::SchemaModel::from_raw(&raw);
    let profile = EntryProfile {
        name: "People".into(),
        object_classes: vec!["person".into()],
        rdn_attr: "cn".into(),
        search_base: "ou=people,dc=example,dc=org".into(),
        show: vec![],
        search_attrs: vec![],
        defaults: Default::default(),
        widgets: Default::default(),
        label: None,
    };
    let (form, autonum) = build_create_form(&schema, &profile, 0, "ou=people,dc=example,dc=org");
    assert!(matches!(form.mode, crate::workflows::edit_form::FormMode::Create { profile_idx: 0, .. }));
    let oc = form.fields.iter().find(|f| f.label.eq_ignore_ascii_case("objectClass")).unwrap();
    assert!(oc.values.iter().any(|v| v.eq_ignore_ascii_case("top")));
    assert!(oc.values.iter().any(|v| v.eq_ignore_ascii_case("person")));
    assert!(form.fields.iter().any(|f| f.label == "sn")); // MUST injected by resync
    assert!(form.object_classes.iter().any(|v| v == "person"));
    assert!(autonum.is_empty()); // no {next:…} default in this profile
}
```

- [ ] **Step 2: Run — expect FAIL:** `cargo test -j4 --lib create 2>&1 | tail -20`

- [ ] **Step 3: Implement** (add to `src/workflows/create.rs`; imports `EditField`, `EditForm`, `FormMode`, `build_edit_form`, `WidgetSpec`, `FieldKind` as needed):

```rust
/// Compose a create-mode [`EditForm`] for `profile` under `container`: a schema-driven
/// empty form (`empty_form_for_profile`), with an editable `objectClass` field seeded
/// with `["top"] + profile.object_classes` (deduped, case-insensitive) so the picker
/// can edit it and `sync_schema_fields` injects the effective MUST/MAY fields; then
/// static defaults are applied. Returns the form plus the autonumber requests
/// `(attr, min, max)` that still need a directory scan (Block B fills them). Pure.
pub fn build_create_form(
    schema: &SchemaModel,
    profile: &EntryProfile,
    profile_idx: usize,
    container: &str,
) -> (EditForm, Vec<(String, u64, u64)>) {
    use crate::schema::FieldKind;
    use crate::workflows::edit_form::{build_edit_form, EditField, FormMode};
    use crate::workflows::form_model::WidgetSpec;

    let model = empty_form_for_profile(schema, profile);
    let mut form = build_edit_form(&model, schema, false);
    form.mode = FormMode::Create {
        profile_idx,
        container: container.to_string(),
    };

    // Canonical objectClass set: ["top"] + profile classes, deduped case-insensitively.
    let mut ocs: Vec<String> = vec!["top".to_string()];
    for oc in &profile.object_classes {
        if !ocs.iter().any(|x| x.eq_ignore_ascii_case(oc)) {
            ocs.push(oc.clone());
        }
    }
    form.object_classes = ocs.clone();

    // Ensure an editable objectClass field carrying that set (auto-injection).
    if let Some(f) = form
        .fields
        .iter_mut()
        .find(|f| f.label.eq_ignore_ascii_case("objectClass"))
    {
        f.values = ocs.clone();
    } else {
        form.fields.push(EditField {
            label: "objectClass".to_string(),
            must: true,
            editable: true,
            multi: true,
            secret: false,
            ordered: false,
            orphaned: false,
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: None,
            values: ocs.clone(),
            baseline: Vec::new(),
        });
    }

    // Regenerate fields for the seeded objectClass set.
    form.sync_schema_fields(schema);

    // Apply static defaults; collect autonumber requests. Work on an attrs map, then
    // write filled values back into the (still-empty) fields.
    let mut attrs: std::collections::BTreeMap<String, Vec<String>> = form
        .fields
        .iter()
        .map(|f| (f.label.clone(), f.values.clone()))
        .collect();
    let autonum = apply_static_defaults(&profile.defaults, &mut attrs);
    for f in &mut form.fields {
        if f.values.is_empty() {
            if let Some(v) = attrs.get(&f.label) {
                if !v.is_empty() {
                    f.values = v.clone();
                }
            }
        }
    }

    (form, autonum)
}
```

- [ ] **Step 4: Run — expect PASS:** `cargo test -j4 --lib create 2>&1 | tail -20`
- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt && cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -3
git add src/workflows/create.rs
git commit -F - <<'MSG'
feat(create): build_create_form (objectClass auto-inject + resync + defaults)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
MSG
```

---

### Task 3: create-mode form header (live DN)

**Files:** Modify + Test: `src/tui/panes/form.rs`

**Interfaces:**
- Consumes: `composed_create_dn`, `FormMode::Create`, `EntryProfile.rdn_attr` (via `UiState.profiles`).
- Produces: in create mode the header shows the composing DN + ` (new)` + dirty `*`.

The current `header_text(form)` is `format!("{}{}", form.dn, mark)`. For create mode `form.dn` is unset; compose it from the `rdn_attr` field's current value + container.

- [ ] **Step 1: Write failing test** (mirror the existing form-pane test harness; build a create-mode form):

```rust
#[test]
fn create_mode_header_composes_dn_from_rdn_field() {
    use crate::workflows::edit_form::{EditField, EditForm, FormMode};
    use crate::schema::FieldKind;
    use crate::workflows::form_model::WidgetSpec;
    // profile_idx 0 with rdn_attr "uid"; one editable uid field.
    let (shared, mut pane, mut ctx_owned) = build_pane_with_create_form(
        0,
        "ou=people,dc=example,dc=org",
        "uid",
        vec![EditField {
            label: "uid".into(), must: true, editable: true, multi: false, secret: false,
            ordered: false, orphaned: false, kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText, widget_binding: None,
            values: vec!["alice".into()], baseline: vec![],
        }],
    );
    let ctx = &mut ctx_owned;
    let mut tick = Event::Broadcast { command: REFRESH, data: 0 };
    pane.handle_event(&mut tick, ctx);
    let hdr = pane.header_text_for_test();
    assert!(hdr.contains("uid=alice,ou=people,dc=example,dc=org"));
    assert!(hdr.contains("(new)"));
}
```

Add a `build_pane_with_create_form(profile_idx, container, rdn_attr, fields)` test helper alongside the existing form-pane test setup: it builds a `UiState` (via `new_for_test`) with one profile whose `rdn_attr` is set, sets `edit_form = Some(EditForm { dn: String::new(), mode: FormMode::Create{profile_idx, container}, object_classes: vec![], fields })`, and constructs the `FormPane`. Add a `#[cfg(test)] pub(crate) fn header_text_for_test(&self) -> String` seam that returns the header it would render.

- [ ] **Step 2: Run — expect FAIL.** Run: `cargo test -j4 --lib panes::form 2>&1 | tail -20`

- [ ] **Step 3: Implement.** Replace the body of `header_text` so it branches on mode. Because `header_text(form)` currently takes only `&EditForm` but create-mode needs the profile's `rdn_attr` (from `UiState.profiles[profile_idx]`), compute the header inside the pane (which holds `state`) rather than the free fn. Concretely:

- Keep `header_text` for `FormMode::Edit` (`form.dn` + mark).
- Add a `FormPane` method `fn header_text(&self) -> String` that reads `state.edit_form`: for `Edit` → `dn + mark`; for `Create { profile_idx, container }` → look up `state.profiles[profile_idx].rdn_attr`, read the current value of the field whose label == that rdn_attr, and `format!("{} (new){}", composed_create_dn(rdn_attr, rdn_value, container), mark)`. Drop the state borrow before touching views (collect the string, then set the header cell). Update `render` and `sync_into_form` to use this method instead of the free `header_text(form)`. The `#[cfg(test)] header_text_for_test` calls the same logic.

Borrow discipline: compute the header string under a short `state.borrow()`, drop it, then `self.group.child_mut(header_id).set_value(...)`.

- [ ] **Step 4: Run — expect PASS:** `cargo test -j4 --lib panes::form 2>&1 | tail -20` (and confirm existing form-pane tests still pass).
- [ ] **Step 5: fmt + clippy + build + commit**

```bash
cargo fmt && cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -3
cargo build -j4 --bin edaptor-tv 2>&1 | tail -2
git add src/tui/panes/form.rs
git commit -F - <<'MSG'
feat(tui/form): create-mode header composes the live DN from the RDN field

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
MSG
```

---

### Task 4: `write_flow` create-submit (ADD)

**Files:** Modify + Test: `src/workflows/write_flow.rs`

**Interfaces:**
- Consumes: `Request::Add { id, dn, attrs }`, `Response::WriteOk/WriteError`.
- Produces: `WriteFlow::submit_create(&mut self, worker: &WorkerHandle, dn: &str, attrs: BTreeMap<String, Vec<String>>, quit_after: bool) -> Result<()>`; `WriteOutcome::Created { dn: String, quit_after: bool }`.

- [ ] **Step 1: Write failing test** (mirror the existing write_flow tests, which exercise `on_response` with a fake `Response`):

```rust
#[test]
fn create_submit_tracks_then_reports_created() {
    let mut wf = WriteFlow::new();
    // No worker in unit tests; drive on_response directly by inserting a Create intent.
    // Simulate: record a Create intent for id, then feed WriteOk.
    let id = wf.alloc_for_test(); // see seam below
    wf.insert_create_for_test(id, "uid=bob,ou=people,dc=example,dc=org".into(), false);
    let out = wf.on_response(&crate::ldap::worker::Response::WriteOk {
        id,
        dn: "uid=bob,ou=people,dc=example,dc=org".into(),
    });
    assert!(matches!(out, WriteOutcome::Created { quit_after: false, .. }));
}
```

(Mirror however the existing write_flow tests inject pending intents; if they call `submit` with a real worker mock, follow that instead and drop the `*_for_test` seams. The existing tests in this file are the template — match them.)

- [ ] **Step 2: Run — expect FAIL.** Run: `cargo test -j4 --lib write_flow 2>&1 | tail -20`

- [ ] **Step 3: Implement.** Add a `WriteIntent::Create { dn: String, quit_after: bool }` variant; `submit_create`:

```rust
    /// Submit a new entry (ADD). On WriteOk, [`on_response`] yields
    /// [`WriteOutcome::Created`].
    pub fn submit_create(
        &mut self,
        worker: &WorkerHandle,
        dn: &str,
        attrs: std::collections::BTreeMap<String, Vec<String>>,
        quit_after: bool,
    ) -> Result<()> {
        let id = self.alloc();
        worker.submit(Request::Add {
            id,
            dn: dn.to_string(),
            attrs,
        })?;
        self.pending.insert(
            id,
            WriteIntent::Create {
                dn: dn.to_string(),
                quit_after,
            },
        );
        Ok(())
    }
```

Add the `WriteOutcome::Created { dn: String, quit_after: bool }` variant and, in `on_response`'s `WriteOk` arm, the match:

```rust
                Some(WriteIntent::Create { dn, quit_after }) => {
                    WriteOutcome::Created { dn, quit_after }
                }
```

(If the test uses `*_for_test` seams, add `#[cfg(test)] pub(crate) fn alloc_for_test(&mut self) -> u64 { self.alloc() }` and `#[cfg(test)] pub(crate) fn insert_create_for_test(&mut self, id: u64, dn: String, quit_after: bool) { self.pending.insert(id, WriteIntent::Create { dn, quit_after }); }`.)

- [ ] **Step 4: Run — expect PASS:** `cargo test -j4 --lib write_flow 2>&1 | tail -20`
- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt && cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -3
git add src/workflows/write_flow.rs
git commit -F - <<'MSG'
feat(write_flow): submit_create (Request::Add) + WriteOutcome::Created

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
MSG
```

---

### Task 5: navigate after create (`apply_write_outcome` Created)

**Files:** Modify + Test: `src/tui/state.rs`

**Interfaces:**
- Consumes: `WriteOutcome::Created` (Task 4), `reread_public`.
- Produces: on `Created`, the state navigates to the new entry (sets `current_leaf`, `list_dirty`, re-reads → reloads in `FormMode::Edit`) and clears the create form.

- [ ] **Step 1: Write failing test.** In the `state.rs` tests, build a worker-less `UiState` with a create-mode `edit_form`, call `apply_write_outcome(WriteOutcome::Created { dn: "uid=bob,…".into(), quit_after: false })`, and assert: `current_leaf == Some("uid=bob,…")`, `list_dirty == true`, and `changed` is set in the returned `PumpResult`. (Re-read submission needs a worker; with `worker: None`, assert the state mutations + that it doesn't panic — mirror how existing `apply_write_outcome` tests handle the worker-less case.)

```rust
#[test]
fn created_navigates_to_new_entry() {
    let mut st = /* new_for_test with a FormMode::Create edit_form */;
    let r = st.apply_write_outcome(WriteOutcome::Created {
        dn: "uid=bob,ou=people,dc=example,dc=org".into(),
        quit_after: false,
    });
    assert_eq!(st.current_leaf.as_deref(), Some("uid=bob,ou=people,dc=example,dc=org"));
    assert!(st.list_dirty);
    assert!(r.changed);
}
```

- [ ] **Step 2: Run — expect FAIL.** Run: `cargo test -j4 --lib tui::state 2>&1 | tail -20`

- [ ] **Step 3: Implement.** In `apply_write_outcome`, add the `Created` arm (capture the form's `object_classes` for the re-read before clearing):

```rust
            WriteOutcome::Created { dn, quit_after } => {
                let ocs = self
                    .edit_form
                    .as_ref()
                    .map(|f| f.object_classes.clone())
                    .unwrap_or_default();
                self.current_leaf = Some(dn.clone());
                self.list_dirty = true;
                self.edit_form = None; // re-read reloads it in Edit mode
                if self.worker.is_some() {
                    self.reread_public(&dn, &ocs);
                }
                PumpResult { changed: true, quit: quit_after, error: false }
            }
```

(Adapt the `PumpResult` construction to the struct's actual field set — see the existing arms.)

- [ ] **Step 4: Run — expect PASS:** `cargo test -j4 --lib tui::state 2>&1 | tail -20`
- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt && cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -3
git add src/tui/state.rs
git commit -F - <<'MSG'
feat(tui/state): navigate to the new entry on WriteOutcome::Created

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
MSG
```

---

### Task 6: profile chooser dialog

**Files:** Create + Test: `src/tui/dialog/profile_chooser.rs`; Modify: `src/tui/dialog/mod.rs` (add `pub(crate) mod profile_chooser;`)

**Interfaces:**
- Produces: a `Dialog` listing profile names + OK/Cancel, with the chosen index surfaced to the controller. Mirror the staging pattern from 2a (`oc_picker.rs`): the dialog keeps the highlighted index in a shared slot; `dispatch` reads it on OK.
- Produces: add `pub chosen_profile: Option<usize>` to `UiState` (set by the chooser, read by dispatch).

- [ ] **Step 1: Write failing test** (headless `Context`, mirror `oc_picker.rs` tests): build the chooser over `["People","Groups"]` with a `Shared`, call `reset_current`, then assert the initial `chosen_profile` reflects the highlighted row (0); after a `Down` event, it reflects 1.

- [ ] **Step 2: Run — expect FAIL.** Run: `cargo test -j4 --lib profile_chooser 2>&1 | tail -20`

- [ ] **Step 3: Implement.** Model on `src/tui/dialog/confirm.rs` (Dialog + button_row) and `src/tui/oc_picker.rs` (ListBox + reset_current seed + staging into shared). The chooser:
  - `pub(crate) fn build(names: Vec<String>, shared: Shared) -> (Box<dyn View>, tv::ViewId)` returning the dialog + the list's ViewId (focus the list).
  - A `ProfileChooser` view (`#[delegate(to = dlg)]`) holding the names + `shared`; `reset_current` seeds the `ListBox` (`new_list`) and writes `shared.borrow_mut().chosen_profile = Some(0)`; `handle_event` updates `chosen_profile` from the list selection after nav. Buttons OK/CANCEL.
  - Add `chosen_profile: Option<usize>` to `UiState` (init `None` in every constructor — grep `set_tree_row: None`).
  - Borrow discipline: never `borrow_mut` during construction; stage in `reset_current`/on events (2a lesson).

- [ ] **Step 4: Run — expect PASS:** `cargo test -j4 --lib profile_chooser 2>&1 | tail -20`
- [ ] **Step 5: fmt + clippy + build + commit**

```bash
cargo fmt && cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -3
cargo build -j4 --bin edaptor-tv 2>&1 | tail -2
git add src/tui/dialog/profile_chooser.rs src/tui/dialog/mod.rs src/tui/state.rs
git commit -F - <<'MSG'
feat(tui): profile chooser dialog + UiState.chosen_profile

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
MSG
```

---

### Task 7: `CREATE` command + menu/status + dispatch entry (`open_create`)

**Files:** Modify: `src/tui/mod.rs`, `src/tui/app.rs`

**Interfaces:**
- Consumes: `profiles_for_container`, `build_create_form` (Task 2), `profile_chooser::build` (Task 6), `chosen_profile`.
- Produces: `CREATE` command (Alt+N); dispatch builds the create form into `state.edit_form`.

- [ ] **Step 1.** In `src/tui/mod.rs` add: `pub const CREATE: tv::Command = tv::Command::custom("edaptor.create");`. In `app.rs` `init_status_line` add `.item("~Alt-N~ New", alt('n'), CREATE)` and `init_menu_bar` add `.command_key("~N~ew", CREATE, alt('n'), "Alt-N")`. Extend the `use crate::tui::{…}` import with `CREATE`.

- [ ] **Step 2.** Add the `CREATE` arm to `dispatch` (after `SAVE`). Pure helper `open_create` lives in `app.rs`:

```rust
    } else if cmd == CREATE {
        // Container = the current branch.
        let container = state.borrow().current_branch.clone();
        let Some(container) = container else {
            state.borrow_mut().status = "Select a container first.".into();
            return;
        };
        let idxs = {
            let st = state.borrow();
            crate::workflows::create::profiles_for_container(&st.profiles, &container)
        };
        match idxs.as_slice() {
            [] => {
                state.borrow_mut().status = "No profile for this container.".into();
            }
            [only] => open_create(state, *only, &container),
            _ => {
                // >1: run the chooser, then open the chosen profile.
                let names: Vec<String> = {
                    let st = state.borrow();
                    idxs.iter().map(|i| st.profiles[*i].name.clone()).collect()
                };
                let (view, focus) = crate::tui::dialog::profile_chooser::build(names, state.clone());
                if prog.exec_view_focused(view, focus) == Command::OK {
                    let chosen = state.borrow_mut().chosen_profile.take();
                    if let Some(rel) = chosen {
                        if let Some(idx) = idxs.get(rel) {
                            open_create(state, *idx, &container);
                        }
                    }
                } else {
                    state.borrow_mut().chosen_profile = None;
                }
            }
        }
```

```rust
/// Build a create-mode form for `profile_idx` under `container` and install it.
/// (Block B posts the autonumber scans for the returned requests; here they are
/// just dropped until B wires them — `let _ = autonum`.)
fn open_create(state: &Shared, profile_idx: usize, container: &str) {
    let form_and_reqs = {
        let st = state.borrow();
        let schema = st.read_flow.schema();
        let profile = &st.profiles[profile_idx];
        crate::workflows::create::build_create_form(schema, profile, profile_idx, container)
    };
    let (form, _autonum) = form_and_reqs;
    let mut st = state.borrow_mut();
    st.edit_form = Some(form);
    st.form_needs_render = true;
}
```

- [ ] **Step 3.** Build + manual check: `cargo build -j4 --bin edaptor-tv 2>&1 | tail -3` and `cargo test -j4 --lib 2>&1 | tail -5` (no regressions). (Live behavior is exercised in Task 9 / acceptance.)
- [ ] **Step 4: fmt + clippy + commit**

```bash
cargo fmt && cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -3
git add src/tui/mod.rs src/tui/app.rs
git commit -F - <<'MSG'
feat(tui): Alt+N create entry point (chooser / fast path) builds the create form

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
MSG
```

---

### Task 8: `do_create` + Alt-S FormMode branch

**Files:** Modify: `src/tui/app.rs`

**Interfaces:**
- Consumes: `plan_create`, `CreatePrep`, `write_flow.submit_create` (Task 4), `confirm::build`.
- Produces: Alt-S in `FormMode::Create` submits an ADD.

- [ ] **Step 1.** In the `SAVE` arm of `dispatch`, branch on the form mode:

```rust
    if cmd == SAVE {
        let is_create = matches!(
            state.borrow().edit_form.as_ref().map(|f| &f.mode),
            Some(crate::workflows::edit_form::FormMode::Create { .. })
        );
        if is_create {
            do_create(prog, state);
        } else {
            let _ = do_save(prog, state, None, false);
        }
    }
```

- [ ] **Step 2.** Implement `do_create` (borrow-disciplined; mirrors `do_save`):

```rust
fn do_create(prog: &mut Program, state: &Shared) {
    use crate::workflows::create::{plan_create, CreatePrep};
    use crate::workflows::edit_form::FormMode;
    // 1. Compute the plan (borrow, drop before exec_view / submit).
    let prep = {
        let st = state.borrow();
        let Some(form) = st.edit_form.as_ref() else { return };
        let FormMode::Create { profile_idx, container } = &form.mode else { return };
        let profile = &st.profiles[*profile_idx];
        plan_create(st.read_flow.schema(), profile, container, &form.to_edit_entry())
    };
    match prep {
        CreatePrep::Error(msg) => {
            let (view, ok) = crate::tui::dialog::error::build(&msg);
            prog.exec_view_focused(view, ok);
        }
        CreatePrep::Confirm { dn, attrs, ldif, .. } => {
            let (view, save) = crate::tui::dialog::confirm::build(&ldif);
            if prog.exec_view_focused(view, save) != Command::OK {
                return; // cancel: keep editing the create form.
            }
            let mut st = state.borrow_mut();
            let crate::tui::state::UiState { worker, write_flow, .. } = &mut *st;
            if let Some(w) = worker.as_ref() {
                let _ = write_flow.submit_create(w, &dn, attrs, false);
            }
        }
    }
}
```

(Block C extends `do_create` to fold the staged password into `attrs`/`ldif` before the confirm — see Task 17.)

- [ ] **Step 3.** Build + lib tests: `cargo build -j4 --bin edaptor-tv 2>&1 | tail -3`; `cargo test -j4 --lib 2>&1 | tail -5`.
- [ ] **Step 4: fmt + clippy + commit**

```bash
cargo fmt && cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -3
git add src/tui/app.rs
git commit -F - <<'MSG'
feat(tui/app): do_create — Alt-S in create mode validates, confirms, submits ADD

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
MSG
```

---

### Task 9: live create test + CHANGES (Block A)

**Files:** Create: `tests/tv_create.rs`; Modify: `CHANGES.md`

- [ ] **Step 1.** Write a gated test in `tests/tv_create.rs` (skip unless `EDAPTOR_TEST_LDAP_URI`; reuse the connect/subschema helpers from `tests/tv_edit_write.rs`, copying as that file does). The test drives the **neutral** path: `build_create_form` for a People profile + a chosen RDN, `plan_create` → `CreatePrep::Confirm`, then submit the ADD via the worker and assert the entry exists; then **delete it** (worker `Request::Delete`, or create under a disposable RDN like `uid=zz-tv-create-test`) so the demo seed is restored. Assert read-back shows the objectClass set.

- [ ] **Step 2.** Run both ways:
```bash
cargo test -j4 --test tv_create 2>&1 | tail -8                              # skip → PASS
scripts/test-ldap.sh start
EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389 EDAPTOR_TEST_ADMIN_PW=adminpassword \
  cargo test -j4 --test tv_create 2>&1 | tail -12                           # live → PASS, demo restored
```

- [ ] **Step 3.** CHANGES.md (unreleased tvision-preview): "New entries can be created from a profile in the tvision UI (Alt+N): a profile chooser (or single-profile fast path), a create-mode form with auto-injected/editable objectClass and live DN, validated and submitted as an LDAP ADD."

- [ ] **Step 4.** Facade guards + commit:
```bash
! grep -rl "use tvision_rs" src | grep -vE "^src/(tui/|bin/edaptor-tv.rs)"
! grep -rl "use ratatui\|use tui_" src | grep -vE "^src/ui/"
cargo fmt && cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -3
git add tests/tv_create.rs CHANGES.md
git commit -F - <<'MSG'
test(tv): gated live create test + CHANGES (Block A)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
MSG
```

---

## Block B — autonumber (AllocFlow)

### Task 10: `AllocFlow`

**Files:** Create + Test: `src/workflows/alloc_flow.rs`; Modify: `src/workflows/mod.rs` (`pub mod alloc_flow;`)

**Interfaces:**
- Consumes: `Request::Search`, `SearchScope::Subtree`, `Response::{Entries, SearchError}`, `decide_allocation` (`workflows::save`).
- Produces: `AllocFlow::{new, request, on_response}`; `AllocOutcome::{Filled{attr,value}, Failed(String), Ignored}`.

- [ ] **Step 1: Write failing test:**

```rust
#[test]
fn alloc_fills_next_free_number() {
    let mut af = AllocFlow::new();
    let id = af.alloc_for_test(); // seam mirroring write_flow
    af.insert_for_test(id, "uidNumber".into(), 10000, 19999);
    let entries = vec![
        crate::ldap::worker::LdapEntry {
            dn: "uid=a,dc=x".into(),
            attrs: [("uidNumber".to_string(), vec!["10000".to_string()])].into_iter().collect(),
            bin_attrs: Default::default(),
        },
        crate::ldap::worker::LdapEntry {
            dn: "uid=b,dc=x".into(),
            attrs: [("uidNumber".to_string(), vec!["10005".to_string()])].into_iter().collect(),
            bin_attrs: Default::default(),
        },
    ];
    let out = af.on_response(&crate::ldap::worker::Response::Entries { id, entries, truncated: false });
    assert!(matches!(out, AllocOutcome::Filled { value, .. } if value == "10006"));
}

#[test]
fn alloc_refuses_truncated_scan() {
    let mut af = AllocFlow::new();
    let id = af.alloc_for_test();
    af.insert_for_test(id, "uidNumber".into(), 10000, 19999);
    let out = af.on_response(&crate::ldap::worker::Response::Entries { id, entries: vec![], truncated: true });
    assert!(matches!(out, AllocOutcome::Failed(_)));
}
```

- [ ] **Step 2: Run — expect FAIL.** Run: `cargo test -j4 --lib alloc_flow 2>&1 | tail -20`

- [ ] **Step 3: Implement** `src/workflows/alloc_flow.rs`:

```rust
//! Async next-free-number allocation: scan an attribute under the base, then pick
//! the next free value in [min,max] via [`crate::workflows::save::decide_allocation`].
//! Mirrors `read_flow`/`write_flow`; ids are disjoint by range so the pump can route
//! responses to exactly one flow.

use std::collections::HashMap;

use anyhow::Result;

use crate::ldap::worker::{Request, Response, SearchScope, WorkerHandle};
use crate::workflows::save::decide_allocation;

/// The result of correlating one scan response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllocOutcome {
    Filled { attr: String, value: String },
    Failed(String),
    Ignored,
}

pub struct AllocFlow {
    next_id: u64,
    pending: HashMap<u64, (String, u64, u64)>, // id -> (attr, min, max)
}

impl Default for AllocFlow {
    fn default() -> Self {
        Self::new()
    }
}

impl AllocFlow {
    pub fn new() -> Self {
        // Above ReadFlow (1) and WriteFlow (1_000_000) ranges.
        AllocFlow { next_id: 2_000_000, pending: HashMap::new() }
    }

    fn alloc(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Post a subtree scan of `attr` under `base`; returns the request id.
    pub fn request(
        &mut self,
        worker: &WorkerHandle,
        base: &str,
        attr: &str,
        min: u64,
        max: u64,
    ) -> Result<u64> {
        let id = self.alloc();
        worker.submit(Request::Search {
            id,
            base: base.to_string(),
            scope: SearchScope::Subtree,
            filter: format!("({attr}=*)"),
            attrs: vec![attr.to_string()],
            size_limit: None,
        })?;
        self.pending.insert(id, (attr.to_string(), min, max));
        Ok(id)
    }

    /// Correlate one response. Pure; ignores non-matching ids/variants.
    pub fn on_response(&mut self, resp: &Response) -> AllocOutcome {
        match resp {
            Response::Entries { id, entries, truncated } => {
                let Some((attr, min, max)) = self.pending.remove(id) else {
                    return AllocOutcome::Ignored;
                };
                let values: Vec<u64> = entries
                    .iter()
                    .flat_map(|e| {
                        e.attrs
                            .iter()
                            .find(|(k, _)| k.eq_ignore_ascii_case(&attr))
                            .map(|(_, v)| v.clone())
                            .unwrap_or_default()
                    })
                    .filter_map(|s| s.parse::<u64>().ok())
                    .collect();
                match decide_allocation(&values, *truncated, min, max) {
                    Ok(n) => AllocOutcome::Filled { attr, value: n.to_string() },
                    Err(e) => AllocOutcome::Failed(e),
                }
            }
            Response::SearchError { id, msg } => {
                if self.pending.remove(id).is_some() {
                    AllocOutcome::Failed(msg.clone())
                } else {
                    AllocOutcome::Ignored
                }
            }
            _ => AllocOutcome::Ignored,
        }
    }

    #[cfg(test)]
    pub(crate) fn alloc_for_test(&mut self) -> u64 { self.alloc() }
    #[cfg(test)]
    pub(crate) fn insert_for_test(&mut self, id: u64, attr: String, min: u64, max: u64) {
        self.pending.insert(id, (attr, min, max));
    }
}
```

(Confirm `Response::SearchError`'s field names against `worker.rs` — adjust `msg` if it differs.)

- [ ] **Step 4: Run — expect PASS:** `cargo test -j4 --lib alloc_flow 2>&1 | tail -20`
- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt && cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -3
git add src/workflows/alloc_flow.rs src/workflows/mod.rs
git commit -F - <<'MSG'
feat(workflows): AllocFlow — async next-free-number scan + allocation

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
MSG
```

---

### Task 11: pump wiring + open_create posts scans

**Files:** Modify: `src/tui/state.rs` (hold `alloc_flow`; a `fill_autonumber` helper), `src/tui/pump.rs` (route Entries→AllocFlow), `src/tui/app.rs` (`open_create` posts scans + `‹allocating…›` placeholder).

**Interfaces:**
- Consumes: `AllocFlow`, `AllocOutcome` (Task 10), the autonumber requests from `build_create_form` (Task 2).
- Produces: numeric create fields auto-fill via async scan.

- [ ] **Step 1: Write failing test** (state.rs): build a `UiState` with a create form having an empty `uidNumber` field showing `‹allocating…›`; call a new `apply_alloc_outcome(AllocOutcome::Filled { attr: "uidNumber".into(), value: "10006".into() })`; assert the field's value becomes `["10006"]` and `form_needs_render`.

- [ ] **Step 2: Run — expect FAIL.** Run: `cargo test -j4 --lib tui::state 2>&1 | tail -20`

- [ ] **Step 3: Implement.**
  - `UiState`: add `pub alloc_flow: AllocFlow` (init `AllocFlow::new()` in all constructors + bootstrap).
  - `apply_alloc_outcome(&mut self, out: AllocOutcome)`: on `Filled { attr, value }`, find the field by label; if its value is empty OR the `‹allocating…›` placeholder, set it to `[value]`; `form_needs_render = true`. On `Failed(msg)` set `status = msg` and clear the placeholder (leave empty). 
  - `pump.rs` `pump_worker`/the drain loop: for each response, after `read_flow.on_response` returns `Ignored`, try `alloc_flow.on_response`; if not `Ignored`, call `apply_alloc_outcome` and set `out.changed`. (Ids are disjoint, so a response matches exactly one flow.) Then fall through to write_flow as today.

  NOTE: `pump_worker` currently destructures/borrows `self`; keep the existing borrow structure and add the alloc step mirroring the read/write steps.
  - `app.rs` `open_create`: after installing the form, for each `(attr, min, max)` in `autonum`: set the field's value to `vec!["‹allocating…›".into()]` (placeholder) and post a scan via `state.alloc_flow.request(worker, base_dn, &attr, min, max)`. Borrow discipline: collect requests, drop borrow, submit, then re-borrow to set placeholders (or set placeholders in the same borrow before submitting — submitting needs `worker` which is in state; follow the `do_save` split-borrow idiom).

- [ ] **Step 4: Run — expect PASS:** `cargo test -j4 --lib tui::state 2>&1 | tail -20`; full suite `cargo test -j4 --lib 2>&1 | tail -5`; `cargo build -j4 --bin edaptor-tv 2>&1 | tail -2`.
- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt && cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -3
git add src/tui/state.rs src/tui/pump.rs src/tui/app.rs
git commit -F - <<'MSG'
feat(tui): auto-allocate next-free numbers on create-form open (async)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
MSG
```

---

### Task 12: autonumber live check + CHANGES (Block B)

**Files:** Modify: `tests/tv_create.rs`, `CHANGES.md`

- [ ] **Step 1.** Extend `tests/tv_create.rs` (gated) with an autonumber assertion: post an `AllocFlow` scan for `uidNumber` under the demo base and assert the returned value is `> max(existing)` (or within range). Reuse the worker handle from the create test.
- [ ] **Step 2.** Run live: `EDAPTOR_TEST_LDAP_URI=… cargo test -j4 --test tv_create 2>&1 | tail -12` → PASS.
- [ ] **Step 3.** CHANGES.md: "Create-form numeric fields with a `{next:MIN-MAX}` default auto-allocate the next free value via a background scan."
- [ ] **Step 4: commit**

```bash
cargo fmt && cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -3
git add tests/tv_create.rs CHANGES.md
git commit -F - <<'MSG'
test(tv): autonumber live check + CHANGES (Block B)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
MSG
```

---

## Block C — password widget (general, TLS-gated)

### Task 13: `connection_encrypted`

**Files:** Modify + Test: `src/tui/state.rs`

**Interfaces:**
- Consumes: `Config::is_encrypted()`.
- Produces: `UiState.connection_encrypted: bool`.

- [ ] **Step 1: Write failing test** (state.rs): `new_for_test` defaults it `false`; assert a `UiState` built with it has `connection_encrypted == false`. (Bootstrap path is exercised live; the unit test pins the field exists + default.)
- [ ] **Step 2: Run — expect FAIL.** Run: `cargo test -j4 --lib tui::state 2>&1 | tail -10`
- [ ] **Step 3: Implement.** Add `pub connection_encrypted: bool` to `UiState`; init `false` in `new_for_test`; in `bootstrap` set it from `config.is_encrypted()`.
- [ ] **Step 4: Run — expect PASS.** `cargo test -j4 --lib tui::state 2>&1 | tail -10`
- [ ] **Step 5: commit**

```bash
cargo fmt && cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -3
git add src/tui/state.rs
git commit -F - <<'MSG'
feat(tui/state): connection_encrypted from Config::is_encrypted()

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
MSG
```

---

### Task 14: neutral `apply_widget_bindings` + wire into edit & create

**Files:** Create + Test: `src/workflows/widget_bind.rs`; Modify: `src/workflows/mod.rs`, `src/tui/state.rs` (hold resolved widgets; call after `build_edit_form` in `pump_worker`), `src/workflows/create.rs` (call in `build_create_form`).

**Interfaces:**
- Consumes: `config::widget::resolve_widgets`, `config::resolver::WidgetResolver` (`new`, `resolve_kind(attr, ocs) -> Option<WidgetKind>`), `WidgetKind`.
- Produces: `pub fn apply_widget_bindings(form: &mut EditForm, resolver: &WidgetResolver, object_classes: &[String])` — sets `field.secret` (true iff Password) and `field.widget_binding` (when unset). Neutral port of `ui::edit_form::inject_resolver_kinds` (`src/ui/edit_form.rs:695`).

- [ ] **Step 1: Write failing test:** build a small schema + a profile with `[profile.widget.userPassword] kind="password"`, `resolve_widgets`, a `WidgetResolver`, an `EditForm` with a `userPassword` field; call `apply_widget_bindings`; assert that field's `secret == true` and `widget_binding` is `Some(WidgetKind::Password(_))`. (Model the resolver construction on `src/config/resolver.rs` tests, e.g. `WidgetResolver::new(&schema, &profiles, &widgets, false)`.)

- [ ] **Step 2: Run — expect FAIL.** Run: `cargo test -j4 --lib widget_bind 2>&1 | tail -20`

- [ ] **Step 3: Implement** `src/workflows/widget_bind.rs` (port `inject_resolver_kinds` to the neutral `EditForm`):

```rust
//! Apply resolved `[profile.widget.<attr>]` bindings onto a neutral `EditForm`'s
//! fields: set `secret` for password fields and attach `widget_binding` where unset.
//! Neutral port of `ui::edit_form::inject_resolver_kinds`.

use crate::config::resolver::WidgetResolver;
use crate::config::widget::WidgetKind;
use crate::workflows::edit_form::EditForm;

pub fn apply_widget_bindings(
    form: &mut EditForm,
    resolver: &WidgetResolver<'_>,
    object_classes: &[String],
) {
    for f in &mut form.fields {
        let kind = resolver.resolve_kind(&f.label, object_classes);
        f.secret = matches!(kind, Some(WidgetKind::Password(_)));
        if f.widget_binding.is_some() {
            continue;
        }
        // Attach config-driven bindings (Password/Choice/Picker/NextNumber/…).
        // objectClass routing stays label-based (2a is_modal_field), so do not set a
        // binding for it here; leave the label-driven path intact.
        if !f.label.eq_ignore_ascii_case("objectClass") {
            f.widget_binding = kind;
        }
    }
}
```

(Match the exact `inject_resolver_kinds` body for the binding-selection details — e.g. whether it skips certain kinds — and replicate them; the reference is `src/ui/edit_form.rs:695-…`.)

- [ ] **Step 4: Wire it.**
  - `UiState`: hold `pub resolved_widgets: Vec<crate::config::widget::ResolvedWidget>` (built in `bootstrap` via `resolve_widgets(&profiles)`; `Vec::new()` in `new_for_test`).
  - `pump_worker` (after `build_edit_form` + `form.object_classes = …`): build a `WidgetResolver::new(self.read_flow.schema(), &self.profiles, &self.resolved_widgets, self.read_only)` and call `apply_widget_bindings(&mut form, &resolver, &object_classes)` before `self.edit_form = Some(form)`. (Watch the borrow: build the resolver from `&self` fields while `form` is a local — disjoint.)
  - `build_create_form`: after `sync_schema_fields`, the caller (`open_create`) applies bindings — OR pass the resolver into `build_create_form`. Simpler: in `open_create`, after building the form, build a `WidgetResolver` and call `apply_widget_bindings(&mut form, &resolver, &form.object_classes.clone())` before installing it.

- [ ] **Step 5: Run — expect PASS:** `cargo test -j4 --lib widget_bind 2>&1 | tail -20`; full suite `cargo test -j4 --lib 2>&1 | tail -5`; `cargo build -j4 --bin edaptor-tv 2>&1 | tail -2`.
- [ ] **Step 6: commit**

```bash
cargo fmt && cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -3
git add src/workflows/widget_bind.rs src/workflows/mod.rs src/workflows/create.rs src/tui/state.rs src/tui/app.rs
git commit -F - <<'MSG'
feat(workflows): apply_widget_bindings (neutral) wired into edit + create

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
MSG
```

---

### Task 15: `PasswordWidget` + `widget_for` routing

**Files:** Modify + Test: `src/tui/widget.rs`; will reference `src/tui/pw_editor.rs` (Task 16) for `PasswordWidget` — so implement `PasswordWidget` in `pw_editor.rs` and route to it here. To keep this task self-contained, define a minimal `PasswordWidget` stub in `pw_editor.rs` first (present + activate), expanded in Task 16.

**Interfaces:**
- Produces: `widget_for` returns `PasswordWidget` when `field.widget_binding == Some(WidgetKind::Password(_))`; `is_modal_field` true for password fields; password `present()` masks.

- [ ] **Step 1: Write failing tests** (widget.rs):

```rust
#[test]
fn password_field_routes_to_password_widget_and_is_modal() {
    use crate::config::widget::{WidgetKind, PasswordWidget as PwCfg};
    let mut f = field(&[], WidgetSpec::ReadOnlyText);
    f.label = "userPassword".into();
    f.widget_binding = Some(WidgetKind::Password(PwCfg { primary: "userPassword".into(), derived: vec![], samba: false }));
    assert!(is_modal_field(&f));
    assert!(matches!(widget_for(&f).activate(&f), Activation::Modal(_)));
}
```

- [ ] **Step 2: Run — expect FAIL.** Run: `cargo test -j4 --lib tui::widget 2>&1 | tail -20`

- [ ] **Step 3: Implement.** In `pw_editor.rs` define `pub(crate) struct PasswordWidget;` implementing `FieldWidget` (`capability()=Static`; `present(f)` → `"‹set›"` if `!f.values.is_empty()` else `"‹unset›"`; `activate(f)` → `Activation::Modal(Box::new(PasswordEditor::for_field(f)))` — `PasswordEditor` is fleshed out in Task 16; a minimal version that builds a refusal/placeholder dialog is fine here). In `widget.rs`:

```rust
pub fn widget_for(field: &EditField) -> Box<dyn FieldWidget> {
    use crate::config::widget::WidgetKind;
    if field.label.eq_ignore_ascii_case("objectClass") {
        Box::new(crate::tui::oc_picker::ObjectClassWidget)
    } else if matches!(field.widget_binding, Some(WidgetKind::Password(_))) {
        Box::new(crate::tui::pw_editor::PasswordWidget)
    } else {
        Box::new(PlainWidget)
    }
}

pub fn is_modal_field(field: &EditField) -> bool {
    use crate::config::widget::WidgetKind;
    field.label.eq_ignore_ascii_case("objectClass")
        || matches!(field.widget_binding, Some(WidgetKind::Password(_)))
}
```

- [ ] **Step 4: Run — expect PASS:** `cargo test -j4 --lib tui::widget 2>&1 | tail -20`
- [ ] **Step 5: commit**

```bash
cargo fmt && cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -3
cargo build -j4 --bin edaptor-tv 2>&1 | tail -2
git add src/tui/widget.rs src/tui/pw_editor.rs src/tui/mod.rs
git commit -F - <<'MSG'
feat(tui): PasswordWidget + widget_for password routing

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
MSG
```

---

### Task 16: `PasswordEditor` dialog (TLS gate + New/Confirm → StageSecret)

**Files:** Modify + Test: `src/tui/pw_editor.rs`

**Interfaces:**
- Consumes: `connection_encrypted` (Task 13), the `FieldEditor` seam (`into_view`), `CommitOutcome::StageSecret`.
- Produces: `PasswordEditor` (a `FieldEditor`): refuses when unencrypted; else a masked New+Confirm dialog that keeps `staged_commit = StageSecret { attrs, cleartext }` live (the 2a model).

- [ ] **Step 1: Write failing tests** (headless, mirror `oc_picker.rs` tests):
  - `refuses_when_unencrypted`: build the editor with `encrypted=false`; `into_view`; the dialog is a refusal (no New/Confirm fields); `staged_commit` stays `None` even after events.
  - `stages_when_match`: `encrypted=true`; type the same value into both fields (drive `handle_event` with char events into the New + Confirm lines); assert `staged_commit == Some(StageSecret { cleartext: "...", .. })`; with a mismatch, `None`.

- [ ] **Step 2: Run — expect FAIL.** Run: `cargo test -j4 --lib pw_editor 2>&1 | tail -20`

- [ ] **Step 3: Implement.** Model on `oc_picker.rs` (Dialog wrapper, `#[delegate(to=dlg)]`, `reset_current`, live staging) and `confirm.rs` (Dialog + button_row). `PasswordEditor` carries `attrs: Vec<String>` (the add-attrs to stage — primary [+ samba handled by fold], from the binding), `encrypted: bool`, and the shared handle. `into_view`:
  - if `!encrypted` → a refusal `Dialog` with the message "Changing a password requires an encrypted connection (ldaps://, ldapi://, or start_tls)." + OK; never stages.
  - else → a Dialog with two masked `InputLine`s ("New:", "Confirm:") + OK/Cancel. `InputLine` masking: set the field's echo/mask mode (check the tvision `InputLine` API for a password/echo-char setter; if none, store the typed text but render it masked — mirror `ui/app/password_editor.rs`'s approach). On each `handle_event`, read both line values; if both non-empty and equal → `shared.borrow_mut().staged_commit = Some(CommitOutcome::StageSecret { attrs: self.attrs.clone(), cleartext })`; else `= None`. (Borrow-safe: short borrow on events; never `borrow_mut` in construction.)
  - The widget's `attrs` come from the field's `WidgetKind::Password(PasswordWidget{ primary, .. })` binding: `attrs = vec![primary]` (the samba secrets are added by `fold_create_password`/the edit fold, not staged here).

- [ ] **Step 4: Run — expect PASS:** `cargo test -j4 --lib pw_editor 2>&1 | tail -20`
- [ ] **Step 5: commit**

```bash
cargo fmt && cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -3
cargo build -j4 --bin edaptor-tv 2>&1 | tail -2
git add src/tui/pw_editor.rs
git commit -F - <<'MSG'
feat(tui/pw_editor): TLS-gated New/Confirm password editor → StageSecret

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
MSG
```

---

### Task 17: stage + fold the password (create + edit)

**Files:** Modify + Test: `src/tui/state.rs` (`apply_commit` StageSecret), `src/tui/app.rs` (`do_create` fold), `src/workflows/save.rs` + `src/workflows/write_flow.rs` (edit prepare folds `password_mods`).

**Interfaces:**
- Consumes: `fold_create_password`, `password_add_attrs`, `prepare_save`'s `password_mods` param, `pending_password`.
- Produces: a staged password is written into the ADD (create) and into the MODIFY (edit).

- [ ] **Step 1: Write failing tests:**
  - state.rs: `apply_commit(idx, StageSecret { attrs: vec!["userPassword".into()], cleartext: "s3cret".into() })` sets `pending_password == Some("s3cret")`, marks the field `secret`/shows `‹set›` (set the field's values to a masked sentinel `["••••••"]`), and `form_needs_render`.
  - save.rs: a unit test that `prepare_save` with a non-empty `password_mods` includes those mods in the resulting `SavePlan::Modify`.

- [ ] **Step 2: Run — expect FAIL.** Run: `cargo test -j4 --lib 'tui::state|save' 2>&1 | tail -20`

- [ ] **Step 3: Implement.**
  - `UiState`: add `pub pending_password: Option<String>` (+ which attrs, e.g. `pending_password_attrs: Vec<String>`) — init `None`/empty everywhere.
  - `apply_commit` `StageSecret { attrs, cleartext }` arm (replace the no-op): `self.pending_password = Some(cleartext); self.pending_password_attrs = attrs;` and set `fields[field_idx].values = vec!["••••••".into()]` (masked sentinel for display + dirty); `form_needs_render = true`.
  - `do_create` (Task 8): before the confirm preview, fold the staged password:
    ```rust
    let pending = st.pending_password.clone();
    // build resolved widgets list (st.resolved_widgets) and now-secs
    let masked_ldif = crate::workflows::create::fold_create_password(&dn, &mut attrs, pending.as_deref(), &st.resolved_widgets, crate::workflows::create::now_unix_secs_or_zero());
    let ldif = masked_ldif.unwrap_or(ldif);
    ```
    (Drop borrows appropriately; `fold_create_password` mutates `attrs` and returns the masked LDIF when it injected secrets.)
  - **Edit fold:** `WriteFlow::prepare` currently passes `password_mods: &[]`. Compute `password_mods` from `pending_password` for edit: port `ui/app/save.rs::stage_pending_password` into a neutral helper (e.g. `workflows::save::password_mods_for(pending, attrs_binding, samba, now) -> Vec<ModOp>`) and have `prepare` pass it. The form's password field is `secret` (so `prepare_save` already excludes its sentinel from the normal diff via `secret_attrs`).

- [ ] **Step 4: Run — expect PASS:** `cargo test -j4 --lib 2>&1 | tail -5`; `cargo build -j4 --bin edaptor-tv 2>&1 | tail -2`.
- [ ] **Step 5: commit**

```bash
cargo fmt && cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -3
git add src/tui/state.rs src/tui/app.rs src/workflows/save.rs src/workflows/write_flow.rs
git commit -F - <<'MSG'
feat: stage + fold the password into create (ADD) and edit (MODIFY)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
MSG
```

---

### Task 18: password live (refusal) + CHANGES + final acceptance

**Files:** Modify: `tests/tv_create.rs`, `CHANGES.md`

- [ ] **Step 1.** Add a gated test asserting the password gate: against the plain `ldap://` demo (`connection_encrypted == false`), `PasswordEditor` built with `encrypted=false` produces a refusal and stages nothing. (This is a headless assertion on the editor — it does not need the live server, but place it where the password behavior is documented; the live server confirms `is_encrypted()==false` for `ldap://`.)
- [ ] **Step 2.** CHANGES.md: "Passwords can be set via a masked New/Confirm editor (create and edit), writing the configured attribute (+ Samba NT hash when enabled); the editor refuses on an unencrypted connection."
- [ ] **Step 3.** Facade guards + full check:
```bash
! grep -rl "use tvision_rs" src | grep -vE "^src/(tui/|bin/edaptor-tv.rs)"
! grep -rl "use ratatui\|use tui_" src | grep -vE "^src/ui/"
make check
```
Expected: guards print nothing; `make check` passes.
- [ ] **Step 4. Live tmux acceptance (agent-driven PTY).** Follow the handover recipe (focus probes: `tmux display-message -p '#{cursor_x}'`; `tmux capture-pane -e` green focus). Verify: Alt+N from `ou=people` → (chooser if >1) → create form with objectClass pre-injected + uidNumber auto-allocating (`‹allocating…›` → a number) → type `uid` → header DN updates live → Alt-S → confirm LDIF → the new entry appears in the leaf list and loads in edit mode. Focus the password field → editor refuses on the plain demo. **Then delete the created test entry** (via `ldapdelete` or a disposable RDN) so the demo seed is restored. Kill the tmux session.
- [ ] **Step 5. commit**

```bash
git add tests/tv_create.rs CHANGES.md
git commit -F - <<'MSG'
test(tv): password-gate test + CHANGES; Phase 2b acceptance (Block C)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
MSG
```

---

## Self-review

**Spec coverage:**
- §1 entry point (branch + fast-path/chooser/0-match) → Tasks 6, 7. ✓
- §2 create-mode form (FormMode::Create, objectClass auto-inject + editable, live DN, defaults) → Tasks 1, 2, 3. ✓
- §3 autonumber (async AllocFlow, ‹allocating…›) → Tasks 10, 11, 12. ✓
- §4 password widget (general, TLS-gated, StageSecret, create+edit fold) → Tasks 13–17. ✓
- §5 submit ADD + navigate → Tasks 4, 5, 8. ✓
- Testing (neutral/widget/async/live + tmux acceptance) → distributed; final acceptance Task 18. ✓
- Acceptance criteria 1–5 → Tasks 8/5 (create+navigate), 2 (objectClass), 11 (autonumber), 16/17 (password), 18 (make check + clean demo). ✓

**Placeholder scan:** No "TBD"/"add error handling". A few tvision-heavy tasks (3, 6, 11, 16) give structure + the exact pattern file to mirror (`oc_picker.rs`, `confirm.rs`, `leaf.rs`) plus the concrete code for the non-tvision logic — the same style as the executed 2a plan, not a placeholder. Two tasks explicitly say "match the existing tests/`inject_resolver_kinds` body" with the file:line reference — that is a DRY/port instruction, not a gap.

**Type consistency:** `FormMode::Create { profile_idx: usize, container: String }`, `composed_create_dn(&str,&str,&str)->String`, `build_create_form(&SchemaModel,&EntryProfile,usize,&str)->(EditForm,Vec<(String,u64,u64)>)`, `WriteFlow::submit_create(&WorkerHandle,&str,BTreeMap<String,Vec<String>>,bool)`, `WriteOutcome::Created{dn,quit_after}`, `AllocFlow::{request,on_response}`/`AllocOutcome::{Filled{attr,value},Failed,Ignored}`, `apply_widget_bindings(&mut EditForm,&WidgetResolver,&[String])`, `widget_for`/`is_modal_field` password routing, `apply_commit` StageSecret, `connection_encrypted` — names/types consistent across tasks.

**Known execution-time checks (flagged, not gaps):**
- `Response::SearchError` field names (Task 10) — confirm against `worker.rs` and adjust.
- `InputLine` password-masking API (Task 16) — confirm the echo/mask setter exists; if not, mirror `ui/app/password_editor.rs`'s store-but-render-masked approach.
- Non-exhaustive `match form.mode` after adding `Create` (Task 1) — the build surfaces every site; add fall-through arms.
- The exact `inject_resolver_kinds` binding-selection body (Task 14) — port verbatim from `src/ui/edit_form.rs:695`.
