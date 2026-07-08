//! The objectClass field editor: a schema-seeded two-column mover. The
//! **Available** column (left) shows the remaining known classes; the **Active**
//! column (right) holds the currently active set. Moving a class toward Active
//! ticks it; moving it away unticks it. STRUCTURAL classes that were already on
//! the entry are shown locked (non-removable); a structural class added this
//! session stays removable so an add can be undone.
//! The prospective `SetValuesThenResyncSchema` outcome is kept in
//! `UiState::staged_commit`. Capability: `NeedsSchema` (no worker).
//!
//! Built on the embedded [`Shuttle`] view (`ui::shuttle`). Unlike membership,
//! the Available column is a *static* set computed locally (all known classes
//! minus the active ones), so there is no async worker search — incremental
//! filtering is the Available list's own `FindMode::Filter`, which narrows the
//! column in place as the user types. The Shuttle notifies via broadcast
//! (`CMD_SHUTTLE_CHANGED`, with the Shuttle's own `ViewId` as `source`); this
//! dialog reacts in its own `handle_event` after delegating, re-reading
//! `Shuttle::selected`.

use std::collections::{BTreeSet, HashSet};

use ldap_types::schema::ObjectClassType;
use tvision_rs::{
    self as tv, delegate, ButtonFlags, ButtonRowAlign, Command, Context, Dialog, Event, FindMode,
    Rect, View, ViewId,
};

use crate::schema::SchemaModel;
use crate::ui::shuttle::{Shuttle, ShuttleRow, CMD_SHUTTLE_CHANGED};
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
        // The active set at open, lowercased for case-insensitive matching against
        // the canonical candidate spelling.
        let ticked: BTreeSet<String> = self.current.iter().map(|s| s.to_lowercase()).collect();
        // Every known class becomes a `ShuttleRow`. A STRUCTURAL class is locked
        // ONLY when it was already on the entry at open — those are the
        // load-bearing classes worth protecting. A structural class ADDED in this
        // session stays unlocked, so the user can undo an add (the reported bug:
        // you could add a class but then never drop it again). Names keep the
        // schema's canonical spelling — `object_class_names` is already sorted.
        let all_rows: Vec<ShuttleRow> = schema
            .object_class_names()
            .into_iter()
            .map(|name| {
                let structural = schema
                    .object_class(&name)
                    .map(|oc| oc.object_class_type == ObjectClassType::Structural)
                    .unwrap_or(false);
                let originally_active = ticked.contains(&name.to_lowercase());
                ShuttleRow {
                    key: name.clone(),
                    label: name,
                    locked: structural && originally_active,
                }
            })
            .collect();
        let picker = ObjectClassPicker::new(all_rows, ticked, shared);
        // Focus the Shuttle itself: it is a direct child of the dialog, so this sets
        // the dialog's current child (events route into it) and cascades focus to the
        // Shuttle's own open-time target (the Available list, for type-to-find).
        let focus = picker.shuttle_id;
        (Box::new(picker), focus)
    }
}

/// The interactive dialog: an embedded [`Shuttle`] (Active / Available) + OK/Cancel.
pub(crate) struct ObjectClassPicker {
    dlg: Dialog,
    /// The embedded two-list transfer widget (a child of `dlg`). Owns the column
    /// geometry, the active set (Selected) and the available set, plus the moves;
    /// it notifies us by broadcast.
    shuttle_id: ViewId,
    shared: Shared,
    /// Every known class as a `ShuttleRow` (canonical, sorted, `locked` precomputed).
    /// The source of truth for both columns: the Active column is the subset that
    /// is ticked, the Available column is the rest (the list's own find mode
    /// narrows the displayed Available rows as the user types).
    all_rows: Vec<ShuttleRow>,
    /// The active rows to seed on first open (stashed because `set_selected` needs
    /// a `Context`, only available once the modal is inserted).
    seed_selected: Vec<ShuttleRow>,
    seeded: bool,
}

