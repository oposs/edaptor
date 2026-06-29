//! The objectClass field editor: a schema-seeded two-column mover. The currently
//! active object classes sit in the **Active** column (left), the remaining known
//! classes in the **Available** column (right). Moving a class toward Active ticks
//! it; moving it away unticks it. STRUCTURAL classes are shown but non-removable.
//! The prospective `SetValuesThenResyncSchema` outcome is kept in
//! `UiState::staged_commit`. Capability: `NeedsSchema` (no worker).
//!
//! Built on the shared [`DualList`] mover (`ui::dual_list`), with
//! `selected_on_left = true` so the active set renders on the left per the user's
//! request. Unlike membership, the Available column is a *static* set computed
//! locally (all known classes minus the active ones, filtered by the search box),
//! so there is no async worker search — `SearchChanged` just refilters in place.

use std::collections::{BTreeSet, HashSet};

use ldap_types::schema::ObjectClassType;
use tvision_rs::{
    self as tv, delegate, ButtonFlags, ButtonRowAlign, Command, Context, Dialog, Event, Rect, View,
};

use crate::schema::SchemaModel;
use crate::ui::dual_list::{DualEvent, DualList, DualRow};
use crate::ui::widget::{Activation, Capability, CommitOutcome, FieldEditor, FieldWidget};
use crate::ui::Shared;
use crate::workflows::edit_form::EditField;

/// The plugin for the objectClass field.
pub(crate) struct ObjectClassWidget;

impl FieldWidget for ObjectClassWidget {
    fn capability(&self) -> Capability {
        Capability::NeedsSchema
    }
    fn present(&self, field: &EditField) -> String {
        crate::ui::widget::present_field(field)
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
        // Every known class becomes a `DualRow`, with `removable` precomputed from
        // the schema: STRUCTURAL classes must not be removed (a structural class is
        // load-bearing for the entry's existence), so they are locked. Names keep
        // the schema's canonical spelling — `object_class_names` is already sorted.
        let all_rows: Vec<DualRow> = schema
            .object_class_names()
            .into_iter()
            .map(|name| {
                let structural = schema
                    .object_class(&name)
                    .map(|oc| oc.object_class_type == ObjectClassType::Structural)
                    .unwrap_or(false);
                DualRow {
                    key: name.clone(),
                    label: name,
                    removable: !structural,
                }
            })
            .collect();
        // The active set, lowercased for case-insensitive matching against the
        // canonical candidate spelling.
        let ticked: BTreeSet<String> = self.current.iter().map(|s| s.to_lowercase()).collect();
        let picker = ObjectClassPicker::new(all_rows, ticked, shared);
        // Focus the search box so typing filters immediately (search-as-you-type);
        // arrow keys are routed by `DualList::handle_event` to the lists.
        let focus = picker
            .dual
            .search_id()
            .expect("objectClass DualList is built with a search box");
        (Box::new(picker), focus)
    }
}

/// The interactive dialog: a two-column mover (Active / Available) + OK/Cancel.
pub(crate) struct ObjectClassPicker {
    dlg: Dialog,
    /// The two-column mover: owns the column geometry, the active set (Selected)
    /// and the available set, plus move/flip/search.
    dual: DualList,
    shared: Shared,
    /// Every known class as a `DualRow` (canonical, sorted, `removable` precomputed).
    /// The source of truth for both columns: the Active column is the subset that
    /// is ticked, the Available column is the rest filtered by the search term.
    all_rows: Vec<DualRow>,
    /// The active rows to seed on first open (stashed because `set_selected` needs
    /// a `Dialog`/`Context`, only available once the modal is inserted).
    seed_selected: Vec<DualRow>,
    /// Last-observed search term (drives the Available filter).
    last_search: String,
    seeded: bool,
}

