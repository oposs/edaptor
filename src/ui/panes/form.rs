//! Editable entry form pane: a header row (DN + dirty marker) over per-field rows,
//! each a static label column + a value `InputLine`. Plain single-value fields are
//! editable; the rest stay disabled (read-only). On every event the editable
//! `InputLine`s are synced into the shared `EditForm` so a `SAVE` sees current
//! values, and the header's dirty marker is refreshed.
//!
//! The form is built on a `ScrollGroup` (rows 1..h) so arbitrarily many fields are
//! supported — there is no longer a fixed `FORM_ROWS` cap. Cells are rebuilt from
//! `EditForm.fields` whenever the shown entry changes.

use tvision_rs::{
    self as tv, delegate, Context, DrawCtx, Event, FieldValue, Group, HelpCtx, InputLine, Key,
    Point, Rect, Role, View,
};
use unicode_width::UnicodeWidthStr;

use crate::ui::help_ctx::{
    FIELD_LAUNCH_PASSWORD, FIELD_LAUNCH_PICKER, FIELD_LIST, FIELD_LIST_HANDLE, FIELD_LIST_ORDERED,
    FIELD_TEXT,
};
use crate::ui::panes::field_label::FieldLabel;
use crate::ui::panes::launch_view::LaunchValueView;
use crate::ui::panes::list_view::ListValueView;
use crate::ui::panes::value_lines::{bullet_lines, masked_line, NOT_SET};
use crate::ui::scroll_group::ScrollGroup;
use crate::ui::widget::{inline_editable, present_field};
use crate::ui::{Shared, ACTIVATE, REFRESH};
use crate::workflows::edit_form::{composed_create_dn, EditField, FormMode};

/// Smallest / largest width the label column is allowed to take. It is sized to
/// fit the longest label (so every field name shows in full) but never eats more
/// than half the pane (see `label_col_width`).
const LABEL_MIN: i32 = 6;
const LABEL_MAX: i32 = 30;
/// One column of breathing room kept after the right-aligned label text.
const LABEL_GAP: i32 = 1;

/// Width for the label column given the longest label (display columns) and the
/// available inner width: fit the longest label plus a gap, but clamp so it stays
/// within `[LABEL_MIN, min(LABEL_MAX, w/2)]` — the value editor always keeps at
/// least half the pane.
fn label_col_width(longest: i32, w: i32) -> i32 {
    let cap = (w / 2).clamp(LABEL_MIN, LABEL_MAX);
    (longest + LABEL_GAP).clamp(LABEL_MIN, cap)
}

/// Short, human-readable hints for common but cryptic LDAP attribute names,
/// shown in parentheses after the attribute in the form's label column (e.g.
/// `sn (surname)`). Keyed by the lower-cased attribute name.
///
/// Deliberately curated and terse: only abbreviations whose meaning is not
/// obvious get an entry — self-explanatory names (`description`, `title`,
/// `homeDirectory`, …) are left bare. Schema `DESC` text is *not* used; it is
/// typically a full sentence, far too long for the label column. Extend this
/// table to cover more attributes.
const ATTR_HINTS: &[(&str, &str)] = &[
    ("cn", "common name"),
    ("sn", "surname"),
    ("gn", "given name"),
    ("c", "country"),
    ("l", "location"),
    ("st", "state"),
    ("o", "organization"),
    ("ou", "org. unit"),
    ("dc", "domain component"),
    ("uid", "login name"),
    ("gecos", "full name"),
    ("mail", "email"),
];

/// The hint for `attr` from [`ATTR_HINTS`], if any (case-insensitive).
fn attr_hint(attr: &str) -> Option<&'static str> {
    let key = attr.to_ascii_lowercase();
    ATTR_HINTS
        .iter()
        .find(|(a, _)| *a == key)
        .map(|(_, hint)| *hint)
}

/// The label shown in the form's label column for `attr`: the attribute name,
/// plus a parenthesised hint when [`ATTR_HINTS`] has one (e.g. `sn (surname)`).
/// The `*` MUST marker is NOT included here — callers append it.
fn display_label(attr: &str) -> String {
    match attr_hint(attr) {
        Some(hint) => format!("{attr} ({hint})"),
        None => attr.to_string(),
    }
}

/// A field's value cell is focusable when the user can interact with it:
/// - `Text` inline-editable fields (single-value free text);
/// - `List` inline-editor fields (multi-value, plain or XOrdered);
/// - `Launch` modal-activated fields (objectClass, password, picker, …).
fn cell_focusable(f: &EditField) -> bool {
    matches!(value_kind(f), ValueKind::List { .. } | ValueKind::Launch) || inline_editable(f)
}

/// Which value-view a field renders as in the form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueKind {
    Text,
    List { ordered: bool },
    Launch,
}

fn value_kind(f: &EditField) -> ValueKind {
    use crate::config::widget::WidgetKind;
    // objectClass always gets a modal picker — check it first before the multi path.
    // This mirrors the widget_for / is_modal_field priority in widget.rs.
    if f.label.eq_ignore_ascii_case("objectClass") {
        return ValueKind::Launch;
    }
    if matches!(f.widget_binding, Some(WidgetKind::XOrdered)) {
        return ValueKind::List { ordered: true };
    }
    if f.editable && f.multi && !f.orphaned && f.widget_binding.is_none() {
        return ValueKind::List { ordered: false };
    }
    if crate::ui::widget::is_modal_field(f) {
        // Remaining modal fields (Password/Choice/Picker/SambaSid) launch.
        return ValueKind::Launch;
    }
    ValueKind::Text
}

/// Display rows a field occupies. `Text` is always one row; list/launch blocks
/// grow with their values, and an empty value set collapses to the single
/// `<not set>` row.
fn block_height(f: &EditField, kind: ValueKind) -> i32 {
    match kind {
        ValueKind::Text => 1,
        ValueKind::List { .. } | ValueKind::Launch => {
            let non_empty: Vec<&String> =
                f.values.iter().filter(|v| !v.trim().is_empty()).collect();
            if non_empty.is_empty() {
                return 1; // the `<not set>` line
            }
            non_empty.iter().map(|v| v.split('\n').count() as i32).sum()
        }
    }
}

/// The StatusLine help context for a field's value view.
/// Returns one of the `FIELD_*` constants from `crate::ui::help_ctx`; the status
/// line's hint mapping references the same constants, so a rename can never
/// silently break the hint display.
fn help_ctx_for(kind: ValueKind, field: &EditField) -> HelpCtx {
    match kind {
        ValueKind::Text => FIELD_TEXT,
        ValueKind::List { ordered: false } => FIELD_LIST,
        ValueKind::List { ordered: true } => FIELD_LIST_ORDERED,
        ValueKind::Launch => {
            if field.secret {
                FIELD_LAUNCH_PASSWORD
            } else {
                FIELD_LAUNCH_PICKER
            }
        }
    }
}

/// The display lines for a `Launch`-kind value block:
/// * secret fields → a single masked line;
/// * multi-value fields → a bulleted list (ordering prefix stripped for XOrdered);
/// * single-value fields → one line with the presented value (no bullet).
fn launch_lines(field: &EditField) -> Vec<String> {
    use crate::config::widget::WidgetKind;
    if field.secret {
        return masked_line();
    }
    if field.multi {
        let ordered = matches!(field.widget_binding, Some(WidgetKind::XOrdered));
        return bullet_lines(&field.values, ordered);
    }
    let presented = present_field(field);
    if presented.trim().is_empty() {
        vec![NOT_SET.to_string()]
    } else {
        vec![presented]
    }
}

pub(crate) struct FormPane {
    /// Outer container: header row 0 (`dn` label + DN value) + ScrollGroup (1..h).
    group: Group,
    /// The `dn` caption in the label column of the header row.
    header_label_id: tv::ViewId,
    /// The DN value (styled as a title) in the value column of the header row.
    header_value_id: tv::ViewId,
    scroll_id: tv::ViewId,
    /// One value `InputLine` id per field, in field order (full-length; no cap).
    value_ids: Vec<tv::ViewId>,
    /// One label `FieldLabel` id per field, parallel to `value_ids`.
    label_ids: Vec<tv::ViewId>,
    /// The display label text per field (attr name + schema `DESC` hint), parallel
    /// to `value_ids` and WITHOUT the `*` MUST marker. Computed once per rebuild
    /// (schema lookups) and reused by `render` to feed each `FieldLabel`.
    labels: Vec<String>,
    /// The value-view kind of each field, parallel to `value_ids`. Drives how
    /// `render` feeds the view and how navigation/activation treat the field.
    kinds: Vec<ValueKind>,
    /// The height of each field block at the last layout, parallel to `value_ids`.
    /// Compared against freshly computed heights in `render` to relayout only when
    /// a block's line count changed.
    block_heights: Vec<i32>,
    /// DN of the entry whose cells are currently built; `None` before first render.
    built_dn: Option<String>,
    /// Width of the label column at the last rebuild (fit to the longest label).
    label_w: i32,
    /// Inner content width at the last rebuild; a change (splitter drag) triggers
    /// a rebuild so the value editors reflow to fill the new width.
    built_w: i32,
    /// Whether the pane held focus at the previous event, so we can detect the
    /// moment focus enters the pane (Tab from another pane) and home the caret —
    /// the framework select-alls the field on focus entry, which we undo.
    was_focused: bool,
    state: Shared,
}

impl FormPane {
    pub(crate) fn new(bounds: Rect, state: Shared) -> Self {
        let mut group = Group::new(bounds);
        // ofFirstClick: a single click into this pane (from another pane) both
        // focuses the pane and lands on the clicked field, rather than needing a
        // second click.
        group.state_mut().options.first_click = true;
        // The pane paints its own background (tvision 0.8 `Group::set_surface`):
        // bright when the pane is focused, receded to the desktop tone when not —
        // in lock-step with the list/tree panes. Replaces the hand-rolled fill.
        group.set_surface(Role::ListNormal, Role::ListInactive);
        let w = bounds.b.x - bounds.a.x;
        let h = bounds.b.y - bounds.a.y;

        // Row 0: the header, laid out like a field row — a `dn` label in the
        // label column and the DN value (styled as a title) in the value column.
        // Both are repositioned to the real label width on the first rebuild.
        let header_label = FieldLabel::label(Rect::new(0, 0, LABEL_MIN, 1));
        let header_label_id = group.insert(Box::new(header_label));
        let mut header_value = FieldLabel::title(Rect::new(LABEL_MIN, 0, w, 1));
        header_value.state.grow_mode.hi_x = true;
        let header_value_id = group.insert(Box::new(header_value));
        // Rows 1..h: scrollable content pane. grow_mode hi_x+hi_y so it fills the pane.
        let mut sg = ScrollGroup::new(Rect::new(0, 1, w, h));
        sg.state_mut().grow_mode.hi_x = true;
        sg.state_mut().grow_mode.hi_y = true;
        let scroll_id = group.insert(Box::new(sg));

        FormPane {
            group,
            header_label_id,
            header_value_id,
            scroll_id,
            value_ids: Vec::new(),
            label_ids: Vec::new(),
            labels: Vec::new(),
            kinds: Vec::new(),
            block_heights: Vec::new(),
            built_dn: None,
            label_w: LABEL_MIN,
            built_w: 0,
            was_focused: false,
            state,
        }
    }

