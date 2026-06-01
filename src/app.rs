//! Application-level domain logic: the backend-agnostic UI action vocabulary.

/// A backend-agnostic UI intent produced by the event loop's key dispatch
/// ([`crate::ui::app::dispatch_key`]) and serviced by
/// [`crate::ui::app::handle_action`]. Keeping it framework-free is what lets the
/// key/dispatch layer stay testable without a terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiAction {
    /// Create a new entry under profile *i* (F7 → profile 0).
    NewEntry(usize),
    /// Delete the entry with this DN (the one shown in the form pane).
    DeleteEntry(String),
    /// Save the edit form (F2).
    FormSave,
    /// Cancel/revert the edit form (F3).
    FormCancel,
    /// Re-run the eager structure scan (F5).
    Refresh,
    /// Nothing actionable.
    None,
}
