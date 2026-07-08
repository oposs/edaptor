# tvision M4 — Rich widgets Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the remaining rich field widgets in the tvision UI — free-text multi-value editor, choice, picker, two-column membership, sambaSID immediate auto-gen — plus samba-context wiring and the X-ORDERED `ordered` side-effect, reaching functional parity with the ratatui UI.

**Architecture:** Each widget plugs into the existing M3 seam (`widget_for` routing → `Activation::Modal(Box<dyn FieldEditor>)` → editor stages a typed `CommitOutcome` into `UiState.staged_commit` → `app::dispatch` ACTIVATE applies it via `apply_commit` on the modal's `OK`). Config + encode/diff logic already exist as neutral domain code (`config::widget`, `workflows::save`); M4 is overwhelmingly tvision-side UI plus one new async `SearchFlow` (mirroring `AllocFlow`) and porting `plan_combined_save` to neutral `workflows`. Build order is static-first; the multi-entry membership write lands last on proven infrastructure.

**Tech Stack:** Rust 2021, `tvision-rs = "0.3"` (alias `tv`), ratatui (untouched, separate binary), podman OpenLDAP demo server.

## Global Constraints

- **Cap build/test parallelism at 4 cores** (shared box): `cargo build -j4`, `cargo test -j4`, `cargo clippy -j4 --all-targets -- -D warnings`. Target dir is `/home/oetiker/scratch/cargo-target` (binary at `…/debug/edaptor-tv`, NOT `./target`).
- **No form-core changes.** Widgets register via `widget_for` / `is_modal_field`; the only seam change permitted is **adding** `Activation::Immediate(CommitOutcome)` and its dispatch arm (additive).
- **Facade boundary (CI guard, must print nothing):** only `src/tui/**` + `src/bin/edaptor-tv.rs` may `use tvision_rs`; only `src/ui/**` may `use ratatui`/`use tui_*`. New neutral code (`workflows::search_flow`, `workflows::pick_state`, `plan_combined_save`) imports neither UI framework.
- **Don't edit `src/ui/**` (ratatui).** Neutral logic is a fresh parity copy in `workflows::*`; dedup deferred to M5.
- **Borrow discipline:** never hold a `UiState`/`RefCell` borrow across `broadcast`/`post`/`exec_view`/`worker.submit`/`new_list`/`child_mut`/`set_value`. Collect into locals → drop borrow → call. In modal views, stage state in `reset_current` / on events — **never** `borrow_mut()` in `new()`/`into_view` (`dispatch` holds `state.borrow()` to pass the schema in).
- **Strict TDD**, atomic commits, crate compiles after every commit, `cargo fmt` before each commit, clippy `--all-targets -D warnings` clean.
- **Live tests gated** by `EDAPTOR_TEST_LDAP_URI` (skip when unset). Base `dc=example,dc=org`, `EDAPTOR_TEST_ADMIN_PW=adminpassword`, `scripts/test-ldap.sh start`.
- **Commit trailer:** `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. Use `git commit -F <file>` for messages containing backticks.
- **Docs:** config detail → mdBook (`docs/src/configuration/widgets.md`); `CHANGES.md` entry for every user-visible change.

## Reference (exact current signatures — do not re-derive)

```rust
// src/tui/widget.rs
pub enum CommitOutcome { SetValues(Vec<String>), StageSecret { attrs: Vec<String>, cleartext: String },
                         SetValuesThenResyncSchema(Vec<String>), Cancelled }
pub enum Activation { Inline, Modal(Box<dyn FieldEditor>) }   // M4 adds: Immediate(CommitOutcome)
pub enum Capability { Static, NeedsSchema, NeedsWorkerSearch }
pub trait FieldEditor { fn into_view(self: Box<Self>, schema: &SchemaModel, shared: Shared) -> (Box<dyn View>, tv::ViewId); }
pub trait FieldWidget { fn capability(&self) -> Capability; fn present(&self, field: &EditField) -> String;
                        fn activate(&self, field: &EditField) -> Activation; }
pub fn widget_for(field: &EditField) -> Box<dyn FieldWidget>;
pub fn is_modal_field(field: &EditField) -> bool;
pub fn inline_editable(field: &EditField) -> bool;  // field.editable && !field.multi && !field.orphaned && field.widget_binding.is_none()

// src/workflows/edit_form.rs
pub struct EditField { pub label: String, pub must: bool, pub editable: bool, pub multi: bool,
  pub secret: bool, pub ordered: bool, pub orphaned: bool, pub kind: FieldKind, pub widget: WidgetSpec,
  pub widget_binding: Option<WidgetKind>, pub values: Vec<String>, pub baseline: Vec<String> }
impl EditField { pub fn current_values(&self) -> Vec<String>; pub fn injected(label, must, schema) -> EditField; }
impl EditForm { pub fn set_value(&mut self, idx, text); pub fn is_dirty(&self) -> bool;
  pub fn to_edit_entry(&self) -> EditEntry; pub fn sync_schema_fields(&mut self, schema: &SchemaModel); }
pub fn value_set_eq(a: &[String], b: &[String]) -> bool;

// src/config/widget.rs
pub enum WidgetKind { Choice(ChoiceWidget), Password(PasswordWidget), Picker(PickerBinding),
                      ObjectClassPicker, SambaSid, NextNumber{min,max}, Readonly, XOrdered }
impl ChoiceWidget { pub fn seed_checked(&self, current: &str) -> Vec<String>;
  pub fn commit_value(&self, current: &str, checked: &[String]) -> String;
  pub fn present_summary(&self, current: &str) -> String;  pub select: Cardinality; pub options: Vec<ChoiceOption>; }
// src/config/relation.rs
pub struct PickerBinding { pub attr: String, pub scope: CandidateScope, pub store: StoreKey,
                           pub select: Option<Cardinality>, pub fanout_attr: Option<String> }
pub enum StoreKey { Dn, Attr(String) }    pub enum Cardinality { Single, Multi }

// src/ldap/worker.rs
Request::Search { id, base, scope: SearchScope, filter, attrs, size_limit: Option<i32> }
Request::Modify { id, dn, changes: Vec<ModOp> }
Response::Entries { id, entries: Vec<LdapEntry>, truncated } | Response::SearchError { id, msg }
                  | Response::WriteOk { id, dn } | Response::WriteError { id, msg }
WorkerHandle::submit(&self, Request) -> Result<()>;   WorkerHandle::poll(&self) -> Option<Response>;

// src/workflows/save.rs
pub fn membership_fanout(entry_dn, baseline: &[String], selected: &[String], holder_attr) -> Vec<(String, ModOp)>;
pub fn would_empty(current_members: &[String], member: &str) -> bool;
pub fn prepare_save(schema, original: &EditEntry, edited: &EditEntry, object_classes, password_mods,
                    mask_attrs, secret_attrs, orphaned_attrs, x_ordered_attrs) -> PrepareSave;
pub enum PrepareSave { Invalid(..), DiffError(..), NoChanges, Ready { plan: SavePlan, dn, ldif } }

// src/workflows/write_flow.rs
impl WriteFlow { pub fn submit(&mut self, worker, plan: SavePlan, old_dn, quit_after) -> Result<()>; ... }
pub enum WriteOutcome { Ignored, Saved{reread_dn,quit_after}, NeedFollowupModify{..}, Error(String), Created{..} }
pub const STAGED_PASSWORD_SENTINEL: &str = "••••••";

// src/workflows/alloc_flow.rs  (template for SearchFlow)  id range 2_000_000 ; ReadFlow=1, WriteFlow=1_000_000
// src/samba/sid.rs
pub fn generate_user_sid(domain: Option<&SambaDomainInfo>, uid_value: Option<&str>) -> Result<String, String>;
// src/samba/mod.rs
pub struct SambaDomainInfo { pub domain_sid: String, pub algorithmic_rid_base: u32 }
// src/config/mod.rs  -> Config has `samba: SambaConfig { domain_sid: Option<String>, algorithmic_rid_base: u32 }`
```

**Live-driving the TUI (tmux PTY)** — see `docs/HANDOVER.md` "Live-driving the TUI"; build first, `-x 210 -y 50`, insert `sleep` between keys.

---

# PART 1 — Free-text multi-value editor + X-ORDERED

Unblocks editing all multi-valued attributes (today only single-valued attrs are editable). Build the editor for plain multi-value fields, then wire the `ordered` flag so X-ORDERED attrs get order-sensitive dirty detection.

### Task 1: X-ORDERED sets `field.ordered`

**Files:**
- Modify: `src/workflows/widget_bind.rs:19-41` (`apply_widget_bindings`)
- Test: `src/workflows/widget_bind.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `EditField.{widget_binding, ordered}`, `WidgetKind::XOrdered`.
- Produces: after `apply_widget_bindings`, a field bound to `XOrdered` has `ordered == true`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn xordered_binding_sets_ordered_flag() {
    use crate::config::widget::WidgetKind;
    let mut form = test_form_with_field("memberUid"); // helper already in this module's tests; else build an EditForm with one field
    // Force the resolver to bind memberUid -> XOrdered (use the existing test resolver builder in this module).
    let resolver = resolver_binding("memberUid", WidgetKind::XOrdered);
    apply_widget_bindings(&mut form, &resolver, &["posixGroup".to_string()]);
    let f = form.fields.iter().find(|f| f.label == "memberUid").unwrap();
    assert!(matches!(f.widget_binding, Some(WidgetKind::XOrdered)));
    assert!(f.ordered, "XOrdered binding must set field.ordered = true");
}
```

(If `test_form_with_field`/`resolver_binding` helpers don't exist, build the `EditForm`/resolver inline using the module's existing test scaffolding — check the current `mod tests` and mirror it.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -j4 -p edaptor xordered_binding_sets_ordered_flag`
Expected: FAIL (`f.ordered` is false).

- [ ] **Step 3: Implement**

In `apply_widget_bindings`, after the binding is attached, set `ordered` for X-ORDERED:

```rust
        if !f.label.eq_ignore_ascii_case("objectClass") {
            f.widget_binding = kind;
        }
        // X-ORDERED attrs are order-sensitive: drive the dirty check + editor.
        if matches!(f.widget_binding, Some(WidgetKind::XOrdered)) {
            f.ordered = true;
        }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -j4 -p edaptor xordered_binding_sets_ordered_flag`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/workflows/widget_bind.rs
git commit -F - <<'EOF'
feat(widget-bind): X-ORDERED binding sets field.ordered for order-sensitive dirty

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
EOF
```

### Task 2: `MultiValueWidget` present + activate + routing

**Files:**
- Create: `src/tui/widget/multivalue.rs`
- Modify: `src/tui/widget.rs` (`widget_for`, `is_modal_field`, declare `mod widget { ... }` submodule or add `pub(crate) mod multivalue;` — match how `oc_picker`/`pw_editor` are declared in `src/tui/mod.rs`)
- Test: `src/tui/widget/multivalue.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `pub(crate) struct MultiValueWidget;` impl `FieldWidget` (`Capability::Static`, `present` = bullet/join summary, `activate` = `Modal(MultiValueEditor)`). `widget_for` routes plain editable multi-value fields (no binding, not objectClass/password) to it; `is_modal_field` returns true for them.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflows::edit_form::EditField;

    fn multi_field(label: &str, vals: &[&str]) -> EditField {
        let mut f = EditField::injected(label.to_string(), false, &crate::schema::SchemaModel::empty_for_test());
        f.multi = true; f.editable = true;
        f.values = vals.iter().map(|s| s.to_string()).collect();
        f
    }

    #[test]
    fn present_lists_values_joined() {
        let w = MultiValueWidget;
        let f = multi_field("mail", &["a@x", "b@x"]);
        assert_eq!(w.present(&f), "a@x, b@x");
    }

    #[test]
    fn present_empty_is_dash() {
        let w = MultiValueWidget;
        let f = multi_field("mail", &[]);
        assert_eq!(w.present(&f), "—");
    }
}
```

(If `SchemaModel::empty_for_test` doesn't exist, use the test schema constructor the other `tui` tests use — grep `tui` tests for the pattern.)

- [ ] **Step 2: Run to verify fail** — `cargo test -j4 -p edaptor multivalue::tests`
Expected: FAIL (module/type missing).

- [ ] **Step 3: Implement the widget**

```rust
//! Free-text multi-value editor: add / edit / delete / reorder rows.
use crate::tui::widget::{Activation, Capability, FieldWidget};
use crate::workflows::edit_form::EditField;

