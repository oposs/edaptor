//! Read-flow workflows: the DIT browser controller, the read flow, and their
//! pure helpers.
//!
//! Everything here is tty-free domain logic. Turning [`browser::BrowserNode`]s
//! into real Turbo Vision outline widgets, and [`crate::ui::form::FormModel`]s
//! into dialogs, happens in [`crate::ui::facade`].

pub mod browser;
pub mod create;
pub mod read_flow;
