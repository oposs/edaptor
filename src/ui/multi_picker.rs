//! Multi-select picker — a two-column "mover" dialog backed by [`Shuttle`].
//! Serves every multi-select `WidgetKind::Picker` binding: a plain multi picker
//! (e.g. `memberUid`, `member`) writes the picked store values onto this entry;
//! a fan-out binding (`fanout_attr.is_some()`, e.g. `memberOf`) instead writes
//! this entry's DN onto each picked candidate at save time (the combined-save
//! path handles that expansion). Any multi-select picker opens a modal with an
//! embedded [`Shuttle`] view (`ui::shuttle`) presenting **Available** and
//! **Members** columns:
//!
//! - **Available** (left): a list of live LDAP candidates with incremental find
//!   (`FindMode::Highlight`). Typing accumulates a query and highlights matches and
//!   broadcasts `Command::LIST_FIND_CHANGED`; this dialog submits an async candidate
//!   search (`SearchFlow`) via the worker exactly like the plain picker; results
//!   arrive on the next pump tick and are broadcast as `REFRESH`, which the dialog
//!   maps into `Shuttle::set_available`. Candidates already in Members are filtered
//!   out (the staged set is never offered for re-adding).
//! - **Members** (right): the staged member DN set, seeded from `field.values`
//!   (the user's current memberships / baseline) via `Shuttle::set_selected`.
//!
//! The `Shuttle` owns the column geometry and move actions (Insert moves into
//! Members, Delete removes, plus [Add]/[Remove] buttons, Enter-on-a-list moves) and
//! the Available list's incremental find; Tab/Shift-Tab focus traversal and the
//! pass-through of the default OK button are handled by the dialog. The Shuttle
//! notifies via broadcast (`CMD_SHUTTLE_CHANGED`); the Available list broadcasts
//! `Command::LIST_FIND_CHANGED` for find edits. This module keeps the
//! multi-picker-specific plumbing: the async candidate-search
//! submit, the pump/`REFRESH` seam that refreshes the Available column, member
//! seeding, and the `staged_commit` write-back.
//!
//! The staged set is mirrored into `staged_commit` as `SetValues(member_dns)`
//! after every move; OK applies it, Cancel discards it. The fan-out write (one
//! MODIFY per group) is produced from the diff against baseline by the
//! combined-save path. Mirrors the `picker` / `oc_picker` module shape: one file
//! holds `MultiPickerWidget` (FieldWidget), `MultiPickerEditor` (FieldEditor) and
//! `MultiPickerDialog` (the interactive `Dialog` view).

use std::collections::HashSet;

use tvision_rs::{
    self as tv, delegate, ButtonFlags, ButtonRowAlign, Command, Context, Dialog, Event, FindMode,
    Rect, View, ViewId,
};

use crate::config::relation::{PickerBinding, StoreKey};
use crate::config::widget::WidgetKind;
use crate::schema::SchemaModel;
use crate::ui::shuttle::{Shuttle, ShuttleRow, CMD_SHUTTLE_CHANGED};
use crate::ui::widget::{Activation, Capability, CommitOutcome, FieldEditor, FieldWidget};
use crate::ui::{Shared, REFRESH};
use crate::workflows::edit_form::EditField;

// ---------------------------------------------------------------------------
// MultiPickerWidget — FieldWidget plugin
// ---------------------------------------------------------------------------

/// The plugin for every multi-select `WidgetKind::Picker`-bound field (fan-out or not).
/// `present` summarises the member count; `activate` opens a `MultiPickerDialog`.
pub(crate) struct MultiPickerWidget;

