//! Membership (fan-out) picker — a two-column "mover" dialog. A
//! `WidgetKind::Picker` binding with `fanout_attr.is_some()` (the back-reference
//! holder attribute, e.g. a group's `member`) opens a modal with two side-by-side
//! `ListBox`es:
//!
//! - **Available** (left): a search `InputLine` on top + a `ListBox` of live LDAP
//!   candidates. Typing submits an async candidate search (`SearchFlow`) via the
//!   worker exactly like the plain picker; results arrive on the next pump tick
//!   and are broadcast as `REFRESH`, which the dialog copies into `available` and
//!   re-renders. Candidates already in Members are marked.
//! - **Members** (right): a `ListBox` of the staged member DN set, seeded from
//!   `field.values` (the user's current memberships / baseline).
//!
//! Move keys: **Enter / →** move the highlighted Available row into Members
//! (de-dup by DN, case-insensitive; no-op if already a member); **Delete / ←**
//! remove the highlighted Members row. **Tab** flips which column the Up/Down keys
//! navigate. **Space is intentionally NOT intercepted** so it types into the
//! search box (multi-word queries). The staged set is mirrored into
//! `staged_commit` as `SetValues(member_dns)` after every move; OK applies it,
//! Cancel discards it. Commit the dialog with the OK button's `Alt+O` accelerator
//! (Enter is reserved for the move action); Esc cancels.
//!
//! This task stages only — the fan-out fan-out write (one MODIFY per group) is
//! produced from the diff against baseline by the combined-save path (a later
//! task). Mirrors the `picker` / `oc_picker` module shape: one file holds
//! `MembershipWidget` (FieldWidget), `MembershipEditor` (FieldEditor) and
//! `MembershipDialog` (the interactive `Dialog` view).

use tvision_rs::{
    self as tv, delegate, ButtonFlags, ButtonRowAlign, Command, Context, Dialog, Event, FieldValue,
    InputLine, Key, Label, ListBox, Rect, View,
};

use crate::config::relation::{PickerBinding, StoreKey};
use crate::config::widget::WidgetKind;
use crate::schema::SchemaModel;
use crate::tui::widget::{Activation, Capability, CommitOutcome, FieldEditor, FieldWidget};
use crate::tui::{Shared, REFRESH};
use crate::workflows::edit_form::EditField;
use crate::workflows::pick_state::Candidate;

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
        // arrow keys are intercepted by `handle_event` and routed to the lists.
        let focus = dlg.search_id;
        (Box::new(dlg), focus)
    }
}

// ---------------------------------------------------------------------------
// MembershipDialog — the interactive two-column mover with live search
// ---------------------------------------------------------------------------

/// Available list (search box + candidates) on the left, Members list on the
/// right. Candidate results arrive via the pump and the `REFRESH` broadcast.
pub(crate) struct MembershipDialog {
    dlg: Dialog,
    search_id: tv::ViewId,
    avail_id: tv::ViewId,
    members_id: tv::ViewId,
    shared: Shared,
    /// Resolved candidate-search scope (groups).
    base: String,
    oc: String,
    attrs: Vec<String>,
    /// `Some(attr)` for a scalar store; `None` for a DN store (the usual case).
    store_attr: Option<String>,
    /// Live candidate set shown on the left, copied from `UiState.search_results`.
    available: Vec<Candidate>,
    /// Staged member set shown on the right (seeded from the field's values).
    members: Vec<Candidate>,
    last_search: String,
    /// Which column the Up/Down keys navigate. `false` ⇒ Available (left).
    focus_members: bool,
    seeded: bool,
}

