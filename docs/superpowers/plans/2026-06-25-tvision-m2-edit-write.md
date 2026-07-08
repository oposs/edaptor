# tvision-rs Migration M2 — Edit + Write Spine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the tvision form pane editable for plain single-value fields and persist edits to LDAP end-to-end (MODIFY + MODRDN), with Confirm (LDIF preview) / Error / Guard dialogs, async save through the worker + pump, and dirty-nav / dirty-quit guards.

**Architecture:** A UI-neutral editable model (`workflows::edit_form`) holds values + baseline + dirty + `to_edit_entry()`. A `workflows::write_flow` wraps `workflows::save::prepare_save` and correlates async worker writes. The tvision form pane owns grapheme-correct `InputLine` editors and syncs committed text into the model. All modal dialogs open from the single `Program::run_app(|prog, cmd|)` dispatch closure; panes and the pump request dialogs by **posting commands** (`SAVE`, `REQUEST_QUIT`, `GUARD_NAV`, `SHOW_ERROR`). Deferred-quit and async-error are driven by the pump posting commands while the modal loop keeps pumping.

**Tech Stack:** Rust 2021, `tvision-rs = "0.1"` (0.1.2), existing domain layer (`config`, `form`, `ldap::worker`, `schema`, `workflows`), `anyhow`.

## Global Constraints

- **Cap build/test parallelism at 4 cores:** always `-j4`. Cargo target dir is `/home/oetiker/scratch/cargo-target` (already configured).
- **tvision-rs version:** published `tvision-rs = "0.1"` (0.1.2). Alias `tv`. No path/git dependency.
- **0.1.1+ API facts (do NOT reintroduce 0.1.0 workarounds):** `Outline` auto-seeds; `Deferred` at crate root; read selection via `value() -> FieldValue::Int`.
- **Facade boundary:** only `src/tui/**` and `src/bin/edaptor-tv.rs` may `use tvision_rs`. Only `src/ui/**` may `use ratatui` / `use tui_*`. The domain layer (incl. new `workflows::edit_form`, `workflows::write_flow`) imports neither.
- **Borrow discipline:** never hold a `RefCell` borrow across `ctx.broadcast`, `ctx.post`, `Program::exec_view`, `ListBox::new_list`, `Group::child_mut`, `InputLine::set_value`, or `worker.submit`/`request_entry`. Collect into locals → drop the borrow → call.
- **Do NOT touch `src/ui/**` (ratatui).** The neutral model is introduced fresh; the ratatui `ui::edit_form` is deleted at the M5 cutover (spec §3).
- **Strict TDD; atomic commits; crate compiles after every commit; `cargo fmt` before every commit; clippy clean (`--all-targets -D warnings`).**
- **Commit trailer:** `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- **Live tests** gated by env `EDAPTOR_TEST_LDAP_URI` (skip when unset). Interactive acceptance needs a human at a terminal (agent sessions have no TTY → `CrosstermBackend::new()` returns ENXIO).

## Verification commands (used throughout)

```bash
cargo build -j4
cargo build -j4 --bin edaptor-tv
cargo test  -j4
cargo clippy -j4 --all-targets -- -D warnings
cargo fmt --check
# facade guard (must print nothing):
! grep -rl "use tvision_rs" src | grep -vE "^src/(tui/|bin/edaptor-tv.rs)"
! grep -rl "use ratatui\|use tui_" src | grep -vE "^src/ui/"
```

---

## File Structure

| File | Responsibility | Task |
|---|---|---|
| `src/workflows/edit_form.rs` | NEW. Neutral `EditField`/`EditForm`, `build_edit_form`, `set_value`, `current_values`, `is_dirty`, `to_edit_entry`, `value_set_eq` | 1 |
| `src/workflows/mod.rs` | Register `pub mod edit_form;` + `pub mod write_flow;` | 1,2 |
| `src/workflows/write_flow.rs` | NEW. `WriteFlow`: `prepare` (pure), `submit`/`submit_followup` (worker), `on_response` (pure), `WriteOutcome` | 2 |
| `src/tui/widget.rs` | `present(&EditField)`, `activate()`, `PlainWidget`, `inline_editable(&EditField)` | 3 |
| `src/tui/state.rs` | `UiState` edit fields + `pump_worker` write routing → `PumpResult` | 4 |
| `src/tui/pump.rs` | `PumpView` acts on `PumpResult` (broadcast / post QUIT / post SHOW_ERROR) | 5 |
| `src/tui/panes/form.rs` | Header row + label column + editable value `InputLine`; per-event sync to `edit_form` | 6 |
| `src/tui/dialog/mod.rs` | NEW. module + `GuardDecision` + `guard_decision()` pure helper | 7 |
| `src/tui/dialog/confirm.rs` | NEW. LDIF-preview confirm dialog builder | 7 |
| `src/tui/dialog/error.rs` | NEW. dismissible error dialog builder | 7 |
| `src/tui/dialog/guard.rs` | NEW. save/discard/stay guard dialog builder | 7 |
| `src/tui/mod.rs` | command constants; wire `run_app` → `app::dispatch` | 8 |
| `src/tui/app.rs` | menu/status Save + Exit→REQUEST_QUIT; `dispatch(prog,cmd,state)`; leaf dirty-nav interception | 8 |
| `src/tui/panes/leaf.rs` | Dirty-nav interception: stash `guard_target`, post `GUARD_NAV` | 8 |
| `tests/tv_edit_write.rs` | NEW. Live (gated) edit+persist and rename integration test | 9 |
| `CHANGES.md` | Unreleased entry for editable tvision form | 9 |

---

## Task 1: Neutral editable form model — `workflows::edit_form`

Port the **logic** of `src/ui/edit_form.rs` into a UI-neutral module with **no `tui_prompts::TextState`**. The tvision pane (Task 6) owns the text editor; the model carries plain `Vec<String>` values + a baseline. Only the subset M2 needs is ported: `EditField`, `EditForm`, `build_edit_form`, `set_value`, `current_values`, `is_dirty`, `to_edit_entry`, `value_set_eq`. (`sync_schema_fields`, picker/fan-out, `FormMode::New` are M3/M4.)

**Files:**
- Create: `src/workflows/edit_form.rs`
- Modify: `src/workflows/mod.rs` (add `pub mod edit_form;`)
- Test: inline `#[cfg(test)] mod tests` in `src/workflows/edit_form.rs`

**Interfaces:**
- Consumes: `workflows::form_model::{FormModel, FormField, WidgetSpec}`, `schema::{FieldKind, SchemaModel}`, `form::changeset::EditEntry`, `config::widget::WidgetKind`.
- Produces:
  - `pub struct EditField { pub label: String, pub must: bool, pub editable: bool, pub multi: bool, pub secret: bool, pub ordered: bool, pub orphaned: bool, pub kind: FieldKind, pub widget: WidgetSpec, pub widget_binding: Option<WidgetKind>, pub values: Vec<String>, pub baseline: Vec<String> }`
  - `impl EditField { pub fn current_values(&self) -> Vec<String> }`
  - `pub enum FormMode { Edit }`
  - `pub struct EditForm { pub dn: String, pub mode: FormMode, pub object_classes: Vec<String>, pub fields: Vec<EditField> }`
  - `impl EditForm { pub fn set_value(&mut self, idx: usize, text: String); pub fn is_dirty(&self) -> bool; pub fn to_edit_entry(&self) -> EditEntry }`
  - `pub fn build_edit_form(model: &FormModel, schema: &SchemaModel, read_only: bool) -> EditForm`

- [ ] **Step 1: Write failing tests for `build_edit_form` + `is_dirty` + `to_edit_entry`**

Add to `src/workflows/edit_form.rs` (create the file with just the test module first to see it fail to compile, then add the impl):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldap::worker::RawSubschema;
    use crate::workflows::form_model::{FormField, FormModel, WidgetSpec};
    use crate::schema::{FieldKind, SchemaModel};

    fn schema() -> SchemaModel {
        let raw = RawSubschema {
            object_classes: vec![
                "( 2.5.6.0 NAME 'top' ABSTRACT MUST objectClass )".to_string(),
                "( 2.5.6.6 NAME 'person' SUP top STRUCTURAL MUST ( sn $ cn ) MAY description )".to_string(),
            ],
            attribute_types: vec![
                "( 2.5.4.3 NAME 'cn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".to_string(),
                "( 2.5.4.4 NAME 'sn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".to_string(),
            ],
            ldap_syntaxes: vec![],
        };
        SchemaModel::from_raw(&raw)
    }

    fn model() -> FormModel {
        FormModel {
            title: "cn=Alice,dc=example,dc=org".to_string(),
            fields: vec![
                FormField { label: "cn".into(), kind: FieldKind::Text, is_must: true, values: vec!["Alice".into()], widget: WidgetSpec::ReadOnlyText },
                FormField { label: "sn".into(), kind: FieldKind::Text, is_must: true, values: vec!["Adams".into()], widget: WidgetSpec::ReadOnlyText },
            ],
        }
    }

    #[test]
    fn build_seeds_values_and_baseline_equal() {
        let f = build_edit_form(&model(), &schema(), false);
        assert_eq!(f.dn, "cn=Alice,dc=example,dc=org");
        assert!(!f.is_dirty());
        assert_eq!(f.fields[0].values, vec!["Alice".to_string()]);
        assert_eq!(f.fields[0].baseline, vec!["Alice".to_string()]);
        assert!(f.fields[0].editable);
    }

    #[test]
    fn read_only_forces_non_editable() {
        let f = build_edit_form(&model(), &schema(), true);
        assert!(f.fields.iter().all(|x| !x.editable));
    }

    #[test]
    fn set_value_marks_dirty_and_to_edit_entry_reflects_it() {
        let mut f = build_edit_form(&model(), &schema(), false);
        f.set_value(0, "Alicia".to_string());
        assert!(f.is_dirty());
        let e = f.to_edit_entry();
        assert_eq!(e.dn, "cn=Alice,dc=example,dc=org");
        assert_eq!(e.attrs.get("cn"), Some(&vec!["Alicia".to_string()]));
    }

    #[test]
    fn emptied_single_field_yields_no_values() {
        let mut f = build_edit_form(&model(), &schema(), false);
        f.set_value(0, "   ".to_string());
        assert_eq!(f.fields[0].current_values(), Vec::<String>::new());
    }

    #[test]
    fn reorder_only_is_not_dirty_setwise() {
        assert!(value_set_eq(&["a".into(), "b".into()], &["b".into(), "a".into()]));
    }

    #[test]
    fn ordered_field_reorder_is_dirty() {
        let mut f = build_edit_form(&model(), &schema(), false);
        f.fields[0].ordered = true;
        f.fields[0].values = vec!["b".into(), "a".into()];
        f.fields[0].baseline = vec!["a".into(), "b".into()];
        assert!(f.is_dirty());
    }
}
```

- [ ] **Step 2: Run to verify it fails (does not compile)**

Run: `cargo test -j4 -p edaptor edit_form 2>&1 | head -20`
Expected: FAIL — `build_edit_form`, `EditForm`, etc. not found.

- [ ] **Step 3: Implement the module above the test block**

Prepend to `src/workflows/edit_form.rs`:

```rust
//! UI-neutral editable form model: the M2 editable shape derived from a read-only
//! [`FormModel`]. Carries plain `Vec<String>` values + a load-time `baseline` for
//! the set-wise dirty check; the text editor itself lives in the tvision pane, so
//! there is NO `TextState` here (cf. the ratatui `ui::edit_form`, deleted at M5).

