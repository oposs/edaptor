//! Application-level domain logic: menu assembly from config profiles.
//!
//! This module is tty-free. It produces backend-agnostic [`MenuDef`]s that the
//! ratatui UI ([`crate::ui::view::menu_bar`]) renders as the top menu bar, and a
//! [`UiAction`] vocabulary the event loop ([`crate::ui::app`]) services. Pure and
//! unit-tested; no terminal needed.

use crate::config::EntryProfile;

/// The quit command id. A plain constant in the menu vocabulary; the ratatui UI
/// renders it as the `[Alt+X] Quit` menu entry and quits on Alt+X / Ctrl+C.
pub const CM_QUIT: u16 = 24;

/// App-local command id for the generic "Delete entry" menu action (M4).
pub const CM_DELETE: u16 = 1001;

/// App-local command id for the "Refresh" menu action (M6): re-run the eager
/// structure scan and rebuild the three panes.
pub const CM_REFRESH: u16 = 1002;

/// App-local command id base for per-profile menu entries. Profile *i* gets
/// `CM_PROFILE_BASE + i`.
pub const CM_PROFILE_BASE: u16 = 1100;

/// A backend-agnostic UI intent produced by the event loop's key dispatch and
/// the menu (via [`menu_action`]) and serviced by [`crate::ui::app::handle_action`].
/// Keeping it framework-free is what lets the menu/key layer stay testable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiAction {
    /// User chose "New <profile #i>" from the menu (profile index).
    NewEntry(usize),
    /// User chose "Delete" for the currently selected entry DN.
    DeleteEntry(String),
    /// The form pane's Save button fired (three-pane editor).
    FormSave,
    /// The form pane's Cancel button fired (three-pane editor).
    FormCancel,
    /// User chose "Refresh": re-run the eager structure scan.
    Refresh,
    /// Nothing actionable.
    None,
}

/// Map a menu command id (from the menu bar / Alt+digit keys) to a [`UiAction`],
/// given the number of configured profiles and the currently selected node DN.
/// Pure and framework-free so it can be unit-tested and called from the UI.
///
/// `CM_DELETE` → `DeleteEntry(selected)`; `CM_REFRESH` → `Refresh`;
/// `CM_PROFILE_BASE + i` → `NewEntry(i)`; anything else (including `CM_QUIT`,
/// handled elsewhere) → `None`.
pub fn menu_action(command: u16, profile_count: usize, selected_dn: Option<&str>) -> UiAction {
    if command == CM_DELETE {
        return match selected_dn {
            Some(dn) if !dn.is_empty() => UiAction::DeleteEntry(dn.to_string()),
            _ => UiAction::None,
        };
    }
    if command == CM_REFRESH {
        return UiAction::Refresh;
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
        label: "Delete".to_string(),
        command: CM_DELETE,
    });
    defs.push(MenuDef {
        label: "Refresh".to_string(),
        command: CM_REFRESH,
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
        assert_eq!(labels, vec!["Users", "Groups", "Delete", "Refresh", "Quit"]);
        assert_eq!(defs.last().unwrap().command, CM_QUIT);
        assert_eq!(defs[0].command, CM_PROFILE_BASE);
        assert_eq!(defs[1].command, CM_PROFILE_BASE + 1);
        assert_eq!(defs[2].command, CM_DELETE);
        assert_eq!(defs[3].command, CM_REFRESH);
    }

    #[test]
    fn menu_defs_with_no_profiles_still_has_delete_refresh_and_quit() {
        let defs = build_menu_defs(&[]);
        let labels: Vec<&str> = defs.iter().map(|d| d.label.as_str()).collect();
        assert_eq!(labels, vec!["Delete", "Refresh", "Quit"]);
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
        // Refresh -> Refresh (no selection needed).
        assert_eq!(menu_action(CM_REFRESH, 2, None), UiAction::Refresh);
        // An unknown command id -> None.
        assert_eq!(menu_action(CM_QUIT, 2, None), UiAction::None);
    }
}
