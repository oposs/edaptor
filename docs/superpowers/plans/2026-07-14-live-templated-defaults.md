# Live Templated Defaults (create-mode autofill) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** In create mode, `[profile.defaults]` template entries (e.g. `cn = "{givenName} {sn}"`) fill and keep updating their target field live as the operator types the sources, until the operator takes the field over.

**Architecture:** A pure latch-and-recompute core in `config/defaults.rs` (no I/O), a per-create-session latch map on `UiState`, and a create-mode-only hook in `FormPane::handle_event` that applies recomputed values to the on-screen editors after each event.

**Tech Stack:** Rust, tvision-rs 0.12 TUI. Cap build/test parallelism at 4 cores.

## Global Constraints

- **Parallelism:** every `cargo` invocation uses `-j4` (shared machine).
- **Lint gate:** `cargo clippy --all-targets -- -D warnings` must pass; `make check` (fmt + clippy + tests) is the definition of done.
- **Comments/identifiers in English.** User-facing docs may be localized, but these are English.
- **Scope:** create mode only; literals and `{next:…}` autonumbers keep one-shot behavior; edit mode stays inert.
- **Branch:** work on `feat/usability` (already checked out).
- **Spec:** `docs/superpowers/specs/2026-07-14-live-templated-defaults-design.md`.

---

## Task 1: Pure core — live-template latch + recompute

**Files:**
- Modify: `src/config/defaults.rs` (add types + two functions + tests)

**Interfaces:**
- Consumes: existing `Seg`, `DefaultValue`, `ProfileDefaults`, and the private `resolve_template(&[Seg], &BTreeMap<String, Vec<String>>) -> Option<String>` (same module).
- Produces:
  - `pub struct LiveTemplateState { pub segs: Vec<Seg>, pub auto: bool, pub last_written: String }`
  - `pub fn live_templates(d: &ProfileDefaults) -> BTreeMap<String, LiveTemplateState>`
  - `pub fn recompute_live(states: &mut BTreeMap<String, LiveTemplateState>, current: &BTreeMap<String, Vec<String>>) -> Vec<(String, String)>`

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module at the bottom of `src/config/defaults.rs` (reuse the existing `cur(&[(&str,&str)])` helper already defined there):