impl ObjectClassPicker {
    fn new(all_rows: Vec<ShuttleRow>, ticked: BTreeSet<String>, shared: Shared) -> Self {
        let mut dlg = Dialog::new(Rect::new(0, 0, 72, 25), Some("Object classes".to_string()));
        dlg.state_mut().options.center_x = true;
        dlg.state_mut().options.center_y = true;
        // Allow the user to resize the dialog (grow flag also enables drag_grow).
        dlg.set_flags(tv::WindowFlags {
            r#move: true,
            close: true,
            grow: true,
            ..tv::WindowFlags::default()
        });
        // Floor the resize at the Shuttle's content minimum PLUS this dialog's own
        // chrome: 2 cols each side (frame + 1-cell breathing gap) and 6 rows
        // vertically (top inset 2, spacer 1, OK/Cancel 2, bottom frame 1). tvision
        // 0.10's settable window minimum governs the interactive corner-drag too
        // (the bare Window floor is only 16×6).
        dlg.set_min_size(tv::Point::new(Shuttle::MIN_W + 4, Shuttle::MIN_H + 6));

        // Conventional transfer layout: Available on the LEFT, Active (the
        // Selected set) on the RIGHT. Insert the Shuttle FIRST so it is the
        // dialog's first selectable child: the modal's open-time `reset_current`
        // then makes it current, so key events route into it (and reach the
        // Available list inside it).
        //
        // Position it in the interior with a 1-cell breathing gap inside the frame
        // (top-left 2,2) and stop above the host's OK/Cancel row (bottom at H-4). A
        // child spanning the frame border would sit on top of the frame's
        // close/move/resize hot-zones and kill the close icon; keeping it off the
        // border avoids that. grow_mode hi_x/hi_y shifts the far edges by the owner
        // delta, so both the right gap and the OK/Cancel row survive a resize.
        let shuttle = Shuttle::new(
            Rect::new(2, 2, 70, 21),
            "Available",
            "Active",
            /* find */ FindMode::Filter,
        );
        let shuttle_id = dlg.insert_child(Box::new(shuttle));

        let button_ids = dlg.button_row(
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
        // Keep OK/Cancel pinned to the bottom-right as the dialog grows: both the
        // top and bottom edges track the owner (lo_y + hi_y translate the fixed-
        // height button down), likewise lo_x + hi_x to the right.
        for id in button_ids {
            if let Some(b) = dlg.child_mut(id) {
                let gm = &mut b.state_mut().grow_mode;
                gm.lo_x = true;
                gm.hi_x = true;
                gm.lo_y = true;
                gm.hi_y = true;
            }
        }

        // The active rows to seed: every candidate whose (lowercased) name is in the
        // ticked set, in canonical sorted order.
        let seed_selected: Vec<ShuttleRow> = all_rows
            .iter()
            .filter(|r| ticked.contains(&r.key.to_lowercase()))
            .cloned()
            .collect();

        ObjectClassPicker {
            dlg,
            shuttle_id,
            shared,
            all_rows,
            seed_selected,
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

    /// Rebuild the Available column = all known classes minus the active set. The
    /// Available list's `FindMode::Filter` narrows the *displayed* rows by the
    /// live query on top of this set (re-applied on each `set_available`), so no
    /// search term is threaded here. Borrow-safe: reads the active key set into a
    /// local (releasing the Shuttle borrow) before touching `set_available`.
    fn refresh_available(&mut self, ctx: &mut Context) {
        let active: HashSet<String> = match self.shuttle_mut() {
            Some(sh) => sh.selected().iter().map(|r| r.key.to_lowercase()).collect(),
            None => return,
        };
        let rows: Vec<ShuttleRow> = self
            .all_rows
            .iter()
            .filter(|r| !active.contains(&r.key.to_lowercase()))
            .cloned()
            .collect();
        if let Some(sh) = self.shuttle_mut() {
            sh.set_available(rows, ctx);
        }
    }

    /// Write the prospective commit into shared state: the active class names in
    /// canonical, candidate-sorted order (equivalent to the previous single-list
    /// behaviour — `candidates` filtered by the ticked set).
    fn update_staged(&mut self) {
        let active: HashSet<String> = match self.shuttle_mut() {
            Some(sh) => sh.selected().iter().map(|r| r.key.to_lowercase()).collect(),
            None => return,
        };
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
        if let Some(sh) = self.shuttle_mut() {
            sh.set_selected(seed, ctx);
        }
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
        // Establish the Shuttle's internal currency (the Available list) before
        // the dialog focuses the Shuttle, so focus cascades onto the list and
        // type-to-find works immediately.
        if let Some(sh) = self.shuttle_mut() {
            sh.reset_current(ctx);
        }
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        // Fallback seed for paths that deliver events without first calling
        // reset_current (e.g. direct unit-test event injection).
        if !self.seeded {
            self.seed(ctx);
        }

        // Delegate first: the Shuttle (the dialog's current child) handles the
        // move keys and broadcasts the outcome; the dialog's OK/Cancel and Tab
        // traversal are handled here too. Incremental filtering of the Available
        // column is the list's own `FindMode::Filter` — it narrows in place and
        // needs no reaction here. We react only to a membership change (a later
        // loop iteration) to rebuild Available and restage.
        self.dlg.handle_event(ev, ctx);

        let changed = matches!(
            &*ev,
            Event::Broadcast { command, source }
                if *source == Some(self.shuttle_id) && *command == CMD_SHUTTLE_CHANGED
        );
        if changed {
            // A class was (un)ticked: rebuild Available = candidates minus the new
            // active set, and restage. (A locked STRUCTURAL row is rejected by the
            // Shuttle before it ever broadcasts, so no special-casing.)
            self.refresh_available(ctx);
            self.update_staged();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldap::worker::RawSubschema;
    use std::cell::RefCell;
    use std::rc::Rc;
    use tvision_rs::{Deferred, Key, KeyEvent};

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

    /// Highlight `label` in the Active (Selected) column by its display position.
    /// The Selected column is an unsorted plain `ListBox`, so the display index is
    /// the model index; rows carry a 2-char lock marker, stripped before matching.
    fn highlight_active_by_label(p: &mut ObjectClassPicker, label: &str, ctx: &mut Context) {
        let sh = p.shuttle_mut().expect("shuttle present");
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
            .unwrap_or_else(|| panic!("{label} not displayed in Active"));
        sh.highlight(id, idx as i32, ctx);
    }

    /// Highlight `label` in the Available column. Available rows render plain (no
    /// marker), so the label is matched directly against the display string.
    fn highlight_avail_by_label(p: &mut ObjectClassPicker, label: &str, ctx: &mut Context) {
        let sh = p.shuttle_mut().expect("shuttle present");
        let id = sh.avail_id_for_test();
        let idx = sh
            .avail_text()
            .iter()
            .position(|s| s.eq_ignore_ascii_case(label))
            .unwrap_or_else(|| panic!("{label} not displayed in Available"));
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
    /// the Shuttle queued (a move → `CMD_SHUTTLE_CHANGED`) back to the picker. In
    /// the real loop these broadcasts are delivered on later iterations; here we
    /// drain them so the picker reacts within the test.
    fn press_and_settle(
        p: &mut ObjectClassPicker,
        key: Key,
        out: &mut std::collections::VecDeque<tv::Event>,
        timers: &mut tv::timer::TimerQueue,
        deferred: &mut Vec<Deferred>,
    ) {
        {
            let mut ev = Event::KeyDown(KeyEvent::from(key));
            let mut ctx = Context::new(out, timers, 0, deferred);
            if let Some(sh) = p.shuttle_mut() {
                sh.handle_event(&mut ev, &mut ctx);
            }
        }
        while let Some(mut bev) = take_broadcast(out) {
            let mut ctx = Context::new(out, timers, 0, deferred);
            p.handle_event(&mut bev, &mut ctx);
        }
    }

    /// Run `f` with a fresh headless `Context` over the given backing stores.
    fn with_ctx<R>(
        out: &mut std::collections::VecDeque<tv::Event>,
        timers: &mut tv::timer::TimerQueue,
        deferred: &mut Vec<Deferred>,
        f: impl FnOnce(&mut Context) -> R,
    ) -> R {
        let mut ctx = Context::new(out, timers, 0, deferred);
        f(&mut ctx)
    }

    /// A STRUCTURAL class ADDED in this session must stay unlocked — only the
    /// classes already on the entry at open are locked. (Reported bug: you could
    /// add a `*` class but then never drop it again.)
    #[test]
    fn dialog_resize_floor_matches_the_shuttle_minimum() {
        // set_min_size must reach the window: the picker delegates size_limits to
        // the dialog, so a large-owner query reports the Shuttle floor, not
        // tvision's bare 16×6 Window default. This is what stops an interactive
        // drag from shrinking the dialog below its usable content size. The dialog
        // floor is the Shuttle content minimum PLUS this dialog's chrome (2 cols each
        // side, 6 rows for the top inset + spacer + OK/Cancel + bottom frame).
        let sh = shared();
        let view = build_view(&sh, &["person"]);
        let (min, _max) = view.size_limits(tv::Point::new(300, 100));
        assert_eq!(min, tv::Point::new(Shuttle::MIN_W + 4, Shuttle::MIN_H + 6));
    }

    #[test]
    fn shuttle_is_inset_off_the_frame_border() {
        // Regression: the embedded Shuttle must NOT span the full dialog rect. A
        // child covering the frame border sits on top of the frame's close / move /
        // resize hot-zones in mouse routing (it is inserted after the frame, so it
        // is in front), and the close icon silently does nothing. The Shuttle is
        // inset into the interior (a 1-cell breathing gap inside the frame) so those
        // hot-zones stay hittable.
        let sh = shared();
        let mut view = build_view(&sh, &["person"]);
        let p = picker_mut(&mut view);
        let b = p
            .shuttle_mut()
            .expect("shuttle present")
            .state()
            .get_bounds();
        assert!(
            b.a.x >= 2 && b.a.y >= 2,
            "shuttle top-left must clear the frame border + breathing gap (was {:?})",
            b.a
        );
    }

    #[test]
    fn session_added_structural_class_can_be_removed() {
        let sh = shared();
        // person (STRUCTURAL) is the only originally-active class → locked.
        // organizationalPerson (STRUCTURAL) is available; adding it must NOT lock it.
        let mut view = build_view(&sh, &["person"]);
        let mut out = std::collections::VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<Deferred> = Vec::new();
        with_ctx(&mut out, &mut timers, &mut deferred, |ctx| {
            view.reset_current(ctx)
        });

        let p = picker_mut(&mut view);
        // Add organizationalPerson from the Available column.
        with_ctx(&mut out, &mut timers, &mut deferred, |ctx| {
            highlight_avail_by_label(p, "organizationalPerson", ctx)
        });
        press_and_settle(p, Key::Insert, &mut out, &mut timers, &mut deferred);
        assert!(
            staged(&sh)
                .iter()
                .any(|s| s.eq_ignore_ascii_case("organizationalPerson")),
            "organizationalPerson must be added"
        );
        // It must be unlocked despite being STRUCTURAL — it wasn't originally active.
        let added_locked = p
            .shuttle_mut()
            .expect("shuttle")
            .selected()
            .iter()
            .find(|r| r.label.eq_ignore_ascii_case("organizationalPerson"))
            .expect("added class present in Active column")
            .locked;
        assert!(
            !added_locked,
            "a structural class added this session must stay unlocked"
        );

        // Now drop it again.
        with_ctx(&mut out, &mut timers, &mut deferred, |ctx| {
            highlight_active_by_label(p, "organizationalPerson", ctx)
        });
        press_and_settle(p, Key::Delete, &mut out, &mut timers, &mut deferred);
        assert!(
            !staged(&sh)
                .iter()
                .any(|s| s.eq_ignore_ascii_case("organizationalPerson")),
            "session-added class must be removable again"
        );
        // The originally-active structural 'person' stays locked.
        let person_locked = p
            .shuttle_mut()
            .expect("shuttle")
            .selected()
            .iter()
            .find(|r| r.label.eq_ignore_ascii_case("person"))
            .expect("person still active")
            .locked;
        assert!(
            person_locked,
            "originally-active structural class stays locked"
        );
    }

    /// Delete removes the *highlighted* Selected row (mapped by label, the index a
    /// real user lands on), leaving a locked sibling untouched. The Selected column
    /// is a plain unsorted `ListBox`, so the display index is the model index — but
    /// this still guards that the highlight, not a fixed slot, drives the removal.
    #[test]
    fn move_out_removes_highlighted_row_and_keeps_locked() {
        let sh = shared();
        // Active = {person (STRUCTURAL → locked), top (ABSTRACT → unlocked)}.
        let mut view = build_view(&sh, &["person", "top"]);
        let mut out = std::collections::VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<Deferred> = Vec::new();
        with_ctx(&mut out, &mut timers, &mut deferred, |ctx| {
            view.reset_current(ctx)
        });

        let p = picker_mut(&mut view);
        with_ctx(&mut out, &mut timers, &mut deferred, |ctx| {
            highlight_active_by_label(p, "top", ctx)
        });
        press_and_settle(p, Key::Delete, &mut out, &mut timers, &mut deferred);

        let v = staged(&sh);
        assert!(
            !v.iter().any(|s| s.eq_ignore_ascii_case("top")),
            "highlighted unlocked 'top' must be removed, got {v:?}"
        );
        assert!(
            v.iter().any(|s| s.eq_ignore_ascii_case("person")),
            "locked 'person' must remain, got {v:?}"
        );
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
                p.shuttle_mut().expect("shuttle").selected().is_empty(),
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
        with_ctx(&mut out, &mut timers, &mut deferred, |ctx| {
            view.reset_current(ctx)
        });

        // The two active classes are now in the Active column — no keypress needed.
        let p = picker_mut(&mut view);
        assert!(p.seeded);
        assert_eq!(
            p.shuttle_mut().expect("shuttle").selected().len(),
            2,
            "top + person must be active after reset_current"
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

    /// Moving an (unlocked) active class away via Delete unticks it and restages.
    /// Ticking is done by moving between columns; the staged commit follows.
    #[test]
    fn move_out_removable_class_unticks_and_restages() {
        let sh = shared();
        let mut view = build_view(&sh, &["top", "person"]);
        let mut out = std::collections::VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<Deferred> = Vec::new();
        with_ctx(&mut out, &mut timers, &mut deferred, |ctx| {
            view.reset_current(ctx)
        });
        assert_eq!(staged(&sh).len(), 2);

        let p = picker_mut(&mut view);
        // `top` is ABSTRACT → unlocked. Highlight it in the Active column, Delete.
        with_ctx(&mut out, &mut timers, &mut deferred, |ctx| {
            highlight_active_by_label(p, "top", ctx)
        });
        press_and_settle(p, Key::Delete, &mut out, &mut timers, &mut deferred);

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
        with_ctx(&mut out, &mut timers, &mut deferred, |ctx| {
            view.reset_current(ctx)
        });

        let p = picker_mut(&mut view);
        // `person` is STRUCTURAL → locked. Highlight it in the Active column, Delete.
        with_ctx(&mut out, &mut timers, &mut deferred, |ctx| {
            highlight_active_by_label(p, "person", ctx)
        });
        press_and_settle(p, Key::Delete, &mut out, &mut timers, &mut deferred);

        assert!(
            staged(&sh).iter().any(|s| s.eq_ignore_ascii_case("person")),
            "structural 'person' must stay ticked — it is locked"
        );
        assert!(
            p.shuttle_mut()
                .expect("shuttle")
                .selected()
                .iter()
                .any(|r| r.label == "person"),
            "structural 'person' must remain in the Active column"
        );
    }

    /// Moving an available class toward Active (Insert) ticks it and restages.
    #[test]
    fn move_in_available_class_ticks_and_restages() {
        let sh = shared();
        // Only `person` active; organizationalPerson + top are available.
        let mut view = build_view(&sh, &["person"]);
        let mut out = std::collections::VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<Deferred> = Vec::new();
        with_ctx(&mut out, &mut timers, &mut deferred, |ctx| {
            view.reset_current(ctx)
        });
        assert_eq!(staged(&sh), vec!["person".to_string()]);

        let p = picker_mut(&mut view);
        // Available column = candidate-sorted minus active: [organizationalPerson,
        // top]. Highlight organizationalPerson and move it in with Insert.
        with_ctx(&mut out, &mut timers, &mut deferred, |ctx| {
            highlight_avail_by_label(p, "organizationalPerson", ctx)
        });
        press_and_settle(p, Key::Insert, &mut out, &mut timers, &mut deferred);

        let v = staged(&sh);
        assert!(
            v.iter()
                .any(|s| s.eq_ignore_ascii_case("organizationalPerson")),
            "moved-in class must be ticked, got {v:?}"
        );
        assert!(
            v.iter().any(|s| s.eq_ignore_ascii_case("person")),
            "person must remain ticked, got {v:?}"
        );
        // Canonical, candidate-sorted order is preserved in the staged commit.
        assert_eq!(
            v,
            vec!["organizationalPerson".to_string(), "person".to_string()]
        );
    }
}
