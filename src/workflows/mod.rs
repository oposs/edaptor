//! Read-flow workflows: the eager structure model, the read flow, the create
//! helpers, and their pure helpers.
//!
//! Everything here is tty-free domain logic. Rendering the [`structure::Structure`]
//! and the [`crate::workflows::form_model::FormModel`]s happens in the tvision UI
//! (`crate::ui`).

pub mod alloc_flow;
pub mod create;
pub mod edit_form;
pub mod form_model;
pub mod labels;
pub mod leaf_search;
pub mod pick_state;
pub mod read_flow;
pub mod resolve_flow;
pub mod samba_compute;
pub mod save;
pub mod search_flow;
pub mod structure;
pub mod widget_bind;
pub mod write_flow;

#[cfg(test)]
pub(crate) mod test_fixtures;
