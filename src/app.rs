//! Application-level domain logic: the backend-agnostic UI action vocabulary.

/// A backend-agnostic UI intent produced by the tvision event loop's key dispatch
/// and serviced by the tvision action handler. Keeping it framework-free lets the
/// key/dispatch layer stay testable without a terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiAction {
    /// Create a new entry under profile *i*.
    NewEntry(usize),
    /// Alt+N: choose which profile to create — a context-filtered chooser (or
    /// direct when exactly one profile matches the current container).
    NewEntryChoose,
    /// Delete the entry with this DN (the one shown in the form pane).
    DeleteEntry(String),
    /// Save the edit form (Alt+S).
    FormSave,
    /// Cancel/revert the edit form (Alt+C).
    FormCancel,
    /// Re-run the eager structure scan (Alt+R).
    Refresh,
    /// Allocate the next free number into a create-form field bound to a
    /// `{next:MIN-MAX}` autonumber (Enter on the field; needs the worker for the
    /// directory scan, so it round-trips through the action handler).
    AllocateNextNumber {
        /// Index of the focused field within the form.
        field_idx: usize,
    },
    /// Nothing actionable.
    None,
}
