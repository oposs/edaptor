//! Shared application state for the tvision UI and the blocking bootstrap that
//! builds it. State is held in `Rc<RefCell<UiState>>` (alias `Shared` in mod.rs).

use anyhow::{anyhow, Result};

use crate::config::tree_label::CompiledTreeRule;
use crate::config::{Config, EntryProfile};
use crate::ldap::worker::{Request, Response, WorkerHandle};
use crate::schema::SchemaModel;
use crate::workflows::alloc_flow::{AllocFlow, AllocOutcome};
use crate::workflows::edit_form::{build_edit_form, EditForm};
use crate::workflows::labels::LabelRule;
use crate::workflows::read_flow::ReadFlow;
use crate::workflows::structure::Structure;
#[cfg(test)]
use crate::workflows::structure::StructureInput;
use crate::workflows::write_flow::{WriteFlow, WriteOutcome};

/// Placeholder text set in autonumber fields while the background scan is pending.
pub const ALLOC_PLACEHOLDER: &str = "‹allocating…›";

/// A dirty-blocked navigation awaiting the guard's decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardTarget {
    Leaf(String, Vec<String>),
    Branch(String),
}

/// What `pump_worker` wants the pump view to do after draining responses.
#[derive(Debug, Default, Clone, Copy)]
pub struct PumpResult {
    pub changed: bool,
    pub quit: bool,
    pub error: bool,
}

/// Everything the panes read/write, behind a single RefCell.
pub struct UiState {
    /// `None` only in headless unit tests.
    pub worker: Option<WorkerHandle>,
    pub read_flow: ReadFlow,
    pub structure: Structure,
    pub base_dn: String,
    pub profiles: Vec<EntryProfile>,
    pub label_rules: Vec<LabelRule>,
    pub tree_rules: Vec<CompiledTreeRule>,
    /// DFS pre-order index → branch DN, matching `Outline`'s `foc` numbering.
    pub branch_dns: Vec<String>,
    pub current_branch: Option<String>,
    pub current_leaf: Option<String>,
    pub search: String,
    /// The loaded editable form (None until a leaf is read).
    pub edit_form: Option<EditForm>,
    /// Async write flow (validate/diff/submit/correlate).
    pub write_flow: WriteFlow,
    /// Async autonumber allocation flow (scan + pick next-free).
    pub alloc_flow: AllocFlow,
    /// Read-only mode disables editing and the save path.
    pub read_only: bool,
    /// Transient status text (e.g. "Saved.").
    pub status: String,
    /// True when a pane must re-render the form from `edit_form`.
    pub form_needs_render: bool,
    /// A dirty-blocked navigation awaiting the guard's decision.
    pub guard_target: Option<GuardTarget>,
    /// Where to navigate after a guard-Save completes: (dn, objectClasses).
    pub pending_nav: Option<(String, Vec<String>)>,
    /// Last async write error, surfaced by the dispatch closure's Error dialog.
    pub last_write_error: Option<String>,
    pub list_dirty: bool,
    /// Pane → controller: the leaf a selector pane wants shown (dn + objectClasses).
    /// Set when the highlight moves; consumed by [`reconcile_selection`]. The panes
    /// never load or guard themselves — they only record this intent.
    pub requested_leaf: Option<(String, Vec<String>)>,
    /// Controller → leaf pane: force the list highlight to this row on the pane's
    /// next event (used to snap the highlight back to `current_leaf` after a guard
    /// "Stay", so highlight and form always agree).
    pub set_leaf_row: Option<i32>,
    /// Pane → controller: the branch a selector pane wants shown (dn).
    /// Set when the tree highlight moves; consumed by [`reconcile_branch`].
    pub requested_branch: Option<String>,
    /// Controller → tree pane: force the tree highlight to this row on the pane's
    /// next event (used to snap back to `current_branch` after a guard "Stay").
    pub set_tree_row: Option<i32>,
    /// Form pane → controller: the field index whose modal editor should open on
    /// the next `ACTIVATE`. Set by the pane, consumed by `app::dispatch`.
    pub activate_field: Option<usize>,
    /// Modal editor → controller: the prospective commit an open editor would
    /// apply. Maintained live by the editor view; applied by `dispatch` on OK.
    pub staged_commit: Option<crate::tui::widget::CommitOutcome>,
    /// Profile chooser → controller: the index the user highlighted when OK was
    /// pressed. Set by `ProfileChooser`; read by `dispatch` to select a profile.
    pub chosen_profile: Option<usize>,
    /// True when the LDAP connection is encrypted (LDAPS, StartTLS, or ldapi://).
    /// The password widget refuses to operate when this is false.
    pub connection_encrypted: bool,
    /// Pre-resolved `[profile.widget.*]` bindings; built once in `bootstrap` via
    /// `config::widget::resolve_widgets` and used by `apply_widget_bindings` every
    /// time a form is opened.
    pub resolved_widgets: Vec<crate::config::widget::ResolvedWidget>,
}

impl UiState {
    pub fn current_leaf_dn(&self) -> Option<&str> {
        self.current_leaf.as_deref()
    }

