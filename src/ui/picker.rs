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
/// For value-lookup pickers `value` also carries the scalar attribute (the
/// `value_attr`) committed on Enter; membership pickers leave it `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub dn: String,
    pub label: String,
    /// The scalar to commit for a value-lookup pick (the chosen entry's
    /// `value_attr`); `None` for membership candidates and when absent/empty.
    pub value: Option<String>,
}

/// Pull the scalar `value_attr` from a candidate's attributes (first value).
pub fn pick_value(
    attrs: &std::collections::BTreeMap<String, Vec<String>>,
    value_attr: &str,
) -> Option<String> {
    attrs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(value_attr))
        .and_then(|(_, vs)| vs.first())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// A row as displayed in the picker: a candidate plus whether it is selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibleRow {
    pub candidate: Candidate,
    pub selected: bool,
    /// True when this row's DN was already persisted (saved) on the entry when
    /// the picker opened — lets the UI mark it (e.g. `*`), including a saved
    /// member toggled off (`selected == false`, `saved == true`).
    pub saved: bool,
}

/// Server-side size cap used for picker candidate searches. Shared between
/// `service_picker_search` (where it is passed as `size_limit`) and the
/// `handle_worker_response` intercept (where hitting this count means there may
/// be more matching entries the server did not return).
pub const PICKER_SEARCH_CAP: i32 = 100;

/// Picker state: the current selection (always shown) and the latest results.
#[derive(Debug, Clone, Default)]
pub struct PickerState {
    pub selected: Vec<Candidate>,
    pub results: Vec<Candidate>,
    /// DNs that were already persisted (saved) on the entry when the picker
    /// opened — seeded from the initial `selected`. Used to mark saved rows in
    /// the UI; a saved member toggled off stays here (so it can show as "will
    /// be removed").
    pub saved: Vec<String>,
    pub cursor: usize,
    /// First visible row index — the scroll offset for the candidate list, kept
    /// in sync with `cursor` by the renderer (via `clamp_scroll`) so the cursor
    /// is always on screen even with hundreds of candidates.
    pub scroll: usize,
    /// True while an incremental search term is active. Flips `visible()` so the
    /// fresh search matches lead and the already-selected members trail (easier
    /// to act on a search result); with no term, selected members lead.
    pub search_active: bool,
    /// True when the last search returned exactly `PICKER_SEARCH_CAP` entries —
    /// a heuristic signal that the server may have more matching entries.
    pub truncated: bool,
}

