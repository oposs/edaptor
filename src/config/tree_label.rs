//! DIT-tree (pane 1) branch-label rules: compile config rules (or a built-in
//! default set), discover the attributes their templates reference, evaluate the
//! first matching rule per node, and width-fit the rendered label so the RDN
//! survives longest. Pane-2 leaf labels and the `‹self›` row are NOT handled
//! here — see `src/ui/app/structure_view.rs`.

use crate::config::label::{parse_label_template, LabelSeg, Piece};
use crate::config::TreeConfig;
use std::collections::BTreeMap;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// A compiled `[[tree.label]]` rule: required attribute names (`when`) plus the
/// parsed template segments.
#[derive(Debug, Clone)]
pub struct CompiledTreeRule {
    pub when: Vec<String>,
    pub template: Vec<LabelSeg>,
}

/// The built-in default rule set used when `[[tree.label]]` is absent:
/// `{rdn} ({cn})` if cn present · else `{rdn} ({description})` if description
/// present · else `{rdn}`.
pub fn default_tree_rules() -> Vec<CompiledTreeRule> {
    vec![
        CompiledTreeRule {
            when: vec!["cn".to_string()],
            template: parse_label_template("{rdn} ({cn})"),
        },
        CompiledTreeRule {
            when: vec!["description".to_string()],
            template: parse_label_template("{rdn} ({description})"),
        },
        CompiledTreeRule {
            when: vec![],
            template: parse_label_template("{rdn}"),
        },
    ]
}

/// Compile config rules into [`CompiledTreeRule`]s, substituting the default set
/// when the config list is empty.
pub fn compile_tree_rules(cfg: &TreeConfig) -> Vec<CompiledTreeRule> {
    if cfg.label.is_empty() {
        return default_tree_rules();
    }
    cfg.label
        .iter()
        .map(|r| CompiledTreeRule {
            when: r.when.clone(),
            template: parse_label_template(&r.template),
        })
        .collect()
}

/// Union of every `{field}` referenced by any rule's template, **excluding the
/// reserved `rdn`**, deduped case-insensitively. Unioned into the structure
/// scan-attrs so templated attributes are actually fetched.
pub fn tree_template_attrs(rules: &[CompiledTreeRule]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for rule in rules {
        for attr in crate::config::label::template_attrs(&rule.template) {
            if attr.eq_ignore_ascii_case("rdn") {
                continue;
            }
            if !out.iter().any(|a| a.eq_ignore_ascii_case(&attr)) {
                out.push(attr);
            }
        }
    }
    out
}

/// A space-delimited run of [`Piece`]s — the unit the trimmer shrinks or drops.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub pieces: Vec<Piece>,
}

impl Segment {
    /// The segment's full rendered text (all pieces concatenated).
    pub fn text(&self) -> String {
        self.pieces.iter().map(|p| p.text.as_str()).collect()
    }
}

/// Split a flat piece list into space-delimited [`Segment`]s. Spaces are
/// separators (not retained). A piece's text is split at ASCII spaces; each
/// sub-run keeps the piece's `from_field` provenance. Empty sub-runs are dropped.
fn split_into_segments(pieces: Vec<Piece>) -> Vec<Segment> {
    let mut segments: Vec<Segment> = Vec::new();
    let mut current: Vec<Piece> = Vec::new();
    for piece in pieces {
        let mut first = true;
        for part in piece.text.split(' ') {
            if !first && !current.is_empty() {
                segments.push(Segment {
                    pieces: std::mem::take(&mut current),
                });
            }
            first = false;
            if !part.is_empty() {
                current.push(Piece {
                    text: part.to_string(),
                    from_field: piece.from_field,
                });
            }
        }
    }
    if !current.is_empty() {
        segments.push(Segment { pieces: current });
    }
    segments
}

/// Pick the first rule whose `when` attributes are all present (case-insensitive,
/// non-empty first value) and render it into segments. `{rdn}` binds to `rdn`.
/// If no rule matches (misconfigured list with no fallback), show just the RDN.
pub fn eval_tree_label(
    rules: &[CompiledTreeRule],
    attrs: &BTreeMap<String, Vec<String>>,
    rdn: &str,
) -> Vec<Segment> {
    for rule in rules {
        if rule.when.iter().all(|w| present(attrs, w)) {
            let pieces = crate::config::label::render_pieces(&rule.template, attrs, rdn);
            return split_into_segments(pieces);
        }
    }
    split_into_segments(vec![Piece {
        text: rdn.to_string(),
        from_field: true,
    }])
}