    /// Test-only constructor: a worker-less state over a pre-built Structure and
    /// schema. `pump_worker` returns false (no worker). Added to in later tasks.
    #[cfg(test)]
    pub fn new_for_test(
        structure: Structure,
        schema: SchemaModel,
        base_dn: String,
        label_rules: Vec<LabelRule>,
        tree_rules: Vec<CompiledTreeRule>,
    ) -> Self {
        UiState {
            worker: None,
            read_flow: ReadFlow::new(schema),
            structure,
            base_dn,
            profiles: Vec::new(),
            label_rules,
            tree_rules,
            branch_dns: Vec::new(),
            current_branch: None,
            current_leaf: None,
            search: String::new(),
            edit_form: None,
            write_flow: WriteFlow::new(),
            alloc_flow: AllocFlow::new(),
            read_only: false,
            status: String::new(),
            form_needs_render: false,
            guard_target: None,
            pending_nav: None,
            last_write_error: None,
            list_dirty: false,
            requested_leaf: None,
            set_leaf_row: None,
            requested_branch: None,
            set_tree_row: None,
            activate_field: None,
            staged_commit: None,
            chosen_profile: None,
            connection_encrypted: false,
            resolved_widgets: Vec::new(),
        }
    }
}

impl UiState {
    /// Drain ready worker responses: install a fresh `EditForm` on a read, and
    /// apply write outcomes. Returns what the pump view should do.
    pub fn pump_worker(&mut self) -> PumpResult {
        use crate::workflows::read_flow::ReadOutcome;
        let mut resps = Vec::new();
        if let Some(w) = self.worker.as_ref() {
            while let Some(r) = w.poll() {
                resps.push(r);
            }
        }
        let mut out = PumpResult::default();
        for resp in &resps {
            // Reads first (Entries/SearchError); disjoint from write variants.
            match self.read_flow.on_response(resp) {
                ReadOutcome::Form {
                    model,
                    object_classes,
                } => {
                    let mut form = build_edit_form(&model, self.read_flow.schema(), self.read_only);
                    form.object_classes = object_classes;
                    {
                        // Build a resolver from &self fields (disjoint from the local
                        // `form`); apply profile-driven bindings before installing.
                        let ocs = form.object_classes.clone();
                        let resolver = crate::config::resolver::WidgetResolver::new(
                            self.read_flow.schema(),
                            &self.profiles,
                            &self.resolved_widgets,
                            self.read_only,
                        );
                        crate::workflows::widget_bind::apply_widget_bindings(
                            &mut form, &resolver, &ocs,
                        );
                    }
                    self.edit_form = Some(form);
                    self.form_needs_render = true;
                    out.changed = true;
                    continue;
                }
                ReadOutcome::Error(msg) => {
                    self.status = msg.clone();
                    out.changed = true;
                    continue;
                }
                ReadOutcome::Ignored => {}
            }
            // Autonumber allocations: Entries/SearchError with alloc-range IDs.
            let alloc_out = self.alloc_flow.on_response(resp);
            if !matches!(alloc_out, AllocOutcome::Ignored) {
                self.apply_alloc_outcome(alloc_out);
                out.changed = true;
            }
            // Then writes (WriteOk/WriteError).
            let outcome = self.write_flow.on_response(resp);
            if !matches!(outcome, WriteOutcome::Ignored) {
                let r = self.apply_write_outcome(outcome);
                out.changed |= r.changed;
                out.quit |= r.quit;
                out.error |= r.error;
            }
        }
        out
    }

    /// Apply one non-ignored alloc outcome to state.
    ///
    /// `Filled`: if the matching field is empty or shows the `‹allocating…›`
    /// placeholder, replace it with the allocated value and flag a re-render.
    /// `Failed`: set the status message, clear any placeholder (leave empty), flag
    /// a re-render.
    pub fn apply_alloc_outcome(&mut self, out: AllocOutcome) {
        match out {
            AllocOutcome::Filled { attr, value } => {
                if let Some(form) = self.edit_form.as_mut() {
                    if let Some(f) = form
                        .fields
                        .iter_mut()
                        .find(|f| f.label.eq_ignore_ascii_case(&attr))
                    {
                        if f.values.is_empty() || f.values == [ALLOC_PLACEHOLDER] {
                            f.values = vec![value];
                            self.form_needs_render = true;
                        }
                    }
                }
            }
            AllocOutcome::Failed { attr, msg } => {
                self.status = msg;
                if let Some(form) = self.edit_form.as_mut() {
                    if let Some(f) = form
                        .fields
                        .iter_mut()
                        .find(|f| f.label.eq_ignore_ascii_case(&attr))
                    {
                        if f.values == [ALLOC_PLACEHOLDER] {
                            f.values = Vec::new();
                        }
                    }
                }
                self.form_needs_render = true;
            }
            AllocOutcome::Ignored => {}
        }
    }

