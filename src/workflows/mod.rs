//! Read-flow workflows: the eager structure model, the read flow, the create
//! helpers, and their pure helpers.
//!
//! Everything here is tty-free domain logic. Rendering the [`structure::Structure`]
//! and the [`crate::workflows::form_model::FormModel`]s happens in the ratatui UI
//! ([`crate::ui::view`]).

pub mod create;
pub mod form_model;
pub mod read_flow;
pub mod save;
pub mod structure;

#[cfg(test)]
pub(crate) mod test_fixtures;
