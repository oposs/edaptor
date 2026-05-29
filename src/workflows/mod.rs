//! Read-flow workflows: the DIT browser controller and its pure helpers.
//!
//! Everything here is tty-free domain logic. Turning [`browser::BrowserNode`]s
//! into real Turbo Vision outline widgets happens in [`crate::ui::facade`].
//! (`read_flow` — selection → form model — is added in Task 6.)

pub mod browser;