    /// Apply one non-ignored write outcome to state, returning the pump action.
    pub fn apply_write_outcome(&mut self, outcome: WriteOutcome) -> PumpResult {
        let mut out = PumpResult {
            changed: true,
            ..Default::default()
        };
        match outcome {
            WriteOutcome::Saved {
                reread_dn,
                quit_after,
            } => {
                self.status = "Saved.".to_string();
                if quit_after {
                    out.quit = true;
                    return out;
                }
                // Navigate to the guard's target if one is pending, else re-read.
                let (dn, profile_ocs) = self.pending_nav.take().unwrap_or((reread_dn, Vec::new()));
                self.reread(&dn, &profile_ocs);
            }
            WriteOutcome::NeedFollowupModify {
                dn,
                mods,
                quit_after,
            } => {
                if let Some(w) = self.worker.as_ref() {
                    let _ = self.write_flow.submit_followup(w, &dn, mods, quit_after);
                }
            }
            WriteOutcome::Error(msg) => {
                self.last_write_error = Some(msg);
                out.error = true;
            }
            WriteOutcome::Ignored => out.changed = false,
            WriteOutcome::Created { dn, quit_after } => {
                let ocs = self
                    .edit_form
                    .as_ref()
                    .map(|f| f.object_classes.clone())
                    .unwrap_or_default();
                self.current_leaf = Some(dn.clone());
                self.list_dirty = true;
                self.edit_form = None; // re-read reloads it in Edit mode
                if self.worker.is_some() {
                    self.reread_public(&dn, &ocs);
                }
                return PumpResult {
                    changed: true,
                    quit: quit_after,
                    error: false,
                };
            }
        }
        out
    }

    /// Public wrapper around the private `reread` for the dispatch closure.
    pub fn reread_public(&mut self, dn: &str, ocs: &[String]) {
        self.reread(dn, ocs);
    }

    /// Submit a base-scope re-read of `dn`, selecting a profile by `ocs`.
    fn reread(&mut self, dn: &str, ocs: &[String]) {
        let Self {
            worker,
            read_flow,
            profiles,
            current_leaf,
            ..
        } = self;
        if let Some(w) = worker.as_ref() {
            let profile = profile_for(profiles, ocs);
            if read_flow.request_entry(w, dn, profile).is_ok() {
                *current_leaf = Some(dn.to_string());
            }
        }
    }

    /// Apply a modal editor's typed `CommitOutcome` to the loaded form. For the
    /// resync variant: write the objectClass field values, mirror them into
    /// `object_classes`, then regenerate fields. Reads schema from `read_flow`
    /// (split-borrow so `edit_form` and `read_flow` are borrowed disjointly).
    pub fn apply_commit(&mut self, field_idx: usize, outcome: crate::tui::widget::CommitOutcome) {
        use crate::tui::widget::CommitOutcome;
        let UiState {
            edit_form,
            read_flow,
            form_needs_render,
            ..
        } = self;
        match outcome {
            CommitOutcome::SetValues(vals) => {
                if let Some(form) = edit_form.as_mut() {
                    if let Some(f) = form.fields.get_mut(field_idx) {
                        f.values = vals;
                    }
                }
            }
            CommitOutcome::SetValuesThenResyncSchema(ocs) => {
                if let Some(form) = edit_form.as_mut() {
                    if let Some(f) = form.fields.get_mut(field_idx) {
                        f.values = ocs.clone();
                    }
                    // The objectClass FIELD's values are authoritative for
                    // `sync_schema_fields`; `object_classes` is the mirror kept for
                    // the save path.
                    form.object_classes = ocs;
                    form.sync_schema_fields(read_flow.schema());
                }
            }
            // StageSecret is M4 (password); no-op here.
            CommitOutcome::StageSecret { .. } => {}
            CommitOutcome::Cancelled => {}
        }
        *form_needs_render = true;
    }

    /// Pane → controller: record that the user moved the highlight to `dn`. The
    /// selector panes call this and nothing else; the load/guard decision is the
    /// controller's ([`reconcile_selection`](Self::reconcile_selection)).
    pub fn request_leaf(&mut self, dn: String, ocs: Vec<String>) {
        self.requested_leaf = Some((dn, ocs));
    }

    /// Controller: reconcile a pending [`requested_leaf`](Self::requested_leaf)
    /// against the entry currently shown in the form. Returns `true` when the
    /// caller (the pump) must raise the dirty guard.
    ///
    /// - already showing it / nothing requested → clear, `false`.
    /// - form **clean** → load it now (the form follows the highlight), `false`.
    /// - form **dirty** → stash it as the guard target and return `true`; the form
    ///   stays pinned until the guard's decision (the pump posts `GUARD_NAV`).
    pub fn reconcile_selection(&mut self) -> bool {
        let Some((dn, ocs)) = self.requested_leaf.take() else {
            return false;
        };
        if self.current_leaf.as_deref() == Some(dn.as_str()) {
            return false;
        }
        let dirty = self
            .edit_form
            .as_ref()
            .map(|f| f.is_dirty())
            .unwrap_or(false);
        if dirty {
            self.guard_target = Some(GuardTarget::Leaf(dn, ocs));
            true
        } else {
            self.reread(&dn, &ocs);
            false
        }
    }