use std::collections::BTreeMap;

use crate::config::widget::WidgetKind;
use crate::form::changeset::EditEntry;
use crate::schema::{FieldKind, SchemaModel};
use crate::workflows::form_model::{FormField, FormModel, WidgetSpec};

/// One editable field.
pub struct EditField {
    pub label: String,
    pub must: bool,
    pub editable: bool,
    pub multi: bool,
    pub secret: bool,
    pub ordered: bool,
    pub orphaned: bool,
    pub kind: FieldKind,
    pub widget: WidgetSpec,
    pub widget_binding: Option<WidgetKind>,
    pub values: Vec<String>,
    pub baseline: Vec<String>,
}

impl EditField {
    /// The field's value set as currently edited.
    ///
    /// - orphaned → `[]` (the diff emits a Delete regardless);
    /// - single + editable → the trimmed `values[0]`; emptied → `[]` so the diff
    ///   emits a delete, not an empty value;
    /// - otherwise → `values` unchanged.
    pub fn current_values(&self) -> Vec<String> {
        if self.orphaned {
            return vec![];
        }
        if !self.multi && self.editable {
            let v = self.values.first().map(|s| s.trim()).unwrap_or("");
            if v.is_empty() {
                vec![]
            } else {
                vec![v.to_string()]
            }
        } else {
            self.values.clone()
        }
    }
}

/// Create vs edit; only `Edit` exists in M2 (`New` is M3's create flow).
pub enum FormMode {
    Edit,
}

/// An editable entry: its DN, objectClasses, and fields.
pub struct EditForm {
    pub dn: String,
    pub mode: FormMode,
    pub object_classes: Vec<String>,
    pub fields: Vec<EditField>,
}

impl EditForm {
    /// Write a committed single-value inline edit into `fields[idx]`.
    pub fn set_value(&mut self, idx: usize, text: String) {
        if let Some(f) = self.fields.get_mut(idx) {
            f.values = vec![text];
        }
    }

    /// Whether any field's current value differs from its baseline. Set-wise
    /// (order-insensitive) unless the field is `ordered`, matching
    /// `changeset::diff` semantics so a pure reorder of an unordered attribute is
    /// NOT dirty.
    pub fn is_dirty(&self) -> bool {
        self.fields.iter().any(|f| {
            let current = f.current_values();
            if f.ordered {
                current != f.baseline
            } else {
                !value_set_eq(&current, &f.baseline)
            }
        })
    }

    /// A pure [`EditEntry`] of every field's current values, keyed by label.
    pub fn to_edit_entry(&self) -> EditEntry {
        let attrs: BTreeMap<String, Vec<String>> = self
            .fields
            .iter()
            .map(|f| (f.label.clone(), f.current_values()))
            .collect();
        EditEntry {
            dn: self.dn.clone(),
            attrs,
        }
    }
}

/// Order-insensitive value-set equality (same length, each element of each side
/// present in the other). The dirty-check sibling of `changeset::diff`.
pub fn value_set_eq(a: &[String], b: &[String]) -> bool {
    a.len() == b.len()
        && a.iter().all(|v| b.iter().any(|w| w == v))
        && b.iter().all(|v| a.iter().any(|w| w == v))
}

/// True when a field is a free-text editor (not binary / boolean-checkbox).
fn field_is_editable(f: &FormField) -> bool {
    !matches!(
        f.widget,
        WidgetSpec::BinaryNote(_) | WidgetSpec::DisabledCheckBox(_)
    )
}

/// Build an [`EditForm`] from a read-only [`FormModel`] + schema. `values` and
/// `baseline` are seeded equal (clean). `editable = !read_only && free-text kind`;
/// `multi` from the schema; `secret`/`ordered`/`orphaned` start `false`
/// (M4/M3 passes refine them). `object_classes` come from the model's `cn=`?—no:
/// they are not on `FormModel`, so the caller passes them via the read path; here
/// we leave them empty and the caller fills `object_classes` (see Task 4 wiring).
pub fn build_edit_form(model: &FormModel, schema: &SchemaModel, read_only: bool) -> EditForm {
    let fields: Vec<EditField> = model
        .fields
        .iter()
        .map(|f| {
            let editable = !read_only && field_is_editable(f);
            EditField {
                label: f.label.clone(),
                must: f.is_must,
                editable,
                multi: !schema.is_single_value(&f.label),
                secret: false,
                ordered: false,
                orphaned: false,
                kind: f.kind,
                widget: f.widget.clone(),
                widget_binding: None,
                values: f.values.clone(),
                baseline: f.values.clone(),
            }
        })
        .collect();

    EditForm {
        dn: model.title.clone(),
        mode: FormMode::Edit,
        object_classes: Vec::new(),
        fields,
    }
}
```

Note: `object_classes` is filled by the caller (Task 4) from the read path's
`ReadOutcome::Form { object_classes, .. }`, since `FormModel` does not carry them.

- [ ] **Step 4: Register the module**

In `src/workflows/mod.rs` add (alphabetical with the other `pub mod` lines):

```rust
pub mod edit_form;
```

- [ ] **Step 5: Run tests to verify pass + clippy + fmt**

Run: `cargo test -j4 edit_form && cargo clippy -j4 --all-targets -- -D warnings && cargo fmt --check`
Expected: tests PASS; clippy clean; fmt clean.

- [ ] **Step 6: Commit**

```bash
git add src/workflows/edit_form.rs src/workflows/mod.rs
git commit -m "feat(tui): neutral workflows::edit_form editable model (M2 T1)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 2: Write flow — `workflows::write_flow`

Wrap `workflows::save::prepare_save` (pure prepare) and correlate async worker
writes. `prepare` and `on_response` are PURE (no worker) and unit-tested by
seeding `pending` exactly like `ReadFlow`'s tests. `submit`/`submit_followup` are
thin worker wrappers covered by the live test (Task 9). Request ids use a private
counter; reads (`Entries`/`SearchError`) and writes (`WriteOk`/`WriteError`) are
disjoint response variants, so the id spaces may overlap harmlessly — each flow
only matches its own variants (assert this invariant in a test).

**Files:**
- Create: `src/workflows/write_flow.rs`
- Modify: `src/workflows/mod.rs` (add `pub mod write_flow;`)
- Test: inline `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `workflows::edit_form::EditForm`, `workflows::save::{prepare_save, PrepareSave, compose_renamed_dn}`, `form::validate::SavePlan`, `form::changeset::{EditEntry, ModOp, ModRdn}`, `schema::SchemaModel`, `ldap::worker::{WorkerHandle, Request, Response}`.
- Produces:
  - `pub enum WriteOutcome { Ignored, Saved { reread_dn: String, quit_after: bool }, NeedFollowupModify { dn: String, mods: Vec<ModOp>, quit_after: bool }, Error(String) }`
  - `pub struct WriteFlow { /* private: next_id, pending */ }`
  - `impl WriteFlow { pub fn new() -> Self; pub fn prepare(&self, form: &EditForm, schema: &SchemaModel) -> PrepareSave; pub fn submit(&mut self, worker: &WorkerHandle, plan: SavePlan, old_dn: &str, quit_after: bool) -> anyhow::Result<()>; pub fn submit_followup(&mut self, worker: &WorkerHandle, dn: &str, mods: Vec<ModOp>, quit_after: bool) -> anyhow::Result<()>; pub fn on_response(&mut self, resp: &Response) -> WriteOutcome }`

- [ ] **Step 1: Write failing tests for `prepare` + `on_response`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldap::worker::{RawSubschema, Response};
    use crate::schema::{FieldKind, SchemaModel};
    use crate::workflows::edit_form::{EditField, EditForm, FormMode};
    use crate::workflows::form_model::WidgetSpec;

    fn schema() -> SchemaModel {
        SchemaModel::from_raw(&RawSubschema {
            object_classes: vec![
                "( 2.5.6.0 NAME 'top' ABSTRACT MUST objectClass )".to_string(),
                "( 2.5.6.6 NAME 'person' SUP top STRUCTURAL MUST ( sn $ cn ) MAY description )".to_string(),
            ],
            attribute_types: vec![
                "( 2.5.4.3 NAME 'cn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".to_string(),
                "( 2.5.4.4 NAME 'sn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".to_string(),
            ],
            ldap_syntaxes: vec![],
        })
    }

    fn field(label: &str, val: &str, base: &str) -> EditField {
        EditField {
            label: label.into(), must: true, editable: true, multi: false,
            secret: false, ordered: false, orphaned: false, kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText, widget_binding: None,
            values: vec![val.into()], baseline: vec![base.into()],
        }
    }

    fn form_with(fields: Vec<EditField>) -> EditForm {
        EditForm { dn: "cn=Alice,dc=example,dc=org".into(), mode: FormMode::Edit,
            object_classes: vec!["top".into(), "person".into()], fields }
    }

    #[test]
    fn prepare_no_change_is_nochanges() {
        let wf = WriteFlow::new();
        let f = form_with(vec![field("cn", "Alice", "Alice"), field("sn", "Adams", "Adams")]);
        assert!(matches!(wf.prepare(&f, &schema()), PrepareSave::NoChanges));
    }

    #[test]
    fn prepare_modify_yields_ready() {
        let wf = WriteFlow::new();
        let f = form_with(vec![field("cn", "Alice", "Alice"), field("sn", "Allen", "Adams")]);
        match wf.prepare(&f, &schema()) {
            PrepareSave::Ready { dn, ldif, .. } => {
                assert_eq!(dn, "cn=Alice,dc=example,dc=org");
                assert!(ldif.contains("sn"));
            }
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    #[test]
    fn write_ok_for_save_intent_reads_back() {
        let mut wf = WriteFlow::new();
        wf.pending.insert(7, WriteIntent::Save { reread_dn: "cn=Bob,dc=x".into(), quit_after: false });
        match wf.on_response(&Response::WriteOk { id: 7, dn: "cn=Bob,dc=x".into() }) {
            WriteOutcome::Saved { reread_dn, quit_after } => {
                assert_eq!(reread_dn, "cn=Bob,dc=x");
                assert!(!quit_after);
            }
            other => panic!("expected Saved, got {other:?}"),
        }
        assert!(wf.pending.is_empty());
    }

    #[test]
    fn write_ok_for_rename_then_modify_requests_followup() {
        let mut wf = WriteFlow::new();
        wf.pending.insert(3, WriteIntent::RenameThenModify {
            new_dn: "cn=New,dc=x".into(),
            mods: vec![ModOp::Replace { attr: "sn".into(), values: vec!["Z".into()] }],
            quit_after: true,
        });
        match wf.on_response(&Response::WriteOk { id: 3, dn: "cn=New,dc=x".into() }) {
            WriteOutcome::NeedFollowupModify { dn, mods, quit_after } => {
                assert_eq!(dn, "cn=New,dc=x");
                assert_eq!(mods.len(), 1);
                assert!(quit_after);
            }
            other => panic!("expected NeedFollowupModify, got {other:?}"),
        }
    }

    #[test]
    fn write_error_surfaces_message() {
        let mut wf = WriteFlow::new();
        wf.pending.insert(9, WriteIntent::Save { reread_dn: "x".into(), quit_after: false });
        match wf.on_response(&Response::WriteError { id: 9, msg: "constraint".into() }) {
            WriteOutcome::Error(m) => assert_eq!(m, "constraint"),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn entries_response_is_ignored_even_on_id_overlap() {
        // A read response with the same id as a pending write must NOT be consumed.
        let mut wf = WriteFlow::new();
        wf.pending.insert(1, WriteIntent::Save { reread_dn: "x".into(), quit_after: false });
        assert!(matches!(
            wf.on_response(&Response::Entries { id: 1, entries: vec![], truncated: false }),
            WriteOutcome::Ignored
        ));
        assert_eq!(wf.pending.len(), 1);
    }
}
```