impl ObjectClassPicker {
    fn new(all_rows: Vec<DualRow>, ticked: BTreeSet<String>, shared: Shared) -> Self {
        let mut dlg = Dialog::new(Rect::new(0, 0, 72, 22), Some("Object classes".to_string()));
        dlg.state_mut().options.center_x = true;
        dlg.state_mut().options.center_y = true;

        // Active (Selected) on the LEFT, Available on the RIGHT, with a search box
        // above the Available column. `selected_on_left = true` flips only the
        // rendered layout — Insert/Right still means "move toward Active".
        let dual = DualList::new(
            &mut dlg,
            Rect::new(0, 0, 72, 22),
            "Active",
            "Available",
            /* with_search */ true,
            /* selected_on_left */ true,
        );

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

        // The active rows to seed: every candidate whose (lowercased) name is in the
        // ticked set, in canonical sorted order.
        let seed_selected: Vec<DualRow> = all_rows
            .iter()
            .filter(|r| ticked.contains(&r.key.to_lowercase()))
            .cloned()
            .collect();

        ObjectClassPicker {
            dlg,
            dual,
            shared,
            all_rows,
            seed_selected,
            last_search: String::new(),
            seeded: false,
        }
    }

    /// Rebuild the Available column = all known classes minus the active set,
    /// filtered by the current search term. Borrow-safe: collects the active key
    /// set first (releasing the `&self.dual` borrow) before touching `set_available`.
    fn refresh_available(&mut self, ctx: &mut Context) {
        let active: HashSet<String> = self
            .dual
            .selected()
            .iter()
            .map(|r| r.key.to_lowercase())
            .collect();
        let needle = self.last_search.to_lowercase();
        let rows: Vec<DualRow> = self
            .all_rows
            .iter()
            .filter(|r| !active.contains(&r.key.to_lowercase()))
            .filter(|r| needle.is_empty() || r.label.to_lowercase().contains(&needle))
            .cloned()
            .collect();
        self.dual.set_available(rows, &mut self.dlg, ctx);
    }

    /// Write the prospective commit into shared state: the active class names in
    /// canonical, candidate-sorted order (equivalent to the previous single-list
    /// behaviour — `candidates` filtered by the ticked set). Borrow is taken and
    /// dropped here only.
    fn update_staged(&self) {
        let active: HashSet<String> = self
            .dual
            .selected()
            .iter()
            .map(|r| r.key.to_lowercase())
            .collect();
        let committed: Vec<String> = self
            .all_rows
            .iter()
            .filter(|r| active.contains(&r.key.to_lowercase()))
            .map(|r| r.key.clone())
            .collect();
        self.shared.borrow_mut().staged_commit =
            Some(CommitOutcome::SetValuesThenResyncSchema(committed));
    }

    /// Seed on first open: publish the active rows, fill the Available column, and
    /// stage the current active set.
    fn seed(&mut self, ctx: &mut Context) {
        self.seeded = true;
        let seed = std::mem::take(&mut self.seed_selected);
        self.dual.set_selected(seed, &mut self.dlg, ctx);
        self.refresh_available(ctx);
        self.update_staged();
    }
}

