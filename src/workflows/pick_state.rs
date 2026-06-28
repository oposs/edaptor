//! Neutral selection state for picker and membership widgets.
//!
//! Framework-free pure logic backing the tvision picker and membership widgets.
//! No tvision_rs, no crate::ui — pure domain logic.

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

/// One candidate: the real entry `dn` (fan-out target; also the key/store value
/// when `store = dn`), the human `label` shown, and `store_value` — the scalar
/// committed into the field and the identity key for dedupe/toggle/selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub dn: String,
    pub label: String,
    /// The value committed for this pick (the DN for `store = dn`, else the
    /// chosen `store` attribute). Also the identity key.
    pub store_value: String,
}

/// Pull the scalar `value_attr` from a candidate's attributes (first value).
pub fn pick_value(attrs: &BTreeMap<String, Vec<String>>, value_attr: &str) -> Option<String> {
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
    /// True when this row's store value was already persisted (saved) on the
    /// entry when the picker opened — lets the UI mark it (e.g. `*`), including a
    /// saved member toggled off (`selected == false`, `saved == true`).
    pub saved: bool,
}

/// Server-side size cap used for picker candidate searches. Shared between
/// `service_picker_search` (where it is passed as `size_limit`) and the
/// `handle_worker_response` intercept (where hitting this count means there may
/// be more matching entries the server did not return).
pub const PICKER_SEARCH_CAP: i32 = 100;

/// Picker selection state: the current selection (always shown) and the latest results.
#[derive(Debug, Clone, Default)]
pub struct PickState {
    pub selected: Vec<Candidate>,
    pub results: Vec<Candidate>,
    /// Store values that were already persisted (saved) on the entry when the
    /// picker opened — seeded from the initial `selected` (the DN for `store =
    /// dn`). Used to mark saved rows in the UI; a saved member toggled off stays
    /// here (so it can show as "will be removed").
    pub saved: Vec<String>,
    pub cursor: usize,
    /// First visible row index — the scroll offset for the candidate list, kept
    /// in sync with `cursor` by the renderer so the cursor is always on screen.
    pub scroll: usize,
    /// True while an incremental search term is active. Flips `visible()` so the
    /// fresh search matches lead and the already-selected members trail.
    pub search_active: bool,
    /// True when the last search returned exactly `PICKER_SEARCH_CAP` entries —
    /// a heuristic signal that the server may have more matching entries.
    pub truncated: bool,
    /// True ⇒ keys (store values) compare case-insensitively (DN store); false ⇒
    /// exact (scalar store). Set at construction from the binding's `StoreKey`.
    pub key_ci: bool,
}

