//! Membership (fan-out) picker — a two-column "mover" dialog. A
//! `WidgetKind::Picker` binding with `fanout_attr.is_some()` (the back-reference
//! holder attribute, e.g. a group's `member`) opens a modal built on the shared
//! [`DualList`] mover (`ui::dual_list`):
//!
//! - **Available** (left): a search box on top + a list of live LDAP candidates.
//!   Typing submits an async candidate search (`SearchFlow`) via the worker
//!   exactly like the plain picker; results arrive on the next pump tick and are
//!   broadcast as `REFRESH`, which the dialog maps into `DualList::set_available`.
//!   Candidates already in Members are marked.
//! - **Members** (right): the staged member DN set, seeded from `field.values`
//!   (the user's current memberships / baseline) via `DualList::set_selected`.
//!
//! `DualList` owns the column geometry and move actions (Insert/→ move into
//! Members, Delete/← remove, plus [Add]/[Remove] buttons, and search-box
//! reporting); Tab/Shift-Tab focus traversal, list navigation and the
//! pass-through of Space/Enter to the search box and the default OK button are
//! handled by the dialog. This module keeps the membership-specific plumbing: the
//! async candidate-search submit, the pump/`REFRESH` seam that refreshes the
//! Available column, member seeding, and the `staged_commit` write-back.
//!
//! The staged set is mirrored into `staged_commit` as `SetValues(member_dns)`
//! after every move; OK applies it, Cancel discards it. The fan-out write (one
//! MODIFY per group) is produced from the diff against baseline by the
//! combined-save path. Mirrors the `picker` / `oc_picker` module shape: one file
//! holds `MembershipWidget` (FieldWidget), `MembershipEditor` (FieldEditor) and
//! `MembershipDialog` (the interactive `Dialog` view).

use tvision_rs::{
    self as tv, delegate, ButtonFlags, ButtonRowAlign, Command, Context, Dialog, Event, Rect, View,
};

use crate::config::relation::{PickerBinding, StoreKey};
use crate::config::widget::WidgetKind;
use crate::schema::SchemaModel;
use crate::ui::dual_list::{DualEvent, DualList, DualRow};
use crate::ui::widget::{Activation, Capability, CommitOutcome, FieldEditor, FieldWidget};
use crate::ui::{Shared, REFRESH};
use crate::workflows::edit_form::EditField;

// ---------------------------------------------------------------------------
// MembershipWidget — FieldWidget plugin
// ---------------------------------------------------------------------------

/// The plugin for fan-out `WidgetKind::Picker`-bound fields (membership).
/// `present` summarises the member count; `activate` opens a `MembershipDialog`.
pub(crate) struct MembershipWidget;

impl FieldWidget for MembershipWidget {
    fn capability(&self) -> Capability {
        Capability::NeedsWorkerSearch
    }

    fn present(&self, field: &EditField) -> String {
        match field.values.len() {
            0 => "\u{2039}none\u{203a}".to_string(), // ‹none›
            1 => field.values[0].clone(),
            n => format!("\u{2039}{n} members\u{203a}"),
        }
    }

    fn activate(&self, field: &EditField) -> Activation {
        match &field.widget_binding {
            Some(WidgetKind::Picker(b)) if b.fanout_attr.is_some() => {
                Activation::Modal(Box::new(MembershipEditor {
                    label: field.label.clone(),
                    binding: b.clone(),
                    current: field.values.clone(),
                }))
            }
            _ => Activation::Inline,
        }
    }
}

// ---------------------------------------------------------------------------
// MembershipEditor — FieldEditor (carries state into the dialog builder)
// ---------------------------------------------------------------------------

/// Carries the field's binding + current member set into the dialog builder.
pub(crate) struct MembershipEditor {
    label: String,
    binding: PickerBinding,
    current: Vec<String>,
}

impl FieldEditor for MembershipEditor {
    fn into_view(
        self: Box<Self>,
        _schema: &SchemaModel,
        shared: Shared,
    ) -> (Box<dyn View>, tv::ViewId) {
        let MembershipEditor {
            label,
            binding,
            current,
        } = *self;
        let dlg = MembershipDialog::new(label, binding, current, shared);
        // Focus the search box so typing searches immediately (search-as-you-type);
        // arrow keys are routed by `DualList::handle_event` to the lists.
        let focus = dlg
            .dual
            .search_id()
            .expect("membership DualList is built with a search box");
        (Box::new(dlg), focus)
    }
}

// ---------------------------------------------------------------------------
// MembershipDialog — the interactive two-column mover with live search
// ---------------------------------------------------------------------------