fn same_dn(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

impl PickerState {
    pub fn new(selected: Vec<Candidate>) -> Self {
        let saved = selected.iter().map(|c| c.dn.clone()).collect();
        PickerState {
            selected,
            results: Vec::new(),
            saved,
            cursor: 0,
            scroll: 0,
            search_active: false,
            truncated: false,
        }
    }

    /// Visible rows. Ordering depends on whether a search is active:
    ///
    /// - **No search term** (`search_active == false`): every selected candidate
    ///   first (marked), then each result not already selected. A selected entry
    ///   never vanishes — selection is independent of the results.
    /// - **Search active** (`search_active == true`): the fresh search results
    ///   lead (each marked if it is also selected), then the selected members
    ///   that did NOT match the term trail at the end. This keeps the matches
    ///   you are searching for at the top instead of buried below the existing
    ///   selection.
    ///
    /// Either way, saved-but-removed members not otherwise shown are appended so
    /// the UI can render them as "will be removed".
    pub fn visible(&self) -> Vec<VisibleRow> {
        let is_saved = |dn: &str| self.saved.iter().any(|d| same_dn(d, dn));
        let is_selected = |dn: &str| self.selected.iter().any(|s| same_dn(&s.dn, dn));
        let in_results = |dn: &str| self.results.iter().any(|r| same_dn(&r.dn, dn));
        let mut rows: Vec<VisibleRow> = Vec::new();
        if self.search_active {
            // Results first (marked when also selected)...
            for r in &self.results {
                rows.push(VisibleRow {
                    saved: is_saved(&r.dn),
                    selected: is_selected(&r.dn),
                    candidate: r.clone(),
                });
            }
            // ...then selected members that did not match the search.
            for c in &self.selected {
                if !in_results(&c.dn) {
                    rows.push(VisibleRow {
                        saved: is_saved(&c.dn),
                        candidate: c.clone(),
                        selected: true,
                    });
                }
            }
        } else {
            // Selected first, then results not already selected.
            for c in &self.selected {
                rows.push(VisibleRow {
                    saved: is_saved(&c.dn),
                    candidate: c.clone(),
                    selected: true,
                });
            }
            for r in &self.results {
                if !is_selected(&r.dn) {
                    rows.push(VisibleRow {
                        saved: is_saved(&r.dn),
                        candidate: r.clone(),
                        selected: false,
                    });
                }
            }
        }
        // Saved members that are neither still selected nor in the current
        // results (e.g. toggled off with no active search) still need a row so
        // the UI can show them as "saved, will be removed". Synthesize from the
        // DN — the friendly label is not retained in `saved`.
        for dn in &self.saved {
            let in_selected = self.selected.iter().any(|s| same_dn(&s.dn, dn));
            let in_results = self.results.iter().any(|r| same_dn(&r.dn, dn));
            if !in_selected && !in_results {
                rows.push(VisibleRow {
                    candidate: Candidate {
                        dn: dn.clone(),
                        label: dn.clone(),
                        value: None,
                    },
                    selected: false,
                    saved: true,
                });
            }
        }
        rows
    }

    pub fn set_results(&mut self, results: Vec<Candidate>) {
        self.results = results;
        // A new result set replaces the list — return to the top so the cursor
        // lands on the first row (the first match when a search is active).
        self.cursor = 0;
        self.scroll = 0;
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
            value: None,
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

    #[test]
    fn pick_value_returns_scalar_case_insensitive() {
        use std::collections::BTreeMap;
        let mut attrs = BTreeMap::new();
        attrs.insert("gidNumber".to_string(), vec!["1234".to_string()]);
        // Attr name lookup is case-insensitive.
        assert_eq!(pick_value(&attrs, "gidnumber"), Some("1234".to_string()));
        assert_eq!(pick_value(&attrs, "gidNumber"), Some("1234".to_string()));
    }

    #[test]
    fn pick_value_trims_and_returns_none_when_absent_or_empty() {
        use std::collections::BTreeMap;
        let mut attrs = BTreeMap::new();
        attrs.insert("gidNumber".to_string(), vec!["  42  ".to_string()]);
        attrs.insert("blank".to_string(), vec!["   ".to_string()]);
        assert_eq!(pick_value(&attrs, "gidNumber"), Some("42".to_string()));
        // Absent attribute → None.
        assert_eq!(pick_value(&attrs, "uidNumber"), None);
        // Present but whitespace-only → None.
        assert_eq!(pick_value(&attrs, "blank"), None);
    }

    #[test]
    fn picker_search_cap_is_100() {
        assert_eq!(PICKER_SEARCH_CAP, 100);
    }

    #[test]
    fn new_marks_seeded_selection_as_saved() {
        let state = PickerState::new(vec![c("uid=bob,ou=people")]);
        assert_eq!(state.saved, vec!["uid=bob,ou=people".to_string()]);
    }

    #[test]
    fn visible_flags_saved_rows() {
        // Open with bob saved (seeded selection); search adds carol (not saved).
        let mut p = PickerState::new(vec![c("uid=bob,ou=people")]);
        p.set_results(vec![c("uid=carol,ou=people")]);
        let rows = p.visible();
        let bob = rows
            .iter()
            .find(|r| r.candidate.dn == "uid=bob,ou=people")
            .expect("bob row present");
        assert!(bob.saved, "bob is a saved member");
        assert!(bob.selected, "bob is selected");
        let carol = rows
            .iter()
            .find(|r| r.candidate.dn == "uid=carol,ou=people")
            .expect("carol row present");
        assert!(!carol.saved, "carol was not saved at open");
    }

    #[test]
    fn search_active_puts_matches_first_and_selected_after() {
        // Selected = [A, B]; a search returns [C, D] (neither selected) plus B
        // (which is selected). With search active, results lead and the
        // non-matching selected member (A) trails.
        let mut p = PickerState::new(vec![c("A"), c("B")]);
        p.set_results(vec![c("C"), c("D"), c("B")]);
        p.search_active = true;
        let rows = p.visible();
        let dns: Vec<_> = rows.iter().map(|r| r.candidate.dn.clone()).collect();
        assert_eq!(dns, vec!["C", "D", "B", "A"], "results first, then unmatched selected");
        // B appears in the results block but is marked selected (no duplicate).
        let b = rows.iter().filter(|r| r.candidate.dn == "B").count();
        assert_eq!(b, 1, "B not duplicated");
        assert!(rows.iter().find(|r| r.candidate.dn == "B").unwrap().selected);
        assert!(rows.iter().find(|r| r.candidate.dn == "A").unwrap().selected);
        assert!(!rows.iter().find(|r| r.candidate.dn == "C").unwrap().selected);
    }

    #[test]
    fn no_search_keeps_selected_first() {
        // Same data, search inactive → original order: selected lead.
        let mut p = PickerState::new(vec![c("A"), c("B")]);
        p.set_results(vec![c("C"), c("D"), c("B")]);
        assert!(!p.search_active, "defaults inactive");
        let dns: Vec<_> = p.visible().iter().map(|r| r.candidate.dn.clone()).collect();
        assert_eq!(dns, vec!["A", "B", "C", "D"], "selected first, then unselected results");
    }

    #[test]
    fn set_results_resets_cursor_and_scroll_to_top() {
        let mut p = PickerState::new(vec![]);
        p.set_results(vec![c("A"), c("B"), c("C")]);
        p.cursor = 2;
        p.scroll = 1;
        p.set_results(vec![c("X"), c("Y")]);
        assert_eq!(p.cursor, 0, "cursor returns to top on new results");
        assert_eq!(p.scroll, 0, "scroll returns to top on new results");
    }

    #[test]
    fn toggling_off_a_saved_member_keeps_saved_true() {
        // Open with bob saved, then toggle bob off.
        let mut p = PickerState::new(vec![c("uid=bob,ou=people")]);
        p.cursor = 0; // bob
        p.toggle_cursor();
        let rows = p.visible();
        let bob = rows
            .iter()
            .find(|r| r.candidate.dn == "uid=bob,ou=people")
            .expect("bob row still present (from results or saved)");
        assert!(!bob.selected, "bob is no longer selected");
        assert!(bob.saved, "bob remains flagged as saved");
    }
}
