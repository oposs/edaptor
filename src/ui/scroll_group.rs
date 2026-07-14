//! A reusable vertical scroll container for child views.
//!
//! tvision-rs has no scroll-container for child widgets (Group has no child
//! offset; Scroller is for self-drawn content). `ScrollGroup` fills the gap: it
//! holds child views at stable *logical* positions and, on scroll, repositions
//! each child's bounds by `-top` — the framework clips children that fall outside
//! the group automatically (`DrawCtx::sub` intersects each child's clip with the
//! group region). A linked vertical `ScrollBar` lives in the right column and is
//! driven through the `ScrollSync` broker. Domain-free: candidate for upstreaming
//! to tvision-rs.

use tvision_rs::{
    self as tv, delegate, Context, Event, Point, Rect, Role, ScrollBar, View, ViewId,
};

pub(crate) struct ScrollGroup {
    group: tv::Group,
    /// Vertical scroll bar in the right column (wired in Task 3).
    v_bar: ViewId,
    /// (child id, logical rect) for repositionable content (excludes the bar).
    content: Vec<(ViewId, Rect)>,
    top: i32,
    inner_w: i32,
    viewport_h: i32,
    /// Set by `change_bounds` (no ctx available there) so that `handle_event`
    /// re-publishes the scroll-bar params on the very next event after a resize.
    bar_dirty: bool,
}