pub(crate) struct MultiValueWidget;

impl FieldWidget for MultiValueWidget {
    fn capability(&self) -> Capability { Capability::Static }
    fn present(&self, field: &EditField) -> String {
        if field.values.iter().all(|v| v.trim().is_empty()) {
            "—".to_string()
        } else {
            field.values.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
        }
    }
    fn activate(&self, field: &EditField) -> Activation {
        Activation::Modal(Box::new(super::super::multivalue_dialog::MultiValueEditor {
            label: field.label.clone(),
            values: field.values.clone(),
            ordered: field.ordered,
        }))
    }
}
```

(Adjust the `super::` path to wherever you place `MultiValueEditor` — Task 3. If you keep widget + editor in one file, reference it directly.)

- [ ] **Step 4: Route in `widget_for` + `is_modal_field`** (`src/tui/widget.rs`)

```rust
pub fn widget_for(field: &EditField) -> Box<dyn FieldWidget> {
    use crate::config::widget::WidgetKind;
    if field.label.eq_ignore_ascii_case("objectClass") {
        Box::new(crate::tui::oc_picker::ObjectClassWidget)
    } else if matches!(field.widget_binding, Some(WidgetKind::Password(_))) {
        Box::new(crate::tui::pw_editor::PasswordWidget)
    } else if field.editable && field.multi && !field.orphaned && field.widget_binding.is_none() {
        Box::new(crate::tui::widget::multivalue::MultiValueWidget)
    } else {
        Box::new(PlainWidget)
    }
}

