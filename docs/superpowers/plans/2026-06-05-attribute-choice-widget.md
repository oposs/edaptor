# Attribute Choice Widget Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a generic, config-driven `[profile.widget.<attr>]` "choice" widget that lets you pick from a fixed vocabulary and (de)serialise a single attribute string — wired for `sambaAcctFlags` (multi, bracketed) and `loginShell` (single, plain).

**Architecture:** A closed palette modelled as enums (no trait/registry), mirroring the existing picker `resolve → store-on-App → tag-fields` pipeline. All token logic (parse, serialise, merge-from-original commit, presentation summary) lives in **pure functions** that are unit-tested without the TUI; the editor overlay reuses the existing `ValueEditor` picker machinery with a static option source and calls those pure helpers on commit.

**Tech Stack:** Rust, ratatui, tui-prompts (`TextState`), serde/toml, `cargo test`.

**Spec:** `docs/superpowers/specs/2026-06-05-attribute-choice-widget-design.md`

---

## File structure

| File | Responsibility | New/Modify |
|---|---|---|
| `src/samba/account.rs` | `parse_bracketed` / `serialize_bracketed` (lossless, canonical order); `samba_acct_flags` reimplemented on top | Modify |
| `src/config/mod.rs` | `EntryProfile.widgets` + `WidgetSpecCfg` / `ChoiceOption` serde | Modify |
| `src/config/widget.rs` | `ChoiceFormat`, `ChoiceWidget`, `ResolvedWidget`, `resolve_widgets`, `widget_for`, + the pure choice helpers (`seed_checked`, `commit_value`, `present_summary`) | New |
| `src/config/mod.rs` (mod decl) | `pub mod widget;` | Modify |
| `src/ui/edit_form.rs` | `EditField.widget_choice`; `tag_widget_fields`; keep choice fields editable | Modify |
| `src/ui/app/mod.rs` | `App.widgets`; `resolve_widgets` in `run`; export | Modify |
| `src/ui/app/action.rs`, `src/ui/app/create.rs` | call `tag_widget_fields` after `tag_picker_fields` | Modify |
| `src/ui/app/value_editor.rs` | `ValueEditor.choice` + `original`; `open_choice`; commit branch | Modify |
| `src/ui/app/input.rs` | Enter on a choice field opens the choice overlay | Modify |
| `src/ui/view.rs` | suppress search box for static source; `field_display_value` summary | Modify |
| `examples/demo-config.toml`, `examples/config.toml` | widget presets | Modify |
| `docs/src/**` | document `[profile.widget.<attr>]` | Modify |