impl ScrollGroup {
    #[allow(dead_code)]
    pub(crate) fn new(bounds: Rect) -> Self {
        let w = bounds.b.x - bounds.a.x;
        let h = bounds.b.y - bounds.a.y;
        let mut group = tv::Group::new(bounds);
        // Paint the group's own backdrop (tvision 0.8 `Group::set_surface`):
        // bright when the pane is focused, receded when not. Replaces the
        // hand-rolled fill in the old `draw` override; the uncovered rows below
        // the content now come from the framework, keyed on the group's own focus.
        group.set_surface(Role::ListNormal, Role::ListInactive);
        // Vertical bar in the right column (width 1 ⇒ vertical). Local coords.
        let v_bar = group.insert(Box::new(ScrollBar::new(Rect::new(w - 1, 0, w, h))));
        ScrollGroup {
            group,
            v_bar,
            content: Vec::new(),
            top: 0,
            inner_w: (w - 1).max(0),
            viewport_h: h.max(0),
            bar_dirty: false,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn inner_width(&self) -> i32 {
        self.inner_w
    }

    #[allow(dead_code)]
    pub(crate) fn add_content(&mut self, view: Box<dyn View>, logical: Rect) -> ViewId {
        let id = self.group.insert(view);
        self.content.push((id, logical));
        self.reposition_one(id, logical);
        id
    }

    #[allow(dead_code)]
    pub(crate) fn child_mut(&mut self, id: ViewId) -> Option<&mut dyn View> {
        self.group.child_mut(id)
    }

    /// Update a content child's *logical* rect (its position in the un-scrolled
    /// content plane) and reposition it for the current scroll `top`. This is the
    /// hook a variable-height layout uses to re-place children after the initial
    /// `add_content`: it keeps `content_height`, `local_bounds_of`, scroll math and
    /// hit-testing consistent (a bare `child_mut(id).change_bounds(..)` would move
    /// the view but leave the stored logical rect stale). No-op for unknown ids.
    pub(crate) fn set_logical(&mut self, id: ViewId, logical: Rect) {
        if let Some(entry) = self.content.iter_mut().find(|(i, _)| *i == id) {
            entry.1 = logical;
        } else {
            return;
        }
        self.reposition_one(id, logical);
    }

    pub(crate) fn content_height(&self) -> i32 {
        self.content.iter().map(|(_, r)| r.b.y).max().unwrap_or(0)
    }

    pub(crate) fn max_top(&self) -> i32 {
        (self.content_height() - self.viewport_h).max(0)
    }

    fn reposition_one(&mut self, id: ViewId, logical: Rect) {
        let b = Rect::new(
            logical.a.x,
            logical.a.y - self.top,
            logical.b.x,
            logical.b.y - self.top,
        );
        if let Some(v) = self.group.child_mut(id) {
            v.change_bounds(b);
        }
    }

    fn reposition(&mut self) {
        let items: Vec<(ViewId, Rect)> = self.content.clone();
        for (id, logical) in items {
            self.reposition_one(id, logical);
        }
    }

    /// Test seam: set `top` and reposition without a Context (no bar republish).
    #[cfg(test)]
    pub(crate) fn set_top_for_test(&mut self, top: i32) {
        self.top = top.clamp(0, self.max_top());
        self.reposition();
    }

    #[cfg(test)]
    pub(crate) fn content_id_for_test(&self, idx: usize) -> ViewId {
        self.content[idx].0
    }

    #[cfg(test)]
    pub(crate) fn top_for_test(&self) -> i32 {
        self.top
    }

    #[allow(dead_code)]
    pub(crate) fn clear_content(&mut self, ctx: &mut Context) {
        let ids: Vec<ViewId> = self.content.iter().map(|(id, _)| *id).collect();
        for id in ids {
            self.group.remove(id, ctx);
        }
        self.content.clear();
        self.top = 0;
    }

    #[allow(dead_code)]
    pub(crate) fn current(&self) -> Option<ViewId> {
        self.group.current()
    }

    pub(crate) fn focus_child(&mut self, id: ViewId, ctx: &mut Context) {
        self.group.focus_child(id, ctx);
        // The bar tracks the cursor (focused row), so a focus move must re-publish
        // its value — this is also what gives the bar a thumb the moment the form
        // opens (`render` focuses the first field before any scroll happens).
        self.publish_bar(ctx);
    }

    /// The logical row of the focused content child — the value the scroll bar's
    /// thumb tracks (listbox semantics: the thumb follows the cursor, not the
    /// viewport offset). Falls back to the scroll `top` when nothing is focused.
    fn focused_row(&self) -> i32 {
        self.group
            .current()
            .and_then(|id| self.logical_of(id))
            .map(|r| r.a.y)
            .unwrap_or(self.top)
    }

    /// The focusable content child nearest logical `row` — the drag target when
    /// the bar's thumb is dragged. "Focusable" mirrors the framework's tab-order
    /// gate (visible, enabled, selectable), so a drag lands on a real field and
    /// never on a read-only label cell. `None` when no content child qualifies.
    fn focus_target_for_row(&mut self, row: i32) -> Option<ViewId> {
        let content = self.content.clone();
        content
            .into_iter()
            .filter(|(id, _)| {
                self.group
                    .child_mut(*id)
                    .map(|c| {
                        let s = c.state();
                        s.state.visible && !s.state.disabled && s.options.selectable
                    })
                    .unwrap_or(false)
            })
            .min_by_key(|(_, r)| (r.a.y - row).abs())
            .map(|(id, _)| id)
    }

    pub(crate) fn logical_of(&self, id: ViewId) -> Option<Rect> {
        self.content.iter().find(|(i, _)| *i == id).map(|(_, r)| *r)
    }

    /// The child's current bounds in this group's local (viewport) coordinates:
    /// its logical rect shifted up by the active scroll `top` — exactly the rect
    /// `reposition_one` assigns to the child. `None` if `id` is not a content
    /// child. Used for hit-testing a child cell against a (group-local) point.
    pub(crate) fn local_bounds_of(&self, id: ViewId) -> Option<Rect> {
        self.logical_of(id)
            .map(|r| Rect::new(r.a.x, r.a.y - self.top, r.b.x, r.b.y - self.top))
    }

    pub(crate) fn ensure_visible(&mut self, logical: Rect, ctx: &mut Context) {
        // A block taller than the viewport cannot fit whole: `logical.a.y < top`
        // (top clipped) and `logical.b.y > top + vh` (bottom clipped) are BOTH
        // true, so the naive top/bottom branches below flip-flop `top` between the
        // block's top and its bottom on every call — a visible ping-pong, since
        // the scroll-to-focused path runs this on every event. Settle instead:
        // once the block already spans the whole viewport, leave `top` alone; only
        // scroll when an edge is stranded, bringing the nearest edge into view.
        if logical.b.y - logical.a.y > self.viewport_h {
            if logical.a.y > self.top {
                self.scroll_to(logical.a.y, ctx);
            } else if logical.b.y < self.top + self.viewport_h {
                self.scroll_to(logical.b.y - self.viewport_h, ctx);
            }
            // else: block already covers the viewport → fixed point, no scroll.
            return;
        }
        if logical.a.y < self.top {
            self.scroll_to(logical.a.y, ctx);
        } else if logical.b.y > self.top + self.viewport_h {
            self.scroll_to(logical.b.y - self.viewport_h, ctx);
        }
    }

    /// A one-row logical rect at the focused child's hardware-cursor row, or
    /// `None` when the focused child exposes no visible cursor (e.g. a read-only
    /// launch block). `Group::cursor_request` returns the caret in viewport-local
    /// coordinates (child-view-local + child origin); adding `self.top` lifts it
    /// back into logical (content) space, matching `logical_of`.
    fn focused_cursor_row(&self, logical: Rect) -> Option<Rect> {
        let vp = self.group.cursor_request()?;
        let y = vp.y + self.top;
        Some(Rect::new(logical.a.x, y, logical.b.x, y + 1))
    }

    /// Scroll so the focused content child stays within the viewport. For a child
    /// TALLER than the viewport (an inline multi-value list block, e.g. a large
    /// group's `memberUid`) the whole rect can never fit, so track the child's
    /// hardware-cursor row instead — that keeps the caret on screen while editing
    /// rather than parking the block's top or bottom edge (which oscillates). Both
    /// the per-event scroll-to-focused and the form's post-edit relayout call this
    /// so the two never disagree on where to scroll.
    pub(crate) fn ensure_focused_visible(&mut self, ctx: &mut Context) {
        if let Some(cur) = self.group.current() {
            if let Some(logical) = self.logical_of(cur) {
                let target = if logical.b.y - logical.a.y > self.viewport_h {
                    self.focused_cursor_row(logical).unwrap_or(logical)
                } else {
                    logical
                };
                self.ensure_visible(target, ctx);
            }
        }
    }

    /// Move focus one viewport page up/down and scroll it into view. Mirrors the
    /// bar-drag path (`focus_target_for_row` → `focus_child` → `ensure_visible`):
    /// the target field is the focusable one nearest the focused row shifted by a
    /// page, so PageUp/PageDown behave like the tree/leaf lists' page keys but land
    /// on a real (editable) field. A no-op when the content fits the viewport (the
    /// caller still consumes the key).
    fn page(&mut self, down: bool, ctx: &mut Context) {
        if self.max_top() == 0 {
            return;
        }
        let step = self.viewport_h.max(1);
        let target_row = if down {
            self.focused_row() + step
        } else {
            self.focused_row() - step
        };
        if let Some(id) = self.focus_target_for_row(target_row) {
            self.focus_child(id, ctx);
            if let Some(logical) = self.logical_of(id) {
                self.ensure_visible(logical, ctx);
            }
        }
    }

    pub(crate) fn scroll_to(&mut self, top: i32, ctx: &mut Context) {
        let clamped = top.clamp(0, self.max_top());
        if clamped != self.top {
            self.top = clamped;
            self.reposition();
        }
        self.publish_bar(ctx);
    }

    /// The bar shows only when this pane is the focused one AND the content
    /// overflows the viewport — so scroll bars appear on the focused pane alone
    /// rather than on every pane at once. Keys on the group's own `focused` (true
    /// only down the current-child chain), so the owning pane no longer needs to
    /// mirror its focus onto this group's `active` flag.
    fn bar_should_show(&self) -> bool {
        self.max_top() > 0 && self.group.state().state.focused
    }

    /// Re-assert the bar's focus+overflow visibility. Called every event so a
    /// pane that loses focus hides its bar on the next tick (the 50ms pump
    /// reaches this view). `request_set_visible` competes in the deferred drain
    /// against any overflow-blind toggle, mirroring leaf/tree's `sync_scrollbar`.
    fn sync_bar_visibility(&mut self, ctx: &mut Context) {
        let show = self.bar_should_show();
        if let Some(b) = self.group.child_mut(self.v_bar) {
            b.state_mut().state.visible = show;
        }
        ctx.request_set_visible(self.v_bar, show);
    }

    /// Publish the bar's params with **listbox semantics**: the thumb tracks the
    /// focused row (the cursor), spanning `0..=content_height-1`, with a page step
    /// of one viewport. This mirrors `ListViewer`/`ListBox` (value = focused item,
    /// max = range-1) so the form's bar behaves like the tree/leaf panes' bars —
    /// and, because it is published on every focus move, the bar shows a thumb
    /// from the moment the form opens rather than only after the first scroll.
    fn publish_bar(&mut self, ctx: &mut Context) {
        let max = (self.content_height() - 1).max(0);
        let value = self.focused_row().clamp(0, max);
        let show = self.bar_should_show();
        if let Some(b) = self.group.child_mut(self.v_bar) {
            b.state_mut().state.visible = show;
        }
        ctx.request_scroll_bar_params(
            self.v_bar,
            Some(value),
            Some(0),
            Some(max),
            Some((self.viewport_h - 1).max(1)),
            Some(1),
        );
    }
}

#[delegate(to = group)]
impl View for ScrollGroup {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    /// Where the hardware cursor wants to sit, in this group's local frame. The
    /// inner group reports the focused child's cursor already shifted by the
    /// active scroll `top`; we suppress it when the focused field has been
    /// scrolled outside the viewport. Otherwise the event loop — which clamps
    /// only the low end of the cursor coordinate (`p.y.max(0)`), never the high
    /// end — would drive the terminal cursor below the pane and wedge the
    /// display. Hiding the cursor while the focused field is off-screen is the
    /// correct, freeze-proof behaviour.
    fn cursor_request(&self) -> Option<Point> {
        let p = self.group.cursor_request()?;
        if p.y < 0 || p.y >= self.viewport_h {
            None
        } else {
            Some(p)
        }
    }

    /// Override `change_bounds` so that a grow-mode resize (driven by
    /// `Group::change_bounds` in the owning FormPane) recomputes `inner_w`,
    /// `viewport_h`, repositions the v_bar and content children, and schedules a
    /// bar-params refresh on the next event (no `Context` is available here).
    ///
    /// The content cells and v_bar have default grow_mode (all-false), so the
    /// delegated `Group::change_bounds` would leave them untouched — we position
    /// them manually instead, then update the group bounds directly.
    fn change_bounds(&mut self, bounds: Rect) {
        // Update the group's own bounds without propagating to children (we
        // position them manually below).
        self.group.state_mut().set_bounds(bounds);
        let w = bounds.b.x - bounds.a.x;
        let h = bounds.b.y - bounds.a.y;
        self.inner_w = (w - 1).max(0);
        self.viewport_h = h.max(0);
        // Reposition the v_bar to the right column at the new height.
        if let Some(b) = self.group.child_mut(self.v_bar) {
            b.change_bounds(Rect::new(w - 1, 0, w, h));
        }
        // Clamp top to the new max_top and reposition all content children.
        self.top = self.top.clamp(0, self.max_top());
        self.reposition();
        // Bar params require a Context; defer the refresh to the next handle_event.
        self.bar_dirty = true;
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        // Flush a pending bar-params refresh deferred from change_bounds (which
        // has no Context).  Idempotent when bar_dirty is false.
        if self.bar_dirty {
            self.publish_bar(ctx);
            self.bar_dirty = false;
        }
        if let Event::Broadcast { command, source } = ev {
            if *command == tv::Command::SCROLL_BAR_CHANGED && *source == Some(self.v_bar) {
                if let Some(id) = self.group.state().id() {
                    ctx.request_scroll_sync(id, None, Some(self.v_bar));
                }
            }
        }
        // PageUp/PageDown page the form: the focused field's InputLine ignores them,
        // so intercept here and move focus one viewport up/down (like the list panes).
        if let Event::KeyDown(k) = ev {
            if matches!(k.key, tv::Key::PageDown | tv::Key::PageUp) {
                self.page(k.key == tv::Key::PageDown, ctx);
                ev.clear();
                self.sync_bar_visibility(ctx);
                return;
            }
        }
        self.group.handle_event(ev, ctx);
        // Scroll-to-focused: keep the focused content child (or, for an oversized
        // block, its caret row) within the viewport.
        self.ensure_focused_visible(ctx);
        // Hide the bar when this pane is not the active one (focus may have just
        // moved away without any scroll/resize to re-publish params).
        self.sync_bar_visibility(ctx);
    }

    fn apply_scroll_sync(&mut self, _h: Option<i32>, v: Option<i32>, ctx: &mut Context) {
        if let Some(v) = v {
            // The bar value is a row (cursor), not a viewport offset: a drag moves
            // focus to the field at that row (then `focus_child` republishes the
            // bar and `ensure_visible` scrolls it on screen), mirroring
            // `ListViewer::apply_scroll`. Falls back to a plain viewport scroll
            // only when no focusable field exists to land on.
            match self.focus_target_for_row(v) {
                Some(id) => {
                    self.focus_child(id, ctx);
                    if let Some(logical) = self.logical_of(id) {
                        self.ensure_visible(logical, ctx);
                    }
                }
                None => self.scroll_to(v, ctx),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tvision_rs::{self as tv, InputLine, Rect};

    fn cell(y: i32, w: i32) -> Box<dyn tv::View> {
        Box::new(InputLine::with_limit(Rect::new(0, y, w, y + 1), 64))
    }

    #[test]
    fn reposition_shifts_content_by_top() {
        let mut sg = ScrollGroup::new(Rect::new(0, 0, 20, 5)); // viewport height 5
        let w = sg.inner_width();
        // 8 logical rows (0..8), taller than the 5-row viewport.
        let mut ids = Vec::new();
        for y in 0..8 {
            ids.push(sg.add_content(cell(y, w), Rect::new(0, y, w, y + 1)));
        }
        // content height 8, viewport 5 → max_top 3
        assert_eq!(sg.content_height(), 8);
        assert_eq!(sg.max_top(), 3);
        // at top=0 the first child sits on its logical row
        assert_eq!(sg.child_mut(ids[0]).unwrap().state().get_bounds().a.y, 0);
        // scroll math is applied by reposition() through set_top (Task 2 wires ctx);
        // here exercise the pure helper:
        sg.set_top_for_test(2);
        assert_eq!(sg.child_mut(ids[0]).unwrap().state().get_bounds().a.y, -2);
        assert_eq!(sg.child_mut(ids[5]).unwrap().state().get_bounds().a.y, 3);
    }

    #[test]
    fn page_keys_move_focus_by_a_viewport() {
        use tvision_rs::Deferred;
        let mut sg = ScrollGroup::new(Rect::new(0, 0, 20, 5)); // viewport height 5
        let w = sg.inner_width();
        let mut ids = Vec::new();
        for y in 0..12 {
            ids.push(sg.add_content(cell(y, w), Rect::new(0, y, w, y + 1)));
        }
        let mut out = std::collections::VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<Deferred> = Vec::new();
        let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);

        // Focus the first field so paging has a reference row.
        sg.focus_child(ids[0], &mut ctx);
        assert_eq!(sg.focused_row(), 0);

        // PageDown pages focus ~one viewport down (5 rows).
        let mut pd = Event::KeyDown(tv::KeyEvent::from(tv::Key::PageDown));
        <ScrollGroup as tv::View>::handle_event(&mut sg, &mut pd, &mut ctx);
        let after_down = sg.focused_row();
        assert!(
            after_down >= 5,
            "PageDown pages focus down a viewport (got row {after_down})"
        );

        // PageUp pages it back up.
        let mut pu = Event::KeyDown(tv::KeyEvent::from(tv::Key::PageUp));
        <ScrollGroup as tv::View>::handle_event(&mut sg, &mut pu, &mut ctx);
        assert!(
            sg.focused_row() < after_down,
            "PageUp pages focus up from row {after_down}"
        );
    }

    #[test]
    fn local_bounds_of_tracks_scroll_top() {
        let mut sg = ScrollGroup::new(Rect::new(0, 0, 20, 5));
        let w = sg.inner_width();
        let mut ids = Vec::new();
        for y in 0..8 {
            ids.push(sg.add_content(cell(y, w), Rect::new(0, y, w, y + 1)));
        }
        // At top=0 the local bounds equal the logical rect.
        assert_eq!(sg.local_bounds_of(ids[3]), Some(Rect::new(0, 3, w, 4)));
        // After scrolling, the local bounds shift up by `top`.
        sg.set_top_for_test(2);
        assert_eq!(sg.local_bounds_of(ids[3]), Some(Rect::new(0, 1, w, 2)));
        // The scroll bar is not content → no local bounds.
        assert_eq!(sg.local_bounds_of(sg.v_bar), None);
    }

    #[test]
    fn inner_width_reserves_bar_lane() {
        let sg = ScrollGroup::new(Rect::new(0, 0, 20, 5));
        assert_eq!(sg.inner_width(), 19); // 20 - 1 bar column
    }

    #[test]
    fn scrolled_child_clips_to_viewport() {
        use tvision_rs::{Buffer, DrawCtx, FieldValue, Point, StaticText, Theme, View};
        // viewport 0..4 rows, content rows 0..8.
        let mut sg = ScrollGroup::new(Rect::new(0, 0, 10, 4));
        let w = sg.inner_width();
        for y in 0..8_i32 {
            // StaticText draws its text at the top-left of its bounds.
            let t = StaticText::new(Rect::new(0, y, w, y + 1), format!("R{y}"));
            sg.add_content(Box::new(t), Rect::new(0, y, w, y + 1));
        }
        // scroll so logical row 2 is at screen row 0; rows 0,1 go to negative y (clip).
        sg.set_top_for_test(2);

        let mut buf = Buffer::new(10, 4);
        let theme = Theme::classic_blue();
        {
            let mut ctx = DrawCtx::new(&mut buf, &theme, Rect::new(0, 0, 10, 4), Point::new(0, 0));
            // Drawing the group draws each child through ctx.sub(child_bounds), which
            // clips to the group region — so the negative-y rows must not appear.
            <ScrollGroup as View>::draw(&mut sg, &mut ctx);
        }
        // Screen row 0 shows logical row 2 ("R2"); rows above it (R0,R1) are clipped out.
        assert_eq!(
            buf.get(0, 0).symbol(),
            "R".chars().next().unwrap().to_string()
        );
        // The glyph at (1,0) is '2' (from "R2"), proving row 2 — not row 0 — is on top.
        assert_eq!(buf.get(1, 0).symbol(), "2");
        let _ = FieldValue::Int(0);
    }

    #[test]
    fn publish_bar_sets_params_and_hides_when_fits() {
        use tvision_rs::{Deferred, FieldValue};
        let mut sg = ScrollGroup::new(Rect::new(0, 0, 10, 5));
        let w = sg.inner_width();
        for y in 0..8 {
            sg.add_content(
                Box::new(tv::InputLine::with_limit(Rect::new(0, y, w, y + 1), 64)),
                Rect::new(0, y, w, y + 1),
            );
        }
        let mut out = std::collections::VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<Deferred> = Vec::new();
        let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);
        sg.scroll_to(2, &mut ctx);
        // The bar tracks the FOCUSED ROW (listbox semantics), not the viewport
        // top: max = content_height-1 = 7 (the last logical row). With no focused
        // child here the value falls back to the scroll top (2).
        let found = deferred.iter().any(|d| {
            matches!(
                d,
                Deferred::ScrollBarSetParams {
                    value: Some(2),
                    max: Some(7),
                    ..
                }
            )
        });
        assert!(
            found,
            "publish_bar must request value=2 max=7 (content rows-1)"
        );
        let _ = FieldValue::Int(0);
    }

    #[test]
    fn bar_value_tracks_focused_row_not_scroll_top() {
        use tvision_rs::Deferred;
        // 8 logical rows in a 4-row viewport (overflowing) on the active pane.
        let mut sg = ScrollGroup::new(Rect::new(0, 0, 10, 4));
        let w = sg.inner_width();
        let mut ids = Vec::new();
        for y in 0..8 {
            ids.push(sg.add_content(cell(y, w), Rect::new(0, y, w, y + 1)));
        }
        sg.group.state_mut().state.focused = true; // the bar only shows on the focused pane
        let mut out = std::collections::VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<Deferred> = Vec::new();
        let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);
        // Focusing a content child (the cursor) must publish the bar with
        // value = that child's logical row, NOT the viewport top — this is the
        // "thumb follows the cursor like a listbox" contract, and publishing on
        // focus is what gives the bar a thumb the moment the form opens.
        sg.focus_child(ids[5], &mut ctx);
        let found = deferred.iter().any(|d| {
            matches!(
                d,
                Deferred::ScrollBarSetParams {
                    value: Some(5),
                    min: Some(0),
                    max: Some(7),
                    ..
                }
            )
        });
        assert!(found, "focusing row 5 must publish bar value=5 max=7");
    }

    #[test]
    fn dragging_bar_moves_focus_to_that_row() {
        use tvision_rs::Deferred;
        // All rows are enabled InputLines → every row is a focus target.
        let mut sg = ScrollGroup::new(Rect::new(0, 0, 10, 4));
        let w = sg.inner_width();
        let mut ids = Vec::new();
        for y in 0..8 {
            ids.push(sg.add_content(cell(y, w), Rect::new(0, y, w, y + 1)));
        }
        sg.group.state_mut().state.focused = true;
        let mut out = std::collections::VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<Deferred> = Vec::new();
        let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);
        // Dragging the bar to value 6 (the read-sync delivers it via
        // apply_scroll_sync) must move focus to the field at that row, so the
        // thumb stays draggable instead of snapping back to the cursor.
        <ScrollGroup as View>::apply_scroll_sync(&mut sg, None, Some(6), &mut ctx);
        assert_eq!(
            sg.current(),
            Some(ids[6]),
            "dragging the bar to row 6 focuses that row's field"
        );
    }

    #[test]
    fn publish_bar_hides_bar_when_content_fits() {
        use tvision_rs::Deferred;
        // 2 content rows in a 5-row viewport → content fits → bar must be HIDDEN.
        let mut sg = ScrollGroup::new(Rect::new(0, 0, 10, 5));
        let w = sg.inner_width();
        for y in 0..2 {
            sg.add_content(
                Box::new(tv::InputLine::with_limit(Rect::new(0, y, w, y + 1), 64)),
                Rect::new(0, y, w, y + 1),
            );
        }
        assert_eq!(sg.max_top(), 0, "content fits → max_top must be 0");
        let mut out = std::collections::VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<Deferred> = Vec::new();
        let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);
        sg.scroll_to(0, &mut ctx);
        let visible = sg.group.child_mut(sg.v_bar).unwrap().state().state.visible;
        assert!(
            !visible,
            "scroll bar must be hidden when content fits in the viewport"
        );
    }