impl FieldWidget for MultiPickerWidget {
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
        use crate::config::relation::Cardinality;
        match &field.widget_binding {
            Some(WidgetKind::Picker(b))
                if b.fanout_attr.is_some() || b.cardinality(field.multi) == Cardinality::Multi =>
            {
                Activation::Modal(Box::new(MultiPickerEditor {
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
// MultiPickerEditor — FieldEditor (carries state into the dialog builder)
// ---------------------------------------------------------------------------

/// Carries the field's binding + current member set into the dialog builder.
pub(crate) struct MultiPickerEditor {
    label: String,
    binding: PickerBinding,
    current: Vec<String>,
}

impl FieldEditor for MultiPickerEditor {
    fn into_view(
        self: Box<Self>,
        _schema: &SchemaModel,
        shared: Shared,
    ) -> (Box<dyn View>, tv::ViewId) {
        let MultiPickerEditor {
            label,
            binding,
            current,
        } = *self;
        let dlg = MultiPickerDialog::new(label, binding, current, shared);
        // Focus the Shuttle itself: it is a direct child of the dialog, so this sets
        // the dialog's current child (events route into it) and cascades focus to the
        // Shuttle's own open-time target (the Available list, for type-to-find).
        let focus = dlg.shuttle_id;
        (Box::new(dlg), focus)
    }
}

// ---------------------------------------------------------------------------
// MultiPickerDialog — the interactive two-column mover with live search
// ---------------------------------------------------------------------------

/// Available list (live candidates with incremental find) on the left, Members
/// list on the right — both owned by the embedded [`Shuttle`]. Candidate results
/// arrive via the pump and the `REFRESH` broadcast.
pub(crate) struct MultiPickerDialog {
    dlg: Dialog,
    /// The embedded two-list transfer widget (a child of `dlg`). Owns the column
    /// geometry, the staged Members set (Selected) and the live candidate set
    /// (Available), plus the moves; it notifies us by broadcast.
    shuttle_id: ViewId,
    shared: Shared,
    /// Resolved candidate-search scope (groups).
    base: String,
    oc: String,
    attrs: Vec<String>,
    /// `Some(attr)` for a scalar store; `None` for a DN store (the usual case).
    store_attr: Option<String>,
    /// The initial Members rows, seeded from the field's values; moved into the
    /// Shuttle on first open (when a `Context` is available).
    seed_members: Vec<ShuttleRow>,
    seeded: bool,
}

impl MultiPickerDialog {
    fn new(label: String, binding: PickerBinding, current: Vec<String>, shared: Shared) -> Self {
        let title = format!("Edit {label}");
        let mut dlg = Dialog::new(Rect::new(0, 0, 80, 22), Some(title));
        dlg.state_mut().options.center_x = true;
        dlg.state_mut().options.center_y = true;

        // Build the two columns. Available on the left (membership convention),
        // Available on the left, Members (the Selected set) on the right — the
        // conventional transfer layout. Insert the Shuttle FIRST so it is the
        // dialog's first selectable child (the modal's open-time reset_current
        // then makes it current, and focus reaches the Available list inside it).
        let shuttle = Shuttle::new(
            Rect::new(0, 0, 80, 22),
            "Available",
            "Members",
            /* find */ FindMode::Highlight,
        );
        let shuttle_id = dlg.insert_child(Box::new(shuttle));

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
        // reveals a nicer one). Members are never locked.
        let seed_members: Vec<ShuttleRow> = current
            .into_iter()
            .map(|v| ShuttleRow {
                key: v.clone(),
                label: v,
                locked: false,
            })
            .collect();

        MultiPickerDialog {
            dlg,
            shuttle_id,
            shared,
            base: binding.scope.base.clone(),
            oc,
            attrs,
            store_attr,
            seed_members,
            seeded: false,
        }
    }

    /// The embedded `Shuttle`, downcast out of the dialog's children.
    fn shuttle_mut(&mut self) -> Option<&mut Shuttle> {
        self.dlg
            .child_mut(self.shuttle_id)?
            .as_any_mut()?
            .downcast_mut::<Shuttle>()
    }

    /// The Available list's current incremental-find query (owned copy; releases
    /// the borrow). Drives the async candidate search.
    fn shuttle_find_query(&mut self) -> String {
        self.shuttle_mut()
            .map(|sh| sh.find_query())
            .unwrap_or_default()
    }

    /// Copy the latest pump-delivered search results into the Available column,
    /// dropping any candidate already in Members (the Selected set is never offered
    /// for re-adding — Available rows render plain, with no "already a member" mark).
    /// Borrow-safe: clones out of `shared` and reads the staged set into locals,
    /// dropping both borrows before touching the Shuttle.
    fn sync_results(&mut self, ctx: &mut Context) {
        let results = {
            let st = self.shared.borrow();
            st.search_results.clone()
        };

        // Upgrade the staged Members' labels from any matching candidate. A member
        // is seeded only from its raw store value (a DN for `member`), so it first
        // renders as that DN; once a candidate search reveals the same store value
        // with a friendly label, adopt it so both columns show the same nice view.
        // (Members not yet returned by a search keep their store value until then.)
        let label_of: std::collections::HashMap<String, String> = results
            .iter()
            .map(|c| (c.store_value.to_lowercase(), c.label.clone()))
            .collect();
        let relabeled: Option<Vec<ShuttleRow>> = self.shuttle_mut().map(|sh| {
            sh.selected()
                .iter()
                .map(|r| ShuttleRow {
                    key: r.key.clone(),
                    label: label_of
                        .get(&r.key.to_lowercase())
                        .cloned()
                        .unwrap_or_else(|| r.label.clone()),
                    locked: r.locked,
                })
                .collect()
        });
        if let Some(rows) = relabeled {
            if let Some(sh) = self.shuttle_mut() {
                sh.set_selected(rows, ctx);
            }
        }

        let already: HashSet<String> = match self.shuttle_mut() {
            Some(sh) => sh.selected().iter().map(|r| r.key.to_lowercase()).collect(),
            None => return,
        };
        let rows: Vec<ShuttleRow> = results
            .into_iter()
            .filter(|c| !already.contains(&c.store_value.to_lowercase()))
            .map(|c| ShuttleRow {
                key: c.store_value,
                label: c.label,
                locked: false,
            })
            .collect();
        if let Some(sh) = self.shuttle_mut() {
            sh.set_available(rows, ctx);
        }
    }

    /// Mirror the staged member set into shared state as the prospective commit.
    /// The Shuttle's Selected rows' `key`s are the member store values.
    fn update_staged(&mut self) {
        let values: Vec<String> = match self.shuttle_mut() {
            Some(sh) => sh.selected().iter().map(|r| r.key.clone()).collect(),
            None => return,
        };
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
        if let Some(sh) = self.shuttle_mut() {
            sh.set_selected(seed, ctx);
        }
        self.sync_results(ctx);
        self.update_staged();
        self.submit_search("");
    }
}

#[delegate(to = dlg)]
impl View for MultiPickerDialog {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    /// Seed on first open (see [`MultiPickerDialog::seed`]).
    fn reset_current(&mut self, ctx: &mut Context) {
        self.dlg.reset_current(ctx);
        if !self.seeded {
            self.seed(ctx);
        }
        // Establish the Shuttle's internal currency (the Available list) before
        // the dialog focuses the Shuttle, so focus cascades onto the list and
        // type-to-find works immediately.
        if let Some(sh) = self.shuttle_mut() {
            sh.reset_current(ctx);
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

        // Delegate first: the Shuttle (the dialog's current child) handles the move
        // keys and the Available list's incremental find, broadcasting the outcome;
        // the dialog's OK/Cancel and Tab traversal are handled here too. We then
        // react to the broadcasts when they are delivered (a later loop iteration).
        self.dlg.handle_event(ev, ctx);

        // Both shuttle lists have a find mode and each broadcasts
        // LIST_FIND_CHANGED with its own id. Only the AVAILABLE list's find drives
        // an async candidate re-query; the Members list's find is a local
        // highlight, so ignore its broadcast (else typing in Members would reload
        // the Available column with the unchanged query).
        let avail_src = self.shuttle_mut().map(|sh| sh.available_id());
        let notice = match &*ev {
            Event::Broadcast { command, source } if *source == Some(self.shuttle_id) => {
                Some(*command)
            }
            Event::Broadcast { command, source }
                if *command == Command::LIST_FIND_CHANGED && *source == avail_src =>
            {
                Some(Command::LIST_FIND_CHANGED)
            }
            _ => None,
        };
        match notice {
            // A find edit: submit an async candidate search for the query. Results
            // land on the next pump's REFRESH broadcast and refill Available.
            Some(cmd) if cmd == Command::LIST_FIND_CHANGED => {
                let term = self.shuttle_find_query();
                self.submit_search(&term);
            }
            // A move: restage the member set and re-filter Available so the moved
            // candidate leaves (move-in) or a freed member reappears (move-out).
            Some(cmd) if cmd == CMD_SHUTTLE_CHANGED => {
                self.sync_results(ctx);
                self.update_staged();
            }
            _ => {}
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
    use tvision_rs::{timer::TimerQueue, Deferred, Key, KeyEvent};

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
        let ed: Box<dyn FieldEditor> = Box::new(MultiPickerEditor {
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

    fn dialog_mut(view: &mut Box<dyn View>) -> &mut MultiPickerDialog {
        view.as_any_mut()
            .and_then(|a| a.downcast_mut::<MultiPickerDialog>())
            .expect("downcast MultiPickerDialog")
    }

    /// Run `f` with a fresh headless `Context` over the given backing stores.
    fn with_ctx<R>(
        out: &mut std::collections::VecDeque<tv::Event>,
        timers: &mut TimerQueue,
        deferred: &mut Vec<Deferred>,
        f: impl FnOnce(&mut Context) -> R,
    ) -> R {
        let mut ctx = Context::new(out, timers, 0, deferred);
        f(&mut ctx)
    }

    /// Highlight `label` in the Available column (plain rows — matched directly).
    fn highlight_avail_by_label(d: &mut MultiPickerDialog, label: &str, ctx: &mut Context) {
        let sh = d.shuttle_mut().expect("shuttle present");
        let id = sh.avail_id_for_test();
        let idx = sh
            .avail_text()
            .iter()
            .position(|s| s.eq_ignore_ascii_case(label))
            .unwrap_or_else(|| panic!("{label} not displayed in Available"));
        sh.highlight(id, idx as i32, ctx);
    }

    /// Highlight `label` in the Members column (Selected rows carry a 2-char marker).
    fn highlight_member_by_label(d: &mut MultiPickerDialog, label: &str, ctx: &mut Context) {
        let sh = d.shuttle_mut().expect("shuttle present");
        let id = sh.selected_id_for_test();
        let idx = sh
            .selected_text()
            .iter()
            .position(|s| {
                s.chars()
                    .skip(2)
                    .collect::<String>()
                    .eq_ignore_ascii_case(label)
            })
            .unwrap_or_else(|| panic!("{label} not displayed in Members"));
        sh.highlight(id, idx as i32, ctx);
    }

    /// Pull the next queued `Event::Broadcast` out of the loop output, if any.
    fn take_broadcast(out: &mut std::collections::VecDeque<tv::Event>) -> Option<tv::Event> {
        let i = out
            .iter()
            .position(|e| matches!(e, Event::Broadcast { .. }))?;
        out.remove(i)
    }

    /// Dispatch `key` directly to the embedded Shuttle (the seam the dialog's focus
    /// routing reaches in the running app), then faithfully deliver any broadcasts
    /// the Shuttle queued (a move → `CMD_SHUTTLE_CHANGED`) back to the dialog. A
    /// `LIST_FIND_CHANGED` broadcast is left in `out` — submitting an async search
    /// is exercised separately; here we only settle moves.
    fn press_and_settle(
        d: &mut MultiPickerDialog,
        key: Key,
        out: &mut std::collections::VecDeque<tv::Event>,
        timers: &mut TimerQueue,
        deferred: &mut Vec<Deferred>,
    ) {
        {
            let mut ev = Event::KeyDown(KeyEvent::from(key));
            let mut ctx = Context::new(out, timers, 0, deferred);
            if let Some(sh) = d.shuttle_mut() {
                sh.handle_event(&mut ev, &mut ctx);
            }
        }
        while let Some(mut bev) = take_broadcast(out) {
            let mut ctx = Context::new(out, timers, 0, deferred);
            d.handle_event(&mut bev, &mut ctx);
        }
    }

    // -- widget routing ----------------------------------------------------

    #[test]
    fn present_summarises_member_count() {
        let w = MultiPickerWidget;
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
            MultiPickerWidget.activate(&f),
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
        with_ctx(&mut out, &mut timers, &mut deferred, |ctx| {
            view.reset_current(ctx)
        });

        // Baseline staged set = [g1]. g1 (already a member) is filtered out of
        // Available, which therefore offers only g2.
        assert_eq!(staged_set(&shared), vec![G1.to_string()]);

        let d = dialog_mut(&mut view);
        // Highlight g2 in Available and press Insert → move into Members.
        with_ctx(&mut out, &mut timers, &mut deferred, |ctx| {
            highlight_avail_by_label(d, G2, ctx)
        });
        press_and_settle(d, Key::Insert, &mut out, &mut timers, &mut deferred);

        let mut got = staged_set(&shared);
        got.sort();
        assert_eq!(got, vec![G1.to_string(), G2.to_string()]);

        // Highlight g1 in Members and press Delete → remove it.
        with_ctx(&mut out, &mut timers, &mut deferred, |ctx| {
            highlight_member_by_label(d, G1, ctx)
        });
        press_and_settle(d, Key::Delete, &mut out, &mut timers, &mut deferred);

        assert_eq!(staged_set(&shared), vec![G2.to_string()]);
    }

    /// A seeded member starts life as its raw store value (a DN), but once a
    /// candidate search reveals the same store value with a friendly label, the
    /// Members column adopts it — so both columns show the same nice view rather
    /// than names on one side and DNs on the other.
    #[test]
    fn seeded_member_adopts_friendly_label_from_matching_candidate() {
        let shared = test_shared();
        // The search returns g1 with a friendly label (not just its DN).
        shared.borrow_mut().search_results = vec![Candidate {
            dn: G1.into(),
            label: "devs (developers)".into(),
            store_value: G1.into(),
        }];
        let mut view = build_dialog(&shared, &[G1]);
        let mut out = std::collections::VecDeque::new();
        let mut timers = TimerQueue::new();
        let mut deferred = Vec::new();
        with_ctx(&mut out, &mut timers, &mut deferred, |ctx| {
            view.reset_current(ctx)
        });

        let d = dialog_mut(&mut view);
        let members = d.shuttle_mut().expect("shuttle present").selected_text();
        assert!(
            members.iter().any(|s| s.contains("devs (developers)")),
            "the seeded member must adopt the candidate's friendly label, got {members:?}"
        );
        assert!(
            !members.iter().any(|s| s.contains(G1)),
            "the raw DN must no longer be shown once a label is known, got {members:?}"
        );
    }

    /// A candidate already in Members is filtered out of the Available column —
    /// the staged set is never offered for re-adding (Available rows render plain,
    /// so there is no "already a member" marker to lean on).
    #[test]
    fn already_member_is_filtered_from_available() {
        let shared = test_shared();
        shared.borrow_mut().search_results = vec![cand(G1), cand(G2)];

        let mut view = build_dialog(&shared, &[G1]);
        let mut out = std::collections::VecDeque::new();
        let mut timers = TimerQueue::new();
        let mut deferred = Vec::new();
        with_ctx(&mut out, &mut timers, &mut deferred, |ctx| {
            view.reset_current(ctx)
        });

        let d = dialog_mut(&mut view);
        let avail = d.shuttle_mut().expect("shuttle").avail_text();
        assert!(
            !avail.iter().any(|s| s.eq_ignore_ascii_case(G1)),
            "the existing member g1 must not appear in Available, got {avail:?}"
        );
        assert!(
            avail.iter().any(|s| s.eq_ignore_ascii_case(G2)),
            "the non-member g2 must appear in Available, got {avail:?}"
        );
    }

    /// Typing a find query (letters and Space) into the focused Available list
    /// must NOT move the highlighted candidate — the list's find mode consumes
    /// the keys to build the query (guards against the old picker's stolen-Space
    /// bug). A move is a Shuttle concern covered by the widget's own tests.
    #[test]
    fn typing_a_find_query_does_not_move_a_candidate() {
        let shared = test_shared();
        shared.borrow_mut().search_results = vec![cand(G1), cand(G2)];

        let mut view = build_dialog(&shared, &[G1]);
        let mut out = std::collections::VecDeque::new();
        let mut timers = TimerQueue::new();
        let mut deferred = Vec::new();
        with_ctx(&mut out, &mut timers, &mut deferred, |ctx| {
            view.reset_current(ctx)
        });

        let d = dialog_mut(&mut view);
        with_ctx(&mut out, &mut timers, &mut deferred, |ctx| {
            let avail = d.shuttle_mut().expect("shuttle").avail_id_for_test();
            d.shuttle_mut().expect("shuttle").focus_for_test(avail, ctx);
            highlight_avail_by_label(d, G2, ctx);
            for ch in ['x', ' ', 'y'] {
                let mut ev = Event::KeyDown(KeyEvent::from(Key::Char(ch)));
                d.shuttle_mut().expect("shuttle").handle_event(&mut ev, ctx);
            }
        });

        let members: Vec<String> = d
            .shuttle_mut()
            .expect("shuttle")
            .selected()
            .iter()
            .map(|r| r.key.clone())
            .collect();
        assert_eq!(
            members,
            vec![G1.to_string()],
            "typing a find query must not move a candidate"
        );
    }

    /// A non-fanout memberUid binding (scalar `uid` store) seeds Available from the
    /// delivered candidates and stages `SetValues([uid])` when a candidate is moved in.
    /// Proves the routing change: MultiPickerDialog handles multi non-fanout pickers.
    #[test]
    fn nonfanout_scalar_picker_seeds_moves_and_stages_uid() {
        use crate::config::relation::{CandidateScope, Cardinality, PickerBinding, StoreKey};

        let shared = test_shared();
        // A delivered candidate whose scalar store_value is a uid (not a DN).
        shared.borrow_mut().search_results = vec![Candidate {
            dn: "uid=ann,ou=people,dc=example,dc=org".into(),
            label: "Ann Smith".into(),
            store_value: "ann".into(),
        }];

        let binding = PickerBinding {
            attr: "memberUid".into(),
            scope: CandidateScope {
                base: "ou=people,dc=example,dc=org".into(),
                object_classes: vec!["inetOrgPerson".into()],
                search_attrs: vec!["uid".into()],
                label_template: None,
            },
            store: StoreKey::Attr("uid".into()),
            select: Some(Cardinality::Multi),
            fanout_attr: None,
        };
        let ed: Box<dyn FieldEditor> = Box::new(MultiPickerEditor {
            label: "memberUid".into(),
            binding,
            current: vec![],
        });
        let (mut view, _focus) = ed.into_view(&schema(), shared.clone());

        let mut out = std::collections::VecDeque::new();
        let mut timers = TimerQueue::new();
        let mut deferred = Vec::new();
        with_ctx(&mut out, &mut timers, &mut deferred, |ctx| {
            view.reset_current(ctx)
        });

        // The scalar candidate is offered in Available (keyed by its uid).
        let d = dialog_mut(&mut view);
        with_ctx(&mut out, &mut timers, &mut deferred, |ctx| {
            highlight_avail_by_label(d, "Ann Smith", ctx);
        });
        press_and_settle(d, Key::Insert, &mut out, &mut timers, &mut deferred);

        assert_eq!(
            shared.borrow().staged_commit,
            Some(CommitOutcome::SetValues(vec!["ann".to_string()])),
            "moving the scalar candidate in must stage its uid, not its DN"
        );
    }

    /// Typing into the Available list broadcasts `LIST_FIND_CHANGED` (the signal
    /// the dialog turns into an async candidate search).
    #[test]
    fn typing_broadcasts_list_find_changed() {
        let shared = test_shared();
        shared.borrow_mut().search_results = vec![cand(G1), cand(G2)];

        let mut view = build_dialog(&shared, &[G1]);
        let mut out = std::collections::VecDeque::new();
        let mut timers = TimerQueue::new();
        let mut deferred = Vec::new();
        with_ctx(&mut out, &mut timers, &mut deferred, |ctx| {
            view.reset_current(ctx)
        });

        let d = dialog_mut(&mut view);
        with_ctx(&mut out, &mut timers, &mut deferred, |ctx| {
            let avail = d.shuttle_mut().expect("shuttle").avail_id_for_test();
            d.shuttle_mut().expect("shuttle").focus_for_test(avail, ctx);
            let mut ev = Event::KeyDown(KeyEvent::from(Key::Char('g')));
            d.shuttle_mut().expect("shuttle").handle_event(&mut ev, ctx);
        });

        assert!(
            out.iter().any(|e| matches!(
                e,
                Event::Broadcast { command, .. } if *command == Command::LIST_FIND_CHANGED
            )),
            "typing must broadcast LIST_FIND_CHANGED"
        );
    }
}