```rust
// --- live templated defaults ---

fn defs(pairs: &[(&str, &str)]) -> ProfileDefaults {
    let mut d = ProfileDefaults::default();
    for (k, v) in pairs {
        d.entries.insert(k.to_string(), parse_default_value(v).unwrap());
    }
    d
}

#[test]
fn live_templates_picks_only_templates() {
    let d = defs(&[
        ("cn", "{givenName} {sn}"),
        ("loginShell", "/bin/bash"),        // literal → excluded
        ("uidNumber", "{next:1000-2000}"),  // autonumber → excluded
    ]);
    let states = live_templates(&d);
    assert_eq!(states.keys().collect::<Vec<_>>(), vec!["cn"]);
    let s = &states["cn"];
    assert!(s.auto);
    assert_eq!(s.last_written, "");
}

#[test]
fn recompute_fills_when_sources_present() {
    let mut states = live_templates(&defs(&[("cn", "{givenName} {sn}")]));
    let changes = recompute_live(&mut states, &cur(&[("givenName", "John"), ("sn", "Doe")]));
    assert_eq!(changes, vec![("cn".to_string(), "John Doe".to_string())]);
    assert_eq!(states["cn"].last_written, "John Doe");
    assert!(states["cn"].auto);
}

#[test]
fn recompute_incomplete_source_clears_target() {
    let mut states = live_templates(&defs(&[("cn", "{givenName} {sn}")]));
    // First fill, then remove sn: the auto target must clear.
    recompute_live(&mut states, &cur(&[("givenName", "John"), ("sn", "Doe")]));
    let changes = recompute_live(&mut states, &cur(&[("givenName", "John"), ("sn", ""), ("cn", "John Doe")]));
    assert_eq!(changes, vec![("cn".to_string(), "".to_string())]);
    assert!(states["cn"].auto);
    assert_eq!(states["cn"].last_written, "");
}

#[test]
fn recompute_stops_when_operator_overrides() {
    let mut states = live_templates(&defs(&[("cn", "{givenName} {sn}")]));
    recompute_live(&mut states, &cur(&[("givenName", "John"), ("sn", "Doe")])); // cn = "John Doe"
    // Operator edits cn to something else, then changes a source.
    let changes = recompute_live(&mut states, &cur(&[("givenName", "Jon"), ("sn", "Doe"), ("cn", "Johnny")]));
    assert!(changes.is_empty(), "operator-owned field is not rewritten");
    assert!(!states["cn"].auto);
    // A further source change is still ignored.
    let changes = recompute_live(&mut states, &cur(&[("givenName", "Jonathan"), ("sn", "Doe"), ("cn", "Johnny")]));
    assert!(changes.is_empty());
    assert!(!states["cn"].auto);
}

#[test]
fn recompute_rearms_when_target_cleared() {
    let mut states = live_templates(&defs(&[("cn", "{givenName} {sn}")]));
    recompute_live(&mut states, &cur(&[("givenName", "John"), ("sn", "Doe")]));
    recompute_live(&mut states, &cur(&[("givenName", "John"), ("sn", "Doe"), ("cn", "Johnny")])); // owned
    assert!(!states["cn"].auto);
    // Operator clears cn → re-arm and refill.
    let changes = recompute_live(&mut states, &cur(&[("givenName", "John"), ("sn", "Doe"), ("cn", "")]));
    assert_eq!(changes, vec![("cn".to_string(), "John Doe".to_string())]);
    assert!(states["cn"].auto);
}

#[test]
fn recompute_our_write_is_not_read_as_override() {
    // Two passes with unchanged sources: the second must NOT flip auto off just
    // because the target now holds our written value.
    let mut states = live_templates(&defs(&[("cn", "{givenName} {sn}")]));
    recompute_live(&mut states, &cur(&[("givenName", "John"), ("sn", "Doe")]));
    let changes = recompute_live(&mut states, &cur(&[("givenName", "John"), ("sn", "Doe"), ("cn", "John Doe")]));
    assert!(changes.is_empty(), "no change: target already equals output");
    assert!(states["cn"].auto, "still auto after our own write is read back");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -j4 --lib config::defaults 2>&1 | tail -20`
Expected: FAIL — `cannot find function 'live_templates'` / `recompute_live` / type `LiveTemplateState`.

- [ ] **Step 3: Implement the core**

Add near the top of `src/config/defaults.rs` (after the `DefaultValue` enum) the type, and after `plan_defaults` the two functions:

```rust
/// Per-target live-template latch (see the live-templated-defaults spec). `segs`
/// is the parsed template; `auto` is true while the target still belongs to the
/// template; `last_written` is the value we last wrote, used to tell our own
/// writes apart from operator edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveTemplateState {
    pub segs: Vec<Seg>,
    pub auto: bool,
    pub last_written: String,
}

/// Build the initial live-template latches from a profile's `[profile.defaults]`:
/// one entry per Template default (literals and autonumbers are skipped). Each
/// starts `auto = true`, `last_written = ""`.
pub fn live_templates(d: &ProfileDefaults) -> BTreeMap<String, LiveTemplateState> {
    d.entries
        .iter()
        .filter_map(|(attr, dv)| match dv {
            DefaultValue::Template(segs) => Some((
                attr.clone(),
                LiveTemplateState {
                    segs: segs.clone(),
                    auto: true,
                    last_written: String::new(),
                },
            )),
            _ => None,
        })
        .collect()
}

/// The first value of `attr` in `current` (case-insensitive key match), or "".
fn first_value(current: &BTreeMap<String, Vec<String>>, attr: &str) -> String {
    current
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(attr))
        .and_then(|(_, v)| v.first())
        .cloned()
        .unwrap_or_default()
}

/// Recompute every auto target against `current` field values, mutating the
/// latches, and return the `(attr, new_value)` changes to apply to the form.
/// Pure. Implements the per-pass rule from the spec:
/// 1. if the target's current value differs from `last_written`, ownership is
///    re-evaluated: `auto = value.is_empty()` (empty ⇒ re-arm, else operator owns);
/// 2. while `auto`, mirror the template: `Some(out)` ⇒ write `out` if it differs;
///    `None` (a source empty) ⇒ clear the target if non-empty.
pub fn recompute_live(
    states: &mut BTreeMap<String, LiveTemplateState>,
    current: &BTreeMap<String, Vec<String>>,
) -> Vec<(String, String)> {
    let mut changes = Vec::new();
    for (attr, st) in states.iter_mut() {
        let value = first_value(current, attr);
        if value != st.last_written {
            st.auto = value.is_empty();
        }
        if !st.auto {
            continue;
        }
        match resolve_template(&st.segs, current) {
            Some(out) => {
                if out != value {
                    st.last_written = out.clone();
                    changes.push((attr.clone(), out));
                }
            }
            None => {
                if !value.is_empty() {
                    st.last_written = String::new();
                    changes.push((attr.clone(), String::new()));
                }
            }
        }
    }
    changes
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -j4 --lib config::defaults 2>&1 | tail -20`
Expected: PASS — all `defaults` tests green (existing + 6 new).

