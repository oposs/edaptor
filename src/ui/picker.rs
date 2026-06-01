//! Pure state for the value-editor's picker mode: a current selection that is
//! ALWAYS shown, merged with the latest (size-capped) search results. No ratatui.

/// One candidate entry: the DN that is stored, and the human label that is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub dn: String,
    pub label: String,
}

/// A row as displayed in the picker: a candidate plus whether it is selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleRow {
    pub candidate: Candidate,
    pub selected: bool,
}

/// Picker state: the current selection (always shown) and the latest results.
#[derive(Debug, Clone, Default)]
pub struct PickerState {
    pub selected: Vec<Candidate>,
    pub results: Vec<Candidate>,
    pub cursor: usize,
}

fn same_dn(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

impl PickerState {
    pub fn new(selected: Vec<Candidate>) -> Self {
        PickerState {
            selected,
            results: Vec::new(),
            cursor: 0,
        }
    }

    /// Visible rows: every selected candidate (marked), then each result not
    /// already selected (unmarked). This is why a selected entry never vanishes
    /// while filtering — selection is independent of the search results.
    pub fn visible(&self) -> Vec<VisibleRow> {
        let mut rows: Vec<VisibleRow> = self
            .selected
            .iter()
            .map(|c| VisibleRow {
                candidate: c.clone(),
                selected: true,
            })
            .collect();
        for r in &self.results {
            if !self.selected.iter().any(|s| same_dn(&s.dn, &r.dn)) {
                rows.push(VisibleRow {
                    candidate: r.clone(),
                    selected: false,
                });
            }
        }
        rows
    }

    pub fn set_results(&mut self, results: Vec<Candidate>) {
        self.results = results;
        let n = self.visible().len();
        if self.cursor >= n {
            self.cursor = n.saturating_sub(1);
        }
    }

    pub fn move_cursor(&mut self, delta: i32) {
        let n = self.visible().len();
        if n == 0 {
            self.cursor = 0;
            return;
        }
        let next = (self.cursor as i32 + delta).clamp(0, n as i32 - 1);
        self.cursor = next as usize;
    }

    /// Toggle the cursor row's membership in the selection.
    pub fn toggle_cursor(&mut self) {
        let rows = self.visible();
        let Some(row) = rows.get(self.cursor) else {
            return;
        };
        let dn = row.candidate.dn.clone();
        if let Some(pos) = self.selected.iter().position(|s| same_dn(&s.dn, &dn)) {
            self.selected.remove(pos);
        } else {
            self.selected.push(row.candidate.clone());
        }
        let n = self.visible().len();
        if self.cursor >= n {
            self.cursor = n.saturating_sub(1);
        }
    }

    pub fn selected_dns(&self) -> Vec<String> {
        self.selected.iter().map(|c| c.dn.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(dn: &str) -> Candidate {
        Candidate {
            dn: dn.into(),
            label: dn.into(),
        }
    }

    #[test]
    fn selected_stays_visible_when_results_exclude_it() {
        // Seed selection = [A]; a search returns only [B] (A does not match).
        let mut p = PickerState::new(vec![c("A")]);
        p.set_results(vec![c("B")]);
        let dns: Vec<_> = p.visible().iter().map(|r| r.candidate.dn.clone()).collect();
        assert_eq!(dns, vec!["A".to_string(), "B".to_string()]); // A still present
        assert!(p.visible()[0].selected);
        assert!(!p.visible()[1].selected);
    }

    #[test]
    fn results_already_selected_are_not_duplicated() {
        let mut p = PickerState::new(vec![c("A")]);
        p.set_results(vec![c("A"), c("B")]);
        let dns: Vec<_> = p.visible().iter().map(|r| r.candidate.dn.clone()).collect();
        assert_eq!(dns, vec!["A".to_string(), "B".to_string()]);
    }

    #[test]
    fn toggle_adds_and_removes() {
        let mut p = PickerState::new(vec![]);
        p.set_results(vec![c("A"), c("B")]);
        p.cursor = 0; // A
        p.toggle_cursor();
        assert_eq!(p.selected_dns(), vec!["A".to_string()]);
        // A now sorts into the selected block at index 0; toggle it off again.
        p.cursor = 0;
        p.toggle_cursor();
        assert!(p.selected_dns().is_empty());
    }

    #[test]
    fn cursor_clamps() {
        let mut p = PickerState::new(vec![c("A")]);
        p.move_cursor(5);
        assert_eq!(p.cursor, 0); // only one visible row
        p.move_cursor(-5);
        assert_eq!(p.cursor, 0);
    }
}