    /// Return a mutable reference to the inner `ScrollGroup`.
    fn scroll_mut(&mut self) -> Option<&mut ScrollGroup> {
        self.group
            .child_mut(self.scroll_id)
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<ScrollGroup>())
    }

    /// Test seam: number of value InputLine cells currently built (one per field,
    /// uncapped after the ScrollGroup rewrite).
    #[cfg(test)]
    pub(crate) fn field_cell_count(&self) -> usize {
        self.value_ids.len()
    }

    /// Test seam: is the value InputLine for field `i` disabled?
    #[cfg(test)]
    pub(crate) fn value_disabled(&mut self, i: usize) -> bool {
        let vid = self.value_ids[i];
        self.scroll_mut()
            .and_then(|sg| sg.child_mut(vid))
            .map(|c| c.state().state.disabled)
            .unwrap_or(true)
    }

    /// Test seam: set the value InputLine text for field `i`.
    #[cfg(test)]
    pub(crate) fn set_value_text(&mut self, i: usize, text: String) {
        let vid = self.value_ids[i];
        if let Some(sg) = self.scroll_mut() {
            if let Some(c) = sg.child_mut(vid) {
                c.set_value(FieldValue::Text(text));
            }
        }
    }

    /// Test seam: current bounds of the header value (title) cell.
    #[cfg(test)]
    pub(crate) fn header_bounds_for_test(&mut self) -> Rect {
        self.group
            .child_mut(self.header_value_id)
            .unwrap()
            .state()
            .get_bounds()
    }

    /// Test seam: current bounds of the ScrollGroup child.
    #[cfg(test)]
    pub(crate) fn scroll_bounds_for_test(&mut self) -> Rect {
        self.group
            .child_mut(self.scroll_id)
            .unwrap()
            .state()
            .get_bounds()
    }

    /// Test seam: the header string the pane would currently render.
    #[cfg(test)]
    pub(crate) fn header_text_for_test(&self) -> String {
        self.header_text()
    }

    /// Test seam: the first display line of field `i`'s LaunchValueView.
    #[cfg(test)]
    pub(crate) fn launch_line_for_test(&mut self, i: usize) -> String {
        let vid = self.value_ids[i];
        self.scroll_mut()
            .and_then(|sg| sg.child_mut(vid))
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<LaunchValueView>())
            .and_then(|lv| lv.first_line_for_test())
            .unwrap_or_default()
    }

    /// Compose the header string from the current form state.
    /// For `Edit` mode: `"<dn><mark>"`.
    /// For `Create` mode: `"<composed_dn> (new)<mark>"` where the RDN value is
    /// read live from the field whose label matches the profile's `rdn_attr`.
    /// Returns an empty string when no form is loaded.
    /// Borrow discipline: takes a single short `state.borrow()` that is dropped
    /// before this method returns — never holds a borrow across a view call.
    fn header_text(&self) -> String {
        let st = self.state.borrow();
        let Some(form) = st.edit_form.as_ref() else {
            return String::new();
        };
        let mark = if form.is_dirty() { " *" } else { "" };
        match &form.mode {
            FormMode::Edit => format!("{}{}", form.dn, mark),
            FormMode::Create {
                profile_idx,
                container,
            } => {
                let rdn_attr = st
                    .profiles
                    .get(*profile_idx)
                    .map(|p| p.rdn_attr.as_str())
                    .unwrap_or("");
                let rdn_value = form
                    .fields
                    .iter()
                    .find(|f| f.label.eq_ignore_ascii_case(rdn_attr))
                    .and_then(|f| f.values.first())
                    .map(String::as_str)
                    .unwrap_or("");
                let composed = composed_create_dn(rdn_attr, rdn_value, container);
                format!("{} (new){}", composed, mark)
            }
        }
    }

    /// Position every field block: the label on the block's first row (in the
    /// right-aligned label column) and the value view spanning the whole block
    /// height. Updates each child's *logical* rect via `ScrollGroup::set_logical`
    /// so content height / hit-testing / scroll math stay consistent. Records the
    /// per-field heights and returns the total content height.
    fn layout_blocks(&mut self, label_w: i32, inner_w: i32, heights: &[i32]) -> i32 {
        let mut y = 0;
        let (lids, vids, kinds) = (
            self.label_ids.clone(),
            self.value_ids.clone(),
            self.kinds.clone(),
        );
        if let Some(sg) = self.scroll_mut() {
            for (i, &h) in heights.iter().enumerate() {
                if let Some(&lid) = lids.get(i) {
                    sg.set_logical(lid, Rect::new(0, y, label_w, y + 1));
                }
                if let Some(&vid) = vids.get(i) {
                    // Editable text fields render as an `InputLine`, which insets its
                    // content one column (the scroll-arrow/cursor gutter). Start the
                    // cell one column left so the value text lands at the value-column
                    // origin, flush with the read-only value views (DN title, list,
                    // launch) that draw from column 0.
                    let vx = match kinds.get(i) {
                        Some(ValueKind::Text) => (label_w - 1).max(0),
                        _ => label_w,
                    };
                    sg.set_logical(vid, Rect::new(vx, y, inner_w, y + h));
                }
                y += h;
            }
        }
        self.block_heights = heights.to_vec();
        y
    }

    /// Rebuild one label+value block per field into the `ScrollGroup`. Called when
    /// the shown entry changes (different `dn`), the field set resizes, or the pane
    /// is resized. Single-value text fields get an editable `InputLine`; multi-value
    /// `List` fields get an inline `ListValueView`; the modal-launch fields get a
    /// read-only `LaunchValueView` bullet block.
    /// Borrow discipline: clone the fields, drop the state borrow, then mutate the
    /// scroll group.
    fn rebuild_cells(&mut self, ctx: &mut Context) {
        // Clone the fields so classification/height/help-ctx can run outside the
        // state borrow (drop it before touching views).
        let (fields, labels): (Vec<EditField>, Vec<String>) = {
            let st = self.state.borrow();
            match st.edit_form.as_ref() {
                None => (Vec::new(), Vec::new()),
                Some(form) => {
                    // Resolve each field's display label (attr + curated hint);
                    // reused by `render`.
                    let labels = form
                        .fields
                        .iter()
                        .map(|f| display_label(&f.label))
                        .collect();
                    (form.fields.clone(), labels)
                }
            }
        }; // state borrow dropped

        // Classify each field and compute its block height.
        let kinds: Vec<ValueKind> = fields.iter().map(value_kind).collect();
        let heights: Vec<i32> = fields
            .iter()
            .zip(&kinds)
            .map(|(f, k)| block_height(f, *k))
            .collect();

        // Build all cells. Accumulate IDs into locals so the `sg` borrow (from
        // `self.group`) does not overlap with writing `self.label_ids`/`self.value_ids`.
        let mut new_lids: Vec<tv::ViewId> = Vec::with_capacity(fields.len());
        let mut new_vids: Vec<tv::ViewId> = Vec::with_capacity(fields.len());
        let label_w;
        let inner_w;
        {
            let Some(sg) = self.scroll_mut() else { return };
            sg.clear_content(ctx);
            let w = sg.inner_width();
            inner_w = w;
            // Size the label column to the longest label so every field name shows
            // in full, right-aligned; the value editor fills the rest of the width.
            let longest = fields
                .iter()
                .zip(&labels)
                .map(|(f, lbl)| {
                    let marker = if f.must { "*" } else { "" };
                    UnicodeWidthStr::width(format!("{lbl}{marker}").as_str()) as i32
                })
                .max()
                .unwrap_or(0);
            label_w = label_col_width(longest, w);
            // Children are added at a placeholder rect; `layout_blocks` below stacks
            // them at their real variable-height positions in one pass.
            for (f, kind) in fields.iter().zip(&kinds) {
                let lid = sg.add_content(
                    Box::new(FieldLabel::label(Rect::new(0, 0, label_w, 1))),
                    Rect::new(0, 0, label_w, 1),
                );
                let hctx = help_ctx_for(*kind, f);
                let vid = match kind {
                    ValueKind::Text => {
                        // The three-surface model is the framework default (tvision
                        // 0.9): only the focused field paints the bright well
                        // (InputNormal), non-focused fields use InputSurface (base3),
                        // and an inactive pane recedes to InputInactive (desktop).
                        let mut il = InputLine::with_limit(Rect::new(0, 0, w, 1), 1024);
                        il.state.state.disabled = !cell_focusable(f);
                        il.state_mut().help_ctx = hctx;
                        sg.add_content(Box::new(il), Rect::new(0, 0, w, 1))
                    }
                    ValueKind::List { ordered } => {
                        // Inline editor: a `ListValueView` wrapping the field's
                        // values. It edits in place (Enter/Ctrl+Enter/Backspace/…)
                        // and signals field navigation via `take_boundary_exit`.
                        let handle = FIELD_LIST_HANDLE;
                        let v = ListValueView::new(
                            Rect::new(0, 0, w, 1),
                            &f.values,
                            *ordered,
                            hctx,
                            handle,
                        );
                        sg.add_content(Box::new(v), Rect::new(0, 0, w, 1))
                    }
                    ValueKind::Launch => {
                        // A read-only bullet block that opens the existing modal
                        // on an action key (password/choice/picker/objectClass).
                        let v = LaunchValueView::new(Rect::new(0, 0, w, 1), hctx);
                        sg.add_content(Box::new(v), Rect::new(0, 0, w, 1))
                    }
                };
                new_lids.push(lid);
                new_vids.push(vid);
            }
        } // sg borrow released; self is free again
        self.label_ids = new_lids;
        self.value_ids = new_vids;
        self.labels = labels;
        self.kinds = kinds;
        self.label_w = label_w;
        self.built_w = inner_w;
        // Stack the blocks at their variable heights (also records tops/heights).
        // The ScrollGroup derives its content extent from the children's logical
        // rects, so there is no separate content-height to set.
        self.layout_blocks(label_w, inner_w, &heights);

        // Reposition the header row's two cells to the same columns as the fields:
        // the `dn` caption fills the label column, the DN value fills the rest.
        let full_w = self.group.state().get_extent().b.x;
        if let Some(v) = self.group.child_mut(self.header_label_id) {
            v.change_bounds(Rect::new(0, 0, label_w, 1));
        }
        if let Some(v) = self.group.child_mut(self.header_value_id) {
            v.change_bounds(Rect::new(label_w, 0, full_w, 1));
        }
    }

    /// Repaint header + all cell texts from `edit_form`; rebuild cells first if
    /// the shown entry changed (different `dn`).
    fn render(&mut self, ctx: &mut Context) {
        let (cur_dn, field_count) = {
            let st = self.state.borrow();
            match st.edit_form.as_ref() {
                Some(f) => (Some(f.dn.clone()), f.fields.len()),
                None => (None, 0),
            }
        };
        // Rebuild cells when the entry changes (different dn) OR the field set
        // changes size OR the pane was resized. Adding/removing an objectClass
        // regenerates the MUST/MAY fields on the SAME entry, growing or shrinking
        // the field list while the dn is unchanged; without the count check the
        // cell vectors go stale and `focusable_value_ids` would index past
        // `value_ids`. A width change (splitter drag) reflows the value editors to
        // fill the new width.
        let inner_w = self.scroll_mut().map(|sg| sg.inner_width()).unwrap_or(0);
        let dn_or_count_changed = cur_dn != self.built_dn || field_count != self.value_ids.len();
        // A width-only reflow keeps the same field focused; an entry/field-set
        // change lands focus on the first field (a fresh form). Capture the
        // focused field index up front so a width reflow can restore it after the
        // cell vectors are rebuilt under new ids.
        let keep_focus_idx = if dn_or_count_changed {
            None
        } else {
            self.focused_field_idx()
        };
        if dn_or_count_changed || inner_w != self.built_w {
            self.rebuild_cells(ctx);
            self.built_dn = cur_dn;
        }

        // Clone the fields so per-kind formatting runs outside the state borrow.
        let fields: Vec<EditField> = {
            let st = self.state.borrow();
            match st.edit_form.as_ref() {
                None => Vec::new(),
                Some(form) => form.fields.clone(),
            }
        }; // state borrow dropped

        // Lookup fields: render `<value> (<name>)` from the resolution cache and
        // kick off resolves for values not yet cached. Collect under one short
        // borrow, then trigger (borrow-free) so we never submit while borrowed.
        use crate::config::widget::WidgetKind;
        use crate::workflows::resolve_flow::LookupKey;
        let mut lookup_lines: Vec<Option<Vec<String>>> = vec![None; fields.len()];
        struct ToResolve {
            key: LookupKey,
            base: String,
            oc: String,
            store: String,
            value: String,
            attrs: Vec<String>,
            template: Vec<crate::config::label::LabelSeg>,
        }
        let mut to_resolve: Vec<ToResolve> = Vec::new();
        {
            let st = self.state.borrow();
            for (i, f) in fields.iter().enumerate() {
                let Some(WidgetKind::Lookup(b)) = &f.widget_binding else {
                    continue;
                };
                let value = f
                    .values
                    .first()
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default();
                if value.is_empty() {
                    lookup_lines[i] = Some(vec![NOT_SET.to_string()]);
                    continue;
                }
                let key = LookupKey {
                    scope_id: b.scope_id(),
                    value: value.clone(),
                };
                match st.lookup_cache.get(&key) {
                    Some(Some(name)) => lookup_lines[i] = Some(vec![format!("{value} ({name})")]),
                    Some(None) => lookup_lines[i] = Some(vec![value.clone()]),
                    None => {
                        lookup_lines[i] = Some(vec![format!("{value} (\u{2026})")]);
                        to_resolve.push(ToResolve {
                            key,
                            base: b.scope.base.clone(),
                            oc: b.object_class().to_string(),
                            store: b.store.clone(),
                            value,
                            attrs: {
                                let mut a = crate::config::label::template_attrs(&b.label_template);
                                if !a.iter().any(|x| x.eq_ignore_ascii_case(&b.store)) {
                                    a.push(b.store.clone());
                                }
                                if !a.iter().any(|x| x.eq_ignore_ascii_case("cn")) {
                                    a.push("cn".into());
                                }
                                a
                            },
                            template: b.label_template.clone(),
                        });
                    }
                }
            }
        } // state borrow dropped
        for r in to_resolve {
            self.state.borrow_mut().resolve_lookup(
                r.key,
                &r.base,
                &r.oc,
                &r.store,
                &r.value,
                &r.attrs,
                &r.template,
            );
        }

        // Compute header with a fresh short borrow (Create mode needs profiles too).
        // The `dn` caption is constant; the DN value goes into the title cell.
        let header = self.header_text();
        if let Some(h) = self.group.child_mut(self.header_label_id) {
            h.set_value(FieldValue::Text("dn".to_string()));
        }
        if let Some(h) = self.group.child_mut(self.header_value_id) {
            h.set_value(FieldValue::Text(header));
        }

        // Feed each value view by its kind and collect fresh block heights. Clone
        // the parallel id/kind vectors before borrowing sg.
        let (label_ids, value_ids, kinds, labels) = (
            self.label_ids.clone(),
            self.value_ids.clone(),
            self.kinds.clone(),
            self.labels.clone(),
        );
        let mut heights: Vec<i32> = Vec::with_capacity(kinds.len());
        {
            let Some(sg) = self.scroll_mut() else {
                return;
            };
            for (i, kind) in kinds.iter().enumerate() {
                let Some(field) = fields.get(i) else { continue };
                heights.push(block_height(field, *kind));
                if let Some(&lid) = label_ids.get(i) {
                    if let Some(l) = sg.child_mut(lid) {
                        // Prefer the precomputed display label (attr + DESC hint);
                        // fall back to the raw attr if the vectors ever disagree.
                        let base = labels.get(i).map(String::as_str).unwrap_or(&field.label);
                        let marker = if field.must { "*" } else { "" };
                        l.set_value(FieldValue::Text(format!("{base}{marker}")));
                    }
                }
                if let Some(&vid) = value_ids.get(i) {
                    if let Some(v) = sg.child_mut(vid) {
                        match kind {
                            ValueKind::Text => {
                                // For Text-kind fields `present_field` equals the raw
                                // first value for editable free-text fields and keeps
                                // the read-only presentation (checkbox/binary) for the
                                // rest — matching the former `widget_for(f).present(f)`.
                                v.set_value(FieldValue::Text(present_field(field)));
                                v.state_mut().state.disabled = !cell_focusable(field);
                            }
                            ValueKind::Launch => {
                                if let Some(lv) = v
                                    .as_any_mut()
                                    .and_then(|a| a.downcast_mut::<LaunchValueView>())
                                {
                                    let lines = lookup_lines
                                        .get(i)
                                        .and_then(|o| o.clone())
                                        .unwrap_or_else(|| launch_lines(field));
                                    lv.set_lines(lines);
                                }
                            }
                            ValueKind::List { .. } => {
                                // Reset the inline editor's model from the field's
                                // values. This runs only on external-change ticks
                                // (`form_needs_render` / width reflow) — never
                                // mid-typing — so resetting is correct here, mirroring
                                // the InputLine re-push contract for Text fields.
                                if let Some(lv) = v
                                    .as_any_mut()
                                    .and_then(|a| a.downcast_mut::<ListValueView>())
                                {
                                    lv.resync(&field.values);
                                }
                            }
                        }
                    }
                }
            }
        } // sg borrow dropped

        // Relayout only when a block's line count changed (values grew/shrank),
        // e.g. an async autonumber result or a picker edit; otherwise the stacked
        // positions are already correct from the last rebuild.
        if heights != self.block_heights {
            let (label_w, inner_w) = (self.label_w, self.built_w);
            self.layout_blocks(label_w, inner_w, &heights);
        }

        // Land focus. On a width-only reflow, restore the previously focused field
        // (its new id at the same index) so a splitter drag never yanks focus back
        // to the top. Otherwise (fresh entry / changed field set) land on the first
        // editable field. Tab switches panes; Up/Down move between fields. Either
        // way, ensure the outer Group routes events to the ScrollGroup.
        let preserved = keep_focus_idx
            .and_then(|i| self.value_ids.get(i).copied())
            .filter(|id| self.focusable_value_ids().contains(id));
        let target = preserved.or_else(|| self.focusable_value_ids().first().copied());
        if let Some(id) = target {
            let scroll_id = self.scroll_id;
            self.group.focus_child(scroll_id, ctx);
            if let Some(sg) = self.scroll_mut() {
                sg.focus_child(id, ctx);
            }
            // Always land with the caret homed to the start of the value (not the
            // select-all-to-end block Turbo Vision leaves on focus). This covers a
            // fresh entry, a re-render of the same form (background refresh: an
            // async autonumber result, a discard), and a width reflow. Plain
            // keystroke editing never re-renders (it does not flag
            // form_needs_render), so this can only home a field the user is not
            // actively typing into.
            self.place_cursor_home(id);
        }
    }

    /// Collapse the given value cell's selection and place the caret at the very
    /// start. A field select-alls its value the moment it becomes focused (Turbo
    /// Vision behaviour); that whole-value selection would be wiped by the first
    /// keystroke, so we clear it and home the caret whenever focus lands on a field.
    fn place_cursor_home(&mut self, id: tv::ViewId) {
        let Some(view) = self.scroll_mut().and_then(|sg| sg.child_mut(id)) else {
            return;
        };
        let Some(any) = view.as_any_mut() else { return };
        if let Some(il) = any.downcast_mut::<InputLine>() {
            // `home()` moves the caret to offset 0, collapses the selection, and
            // scrolls the field fully left. tvision-rs 0.11+ derives the screen
            // cursor from `cur_pos`/`first_pos` in `cursor_request`, so homing is
            // reflected on the next pump with no manual cursor resync. (The Turbo
            // Vision default select-alls a field to the end on focus; this is what
            // undoes that so the first keystroke does not wipe the value.)
            il.home();
        } else if let Some(lv) = any.downcast_mut::<ListValueView>() {
            // Multi-value inline editor: land on the first line so navigating into
            // the field always opens at the top (parity with the text fields).
            lv.cursor_home();
        }
    }

    /// The value-cell view ids of focusable rows (inline-editable OR modal), in
    /// display order. Full-length — no `FORM_ROWS` cap.
    fn focusable_value_ids(&self) -> Vec<tv::ViewId> {
        let st = self.state.borrow();
        match st.edit_form.as_ref() {
            None => Vec::new(),
            Some(form) => form
                .fields
                .iter()
                .enumerate()
                .filter(|(_, f)| cell_focusable(f))
                // `get(i)`, not `[i]`: stay panic-proof if the cell vector is ever
                // momentarily out of sync with `fields` (e.g. mid objectClass
                // resync, before `render` rebuilds the cells).
                .filter_map(|(i, _)| self.value_ids.get(i).copied())
                .collect(),
        }
    }

    /// Move focus to the prev/next focusable field, clamping at the ends (so
    /// Down on the last field and Up on the first stay put — no wrap-around,
    /// matching the list panes). Focuses the first focusable when none is
    /// currently focused.
    fn focus_field(&mut self, delta: i32, ctx: &mut Context) {
        let ids = self.focusable_value_ids();
        if ids.is_empty() {
            return;
        }
        // Current focused field lives inside the ScrollGroup.
        let cur = self.scroll_mut().and_then(|sg| sg.current());
        let pos = cur.and_then(|c| ids.iter().position(|id| *id == c));
        let last = ids.len() as i32 - 1;
        let next = match pos {
            Some(p) => (p as i32 + delta).clamp(0, last) as usize,
            None if delta >= 0 => 0,
            None => ids.len() - 1,
        };
        let next_id = ids[next];
        if let Some(sg) = self.scroll_mut() {
            sg.focus_child(next_id, ctx);
            // Scroll immediately so the newly focused field — and thus the
            // hardware cursor — is on screen this frame, not one pump tick later.
            if let Some(logical) = sg.logical_of(next_id) {
                sg.ensure_visible(logical, ctx);
            }
        }
        // Enter the field with the caret at the start, not a select-all block.
        self.place_cursor_home(next_id);
    }

    /// If pane-local point `pt` falls inside a field's read-only label cell,
    /// return that field's paired value-editor id so a click on the label can
    /// focus the value editor.
    ///
    /// Coordinates: `pt` is pane-local — the owning Group translates a mouse
    /// position into this pane's frame before delivery (`Group::deliver`
    /// subtracts the child origin). A label's pane-local rect is its
    /// ScrollGroup-local bounds (`local_bounds_of`, which already accounts for
    /// the scroll `top`) offset by the ScrollGroup's origin within this pane.
    /// Label cells are disabled InputLines, so the inner group never focuses
    /// them on a click; this lookup lets the caller redirect that click.
    fn value_id_for_label_hit(&mut self, pt: Point) -> Option<tv::ViewId> {
        // Top-left of the ScrollGroup within this pane (pane-local origin).
        let sg_origin = self.group.child_mut(self.scroll_id)?.state().get_bounds().a;
        // Snapshot the parallel ids so the ScrollGroup borrow below does not
        // overlap the `self.label_ids` / `self.value_ids` reads.
        let pairs: Vec<(tv::ViewId, tv::ViewId)> = self
            .label_ids
            .iter()
            .copied()
            .zip(self.value_ids.iter().copied())
            .collect();
        let sg = self.scroll_mut()?;
        for (label_id, value_id) in pairs {
            if let Some(lr) = sg.local_bounds_of(label_id) {
                let r = Rect::new(
                    lr.a.x + sg_origin.x,
                    lr.a.y + sg_origin.y,
                    lr.b.x + sg_origin.x,
                    lr.b.y + sg_origin.y,
                );
                if r.contains(pt) {
                    return Some(value_id);
                }
            }
        }
        None
    }

    /// The field index whose value cell currently holds focus, if any.
    fn focused_field_idx(&mut self) -> Option<usize> {
        let cur = self.scroll_mut().and_then(|sg| sg.current())?;
        self.value_ids.iter().position(|id| *id == cur)
    }

    /// The value-view kind of the focused field, if any.
    fn focused_kind(&mut self) -> Option<ValueKind> {
        let idx = self.focused_field_idx()?;
        self.kinds.get(idx).copied()
    }

    /// Whether the focused field renders as a `LaunchValueView` (Launch only):
    /// an action key on it requests activation of the field's modal editor.
    /// `List` fields edit inline and are handled on their own routing path.
    fn focused_is_launch_view(&mut self) -> bool {
        matches!(self.focused_kind(), Some(ValueKind::Launch))
    }

    /// Whether the focused field renders as an inline `ListValueView`.
    fn focused_is_list_view(&mut self) -> bool {
        matches!(self.focused_kind(), Some(ValueKind::List { .. }))
    }

    /// Read (and clear) the focused `ListValueView`'s one-shot boundary-exit
    /// signal: `Some(-1)` when Up hit the top, `Some(1)` when Down hit the
    /// bottom, `None` otherwise (or the focused child is not a list view).
    fn focused_list_take_boundary_exit(&mut self) -> Option<i32> {
        let cur = self.scroll_mut().and_then(|sg| sg.current())?;
        self.scroll_mut()
            .and_then(|sg| sg.child_mut(cur))
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<ListValueView>())
            .and_then(|lv| lv.take_boundary_exit())
    }

    /// The current display-line count of the focused `ListValueView`, if the
    /// focused field is a list. Used to detect a line-count change after an edit
    /// and trigger a relayout so the block grows / shrinks live.
    fn focused_list_line_count(&mut self) -> Option<i32> {
        let cur = self.scroll_mut().and_then(|sg| sg.current())?;
        self.scroll_mut()
            .and_then(|sg| sg.child_mut(cur))
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<ListValueView>())
            .map(|lv| lv.line_count())
    }

    /// Recompute every block's height from the current view state and, if it
    /// differs from the laid-out heights, relayout so the edited list block
    /// grows / shrinks and the following blocks shift. Uses the live
    /// `ListValueView::line_count()` for list fields and `block_height` for the
    /// rest. Keeps the focused block visible afterwards. No-op when nothing moved.
    fn relayout_after_list_edit(&mut self, ctx: &mut Context) {
        // Snapshot field values + kinds under a short state borrow, then drop it
        // before touching any views (borrow discipline).
        let fields: Vec<EditField> = {
            let st = self.state.borrow();
            match st.edit_form.as_ref() {
                None => return,
                Some(form) => form.fields.clone(),
            }
        };
        let (value_ids, kinds) = (self.value_ids.clone(), self.kinds.clone());
        let mut heights: Vec<i32> = Vec::with_capacity(kinds.len());
        if let Some(sg) = self.scroll_mut() {
            for (i, kind) in kinds.iter().enumerate() {
                let h = match kind {
                    ValueKind::List { .. } => value_ids
                        .get(i)
                        .and_then(|&vid| sg.child_mut(vid))
                        .and_then(|v| v.as_any_mut())
                        .and_then(|a| a.downcast_mut::<ListValueView>())
                        .map(|lv| lv.line_count())
                        .unwrap_or(1),
                    _ => fields.get(i).map(|f| block_height(f, *kind)).unwrap_or(1),
                };
                heights.push(h);
            }
        }
        if heights != self.block_heights {
            let (label_w, inner_w) = (self.label_w, self.built_w);
            self.layout_blocks(label_w, inner_w, &heights);
            // Keep the focused block on screen after it grew / shifted. Use the
            // caret-aware path so an oversized list block tracks its caret row
            // rather than parking an edge (and matches the per-event scroll).
            if let Some(sg) = self.scroll_mut() {
                sg.ensure_focused_visible(ctx);
            }
        }
    }

    /// Read (and clear) the focused `LaunchValueView`'s one-shot activate flag. The
    /// view sets it from its own `handle_event` when an action key arrives; the pane
    /// then posts `ACTIVATE`. `false` when the focused child is not a launch view.
    fn focused_launch_take_activate(&mut self) -> bool {
        let Some(cur) = self.scroll_mut().and_then(|sg| sg.current()) else {
            return false;
        };
        self.scroll_mut()
            .and_then(|sg| sg.child_mut(cur))
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<LaunchValueView>())
            .map(|lv| lv.take_activate())
            .unwrap_or(false)
    }

    /// Sync each editable value view into `edit_form`; refresh header.
    /// `Text` fields pull their `InputLine` text (single value); inline `List`
    /// fields pull the full value vector from their `ListValueView`.
    /// Borrow discipline: collect indices (drop state borrow), read from scroll group
    /// (drop sg borrow), then mutate state (drop before touching group header).
    fn sync_into_form(&mut self) {
        // Collect editable Text-field indices; drop borrow before accessing views.
        // (List fields are pulled separately below, keyed on `self.kinds`.)
        let editable: Vec<usize> = {
            let st = self.state.borrow();
            match st.edit_form.as_ref() {
                None => Vec::new(),
                Some(form) => form
                    .fields
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| inline_editable(f))
                    .map(|(i, _)| i)
                    .collect(),
            }
        }; // state borrow dropped

        // Read current texts (Text) and value vectors (List) from the ScrollGroup.
        let (value_ids, kinds) = (self.value_ids.clone(), self.kinds.clone());
        let mut text_edits: Vec<(usize, String)> = Vec::new();
        let mut list_edits: Vec<(usize, Vec<String>)> = Vec::new();
        if let Some(sg) = self.scroll_mut() {
            for &i in &editable {
                if let Some(&vid) = value_ids.get(i) {
                    if let Some(FieldValue::Text(s)) = sg.child_mut(vid).and_then(|v| v.value()) {
                        text_edits.push((i, s));
                    }
                }
            }
            for (i, kind) in kinds.iter().enumerate() {
                if !matches!(kind, ValueKind::List { .. }) {
                    continue;
                }
                if let Some(vals) = value_ids
                    .get(i)
                    .and_then(|&vid| sg.child_mut(vid))
                    .and_then(|v| v.as_any_mut())
                    .and_then(|a| a.downcast_mut::<ListValueView>())
                    .map(|lv| lv.to_values())
                {
                    list_edits.push((i, vals));
                }
            }
        } // sg borrow dropped

        // Write edits back into the form; borrow_mut dropped before header is computed.
        // Do NOT set `form_needs_render` — that would force a resync that resets the
        // inline editor's cursor mid-edit. The dirty state is updated in place.
        {
            let mut st = self.state.borrow_mut();
            if let Some(form) = st.edit_form.as_mut() {
                for (i, s) in text_edits {
                    if form
                        .fields
                        .get(i)
                        .map(|f| f.values.first().map(String::as_str))
                        != Some(Some(s.as_str()))
                    {
                        form.set_value(i, s);
                    }
                }
                for (i, vals) in list_edits {
                    if let Some(f) = form.fields.get_mut(i) {
                        if f.values != vals {
                            f.values = vals;
                        }
                    }
                }
            }
        } // borrow_mut dropped

        // Compute header after writes are committed; Create mode needs profiles.
        let text = self.header_text();
        if let Some(h) = self.group.child_mut(self.header_value_id) {
            h.set_value(FieldValue::Text(text));
        }
    }
}