> **Important codebase fact:** `EditField` has **no `Default`** and is built via explicit struct literals in many places (`build_edit_form`, `inject_password_fields`'s `mk` closure, and ~9 `#[cfg(test)]` literals across `edit_form.rs` and `value_editor.rs`). Adding the `widget_choice` field (Task 5) requires adding `widget_choice: None` to **every** literal or the crate won't compile. Use `cargo build` to find them all.

---

## Task 1: Samba bracketed parse/serialize (pure)

**Files:**
- Modify: `src/samba/account.rs`
- Test: `src/samba/account.rs` (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Write failing tests**

Add to the `tests` module in `src/samba/account.rs`:

```rust
#[test]
fn bracketed_round_trips_and_is_canonical() {
    // parse drops brackets + padding, keeps the letter set
    let set = parse_bracketed("[DU         ]");
    assert!(set.contains(&'D') && set.contains(&'U') && set.len() == 2);
    // serialize emits canonical order N D H T U M W S L X I, 11-wide, bracketed
    assert_eq!(serialize_bracketed(&set), "[DU         ]");
    // out-of-order input still serialises canonically (D before U)
    let mut s = std::collections::BTreeSet::new();
    s.insert('U');
    s.insert('D');
    assert_eq!(serialize_bracketed(&s), "[DU         ]");
    // width is always 11 interior (13 total)
    assert_eq!(serialize_bracketed(&s).len(), 13);
}

#[test]
fn bracketed_is_lossless_for_unmanaged_letters() {
    // W (workstation trust) is preserved even though the UI never surfaces it
    let set = parse_bracketed("[UXW        ]");
    assert!(set.contains(&'W'));
    assert_eq!(serialize_bracketed(&set), "[UWX        ]"); // canonical: U,W,X
}

#[test]
fn bracketed_tolerates_missing_brackets_and_empty() {
    assert!(parse_bracketed("U").contains(&'U'));
    assert_eq!(serialize_bracketed(&std::collections::BTreeSet::new()), "[           ]");
}

#[test]
fn samba_acct_flags_golden_unchanged() {
    assert_eq!(samba_acct_flags(false), "[U          ]");
    assert_eq!(samba_acct_flags(true), "[DU         ]");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib samba::account::tests::bracketed 2>&1 | tail -20`
Expected: FAIL — `cannot find function parse_bracketed`.

- [ ] **Step 3: Implement parse/serialize and reimplement `samba_acct_flags`**

In `src/samba/account.rs`, add near the top of the module (after the `use` lines):

```rust
use std::collections::BTreeSet;

/// The 11 Samba ACB flag letters in canonical `pdb_encode_acct_ctrl` order.
/// The interior of `sambaAcctFlags` is exactly this wide (11), which is why a
/// fully-flagged account is `[NDHTUMWSLXI]`.
const ACB_ORDER: [char; 11] = ['N', 'D', 'H', 'T', 'U', 'M', 'W', 'S', 'L', 'X', 'I'];

/// Parse a `sambaAcctFlags` value into the set of present letters. Tolerant of
/// missing brackets; padding spaces are dropped; unknown letters are kept
/// (lossless). Case-sensitive (Samba letters are uppercase).
pub fn parse_bracketed(s: &str) -> BTreeSet<char> {
    let inner = s.trim().strip_prefix('[').unwrap_or(s.trim());
    let inner = inner.strip_suffix(']').unwrap_or(inner);
    inner.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Serialise a letter set to the canonical bracketed form: known letters in
/// `ACB_ORDER`, then any unknown letters (sorted) for losslessness, left-
/// justified to width 11 inside `[`…`]`.
pub fn serialize_bracketed(set: &BTreeSet<char>) -> String {
    let mut letters: String = ACB_ORDER.iter().filter(|c| set.contains(c)).collect();
    let mut unknown: Vec<char> = set.iter().copied().filter(|c| !ACB_ORDER.contains(c)).collect();
    unknown.sort_unstable();
    letters.extend(unknown);
    format!("[{letters:<11}]")
}
```

Then replace the body of `samba_acct_flags` so create + edit share one serialiser:

```rust
pub fn samba_acct_flags(disabled: bool) -> String {
    let mut set = BTreeSet::new();
    set.insert('U'); // normal user account is always set on create
    if disabled {
        set.insert('D');
    }
    serialize_bracketed(&set)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib samba::account 2>&1 | tail -20`
Expected: PASS — all `bracketed_*` and `samba_acct_flags_golden_unchanged` green, plus the pre-existing account tests still pass.

- [ ] **Step 5: Commit**

```bash
git add src/samba/account.rs
git commit -m "feat(samba): lossless sambaAcctFlags parse/serialize; reuse for samba_acct_flags"
```

---

## Task 2: Config serde — `[profile.widget.<attr>]`

**Files:**
- Modify: `src/config/mod.rs`
- Test: `src/config/mod.rs` (`#[cfg(test)]`)

- [ ] **Step 1: Write a failing deserialisation test**

Add to the config tests in `src/config/mod.rs`:

```rust
#[test]
fn parses_profile_widget_table() {
    let toml = r#"
[server]
uri = "ldap://x"
base_dn = "dc=x"
bind_dn = "cn=a,dc=x"

[[profile]]
name = "user"
object_classes = ["inetOrgPerson"]

[profile.widget.sambaAcctFlags]
kind = "choice"
select = "multi"
format = "bracketed"
options = [ { value = "D", label = "Disabled" }, { value = "X", label = "No expire" } ]

[profile.widget.loginShell]
kind = "choice"
select = "single"
format = "plain"
options = [ { value = "/bin/bash", label = "Bash" } ]
"#;
    let cfg: Config = toml::from_str(toml).expect("parse");
    let p = &cfg.profiles[0];
    assert_eq!(p.widgets.len(), 2);
    let WidgetSpecCfg::Choice { select, format, options } = &p.widgets["sambaAcctFlags"];
    assert_eq!(select, "multi");
    assert_eq!(format, "bracketed");
    assert_eq!(options[0].value, "D");
    assert_eq!(options[0].label, "Disabled");
}
```

(Adjust the `[server]` keys to match the crate's actual `ServerConfig` required fields — check the top of `src/config/mod.rs`; copy the minimal shape used by an existing config test.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib config::tests::parses_profile_widget_table 2>&1 | tail -20`
Expected: FAIL — no field `widgets` / no type `WidgetSpecCfg`.

- [ ] **Step 3: Add the serde model**

In `src/config/mod.rs`, add the structs (near `PickerSpec`):

```rust
/// One option in a `choice` widget: the stored token and its UI label.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
pub struct ChoiceOption {
    /// The token stored in the encoded value (a samba letter, a shell path, …).
    pub value: String,
    /// The human-facing label shown in the checklist and the summary.
    pub label: String,
}

/// A `[profile.widget.<attr>]` binding. `kind`-tagged so future widget kinds add
/// variants without breaking existing config.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum WidgetSpecCfg {
    /// Pick from a fixed vocabulary; (de)serialise a single attribute string.
    Choice {
        /// `"single"` or `"multi"`.
        select: String,
        /// `"plain"` | `"bracketed"` (now); `"bitmask"` | `"delimited"` (reserved).
        format: String,
        /// The selectable options (non-empty; validated at resolve time).
        options: Vec<ChoiceOption>,
    },
}
```

Add the field to `EntryProfile` (right after `pickers`):

```rust
    /// Per-attribute rich-widget bindings (`[profile.widget.<attr>]`).
    #[serde(default, rename = "widget")]
    pub widgets: std::collections::BTreeMap<String, WidgetSpecCfg>,
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test --lib config::tests::parses_profile_widget_table 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/config/mod.rs
git commit -m "feat(config): parse [profile.widget.<attr>] choice bindings"
```

---

## Task 3: Resolve widgets + pure choice helpers

**Files:**
- Create: `src/config/widget.rs`
- Modify: `src/config/mod.rs` (add `pub mod widget;`)
- Test: `src/config/widget.rs` (`#[cfg(test)]`)

- [ ] **Step 1: Declare the module**

In `src/config/mod.rs`, add alongside the other `pub mod` lines (e.g. near `pub mod relation;`):

```rust
pub mod widget;
```

- [ ] **Step 2: Write failing tests**

Create `src/config/widget.rs` with the tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ChoiceOption, EntryProfile, WidgetSpecCfg};

    fn profile_with(attr: &str, select: &str, format: &str, opts: &[(&str, &str)]) -> EntryProfile {
        let mut p = EntryProfile::default();
        p.name = "user".into();
        p.object_classes = vec!["inetOrgPerson".into()];
        p.widgets.insert(
            attr.into(),
            WidgetSpecCfg::Choice {
                select: select.into(),
                format: format.into(),
                options: opts
                    .iter()
                    .map(|(v, l)| ChoiceOption { value: v.to_string(), label: l.to_string() })
                    .collect(),
            },
        );
        p
    }

    #[test]
    fn resolves_bracketed_and_plain() {
        let profiles = vec![
            profile_with("sambaAcctFlags", "multi", "bracketed", &[("D", "Disabled")]),
        ];
        let resolved = resolve_widgets(&profiles).expect("ok");
        let w = widget_for(&resolved, &["inetOrgPerson".into()], "sambaacctflags").unwrap();
        assert_eq!(w.select, crate::config::relation::Cardinality::Multi);
        assert!(matches!(w.format, ChoiceFormat::Bracketed));
    }

    #[test]
    fn rejects_empty_options_and_unknown_format() {
        let p_empty = profile_with("a", "single", "plain", &[]);
        assert!(resolve_widgets(&vec![p_empty]).is_err());
        let p_bad = profile_with("a", "single", "nope", &[("x", "X")]);
        assert!(resolve_widgets(&vec![p_bad]).is_err());
    }

    #[test]
    fn reserved_formats_error_until_wired() {
        let p = profile_with("a", "multi", "bitmask", &[("x", "X")]);
        assert!(resolve_widgets(&vec![p]).is_err());
    }

    #[test]
    fn bracketed_commit_merges_from_original_and_preserves_unmanaged() {
        // config manages only D and X; original carries U and W
        let w = ChoiceWidget {
            select: crate::config::relation::Cardinality::Multi,
            format: ChoiceFormat::Bracketed,
            options: vec![
                ChoiceOption { value: "D".into(), label: "Disabled".into() },
                ChoiceOption { value: "X".into(), label: "No expire".into() },
            ],
        };
        // seed: which options are checked given the original value
        let checked = w.seed_checked("[UW         ]");
        assert!(checked.is_empty(), "neither D nor X set originally");
        // commit with D newly checked → U and W preserved, D added (canonical)
        let v = w.commit_value("[UW         ]", &["D".to_string()]);
        assert_eq!(v, "[DUW        ]");
    }

    #[test]
    fn plain_commit_replaces_value_and_summarises() {
        let w = ChoiceWidget {
            select: crate::config::relation::Cardinality::Single,
            format: ChoiceFormat::Plain,
            options: vec![
                ChoiceOption { value: "/bin/bash".into(), label: "Bash".into() },
                ChoiceOption { value: "/bin/sh".into(), label: "POSIX sh".into() },
            ],
        };
        assert_eq!(w.seed_checked("/bin/sh"), vec!["/bin/sh".to_string()]);
        assert_eq!(w.commit_value("/bin/bash", &["/bin/sh".to_string()]), "/bin/sh");
        assert_eq!(w.present_summary("/bin/sh"), "POSIX sh");
        assert_eq!(w.present_summary("/bin/zsh"), "/bin/zsh"); // off-list → raw
    }

    #[test]
    fn bracketed_summary_joins_set_labels() {
        let w = ChoiceWidget {
            select: crate::config::relation::Cardinality::Multi,
            format: ChoiceFormat::Bracketed,
            options: vec![
                ChoiceOption { value: "D".into(), label: "Disabled".into() },
                ChoiceOption { value: "X".into(), label: "No expire".into() },
            ],
        };
        assert_eq!(w.present_summary("[DU         ]"), "Disabled");
        assert_eq!(w.present_summary("[U          ]"), "—");
    }
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test --lib config::widget 2>&1 | tail -20`
Expected: FAIL — types/functions not defined.

- [ ] **Step 4: Implement `src/config/widget.rs`**

Prepend (above the `tests` mod) the implementation:

```rust
//! Resolved `[profile.widget.<attr>]` choice widgets + the pure token logic
//! (parse/serialise/commit/summary). Mirrors `config::relation` for pickers.

use std::collections::BTreeSet;

use crate::config::relation::Cardinality;
use crate::config::{ChoiceOption, EntryProfile, WidgetSpecCfg};

/// How a choice widget's value string is encoded. `Bitmask`/`Delimited` are
/// reserved — they parse in config but error at resolve time until wired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChoiceFormat {
    /// Single token; the value *is* the chosen option (e.g. `loginShell`).
    Plain,
    /// Samba `sambaAcctFlags`-style bracketed letters (owned by `samba::account`).
    Bracketed,
}

/// A resolved, ready-to-use choice widget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceWidget {
    pub select: Cardinality,
    pub format: ChoiceFormat,
    pub options: Vec<ChoiceOption>,
}

