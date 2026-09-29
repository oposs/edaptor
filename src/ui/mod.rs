//! tvision-rs UI: the three-pane LDAP browser/editor. Built under `src/tui/`
//! during M1-M4 and renamed to `src/ui/` at the M5b cutover; it is now the sole
//! UI. Only this module tree may `use tvision_rs`.

mod app;
pub(crate) mod help_ctx;
pub(crate) mod lookup;
pub(crate) mod theme;
// Keep `pub`: builders and guard_decision are not yet called from non-test code
// (wired in Task 8). `pub` suppresses the dead_code lint without `#[allow]`.
pub(crate) mod choice;
pub mod dialog;
pub(crate) mod multi_picker;
pub(crate) mod oc_picker;
pub(crate) mod ordered;
pub(crate) mod panes;
pub(crate) mod picker;
pub(crate) mod pump;
pub(crate) mod pw_editor;
pub(crate) mod scroll_group;
pub(crate) mod shuttle;
// `pub` (not `pub(crate)`): the `edaptor` binary (`src/main.rs`) calls
// `edaptor::ui::startup::resolve_config_path`, so the module must be a
// crate-external item.
pub mod startup;
mod state;
#[cfg(test)]
pub(crate) mod test_support;
// Keep `pub`: the FieldWidget plugin contract is defined here for M1 and
// consumed in M2. `pub` keeps the as-yet-unused contract types visible as
// public API surface so they are NOT dead_code — no `#[allow]` needed.
pub mod widget;

use std::cell::RefCell;
use std::rc::Rc;

pub use state::UiState;

/// Shared mutable app state, cloned into each pane factory closure.
pub type Shared = Rc<RefCell<UiState>>;

/// Broadcast command: re-render all panes from current `UiState`.
pub const REFRESH: tv::Command = tv::Command::custom("edaptor.refresh");

/// Form pane command: activate the focused field's modal editor.
pub const ACTIVATE: tv::Command = tv::Command::custom("edaptor.activate_field");

/// App-level commands routed to `app::dispatch` via `run_app`.
pub const SAVE: tv::Command = tv::Command::custom("edaptor.save");
pub const CREATE: tv::Command = tv::Command::custom("edaptor.create");
pub const REQUEST_QUIT: tv::Command = tv::Command::custom("edaptor.request_quit");
pub const GUARD_NAV: tv::Command = tv::Command::custom("edaptor.guard_nav");
pub const SHOW_ERROR: tv::Command = tv::Command::custom("edaptor.show_error");

pub const STARTUP: tv::Command = tv::Command::custom("edaptor.startup");

/// Re-run the eager structure scan (Alt+R) — the escape hatch for structure
/// staleness that no local reflow can see (another client created a container).
pub const RELOAD: tv::Command = tv::Command::custom("edaptor.reload");

/// A one-shot action to run once the TUI has started (schema is already loaded by
/// `bootstrap`). Carried on `UiState::pending_startup`, posted by the pump as the
/// `STARTUP` command, and executed once in `app::dispatch`.
#[derive(Debug, Clone)]
pub enum StartupAction {
    /// Open a create form for `profile_idx` under `container`.
    Create {
        profile_idx: usize,
        container: String,
    },
    /// Show the all-profiles chooser, then open a create form for the pick under
    /// `container` (the pick's `search_base` when `None`).
    ChooseThenCreate { container: Option<String> },
}

use anyhow::Result;
use tvision_rs::{self as tv, CrosstermBackend};

use crate::config::Config;

/// What `edaptor tui-create` asked for, before profiles exist (names are
/// resolved against the merged profiles after `bootstrap`).
#[derive(Debug, Clone)]
pub enum StartupRequest {
    Create {
        profile: String,
        container: Option<String>,
    },
    Choose {
        container: Option<String>,
    },
}

