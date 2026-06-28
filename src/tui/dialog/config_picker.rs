//! Config-picker dialog: shown by the startup flow when config discovery finds
//! more than one candidate. A `ListBox` of config names plus a two-line
//! read-only detail pane (description + full path) for the highlighted entry.
//!
//! Pattern mirrors `dialog::profile_chooser` / `oc_picker`: a `Dialog`-wrapping
//! `View` with `#[delegate(to = dlg)]`, list seeded in `reset_current` (NOT in
//! `new()`), highlighted index staged into a caller-owned cell. Unlike the
//! in-app dialogs it owns its own `Rc<RefCell<Option<usize>>>` rather than the
//! app `UiState` (which does not exist yet at startup).
//!
//! `dead_code` is suppressed at the module level: the public API here is wired
//! by a later M5a task (startup Program); nothing outside `#[cfg(test)]` calls
//! it yet.
#![allow(dead_code)]

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use tvision_rs::{
    self as tv, delegate, ButtonFlags, ButtonRowAlign, Command, Context, Dialog, Event, FieldValue,
    InputLine, Key, ListBox, Rect, View,
};

/// One discovered config, flattened for display (decoupled from `ConfigCandidate`
/// so the dialog is testable without filesystem discovery).
pub(crate) struct PickerItem {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
}

/// A disabled (read-only, skip-focus) `InputLine` whose text we set at runtime.
/// `StaticText` has no `set_value`, so we reuse the form-pane `ro_cell` idiom.
fn ro_cell(bounds: Rect) -> InputLine {
    let mut il = InputLine::with_limit(bounds, 1024);
    il.state.state.disabled = true;
    il
}

pub(crate) struct ConfigPicker {
    dlg: Dialog,
    list_id: tv::ViewId,
    pub(crate) desc_id: tv::ViewId,
    pub(crate) path_id: tv::ViewId,
    items: Vec<PickerItem>,
    selected: Rc<RefCell<Option<usize>>>,
}

impl ConfigPicker {
    fn new(items: Vec<PickerItem>, selected: Rc<RefCell<Option<usize>>>) -> Self {
        let list_rows = items.len().clamp(3, 12) as i32;
        // frame + list + gap + desc + path + gap + buttons + frame
        let height = 1 + list_rows + 1 + 1 + 1 + 1 + 2 + 1;
        let width = 72;
        let mut dlg = Dialog::new(
            Rect::new(0, 0, width, height),
            Some("Select configuration".to_string()),
        );
        dlg.state_mut().options.center_x = true;
        dlg.state_mut().options.center_y = true;

        let list = ListBox::new(Rect::new(2, 1, width - 2, 1 + list_rows), 1, None, None);
        let list_id = dlg.insert_child(Box::new(list));

        let desc_y = 1 + list_rows + 1;
        let desc_id = dlg.insert_child(Box::new(ro_cell(Rect::new(
            2,
            desc_y,
            width - 2,
            desc_y + 1,
        ))));
        let path_y = desc_y + 1;
        let path_id = dlg.insert_child(Box::new(ro_cell(Rect::new(
            2,
            path_y,
            width - 2,
            path_y + 1,
        ))));

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

        ConfigPicker {
            dlg,
            list_id,
            desc_id,
            path_id,
            items,
            selected,
        }
    }

    /// Read the current list-highlight index.
    fn current_index(&mut self) -> Option<usize> {
        match self.dlg.child_mut(self.list_id).and_then(|v| v.value()) {
            Some(FieldValue::Int(i)) if i >= 0 => Some(i as usize),
            _ => None,
        }
    }

    /// Stage the highlight into the caller's cell and refresh the detail cells.
    fn stage_and_show(&mut self) {
        let idx = self.current_index().unwrap_or(0);
        *self.selected.borrow_mut() = Some(idx);
        let (desc, path) = match self.items.get(idx) {
            Some(it) => (
                it.description.clone(),
                it.path.to_string_lossy().into_owned(),
            ),
            None => (String::new(), String::new()),
        };
        if let Some(c) = self.dlg.child_mut(self.desc_id) {
            c.set_value(FieldValue::Text(desc));
        }
        if let Some(c) = self.dlg.child_mut(self.path_id) {
            c.set_value(FieldValue::Text(path));
        }
    }

