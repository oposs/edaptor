//! Choice field widget (radio / checkbox): a fixed-vocabulary selector over a
//! `WidgetKind::Choice(ChoiceWidget)` binding. Single-select shows radio
//! markers `(•)` / `( )`; multi-select shows checkbox markers `[x]` / `[ ]`.
//! Delegates all encode/decode/summary logic to the config `ChoiceWidget`.
//! Mirrors the `oc_picker` modal pattern: one file holds `ChoiceWidget`
//! (FieldWidget), `ChoiceEditor` (FieldEditor), and `ChoiceDialog` (Dialog View).

use tvision_rs::{
    self as tv, delegate, ButtonFlags, ButtonRowAlign, Command, Context, Dialog, Event, FieldValue,
    Key, ListBox, Rect, View,
};

use crate::config::relation::Cardinality;
use crate::config::widget::{ChoiceWidget as CfgChoice, WidgetKind};
use crate::schema::SchemaModel;
use crate::tui::widget::{Activation, Capability, CommitOutcome, FieldEditor, FieldWidget};
use crate::tui::Shared;
use crate::workflows::edit_form::EditField;

// ---------------------------------------------------------------------------
// ChoiceWidget — FieldWidget plugin
// ---------------------------------------------------------------------------

/// The plugin for `WidgetKind::Choice`-bound fields. `present` delegates to
/// `cfg.present_summary`; `activate` opens a `ChoiceDialog`.
pub(crate) struct ChoiceWidget;

impl FieldWidget for ChoiceWidget {
    fn capability(&self) -> Capability {
        Capability::Static
    }

    fn present(&self, field: &EditField) -> String {
        match &field.widget_binding {
            Some(WidgetKind::Choice(cfg)) => {
                cfg.present_summary(field.values.first().map(|s| s.as_str()).unwrap_or(""))
            }
            _ => crate::tui::widget::present_field(field),
        }
    }

    fn activate(&self, field: &EditField) -> Activation {
        match &field.widget_binding {
            Some(WidgetKind::Choice(cfg)) => Activation::Modal(Box::new(ChoiceEditor {
                label: field.label.clone(),
                cfg: cfg.clone(),
                current: field.values.first().cloned().unwrap_or_default(),
            })),
            _ => Activation::Inline,
        }
    }
}

// ---------------------------------------------------------------------------
// ChoiceEditor — FieldEditor (carries state into the dialog builder)
// ---------------------------------------------------------------------------

/// Carries the field's config and current value into the dialog builder.
pub(crate) struct ChoiceEditor {
    pub label: String,
    pub cfg: CfgChoice,
    pub current: String,
}

impl FieldEditor for ChoiceEditor {
    fn into_view(
        self: Box<Self>,
        _schema: &SchemaModel,
        shared: Shared,
    ) -> (Box<dyn View>, tv::ViewId) {
        let ChoiceEditor {
            label,
            cfg,
            current,
        } = *self;
        let dlg = ChoiceDialog::new(label, cfg, current, shared);
        let focus = dlg.list_id;
        (Box::new(dlg), focus)
    }
}

// ---------------------------------------------------------------------------
// ChoiceDialog — the interactive modal
// ---------------------------------------------------------------------------

/// The interactive dialog: a list of options with radio/checkbox prefixes and
/// OK/Cancel buttons. Maintains `checked` (set of option values that are
/// selected), updates `shared.staged_commit` live after every toggle.
pub(crate) struct ChoiceDialog {
    dlg: Dialog,
    list_id: tv::ViewId,
    shared: Shared,
    cfg: CfgChoice,
    /// The original field value (used by `commit_value` for lossless merge).
    current: String,
    /// Option values currently checked (parallel to `cfg.options` indices).
    checked: Vec<String>,
    /// Cached display rows (rebuilt on every `refresh_list`).
    rows: Vec<String>,
}