/// An attribute is "present" when it exists (case-insensitively) with a non-empty
/// first value. The reserved `rdn` token is always considered present because it is
/// bound from the node's DN at render time and is never stored in `attrs`.
fn present(attrs: &BTreeMap<String, Vec<String>>, name: &str) -> bool {
    // The reserved `rdn` token is always available (bound from the node's DN at
    // render time, never stored in `attrs`), so a `when` requiring it matches.
    if name.eq_ignore_ascii_case("rdn") {
        return true;
    }
    attrs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .and_then(|(_, v)| v.first())
        .map(|s| !s.is_empty())
        .unwrap_or(false)
}

/// Display width in columns (CJK = 2, combining marks per `unicode-width`).
fn str_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Join segments with single spaces (their on-screen separators).
fn join_text(segs: &[&Segment]) -> String {
    segs.iter().map(|s| s.text()).collect::<Vec<_>>().join(" ")
}

/// Longest prefix of `s` whose display width is ≤ `cols`.
fn take_cols(s: &str, cols: usize) -> String {
    let mut out = String::new();
    let mut used = 0usize;
    for ch in s.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > cols {
            break;
        }
        out.push(ch);
        used += w;
    }
    out
}

/// Trim `s` to fit `budget`, reserving 1 column for the trailing `…`, but never
/// returning an empty string (forces ≥1 visible char — the first-segment guard).
fn truncate_with_ellipsis(s: &str, budget: usize) -> String {
    let mut kept = take_cols(s, budget.saturating_sub(1));
    if kept.is_empty() {
        if let Some(c) = s.chars().next() {
            kept.push(c);
        }
    }
    format!("{kept}…")
}

/// Fit one segment into `budget` columns by trimming its **field** characters
/// from the end and replacing the removed tail with a single `…` (literal
/// decoration is preserved). Returns `None` when the segment cannot fit keeping
/// ≥1 field char (pure-literal, or field fully consumed) so the caller drops it.
/// When `guard` is set (the protected first segment) it is never dropped: it is
/// trimmed to a 1-char-+`…` minimum, trimming its literal text if it has no field.
fn fit_segment(seg: &Segment, budget: usize, guard: bool) -> Option<String> {
    let full = seg.text();
    if str_width(&full) <= budget {
        return Some(full);
    }
    let has_field = seg.pieces.iter().any(|p| p.from_field);
    if !has_field {
        return if guard {
            Some(truncate_with_ellipsis(&full, budget))
        } else {
            None
        };
    }
    let lit_w: usize = seg
        .pieces
        .iter()
        .filter(|p| !p.from_field)
        .map(|p| str_width(&p.text))
        .sum();
    let floor = lit_w + 1; // literal decoration + the single `…`
    if floor > budget && !guard {
        return None;
    }
    let field_cols = budget.saturating_sub(floor);
    let mut out = String::new();
    let mut remaining = field_cols;
    let mut kept_any = false;
    let mut ellipsis_done = false;
    for piece in &seg.pieces {
        if piece.from_field {
            for ch in piece.text.chars() {
                let w = UnicodeWidthChar::width(ch).unwrap_or(0);
                if !ellipsis_done && w <= remaining {
                    out.push(ch);
                    remaining -= w;
                    kept_any = true;
                } else if !ellipsis_done {
                    // First char that does not fit: place the ellipsis once.
                    // The guard forces at least one visible field char.
                    if guard && !kept_any {
                        out.push(ch);
                        kept_any = true;
                    }
                    out.push('…');
                    ellipsis_done = true;
                }
                // chars after the ellipsis are dropped
            }
        } else {
            out.push_str(&piece.text);
        }
    }
    if !ellipsis_done {
        // A present-but-empty field value: no chars were processed, so still mark
        // the trimmed-but-present value with the ellipsis.
        out.push('…');
    }
    if !kept_any && !guard {
        return None; // field fully consumed → drop the whole segment
    }
    Some(out)
}

