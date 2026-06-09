//! The editable form model.
//!
//! `FormModel`/`FormField` (`crate::ui::form`) are read-only-oriented and carry
//! no edit state, so the editable shape is net-new here: [`EditField`] adds the
//! `multi` / `secret` / `ordered` / `editable` flags and a `TextState` edit
//! engine, and [`EditForm`] groups them under a DN.
//!
//! P1 builds this for *display* (read-only). P2-T1 adds the `baseline`, the
//! set-wise dirty check, and `to_edit_entry()` — deliberately left out here so
//! that work stays a self-contained, unit-tested task.

use std::collections::BTreeMap;

use tui_prompts::{State, TextState};

use crate::config::relation::{PickerBinding, StoreKey};
use crate::form::changeset::{is_secret_attr, is_x_ordered, EditEntry};
use crate::schema::{FieldKind, SchemaModel};
use crate::ui::form::{FormField, FormModel, WidgetSpec};
use crate::ui::picker::{Candidate, PickerState};

/// One field of the editable form.
pub struct EditField {
    /// Attribute name (a `*` suffix marks MUST on render).
    pub label: String,
    /// Whether the attribute is in the effective MUST set.
    pub must: bool,
    /// Whether this field accepts edits (read-only kinds and global read-only
    /// mode force this false).
    pub editable: bool,
    /// Whether the attribute is multi-valued (edited via the value-editor popup).
    pub multi: bool,
    /// Whether the attribute holds a secret (rendered masked, never in clear).
    pub secret: bool,
    /// Whether the attribute is X-ORDERED (the `{n}` prefix makes order matter).
    pub ordered: bool,
    /// The attribute's current string values (display order).
    pub values: Vec<String>,
    /// The classified syntax (drives read-only display formatting).
    pub kind: FieldKind,
    /// The read-only widget choice (checkbox / binary note formatting).
    pub widget: WidgetSpec,
    /// Inline single-value edit state, seeded from `values[0]`. The Unicode-correct
    /// edit engine (tui-prompts); rendering is done by hand so the pane owns its bg.
    pub editor: TextState<'static>,
    /// `Some` when bound to a `[profile.widget.<attr>]` widget (choice or password).
    pub widget_binding: Option<crate::config::widget::WidgetKind>,
    /// True when this attribute is no longer permitted by the current objectClasses.
    /// Rendered CROSSED_OUT+DIM. current_values() returns [] → diff emits Delete.
    pub orphaned: bool,
}

