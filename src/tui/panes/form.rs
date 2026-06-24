//! Read-only entry form pane: one InputLine row per field, `label: value`.

use tvision_rs::{self as tv, delegate, Context, Event, FieldValue, Group, InputLine, Rect, View};

use crate::tui::widget::present_field;
use crate::tui::{Shared, REFRESH};
use crate::workflows::form_model::FormModel;

const FORM_ROWS: usize = 32;

/// Render a FormModel into `"label: value"` strings (MUST marked with `*`).
fn render_rows(model: &FormModel) -> Vec<String> {
    model
        .fields
        .iter()
        .map(|f| {
            let marker = if f.is_must { " *" } else { "" };
            format!("{}{}: {}", f.label, marker, present_field(f))
        })
        .collect()
}

pub struct FormPane {
    group: Group,
    rows: Vec<tv::ViewId>,
    state: Shared,
}

impl FormPane {
    pub fn new(bounds: Rect, state: Shared) -> Self {
        let mut group = Group::new(bounds);
        let w = bounds.b.x - bounds.a.x;
        let mut rows = Vec::new();
        for i in 0..FORM_ROWS {
            let y = i as i32;
            let il = InputLine::with_limit(Rect::new(0, y, w, y + 1), 1024);
            rows.push(group.insert(Box::new(il)));
        }
        FormPane { group, rows, state }
    }
}

#[delegate(to = group)]
impl View for FormPane {
    fn as_any_mut(&mut self) -> Option<&mut dyn core::any::Any> {
        Some(self)
    }

    fn handle_event(&mut self, ev: &mut Event, ctx: &mut Context) {
        let is_refresh = matches!(ev, Event::Broadcast { command, .. } if *command == REFRESH);
        if is_refresh && self.state.borrow().form_dirty {
            let lines: Vec<String> = {
                let mut st = self.state.borrow_mut();
                st.form_dirty = false;
                st.form.as_ref().map(render_rows).unwrap_or_default()
            }; // borrow dropped before mutating children
            for (i, &id) in self.rows.iter().enumerate() {
                let text = lines.get(i).cloned().unwrap_or_default();
                if let Some(child) = self.group.child_mut(id) {
                    child.set_value(FieldValue::Text(text));
                }
            }
        }
        self.group.handle_event(ev, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::FieldKind;
    use crate::workflows::form_model::{FormField, WidgetSpec};

    #[test]
    fn test_render_rows_labels_and_must_marker() {
        let model = FormModel {
            title: "cn=a,dc=x".into(),
            fields: vec![
                FormField {
                    label: "cn".into(),
                    kind: FieldKind::Text,
                    is_must: true,
                    values: vec!["a".into()],
                    widget: WidgetSpec::ReadOnlyText,
                },
                FormField {
                    label: "mail".into(),
                    kind: FieldKind::Text,
                    is_must: false,
                    values: vec!["a@x".into(), "b@x".into()],
                    widget: WidgetSpec::ReadOnlyText,
                },
            ],
        };
        let rows = render_rows(&model);
        assert_eq!(rows[0], "cn *: a");
        assert_eq!(rows[1], "mail: \u{2039}2 values\u{203a}");
    }
}