    #[test]
    fn ensure_visible_scrolls_offscreen_child_into_view() {
        let mut sg = ScrollGroup::new(Rect::new(0, 0, 10, 4)); // viewport rows 0..4
        let w = sg.inner_width();
        for y in 0..8 {
            sg.add_content(
                Box::new(tv::InputLine::with_limit(Rect::new(0, y, w, y + 1), 64)),
                Rect::new(0, y, w, y + 1),
            );
        }
        let mut out = std::collections::VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);
        // Row 6 is below the viewport (top=0 shows 0..4). ensure_visible scrolls so
        // its bottom (7) is the viewport bottom → top = 7 - 4 = 3.
        sg.ensure_visible(Rect::new(0, 6, w, 7), &mut ctx);
        let id = sg.content_id_for_test(6);
        let y = sg.child_mut(id).unwrap().state().get_bounds().a.y;
        assert!(
            (0..4).contains(&y),
            "row 6 must be inside the viewport, got y={y}"
        );
    }

    #[test]
    fn ensure_visible_settles_for_block_taller_than_the_viewport() {
        // Regression: a focused block taller than the viewport must not
        // oscillate. The form's inline multi-value list (e.g. a large group's
        // memberUid) is ONE tall child; the scroll-to-focused path runs
        // ensure_visible on every event (incl. the 50ms pump), and the old
        // top/bottom branches flip-flopped `top` between the block's top and its
        // bottom on successive calls — a visible on-screen ping-pong.
        let mut sg = ScrollGroup::new(Rect::new(0, 0, 10, 5)); // viewport rows 0..5
        let w = sg.inner_width();
        for y in 0..30 {
            sg.add_content(
                Box::new(tv::InputLine::with_limit(Rect::new(0, y, w, y + 1), 64)),
                Rect::new(0, y, w, y + 1),
            );
        }
        let mut out = std::collections::VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);
        // A single 20-row block (rows 3..23) that dwarfs the 5-row viewport.
        let block = Rect::new(0, 3, w, 23);
        sg.ensure_visible(block, &mut ctx);
        let first = sg.top_for_test();
        sg.ensure_visible(block, &mut ctx);
        let second = sg.top_for_test();
        assert_eq!(
            first, second,
            "scroll must settle for an oversized block, not ping-pong \
             (got top={first} then top={second})"
        );
    }

    #[test]
    fn backdrop_fill_covers_uncovered_rows() {
        use tvision_rs::{Buffer, DrawCtx, Point, StaticText, Theme, View};
        // 2 content rows in a 6-row viewport → rows 2-5 are uncovered.
        let mut sg = ScrollGroup::new(Rect::new(0, 0, 10, 6));
        let w = sg.inner_width();
        for y in 0..2_i32 {
            let t = StaticText::new(Rect::new(0, y, w, y + 1), format!("R{y}"));
            sg.add_content(Box::new(t), Rect::new(0, y, w, y + 1));
        }

        let mut buf = Buffer::new(10, 6);
        let theme = Theme::classic_blue();
        {
            let mut ctx = DrawCtx::new(&mut buf, &theme, Rect::new(0, 0, 10, 6), Point::new(0, 0));
            <ScrollGroup as View>::draw(&mut sg, &mut ctx);
        }
        // Row 3 (below the 2 content rows) must be blank space, not a desktop glyph.
        let sym = buf.get(0, 3).symbol();
        assert_eq!(
            sym, " ",
            "uncovered row must be blank space from backdrop fill, got {sym:?}"
        );
    }

    /// `change_bounds` must recompute `inner_w`, `viewport_h`, reposition the
    /// v_bar, and clamp/reposition content — all without a Context.
    ///
    /// This exercises the real framework resize path (Group::change_bounds →
    /// child.change_bounds) rather than the dead `on_bounds_changed` hook.
    #[test]
    fn change_bounds_updates_geometry_and_repositions() {
        let mut sg = ScrollGroup::new(Rect::new(0, 0, 20, 5));
        let old_w = sg.inner_width();
        // Add 8 content rows so there is something to scroll.
        for y in 0..8 {
            sg.add_content(cell(y, old_w), Rect::new(0, y, old_w, y + 1));
        }
        // Resize to a larger rect (mimics the Splitter driving the FormPane Group).
        <ScrollGroup as tv::View>::change_bounds(&mut sg, Rect::new(0, 0, 30, 10));

        // inner_w and viewport_h must reflect the new size.
        assert_eq!(sg.inner_width(), 29, "inner_w = w-1 = 29");
        assert_eq!(sg.viewport_h, 10, "viewport_h = h = 10");

        // v_bar must now occupy the new right column at the new height.
        let bar_bounds = sg.group.child_mut(sg.v_bar).unwrap().state().get_bounds();
        assert_eq!(
            bar_bounds,
            Rect::new(29, 0, 30, 10),
            "v_bar must sit in column 29 spanning new height 10"
        );

        // content height 8 ≤ new viewport_h 10 → max_top is now 0; top clamped.
        assert_eq!(sg.max_top(), 0, "all content fits → max_top 0");
        assert_eq!(sg.top, 0, "top clamped to new max_top");

        // First content child must still be at screen row 0 (top=0).
        let id0 = sg.content_id_for_test(0);
        assert_eq!(
            sg.child_mut(id0).unwrap().state().get_bounds().a.y,
            0,
            "first content row at screen y=0"
        );
    }

    #[test]
    fn bar_hidden_when_pane_inactive_even_if_overflowing() {
        let mut sg = ScrollGroup::new(Rect::new(0, 0, 10, 4)); // viewport height 4
        let w = sg.inner_width();
        for y in 0..8 {
            sg.add_content(
                Box::new(tv::InputLine::with_limit(Rect::new(0, y, w, y + 1), 64)),
                Rect::new(0, y, w, y + 1),
            );
        }
        assert!(sg.max_top() > 0, "content must overflow the viewport");
        // Inactive pane → bar hidden despite overflow (scroll bars live on the
        // active pane only).
        sg.group.state_mut().state.focused = false;
        assert!(!sg.bar_should_show(), "inactive pane must hide its bar");
        // Active pane → bar shown.
        sg.group.state_mut().state.focused = true;
        assert!(
            sg.bar_should_show(),
            "active overflowing pane shows its bar"
        );
    }

    #[test]
    fn cursor_suppressed_when_focused_field_scrolled_offscreen() {
        let mut sg = ScrollGroup::new(Rect::new(0, 0, 10, 4)); // viewport rows 0..4
        let w = sg.inner_width();
        let mut ids = Vec::new();
        for y in 0..8 {
            ids.push(sg.add_content(
                Box::new(tv::InputLine::with_limit(Rect::new(0, y, w, y + 1), 64)),
                Rect::new(0, y, w, y + 1),
            ));
        }
        let mut out = std::collections::VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);
        sg.group.state_mut().state.focused = true;
        sg.focus_child(ids[7], &mut ctx); // focus the last (row 7) field
                                          // Make the focused field's cursor visible so group.cursor_request()
                                          // surfaces a point — otherwise the viewport gate is never exercised.
        if let Some(c) = sg.group.child_mut(ids[7]) {
            let st = c.state_mut();
            st.cursor = Point::new(0, 0);
            st.state.cursor_vis = true;
            st.state.focused = true; // the group isn't focused headlessly; force it
        }
        // top=0 shows rows 0..4, so row 7 is below the viewport → cursor hidden,
        // never driven off the pane.
        assert_eq!(
            <ScrollGroup as View>::cursor_request(&sg),
            None,
            "off-screen focused field must not request a cursor"
        );
        // Scroll so row 7 lands inside the viewport → the cursor surfaces again.
        sg.set_top_for_test(4);
        assert!(
            <ScrollGroup as View>::cursor_request(&sg).is_some(),
            "on-screen focused field must request a cursor"
        );
    }

    /// A focusable stub whose hardware cursor sits on a caller-chosen row — an
    /// InputLine is single-line and snaps its cursor back to row 0, so it can't
    /// stand in for a multi-row inline block.
    struct CursorCell {
        state: tv::ViewState,
    }
    impl tv::View for CursorCell {
        fn state(&self) -> &tv::ViewState {
            &self.state
        }
        fn state_mut(&mut self) -> &mut tv::ViewState {
            &mut self.state
        }
        fn draw(&mut self, _ctx: &mut tv::DrawCtx) {}
    }

    #[test]
    fn scroll_to_focused_tracks_the_caret_row_of_an_oversized_block() {
        // A focused block taller than the viewport must scroll to the CARET row,
        // not park the block's top or bottom edge — otherwise the caret can sit
        // off-screen and editing (e.g. a large group's memberUid) is impossible.
        let mut sg = ScrollGroup::new(Rect::new(0, 0, 10, 5)); // viewport rows 0..5
        let w = sg.inner_width();
        // ONE 20-row block, caret parked on view-local row 15.
        let mut state = tv::ViewState::new(Rect::new(0, 0, w, 20));
        state.options.selectable = true;
        state.show_cursor();
        state.set_cursor(0, 15);
        let id = sg.add_content(Box::new(CursorCell { state }), Rect::new(0, 0, w, 20));
        let mut out = std::collections::VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);
        sg.group.state_mut().state.focused = true;
        sg.focus_child(id, &mut ctx);
        // The group isn't focused headlessly; force the child's focus so its
        // cursor_request() surfaces the caret.
        if let Some(c) = sg.group.child_mut(id) {
            c.state_mut().state.focused = true;
        }
        // A benign broadcast drives the scroll-to-focused tail of handle_event
        // without disturbing the stub's cursor.
        let mut ev = Event::Broadcast {
            command: tv::Command::custom("edaptor.test.noop"),
            source: None,
        };
        <ScrollGroup as View>::handle_event(&mut sg, &mut ev, &mut ctx);
        let top = sg.top_for_test();
        assert!(
            (top..top + 5).contains(&15),
            "caret row 15 must be inside the viewport rows {top}..{}",
            top + 5
        );
    }

    #[test]
    fn apply_scroll_sync_focuses_dragged_row_and_scrolls_it_into_view() {
        use tvision_rs::Deferred;
        let mut sg = ScrollGroup::new(Rect::new(0, 0, 10, 5)); // viewport rows 0..5
        let w = sg.inner_width();
        let mut ids = Vec::new();
        for y in 0..8 {
            ids.push(sg.add_content(cell(y, w), Rect::new(0, y, w, y + 1)));
        }
        let mut out = std::collections::VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<Deferred> = Vec::new();
        let mut ctx = Context::new(&mut out, &mut timers, 0, &mut deferred);
        // Dragging the bar to row 7 (below the viewport) moves focus there and
        // scrolls so the focused field is visible — the bar is a cursor, not a
        // viewport offset.
        <ScrollGroup as View>::apply_scroll_sync(&mut sg, None, Some(7), &mut ctx);
        assert_eq!(sg.current(), Some(ids[7]), "drag focuses the row-7 field");
        let y = sg.child_mut(ids[7]).unwrap().state().get_bounds().a.y;
        assert!(
            (0..5).contains(&y),
            "row 7 must be scrolled into the viewport, got y={y}"
        );
    }
}
