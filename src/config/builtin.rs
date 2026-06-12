//! Baked-in objectClass → attribute → widget-spec defaults, compiled into the
//! binary via `include_str!`. Loaded once at first access.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::config::WidgetSpecCfg;

/// objectClass name (lower-cased) → attribute name (lower-cased) → spec.
pub type BuiltinSchema = HashMap<String, HashMap<String, WidgetSpecCfg>>;

static BUILTIN: OnceLock<BuiltinSchema> = OnceLock::new();

/// Returns the singleton baked-in schema. Panics on a malformed bundled TOML
/// (a compile-time invariant, not a runtime error).
pub fn builtin_schema() -> &'static BuiltinSchema {
    BUILTIN.get_or_init(|| {
        let raw: HashMap<String, HashMap<String, WidgetSpecCfg>> =
            toml::from_str(include_str!("builtin_schema.toml"))
                .expect("builtin_schema.toml is always valid");
        // Lower-case all keys for case-insensitive lookup.
        raw.into_iter()
            .map(|(oc, attrs)| {
                (
                    oc.to_lowercase(),
                    attrs
                        .into_iter()
                        .map(|(a, w)| (a.to_lowercase(), w))
                        .collect(),
                )
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loginshell_is_choice() {
        let bs = builtin_schema();
        assert!(matches!(
            bs["posixaccount"]["loginshell"],
            WidgetSpecCfg::Choice { .. }
        ));
    }

    #[test]
    fn userpassword_samba_is_password_with_samba() {
        let bs = builtin_schema();
        assert!(matches!(
            bs["sambasamaccount"]["userpassword"],
            WidgetSpecCfg::Password { samba: true }
        ));
    }

    #[test]
    fn memberof_is_readonly() {
        let bs = builtin_schema();
        assert!(matches!(
            bs["inetorgperson"]["memberof"],
            WidgetSpecCfg::Readonly
        ));
    }

    #[test]
    fn olcaccess_is_x_ordered() {
        let bs = builtin_schema();
        assert!(matches!(
            bs["olcglobal"]["olcaccess"],
            WidgetSpecCfg::XOrdered
        ));
    }

    #[test]
    fn shadowaccount_has_no_userpassword_entry() {
        // shadowAccount.userPassword was removed to avoid clobbering the
        // samba=true entry when both shadowAccount and sambaSamAccount are
        // present on the same entry (alphabetical walk, last wins).
        let bs = builtin_schema();
        assert!(
            bs.get("shadowaccount")
                .and_then(|m| m.get("userpassword"))
                .is_none(),
            "shadowAccount.userPassword must not be in the builtin schema"
        );
    }
}