/// Compare two store-value keys. Case-insensitive for DN stores (`ci == true`),
/// exact otherwise. A free function so closures in `visible()` can call it
/// without borrowing `&self` while `self.saved`/`selected`/`results` are borrowed.
fn same_key(ci: bool, a: &str, b: &str) -> bool {
    if ci {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

impl PickState {
    pub fn new(selected: Vec<Candidate>, key_ci: bool) -> Self {
        let saved = selected.iter().map(|c| c.store_value.clone()).collect();
        PickState {
            selected,
            results: Vec::new(),
            saved,
            cursor: 0,
            scroll: 0,
            search_active: false,
            truncated: false,
            key_ci,
        }
    }

    /// Visible rows. Ordering depends on whether a search is active:
    ///
    /// - **No search term** (`search_active == false`): every selected candidate
    ///   first (marked), then each result not already selected.
    /// - **Search active** (`search_active == true`): the fresh search results
    ///   lead (each marked if it is also selected), then the selected members
    ///   that did NOT match the term trail at the end.
    ///
    /// Either way, saved-but-removed members not otherwise shown are appended so
    /// the UI can render them as "will be removed".
    pub fn visible(&self) -> Vec<VisibleRow> {
        let ci = self.key_ci;
        let is_saved = |sv: &str| self.saved.iter().any(|d| same_key(ci, d, sv));
        let is_selected = |sv: &str| {
            self.selected
                .iter()
                .any(|s| same_key(ci, &s.store_value, sv))
        };
        let in_results = |sv: &str| {
            self.results
                .iter()
                .any(|r| same_key(ci, &r.store_value, sv))
        };
        let mut rows: Vec<VisibleRow> = Vec::new();
        if self.search_active {
            // Results first (marked when also selected)...
            for r in &self.results {
                rows.push(VisibleRow {
                    saved: is_saved(&r.store_value),
                    selected: is_selected(&r.store_value),
                    candidate: r.clone(),
                });
            }
            // ...then selected members that did not match the search.
            for c in &self.selected {
                if !in_results(&c.store_value) {
                    rows.push(VisibleRow {
                        saved: is_saved(&c.store_value),
                        candidate: c.clone(),
                        selected: true,
                    });
                }
            }
        } else {
            // Selected first, then results not already selected.
            for c in &self.selected {
                rows.push(VisibleRow {
                    saved: is_saved(&c.store_value),
                    candidate: c.clone(),
                    selected: true,
                });
            }
            for r in &self.results {
                if !is_selected(&r.store_value) {
                    rows.push(VisibleRow {
                        saved: is_saved(&r.store_value),
                        candidate: r.clone(),
                        selected: false,
                    });
                }
            }
        }
        // Saved members that are neither still selected nor in the current
        // results (e.g. toggled off with no active search) still need a row so
        // the UI can show them as "saved, will be removed". Synthesize from the
        // saved store value — the friendly label is not retained in `saved`.
        for sv in &self.saved {
            let in_selected = self
                .selected
                .iter()
                .any(|s| same_key(ci, &s.store_value, sv));
            let in_results = self
                .results
                .iter()
                .any(|r| same_key(ci, &r.store_value, sv));
            if !in_selected && !in_results {
                rows.push(VisibleRow {
                    candidate: Candidate {
                        dn: sv.clone(),
                        label: sv.clone(),
                        store_value: sv.clone(),
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
        let sv = row.candidate.store_value.clone();
        let ci = self.key_ci;
        if let Some(pos) = self
            .selected
            .iter()
            .position(|s| same_key(ci, &s.store_value, &sv))
        {
            self.selected.remove(pos);
        } else {
            self.selected.push(row.candidate.clone());
        }
        let n = self.visible().len();
        if self.cursor >= n {
            self.cursor = n.saturating_sub(1);
        }
    }

    /// Store values of the current selection — what a direct-write commit writes.
    pub fn selected_values(&self) -> Vec<String> {
        self.selected
            .iter()
            .map(|c| c.store_value.clone())
            .collect()
    }

    /// Real entry DNs of the current selection — fan-out targets (`store = dn`).
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
            store_value: dn.into(),
        }
    }

    // --- selected-first ordering (no search active) ---

    #[test]
    fn no_search_keeps_selected_first() {
        let mut p = PickState::new(vec![c("A"), c("B")], true);
        p.set_results(vec![c("C"), c("D"), c("B")]);
        assert!(!p.search_active, "search_active defaults false");
        let dns: Vec<_> = p.visible().iter().map(|r| r.candidate.dn.clone()).collect();
        assert_eq!(
            dns,
            vec!["A", "B", "C", "D"],
            "selected first, then unselected results"
        );
    }

    #[test]
    fn selected_stays_visible_when_results_exclude_it() {
        let mut p = PickState::new(vec![c("A")], true);
        p.set_results(vec![c("B")]);
        let rows = p.visible();
        let dns: Vec<_> = rows.iter().map(|r| r.candidate.dn.clone()).collect();
        assert_eq!(dns, vec!["A", "B"]);
        assert!(rows[0].selected);
        assert!(!rows[1].selected);
    }

    #[test]
    fn results_already_selected_are_not_duplicated() {
        let mut p = PickState::new(vec![c("A")], true);
        p.set_results(vec![c("A"), c("B")]);
        let dns: Vec<_> = p.visible().iter().map(|r| r.candidate.dn.clone()).collect();
        assert_eq!(dns, vec!["A", "B"]);
    }

    // --- DN-case-insensitive toggle ---

    #[test]
    fn dn_store_keys_case_insensitively() {
        let mut p = PickState::new(vec![c("UID=Bob,OU=people")], true);
        p.set_results(vec![c("uid=bob,ou=people")]);
        assert_eq!(p.visible().len(), 1, "same DN different case = 1 row");
    }

    #[test]
    fn toggle_adds_and_removes() {
        let mut p = PickState::new(vec![], true);
        p.set_results(vec![c("A"), c("B")]);
        p.cursor = 0;
        p.toggle_cursor();
        assert_eq!(p.selected_dns(), vec!["A".to_string()]);
        p.cursor = 0; // A is now in the selected block at top
        p.toggle_cursor();
        assert!(p.selected_dns().is_empty());
    }

    // --- saved-but-removed marker ---

    #[test]
    fn toggling_off_a_saved_member_keeps_saved_true() {
        let mut p = PickState::new(vec![c("uid=bob,ou=people")], true);
        p.cursor = 0;
        p.toggle_cursor();
        let rows = p.visible();
        let bob = rows
            .iter()
            .find(|r| r.candidate.dn == "uid=bob,ou=people")
            .expect("bob row still present");
        assert!(!bob.selected, "bob no longer selected");
        assert!(bob.saved, "bob remains flagged as saved");
    }

    #[test]
    fn new_marks_seeded_selection_as_saved() {
        let state = PickState::new(vec![c("uid=bob,ou=people")], true);
        assert_eq!(state.saved, vec!["uid=bob,ou=people".to_string()]);
    }

    // --- search-active ordering ---

    #[test]
    fn search_active_puts_matches_first_and_selected_after() {
        let mut p = PickState::new(vec![c("A"), c("B")], true);
        p.set_results(vec![c("C"), c("D"), c("B")]);
        p.search_active = true;
        let rows = p.visible();
        let dns: Vec<_> = rows.iter().map(|r| r.candidate.dn.clone()).collect();
        assert_eq!(
            dns,
            vec!["C", "D", "B", "A"],
            "results first, then unmatched selected"
        );
        let b_count = rows.iter().filter(|r| r.candidate.dn == "B").count();
        assert_eq!(b_count, 1, "B not duplicated");
        assert!(
            rows.iter()
                .find(|r| r.candidate.dn == "B")
                .unwrap()
                .selected
        );
        assert!(
            rows.iter()
                .find(|r| r.candidate.dn == "A")
                .unwrap()
                .selected
        );
        assert!(
            !rows
                .iter()
                .find(|r| r.candidate.dn == "C")
                .unwrap()
                .selected
        );
    }

    // --- scalar (exact) store ---

    #[test]
    fn scalar_store_keys_by_value_exact() {
        let mut p = PickState::new(
            vec![Candidate {
                dn: "alice".into(),
                label: "alice".into(),
                store_value: "alice".into(),
            }],
            false,
        );
        p.set_results(vec![Candidate {
            dn: "uid=Alice,ou=people".into(),
            label: "Alice".into(),
            store_value: "Alice".into(),
        }]);
        let vals: Vec<_> = p
            .visible()
            .iter()
            .map(|r| r.candidate.store_value.clone())
            .collect();
        assert_eq!(vals, vec!["alice".to_string(), "Alice".to_string()]);
    }

    #[test]
    fn selected_values_returns_store_values() {
        let mut p = PickState::new(vec![], false);
        p.set_results(vec![
            Candidate {
                dn: "uid=a,o=x".into(),
                label: "A".into(),
                store_value: "1001".into(),
            },
            Candidate {
                dn: "uid=b,o=x".into(),
                label: "B".into(),
                store_value: "1002".into(),
            },
        ]);
        p.cursor = 0;
        p.toggle_cursor();
        p.cursor = 1;
        p.toggle_cursor();
        let mut vals = p.selected_values();
        vals.sort();
        assert_eq!(vals, vec!["1001".to_string(), "1002".to_string()]);
    }

    // --- cursor / scroll ---

    #[test]
    fn cursor_clamps() {
        let mut p = PickState::new(vec![c("A")], true);
        p.move_cursor(5);
        assert_eq!(p.cursor, 0);
        p.move_cursor(-5);
        assert_eq!(p.cursor, 0);
    }

    #[test]
    fn move_cursor_advances_and_stops_at_last() {
        let mut p = PickState::new(vec![c("A")], true);
        p.set_results(vec![c("B"), c("C")]);
        p.move_cursor(1);
        assert_eq!(p.cursor, 1);
        p.move_cursor(1);
        assert_eq!(p.cursor, 2);
        p.move_cursor(1);
        assert_eq!(p.cursor, 2);
    }

    #[test]
    fn set_results_resets_cursor_and_scroll_to_top() {
        let mut p = PickState::new(vec![], true);
        p.set_results(vec![c("A"), c("B"), c("C")]);
        p.cursor = 2;
        p.scroll = 1;
        p.set_results(vec![c("X"), c("Y")]);
        assert_eq!(p.cursor, 0);
        assert_eq!(p.scroll, 0);
    }

    // --- misc ---

    #[test]
    fn truncated_defaults_false_and_is_settable() {
        let mut p = PickState::new(vec![c("A")], true);
        assert!(!p.truncated);
        p.truncated = true;
        assert!(p.truncated);
        let p2 = PickState::default();
        assert!(!p2.truncated);
    }

    #[test]
    fn picker_search_cap_is_100() {
        assert_eq!(PICKER_SEARCH_CAP, 100);
    }

    // --- filter helpers ---

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
    fn pick_value_returns_scalar_case_insensitive() {
        use std::collections::BTreeMap;
        let mut attrs = BTreeMap::new();
        attrs.insert("gidNumber".to_string(), vec!["1234".to_string()]);
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
        assert_eq!(pick_value(&attrs, "uidNumber"), None);
        assert_eq!(pick_value(&attrs, "blank"), None);
    }

    #[test]
    fn visible_flags_saved_rows() {
        let mut p = PickState::new(vec![c("uid=bob,ou=people")], true);
        p.set_results(vec![c("uid=carol,ou=people")]);
        let rows = p.visible();
        let bob = rows
            .iter()
            .find(|r| r.candidate.dn == "uid=bob,ou=people")
            .expect("bob row present");
        assert!(bob.saved);
        assert!(bob.selected);
        let carol = rows
            .iter()
            .find(|r| r.candidate.dn == "uid=carol,ou=people")
            .expect("carol row present");
        assert!(!carol.saved);
    }
}
