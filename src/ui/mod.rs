//! Terminal UI layer (ratatui).
//!
//! Boundary rule: ratatui / crossterm are imported only inside this `ui` module
//! ([`app`], [`view`], [`edit_form`]). Every other module (`form`, `crate::app`,
//! `crate::workflows`) deals in plain domain types, so the rest of the crate
//! stays testable without a terminal and the TUI backend stays swappable.

pub mod app;
pub mod config_picker;
pub mod edit_form;
pub mod form;
pub mod form_state;
pub mod picker;
pub mod view;