/// Available list (search box + candidates) on the left, Members list on the
/// right — both owned by the shared [`DualList`]. Candidate results arrive via
/// the pump and the `REFRESH` broadcast.
pub(crate) struct MembershipDialog {
    dlg: Dialog,
    /// The two-column mover: owns the column geometry, the staged Members set
    /// (Selected) and the live candidate set (Available), plus move/flip/search.
    dual: DualList,
    shared: Shared,
    /// Resolved candidate-search scope (groups).
    base: String,
    oc: String,
    attrs: Vec<String>,
    /// `Some(attr)` for a scalar store; `None` for a DN store (the usual case).
    store_attr: Option<String>,
    /// The initial Members rows, seeded from the field's values; moved into the
    /// `DualList` on first open (when a `Dialog`/`Context` is available).
    seed_members: Vec<DualRow>,
    seeded: bool,
}

impl MembershipDialog {
    fn new(label: String, binding: PickerBinding, current: Vec<String>, shared: Shared) -> Self {
        let title = format!("Edit {label}");
        let mut dlg = Dialog::new(Rect::new(0, 0, 80, 22), Some(title));
        dlg.state_mut().options.center_x = true;
        dlg.state_mut().options.center_y = true;

        // Build the two columns. Available on the left (membership convention), a
        // search box above it; Members (Selected) on the right.
        let dual = DualList::new(
            &mut dlg,
            Rect::new(0, 0, 80, 22),
            "Available",
            "Members",
            /* with_search */ true,
            /* selected_on_left */ false,
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

        // Resolve the candidate-search scope from the binding (mirrors picker).
        let store_attr = match &binding.store {
            StoreKey::Dn => None,
            StoreKey::Attr(a) => Some(a.clone()),
        };
        let oc = binding
            .scope
            .object_classes
            .first()
            .cloned()
            .unwrap_or_default();
        let mut attrs: Vec<String> = vec!["cn".to_string(), "uid".to_string()];
        for a in &binding.scope.search_attrs {
            if !attrs.iter().any(|x| x.eq_ignore_ascii_case(a)) {
                attrs.push(a.clone());
            }
        }
        if let Some(a) = &store_attr {
            if !attrs.iter().any(|x| x.eq_ignore_ascii_case(a)) {
                attrs.push(a.clone());
            }
        }

        // Seed the staged members from the field's current values. Each value is a
        // member DN (the store value / `key` and the friendly label until a search
        // reveals a nicer one). Members are always removable.
        let seed_members: Vec<DualRow> = current
            .into_iter()
            .map(|v| DualRow {
                key: v.clone(),
                label: v,
                removable: true,
            })
            .collect();

        MembershipDialog {
            dlg,
            dual,
            shared,
            base: binding.scope.base.clone(),
            oc,
            attrs,
            store_attr,
            seed_members,
            seeded: false,
        }
    }

    /// Copy the latest pump-delivered search results into the Available column.
    /// Borrow-safe: clones out of `shared`, drops the borrow before touching the
    /// `DualList`. Candidates map to `DualRow { key: store_value, label,
    /// removable: true }`.
    fn sync_results(&mut self, ctx: &mut Context) {
        let results = {
            let st = self.shared.borrow();
            st.search_results.clone()
        };
        let rows: Vec<DualRow> = results
            .into_iter()
            .map(|c| DualRow {
                key: c.store_value,
                label: c.label,
                removable: true,
            })
            .collect();
        self.dual.set_available(rows, &mut self.dlg, ctx);
    }

    /// Mirror the staged member set into shared state as the prospective commit.
    /// The `DualList` Selected rows' `key`s are the member store values.
    fn update_staged(&self) {
        let values: Vec<String> = self.dual.selected().iter().map(|r| r.key.clone()).collect();
        self.shared.borrow_mut().staged_commit = Some(CommitOutcome::SetValues(values));
    }

    /// Submit a candidate search for `term` via the worker. One atomic borrow.
    fn submit_search(&self, term: &str) {
        self.shared.borrow_mut().submit_search(
            &self.base,
            &self.oc,
            term,
            &self.attrs,
            self.store_attr.as_deref(),
        );
    }

    /// Seed on first open: publish the seeded Members, copy any already-delivered
    /// results into Available, stage the current members, and kick off an initial
    /// (empty-term) candidate search so the Available column fills in.
    fn seed(&mut self, ctx: &mut Context) {
        self.seeded = true;
        let seed = std::mem::take(&mut self.seed_members);
        self.dual.set_selected(seed, &mut self.dlg, ctx);
        self.sync_results(ctx);
        self.update_staged();
        self.submit_search("");
    }
}

#[delegate(to = dlg)]
impl View for MembershipDialog {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    /// Seed on first open (see [`MembershipDialog::seed`]).
    fn reset_current(&mut self, ctx: &mut Context) {
        self.dlg.reset_current(ctx);
        if !self.seeded {
            self.seed(ctx);
        }
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        // Fallback seed for paths that deliver events without reset_current.
        if !self.seeded {
            self.seed(ctx);
        }

        // Pump-delivered results: refresh the Available column from shared state.
        if matches!(ev, Event::Broadcast { command, .. } if *command == REFRESH) {
            self.sync_results(ctx);
            self.dlg.handle_event(ev, ctx);
            return;
        }

        // Delegate the column interaction (move/flip/nav/search) to the DualList.
        // Space and Enter are intentionally not intercepted, so they reach the
        // search box and the dialog's default OK button respectively.
        match self.dual.handle_event(ev, &mut self.dlg, ctx) {
            DualEvent::SearchChanged(term) => self.submit_search(&term),
            DualEvent::MovedIn(_) | DualEvent::MovedOut(_) => self.update_staged(),
            DualEvent::FlippedFocus | DualEvent::None => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::relation::{CandidateScope, Cardinality};
    use crate::ldap::worker::RawSubschema;
    use crate::schema::FieldKind;
    use crate::workflows::form_model::WidgetSpec;
    use crate::workflows::pick_state::Candidate;
    use std::cell::RefCell;
    use std::rc::Rc;
    use tvision_rs::{timer::TimerQueue, Deferred, FieldValue, Key, KeyEvent};

    const G1: &str = "cn=devs,ou=groups,dc=example,dc=org";
    const G2: &str = "cn=ops,ou=groups,dc=example,dc=org";

    fn schema() -> SchemaModel {
        SchemaModel::from_raw(&RawSubschema::default())
    }

    fn test_shared() -> Shared {
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
        timers: &'a mut TimerQueue,
        deferred: &'a mut Vec<Deferred>,
    ) -> Context<'a> {
        Context::new(out, timers, 0, deferred)
    }

    fn group_scope() -> CandidateScope {
        CandidateScope {
            base: "ou=groups,dc=example,dc=org".into(),
            object_classes: vec!["groupOfNames".into()],
            search_attrs: vec!["cn".into()],
            label_template: None,
        }
    }

    fn fanout_binding() -> PickerBinding {
        PickerBinding {
            attr: "memberOf".into(),
            scope: group_scope(),
            store: StoreKey::Dn,
            select: Some(Cardinality::Multi),
            fanout_attr: Some("member".into()),
        }
    }

    fn membership_field(label: &str, values: &[&str]) -> EditField {
        EditField {
            label: label.into(),
            must: false,
            editable: true,
            multi: true,
            secret: false,
            ordered: false,
            orphaned: false,
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: Some(WidgetKind::Picker(fanout_binding())),
            values: values.iter().map(|s| s.to_string()).collect(),
            baseline: values.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn cand(dn: &str) -> Candidate {
        Candidate {
            dn: dn.into(),
            label: dn.into(),
            store_value: dn.into(),
        }
    }

    fn build_dialog(shared: &Shared, current: &[&str]) -> Box<dyn View> {
        let ed: Box<dyn FieldEditor> = Box::new(MembershipEditor {
            label: "memberOf".into(),
            binding: fanout_binding(),
            current: current.iter().map(|s| s.to_string()).collect(),
        });
        let (view, _focus) = ed.into_view(&schema(), shared.clone());
        view
    }

    fn staged_set(shared: &Shared) -> Vec<String> {
        match shared.borrow().staged_commit.clone() {
            Some(CommitOutcome::SetValues(v)) => v,
            other => panic!("expected SetValues, got {other:?}"),
        }
    }

    // -- widget routing ----------------------------------------------------

    #[test]
    fn present_summarises_member_count() {
        let w = MembershipWidget;
        assert_eq!(
            w.present(&membership_field("memberOf", &[])),
            "\u{2039}none\u{203a}"
        );
        assert_eq!(w.present(&membership_field("memberOf", &[G1])), G1);
        assert_eq!(
            w.present(&membership_field("memberOf", &[G1, G2])),
            "\u{2039}2 members\u{203a}"
        );
    }

    #[test]
    fn fanout_picker_activates_modal() {
        let f = membership_field("memberOf", &[]);
        assert!(matches!(
            MembershipWidget.activate(&f),
            Activation::Modal(_)
        ));
    }

    // -- two-column move logic --------------------------------------------

    /// Seed two group candidates, baseline Members = [g1]; moving g2 from
    /// Available into Members stages [g1, g2]; removing g1 stages [g2].
    #[test]
    fn move_in_and_out_updates_staged() {
        let shared = test_shared();
        shared.borrow_mut().search_results = vec![cand(G1), cand(G2)];

        let mut view = build_dialog(&shared, &[G1]);
        let mut out = std::collections::VecDeque::new();
        let mut timers = TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        view.reset_current(&mut ctx);

        // Baseline staged set = [g1].
        assert_eq!(staged_set(&shared), vec![G1.to_string()]);

        let dlg = view
            .as_any_mut()
            .and_then(|a| a.downcast_mut::<MembershipDialog>())
            .expect("downcast MembershipDialog");

        // Highlight g2 (index 1 in Available) and press Right → move into Members.
        if let Some(list) = dlg.dlg.child_mut(dlg.dual.avail_id_for_test()) {
            list.set_value_ctx(FieldValue::Int(1), &mut ctx);
        }
        let mut ev = Event::KeyDown(KeyEvent::from(Key::Right));
        dlg.handle_event(&mut ev, &mut ctx);

        let mut got = staged_set(&shared);
        got.sort();
        assert_eq!(got, vec![G1.to_string(), G2.to_string()]);

        // Highlight g1 (index 0 in Members) and press Delete → remove it.
        if let Some(list) = dlg.dlg.child_mut(dlg.dual.selected_id_for_test()) {
            list.set_value_ctx(FieldValue::Int(0), &mut ctx);
        }
        let mut ev = Event::KeyDown(KeyEvent::from(Key::Delete));
        dlg.handle_event(&mut ev, &mut ctx);

        assert_eq!(staged_set(&shared), vec![G2.to_string()]);
    }

    /// Moving a row that is already a member is a no-op (no duplicate).
    #[test]
    fn move_in_is_deduped() {
        let shared = test_shared();
        shared.borrow_mut().search_results = vec![cand(G1), cand(G2)];

        let mut view = build_dialog(&shared, &[G1]);
        let mut out = std::collections::VecDeque::new();
        let mut timers = TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        view.reset_current(&mut ctx);

        let dlg = view
            .as_any_mut()
            .and_then(|a| a.downcast_mut::<MembershipDialog>())
            .expect("downcast");

        // Highlight g1 (already a member, index 0 in Available) and press Insert.
        if let Some(list) = dlg.dlg.child_mut(dlg.dual.avail_id_for_test()) {
            list.set_value_ctx(FieldValue::Int(0), &mut ctx);
        }
        let mut ev = Event::KeyDown(KeyEvent::from(Key::Insert));
        dlg.handle_event(&mut ev, &mut ctx);

        assert_eq!(
            staged_set(&shared),
            vec![G1.to_string()],
            "moving an existing member must not duplicate it"
        );
    }

    /// Enter must NOT move a row — it is reserved for the dialog's default OK
    /// button (mirrors `space_does_not_move_a_row`; guards against the M4 parity
    /// regression where Enter was consumed by the custom handler).
    #[test]
    fn enter_does_not_move_a_row() {
        let shared = test_shared();
        shared.borrow_mut().search_results = vec![cand(G1), cand(G2)];

        let mut view = build_dialog(&shared, &[G1]);
        let mut out = std::collections::VecDeque::new();
        let mut timers = TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        view.reset_current(&mut ctx);

        let dlg = view
            .as_any_mut()
            .and_then(|a| a.downcast_mut::<MembershipDialog>())
            .expect("downcast");

        // Highlight g2 in Available, press Enter — must NOT move it into Members.
        if let Some(list) = dlg.dlg.child_mut(dlg.dual.avail_id_for_test()) {
            list.set_value_ctx(FieldValue::Int(1), &mut ctx);
        }
        let mut ev = Event::KeyDown(KeyEvent::from(Key::Enter));
        dlg.handle_event(&mut ev, &mut ctx);

        assert_eq!(
            staged_set(&shared),
            vec![G1.to_string()],
            "Enter must not move a candidate — it is reserved for the default OK button"
        );
    }

    /// Space must NOT move a row — it must reach the search box so users can type
    /// multi-word queries (guards against the picker's stolen-Space bug).
    #[test]
    fn space_does_not_move_a_row() {
        let shared = test_shared();
        shared.borrow_mut().search_results = vec![cand(G1), cand(G2)];

        let mut view = build_dialog(&shared, &[G1]);
        let mut out = std::collections::VecDeque::new();
        let mut timers = TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        view.reset_current(&mut ctx);

        let dlg = view
            .as_any_mut()
            .and_then(|a| a.downcast_mut::<MembershipDialog>())
            .expect("downcast");

        // Highlight g2 in Available, press Space — must NOT move it.
        if let Some(list) = dlg.dlg.child_mut(dlg.dual.avail_id_for_test()) {
            list.set_value_ctx(FieldValue::Int(1), &mut ctx);
        }
        let mut ev = Event::KeyDown(KeyEvent::from(Key::Char(' ')));
        dlg.handle_event(&mut ev, &mut ctx);

        assert_eq!(
            staged_set(&shared),
            vec![G1.to_string()],
            "Space must not move a candidate — it belongs to the search box"
        );
    }
}