    /// Pane → controller: record that the user moved the tree highlight to `dn`.
    pub fn request_branch(&mut self, dn: String) {
        self.requested_branch = Some(dn);
    }

    /// The DFS row index of `current_branch` in `branch_dns`, or `None`.
    /// Used to snap the tree highlight back to the pinned branch on a guard "Stay".
    pub fn current_branch_row(&self) -> Option<i32> {
        let cur = self.current_branch.as_deref()?;
        self.branch_dns
            .iter()
            .position(|d| d == cur)
            .map(|i| i as i32)
    }

    /// Controller: reconcile a pending [`requested_branch`](Self::requested_branch)
    /// against the currently-shown branch. Returns `true` when the caller (the pump)
    /// must raise the dirty guard.
    ///
    /// - nothing requested → `false`.
    /// - same as `current_branch` → `false` (no-op).
    /// - form **clean** → switch (`current_branch = dn`, `list_dirty = true`), `false`.
    /// - form **dirty** → stash [`GuardTarget::Branch`] and return `true`; the form
    ///   stays pinned until the guard's decision.
    pub fn reconcile_branch(&mut self) -> bool {
        let Some(dn) = self.requested_branch.take() else {
            return false;
        };
        if self.current_branch.as_deref() == Some(dn.as_str()) {
            return false;
        }
        let dirty = self
            .edit_form
            .as_ref()
            .map(|f| f.is_dirty())
            .unwrap_or(false);
        if dirty {
            self.guard_target = Some(GuardTarget::Branch(dn));
            true
        } else {
            self.current_branch = Some(dn);
            self.list_dirty = true;
            false
        }
    }
}

impl UiState {
    /// (label, dn) rows for the current branch, filtered by `search`, using the
    /// configured column-2 label rules. Empty when no branch is selected.
    pub fn leaf_rows(&self) -> Vec<(String, String)> {
        match &self.current_branch {
            Some(b) => crate::workflows::labels::compute_rows(
                &self.structure,
                b,
                &self.search,
                &self.label_rules,
            ),
            None => Vec::new(),
        }
    }

    /// The list row index of the entry currently shown in the form (`current_leaf`),
    /// or `None` if it is not in the current rows. Used to snap the highlight back
    /// to the pinned form on a guard "Stay".
    pub fn current_leaf_row(&self) -> Option<i32> {
        let cur = self.current_leaf.as_deref()?;
        self.leaf_rows()
            .iter()
            .position(|(_l, dn)| dn == cur)
            .map(|i| i as i32)
    }
}

/// First profile whose declared object_classes are all present on the entry.
pub fn profile_for<'a>(profiles: &'a [EntryProfile], ocs: &[String]) -> Option<&'a EntryProfile> {
    profiles.iter().find(|p| {
        !p.object_classes.is_empty()
            && p.object_classes
                .iter()
                .all(|need| ocs.iter().any(|have| have.eq_ignore_ascii_case(need)))
    })
}