impl MembershipDialog {
    fn new(label: String, binding: PickerBinding, current: Vec<String>, shared: Shared) -> Self {
        let title = format!("Edit {label}");
        let mut dlg = Dialog::new(Rect::new(0, 0, 80, 22), Some(title));
        dlg.state_mut().options.center_x = true;
        dlg.state_mut().options.center_y = true;

        // Column headers.
        dlg.insert_child(Box::new(Label::new(
            Rect::new(2, 1, 38, 2),
            "Available",
            None,
        )));
        dlg.insert_child(Box::new(Label::new(
            Rect::new(42, 1, 78, 2),
            "Members",
            None,
        )));

        // Left column: search box (row 2) over the candidate list (rows 4..18).
        let search = InputLine::with_limit(Rect::new(2, 2, 38, 3), 128);
        let search_id = dlg.insert_child(Box::new(search));
        let avail = ListBox::new(Rect::new(2, 4, 38, 18), 1, None, None);
        let avail_id = dlg.insert_child(Box::new(avail));

        // Right column: the staged members list (rows 4..18).
        let members_list = ListBox::new(Rect::new(42, 4, 78, 18), 1, None, None);
        let members_id = dlg.insert_child(Box::new(members_list));

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
        // member DN (the store value and the friendly label until a search reveals
        // a nicer one).
        let members: Vec<Candidate> = current
            .into_iter()
            .map(|v| Candidate {
                dn: v.clone(),
                label: v.clone(),
                store_value: v,
            })
            .collect();

        MembershipDialog {
            dlg,
            search_id,
            avail_id,
            members_id,
            shared,
            base: binding.scope.base.clone(),
            oc,
            attrs,
            store_attr,
            available: Vec::new(),
            members,
            last_search: String::new(),
            focus_members: false,
            seeded: false,
        }
    }

    /// Whether `dn` is already a staged member (case-insensitive DN compare).
    fn is_member(&self, dn: &str) -> bool {
        self.members
            .iter()
            .any(|m| m.store_value.eq_ignore_ascii_case(dn))
    }

    /// Rebuild the Available `ListBox` rows, marking candidates already in Members.
    fn rebuild_avail(&mut self, ctx: &mut Context, preserve_cursor: bool) {
        let rows: Vec<String> = self
            .available
            .iter()
            .map(|c| {
                let mark = if self.is_member(&c.store_value) {
                    "\u{2713} " // ✓
                } else {
                    "  "
                };
                format!("{mark}{}", c.label)
            })
            .collect();
        Self::repopulate(&mut self.dlg, self.avail_id, rows, ctx, preserve_cursor);
    }

    /// Rebuild the Members `ListBox` rows from the staged set.
    fn rebuild_members(&mut self, ctx: &mut Context, preserve_cursor: bool) {
        let rows: Vec<String> = self.members.iter().map(|m| m.label.clone()).collect();
        Self::repopulate(&mut self.dlg, self.members_id, rows, ctx, preserve_cursor);
    }