impl ChoiceDialog {
    fn new(label: String, cfg: CfgChoice, current: String, shared: Shared) -> Self {
        let title = format!("Edit {label}");
        let height = (cfg.options.len() as i32 + 4).clamp(8, 24);
        let list_height = height - 3;
        let mut dlg = Dialog::new(Rect::new(0, 0, 50, height), Some(title));
        dlg.state_mut().options.center_x = true;
        dlg.state_mut().options.center_y = true;
        let list = ListBox::new(Rect::new(2, 1, 48, 1 + list_height), 1, None, None);
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
        ChoiceDialog {
            dlg,
            list_id,
            shared,
            cfg,
            current,
            checked: Vec::new(),
            rows: Vec::new(),
        }
    }

    /// Format one option row with its radio/checkbox marker.
    fn format_row(cfg: &CfgChoice, label: &str, is_checked: bool) -> String {
        match cfg.select {
            Cardinality::Single => {
                if is_checked {
                    format!("(•) {label}")
                } else {
                    format!("( ) {label}")
                }
            }
            Cardinality::Multi => {
                if is_checked {
                    format!("[x] {label}")
                } else {
                    format!("[ ] {label}")
                }
            }
        }
    }

    /// Rebuild display rows from current `checked` set. Preserve cursor if
    /// `preserve_cursor` is true (use on toggle; false on initial seed).
    fn refresh_list(&mut self, ctx: &mut Context, preserve_cursor: bool) {
        self.rows = self
            .cfg
            .options
            .iter()
            .map(|opt| {
                let is_checked = self.checked.iter().any(|c| c == &opt.value);
                Self::format_row(&self.cfg, &opt.label, is_checked)
            })
            .collect();
        let rows_len = self.rows.len();
        let rows = self.rows.clone();
        if let Some(list) = self.dlg.child_mut(self.list_id) {
            let saved_sel: Option<i32> = if preserve_cursor {
                match list.value() {
                    Some(FieldValue::Int(i)) => Some(i),
                    _ => None,
                }
            } else {
                None
            };
            if let Some(lb) = list.as_any_mut().and_then(|a| a.downcast_mut::<ListBox>()) {
                lb.new_list(rows, ctx);
            }
            if let Some(sel) = saved_sel {
                let clamped = sel.min((rows_len.saturating_sub(1)) as i32).max(0);
                list.set_value_ctx(FieldValue::Int(clamped), ctx);
            }
        }
    }

    /// Write the prospective commit into shared state. Short borrow only.
    fn update_staged(&self) {
        let value = self.cfg.commit_value(&self.current, &self.checked);
        self.shared.borrow_mut().staged_commit = Some(CommitOutcome::SetValues(vec![value]));
    }