- [ ] **Step 5: Lint**

Run: `cargo clippy -j4 --lib -- -D warnings 2>&1 | tail -20`
Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
git add src/config/defaults.rs
git commit -m "feat(config): live-template latch + recompute (pure core)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Wire live templates into the create form and the form view

**Files:**
- Modify: `src/ui/state.rs` — add `live_templates` field to `UiState`; init in both constructors (`new_for_test` ~line 141, `new` ~line 798).
- Modify: `src/ui/app.rs` — `open_create` (~line 312) builds the latches when installing a create form.
- Modify: `src/ui/panes/form.rs` — add `apply_live_templates`; call it in `handle_event` after `sync_into_form()`; add form-level tests.

**Interfaces:**
- Consumes: `crate::config::defaults::{live_templates, recompute_live, LiveTemplateState}` (Task 1); existing `FormPane::set_value_text(i, String)`, `FormMode::Create`.
- Produces: `UiState.live_templates: BTreeMap<String, LiveTemplateState>`; `FormPane::apply_live_templates(&mut self, ctx)`.

- [ ] **Step 1: Add the field to `UiState`**

In `src/ui/state.rs`, add to the `pub struct UiState` (near `edit_form`):

```rust
    /// Create-mode live-template latches (attr → latch), built by `open_create`
    /// from the profile's `[profile.defaults]` templates. Empty in edit mode;
    /// consulted only while the form is in `Create` mode. See
    /// `config::defaults::recompute_live`.
    pub live_templates: std::collections::BTreeMap<String, crate::config::defaults::LiveTemplateState>,
```

In `new_for_test` (the `UiState { ... }` literal, ~line 141) and in `new` (the `UiState { ... }` literal, ~line 798), add:

```rust
            live_templates: std::collections::BTreeMap::new(),
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -j4 2>&1 | tail -20`
Expected: builds (both constructors satisfied; no other literal sites — the `let UiState { .. }` destructures use `..`).

- [ ] **Step 3: Build the latches in `open_create`**

In `src/ui/app.rs`, `open_create`, inside the block that already borrows `st` and the profile (where `apply_widget_bindings` runs), after the widget bindings block and before `let mut st = state.borrow_mut();`, compute the latches from the same profile. Concretely, extend the existing short borrow so it also returns the latches, OR add a dedicated short borrow:

```rust
    // Build the create-mode live-template latches from the profile's defaults.
    let live = {
        let st = state.borrow();
        crate::config::defaults::live_templates(&st.profiles[profile_idx].defaults)
    };
```

Then in the `let mut st = state.borrow_mut();` block that installs the form, add:

```rust
    st.live_templates = live;
```

(Place `st.live_templates = live;` right after `st.edit_form = Some(form);`.)

- [ ] **Step 4: Write the failing form-level tests**

Add to the `tests` module in `src/ui/panes/form.rs` (reuse `build_pane_with_create_form`, `ef`, `headless_ctx`, and imports already present):