#[delegate(to = group)]
impl View for FormPane {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    fn draw(&mut self, ctx: &mut DrawCtx) {
        // Mark the label of the field that holds focus so it gets the blue
        // current-row chip. Everything focus-driven is now the framework's job:
        // the pane background recedes via `Group::set_surface`, the labels/values
        // via `ctx.owner_active()` (and the value wells via the InputLine
        // self-focus surface). The pane no longer mirrors focus or repaints cells.
        let active_idx = self.focused_field_idx();
        let label_ids = self.label_ids.clone();
        if let Some(sg) = self.scroll_mut() {
            for (i, &lid) in label_ids.iter().enumerate() {
                if let Some(fl) = sg
                    .child_mut(lid)
                    .and_then(|v| v.as_any_mut())
                    .and_then(|a| a.downcast_mut::<FieldLabel>())
                {
                    fl.set_active(Some(i) == active_idx);
                }
            }
        }
        self.group.draw(ctx);
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        // Render whenever the form needs it, on ANY event. The dispatch closure
        // (Discard, re-read) only sets `form_needs_render` — it cannot broadcast
        // REFRESH (Program has no broadcast) — and the 50ms pump timer reaches
        // this view, so a flagged re-render repaints within one tick.
        // Also re-render on a width change (splitter drag): render() reflows the
        // value editors to fill the new width. `form_needs_render` covers content
        // changes; the width check covers geometry changes that carry no flag.
        let width_changed =
            self.scroll_mut().map(|sg| sg.inner_width()).unwrap_or(0) != self.built_w;
        if width_changed || self.state.borrow().form_needs_render {
            self.state.borrow_mut().form_needs_render = false;
            self.render(ctx);
        }
        let _ = REFRESH; // REFRESH still drives other panes; retained import

