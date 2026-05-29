//! Terminal UI layer.
//!
//! Boundary rule: [`facade`] is the ONLY module in the crate that may import
//! `turbo_vision`. Every other module deals in plain domain types, so the rest
//! of the crate stays testable without a terminal and the TUI backend stays
//! swappable.

pub mod facade;
// `form` (the schema-driven read-only form model) is added in Task 5.