- [ ] **Step 2: Run to verify it fails (does not compile)**

Run: `cargo test -j4 write_flow 2>&1 | head -20`
Expected: FAIL — `WriteFlow`, `WriteIntent`, `WriteOutcome` not found.

- [ ] **Step 3: Implement the module**

Prepend to `src/workflows/write_flow.rs`:

```rust
//! Async write flow: validate + diff an [`EditForm`] save (via
//! [`crate::workflows::save::prepare_save`]) and correlate the worker's write
//! responses. `prepare` and `on_response` are pure; `submit`/`submit_followup`
//! are thin worker wrappers. Mirrors `read_flow` but for writes; the two never
//! collide because read and write responses are disjoint `Response` variants.

use std::collections::HashMap;

use anyhow::Result;

use crate::form::changeset::{EditEntry, ModOp};
use crate::form::validate::SavePlan;
use crate::ldap::worker::{Request, Response, WorkerHandle};
use crate::schema::SchemaModel;
use crate::workflows::edit_form::EditForm;
use crate::workflows::save::{compose_renamed_dn, prepare_save, PrepareSave};

/// What a pending write means once its `WriteOk` arrives.
#[derive(Debug, Clone)]
enum WriteIntent {
    /// A plain save (or a rename's final leg): re-read `reread_dn` afterwards.
    Save { reread_dn: String, quit_after: bool },
    /// A rename's first leg: on success, submit `mods` against `new_dn`.
    RenameThenModify { new_dn: String, mods: Vec<ModOp>, quit_after: bool },
}

/// The app-facing result of correlating one write response.
#[derive(Debug, Clone)]
pub enum WriteOutcome {
    /// Not one of our pending writes.
    Ignored,
    /// A write completed; re-read `reread_dn` (unless quitting).
    Saved { reread_dn: String, quit_after: bool },
    /// A rename's MODRDN landed; caller must submit the deferred `mods` via
    /// [`WriteFlow::submit_followup`].
    NeedFollowupModify { dn: String, mods: Vec<ModOp>, quit_after: bool },
    /// A write failed; `msg` is already human-mapped by the worker.
    Error(String),
}

/// Tracks in-flight writes and turns the edit form into a save plan.
pub struct WriteFlow {
    next_id: u64,
    pending: HashMap<u64, WriteIntent>,
}

impl Default for WriteFlow {
    fn default() -> Self {
        Self::new()
    }
}

impl WriteFlow {
    pub fn new() -> Self {
        // Start above ReadFlow's range as defence in depth; correctness does not
        // rely on it (read/write response variants are disjoint).
        WriteFlow { next_id: 1_000_000, pending: HashMap::new() }
    }

    fn alloc(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Validate + diff `form` into a [`PrepareSave`]. Pure (no worker, no clock).
    /// Password staging is M4, so `password_mods`/`mask_attrs` are empty here.
    pub fn prepare(&self, form: &EditForm, schema: &SchemaModel) -> PrepareSave {
        let original = EditEntry {
            dn: form.dn.clone(),
            attrs: form
                .fields
                .iter()
                .map(|f| (f.label.clone(), f.baseline.clone()))
                .collect(),
        };
        let edited = form.to_edit_entry();
        let secret_attrs: Vec<String> = form
            .fields
            .iter()
            .filter(|f| f.secret)
            .map(|f| f.label.clone())
            .collect();
        let orphaned: Vec<&str> = form
            .fields
            .iter()
            .filter(|f| f.orphaned)
            .map(|f| f.label.as_str())
            .collect();
        let x_ordered: std::collections::HashSet<String> = form
            .fields
            .iter()
            .filter(|f| f.ordered)
            .map(|f| f.label.clone())
            .collect();
        prepare_save(
            schema,
            &original,
            &edited,
            &form.object_classes,
            &[],            // password_mods (M4)
            &[],            // mask_attrs (M4)
            &secret_attrs,
            &orphaned,
            &x_ordered,
        )
    }

    /// Submit a [`SavePlan`] to the worker, tracking what each id means.
    pub fn submit(
        &mut self,
        worker: &WorkerHandle,
        plan: SavePlan,
        old_dn: &str,
        quit_after: bool,
    ) -> Result<()> {
        match plan {
            SavePlan::Nothing => {}
            SavePlan::Modify(mods) => {
                let id = self.alloc();
                worker.submit(Request::Modify { id, dn: old_dn.to_string(), changes: mods })?;
                self.pending.insert(id, WriteIntent::Save { reread_dn: old_dn.to_string(), quit_after });
            }
            SavePlan::RenameOnly(modrdn) => {
                let id = self.alloc();
                let new_dn = compose_renamed_dn(old_dn, &modrdn.new_rdn);
                worker.submit(Request::ModRdn {
                    id, dn: old_dn.to_string(), new_rdn: modrdn.new_rdn,
                    delete_old: modrdn.delete_old, new_superior: modrdn.new_superior,
                })?;
                self.pending.insert(id, WriteIntent::Save { reread_dn: new_dn, quit_after });
            }
            SavePlan::Rename { modrdn, then_mods } => {
                let id = self.alloc();
                let new_dn = compose_renamed_dn(old_dn, &modrdn.new_rdn);
                worker.submit(Request::ModRdn {
                    id, dn: old_dn.to_string(), new_rdn: modrdn.new_rdn,
                    delete_old: modrdn.delete_old, new_superior: modrdn.new_superior,
                })?;
                self.pending.insert(id, WriteIntent::RenameThenModify { new_dn, mods: then_mods, quit_after });
            }
        }
        Ok(())
    }

    /// Submit the deferred modifications of a rename's second leg.
    pub fn submit_followup(
        &mut self,
        worker: &WorkerHandle,
        dn: &str,
        mods: Vec<ModOp>,
        quit_after: bool,
    ) -> Result<()> {
        let id = self.alloc();
        worker.submit(Request::Modify { id, dn: dn.to_string(), changes: mods })?;
        self.pending.insert(id, WriteIntent::Save { reread_dn: dn.to_string(), quit_after });
        Ok(())
    }

    /// Correlate one polled [`Response`]. Pure; ignores non-write variants.
    pub fn on_response(&mut self, resp: &Response) -> WriteOutcome {
        match resp {
            Response::WriteOk { id, .. } => match self.pending.remove(id) {
                Some(WriteIntent::Save { reread_dn, quit_after }) => {
                    WriteOutcome::Saved { reread_dn, quit_after }
                }
                Some(WriteIntent::RenameThenModify { new_dn, mods, quit_after }) => {
                    WriteOutcome::NeedFollowupModify { dn: new_dn, mods, quit_after }
                }
                None => WriteOutcome::Ignored,
            },
            Response::WriteError { id, msg } => {
                if self.pending.remove(id).is_some() {
                    WriteOutcome::Error(msg.clone())
                } else {
                    WriteOutcome::Ignored
                }
            }
            _ => WriteOutcome::Ignored,
        }
    }
}
```

- [ ] **Step 4: Register the module**

In `src/workflows/mod.rs` add:

```rust
pub mod write_flow;
```

- [ ] **Step 5: Run tests + clippy + fmt**

Run: `cargo test -j4 write_flow && cargo clippy -j4 --all-targets -- -D warnings && cargo fmt --check`
Expected: PASS / clean / clean.

- [ ] **Step 6: Commit**

```bash
git add src/workflows/write_flow.rs src/workflows/mod.rs
git commit -m "feat(tui): workflows::write_flow async save correlation (M2 T2)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 3: Widget trait + editability gate — `tui::widget`

Switch `present` to take `&EditField`, add `activate()`, and add the M2 editability
predicate `inline_editable`. The read-only presenters port across unchanged in body.

**Files:**
- Modify: `src/tui/widget.rs`
- Test: inline `#[cfg(test)] mod tests` (update existing)

**Interfaces:**
- Consumes: `workflows::edit_form::EditField`, `workflows::form_model::WidgetSpec`.
- Produces:
  - `pub fn present_field(field: &EditField) -> String`
  - `pub fn inline_editable(field: &EditField) -> bool`
  - `pub enum Activation { Inline }` (unchanged), `pub trait FieldWidget { fn capability(&self)->Capability; fn present(&self, &EditField)->String; fn activate(&self, &EditField)->Activation }`, `pub struct PlainWidget`.

- [ ] **Step 1: Update the failing tests**