        // When focus enters the pane (Tab from another pane), the framework
        // select-alls the current field (Turbo Vision focus behaviour), leaving the
        // caret at the end over a highlighted block that the first keystroke would
        // wipe. Detect the unfocused→focused transition and home the caret — unless
        // the focus came from a mouse click, where the click position should win.
        let entered_by_click = matches!(ev, Event::MouseDown(_));
        let focused_now = self.group.state().state.focused;
        if focused_now && !self.was_focused && !entered_by_click {
            if let Some(cur) = self.scroll_mut().and_then(|sg| sg.current()) {
                self.place_cursor_home(cur);
            }
        }
        self.was_focused = focused_now;

        // A click on a field's read-only label cell focuses that field's value
        // editor. Label cells are disabled InputLines, so the inner group never
        // focuses them itself; intercept the click here and redirect focus to
        // the paired value editor, mirroring `render()`'s focus landing (focus
        // the ScrollGroup in the outer group, then the value cell within it).
        // `me.position` is already pane-local (the parent group translates it
        // before delivery).
        if let Event::MouseDown(me) = ev {
            let pos = me.position;
            if let Some(vid) = self.value_id_for_label_hit(pos) {
                let scroll_id = self.scroll_id;
                self.group.focus_child(scroll_id, ctx);
                if let Some(sg) = self.scroll_mut() {
                    sg.focus_child(vid, ctx);
                }
                self.place_cursor_home(vid);
                ev.clear();
                self.sync_into_form();
                return;
            }
        }

        // Tab is reserved for switching panes. Do not let the inner group consume
        // it for intra-pane focus cycling — return without clearing so the parent
        // Splitter receives it and moves to the next pane.
        if matches!(ev, Event::KeyDown(k) if k.key == Key::Tab) {
            self.sync_into_form();
            return;
        }

        // Mouse wheel scrolls the form by MOVING FOCUS through fields, not by
        // sliding content under a stationary cursor. Moving focus lets
        // `ensure_visible` scroll the form so the focused field — and the hardware
        // cursor — stays on screen, so the wheel "advances the cursor" and can
        // never strand it off-screen (which previously wedged the display). The
        // form consumes the wheel only when the cursor is over IT — tvision
        // delivers the wheel non-positionally (the splitter offers it to each pane
        // in turn), so without this gate the form, as the splitter's last child,
        // would grab every wheel regardless of the pointer.
        if super::wheel_misses_pane(self.group.state(), ev) {
            return; // cursor is over a sibling pane: let the wheel propagate
        }
        if let Event::MouseWheel(me) = ev {
            let delta = match me.wheel {
                tv::event::MouseWheel::Down => 1,
                tv::event::MouseWheel::Up => -1,
                _ => 0,
            };
            if delta != 0 {
                self.focus_field(delta, ctx);
                self.sync_into_form();
            }
            ev.clear();
            return;
        }

        // Route keystrokes by the focused field's value-view kind. A focused
        // inline `ListValueView` gets first crack at every key so it can edit
        // in place; field navigation across a list boundary is driven SOLELY by
        // its `take_boundary_exit()` flag (the view always consumes Up/Down, so
        // we never infer navigation from an unconsumed event). For Text and
        // Launch fields, Up/Down move focus between fields as before.
        let is_keydown = matches!(ev, Event::KeyDown(_));
        let nav = matches!(ev, Event::KeyDown(k) if matches!(k.key, Key::Up | Key::Down));
        if is_keydown && self.focused_is_list_view() {
            // The list view edits (or moves the caret / reorders). Snapshot its
            // line count first so we can detect a grow / shrink afterwards.
            let before = self.focused_list_line_count();
            self.group.handle_event(ev, ctx);
            // Boundary navigation: the view flags an edge-hit on Up/Down; on that
            // signal, move to the previous / next field.
            if let Some(dir) = self.focused_list_take_boundary_exit() {
                self.focus_field(dir, ctx);
            } else if self.focused_list_line_count() != before {
                // The edit changed the block's line count: relayout so it grows /
                // shrinks and the following blocks shift, then keep it visible.
                self.relayout_after_list_edit(ctx);
            }
        } else if nav {
            let down = matches!(ev, Event::KeyDown(k) if k.key == Key::Down);
            self.focus_field(if down { 1 } else { -1 }, ctx);
            ev.clear();
        } else if is_keydown && self.focused_is_launch_view() {
            // Action key on a read-only launch/list block: route it to the focused
            // `LaunchValueView` (nav keys pass through untouched; any other key sets
            // its activate flag and consumes the event). If it requested activation,
            // record the field index and post ACTIVATE so the controller opens the
            // field's modal editor.
            self.group.handle_event(ev, ctx);
            if self.focused_launch_take_activate() {
                if let Some(idx) = self.focused_field_idx() {
                    self.state.borrow_mut().activate_field = Some(idx);
                    ctx.post(ACTIVATE);
                }
            }
        } else {
            self.group.handle_event(ev, ctx);
        }
        // Keep edit_form current with the on-screen editors.
        self.sync_into_form();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{FieldKind, SchemaModel};
    use crate::ui::UiState;
    use crate::workflows::edit_form::{EditField, EditForm, FormMode};
    use crate::workflows::form_model::WidgetSpec;
    use crate::workflows::structure::Structure;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    /// Build a FormPane over a Shared state in create mode.
    /// The profile at `profile_idx` has `rdn_attr` set; the form is seeded with
    /// `fields` and `mode = FormMode::Create { profile_idx, container }`.
    fn build_pane_with_create_form(
        profile_idx: usize,
        container: &str,
        rdn_attr: &str,
        fields: Vec<EditField>,
    ) -> (Shared, FormPane) {
        use crate::config::EntryProfile;
        use crate::ldap::worker::RawSubschema;
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let structure = Structure::build("dc=x", vec![]);
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        // Ensure the profile at profile_idx exists with the correct rdn_attr.
        st.profiles = vec![EntryProfile {
            rdn_attr: rdn_attr.to_string(),
            ..Default::default()
        }];
        st.edit_form = Some(EditForm {
            dn: String::new(),
            mode: FormMode::Create {
                profile_idx,
                container: container.to_string(),
            },
            object_classes: vec![],
            fields,
        });
        st.form_needs_render = true;
        let shared: Shared = Rc::new(RefCell::new(st));
        let pane = FormPane::new(Rect::new(0, 0, 80, 20), shared.clone());
        (shared, pane)
    }