#[delegate(to = dlg)]
impl View for ObjectClassPicker {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    /// Seed the columns on first open. `exec_view` calls `reset_current` with a
    /// Context right after modal insertion (before the first draw and before any
    /// event is delivered), so this is the deterministic open hook.
    ///
    /// NOTE: `on_bounds_changed` was considered but does NOT fire for a modal
    /// inserted via `Group::insert` in tvision-rs 0.3.0 — that path calls
    /// `set_bounds` directly without going through `Deferred::ChangeBounds`, so the
    /// post-apply hook never runs. `reset_current` is the correct one-time-init
    /// hook (same pattern as `FileDialog::reset_current`'s `readDirectory`).
    fn reset_current(&mut self, ctx: &mut Context) {
        self.dlg.reset_current(ctx);
        if !self.seeded {
            self.seed(ctx);
        }
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        // Fallback seed for paths that deliver events without first calling
        // reset_current (e.g. direct unit-test event injection).
        if !self.seeded {
            self.seed(ctx);
        }

        // Delegate the column interaction (move/flip/nav/search) to the DualList.
        // Space and Enter are intentionally not intercepted, so they reach the
        // search box and the dialog's default OK button respectively.
        match self.dual.handle_event(ev, &mut self.dlg, ctx) {
            DualEvent::MovedIn(_) | DualEvent::MovedOut(_) => {
                // A class was (un)ticked: rebuild Available = candidates minus the
                // new active set, and restage. (`MovedOut` is rejected automatically
                // for non-removable STRUCTURAL rows, so no special-casing here.)
                self.refresh_available(ctx);
                self.update_staged();
            }
            DualEvent::SearchChanged(term) => {
                self.last_search = term;
                self.refresh_available(ctx);
            }
            DualEvent::FlippedFocus | DualEvent::None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldap::worker::RawSubschema;
    use std::cell::RefCell;
    use std::rc::Rc;
    use tvision_rs::{Deferred, FieldValue, Key, KeyEvent};

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
        let st = crate::ui::state::UiState::new_for_test(
            Structure::build("dc=example,dc=org", vec![]),
            schema(),
            "dc=example,dc=org".into(),
            Vec::new(),
            Vec::new(),
        );
        Rc::new(RefCell::new(st))
    }

    fn headless_ctx<'a>(
        out: &'a mut std::collections::VecDeque<tv::Event>,
        timers: &'a mut tv::timer::TimerQueue,
        deferred: &'a mut Vec<Deferred>,
    ) -> Context<'a> {
        Context::new(out, timers, 0, deferred)
    }

    fn build_view(sh: &Shared, current: &[&str]) -> Box<dyn View> {
        let ed: Box<dyn FieldEditor> = Box::new(ObjectClassEditor {
            current: current.iter().map(|s| s.to_string()).collect(),
        });
        let (view, _focus) = ed.into_view(&schema(), sh.clone());
        view
    }

    fn picker_mut(view: &mut Box<dyn View>) -> &mut ObjectClassPicker {
        view.as_any_mut()
            .and_then(|a| a.downcast_mut::<ObjectClassPicker>())
            .expect("must downcast to ObjectClassPicker")
    }

    fn staged(sh: &Shared) -> Vec<String> {
        match sh.borrow().staged_commit.clone() {
            Some(CommitOutcome::SetValuesThenResyncSchema(v)) => v,
            other => panic!("expected resync outcome, got {other:?}"),
        }
    }

    /// Index of `label` within the Selected (Active) column's current rows.
    fn active_index(p: &ObjectClassPicker, label: &str) -> usize {
        p.dual
            .selected()
            .iter()
            .position(|r| r.label.eq_ignore_ascii_case(label))
            .unwrap_or_else(|| panic!("{label} not in active column"))
    }

    #[test]
    fn into_view_preticks_current_and_stages_them() {
        let sh = shared();
        let mut view = build_view(&sh, &["top", "person"]);
        // staging happens in reset_current (safe: no state borrow held here).
        let mut out = std::collections::VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<Deferred> = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        view.reset_current(&mut ctx);
        let v = staged(&sh);
        assert!(v.iter().any(|s| s.eq_ignore_ascii_case("top")));
        assert!(v.iter().any(|s| s.eq_ignore_ascii_case("person")));
        assert!(!v
            .iter()
            .any(|s| s.eq_ignore_ascii_case("organizationalPerson")));
    }

    /// The columns must be seeded deterministically by reset_current (the hook that
    /// exec_view calls with a Context before the first draw), without requiring any
    /// prior key event.
    #[test]
    fn reset_current_seeds_columns_before_first_event() {
        let sh = shared();
        let mut view = build_view(&sh, &["top", "person"]);

        // Before reset_current nothing is seeded: the Active column is empty and no
        // commit has been staged.
        {
            let p = picker_mut(&mut view);
            assert!(!p.seeded, "must not be seeded before reset_current fires");
            assert!(
                p.dual.selected().is_empty(),
                "Active column must be empty before reset_current"
            );
            // All three schema classes are tracked as candidates.
            assert_eq!(p.all_rows.len(), 3, "all 3 candidates tracked");
        }
        assert!(sh.borrow().staged_commit.is_none());

        // Fire reset_current with a headless Context (mirrors exec_view's call).
        let mut out = std::collections::VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<Deferred> = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        view.reset_current(&mut ctx);

        // The two active classes are now in the Active column — no keypress needed.
        let p = picker_mut(&mut view);
        assert!(p.seeded);
        assert_eq!(
            p.dual.selected().len(),
            2,
            "top + person must be active after reset_current, got {:?}",
            p.dual.selected()
        );
    }

    /// Case-insensitive pre-tick: names in current that differ in case from the schema
    /// must still match and the staged commit must use the schema's canonical spelling.
    #[test]
    fn case_insensitive_pretick_stages_canonical_names() {
        let sh = shared();
        // "TOP" and "Person" do not match the schema's exact "top" / "person" spelling.
        let mut view = build_view(&sh, &["TOP", "Person"]);
        let mut out = std::collections::VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<Deferred> = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        view.reset_current(&mut ctx);
        let v = staged(&sh);
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

    /// Moving a (removable) active class away via Delete unticks it and restages.
    /// Replaces the old `space_toggle_preserves_list_cursor` test: ticking is now
    /// done by moving between columns, and cursor preservation lives in DualList.
    #[test]
    fn move_out_removable_class_unticks_and_restages() {
        let sh = shared();
        let mut view = build_view(&sh, &["top", "person"]);
        let mut out = std::collections::VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<Deferred> = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        view.reset_current(&mut ctx);
        assert_eq!(staged(&sh).len(), 2);

        let p = picker_mut(&mut view);
        // `top` is ABSTRACT → removable. Highlight it in the Active column, Delete.
        let idx = active_index(p, "top");
        if let Some(list) = p.dlg.child_mut(p.dual.selected_id_for_test()) {
            list.set_value_ctx(FieldValue::Int(idx as i32), &mut ctx);
        }
        let mut ev = Event::KeyDown(KeyEvent::from(Key::Delete));
        p.handle_event(&mut ev, &mut ctx);

        let v = staged(&sh);
        assert!(
            !v.iter().any(|s| s.eq_ignore_ascii_case("top")),
            "top must be unticked after move-out, got {v:?}"
        );
        assert!(
            v.iter().any(|s| s.eq_ignore_ascii_case("person")),
            "person must remain ticked, got {v:?}"
        );
    }

    /// A STRUCTURAL active class is non-removable: Delete is a no-op, it stays
    /// ticked. This is the new structural-lock behaviour (absent in the old picker).
    #[test]
    fn structural_class_cannot_be_moved_out() {
        let sh = shared();
        let mut view = build_view(&sh, &["top", "person"]);
        let mut out = std::collections::VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<Deferred> = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        view.reset_current(&mut ctx);

        let p = picker_mut(&mut view);
        // `person` is STRUCTURAL → locked. Highlight it in the Active column, Delete.
        let idx = active_index(p, "person");
        if let Some(list) = p.dlg.child_mut(p.dual.selected_id_for_test()) {
            list.set_value_ctx(FieldValue::Int(idx as i32), &mut ctx);
        }
        let mut ev = Event::KeyDown(KeyEvent::from(Key::Delete));
        p.handle_event(&mut ev, &mut ctx);

        assert!(
            staged(&sh).iter().any(|s| s.eq_ignore_ascii_case("person")),
            "structural 'person' must stay ticked — it is non-removable"
        );
        assert!(
            p.dual.selected().iter().any(|r| r.label == "person"),
            "structural 'person' must remain in the Active column"
        );
    }

    /// Moving an available class toward Active (Right/Insert) ticks it and restages.
    #[test]
    fn move_in_available_class_ticks_and_restages() {
        let sh = shared();
        // Only `person` active; organizationalPerson + top are available.
        let mut view = build_view(&sh, &["person"]);
        let mut out = std::collections::VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<Deferred> = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        view.reset_current(&mut ctx);
        assert_eq!(staged(&sh), vec!["person".to_string()]);

        let p = picker_mut(&mut view);
        // Available column order is candidate-sorted minus active:
        // [organizationalPerson, top]. Highlight index 0 and move it in.
        if let Some(list) = p.dlg.child_mut(p.dual.avail_id_for_test()) {
            list.set_value_ctx(FieldValue::Int(0), &mut ctx);
        }
        let mut ev = Event::KeyDown(KeyEvent::from(Key::Right));
        p.handle_event(&mut ev, &mut ctx);

        let v = staged(&sh);
        assert!(
            v.iter().any(|s| s.eq_ignore_ascii_case("organizationalPerson")),
            "moved-in class must be ticked, got {v:?}"
        );
        assert!(
            v.iter().any(|s| s.eq_ignore_ascii_case("person")),
            "person must remain ticked, got {v:?}"
        );
        // Canonical, candidate-sorted order is preserved in the staged commit.
        assert_eq!(v, vec!["organizationalPerson".to_string(), "person".to_string()]);
    }
}
