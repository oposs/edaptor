//! `Shuttle` — a domain-free two-list "transfer" widget (work in progress).
//!
//! This is the clean re-incubation of `dual_list.rs`: instead of a controller
//! that reaches into the host's `Dialog`, the `Shuttle` will *be* a `View`
//! embedding a `Group` and owning its children, notifying the owner by
//! broadcast. The pure column logic — move / de-dup / lock — lives in
//! [`ShuttleModel`], which is tvision-free and unit-testable without a `Dialog`.

/// One row in either column. Domain-free: `key` is the host's stable identity
/// (a DN, an object-class name, …) used for de-duplication; `label` is the
/// display text; `locked` blocks moving the row out of the Selected column.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(dead_code)] // fields wired in as the model grows
pub(crate) struct ShuttleRow {
    pub key: String,
    pub label: String,
    pub locked: bool,
}

/// The pure, tvision-free model of the two columns. The `Shuttle` `View` will
/// wrap one of these plus the child views; all move/de-dup/lock logic lives
/// here so it is exercised without a `Dialog`.
#[derive(Default)]
#[allow(dead_code)] // grows under TDD; trimmed before the View is wired up
pub(crate) struct ShuttleModel {
    available: Vec<ShuttleRow>,
    selected: Vec<ShuttleRow>,
}

#[allow(dead_code)]
impl ShuttleModel {
    fn available(&self) -> &[ShuttleRow] {
        &self.available
    }

    fn selected(&self) -> &[ShuttleRow] {
        &self.selected
    }

    fn set_available(&mut self, rows: Vec<ShuttleRow>) {
        self.available = rows;
    }

    fn set_selected(&mut self, rows: Vec<ShuttleRow>) {
        self.selected = rows;
    }

    /// Move `available[idx]` into Selected. No-op (returns `false`) when `idx`
    /// is out of range or the key is already selected (case-insensitive).
    /// Returns `true` when the Selected set changed.
    fn move_in(&mut self, idx: usize) -> bool {
        let Some(row) = self.available.get(idx) else {
            return false;
        };
        if self.is_selected(&row.key) {
            return false;
        }
        let row = self.available[idx].clone();
        self.selected.push(row);
        true
    }

    /// Whether `key` is already in the Selected set (case-insensitive).
    fn is_selected(&self, key: &str) -> bool {
        self.selected.iter().any(|r| r.key.eq_ignore_ascii_case(key))
    }

    /// Remove `selected[idx]` from Selected. No-op (returns `false`) when `idx`
    /// is out of range or the row is `locked`. Returns `true` when the Selected
    /// set changed.
    fn move_out(&mut self, idx: usize) -> bool {
        let Some(row) = self.selected.get(idx) else {
            return false;
        };
        if row.locked {
            return false;
        }
        self.selected.remove(idx);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(key: &str) -> ShuttleRow {
        ShuttleRow {
            key: key.into(),
            label: key.into(),
            locked: false,
        }
    }

    fn locked(key: &str) -> ShuttleRow {
        ShuttleRow {
            key: key.into(),
            label: key.into(),
            locked: true,
        }
    }

    fn keys(rows: &[ShuttleRow]) -> Vec<&str> {
        rows.iter().map(|r| r.key.as_str()).collect()
    }

    #[test]
    fn move_in_appends_available_row_to_selected() {
        let mut m = ShuttleModel::default();
        m.set_available(vec![row("a"), row("b")]);
        assert!(
            m.move_in(0),
            "moving an available row must change the Selected set"
        );
        assert_eq!(keys(m.selected()), ["a"]);
    }

    #[test]
    fn move_in_is_deduped_case_insensitively() {
        let mut m = ShuttleModel::default();
        m.set_available(vec![row("Alice")]);
        m.set_selected(vec![row("alice")]);
        assert!(
            !m.move_in(0),
            "a key already selected (any case) must not be added again"
        );
        assert_eq!(keys(m.selected()), ["alice"], "no duplicate appended");
    }

    #[test]
    fn move_out_removes_a_removable_selected_row() {
        let mut m = ShuttleModel::default();
        m.set_selected(vec![row("x"), row("y")]);
        assert!(
            m.move_out(0),
            "removing an unlocked row must change the Selected set"
        );
        assert_eq!(keys(m.selected()), ["y"]);
    }

    #[test]
    fn move_out_rejects_a_locked_row() {
        let mut m = ShuttleModel::default();
        m.set_selected(vec![locked("top")]);
        assert!(
            !m.move_out(0),
            "a locked row must not be removable"
        );
        assert_eq!(keys(m.selected()), ["top"], "locked row stays put");
    }

    #[test]
    fn out_of_range_moves_are_noops() {
        let mut m = ShuttleModel::default();
        m.set_available(vec![row("a")]);
        m.set_selected(vec![row("b")]);
        assert!(!m.move_in(9), "out-of-range move_in is a no-op");
        assert!(!m.move_out(9), "out-of-range move_out is a no-op");
        assert_eq!(keys(m.available()), ["a"]);
        assert_eq!(keys(m.selected()), ["b"]);
    }
}
