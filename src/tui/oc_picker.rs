//! The objectClass field editor: a schema-seeded multi-select dialog. Lists all
//! object-class names (current ones pre-ticked), client-substring-filters, and
//! keeps the prospective `SetValuesThenResyncSchema` outcome in
//! `UiState::staged_commit`. Capability: `NeedsSchema` (no worker).

use std::collections::BTreeSet;

use tvision_rs::{
    self as tv, delegate, ButtonFlags, ButtonRowAlign, Command, Context, Dialog, Event, FieldValue,
    InputLine, Key, ListBox, Rect, View,
};

use crate::schema::SchemaModel;
use crate::tui::widget::{Activation, Capability, CommitOutcome, FieldEditor, FieldWidget};
use crate::tui::Shared;
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
    candidates: Vec<String>,  // all OC names, sorted
    ticked: BTreeSet<String>, // lowercased ticked names
    filtered: Vec<String>,    // current display order (subset of candidates)
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
        let me = ObjectClassPicker {
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

    /// Seed the candidate list on first open. `exec_view` calls `reset_current`
    /// with a Context right after modal insertion (before the first draw and before
    /// any event is delivered), so this is the deterministic open hook.
    ///
    /// NOTE: `on_bounds_changed` was considered but does NOT fire for a modal
    /// inserted via `Group::insert` in tvision-rs 0.3.0 — that path calls
    /// `set_bounds` directly without going through `Deferred::ChangeBounds`, so the
    /// post-apply hook never runs. `reset_current` is the correct one-time-init
    /// hook (same pattern as `FileDialog::reset_current`'s `readDirectory`).
    fn reset_current(&mut self, ctx: &mut Context) {
        self.dlg.reset_current(ctx);
        if self.filtered.is_empty() {
            self.refresh_list(ctx);
        }
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        // Fallback seed: reset_current populates the list when exec_view is used.
        // This guard covers any path that delivers events without first calling
        // reset_current (e.g. direct unit-test event injection).
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
                "( 2.5.4.3 NAME 'cn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".into()
            ],
            ldap_syntaxes: vec![],
        };
        SchemaModel::from_raw(&raw)
    }

    // A worker-less Shared for staging assertions. Uses Structure::build with an
    // empty input list (no StructureInput::default — the type does not impl Default).
    fn shared() -> Shared {
        use crate::workflows::structure::Structure;
        let st = crate::tui::state::UiState::new_for_test(
            Structure::build("dc=example,dc=org", vec![]),
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
        let staged = sh.borrow().staged_commit.clone();
        match staged {
            Some(CommitOutcome::SetValuesThenResyncSchema(v)) => {
                assert!(v.iter().any(|s| s.eq_ignore_ascii_case("top")));
                assert!(v.iter().any(|s| s.eq_ignore_ascii_case("person")));
                assert!(!v
                    .iter()
                    .any(|s| s.eq_ignore_ascii_case("organizationalPerson")));
            }
            other => panic!("expected resync outcome, got {other:?}"),
        }
    }

    /// TDD: list must be seeded deterministically by reset_current (the hook that
    /// exec_view calls with a Context before the first draw), without requiring any
    /// prior key event.
    #[test]
    fn reset_current_seeds_list_before_first_event() {
        use tvision_rs::Deferred;
        let sh = shared();
        let ed: Box<dyn FieldEditor> = Box::new(ObjectClassEditor {
            current: vec!["top".into(), "person".into()],
        });
        let (mut view, _focus_id) = ed.into_view(&schema(), sh.clone());

        // Before reset_current the filtered list must still be empty (no event has run).
        {
            let picker = view
                .as_any_mut()
                .and_then(|a| a.downcast_mut::<ObjectClassPicker>())
                .expect("must downcast to ObjectClassPicker");
            assert!(
                picker.filtered.is_empty(),
                "filtered must be empty before reset_current fires"
            );
        }

        // Fire reset_current with a headless Context (mirrors exec_view's call).
        let mut out: std::collections::VecDeque<tv::Event> = std::collections::VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<Deferred> = Vec::new();
        let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);
        view.reset_current(&mut ctx);

        // All 3 schema candidates must now be listed — no keypress needed.
        let picker = view
            .as_any_mut()
            .and_then(|a| a.downcast_mut::<ObjectClassPicker>())
            .expect("must downcast to ObjectClassPicker");
        assert_eq!(
            picker.filtered.len(),
            3,
            "all 3 candidates must be listed after reset_current, got {:?}",
            picker.filtered
        );
    }

    /// Case-insensitive pre-tick: names in current that differ in case from the schema
    /// must still match and the staged commit must use the schema's canonical spelling.
    #[test]
    fn case_insensitive_pretick_stages_canonical_names() {
        let sh = shared();
        // "TOP" and "Person" do not match the schema's exact "top" / "person" spelling.
        let ed: Box<dyn FieldEditor> = Box::new(ObjectClassEditor {
            current: vec!["TOP".into(), "Person".into()],
        });
        let _ = ed.into_view(&schema(), sh.clone());
        let staged = sh.borrow().staged_commit.clone();
        match staged {
            Some(CommitOutcome::SetValuesThenResyncSchema(v)) => {
                assert!(
                    v.iter().any(|s| s == "top"),
                    "canonical 'top' expected in commit, got {v:?}"
                );
                assert!(
                    v.iter().any(|s| s == "person"),
                    "canonical 'person' expected in commit, got {v:?}"
                );
                assert!(
                    !v.iter().any(|s| s == "organizationalPerson"),
                    "unticked OC must not appear in commit"
                );
            }
            other => panic!("expected resync outcome, got {other:?}"),
        }
    }
}