Replace the `tests` module's `field` helper and add the new tests in `src/tui/widget.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::FieldKind;
    use crate::workflows::edit_form::EditField;
    use crate::workflows::form_model::WidgetSpec;

    fn field(values: &[&str], widget: WidgetSpec) -> EditField {
        EditField {
            label: "attr".into(), must: false, editable: true, multi: false,
            secret: false, ordered: false, orphaned: false, kind: FieldKind::Text,
            widget, widget_binding: None,
            values: values.iter().map(|s| s.to_string()).collect(),
            baseline: values.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn test_present_single_text() {
        assert_eq!(present_field(&field(&["hello"], WidgetSpec::ReadOnlyText)), "hello");
    }

    #[test]
    fn test_present_multi_summarizes_count() {
        assert_eq!(present_field(&field(&["a", "b", "c"], WidgetSpec::ReadOnlyText)), "‹3 values›");
    }

    #[test]
    fn test_present_checkbox() {
        assert_eq!(present_field(&field(&["TRUE"], WidgetSpec::DisabledCheckBox(true))), "[x]");
    }

    #[test]
    fn test_plain_activate_is_inline() {
        assert_eq!(PlainWidget.activate(&field(&["x"], WidgetSpec::ReadOnlyText)), Activation::Inline);
    }

    #[test]
    fn test_inline_editable_plain_single_true() {
        assert!(inline_editable(&field(&["x"], WidgetSpec::ReadOnlyText)));
    }

    #[test]
    fn test_inline_editable_multi_false() {
        let mut f = field(&["x"], WidgetSpec::ReadOnlyText);
        f.multi = true;
        assert!(!inline_editable(&f));
    }

    #[test]
    fn test_inline_editable_binary_false() {
        let mut f = field(&[], WidgetSpec::BinaryNote(8));
        f.editable = false;
        assert!(!inline_editable(&f));
    }

    #[test]
    fn test_inline_editable_orphaned_false() {
        let mut f = field(&["x"], WidgetSpec::ReadOnlyText);
        f.orphaned = true;
        assert!(!inline_editable(&f));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -j4 -p edaptor widget 2>&1 | head -20`
Expected: FAIL — `present_field` takes `FormField`, `activate`/`inline_editable` missing.

- [ ] **Step 3: Update the implementation**

In `src/tui/widget.rs`: change the import and the three signatures.

Replace `use crate::workflows::form_model::{FormField, WidgetSpec};` with:

```rust
use crate::workflows::edit_form::EditField;
use crate::workflows::form_model::WidgetSpec;
```

In the `FieldWidget` trait add `activate`:

```rust
pub trait FieldWidget {
    fn capability(&self) -> Capability;
    /// The read-only value-cell text for `field`.
    fn present(&self, field: &EditField) -> String;
    /// How `field` is edited. M2: plain fields return `Inline`.
    fn activate(&self, field: &EditField) -> Activation;
}
```

In `impl FieldWidget for PlainWidget` add:

```rust
    fn activate(&self, _field: &EditField) -> Activation {
        Activation::Inline
    }
```

Change `present_field` signature to `pub fn present_field(field: &EditField) -> String` (body unchanged — it only reads `field.values` and `field.widget`).

Append the editability predicate:

```rust
/// Whether a field is inline-editable in M2: a free-text plain single-value field
/// that is writable and not orphaned and not bound to a rich widget (choice /
/// picker / membership / objectClass — those land in M3/M4).
pub fn inline_editable(field: &EditField) -> bool {
    field.editable && !field.multi && !field.orphaned && field.widget_binding.is_none()
}
```

- [ ] **Step 4: Run tests + clippy + fmt**

Run: `cargo test -j4 widget && cargo clippy -j4 --all-targets -- -D warnings && cargo fmt --check`
Expected: PASS / clean / clean. (Note: `src/tui/panes/form.rs` still calls `present_field` with `FormField` — Task 6 fixes it; until then the crate may not build. To keep the crate green per-commit, do Step 5.)

- [ ] **Step 5: Keep the crate compiling — adapt the one M1 caller**

`src/tui/panes/form.rs::render_rows` currently calls `present_field(f)` with `FormField`. Temporarily map it through a throwaway `EditField` so the crate builds before Task 6 rewrites the pane. In `render_rows`, replace `present_field(f)` with an inline build:

```rust
// TEMPORARY shim until Task 6 rewrites this pane to own an EditForm.
let ef = crate::workflows::edit_form::EditField {
    label: f.label.clone(), must: f.is_must, editable: false, multi: false,
    secret: false, ordered: false, orphaned: false, kind: f.kind,
    widget: f.widget.clone(), widget_binding: None,
    values: f.values.clone(), baseline: f.values.clone(),
};
let cell = crate::tui::widget::present_field(&ef);
```

(Use `cell` where `present_field(f)` was. This shim is deleted in Task 6.)

Run: `cargo build -j4 && cargo test -j4 && cargo clippy -j4 --all-targets -- -D warnings`
Expected: builds; tests PASS; clippy clean.

- [ ] **Step 6: Commit**

```bash
git add src/tui/widget.rs src/tui/panes/form.rs
git commit -m "feat(tui): widget activate() + inline_editable gate over EditField (M2 T3)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 4: UiState edit fields + write routing — `tui::state`

Replace the read-only `form: Option<FormModel>` with `edit_form: Option<EditForm>`,
add the write flow + intents + flags, and route write responses in `pump_worker`,
returning a richer `PumpResult` the pump acts on.

**Files:**
- Modify: `src/tui/state.rs`
- Test: inline `#[cfg(test)] mod tests` (add)

**Interfaces:**
- Consumes: `workflows::edit_form::{EditForm, build_edit_form}`, `workflows::write_flow::{WriteFlow, WriteOutcome}`, `workflows::read_flow::ReadOutcome`.
- Produces (on `UiState`): `pub edit_form: Option<EditForm>`, `pub write_flow: WriteFlow`, `pub read_only: bool`, `pub status: String`, `pub form_needs_render: bool`, `pub guard_target: Option<(String, Vec<String>)>`, `pub pending_nav: Option<(String, Vec<String>)>`, `pub last_write_error: Option<String>`; and `pub fn pump_worker(&mut self) -> PumpResult`.
- Produces: `pub struct PumpResult { pub changed: bool, pub quit: bool, pub error: bool }`.

- [ ] **Step 1: Write a failing test for write routing in `pump_worker`**

Add to `src/tui/state.rs` tests (create the module if absent). The test drives a
`Saved` outcome by pre-seeding the write flow through a real `submit` is not
possible without a worker, so test the routing via `on_response` integration:
inject a `WriteOk` by calling `pump_worker` after manually staging a pending write
through a worker-less path. Instead, unit-test the small pure helper
`apply_write_outcome` that `pump_worker` delegates to:

```rust
#[cfg(test)]
mod write_routing_tests {
    use super::*;
    use crate::workflows::write_flow::WriteOutcome;

    fn empty_state() -> UiState {
        use crate::ldap::worker::RawSubschema;
        use crate::workflows::structure::Structure;
        let schema = crate::schema::SchemaModel::from_raw(&RawSubschema::default());
        let structure = Structure::build("dc=x", vec![]);
        UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new())
    }

    #[test]
    fn saved_without_quit_requests_reread_and_sets_status() {
        let mut st = empty_state();
        let res = st.apply_write_outcome(WriteOutcome::Saved {
            reread_dn: "cn=a,dc=x".into(), quit_after: false,
        });
        assert!(res.changed);
        assert!(!res.quit);
        assert_eq!(st.status, "Saved.");
    }

    #[test]
    fn saved_with_quit_sets_quit_flag() {
        let mut st = empty_state();
        let res = st.apply_write_outcome(WriteOutcome::Saved {
            reread_dn: "x".into(), quit_after: true,
        });
        assert!(res.quit);
    }

    #[test]
    fn write_error_sets_error_flag_and_message() {
        let mut st = empty_state();
        let res = st.apply_write_outcome(WriteOutcome::Error("boom".into()));
        assert!(res.error);
        assert_eq!(st.last_write_error.as_deref(), Some("boom"));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -j4 write_routing 2>&1 | head -20`
Expected: FAIL — fields/methods not found.

- [ ] **Step 3: Update `UiState`**

In `src/tui/state.rs`:

Replace the `form: Option<FormModel>` field and `form_dirty` with:

```rust
    /// The loaded editable form (None until a leaf is read).
    pub edit_form: Option<EditForm>,
    /// Async write flow (validate/diff/submit/correlate).
    pub write_flow: WriteFlow,
    /// Read-only mode disables editing and the save path.
    pub read_only: bool,
    /// Transient status text (e.g. "Saved.").
    pub status: String,
    /// True when a pane must re-render the form from `edit_form`.
    pub form_needs_render: bool,
    /// A dirty-blocked navigation awaiting the guard's decision: (dn, objectClasses).
    pub guard_target: Option<(String, Vec<String>)>,
    /// Where to navigate after a guard-Save completes: (dn, objectClasses).
    pub pending_nav: Option<(String, Vec<String>)>,
    /// Last async write error, surfaced by the dispatch closure's Error dialog.
    pub last_write_error: Option<String>,
```

Update imports: add
```rust
use crate::workflows::edit_form::{build_edit_form, EditForm};
use crate::workflows::write_flow::{WriteFlow, WriteOutcome};
```
and drop the now-unused `use crate::workflows::form_model::FormModel;` if present.

Add the `PumpResult` struct near the top:

```rust
/// What `pump_worker` wants the pump view to do after draining responses.
#[derive(Debug, Default, Clone, Copy)]
pub struct PumpResult {
    pub changed: bool,
    pub quit: bool,
    pub error: bool,
}
```

Update `new_for_test` to initialise the new fields (replace the old `form`/`form_dirty` lines):

```rust
            edit_form: None,
            write_flow: WriteFlow::new(),
            read_only: false,
            status: String::new(),
            form_needs_render: false,
            guard_target: None,
            pending_nav: None,
            last_write_error: None,
```

(Do the same in the real `bootstrap` constructor in this file — find every `UiState { .. }` literal and add the new fields; remove `form`/`form_dirty`.)

- [ ] **Step 4: Rewrite `pump_worker` to route reads AND writes**

Replace the existing `pump_worker` body with:

```rust
    /// Drain ready worker responses: install a fresh `EditForm` on a read, and
    /// apply write outcomes. Returns what the pump view should do.
    pub fn pump_worker(&mut self) -> PumpResult {
        use crate::workflows::read_flow::ReadOutcome;
        let mut resps = Vec::new();
        if let Some(w) = self.worker.as_ref() {
            while let Some(r) = w.poll() {
                resps.push(r);
            }
        }
        let mut out = PumpResult::default();
        for resp in &resps {
            // Reads first (Entries/SearchError); disjoint from write variants.
            match self.read_flow.on_response(resp) {
                ReadOutcome::Form { model, object_classes } => {
                    let mut form = build_edit_form(&model, self.read_flow.schema(), self.read_only);
                    form.object_classes = object_classes;
                    self.edit_form = Some(form);
                    self.form_needs_render = true;
                    out.changed = true;
                    continue;
                }
                ReadOutcome::Error(msg) => {
                    self.status = msg.clone();
                    out.changed = true;
                    continue;
                }
                ReadOutcome::Ignored => {}
            }
            // Then writes (WriteOk/WriteError).
            let outcome = self.write_flow.on_response(resp);
            if !matches!(outcome, WriteOutcome::Ignored) {
                let r = self.apply_write_outcome(outcome);
                out.changed |= r.changed;
                out.quit |= r.quit;
                out.error |= r.error;
            }
        }
        out
    }

    /// Apply one non-ignored write outcome to state, returning the pump action.
    pub fn apply_write_outcome(&mut self, outcome: WriteOutcome) -> PumpResult {
        let mut out = PumpResult { changed: true, ..Default::default() };
        match outcome {
            WriteOutcome::Saved { reread_dn, quit_after } => {
                self.status = "Saved.".to_string();
                if quit_after {
                    out.quit = true;
                    return out;
                }
                // Navigate to the guard's target if one is pending, else re-read.
                let (dn, profile_ocs) = self
                    .pending_nav
                    .take()
                    .unwrap_or((reread_dn, Vec::new()));
                self.reread(&dn, &profile_ocs);
            }
            WriteOutcome::NeedFollowupModify { dn, mods, quit_after } => {
                if let Some(w) = self.worker.as_ref() {
                    let _ = self.write_flow.submit_followup(w, &dn, mods, quit_after);
                }
            }
            WriteOutcome::Error(msg) => {
                self.last_write_error = Some(msg);
                out.error = true;
            }
            WriteOutcome::Ignored => out.changed = false,
        }
        out
    }

    /// Submit a base-scope re-read of `dn`, selecting a profile by `ocs`.
    fn reread(&mut self, dn: &str, ocs: &[String]) {
        let Self { worker, read_flow, profiles, current_leaf, .. } = self;
        if let Some(w) = worker.as_ref() {
            let profile = crate::tui::state::profile_for(profiles, ocs);
            if read_flow.request_entry(w, dn, profile).is_ok() {
                *current_leaf = Some(dn.to_string());
            }
        }
    }
```

(If `profile_for` is a free function in this module, call it directly; adjust the
path to match its actual location.)

- [ ] **Step 5: Run tests + build + clippy + fmt**

Run: `cargo build -j4 && cargo test -j4 write_routing && cargo clippy -j4 --all-targets -- -D warnings && cargo fmt --check`
Expected: builds (the form pane still references the old `form`/`form_dirty` — if so, apply the minimal rename in `form.rs` now: `form_dirty` → `form_needs_render`, `form` → `edit_form`, keeping the Task 3 shim; Task 6 rewrites it fully). Tests PASS; clippy clean.

- [ ] **Step 6: Commit**

```bash
git add src/tui/state.rs src/tui/panes/form.rs
git commit -m "feat(tui): UiState edit_form + write_flow routing in pump_worker (M2 T4)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 5: Command constants + pump acts on `PumpResult` — `tui::{mod,pump}`

Define the four app-level command constants (so this commit compiles standalone),
then make `PumpView` broadcast on change, post `SHOW_ERROR` on an async write
error, and post `Command::QUIT` on a deferred-quit. `Context::post` enqueues a
command that survives view routing into the program's `app_commands`, where the
`run_app` closure (Task 8) handles it; `Command::QUIT` is consumed by the built-in
handler and ends the loop.

**Files:**
- Modify: `src/tui/mod.rs` (add the four command constants)
- Modify: `src/tui/pump.rs`
- Test: none new (behaviour covered by Task 9 live test; pump has no headless seam for posts).

**Interfaces:**
- Produces in `tui::mod`: `pub const SAVE`, `REQUEST_QUIT`, `GUARD_NAV`, `SHOW_ERROR` (`tv::Command::custom`).
- Consumes: `tui::state::PumpResult`.

- [ ] **Step 1: Add the command constants**

In `src/tui/mod.rs`, near `pub const REFRESH`:

```rust
/// App-level commands routed to `app::dispatch` via `run_app`.
pub const SAVE: tv::Command = tv::Command::custom("edaptor.save");
pub const REQUEST_QUIT: tv::Command = tv::Command::custom("edaptor.request_quit");
pub const GUARD_NAV: tv::Command = tv::Command::custom("edaptor.guard_nav");
pub const SHOW_ERROR: tv::Command = tv::Command::custom("edaptor.show_error");
```

- [ ] **Step 2: Update `handle_event`**

In `src/tui/pump.rs`, replace the `Event::Timer` block:

```rust
        if matches!(ev, Event::Timer(_)) {
            let r = self.state.borrow_mut().pump_worker();
            if r.changed {
                ctx.broadcast(crate::tui::REFRESH, None);
            }
            if r.error {
                ctx.post(crate::tui::SHOW_ERROR);
            }
            if r.quit {
                ctx.post(tv::Command::QUIT);
            }
        }
```

`tv::Command::QUIT` needs no extra import (qualified via `tv::`).

- [ ] **Step 3: Build + clippy + fmt**

Run: `cargo build -j4 --bin edaptor-tv && cargo clippy -j4 --all-targets -- -D warnings && cargo fmt --check`
Expected: builds (the `run_app` closure is still M1's `|_,_| {}`, so `SHOW_ERROR`
posted before Task 8 is simply unhandled — harmless); clean; clean.

- [ ] **Step 4: Commit**

```bash
git add src/tui/mod.rs src/tui/pump.rs
git commit -m "feat(tui): app command constants + pump posts refresh/error/quit (M2 T5)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 6: Editable form pane — `tui::panes::form`

Rewrite the pane: row 0 is a header (`DN` + ` *` dirty marker), rows 1..N are
fields rendered as a **label column + value `InputLine`**. Editable rows
(`inline_editable`) are enabled; the rest stay `disabled`. Every event syncs each
editable `InputLine`'s text into `edit_form` (so a `SAVE` always sees current
values) and recomputes the header dirty marker.

**Files:**
- Modify: `src/tui/panes/form.rs` (full rewrite; delete the Task 3 shim)
- Test: inline `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `workflows::edit_form::EditForm`, `tui::widget::{present_field, inline_editable}`, `Shared`, `REFRESH`.

- [ ] **Step 1: Write failing tests (headless)**

Replace the `tests` module in `src/tui/panes/form.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{FieldKind, SchemaModel};
    use crate::tui::UiState;
    use crate::workflows::edit_form::{EditField, EditForm, FormMode};
    use crate::workflows::form_model::WidgetSpec;
    use crate::workflows::structure::Structure;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    fn ef(label: &str, val: &str, editable: bool) -> EditField {
        EditField {
            label: label.into(), must: false, editable, multi: false, secret: false,
            ordered: false, orphaned: false, kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText, widget_binding: None,
            values: vec![val.into()], baseline: vec![val.into()],
        }
    }

    fn state_with_form() -> Shared {
        use crate::ldap::worker::RawSubschema;
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let structure = Structure::build("dc=x", vec![]);
        let mut st = UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.edit_form = Some(EditForm {
            dn: "cn=a,dc=x".into(), mode: FormMode::Edit, object_classes: vec![],
            fields: vec![ef("cn", "a", true), ef("creatorsName", "admin", false)],
        });
        st.form_needs_render = true;
        Rc::new(RefCell::new(st))
    }

    fn headless_ctx<'a>(
        out: &'a mut VecDeque<Event>,
        timers: &'a mut tv::timer::TimerQueue,
        deferred: &'a mut Vec<tv::Deferred>,
    ) -> Context<'a> {
        Context::new(out, timers, 0, deferred)
    }

    #[test]
    fn editable_rows_enabled_static_rows_disabled() {
        let shared = state_with_form();
        let mut pane = FormPane::new(Rect::new(0, 0, 80, FORM_ROWS as i32 + 1), shared.clone());
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        let mut ev = Event::Broadcast { command: REFRESH, source: None };
        pane.handle_event(&mut ev, &mut ctx);
        // value row 0 (cn) editable → enabled; value row 1 (creatorsName) disabled.
        assert!(!pane.value_disabled(0));
        assert!(pane.value_disabled(1));
    }

    #[test]
    fn editing_value_inputline_marks_form_dirty() {
        let shared = state_with_form();
        let mut pane = FormPane::new(Rect::new(0, 0, 80, FORM_ROWS as i32 + 1), shared.clone());
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        let mut ev = Event::Broadcast { command: REFRESH, source: None };
        pane.handle_event(&mut ev, &mut ctx);
        // Simulate a committed edit by writing the value InputLine's data directly.
        pane.set_value_text(0, "abc".into());
        let mut ev2 = Event::KeyDown(tv::KeyEvent::from(tv::Key::Char('x')));
        pane.handle_event(&mut ev2, &mut ctx);
        assert!(shared.borrow().edit_form.as_ref().unwrap().is_dirty());
    }

    #[test]
    fn umlaut_edit_roundtrips_graphemes() {
        // Grapheme-correct edit regression (folded from the spike umlaut test):
        // a multibyte value set into the InputLine survives the sync into edit_form.
        let shared = state_with_form();
        let mut pane = FormPane::new(Rect::new(0, 0, 80, FORM_ROWS as i32 + 1), shared.clone());
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        let mut ev = Event::Broadcast { command: REFRESH, source: None };
        pane.handle_event(&mut ev, &mut ctx);
        pane.set_value_text(0, "Müller-Lüdenscheidt".into());
        let mut ev2 = Event::KeyDown(tv::KeyEvent::from(tv::Key::Char('x')));
        pane.handle_event(&mut ev2, &mut ctx);
        let st = shared.borrow();
        assert_eq!(st.edit_form.as_ref().unwrap().fields[0].values, vec!["Müller-Lüdenscheidt".to_string()]);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -j4 -p edaptor panes::form 2>&1 | head -20`
Expected: FAIL — `value_disabled`, `set_value_text`, header behaviour missing.

- [ ] **Step 3: Rewrite the pane**

Replace the whole non-test part of `src/tui/panes/form.rs`:

```rust
//! Editable entry form pane: a header row (DN + dirty marker) over per-field rows,
//! each a static label column + a value `InputLine`. Plain single-value fields are
//! editable; the rest stay disabled (read-only). On every event the editable
//! `InputLine`s are synced into the shared `EditForm` so a `SAVE` sees current
//! values, and the header's dirty marker is refreshed.

use tvision_rs::{
    self as tv, delegate, Context, Event, FieldValue, Group, InputLine, Rect, View,
};

use crate::tui::widget::{inline_editable, present_field};
use crate::tui::{Shared, REFRESH};
use crate::workflows::edit_form::EditForm;

const FORM_ROWS: usize = 32;
/// Columns reserved for the label before the value `InputLine`.
const LABEL_W: i32 = 22;

/// A disabled (read-only, skip-focus) `InputLine` used for header/label cells.
/// `StaticText` has no `set_value`, so we reuse the M1 disabled-InputLine idiom
/// for any cell whose text we update at render time.
fn ro_cell(bounds: Rect) -> InputLine {
    let mut il = InputLine::with_limit(bounds, 1024);
    il.state.state.disabled = true;
    il
}

pub(crate) struct FormPane {
    group: Group,
    header_id: tv::ViewId,
    /// Per field row: the value `InputLine` id (label is a disabled InputLine).
    value_ids: Vec<tv::ViewId>,
    label_ids: Vec<tv::ViewId>,
    state: Shared,
}

/// `"DN"` plus a ` *` marker when dirty.
fn header_text(form: &EditForm) -> String {
    let mark = if form.is_dirty() { " *" } else { "" };
    format!("{}{}", form.dn, mark)
}

impl FormPane {
    pub(crate) fn new(bounds: Rect, state: Shared) -> Self {
        let mut group = Group::new(bounds);
        let w = bounds.b.x - bounds.a.x;

        // Row 0: header (read-only cell).
        let header_id = group.insert(Box::new(ro_cell(Rect::new(0, 0, w, 1))));

        let mut value_ids = Vec::new();
        let mut label_ids = Vec::new();
        for i in 0..FORM_ROWS {
            let y = i as i32 + 1; // rows start below the header
            label_ids.push(group.insert(Box::new(ro_cell(Rect::new(0, y, LABEL_W, y + 1)))));
            let mut il = InputLine::with_limit(Rect::new(LABEL_W, y, w, y + 1), 1024);
            il.state.state.disabled = true; // default read-only; refresh enables editable rows
            value_ids.push(group.insert(Box::new(il)));
        }
        FormPane { group, header_id, value_ids, label_ids, state }
    }

    /// Test seam: is the value InputLine for field `i` disabled?
    #[cfg(test)]
    pub(crate) fn value_disabled(&mut self, i: usize) -> bool {
        self.group
            .child_mut(self.value_ids[i])
            .map(|c| c.state().state.disabled)
            .unwrap_or(true)
    }

    /// Test seam: set the value InputLine text for field `i`.
    #[cfg(test)]
    pub(crate) fn set_value_text(&mut self, i: usize, text: String) {
        if let Some(c) = self.group.child_mut(self.value_ids[i]) {
            c.set_value(FieldValue::Text(text));
        }
    }

    /// Repaint header + all rows from `edit_form`.
    fn render(&mut self, ctx: &mut Context) {
        let _ = ctx;
        let (header, rows): (String, Vec<(String, String, bool)>) = {
            let st = self.state.borrow();
            match st.edit_form.as_ref() {
                None => (String::new(), Vec::new()),
                Some(form) => {
                    let header = header_text(form);
                    let rows = form
                        .fields
                        .iter()
                        .map(|f| {
                            let marker = if f.must { "*" } else { "" };
                            let label = format!("{}{}", f.label, marker);
                            (label, present_field(f), inline_editable(f))
                        })
                        .collect();
                    (header, rows)
                }
            }
        }; // borrow dropped

        if let Some(h) = self.group.child_mut(self.header_id) {
            h.set_value(FieldValue::Text(header));
        }
        for i in 0..FORM_ROWS {
            let (label, value, editable) = rows
                .get(i)
                .cloned()
                .unwrap_or_else(|| (String::new(), String::new(), false));
            if let Some(l) = self.group.child_mut(self.label_ids[i]) {
                l.set_value(FieldValue::Text(label));
            }
            if let Some(v) = self.group.child_mut(self.value_ids[i]) {
                v.set_value(FieldValue::Text(value));
                v.state_mut().state.disabled = !editable;
            }
        }
    }

    /// Sync each editable value InputLine's text into `edit_form`; refresh header.
    fn sync_into_form(&mut self) {
        // Collect (idx, text) for editable rows, then apply under one mut borrow.
        let editable: Vec<usize> = {
            let st = self.state.borrow();
            match st.edit_form.as_ref() {
                None => Vec::new(),
                Some(form) => form
                    .fields
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| inline_editable(f))
                    .map(|(i, _)| i)
                    .collect(),
            }
        };
        let mut edits: Vec<(usize, String)> = Vec::new();
        for &i in &editable {
            if let Some(v) = self.group.child_mut(self.value_ids[i]).and_then(|v| v.value()) {
                if let FieldValue::Text(s) = v {
                    edits.push((i, s));
                }
            }
        }
        let header = {
            let mut st = self.state.borrow_mut();
            if let Some(form) = st.edit_form.as_mut() {
                for (i, s) in edits {
                    if form.fields.get(i).map(|f| f.values.first().map(String::as_str)) != Some(Some(s.as_str())) {
                        form.set_value(i, s);
                    }
                }
                Some(header_text(form))
            } else {
                None
            }
        };
        if let (Some(text), Some(h)) = (header, self.group.child_mut(self.header_id)) {
            h.set_value(FieldValue::Text(text));
        }
    }
}

