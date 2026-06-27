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

use tvision_rs::{self as tv, Context, Rect, ScrollBar, View, ViewId};

// Tasks 2-6 wire the remaining methods and construct this struct.
// `pub(crate)` visibility suppresses dead_code in callers once wired; until
// then we suppress explicitly so `-D warnings` stays clean across all tasks.
#[allow(dead_code)]
pub(crate) struct ScrollGroup {
    group: tv::Group,
    /// Vertical scroll bar in the right column (wired in Task 3).
    #[allow(dead_code)]
    v_bar: ViewId,
    /// (child id, logical rect) for repositionable content (excludes the bar).
    content: Vec<(ViewId, Rect)>,
    top: i32,
    inner_w: i32,
    viewport_h: i32,
}

#[allow(dead_code)]
impl ScrollGroup {
    pub(crate) fn new(bounds: Rect) -> Self {
        let w = bounds.b.x - bounds.a.x;
        let h = bounds.b.y - bounds.a.y;
        let mut group = tv::Group::new(bounds);
        // Vertical bar in the right column (width 1 ⇒ vertical). Local coords.
        let v_bar = group.insert(Box::new(ScrollBar::new(Rect::new(w - 1, 0, w, h))));
        ScrollGroup {
            group,
            v_bar,
            content: Vec::new(),
            top: 0,
            inner_w: (w - 1).max(0),
            viewport_h: h.max(0),
        }
    }

    pub(crate) fn inner_width(&self) -> i32 {
        self.inner_w
    }

    pub(crate) fn add_content(&mut self, view: Box<dyn View>, logical: Rect) -> ViewId {
        let id = self.group.insert(view);
        self.content.push((id, logical));
        self.reposition_one(id, logical);
        id
    }

    pub(crate) fn child_mut(&mut self, id: ViewId) -> Option<&mut dyn View> {
        self.group.child_mut(id)
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

    pub(crate) fn clear_content(&mut self, ctx: &mut Context) {
        let ids: Vec<ViewId> = self.content.iter().map(|(id, _)| *id).collect();
        for id in ids {
            self.group.remove(id, ctx);
        }
        self.content.clear();
        self.top = 0;
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
    fn inner_width_reserves_bar_lane() {
        let sg = ScrollGroup::new(Rect::new(0, 0, 20, 5));
        assert_eq!(sg.inner_width(), 19); // 20 - 1 bar column
    }
}