/// A resolved widget bound to its owning profile's object classes (for matching).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedWidget {
    pub owner_object_classes: Vec<String>,
    pub attr: String,
    pub widget: ChoiceWidget,
}

/// Resolve every `[profile.widget.*]`. Returns `Err(msg)` on an invalid binding
/// (empty options, unknown select/format, or a reserved-but-unwired format) so
/// the operator sees a loud config error rather than a silent no-op.
pub fn resolve_widgets(profiles: &[EntryProfile]) -> Result<Vec<ResolvedWidget>, String> {
    let mut out = Vec::new();
    for owner in profiles {
        for (attr, spec) in &owner.widgets {
            let WidgetSpecCfg::Choice { select, format, options } = spec;
            if options.is_empty() {
                return Err(format!("[profile.widget.{attr}]: options must not be empty"));
            }
            let select = match select.to_ascii_lowercase().as_str() {
                "single" => Cardinality::Single,
                "multi" => Cardinality::Multi,
                other => return Err(format!("[profile.widget.{attr}]: bad select \"{other}\"")),
            };
            let format = match format.to_ascii_lowercase().as_str() {
                "plain" => ChoiceFormat::Plain,
                "bracketed" => ChoiceFormat::Bracketed,
                "bitmask" | "delimited" => {
                    return Err(format!(
                        "[profile.widget.{attr}]: format \"{format}\" not yet implemented"
                    ))
                }
                other => return Err(format!("[profile.widget.{attr}]: bad format \"{other}\"")),
            };
            out.push(ResolvedWidget {
                owner_object_classes: owner.object_classes.clone(),
                attr: attr.clone(),
                widget: ChoiceWidget { select, format, options: options.clone() },
            });
        }
    }
    Ok(out)
}

