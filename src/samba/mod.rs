//! Samba lifecycle logic (spec §9) — pure, headless, fully unit-tested.
//!
//! This module produces/consumes plain types (`Vec<ModOp>`,
//! `BTreeMap<String, Vec<String>>`, `String`). It performs no terminal or
//! network I/O: SID/RID math, NT hashing, account flags, and the synced
//! password mod-set all live here so they can be golden-pinned in unit tests.

pub mod account;
pub mod groupmap;
pub mod nthash;
pub mod password;
pub mod sid;

/// Resolved Samba domain context: the domain SID (e.g. `S-1-5-21-...`) and the
/// algorithmic RID base (default 1000). Discovered from the live `sambaDomain`
/// entry, or supplied as a config fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SambaDomainInfo {
    pub domain_sid: String,
    pub algorithmic_rid_base: u32,
}
