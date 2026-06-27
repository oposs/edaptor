# M3 Phase 2a — objectClass widget + live schema resync — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a user edit an existing entry's `objectClass` via a schema-seeded
multi-select picker, and have the form's fields add/orphan **live** in response —
driven by a typed `SetValuesThenResyncSchema` outcome, with no global resync flag.

**Architecture:** Port the ratatui `sync_schema_fields` into the neutral
`workflows::edit_form::EditForm`. Add the first reusable modal-widget seam
(`Activation::Modal(Box<dyn FieldEditor>)` + a generic dispatch in `app::dispatch`).
The objectClass picker is the first `FieldEditor`: it builds a tvision `Dialog`
(search + ticked `ListBox`) that keeps a prospective `CommitOutcome` in
`UiState::staged_commit`; `dispatch` applies it on an `OK` return. The form pane
detects `Enter` on a modal-activatable row and posts `ACTIVATE` (controller-owned,
mirroring the guard pattern).

**Tech Stack:** Rust, tvision-rs 0.3.0, the existing `workflows`/`schema` neutral
layers. Headless tvision view tests via `Context::new`; gated live LDAP test.

## Global Constraints

- **Scope = Phase 2a only.** No create flow (`FormMode::Create`, Alt+N chooser,
  auto-inject) — that is Phase 2b. No M4 rich widgets.
- **Facade boundary:** only `src/tui/**` may `use tvision_rs`; the neutral
  `workflows::edit_form` imports no UI crate. Both facade guards must stay clean:
  `! grep -rl "use tvision_rs" src | grep -vE "^src/(tui/|bin/edaptor-tv.rs)"` and
  `! grep -rl "use ratatui\|use tui_" src | grep -vE "^src/ui/"`.
- **Do NOT touch `src/ui/**` (ratatui).** It is the reference, not a target.
- **Match ratatui parity** for objectClass behaviour: any class tick/untick allowed
  (server validates on save); removing a class orphans now-disallowed fields (sink
  to bottom, non-editable, dropped on save); adding injects empty fields. No
  structural-class lock, no data-loss warning.
- **Borrow discipline:** never hold a `RefCell`/`UiState` borrow across
  `exec_view`/`exec_view_focused`/`ctx.post`/`ctx.broadcast`/`new_list`/`child_mut`/
  `set_value`/`focus_child`. Collect into locals, drop the borrow, then call.
- **Cap build/test parallelism at 4 cores:** `cargo … -j4`. Target dir is
  `/home/oetiker/scratch/cargo-target` (the `edaptor-tv` binary lives there).
- **Strict TDD**, atomic commits, crate compiles + `cargo fmt` + clippy
  `--all-targets -j4 -- -D warnings` clean after every commit.
- **Commit trailer:** `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
  Use `git commit -F <file>`/heredoc for messages containing backticks.
- **Live tests gated** by `EDAPTOR_TEST_LDAP_URI` (skip when unset). Base
  `dc=example,dc=org`, `EDAPTOR_TEST_ADMIN_PW=adminpassword`.

## File structure

| File | Change | Responsibility |
|---|---|---|
| `src/workflows/edit_form.rs` | modify | Add `EditField::injected`, `order_fields`, `EditForm::sync_schema_fields` (neutral port). |
| `src/tui/widget.rs` | modify | `Activation::Modal(Box<dyn FieldEditor>)`, `trait FieldEditor`, drop non-derivable derives, fix tests. |
| `src/tui/state.rs` | modify | `activate_field`, `staged_commit` fields; `apply_commit` method. |
| `src/tui/oc_picker.rs` | **create** | `ObjectClassWidget` (`FieldWidget`), `ObjectClassEditor` (`FieldEditor`), the picker `Dialog` view; `widget_for` registry. |
| `src/tui/mod.rs` | modify | Declare `mod oc_picker`; add `ACTIVATE` command constant. |
| `src/tui/panes/form.rs` | modify | Focusable = inline OR modal; enable modal cells; nav includes them; `Enter`→`ACTIVATE`; swallow edit keys on modal rows. |
| `src/tui/app.rs` | modify | Handle `ACTIVATE`: build editor, run modal, apply staged `CommitOutcome`. |
| `tests/tv_objectclass.rs` | **create** | Gated live test: edit objectClass on a demo entry → fields add/orphan. |
| `CHANGES.md` | modify | User-visible entry. |

---

### Task 1: Neutral `order_fields` + `EditField::injected`

**Files:**
- Modify: `src/workflows/edit_form.rs`
- Test: `src/workflows/edit_form.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `EditField`, `EditForm`, `SchemaModel`, `WidgetSpec`, `FieldKind` (all already imported in the file).
- Produces:
  - `pub fn order_fields(form: &mut EditForm)`
  - `impl EditField { pub fn injected(label: String, must: bool, schema: &SchemaModel) -> EditField }`

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `src/workflows/edit_form.rs`:

```rust
#[test]
fn injected_field_resolves_kind_and_multi_from_schema() {
    let s = schema();
    // `cn` is SINGLE-VALUE in the fixture; `sn` is multi.
    let cn = EditField::injected("cn".into(), true, &s);
    assert!(!cn.multi);
    assert!(cn.must);
    assert!(cn.editable);
    assert!(cn.values.is_empty() && cn.baseline.is_empty());
    let sn = EditField::injected("sn".into(), false, &s);
    assert!(sn.multi);
    assert!(!sn.must);
}

#[test]
fn order_fields_puts_must_first_then_populated_then_empty() {
    let mut f = build_edit_form(&model(), &schema(), false);
    // model() has cn (must, populated) and sn (must, populated): add an empty
    // optional and a populated optional to exercise all three buckets.
    f.fields.push(EditField::injected("description".into(), false, &schema())); // empty optional
    let mut populated_opt = EditField::injected("givenName".into(), false, &schema());
    populated_opt.values = vec!["x".into()];
    f.fields.push(populated_opt);
    order_fields(&mut f);
    let labels: Vec<&str> = f.fields.iter().map(|x| x.label.as_str()).collect();
    // must (cn, sn) first (alphabetical), then populated optional (givenName),
    // then empty optional (description) last.
    assert_eq!(labels, vec!["cn", "sn", "givenName", "description"]);
}

#[test]
fn order_fields_sinks_orphaned_to_bottom() {
    let mut f = build_edit_form(&model(), &schema(), false);
    f.fields[0].orphaned = true; // cn orphaned → current_values() == [] → bucket 2
    order_fields(&mut f);
    assert_eq!(f.fields.last().unwrap().label, "cn");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -j4 -p edaptor --lib edit_form::tests 2>&1 | tail -20`
(use the crate name shown by `cargo metadata` if not `edaptor`).
Expected: FAIL — `EditField::injected` and `order_fields` are not defined.

- [ ] **Step 3: Implement `EditField::injected` and `order_fields`**

In `src/workflows/edit_form.rs`, add to the `impl EditField` block (after
`current_values`):

```rust
    /// A freshly schema-injected editable field with no `FormField` backing:
    /// empty values/baseline, free-text widget, `kind`/`multi` resolved from
    /// schema. Used by [`EditForm::sync_schema_fields`] when an objectClass change
    /// brings a new attribute into MUST∪MAY.
    pub fn injected(label: String, must: bool, schema: &SchemaModel) -> EditField {
        let multi = !schema.is_single_value(&label);
        let kind = schema.field_kind(&label);
        EditField {
            label,
            must,
            editable: true,
            multi,
            secret: false,
            ordered: false,
            orphaned: false,
            kind,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: None,
            values: Vec::new(),
            baseline: Vec::new(),
        }
    }
```

