//! Application-level domain logic: menu assembly from config profiles.
//!
//! This module is tty-free. It produces backend-agnostic [`MenuDef`]s that the
//! facade ([`crate::ui::facade`]) turns into real Turbo Vision menu widgets.
//! Only [`build_menu_defs`] is unit-tested; the facade wrappers it feeds need a
//! terminal and are not.

use crate::config::EntryProfile;

/// The Turbo Vision quit command id (real value 24, verified in the crate
/// source). Mirrored here as a plain constant so this module needs no Turbo
/// Vision import; the facade's `cm_quit_matches_app` test asserts the two agree.
pub const CM_QUIT: u16 = 24;

/// App-local command id for the generic DIT browser menu entry. Chosen above
/// Turbo Vision's standard `CM_*` ids.
pub const CM_BROWSER: u16 = 1000;

/// App-local command id for the generic "Delete entry" menu action (M4).
pub const CM_DELETE: u16 = 1001;

/// App-local command id base for per-profile menu entries. Profile *i* gets
/// `CM_PROFILE_BASE + i`.
pub const CM_PROFILE_BASE: u16 = 1100;

/// A backend-agnostic UI intent surfaced by the facade's event handling and
/// consumed by the main loop. Keeping it turbo-vision-free is what lets
/// `main.rs` react to outline activation and menu commands without importing
/// `turbo_vision` (the facade translates raw TV events into these).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiAction {
    /// User activated (Enter) the node with this DN; the `loaded` flag decides
    /// expand-vs-read in [`crate::workflows::browser::on_select`].
    Activate { dn: String, loaded: bool },
    /// User chose "New <profile #i>" from the menu (profile index).
    NewEntry(usize),
    /// User chose "Delete" for the currently selected entry DN.
    DeleteEntry(String),
    /// Nothing actionable.
    None,
}

/// What the manual event loop hands the single `on_event` callback each turn:
/// either an idle tick (drain the worker channel) or a resolved [`UiAction`].
/// One callback (rather than two) so the caller can own `&mut` state without a
/// double-mutable-borrow conflict between idle and action handling.
pub enum LoopEvent {
    /// An idle tick: drain the worker's response channel.
    Idle,
    /// A resolved user action (outline activation / menu command).
    Action(UiAction),
}

/// Map a menu command id (as emitted by the facade) to a [`UiAction`], given the
/// number of configured profiles and the currently selected node DN. Pure and
/// turbo-vision-free so it can be unit-tested and called from the facade.
///
/// `CM_DELETE` → `DeleteEntry(selected)`; `CM_PROFILE_BASE + i` → `NewEntry(i)`;
/// anything else (including `CM_BROWSER`/`CM_QUIT`, handled elsewhere) → `None`.
pub fn menu_action(command: u16, profile_count: usize, selected_dn: Option<&str>) -> UiAction {
    if command == CM_DELETE {
        return match selected_dn {
            Some(dn) if !dn.is_empty() => UiAction::DeleteEntry(dn.to_string()),
            _ => UiAction::None,
        };
    }
    if command >= CM_PROFILE_BASE {
        let i = (command - CM_PROFILE_BASE) as usize;
        if i < profile_count {
            return UiAction::NewEntry(i);
        }
    }
    UiAction::None
}

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
        label: "Delete".to_string(),
        command: CM_DELETE,
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
        assert_eq!(labels, vec!["Users", "Groups", "Browser", "Delete", "Quit"]);
        assert_eq!(defs.last().unwrap().command, CM_QUIT);
        assert_eq!(defs[0].command, CM_PROFILE_BASE);
        assert_eq!(defs[1].command, CM_PROFILE_BASE + 1);
        assert_eq!(defs[2].command, CM_BROWSER);
    }

    #[test]
    fn menu_defs_with_no_profiles_still_has_browser_and_quit() {
        let defs = build_menu_defs(&[]);
        let labels: Vec<&str> = defs.iter().map(|d| d.label.as_str()).collect();
        assert_eq!(labels, vec!["Browser", "Delete", "Quit"]);
    }

    #[test]
    fn menu_action_maps_profile_delete_and_unknown() {
        // Profile command -> NewEntry(index).
        assert_eq!(menu_action(CM_PROFILE_BASE, 2, None), UiAction::NewEntry(0));
        assert_eq!(
            menu_action(CM_PROFILE_BASE + 1, 2, None),
            UiAction::NewEntry(1)
        );
        // Out-of-range profile index -> None.
        assert_eq!(menu_action(CM_PROFILE_BASE + 9, 2, None), UiAction::None);
        // Delete with a selection -> DeleteEntry.
        assert_eq!(
            menu_action(CM_DELETE, 2, Some("cn=x,dc=example,dc=org")),
            UiAction::DeleteEntry("cn=x,dc=example,dc=org".to_string())
        );
        // Delete without a selection -> None.
        assert_eq!(menu_action(CM_DELETE, 2, None), UiAction::None);
        // Browser is handled elsewhere -> None here.
        assert_eq!(menu_action(CM_BROWSER, 2, None), UiAction::None);
    }
}
