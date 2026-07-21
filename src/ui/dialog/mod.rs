//! Modal dialogs for the edit/write spine. Builders return `Box<dyn View>` run via
//! `Program::exec_view`; buttons use the modal-exit commands so `exec_view` returns
//! which was pressed. All `exec_view` calls live in `ui::app::dispatch`.

// Submodules declared here — implementations live in their own files.
// pub(crate) keeps them visible within the crate while avoiding dead_code
// warnings (each is referenced from the smoke tests below).
pub mod config_picker;
pub mod confirm;
pub mod conflict;
pub mod container_chooser;
pub mod error;
pub mod guard;
pub mod profile_chooser;

use tvision_rs::Command;

/// The user's answer to the dirty guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardDecision {
    Save,
    Discard,
    Stay,
}

/// Map a guard dialog's returned command to a decision. `YES`=Save, `NO`=Discard,
/// anything else (incl. `CANCEL` / window close) = Stay (the safe default).
pub fn guard_decision(answer: Command) -> GuardDecision {
    if answer == Command::YES {
        GuardDecision::Save
    } else if answer == Command::NO {
        GuardDecision::Discard
    } else {
        GuardDecision::Stay
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tvision_rs::Command;

    // --- guard_decision unit tests ---

    #[test]
    fn yes_is_save_no_is_discard_else_stay() {
        assert_eq!(guard_decision(Command::YES), GuardDecision::Save);
        assert_eq!(guard_decision(Command::NO), GuardDecision::Discard);
        assert_eq!(guard_decision(Command::CANCEL), GuardDecision::Stay);
        assert_eq!(
            guard_decision(Command::custom("whatever")),
            GuardDecision::Stay
        );
    }

    // --- builder smoke tests (prevent dead_code under clippy -D warnings) ---

    #[test]
    fn confirm_builds_without_panic() {
        let _v = confirm::build("dn: cn=test,dc=example,dc=com\nchangeType: modify\n");
        // If Dialog + StaticText + button_row construction panics, this fails.
    }

    #[test]
    fn error_builds_without_panic() {
        let _v = error::build("LDAP error: insufficient access");
    }

    #[test]
    fn conflict_builds_without_panic() {
        let _v = conflict::build("Conflicting attribute(s): description.");
    }

    #[test]
    fn guard_builds_without_panic() {
        let _v = guard::build();
    }

    #[test]
    fn config_picker_builds_without_panic() {
        use std::cell::RefCell;
        use std::path::PathBuf;
        use std::rc::Rc;
        let items = vec![config_picker::PickerItem {
            name: "a".into(),
            description: "desc".into(),
            path: PathBuf::from("/tmp/a.toml"),
        }];
        let (_v, _id) = config_picker::build(items, Rc::new(RefCell::new(None)));
    }

    #[test]
    fn profile_chooser_builds_without_panic() {
        use crate::ldap::worker::RawSubschema;
        use crate::workflows::structure::Structure;
        use std::cell::RefCell;
        use std::rc::Rc;
        let schema = crate::schema::SchemaModel::from_raw(&RawSubschema::default());
        let st = crate::ui::state::UiState::new_for_test(
            Structure::build("dc=example,dc=org", vec![]),
            schema,
            "dc=example,dc=org".into(),
            Vec::new(),
            Vec::new(),
        );
        let shared = Rc::new(RefCell::new(st));
        let _v = profile_chooser::build(vec!["People".into(), "Groups".into()], shared);
    }

    #[test]
    fn container_chooser_builds_without_panic() {
        use crate::ldap::worker::RawSubschema;
        use crate::workflows::structure::Structure;
        use std::cell::RefCell;
        use std::rc::Rc;
        let schema = crate::schema::SchemaModel::from_raw(&RawSubschema::default());
        let st = crate::ui::state::UiState::new_for_test(
            Structure::build("dc=example,dc=org", vec![]),
            schema,
            "dc=example,dc=org".into(),
            Vec::new(),
            Vec::new(),
        );
        let shared = Rc::new(RefCell::new(st));
        let _v = container_chooser::build(
            "dc=example,dc=org".into(),
            "ou=people,dc=example,dc=org".into(),
            shared,
        );
    }
}
