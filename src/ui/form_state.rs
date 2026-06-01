//! Pure decision logic for the "leave a dirty form" guard (spec §5.6).

/// What the user chose in the Save/Discard/Stay dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardChoice {
    Save,
    Discard,
    Stay,
}

/// What the navigation handler should do after consulting the guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardOutcome {
    /// Proceed with the pending navigation (re-spin pane 3).
    Proceed,
    /// Run the save flow first, then proceed.
    SaveThenProceed,
    /// Cancel the navigation; keep editing.
    Cancel,
}

/// Decide what to do when the selection is about to change.
/// Clean forms always proceed; dirty forms route by the user's choice.
pub fn guard_decision(dirty: bool, choice: Option<GuardChoice>) -> GuardOutcome {
    if !dirty {
        return GuardOutcome::Proceed;
    }
    match choice {
        Some(GuardChoice::Save) => GuardOutcome::SaveThenProceed,
        Some(GuardChoice::Discard) => GuardOutcome::Proceed,
        Some(GuardChoice::Stay) | None => GuardOutcome::Cancel,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_form_always_proceeds() {
        assert_eq!(guard_decision(false, None), GuardOutcome::Proceed);
        assert_eq!(
            guard_decision(false, Some(GuardChoice::Stay)),
            GuardOutcome::Proceed
        );
    }

    #[test]
    fn dirty_routes_by_choice() {
        assert_eq!(
            guard_decision(true, Some(GuardChoice::Save)),
            GuardOutcome::SaveThenProceed
        );
        assert_eq!(
            guard_decision(true, Some(GuardChoice::Discard)),
            GuardOutcome::Proceed
        );
        assert_eq!(
            guard_decision(true, Some(GuardChoice::Stay)),
            GuardOutcome::Cancel
        );
        assert_eq!(guard_decision(true, None), GuardOutcome::Cancel);
    }
}