pub fn is_modal_field(field: &EditField) -> bool {
    use crate::config::widget::WidgetKind;
    field.label.eq_ignore_ascii_case("objectClass")
        || matches!(field.widget_binding, Some(WidgetKind::Password(_)))
        || (field.editable && field.multi && !field.orphaned && field.widget_binding.is_none())
}
```

- [ ] **Step 5: Run to verify pass** — `cargo test -j4 -p edaptor multivalue::tests` → PASS. Then `cargo build -j4 --bin edaptor-tv` (compiles even though Task 3's dialog is a stub — temporarily stub `MultiValueEditor` to make this task compile in isolation, or fold Task 3 in before this step). 

  **Note:** to keep the crate compiling after each commit, implement Task 3's `MultiValueEditor` skeleton (struct + `FieldEditor::into_view` returning a minimal dialog) before committing Task 2. Commit Tasks 2+3 together if needed for compilation.

- [ ] **Step 6: Commit** (`feat(tui): MultiValueWidget present + routing for plain multi-value fields`).

### Task 3: `MultiValueEditor` dialog (ListBox + InputLine + add/del/reorder)

**Files:**
- Create/extend: `src/tui/multivalue_dialog.rs` (or the same file as Task 2)
- Modify: `src/tui/mod.rs` (declare the module)
- Test: same file (`#[cfg(test)] mod tests` — headless `Context`)

**Interfaces:**
- Consumes: `Shared` (`Rc<RefCell<UiState>>`), `CommitOutcome::SetValues`.
- Produces: `pub(crate) struct MultiValueEditor { pub label, pub values: Vec<String>, pub ordered: bool }` impl `FieldEditor`. The dialog stages `CommitOutcome::SetValues(rows trimmed, empties dropped)` into `shared.staged_commit` live (in `reset_current` and after every mutating key), so the OK path applies it.

Follow the `oc_picker.rs` modal pattern exactly: a `Dialog` owning a `ListBox`, seed in `reset_current`, `update_staged()` after each change, OK button (`Command::OK`) so `exec_view_focused` returns OK. Keys: Up/Down navigate; an `InputLine` edits the selected row; **Alt+a/Insert** add a row at `sel+1`; **Alt+d/Delete** remove `sel`; **Alt+Up/Alt+Down** swap rows (bounded); OK commits. Mirror the borrow discipline (no `borrow_mut` in `into_view`).

- [ ] **Step 1: Write the failing headless test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::test_support::*; // headless Context helpers used by other tui tests; else inline per HANDOVER recipe

    #[test]
    fn ok_stages_trimmed_nonempty_values() {
        let shared = test_shared();           // Rc<RefCell<UiState>> seeded minimally
        let ed = Box::new(MultiValueEditor { label: "mail".into(),
            values: vec!["a@x".into(), "  ".into(), "b@x".into()], ordered: false });
        let (mut view, _focus) = ed.into_view(&schema_for_test(), shared.clone());
        let mut ctx = headless_ctx();
        view.reset_current(&mut ctx);          // seeds + stages
        let staged = shared.borrow().staged_commit.clone();
        assert_eq!(staged, Some(crate::tui::widget::CommitOutcome::SetValues(
            vec!["a@x".into(), "b@x".into()])));
    }
}
```

(Use the same headless-`Context` construction the existing `tui` widget tests use — grep for `Context::new` / `Buffer::new` in `src/tui` tests and reuse that scaffolding; the HANDOVER "Headless view tests" note documents it.)

- [ ] **Step 2: Run to verify fail** — `cargo test -j4 -p edaptor multivalue_dialog` → FAIL.

- [ ] **Step 3: Implement the dialog** (full struct + `FieldEditor::into_view` + `View` impl with the key handling above + `update_staged()` that trims rows and drops empties → `SetValues`). Model it line-for-line on `src/tui/oc_picker.rs` (ListBox build, `refresh_list`, `reset_current`, `update_staged`).

- [ ] **Step 4: Run to verify pass** — `cargo test -j4 -p edaptor multivalue_dialog` → PASS.

- [ ] **Step 5: Add reorder + delete tests** (Alt+Down swaps; Alt+d removes; delete-all then navigate doesn't panic — parity with ratatui `value_editor` tests). Run → PASS.

- [ ] **Step 6: Commit** (`feat(tui): MultiValueEditor dialog — add/edit/delete/reorder, stages SetValues`).

### Task 4: Live acceptance — edit a multi-value attribute

- [ ] **Step 1:** `cargo build -j4 --bin edaptor-tv`; `scripts/test-ldap.sh start`.
- [ ] **Step 2:** Drive via tmux (HANDOVER recipe): navigate to a user, focus `mail` (a multi-valued attr — now focusable), Enter opens the editor, add a value, OK, verify the form cell updates and dirty `*` shows. **Discard** (don't persist to demo data).
- [ ] **Step 3:** Capture-pane confirms the editor renders rows + the cell updates. Kill the tmux session.
- [ ] **Step 4:** `CHANGES.md` entry under the unreleased tvision section: "tvision UI: editable free-text multi-value fields (add/edit/delete/reorder); X-ORDERED attrs are order-sensitive." Commit.

---

# PART 2 — Choice widget

Radio (single) / checkbox (multi) over the already-neutral `config::widget::ChoiceWidget` (lossless seed/commit). Covers Plain (`loginShell`) and Bracketed (Samba `[DU         ]`).

### Task 5: `ChoiceWidget` (tui) present + activate + routing

**Files:**
- Create: `src/tui/widget/choice.rs`
- Modify: `src/tui/widget.rs` (`widget_for`, `is_modal_field`), `src/tui/mod.rs`
- Test: `src/tui/widget/choice.rs`

**Interfaces:**
- Produces: `pub(crate) struct ChoiceWidget;` impl `FieldWidget`. `present` delegates to the config `present_summary` of the bound `WidgetKind::Choice(cfg)`. `activate` → `Modal(ChoiceEditor)`. Routing: `matches!(field.widget_binding, Some(WidgetKind::Choice(_)))`.

- [ ] **Step 1: Failing test**

```rust
#[test]
fn present_uses_config_summary() {
    use crate::config::widget::{WidgetKind, ChoiceWidget as Cfg, ChoiceOption, Cardinality, ChoiceFormat};
    let cfg = Cfg { select: Cardinality::Single, format: ChoiceFormat::Plain,
        options: vec![ChoiceOption { value: "/bin/bash".into(), label: "Bash".into() }] };
    let mut f = single_field("loginShell", "/bin/bash");
    f.widget_binding = Some(WidgetKind::Choice(cfg));
    assert_eq!(ChoiceWidget.present(&f), "Bash");
}
```

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement**

```rust
use crate::config::widget::WidgetKind;
use crate::tui::widget::{Activation, Capability, FieldWidget};
use crate::workflows::edit_form::EditField;

