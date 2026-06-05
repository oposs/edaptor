//! DIT-tree (pane 1) branch-label rules: compile config rules (or a built-in
//! default set), discover the attributes their templates reference, evaluate the
//! first matching rule per node, and width-fit the rendered label so the RDN
//! survives longest. Pane-2 leaf labels and the `‹self›` row are NOT handled
//! here — see `src/ui/app/structure_view.rs`.

use crate::config::label::{parse_label_template, LabelSeg};
use crate::config::TreeConfig;

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

#[cfg(test)]
mod tests {
    use super::*;

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