#[delegate(to = group)]
impl View for FormPane {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        // Render whenever the form needs it, on ANY event. The dispatch closure
        // (Discard, re-read) only sets `form_needs_render` — it cannot broadcast
        // REFRESH (Program has no broadcast) — and the 50ms pump timer reaches
        // this view, so a flagged re-render repaints within one tick.
        if self.state.borrow().form_needs_render {
            self.state.borrow_mut().form_needs_render = false;
            self.render(ctx);
        }
        let _ = REFRESH; // (REFRESH still drives other panes; retained import)
        self.group.handle_event(ev, ctx);
        // Keep edit_form current with the on-screen editors.
        self.sync_into_form();
    }
}
```

Delete the Task 3 temporary shim (this rewrite replaces `render_rows`). Drop the
`let _ = REFRESH;` line and the `REFRESH` import if nothing else in the file uses
it after the rewrite (it is only kept to avoid an unused-import error if present).

- [ ] **Step 4: Run tests + build + clippy + fmt**

Run: `cargo test -j4 panes::form && cargo build -j4 --bin edaptor-tv && cargo clippy -j4 --all-targets -- -D warnings && cargo fmt --check`
Expected: PASS / builds / clean / clean.

- [ ] **Step 5: Commit**

```bash
git add src/tui/panes/form.rs
git commit -m "feat(tui): editable form pane (header + label column + value InputLine) (M2 T6)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 7: Dialogs — `tui::dialog::{mod,confirm,error,guard}`

