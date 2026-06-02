//! Pure state for the value-editor's picker mode: a current selection that is
//! ALWAYS shown, merged with the latest (size-capped) search results. No ratatui.

use std::collections::BTreeMap;

/// RFC 4515 filter-value escaping for the five special bytes.
pub fn escape_filter(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '*' => out.push_str(r"\2a"),
            '(' => out.push_str(r"\28"),
            ')' => out.push_str(r"\29"),
            '\\' => out.push_str(r"\5c"),
            '\0' => out.push_str(r"\00"),
            _ => out.push(ch),
        }
    }
    out
}

/// Build the candidate search filter. Empty `term` → objectClass only; otherwise
/// AND each objectClass with an OR of `attr=*term*` over each search attribute.
///
/// Single class, no term: `(objectClass=X)` — bare, no outer `(&...)`.
/// Otherwise: `(&(objectClass=a)(objectClass=b)(|(attr=*term*)))`.
pub fn build_member_filter(
    object_classes: &[String],
    search_attrs: &[String],
    term: &str,
) -> String {
    let oc_filters: String = object_classes
        .iter()
        .map(|oc| format!("(objectClass={})", escape_filter(oc)))
        .collect();
    let has_term_group = !term.is_empty() && !search_attrs.is_empty();
    // Single class, no term group: return bare filter (preserves legacy shape).
    if object_classes.len() == 1 && !has_term_group {
        return oc_filters;
    }
    if has_term_group {
        let esc = escape_filter(term);
        let ors: String = search_attrs
            .iter()
            .map(|a| format!("({a}=*{esc}*)"))
            .collect();
        format!("(&{oc_filters}(|{ors}))")
    } else {
        format!("(&{oc_filters})")
    }
}

/// A candidate's display label: first `cn` value, else the raw DN.
pub fn candidate_label(dn: &str, attrs: &BTreeMap<String, Vec<String>>) -> String {
    attrs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("cn"))
        .and_then(|(_, v)| v.first().cloned())
        .unwrap_or_else(|| dn.to_string())
}

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

/// Server-side size cap used for picker candidate searches. Shared between
/// `service_picker_search` (where it is passed as `size_limit`) and the
/// `handle_worker_response` intercept (where hitting this count means there may
/// be more matching entries the server did not return).
pub const PICKER_SEARCH_CAP: i32 = 20;

/// Picker state: the current selection (always shown) and the latest results.
#[derive(Debug, Clone, Default)]
pub struct PickerState {
    pub selected: Vec<Candidate>,
    pub results: Vec<Candidate>,
    pub cursor: usize,
    /// True when the last search returned exactly `PICKER_SEARCH_CAP` entries —
    /// a heuristic signal that the server may have more matching entries.
    pub truncated: bool,
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
            truncated: false,
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

    #[test]
    fn move_cursor_advances_and_stops_at_last() {
        let mut p = PickerState::new(vec![c("A")]);
        p.set_results(vec![c("B"), c("C")]);
        p.move_cursor(1);
        assert_eq!(p.cursor, 1);
        p.move_cursor(1);
        assert_eq!(p.cursor, 2);
        p.move_cursor(1);
        assert_eq!(p.cursor, 2);
    }

    #[test]
    fn escapes_filter_specials() {
        assert_eq!(escape_filter("a*b(c)\\d"), r"a\2ab\28c\29\5cd");
    }

    #[test]
    fn builds_or_filter_with_objectclass_and_term() {
        let f = build_member_filter(
            &["inetOrgPerson".into()],
            &["uid".into(), "cn".into()],
            "ann",
        );
        assert_eq!(f, "(&(objectClass=inetOrgPerson)(|(uid=*ann*)(cn=*ann*)))");
    }

    #[test]
    fn empty_term_filters_objectclass_only() {
        let f = build_member_filter(&["groupOfNames".into()], &["cn".into()], "");
        assert_eq!(f, "(objectClass=groupOfNames)");
    }

    #[test]
    fn empty_search_attrs_with_term_returns_oc_only() {
        let f = build_member_filter(&["inetOrgPerson".into()], &[], "ann");
        assert_eq!(f, "(objectClass=inetOrgPerson)");
    }

    #[test]
    fn member_filter_ands_multiple_object_classes() {
        let f = build_member_filter(
            &["posixAccount".into(), "inetOrgPerson".into()],
            &["cn".into(), "uid".into()],
            "ali",
        );
        assert!(f.starts_with("(&(objectClass=posixAccount)(objectClass=inetOrgPerson)"));
        assert!(f.contains("(cn=*ali*)"));
        assert!(f.contains("(uid=*ali*)"));
    }

    #[test]
    fn member_filter_single_class_unchanged_shape() {
        let f = build_member_filter(&["inetOrgPerson".into()], &["cn".into()], "bob");
        assert_eq!(f, "(&(objectClass=inetOrgPerson)(|(cn=*bob*)))");
    }

    #[test]
    fn label_prefers_cn_then_dn() {
        use std::collections::BTreeMap;
        let mut attrs = BTreeMap::new();
        attrs.insert("cn".to_string(), vec!["Ann Smith".to_string()]);
        assert_eq!(candidate_label("uid=ann,ou=people", &attrs), "Ann Smith");
        assert_eq!(
            candidate_label("uid=bob,ou=people", &BTreeMap::new()),
            "uid=bob,ou=people"
        );
    }

    #[test]
    fn truncated_defaults_false_and_is_settable() {
        let mut p = PickerState::new(vec![c("A")]);
        assert!(!p.truncated, "truncated should default to false");
        p.truncated = true;
        assert!(p.truncated, "truncated should be settable");
        // Default trait also produces false.
        let p2 = PickerState::default();
        assert!(
            !p2.truncated,
            "Default impl should also set truncated=false"
        );
    }
}
