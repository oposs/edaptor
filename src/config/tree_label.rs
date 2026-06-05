//! DIT-tree (pane 1) branch-label rules: compile config rules (or a built-in
//! default set), discover the attributes their templates reference, evaluate the
//! first matching rule per node, and width-fit the rendered label so the RDN
//! survives longest. Pane-2 leaf labels and the `‹self›` row are NOT handled
//! here — see `src/ui/app/structure_view.rs`.

use crate::config::label::{parse_label_template, LabelSeg, Piece};
use crate::config::TreeConfig;
use std::collections::BTreeMap;

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
/// first value.
fn present(attrs: &BTreeMap<String, Vec<String>>, name: &str) -> bool {
    attrs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .and_then(|(_, v)| v.first())
        .map(|s| !s.is_empty())
        .unwrap_or(false)
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
}
