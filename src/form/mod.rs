//! Pure write-path domain logic (tty-free, network-free): attribute diffing
//! into a [`changeset::ChangeSet`] and client-side validation.
//!
//! These modules MUST NOT import `turbo_vision` (presentation) and MUST NOT
//! import `ldap::worker::LdapEntry` (keeps the dependency one-directional:
//! `ldap::worker` imports `ModOp` FROM here, never the reverse). The caller
//! converts an `LdapEntry` into an [`changeset::EditEntry`] at the boundary.

pub mod changeset;
pub mod validate;