    /// Build a FormPane over a Shared state seeded with the given fields.
    /// Returns `(shared, pane)`. The caller creates its own headless context.
    fn build_pane_with_form(fields: Vec<EditField>) -> (Shared, FormPane) {
        use crate::ldap::worker::RawSubschema;
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let structure = Structure::build("dc=x", vec![]);
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.edit_form = Some(EditForm {
            dn: "cn=test,dc=x".into(),
            mode: FormMode::Edit,
            object_classes: vec![],
            fields,
        });
        st.form_needs_render = true;
        let shared: Shared = Rc::new(RefCell::new(st));
        let pane = FormPane::new(Rect::new(0, 0, 80, 20), shared.clone());
        (shared, pane)
    }

    fn ef(label: &str, val: &str, editable: bool) -> EditField {
        EditField {
            label: label.into(),
            must: false,
            editable,
            multi: false,
            secret: false,
            ordered: false,
            orphaned: false,
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: None,
            values: vec![val.into()],
            baseline: vec![val.into()],
        }
    }

    fn state_with_form() -> Shared {
        use crate::ldap::worker::RawSubschema;
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let structure = Structure::build("dc=x", vec![]);
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.edit_form = Some(EditForm {
            dn: "cn=a,dc=x".into(),
            mode: FormMode::Edit,
            object_classes: vec![],
            fields: vec![ef("cn", "a", true), ef("creatorsName", "admin", false)],
        });
        st.form_needs_render = true;
        Rc::new(RefCell::new(st))
    }

