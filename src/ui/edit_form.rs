//! The editable form model (skeleton — built out in P2).
//!
//! `FormModel`/`FormField` (`crate::ui::form`) are read-only-oriented and carry
//! no edit state, so the editable shape is net-new here: `EditField { multi,
//! secret, ordered, editor: TextState, … }` plus an `EditForm` with a baseline
//! for the set-wise dirty check, and `build_edit_form(&FormModel, &SchemaModel,
//! read_only)`. The multi-value `ValueEditor` popup also lives here.
//!
//! Intentionally empty in P0; populated in P2-T1.