Builder functions returning `Box<dyn View>` modal dialogs, plus a PURE
`guard_decision()` mapping the modal's returned `Command` to an action. Buttons use
the modal-exit commands (`OK`/`CANCEL`/`YES`/`NO`) so `exec_view` returns them.

**Files:**
- Create: `src/tui/dialog/mod.rs`, `confirm.rs`, `error.rs`, `guard.rs`
- Modify: `src/tui/mod.rs` (add `mod dialog;`)
- Test: inline tests in `mod.rs` for `guard_decision`

**Interfaces:**
- Produces:
  - `confirm::build(ldif: &str) -> Box<dyn View>` (Save = `Command::OK`, Cancel = `Command::CANCEL`)
  - `error::build(text: &str) -> Box<dyn View>` (Dismiss = `Command::OK`)
  - `guard::build() -> Box<dyn View>` (Save = `Command::YES`, Discard = `Command::NO`, Stay = `Command::CANCEL`)
  - `pub enum GuardDecision { Save, Discard, Stay }`
  - `pub fn guard_decision(answer: Command) -> GuardDecision`

- [ ] **Step 1: Write failing test for `guard_decision`**

In `src/tui/dialog/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tvision_rs::Command;

    #[test]
    fn yes_is_save_no_is_discard_else_stay() {
        assert_eq!(guard_decision(Command::YES), GuardDecision::Save);
        assert_eq!(guard_decision(Command::NO), GuardDecision::Discard);
        assert_eq!(guard_decision(Command::CANCEL), GuardDecision::Stay);
        assert_eq!(guard_decision(Command::custom("whatever")), GuardDecision::Stay);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -j4 guard_decision 2>&1 | head`
Expected: FAIL — module/symbols missing.

- [ ] **Step 3: Implement `mod.rs`**

`src/tui/dialog/mod.rs`:

```rust
//! Modal dialogs for the edit/write spine. Builders return `Box<dyn View>` run via
//! `Program::exec_view`; buttons use the modal-exit commands so `exec_view` returns
//! which was pressed. All `exec_view` calls live in `tui::app::dispatch`.

pub(crate) mod confirm;
pub(crate) mod error;
pub(crate) mod guard;

use tvision_rs::Command;

/// The user's answer to the dirty guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GuardDecision {
    Save,
    Discard,
    Stay,
}

/// Map a guard dialog's returned command to a decision. `YES`=Save, `NO`=Discard,
/// anything else (incl. `CANCEL` / window close) = Stay (the safe default).
pub(crate) fn guard_decision(answer: Command) -> GuardDecision {
    if answer == Command::YES {
        GuardDecision::Save
    } else if answer == Command::NO {
        GuardDecision::Discard
    } else {
        GuardDecision::Stay
    }
}
```

- [ ] **Step 4: Implement the three builders**

`src/tui/dialog/confirm.rs`:

```rust
//! Save-confirmation dialog: shows the (secret-masked) LDIF preview with Save/Cancel.

use tvision_rs::{
    Button, ButtonFlags, ButtonRowAlign, Command, Dialog, Rect, StaticText, View,
};

/// Build the confirm dialog. Returns `Command::OK` (Save) or `Command::CANCEL`.
pub(crate) fn build(ldif: &str) -> Box<dyn View> {
    let mut dlg = Dialog::new(Rect::new(0, 0, 70, 20), Some("Confirm save".to_string()));
    dlg.state_mut().options.center_x = true;
    dlg.state_mut().options.center_y = true;
    dlg.insert_child(Box::new(StaticText::new(Rect::new(2, 2, 68, 16), ldif.to_string())));
    dlg.button_row(
        &[
            ("~S~ave", Command::OK, ButtonFlags { default: true, ..ButtonFlags::new() }),
            ("~C~ancel", Command::CANCEL, ButtonFlags::new()),
        ],
        ButtonRowAlign::Right,
    );
    Box::new(dlg)
}
```

`src/tui/dialog/error.rs`:

