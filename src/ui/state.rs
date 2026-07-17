//! Shared application state for the tvision UI and the blocking bootstrap that
//! builds it. State is held in `Rc<RefCell<UiState>>` (alias `Shared` in mod.rs).

use anyhow::{anyhow, Result};

use crate::config::tree_label::CompiledTreeRule;
use crate::config::{Config, EntryProfile};
use crate::ldap::worker::{Request, Response, SearchScope, WorkerHandle};
use crate::schema::SchemaModel;
use crate::workflows::alloc_flow::{AllocFlow, AllocOutcome};
use crate::workflows::edit_form::{build_edit_form, EditForm};
use crate::workflows::labels::LabelRule;
use crate::workflows::pick_state::Candidate;
use crate::workflows::read_flow::ReadFlow;
use crate::workflows::resolve_flow::{LookupKey, ResolveFlow, ResolveOutcome};
use crate::workflows::search_flow::{SearchFlow, SearchOutcome};
use crate::workflows::structure::Structure;
#[cfg(test)]
use crate::workflows::structure::StructureInput;
use crate::workflows::write_flow::{WriteFlow, WriteOutcome, STAGED_PASSWORD_SENTINEL};

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
    /// Create-mode live-template latches (attr → latch), built by `open_create`
    /// from the profile's `[profile.defaults]` templates. Empty in edit mode;
    /// consulted only while the form is in `Create` mode. See
    /// `config::defaults::recompute_live`.
    pub live_templates:
        std::collections::BTreeMap<String, crate::config::defaults::LiveTemplateState>,
    /// Create-mode `{auto:…}` computed defaults (attr → kind), built by
    /// `open_create` from the profile's `[profile.defaults]`. Re-evaluated after
    /// each autonumber allocation so e.g. `sambaSID` fills once `uidNumber` resolves.
    pub computed_defaults:
        std::collections::BTreeMap<String, crate::config::defaults::ComputedKind>,
    /// Async write flow (validate/diff/submit/correlate).
    pub write_flow: WriteFlow,
    /// Async autonumber allocation flow (scan + pick next-free).
    pub alloc_flow: AllocFlow,
    /// Async candidate search flow for picker / membership widgets.
    pub search_flow: SearchFlow,
    /// Last search results delivered by `search_flow` (via `pump_worker`).
    pub search_results: Vec<Candidate>,
    /// Async reverse name-resolution for `lookup` widgets.
    pub resolve_flow: ResolveFlow,
    /// Resolved friendly names for `lookup` fields, keyed by scope+value.
    /// `Some(name)` = resolved; `None` = resolved but no candidate matched.
    pub lookup_cache: std::collections::HashMap<LookupKey, Option<String>>,
    /// True when the last search was truncated at `PICKER_SEARCH_CAP`.
    pub search_truncated: bool,
    /// Read-only mode disables editing and the save path.
    pub read_only: bool,
    /// Transient status text (e.g. "Saved.").
    pub status: String,
    /// True when a pane must re-render the form from `edit_form`.
    pub form_needs_render: bool,
    /// One-shot: a freshly-opened create form wants pane-level focus moved to the
    /// form pane (panel 3), so the operator can type immediately without Tabbing
    /// over from the tree/list. The form pane consumes and clears this on its next
    /// event via [`UiState::take_focus_form_request`].
    pub focus_form_request: bool,
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
    pub staged_commit: Option<crate::ui::widget::CommitOutcome>,
    /// Profile chooser → controller: the index the user highlighted when OK was
    /// pressed. Set by `ProfileChooser`; read by `dispatch` to select a profile.
    pub chosen_profile: Option<usize>,
    /// Container chooser → controller: the row the user highlighted (0 = current
    /// branch, 1 = the profile's search_base) when OK was pressed. Set by
    /// `ContainerChooser`; read by `dispatch` in the create container rule.
    pub chosen_container: Option<usize>,
    /// One-shot action to run after the TUI starts (set by `tui-create` via
    /// `ui::run`; `None` for a normal launch). Posted once by the pump as `STARTUP`,
    /// taken by `dispatch`. See [`crate::ui::StartupAction`].
    pub pending_startup: Option<crate::ui::StartupAction>,
    /// True when the LDAP connection is encrypted (LDAPS, StartTLS, or ldapi://).
    /// The password widget refuses to operate when this is false.
    pub connection_encrypted: bool,
    /// Pre-resolved `[profile.widget.*]` bindings; built once in `bootstrap` via
    /// `config::widget::resolve_widgets` and used by `apply_widget_bindings` every
    /// time a form is opened.
    pub resolved_widgets: Vec<crate::config::widget::ResolvedWidget>,
    /// Cleartext staged by the password editor (Task 16/17); folded into the ADD or
    /// MODIFY on submit. Never rendered or logged — the form shows "••••••" instead.
    pub pending_password: Option<String>,
    /// The attribute names the staged password targets (primary first).
    pub pending_password_attrs: Vec<String>,
    /// Samba domain context resolved from the config `[samba]` table (or later from
    /// a live `sambaDomain` LDAP entry). `None` when no Samba domain is configured;
    /// drives `samba_enabled` in the widget resolver so SambaSid widgets activate.
    pub samba_domain: Option<crate::samba::SambaDomainInfo>,
    /// True when the server advertises RFC 5805 transactions; drives the atomic
    /// companion-create path (vs. the sequential fallback). Set in `bootstrap`.
    pub server_supports_txn: bool,
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
            live_templates: std::collections::BTreeMap::new(),
            computed_defaults: std::collections::BTreeMap::new(),
            write_flow: WriteFlow::new(),
            alloc_flow: AllocFlow::new(),
            search_flow: SearchFlow::new(),
            search_results: Vec::new(),
            resolve_flow: ResolveFlow::new(),
            lookup_cache: std::collections::HashMap::new(),
            search_truncated: false,
            read_only: false,
            status: String::new(),
            form_needs_render: false,
            focus_form_request: false,
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
            chosen_container: None,
            pending_startup: None,
            connection_encrypted: false,
            resolved_widgets: Vec::new(),
            pending_password: None,
            pending_password_attrs: Vec::new(),
            samba_domain: None,
            server_supports_txn: false,
        }
    }
}