/// Resolve a startup request against the merged profiles (case-insensitive).
pub fn resolve_startup(
    profiles: &[crate::config::EntryProfile],
    req: StartupRequest,
) -> Result<StartupAction, String> {
    match req {
        StartupRequest::Choose { container } => Ok(StartupAction::ChooseThenCreate { container }),
        StartupRequest::Create { profile, container } => {
            let idx = crate::workflows::create::resolve_profile_arg(profiles, Some(&profile))?
                .expect("Some(name) resolves to Some(idx) or an error");
            let dn = container.unwrap_or_else(|| profiles[idx].search_base.clone());
            if dn.trim().is_empty() {
                return Err(format!(
                    "profile '{}' has no search_base; pass --container",
                    profiles[idx].name
                ));
            }
            Ok(StartupAction::Create {
                profile_idx: idx,
                container: dn,
            })
        }
    }
}

/// Spawn the worker, fetch schema + structure, then run the TUI. `startup` runs a
/// one-shot action (e.g. open a create form) once the loop starts; `None` = normal browse.
pub fn run(config: Config, password: String, startup: Option<StartupRequest>) -> Result<()> {
    let mut booted = state::bootstrap(config, password)?;
    // Resolved before the screen takeover, so an unknown name is reported on the terminal.
    booted.pending_startup = startup
        .map(|r| resolve_startup(&booted.profiles, r))
        .transpose()
        .map_err(|e| anyhow::anyhow!(e))?;
    let state: Shared = Rc::new(RefCell::new(booted));
    let backend = Box::new(CrosstermBackend::new()?);
    let mut program = app::build_program(backend, state.clone());
    let dispatch_state = state.clone();
    program.run_app(move |prog, cmd| app::dispatch(prog, cmd, &dispatch_state));
    Ok(())
}

#[cfg(test)]
mod startup_tests {
    use super::*;
    use crate::config::EntryProfile;

    fn profiles() -> Vec<EntryProfile> {
        vec![
            EntryProfile {
                name: "user".into(),
                search_base: "ou=people,dc=example,dc=org".into(),
                ..Default::default()
            },
            EntryProfile {
                name: "user-people".into(),
                search_base: "ou=people,dc=example,dc=org".into(),
                ..Default::default()
            },
            EntryProfile {
                name: "NoBase".into(),
                ..Default::default()
            },
        ]
    }

    #[test]
    fn create_resolves_a_merged_name_to_its_index() {
        let req = StartupRequest::Create {
            profile: "USER-PEOPLE".into(),
            container: None,
        };
        match resolve_startup(&profiles(), req).unwrap() {
            StartupAction::Create {
                profile_idx,
                container,
            } => {
                assert_eq!(profile_idx, 1);
                assert_eq!(container, "ou=people,dc=example,dc=org");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn container_override_wins() {
        let req = StartupRequest::Create {
            profile: "user".into(),
            container: Some("ou=x,dc=example,dc=org".into()),
        };
        let a = resolve_startup(&profiles(), req).unwrap();
        assert!(
            matches!(a, StartupAction::Create { container, .. } if container == "ou=x,dc=example,dc=org")
        );
    }

    #[test]
    fn unknown_profile_lists_valid_names() {
        let req = StartupRequest::Create {
            profile: "Admins".into(),
            container: None,
        };
        let e = resolve_startup(&profiles(), req).unwrap_err();
        assert!(e.contains("Admins") && e.contains("user-people"), "{e}");
    }

    #[test]
    fn empty_search_base_without_container_errors() {
        let req = StartupRequest::Create {
            profile: "nobase".into(),
            container: None,
        };
        let e = resolve_startup(&profiles(), req).unwrap_err();
        assert!(e.contains("search_base"));
    }

    #[test]
    fn choose_passes_through() {
        assert!(matches!(
            resolve_startup(&profiles(), StartupRequest::Choose { container: None }).unwrap(),
            StartupAction::ChooseThenCreate { container: None }
        ));
    }
}