    /// Test helper: read a detail cell's current text.
    #[cfg(test)]
    pub(crate) fn detail_text(&mut self, id: tv::ViewId) -> String {
        match self.dlg.child_mut(id).and_then(|v| v.value()) {
            Some(FieldValue::Text(t)) => t,
            _ => String::new(),
        }
    }
}

#[delegate(to = dlg)]
impl View for ConfigPicker {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    fn reset_current(&mut self, ctx: &mut Context) {
        self.dlg.reset_current(ctx);
        let rows: Vec<String> = self.items.iter().map(|it| it.name.clone()).collect();
        if let Some(list) = self.dlg.child_mut(self.list_id) {
            if let Some(lb) = list.as_any_mut().and_then(|a| a.downcast_mut::<ListBox>()) {
                lb.new_list(rows, ctx);
            }
        }
        self.stage_and_show();
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        let nav = matches!(
            ev,
            Event::KeyDown(k)
                if matches!(k.key, Key::Up | Key::Down | Key::PageUp | Key::PageDown)
        );
        if nav {
            if let Some(list) = self.dlg.child_mut(self.list_id) {
                list.handle_event(ev, ctx);
            }
            self.stage_and_show();
        } else {
            self.dlg.handle_event(ev, ctx);
        }
    }
}

/// Build the config-picker dialog. Returns `(view, list_view_id)`; pass the id as
/// the focus target to `exec_view_focused` so the list is active immediately.
pub(crate) fn build(
    items: Vec<PickerItem>,
    selected: Rc<RefCell<Option<usize>>>,
) -> (Box<dyn View>, tv::ViewId) {
    let picker = ConfigPicker::new(items, selected);
    let list_id = picker.list_id;
    (Box::new(picker), list_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::rc::Rc;
    use tvision_rs::{self as tv, Context, Event, FieldValue, Key, KeyEvent};

    fn items() -> Vec<PickerItem> {
        vec![
            PickerItem {
                name: "production".into(),
                description: "prod directory".into(),
                path: PathBuf::from("/etc/edaptor/prod.toml"),
            },
            PickerItem {
                name: "lab".into(),
                description: "local lab".into(),
                path: PathBuf::from("/home/me/.config/edaptor/lab.toml"),
            },
        ]
    }

    fn make_ctx<'a>(
        out: &'a mut VecDeque<Event>,
        timers: &'a mut tv::timer::TimerQueue,
        deferred: &'a mut Vec<tv::Deferred>,
    ) -> Context<'a> {
        Context::new(out, timers, 0, deferred)
    }

    /// reset_current seeds the list, stages index 0, and fills the detail cells
    /// for item 0; a Down event restages the index to 1 and refreshes the detail.
    #[test]
    fn reset_seeds_index_zero_and_down_updates_index_and_detail() {
        let selected: Rc<RefCell<Option<usize>>> = Rc::new(RefCell::new(None));
        let (mut view, _focus) = build(items(), selected.clone());

        assert_eq!(*selected.borrow(), None, "None before reset_current");

        let mut out = VecDeque::new();
        let mut timers = tv::timer::TimerQueue::new();
        let mut deferred: Vec<tv::Deferred> = Vec::new();
        let mut ctx = make_ctx(&mut out, &mut timers, &mut deferred);
        view.reset_current(&mut ctx);
        assert_eq!(*selected.borrow(), Some(0), "Some(0) after reset_current");

        // Detail cells reflect item 0.
        {
            let picker = view
                .as_any_mut()
                .and_then(|a| a.downcast_mut::<ConfigPicker>())
                .expect("downcast ConfigPicker");
            let desc = picker.detail_text(picker.desc_id);
            let path = picker.detail_text(picker.path_id);
            assert_eq!(desc, "prod directory");
            assert_eq!(path, "/etc/edaptor/prod.toml");

            let mut ev = Event::KeyDown(KeyEvent::from(Key::Down));
            picker.handle_event(&mut ev, &mut ctx);
        }
        assert_eq!(*selected.borrow(), Some(1), "Some(1) after Down");

        let picker = view
            .as_any_mut()
            .and_then(|a| a.downcast_mut::<ConfigPicker>())
            .expect("downcast ConfigPicker");
        assert_eq!(
            picker.detail_text(picker.path_id),
            "/home/me/.config/edaptor/lab.toml"
        );
        let _ = FieldValue::Int(0); // keep the import used if asserts change
    }
}
