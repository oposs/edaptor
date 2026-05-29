//! Application-level domain logic: menu assembly from config profiles.
//!
//! This module is tty-free. It produces backend-agnostic [`MenuDef`]s that the
//! facade ([`crate::ui::facade`]) turns into real Turbo Vision menu widgets.
//! Only [`build_menu_defs`] is unit-tested; the facade wrappers it feeds need a
//! terminal and are not.

use crate::config::EntryProfile;

/// Turbo Vision's quit command id (mirrored from
/// `turbo_vision::core::command::CM_QUIT` = 24, verified in the crate source).
/// Kept as a plain constant here so this module stays free of any `turbo_vision`
/// import; the facade's `cm_quit_matches_app` test asserts the two agree.
pub const CM_QUIT: u16 = 24;

/// App-local command id for the generic DIT browser menu entry. Chosen above
/// Turbo Vision's standard `CM_*` ids.
pub const CM_BROWSER: u16 = 1000;

/// App-local command id base for per-profile menu entries. Profile *i* gets
/// `CM_PROFILE_BASE + i`.
pub const CM_PROFILE_BASE: u16 = 1100;

/// A backend-agnostic menu entry: a label plus the command id it fires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuDef {
    /// The (hotkey-marked) label shown in the menu.
    pub label: String,
    /// The command id dispatched when the entry is chosen.
    pub command: u16,
}

/// Build the menu entries from the configured profiles.
///
/// One entry per profile (in config order), then a generic "Browser" entry,
/// then "Quit". Pure and tty-free.
pub fn build_menu_defs(profiles: &[EntryProfile]) -> Vec<MenuDef> {
    let mut defs: Vec<MenuDef> = profiles
        .iter()
        .enumerate()
        .map(|(i, p)| MenuDef {
            label: p.name.clone(),
            command: CM_PROFILE_BASE + i as u16,
        })
        .collect();
    defs.push(MenuDef {
        label: "Browser".to_string(),
        command: CM_BROWSER,
    });
    defs.push(MenuDef {
        label: "Quit".to_string(),
        command: CM_QUIT,
    });
    defs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(name: &str, object_class: &str) -> EntryProfile {
        EntryProfile {
            name: name.to_string(),
            object_class: object_class.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn menu_defs_from_profiles() {
        let profiles = vec![
            profile("Users", "inetOrgPerson"),
            profile("Groups", "groupOfNames"),
        ];
        let defs = build_menu_defs(&profiles);
        let labels: Vec<&str> = defs.iter().map(|d| d.label.as_str()).collect();
        assert_eq!(labels, vec!["Users", "Groups", "Browser", "Quit"]);
        assert_eq!(defs.last().unwrap().command, CM_QUIT);
        assert_eq!(defs[0].command, CM_PROFILE_BASE);
        assert_eq!(defs[1].command, CM_PROFILE_BASE + 1);
        assert_eq!(defs[2].command, CM_BROWSER);
    }

    #[test]
    fn menu_defs_with_no_profiles_still_has_browser_and_quit() {
        let defs = build_menu_defs(&[]);
        let labels: Vec<&str> = defs.iter().map(|d| d.label.as_str()).collect();
        assert_eq!(labels, vec!["Browser", "Quit"]);
    }
}