/// The choice widget for `(entry object classes, attr)`, if any. `.any()` owner
/// objectClass overlap, matching `picker_for`.
pub fn widget_for<'a>(
    widgets: &'a [ResolvedWidget],
    ocs: &[String],
    attr: &str,
) -> Option<&'a ChoiceWidget> {
    widgets
        .iter()
        .find(|w| {
            w.attr.eq_ignore_ascii_case(attr)
                && w.owner_object_classes
                    .iter()
                    .any(|oc| ocs.iter().any(|e| e.eq_ignore_ascii_case(oc)))
        })
        .map(|w| &w.widget)
}

impl ChoiceWidget {
    /// Parse `value` into the present-token set (format-specific).
    fn parse(&self, value: &str) -> BTreeSet<String> {
        match self.format {
            ChoiceFormat::Plain => {
                if value.trim().is_empty() {
                    BTreeSet::new()
                } else {
                    [value.trim().to_string()].into_iter().collect()
                }
            }
            ChoiceFormat::Bracketed => crate::samba::account::parse_bracketed(value)
                .into_iter()
                .map(|c| c.to_string())
                .collect(),
        }
    }

    /// Serialise a present-token set back to the encoded value.
    fn serialize(&self, set: &BTreeSet<String>) -> String {
        match self.format {
            ChoiceFormat::Plain => set.iter().next().cloned().unwrap_or_default(),
            ChoiceFormat::Bracketed => {
                let chars: BTreeSet<char> = set.iter().filter_map(|s| s.chars().next()).collect();
                crate::samba::account::serialize_bracketed(&chars)
            }
        }
    }

    /// Which option `value`s should be pre-checked when opening the editor over
    /// `current` (the option values whose token is present).
    pub fn seed_checked(&self, current: &str) -> Vec<String> {
        let present = self.parse(current);
        self.options
            .iter()
            .map(|o| o.value.clone())
            .filter(|v| present.contains(v))
            .collect()
    }

    /// Assemble the new encoded value: seed from `current` (lossless — preserves
    /// tokens the UI never surfaced), then set/clear only the configured options
    /// per `checked`. For single-select, `checked` holds at most one value.
    pub fn commit_value(&self, current: &str, checked: &[String]) -> String {
        let mut set = self.parse(current);
        if matches!(self.select, Cardinality::Single) {
            // single replaces the whole value: drop every configured option first
            for o in &self.options {
                set.remove(&o.value);
            }
        }
        for o in &self.options {
            if checked.iter().any(|c| c == &o.value) {
                set.insert(o.value.clone());
            } else {
                set.remove(&o.value);
            }
        }
        self.serialize(&set)
    }