impl EditField {
    /// The field's value set as currently edited.
    ///
    /// - orphaned field → `[]` (the diff will emit a Delete regardless of editor state);
    /// - multi field → `values` (the multi-value popup writes edits back there);
    /// - single + editable → the live editor, trimmed; an emptied field yields no
    ///   values so the diff emits a delete (not an empty value);
    /// - single + not editable → the original `values` (read-only kinds are kept).
    pub fn current_values(&self) -> Vec<String> {
        if self.orphaned {
            return vec![];
        }
        if self.multi {
            self.values.clone()
        } else if self.editable {
            let v = self.editor.value().trim();
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

/// The bound picker, if this field carries a `kind = "picker"`/`"membership"`
/// widget. `None` for choice/password/plain fields.
fn widget_picker(f: &EditField) -> Option<&crate::config::relation::PickerBinding> {
    match &f.widget_binding {
        Some(crate::config::widget::WidgetKind::Picker(b)) => Some(b),
        _ => None,
    }
}

/// The fan-out back-ref attr for a field (a `kind = "membership"` widget), if any.
pub(crate) fn fanout_attr_of(f: &EditField) -> Option<&str> {
    widget_picker(f).and_then(|b| b.fanout_attr.as_deref())
}

/// The pop-up editor for a multi-valued attribute: one inline edit row per
/// value, with insert / delete / reorder. Lives as an overlay over pane 3; on
/// commit its [`committed_values`](ValueEditor::committed_values) replace the
/// field's `values`. In **picker mode** (`picker.is_some()`) the rows are
/// replaced by a searchable candidate list. (Spike `ValueEditor`.)
pub struct ValueEditor {
    /// Index of the field being edited (into [`EditForm::fields`]).
    pub field: usize,
    /// The attribute name (shown in the popup title).
    pub label: String,
    /// Whether the attribute is X-ORDERED (order matters → shown in the hint).
    pub ordered: bool,
    /// Whether the attribute is secret (rows render masked).
    pub secret: bool,
    /// One edit state per value row (free-text mode only; empty in picker mode).
    pub rows: Vec<TextState<'static>>,
    /// The selected row index (free-text mode only).
    pub sel: usize,
    /// First visible row — scroll offset for the free-text value list, kept in
    /// sync with `sel` by the renderer so the selected row stays on screen with
    /// many values (e.g. a posixGroup's `memberUid`).
    pub scroll: usize,
    /// `Some` in picker mode; `None` for the free-text editor.
    pub picker: Option<PickerState>,
    /// The picker's incremental-search box (Unicode-correct edit engine).
    pub search: TextState<'static>,
    /// The resolved picker binding driving this editor's search/commit (unified
    /// path). `None` for the plain free-text multi-value editor. Boxed to keep
    /// `ValueEditor` (and thus the `Overlay` enum) small.
    pub binding: Option<Box<crate::config::relation::PickerBinding>>,
    /// `Some` ⇒ a static choice widget editor (no LDAP search). Reuses the
    /// picker UI (a candidate list) but the candidates are the widget's fixed
    /// options and the commit assembles the encoded string via the pure helper.
    pub choice: Option<crate::config::widget::ChoiceWidget>,
    /// The field's original value, for the lossless merge-from-original commit.
    pub choice_original: String,
    /// True when this editor manages the objectClass field (schema-seeded picker;
    /// no LDAP search). Triggers `sync_schema_fields` on commit via
    /// `App::objectclass_sync_pending`.
    pub objectclass: bool,
}

impl ValueEditor {
    /// Open a plain free-text multi-value editor over `field` (at `field_idx`),
    /// seeding one row per value. No picker.
    pub fn open_plain(field_idx: usize, field: &EditField) -> Self {
        let rows = field
            .values
            .iter()
            .map(|v| TextState::new().with_value(v.clone()))
            .collect();
        ValueEditor {
            field: field_idx,
            label: field.label.clone(),
            ordered: field.ordered,
            secret: field.secret,
            rows,
            sel: 0,
            scroll: 0,
            picker: None,
            search: TextState::new(),
            binding: None,
            choice: None,
            choice_original: String::new(),
            objectclass: false,
        }
    }

    /// Open the picker for a `[profile.widget.<attr>]` picker-bound field. Seeds the
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
                dn: v.clone(),
                label: v.clone(),
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
            binding: Some(Box::new(binding.clone())),
            choice: None,
            choice_original: String::new(),
            objectclass: false,
        }
    }

    /// Open a static choice editor for a `[profile.widget.<attr>]`-bound field.
    /// Reuses the picker UI but seeds the candidate list from the widget's fixed
    /// options (no LDAP search): `results` holds every option, `selected` the
    /// currently-checked subset (per [`ChoiceWidget::seed_checked`]). `saved` is
    /// left empty — a static list has no saved/"will be removed" semantics — and
    /// `key_ci` is false so option tokens compare exactly. Toggling reorders
    /// checked options to the top (acceptable for a small fixed list).
    pub fn open_choice(
        field_idx: usize,
        field: &EditField,
        widget: &crate::config::widget::ChoiceWidget,
    ) -> Self {
        let original = field.current_values().first().cloned().unwrap_or_default();
        let checked = widget.seed_checked(&original);
        let all: Vec<Candidate> = widget
            .options
            .iter()
            .map(|o| Candidate {
                dn: o.value.clone(),
                label: o.label.clone(),
                store_value: o.value.clone(),
            })
            .collect();
        let selected: Vec<Candidate> = all
            .iter()
            .filter(|c| checked.iter().any(|v| v == &c.store_value))
            .cloned()
            .collect();
        let picker = PickerState {
            selected,
            results: all,
            saved: Vec::new(),
            cursor: 0,
            scroll: 0,
            search_active: false,
            truncated: false,
            key_ci: false,
        };
        ValueEditor {
            field: field_idx,
            label: field.label.clone(),
            ordered: false,
            secret: field.secret,
            rows: Vec::new(),
            sel: 0,
            scroll: 0,
            picker: Some(picker),
            search: TextState::new(),
            binding: None,
            choice: Some(widget.clone()),
            choice_original: original,
            objectclass: false,
        }
    }

    /// Open the objectClass picker. Candidates are empty on open; `service_picker_search`
    /// populates them from the schema on the first tick via `PICKER_INIT_QUERY` sentinel.
    /// The currently-selected OC names are pre-ticked in the picker's `selected` list.
    pub fn open_objectclass(field_idx: usize, field: &EditField) -> Self {
        let selected: Vec<Candidate> = field
            .values
            .iter()
            .map(|v| Candidate {
                dn: v.clone(),
                label: v.clone(),
                store_value: v.clone(),
            })
            .collect();
        let picker = PickerState {
            selected,
            results: Vec::new(), // populated by service_picker_search on first tick
            saved: Vec::new(),
            cursor: 0,
            scroll: 0,
            search_active: false,
            truncated: false,
            key_ci: true, // OC names are case-insensitive
        };
        ValueEditor {
            field: field_idx,
            label: field.label.clone(),
            ordered: false,
            secret: false,
            rows: Vec::new(),
            sel: 0,
            scroll: 0,
            picker: Some(picker),
            search: TextState::new(),
            binding: None,
            choice: None,
            choice_original: String::new(),
            objectclass: true,
        }
    }

    /// The values to write back on commit: each row trimmed, blank rows dropped.
    pub fn committed_values(&self) -> Vec<String> {
        // Picker mode commits DNs via picker_editor_key (Task 4.4); `rows` is
        // always empty in picker mode, so this path must not be used for it.
        debug_assert!(
            self.picker.is_none(),
            "committed_values called in picker mode"
        );
        self.rows
            .iter()
            .map(|r| r.value().trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }
}

/// Whether the form edits an existing entry or composes a new one.
pub enum FormMode {
    /// Editing an entry already in the directory (diff against `baseline`).
    Edit,
    /// Composing a new entry of `profile_idx`, to be added under `container`.
    Create {
        profile_idx: usize,
        container: String,
    },
}

/// The editable form for one entry.
pub struct EditForm {
    /// The entry's distinguished name.
    pub dn: String,
    /// The ordered fields.
    pub fields: Vec<EditField>,
    /// Immutable snapshot of the original server values (label → values), the
    /// reference the dirty check compares the current edits against.
    pub baseline: BTreeMap<String, Vec<String>>,
    /// Edit an existing entry, or compose a new one (Create → Add on save).
    pub mode: FormMode,
    /// A password change staged by the PasswordEditor popup (cleartext), pending
    /// the next save. The password fields are read-only, so the new value cannot
    /// live in a field editor; it lives here. Cleared on save/revert.
    pub pending_password: Option<String>,
}

impl EditForm {
    /// True when this form composes a not-yet-saved new entry.
    pub fn is_new(&self) -> bool {
        matches!(self.mode, FormMode::Create { .. })
    }

    /// The entry as currently edited, in the shape the save path's
    /// [`crate::form::changeset::diff`] consumes.
    ///
    /// Fields excluded from the own-entry diff (their changes drive the
    /// per-candidate fan-out save instead, not the single-entry diff): fields
    /// whose picker binding sets `fanout_attr` (e.g. `memberOf`).
    ///
    /// The caller must strip the SAME labels from the `original` (baseline) side
    /// before calling [`crate::form::changeset::diff`] to avoid spurious deletes
    /// (via [`fanout_labels`](EditForm::fanout_labels)).
    ///
    /// All other fields are included — even those whose [`EditField::current_values`]
    /// is empty — so a cleared field diffs to a delete.
    pub fn to_edit_entry(&self) -> EditEntry {
        let attrs = self
            .fields
            .iter()
            .filter(|f| fanout_attr_of(f).is_none())
            .map(|f| (f.label.clone(), f.current_values()))
            .collect();
        EditEntry {
            dn: self.dn.clone(),
            attrs,
        }
    }

    /// Labels of fields whose picker binding fans out (excluded from the own-entry
    /// diff; their change drives the per-candidate fan-out save).
    pub fn fanout_labels(&self) -> Vec<String> {
        self.fields
            .iter()
            .filter(|f| fanout_attr_of(f).is_some())
            .map(|f| f.label.clone())
            .collect()
    }

    /// Labels of fields currently marked orphaned (will be deleted on save).
    pub fn orphaned_labels(&self) -> Vec<String> {
        self.fields
            .iter()
            .filter(|f| f.orphaned)
            .map(|f| f.label.clone())
            .collect()
    }

    /// Whether any field's current value SET differs from its baseline SET.
    ///
    /// Set-wise / order-insensitive, matching `changeset::diff`'s `value_set_eq`
    /// semantics, so a pure reorder of a multi-valued attribute is NOT dirty. A
    /// missing baseline key is treated as an empty set.
    pub fn is_dirty(&self) -> bool {
        if self.pending_password.is_some() {
            return true;
        }
        const EMPTY: &Vec<String> = &Vec::new();
        self.fields.iter().any(|f| {
            let current = f.current_values();
            let baseline = self.baseline.get(&f.label).unwrap_or(EMPTY);
            !value_set_eq(&current, baseline)
        })
    }
}

/// Order-insensitive value-set equality: same length and every element of each
/// side appears in the other (symmetric check). This is the dirty-check sibling
/// of `changeset::diff`'s value comparison; note the changeset version is
/// currently only a one-directional subset check (a latent asymmetry masked by
/// LDAP per-attribute value uniqueness — to be made symmetric in P5).
pub(crate) fn value_set_eq(a: &[String], b: &[String]) -> bool {
    a.len() == b.len()
        && a.iter().all(|v| b.iter().any(|w| w == v))
        && b.iter().all(|v| a.iter().any(|w| w == v))
}

/// Build an [`EditForm`] from a read-only [`FormModel`] plus the server schema.
///
/// - `multi`    = the attribute is not single-valued in the schema;
/// - `editable` = not global-read-only AND the field kind is editable
///   (binary / boolean-checkbox / and normally `memberOf` stay static —
///   [`field_is_editable`]). Picker/membership widgets are tagged separately by
///   [`tag_widget_fields`] at the call seams, which may override editability.
/// - `secret`   = a password attribute ([`crate::form::changeset::is_secret_attr`]);
/// - `ordered`  = an X-ORDERED config attribute ([`is_x_ordered`]).
///
/// P1 uses the result purely for display. The single-value `editor` is seeded
/// from `values[0]` so P2's editing has its starting point.
pub fn build_edit_form(model: &FormModel, schema: &SchemaModel, read_only: bool) -> EditForm {
    let fields: Vec<EditField> = model
        .fields
        .iter()
        .map(|f| {
            let editable = !read_only && field_is_editable(f);
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
                widget_binding: None,
                orphaned: false,
            }
        })
        .collect();

    // The immutable snapshot of original server values the dirty check compares
    // against: every field's original `values`, keyed by label.
    let baseline = fields
        .iter()
        .map(|f| (f.label.clone(), f.values.clone()))
        .collect();

    EditForm {
        dn: model.title.clone(),
        fields,
        baseline,
        mode: FormMode::Edit,
        pending_password: None,
    }
}

/// Attach a `[profile.widget.<attr>]` widget (choice / password / picker /
/// membership) to each matching field. Choice fields stay editable (Enter opens
/// the choice overlay). Password fields stay read-only inline; Enter opens the
/// password popup. Picker fields open the candidate picker; a membership
/// (fan-out) binding forces the field editable (its value fans out, it is never
/// written to the field itself), honoring global read-only. `.any()` objectClass
/// matching, mirroring `widget_for`.
pub fn tag_widget_fields(
    form: &mut EditForm,
    widgets: &[crate::config::widget::ResolvedWidget],
    object_classes: &[String],
    read_only: bool,
) {
    use crate::config::widget::WidgetKind;
    let has_oc = |ocs: &[String]| {
        ocs.iter()
            .any(|oc| object_classes.iter().any(|e| e.eq_ignore_ascii_case(oc)))
    };
    for rw in widgets {
        if !has_oc(&rw.owner_object_classes) {
            continue;
        }
        match &rw.kind {
            WidgetKind::Picker(binding) => {
                if let Some(f) = form
                    .fields
                    .iter_mut()
                    .find(|f| f.label.eq_ignore_ascii_case(&rw.attr))
                {
                    if binding.fanout_attr.is_some() {
                        f.editable = !read_only;
                        f.widget_binding = Some(rw.kind.clone());
                    } else if f.editable {
                        f.widget_binding = Some(rw.kind.clone());
                    }
                }
            }
            WidgetKind::Choice(_) => {
                if read_only {
                    continue;
                }
                if let Some(f) = form
                    .fields
                    .iter_mut()
                    .find(|f| f.label.eq_ignore_ascii_case(&rw.attr))
                {
                    f.widget_binding = Some(rw.kind.clone());
                    f.editable = true;
                }
            }
            WidgetKind::Password(pw) => {
                if read_only {
                    continue;
                }
                let mut targets = vec![pw.primary.clone()];
                targets.extend(pw.derived.iter().cloned());
                for f in form.fields.iter_mut() {
                    if targets.iter().any(|t| t.eq_ignore_ascii_case(&f.label)) {
                        f.widget_binding = Some(rw.kind.clone());
                    }
                }
            }
            WidgetKind::ObjectClassPicker => {
                // Auto-injected; no tagging action needed — the injection already
                // set widget_binding = Some(ObjectClassPicker) on the field.
            }
        }
    }
}

/// Reorder a built form's fields into: mandatory, then populated-or-special
/// (non-empty value, secret/password, or picker-bound),
/// then the rest — each bucket alphabetical (case-insensitive) by label.
pub fn order_fields(form: &mut EditForm) {
    fn bucket(f: &EditField) -> u8 {
        if f.must {
            0
        } else if !f.current_values().is_empty() || f.secret || widget_picker(f).is_some() {
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

/// Port of the facade's editability rule: `memberOf` is server-maintained and
/// binary / boolean-checkbox kinds are not free-text, so none of them edit.
fn field_is_editable(field: &FormField) -> bool {
    if field.label.eq_ignore_ascii_case("memberOf") {
        return false;
    }
    // Secret/hash attributes are managed by the password flow (the injected
    // userPassword + confirm fields derive `sambaNTPassword` via the NT hash) —
    // never hand-edited inline. A raw edit of `sambaNTPassword`/`sambaLMPassword`
    // would be written verbatim (no hashing), storing a broken/cleartext value,
    // and would leak into the confirm preview. So they are display-only (masked).
    // The injected password fields set `editable: true` directly, so they are
    // unaffected; change passwords there or via the `passwd` subcommand.
    if is_secret_attr(&field.label) {
        return false;
    }
    !matches!(
        field.widget,
        WidgetSpec::BinaryNote(_) | WidgetSpec::DisabledCheckBox(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldap::worker::{LdapEntry, RawSubschema};
    use crate::ui::form::build_form_model;
    use std::collections::BTreeMap;

    fn empty_schema() -> SchemaModel {
        SchemaModel::from_raw(&crate::ldap::worker::RawSubschema {
            object_classes: vec![],
            attribute_types: vec![],
            ldap_syntaxes: vec![],
        })
    }

    #[test]
    fn fanout_labels_come_from_picker_binding() {
        use crate::config::relation::{CandidateScope, PickerBinding, StoreKey};
        let mk = |fanout: Option<String>| EditForm {
            dn: "uid=bob,ou=people,dc=x".into(),
            fields: vec![EditField {
                label: "memberOf".into(),
                must: false,
                editable: true,
                multi: true,
                secret: false,
                ordered: false,
                values: vec![],
                kind: FieldKind::DistinguishedName,
                widget: WidgetSpec::ReadOnlyText,
                editor: TextState::new(),
                widget_binding: Some(crate::config::widget::WidgetKind::Picker(PickerBinding {
                    attr: "memberOf".into(),
                    scope: CandidateScope {
                        base: "".into(),
                        object_classes: vec![],
                        search_attrs: vec![],
                        label_template: None,
                    },
                    store: StoreKey::Dn,
                    select: None,
                    fanout_attr: fanout,
                })),
                orphaned: false,
            }],
            baseline: Default::default(),
            mode: FormMode::Edit,
            pending_password: None,
        };
        let with_fanout = mk(Some("member".into()));
        assert_eq!(with_fanout.fanout_labels(), vec!["memberOf".to_string()]);
        // A fan-out field is excluded from the own-entry diff (its change drives
        // the per-candidate fan-out save instead).
        assert!(!with_fanout.to_edit_entry().attrs.contains_key("memberOf"));
        // A picker field WITHOUT fanout_attr is NOT a fanout label and IS included
        // in the own-entry diff.
        let no_fanout = mk(None);
        assert!(no_fanout.fanout_labels().is_empty());
        assert!(no_fanout.to_edit_entry().attrs.contains_key("memberOf"));
    }

    #[test]
    fn editform_mode_defaults_to_edit_and_reports_not_new() {
        use crate::ui::form::FormModel;
        let model = FormModel {
            title: "cn=x,dc=example,dc=org".into(),
            fields: vec![],
        };
        let form = build_edit_form(&model, &empty_schema(), false);
        assert!(matches!(form.mode, FormMode::Edit));
        assert!(!form.is_new());
    }

    fn schema() -> SchemaModel {
        let raw = RawSubschema {
            object_classes: vec![
                "( 2.5.6.0 NAME 'top' ABSTRACT MUST objectClass )".to_string(),
                "( 2.5.6.6 NAME 'person' SUP top STRUCTURAL MUST ( sn $ cn ) \
                  MAY ( userPassword $ description ) )"
                    .to_string(),
                "( 1.2.3 NAME 'demoPerson' SUP person STRUCTURAL MAY mail )".to_string(),
            ],
            attribute_types: vec![
                // cn single-valued; mail multi-valued (no SINGLE-VALUE).
                "( 2.5.4.3 NAME 'cn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 SINGLE-VALUE )"
                    .to_string(),
                "( 2.5.4.4 NAME 'sn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 SINGLE-VALUE )"
                    .to_string(),
                "( 1.1.1 NAME 'mail' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".to_string(),
                "( 1.1.9 NAME 'userPassword' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".to_string(),
            ],
            ldap_syntaxes: vec![],
        };
        SchemaModel::from_raw(&raw)
    }

    fn entry() -> LdapEntry {
        let mut attrs = BTreeMap::new();
        attrs.insert("cn".to_string(), vec!["Alice".to_string()]);
        attrs.insert("sn".to_string(), vec!["Adams".to_string()]);
        attrs.insert(
            "mail".to_string(),
            vec!["a@x.org".to_string(), "a@y.org".to_string()],
        );
        attrs.insert("userPassword".to_string(), vec!["secret".to_string()]);
        LdapEntry {
            dn: "cn=Alice,dc=example,dc=org".to_string(),
            attrs,
            bin_attrs: BTreeMap::new(),
        }
    }

    #[test]
    fn flags_are_set_from_schema_and_rules() {
        let model = build_form_model(&schema(), &["demoPerson"], &entry(), &[]);
        let form = build_edit_form(&model, &schema(), false);
        let field = |name: &str| form.fields.iter().find(|f| f.label == name).unwrap();

        assert!(!field("cn").multi, "cn is single-valued");
        assert!(field("mail").multi, "mail is multi-valued");
        assert!(field("userPassword").secret, "userPassword is secret");
        assert!(!field("cn").secret);
        assert!(field("cn").editable, "cn edits in writable mode");
    }

    #[test]
    fn secret_fields_are_not_editable() {
        // Password/hash attributes (userPassword, sambaNTPassword, sambaLMPassword)
        // are managed by the password flow — never hand-edited inline. A direct
        // edit would be written verbatim (no NT-hash) and leak into the confirm
        // preview, so they render masked + read-only. The injected password
        // fields stay editable independently (they set `editable: true` directly).
        let model = build_form_model(&schema(), &["demoPerson"], &entry(), &[]);
        let form = build_edit_form(&model, &schema(), false);
        let field = |name: &str| form.fields.iter().find(|f| f.label == name).unwrap();
        assert!(field("userPassword").secret);
        assert!(
            !field("userPassword").editable,
            "secret/hash fields must be read-only inline"
        );
        assert!(field("cn").editable, "non-secret fields stay editable");
    }

    #[test]
    fn value_editor_open_and_commit_drops_empties() {
        let model = build_form_model(&schema(), &["demoPerson"], &entry(), &[]);
        let form = build_edit_form(&model, &schema(), false);
        let mail_idx = form.fields.iter().position(|f| f.label == "mail").unwrap();
        let mut ve = ValueEditor::open_plain(mail_idx, &form.fields[mail_idx]);
        assert_eq!(ve.rows.len(), 2); // a@x.org, a@y.org
        assert_eq!(ve.label, "mail");
        // Add a blank row and a whitespace row; commit drops both.
        ve.rows.push(TextState::new());
        ve.rows.push(TextState::new().with_value("   ".to_string()));
        ve.rows
            .push(TextState::new().with_value(" c@z.org ".to_string()));
        let committed = ve.committed_values();
        assert_eq!(committed, vec!["a@x.org", "a@y.org", "c@z.org"]);
    }

    #[test]
    fn read_only_mode_disables_all_editing() {
        let model = build_form_model(&schema(), &["demoPerson"], &entry(), &[]);
        let form = build_edit_form(&model, &schema(), true);
        assert!(form.fields.iter().all(|f| !f.editable));
        assert_eq!(form.dn, "cn=Alice,dc=example,dc=org");
    }

    /// Build a writable form over the standard demo entry.
    fn writable_form() -> EditForm {
        let model = build_form_model(&schema(), &["demoPerson"], &entry(), &[]);
        build_edit_form(&model, &schema(), false)
    }

    fn field_index(form: &EditForm, name: &str) -> usize {
        form.fields.iter().position(|f| f.label == name).unwrap()
    }

    #[test]
    fn fresh_form_is_not_dirty() {
        let form = writable_form();
        assert!(!form.is_dirty());
    }

    #[test]
    fn editing_a_single_field_makes_it_dirty() {
        let mut form = writable_form();
        let i = field_index(&form, "cn");
        form.fields[i].editor = TextState::new().with_value("changed");

        assert!(form.is_dirty());
        assert_eq!(
            form.to_edit_entry().attrs["cn"],
            vec!["changed".to_string()]
        );
    }

    #[test]
    fn reorder_of_multi_value_is_not_dirty() {
        let mut form = writable_form();
        let i = field_index(&form, "mail");
        form.fields[i].values.reverse();

        assert!(!form.is_dirty(), "a pure reorder is set-wise equal");
    }

    #[test]
    fn adding_a_multi_value_is_dirty() {
        let mut form = writable_form();
        let i = field_index(&form, "mail");
        form.fields[i].values.push("a@z.org".to_string());

        assert!(form.is_dirty());
    }

    #[test]
    fn emptying_a_single_field_drops_the_value() {
        let mut form = writable_form();
        let i = field_index(&form, "cn");
        form.fields[i].editor = TextState::new().with_value("");

        assert!(form.fields[i].current_values().is_empty());
        assert!(form.to_edit_entry().attrs["cn"].is_empty());
    }

    #[test]
    fn reverting_an_edit_clears_dirty() {
        let mut form = writable_form();
        let i = field_index(&form, "cn");
        form.fields[i].editor = TextState::new().with_value("changed");
        assert!(form.is_dirty());

        // Restore the original value: back to clean.
        form.fields[i].editor = TextState::new().with_value("Alice");
        assert!(!form.is_dirty());
    }

    #[test]
    fn order_fields_buckets_mandatory_then_populated_special_then_rest() {
        // label, must, secret, values — everything else default.
        // Single editable fields drive `current_values()` from the editor, so seed
        // the editor (not just `values`) to mirror how `build_edit_form` builds them.
        let mk = |label: &str, must: bool, secret: bool, values: Vec<&str>| {
            let seed = values.first().map(|s| s.to_string()).unwrap_or_default();
            EditField {
                label: label.into(),
                must,
                editable: true,
                multi: false,
                secret,
                ordered: false,
                values: values.into_iter().map(String::from).collect(),
                kind: crate::schema::FieldKind::Text,
                widget: crate::ui::form::WidgetSpec::ReadOnlyText,
                editor: TextState::new().with_value(seed),
                widget_binding: None,
                orphaned: false,
            }
        };
        let mut form = EditForm {
            dn: "uid=alice,ou=people,dc=example,dc=org".into(),
            fields: vec![
                mk("sn", true, false, vec![]),           // bucket 0
                mk("cn", true, false, vec![]),           // bucket 0
                mk("mail", false, false, vec!["x"]),     // bucket 1 (populated)
                mk("userPassword", false, true, vec![]), // bucket 1 (secret)
                mk("displayName", false, false, vec![]), // bucket 2
                mk("audio", false, false, vec![]),       // bucket 2
            ],
            baseline: Default::default(),
            mode: FormMode::Edit,
            pending_password: None,
        };
        order_fields(&mut form);
        let labels: Vec<&str> = form.fields.iter().map(|f| f.label.as_str()).collect();
        assert_eq!(
            labels,
            vec![
                // bucket 0, alphabetical (case-insensitive)
                "cn",
                "sn", // bucket 1, alphabetical
                "mail",
                "userPassword", // bucket 2, alphabetical
                "audio",
                "displayName",
            ]
        );
    }

    #[test]
    fn order_fields_populated_optional_sorts_ahead_of_empty_optional() {
        let mk = |label: &str, values: Vec<&str>| {
            let seed = values.first().map(|s| s.to_string()).unwrap_or_default();
            EditField {
                label: label.into(),
                must: false,
                editable: true,
                multi: false,
                secret: false,
                ordered: false,
                values: values.into_iter().map(String::from).collect(),
                kind: crate::schema::FieldKind::Text,
                widget: crate::ui::form::WidgetSpec::ReadOnlyText,
                editor: TextState::new().with_value(seed),
                widget_binding: None,
                orphaned: false,
            }
        };
        let mut form = EditForm {
            dn: "uid=alice,ou=people,dc=example,dc=org".into(),
            // "zoo" populated (bucket 1) must come before "aaa" empty (bucket 2),
            // even though it loses alphabetically — bucket dominates.
            fields: vec![mk("aaa", vec![]), mk("zoo", vec!["v"])],
            baseline: Default::default(),
            mode: FormMode::Edit,
            pending_password: None,
        };
        order_fields(&mut form);
        let labels: Vec<&str> = form.fields.iter().map(|f| f.label.as_str()).collect();
        assert_eq!(labels, vec!["zoo", "aaa"]);
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
            widget_binding: None,
            orphaned: false,
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
        assert_eq!(p.selected.len(), 1);
        assert_eq!(p.selected[0].store_value, "1001");
        assert!(!p.key_ci, "scalar store → exact key compare");
        assert_eq!(ve.binding.as_ref().unwrap().attr, "gidNumber");
    }

    #[test]
    fn tag_widget_fields_attaches_matching_choice() {
        use crate::config::relation::Cardinality;
        use crate::config::widget::{ChoiceFormat, ChoiceWidget, ResolvedWidget, WidgetKind};
        use crate::config::ChoiceOption;

        let mut form = writable_form();
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
            widget_binding: None,
            orphaned: false,
        });
        let widgets = vec![ResolvedWidget {
            owner_object_classes: vec!["demoPerson".into()],
            attr: "loginShell".into(),
            kind: WidgetKind::Choice(ChoiceWidget {
                select: Cardinality::Single,
                format: ChoiceFormat::Plain,
                options: vec![ChoiceOption {
                    value: "/bin/bash".into(),
                    label: "Bash".into(),
                }],
            }),
        }];
        tag_widget_fields(&mut form, &widgets, &["demoPerson".to_string()], false);
        let f = form
            .fields
            .iter()
            .find(|f| f.label == "loginShell")
            .unwrap();
        assert!(matches!(f.widget_binding, Some(WidgetKind::Choice(_))));
        assert!(f.editable, "a choice field stays editable");
    }

    #[test]
    fn tag_widget_fields_attaches_picker_and_forces_fanout_editable() {
        use crate::config::relation::{CandidateScope, PickerBinding, StoreKey};
        use crate::config::widget::{ResolvedWidget, WidgetKind};

        let scope = || CandidateScope {
            base: "ou=groups,dc=x".into(),
            object_classes: vec!["groupOfNames".into()],
            search_attrs: vec!["cn".into()],
            label_template: None,
        };
        // Three picker-bound fields:
        //   memberOf — fan-out (membership) on an operationally read-only field;
        //   member   — plain picker on an editable field (tagged via the else-if);
        //   secretary — plain picker on a non-editable field (must NOT be tagged).
        let plain_field = |label: &str, editable: bool| EditField {
            label: label.into(),
            must: false,
            editable,
            multi: true,
            secret: false,
            ordered: false,
            values: vec![],
            kind: FieldKind::DistinguishedName,
            widget: WidgetSpec::ReadOnlyText,
            editor: TextState::new(),
            widget_binding: None,
            orphaned: false,
        };
        let mk_form = || {
            let mut form = writable_form();
            form.fields.push(plain_field("memberOf", false)); // server-maintained, read-only
            form.fields.push(plain_field("member", true)); // editable plain picker
            form.fields.push(plain_field("secretary", false)); // non-editable plain picker
            form
        };
        let plain_picker = |attr: &str| {
            WidgetKind::Picker(PickerBinding {
                attr: attr.into(),
                scope: scope(),
                store: StoreKey::Dn,
                select: None,
                fanout_attr: None,
            })
        };
        let widgets = || {
            vec![
                ResolvedWidget {
                    owner_object_classes: vec!["demoPerson".into()],
                    attr: "memberOf".into(),
                    kind: WidgetKind::Picker(PickerBinding {
                        attr: "memberOf".into(),
                        scope: scope(),
                        store: StoreKey::Dn,
                        select: None,
                        fanout_attr: Some("member".into()),
                    }),
                },
                ResolvedWidget {
                    owner_object_classes: vec!["demoPerson".into()],
                    attr: "member".into(),
                    kind: plain_picker("member"),
                },
                ResolvedWidget {
                    owner_object_classes: vec!["demoPerson".into()],
                    attr: "secretary".into(),
                    kind: plain_picker("secretary"),
                },
            ]
        };
        let find = |form: &EditForm, label: &str| {
            form.fields
                .iter()
                .find(|f| f.label == label)
                .map(|f| {
                    (
                        f.editable,
                        matches!(f.widget_binding, Some(WidgetKind::Picker(_))),
                    )
                })
                .unwrap()
        };

        // Writable: the fan-out picker is tagged and forced editable.
        let mut form = mk_form();
        tag_widget_fields(&mut form, &widgets(), &["demoPerson".to_string()], false);
        let (mof_editable, mof_tagged) = find(&form, "memberOf");
        assert!(mof_tagged, "fan-out picker is tagged");
        assert!(
            mof_editable,
            "a fan-out picker forces editability despite operational read-only"
        );
        // A plain picker on an already-editable field IS tagged (the else-if branch).
        let (mem_editable, mem_tagged) = find(&form, "member");
        assert!(mem_tagged, "an editable plain picker gets a widget binding");
        assert!(mem_editable, "an editable plain picker stays editable");
        // A plain picker on a non-editable field is NOT tagged.
        let (sec_editable, sec_tagged) = find(&form, "secretary");
        assert!(
            !sec_tagged,
            "a non-editable plain picker must not be tagged (would be unreachable)"
        );
        assert!(!sec_editable, "a non-editable plain picker stays read-only");

        // Global read-only: the fan-out field is still tagged (so the popup is
        // reachable), but NOT editable.
        let mut form2 = mk_form();
        tag_widget_fields(&mut form2, &widgets(), &["demoPerson".to_string()], true);
        let (mof2_editable, mof2_tagged) = find(&form2, "memberOf");
        assert!(mof2_tagged);
        assert!(
            !mof2_editable,
            "global read-only must not force a fan-out field editable"
        );
    }

    #[test]
    fn pending_password_makes_form_dirty() {
        let mut form = writable_form();
        assert!(!form.is_dirty());
        form.pending_password = Some("hunter2".into());
        assert!(form.is_dirty(), "a staged password change is dirty");
    }

    #[test]
    fn orphaned_field_current_values_returns_empty() {
        let mut form = writable_form();
        let i = field_index(&form, "cn");
        form.fields[i].orphaned = true;
        // Even with a live value in the editor, orphaned returns [].
        form.fields[i].editor = TextState::new().with_value("Alice");
        assert!(
            form.fields[i].current_values().is_empty(),
            "orphaned field must return [] from current_values()"
        );
    }

    #[test]
    fn orphaned_labels_lists_orphaned_fields() {
        let mut form = writable_form();
        let i = field_index(&form, "cn");
        form.fields[i].orphaned = true;
        assert!(form.orphaned_labels().contains(&"cn".to_string()));
        form.fields[i].orphaned = false;
        assert!(form.orphaned_labels().is_empty());
    }

    #[test]
    fn orphaned_field_makes_form_dirty() {
        // An orphaned field with current_values()==[] but baseline ["Alice"] IS dirty
        // (it will emit a Delete). Verify is_dirty() sees it.
        let mut form = writable_form();
        let i = field_index(&form, "cn");
        // baseline has "Alice" for cn (set by writable_form via build_edit_form)
        form.fields[i].orphaned = true;
        assert!(
            form.is_dirty(),
            "orphaned field with non-empty baseline is dirty"
        );
    }

    #[test]
    fn open_objectclass_seeds_picker_from_field_values() {
        use crate::ldap::worker::RawSubschema;
        let raw = RawSubschema {
            object_classes: vec![
                "( 2.5.6.0 NAME 'top' ABSTRACT MUST objectClass )".to_string(),
                "( 2.5.6.6 NAME 'person' SUP top STRUCTURAL MUST ( sn $ cn ) )".to_string(),
                "( 1.2 NAME 'org' STRUCTURAL MAY ou )".to_string(),
            ],
            attribute_types: vec![],
            ldap_syntaxes: vec![],
        };
        let _ = SchemaModel::from_raw(&raw); // verify the raw parses; not passed to open_objectclass
        let field = EditField {
            label: "objectClass".into(),
            must: true,
            editable: true,
            multi: true,
            secret: false,
            ordered: false,
            values: vec!["top".into(), "person".into()],
            kind: crate::schema::FieldKind::Text,
            widget: crate::ui::form::WidgetSpec::ReadOnlyText,
            editor: TextState::new(),
            widget_binding: None,
            orphaned: false,
        };
        let ve = ValueEditor::open_objectclass(0, &field);
        assert!(ve.objectclass, "objectclass flag set");
        assert!(ve.binding.is_none(), "no LDAP binding for OC picker");
        assert!(ve.choice.is_none(), "not a choice editor");
        let picker = ve.picker.as_ref().expect("picker present");
        // The initial results are empty (populated by service_picker_search on first tick)
        assert!(picker.results.is_empty(), "results start empty");
        // selected should be seeded from field.values
        assert_eq!(
            picker.selected.len(),
            2,
            "two currently-selected OCs pre-ticked"
        );
        let selected_names: Vec<&str> = picker
            .selected
            .iter()
            .map(|c| c.store_value.as_str())
            .collect();
        assert!(selected_names.contains(&"top"));
        assert!(selected_names.contains(&"person"));
    }

    #[test]
    fn tag_widget_fields_tags_primary_and_derived_for_password() {
        use crate::config::widget::{PasswordWidget, ResolvedWidget, WidgetKind};
        // writable_form() is built from entry() which has a userPassword field,
        // using schema() which declares demoPerson as an object class.
        let mut form = writable_form();
        // Add a sambaNTPassword field (not in the standard schema, so add manually).
        form.fields.push(EditField {
            label: "sambaNTPassword".into(),
            must: false,
            editable: false,
            multi: false,
            secret: true,
            ordered: false,
            values: vec!["DEAD".into()],
            kind: crate::schema::FieldKind::Text,
            widget: crate::ui::form::WidgetSpec::ReadOnlyText,
            editor: TextState::new(),
            widget_binding: None,
            orphaned: false,
        });
        let widgets = vec![ResolvedWidget {
            owner_object_classes: vec!["demoPerson".into()],
            attr: "userPassword".into(),
            kind: WidgetKind::Password(PasswordWidget {
                primary: "userPassword".into(),
                derived: vec!["sambaNTPassword".into(), "sambaPwdLastSet".into()],
                samba: true,
            }),
        }];
        tag_widget_fields(&mut form, &widgets, &["demoPerson".to_string()], false);
        let tagged = |n: &str| {
            matches!(
                form.fields
                    .iter()
                    .find(|f| f.label == n)
                    .unwrap()
                    .widget_binding,
                Some(WidgetKind::Password(_))
            )
        };
        assert!(
            tagged("userPassword"),
            "primary field must be tagged Password"
        );
        assert!(
            tagged("sambaNTPassword"),
            "derived field must be tagged Password"
        );
    }
}