/// Fit `segments` (joined by single spaces) into `avail` display columns. Trims
/// the rightmost segment's field first; drops a segment whole once its field is
/// consumed; never fully removes the first segment (the RDN survives longest).
pub fn fit_label(segments: &[Segment], avail: usize) -> String {
    if segments.is_empty() {
        return String::new();
    }
    let mut n = segments.len();
    loop {
        let head: Vec<&Segment> = segments[..n].iter().collect();
        let joined = join_text(&head);
        if str_width(&joined) <= avail {
            return joined;
        }
        let last = n - 1;
        let only_first = last == 0;
        let (head_str, head_w) = if only_first {
            (String::new(), 0usize)
        } else {
            let hs = join_text(&segments[..last].iter().collect::<Vec<_>>());
            let w = str_width(&hs) + 1; // + separating space
            (hs, w)
        };
        let budget = avail.saturating_sub(head_w);
        match fit_segment(&segments[last], budget, only_first) {
            Some(text) => {
                return if only_first {
                    text
                } else {
                    format!("{head_str} {text}")
                };
            }
            None => {
                n -= 1; // drop the last segment, retry
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attrs(pairs: &[(&str, &str)]) -> BTreeMap<String, Vec<String>> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), vec![v.to_string()]))
            .collect()
    }

    #[test]
    fn eval_first_matching_rule_wins_and_splits_into_segments() {
        let rules = default_tree_rules();
        let a = attrs(&[("description", "People")]); // cn absent → description rule
        let segs = eval_tree_label(&rules, &a, "ou=people");
        let texts: Vec<String> = segs.iter().map(|s| s.text()).collect();
        assert_eq!(texts, vec!["ou=people".to_string(), "(People)".to_string()]);
        // RDN segment is all-field; "(People)" is lit "(" + field "People" + lit ")".
        assert!(segs[0].pieces.iter().all(|p| p.from_field));
        assert_eq!(segs[1].pieces.len(), 3);
        assert!(!segs[1].pieces[0].from_field && segs[1].pieces[0].text == "(");
        assert!(segs[1].pieces[1].from_field && segs[1].pieces[1].text == "People");
        assert!(!segs[1].pieces[2].from_field && segs[1].pieces[2].text == ")");
    }

    #[test]
    fn eval_presence_is_case_insensitive_and_requires_non_empty() {
        let rules = default_tree_rules();
        // cn present but empty → cn rule skipped; description present → description rule.
        let mut a = attrs(&[("DESCRIPTION", "Staff")]);
        a.insert("cn".to_string(), vec!["".to_string()]);
        let segs = eval_tree_label(&rules, &a, "ou=staff");
        let texts: Vec<String> = segs.iter().map(|s| s.text()).collect();
        assert_eq!(texts, vec!["ou=staff".to_string(), "(Staff)".to_string()]);
    }

    #[test]
    fn eval_falls_back_to_rdn_when_no_field_attrs() {
        let rules = default_tree_rules();
        let a: BTreeMap<String, Vec<String>> = BTreeMap::new(); // neither cn nor description
        let segs = eval_tree_label(&rules, &a, "ou=people");
        let texts: Vec<String> = segs.iter().map(|s| s.text()).collect();
        assert_eq!(texts, vec!["ou=people".to_string()]);
    }

    #[test]
    fn eval_when_rdn_is_treated_as_always_present() {
        // A `when` that requires the reserved `rdn` token must always match
        // (rdn is always available), so this rule fires even with empty attrs.
        let rules = vec![CompiledTreeRule {
            when: vec!["RDN".to_string()], // also checks case-insensitivity
            template: parse_label_template("{rdn}!"),
        }];
        let a: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let segs = eval_tree_label(&rules, &a, "ou=x");
        assert_eq!(
            segs.iter().map(|s| s.text()).collect::<Vec<_>>(),
            vec!["ou=x!".to_string()]
        );
    }

    #[test]
    fn eval_with_no_matching_rule_and_no_fallback_shows_rdn() {
        // Misconfigured: a single rule that requires an absent attr, no fallback.
        let rules = vec![CompiledTreeRule {
            when: vec!["mail".to_string()],
            template: parse_label_template("{rdn} <{mail}>"),
        }];
        let a: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let segs = eval_tree_label(&rules, &a, "uid=jane");
        let texts: Vec<String> = segs.iter().map(|s| s.text()).collect();
        assert_eq!(texts, vec!["uid=jane".to_string()]);
    }

    #[test]
    fn split_keeps_field_provenance_on_space_separated_field_values() {
        // A field value with an internal space splits into two field segments.
        let rules = vec![CompiledTreeRule {
            when: vec![],
            template: parse_label_template("{cn}"),
        }];
        let a = attrs(&[("cn", "Ada Lovelace")]);
        let segs = eval_tree_label(&rules, &a, "cn=ada");
        let texts: Vec<String> = segs.iter().map(|s| s.text()).collect();
        assert_eq!(texts, vec!["Ada".to_string(), "Lovelace".to_string()]);
        assert!(segs.iter().all(|s| s.pieces.iter().all(|p| p.from_field)));
    }

    #[test]
    fn empty_config_compiles_to_default_rule_set() {
        let cfg = TreeConfig::default();
        let rules = compile_tree_rules(&cfg);
        // cn rule, description rule, unconditional {rdn} fallback.
        assert_eq!(rules.len(), 3);
        assert_eq!(rules[0].when, vec!["cn".to_string()]);
        assert_eq!(rules[1].when, vec!["description".to_string()]);
        assert!(rules[2].when.is_empty());
    }

    #[test]
    fn non_empty_config_compiles_rules_verbatim() {
        let cfg = TreeConfig {
            label: vec![crate::config::TreeLabelRule {
                when: vec!["ou".to_string()],
                template: "{rdn} [{ou}]".to_string(),
            }],
        };
        let rules = compile_tree_rules(&cfg);
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].when, vec!["ou".to_string()]);
        assert_eq!(rules[0].template, parse_label_template("{rdn} [{ou}]"));
    }

    #[test]
    fn tree_template_attrs_unions_and_excludes_rdn() {
        let rules = default_tree_rules();
        let attrs = tree_template_attrs(&rules);
        // cn and description are referenced; rdn is excluded.
        assert!(attrs.iter().any(|a| a.eq_ignore_ascii_case("cn")));
        assert!(attrs.iter().any(|a| a.eq_ignore_ascii_case("description")));
        assert!(!attrs.iter().any(|a| a.eq_ignore_ascii_case("rdn")));
    }

    #[test]
    fn tree_template_attrs_dedups_case_insensitively() {
        let rules = vec![CompiledTreeRule {
            when: vec![],
            template: parse_label_template("{CN}-{cn}"),
        }];
        let attrs = tree_template_attrs(&rules);
        assert_eq!(attrs.len(), 1);
    }

    #[test]
    fn default_rules_scan_attrs_are_cn_and_description_only() {
        let mut attrs = tree_template_attrs(&default_tree_rules());
        attrs.sort();
        assert_eq!(attrs, vec!["cn".to_string(), "description".to_string()]);
    }

    // Helper: build the two-segment label for `{rdn} ({description})`,
    // rdn="ou=people" (width 9), description="People" → segments
    // ["ou=people"(field 9), "(People)"(lit"("+field"People"+lit")" = 8)].
    fn people_segments() -> Vec<Segment> {
        let rules = default_tree_rules();
        let a = attrs(&[("description", "People")]);
        eval_tree_label(&rules, &a, "ou=people")
    }

    #[test]
    fn fit_full_when_it_fits() {
        let segs = people_segments();
        assert_eq!(fit_label(&segs, 18), "ou=people (People)"); // width 18
        assert_eq!(fit_label(&segs, 30), "ou=people (People)");
    }

    #[test]
    fn fit_ellipsizes_last_segment_field_from_the_end() {
        let segs = people_segments();
        // avail 17: last-segment budget 7 → "(Peop…)" (2 lit + 4 field + 1 ellipsis).
        assert_eq!(fit_label(&segs, 17), "ou=people (Peop…)");
        // avail 16: budget 6 → "(Peo…)".
        assert_eq!(fit_label(&segs, 16), "ou=people (Peo…)");
        // avail 14: budget 4 → one field char kept "(P…)".
        assert_eq!(fit_label(&segs, 14), "ou=people (P…)");
    }

    #[test]
    fn fit_drops_last_segment_once_field_is_consumed() {
        let segs = people_segments();
        // avail 13: budget 3 = literals(2)+ellipsis(1), 0 field cols → drop "(...)".
        assert_eq!(fit_label(&segs, 13), "ou=people");
        assert_eq!(fit_label(&segs, 9), "ou=people"); // first segment fits exactly
    }

    #[test]
    fn fit_ellipsizes_protected_first_segment_field() {
        let segs = people_segments();
        // Only the first segment remains and still doesn't fit.
        assert_eq!(fit_label(&segs, 8), "ou=peop…"); // 7 field cols + ellipsis
        assert_eq!(fit_label(&segs, 7), "ou=peo…");
        assert_eq!(fit_label(&segs, 2), "o…"); // 1-char minimum + ellipsis
        assert_eq!(fit_label(&segs, 1), "o…"); // min overflows a too-narrow pane
    }

    #[test]
    fn fit_drops_pure_literal_segment_as_a_unit() {
        // Template "{cn} -- end": segments ["X"(field), "--"(lit), "end"(lit)].
        let rules = vec![CompiledTreeRule {
            when: vec![],
            template: parse_label_template("{cn} -- end"),
        }];
        let a = attrs(&[("cn", "X")]);
        let segs = eval_tree_label(&rules, &a, "cn=x");
        assert_eq!(
            segs.iter().map(|s| s.text()).collect::<Vec<_>>(),
            vec!["X", "--", "end"]
        );
        // "X -- end" width 8; at avail 5 the pure-literal "end" drops whole → "X --".
        assert_eq!(fit_label(&segs, 5), "X --");
        // at avail 3 "--" also drops → "X".
        assert_eq!(fit_label(&segs, 3), "X");
    }

    #[test]
    fn fit_multi_field_template_trims_rightmost_segment_first() {
        let rules = vec![CompiledTreeRule {
            when: vec![],
            template: parse_label_template("{cn} - {rdn}"),
        }];
        let a = attrs(&[("cn", "Group")]);
        let segs = eval_tree_label(&rules, &a, "cn=group");
        // ["Group"(5), "-"(1 lit), "cn=group"(8 field)] joined "Group - cn=group" = 16.
        assert_eq!(fit_label(&segs, 16), "Group - cn=group");
        // avail 14: last-seg budget = 14 - (len("Group -")=7 + space 1) = 6 → "cn=gr…"
        // (field_cols = 6 − 1 = 5 → 5 chars kept).
        assert_eq!(fit_label(&segs, 14), "Group - cn=gr…");
    }

    #[test]
    fn fit_is_unicode_width_aware_for_cjk() {
        // description with CJK (each 2 cols): "日本" width 4 → "(日本)" width 6.
        let rules = default_tree_rules();
        let a = attrs(&[("description", "日本")]);
        let segs = eval_tree_label(&rules, &a, "ou=x");
        // segments ["ou=x"(4), "(日本)"(6)] joined "ou=x (日本)" width 4+1+6 = 11.
        assert_eq!(fit_label(&segs, 11), "ou=x (日本)");
        // avail 9: last-seg budget = 9 - 5 = 4 = lit(2)+ellipsis(1)+1 col → 0 CJK
        // chars fit in 1 col → drop "(...)" → "ou=x".
        assert_eq!(fit_label(&segs, 9), "ou=x");
        // avail 10: budget 5 → field cols 2 → one CJK char "(日…)" width 2+2+1=5.
        assert_eq!(fit_label(&segs, 10), "ou=x (日…)");
    }

    #[test]
    fn fit_empty_segment_list_is_empty_string() {
        assert_eq!(fit_label(&[], 20), "");
    }

    #[test]
    fn fit_empty_field_value_segment() {
        // Template "({cn})" with cn present-but-empty: segment "()" + an empty field.
        let rules = vec![CompiledTreeRule {
            when: vec![],
            template: parse_label_template("({cn})"),
        }];
        let mut a = std::collections::BTreeMap::new();
        a.insert("cn".to_string(), vec!["".to_string()]);
        let segs = eval_tree_label(&rules, &a, "cn=x");
        // Fits as-is when wide enough (no trimming needed): "()" width 2.
        assert_eq!(fit_label(&segs, 5), "()");
    }

    #[test]
    fn fit_first_segment_with_framing_literals_under_pressure() {
        // Template "[{rdn}]": single segment "[ou=people]" (lit "[" + field + lit "]").
        let rules = vec![CompiledTreeRule {
            when: vec![],
            template: parse_label_template("[{rdn}]"),
        }];
        let a: std::collections::BTreeMap<String, Vec<String>> = std::collections::BTreeMap::new();
        let segs = eval_tree_label(&rules, &a, "ou=people");
        // Wide: full label.
        assert_eq!(fit_label(&segs, 11), "[ou=people]");
        // Under extreme pressure the guard keeps >=1 field char + ellipsis. Both
        // framing literals are preserved (literal pieces are emitted unconditionally),
        // so the field is trimmed *between* them → "[o…]" (documented behavior).
        assert_eq!(fit_label(&segs, 2), "[o…]");
    }
}