    /// Read-only summary: the labels of present options joined with `, `, or the
    /// raw value when nothing matches (off-list plain), or `—` when empty.
    pub fn present_summary(&self, current: &str) -> String {
        let present = self.parse(current);
        let labels: Vec<&str> = self
            .options
            .iter()
            .filter(|o| present.contains(&o.value))
            .map(|o| o.label.as_str())
            .collect();
        if !labels.is_empty() {
            labels.join(", ")
        } else if matches!(self.format, ChoiceFormat::Plain) && !current.trim().is_empty() {
            current.trim().to_string() // off-list value: show it raw
        } else {
            "—".to_string()
        }
    }
}
```

> Note: `samba::account::parse_bracketed`/`serialize_bracketed` are `pub` from Task 1, so `config::widget` can call across modules. Confirm `pub mod samba;`/`pub mod account;` visibility allows it (they are already `pub` in the crate).

- [ ] **Step 5: Run to verify it passes**

Run: `cargo test --lib config::widget 2>&1 | tail -30`
Expected: PASS — all tests green.

- [ ] **Step 6: Commit**

```bash
git add src/config/mod.rs src/config/widget.rs
git commit -m "feat(config): resolve_widgets + pure choice parse/serialize/commit/summary"
```

---

## Task 4: `EditField.widget_choice` + `tag_widget_fields`

**Files:**
- Modify: `src/ui/edit_form.rs`
- Test: `src/ui/edit_form.rs` (`#[cfg(test)]`)

- [ ] **Step 1: Add the field to `EditField`**

In `src/ui/edit_form.rs`, add to the `EditField` struct (after `picker`):

```rust
    /// `Some` when bound to a `[profile.widget.<attr>]` choice widget.
    pub widget_choice: Option<crate::config::widget::ChoiceWidget>,
```

- [ ] **Step 2: Make the crate compile again (add `widget_choice: None` everywhere)**

Run `cargo build 2>&1 | grep -A2 "missing field"` to list every `EditField { … }` literal, and add `widget_choice: None,` to each: `build_edit_form` (~line 305), the `mk` closure in `inject_password_fields` (~line 343), and every `#[cfg(test)]` literal in `edit_form.rs` and `value_editor.rs`.

Run: `cargo build 2>&1 | tail -5`
Expected: builds clean.

- [ ] **Step 3: Write a failing test for `tag_widget_fields`**

Add to the `edit_form.rs` tests:

```rust
#[test]
fn tag_widget_fields_attaches_matching_choice() {
    use crate::config::widget::{ChoiceFormat, ChoiceWidget, ResolvedWidget};
    use crate::config::relation::Cardinality;
    use crate::config::ChoiceOption;

    let mut form = writable_form(); // has a userPassword field; add a generic one is unneeded
    // ensure there is a field named "loginShell" to tag
    form.fields.push(EditField {
        label: "loginShell".into(),
        must: false,
        editable: true,
        multi: false,
        secret: false,
        ordered: false,
        values: vec!["/bin/bash".into()],
        kind: crate::schema::FieldKind::Text,
        widget: crate::ui::form::WidgetSpec::ReadOnlyText,
        editor: TextState::new().with_value("/bin/bash".to_string()),
        picker: None,
        widget_choice: None,
    });
    let widgets = vec![ResolvedWidget {
        owner_object_classes: vec!["demoPerson".into()],
        attr: "loginShell".into(),
        widget: ChoiceWidget {
            select: Cardinality::Single,
            format: ChoiceFormat::Plain,
            options: vec![ChoiceOption { value: "/bin/bash".into(), label: "Bash".into() }],
        },
    }];
    tag_widget_fields(&mut form, &widgets, &["demoPerson".to_string()], false);
    let f = form.fields.iter().find(|f| f.label == "loginShell").unwrap();
    assert!(f.widget_choice.is_some());
    assert!(f.editable, "a choice field stays editable");
}
```

- [ ] **Step 4: Run to verify it fails**

Run: `cargo test --lib edit_form::tests::tag_widget_fields_attaches 2>&1 | tail -20`
Expected: FAIL — `tag_widget_fields` not found.

- [ ] **Step 5: Implement `tag_widget_fields`**

In `src/ui/edit_form.rs`, near `tag_picker_fields`:

```rust
/// Attach a `[profile.widget.<attr>]` choice widget to each matching field. A
/// choice field stays editable (Enter opens the choice overlay). Mirrors
/// `tag_picker_fields`; `.any()` objectClass matching via `widget_for`.
pub fn tag_widget_fields(
    form: &mut EditForm,
    widgets: &[crate::config::widget::ResolvedWidget],
    object_classes: &[String],
    read_only: bool,
) {
    if read_only {
        return;
    }
    for field in &mut form.fields {
        if let Some(w) =
            crate::config::widget::widget_for(widgets, object_classes, &field.label)
        {
            field.widget_choice = Some(w.clone());
            field.editable = true;
        }
    }
}
```