```rust
#[test]
fn create_live_template_fills_cn_from_given_and_sn() {
    use crate::config::defaults::{live_templates, parse_default_value, ProfileDefaults};
    let (shared, mut pane) = build_pane_with_create_form(
        0,
        "dc=x",
        "uid",
        vec![
            ef("givenName", "", true),
            ef("sn", "", true),
            ef("cn", "", true),
        ],
    );
    // Seed the create-mode latches the way open_create would.
    {
        let mut d = ProfileDefaults::default();
        d.entries
            .insert("cn".into(), parse_default_value("{givenName} {sn}").unwrap());
        shared.borrow_mut().live_templates = live_templates(&d);
    }
    let mut out = VecDeque::new();
    let mut timers = tv::timer::TimerQueue::new();
    let mut deferred = Vec::new();
    let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
    let mut refresh = Event::Broadcast { command: REFRESH, source: None };
    pane.handle_event(&mut refresh, &mut ctx); // initial render seeds editors

    // Type into the source editors, then pump one event so the hook recomputes.
    pane.set_value_text(0, "John".into());
    pane.set_value_text(1, "Doe".into());
    let mut tick = Event::Broadcast { command: REFRESH, source: None };
    pane.handle_event(&mut tick, &mut ctx);

    let cn = shared.borrow();
    let cn = cn.edit_form.as_ref().unwrap();
    let cn = cn.fields.iter().find(|f| f.label == "cn").unwrap();
    assert_eq!(cn.values, vec!["John Doe".to_string()]);
}

#[test]
fn edit_mode_never_live_fills() {
    // An edit-mode form with empty live_templates must not touch cn even if a
    // template-shaped default would apply.
    let (shared, mut pane) = build_pane_with_form(vec![
        ef("givenName", "John", true),
        ef("sn", "Doe", true),
        ef("cn", "", true),
    ]);
    // live_templates stays empty (edit mode): the hook is gated on Create.
    let mut out = VecDeque::new();
    let mut timers = tv::timer::TimerQueue::new();
    let mut deferred = Vec::new();
    let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
    let mut tick = Event::Broadcast { command: REFRESH, source: None };
    pane.handle_event(&mut tick, &mut ctx);
    let st = shared.borrow();
    let cn = st.edit_form.as_ref().unwrap().fields.iter().find(|f| f.label == "cn").unwrap();
    // Robust against empty being stored as [] or [""]: it must NOT have been filled.
    assert_ne!(cn.values.first().map(String::as_str), Some("John Doe"));
    assert!(cn.values.first().map(String::as_str).unwrap_or("").is_empty(),
        "edit mode leaves cn empty");
}
```

- [ ] **Step 5: Run the new tests to verify they fail**