    /// The option index under the list highlight, if any.
    fn highlighted_index(&mut self) -> Option<usize> {
        match self.dlg.child_mut(self.list_id).and_then(|v| v.value()) {
            Some(FieldValue::Int(i)) if i >= 0 => {
                let idx = i as usize;
                if idx < self.cfg.options.len() {
                    Some(idx)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Toggle (multi) or replace-select (single) the option at `idx`.
    fn toggle_option(&mut self, idx: usize, ctx: &mut Context) {
        let value = self.cfg.options[idx].value.clone();
        match self.cfg.select {
            Cardinality::Multi => {
                if let Some(pos) = self.checked.iter().position(|c| c == &value) {
                    self.checked.remove(pos);
                } else {
                    self.checked.push(value);
                }
            }
            Cardinality::Single => {
                self.checked.clear();
                self.checked.push(value);
            }
        }
        self.refresh_list(ctx, true);
        self.update_staged();
    }
}

#[delegate(to = dlg)]
impl View for ChoiceDialog {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    /// Seed the option list and pre-tick checked values on first open.
    fn reset_current(&mut self, ctx: &mut Context) {
        self.dlg.reset_current(ctx);
        if self.rows.is_empty() {
            self.checked = self.cfg.seed_checked(&self.current);
            self.refresh_list(ctx, false);
            self.update_staged();
        }
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        // Fallback seed if reset_current was not called before events arrive.
        if self.rows.is_empty() && !self.cfg.options.is_empty() {
            self.checked = self.cfg.seed_checked(&self.current);
            self.refresh_list(ctx, false);
            self.update_staged();
        }

        let space = matches!(ev, Event::KeyDown(k) if k.key == Key::Char(' '));
        let nav = matches!(
            ev,
            Event::KeyDown(k)
                if matches!(k.key, Key::Up | Key::Down | Key::PageUp | Key::PageDown)
        );

        if space {
            if let Some(idx) = self.highlighted_index() {
                self.toggle_option(idx, ctx);
            }
            ev.clear();
        } else if nav {
            if let Some(list) = self.dlg.child_mut(self.list_id) {
                list.handle_event(ev, ctx);
            }
        } else {
            self.dlg.handle_event(ev, ctx);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::widget::{ChoiceFormat, ChoiceWidget as Cfg};
    use crate::config::ChoiceOption as Opt;
    use crate::ldap::worker::RawSubschema;
    use crate::schema::{FieldKind, SchemaModel};
    use crate::tui::widget::CommitOutcome;
    use crate::workflows::form_model::WidgetSpec;
    use std::cell::RefCell;
    use std::rc::Rc;
    use tvision_rs::{timer::TimerQueue, Deferred, KeyEvent, KeyModifiers};

    // ------------------------------------------------------------------
    // Test scaffolding (mirrors multivalue.rs / oc_picker.rs patterns)
    // ------------------------------------------------------------------

    fn schema_for_test() -> SchemaModel {
        SchemaModel::from_raw(&RawSubschema::default())
    }

    fn test_shared() -> Shared {
        use crate::workflows::structure::Structure;
        let st = crate::tui::state::UiState::new_for_test(
            Structure::build("dc=example,dc=org", vec![]),
            schema_for_test(),
            "dc=example,dc=org".into(),
            Vec::new(),
            Vec::new(),
        );
        Rc::new(RefCell::new(st))
    }

    fn headless_ctx<'a>(
        out: &'a mut std::collections::VecDeque<tv::Event>,
        timers: &'a mut TimerQueue,
        deferred: &'a mut Vec<Deferred>,
    ) -> Context<'a> {
        Context::new(out, timers, 0, deferred)
    }

    fn single_field(label: &str, value: &str) -> crate::workflows::edit_form::EditField {
        crate::workflows::edit_form::EditField {
            label: label.into(),
            must: false,
            editable: true,
            multi: false,
            secret: false,
            ordered: false,
            orphaned: false,
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: None,
            values: vec![value.to_string()],
            baseline: vec![value.to_string()],
        }
    }

    /// Find option by value in cfg, navigate to that row and send Space.
    fn toggle_row(view: &mut Box<dyn View>, ctx: &mut Context, value: &str) {
        // downcast to ChoiceDialog
        let dlg = view
            .as_any_mut()
            .and_then(|a| a.downcast_mut::<ChoiceDialog>())
            .expect("must downcast to ChoiceDialog");
        // find the row index for `value`
        let idx = dlg
            .cfg
            .options
            .iter()
            .position(|o| o.value == value)
            .expect("option value not found");
        // set the list selection to that index
        if let Some(list) = dlg.dlg.child_mut(dlg.list_id) {
            list.set_value_ctx(FieldValue::Int(idx as i32), ctx);
        }
        // send Space to toggle
        let mut ev = Event::KeyDown(KeyEvent::new(Key::Char(' '), KeyModifiers::default()));
        dlg.handle_event(&mut ev, ctx);
    }

    // ------------------------------------------------------------------
    // Task 5: present delegates to cfg.present_summary
    // ------------------------------------------------------------------

    #[test]
    fn present_uses_config_summary() {
        let cfg = Cfg {
            select: Cardinality::Single,
            format: ChoiceFormat::Plain,
            options: vec![Opt {
                value: "/bin/bash".into(),
                label: "Bash".into(),
            }],
        };
        let mut f = single_field("loginShell", "/bin/bash");
        f.widget_binding = Some(WidgetKind::Choice(cfg));
        assert_eq!(ChoiceWidget.present(&f), "Bash");
    }

    // ------------------------------------------------------------------
    // Task 6 tests
    // ------------------------------------------------------------------

    #[test]
    fn bracketed_multi_merge_is_lossless() {
        let cfg = Cfg {
            select: Cardinality::Multi,
            format: ChoiceFormat::Bracketed,
            options: vec![
                Opt {
                    value: "D".into(),
                    label: "Disabled".into(),
                },
                Opt {
                    value: "U".into(),
                    label: "User".into(),
                },
            ],
        };
        let shared = test_shared();
        let ed = Box::new(ChoiceEditor {
            label: "sambaAcctFlags".into(),
            cfg,
            current: "[U          ]".into(),
        });
        let (mut v, _) = ed.into_view(&schema_for_test(), shared.clone());
        let mut out = std::collections::VecDeque::new();
        let mut timers = TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        v.reset_current(&mut ctx);
        // toggle "D" on (drive a Space on the D row)
        toggle_row(&mut v, &mut ctx, "D");
        let staged = shared.borrow().staged_commit.clone();
        assert_eq!(
            staged,
            Some(CommitOutcome::SetValues(vec!["[DU         ]".into()]))
        );
    }

    #[test]
    fn plain_single_radio_replaces_selection() {
        let cfg = Cfg {
            select: Cardinality::Single,
            format: ChoiceFormat::Plain,
            options: vec![
                Opt {
                    value: "/bin/bash".into(),
                    label: "Bash".into(),
                },
                Opt {
                    value: "/bin/zsh".into(),
                    label: "Zsh".into(),
                },
            ],
        };
        let shared = test_shared();
        let ed = Box::new(ChoiceEditor {
            label: "loginShell".into(),
            cfg,
            current: "/bin/bash".into(),
        });
        let (mut v, _) = ed.into_view(&schema_for_test(), shared.clone());
        let mut out = std::collections::VecDeque::new();
        let mut timers = TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        v.reset_current(&mut ctx);
        // initial state: bash is selected
        {
            let staged = shared.borrow().staged_commit.clone();
            assert_eq!(
                staged,
                Some(CommitOutcome::SetValues(vec!["/bin/bash".into()]))
            );
        }
        // toggle /bin/zsh — radio replaces bash with zsh
        toggle_row(&mut v, &mut ctx, "/bin/zsh");
        let staged = shared.borrow().staged_commit.clone();
        assert_eq!(
            staged,
            Some(CommitOutcome::SetValues(vec!["/bin/zsh".into()]))
        );
    }

    #[test]
    fn reset_current_seeds_checked_from_current_value() {
        let cfg = Cfg {
            select: Cardinality::Multi,
            format: ChoiceFormat::Bracketed,
            options: vec![
                Opt {
                    value: "D".into(),
                    label: "Disabled".into(),
                },
                Opt {
                    value: "U".into(),
                    label: "User".into(),
                },
            ],
        };
        let shared = test_shared();
        let ed = Box::new(ChoiceEditor {
            label: "sambaAcctFlags".into(),
            cfg,
            current: "[U          ]".into(),
        });
        let (mut v, _) = ed.into_view(&schema_for_test(), shared.clone());
        let mut out = std::collections::VecDeque::new();
        let mut timers = TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        v.reset_current(&mut ctx);
        // U is set, D is not → staged should contain [U          ]
        let staged = shared.borrow().staged_commit.clone();
        assert_eq!(
            staged,
            Some(CommitOutcome::SetValues(vec!["[U          ]".into()]))
        );
    }
}