- [ ] **Step 6: Run to verify it passes**

Run: `cargo test --lib edit_form 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add src/ui/edit_form.rs src/ui/app/value_editor.rs
git commit -m "feat(ui): EditField.widget_choice + tag_widget_fields"
```

---

## Task 5: Wire resolve into `App` and the form-build seams

**Files:**
- Modify: `src/ui/app/mod.rs`, `src/ui/app/action.rs`, `src/ui/app/create.rs`

- [ ] **Step 1: Add `App.widgets` and resolve it in `run`**

In `src/ui/app/mod.rs`: add a field to the `App` struct (near `pub pickers`):

```rust
    pub widgets: Vec<crate::config::widget::ResolvedWidget>,
```

In `run`, after `let pickers = resolve_pickers(&config.profiles);` (~line 128):

```rust
    let widgets = crate::config::widget::resolve_widgets(&config.profiles)
        .map_err(|e| anyhow::anyhow!("config error: {e}"))?;
```

And add `widgets,` to the `App { … }` initialiser (near `pickers,`).

- [ ] **Step 2: Call `tag_widget_fields` in `build_loaded_form`**

In `src/ui/app/action.rs::build_loaded_form`, the signature must receive widgets. Add a parameter `widgets: &[crate::config::widget::ResolvedWidget]` and, right after the existing `tag_picker_fields(...)` call (~line 492), add:

```rust
    crate::ui::edit_form::tag_widget_fields(&mut form, widgets, &ocs, read_only);
```

Update every caller of `build_loaded_form` to pass `&app.widgets` (use `cargo build` to find them — read flow + post-combined-save reload).

- [ ] **Step 3: Call `tag_widget_fields` in the create form**

In `src/ui/app/create.rs` (~line 147, after `tag_picker_fields`), add the same call, threading widgets from the caller (`&app.widgets`). Update its callers.

- [ ] **Step 4: Build + run the whole suite**

Run: `cargo build 2>&1 | tail -5 && cargo test --lib 2>&1 | tail -5`
Expected: builds; all existing tests still pass (no behaviour change yet — no field is tagged unless config declares a widget).

- [ ] **Step 5: Commit**

```bash
git add src/ui/app/mod.rs src/ui/app/action.rs src/ui/app/create.rs
git commit -m "feat(app): resolve choice widgets at startup and tag fields on build"
```

---

## Task 6: The choice overlay — open, commit, render

**Files:**
- Modify: `src/ui/app/value_editor.rs`, `src/ui/app/input.rs`, `src/ui/view.rs`
- Test: `src/ui/app/value_editor.rs` (`#[cfg(test)]`)

**Approach:** reuse `ValueEditor`'s picker mode with a *static* candidate list (the options) and no LDAP search. Carry the resolved `ChoiceWidget` + the original value on the editor; on Alt+S, call `ChoiceWidget::commit_value` and write the result into `field.editor` **and** `field.values`.

- [ ] **Step 1: Extend `ValueEditor` and add `open_choice`**

In `src/ui/app/value_editor.rs` (and the struct def in `src/ui/edit_form.rs` where `ValueEditor` lives), add two fields:

```rust
    /// `Some` ⇒ this editor is a static choice widget (no LDAP search).
    pub choice: Option<crate::config::widget::ChoiceWidget>,
    /// The field's original value, for the lossless merge-from-original commit.
    pub choice_original: String,
```

Add `choice: None, choice_original: String::new(),` to the existing `open_plain` and `open` constructors. Add a new constructor:

```rust
impl ValueEditor {
    /// Open a static choice editor over `field` at `field_idx`. Seeds the picker
    /// candidate list from the widget options and pre-selects the present ones.
    pub fn open_choice(
        field_idx: usize,
        field: &EditField,
        widget: &crate::config::widget::ChoiceWidget,
    ) -> Self {
        use crate::ui::picker::{Candidate, PickerState};
        let original = field.current_values().first().cloned().unwrap_or_default();
        let checked = widget.seed_checked(&original);
        let candidates: Vec<Candidate> = widget
            .options
            .iter()
            .map(|o| Candidate { dn: o.value.clone(), label: o.label.clone(), store_value: o.value.clone() })
            .collect();
        let selected: Vec<Candidate> = candidates
            .iter()
            .filter(|c| checked.iter().any(|v| v == &c.store_value))
            .cloned()
            .collect();
        let mut ve = ValueEditor::open_plain(field_idx, field); // base shell
        ve.label = field.label.clone();
        ve.picker = Some(PickerState::with_candidates(candidates, selected)); // see note
        ve.choice = Some(widget.clone());
        ve.choice_original = original;
        ve
    }
}
```

> **Note:** `PickerState::new` currently seeds from *selected* candidates and expects a live search to fill the rest. A static list needs the full candidate set present with the selected ones marked. Check `src/ui/picker.rs` for the constructor shape; if there is no `with_candidates`, add a small one that takes `(all, selected)` and skips the search-pending state. Keep it minimal.

