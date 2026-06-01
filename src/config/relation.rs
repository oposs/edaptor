//! Membership relations: one `[[relation]]` declares both ends of a symmetric
//! holder↔candidate link (e.g. group.member ↔ user.memberOf). Pure; resolved
//! against the configured [`EntryProfile`]s into directional [`ResolvedRelation`]s.

use serde::Deserialize;

/// A symmetric membership relation as declared in `[[relation]]`. Template names
/// (`holder`, `candidate`) reference `[[profile]]` `name`s.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
pub struct Relation {
    pub name: String,
    /// Template whose entry OWNS the link attribute.
    pub holder: String,
    /// The real, writable attribute on the holder (e.g. `member`).
    pub holder_attr: String,
    /// Template that scopes the picker's candidate search (e.g. `user`).
    pub candidate: String,
    /// Virtual back-reference field shown on the candidate side (e.g. `memberOf`).
    pub back_attr: String,
}

#[cfg(test)]
mod tests {
    use crate::config::Config;

    #[test]
    fn parses_relation_block() {
        let cfg: Config = toml::from_str(
            r#"
            [server]
            uri = "ldaps://x"
            base_dn = "dc=x"
            [auth]
            [[relation]]
            name = "group-membership"
            holder = "group"
            holder_attr = "member"
            candidate = "user"
            back_attr = "memberOf"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.relations.len(), 1);
        assert_eq!(cfg.relations[0].holder_attr, "member");
        assert_eq!(cfg.relations[0].back_attr, "memberOf");
    }
}