Run: `cargo test -j4 --lib create_live_template_fills_cn_from_given_and_sn edit_mode_never_live_fills 2>&1 | tail -20`
Expected: `create_live_template_fills_cn_from_given_and_sn` FAILS (cn stays empty — no hook yet); `edit_mode_never_live_fills` passes (nothing fills it anyway — that's fine, it guards against regressions).

- [ ] **Step 6: Implement `apply_live_templates` and the hook**

In `src/ui/panes/form.rs`, add the method to the `impl FormPane` block (near `sync_into_form`):

```rust
    /// Create-mode only: recompute live templated defaults (e.g. `cn` from
    /// `givenName`/`sn`) and push any changes into the on-screen editors. No-op in
    /// edit mode (`live_templates` empty and the mode guard fails). Borrow shared
    /// state, compute, release, then write the editors.
    fn apply_live_templates(&mut self, _ctx: &mut Context) {
        let changes: Vec<(usize, String)> = {
            let mut st = self.state.borrow_mut();
            let crate::ui::state::UiState { edit_form, live_templates, .. } = &mut *st;
            let Some(form) = edit_form.as_mut() else { return };
            if !matches!(form.mode, FormMode::Create { .. }) {
                return;
            }
            if live_templates.is_empty() {
                return;
            }
            let current: std::collections::BTreeMap<String, Vec<String>> = form
                .fields
                .iter()
                .map(|f| (f.label.clone(), f.values.clone()))
                .collect();
            let mut out = Vec::new();
            for (attr, value) in
                crate::config::defaults::recompute_live(live_templates, &current)
            {
                if let Some(i) = form
                    .fields
                    .iter()
                    .position(|f| f.label.eq_ignore_ascii_case(&attr))
                {
                    form.fields[i].values = vec![value.clone()];
                    out.push((i, value));
                }
            }
            out
        };
        for (i, value) in changes {
            self.set_value_text(i, value);
        }
    }
```

Then in `handle_event`, replace the final `self.sync_into_form();` (the last line before the closing brace of `handle_event`, ~line 1231) with:

```rust
        // Keep edit_form current with the on-screen editors.
        self.sync_into_form();
        // Create mode: mirror live templated defaults (cn/displayName) into the
        // still-auto target fields.
        self.apply_live_templates(ctx);
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -j4 --lib create_live_template_fills_cn_from_given_and_sn edit_mode_never_live_fills 2>&1 | tail -20`
Expected: both PASS.

- [ ] **Step 8: Full check**

Run: `make check`
Expected: `All checks passed!`

- [ ] **Step 9: Commit**

```bash
git add src/ui/state.rs src/ui/app.rs src/ui/panes/form.rs
git commit -m "feat(ui): live-fill templated defaults (cn/displayName) in create mode

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Documentation, example config, changelog

**Files:**
- Modify: `docs/src/configuration/defaults.md`
- Modify: `examples/config.toml` (the annotated reference embedded by `full-example.md`) — add `cn`/`displayName` template defaults with a comment, if not already present.
- Modify: `CHANGES.md` — "New" entry under Unreleased.

**Interfaces:** none (docs only).

- [ ] **Step 1: Document live templating in `defaults.md`**

Read `docs/src/configuration/defaults.md` first. Add a subsection after the template-value description explaining the create-mode live behavior. Use this content (adapt headings to the page's existing style):

```markdown
### Live templating in create mode

When you create a new entry, a **template** default (one containing `{field}`
placeholders, e.g. `cn = "{givenName} {sn}"`) does more than fill once: it keeps
the target in sync with its sources **as you type**, for as long as you have not
edited the target yourself.

- The target fills the moment all its `{…}` sources have values, and re-computes
  whenever a source changes.
- If you type your own value into the target, eDAPtor stops tracking it — the
  field is yours.
- Clear the target back to empty and it **re-arms**: live tracking resumes.
- While any `{…}` source is still empty, the auto target is shown empty.

Literal defaults (`loginShell = "/bin/bash"`) and autonumber defaults
(`{next:MIN-MAX}`) are **not** live — they are applied once. Live templating
applies to **create mode only**; editing an existing entry never rewrites a field
from a template.

Example:

    [profile.defaults]
    cn          = "{givenName} {sn}"
    displayName = "{givenName} {sn}"
```

- [ ] **Step 2: Add the templates to the example config**

Open `examples/config.toml`, find a `[<...>.defaults]` table for a user/person
profile. If `cn`/`displayName` template defaults are not present, add them with a
one-line comment:

```toml
# In create mode these fill live from givenName/sn until you edit them.
cn          = "{givenName} {sn}"
displayName = "{givenName} {sn}"
```

If the profile’s `defaults` table does not exist, add it under that profile.
(Keep `examples/config.toml` and `docs/src/configuration/full-example.md` consistent
— the latter embeds the former; no separate edit needed if it uses an include.)

- [ ] **Step 3: Changelog entry**

In `CHANGES.md`, under `## Unreleased` → `### New`, add:

```markdown
- **Live autofill for templated defaults (create mode).** A `[profile.defaults]`
  template such as `cn = "{givenName} {sn}"` now fills *and keeps updating* the
  target as you type its sources when creating an entry, until you edit the target
  yourself (clear it to re-arm). Literals and autonumbers are unchanged; editing
  existing entries is unaffected. See
  [Configuration → Defaults](https://oposs.github.io/edaptor/configuration/defaults.html).
```

- [ ] **Step 4: Build the docs**

Run: `make docs 2>&1 | tail -20`
Expected: mdBook builds with no errors.

- [ ] **Step 5: Commit**

```bash
git add docs/src/configuration/defaults.md examples/config.toml CHANGES.md
git commit -m "docs: live templated defaults (create-mode autofill)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Verification (whole feature)

- [ ] `make check` green.
- [ ] Manual smoke (optional, needs the podman LDAP): `scripts/test-ldap.sh start`,
  then create a user under a profile with `cn = "{givenName} {sn}"`; type givenName
  and sn and watch `cn` fill live; edit `cn`, confirm it stops; clear `cn`, confirm
  it re-arms.