/// Blocking startup: spawn the worker, fetch schema + eager structure, build the
/// compiled label rules and the ReadFlow. Mirrors `ui::app::run`'s bootstrap.
pub(crate) fn bootstrap(config: Config, password: String) -> Result<UiState> {
    use crate::workflows::labels::{label_rules, structure_inputs, structure_scan_attrs};
    let base_dn = config.server.base_dn.clone();
    let profiles = config.profiles.clone();
    let resolved_widgets = crate::config::widget::resolve_widgets(&profiles)
        .map_err(|e| anyhow!("widget config error: {e}"))?;
    let label_rules = label_rules(&profiles);
    let tree_rules = crate::config::tree_label::compile_tree_rules(&config.tree);
    // Fetch the attributes the label/tree templates reference, so labels render.
    let scan_attrs = structure_scan_attrs(&label_rules, &tree_rules);

    let connection_encrypted = config.is_encrypted();
    let worker = WorkerHandle::spawn(config, password)?;

    let raw = match worker.request(Request::FetchSubschema)? {
        Response::Subschema(raw) => raw,
        other => return Err(anyhow!("FetchSubschema: unexpected {other:?}")),
    };
    let schema = SchemaModel::from_raw(&raw);

    let nodes = match worker.request(Request::LoadStructure {
        id: 0,
        base: base_dn.clone(),
        page_size: 500,
        attrs: scan_attrs,
    })? {
        Response::StructureEntries { nodes, .. } => nodes,
        other => return Err(anyhow!("LoadStructure: unexpected {other:?}")),
    };
    let structure = Structure::build(&base_dn, structure_inputs(nodes));

    Ok(UiState {
        worker: Some(worker),
        read_flow: ReadFlow::new(schema),
        structure,
        base_dn,
        profiles,
        label_rules,
        tree_rules,
        branch_dns: Vec::new(),
        current_branch: None,
        current_leaf: None,
        search: String::new(),
        edit_form: None,
        write_flow: WriteFlow::new(),
        alloc_flow: AllocFlow::new(),
        read_only: false,
        status: String::new(),
        form_needs_render: false,
        guard_target: None,
        pending_nav: None,
        last_write_error: None,
        list_dirty: false,
        requested_leaf: None,
        set_leaf_row: None,
        requested_branch: None,
        set_tree_row: None,
        activate_field: None,
        staged_commit: None,
        chosen_profile: None,
        connection_encrypted,
        resolved_widgets,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldap::worker::RawSubschema;
    use std::collections::BTreeMap;

    fn si(dn: &str, child_hint: Option<&str>) -> StructureInput {
        StructureInput {
            dn: dn.into(),
            cn: child_hint.map(Into::into),
            description: None,
            object_classes: vec![],
            attrs: BTreeMap::new(),
        }
    }

    #[test]
    fn test_profile_for_matches_all_ocs() {
        let mut p = crate::config::EntryProfile {
            name: "user".into(),
            object_classes: vec!["inetOrgPerson".into()],
            rdn_attr: String::new(),
            search_base: String::new(),
            show: vec![],
            search_attrs: vec![],
            defaults: Default::default(),
            widgets: Default::default(),
            label: None,
        };
        let profiles = vec![p.clone()];
        assert!(profile_for(&profiles, &["inetOrgPerson".into(), "top".into()]).is_some());
        assert!(profile_for(&profiles, &["organizationalUnit".into()]).is_none());
        p.object_classes.clear();
        assert!(profile_for(&[p], &["anything".into()]).is_none());
    }

    #[test]
    fn test_state_starts_empty() {
        let structure = Structure::build("dc=x", vec![si("dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let st = UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        assert!(st.current_leaf_dn().is_none());
        assert!(st.edit_form.is_none());
        assert!(!st.list_dirty);
    }

    #[test]
    fn test_pump_worker_noop_without_worker() {
        let structure = Structure::build("dc=x", vec![si("dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        assert!(!st.pump_worker().changed);
        assert!(st.edit_form.is_none());
    }

    #[test]
    fn apply_commit_resyncs_on_objectclass_change() {
        use crate::schema::FieldKind;
        use crate::tui::widget::CommitOutcome;
        use crate::workflows::edit_form::{EditField, EditForm, FormMode};
        use crate::workflows::form_model::WidgetSpec;

        // Minimal schema with person (MUST sn,cn MAY description).
        let raw = crate::ldap::worker::RawSubschema {
            object_classes: vec![
                "( 2.5.6.0 NAME 'top' ABSTRACT MUST objectClass )".into(),
                "( 2.5.6.6 NAME 'person' SUP top STRUCTURAL MUST ( sn $ cn ) MAY description )"
                    .into(),
            ],
            attribute_types: vec![
                "( 2.5.4.3 NAME 'cn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 SINGLE-VALUE )".into(),
                "( 2.5.4.4 NAME 'sn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".into(),
                "( 2.5.4.13 NAME 'description' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".into(),
            ],
            ldap_syntaxes: vec![],
        };
        let schema = crate::schema::SchemaModel::from_raw(&raw);
        let structure = Structure::build("dc=example,dc=org", vec![]);
        let mut st = UiState::new_for_test(
            structure,
            schema,
            "dc=example,dc=org".into(),
            Vec::new(),
            Vec::new(),
        );
        let oc_field = EditField {
            label: "objectClass".into(),
            must: true,
            editable: false,
            multi: true,
            secret: false,
            ordered: false,
            orphaned: false,
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: None,
            values: vec!["top".into()],
            baseline: vec!["top".into()],
        };
        st.edit_form = Some(EditForm {
            dn: "cn=Bob,dc=example,dc=org".into(),
            mode: FormMode::Edit,
            object_classes: vec!["top".into()],
            fields: vec![oc_field],
        });

        // Commit "top, person": objectClass values updated, fields injected, render flagged.
        st.apply_commit(
            0,
            CommitOutcome::SetValuesThenResyncSchema(vec!["top".into(), "person".into()]),
        );
        let form = st.edit_form.as_ref().unwrap();
        assert_eq!(
            form.object_classes,
            vec!["top".to_string(), "person".to_string()]
        );
        let oc = form
            .fields
            .iter()
            .find(|f| f.label == "objectClass")
            .unwrap();
        assert_eq!(oc.values, vec!["top".to_string(), "person".to_string()]);
        assert!(
            form.fields.iter().any(|f| f.label == "sn"),
            "MUST attr sn injected"
        );
        assert!(
            form.fields.iter().any(|f| f.label == "cn"),
            "MUST attr cn injected"
        );
        assert!(st.form_needs_render);
    }

    /// Fix 3 (T4): `CommitOutcome::SetValues` must update the field's values and
    /// flag `form_needs_render`, without touching `object_classes` or resync.
    #[test]
    fn apply_commit_set_values_updates_field_and_flags_render() {
        use crate::schema::FieldKind;
        use crate::tui::widget::CommitOutcome;
        use crate::workflows::edit_form::{EditField, EditForm, FormMode};
        use crate::workflows::form_model::WidgetSpec;

        let raw = crate::ldap::worker::RawSubschema::default();
        let schema = crate::schema::SchemaModel::from_raw(&raw);
        let structure = Structure::build("dc=example,dc=org", vec![]);
        let mut st = UiState::new_for_test(
            structure,
            schema,
            "dc=example,dc=org".into(),
            Vec::new(),
            Vec::new(),
        );
        let plain_field = EditField {
            label: "description".into(),
            must: false,
            editable: true,
            multi: false,
            secret: false,
            ordered: false,
            orphaned: false,
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: None,
            values: vec!["old".into()],
            baseline: vec!["old".into()],
        };
        st.edit_form = Some(EditForm {
            dn: "cn=test,dc=example,dc=org".into(),
            mode: FormMode::Edit,
            object_classes: vec!["top".into()],
            fields: vec![plain_field],
        });

        st.apply_commit(0, CommitOutcome::SetValues(vec!["newval".into()]));

        let form = st.edit_form.as_ref().unwrap();
        assert_eq!(
            form.fields[0].values,
            vec!["newval".to_string()],
            "SetValues must write the new values into the field"
        );
        assert!(
            st.form_needs_render,
            "SetValues must flag form_needs_render"
        );
        assert_eq!(
            form.object_classes,
            vec!["top".to_string()],
            "SetValues must not touch object_classes"
        );
    }

    /// TDD Step 1 (RED → GREEN): apply_alloc_outcome Filled replaces the
    /// ‹allocating…› placeholder with the allocated value and flags form_needs_render.
    #[test]
    fn apply_alloc_outcome_filled_sets_field_value() {
        use crate::schema::FieldKind;
        use crate::workflows::alloc_flow::AllocOutcome;
        use crate::workflows::edit_form::{EditField, EditForm, FormMode};
        use crate::workflows::form_model::WidgetSpec;

        let raw = RawSubschema::default();
        let schema = SchemaModel::from_raw(&raw);
        let structure = Structure::build("dc=x", vec![]);
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());

        let uid_field = EditField {
            label: "uidNumber".into(),
            must: false,
            editable: true,
            multi: false,
            secret: false,
            ordered: false,
            orphaned: false,
            kind: FieldKind::Integer,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: None,
            values: vec![ALLOC_PLACEHOLDER.to_string()],
            baseline: vec![],
        };
        st.edit_form = Some(EditForm {
            dn: String::new(),
            mode: FormMode::Create {
                profile_idx: 0,
                container: "ou=people,dc=x".into(),
            },
            object_classes: vec![],
            fields: vec![uid_field],
        });
        st.form_needs_render = false;

        st.apply_alloc_outcome(AllocOutcome::Filled {
            attr: "uidNumber".into(),
            value: "10006".into(),
        });

        let form = st.edit_form.as_ref().unwrap();
        assert_eq!(
            form.fields[0].values,
            vec!["10006".to_string()],
            "Filled should replace the placeholder with the allocated value"
        );
        assert!(st.form_needs_render, "form_needs_render must be set");
    }

    /// A field holding a user-typed value (NOT the placeholder) must be left
    /// untouched by a `Filled` outcome for that attribute.
    #[test]
    fn apply_alloc_outcome_does_not_clobber_user_value() {
        use crate::schema::FieldKind;
        use crate::workflows::alloc_flow::AllocOutcome;
        use crate::workflows::edit_form::{EditField, EditForm, FormMode};
        use crate::workflows::form_model::WidgetSpec;

        let raw = RawSubschema::default();
        let schema = SchemaModel::from_raw(&raw);
        let structure = Structure::build("dc=x", vec![]);
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());

        let uid_field = EditField {
            label: "uidNumber".into(),
            must: false,
            editable: true,
            multi: false,
            secret: false,
            ordered: false,
            orphaned: false,
            kind: FieldKind::Integer,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: None,
            // user already typed a real value — not the placeholder
            values: vec!["12345".to_string()],
            baseline: vec![],
        };
        st.edit_form = Some(EditForm {
            dn: String::new(),
            mode: FormMode::Create {
                profile_idx: 0,
                container: "ou=people,dc=x".into(),
            },
            object_classes: vec![],
            fields: vec![uid_field],
        });
        st.form_needs_render = false;

        st.apply_alloc_outcome(AllocOutcome::Filled {
            attr: "uidNumber".into(),
            value: "99999".into(),
        });

        let form = st.edit_form.as_ref().unwrap();
        assert_eq!(
            form.fields[0].values,
            vec!["12345".to_string()],
            "Filled must not overwrite a user-typed value"
        );
        // form_needs_render must remain false — no update happened
        assert!(
            !st.form_needs_render,
            "form_needs_render must not be set when no field was updated"
        );
    }

    /// Task 13 (RED → GREEN): new_for_test must default connection_encrypted to false.
    #[test]
    fn new_for_test_defaults_connection_encrypted_false() {
        let structure = Structure::build("dc=x", vec![si("dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let st = UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        assert!(
            !st.connection_encrypted,
            "connection_encrypted must default to false"
        );
    }

    /// `Failed { attr: "uidNumber", .. }` must clear only the uidNumber placeholder
    /// and leave gidNumber's placeholder intact.
    #[test]
    fn apply_alloc_outcome_failed_clears_only_that_field() {
        use crate::schema::FieldKind;
        use crate::workflows::alloc_flow::AllocOutcome;
        use crate::workflows::edit_form::{EditField, EditForm, FormMode};
        use crate::workflows::form_model::WidgetSpec;

        let raw = RawSubschema::default();
        let schema = SchemaModel::from_raw(&raw);
        let structure = Structure::build("dc=x", vec![]);
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());

        let make_placeholder_field = |label: &str| EditField {
            label: label.into(),
            must: false,
            editable: true,
            multi: false,
            secret: false,
            ordered: false,
            orphaned: false,
            kind: FieldKind::Integer,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: None,
            values: vec![ALLOC_PLACEHOLDER.to_string()],
            baseline: vec![],
        };

        st.edit_form = Some(EditForm {
            dn: String::new(),
            mode: FormMode::Create {
                profile_idx: 0,
                container: "ou=people,dc=x".into(),
            },
            object_classes: vec![],
            fields: vec![
                make_placeholder_field("uidNumber"),
                make_placeholder_field("gidNumber"),
            ],
        });
        st.form_needs_render = false;

        st.apply_alloc_outcome(AllocOutcome::Failed {
            attr: "uidNumber".into(),
            msg: "scan truncated".into(),
        });

        let form = st.edit_form.as_ref().unwrap();
        assert_eq!(
            form.fields[0].values,
            Vec::<String>::new(),
            "uidNumber placeholder must be cleared on failure"
        );
        assert_eq!(
            form.fields[1].values,
            vec![ALLOC_PLACEHOLDER.to_string()],
            "gidNumber placeholder must be left untouched"
        );
        assert_eq!(st.status, "scan truncated", "status must show the error");
        assert!(st.form_needs_render, "form_needs_render must be set");
    }
}

#[cfg(test)]
mod write_routing_tests {
    use super::*;
    use crate::ldap::worker::RawSubschema;
    use crate::workflows::write_flow::WriteOutcome;
    use std::collections::BTreeMap;

    fn empty_state() -> UiState {
        use crate::ldap::worker::RawSubschema;
        use crate::workflows::structure::Structure;
        let schema = crate::schema::SchemaModel::from_raw(&RawSubschema::default());
        let structure = Structure::build("dc=x", vec![]);
        UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new())
    }

    /// A minimal single-field form; `dirty` controls whether the field diverges
    /// from its baseline (so `is_dirty()` is true).
    fn form_with_dirty(dirty: bool) -> crate::workflows::edit_form::EditForm {
        use crate::schema::FieldKind;
        use crate::workflows::edit_form::{EditField, EditForm, FormMode};
        use crate::workflows::form_model::WidgetSpec;
        let field = EditField {
            label: "cn".into(),
            must: true,
            editable: true,
            multi: false,
            secret: false,
            ordered: false,
            orphaned: false,
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: None,
            values: vec![if dirty { "new" } else { "base" }.into()],
            baseline: vec!["base".into()],
        };
        EditForm {
            dn: "cn=a,dc=x".into(),
            mode: FormMode::Edit,
            object_classes: vec!["top".into()],
            fields: vec![field],
        }
    }

    #[test]
    fn reconcile_no_request_is_noop() {
        let mut st = empty_state();
        assert!(!st.reconcile_selection());
        assert!(st.guard_target.is_none());
    }

    #[test]
    fn reconcile_same_leaf_clears_without_guard() {
        let mut st = empty_state();
        st.current_leaf = Some("cn=a,dc=x".into());
        st.request_leaf("cn=a,dc=x".into(), vec![]);
        assert!(!st.reconcile_selection(), "already shown → no guard");
        assert!(st.requested_leaf.is_none(), "request consumed");
        assert!(st.guard_target.is_none());
    }

    #[test]
    fn reconcile_clean_form_loads_without_guard() {
        // Clean form (or none): the controller loads directly — no guard. (No worker
        // in the test, so the read is a no-op, but the decision path is clean.)
        let mut st = empty_state();
        st.edit_form = Some(form_with_dirty(false));
        st.request_leaf("cn=b,dc=x".into(), vec![]);
        assert!(!st.reconcile_selection(), "clean form → load, no guard");
        assert!(st.requested_leaf.is_none());
        assert!(st.guard_target.is_none());
    }

    #[test]
    fn reconcile_dirty_form_raises_guard_with_target() {
        // The reported bug: switching entries while the form is dirty must raise the
        // guard (here, return true so the pump posts GUARD_NAV) — regardless of which
        // input drove the selection.
        let mut st = empty_state();
        st.current_leaf = Some("cn=a,dc=x".into());
        st.edit_form = Some(form_with_dirty(true));
        st.request_leaf("cn=b,dc=x".into(), vec!["top".into()]);
        assert!(st.reconcile_selection(), "dirty form → raise guard");
        assert_eq!(
            st.guard_target,
            Some(GuardTarget::Leaf("cn=b,dc=x".into(), vec!["top".into()])),
            "the requested entry becomes the guard target"
        );
        assert!(
            st.requested_leaf.is_none(),
            "request consumed into the guard"
        );
        assert_eq!(
            st.current_leaf.as_deref(),
            Some("cn=a,dc=x"),
            "the form stays pinned to the current entry"
        );
    }

    #[test]
    fn saved_without_quit_requests_reread_and_sets_status() {
        let mut st = empty_state();
        let res = st.apply_write_outcome(WriteOutcome::Saved {
            reread_dn: "cn=a,dc=x".into(),
            quit_after: false,
        });
        assert!(res.changed);
        assert!(!res.quit);
        assert_eq!(st.status, "Saved.");
    }

    #[test]
    fn saved_with_quit_sets_quit_flag() {
        let mut st = empty_state();
        let res = st.apply_write_outcome(WriteOutcome::Saved {
            reread_dn: "x".into(),
            quit_after: true,
        });
        assert!(res.quit);
    }

    #[test]
    fn write_error_sets_error_flag_and_message() {
        let mut st = empty_state();
        let res = st.apply_write_outcome(WriteOutcome::Error("boom".into()));
        assert!(res.error);
        assert_eq!(st.last_write_error.as_deref(), Some("boom"));
    }

    #[test]
    fn created_navigates_to_new_entry() {
        use crate::schema::FieldKind;
        use crate::workflows::edit_form::{EditField, EditForm, FormMode};
        use crate::workflows::form_model::WidgetSpec;
        let mut st = empty_state();
        st.edit_form = Some(EditForm {
            dn: "uid=bob,ou=people,dc=example,dc=org".into(),
            mode: FormMode::Create {
                profile_idx: 0,
                container: "ou=people,dc=example,dc=org".into(),
            },
            object_classes: vec!["inetOrgPerson".into(), "top".into()],
            fields: vec![EditField {
                label: "uid".into(),
                must: true,
                editable: true,
                multi: false,
                secret: false,
                ordered: false,
                orphaned: false,
                kind: FieldKind::Text,
                widget: WidgetSpec::ReadOnlyText,
                widget_binding: None,
                values: vec!["bob".into()],
                baseline: vec![],
            }],
        });
        let r = st.apply_write_outcome(WriteOutcome::Created {
            dn: "uid=bob,ou=people,dc=example,dc=org".into(),
            quit_after: false,
        });
        assert_eq!(
            st.current_leaf.as_deref(),
            Some("uid=bob,ou=people,dc=example,dc=org")
        );
        assert!(st.list_dirty);
        assert!(r.changed);
        // With worker: None, edit_form is cleared (reread skipped but state mutations apply).
        assert!(st.edit_form.is_none());
    }

    fn si(dn: &str, child_hint: Option<&str>) -> StructureInput {
        StructureInput {
            dn: dn.into(),
            cn: child_hint.map(Into::into),
            description: None,
            object_classes: vec![],
            attrs: BTreeMap::new(),
        }
    }

    fn structure_inputs_from(inputs: Vec<StructureInput>) -> Vec<StructureInput> {
        inputs
    }

    fn dirty_form(dn: &str) -> crate::workflows::edit_form::EditForm {
        let mut f = form_with_dirty(true);
        f.dn = dn.into();
        f
    }

    #[test]
    fn reconcile_branch_clean_switches_dirty_guards() {
        let inputs = vec![
            si("dc=x", None),
            si("ou=p,dc=x", None),
            si("ou=q,dc=x", None),
            si("cn=a,ou=p,dc=x", Some("a")),
            si("cn=b,ou=q,dc=x", Some("b")),
        ];
        let structure = Structure::build("dc=x", structure_inputs_from(inputs));
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.branch_dns = vec!["dc=x".into(), "ou=p,dc=x".into(), "ou=q,dc=x".into()];
        st.current_branch = Some("ou=p,dc=x".into());

        // Clean form → switch immediately.
        st.request_branch("ou=q,dc=x".into());
        assert!(!st.reconcile_branch());
        assert_eq!(st.current_branch.as_deref(), Some("ou=q,dc=x"));
        assert!(st.list_dirty);

        // Dirty form → stash a Branch guard target, signal guard, do not switch.
        st.current_branch = Some("ou=p,dc=x".into());
        st.edit_form = Some(dirty_form("cn=a,ou=p,dc=x"));
        st.request_branch("ou=q,dc=x".into());
        assert!(st.reconcile_branch());
        assert!(
            matches!(st.guard_target, Some(GuardTarget::Branch(ref b)) if b == "ou=q,dc=x"),
            "dirty form → stash Branch guard target"
        );
        assert_eq!(
            st.current_branch.as_deref(),
            Some("ou=p,dc=x"),
            "stays until guarded"
        );
    }
}