pub(crate) struct ChoiceWidget;
impl FieldWidget for ChoiceWidget {
    fn capability(&self) -> Capability { Capability::Static }
    fn present(&self, field: &EditField) -> String {
        match &field.widget_binding {
            Some(WidgetKind::Choice(cfg)) =>
                cfg.present_summary(field.values.first().map(|s| s.as_str()).unwrap_or("")),
            _ => crate::tui::widget::present_field(field),
        }
    }
    fn activate(&self, field: &EditField) -> Activation {
        match &field.widget_binding {
            Some(WidgetKind::Choice(cfg)) => Activation::Modal(Box::new(
                crate::tui::choice_dialog::ChoiceEditor {
                    label: field.label.clone(),
                    cfg: cfg.clone(),
                    current: field.values.first().cloned().unwrap_or_default(),
                })),
            _ => Activation::Inline,
        }
    }
}
```

- [ ] **Step 4: Route** in `widget_for` (before the multi-value arm) + `is_modal_field`: add `|| matches!(field.widget_binding, Some(WidgetKind::Choice(_)))`.

- [ ] **Step 5: Run → PASS** (commit with Task 6 for compilation, as in Part 1).

### Task 6: `ChoiceEditor` dialog (radio / checkbox over ListBox)

**Files:** Create `src/tui/choice_dialog.rs`; modify `src/tui/mod.rs`; test same file.

**Interfaces:**
- Produces: `pub(crate) struct ChoiceEditor { pub label, pub cfg: config::widget::ChoiceWidget, pub current: String }` impl `FieldEditor`. Seeds checked rows from `cfg.seed_checked(&current)`. Single → radio `(•)/( )` (Space replaces selection); Multi → checkbox `[x]/[ ]` (Space toggles). `update_staged()` → `CommitOutcome::SetValues(vec![cfg.commit_value(&current, &checked)])` (single-valued field; one assembled string).

- [ ] **Step 1: Failing test** — checkbox toggle then OK stages the lossless-merged value:

```rust
#[test]
fn bracketed_multi_merge_is_lossless() {
    use crate::config::widget::{ChoiceWidget as Cfg, ChoiceOption, Cardinality, ChoiceFormat};
    let cfg = Cfg { select: Cardinality::Multi, format: ChoiceFormat::Bracketed, options: vec![
        ChoiceOption { value: "D".into(), label: "Disabled".into() },
        ChoiceOption { value: "U".into(), label: "User".into() } ] };
    let shared = test_shared();
    let ed = Box::new(ChoiceEditor { label: "sambaAcctFlags".into(), cfg, current: "[U          ]".into() });
    let (mut v, _) = ed.into_view(&schema_for_test(), shared.clone());
    let mut ctx = headless_ctx();
    v.reset_current(&mut ctx);
    // toggle "D" on (drive a Space on the D row — set sel then send KeyDown Space; mirror oc_picker test)
    toggle_row(&mut v, &mut ctx, "D");
    let staged = shared.borrow().staged_commit.clone();
    assert_eq!(staged, Some(crate::tui::widget::CommitOutcome::SetValues(vec!["[DU         ]".into()])));
}
```

- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement** the dialog modeled on `oc_picker.rs` (ListBox with `(•)/( )` or `[x]/[ ]` prefixes per `cfg.select`; Space toggles/replaces; `update_staged` calls `cfg.commit_value`).
- [ ] **Step 4: Run → PASS.** Add a single-radio test (`loginShell` Plain: Space replaces → `SetValues(vec!["/bin/zsh"])`).
- [ ] **Step 5: Commit** (`feat(tui): ChoiceEditor — radio/checkbox over config ChoiceWidget, lossless commit`). `cargo clippy -j4 --all-targets -- -D warnings` clean.

### Task 7: Live acceptance — choice

- [ ] Drive tmux: a Samba user's `sambaAcctFlags` → Enter → toggle a flag → OK → cell shows merged labels + dirty. Discard. `CHANGES.md`: "tvision UI: choice widget (radio/checkbox, lossless encode incl. Samba flags)." Commit.

---

# PART 3 — sambaSID immediate + samba context

`Activation::Immediate` is added to the seam; sambaSID is computed from the sibling `uidNumber` + `UiState` samba domain via a neutral helper, special-cased in dispatch (it needs cross-field + ctx the widget trait can't supply).

### Task 8: Samba domain context in `UiState` + wire `samba_enabled`

**Files:**
- Modify: `src/tui/state.rs` (add `pub samba_domain: Option<SambaDomainInfo>` to `UiState`; set it where `UiState` is constructed from `Config` — grep `UiState {` constructor in `src/tui/mod.rs`/`app.rs`); both `WidgetResolver::new(..., samba_enabled)` sites (`state.rs:~186`, `app.rs:~298`) pass `self.samba_domain.is_some()` instead of `false`.
- Test: `src/tui/state.rs`

**Interfaces:**
- Produces: `UiState.samba_domain: Option<crate::samba::SambaDomainInfo>`; `samba_enabled` = `samba_domain.is_some()` at both resolver sites.

- [ ] **Step 1: Failing test** — construct a `UiState` with a `Config` carrying `[samba] domain_sid = "S-1-5-21-1-2-3"` and assert `samba_domain` is `Some` with that SID. (Use the existing `UiState` test constructor; grep `fn new` / test builders in `src/tui`.)
- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement** — thread `SambaDomainInfo { domain_sid, algorithmic_rid_base }` from `Config.samba` (only when `domain_sid.is_some()`) into `UiState`; flip both `samba_enabled` args.
- [ ] **Step 4: Run → PASS;** `cargo build -j4 --bin edaptor-tv`.
- [ ] **Step 5: Commit** (`feat(tui/state): samba domain context in UiState; wire samba_enabled`).

### Task 9: `Activation::Immediate` + neutral SID compute + dispatch arm

**Files:**
- Modify: `src/tui/widget.rs` (add `Immediate(CommitOutcome)` to `Activation`)
- Create: `src/workflows/samba_compute.rs` (neutral helper) — declared in `src/workflows/mod.rs`
- Modify: `src/tui/app.rs` (ACTIVATE dispatch: handle sambaSID before building a modal)
- Modify: `src/tui/widget.rs` (`is_modal_field`/focusability so a `SambaSid`-bound field is activatable)
- Test: `src/workflows/samba_compute.rs`; dispatch path covered by a `state` test

**Interfaces:**
- Produces: `pub fn samba_sid_for_form(form: &EditForm, domain: Option<&SambaDomainInfo>) -> Result<String, String>` — finds the `uidNumber` field value, delegates to `samba::sid::generate_user_sid`. Dispatch: when the active field's `widget_binding == Some(WidgetKind::SambaSid)`, compute and either `apply_commit(idx, SetValues(vec![sid]))` or open the existing error dialog; do NOT open a modal.

- [ ] **Step 1: Failing test** (neutral helper):

```rust
#[test]
fn computes_sid_from_sibling_uidnumber() {
    let form = form_with(&[("uidNumber", &["1000"]), ("sambaSID", &[])]);
    let dom = SambaDomainInfo { domain_sid: "S-1-5-21-1-2-3".into(), algorithmic_rid_base: 1000 };
    assert_eq!(samba_sid_for_form(&form, Some(&dom)).unwrap(), "S-1-5-21-1-2-3-3000");
}
#[test]
fn errors_without_uidnumber() {
    let form = form_with(&[("sambaSID", &[])]);
    let dom = SambaDomainInfo { domain_sid: "S-1-5-21-1-2-3".into(), algorithmic_rid_base: 1000 };
    assert!(samba_sid_for_form(&form, Some(&dom)).is_err());
}
```

- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement** the helper:

```rust
//! Neutral sambaSID computation from an edit form's uidNumber.
use crate::samba::SambaDomainInfo;
use crate::workflows::edit_form::EditForm;

pub fn samba_sid_for_form(form: &EditForm, domain: Option<&SambaDomainInfo>) -> Result<String, String> {
    let uid = form.fields.iter()
        .find(|f| f.label.eq_ignore_ascii_case("uidNumber"))
        .and_then(|f| f.values.first().map(|s| s.as_str()));
    crate::samba::sid::generate_user_sid(domain, uid)
}
```

- [ ] **Step 4: Add `Activation::Immediate(CommitOutcome)`** and the dispatch arm in `app.rs` (before the modal build):

```rust
} else if cmd == ACTIVATE {
    let idx = state.borrow_mut().activate_field.take();
    let Some(idx) = idx else { return; };
    // sambaSID: immediate compute, no modal.
    let is_sid = {
        let st = state.borrow();
        st.edit_form.as_ref().and_then(|f| f.fields.get(idx))
          .map(|f| matches!(f.widget_binding, Some(crate::config::widget::WidgetKind::SambaSid)))
          .unwrap_or(false)
    };
    if is_sid {
        let res = {
            let st = state.borrow();
            crate::workflows::samba_compute::samba_sid_for_form(
                st.edit_form.as_ref().unwrap(), st.samba_domain.as_ref())
        };
        match res {
            Ok(sid) => state.borrow_mut().apply_commit(idx,
                crate::tui::widget::CommitOutcome::SetValues(vec![sid])),
            Err(msg) => { let (v, f) = crate::tui::dialog::error::build(&msg); prog.exec_view_focused(v, f); }
        }
        return;
    }
    // ... existing Modal build path unchanged ...
}
```

(Confirm `dialog::error::build` signature — grep `src/tui/dialog/error.rs`; adjust if it returns just a view.)

- [ ] **Step 5: Route + focusability** — `widget_for` returns a `SambaSidWidget` (a tiny `FieldWidget` whose `present` shows the current value or `‹unset›`); add `SambaSid` to `is_modal_field` so the field is focusable + edit-key-swallowing.
- [ ] **Step 6: Run → PASS;** `cargo build -j4 --bin edaptor-tv`; clippy clean.
- [ ] **Step 7: Live acceptance** — Samba user create form: focus `sambaSID` (empty) after `uidNumber` is set → Enter → SID fills. Empty uidNumber → error dialog. Discard. `CHANGES.md`: "tvision UI: sambaSID immediate auto-generate from uidNumber + domain SID." Commit.

---

# PART 4 — SearchFlow + picker

The shared async LDAP search (mirrors `AllocFlow`), the neutral selection state, and the picker dialog.

### Task 10: `workflows::pick_state` (neutral selection state)

**Files:** Create `src/workflows/pick_state.rs`; declare in `src/workflows/mod.rs`; test same file. **Fresh parity copy** of the pure logic in `src/ui/picker.rs` (do NOT touch the ratatui file).

**Interfaces:**
- Produces: a `PickState` holding `selected: Vec<String>`, `saved: Vec<String>`, `results: Vec<Candidate>`, with `visible_rows(searching: bool) -> Vec<Row>` (selected-first when not searching; matches-first when searching; selected-but-unmatched still visible; saved-but-removed synthesized at end). Key compare: DN → case-insensitive; scalar → exact. `toggle(key)`, `is_selected(key)`, `commit() -> Vec<String>`.

- [ ] **Step 1:** Read `src/ui/picker.rs`, copy the pure types/algorithms into `pick_state.rs` (rename module path; strip any ratatui imports — there should be none, it's pure). Port its unit tests too.
- [ ] **Step 2: Failing test** — selected-first ordering + dn-case-insensitive toggle (copy 2-3 representative ratatui picker tests).
- [ ] **Step 3: Implement / paste** the ported code until tests pass.
- [ ] **Step 4: Run → PASS;** facade guard clean (no `use ratatui`).
- [ ] **Step 5: Commit** (`feat(workflows): pick_state — neutral selection state (parity copy of ui::picker)`).

### Task 11: `workflows::search_flow::SearchFlow`

**Files:** Create `src/workflows/search_flow.rs`; declare in `mod.rs`; test same file. Mirror `alloc_flow.rs` exactly.

**Interfaces:**
- Produces:
```rust
pub struct SearchFlow { next_id: u64, latest: Option<u64> }  // id range 3_000_000+
impl SearchFlow {
    pub fn new() -> Self;                       // next_id = 3_000_000
    pub fn request(&mut self, worker: &WorkerHandle, base: &str, oc: &str, term: &str,
                   attrs: &[String]) -> Result<u64>;   // builds filter, submits Search, records latest id
    pub fn on_response(&mut self, resp: &Response) -> SearchOutcome;  // Ignored unless id == latest
}
pub enum SearchOutcome { Results { rows: Vec<SearchRow>, truncated: bool }, Failed(String), Ignored }
pub fn build_search_filter(oc: &str, term: &str) -> String;   // RFC-4515 escaped
pub const PICKER_SEARCH_CAP: i32 = 100;
```

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn filter_empty_term_is_objectclass_only() {
    assert_eq!(build_search_filter("posixGroup", ""), "(objectClass=posixGroup)");
}
#[test]
fn filter_escapes_and_substrings() {
    assert_eq!(build_search_filter("posixGroup", "a*b"),
        "(&(objectClass=posixGroup)(|(cn=*a\\2ab*)(uid=*a\\2ab*)))");
}
#[test]
fn stale_response_is_ignored() {
    let mut sf = SearchFlow::new();
    // simulate: latest id advanced past an old id
    let old = 3_000_000;
    sf.force_latest(3_000_001);   // test-only setter, or call request twice with a stub worker
    let resp = Response::Entries { id: old, entries: vec![], truncated: false };
    assert!(matches!(sf.on_response(&resp), SearchOutcome::Ignored));
}
```

- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement** — `request` allocates an id, builds the filter (escape `*()\\NUL` per RFC 4515), submits `Request::Search { scope: SearchScope::Subtree, size_limit: Some(PICKER_SEARCH_CAP), attrs }`, sets `latest = Some(id)`. `on_response` returns `Ignored` unless `id == latest`; on `Entries` maps to `SearchRow { dn, store_value, label }`, honoring truncation.
- [ ] **Step 4: Run → PASS.**
- [ ] **Step 5: Commit** (`feat(workflows): SearchFlow — async LDAP candidate search (mirrors AllocFlow)`).

### Task 12: `UiState` search integration (pump correlation + apply)

**Files:** Modify `src/tui/state.rs` (`UiState.search_flow: SearchFlow`; `apply_search_results`; correlate in `pump_worker` alongside `alloc_flow`); test same file.

**Interfaces:**
- Produces: `UiState.search_flow`; `UiState.search_results: Vec<SearchRow>` + `search_truncated: bool` (read by the picker dialog); `pump_worker` calls `self.search_flow.on_response(resp)` and, on `Results`, stores them (borrow-safe).

- [ ] **Step 1: Failing test** — feed a `Response::Entries` with the latest search id through `pump_worker`; assert `search_results` populated and `out.changed`.
- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement** — add the correlation branch after the `alloc_flow` branch in `pump_worker`:
```rust
        let s_out = self.search_flow.on_response(resp);
        if !matches!(s_out, SearchOutcome::Ignored) {
            self.apply_search_results(s_out);
            out.changed = true;
            continue;
        }
```
- [ ] **Step 4: Run → PASS;** build.
- [ ] **Step 5: Commit** (`feat(tui/state): pump correlation + apply for SearchFlow`).

