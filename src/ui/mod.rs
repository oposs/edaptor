//! Terminal UI layer.
//!
//! Boundary rule: [`facade`] is the ONLY module in the crate that may import
//! `turbo_vision`. Every other module (`form`, `crate::app`,
//! `crate::workflows`) deals in the plain domain types defined alongside them,
//! so the rest of the crate stays testable without a terminal and the TUI
//! backend stays swappable.

pub mod facade;
pub mod form;
pub mod form_state;