Add this free function near the bottom of the file (before `#[cfg(test)]`):

```rust
/// Reorder a built form's fields into: mandatory, then populated-or-special
/// (non-empty current value, secret, or widget-bound), then the rest — each
/// bucket case-insensitive by label. Orphaned fields have empty `current_values`,
/// so they fall into the last bucket. Neutral port of `ui::edit_form::order_fields`
/// (the ratatui picker probe becomes `widget_binding.is_some()`).
pub fn order_fields(form: &mut EditForm) {
    fn bucket(f: &EditField) -> u8 {
        if f.must {
            0
        } else if !f.current_values().is_empty() || f.secret || f.widget_binding.is_some() {
            1
        } else {
            2
        }
    }
    form.fields.sort_by(|a, b| {
        bucket(a)
            .cmp(&bucket(b))
            .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
    });
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -j4 -p edaptor --lib edit_form::tests 2>&1 | tail -20`
Expected: PASS (all edit_form tests, including the three new ones).

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt
cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -5
git add src/workflows/edit_form.rs
git commit -F - <<'MSG'
feat(edit_form): neutral order_fields + EditField::injected

Foundation for the schema resync: a field constructor for schema-injected
attrs and the bucketed field ordering (must / populated / rest), ported from
the ratatui ui::edit_form reference.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
MSG
```

---

### Task 2: Neutral `EditForm::sync_schema_fields`

**Files:**
- Modify: `src/workflows/edit_form.rs`
- Test: `src/workflows/edit_form.rs` tests module

**Interfaces:**
- Consumes: `EditField::injected`, `order_fields` (Task 1); `SchemaModel::effective_attributes`.
- Produces: `impl EditForm { pub fn sync_schema_fields(&mut self, schema: &SchemaModel) }`

**Note on the test fixture:** the existing `schema()` helper only defines `top` and
`person`. This task's tests need an entry whose objectClass set can change MUST∪MAY,
so add a richer fixture **in the test module** (do not change the existing one):

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module:

```rust
fn schema_oc() -> SchemaModel {
    // top (MUST objectClass); person (MUST sn,cn; MAY description);
    // organizationalPerson SUP person (MAY title, ou);
    // extensibleObject (no extra attrs, used to test removal).
    let raw = RawSubschema {
        object_classes: vec![
            "( 2.5.6.0 NAME 'top' ABSTRACT MUST objectClass )".into(),
            "( 2.5.6.6 NAME 'person' SUP top STRUCTURAL MUST ( sn $ cn ) MAY description )".into(),
            "( 2.5.6.7 NAME 'organizationalPerson' SUP person STRUCTURAL MAY ( title $ ou ) )".into(),
        ],
        attribute_types: vec![
            "( 2.5.4.3 NAME 'cn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 SINGLE-VALUE )".into(),
            "( 2.5.4.4 NAME 'sn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".into(),
            "( 2.5.4.13 NAME 'description' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".into(),
            "( 2.5.4.12 NAME 'title' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".into(),
            "( 2.5.4.11 NAME 'ou' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".into(),
        ],
        ldap_syntaxes: vec![],
    };
    SchemaModel::from_raw(&raw)
}

/// Build an EditForm with an explicit objectClass field carrying `ocs`.
fn form_with_ocs(ocs: &[&str]) -> EditForm {
    let oc_field = EditField {
        label: "objectClass".into(),
        must: true,
        editable: false,
        multi: true,
        secret: false,
        ordered: false,
        orphaned: false,
        kind: FieldKind::Text,
        widget: WidgetSpec::ReadOnlyText,
        widget_binding: None,
        values: ocs.iter().map(|s| s.to_string()).collect(),
        baseline: ocs.iter().map(|s| s.to_string()).collect(),
    };
    EditForm {
        dn: "cn=Bob,dc=example,dc=org".into(),
        mode: FormMode::Edit,
        object_classes: ocs.iter().map(|s| s.to_string()).collect(),
        fields: vec![oc_field],
    }
}

#[test]
fn sync_injects_must_and_may_fields_for_classes() {
    let mut f = form_with_ocs(&["top", "person"]);
    f.sync_schema_fields(&schema_oc());
    let has = |l: &str| f.fields.iter().any(|x| x.label.eq_ignore_ascii_case(l));
    assert!(has("cn") && has("sn") && has("description"));
    let cn = f.fields.iter().find(|x| x.label == "cn").unwrap();
    assert!(cn.must); // person MUST cn
    let desc = f.fields.iter().find(|x| x.label == "description").unwrap();
    assert!(!desc.must); // person MAY description
}

#[test]
fn sync_orphans_fields_when_class_removed() {
    // Start with organizationalPerson (title/ou allowed + populated), then remove it.
    let mut f = form_with_ocs(&["top", "organizationalPerson"]);
    f.sync_schema_fields(&schema_oc()); // title/ou now injected & allowed
    if let Some(t) = f.fields.iter_mut().find(|x| x.label == "title") {
        t.values = vec!["Boss".into()];
    }
    // Now drop down to plain person: title/ou leave MUST∪MAY → orphaned.
    f.fields
        .iter_mut()
        .find(|x| x.label.eq_ignore_ascii_case("objectClass"))
        .unwrap()
        .values = vec!["top".into(), "person".into()];
    f.sync_schema_fields(&schema_oc());
    let title = f.fields.iter().find(|x| x.label == "title").unwrap();
    assert!(title.orphaned);
    assert!(!title.must);
    // title still present but sunk to the bottom region; objectClass never orphaned.
    let oc = f
        .fields
        .iter()
        .find(|x| x.label.eq_ignore_ascii_case("objectClass"))
        .unwrap();
    assert!(!oc.orphaned);
}