- [ ] **Step 2: Open the overlay on Enter for a choice field**

In `src/ui/app/value_editor.rs::open_value_editor` (~line 24), branch **before** the picker/plain branches:

```rust
    if let Some(w) = field.widget_choice.clone() {
        let ve = ValueEditor::open_choice(focus, field, &w);
        app.overlay = Some(Overlay::ValueEditor(ve));
        return;
    }
```

(`field` and `focus` are already in scope there; match the existing borrow pattern.)

- [ ] **Step 3: Write a failing commit test**

Add to the `value_editor.rs` tests (model it on `lookup_alt_s_commits_scalar_to_field_editor`):

```rust
#[test]
fn choice_commit_writes_assembled_string_to_editor() {
    use crate::config::widget::{ChoiceFormat, ChoiceWidget};
    use crate::config::relation::Cardinality;
    use crate::config::ChoiceOption;
    // build an app whose focused field is sambaAcctFlags = "[U          ]"
    let widget = ChoiceWidget {
        select: Cardinality::Multi,
        format: ChoiceFormat::Bracketed,
        options: vec![ChoiceOption { value: "D".into(), label: "Disabled".into() }],
    };
    let mut app = app_with_choice_field("sambaAcctFlags", "[U          ]", &widget); // helper below
    open_value_editor(&mut app, &Structure::default());
    // toggle the only option (D) on, then commit
    value_editor_key(&mut app, key(KeyCode::Enter)); // toggle D in multi mode
    value_editor_key(&mut app, alt(KeyCode::Char('s')));
    let f = &app.form.as_ref().unwrap().fields[0];
    assert_eq!(f.editor.value(), "[DU         ]"); // U preserved, D added, canonical
    assert_eq!(f.current_values(), vec!["[DU         ]".to_string()]);
}
```

Write the small `app_with_choice_field` helper next to the existing `app_with_value_editor`, constructing a one-field form (field has `widget_choice: Some(widget.clone())`, `editable: true`, `multi: false`, `values=[value]`, editor seeded with `value`).

- [ ] **Step 4: Run to verify it fails**

Run: `cargo test --lib value_editor::tests::choice_commit_writes 2>&1 | tail -20`
Expected: FAIL — commit still uses the picker store path, not the choice serialiser.

- [ ] **Step 5: Implement the commit branch**

In the Alt+S commit handling inside `picker_editor_key` (the branch that today writes `field.values = picker.selected_values()` / single-select writes `editor`+`values`, ~value_editor.rs:72-115), add a guard at the top of the commit: if `ve.choice` is `Some`, compute and write the assembled string instead:

```rust
    if let Some(w) = ve.choice.clone() {
        let checked: Vec<String> = ve
            .picker
            .as_ref()
            .map(|p| p.selected_values())
            .unwrap_or_default();
        let value = w.commit_value(&ve.choice_original, &checked);
        if let Some(form) = app.form.as_mut() {
            if let Some(field) = form.fields.get_mut(ve.field) {
                field.editor = TextState::new().with_value(value.clone());
                field.values = if value.is_empty() { vec![] } else { vec![value] };
            }
        }
        app.overlay = None;
        return;
    }
```

(Match the exact borrow/`overlay.take()` pattern already used in that block; `selected_values()` already exists on `PickerState`.)

- [ ] **Step 6: Run to verify it passes**

Run: `cargo test --lib value_editor 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 7: Suppress the search box for a static source**

In `src/ui/view.rs::render_value_editor` (~line 414), the picker branch renders a `Search:` row. Gate it so a static choice editor (where `ve.choice.is_some()`) does **not** render the search row and starts the candidate list at the top. Also ensure key handling: in `value_editor_key`/`picker_editor_key`, when `ve.choice.is_some()`, route plain character keys to **toggle/navigation only** (no search box typing). The simplest implementation: treat `ve.choice.is_some()` like the existing "no search" cases — skip `service_picker_search`.

Run (visual self-check): `cargo test --lib 2>&1 | tail -5`
Expected: PASS (rendering covered by existing render tests; add one asserting no `Search:` line when `choice.is_some()` if a render-to-buffer test harness exists, mirroring `render_value_editor_single_select_uses_radio_markers`).

- [ ] **Step 8: Commit**

```bash
git add src/ui/app/value_editor.rs src/ui/app/input.rs src/ui/view.rs src/ui/edit_form.rs src/ui/picker.rs
git commit -m "feat(ui): static choice overlay — open, toggle, lossless commit"
```

---

## Task 7: Read-only presentation summary

**Files:**
- Modify: `src/ui/view.rs`
- Test: `src/ui/view.rs` (`#[cfg(test)]`)

- [ ] **Step 1: Write a failing test**

In `src/ui/view.rs` tests, build an `EditField` with `widget_choice: Some(bracketed D/X)` and `values = ["[DU         ]"]`, and assert:

```rust
assert_eq!(field_display_value(&fld), "Disabled");
```