    fn headless_ctx<'a>(
        out: &'a mut VecDeque<Event>,
        timers: &'a mut tv::timer::TimerQueue,
        deferred: &'a mut Vec<tv::Deferred>,
    ) -> Context<'a> {
        Context::new(out, timers, 0, deferred)
    }

    /// Regression: children must grow via grow_mode when the Splitter drives
    /// Group::change_bounds — NOT via an on_bounds_changed override (which the
    /// framework never calls for Splitter-nested panes).
    ///
    /// TDD evidence: before grow_mode flags were set (hi_x on header, hi_x+hi_y
    /// on scroll), this test FAILED — children kept their original Rect. After
    /// setting the flags, Group::change_bounds propagates the delta and this PASSES.
    #[test]
    fn grow_mode_resize_fills_pane() {
        let shared = state_with_form();
        let mut pane = FormPane::new(Rect::new(0, 0, 40, 6), shared);
        // Simulate Splitter driving a resize: just change_bounds, no on_bounds_changed.
        <FormPane as View>::change_bounds(&mut pane, Rect::new(0, 0, 80, 20));
        // Header spans the new full width; scroll child fills rows 1..20.
        assert_eq!(
            pane.header_bounds_for_test().b.x,
            80,
            "header must widen to new width (hi_x)"
        );
        assert_eq!(
            pane.scroll_bounds_for_test(),
            Rect::new(0, 1, 80, 20),
            "scroll group must fill remaining width+height (hi_x+hi_y)"
        );
    }

    #[test]
    fn updown_moves_focus_and_clamps_at_ends() {
        // Tab switches panes (consumed by the Splitter), so intra-pane field
        // navigation uses Up/Down. Render focuses the first editable field; Up/Down
        // cycle among editable rows, skipping read-only ones.
        use crate::ldap::worker::RawSubschema;
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let structure = Structure::build("dc=x", vec![]);
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        // A read-only field is neither inline-editable nor a List/Launch field,
        // so it is skipped by focus cycling.
        let cn = ef("cn", "a", false);
        st.edit_form = Some(EditForm {
            dn: "cn=a,dc=x".into(),
            mode: FormMode::Edit,
            object_classes: vec![],
            fields: vec![cn, ef("gidNumber", "1001", true), ef("sn", "Bar", true)],
        });
        st.form_needs_render = true;
        let shared: Shared = Rc::new(RefCell::new(st));
        // Use a concrete height (no longer references the removed FORM_ROWS constant).
        let mut pane = FormPane::new(Rect::new(0, 0, 80, 20), shared.clone());
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut ev, &mut ctx); // render + focus first editable

        let focusable = pane.focusable_value_ids();
        assert_eq!(
            focusable.len(),
            2,
            "cn (read-only) is not focusable; gidNumber+sn are"
        );
        // Focus lives inside the ScrollGroup; query it via scroll_mut().
        let cur = pane.scroll_mut().and_then(|sg| sg.current());
        assert_eq!(
            cur,
            Some(focusable[0]),
            "render focuses the first focusable field"
        );

        let mut d = Event::KeyDown(tv::KeyEvent::from(tv::Key::Down));
        pane.handle_event(&mut d, &mut ctx);
        let cur = pane.scroll_mut().and_then(|sg| sg.current());
        assert_eq!(cur, Some(focusable[1]), "Down → next focusable field");

        // Down on the LAST focusable field clamps — it must NOT wrap back to the
        // top (the reported "focus jumps back to the top from the bottom" bug).
        let mut d = Event::KeyDown(tv::KeyEvent::from(tv::Key::Down));
        pane.handle_event(&mut d, &mut ctx);
        let cur = pane.scroll_mut().and_then(|sg| sg.current());
        assert_eq!(
            cur,
            Some(focusable[1]),
            "Down on the last field stays put (clamp, no wrap)"
        );

        let mut u = Event::KeyDown(tv::KeyEvent::from(tv::Key::Up));
        pane.handle_event(&mut u, &mut ctx);
        let cur = pane.scroll_mut().and_then(|sg| sg.current());
        assert_eq!(cur, Some(focusable[0]), "Up → previous focusable field");

        // Up on the FIRST focusable field clamps at the top too (no wrap to bottom).
        let mut u = Event::KeyDown(tv::KeyEvent::from(tv::Key::Up));
        pane.handle_event(&mut u, &mut ctx);
        let cur = pane.scroll_mut().and_then(|sg| sg.current());
        assert_eq!(
            cur,
            Some(focusable[0]),
            "Up on the first field stays put (clamp, no wrap)"
        );
    }

    #[test]
    fn mouse_wheel_scrolls_only_when_cursor_is_over_the_pane() {
        // tvision delivers the wheel non-positionally (the splitter offers it to
        // each pane until one consumes it), with the position translated into the
        // pane's local frame. The form must scroll only when the cursor is over
        // IT — a wheel whose local position lies outside the pane's extent (the
        // cursor is over a sibling pane) must be left unconsumed so it propagates.
        use crate::ldap::worker::RawSubschema;
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let structure = Structure::build("dc=x", vec![]);
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.edit_form = Some(EditForm {
            dn: "cn=a,dc=x".into(),
            mode: FormMode::Edit,
            object_classes: vec![],
            fields: vec![ef("gidNumber", "1001", true), ef("sn", "Bar", true)],
        });
        st.form_needs_render = true;
        let shared: Shared = Rc::new(RefCell::new(st));
        let mut pane = FormPane::new(Rect::new(0, 0, 80, 20), shared.clone());
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut ev, &mut ctx); // render + focus first focusable
        let focusable = pane.focusable_value_ids();
        let cur = pane.scroll_mut().and_then(|sg| sg.current());
        assert_eq!(cur, Some(focusable[0]));

        // Cursor NOT over this pane (local x is left of the pane): the wheel must
        // be left unconsumed (still live) and must not move focus.
        let mut w = Event::MouseWheel(tv::event::MouseEvent {
            position: tv::Point::new(-5, 5),
            wheel: tv::event::MouseWheel::Down,
            ..Default::default()
        });
        pane.handle_event(&mut w, &mut ctx);
        assert!(
            !w.is_nothing(),
            "a wheel over a sibling pane must stay unconsumed so it propagates"
        );
        let cur = pane.scroll_mut().and_then(|sg| sg.current());
        assert_eq!(
            cur,
            Some(focusable[0]),
            "a wheel that misses the pane must not move focus"
        );

        // Cursor OVER this pane: wheel down advances focus and consumes the event.
        let mut w = Event::MouseWheel(tv::event::MouseEvent {
            position: tv::Point::new(10, 5),
            wheel: tv::event::MouseWheel::Down,
            ..Default::default()
        });
        pane.handle_event(&mut w, &mut ctx);
        assert!(w.is_nothing(), "a wheel over the pane is consumed");
        let cur = pane.scroll_mut().and_then(|sg| sg.current());
        assert_eq!(
            cur,
            Some(focusable[1]),
            "a wheel over the pane advances focus"
        );
    }

    #[test]
    fn growing_field_set_on_same_dn_rebuilds_cells_without_panic() {
        // Adding an objectClass regenerates MUST/MAY fields on the SAME entry, so
        // the field list grows while the dn is unchanged. render() must rebuild the
        // cell vectors (not skip on the unchanged dn), or focusable_value_ids()
        // indexes past value_ids and panics (the reported crash).
        use crate::ldap::worker::RawSubschema;
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let structure = Structure::build("dc=x", vec![]);
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.edit_form = Some(EditForm {
            dn: "cn=a,dc=x".into(),
            mode: FormMode::Edit,
            object_classes: vec![],
            fields: vec![ef("cn", "a", true), ef("sn", "B", true)],
        });
        st.form_needs_render = true;
        let shared: Shared = Rc::new(RefCell::new(st));
        let mut pane = FormPane::new(Rect::new(0, 0, 80, 20), shared.clone());
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(
            &mut ev,
            &mut headless_ctx(&mut out, &mut timers, &mut deferred),
        );
        assert_eq!(pane.field_cell_count(), 2);

        // Simulate the objectClass add: same dn, two extra fields.
        {
            let mut s = shared.borrow_mut();
            let form = s.edit_form.as_mut().unwrap();
            form.fields.push(ef("gidNumber", "1001", true));
            form.fields.push(ef("uidNumber", "1002", true));
            s.form_needs_render = true;
        }
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        // Must not panic, and the cells must be rebuilt to match the grown field set.
        pane.handle_event(
            &mut ev,
            &mut headless_ctx(&mut out, &mut timers, &mut deferred),
        );
        assert_eq!(
            pane.field_cell_count(),
            4,
            "cells rebuilt to match the grown field set"
        );
        assert_eq!(pane.focusable_value_ids().len(), 4);
    }

    #[test]
    fn builds_a_cell_per_field_no_row_cap() {
        // Each field must get its own value cell; there must be no fixed row cap.
        // Regression guard: old code capped at FORM_ROWS=32 and panicked beyond that.
        use crate::ldap::worker::RawSubschema;
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let structure = Structure::build("dc=x", vec![]);
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        let fields: Vec<EditField> = (0..40)
            .map(|i| ef(&format!("attr{i}"), "v", true))
            .collect();
        st.edit_form = Some(EditForm {
            dn: "cn=a,dc=x".into(),
            mode: FormMode::Edit,
            object_classes: vec![],
            fields,
        });
        st.form_needs_render = true;
        let shared: Shared = Rc::new(RefCell::new(st));
        let mut pane = FormPane::new(Rect::new(0, 0, 80, 12), shared.clone()); // small viewport
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut ev, &mut ctx); // must not panic; builds 40 rows
        assert_eq!(
            pane.field_cell_count(),
            40,
            "one value cell per field, uncapped"
        );
    }

    #[test]
    fn editable_rows_enabled_static_rows_disabled() {
        let shared = state_with_form();
        let mut pane = FormPane::new(Rect::new(0, 0, 80, 20), shared.clone());
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut ev, &mut ctx);
        // value row 0 (cn) editable → enabled; value row 1 (creatorsName) disabled.
        assert!(!pane.value_disabled(0));
        assert!(pane.value_disabled(1));
    }

    #[test]
    fn editing_value_inputline_marks_form_dirty() {
        let shared = state_with_form();
        let mut pane = FormPane::new(Rect::new(0, 0, 80, 20), shared.clone());
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut ev, &mut ctx);
        // Simulate a committed edit by writing the value InputLine's data directly.
        pane.set_value_text(0, "abc".into());
        let mut ev2 = Event::KeyDown(tv::KeyEvent::from(tv::Key::Char('x')));
        pane.handle_event(&mut ev2, &mut ctx);
        assert!(shared.borrow().edit_form.as_ref().unwrap().is_dirty());
    }

    #[test]
    fn enter_on_objectclass_row_posts_activate() {
        // TDD for Task 6: objectClass row must be focusable; Enter on it sets
        // activate_field and posts ACTIVATE.
        let (shared, mut pane) = build_pane_with_form(vec![
            EditField {
                label: "cn".into(),
                must: true,
                editable: true,
                multi: false,
                secret: false,
                ordered: false,
                orphaned: false,
                kind: FieldKind::Text,
                widget: WidgetSpec::ReadOnlyText,
                widget_binding: None,
                values: vec!["Bob".into()],
                baseline: vec!["Bob".into()],
            },
            EditField {
                label: "objectClass".into(),
                must: true,
                editable: false,
                multi: true,
                secret: false,
                ordered: false,
                orphaned: false,
                kind: FieldKind::Text,
                widget: WidgetSpec::ReadOnlyText,
                widget_binding: None,
                values: vec!["top".into(), "person".into()],
                baseline: vec!["top".into(), "person".into()],
            },
        ]);
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();

        // Initial render + focus first focusable (cn).
        let mut tick = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(
            &mut tick,
            &mut headless_ctx(&mut out, &mut timers, &mut deferred),
        );

        // Move focus down to the objectClass row.
        let mut down = Event::KeyDown(tv::KeyEvent::from(tv::Key::Down));
        pane.handle_event(
            &mut down,
            &mut headless_ctx(&mut out, &mut timers, &mut deferred),
        );

        // Press Enter — must record activate_field = Some(1) and post ACTIVATE.
        let mut enter = Event::KeyDown(tv::KeyEvent::from(tv::Key::Enter));
        pane.handle_event(
            &mut enter,
            &mut headless_ctx(&mut out, &mut timers, &mut deferred),
        );

        assert_eq!(shared.borrow().activate_field, Some(1));
        // T6: ACTIVATE command must also be posted into the event queue so the
        // controller (app::dispatch) can open the modal editor.
        assert!(
            out.iter()
                .any(|e| matches!(e, Event::Command(cmd) if *cmd == ACTIVATE)),
            "ACTIVATE command must be posted to the event queue after Enter on objectClass"
        );
    }

    /// TDD for Task 3: create-mode header must compose the live DN from the RDN
    /// field value + container, and include the `(new)` label.
    #[test]
    fn create_mode_header_composes_dn_from_rdn_field() {
        let (_shared, mut pane) = build_pane_with_create_form(
            0,
            "ou=people,dc=example,dc=org",
            "uid",
            vec![EditField {
                label: "uid".into(),
                must: true,
                editable: true,
                multi: false,
                secret: false,
                ordered: false,
                orphaned: false,
                kind: FieldKind::Text,
                widget: WidgetSpec::ReadOnlyText,
                widget_binding: None,
                values: vec!["alice".into()],
                baseline: vec![],
            }],
        );
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut tick = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(
            &mut tick,
            &mut headless_ctx(&mut out, &mut timers, &mut deferred),
        );
        let hdr = pane.header_text_for_test();
        assert!(
            hdr.contains("uid=alice,ou=people,dc=example,dc=org"),
            "header must contain the composed DN; got: {hdr:?}"
        );
        assert!(
            hdr.contains("(new)"),
            "header must contain (new); got: {hdr:?}"
        );
    }

    #[test]
    fn click_on_label_maps_to_paired_value_field() {
        // A pane-local point inside a field's label cell must resolve to that
        // field's value-editor id (so a label click can focus the value editor).
        // Geometry: the label column spans x 0..label_w (sized to the longest
        // label); the ScrollGroup starts at pane row 1 (header is row 0) with
        // scroll top=0, so field row i's label occupies pane-local y = i + 1.
        let shared = state_with_form();
        let mut pane = FormPane::new(Rect::new(0, 0, 80, 20), shared);
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut ev, &mut ctx); // build cells

        let v0 = pane.value_ids[0];
        let v1 = pane.value_ids[1];
        // Inside row-0 / row-1 label cells → paired value ids.
        assert_eq!(
            pane.value_id_for_label_hit(Point::new(5, 1)),
            Some(v0),
            "click inside row-0 label maps to field 0's value editor"
        );
        assert_eq!(
            pane.value_id_for_label_hit(Point::new(5, 2)),
            Some(v1),
            "click inside row-1 label maps to field 1's value editor"
        );
        // The header row (y=0) is not a label cell.
        assert_eq!(pane.value_id_for_label_hit(Point::new(5, 0)), None);
        // A point in the value column (x >= label_w) is not a label hit.
        assert_eq!(
            pane.value_id_for_label_hit(Point::new(pane.label_w + 2, 1)),
            None
        );
        // A point below the last field is not a label hit.
        assert_eq!(pane.value_id_for_label_hit(Point::new(5, 8)), None);
    }

    /// Read `(cur_pos, first_pos)` of the currently focused value InputLine, if
    /// any. In tvision-rs 0.11+ the screen cursor is derived from these two in
    /// `cursor_request` (`x = displayed_pos(cur_pos) - first_pos + 1`), so a homed
    /// field is exactly `(0, 0)` — asserting on them fully pins the rendered
    /// cursor without depending on the focus/visibility gate of `cursor_request`.
    fn focused_caret(pane: &mut FormPane) -> Option<(i32, i32)> {
        let cur = pane.scroll_mut().and_then(|sg| sg.current())?;
        pane.scroll_mut()
            .and_then(|sg| sg.child_mut(cur))
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<InputLine>())
            .map(|il| (il.cur_pos, il.first_pos))
    }

    #[test]
    fn navigating_fields_homes_the_caret() {
        // Moving Down/Up onto a field must land the caret at the START of the
        // value, not the end (Turbo Vision select-alls on focus, cursor at end).
        use crate::ldap::worker::RawSubschema;
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let structure = Structure::build("dc=x", vec![]);
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.edit_form = Some(EditForm {
            dn: "cn=a,dc=x".into(),
            mode: FormMode::Edit,
            object_classes: vec![],
            fields: vec![ef("cn", "hello", true), ef("sn", "world", true)],
        });
        st.form_needs_render = true;
        let shared: Shared = Rc::new(RefCell::new(st));
        let mut pane = FormPane::new(Rect::new(0, 0, 80, 20), shared.clone());
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        // The caret must land at the field start — both the logical offset
        // (cur_pos) and the scroll (first_pos), which together determine the
        // rendered cursor. The old bug set cur_pos but left the screen cursor
        // stale at the end; `home()` now moves both and 0.11 derives the cursor.
        pane.handle_event(&mut ev, &mut ctx); // render focuses first field ("hello")
        assert_eq!(
            focused_caret(&mut pane),
            Some((0, 0)),
            "first field's caret must be homed after the initial render"
        );

        let mut d = Event::KeyDown(tv::KeyEvent::from(tv::Key::Down));
        pane.handle_event(&mut d, &mut ctx);
        assert_eq!(
            focused_caret(&mut pane),
            Some((0, 0)),
            "caret must be homed after moving Down onto the next field"
        );

        // A background-driven re-render (form_needs_render flagged again by a
        // worker / pump, as happens live) must not pop the caret back to the end.
        shared.borrow_mut().form_needs_render = true;
        let mut tick = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut tick, &mut ctx);
        assert_eq!(
            focused_caret(&mut pane),
            Some((0, 0)),
            "caret must stay homed across a background-driven re-render"
        );
    }

    #[test]
    fn umlaut_edit_roundtrips_graphemes() {
        // Grapheme-correct edit regression (folded from the spike umlaut test):
        // a multibyte value set into the InputLine survives the sync into edit_form.
        let shared = state_with_form();
        let mut pane = FormPane::new(Rect::new(0, 0, 80, 20), shared.clone());
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut ev, &mut ctx);
        pane.set_value_text(0, "Müller-Lüdenscheidt".into());
        // Trigger sync with a non-editing event (a live keystroke would land in the
        // now-focused field and replace it — that is correct edit behaviour, not what
        // this grapheme regression checks).
        let mut ev2 = Event::Broadcast {
            command: tv::Command::custom("test.noop"),
            source: None,
        };
        pane.handle_event(&mut ev2, &mut ctx);
        let st = shared.borrow();
        assert_eq!(
            st.edit_form.as_ref().unwrap().fields[0].values,
            vec!["Müller-Lüdenscheidt".to_string()]
        );
    }

    /// Focus visualization matching the tree / leaf panes:
    /// * the selected field's LABEL carries the blue current-row chip (faded when
    ///   the pane is unfocused);
    /// * non-selected value fields carry NO special background (they sit on the
    ///   pane surface), while the one focused field is the bright input well;
    /// * the focused field is NOT select-all'd (no blue bar over its value);
    /// * the empty area below the fields dims with the pane.
    #[test]
    fn form_focus_visualization_matches_the_list_panes() {
        use tvision_rs::{Buffer, Color, Point};
        let mut pane = FormPane::new(Rect::new(0, 0, 60, 10), state_with_form());
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut ev, &mut ctx);

        let theme = crate::ui::theme::edaptor_theme();
        let base3 = theme.style(Role::ListNormal).bg;
        let desktop = theme.style(Role::ListInactive).bg;
        let chip = theme.style(Role::ListFocused).bg; // blue current-row chip
        let faded = theme.style(Role::ListSelected).bg; // faded (unfocused) chip
        let input_bg = theme.style(Role::InputNormal).bg; // focused input well
        let accent = theme.style(Role::InputSelected).bg; // select-all highlight
        let label_w = pane.label_w as u16;

        // Draw with the given focus, propagating focus into the current value cell
        // as the framework would, and return a sampler over the resulting buffer.
        let draw = |pane: &mut FormPane, focused: bool| -> Buffer {
            pane.group.state_mut().state.focused = focused;
            // Propagate focus down the chain as the running app would: the
            // ScrollGroup (so its own `set_surface` backdrop and the `owner_active`
            // it fans to the labels/value cells track the pane) and the current
            // value cell (whose own `state.focused` drives its self-focus well).
            if let Some(sg) = pane.scroll_mut() {
                sg.state_mut().state.focused = focused;
            }
            if let Some(cur) = pane.scroll_mut().and_then(|sg| sg.current()) {
                if let Some(sg) = pane.scroll_mut() {
                    if let Some(c) = sg.child_mut(cur) {
                        c.state_mut().state.focused = focused;
                    }
                }
            }
            let mut buf = Buffer::new(60, 10);
            {
                let mut dctx =
                    DrawCtx::new(&mut buf, &theme, Rect::new(0, 0, 60, 10), Point::new(0, 0));
                <FormPane as View>::draw(pane, &mut dctx);
            }
            buf
        };
        let bg = |buf: &Buffer, x: u16, y: u16| -> Color { buf.get(x, y).style().bg };

        // Row layout: header at pane y 0; field 0 ("cn", the active field) at y 1;
        // field 1 ("creatorsName", read-only, non-selected) at y 2.
        let focused = draw(&mut pane, true);
        // Selected field's label → blue chip.
        assert_eq!(bg(&focused, 0, 1), chip, "active label gets the blue chip");
        // Focused field's value → bright input well, and NOT a select-all bar.
        assert_eq!(
            bg(&focused, label_w, 1),
            input_bg,
            "active field is the input well"
        );
        assert_ne!(
            bg(&focused, label_w + 1, 1),
            accent,
            "the focused field must not be select-all'd (no blue value bar)"
        );
        // Non-selected value field → pane surface, no special background.
        assert_eq!(
            bg(&focused, 40, 2),
            base3,
            "non-selected field carries no special background (blends into base3)"
        );
        // Empty area below the fields is the bright fill.
        assert_eq!(bg(&focused, 40, 8), base3, "focused fill is bright");

        let unfocused = draw(&mut pane, false);
        // Selected label fades (like the list panes' unfocused current row).
        assert_eq!(bg(&unfocused, 0, 1), faded, "unfocused active label fades");
        // Value cells recede to the deselected surface too — the whole form dims.
        assert_eq!(
            bg(&unfocused, label_w, 1),
            desktop,
            "unfocused value cell recedes to the deselected surface"
        );
        assert_eq!(
            bg(&unfocused, 40, 2),
            desktop,
            "unfocused non-selected value cell recedes too"
        );
        // Empty area recedes to the desktop tone.
        assert_eq!(bg(&unfocused, 40, 8), desktop, "unfocused fill recedes");
    }

    #[test]
    fn entering_a_field_places_the_caret_at_the_start_without_select_all() {
        // Feedback: entering a field must NOT select all its content (which the
        // first keypress would wipe); the caret starts at position 0 instead.
        let mut pane = FormPane::new(Rect::new(0, 0, 60, 10), state_with_form());
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut ev, &mut ctx); // lands focus on the first field

        let cur = pane.scroll_mut().and_then(|sg| sg.current()).unwrap();
        let (cur_pos, sel_len) = pane
            .scroll_mut()
            .and_then(|sg| sg.child_mut(cur))
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<InputLine>())
            .map(|il| (il.cur_pos, il.sel_end - il.sel_start))
            .unwrap();
        assert_eq!(cur_pos, 0, "caret starts at the beginning of the field");
        assert_eq!(sel_len, 0, "no select-all block on entering the field");
    }

    #[test]
    fn tab_into_pane_homes_the_caret() {
        // When focus enters the pane from another pane, the framework select-alls
        // the current field (caret at the end). The pane must detect the focus
        // transition and home the caret so the first keypress does not wipe it.
        let mut pane = FormPane::new(Rect::new(0, 0, 60, 10), state_with_form());
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut ev, &mut ctx); // pane unfocused in this headless setup

        // Simulate the framework's focus-entry select-all: caret at the end over a
        // full-value selection.
        let cur = pane.scroll_mut().and_then(|sg| sg.current()).unwrap();
        if let Some(il) = pane
            .scroll_mut()
            .and_then(|sg| sg.child_mut(cur))
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<InputLine>())
        {
            il.cur_pos = 1;
            il.sel_start = 0;
            il.sel_end = 1;
        }

        // Focus now enters the pane; the next event must home the caret.
        pane.group.state_mut().state.focused = true;
        let mut tick = Event::Broadcast {
            command: tv::Command::custom("test.noop"),
            source: None,
        };
        pane.handle_event(&mut tick, &mut ctx);

        let (cur_pos, sel_len) = pane
            .scroll_mut()
            .and_then(|sg| sg.child_mut(cur))
            .and_then(|v| v.as_any_mut())
            .and_then(|a| a.downcast_mut::<InputLine>())
            .map(|il| (il.cur_pos, il.sel_end - il.sel_start))
            .unwrap();
        assert_eq!(cur_pos, 0, "focus entry homes the caret");
        assert_eq!(sel_len, 0, "focus entry clears the select-all block");
    }

    #[test]
    fn resize_reflows_value_width_and_keeps_focus() {
        // A wider pane (splitter drag) must reflow the value editors to fill the
        // new width, and must NOT yank focus back to the first field.
        use crate::ldap::worker::RawSubschema;
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let structure = Structure::build("dc=x", vec![]);
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.edit_form = Some(EditForm {
            dn: "cn=a,dc=x".into(),
            mode: FormMode::Edit,
            object_classes: vec![],
            fields: vec![
                ef("cn", "a", true),
                ef("sn", "b", true),
                ef("mail", "c", true),
            ],
        });
        st.form_needs_render = true;
        let shared: Shared = Rc::new(RefCell::new(st));
        let mut pane = FormPane::new(Rect::new(0, 0, 40, 10), shared);
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut ev, &mut ctx);

        // Move focus to the last field (sn/mail); it must survive the resize.
        pane.focus_field(2, &mut ctx);
        let focused_before = pane.scroll_mut().and_then(|sg| sg.current());
        assert_eq!(focused_before, Some(pane.value_ids[2]));

        // The value cell fills to the (inner) width before the resize.
        let vid = pane.value_ids[2];
        let w_before = pane
            .scroll_mut()
            .and_then(|sg| sg.logical_of(vid))
            .map(|r| r.b.x)
            .unwrap();

        // Splitter drives a wider bound; a following event reflows the cells.
        <FormPane as View>::change_bounds(&mut pane, Rect::new(0, 0, 70, 10));
        let mut tick = Event::Broadcast {
            command: tv::Command::custom("test.noop"),
            source: None,
        };
        pane.handle_event(&mut tick, &mut ctx);

        let vid = pane.value_ids[2];
        let w_after = pane
            .scroll_mut()
            .and_then(|sg| sg.logical_of(vid))
            .map(|r| r.b.x)
            .unwrap();
        assert!(
            w_after > w_before,
            "value editor must widen with the pane (was {w_before}, now {w_after})"
        );
        // Focus stayed on the same field (index 2), not snapped back to the top.
        let focused_after = pane.scroll_mut().and_then(|sg| sg.current());
        assert_eq!(
            focused_after,
            Some(pane.value_ids[2]),
            "a resize must preserve the focused field, not reset to the first"
        );
    }

    #[test]
    fn multi_value_field_block_is_multiple_rows_tall() {
        // A 3-value plain multi field renders as one block three rows tall (List
        // kind → read-only bullet block); the next field starts below it, not one
        // row down. This pins the variable-height stacking.
        let mail = EditField {
            label: "mail".into(),
            must: false,
            editable: true,
            multi: true,
            secret: false,
            ordered: false,
            orphaned: false,
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: None,
            values: vec!["a@x".into(), "b@x".into(), "c@x".into()],
            baseline: vec!["a@x".into(), "b@x".into(), "c@x".into()],
        };
        let (_shared, mut pane) = build_pane_with_form(vec![mail, ef("cn", "z", true)]);
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(
            &mut ev,
            &mut headless_ctx(&mut out, &mut timers, &mut deferred),
        );

        // mail → 3-value List block (height 3); cn → Text block (height 1).
        assert_eq!(pane.kinds[0], ValueKind::List { ordered: false });
        assert_eq!(pane.kinds[1], ValueKind::Text);
        assert_eq!(pane.block_heights, vec![3, 1]);
        // The blocks stack by summing heights: mail spans logical rows 0..3, so cn
        // begins at row 3 (three rows below), not one row down.
        let mail_vid = pane.value_ids[0];
        let cn_vid = pane.value_ids[1];
        let mail_rect = pane
            .scroll_mut()
            .and_then(|sg| sg.logical_of(mail_vid))
            .unwrap();
        let cn_rect = pane
            .scroll_mut()
            .and_then(|sg| sg.logical_of(cn_vid))
            .unwrap();
        assert_eq!(mail_rect.a.y, 0, "the first block starts at logical row 0");
        assert_eq!(
            mail_rect.b.y - mail_rect.a.y,
            3,
            "the multi-value block's value view is three rows tall"
        );
        assert_eq!(
            cn_rect.a.y, 3,
            "the next field starts three rows below (blocks stack by summed height)"
        );
    }

    #[test]
    fn action_key_on_launch_field_posts_activate() {
        // A Launch field (objectClass) renders as a read-only block; pressing ANY
        // action key on it (not just Enter) must request activation: set
        // activate_field and post ACTIVATE so the controller opens the modal.
        let (shared, mut pane) = build_pane_with_form(vec![
            ef("cn", "Bob", true),
            EditField {
                label: "objectClass".into(),
                must: true,
                editable: false,
                multi: true,
                secret: false,
                ordered: false,
                orphaned: false,
                kind: FieldKind::Text,
                widget: WidgetSpec::ReadOnlyText,
                widget_binding: None,
                values: vec!["top".into(), "person".into()],
                baseline: vec!["top".into(), "person".into()],
            },
        ]);
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut tick = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(
            &mut tick,
            &mut headless_ctx(&mut out, &mut timers, &mut deferred),
        );
        // Move focus to the objectClass (Launch) field.
        let mut down = Event::KeyDown(tv::KeyEvent::from(tv::Key::Down));
        pane.handle_event(
            &mut down,
            &mut headless_ctx(&mut out, &mut timers, &mut deferred),
        );
        assert_eq!(pane.focused_field_idx(), Some(1));
        // Press a printable key — the launch path must post ACTIVATE for field 1.
        let mut key = Event::KeyDown(tv::KeyEvent::from(tv::Key::Char('x')));
        pane.handle_event(
            &mut key,
            &mut headless_ctx(&mut out, &mut timers, &mut deferred),
        );
        assert_eq!(shared.borrow().activate_field, Some(1));
        assert!(
            out.iter()
                .any(|e| matches!(e, Event::Command(cmd) if *cmd == ACTIVATE)),
            "an action key on a Launch field must post ACTIVATE"
        );
    }

    /// Build a plain multi-value (unordered `List`) field with the given values.
    fn multi_list(label: &str, values: &[&str]) -> EditField {
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
            widget_binding: None,
            values: values.iter().map(|s| s.to_string()).collect(),
            baseline: values.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn list_field_grows_block_when_item_added() {
        // A focused inline List field that gains an item must grow its block by one
        // row, pushing the following field's block down by one.
        let (_shared, mut pane) =
            build_pane_with_form(vec![multi_list("mail", &["a@x"]), ef("cn", "z", true)]);
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut ev, &mut ctx); // render + focus first focusable (mail)

        // The List field is a ListValueView and is focused.
        assert_eq!(pane.kinds[0], ValueKind::List { ordered: false });
        assert_eq!(pane.focused_field_idx(), Some(0));

        // Block heights: mail=1 (single value), cn=1. Record the following field's top.
        assert_eq!(pane.block_heights, vec![1, 1]);
        let cn_vid = pane.value_ids[1];
        let cn_top_before = pane
            .scroll_mut()
            .and_then(|sg| sg.logical_of(cn_vid))
            .map(|r| r.a.y)
            .unwrap();

        // Move to the end of "a@x" and press Enter → splits into ["a@x", ""] → 2 rows.
        let mut end = Event::KeyDown(tv::KeyEvent::from(tv::Key::End));
        pane.handle_event(&mut end, &mut ctx);
        let mut enter = Event::KeyDown(tv::KeyEvent::from(tv::Key::Enter));
        pane.handle_event(&mut enter, &mut ctx);

        // The List block grew by one row; the following field shifted down by one.
        assert_eq!(
            pane.block_heights[0], 2,
            "the List block grew to two rows after Enter"
        );
        let cn_top_after = pane
            .scroll_mut()
            .and_then(|sg| sg.logical_of(cn_vid))
            .map(|r| r.a.y)
            .unwrap();
        assert_eq!(
            cn_top_after,
            cn_top_before + 1,
            "the following field's block moved down by one row"
        );
    }

    #[test]
    fn down_past_list_bottom_moves_to_next_field() {
        // Repeated Down inside a focused List field must eventually cross the bottom
        // boundary and move focus to the next focusable field (via boundary-exit).
        let (_shared, mut pane) = build_pane_with_form(vec![
            multi_list("mail", &["a@x", "b@x"]),
            ef("cn", "z", true),
        ]);
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut ev, &mut ctx); // focus the List field (index 0)
        assert_eq!(pane.focused_field_idx(), Some(0));

        // Press Down a few times: within the two-row list it moves the caret, and the
        // Down that hits the bottom edge crosses to the next field (cn, index 1).
        for _ in 0..4 {
            if pane.focused_field_idx() == Some(1) {
                break;
            }
            let mut d = Event::KeyDown(tv::KeyEvent::from(tv::Key::Down));
            pane.handle_event(&mut d, &mut ctx);
        }
        assert_eq!(
            pane.focused_field_idx(),
            Some(1),
            "Down past the list bottom moves focus to the next field"
        );
    }

    /// Build a helper for an ordered (XOrdered) multi-value list field.
    fn multi_list_ordered(label: &str, values: &[&str]) -> EditField {
        use crate::config::widget::WidgetKind;
        EditField {
            label: label.into(),
            must: false,
            editable: true,
            multi: true,
            secret: false,
            ordered: true,
            orphaned: false,
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: Some(WidgetKind::XOrdered),
            values: values.iter().map(|s| s.to_string()).collect(),
            baseline: values.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn list_edit_syncs_values_and_marks_dirty() {
        // Integration: a keystroke sequence in a focused ListValueView must flow through
        // sync_into_form into edit_form.fields[i].values with correct trimming, and the
        // form must be dirty. This guards the List edit→edit_form seam end-to-end.
        let (shared, mut pane) =
            build_pane_with_form(vec![multi_list("mail", &["a@x"]), ef("cn", "z", true)]);
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);

        // Render and focus the first (List) field.
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut ev, &mut ctx);
        assert_eq!(pane.focused_field_idx(), Some(0));

        // Move to end of "a@x", press Enter to open a new empty item, then type "b@x".
        // Each handle_event call ends with sync_into_form, so by the last key the
        // edit_form sees the updated value vector.
        let mut end = Event::KeyDown(tv::KeyEvent::from(tv::Key::End));
        pane.handle_event(&mut end, &mut ctx);
        let mut enter = Event::KeyDown(tv::KeyEvent::from(tv::Key::Enter));
        pane.handle_event(&mut enter, &mut ctx);
        for c in "b@x".chars() {
            let mut k = Event::KeyDown(tv::KeyEvent::from(tv::Key::Char(c)));
            pane.handle_event(&mut k, &mut ctx);
        }

        let st = shared.borrow();
        let form = st.edit_form.as_ref().unwrap();
        assert_eq!(
            form.fields[0].values,
            vec!["a@x".to_string(), "b@x".to_string()],
            "sync_into_form must write the trimmed value vector into edit_form"
        );
        assert!(
            form.is_dirty(),
            "adding a value to a List field must mark the form dirty"
        );
    }

    #[test]
    fn ordered_list_load_without_edit_is_not_dirty() {
        // Regression: an XOrdered field loaded with values that lack canonical
        // `{n}` prefixes (e.g. a plain `description` treated as x_ordered) must
        // NOT be marked dirty just because sync_into_form reconstructs the `{n}`
        // prefixes on load. Navigating between such entries otherwise raised a
        // phantom "unsaved changes" guard even though nothing was edited.
        let (shared, mut pane) = build_pane_with_form(vec![
            multi_list_ordered("description", &["hello", "world"]),
            ef("cn", "z", true),
        ]);
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);

        // Render + a benign event → sync_into_form runs and reconstructs the `{n}`
        // prefixes into `values`, exactly as navigation would.
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut ev, &mut ctx);
        let mut noop = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut noop, &mut ctx);

        let st = shared.borrow();
        let form = st.edit_form.as_ref().unwrap();
        assert!(
            !form.is_dirty(),
            "loading an ordered list (no edits) must not be dirty; values={:?} baseline={:?}",
            form.fields[0].values,
            form.fields[0].baseline
        );
    }

    #[test]
    fn ordered_list_edit_reconstructs_ordering_prefixes() {
        // Integration: typing a new value into a focused XOrdered ListValueView must
        // appear in edit_form with reconstructed {n} prefixes — proving that
        // sync_into_form calls to_values(ordered=true) for ordered List fields.
        let (shared, mut pane) = build_pane_with_form(vec![
            multi_list_ordered("olcAccess", &["{0}read", "{1}write"]),
            ef("cn", "z", true),
        ]);
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);

        // Render and focus the ordered List field.
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut ev, &mut ctx);
        assert_eq!(pane.focused_field_idx(), Some(0));

        // Navigate to the end of the second value and press Enter to add a third item,
        // then type "exec". The model strips ordering prefixes on load, reconstructs them
        // on to_values(true), so edit_form should see {0}read, {1}write, {2}exec.
        let mut down = Event::KeyDown(tv::KeyEvent::from(tv::Key::Down));
        pane.handle_event(&mut down, &mut ctx);
        let mut end = Event::KeyDown(tv::KeyEvent::from(tv::Key::End));
        pane.handle_event(&mut end, &mut ctx);
        let mut enter = Event::KeyDown(tv::KeyEvent::from(tv::Key::Enter));
        pane.handle_event(&mut enter, &mut ctx);
        for c in "exec".chars() {
            let mut k = Event::KeyDown(tv::KeyEvent::from(tv::Key::Char(c)));
            pane.handle_event(&mut k, &mut ctx);
        }

        let st = shared.borrow();
        let form = st.edit_form.as_ref().unwrap();
        assert_eq!(
            form.fields[0].values,
            vec![
                "{0}read".to_string(),
                "{1}write".to_string(),
                "{2}exec".to_string()
            ],
            "sync_into_form must reconstruct {{n}} ordering prefixes for XOrdered fields"
        );
        assert!(
            form.is_dirty(),
            "adding a value to an ordered List field must mark the form dirty"
        );
    }

    #[cfg(test)]
    mod value_kind_tests {
        use super::*;
        use crate::config::widget::WidgetKind;
        use crate::workflows::edit_form::EditField;

        // Reuse the existing ef(...) test builder in this file (form.rs:799) where possible,
        // extended locally for widget_binding/multi/ordered.
        fn field(label: &str, multi: bool, binding: Option<WidgetKind>) -> EditField {
            let mut f = super::ef(label, "", true); // ef sets editable=true, multi=false
            f.multi = multi;
            f.widget_binding = binding;
            f
        }

        #[test]
        fn single_value_text_is_text_kind() {
            let f = field("cn", false, None);
            assert_eq!(value_kind(&f), ValueKind::Text);
            assert_eq!(block_height(&f, ValueKind::Text), 1);
        }

        #[test]
        fn plain_multi_is_list_unordered() {
            let f = field("mail", true, None);
            assert_eq!(value_kind(&f), ValueKind::List { ordered: false });
        }

        #[test]
        fn xordered_is_list_ordered() {
            let f = field("olcAccess", true, Some(WidgetKind::XOrdered));
            assert_eq!(value_kind(&f), ValueKind::List { ordered: true });
        }

        #[test]
        fn objectclass_is_launch() {
            let f = field("objectClass", true, None);
            assert_eq!(value_kind(&f), ValueKind::Launch);
        }

        #[test]
        fn empty_multi_block_is_one_line() {
            let f = field("mail", true, None); // values empty
            assert_eq!(block_height(&f, ValueKind::List { ordered: false }), 1);
        }

        #[test]
        fn three_values_one_with_newline_is_four_lines() {
            let mut f = field("mail", true, None);
            f.values = vec!["a".into(), "b\ncont".into(), "c".into()];
            assert_eq!(block_height(&f, ValueKind::List { ordered: false }), 4);
        }
    }

    /// Tests that `help_ctx_for` returns the matching constant from `ui::help_ctx`
    /// for each `ValueKind`. Guards the pane ↔ status-line mapping so a rename
    /// of either side is caught at compile time (the constants) and test time (the
    /// name comparison).
    #[cfg(test)]
    mod help_ctx_for_tests {
        use super::*;
        use crate::config::widget::WidgetKind;
        use crate::ui::help_ctx::{
            FIELD_LAUNCH_PASSWORD, FIELD_LAUNCH_PICKER, FIELD_LIST, FIELD_LIST_ORDERED, FIELD_TEXT,
        };

        fn plain_field() -> EditField {
            ef("cn", "value", true)
        }

        fn secret_field() -> EditField {
            let mut f = ef("userPassword", "", true);
            f.secret = true;
            f
        }

        fn multi_field() -> EditField {
            let mut f = ef("mail", "", true);
            f.multi = true;
            f
        }

        #[test]
        fn text_kind_returns_field_text() {
            let f = plain_field();
            assert_eq!(help_ctx_for(ValueKind::Text, &f), FIELD_TEXT);
        }

        #[test]
        fn list_unordered_returns_field_list() {
            let f = multi_field();
            assert_eq!(
                help_ctx_for(ValueKind::List { ordered: false }, &f),
                FIELD_LIST
            );
        }

        #[test]
        fn list_ordered_returns_field_list_ordered() {
            let f = multi_field();
            assert_eq!(
                help_ctx_for(ValueKind::List { ordered: true }, &f),
                FIELD_LIST_ORDERED
            );
        }

        #[test]
        fn launch_non_secret_returns_field_launch_picker() {
            let f = plain_field();
            assert_eq!(help_ctx_for(ValueKind::Launch, &f), FIELD_LAUNCH_PICKER);
        }

        #[test]
        fn launch_secret_returns_field_launch_password() {
            let f = secret_field();
            assert_eq!(help_ctx_for(ValueKind::Launch, &f), FIELD_LAUNCH_PASSWORD);
        }

        #[test]
        fn xordered_binding_help_ctx_is_list_ordered() {
            // End-to-end: value_kind maps XOrdered → List { ordered: true };
            // help_ctx_for then maps that to FIELD_LIST_ORDERED.
            let mut f = ef("olcAccess", "", true);
            f.multi = true;
            f.widget_binding = Some(WidgetKind::XOrdered);
            let kind = value_kind(&f);
            assert_eq!(kind, ValueKind::List { ordered: true });
            assert_eq!(help_ctx_for(kind, &f), FIELD_LIST_ORDERED);
        }
    }

    // ---- Task 6: lookup display helpers and tests ----

    fn lookup_binding_for_test() -> crate::config::widget::WidgetKind {
        use crate::config::relation::{CandidateScope, LookupBinding};
        crate::config::widget::WidgetKind::Lookup(LookupBinding {
            attr: "gidNumber".into(),
            scope: CandidateScope {
                base: "ou=groups,dc=x".into(),
                object_classes: vec!["posixGroup".into()],
                search_attrs: vec!["cn".into()],
                label_template: None,
            },
            store: "gidNumber".into(),
            label_template: crate::config::label::parse_label_template("{cn}"),
        })
    }

    /// Build a pane whose single field is a `gidNumber` lookup with value `5000`,
    /// and optionally pre-seed the resolution cache. Returns the rendered line for
    /// that field's value view.
    fn lookup_line_after_render(cache: Option<Option<String>>) -> String {
        use crate::workflows::resolve_flow::LookupKey;
        let mut field = ef("gidNumber", "5000", false);
        field.widget_binding = Some(lookup_binding_for_test());
        let (shared, mut pane) = build_pane_with_form(vec![field]);
        if let Some(entry) = cache {
            let key = LookupKey {
                scope_id: "ou=groups,dc=x|posixGroup|gidNumber".into(),
                value: "5000".into(),
            };
            shared.borrow_mut().lookup_cache.insert(key, entry);
        }
        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred = Vec::new();
        let mut ctx = headless_ctx(&mut out, &mut timers, &mut deferred);
        let mut ev = Event::Broadcast {
            command: REFRESH,
            source: None,
        };
        pane.handle_event(&mut ev, &mut ctx);
        pane.launch_line_for_test(0)
    }

    #[test]
    fn lookup_resolved_shows_value_and_name() {
        assert_eq!(
            lookup_line_after_render(Some(Some("staff".into()))),
            "5000 (staff)"
        );
    }

    #[test]
    fn lookup_unresolved_not_found_shows_bare_value() {
        assert_eq!(lookup_line_after_render(Some(None)), "5000");
    }

    #[test]
    fn lookup_uncached_shows_ellipsis_placeholder() {
        assert_eq!(lookup_line_after_render(None), "5000 (\u{2026})");
    }

    /// The label column shows the attribute name plus a curated hint.
    mod display_label {
        use super::*;

        #[test]
        fn appends_hint_for_known_attr() {
            assert_eq!(display_label("sn"), "sn (surname)");
            assert_eq!(display_label("l"), "l (location)");
        }

        #[test]
        fn hint_lookup_is_case_insensitive() {
            assert_eq!(display_label("SN"), "SN (surname)");
            assert_eq!(display_label("OU"), "OU (org. unit)");
        }

        #[test]
        fn bare_name_for_unmapped_attr() {
            // Self-explanatory / descriptive names keep their bare name.
            assert_eq!(display_label("homeDirectory"), "homeDirectory");
            assert_eq!(display_label("uidNumber"), "uidNumber");
            assert_eq!(display_label("givenName"), "givenName");
            assert_eq!(display_label("somethingCustom"), "somethingCustom");
        }
    }
}
