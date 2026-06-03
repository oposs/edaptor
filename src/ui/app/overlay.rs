//! Modal overlay state and the deferred-action enums it carries.

use std::collections::BTreeMap;

use crate::form::changeset::ModOp;
use crate::form::validate::SavePlan;
use crate::ui::edit_form::ValueEditor;
use crate::workflows::structure::StructureInput;

use super::Pane;

/// A modal overlay drawn on top of the panes; while one is open it captures all
/// keys (plan §3.4).
pub enum Overlay {
    /// A yes/no confirmation (e.g. the save LDIF preview) carrying the action to
    /// run on confirm.
    Confirm {
        /// Dialog title.
        title: String,
        /// Body text (e.g. the LDIF preview).
        body: String,
        /// What to do when the user confirms.
        action: PendingAction,
    },
    /// An error message; any key dismisses it.
    Error {
        /// The message to show.
        text: String,
    },
    /// The multi-value popup editor (Enter on a multi field).
    ValueEditor(ValueEditor),
    /// The Save/Discard/Stay guard shown when leaving a dirty form — by changing
    /// the selection, moving focus off the form pane, or quitting. Carries the
    /// pending [`GuardIntent`] to resume once the user chooses.
    Guard {
        /// What to do once the guard is resolved.
        intent: GuardIntent,
    },
    /// Alt+N profile chooser: pick which template to create. Each entry is the
    /// profile's `(index, name)`; `sel` is the highlighted row. The name is carried
    /// here so the render layer (which lacks `profiles`) can show it.
    ChooseProfile {
        /// The offered profiles as `(profile_index, name)`, in display order.
        entries: Vec<(usize, String)>,
        /// The highlighted row (into `entries`).
        sel: usize,
    },
}

/// What a dirty-form [`Overlay::Guard`] should resume once the user resolves it.
#[derive(Clone)]
pub enum GuardIntent {
    /// Navigate the form to a leaf DN (`None` clears it) — the selection-change
    /// guard fired from `reconcile`.
    Nav(Option<String>),
    /// Move focus to another pane — the Tab/Shift-Tab-off-the-form guard.
    Focus(Pane),
    /// Quit the application — the Alt+X / quit-while-dirty guard.
    Quit,
}

/// What a confirmed [`Overlay::Confirm`] (or resolved [`Overlay::Guard`]) should
/// do once the worker is available.
pub enum PendingAction {
    /// Submit a prepared save plan against `dn`; `nav` is a deferred navigation
    /// target (set when a guard's Save resolves), serviced after the write.
    Save {
        /// The save plan to submit on confirm.
        plan: SavePlan,
        /// The (old) DN the plan targets.
        dn: String,
        /// A deferred navigation target (the entry to move to after the save).
        nav: Option<String>,
    },
    /// Submit an `Add` for a newly created entry, then splice it into the tree.
    Create {
        /// The new entry's DN.
        dn: String,
        /// The new entry's attributes.
        attrs: BTreeMap<String, Vec<String>>,
        /// The container DN the entry is added under (for the structure splice).
        parent: String,
    },
    /// Submit a `Delete` for `dn`, then reflow the structure.
    Delete {
        /// The DN to delete.
        dn: String,
    },
    /// Open a Create-mode form for the chosen profile (resolved from the Alt+N
    /// profile chooser, which lacks the schema/profiles to build the form itself).
    OpenCreate {
        /// The chosen profile index.
        profile_idx: usize,
    },
    /// A resolved dirty-form guard: perform `intent`, running the save flow first
    /// when `save` is true (Save) or proceeding directly when false (Discard).
    ResolveGuard {
        /// What to do (navigate / change focus / quit).
        intent: GuardIntent,
        /// Whether to save the dirty form before performing the intent.
        save: bool,
    },
    /// A combined membership save: own-entry MODIFY + per-holder fan-out MODIFYs,
    /// applied synchronously (spec §6.3). `entry_dn` is the edited candidate entry.
    CombinedSave {
        /// The candidate entry being saved.
        entry_dn: String,
        /// Own-entry attribute mods (empty when only membership changed).
        own_mods: Vec<ModOp>,
        /// Per-holder fan-out: (group_dn, Add/Delete op for holder_attr).
        fanout: Vec<(String, ModOp)>,
        /// A dirty-form guard intent to perform after a successful save (set when
        /// the combined save is reached via a Save-then-resume guard); `None` for
        /// a plain Alt+S save.
        then_intent: Option<GuardIntent>,
    },
}

/// What the run-loop should do when a write's `WriteOk` arrives, keyed by id.
pub(crate) enum PostWrite {
    /// A form save (Modify / RenameOnly): re-read `reread_dn` into the form,
    /// unless `nav` is set (a guard Save) in which case navigate there instead.
    Save {
        /// The DN to re-read once the write succeeds.
        reread_dn: String,
        /// A deferred navigation target (the entry the user moved to while dirty).
        nav: Option<String>,
        /// Quit once the write succeeds (a quit-while-dirty guard's Save).
        then_quit: bool,
    },
    /// A create: splice the new entry into the eager [`Structure`] under `parent`.
    Created {
        /// The container the entry was added under.
        parent: String,
        /// The new entry's structure row.
        input: StructureInput,
    },
    /// A delete: drop `dn` from the [`Structure`] and reflow.
    Deleted {
        /// The removed entry's DN.
        dn: String,
    },
}