(Use the existing test field-builder in `view.rs`; set `widget_choice` accordingly.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib view::tests 2>&1 | grep -i summary` (or the test name)
Expected: FAIL — still shows the raw value.

- [ ] **Step 3: Implement the summary branch**

In `field_display_value` (`src/ui/view.rs:296`), at the very top (before the `secret` and `multi` arms — a choice field is single-valued and non-secret), add:

```rust
    if let Some(w) = &fld.widget_choice {
        let current = fld.current_values().first().cloned().unwrap_or_default();
        return w.present_summary(&current);
    }
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test --lib view 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/ui/view.rs
git commit -m "feat(ui): render choice fields as a set-labels summary"
```

---

## Task 8: Config presets + docs

**Files:**
- Modify: `examples/demo-config.toml`, `examples/config.toml`, `docs/src/**`

- [ ] **Step 1: Add presets to `examples/demo-config.toml`**

Under the `name = "user"` profile (after `[profile.password]`):

```toml
[profile.widget.sambaAcctFlags]
kind   = "choice"
select = "multi"
format = "bracketed"
options = [
  { value = "D", label = "Disabled" },
  { value = "X", label = "Password never expires" },
  { value = "N", label = "No password required" },
]

[profile.widget.loginShell]
kind   = "choice"
select = "single"
format = "plain"
options = [
  { value = "/bin/bash",     label = "Bash" },
  { value = "/bin/sh",       label = "POSIX sh" },
  { value = "/sbin/nologin", label = "No login" },
]
```

Mirror into `examples/config.toml` if it has an equivalent user profile.

- [ ] **Step 2: Verify the example config still loads**

Run: `cargo test 2>&1 | tail -5` (if there's a config-loads-examples test) **or** `cargo run -- --config examples/demo-config.toml --check` if such a flag exists; otherwise add a unit test that `toml::from_str`-parses the example file and calls `resolve_widgets` without error.
Expected: parses + resolves clean.

- [ ] **Step 3: Document `[profile.widget.<attr>]`**

Add a section to the config reference under `docs/src/` (find the page documenting `[profile.picker.<attr>]` and add a sibling `[profile.widget.<attr>]` section: the `choice` kind, `select`, `format` (`plain`/`bracketed`; note `bitmask`/`delimited` reserved), `options`, and the lossless/preserve-unlisted behaviour). Link the design spec.

- [ ] **Step 4: Commit**

```bash
git add examples/ docs/
git commit -m "docs: ship sambaAcctFlags + loginShell choice presets and reference"
```

---

## Task 9: Live smoke test (manual, tmux)

**Files:** none (verification only)

- [ ] **Step 1: Start the test LDAP**

Run: `scripts/test-ldap.sh start`
Expected: provisioned OpenLDAP container up. (Per memory: `pkill -f edaptor` would kill this container — do not use it.)

- [ ] **Step 2: Run the app in tmux against the demo config and verify sambaAcctFlags**

Launch eDAPtor (per the project `run` skill / `edaptor-m4-handoff` memory), navigate to a `ou=people` user, focus `sambaAcctFlags` → it shows a summary (e.g. `—`), press Enter → checklist of Disabled/No-expire/No-pwd, toggle **Disabled**, Alt+S, then Alt+S to save. Re-read the entry.
Expected: value becomes `[DU         ]`; `U` preserved; field summary shows `Disabled`.

- [ ] **Step 3: Verify loginShell single-select**

Focus `loginShell`, Enter → radio list of shells, pick `No login`, Alt+S, save.
Expected: value becomes `/sbin/nologin`; summary shows `No login`. Pick a user whose shell is off-list and confirm a no-op commit preserves it.

- [ ] **Step 4: Verify no spurious dirty**

Navigate between several `ou=people` users **without** opening any editor.
Expected: no Save/Discard/Stay guard (confirms choice tagging didn't reintroduce a baseline mismatch).

- [ ] **Step 5: Final full test run + commit any fixups**

Run: `cargo test 2>&1 | tail -5`
Expected: all green.

---

## Self-review notes (author)

- **Spec coverage:** config seam (T2/T3/T5), bracketed+plain parse/serialize (T1/T3), merge-from-original commit (T3/T6), `current_values()`→`editor` checkpoint (T6 test), set-labels presentation (T7), `.any()` OC match (T3), reserved formats error (T3), samba refactor + golden (T1), demo presets + docs (T8), live smoke (T9). Future extension points are documented in the spec, intentionally unbuilt.
- **Open implementation decision (flagged in spec):** if threading a static source through `ValueEditor`'s picker mode entangles search/binding assumptions (T6 Step 1 note re `PickerState`), fall back to a dedicated minimal `Overlay::Choice` variant — the pure helpers (T1/T3) are unchanged either way, so only T6/T7's wiring shifts.
- **Type consistency:** `ChoiceWidget`/`ChoiceFormat`/`ResolvedWidget`/`ChoiceOption`/`WidgetSpecCfg`, `resolve_widgets`/`widget_for`/`tag_widget_fields`/`open_choice`/`commit_value`/`seed_checked`/`present_summary` are used identically across tasks.