impl UiState {
    /// Drain ready worker responses: install a fresh `EditForm` on a read, and
    /// apply write outcomes. Returns what the pump view should do.
    pub fn pump_worker(&mut self) -> PumpResult {
        let mut resps = Vec::new();
        if let Some(w) = self.worker.as_ref() {
            while let Some(r) = w.poll() {
                resps.push(r);
            }
        }
        self.process_responses(&resps)
    }

    /// Process a batch of worker responses through all correlation branches.
    ///
    /// Extracted so tests can supply responses without a live [`WorkerHandle`].
    fn process_responses(&mut self, resps: &[Response]) -> PumpResult {
        use crate::workflows::read_flow::ReadOutcome;
        let mut out = PumpResult::default();
        for resp in resps {
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
                        let samba_enabled = self.samba_domain.is_some();
                        let resolver = crate::config::resolver::WidgetResolver::new(
                            self.read_flow.schema(),
                            &self.profiles,
                            &self.resolved_widgets,
                            samba_enabled,
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
            // Candidate search: Entries/SearchError with search-range IDs (3_000_000+).
            let s_out = self.search_flow.on_response(resp);
            if !matches!(s_out, SearchOutcome::Ignored) {
                self.apply_search_results(s_out);
                out.changed = true;
                continue;
            }
            // Reverse name-resolution: Entries/SearchError with resolve-range ids (4_000_000+).
            let r_out = self.resolve_flow.on_response(resp);
            if !matches!(r_out, ResolveOutcome::Ignored) {
                self.apply_resolve_outcome(r_out);
                out.changed = true;
                continue;
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

    /// Test-only: drive the same processing pipeline as `pump_worker` with
    /// externally-supplied responses. Allows unit tests to verify pump
    /// correlation branches without a live [`WorkerHandle`].
    #[cfg(test)]
    pub fn pump_responses_for_test(&mut self, resps: &[Response]) -> PumpResult {
        self.process_responses(resps)
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
        // A freshly-allocated number (e.g. `uidNumber`) may be the input a computed
        // default was waiting on — try to fill `{auto:…}` targets now.
        self.recompute_computed_defaults();
    }

    /// Fill any create-mode `{auto:…}` computed default whose inputs are now ready.
    /// Currently: `sambaSID`, derived from the sibling `uidNumber` and the Samba
    /// domain. Only *empty* targets are filled, so an operator-typed value (or an
    /// earlier compute) is never overwritten. No-op outside create mode.
    /// Consume the one-shot "focus the form pane" request, returning whether it was
    /// set. The form pane calls this on each event; a `true` result means it should
    /// pull pane-level focus to itself (a create form just opened).
    pub fn take_focus_form_request(&mut self) -> bool {
        std::mem::take(&mut self.focus_form_request)
    }

    pub fn recompute_computed_defaults(&mut self) {
        use crate::config::defaults::ComputedKind;
        if self.computed_defaults.is_empty() {
            return;
        }
        let Some(form) = self.edit_form.as_ref() else {
            return;
        };
        if !matches!(
            form.mode,
            crate::workflows::edit_form::FormMode::Create { .. }
        ) {
            return;
        }
        // Compute against the current (immutably-borrowed) form; collect fills.
        let mut fills: Vec<(String, String)> = Vec::new();
        for (attr, kind) in &self.computed_defaults {
            let target_empty = form
                .fields
                .iter()
                .find(|f| f.label.eq_ignore_ascii_case(attr))
                .map(|f| f.values.iter().all(|s| s.trim().is_empty()));
            // Skip if the attr isn't on the form or already has a value.
            if target_empty != Some(true) {
                continue;
            }
            let computed = match kind {
                ComputedKind::SambaSid => crate::workflows::samba_compute::samba_sid_for_form(
                    form,
                    self.samba_domain.as_ref(),
                )
                .ok(),
            };
            if let Some(v) = computed {
                if !v.trim().is_empty() {
                    fills.push((attr.clone(), v));
                }
            }
        }
        if fills.is_empty() {
            return;
        }
        if let Some(form) = self.edit_form.as_mut() {
            for (attr, v) in fills {
                if let Some(f) = form
                    .fields
                    .iter_mut()
                    .find(|f| f.label.eq_ignore_ascii_case(&attr))
                {
                    f.values = vec![v];
                }
            }
        }
        self.form_needs_render = true;
    }

    /// Submit a candidate search under `base` for entries of object class `oc`
    /// matching `term`, returning `attrs` per entry. `store_attr` declares how the
    /// arriving candidates' `store_value` is derived (`None` ⇒ DN, `Some(attr)` ⇒
    /// scalar). No-op without a live worker.
    ///
    /// Borrow-safe: `self.worker` and `self.search_flow` are disjoint fields, so
    /// this is a single atomic `&mut self`. Call it as
    /// `shared.borrow_mut().submit_search(...)` — never while holding any other
    /// borrow (the worker `submit` happens inside this method, mirroring the pump).
    pub fn submit_search(
        &mut self,
        base: &str,
        oc: &str,
        term: &str,
        attrs: &[String],
        store_attr: Option<&str>,
    ) {
        if let Some(w) = self.worker.as_ref() {
            let _ = self
                .search_flow
                .request(w, base, oc, term, attrs, store_attr);
        }
    }

    /// Kick off a reverse name-resolution for a `lookup` field's value, unless it
    /// is already cached or in flight. No-op without a live worker.
    ///
    /// Borrow-safe: a single atomic `&mut self`. Call as
    /// `shared.borrow_mut().resolve_lookup(...)` — never while holding another borrow.
    #[allow(clippy::too_many_arguments)]
    pub fn resolve_lookup(
        &mut self,
        key: LookupKey,
        base: &str,
        oc: &str,
        store_attr: &str,
        value: &str,
        attrs: &[String],
        template: &[crate::config::label::LabelSeg],
    ) {
        if self.lookup_cache.contains_key(&key) || self.resolve_flow.is_pending(&key) {
            return;
        }
        if let Some(w) = self.worker.as_ref() {
            let _ =
                self.resolve_flow
                    .request(w, base, oc, store_attr, value, attrs, template.to_vec());
        }
    }

    /// Kick off a reverse name-resolution for a DN-keyed reference (a group
    /// `member`), reading the entry by DN and caching its `cn (uid)` label under
    /// the member scope ([`resolve_flow::member_key`]). Lets the membership editor
    /// show pre-existing members with friendly names immediately, instead of only
    /// those returned by the capped candidate search. Dedups against the cache and
    /// any in-flight resolve; no-op without a live worker.
    ///
    /// Borrow-safe: a single atomic `&mut self` — never call while holding another
    /// borrow.
    pub fn resolve_member(&mut self, dn: &str) {
        let key = crate::workflows::resolve_flow::member_key(dn);
        if self.lookup_cache.contains_key(&key) || self.resolve_flow.is_pending(&key) {
            return;
        }
        if let Some(w) = self.worker.as_ref() {
            let attrs = ["cn".to_string(), "uid".to_string()];
            let _ = self.resolve_flow.request_by_dn(w, dn, &attrs);
        }
    }

    /// Apply one non-ignored resolve outcome: cache the name (or `None` when not
    /// found) and flag a re-render so the form repaints `<value> (<name>)`.
    pub fn apply_resolve_outcome(&mut self, out: ResolveOutcome) {
        match out {
            ResolveOutcome::Resolved { key, name } => {
                self.lookup_cache.insert(key, Some(name));
                self.form_needs_render = true;
            }
            ResolveOutcome::NotFound { key } => {
                self.lookup_cache.insert(key, None);
                self.form_needs_render = true;
            }
            ResolveOutcome::Ignored => {}
        }
    }

    /// Apply one non-ignored search outcome to state.
    ///
    /// `Results`: store the rows and truncated flag.
    /// `Failed`: set status and clear results.
    /// `Ignored`: must not be passed here (callers should check before calling).
    pub fn apply_search_results(&mut self, out: SearchOutcome) {
        match out {
            SearchOutcome::Results { rows, truncated } => {
                self.search_results = rows;
                self.search_truncated = truncated;
            }
            SearchOutcome::Failed(msg) => {
                self.status = msg;
                self.search_results = Vec::new();
                self.search_truncated = false;
            }
            SearchOutcome::Ignored => {}
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
            WriteOutcome::CombinedSaved {
                reread_dn,
                quit_after,
            } => {
                // A combined membership save completed; treat exactly like Saved:
                // re-read the user entry (or navigate to a pending guard target).
                self.status = "Saved.".to_string();
                if quit_after {
                    out.quit = true;
                    return out;
                }
                let (dn, profile_ocs) = self.pending_nav.take().unwrap_or((reread_dn, Vec::new()));
                self.reread(&dn, &profile_ocs);
            }
            WriteOutcome::BatchProgress { .. } => {
                // A non-final leg of a combined save landed; nothing user-visible yet.
                out.changed = false;
            }
            WriteOutcome::NeedFollowupCreate {
                dn,
                attrs,
                companion_dn,
                quit_after,
            } => {
                if let Some(w) = self.worker.as_ref() {
                    let _ = self.write_flow.submit_followup_create(
                        w,
                        &dn,
                        attrs,
                        &companion_dn,
                        quit_after,
                    );
                }
            }
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
    pub fn apply_commit(&mut self, field_idx: usize, outcome: crate::ui::widget::CommitOutcome) {
        use crate::ui::widget::CommitOutcome;
        let UiState {
            edit_form,
            read_flow,
            profiles,
            form_needs_render,
            pending_password,
            pending_password_attrs,
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
                    // In create mode, restore the profile's preferred field order
                    // (`sync_schema_fields` alphabetises within buckets).
                    if let crate::workflows::edit_form::FormMode::Create { profile_idx, .. } =
                        form.mode
                    {
                        if let Some(p) = profiles.get(profile_idx) {
                            crate::workflows::edit_form::reorder_by_show(form, &p.show);
                        }
                    }
                }
            }
            CommitOutcome::StageSecret { attrs, cleartext } => {
                // Stash the cleartext; set the masked sentinel so present() shows
                // ‹set› and is_dirty() sees a change. Cleartext is never rendered.
                *pending_password = Some(cleartext);
                *pending_password_attrs = attrs;
                if let Some(form) = edit_form.as_mut() {
                    if let Some(f) = form.fields.get_mut(field_idx) {
                        f.values = vec![STAGED_PASSWORD_SENTINEL.to_string()];
                    }
                }
            }
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
    /// - form **clean** → switch via [`commit_branch`](Self::commit_branch)
    ///   (also clears the leaf search), `false`.
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
            self.commit_branch(dn);
            false
        }
    }

    /// Commit a navigation to `dn` as the shown branch: switch, mark the leaf
    /// list dirty, and **drop any active leaf search** so the new branch is
    /// listed unfiltered. Navigating the tree (pane 1) must reset the leaf
    /// list's incremental find (pane 2); this is the single commit point for
    /// that reset, shared by [`reconcile_branch`] and the dirty-form guard
    /// paths. The `LeafPane` mirrors the cleared `search` back onto its
    /// `ListBox` find query on the next repopulate.
    pub fn commit_branch(&mut self, dn: String) {
        self.current_branch = Some(dn);
        self.list_dirty = true;
        self.search = String::new();
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

/// Derive a `SambaDomainInfo` from the config `[samba]` table.
/// Returns `None` when no `domain_sid` is configured.
pub(crate) fn samba_info_from_config(config: &Config) -> Option<crate::samba::SambaDomainInfo> {
    config
        .samba
        .domain_sid
        .as_deref()
        .map(|sid| crate::samba::SambaDomainInfo {
            domain_sid: sid.to_string(),
            algorithmic_rid_base: config.samba.algorithmic_rid_base,
        })
}

/// True when any resolved widget is a `sambaSID` field — i.e. the samba domain
/// is actually needed, so a live discovery search at startup is worth issuing.
fn samba_in_use(widgets: &[crate::config::widget::ResolvedWidget]) -> bool {
    use crate::config::widget::WidgetKind;
    widgets
        .iter()
        .any(|w| matches!(w.kind, WidgetKind::SambaSid))
}

/// Discover the samba domain context from a live `sambaDomain` entry under
/// `base` (best-effort). Returns the first entry that parses via
/// [`crate::samba::sid::parse_samba_domain`]; `None` when none is found, the
/// search fails, or access is denied — callers fall back to the config
/// `domain_sid`.
fn discover_samba_domain(
    worker: &WorkerHandle,
    base: &str,
) -> Option<crate::samba::SambaDomainInfo> {
    let resp = worker
        .request(Request::Search {
            id: 0,
            base: base.to_string(),
            scope: SearchScope::Subtree,
            filter: "(objectClass=sambaDomain)".to_string(),
            attrs: vec![
                "sambaSID".to_string(),
                "sambaAlgorithmicRidBase".to_string(),
            ],
            size_limit: Some(5),
        })
        .ok()?;
    let Response::Entries { entries, .. } = resp else {
        return None;
    };
    entries
        .iter()
        .find_map(|e| crate::samba::sid::parse_samba_domain(&e.attrs))
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
    let samba_from_config = samba_info_from_config(&config);
    let worker = WorkerHandle::spawn(config, password)?;
    // M5c: prefer a live sambaDomain entry when the Samba domain SID is actually
    // needed — either a `sambaSID` widget OR a `{auto:sambaSID}` computed default
    // (the computed default derives the SID from the domain too, so it must trigger
    // discovery just like the widget). Fall back to the static config domain_sid
    // (or no samba at all).
    let samba_needed = samba_in_use(&resolved_widgets)
        || profiles
            .iter()
            .any(|p| crate::config::defaults::uses_computed_samba_sid(&p.defaults));
    let samba_domain = if samba_needed {
        discover_samba_domain(&worker, &base_dn).or(samba_from_config)
    } else {
        samba_from_config
    };

    let raw = match worker.request(Request::FetchSubschema)? {
        Response::Subschema(raw) => raw,
        other => return Err(anyhow!("FetchSubschema: unexpected {other:?}")),
    };
    let schema = SchemaModel::from_raw(&raw);

    // Tolerant capability probe: a failed/absent root DSE just means "no txn
    // support" (never fail bootstrap over it).
    let server_supports_txn = match worker.request(Request::FetchRootDse) {
        Ok(Response::RootDse {
            supported_extensions,
        }) => crate::ldap::worker::txn_supported(&supported_extensions),
        _ => false,
    };

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
        live_templates: std::collections::BTreeMap::new(),
        computed_defaults: std::collections::BTreeMap::new(),
        write_flow: WriteFlow::new(),
        alloc_flow: AllocFlow::new(),
        search_flow: SearchFlow::new(),
        search_results: Vec::new(),
        resolve_flow: ResolveFlow::new(),
        lookup_cache: std::collections::HashMap::new(),
        search_truncated: false,
        read_only: false,
        status: String::new(),
        form_needs_render: false,
        focus_form_request: false,
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
        chosen_container: None,
        pending_startup: None,
        connection_encrypted,
        resolved_widgets,
        pending_password: None,
        pending_password_attrs: Vec::new(),
        samba_domain,
        server_supports_txn,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldap::worker::RawSubschema;
    use std::collections::BTreeMap;

    #[test]
    fn samba_in_use_true_only_with_samba_sid_widget() {
        use crate::config::widget::{ResolvedWidget, WidgetKind};
        let none: Vec<ResolvedWidget> = vec![ResolvedWidget {
            owner_object_classes: vec!["posixGroup".into()],
            attr: "memberUid".into(),
            kind: WidgetKind::XOrdered,
        }];
        assert!(!super::samba_in_use(&none));

        let with_samba = vec![ResolvedWidget {
            owner_object_classes: vec!["sambaSamAccount".into()],
            attr: "sambaSID".into(),
            kind: WidgetKind::SambaSid,
        }];
        assert!(super::samba_in_use(&with_samba));
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
            companion: None,
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
        use crate::ui::widget::CommitOutcome;
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
        use crate::ui::widget::CommitOutcome;
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

    /// Dispatch semantics for a `sambaSID`-bound field: compute the SID from the
    /// sibling `uidNumber` + `samba_domain` (as the ACTIVATE handler does) and
    /// `apply_commit(SetValues)` fills the field. Mirrors the app.rs special-case
    /// without a live `Program`.
    #[test]
    fn sambasid_dispatch_computes_and_fills_field() {
        use crate::config::widget::WidgetKind;
        use crate::schema::FieldKind;
        use crate::ui::widget::CommitOutcome;
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
        st.samba_domain = Some(crate::samba::SambaDomainInfo {
            domain_sid: "S-1-5-21-1-2-3".into(),
            algorithmic_rid_base: 1000,
        });
        let mk = |label: &str, binding: Option<WidgetKind>, values: Vec<String>| EditField {
            label: label.into(),
            must: false,
            editable: true,
            multi: false,
            secret: false,
            ordered: false,
            orphaned: false,
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: binding,
            baseline: values.clone(),
            values,
        };
        st.edit_form = Some(EditForm {
            dn: "cn=test,dc=example,dc=org".into(),
            mode: FormMode::Edit,
            object_classes: vec!["top".into()],
            fields: vec![
                mk("uidNumber", None, vec!["1000".into()]),
                mk("sambaSID", Some(WidgetKind::SambaSid), vec![]),
            ],
        });

        // Ok branch: compute (helper) + apply_commit, as the dispatch does.
        let sid = crate::workflows::samba_compute::samba_sid_for_form(
            st.edit_form.as_ref().unwrap(),
            st.samba_domain.as_ref(),
        )
        .expect("sid computes");
        st.apply_commit(1, CommitOutcome::SetValues(vec![sid]));
        assert_eq!(
            st.edit_form.as_ref().unwrap().fields[1].values,
            vec!["S-1-5-21-1-2-3-3000".to_string()],
        );

        // Err branch: clearing uidNumber makes generation fail (no field write).
        st.edit_form.as_mut().unwrap().fields[0].values.clear();
        assert!(crate::workflows::samba_compute::samba_sid_for_form(
            st.edit_form.as_ref().unwrap(),
            st.samba_domain.as_ref(),
        )
        .is_err());
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

    /// `{auto:sambaSID}`: an empty computed target fills once its input
    /// (`uidNumber`) resolves via the alloc outcome, but not before.
    #[test]
    fn apply_alloc_outcome_fills_computed_samba_sid() {
        use crate::config::defaults::ComputedKind;
        use crate::schema::FieldKind;
        use crate::workflows::alloc_flow::AllocOutcome;
        use crate::workflows::edit_form::{EditField, EditForm, FormMode};
        use crate::workflows::form_model::WidgetSpec;

        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let structure = Structure::build("dc=x", vec![]);
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.samba_domain = Some(crate::samba::SambaDomainInfo {
            domain_sid: "S-1-5-21-1-2-3".into(),
            algorithmic_rid_base: 1000,
        });
        st.computed_defaults
            .insert("sambaSID".into(), ComputedKind::SambaSid);

        let mk = |label: &str, vals: Vec<String>| EditField {
            label: label.into(),
            must: false,
            editable: true,
            multi: false,
            secret: false,
            ordered: false,
            orphaned: false,
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: None,
            values: vals,
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
                mk("uidNumber", vec![ALLOC_PLACEHOLDER.to_string()]),
                mk("sambaSID", vec![]),
            ],
        });

        // Before uidNumber resolves: sambaSID stays empty (placeholder isn't numeric).
        st.recompute_computed_defaults();
        assert!(st.edit_form.as_ref().unwrap().fields[1]
            .values
            .iter()
            .all(|s| s.trim().is_empty()));

        // uidNumber resolves → sambaSID computed from it.
        st.apply_alloc_outcome(AllocOutcome::Filled {
            attr: "uidNumber".into(),
            value: "1000".into(),
        });
        let f = st.edit_form.as_ref().unwrap();
        assert_eq!(f.fields[0].values, vec!["1000".to_string()]);
        assert_eq!(
            f.fields[1].values,
            vec!["S-1-5-21-1-2-3-3000".to_string()],
            "sambaSID auto-computed from the resolved uidNumber"
        );
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

    /// Task 17 RED: StageSecret sets pending_password + masked sentinel + render.
    #[test]
    fn apply_commit_stage_secret_sets_pending_and_masks_field() {
        use crate::schema::FieldKind;
        use crate::ui::widget::CommitOutcome;
        use crate::workflows::edit_form::{EditField, EditForm, FormMode};
        use crate::workflows::form_model::WidgetSpec;

        let raw = RawSubschema::default();
        let schema = SchemaModel::from_raw(&raw);
        let structure = Structure::build("dc=x", vec![]);
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());

        let pw_field = EditField {
            label: "userPassword".into(),
            must: false,
            editable: true,
            multi: false,
            secret: true,
            ordered: false,
            orphaned: false,
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: None,
            values: vec![],
            baseline: vec![],
        };
        st.edit_form = Some(EditForm {
            dn: "uid=bob,dc=example,dc=org".into(),
            mode: FormMode::Edit,
            object_classes: vec!["inetOrgPerson".into()],
            fields: vec![pw_field],
        });
        st.form_needs_render = false;

        st.apply_commit(
            0,
            CommitOutcome::StageSecret {
                attrs: vec!["userPassword".into()],
                cleartext: "s3cret".into(),
            },
        );

        assert_eq!(
            st.pending_password.as_deref(),
            Some("s3cret"),
            "pending_password must be stashed"
        );
        assert_eq!(
            st.pending_password_attrs,
            vec!["userPassword".to_string()],
            "pending_password_attrs must be stashed"
        );
        let form = st.edit_form.as_ref().unwrap();
        assert_eq!(
            form.fields[0].values,
            vec!["••••••".to_string()],
            "field must show masked sentinel"
        );
        assert!(st.form_needs_render, "form_needs_render must be set");
    }

    /// Task 8 (RED): samba_domain defaults to None from new_for_test; Some when Config
    /// has domain_sid set.
    #[test]
    fn samba_domain_threaded_from_config() {
        use crate::samba::SambaDomainInfo;
        let structure = Structure::build("dc=x", vec![]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let st = UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        assert!(
            st.samba_domain.is_none(),
            "samba_domain must be None in new_for_test"
        );

        // Verify the config → SambaDomainInfo mapping via the pure helper.
        let cfg: crate::config::Config = toml::from_str(
            r#"
            [server]
            uri = "ldap://x"
            base_dn = "dc=x"
            [auth]
            [samba]
            domain_sid = "S-1-5-21-1-2-3"
        "#,
        )
        .unwrap();
        let info = samba_info_from_config(&cfg);
        assert_eq!(
            info,
            Some(SambaDomainInfo {
                domain_sid: "S-1-5-21-1-2-3".to_string(),
                algorithmic_rid_base: 1000,
            }),
            "samba_domain must be Some with correct SID and default rid_base=1000"
        );

        // When domain_sid is absent, must be None.
        let cfg_no_samba: crate::config::Config = toml::from_str(
            r#"
            [server]
            uri = "ldap://x"
            base_dn = "dc=x"
            [auth]
        "#,
        )
        .unwrap();
        assert!(
            samba_info_from_config(&cfg_no_samba).is_none(),
            "samba_domain must be None when domain_sid not configured"
        );
    }

    /// Task 12 (RED → GREEN): a `Response::Entries` carrying the latest search id
    /// must populate `search_results` and return `PumpResult.changed = true` when
    /// driven through `pump_worker` (via the test helper that bypasses the worker).
    #[test]
    fn pump_search_flow_populates_results_and_signals_changed() {
        use crate::ldap::worker::{LdapEntry, Response};
        use std::collections::BTreeMap;

        let structure = Structure::build("dc=x", vec![si("dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());

        // Set the latest id without a live worker.
        let search_id = 3_000_000u64;
        st.search_flow.force_latest(search_id);

        // Build two candidate entries.
        let entries = vec![
            {
                let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
                attrs.insert("cn".to_string(), vec!["Alice".to_string()]);
                LdapEntry {
                    dn: "uid=alice,dc=x".to_string(),
                    attrs,
                    bin_attrs: Default::default(),
                }
            },
            LdapEntry {
                dn: "uid=bob,dc=x".to_string(),
                attrs: BTreeMap::new(),
                bin_attrs: Default::default(),
            },
        ];
        let resp = Response::Entries {
            id: search_id,
            entries,
            truncated: false,
        };

        let result = st.pump_responses_for_test(&[resp]);

        assert!(
            result.changed,
            "pump must signal changed when search results arrive"
        );
        assert_eq!(st.search_results.len(), 2, "both candidates must be stored");
        assert_eq!(st.search_results[0].label, "Alice");
        assert_eq!(st.search_results[1].label, "uid=bob,dc=x");
        assert!(!st.search_truncated, "truncated flag must be forwarded");
    }

    /// Task 12: a `Response::Entries` for a STALE search id must NOT populate
    /// results and must NOT signal changed.
    #[test]
    fn pump_search_flow_stale_response_is_ignored() {
        use crate::ldap::worker::Response;

        let structure = Structure::build("dc=x", vec![si("dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());

        // latest is 3_000_001 but we send a response for 3_000_000.
        st.search_flow.force_latest(3_000_001);
        let resp = Response::Entries {
            id: 3_000_000,
            entries: vec![],
            truncated: false,
        };

        let result = st.pump_responses_for_test(&[resp]);
        assert!(
            !result.changed,
            "stale search response must not signal changed"
        );
        assert!(
            st.search_results.is_empty(),
            "stale response must not populate results"
        );
    }

    /// Task 12: new_for_test must default connection_encrypted to false.
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

    #[test]
    fn resolve_lookup_dedups_by_cache_and_pending() {
        use crate::config::label::parse_label_template;
        use crate::workflows::resolve_flow::LookupKey;
        let mut st = super::UiState::new_for_test(
            crate::workflows::structure::Structure::build("dc=x", vec![]),
            crate::schema::model::SchemaModel::from_raw(
                &crate::ldap::worker::RawSubschema::default(),
            ),
            "dc=x".into(),
            Vec::new(),
            Vec::new(),
        );
        let key = LookupKey {
            scope_id: "ou=groups,dc=x|posixGroup|gidNumber".into(),
            value: "5000".into(),
        };
        let tmpl = parse_label_template("{cn}");
        let attrs = vec!["cn".to_string()];
        // No worker → request() is a no-op, but the pending/cache guards are pure.
        // First: cache already has it → is_pending stays false, no attempt to submit.
        st.lookup_cache.insert(key.clone(), Some("staff".into()));
        st.resolve_lookup(
            key.clone(),
            "ou=groups,dc=x",
            "posixGroup",
            "gidNumber",
            "5000",
            &attrs,
            &tmpl,
        );
        assert!(
            !st.resolve_flow.is_pending(&key),
            "cached key must not be resubmitted"
        );
    }

    #[test]
    fn apply_resolve_outcome_fills_cache_and_flags_render() {
        use crate::workflows::resolve_flow::{LookupKey, ResolveOutcome};
        let mut st = super::UiState::new_for_test(
            crate::workflows::structure::Structure::build("dc=x", vec![]),
            crate::schema::model::SchemaModel::from_raw(
                &crate::ldap::worker::RawSubschema::default(),
            ),
            "dc=x".into(),
            Vec::new(),
            Vec::new(),
        );
        let key = LookupKey {
            scope_id: "s".into(),
            value: "5000".into(),
        };
        st.form_needs_render = false;
        st.apply_resolve_outcome(ResolveOutcome::Resolved {
            key: key.clone(),
            name: "staff".into(),
        });
        assert_eq!(st.lookup_cache.get(&key), Some(&Some("staff".to_string())));
        assert!(st.form_needs_render);

        let key2 = LookupKey {
            scope_id: "s".into(),
            value: "9999".into(),
        };
        st.apply_resolve_outcome(ResolveOutcome::NotFound { key: key2.clone() });
        assert_eq!(st.lookup_cache.get(&key2), Some(&None));
    }

    #[test]
    fn process_responses_routes_resolve_entries_into_cache() {
        use crate::config::label::parse_label_template;
        use crate::workflows::resolve_flow::LookupKey;
        let mut st = super::UiState::new_for_test(
            crate::workflows::structure::Structure::build("dc=x", vec![]),
            crate::schema::model::SchemaModel::from_raw(
                &crate::ldap::worker::RawSubschema::default(),
            ),
            "dc=x".into(),
            Vec::new(),
            Vec::new(),
        );
        // Register an in-flight resolve for id 4_000_000, then feed its response.
        let key = LookupKey {
            scope_id: "dc=x|posixGroup|gidNumber".into(),
            value: "5000".into(),
        };
        st.resolve_flow
            .force_pending(4_000_000, key.clone(), parse_label_template("{cn}"));
        let mut attrs = std::collections::BTreeMap::new();
        attrs.insert("cn".to_string(), vec!["staff".to_string()]);
        let resp = crate::ldap::worker::Response::Entries {
            id: 4_000_000,
            entries: vec![crate::ldap::worker::LdapEntry {
                dn: "cn=staff,dc=x".into(),
                attrs,
                bin_attrs: Default::default(),
            }],
            truncated: false,
        };
        st.pump_responses_for_test(&[resp]);
        assert_eq!(st.lookup_cache.get(&key), Some(&Some("staff".to_string())));
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