#[test]
fn sync_preserves_values_on_still_allowed_fields() {
    let mut f = form_with_ocs(&["top", "person"]);
    f.sync_schema_fields(&schema_oc());
    f.fields.iter_mut().find(|x| x.label == "cn").unwrap().values = vec!["Bob".into()];
    // add organizationalPerson; cn stays allowed and keeps its value.
    f.fields
        .iter_mut()
        .find(|x| x.label.eq_ignore_ascii_case("objectClass"))
        .unwrap()
        .values = vec!["top".into(), "person".into(), "organizationalPerson".into()];
    f.sync_schema_fields(&schema_oc());
    let cn = f.fields.iter().find(|x| x.label == "cn").unwrap();
    assert_eq!(cn.values, vec!["Bob".to_string()]);
    assert!(!cn.orphaned);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -j4 -p edaptor --lib edit_form::tests::sync 2>&1 | tail -20`
Expected: FAIL — `sync_schema_fields` not defined.

- [ ] **Step 3: Implement `sync_schema_fields`**

Add to the `impl EditForm` block in `src/workflows/edit_form.rs`:

```rust
    /// Recompute the form's fields from the current `objectClass` field values:
    /// flag fields that left MUST∪MAY as `orphaned`, refresh `must`, inject empty
    /// fields for newly-allowed attrs, then reorder. Faithful neutral port of
    /// `ui::edit_form::sync_schema_fields`. Values on still-allowed fields are
    /// preserved; objectClass is never orphaned. No-op-safe if no objectClass field.
    pub fn sync_schema_fields(&mut self, schema: &SchemaModel) {
        let oc_values: Vec<String> = self
            .fields
            .iter()
            .find(|f| f.label.eq_ignore_ascii_case("objectClass"))
            .map(|f| f.values.clone())
            .unwrap_or_default();
        let oc_refs: Vec<&str> = oc_values.iter().map(|s| s.as_str()).collect();

        let resolved = schema.effective_attributes(&oc_refs);
        let allowed: std::collections::BTreeSet<String> = resolved
            .must
            .iter()
            .chain(resolved.may.iter())
            .map(|s| s.to_lowercase())
            .chain(std::iter::once("objectclass".to_string()))
            .collect();

        for field in &mut self.fields {
            let key = field.label.to_lowercase();
            if key == "objectclass" {
                field.orphaned = false;
                continue;
            }
            let in_allowed = allowed.contains(&key);
            field.orphaned = !in_allowed;
            field.must = in_allowed
                && resolved
                    .must
                    .iter()
                    .any(|m| m.eq_ignore_ascii_case(&field.label));
        }

        let existing: std::collections::HashSet<String> =
            self.fields.iter().map(|f| f.label.to_lowercase()).collect();
        for attr in resolved.must.iter().chain(resolved.may.iter()) {
            if existing.contains(&attr.to_lowercase()) {
                continue;
            }
            let is_must = resolved.must.contains(attr);
            self.fields
                .push(EditField::injected(attr.clone(), is_must, schema));
        }

        order_fields(self);
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -j4 -p edaptor --lib edit_form::tests 2>&1 | tail -20`
Expected: PASS (all edit_form tests).

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt
cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -5
git add src/workflows/edit_form.rs
git commit -F - <<'MSG'
feat(edit_form): neutral EditForm::sync_schema_fields

Port the ratatui sync_schema_fields into the neutral model: editing the
objectClass field's values recomputes orphaned/must, injects new MUST∪MAY
fields, and reorders. Drives the live add/orphan behaviour. No-op-safe when
no objectClass field is present.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
MSG
```

---

### Task 3: Widget contract — `Activation::Modal` + `FieldEditor`

**Files:**
- Modify: `src/tui/widget.rs`
- Test: `src/tui/widget.rs` tests module (fix the two `assert_eq!(Activation::Inline)` cases)

**Interfaces:**
- Consumes: `EditField`, tvision `View`/`ViewId`, `crate::tui::Shared`, `crate::schema::SchemaModel`.
- Produces:
  - `pub enum Activation { Inline, Modal(Box<dyn FieldEditor>) }` (no derives)
  - `pub trait FieldEditor { fn into_view(self: Box<Self>, schema: &SchemaModel, shared: Shared) -> (Box<dyn View>, tv::ViewId); }`

- [ ] **Step 1: Update the two existing tests to not require `PartialEq` on `Activation`**

In `src/tui/widget.rs` tests, replace:

```rust
    #[test]
    fn test_plain_activate_is_inline() {
        assert_eq!(
            PlainWidget.activate(&field(&["x"], WidgetSpec::ReadOnlyText)),
            Activation::Inline
        );
    }
```

with:

```rust
    #[test]
    fn test_plain_activate_is_inline() {
        assert!(matches!(
            PlainWidget.activate(&field(&["x"], WidgetSpec::ReadOnlyText)),
            Activation::Inline
        ));
    }
```

(There is only one `assert_eq!(.., Activation::Inline)` site; the activate call in
other tests is fine. Verify with `grep -n "Activation::Inline" src/tui/widget.rs`.)

- [ ] **Step 2: Run to verify the build currently still passes (baseline)**

Run: `cargo test -j4 -p edaptor --lib tui::widget::tests 2>&1 | tail -10`
Expected: PASS (the `matches!` rewrite is behaviour-equivalent for now).

- [ ] **Step 3: Add the modal seam**

In `src/tui/widget.rs`, extend the imports at the top:

```rust
use crate::schema::SchemaModel;
use crate::tui::Shared;
use crate::workflows::edit_form::EditField;
use crate::workflows::form_model::WidgetSpec;
use tvision_rs::{self as tv, View};
```

Replace the `Activation` enum and its derive with:

```rust
/// How a field is edited. `Inline` = grapheme edit in place (M2). `Modal` = a
/// dialog editor that yields a typed `CommitOutcome` (M3+: the first impl is the
/// objectClass picker). Not `PartialEq`/`Clone`: it carries a trait object.
pub enum Activation {
    Inline,
    Modal(Box<dyn FieldEditor>),
}

/// A modal field editor: builds its tvision dialog and keeps the prospective
/// `CommitOutcome` in `shared.borrow_mut().staged_commit` as the user interacts.
/// `dispatch` reads it back by the `exec_view` return code (apply on OK, discard
/// on CANCEL). Returns the view plus the `ViewId` to focus initially.
pub trait FieldEditor {
    fn into_view(
        self: Box<Self>,
        schema: &SchemaModel,
        shared: Shared,
    ) -> (Box<dyn View>, tv::ViewId);
}
```

(Leave `CommitOutcome`, `Capability`, `FieldWidget`, `PlainWidget`, `present_field`,
`inline_editable` unchanged. `PlainWidget::activate` still returns
`Activation::Inline` — that still compiles.)

- [ ] **Step 4: Run to verify it compiles and tests pass**

Run: `cargo test -j4 -p edaptor --lib tui::widget::tests 2>&1 | tail -10`
Expected: PASS. The `Modal` variant and `FieldEditor` are unconstructed for now
(same pattern as the already-present unconstructed `CommitOutcome::StageSecret`),
so `clippy -D warnings` stays clean — verify next.

- [ ] **Step 5: clippy + fmt + commit**

```bash
cargo fmt
cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -5
git add src/tui/widget.rs
git commit -F - <<'MSG'
feat(tui/widget): add Activation::Modal + FieldEditor seam

The reusable modal-widget contract (umbrella §4): a field editor builds a
dialog and stages a typed CommitOutcome via shared state. Drops the derives
on Activation (now carries a trait object); the one assert_eq test becomes a
matches!. First impl (objectClass) lands in a later task.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
MSG
```

---

### Task 4: `UiState` — `activate_field`, `staged_commit`, `apply_commit`

**Files:**
- Modify: `src/tui/state.rs`
- Test: `src/tui/state.rs` tests module

**Interfaces:**
- Consumes: `CommitOutcome` (`crate::tui::widget`), `EditForm::sync_schema_fields` (Task 2), `ReadFlow::schema`.
- Produces:
  - `UiState.activate_field: Option<usize>`
  - `UiState.staged_commit: Option<crate::tui::widget::CommitOutcome>`
  - `impl UiState { pub fn apply_commit(&mut self, field_idx: usize, outcome: CommitOutcome) }`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `src/tui/state.rs` (it already constructs a
`new_for_test` state — reuse that pattern; if a helper builds a state with an
`edit_form`, reuse it, otherwise build inline as below):

```rust
#[test]
fn apply_commit_resyncs_on_objectclass_change() {
    use crate::tui::widget::CommitOutcome;
    use crate::workflows::edit_form::{EditField, EditForm, FormMode};
    use crate::schema::FieldKind;
    use crate::workflows::form_model::WidgetSpec;

    // Minimal schema with person (MUST sn,cn MAY description).
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
    let structure = Structure::from_input(StructureInput::default());
    let mut st = UiState::new_for_test(
        structure,
        schema,
        "dc=example,dc=org".into(),
        Vec::new(),
        Vec::new(),
    );
    let oc_field = EditField {
        label: "objectClass".into(),
        must: true,
        editable: false,
        multi: true,
        secret: false,
        ordered: false,
        orphaned: false,
        kind: FieldKind::Text,
        widget: WidgetSpec::ReadOnlyText,
        widget_binding: None,
        values: vec!["top".into()],
        baseline: vec!["top".into()],
    };
    st.edit_form = Some(EditForm {
        dn: "cn=Bob,dc=example,dc=org".into(),
        mode: FormMode::Edit,
        object_classes: vec!["top".into()],
        fields: vec![oc_field],
    });

    // Commit "top, person": objectClass values updated, fields injected, render flagged.
    st.apply_commit(
        0,
        CommitOutcome::SetValuesThenResyncSchema(vec!["top".into(), "person".into()]),
    );
    let form = st.edit_form.as_ref().unwrap();
    assert_eq!(form.object_classes, vec!["top".to_string(), "person".to_string()]);
    let oc = form.fields.iter().find(|f| f.label == "objectClass").unwrap();
    assert_eq!(oc.values, vec!["top".to_string(), "person".to_string()]);
    assert!(form.fields.iter().any(|f| f.label == "sn"));
    assert!(st.form_needs_render);
}
```

If `Structure::from_input`/`StructureInput::default` is not the exact constructor,
use whatever the existing `state.rs` tests already use to build a `Structure`
(grep the test module for how `new_for_test` is called).

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -j4 -p edaptor --lib tui::state::tests::apply_commit 2>&1 | tail -20`
Expected: FAIL — `apply_commit` and the new fields do not exist.

- [ ] **Step 3: Add the state fields**

In `src/tui/state.rs`, add to the `UiState` struct (after `set_tree_row`):

```rust
    /// Form pane → controller: the field index whose modal editor should open on
    /// the next `ACTIVATE`. Set by the pane, consumed by `app::dispatch`.
    pub activate_field: Option<usize>,
    /// Modal editor → controller: the prospective commit an open editor would
    /// apply. Maintained live by the editor view; applied by `dispatch` on OK.
    pub staged_commit: Option<crate::tui::widget::CommitOutcome>,
```

Initialise both to `None` in `new_for_test` (add the two lines alongside the other
`None` initialisers) **and** wherever the production `UiState { .. }` is built
(grep for `set_tree_row: None` to find every constructor — add `activate_field:
None, staged_commit: None,` next to it in each).

- [ ] **Step 4: Add `apply_commit`**

Add to an `impl UiState` block in `src/tui/state.rs`:

```rust
    /// Apply a modal editor's typed `CommitOutcome` to the loaded form. For the
    /// resync variant: write the objectClass field values, mirror them into
    /// `object_classes`, then regenerate fields. Reads schema from `read_flow`
    /// (split-borrow so `edit_form` and `read_flow` are borrowed disjointly).
    pub fn apply_commit(&mut self, field_idx: usize, outcome: crate::tui::widget::CommitOutcome) {
        use crate::tui::widget::CommitOutcome;
        let UiState {
            edit_form,
            read_flow,
            form_needs_render,
            ..
        } = self;
        match outcome {
            CommitOutcome::SetValues(vals) => {
                if let Some(form) = edit_form.as_mut() {
                    if let Some(f) = form.fields.get_mut(field_idx) {
                        f.values = vals;
                    }
                }
            }
            CommitOutcome::SetValuesThenResyncSchema(ocs) => {
                if let Some(form) = edit_form.as_mut() {
                    if let Some(f) = form.fields.get_mut(field_idx) {
                        f.values = ocs.clone();
                    }
                    form.object_classes = ocs;
                    form.sync_schema_fields(read_flow.schema());
                }
            }
            // StageSecret is M4 (password); no-op here.
            CommitOutcome::StageSecret { .. } => {}
            CommitOutcome::Cancelled => {}
        }
        *form_needs_render = true;
    }
```

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test -j4 -p edaptor --lib tui::state::tests 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 6: fmt + clippy + commit**

```bash
cargo fmt
cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -5
git add src/tui/state.rs
git commit -F - <<'MSG'
feat(tui/state): activate_field, staged_commit, apply_commit

Controller state for the modal seam: the pane records which field to activate,
the editor stages a CommitOutcome, and apply_commit applies it — including the
objectClass resync (update field values + object_classes, regenerate fields).

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
MSG
```

---

### Task 5: ObjectClass picker (`oc_picker.rs`) + `widget_for`

**Files:**
- Create: `src/tui/oc_picker.rs`
- Modify: `src/tui/mod.rs` (add `pub(crate) mod oc_picker;`)
- Modify: `src/tui/widget.rs` (add `widget_for` + `is_modal_field`)
- Test: `src/tui/oc_picker.rs` tests module

**Interfaces:**
- Consumes: `FieldWidget`, `Activation`, `FieldEditor`, `CommitOutcome`, `Capability`, `present_field` (Task 3); `EditField`; `Shared`; `SchemaModel::object_class_names`; tvision `Dialog`/`ListBox`/`InputLine`/`Command`/`ButtonFlags`/`ButtonRowAlign`.
- Produces:
  - `pub(crate) struct ObjectClassWidget;` + `impl FieldWidget for ObjectClassWidget`
  - `struct ObjectClassEditor { current: Vec<String> }` + `impl FieldEditor`
  - `struct ObjectClassPicker { .. }` (the dialog view)
  - In `widget.rs`: `pub fn widget_for(field: &EditField) -> Box<dyn FieldWidget>` and `pub fn is_modal_field(field: &EditField) -> bool`

- [ ] **Step 1: Add `is_modal_field` + `widget_for` to `widget.rs` (with tests)**

Append to `src/tui/widget.rs` (after `inline_editable`):

```rust
/// The widget plugin for a field. M3 Phase 2a: the objectClass field gets the
/// objectClass picker; everything else is plain. (M4 extends this to dispatch on
/// `field.widget_binding` — no form-core change.)
pub fn widget_for(field: &EditField) -> Box<dyn FieldWidget> {
    if field.label.eq_ignore_ascii_case("objectClass") {
        Box::new(crate::tui::oc_picker::ObjectClassWidget)
    } else {
        Box::new(PlainWidget)
    }
}

/// Whether a field opens a modal editor on activation (vs inline edit). Cheap
/// label-based check used by the form pane for focus/nav/Enter without building
/// an editor. Mirrors the `widget_for` routing.
pub fn is_modal_field(field: &EditField) -> bool {
    field.label.eq_ignore_ascii_case("objectClass")
}
```

Add tests to the `widget.rs` tests module:

```rust
    #[test]
    fn objectclass_is_modal_field() {
        let mut f = field(&["top"], WidgetSpec::ReadOnlyText);
        f.label = "objectClass".into();
        assert!(is_modal_field(&f));
        assert!(matches!(
            widget_for(&f).activate(&f),
            Activation::Modal(_)
        ));
    }

    #[test]
    fn plain_field_is_not_modal() {
        let f = field(&["x"], WidgetSpec::ReadOnlyText);
        assert!(!is_modal_field(&f));
        assert!(matches!(widget_for(&f).activate(&f), Activation::Inline));
    }
```

(These will not compile until `ObjectClassWidget` exists — that's the next step;
run them together in Step 4.)

- [ ] **Step 2: Declare the module**

In `src/tui/mod.rs`, add alongside the other `mod` declarations:

```rust
pub(crate) mod oc_picker;
```

Add (near the other `pub const … = tv::Command::custom(...)`):

```rust
pub const ACTIVATE: tv::Command = tv::Command::custom("edaptor.activate_field");
```

- [ ] **Step 3: Write `src/tui/oc_picker.rs`**

```rust
//! The objectClass field editor: a schema-seeded multi-select dialog. Lists all
//! object-class names (current ones pre-ticked), client-substring-filters, and
//! keeps the prospective `SetValuesThenResyncSchema` outcome in
//! `UiState::staged_commit`. Capability: `NeedsSchema` (no worker).

use std::collections::BTreeSet;

use tvision_rs::{
    self as tv, delegate, ButtonFlags, ButtonRowAlign, Command, Context, Dialog, Event, FieldValue,
    InputLine, Key, ListBox, Rect, View,
};

use crate::tui::widget::{Activation, Capability, CommitOutcome, FieldEditor, FieldWidget};
use crate::tui::Shared;
use crate::schema::SchemaModel;
use crate::workflows::edit_form::EditField;

/// The plugin for the objectClass field.
pub(crate) struct ObjectClassWidget;

impl FieldWidget for ObjectClassWidget {
    fn capability(&self) -> Capability {
        Capability::NeedsSchema
    }
    fn present(&self, field: &EditField) -> String {
        crate::tui::widget::present_field(field)
    }
    fn activate(&self, field: &EditField) -> Activation {
        Activation::Modal(Box::new(ObjectClassEditor {
            current: field.values.clone(),
        }))
    }
}

/// Carries the field's current objectClass values into the dialog builder.
pub(crate) struct ObjectClassEditor {
    current: Vec<String>,
}

impl FieldEditor for ObjectClassEditor {
    fn into_view(
        self: Box<Self>,
        schema: &SchemaModel,
        shared: Shared,
    ) -> (Box<dyn View>, tv::ViewId) {
        let candidates = schema.object_class_names();
        let ticked: BTreeSet<String> = self.current.iter().map(|s| s.to_lowercase()).collect();
        let picker = ObjectClassPicker::new(candidates, ticked, shared);
        let focus = picker.list_id;
        (Box::new(picker), focus)
    }
}

/// The interactive dialog: search box + ticked candidate list + OK/Cancel.
pub(crate) struct ObjectClassPicker {
    dlg: Dialog,
    search_id: tv::ViewId,
    list_id: tv::ViewId,
    shared: Shared,
    candidates: Vec<String>, // all OC names, sorted
    ticked: BTreeSet<String>, // lowercased ticked names
    filtered: Vec<String>,   // current display order (subset of candidates)
    last_search: String,
}

impl ObjectClassPicker {
    fn new(candidates: Vec<String>, ticked: BTreeSet<String>, shared: Shared) -> Self {
        let mut dlg = Dialog::new(Rect::new(0, 0, 56, 22), Some("Object classes".to_string()));
        dlg.state_mut().options.center_x = true;
        dlg.state_mut().options.center_y = true;
        // Search box (row 1) + list (rows 2..18) inside the dialog frame.
        let search = InputLine::with_limit(Rect::new(2, 1, 54, 2), 64);
        let search_id = dlg.insert_child(Box::new(search));
        let list = ListBox::new(Rect::new(2, 3, 54, 18), 1, None, None);
        let list_id = dlg.insert_child(Box::new(list));
        dlg.button_row(
            &[
                (
                    "~O~K",
                    Command::OK,
                    ButtonFlags {
                        default: true,
                        ..ButtonFlags::new()
                    },
                ),
                ("~C~ancel", Command::CANCEL, ButtonFlags::new()),
            ],
            ButtonRowAlign::Right,
        );
        let mut me = ObjectClassPicker {
            dlg,
            search_id,
            list_id,
            shared,
            candidates,
            ticked,
            filtered: Vec::new(),
            last_search: String::new(),
        };
        me.update_staged(); // reflect the pre-ticked set even with no interaction
        me
    }

    /// Rebuild the visible list from `candidates` filtered by `last_search`,
    /// each row prefixed with a tick marker.
    fn refresh_list(&mut self, ctx: &mut Context) {
        let needle = self.last_search.to_lowercase();
        self.filtered = self
            .candidates
            .iter()
            .filter(|c| needle.is_empty() || c.to_lowercase().contains(&needle))
            .cloned()
            .collect();
        let rows: Vec<String> = self
            .filtered
            .iter()
            .map(|c| {
                let mark = if self.ticked.contains(&c.to_lowercase()) {
                    "[x]"
                } else {
                    "[ ]"
                };
                format!("{mark} {c}")
            })
            .collect();
        if let Some(list) = self.dlg.child_mut(self.list_id) {
            if let Some(lb) = list.as_any_mut().and_then(|a| a.downcast_mut::<ListBox>()) {
                lb.new_list(rows, ctx);
            }
        }
    }

    /// The candidate name under the list highlight, if any.
    fn highlighted(&mut self) -> Option<String> {
        let sel = match self.dlg.child_mut(self.list_id).and_then(|v| v.value()) {
            Some(FieldValue::Int(i)) => i as usize,
            _ => return None,
        };
        self.filtered.get(sel).cloned()
    }

    /// Write the prospective commit (sorted-by-candidate-order ticked names) into
    /// shared state. Borrow is taken and dropped here only.
    fn update_staged(&self) {
        let committed: Vec<String> = self
            .candidates
            .iter()
            .filter(|c| self.ticked.contains(&c.to_lowercase()))
            .cloned()
            .collect();
        self.shared.borrow_mut().staged_commit =
            Some(CommitOutcome::SetValuesThenResyncSchema(committed));
    }

    fn current_search(&mut self) -> String {
        match self.dlg.child_mut(self.search_id).and_then(|v| v.value()) {
            Some(FieldValue::Text(s)) => s,
            _ => String::new(),
        }
    }
}

#[delegate(to = dlg)]
impl View for ObjectClassPicker {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        // Seed the list on first paint (filtered empty until first refresh).
        if self.filtered.is_empty() && !self.candidates.is_empty() && self.last_search.is_empty() {
            self.refresh_list(ctx);
        }

        // Space toggles the highlighted candidate's tick.
        let space = matches!(ev, Event::KeyDown(k) if k.key == Key::Char(' '));
        let nav = matches!(
            ev,
            Event::KeyDown(k)
                if matches!(k.key, Key::Up | Key::Down | Key::PageUp | Key::PageDown)
        );
        if space {
            if let Some(name) = self.highlighted() {
                let key = name.to_lowercase();
                if !self.ticked.remove(&key) {
                    self.ticked.insert(key);
                }
                self.refresh_list(ctx);
                self.update_staged();
            }
            ev.clear();
        } else if nav {
            if let Some(list) = self.dlg.child_mut(self.list_id) {
                list.handle_event(ev, ctx);
            }
        } else {
            self.dlg.handle_event(ev, ctx);
        }

        // Refilter when the search text changed.
        let cur = self.current_search();
        if cur != self.last_search {
            self.last_search = cur;
            self.refresh_list(ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldap::worker::RawSubschema;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn schema() -> SchemaModel {
        let raw = RawSubschema {
            object_classes: vec![
                "( 2.5.6.0 NAME 'top' ABSTRACT MUST objectClass )".into(),
                "( 2.5.6.6 NAME 'person' SUP top STRUCTURAL MUST ( sn $ cn ) )".into(),
                "( 2.5.6.7 NAME 'organizationalPerson' SUP person STRUCTURAL )".into(),
            ],
            attribute_types: vec![
                "( 2.5.4.3 NAME 'cn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".into(),
            ],
            ldap_syntaxes: vec![],
        };
        SchemaModel::from_raw(&raw)
    }

    // A worker-less Shared for staging assertions. Reuse the crate's test
    // UiState constructor (see state.rs tests) to build it.
    fn shared() -> Shared {
        use crate::workflows::structure::{Structure, StructureInput};
        let st = crate::tui::state::UiState::new_for_test(
            Structure::from_input(StructureInput::default()),
            schema(),
            "dc=example,dc=org".into(),
            Vec::new(),
            Vec::new(),
        );
        Rc::new(RefCell::new(st))
    }

    #[test]
    fn into_view_preticks_current_and_stages_them() {
        let sh = shared();
        let ed: Box<dyn FieldEditor> = Box::new(ObjectClassEditor {
            current: vec!["top".into(), "person".into()],
        });
        let _ = ed.into_view(&schema(), sh.clone());
        // construction stages the pre-ticked set immediately.
        match sh.borrow().staged_commit.clone() {
            Some(CommitOutcome::SetValuesThenResyncSchema(v)) => {
                assert!(v.iter().any(|s| s.eq_ignore_ascii_case("top")));
                assert!(v.iter().any(|s| s.eq_ignore_ascii_case("person")));
                assert!(!v.iter().any(|s| s.eq_ignore_ascii_case("organizationalPerson")));
            }
            other => panic!("expected resync outcome, got {other:?}"),
        }
    }
}
```

If `Structure::from_input`/`StructureInput::default` differs, mirror exactly how
`state.rs` builds a `Structure` in its tests.

- [ ] **Step 4: Run tests**

Run: `cargo test -j4 -p edaptor --lib oc_picker 2>&1 | tail -20`
and `cargo test -j4 -p edaptor --lib tui::widget::tests 2>&1 | tail -10`
Expected: PASS (picker staging test + the two `widget_for` routing tests).

If `#[delegate(to = dlg)]` reports a missing forwarded `View` method, the
delegate macro's method list is per-type; replicate the exact `#[delegate(to = …)]`
usage from `panes/leaf.rs` (it delegates a `Group`); `Dialog` forwards the same
`View` surface. If a specific method is missing, add an explicit override that
calls `self.dlg.<method>(…)`.

- [ ] **Step 5: fmt + clippy + commit**

```bash
cargo fmt
cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -5
git add src/tui/oc_picker.rs src/tui/mod.rs src/tui/widget.rs
git commit -F - <<'MSG'
feat(tui): objectClass picker widget + widget_for registry

ObjectClassWidget (NeedsSchema) opens a schema-seeded multi-select dialog:
all object-class names listed, current ones pre-ticked, client substring
filter, Space toggles. Keeps a prospective SetValuesThenResyncSchema in
staged_commit. widget_for routes objectClass→picker, else plain; is_modal_field
is the pane's cheap tag check. Adds the ACTIVATE command.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
MSG
```

---

### Task 6: Form pane — focus, nav, and Enter→ACTIVATE for modal rows

**Files:**
- Modify: `src/tui/panes/form.rs`
- Test: `src/tui/panes/form.rs` tests module

**Interfaces:**
- Consumes: `is_modal_field` (Task 5), `inline_editable`, `ACTIVATE` command.
- Produces: behaviour — the objectClass row is focusable; Up/Down land on it; Enter on it sets `activate_field` + posts `ACTIVATE`; character/edit keys are swallowed on it.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `src/tui/panes/form.rs` (mirror the existing tests
that build a `FormPane` over a `Shared` with an `edit_form` and feed events). Build
a form whose fields include an `objectClass` (multi) row:

```rust
#[test]
fn enter_on_objectclass_row_posts_activate() {
    use crate::workflows::edit_form::{EditField, EditForm, FormMode};
    use crate::schema::FieldKind;
    use crate::workflows::form_model::WidgetSpec;
    // Build a state with a form: one inline field (cn) + an objectClass row.
    let (shared, mut pane, mut ctx_owned) = build_pane_with_form(vec![
        EditField {
            label: "cn".into(), must: true, editable: true, multi: false, secret: false,
            ordered: false, orphaned: false, kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText, widget_binding: None,
            values: vec!["Bob".into()], baseline: vec!["Bob".into()],
        },
        EditField {
            label: "objectClass".into(), must: true, editable: false, multi: true, secret: false,
            ordered: false, orphaned: false, kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText, widget_binding: None,
            values: vec!["top".into(), "person".into()],
            baseline: vec!["top".into(), "person".into()],
        },
    ]);
    let ctx = &mut ctx_owned;
    // Initial render + focus first editable (cn).
    let mut tick = Event::Timer;
    pane.handle_event(&mut tick, ctx);
    // Move focus down to the objectClass row, then press Enter.
    let mut down = Event::KeyDown(tv::KeyEvent::from(tv::Key::Down));
    pane.handle_event(&mut down, ctx);
    let mut enter = Event::KeyDown(tv::KeyEvent::from(tv::Key::Enter));
    pane.handle_event(&mut enter, ctx);
    assert_eq!(shared.borrow().activate_field, Some(1));
}
```

Provide a `build_pane_with_form` test helper in the same module if one does not
already exist — base it on the existing form-pane test setup (`Context::new` with a
`Buffer`/timers/deferred, an `Rc<RefCell<UiState>>` seeded via `new_for_test`, the
`edit_form` set, `FormPane::new(bounds, shared.clone())`). Reuse the harness the
current tests already use (e.g. `header_bounds_for_test`'s callers) rather than
inventing a new one.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -j4 -p edaptor --lib panes::form::tests::enter_on_objectclass 2>&1 | tail -20`
Expected: FAIL — objectClass row is not focusable / Enter not handled, so
`activate_field` stays `None`.

- [ ] **Step 3: Make modal rows focusable**

In `src/tui/panes/form.rs`, add the import:

```rust
use crate::tui::widget::{inline_editable, is_modal_field, present_field};
use crate::tui::{Shared, ACTIVATE, REFRESH};
```

Add a focus predicate helper near `inline_editable` usage (module-private fn):

```rust
/// A field's value cell is focusable if it is inline-editable OR a modal-activated
/// field (objectClass): the latter is read-only text but must accept focus + Enter.
fn cell_focusable(f: &crate::workflows::edit_form::EditField) -> bool {
    inline_editable(f) || is_modal_field(f)
}
```

In `rebuild_cells`, change the metadata collection and the disabled flag from
`inline_editable(f)` to `cell_focusable(f)`:

```rust
        let fields: Vec<(String, bool)> = {
            let st = self.state.borrow();
            match st.edit_form.as_ref() {
                None => Vec::new(),
                Some(form) => form
                    .fields
                    .iter()
                    .map(|f| {
                        let marker = if f.must { "*" } else { "" };
                        (format!("{}{}", f.label, marker), cell_focusable(f))
                    })
                    .collect(),
            }
        };
```

(The loop already sets `il.state.state.disabled = !editable;` from that bool — now
modal cells are enabled/focusable.)

In `render`, the per-row `editable` used to re-set `disabled` must also use
`cell_focusable` so modal rows stay focusable on repaint:

```rust
                            (
                                format!("{}{}", f.label, marker),
                                present_field(f),
                                cell_focusable(f),
                            )
```

Rename `editable_value_ids` to use `cell_focusable` (it drives Up/Down nav and the
initial focus, so modal rows must be in it):

```rust
    fn focusable_value_ids(&self) -> Vec<tv::ViewId> {
        let st = self.state.borrow();
        match st.edit_form.as_ref() {
            None => Vec::new(),
            Some(form) => form
                .fields
                .iter()
                .enumerate()
                .filter(|(_, f)| cell_focusable(f))
                .map(|(i, _)| self.value_ids[i])
                .collect(),
        }
    }
```

Update both call sites (`render`'s `self.editable_value_ids().first()` and
`focus_field`'s `self.editable_value_ids()`) to `self.focusable_value_ids()`.

- [ ] **Step 4: Intercept Enter / swallow edits on modal rows**

Add helpers to `impl FormPane`:

```rust
    /// The field index whose value cell currently holds focus, if any.
    fn focused_field_idx(&mut self) -> Option<usize> {
        let cur = self.scroll_mut().and_then(|sg| sg.current())?;
        self.value_ids.iter().position(|id| *id == cur)
    }

    /// Whether the focused field opens a modal editor (objectClass).
    fn focused_is_modal(&mut self) -> bool {
        let Some(idx) = self.focused_field_idx() else {
            return false;
        };
        let st = self.state.borrow();
        st.edit_form
            .as_ref()
            .and_then(|f| f.fields.get(idx))
            .map(is_modal_field)
            .unwrap_or(false)
    }
```

In `handle_event`, after the `form_needs_render` block and before the Up/Down nav
block, insert the modal-row handling:

```rust
        // Enter on a modal row (objectClass) opens its editor via the controller:
        // record the field index, post ACTIVATE (capture-free), consume the key.
        let enter = matches!(ev, Event::KeyDown(k) if k.key == Key::Enter);
        if enter && self.focused_is_modal() {
            if let Some(idx) = self.focused_field_idx() {
                self.state.borrow_mut().activate_field = Some(idx);
                ctx.post(ACTIVATE);
            }
            ev.clear();
            return;
        }
        // Swallow text edits on a modal row: its value comes from the picker, not
        // typing. (The cell is enabled only so it can take focus + Enter.)
        let edit_key = matches!(
            ev,
            Event::KeyDown(k) if matches!(k.key, Key::Char(_) | Key::Backspace | Key::Delete)
        );
        if edit_key && self.focused_is_modal() {
            ev.clear();
            return;
        }
```

(Keep the existing Up/Down nav block and the trailing `self.sync_into_form();` —
`sync_into_form` already filters to `inline_editable` fields, so it never reads the
modal cell's text. The early `return`s above skip the sync on activate/swallow,
which is correct since no inline edit occurred.)

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test -j4 -p edaptor --lib panes::form::tests 2>&1 | tail -20`
Expected: PASS (the new test plus all existing form-pane tests — confirm none
regressed from the `editable_value_ids`→`focusable_value_ids` rename).

- [ ] **Step 6: fmt + clippy + commit**

```bash
cargo fmt
cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -5
git add src/tui/panes/form.rs
git commit -F - <<'MSG'
feat(tui/form): focus + Enter-activate modal rows (objectClass)

The objectClass row is now focusable (cell_focusable = inline OR modal) and
reachable via Up/Down; Enter on it records activate_field and posts ACTIVATE;
character/edit keys are swallowed so its value comes only from the picker.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
MSG
```

---

### Task 7: Dispatch — run the editor, apply the outcome

**Files:**
- Modify: `src/tui/app.rs`
- Test: covered by the existing dispatch unit seams + the live test (Task 8). Add a
  focused unit test for the apply path only if a `Program`-free seam exists; the
  modal `exec_view` itself is validated live.

**Interfaces:**
- Consumes: `ACTIVATE`, `widget_for`, `Activation::Modal`, `FieldEditor::into_view`, `UiState::apply_commit`, `staged_commit`.
- Produces: `dispatch` handles `cmd == ACTIVATE`.

- [ ] **Step 1: Add the ACTIVATE branch to `dispatch`**

In `src/tui/app.rs`, extend the imports:

```rust
use crate::tui::widget::{widget_for, Activation};
use crate::tui::{Shared, ACTIVATE, GUARD_NAV, REQUEST_QUIT, SAVE, SHOW_ERROR};
```

Add a new `else if` arm in `dispatch` (after the `SAVE` arm, before `GUARD_NAV` is
fine):

```rust
    } else if cmd == ACTIVATE {
        // Open a field's modal editor. The pane recorded which field.
        let idx = state.borrow_mut().activate_field.take();
        let Some(idx) = idx else {
            return;
        };
        // Build the editor from the field (drops the borrow before exec_view).
        let editor = {
            let st = state.borrow();
            st.edit_form
                .as_ref()
                .and_then(|f| f.fields.get(idx))
                .and_then(|field| match widget_for(field).activate(field) {
                    Activation::Modal(ed) => Some(ed),
                    Activation::Inline => None,
                })
        };
        let Some(editor) = editor else {
            return;
        };
        // Build the view (schema borrowed; Shared is an Rc clone, not a borrow).
        let (view, focus) = {
            let st = state.borrow();
            editor.into_view(st.read_flow.schema(), state.clone())
        };
        let answer = prog.exec_view_focused(view, focus);
        if answer == Command::OK {
            let outcome = state.borrow_mut().staged_commit.take();
            if let Some(outcome) = outcome {
                state.borrow_mut().apply_commit(idx, outcome);
            }
        } else {
            state.borrow_mut().staged_commit = None;
        }
```

- [ ] **Step 2: Build and run the full lib test suite**

Run: `cargo test -j4 -p edaptor --lib 2>&1 | tail -20`
Expected: PASS (no regressions; dispatch compiles with the new arm).

- [ ] **Step 3: Confirm the dev binary builds**

Run: `cargo build -j4 --bin edaptor-tv 2>&1 | tail -5`
Expected: `Finished`.

- [ ] **Step 4: fmt + clippy + commit**

```bash
cargo fmt
cargo clippy -j4 --all-targets -- -D warnings 2>&1 | tail -5
git add src/tui/app.rs
git commit -F - <<'MSG'
feat(tui/app): dispatch ACTIVATE → run field editor, apply outcome

The single exec_view site builds the focused field's modal editor, runs it,
and on OK applies the staged CommitOutcome (objectClass resync); CANCEL
discards it. Completes the controller-owned modal seam.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
MSG
```

---

### Task 8: Live test, docs, facade guards, acceptance

**Files:**
- Create: `tests/tv_objectclass.rs`
- Modify: `CHANGES.md`

**Interfaces:** none (integration + docs).

- [ ] **Step 1: Write the gated live test**

Model it on `tests/tv_edit_write.rs` (same gating idiom: skip unless
`EDAPTOR_TEST_LDAP_URI` is set; bind with `EDAPTOR_TEST_ADMIN_PW`). The test drives
the **neutral** path end-to-end against real schema: read an entry, flip its
objectClass set via `EditForm::sync_schema_fields`, and assert the field set
changes. (The modal UI is validated by the tmux acceptance step.)

```rust
//! Gated live test: editing objectClass regenerates the neutral form's fields.
//! Skips unless EDAPTOR_TEST_LDAP_URI is set.

#[test]
fn objectclass_change_regenerates_fields() {
    let Ok(_uri) = std::env::var("EDAPTOR_TEST_LDAP_URI") else {
        eprintln!("skipping: EDAPTOR_TEST_LDAP_URI not set");
        return;
    };
    // Build a SchemaModel from the live server (reuse the helper tv_edit_write.rs
    // uses to fetch cn=subschema), then:
    //  1. build an EditForm for an existing posixAccount entry,
    //  2. add an auxiliary class (e.g. add "organizationalPerson" if absent, or
    //     drop "sambaSamAccount" if present) by editing the objectClass field,
    //  3. call sync_schema_fields and assert a known MAY/MUST attr is injected
    //     (added class) or marked orphaned (removed class).
    // Keep it read-only: never submit a write to the server.
}
```

Fill the body using the exact subschema-fetch + connect helpers in
`tests/tv_edit_write.rs` (do not duplicate connection logic — factor or copy the
helper as that file does). Assert on `field.orphaned` / presence of an injected
label. **No writes** are submitted.

- [ ] **Step 2: Run the gated test both ways**

```bash
cargo test -j4 --test tv_objectclass 2>&1 | tail -10   # skips (env unset) → PASS
scripts/test-ldap.sh start
EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389 EDAPTOR_TEST_ADMIN_PW=adminpassword \
  cargo test -j4 --test tv_objectclass 2>&1 | tail -10  # runs against demo → PASS
```
Expected: PASS in both modes.

- [ ] **Step 3: Update CHANGES.md**

Add under the unreleased tvision-preview section:

```markdown
- **tvision UI:** the `objectClass` field is now editable via a schema-seeded
  multi-select picker (search + tick). Changing the set regenerates the form's
  fields live — newly-allowed attributes appear, now-disallowed ones are marked
  orphaned (dropped on save) — driven by a typed resync outcome.
```

- [ ] **Step 4: Facade guards + full check**

```bash
! grep -rl "use tvision_rs" src | grep -vE "^src/(tui/|bin/edaptor-tv.rs)"
! grep -rl "use ratatui\|use tui_" src | grep -vE "^src/ui/"
make check
```
Expected: both guards print nothing; `make check` = all checks passed.

- [ ] **Step 5: Live tmux acceptance (agent-driven PTY)**

Follow the handover recipe. Drive the real `edaptor-tv` and verify the objectClass
resync interactively, leaving demo data intact:

```bash
scripts/test-ldap.sh start
tmux kill-session -t edtv 2>/dev/null
tmux new-session -d -s edtv -x 210 -y 50
tmux send-keys -t edtv 'export EDAPTOR_TEST_ADMIN_PW=adminpassword' Enter
tmux send-keys -t edtv '/home/oetiker/scratch/cargo-target/debug/edaptor-tv --config examples/demo-config.toml' Enter
sleep 5
# Navigate to a user (ou=people), focus the form, move down to the objectClass
# row, press Enter to open the picker, tick/untick a class, OK; observe fields
# add/orphan live. Then Discard (do NOT save) to leave demo data untouched.
```

Focus probes (from the Phase 1 acceptance): `tmux display-message -p '#{cursor_x}'`
locates the focused pane by column (tree<69 / leaf 70-135 / form>137);
`tmux capture-pane -e` renders the focused element bright-green `(0,170,0)`. Drive
to the objectClass row, Enter → picker dialog appears; Space toggles a tick; OK →
the form gains/loses fields. Confirm in `ldapsearch` that the demo entry is
unchanged afterwards (no write submitted). Kill the tmux session when done.

- [ ] **Step 6: Commit docs + test**

```bash
git add tests/tv_objectclass.rs CHANGES.md
git commit -F - <<'MSG'
test(tv): gated live objectClass resync test + CHANGES

Gated integration test (skips without EDAPTOR_TEST_LDAP_URI): editing the
objectClass field regenerates the neutral form's fields. CHANGES entry for the
new objectClass picker + live field regeneration. make check green; both facade
guards clean; tmux PTY acceptance passed with demo data intact.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
MSG
```

---

## Self-review

**Spec coverage:**
- Port `sync_schema_fields` → Task 2 (+ Task 1 helpers). ✓
- Reusable `FieldEditor` modal seam → Task 3 (contract) + Task 7 (dispatch). ✓
- ObjectClass picker (`NeedsSchema`, seed/pre-tick/filter/toggle, staged outcome) → Task 5. ✓
- Live resync via typed outcome, no global flag → Task 4 (`apply_commit`) + Task 7. ✓
- Form-pane Enter→activate, modal-cell focus/gating → Task 6. ✓
- `object_classes` ↔ objectClass field kept consistent → Task 4 `apply_commit`. ✓
- Error/no-op safety (missing objectClass field / schema) → Task 2 (find-guarded), Task 7 (`let Some(..) else return`). ✓
- Tests: neutral (T1/T2), widget/registry (T3/T5), picker dialog (T5), pane (T6), live (T8). ✓
- Acceptance criteria + facade guards + `make check` → Task 8. ✓

**Placeholder scan:** No "TBD"/"add error handling"/"similar to". Two tasks (T5/T8)
reference reusing an existing test-fixture/connection helper verbatim rather than
re-listing it — that is a deliberate DRY instruction with the exact source file
named, not a placeholder.

**Type consistency:** `Activation::Modal(Box<dyn FieldEditor>)`,
`FieldEditor::into_view(self: Box<Self>, &SchemaModel, Shared) -> (Box<dyn View>, ViewId)`,
`CommitOutcome::SetValuesThenResyncSchema(Vec<String>)`,
`UiState::apply_commit(&mut self, usize, CommitOutcome)`,
`widget_for(&EditField) -> Box<dyn FieldWidget>`, `is_modal_field(&EditField) -> bool`,
`EditField::injected(String, bool, &SchemaModel)`, `order_fields(&mut EditForm)`,
`ACTIVATE` command — names/signatures match across Tasks 3–7. The pane uses
`focusable_value_ids` (renamed from `editable_value_ids`) consistently at both call
sites.

**Known execution-time check (flagged, not a gap):** Task 5 Step 4 notes the
`#[delegate(to = dlg)]` macro forwards a fixed `View` method list; if the build
reports a missing forwarded method on `Dialog`, add an explicit override calling
`self.dlg.<method>`. The TDD compile step surfaces this immediately.