### Task 13–14: `PickerWidget` + `PickerEditor` dialog

**Files:** Create `src/tui/picker_dialog.rs` + `src/tui/widget/picker.rs`; modify `src/tui/widget.rs` (`widget_for`/`is_modal_field` for `WidgetKind::Picker(b)` with `b.fanout_attr.is_none()`), `src/tui/mod.rs`; test the dialog headless + a gated live test in `tests/tv_picker.rs`.

**Interfaces:**
- `PickerWidget`: `present` = selected store-values joined (or `‹none›`); `activate` → `Modal(PickerEditor { binding })`.
- `PickerEditor` (`FieldEditor`): a `Dialog` with a search `InputLine` on top + a `ListBox`. On each keystroke in the search box: post a `SearchFlow.request` via the worker (through `Shared`); `reset_current` and the pump-driven REFRESH rebuild the list from `UiState.search_results` + `pick_state` (selected-first; cap hint when `search_truncated`). Single (`select == Single` or auto-single) → radio (Enter replaces); Multi → checkbox (Space toggles). `update_staged` → `CommitOutcome::SetValues(pick_state.commit())`.

- [ ] **Task 13 Step 1:** Failing test for `PickerWidget::present` (selected values joined). Run → FAIL → implement → PASS. Commit (folded with Task 14 for compilation).
- [ ] **Task 14 Step 1:** Failing headless test: seed `UiState.search_results` with two rows, `reset_current` builds the list, toggle one (multi) → `staged_commit == SetValues([that store_value])`.
- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement** the dialog (model on `oc_picker.rs`; the search box submits `search_flow.request` — collect args, drop borrow, then `worker.submit` per borrow discipline; the list rebuilds from `search_results` on REFRESH). Single vs multi from `binding.select` (fallback to schema arity via the field's `multi`).
- [ ] **Step 4: Run → PASS.**
- [ ] **Step 5: Gated live test** `tests/tv_picker.rs` (skips without `EDAPTOR_TEST_LDAP_URI`): drive a picker-bound field (e.g. `gidNumber` with `kind="picker"`), search returns real groups, select one, assert `staged_commit`/applied field value. Use the demo config.
- [ ] **Step 6: Live acceptance** (tmux): open a picker field, type to search, select, OK. Discard. `CHANGES.md`: "tvision UI: picker widget with live LDAP search (single/multi, selected-first, 100-cap)." Commit.

---

# PART 5 — Membership (two-column) + combined save + multi-entry write

The riskiest part, built last. Port the combining logic to neutral, extend `write_flow` for multi-entry, build the two-column mover, and extend the confirm dialog with a combined LDIF preview.

### Task 15: neutral `plan_combined_save` in `workflows::save`

**Files:** Modify `src/workflows/save.rs` (port `plan_combined_save` + `CombinedSave` from `src/ui/app/save.rs`, neutralized); also port `fanout_labels()` / `fanout_attr_of()` logic onto `workflows::edit_form` if not present (check first — grep `fanout` in `workflows/edit_form.rs`). Test same file.

**Interfaces:**
- Produces:
```rust
pub struct CombinedSave { pub own_dn: String, pub own_mods: Vec<ModOp>, pub fanout: Vec<(String, ModOp)>, pub ldif: String }
pub enum PlanCombined { Invalid(Vec<ValidationError>), DiffError(String), NoChanges,
                        RenameWithMembershipUnsupported, Ready(CombinedSave) }
pub fn plan_combined_save(schema, form: &EditForm, /* + the same masking/secret/orphaned/x_ordered args prepare_save takes */)
    -> PlanCombined;
```
Own-entry diff with each fan-out (back-ref) label stripped from BOTH original and edited; per-holder ops via `membership_fanout(form.dn, baseline, current_values, holder_attr)`; combined LDIF (own changeset + one stanza per touched group). Reject rename combined with a membership change (v1).

- [ ] **Step 1:** Read `src/ui/app/save.rs` `plan_combined_save`/`CombinedSave`/`apply_combined_save` and `src/ui/edit_form.rs` `fanout_labels`/`fanout_attr_of`. 
- [ ] **Step 2: Failing tests** — (a) a memberOf change with `via=member` yields fanout adds/deletes per group and NO own-entry memberOf mod; (b) back-ref stripped from both sides; (c) rename+membership → `RenameWithMembershipUnsupported`. (`membership_fanout`/`would_empty` already have tests — reuse.)
- [ ] **Step 3: Implement** the neutral port (no `crate::ui` imports; use `workflows::edit_form`).
- [ ] **Step 4: Run → PASS;** facade guard clean.
- [ ] **Step 5: Commit** (`feat(workflows/save): neutral plan_combined_save — own diff + membership fan-out`).

### Task 16: multi-entry write in `WriteFlow` + last-member pre-validation

**Files:** Modify `src/workflows/write_flow.rs` (a `submit_combined` that submits the own MODIFY + N per-group MODIFYs, tracking all ids under one logical batch; pre-validate `would_empty` before submitting any leg; report completion when all legs land). Test same file.

**Interfaces:**
- Produces:
```rust
impl WriteFlow {
    pub fn submit_combined(&mut self, worker, combined: CombinedSave, group_members: &HashMap<String, Vec<String>>,
                           reread_dn: &str, quit_after: bool) -> Result<(), String>;  // Err = last-member abort (nothing submitted)
}
// WriteOutcome gains: BatchProgress { remaining: usize } and reuses Saved{reread_dn,quit_after} when the batch completes,
// or a new CombinedSaved { reread_dn, quit_after }. Error on any leg aborts/report.
```
`group_members` supplies each affected group's current `member` for the `would_empty` check on removals (the dispatch fetches these, or passes what it has; if unavailable, the check is conservative — document it). Track outstanding ids in a `batch` map; `on_response` decrements and yields `CombinedSaved` on the last `WriteOk`.

- [ ] **Step 1: Failing tests** — (a) `submit_combined` with one add + one remove submits 2 (or 3 incl. own) Modify requests with distinct ids; (b) a removal that would empty a group returns `Err` and submits nothing; (c) `on_response` yields `CombinedSaved` only after the last leg.
- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement** — pre-validate all removals via `would_empty`; if any fails, return `Err(msg)` before any `worker.submit`. Otherwise submit own (if `own_mods` non-empty) + each fanout op as `Request::Modify`, record ids in a batch, return Ok. `on_response`: on `WriteOk` for a batch id, decrement; last one → `CombinedSaved`. Any `WriteError` → `Error` (note: partial writes already applied — surface clearly in the message).
- [ ] **Step 4: Run → PASS.**
- [ ] **Step 5: Commit** (`feat(workflows/write_flow): submit_combined — multi-entry membership write + last-member guard`).

### Task 17: combined LDIF preview in the confirm dialog

**Files:** Modify `src/tui/dialog/confirm.rs` (the builder already takes `ldif: &str` — the combined LDIF from `CombinedSave.ldif` flows in unchanged; verify the StaticText area is tall enough / scrolls for multi-stanza previews — if not, widen/heighten or swap to a scrollable text). Test: a render/headless check is optional; primarily verified live.

- [ ] **Step 1:** Confirm `confirm::build(ldif)` renders multi-line combined LDIF; if the fixed `Rect::new(2,2,68,16)` truncates, enlarge the dialog or use a scroll region. Make the minimal change.
- [ ] **Step 2:** Commit (`feat(tui/dialog): confirm renders combined multi-entry LDIF preview`).

### Task 18: `MembershipWidget` + two-column mover dialog

**Files:** Create `src/tui/membership_dialog.rs` + `src/tui/widget/membership.rs`; modify `src/tui/widget.rs` (`widget_for`/`is_modal_field` for `WidgetKind::Picker(b)` with `b.fanout_attr.is_some()`), `src/tui/mod.rs`; test the dialog headless.

**Interfaces:**
- `MembershipWidget`: `present` = member count / joined CNs (or `‹none›`); `activate` → `Modal(MembershipEditor { binding })`.
- `MembershipEditor` (`FieldEditor`): a `Dialog` with **two `ListBox` columns** — left **Available** (a search `InputLine` above it, fed by `SearchFlow` like the picker), right **Members** (the staged DN set, seeded from `field.values` = baseline `memberOf`). Keys: Enter/→ move highlighted Available row → Members (de-dupe, DN case-insensitive); Del/← remove highlighted Members row; search box focuses Available. `update_staged` → `CommitOutcome::SetValues(members)`. A row already in Members is marked in Available.

- [ ] **Step 1: Failing headless test** — seed `search_results` with two groups, baseline Members = `[g1]`; move `g2` from Available → Members; `staged_commit == SetValues([g1, g2])` (order/case per spec). Remove `g1`; → `SetValues([g2])`.
- [ ] **Step 2: Run → FAIL.**
- [ ] **Step 3: Implement** the two-column dialog. Layout: split the dialog body into left/right halves; each a `ListBox`; the search `InputLine` spans the left column top. Move keys operate on the focused column. Reuse `pick_state` for membership de-dupe/compare if convenient, or a simple `Vec<String>` with case-insensitive contains. Borrow discipline on `search_flow.request`.
- [ ] **Step 4: Run → PASS.** Add a de-dupe test (moving an already-member row is a no-op).
- [ ] **Step 5: Commit** (`feat(tui): MembershipEditor — two-column mover, fan-out staged as SetValues`).

### Task 19: dispatch — combined save path + apply

**Files:** Modify `src/tui/app.rs` (the Save dispatch: when the form has a fan-out/membership field with changes, build a `CombinedSave` via `plan_combined_save`, show the confirm with combined LDIF, and on OK call `write_flow.submit_combined` instead of the single-entry `submit`); `src/tui/state.rs` (`apply` the `CombinedSaved` outcome — re-read the current entry; clear dirty). Test: gated live test in `tests/tv_membership.rs`.

**Interfaces:**
- Consumes: `plan_combined_save`, `WriteFlow::submit_combined`, `WriteOutcome::CombinedSaved`.
- Produces: the membership save round-trip.

- [ ] **Step 1:** In the Save path, branch on "does the form have a fan-out field with a non-empty diff?" (use `fanout_labels()` + per-field dirty). If yes → `plan_combined_save` → confirm(combined ldif) → on OK `submit_combined`. Else → existing single-entry path unchanged.
- [ ] **Step 2:** Handle `WriteOutcome::CombinedSaved` in `apply_write_outcome` (re-read current entry like `Saved`).
- [ ] **Step 3: Gated live test** `tests/tv_membership.rs`: create a temp user, add it to a group via the membership flow, submit, assert the group's `member` now contains the user DN and the user's `memberOf` was NOT written by us; remove it again; clean up the temp user. (Mirror `tests/tv_create.rs` RAII cleanup.)
- [ ] **Step 4:** `cargo test -j4`; `EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389 cargo test -j4 --test tv_membership` (with the demo server up). PASS.
- [ ] **Step 5: Commit** (`feat(tui): membership combined-save round-trip (multi-entry fan-out)`).

### Task 20: Live acceptance + docs + final gate

- [ ] **Step 1:** tmux live: open a user's membership field, search a group, move it to Members, OK, Save → combined LDIF preview lists the user (if own mods) + the group stanza → confirm → verify in a fresh read the group gained the member. **Use a temp entry**; restore demo data.
- [ ] **Step 2:** `CHANGES.md`: "tvision UI: membership widget (two-column mover) with multi-entry fan-out write, last-member guard, and combined LDIF preview."
- [ ] **Step 3:** Update `docs/src/configuration/widgets.md` — note the tvision UI now implements choice/picker/membership/multi-value/sambaSID at parity (config format unchanged).
- [ ] **Step 4:** Final gate:
```bash
cargo fmt --check
cargo clippy -j4 --all-targets -- -D warnings
cargo test -j4
! grep -rl "use tvision_rs" src | grep -vE "^src/(tui/|bin/edaptor-tv.rs)"
! grep -rl "use ratatui\|use tui_" src | grep -vE "^src/ui/"
make check
```
All green; both guards print nothing.
- [ ] **Step 5:** Update `docs/HANDOVER.md` — M4 complete; the only remaining migration work is M5 (startup flow + cutover + dedup of the `pick_state`/`edit_form` parity copies + X-ORDERED `{n}` prefix on save). Commit.

---

## Self-review notes (resolved)

- **Spec coverage:** multi-value editor (Part 1) · X-ORDERED flag (T1) · choice (Part 2) · sambaSID + samba ctx (Part 3) · SearchFlow + picker (Part 4) · membership two-column + combined save + multi-entry write + combined preview (Part 5). All §4 spec components mapped. X-ORDERED `{n}` prefix-on-save is explicitly **out of scope** (spec §8) — not planned here.
- **Seam change:** only `Activation::Immediate` is added (additive, spec-sanctioned); no form-core change.
- **Type consistency:** `CommitOutcome::SetValues` used by every widget; `SearchFlow`/`SearchOutcome`/`SearchRow`, `PickState`, `CombinedSave`/`PlanCombined`, `WriteOutcome::CombinedSaved` named consistently across tasks.
- **Compilation discipline:** Tasks whose widget references a not-yet-built dialog (T2→T3, T5→T6, T13→T14) are committed together so the crate compiles after every commit.
- **Open detail deferred to implementation:** the `would_empty` group-member source in `submit_combined` (T16) — fetch current members in dispatch, or pass the membership editor's known set; document whichever is chosen.