    /// Replace a list's rows, optionally preserving (and clamping) the cursor.
    fn repopulate(
        dlg: &mut Dialog,
        id: tv::ViewId,
        rows: Vec<String>,
        ctx: &mut Context,
        preserve_cursor: bool,
    ) {
        let rows_len = rows.len();
        if let Some(list) = dlg.child_mut(id) {
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

    /// Copy the latest pump-delivered search results into `available` and re-render
    /// the left column. Borrow-safe: clones out of `shared`, drops the borrow.
    fn sync_results(&mut self, ctx: &mut Context) {
        let results = {
            let st = self.shared.borrow();
            st.search_results.clone()
        };
        self.available = results;
        self.rebuild_avail(ctx, false);
    }

    /// Mirror the staged member set into shared state as the prospective commit.
    fn update_staged(&self) {
        let values: Vec<String> = self.members.iter().map(|m| m.store_value.clone()).collect();
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

    /// The current search-box text.
    fn current_search(&mut self) -> String {
        match self.dlg.child_mut(self.search_id).and_then(|v| v.value()) {
            Some(FieldValue::Text(s)) => s,
            _ => String::new(),
        }
    }

    /// The highlight index of `list_id`, if any.
    fn highlighted(&mut self, list_id: tv::ViewId) -> Option<usize> {
        match self.dlg.child_mut(list_id).and_then(|v| v.value()) {
            Some(FieldValue::Int(i)) if i >= 0 => Some(i as usize),
            _ => None,
        }
    }

    /// Move the highlighted Available candidate into Members (de-dup, no-op if
    /// already a member or nothing highlighted).
    fn move_into_members(&mut self, ctx: &mut Context) {
        let Some(idx) = self.highlighted(self.avail_id) else {
            return;
        };
        let Some(cand) = self.available.get(idx).cloned() else {
            return;
        };
        if self.is_member(&cand.store_value) {
            return; // de-dup: already a member.
        }
        self.members.push(cand);
        self.rebuild_members(ctx, false);
        self.rebuild_avail(ctx, true); // refresh the ✓ marker on the moved row.
        self.update_staged();
    }

    /// Remove the highlighted Members row (no-op if nothing highlighted).
    fn remove_from_members(&mut self, ctx: &mut Context) {
        let Some(idx) = self.highlighted(self.members_id) else {
            return;
        };
        if idx >= self.members.len() {
            return;
        }
        self.members.remove(idx);
        self.rebuild_members(ctx, true);
        self.rebuild_avail(ctx, true); // drop the ✓ marker on the removed member.
        self.update_staged();
    }
}

#[delegate(to = dlg)]
impl View for MembershipDialog {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    /// Seed on first open: render the seeded Members, copy any already-delivered
    /// results into `available`, stage the current members, and kick off an
    /// initial (empty-term) candidate search so the Available list fills in.
    fn reset_current(&mut self, ctx: &mut Context) {
        self.dlg.reset_current(ctx);
        if !self.seeded {
            self.seeded = true;
            self.rebuild_members(ctx, false);
            self.sync_results(ctx);
            self.update_staged();
            self.submit_search("");
        }
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        // Fallback seed for paths that deliver events without reset_current.
        if !self.seeded {
            self.seeded = true;
            self.rebuild_members(ctx, false);
            self.sync_results(ctx);
            self.update_staged();
            self.submit_search("");
        }

        // Pump-delivered results: refresh the Available column from shared state.
        if matches!(ev, Event::Broadcast { command, .. } if *command == REFRESH) {
            self.sync_results(ctx);
            self.dlg.handle_event(ev, ctx);
            return;
        }

        // Move keys. Space is NOT intercepted here so it reaches the search box.
        let move_in = matches!(ev, Event::KeyDown(k) if matches!(k.key, Key::Enter | Key::Right));
        let move_out = matches!(ev, Event::KeyDown(k) if matches!(k.key, Key::Delete | Key::Left));
        let toggle_focus = matches!(ev, Event::KeyDown(k) if k.key == Key::Tab);
        let nav = matches!(
            ev,
            Event::KeyDown(k)
                if matches!(k.key, Key::Up | Key::Down | Key::PageUp | Key::PageDown)
        );

        if move_in {
            self.move_into_members(ctx);
            ev.clear();
        } else if move_out {
            self.remove_from_members(ctx);
            ev.clear();
        } else if toggle_focus {
            // Flip which column the Up/Down keys drive; keep the framework focus on
            // the search box so typing keeps working.
            self.focus_members = !self.focus_members;
            ev.clear();
        } else if nav {
            let id = if self.focus_members {
                self.members_id
            } else {
                self.avail_id
            };
            if let Some(list) = self.dlg.child_mut(id) {
                list.handle_event(ev, ctx);
            }
        } else {
            self.dlg.handle_event(ev, ctx);
        }

        // Submit a fresh search when the search text changed.
        let cur = self.current_search();
        if cur != self.last_search {
            self.last_search = cur.clone();
            self.submit_search(&cur);
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
    use std::cell::RefCell;
    use std::rc::Rc;
    use tvision_rs::{timer::TimerQueue, Deferred, KeyEvent};

    const G1: &str = "cn=devs,ou=groups,dc=example,dc=org";
    const G2: &str = "cn=ops,ou=groups,dc=example,dc=org";

    fn schema() -> SchemaModel {
        SchemaModel::from_raw(&RawSubschema::default())
    }

    fn test_shared() -> Shared {
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
        if let Some(list) = dlg.dlg.child_mut(dlg.avail_id) {
            list.set_value_ctx(FieldValue::Int(1), &mut ctx);
        }
        let mut ev = Event::KeyDown(KeyEvent::from(Key::Right));
        dlg.handle_event(&mut ev, &mut ctx);

        let mut got = staged_set(&shared);
        got.sort();
        assert_eq!(got, vec![G1.to_string(), G2.to_string()]);

        // Highlight g1 (index 0 in Members) and press Delete → remove it.
        if let Some(list) = dlg.dlg.child_mut(dlg.members_id) {
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

        // Highlight g1 (already a member, index 0 in Available) and press Enter.
        if let Some(list) = dlg.dlg.child_mut(dlg.avail_id) {
            list.set_value_ctx(FieldValue::Int(0), &mut ctx);
        }
        let mut ev = Event::KeyDown(KeyEvent::from(Key::Enter));
        dlg.handle_event(&mut ev, &mut ctx);

        assert_eq!(
            staged_set(&shared),
            vec![G1.to_string()],
            "moving an existing member must not duplicate it"
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
        if let Some(list) = dlg.dlg.child_mut(dlg.avail_id) {
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