```rust
//! Dismissible error dialog.

use tvision_rs::{
    Button, ButtonFlags, ButtonRowAlign, Command, Dialog, Rect, StaticText, View,
};

/// Build the error dialog. Returns `Command::OK` on dismiss.
pub(crate) fn build(text: &str) -> Box<dyn View> {
    let mut dlg = Dialog::new(Rect::new(0, 0, 60, 12), Some("Error".to_string()));
    dlg.state_mut().options.center_x = true;
    dlg.state_mut().options.center_y = true;
    dlg.insert_child(Box::new(StaticText::new(Rect::new(2, 2, 58, 9), text.to_string())));
    dlg.button_row(
        &[("~O~K", Command::OK, ButtonFlags { default: true, ..ButtonFlags::new() })],
        ButtonRowAlign::Center,
    );
    Box::new(dlg)
}
```

`src/tui/dialog/guard.rs`:

```rust
//! Dirty-guard dialog: Save / Discard / Stay over an unsaved form.

use tvision_rs::{
    Button, ButtonFlags, ButtonRowAlign, Command, Dialog, Rect, StaticText, View,
};

/// Build the guard dialog. Returns `Command::YES` (Save), `Command::NO` (Discard),
/// or `Command::CANCEL` (Stay).
pub(crate) fn build() -> Box<dyn View> {
    let mut dlg = Dialog::new(Rect::new(0, 0, 56, 9), Some("Unsaved changes".to_string()));
    dlg.state_mut().options.center_x = true;
    dlg.state_mut().options.center_y = true;
    dlg.insert_child(Box::new(StaticText::new(
        Rect::new(2, 2, 54, 4),
        "This entry has unsaved changes.".to_string(),
    )));
    dlg.button_row(
        &[
            ("~S~ave", Command::YES, ButtonFlags { default: true, ..ButtonFlags::new() }),
            ("~D~iscard", Command::NO, ButtonFlags::new()),
            ("S~t~ay", Command::CANCEL, ButtonFlags::new()),
        ],
        ButtonRowAlign::Right,
    );
    Box::new(dlg)
}
```

(Each `use ... Button` may be unused if `button_row` constructs buttons internally;
drop the `Button` import if clippy flags it. Verify `dlg.state_mut().options.center_x`
matches the tvdemo example field path — if the field is named differently in
0.1.2, use the exact path shown in `examples/tvdemo.rs:1556`.)

- [ ] **Step 5: Register + test + clippy + fmt**

In `src/tui/mod.rs` add `mod dialog;` (near the other `mod` lines).

Run: `cargo test -j4 guard_decision && cargo build -j4 --bin edaptor-tv && cargo clippy -j4 --all-targets -- -D warnings && cargo fmt --check`
Expected: PASS / builds / clean / clean.

- [ ] **Step 6: Commit**

```bash
git add src/tui/dialog src/tui/mod.rs
git commit -m "feat(tui): confirm/error/guard dialog builders + guard_decision (M2 T7)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 8: Dispatch + menu/status + dirty-nav interception — `tui::{app,mod,panes::leaf}`

Wire the single `run_app` dispatch closure (the only place that calls `exec_view`),
add a Save action and route Exit through `REQUEST_QUIT`, and make the leaf pane
intercept a dirty navigation by posting `GUARD_NAV` with the target stashed.

**Files:**
- Modify: `src/tui/mod.rs` (command constants; wire `run_app`)
- Modify: `src/tui/app.rs` (menu/status; `dispatch`)
- Modify: `src/tui/panes/leaf.rs` (dirty-nav interception)
- Modify: `src/tui/pump.rs` (fold its commit here if deferred from Task 5)
- Test: inline test in `app.rs` for the pure `save_flow_action` helper

**Interfaces:**
- Consumes from `tui::mod`: `SAVE`, `REQUEST_QUIT`, `GUARD_NAV`, `SHOW_ERROR` (defined in Task 5).
- Produces in `tui::app`: `pub(crate) fn dispatch(prog: &mut Program, cmd: Command, state: &Shared)`.

- [ ] **Step 1: Wire the dispatch closure in `run`**

In `src/tui/mod.rs::run`, replace `program.run_app(|_prog, _cmd| {});` with:

```rust
    let dispatch_state = state.clone();
    program.run_app(move |prog, cmd| app::dispatch(prog, cmd, &dispatch_state));
```

Make `app` items reachable: ensure `mod app;` exposes `dispatch` (`pub(crate) fn`).

- [ ] **Step 2: Write a failing test for the pure save-flow decision**

In `src/tui/app.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflows::save::PrepareSave;
    use crate::form::validate::ValidationError;

    #[test]
    fn save_flow_action_classifies_prepare() {
        assert!(matches!(save_flow_action(&PrepareSave::NoChanges), SaveAction::Status(_)));
        assert!(matches!(
            save_flow_action(&PrepareSave::Invalid(vec![ValidationError::MissingMust("cn".into())])),
            SaveAction::Error(_)
        ));
        assert!(matches!(
            save_flow_action(&PrepareSave::DiffError("bad".into())),
            SaveAction::Error(_)
        ));
        let ready = PrepareSave::Ready { plan: crate::form::validate::SavePlan::Nothing, dn: "d".into(), ldif: "L".into() };
        assert!(matches!(save_flow_action(&ready), SaveAction::Confirm(_)));
    }
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -j4 save_flow_action 2>&1 | head`
Expected: FAIL — `save_flow_action`/`SaveAction` missing.

- [ ] **Step 4: Implement menu/status + dispatch**

In `src/tui/app.rs`:

Add imports:
```rust
use tvision_rs::Command;
use crate::tui::dialog::{confirm, error, guard, guard_decision, GuardDecision};
use crate::tui::{GUARD_NAV, REQUEST_QUIT, SAVE, SHOW_ERROR};
use crate::form::validate::format_validation_errors;
use crate::workflows::save::PrepareSave;
```

Bind a Save item and route Exit through `REQUEST_QUIT`. In `init_status_line`, add a Save hint; in `init_menu_bar`, add Save + change Exit's command. Replace the two init fns' relevant lines:

```rust
fn init_status_line(r: Rect) -> Option<Box<dyn View>> {
    let mut r = r;
    r.a.y = r.b.y - 1;
    let defs = StatusDef::list()
        .def_all(|d| {
            d.item("~Alt-S~ Save", alt('s'), SAVE)
                .item("~Alt-X~ Exit", alt('x'), REQUEST_QUIT)
        })
        .build();
    Some(Box::new(StatusLine::new(r, defs)))
}

fn init_menu_bar(r: Rect) -> Option<Box<dyn View>> {
    let mut r = r;
    r.b.y = r.a.y + 1;
    let menu = tv::Menu::builder()
        .submenu("~F~ile", alt('f'), |m| {
            m.command_key("~S~ave", SAVE, alt('s'), "Alt-S")
                .command_key("E~x~it", REQUEST_QUIT, alt('x'), "Alt-X")
        })
        .build();
    Some(Box::new(tv::MenuBar::new(r, menu)))
}
```

(Confirm the `StatusDef`/`Menu` builder method names against M1's existing code;
keep them identical to what compiles there. The only change is the bound command
and the added Save item.)

Add the pure helper + dispatch:

```rust
/// What the save flow should do for a given prepare result.
pub(crate) enum SaveAction {
    Status(String),
    Error(String),
    Confirm(String), // the LDIF to preview
}

/// Pure classification of a `PrepareSave` into a dispatch action.
pub(crate) fn save_flow_action(prepared: &PrepareSave) -> SaveAction {
    match prepared {
        PrepareSave::NoChanges => SaveAction::Status("No changes.".to_string()),
        PrepareSave::Invalid(errs) => SaveAction::Error(format_validation_errors(errs)),
        PrepareSave::DiffError(e) => SaveAction::Error(e.clone()),
        PrepareSave::Ready { ldif, .. } => SaveAction::Confirm(ldif.clone()),
    }
}

/// The single seam that opens modal dialogs (has `&mut Program`). Triggered by
/// commands posted from panes / the pump.
pub(crate) fn dispatch(prog: &mut Program, cmd: Command, state: &Shared) {
    if cmd == SAVE {
        do_save(prog, state, None, false);
    } else if cmd == GUARD_NAV {
        // A dirty-blocked navigation: ask, then act on the stashed target.
        let target = state.borrow().guard_target.clone();
        match run_guard(prog) {
            GuardDecision::Save => do_save(prog, state, target, false),
            GuardDecision::Discard => {
                // discard_edits sets form_needs_render; the re-read's worker
                // response drives a REFRESH via the pump — no Program broadcast.
                discard_edits(state);
                if let Some((dn, ocs)) = target {
                    state.borrow_mut().reread_public(&dn, &ocs);
                }
                state.borrow_mut().guard_target = None;
            }
            GuardDecision::Stay => {
                state.borrow_mut().guard_target = None;
            }
        }
    } else if cmd == REQUEST_QUIT {
        let dirty = state
            .borrow()
            .edit_form
            .as_ref()
            .map(|f| f.is_dirty())
            .unwrap_or(false);
        if !dirty {
            prog.end_modal(Command::QUIT); // sets end_state → run loop ends
            return;
        }
        match run_guard(prog) {
            GuardDecision::Save => do_save(prog, state, None, true),
            GuardDecision::Discard => prog.end_modal(Command::QUIT),
            GuardDecision::Stay => {}
        }
    } else if cmd == SHOW_ERROR {
        let msg = state.borrow_mut().last_write_error.take();
        if let Some(msg) = msg {
            prog.exec_view(error::build(&msg));
        }
    }
}

/// Run the guard modal and decode the answer. (`exec_view` re-enters the loop; the
/// pump keeps draining, so an in-flight write still completes.)
fn run_guard(prog: &mut Program) -> GuardDecision {
    let answer = prog.exec_view(guard::build());
    guard_decision(answer)
}

/// Prepare → (Status | Error | Confirm→submit). `nav` is a post-save navigation
/// target (guard-nav case); `quit_after` defers a quit until the write lands.
fn do_save(prog: &mut Program, state: &Shared, nav: Option<(String, Vec<String>)>, quit_after: bool) {
    // 1. Prepare (borrow, compute, drop borrow before any exec_view / submit).
    let prepared = {
        let st = state.borrow();
        match st.edit_form.as_ref() {
            None => return,
            Some(form) => st.write_flow.prepare(form, st.read_flow.schema()),
        }
    };
    match save_flow_action(&prepared) {
        SaveAction::Status(s) => {
            let mut st = state.borrow_mut();
            st.status = s;
            st.guard_target = None;
            st.form_needs_render = true; // repaints on the next pump tick
        }
        SaveAction::Error(text) => {
            prog.exec_view(error::build(&text));
        }
        SaveAction::Confirm(ldif) => {
            if prog.exec_view(confirm::build(&ldif)) != Command::OK {
                return; // Cancel: keep editing.
            }
            // 2. Submit the plan we prepared. Re-extract Ready for the plan/dn.
            if let PrepareSave::Ready { plan, dn, .. } = prepared {
                let mut st = state.borrow_mut();
                st.pending_nav = nav;
                st.guard_target = None;
                let crate::tui::state::UiState { worker, write_flow, .. } = &mut *st;
                if let Some(w) = worker.as_ref() {
                    let _ = write_flow.submit(w, plan, &dn, quit_after);
                }
            }
        }
    }
}

/// Reset every field's edited values back to baseline (drop unsaved edits).
fn discard_edits(state: &Shared) {
    let mut st = state.borrow_mut();
    if let Some(form) = st.edit_form.as_mut() {
        for f in &mut form.fields {
            f.values = f.baseline.clone();
        }
    }
    st.form_needs_render = true;
}
```

Add a small public re-read shim on `UiState` (in `state.rs`) so `dispatch` can
trigger a re-read after Discard:

```rust
    /// Public wrapper around the private `reread` for the dispatch closure.
    pub fn reread_public(&mut self, dn: &str, ocs: &[String]) {
        self.reread(dn, ocs);
    }
```

**Quit mechanism (verified, tvision 0.1.2):** `Program` has **no** `post`/`broadcast`.
The closure ends the app by `prog.end_modal(Command::QUIT)` — `end_modal(cmd)` sets
`self.end_state = Some(cmd)` (`program.rs:760`), which breaks `run_app`'s inner
`while self.end_state.is_none()` loop and ends the program (after `valid_end`).
The **deferred-quit** (a `quit_after` write completing later) is posted by the pump
view via `ctx.post(Command::QUIT)` (Task 5) — views *do* have `Context::post`, and
the built-in QUIT handler consumes it to set `end_state`. Repaints after a
closure-only state change use `form_needs_render` (Task 6 renders on any event), not
a broadcast.

- [ ] **Step 5: Leaf pane dirty-nav interception**

In `src/tui/panes/leaf.rs::submit_selected`, before issuing the read, check dirty
and divert to the guard. Replace the final block (`let mut st = ...; ... request_entry ...`):

```rust
        let dirty = {
            let st = self.state.borrow();
            st.edit_form.as_ref().map(|f| f.is_dirty()).unwrap_or(false)
        };
        if dirty {
            // Divert: stash the target and ask the dispatch closure to guard.
            {
                let mut st = self.state.borrow_mut();
                st.guard_target = Some((dn.clone(), ocs.clone()));
            }
            ctx_post_guard(self);
            return;
        }

        let mut st = self.state.borrow_mut();
        if st.current_leaf.as_deref() == Some(dn.as_str()) {
            return;
        }
        let crate::tui::state::UiState { worker, read_flow, profiles, current_leaf, .. } = &mut *st;
        if let Some(w) = worker.as_ref() {
            let profile = profile_for(profiles, &ocs);
            if read_flow.request_entry(w, &dn, profile).is_ok() {
                *current_leaf = Some(dn);
            }
        }
```

`submit_selected` has no `ctx`. Thread the `&mut Context` into it: change its
signature to `fn submit_selected(&mut self, ctx: &mut Context)` and at the call
site (in `handle_event`) pass `ctx`. Replace `ctx_post_guard(self)` with
`ctx.post(crate::tui::GUARD_NAV);` directly (drop the helper). Add
`use crate::tui::GUARD_NAV;` if convenient or use the full path.

- [ ] **Step 6: Build + tests + clippy + fmt**

Run: `cargo build -j4 --bin edaptor-tv && cargo test -j4 && cargo clippy -j4 --all-targets -- -D warnings && cargo fmt --check`
Expected: builds / PASS / clean / clean. Run the facade guards (must print nothing):
```bash
! grep -rl "use tvision_rs" src | grep -vE "^src/(tui/|bin/edaptor-tv.rs)"
! grep -rl "use ratatui\|use tui_" src | grep -vE "^src/ui/"
```

- [ ] **Step 7: Commit**

```bash
git add src/tui/mod.rs src/tui/app.rs src/tui/panes/leaf.rs src/tui/state.rs
git commit -m "feat(tui): save/guard/quit dispatch via run_app; dirty-nav guard (M2 T8)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 9: Live integration test + docs

Add a gated end-to-end test that edits + persists a real entry and renames one
(MODRDN), and a CHANGES.md entry.

**Files:**
- Create: `tests/tv_edit_write.rs`
- Modify: `CHANGES.md`

**Interfaces:**
- Consumes: `edaptor::workflows::{edit_form, write_flow, read_flow}`, `edaptor::ldap::worker::WorkerHandle`, `edaptor::config`. (Confirm the crate's public re-export path used by existing tests under `tests/`.)

- [ ] **Step 1: Write the gated integration test**

`tests/tv_edit_write.rs` — model it on the existing read integration test (find one
under `tests/` that connects via `EDAPTOR_TEST_LDAP_URI`; reuse its connect/bootstrap
helper). The test skips when `EDAPTOR_TEST_LDAP_URI` is unset.

```rust
//! Live edit+write path (skipped unless EDAPTOR_TEST_LDAP_URI is set).
//! Mirrors the read integration test's connection bootstrap.

#[test]
fn edit_persists_and_reads_back() {
    let Ok(_uri) = std::env::var("EDAPTOR_TEST_LDAP_URI") else {
        eprintln!("skipping: EDAPTOR_TEST_LDAP_URI unset");
        return;
    };
    // 1. Connect a worker, read a known entry into an EditForm.
    // 2. edit_form.set_value(idx_of("description"), "edaptor-m2-test");
    // 3. write_flow.prepare(...) -> Ready; write_flow.submit(worker, plan, dn, false).
    // 4. Poll until WriteOk; on_response -> Saved; re-read; assert description updated.
    // (Fill in using the existing test's helpers for connect + poll.)
    todo!("implement against the shared live-test harness");
}
```

Because this requires the shared live harness, the implementer wires it to the
existing helper module (do NOT invent a new connection path). If no reusable
harness exists, gate-and-skip is acceptable for CI; the human acceptance run
(below) is the real gate.

- [ ] **Step 2: Verify it compiles and skips cleanly**

Run: `cargo test -j4 --test tv_edit_write`
Expected: prints the skip line and passes (env unset in CI).

- [ ] **Step 3: CHANGES.md entry**

Under the current unreleased section in `CHANGES.md`, add:

```markdown
- tvision UI (preview, `edaptor-tv`): the entry form is now editable for plain
  single-value attributes, with an LDIF-preview save confirmation, async writes
  (MODIFY + rename/MODRDN), and dirty-change guards on navigation and quit.
```

- [ ] **Step 4: Commit**

```bash
git add tests/tv_edit_write.rs CHANGES.md
git commit -m "test(tui): gated live edit+write integration; changelog (M2 T9)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

- [ ] **Step 5: Human interactive acceptance (no TTY in agent sessions)**

A human runs, against the podman demo server:
```bash
scripts/test-ldap.sh start
export EDAPTOR_TEST_ADMIN_PW=adminpassword
cargo run --bin edaptor-tv -- --config examples/demo-config.toml
```
Confirm, per umbrella M2 accept:
- Navigate DIT → leaf → form; edit a plain attribute (e.g. `description`), Alt-S → LDIF preview is correct → Save persists; the form re-reads with the new value.
- Edit `cn` (RDN) and save → the entry is renamed (MODRDN) and the form follows the new DN.
- With unsaved edits, selecting another leaf raises the guard (Save/Discard/Stay); Stay keeps editing, Discard reverts, Save persists then navigates.
- With unsaved edits, Alt-X raises the guard; Save defers the quit until the write lands; Discard quits; Stay cancels.
- A umlaut value (e.g. `Müller`) edits and persists correctly.
- An invalid edit (clear a MUST attribute) surfaces the Error dialog and does not write.

---

## Self-Review

**Spec coverage:**
- Neutral editable model (spec §3) → Task 1. ✓
- Form pane rewrite, label column + value InputLine, editability gate (spec §4) → Tasks 3 (gate), 6 (pane). ✓
- FieldWidget `activate` + registry seam (spec §5) → Task 3 (`activate`, `inline_editable`; the minimal "registry" is the `PlainWidget`/`inline_editable` dispatch — a full `WidgetKind`→plugin map is only needed when M4 adds non-plain `activate`s, noted in §5 as minimal in M2). ✓
- WriteFlow + pump correlation + MODIFY/MODRDN + post-write re-read (spec §6) → Tasks 2, 4, 5. ✓
- Confirm/Error/Guard via exec_view (spec §7) → Tasks 7, 8. ✓
- Dirty tracking + nav/quit guards + deferred quit (spec §8) → Tasks 4 (dirty), 6 (header marker), 8 (guards + deferred quit). Focus-switch guard deferred to M3 per spec §8. ✓
- Testing (spec §10): headless edit_form/write_flow/widget/pane + umlaut → Tasks 1,2,3,6; live gated → Task 9; interactive → Task 9 Step 5. ✓

**Placeholder scan:** Task 9 Step 1 intentionally leaves a `todo!()` keyed to the
shared live harness (the only project-specific unknown — the existing live-test
connection helper). Flagged explicitly; not a silent gap. All other steps carry
full code.

**Type consistency:** `present_field(&EditField)` (T3) consumed by T6; `inline_editable`
(T3) used in T6/leaf; `WriteOutcome`/`WriteFlow` (T2) consumed by T4; `PumpResult`
(T4) consumed by T5; `SaveAction`/`save_flow_action` (T8) tested in T8; `guard_decision`/
`GuardDecision` (T7) consumed by T8; `reread`/`reread_public` (T4/T8) consistent.

**Resolved API facts (verified against tvision 0.1.2 source — already baked into
the code above, NOT open questions):**
- `StaticText` has only `new`/`text`/`set_text` (no `set_value`) — so the form
  pane's dynamic header/label cells use disabled `InputLine`s (`ro_cell`, Task 6);
  `StaticText` is used only for the dialogs' static text (Task 7).
- `Program` has no `post`/`broadcast`; the closure quits via
  `prog.end_modal(Command::QUIT)` (sets `end_state`, `program.rs:760`), and the
  pump posts deferred-quit/error via `ctx.post` (Task 5/8).
- `Dialog::state_mut().options.center_x/center_y` is valid (`examples/tvdemo.rs:556`).
- Custom commands posted via `ctx.post` reach the `run_app(|prog,cmd|)` closure
  through `app_commands` (`program.rs:894`); `QUIT` is consumed by the built-in
  handler and never reaches the closure — hence the custom `REQUEST_QUIT`.

- `StatusItemsBuilder::item` (`status/mod.rs:262`) and `Menu`'s `command_key`
  (`menu/mod.rs:401`) both take `self` and return `Self`, so the chained
  `.item(..).item(..)` / `.command_key(..).command_key(..)` in Task 8 is valid.

No open API questions remain; every code block above is written against verified
0.1.2 signatures.
