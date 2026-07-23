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
use crate::workflows::leaf_search::{LeafSearchFlow, LeafSearchOutcome};
use crate::workflows::pick_state::Candidate;
use crate::workflows::read_flow::ReadFlow;
use crate::workflows::resolve_flow::{LookupKey, ResolveFlow, ResolveOutcome};
use crate::workflows::search_flow::{SearchFlow, SearchOutcome};
use crate::workflows::structure::{Structure, StructureInput};
use crate::workflows::write_flow::{WriteFlow, WriteOutcome, STAGED_PASSWORD_SENTINEL};

/// Placeholder text set in autonumber fields while the background scan is pending.
pub const ALLOC_PLACEHOLDER: &str = "‹allocating…›";

/// A dirty-blocked navigation awaiting the guard's decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardTarget {
    Leaf(String, Vec<String>),
    Branch(String),
}

/// What a pane should do with its highlight after rebuilding its row source.
///
/// A rebuild must never look like an operator navigation, so the controller
/// answers with a **DN** — resolved against the freshly-built rows by the pane —
/// rather than a row index computed against the rows the rebuild just replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HighlightPlan {
    /// Highlight this DN. The form does not move.
    Pin(String),
    /// Highlight this DN and let the form follow it.
    Follow(String),
    /// Nothing to highlight.
    Clear,
}

/// What `pump_worker` wants the pump view to do after draining responses.
#[derive(Debug, Default, Clone, Copy)]
pub struct PumpResult {
    pub changed: bool,
    pub quit: bool,
    pub error: bool,
}

/// A pending concurrent-modification prompt, stashed by [`UiState::resolve_conflict`]
/// when a re-read shows the other client's change overlaps our edit. Drained by the
/// dispatch layer, which opens the Reload/Overwrite/Cancel dialog.
#[derive(Debug, Clone)]
pub struct ConflictPrompt {
    /// The entry's DN (for the Reload re-read and the Overwrite resubmit).
    pub dn: String,
    /// The dialog body naming the conflicting attribute(s).
    pub text: String,
    /// Whether the original save was a save-and-quit (deferred until the write lands).
    pub quit_after: bool,
    /// The entry's fresh `entryCSN` learned on re-read; adopted only if the operator
    /// chooses Overwrite (never before — a premature adopt would let the next plain
    /// save silently clobber the other client's change).
    pub fresh_csn: Option<String>,
}

/// The result of a conflict re-read: (fresh per-attribute values, the attribute
/// names that changed since our baseline = the other client's edits, the fresh
/// `entryCSN`).
type ConflictReread = (
    std::collections::BTreeMap<String, Vec<String>>,
    Vec<String>,
    Option<String>,
);

/// True if any attribute name appears in both sets (case-insensitive). Used to
/// decide whether a concurrent modification can be silently rebased (disjoint)
/// or must be surfaced to the operator (overlap).
pub fn attrs_overlap(ours: &[&str], theirs: &[&str]) -> bool {
    ours.iter()
        .any(|a| theirs.iter().any(|b| a.eq_ignore_ascii_case(b)))
}

/// Rebase a form onto the server's fresh state after a disjoint concurrent change.
///
/// For each field, adopt the server's fresh value as the new `baseline`. For fields
/// the operator did NOT edit, also adopt the fresh value into `values` — this folds
/// in the other client's disjoint change so the resubmit does not revert it. Fields
/// the operator DID edit keep their edited `values` (in the disjoint case the fresh
/// value equals the old baseline, so the baseline overwrite is a no-op there), so
/// only the operator's edits still diff. `entryCSN` is skipped (it is not a field).
fn rebase_baselines(form: &mut EditForm, fresh: &std::collections::BTreeMap<String, Vec<String>>) {
    let dirty: std::collections::HashSet<String> = form
        .dirty_labels()
        .iter()
        .map(|l| l.to_ascii_lowercase())
        .collect();
    for f in form.fields.iter_mut() {
        if f.label.eq_ignore_ascii_case("entryCSN") {
            continue;
        }
        let fresh_vals = fresh
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(&f.label))
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        if dirty.contains(&f.label.to_ascii_lowercase()) {
            f.baseline = fresh_vals; // keep the operator's edited `values`
        } else {
            f.values = fresh_vals.clone();
            f.baseline = fresh_vals;
        }
    }
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
    /// The attribute names the label/tree templates reference — what the eager
    /// scan fetches, and what a per-entry read projects onto its structure node.
    pub scan_attrs: Vec<String>,
    /// DFS pre-order index → branch DN, matching `Outline`'s `foc` numbering.
    pub branch_dns: Vec<String>,
    pub current_branch: Option<String>,
    pub current_leaf: Option<String>,
    pub search: String,
    /// Live one-level find backing the entry list (supersedes on every keystroke).
    pub leaf_search: LeafSearchFlow,
    /// DNs returned by the newest find, or `None` when no find is active / none has
    /// landed yet. `leaf_rows` falls back to filtering the cached projection then.
    pub leaf_search_rows: Option<Vec<String>>,
    /// True when the newest find hit `LEAF_SEARCH_CAP`.
    pub leaf_search_truncated: bool,
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
    /// True when the DIT tree pane must rebuild its node set (a branch appeared,
    /// disappeared, or changed its rendered label). The tree pane clears it.
    pub tree_dirty: bool,
    /// Pane → controller: the leaf a selector pane wants shown (dn + objectClasses).
    /// Set when the highlight moves; consumed by [`reconcile_selection`]. The panes
    /// never load or guard themselves — they only record this intent.
    pub requested_leaf: Option<(String, Vec<String>)>,
    /// Pane → controller: the branch a selector pane wants shown (dn).
    /// Set when the tree highlight moves; consumed by [`reconcile_branch`].
    pub requested_branch: Option<String>,
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
    /// Whether the server advertises the RFC 4528 Assertion control. When false,
    /// writes fall back to blind (no optimistic-concurrency protection).
    pub assertion_supported: bool,
    /// Set once the first blind (unprotected) write has warned the operator, so
    /// the "concurrent edits may be lost" notice is shown only once per session.
    pub concurrency_warned: bool,
    /// A pending concurrent-modification prompt (overlap case), surfaced by the
    /// dispatch layer as the Reload/Overwrite/Cancel dialog. See [`ConflictPrompt`].
    pub last_conflict: Option<ConflictPrompt>,
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
            scan_attrs: Vec::new(),
            branch_dns: Vec::new(),
            current_branch: None,
            current_leaf: None,
            search: String::new(),
            leaf_search: LeafSearchFlow::new(),
            leaf_search_rows: None,
            leaf_search_truncated: false,
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
            tree_dirty: false,
            requested_leaf: None,
            requested_branch: None,
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
            assertion_supported: false,
            concurrency_warned: false,
            last_conflict: None,
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
                    baseline_csn,
                    dn,
                    attrs,
                } => {
                    // Refresh this entry's structure node from the live read before
                    // installing the form, so the list/tree agree with what is shown.
                    self.upsert_from_read(&dn, &attrs);
                    let mut form = build_edit_form(&model, self.read_flow.schema(), self.read_only);
                    form.object_classes = object_classes;
                    form.baseline_csn = baseline_csn;
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
            // Entry-list find: Entries/SearchError with leaf-search ids (5_000_000+).
            let l_out = self.leaf_search.on_response(resp);
            if !matches!(l_out, LeafSearchOutcome::Ignored) {
                self.apply_leaf_search_outcome(l_out);
                out.changed = true;
                continue;
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
                renamed_from,
                quit_after,
            } => {
                self.status = "Saved.".to_string();
                // Check quit BEFORE the rename rescan below: on save-and-quit there is
                // no pane left to show the rescanned structure to, so a renamed
                // container would pay for a full blocking scan whose result is
                // immediately discarded.
                if quit_after {
                    out.quit = true;
                    return out;
                }
                // Our own write may have changed any label we cached (including via a
                // rename), and the cache stores negatives too — drop it wholesale and
                // let the visible fields re-resolve lazily.
                self.lookup_cache.clear();
                // A rename (MODRDN) invalidates the node under the OLD dn. The signal
                // travels with the write itself: deriving it from `current_leaf` would
                // misfire when the operator navigates away while a save is in flight,
                // deleting a live node. A rebuild is idempotent, so a rename always
                // asks for one — `Structure::remove` only reports the parent's
                // branch->leaf flip, not that a container's label moved in the tree.
                if let Some(old) = renamed_from {
                    // A renamed CONTAINER takes its whole subtree with it: every
                    // descendant DN changed on the server, so no local reflow is
                    // correct. Re-scan instead (the same work Alt+R does).
                    let was_branch = self
                        .structure
                        .get(&old)
                        .map(|n| n.is_branch())
                        .unwrap_or(false);
                    if was_branch {
                        let was_current = self
                            .current_branch
                            .as_deref()
                            .map(|b| b.eq_ignore_ascii_case(&old))
                            .unwrap_or(false);
                        // The rename itself already succeeded — a rescan failure here
                        // must not be reported as if the save had failed, replacing
                        // "Saved." with an error the operator would read as the write
                        // being lost. Combine the two into one message instead: the
                        // model is knowingly stale, but the save was not.
                        match self.reload_structure() {
                            // A successful rescan sets its own "Reloaded N entries.",
                            // which would silently replace the confirmation for the
                            // action the operator actually took. The rescan is an
                            // implementation detail of the rename; the save is the news.
                            Ok(_) => self.status = "Saved.".to_string(),
                            Err(msg) => {
                                self.status = format!("Saved, but the rescan failed: {msg}")
                            }
                        }
                        // Keep the operator on the container they just renamed
                        // rather than falling back to the base DN.
                        if was_current {
                            self.current_branch = Some(reread_dn.clone());
                        }
                    } else {
                        self.structure.remove(&old);
                    }
                    self.tree_dirty = true;
                    self.list_dirty = true;
                }
                // Navigate to the guard's target if one is pending, else re-read.
                // The plain-save fallback must carry the entry's REAL objectClasses
                // (from the just-saved form, still installed here) so the reread
                // resolves the same profile — and thus the same `show` field order —
                // as the original load. Passing empty ocs would pick no profile and
                // re-order the form (show-front block lost), desyncing the pane's
                // cached labels from the fresh values.
                let fallback_ocs = self
                    .edit_form
                    .as_ref()
                    .map(|f| f.object_classes.clone())
                    .unwrap_or_default();
                let (dn, profile_ocs) =
                    self.pending_nav.take().unwrap_or((reread_dn, fallback_ocs));
                self.reread(&dn, &profile_ocs);
            }
            WriteOutcome::NeedFollowupModify {
                dn,
                mods,
                renamed_from,
                quit_after,
            } => {
                if let Some(w) = self.worker.as_ref() {
                    let _ = self
                        .write_flow
                        .submit_followup(w, &dn, mods, renamed_from, quit_after);
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
                self.lookup_cache.clear();
                if quit_after {
                    out.quit = true;
                    return out;
                }
                // Same as `Saved`: carry the real objectClasses so the reread keeps
                // the original profile/`show` field order (see the note above).
                let fallback_ocs = self
                    .edit_form
                    .as_ref()
                    .map(|f| f.object_classes.clone())
                    .unwrap_or_default();
                let (dn, profile_ocs) =
                    self.pending_nav.take().unwrap_or((reread_dn, fallback_ocs));
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
            // Concurrent modification (rc 122): the assertion failed because the
            // entry changed on the server since we read it. Re-read it fresh, then
            // rebase silently (disjoint change) or stash a prompt (overlap).
            WriteOutcome::Conflict { dn, quit_after } => {
                let reread = self.reread_blocking_for_conflict(&dn);
                return self.resolve_conflict(dn, quit_after, reread);
            }
            WriteOutcome::Created { dn, quit_after } => {
                let ocs = self
                    .edit_form
                    .as_ref()
                    .map(|f| f.object_classes.clone())
                    .unwrap_or_default();
                // A leftover incremental-find query would hide the new row; the
                // cached labels may be stale for the same reason as on Saved.
                self.search.clear();
                // Harmless today only because `leaf_rows()` checks `search.is_empty()`
                // first — clear explicitly so this stays correct if that check moves.
                self.leaf_search_rows = None;
                self.leaf_search_truncated = false;
                // Cancel any in-flight find so its outcome, arriving after Created,
                // cannot overwrite `status` or re-install `leaf_search_rows`.
                self.leaf_search.cancel();
                self.lookup_cache.clear();
                self.current_leaf = Some(dn.clone());
                self.list_dirty = true;
                self.edit_form = None; // re-read reloads it in Edit mode
                if self.worker.is_some() {
                    // The write path's own re-read: use the non-clearing variant so
                    // a status set for this create survives to be seen.
                    self.reread(&dn, &ocs);
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

    /// The operator started a new action: whatever the status line was reporting
    /// described the previous one.
    ///
    /// Call this at the **call site** of each operator action, never inside a
    /// shared helper — `reread` is reached both by a navigation and by a rename's
    /// post-write re-read, and clearing there made every rename eat its own
    /// "Saved." (fixed in `c016f2a`).
    pub fn begin_operator_action(&mut self) {
        self.status.clear();
    }

    /// Public wrapper around the private `reread` for the dispatch closure.
    ///
    /// Every caller of this wrapper is an operator navigating to another entry
    /// (the guard's Discard path, the container chooser), so it clears `status`:
    /// a message describing the previous action no longer describes what is on
    /// screen. The write path calls the private [`reread`] instead, which does
    /// not clear — see the note there.
    pub fn reread_public(&mut self, dn: &str, ocs: &[String]) {
        self.begin_operator_action();
        self.reread(dn, ocs);
    }

    /// Submit a base-scope re-read of `dn`, selecting a profile by `ocs`.
    ///
    /// Deliberately does NOT clear `status`. This is the write path's re-read:
    /// `apply_write_outcome` sets "Saved." and calls this in the same breath to
    /// refresh the entry, so clearing here would erase the confirmation before it
    /// could be seen — including on a rename, where the re-read targets the NEW
    /// dn and so cannot be distinguished from navigation by comparing dns.
    /// Operator-initiated reads clear `status` at their own call sites
    /// ([`reread_public`] and [`reconcile_selection`]).
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

    /// The attribute names this save is writing — the labels of every dirty field.
    /// Delegates to [`EditForm::dirty_labels`]; empty when no form is loaded.
    pub fn attrs_in_flight(&self) -> Vec<String> {
        self.edit_form
            .as_ref()
            .map(|f| f.dirty_labels())
            .unwrap_or_default()
    }

    /// Synchronous base-scope re-read of `dn` requesting `*` + `entryCSN`, used on a
    /// write conflict to learn (fresh per-attribute values, the attribute names that
    /// differ from the form's stored baseline = the other client's changes, the
    /// fresh `entryCSN`). `None` when there is no worker or the read fails/returns
    /// nothing. Uses the blocking `worker.request` path (like
    /// `fetch_group_members_for_must`).
    fn reread_blocking_for_conflict(&self, dn: &str) -> Option<ConflictReread> {
        let worker = self.worker.as_ref()?;
        let resp = worker
            .request(Request::Search {
                id: 0,
                base: dn.to_string(),
                scope: SearchScope::Base,
                filter: "(objectClass=*)".to_string(),
                attrs: vec!["*".to_string(), "entryCSN".to_string()],
                size_limit: Some(1),
            })
            .ok()?;
        let Response::Entries { entries, .. } = resp else {
            return None;
        };
        let entry = entries.into_iter().next()?;
        let fresh_values = entry.attrs;
        let fresh_csn = fresh_values
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("entryCSN"))
            .and_then(|(_, v)| v.first().cloned());
        let changed = self.attrs_changed_since_baseline(&fresh_values);
        Some((fresh_values, changed, fresh_csn))
    }

    /// The form-field labels whose fresh server value differs from the form's stored
    /// baseline — i.e. what the other client changed since we read the entry.
    /// `entryCSN` is excluded (it always changes). Compares set-wise via
    /// `value_set_eq`, matching the dirty check's semantics.
    fn attrs_changed_since_baseline(
        &self,
        fresh: &std::collections::BTreeMap<String, Vec<String>>,
    ) -> Vec<String> {
        let Some(form) = self.edit_form.as_ref() else {
            return Vec::new();
        };
        form.fields
            .iter()
            .filter(|f| !f.label.eq_ignore_ascii_case("entryCSN"))
            .filter_map(|f| {
                let fresh_vals = fresh
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(&f.label))
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default();
                if crate::workflows::edit_form::value_set_eq(&fresh_vals, &f.baseline) {
                    None
                } else {
                    Some(f.label.clone())
                }
            })
            .collect()
    }

    /// Decide a concurrent modification given the (optional) re-read result. Pure
    /// enough to unit-test: pass a synthetic `reread`.
    ///
    /// - `None` re-read → surface a plain write error.
    /// - disjoint (our dirty fields vs their changed attrs do not overlap) → adopt
    ///   the fresh CSN, rebase baselines, and resubmit silently.
    /// - overlap → stash a [`ConflictPrompt`] for the dispatch layer.
    fn resolve_conflict(
        &mut self,
        dn: String,
        quit_after: bool,
        reread: Option<ConflictReread>,
    ) -> PumpResult {
        let mut out = PumpResult {
            changed: true,
            ..Default::default()
        };
        match reread {
            Some((fresh_values, changed_attrs, fresh_csn)) => {
                let ours = self.attrs_in_flight();
                let ours_refs: Vec<&str> = ours.iter().map(String::as_str).collect();
                let theirs_refs: Vec<&str> = changed_attrs.iter().map(String::as_str).collect();
                if !attrs_overlap(&ours_refs, &theirs_refs) {
                    // Disjoint → rebase silently: adopt the fresh CSN, fold the other
                    // client's changes into untouched fields, and resubmit our edit.
                    if let Some(f) = self.edit_form.as_mut() {
                        f.baseline_csn = fresh_csn;
                        rebase_baselines(f, &fresh_values);
                    }
                    self.resubmit_save(quit_after);
                } else {
                    self.last_conflict = Some(ConflictPrompt {
                        dn,
                        text: format!(
                            "This entry was changed by someone else since you opened \
                             it.\n\nConflicting attribute(s): {}.\n\nReload to discard \
                             your edits, Overwrite to force your version, or Cancel to \
                             keep editing.",
                            changed_attrs.join(", ")
                        ),
                        quit_after,
                        fresh_csn,
                    });
                    out.error = true;
                }
            }
            None => {
                self.last_write_error = Some(
                    "Entry changed on the server and could not be re-read. Reload before \
                     retrying."
                        .to_string(),
                );
                out.error = true;
            }
        }
        out
    }

    /// Re-run the save prepare + submit against the current (rebased) form, silently
    /// (no confirmation dialog — the operator already confirmed this save once, and
    /// the disjoint rebase changed nothing they need to re-approve). Asserts the
    /// form's current `baseline_csn` when the server supports it. No-op without a
    /// worker or a ready plan.
    fn resubmit_save(&mut self, quit_after: bool) {
        let plan_dn = {
            let Some(form) = self.edit_form.as_ref() else {
                return;
            };
            let prepared = self.write_flow.prepare(
                form,
                self.read_flow.schema(),
                self.pending_password.as_deref(),
                &self.resolved_widgets,
            );
            match prepared {
                crate::workflows::save::PrepareSave::Ready { plan, dn, .. } => Some((plan, dn)),
                _ => None,
            }
        };
        let Some((plan, dn)) = plan_dn else {
            return;
        };
        let assert_csn = if self.assertion_supported {
            self.edit_form.as_ref().and_then(|f| f.baseline_csn.clone())
        } else {
            None
        };
        let Self {
            worker, write_flow, ..
        } = self;
        if let Some(w) = worker.as_ref() {
            let _ = write_flow.submit(w, plan, &dn, assert_csn, quit_after);
        }
    }

    /// Apply a modal editor's typed `CommitOutcome` to the loaded form. For the
    /// resync variant: write the objectClass field values, mirror them into
    /// `object_classes`, then regenerate fields. Reads schema from `read_flow`
    /// (split-borrow so `edit_form` and `read_flow` are borrowed disjointly).
    pub fn apply_commit(&mut self, field_idx: usize, outcome: crate::ui::widget::CommitOutcome) {
        use crate::ui::widget::CommitOutcome;
        // Committing a field edit is a new operator action: any status left over
        // from a previous one (a save confirmation, a search failure) no longer
        // describes what's on screen. Cleared FIRST, before the match below, so it
        // cannot erase anything this call might set later (it sets none today).
        self.begin_operator_action();
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
            // The operator opened another entry: whatever the status line was
            // reporting described the previous action, not this one.
            self.begin_operator_action();
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
        // A container switch is a new operator action: any status left over from
        // the previous one (a search failure, a stale save confirmation) no longer
        // describes what's on screen. Cleared FIRST so a message this same call
        // sets later (there is none today, but a future one would) survives.
        self.begin_operator_action();
        self.current_branch = Some(dn);
        self.list_dirty = true;
        self.search = String::new();
        // Another container's live hits must not leak into this one.
        self.leaf_search_rows = None;
        self.leaf_search_truncated = false;
        // Cancel any in-flight find too: its response, once landed, would carry the
        // OLD container's DNs, and the deliberate "keep previous rows while the next
        // search is in flight" behaviour would then show them under the new one.
        self.leaf_search.cancel();
    }

    /// The entry list's find changed: mirror the query and answer it from the
    /// directory. An empty query drops the live rows and returns pane 2 to the
    /// container listing; a non-empty one submits a fresh one-level search whose
    /// predecessor (if any) is superseded. No-op without a worker or a branch.
    pub fn set_leaf_search(&mut self, query: String) {
        // Typing a new find query is a new operator action: any status left over
        // from a previous one no longer describes what's on screen. The find's own
        // outcome (e.g. the truncation notice) lands later, asynchronously, via
        // `apply_leaf_search_outcome` — never in this same call — so clearing here
        // cannot erase it before it is seen.
        self.begin_operator_action();
        self.search = query;
        self.list_dirty = true;
        if self.search.is_empty() {
            self.leaf_search_rows = None;
            self.leaf_search_truncated = false;
            return;
        }
        let Some(branch) = self.current_branch.clone() else {
            return;
        };
        let filter_attrs = crate::workflows::labels::label_rule_attrs(&self.label_rules);
        let Self {
            worker,
            leaf_search,
            scan_attrs,
            search,
            ..
        } = self;
        if let Some(w) = worker.as_ref() {
            let _ = leaf_search.request(w, &branch, search, &filter_attrs, scan_attrs);
        }
    }

    /// Apply one non-ignored find outcome.
    ///
    /// `Results`: upsert every hit into the structure (so entries other clients
    /// created become permanent local nodes, not transient rows), then keep their
    /// DNs as the list's row source. `Failed`: surface the error and drop back to
    /// the cached projection so the pane is never blank over a transient failure.
    pub fn apply_leaf_search_outcome(&mut self, out: LeafSearchOutcome) {
        match out {
            LeafSearchOutcome::Results { entries, truncated } => {
                let mut dns = Vec::with_capacity(entries.len());
                for e in &entries {
                    self.upsert_from_read(&e.dn, &e.attrs);
                    dns.push(e.dn.clone());
                }
                self.leaf_search_rows = Some(dns);
                self.leaf_search_truncated = truncated;
                if truncated {
                    self.status = format!(
                        "Showing the first {} matches — narrow the search.",
                        crate::workflows::leaf_search::LEAF_SEARCH_CAP
                    );
                }
                // `leaf_rows()` now answers from the rows just installed, so the
                // highlight is re-resolved from `leaf_highlight_plan` on the
                // coming rebuild rather than an index computed here against a row
                // source about to be replaced.
                self.list_dirty = true;
            }
            LeafSearchOutcome::Failed(msg) => {
                self.status = format!("Search failed: {msg}");
                self.leaf_search_rows = None;
                self.leaf_search_truncated = false;
                // Same reasoning as the `Results` arm above.
                self.list_dirty = true;
            }
            LeafSearchOutcome::Ignored => {}
        }
    }
}

impl UiState {
    /// (label, dn) rows for the current branch, using the configured column-2 label
    /// rules. Empty when no branch is selected.
    ///
    /// | State | Source |
    /// |---|---|
    /// | no query | the structure projection |
    /// | query + live results | those results, rendered and sorted by label |
    /// | query, none landed yet or the find failed | the cached projection, filtered |
    ///
    /// This is the single row source for the pane: the list's selection index maps
    /// 1:1 onto it, so the selection→DN mapping stays correct in every state.
    pub fn leaf_rows(&self) -> Vec<(String, String)> {
        let Some(branch) = self.current_branch.as_deref() else {
            return Vec::new();
        };
        match (self.search.is_empty(), self.leaf_search_rows.as_deref()) {
            (false, Some(dns)) => crate::workflows::labels::compute_rows_for_dns(
                &self.structure,
                branch,
                &self.search,
                &self.label_rules,
                dns,
            ),
            _ => crate::workflows::labels::compute_rows(
                &self.structure,
                branch,
                &self.search,
                &self.label_rules,
            ),
        }
    }

    /// Where the entry list's highlight belongs after a rebuild, and whether the
    /// form should follow it. See the truth table in the design doc.
    ///
    /// `Follow` is produced only for a **clean** form: typing a find is
    /// navigation, so the form tracks the first hit — but never at the cost of
    /// unsaved edits, and never by raising the dirty guard mid-keystroke.
    ///
    /// The `‹self›` row (the branch's own entry, always row 0 of `leaf_rows`
    /// when the branch carries no active filter) is not a "first hit": it is
    /// the I4 trap (see the design doc) that let a plain rebuild drag the form
    /// onto the container. It still answers `Pin(current_leaf)` when the
    /// operator has it open, but it is never the *fallback* first row.
    pub fn leaf_highlight_plan(&self) -> HighlightPlan {
        let rows = self.leaf_rows();
        let branch = self.current_branch.as_deref();
        let is_self_row = |dn: &str| branch.is_some_and(|b| dn.eq_ignore_ascii_case(b));
        // "Open entry is in the rows" is checked before any "absent" case, per
        // the truth table — including when the open entry IS the `‹self›` row
        // (a childless container the operator has open). Only once that's
        // ruled out does the fallback-first-row search, which skips `‹self›`,
        // get to decide there is nothing to highlight.
        if let Some(cur) = self.current_leaf.as_deref() {
            if rows.iter().any(|(_, dn)| dn.eq_ignore_ascii_case(cur)) {
                return HighlightPlan::Pin(cur.to_string());
            }
            let Some((_, first_dn)) = rows.iter().find(|(_, dn)| !is_self_row(dn)) else {
                return HighlightPlan::Clear;
            };
            let dirty = self
                .edit_form
                .as_ref()
                .map(|f| f.is_dirty())
                .unwrap_or(false);
            if dirty {
                return HighlightPlan::Pin(first_dn.clone());
            }
            return HighlightPlan::Follow(first_dn.clone());
        }
        match rows.iter().find(|(_, dn)| !is_self_row(dn)) {
            Some((_, first_dn)) => HighlightPlan::Pin(first_dn.clone()),
            None => HighlightPlan::Clear,
        }
    }

    /// Where the tree's highlight belongs after a rebuild. Only ever `Pin` or
    /// `Clear`: unlike the entry list, the tree never moves the form by itself.
    pub fn branch_highlight_plan(&self) -> HighlightPlan {
        let Some(cur) = self.current_branch.as_deref() else {
            return HighlightPlan::Clear;
        };
        if self.branch_dns.iter().any(|d| d.eq_ignore_ascii_case(cur)) {
            HighlightPlan::Pin(cur.to_string())
        } else {
            HighlightPlan::Clear
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

    /// Refresh the structure node for a freshly-read entry.
    ///
    /// Projects the raw attributes onto the label/tree template attributes
    /// (`scan_attrs`) plus `objectClass`, so a node never carries the entry's whole
    /// attribute set, then upserts it. Marks the leaf list dirty and — when the
    /// upsert reports a branch-level change — the tree too. `list_dirty` is what
    /// drives the leaf pane's next rebuild, which resolves `leaf_highlight_plan`
    /// against the fresh rows — pinning to the refreshed entry when it is the one
    /// on screen, which is what makes a newly created entry both appear AND
    /// become selected.
    ///
    /// Called for every entry read: navigation clicks and post-write re-reads alike,
    /// so any entry the operator visits self-heals from live data.
    pub(crate) fn upsert_from_read(
        &mut self,
        dn: &str,
        attrs: &std::collections::BTreeMap<String, Vec<String>>,
    ) {
        let first = |name: &str| {
            attrs
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .and_then(|(_, v)| v.first().cloned())
        };
        let mut kept: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        for want in &self.scan_attrs {
            if let Some((k, v)) = attrs.iter().find(|(k, _)| k.eq_ignore_ascii_case(want)) {
                kept.insert(k.clone(), v.clone());
            }
        }
        let object_classes = attrs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("objectClass"))
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        let input = StructureInput {
            dn: dn.to_string(),
            cn: first("cn"),
            description: first("description"),
            object_classes,
            attrs: kept,
        };
        if self.structure.upsert(input) {
            self.tree_dirty = true;
        }
        self.list_dirty = true;
    }

    /// Install a freshly scanned structure, keeping the operator's place.
    ///
    /// The current container and entry are preserved **by DN** when they still
    /// exist; a vanished container falls back to the base DN and a vanished entry to
    /// no selection. Every projection derived from the old scan is dropped: the find
    /// query, its live rows, and the reverse-label cache (which caches negatives, so
    /// a stale miss would otherwise outlive the refresh). Pure — no I/O — so the
    /// place-keeping rules are unit-testable.
    pub fn adopt_structure(&mut self, structure: Structure) {
        self.structure = structure;
        if let Some(branch) = self.current_branch.clone() {
            if self.structure.get(&branch).is_none() {
                self.current_branch = Some(self.base_dn.clone());
            }
        }
        if let Some(leaf) = self.current_leaf.clone() {
            if self.structure.get(&leaf).is_none() {
                self.current_leaf = None;
            }
        }
        self.search.clear();
        self.leaf_search_rows = None;
        self.leaf_search_truncated = false;
        // Abandon any in-flight find: its response would arrive after the reload and,
        // although its rows can no longer render (the query is cleared), its status
        // message would clobber the reload confirmation the operator just triggered.
        self.leaf_search.cancel();
        self.lookup_cache.clear();
        self.list_dirty = true;
        self.tree_dirty = true;
        // The pane rebuilds from scratch on the coming REFRESH and resolves
        // `leaf_highlight_plan` against the fresh rows itself — pinning back to
        // `current_leaf` when it survived, or clearing when it is gone.
    }

    /// Re-run the eager structure scan and adopt the result (Alt+R).
    ///
    /// Blocking, like the bootstrap scan it repeats: the TUI is unresponsive for its
    /// duration, which is acceptable for an explicit, operator-initiated action. The
    /// open edit form is deliberately left untouched, so unsaved work is never at
    /// risk and no dirty-form guard is needed. On failure the previous structure is
    /// kept and the error is surfaced in the status line *and* returned so the
    /// caller can put it in front of the operator — a failed reload must never look
    /// like a successful no-op one.
    ///
    /// Returns `Ok(count)` (entries adopted) on success, `Err(msg)` on failure.
    /// With no worker attached there is nothing to reload; treated as a no-op
    /// success (`Ok(0)`) rather than an error, since it is not a failed action.
    pub fn reload_structure(&mut self) -> Result<usize, String> {
        let Some(worker) = self.worker.as_ref() else {
            return Ok(0);
        };
        let resp = worker.request(Request::LoadStructure {
            id: 0,
            base: self.base_dn.clone(),
            page_size: 500,
            attrs: self.scan_attrs.clone(),
        });
        match resp {
            Ok(Response::StructureEntries { nodes, .. }) => {
                let count = nodes.len();
                let structure = Structure::build(
                    &self.base_dn,
                    crate::workflows::labels::structure_inputs(nodes),
                );
                self.adopt_structure(structure);
                self.status = format!("Reloaded {count} entries.");
                Ok(count)
            }
            Ok(Response::StructureError { msg, .. }) => {
                self.status = format!("Reload failed: {msg}");
                Err(msg)
            }
            Ok(other) => {
                let msg = format!("unexpected {other:?}");
                self.status = format!("Reload failed: {msg}");
                Err(msg)
            }
            Err(e) => {
                let msg = e.to_string();
                self.status = format!("Reload failed: {msg}");
                Err(msg)
            }
        }
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

    // Tolerant capability probe: a failed/absent root DSE just means "no
    // support" for txn / assertion (never fail bootstrap over it).
    let (server_supports_txn, assertion_supported) = match worker.request(Request::FetchRootDse) {
        Ok(Response::RootDse {
            supported_extensions,
            supported_controls,
        }) => (
            crate::ldap::worker::txn_supported(&supported_extensions),
            crate::ldap::worker::assertion_supported(&supported_controls),
        ),
        _ => (false, false),
    };

    let nodes = match worker.request(Request::LoadStructure {
        id: 0,
        base: base_dn.clone(),
        page_size: 500,
        attrs: scan_attrs.clone(),
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
        scan_attrs: scan_attrs.clone(),
        branch_dns: Vec::new(),
        current_branch: None,
        current_leaf: None,
        search: String::new(),
        leaf_search: LeafSearchFlow::new(),
        leaf_search_rows: None,
        leaf_search_truncated: false,
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
        tree_dirty: false,
        requested_leaf: None,
        requested_branch: None,
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
        assertion_supported,
        concurrency_warned: false,
        last_conflict: None,
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
            baseline_csn: None,
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
            baseline_csn: None,
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
            baseline_csn: None,
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
            baseline_csn: None,
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
            baseline_csn: None,
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
            baseline_csn: None,
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
            baseline_csn: None,
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
            baseline_csn: None,
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

    #[test]
    fn upsert_from_read_projects_scan_attrs_and_marks_dirty() {
        let structure = Structure::build("dc=x", vec![si("dc=x", None), si("ou=p,dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.scan_attrs = vec!["cn".to_string()];
        let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        attrs.insert("cn".to_string(), vec!["Bob".to_string()]);
        attrs.insert("objectClass".to_string(), vec!["person".to_string()]);
        // `sn` is NOT in scan_attrs and must not be stored on the node.
        attrs.insert("sn".to_string(), vec!["Baker".to_string()]);

        st.upsert_from_read("uid=bob,ou=p,dc=x", &attrs);

        let node = st
            .structure
            .get("uid=bob,ou=p,dc=x")
            .expect("node inserted");
        assert_eq!(node.label, "Bob", "label rendered from cn");
        assert_eq!(node.object_classes, vec!["person".to_string()]);
        assert!(node.attrs.contains_key("cn"));
        assert!(
            !node.attrs.contains_key("sn"),
            "only scan_attrs are projected onto the node"
        );
        assert!(st.list_dirty, "the leaf list must rebuild");
        assert!(
            st.tree_dirty,
            "ou=p flipped leaf->branch, so the tree must rebuild too"
        );
    }

    #[test]
    fn upsert_from_read_snaps_the_highlight_to_the_shown_entry() {
        let structure = Structure::build("dc=x", vec![si("dc=x", None), si("ou=p,dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.scan_attrs = vec!["cn".to_string()];
        st.current_branch = Some("ou=p,dc=x".into());
        st.current_leaf = Some("uid=bob,ou=p,dc=x".into());
        let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        attrs.insert("cn".to_string(), vec!["Bob".to_string()]);

        st.upsert_from_read("uid=bob,ou=p,dc=x", &attrs);

        // Rows are [‹self› ou=p, Bob] → the new entry is among them, so the
        // highlight plan pins to it rather than falling back to the ‹self› row.
        assert_eq!(
            st.leaf_highlight_plan(),
            HighlightPlan::Pin("uid=bob,ou=p,dc=x".to_string())
        );
    }

    #[test]
    fn saved_under_a_new_dn_drops_the_stale_node() {
        // A rename (MODRDN) makes the server echo a different DN than the form was
        // loaded with; the old node must not linger in the entry list.
        let structure = Structure::build(
            "dc=x",
            vec![
                si("dc=x", None),
                si("ou=p,dc=x", None),
                si("uid=old,ou=p,dc=x", Some("Old")),
            ],
        );
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.current_branch = Some("ou=p,dc=x".into());
        st.current_leaf = Some("uid=old,ou=p,dc=x".into());

        st.apply_write_outcome(WriteOutcome::Saved {
            reread_dn: "uid=new,ou=p,dc=x".into(),
            renamed_from: Some("uid=old,ou=p,dc=x".into()),
            quit_after: false,
        });

        assert!(
            st.structure.get("uid=old,ou=p,dc=x").is_none(),
            "the pre-rename node must be removed"
        );
        assert!(st.list_dirty);
        assert!(st.tree_dirty, "a rename must trigger a tree rebuild too");
    }

    /// Renaming a CONTAINER must not orphan its subtree. A plain `structure.remove`
    /// deletes exactly the renamed node — its children stay linked under the OLD DN,
    /// which no longer exists on the server. That case must re-scan instead (no
    /// local reflow is correct: every descendant DN changed too), so it must NOT
    /// take the plain-`remove` path a leaf rename takes.
    ///
    /// `new_for_test` installs no worker, so `reload_structure` returns early and
    /// leaves `structure` untouched — exactly the observable difference from the
    /// leaf case above (which unconditionally removes the old node). That is what
    /// this test asserts: not that a rescan happened (it can't, without a worker),
    /// but that the branch case did NOT delete the old node/subtree the way the
    /// leaf case does.
    #[test]
    fn saved_renaming_a_container_does_not_orphan_its_subtree() {
        let structure = Structure::build(
            "dc=x",
            vec![
                si("dc=x", None),
                si("ou=old,dc=x", None),
                si("uid=child,ou=old,dc=x", Some("Child")),
            ],
        );
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.current_branch = Some("ou=old,dc=x".into());

        st.apply_write_outcome(WriteOutcome::Saved {
            reread_dn: "ou=new,dc=x".into(),
            renamed_from: Some("ou=old,dc=x".into()),
            quit_after: false,
        });

        // Unlike the leaf case, the old branch and its child are left in place for
        // the rescan to replace — a plain `remove` would delete the container and
        // strand the child under a DN that no longer exists on the server.
        assert!(
            st.structure.get("ou=old,dc=x").is_some(),
            "a renamed CONTAINER must not be plain-removed — that would orphan its subtree"
        );
        assert!(
            st.structure.get("uid=child,ou=old,dc=x").is_some(),
            "the child must not be stranded under the old, now-invalid DN"
        );
        assert!(st.list_dirty);
        assert!(st.tree_dirty);
        // The operator stays on the container they just renamed rather than falling
        // back to the base DN.
        assert_eq!(st.current_branch.as_deref(), Some("ou=new,dc=x"));
    }

    #[test]
    fn saved_under_the_same_dn_keeps_the_node() {
        let structure = Structure::build(
            "dc=x",
            vec![
                si("dc=x", None),
                si("ou=p,dc=x", None),
                si("uid=a,ou=p,dc=x", Some("A")),
            ],
        );
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.current_leaf = Some("uid=a,ou=p,dc=x".into());

        st.apply_write_outcome(WriteOutcome::Saved {
            reread_dn: "uid=a,ou=p,dc=x".into(),
            renamed_from: None,
            quit_after: false,
        });

        assert!(st.structure.get("uid=a,ou=p,dc=x").is_some());
    }

    /// Regression for the race in commit `dcfdab5`'s rename detection.
    /// The buggy code inferred a rename from `current_leaf != reread_dn`. When the
    /// operator navigated away from A to B while A's save was in flight, the
    /// delayed save response left `current_leaf = B` but `reread_dn = A`, triggering
    /// the false-rename check. The buggy code then deleted the node named by
    /// `current_leaf` (B), not the saved entry (A). The fix carries the rename
    /// signal explicitly via `renamed_from: Option<String>`, so a plain
    /// (non-renaming) save never deletes any node regardless of `current_leaf`.
    #[test]
    fn navigating_away_during_a_save_does_not_delete_the_saved_node() {
        let structure = Structure::build(
            "dc=x",
            vec![
                si("dc=x", None),
                si("ou=p,dc=x", None),
                si("uid=a,ou=p,dc=x", Some("A")),
                si("uid=b,ou=p,dc=x", Some("B")),
            ],
        );
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        // The user saved A, then navigated to B (e.g. discarded A's now-dirty
        // form) before A's WriteOk came back. This leaves:
        // - current_leaf = B (where the operator is now)
        // - a Saved outcome for A in flight
        st.current_leaf = Some("uid=b,ou=p,dc=x".into());

        st.apply_write_outcome(WriteOutcome::Saved {
            reread_dn: "uid=a,ou=p,dc=x".into(),
            renamed_from: None,
            quit_after: false,
        });

        // Assert that B (the entry the operator navigated to) still exists.
        // This is the node the buggy code would have incorrectly deleted,
        // because it mistook the current_leaf/reread_dn mismatch for a rename.
        assert!(
            st.structure.get("uid=b,ou=p,dc=x").is_some(),
            "navigating away does not delete the live entry; the buggy code would \
             have deleted current_leaf when comparing current_leaf != reread_dn"
        );
        // Also assert that A (the saved entry) still exists (a non-rename save
        // deletes nothing).
        assert!(
            st.structure.get("uid=a,ou=p,dc=x").is_some(),
            "a plain save never deletes the saved node"
        );
    }

    #[test]
    fn a_write_clears_the_lookup_cache() {
        use crate::workflows::resolve_flow::LookupKey;
        let structure = Structure::build("dc=x", vec![si("dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        let key = LookupKey {
            scope_id: "dc=x|posixGroup|gidNumber".into(),
            value: "5000".into(),
        };
        st.lookup_cache.insert(key.clone(), Some("staff".into()));
        st.current_leaf = Some("uid=a,dc=x".into());

        st.apply_write_outcome(WriteOutcome::Saved {
            reread_dn: "uid=a,dc=x".into(),
            renamed_from: None,
            quit_after: false,
        });

        assert!(
            st.lookup_cache.is_empty(),
            "our own write may have changed any label — drop the whole cache"
        );
    }

    #[test]
    fn created_clears_a_stale_find_query() {
        let structure = Structure::build("dc=x", vec![si("dc=x", None), si("ou=p,dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.current_branch = Some("ou=p,dc=x".into());
        st.search = "zzz".into();
        // Simulate a find in flight when Created lands.
        st.leaf_search.force_latest(9_999_999);

        st.apply_write_outcome(WriteOutcome::Created {
            dn: "uid=bob,ou=p,dc=x".into(),
            quit_after: false,
        });

        assert!(
            st.search.is_empty(),
            "a stale query must not hide the entry just created"
        );
        // The in-flight find must be cancelled too, like commit_branch/adopt_structure
        // already do — otherwise its outcome would arrive after Created and overwrite
        // `status` or re-install `leaf_search_rows`.
        let resp = crate::ldap::worker::Response::Entries {
            id: 9_999_999,
            entries: vec![],
            truncated: false,
        };
        assert_eq!(
            st.leaf_search.on_response(&resp),
            LeafSearchOutcome::Ignored,
            "the pre-Created find must have been cancelled"
        );
    }

    #[test]
    fn leaf_rows_uses_live_results_when_a_query_is_active() {
        let structure = Structure::build(
            "dc=x",
            vec![
                si("dc=x", None),
                si("ou=p,dc=x", None),
                si("uid=a,ou=p,dc=x", Some("A")),
            ],
        );
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.current_branch = Some("ou=p,dc=x".into());

        // No query → the structure projection.
        assert_eq!(st.leaf_rows().len(), 2, "‹self› + uid=a");

        // Query with live results in hand → the live rows.
        st.scan_attrs = vec!["cn".to_string()];
        let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        attrs.insert("cn".to_string(), vec!["Bee".to_string()]);
        st.upsert_from_read("uid=b,ou=p,dc=x", &attrs);
        st.search = "bee".into();
        st.leaf_search_rows = Some(vec!["uid=b,ou=p,dc=x".to_string()]);
        let rows = st.leaf_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, "uid=b,ou=p,dc=x");

        // Query with NO results yet (in flight or failed) → cached filter fallback.
        st.leaf_search_rows = None;
        st.search = "a".into();
        assert_eq!(
            st.leaf_rows().len(),
            1,
            "falls back to filtering the cached projection"
        );
    }

    #[test]
    fn switching_branch_drops_live_results() {
        let structure = Structure::build("dc=x", vec![si("dc=x", None), si("ou=p,dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.search = "ann".into();
        st.leaf_search_rows = Some(vec!["uid=ann,ou=q,dc=x".to_string()]);

        st.commit_branch("ou=p,dc=x".into());

        assert!(st.search.is_empty());
        assert!(
            st.leaf_search_rows.is_none(),
            "another branch's hits must not leak into this one"
        );
    }

    #[test]
    fn empty_query_clears_live_results_without_a_search() {
        let structure = Structure::build("dc=x", vec![si("dc=x", None), si("ou=p,dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.current_branch = Some("ou=p,dc=x".into());
        st.leaf_search_rows = Some(vec!["uid=ann,ou=p,dc=x".to_string()]);

        st.set_leaf_search(String::new());

        assert!(st.leaf_search_rows.is_none());
        assert!(st.list_dirty);
    }

    /// A non-empty query with a worker and a current branch must submit a
    /// one-level `Request::Search` scoped to that branch, capped at
    /// `LEAF_SEARCH_CAP`.
    #[test]
    fn set_leaf_search_submits_a_scoped_one_level_search() {
        let structure = Structure::build("dc=x", vec![si("dc=x", None), si("ou=p,dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.current_branch = Some("ou=p,dc=x".into());
        // A differently-cased "CN" is already among the caller's fetch attrs — the
        // augmentation must recognize it and not add a case-duplicate "cn".
        st.scan_attrs = vec!["CN".to_string()];
        let (worker, rx) = WorkerHandle::recording();
        st.worker = Some(worker);

        st.set_leaf_search("ann".into());

        let (req, _) = rx.try_recv().expect("a search must have been submitted");
        match req {
            Request::Search {
                base,
                scope,
                filter,
                size_limit,
                attrs,
                ..
            } => {
                assert_eq!(base, "ou=p,dc=x");
                assert_eq!(scope, SearchScope::OneLevel);
                assert_eq!(filter, "(|(cn=*ann*)(uid=*ann*))");
                assert_eq!(
                    size_limit,
                    Some(crate::workflows::leaf_search::LEAF_SEARCH_CAP)
                );
                // `cn`/`description`/`objectClass` are appended for the row label,
                // case-insensitively deduped against the caller's own attrs — exactly
                // one of each, never a case-duplicate.
                for want in ["cn", "description", "objectClass"] {
                    assert_eq!(
                        attrs
                            .iter()
                            .filter(|a| a.eq_ignore_ascii_case(want))
                            .count(),
                        1,
                        "{want} must appear exactly once in {attrs:?}"
                    );
                }
            }
            other => panic!("expected Request::Search, got {other:?}"),
        }
    }

    /// `Results` upserts the hits into the structure (so another client's entry
    /// becomes a permanent local node), keeps their DNs as `leaf_search_rows`, and
    /// marks the list dirty.
    #[test]
    fn apply_leaf_search_results_upserts_and_sets_rows() {
        use crate::ldap::worker::LdapEntry;

        let structure = Structure::build("dc=x", vec![si("dc=x", None), si("ou=p,dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.current_branch = Some("ou=p,dc=x".into());
        st.list_dirty = false;

        let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        attrs.insert("cn".to_string(), vec!["Ann".to_string()]);
        let entry = LdapEntry {
            dn: "uid=ann,ou=p,dc=x".to_string(),
            attrs,
            bin_attrs: Default::default(),
        };

        st.apply_leaf_search_outcome(LeafSearchOutcome::Results {
            entries: vec![entry],
            truncated: false,
        });

        assert!(
            st.structure.get("uid=ann,ou=p,dc=x").is_some(),
            "a hit from another client must become a permanent local node"
        );
        assert_eq!(
            st.leaf_search_rows,
            Some(vec!["uid=ann,ou=p,dc=x".to_string()])
        );
        assert!(!st.leaf_search_truncated);
        assert!(st.list_dirty);
    }

    /// `upsert_from_read` (called once per hit, inside the loop) no longer computes
    /// any snap itself — `leaf_highlight_plan` is resolved once, on the pane's next
    /// rebuild, against `self.leaf_rows()` as it reads AFTER the loop replaces
    /// `leaf_search_rows`. So the plan must name Zoe regardless of whatever row the
    /// STALE single-hit `leaf_search_rows` (the previous query's answer, still in
    /// place mid-loop) would have given her.
    #[test]
    fn apply_leaf_search_results_recomputes_the_snap_row_against_the_new_rows() {
        use crate::ldap::worker::LdapEntry;

        // ou=p,dc=x already contains "Zoe" (found by an earlier, different query);
        // its label is the plain RDN "ou=p", which does not contain "z", so the
        // ‹self› row is filtered out of every projection below.
        let structure = Structure::build(
            "dc=x",
            vec![
                si("dc=x", None),
                si("ou=p,dc=x", None),
                si("uid=zoe,ou=p,dc=x", Some("Zoe")),
            ],
        );
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.current_branch = Some("ou=p,dc=x".into());
        st.current_leaf = Some("uid=zoe,ou=p,dc=x".into());
        st.search = "z".to_string();
        // The stale row source from the previous query: just Zoe, at index 0.
        st.leaf_search_rows = Some(vec!["uid=zoe,ou=p,dc=x".to_string()]);

        let mut liz_attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        liz_attrs.insert("cn".to_string(), vec!["Liz".to_string()]);
        let mut zoe_attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        zoe_attrs.insert("cn".to_string(), vec!["Zoe".to_string()]);

        st.apply_leaf_search_outcome(LeafSearchOutcome::Results {
            entries: vec![
                LdapEntry {
                    dn: "uid=liz,ou=p,dc=x".to_string(),
                    attrs: liz_attrs,
                    bin_attrs: Default::default(),
                },
                LdapEntry {
                    dn: "uid=zoe,ou=p,dc=x".to_string(),
                    attrs: zoe_attrs,
                    bin_attrs: Default::default(),
                },
            ],
            truncated: false,
        });

        // Final rows, sorted by label: [Liz, Zoe] — Zoe is row 1, not row 0.
        assert_eq!(
            st.leaf_rows(),
            vec![
                ("Liz".to_string(), "uid=liz,ou=p,dc=x".to_string()),
                ("Zoe".to_string(), "uid=zoe,ou=p,dc=x".to_string()),
            ]
        );
        assert_eq!(
            st.leaf_highlight_plan(),
            HighlightPlan::Pin("uid=zoe,ou=p,dc=x".to_string()),
            "must pin Zoe by DN against the FINAL rows, not the row the stale \
             previous-query source gave it mid-loop"
        );
    }

    /// `Results { truncated: true }` sets `leaf_search_truncated` and reports the
    /// cap in `status`.
    #[test]
    fn apply_leaf_search_results_truncated_reports_the_cap() {
        let structure = Structure::build("dc=x", vec![si("dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());

        st.apply_leaf_search_outcome(LeafSearchOutcome::Results {
            entries: vec![],
            truncated: true,
        });

        assert!(st.leaf_search_truncated);
        assert!(
            st.status
                .contains(&crate::workflows::leaf_search::LEAF_SEARCH_CAP.to_string()),
            "status must mention the cap: {}",
            st.status
        );
    }

    /// `Failed` surfaces the error, drops `leaf_search_rows` (so `leaf_rows()`
    /// falls back to the cached projection instead of a blank pane), and marks
    /// the list dirty.
    #[test]
    fn apply_leaf_search_failed_falls_back_to_cached_projection() {
        let structure = Structure::build("dc=x", vec![si("dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.leaf_search_rows = Some(vec!["uid=stale,dc=x".to_string()]);
        st.list_dirty = false;

        st.apply_leaf_search_outcome(LeafSearchOutcome::Failed("Operations error".into()));

        assert!(st.status.contains("Operations error"));
        assert!(st.leaf_search_rows.is_none());
        assert!(st.list_dirty);
    }

    /// I4 regression: a `Results` outcome whose hits do NOT include `current_leaf`
    /// is the common case for a find (the operator is looking at one entry while
    /// searching for another). The rendered rows are exactly the hits, so
    /// `current_leaf` genuinely has no row here — `leaf_highlight_plan` must fall
    /// through to its "open entry absent" branch and follow the first hit (the form
    /// is clean) rather than pin a stale row, which is what let the pane's row-0
    /// refocus on the coming rebuild be reported as a fresh selection, navigating
    /// the form away and clearing the status this outcome just set.
    #[test]
    fn apply_leaf_search_results_without_current_leaf_among_hits_recomputes_the_snap() {
        use crate::ldap::worker::LdapEntry;

        let structure = Structure::build(
            "dc=x",
            vec![
                si("dc=x", None),
                si("ou=p,dc=x", None),
                si("uid=ann,ou=p,dc=x", Some("Ann")),
                si("uid=bob,ou=p,dc=x", Some("Bob")),
            ],
        );
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.current_branch = Some("ou=p,dc=x".into());
        st.current_leaf = Some("uid=bob,ou=p,dc=x".into());
        st.search = "ann".to_string();

        let mut ann_attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        ann_attrs.insert("cn".to_string(), vec!["Ann".to_string()]);
        st.apply_leaf_search_outcome(LeafSearchOutcome::Results {
            entries: vec![LdapEntry {
                dn: "uid=ann,ou=p,dc=x".to_string(),
                attrs: ann_attrs,
                bin_attrs: Default::default(),
            }],
            truncated: false,
        });

        assert_eq!(
            st.leaf_highlight_plan(),
            HighlightPlan::Follow("uid=ann,ou=p,dc=x".to_string()),
            "Bob (current_leaf) is not among the hits, so a clean form follows \
             the search to the first hit instead of leaving a stale highlight"
        );
    }

    /// I4 regression: when `current_leaf` is still present in the
    /// cached-projection fallback (the rows `Failed` drops back to),
    /// `leaf_highlight_plan` must pin to it so the pane's row-0 refocus is not
    /// mistaken for a fresh selection — which would navigate the form away and
    /// erase the "Search failed: …" message just set above.
    #[test]
    fn apply_leaf_search_failed_snaps_to_current_leaf_in_the_fallback_projection() {
        let structure = Structure::build(
            "dc=x",
            vec![
                si("dc=x", None),
                si("ou=p,dc=x", None),
                si("uid=ann,ou=p,dc=x", Some("Ann")),
            ],
        );
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.current_branch = Some("ou=p,dc=x".into());
        st.current_leaf = Some("uid=ann,ou=p,dc=x".into());
        // A previous (now failing) query's rows, about to be dropped.
        st.leaf_search_rows = Some(vec!["uid=stale,ou=p,dc=x".to_string()]);

        st.apply_leaf_search_outcome(LeafSearchOutcome::Failed("Operations error".into()));

        // Fallback projection: [‹self› ou=p, Ann] — Ann is row 1.
        assert_eq!(
            st.leaf_rows(),
            vec![
                ("‹self› ou=p".to_string(), "ou=p,dc=x".to_string()),
                ("Ann".to_string(), "uid=ann,ou=p,dc=x".to_string()),
            ]
        );
        assert_eq!(
            st.leaf_highlight_plan(),
            HighlightPlan::Pin("uid=ann,ou=p,dc=x".to_string()),
            "current_leaf is still visible in the fallback projection, so the \
             plan must pin to it"
        );
    }

    /// Regression for the cross-branch leak: a search issued under container A
    /// still in flight when the operator commits to container B must be ignored
    /// when its response lands, even though B's own query has landed rows.
    #[test]
    fn commit_branch_cancels_in_flight_search_so_its_response_is_ignored() {
        use crate::ldap::worker::LdapEntry;

        let structure = Structure::build(
            "dc=x",
            vec![
                si("dc=x", None),
                si("ou=a,dc=x", None),
                si("ou=b,dc=x", None),
            ],
        );
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.current_branch = Some("ou=a,dc=x".into());
        let (worker, rx) = WorkerHandle::recording();
        st.worker = Some(worker);

        // A find is issued under container A and lands rows (deliberately kept
        // visible in flight — see the module docs).
        st.set_leaf_search("ann".into());
        let (req_a, _) = rx.try_recv().expect("container A's search was submitted");
        let id_a = match req_a {
            Request::Search { id, .. } => id,
            other => panic!("expected Request::Search, got {other:?}"),
        };

        // Operator switches to container B before A's response lands.
        st.commit_branch("ou=b,dc=x".into());

        // A's (now superseded) response finally arrives.
        let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        attrs.insert("cn".to_string(), vec!["Ann".to_string()]);
        let resp = Response::Entries {
            id: id_a,
            entries: vec![LdapEntry {
                dn: "uid=ann,ou=a,dc=x".to_string(),
                attrs,
                bin_attrs: Default::default(),
            }],
            truncated: false,
        };
        st.pump_responses_for_test(&[resp]);

        assert!(
            st.leaf_search_rows.is_none(),
            "container A's cancelled search must not leak its DNs into container B"
        );
    }

    /// `status` must not pin the status line forever: switching containers,
    /// typing a find query, or committing a field edit is a new operator action,
    /// so any status left over from a previous one is cleared.
    #[test]
    fn commit_branch_clears_a_stale_status() {
        let structure = Structure::build("dc=x", vec![si("dc=x", None), si("ou=p,dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.status = "Reload failed: timeout".into();

        st.commit_branch("ou=p,dc=x".into());

        assert!(st.status.is_empty());
    }

    #[test]
    fn set_leaf_search_clears_a_stale_status() {
        let structure = Structure::build("dc=x", vec![si("dc=x", None), si("ou=p,dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.status = "Saved.".into();

        st.set_leaf_search("ann".into());

        assert!(st.status.is_empty());
    }

    #[test]
    fn apply_commit_clears_a_stale_status() {
        use crate::ui::widget::CommitOutcome;

        let structure = Structure::build("dc=x", vec![si("dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.status = "Saved.".into();

        st.apply_commit(0, CommitOutcome::Cancelled);

        assert!(st.status.is_empty());
    }

    /// The read that follows a save re-reads the SAME entry it just wrote —
    /// `reread`'s status-clearing must not treat that as "opening another entry",
    /// or "Saved." would be erased before the operator ever sees it.
    #[test]
    fn saved_status_survives_the_post_save_reread_of_the_same_entry() {
        let structure = Structure::build(
            "dc=x",
            vec![
                si("dc=x", None),
                si("ou=p,dc=x", None),
                si("uid=a,ou=p,dc=x", Some("A")),
            ],
        );
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.current_leaf = Some("uid=a,ou=p,dc=x".into());

        st.apply_write_outcome(WriteOutcome::Saved {
            reread_dn: "uid=a,ou=p,dc=x".into(),
            renamed_from: None,
            quit_after: false,
        });

        assert_eq!(st.status, "Saved.");
    }

    /// By contrast, reading a genuinely DIFFERENT entry — the operator navigating
    /// the entry list — clears a stale status, via `reconcile_selection`'s clean
    /// (non-dirty) path into `reread`.
    #[test]
    fn reconcile_selection_clears_a_stale_status_when_opening_a_different_entry() {
        let structure = Structure::build(
            "dc=x",
            vec![
                si("dc=x", None),
                si("ou=p,dc=x", None),
                si("uid=a,ou=p,dc=x", Some("A")),
                si("uid=b,ou=p,dc=x", Some("B")),
            ],
        );
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.current_leaf = Some("uid=a,ou=p,dc=x".into());
        st.status = "Search failed: timeout".into();
        st.request_leaf("uid=b,ou=p,dc=x".into(), Vec::new());

        let guard_raised = st.reconcile_selection();

        assert!(!guard_raised, "a clean form must not raise the guard");
        assert!(
            st.status.is_empty(),
            "opening a different entry must clear a stale status"
        );
    }

    #[test]
    fn adopt_structure_keeps_a_still_existing_branch_and_leaf() {
        let old = Structure::build(
            "dc=x",
            vec![
                si("dc=x", None),
                si("ou=p,dc=x", None),
                si("uid=a,ou=p,dc=x", Some("A")),
            ],
        );
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st = UiState::new_for_test(old, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.current_branch = Some("ou=p,dc=x".into());
        st.current_leaf = Some("uid=a,ou=p,dc=x".into());
        st.search = "zzz".into();
        st.leaf_search_rows = Some(vec!["uid=a,ou=p,dc=x".to_string()]);
        st.lookup_cache.insert(
            crate::workflows::resolve_flow::member_key("uid=a,ou=p,dc=x"),
            Some("A".into()),
        );

        let fresh = Structure::build(
            "dc=x",
            vec![
                si("dc=x", None),
                si("ou=p,dc=x", None),
                si("uid=a,ou=p,dc=x", Some("A")),
            ],
        );
        st.adopt_structure(fresh);

        assert_eq!(st.current_branch.as_deref(), Some("ou=p,dc=x"));
        assert_eq!(st.current_leaf.as_deref(), Some("uid=a,ou=p,dc=x"));
        assert!(st.search.is_empty());
        assert!(st.leaf_search_rows.is_none());
        assert!(st.lookup_cache.is_empty());
        assert!(st.list_dirty);
        assert!(st.tree_dirty);
        assert_eq!(
            st.leaf_highlight_plan(),
            HighlightPlan::Pin("uid=a,ou=p,dc=x".to_string()),
            "the rebuild re-focuses row 0; the plan must pull the highlight back \
             onto the entry on screen instead of letting row 0 be reported as a \
             fresh selection"
        );
        assert_eq!(
            st.current_leaf_row(),
            Some(1),
            "row 0 is the ‹self› row of ou=p,dc=x; uid=a is the only leaf, at row 1"
        );
    }

    /// Regression for the reload race: a find issued before Alt+R still in flight
    /// when the blocking reload completes must be ignored when its response lands,
    /// so a straggling failure/truncation message can't clobber the "Reloaded N
    /// entries." confirmation the operator just saw.
    #[test]
    fn adopt_structure_cancels_in_flight_search_so_its_response_is_ignored() {
        let structure = Structure::build("dc=x", vec![si("dc=x", None), si("ou=a,dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.current_branch = Some("ou=a,dc=x".into());
        let (worker, rx) = WorkerHandle::recording();
        st.worker = Some(worker);

        // A find is issued and still in flight when the reload happens.
        st.set_leaf_search("ann".into());
        let (req, _) = rx.try_recv().expect("the search was submitted");
        let id = match req {
            Request::Search { id, .. } => id,
            other => panic!("expected Request::Search, got {other:?}"),
        };

        // Alt+R adopts a freshly scanned structure, then reports success — mirroring
        // what `reload_structure` does after a successful blocking scan.
        let fresh = Structure::build("dc=x", vec![si("dc=x", None), si("ou=a,dc=x", None)]);
        st.adopt_structure(fresh);
        st.status = "Reloaded 3 entries.".to_string();

        // The straggling (now superseded) response finally arrives.
        let resp = Response::SearchError {
            id,
            msg: "boom".to_string(),
        };
        st.pump_responses_for_test(&[resp]);

        assert_eq!(
            st.status, "Reloaded 3 entries.",
            "a straggling find response must not overwrite the reload confirmation"
        );
        assert!(st.leaf_search_rows.is_none());
    }

    #[test]
    fn adopt_structure_falls_back_when_the_branch_is_gone() {
        let old = Structure::build("dc=x", vec![si("dc=x", None), si("ou=p,dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st = UiState::new_for_test(old, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.current_branch = Some("ou=gone,dc=x".into());
        st.current_leaf = Some("uid=ghost,ou=gone,dc=x".into());

        let fresh = Structure::build("dc=x", vec![si("dc=x", None), si("ou=p,dc=x", None)]);
        st.adopt_structure(fresh);

        assert_eq!(
            st.current_branch.as_deref(),
            Some("dc=x"),
            "a vanished container falls back to the base DN"
        );
        assert_eq!(st.current_leaf, None);
        // No current_leaf: the plan falls back to pinning the first real row —
        // here ou=p,dc=x, dc=x's only (childless, so leaf) child.
        assert_eq!(
            st.leaf_highlight_plan(),
            HighlightPlan::Pin("ou=p,dc=x".to_string())
        );
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
            baseline_csn: None,
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
            renamed_from: None,
            quit_after: false,
        });
        assert!(res.changed);
        assert!(!res.quit);
        assert_eq!(st.status, "Saved.");
    }

    /// The write path's re-read must not erase the confirmation it was set with.
    ///
    /// A rename re-reads the NEW dn while `current_leaf` still holds the old one,
    /// so any attempt to distinguish "operator navigated away" from "the save is
    /// refreshing what it just wrote" by comparing dns silently eats "Saved." on
    /// every rename. Operator-initiated reads clear `status` at their own call
    /// sites instead.
    #[test]
    fn a_rename_keeps_its_saved_confirmation() {
        let mut st = empty_state();
        st.current_leaf = Some("cn=old,dc=x".into());
        st.apply_write_outcome(WriteOutcome::Saved {
            reread_dn: "cn=new,dc=x".into(),
            renamed_from: Some("cn=old,dc=x".into()),
            quit_after: false,
        });
        assert_eq!(st.status, "Saved.");
    }

    /// Follow-up #2: a status message must not outlive the action it describes.
    /// Guard "Stay" is an operator action — they explicitly chose to keep
    /// editing — so a "Saved." left over from a previous action must go. This
    /// tests a real CALL SITE, not the helper: a test that only asserted
    /// `begin_operator_action()` empties the string would be a tautology over a
    /// one-line function, and the whole point of the task is that the call sites
    /// actually call it.
    #[test]
    fn guard_stay_clears_a_stale_status() {
        let mut st = st_with_rows(&["cn=a,ou=p,dc=x"]);
        st.current_leaf = Some("cn=a,ou=p,dc=x".to_string());
        st.status = "Saved.".to_string();
        crate::ui::app::apply_cancelled_guard_save(&mut st);
        assert!(
            st.status.is_empty(),
            "guard Stay must not leave a message describing the previous action"
        );
    }

    /// Renaming a CONTAINER triggers a full rescan, and the rescan sets its own
    /// "Reloaded N entries." — which would silently replace the confirmation for
    /// the action the operator actually took. The rescan is an implementation
    /// detail of the rename; the save is the news.
    #[test]
    fn a_container_rename_reports_the_save_not_the_rescan() {
        // `ou=old` is a BRANCH (it has a child), so the rename takes the rescan path.
        let structure = Structure::build(
            "dc=x",
            vec![
                si("dc=x", None),
                si("ou=old,dc=x", None),
                si("cn=kid,ou=old,dc=x", Some("Kid")),
            ],
        );
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.current_leaf = Some("ou=old,dc=x".into());
        st.current_branch = Some("ou=old,dc=x".into());
        let (worker, rx) = WorkerHandle::recording();
        st.worker = Some(worker);

        // The rescan blocks on `worker.request`, so answer it from another thread.
        let responder = std::thread::spawn(move || {
            let (req, reply) = rx.recv().expect("a LoadStructure request must be sent");
            let Request::LoadStructure { id, .. } = req else {
                panic!("expected Request::LoadStructure, got {req:?}");
            };
            let _ = reply.send(Response::StructureEntries {
                id,
                nodes: vec![crate::ldap::worker::StructureNodeRaw {
                    dn: "dc=x".into(),
                    cn: None,
                    description: None,
                    object_classes: vec![],
                    attrs: BTreeMap::new(),
                }],
            });
        });

        st.apply_write_outcome(WriteOutcome::Saved {
            reread_dn: "ou=new,dc=x".into(),
            renamed_from: Some("ou=old,dc=x".into()),
            quit_after: false,
        });
        responder.join().expect("responder thread must not panic");

        assert_eq!(
            st.status, "Saved.",
            "the rescan's own message must not bury the save confirmation"
        );
    }

    /// The mirror image: opening another entry DOES drop the stale message.
    #[test]
    fn navigating_to_another_entry_clears_a_stale_status() {
        let mut st = empty_state();
        st.status = "Saved.".to_string();
        st.reread_public("cn=b,dc=x", &[]);
        assert_eq!(st.status, "");
    }

    #[test]
    fn saved_with_quit_sets_quit_flag() {
        let mut st = empty_state();
        let res = st.apply_write_outcome(WriteOutcome::Saved {
            reread_dn: "x".into(),
            renamed_from: None,
            quit_after: true,
        });
        assert!(res.quit);
    }

    /// Minor A regression: a save-and-quit on a renamed CONTAINER must not pay for
    /// the rescan — there is no pane left to show its result to. The early return
    /// must land BEFORE `reload_structure` is even called.
    #[test]
    fn saved_container_rename_with_quit_skips_the_rescan() {
        let structure = Structure::build("dc=x", vec![si("dc=x", None), si("ou=old,dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.current_branch = Some("ou=old,dc=x".into());
        let (worker, rx) = WorkerHandle::recording();
        st.worker = Some(worker);

        let res = st.apply_write_outcome(WriteOutcome::Saved {
            reread_dn: "ou=new,dc=x".into(),
            renamed_from: Some("ou=old,dc=x".into()),
            quit_after: true,
        });

        assert!(res.quit);
        assert_eq!(
            st.status, "Saved.",
            "the quit path must still report the save"
        );
        assert!(
            rx.try_recv().is_err(),
            "no LoadStructure request must be sent on a save-and-quit"
        );
    }

    /// Minor B regression: when the post-rename rescan itself fails, the operator
    /// must not be told the SAVE failed (that would misreport a write that
    /// actually succeeded) — the two outcomes are combined into one message.
    #[test]
    fn saved_container_rename_reports_a_failed_rescan_without_hiding_the_save() {
        let structure = Structure::build(
            "dc=x",
            vec![
                si("dc=x", None),
                si("ou=old,dc=x", None),
                si("uid=child,ou=old,dc=x", Some("Child")),
            ],
        );
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        st.current_branch = Some("ou=old,dc=x".into());
        let (worker, rx) = WorkerHandle::recording();
        st.worker = Some(worker);

        let responder = std::thread::spawn(move || {
            let (req, reply) = rx.recv().expect("a LoadStructure request must be sent");
            let Request::LoadStructure { id, .. } = req else {
                panic!("expected Request::LoadStructure, got {req:?}");
            };
            let _ = reply.send(Response::StructureError {
                id,
                msg: "Size limit exceeded".into(),
                truncated: true,
            });
        });

        st.apply_write_outcome(WriteOutcome::Saved {
            reread_dn: "ou=new,dc=x".into(),
            renamed_from: Some("ou=old,dc=x".into()),
            quit_after: false,
        });
        responder.join().expect("responder thread must not panic");

        assert_eq!(
            st.status, "Saved, but the rescan failed: Size limit exceeded",
            "the save's own success must not be erased by an unrelated rescan failure"
        );
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
            baseline_csn: None,
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

    // --- Task 7: concurrent-modification (rebase-or-prompt) ---

    #[test]
    fn conflict_overlap_detection() {
        // attrs we are writing vs attrs changed by the other client.
        let ours = ["description", "telephoneNumber"];
        let theirs_disjoint = ["mail"];
        let theirs_overlap = ["telephoneNumber"];
        assert!(!crate::ui::state::attrs_overlap(&ours, &theirs_disjoint));
        assert!(crate::ui::state::attrs_overlap(&ours, &theirs_overlap));
        // Case-insensitive: LDAP attribute names are not case-sensitive.
        assert!(crate::ui::state::attrs_overlap(
            &["Description"],
            &["description"]
        ));
    }

    #[test]
    fn attrs_in_flight_lists_only_dirty_field_labels() {
        let mut st = empty_state();
        st.edit_form = Some(form_with_dirty(true)); // single "cn" field, edited
        assert_eq!(st.attrs_in_flight(), vec!["cn".to_string()]);
        st.edit_form = Some(form_with_dirty(false)); // unchanged
        assert!(st.attrs_in_flight().is_empty());
    }

    /// A two-field form: `cn` edited (dirty), `description` untouched.
    fn form_cn_dirty_desc_clean() -> crate::workflows::edit_form::EditForm {
        use crate::schema::FieldKind;
        use crate::workflows::edit_form::{EditField, EditForm, FormMode};
        use crate::workflows::form_model::WidgetSpec;
        let mk = |label: &str, val: &str, base: &str| EditField {
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
            values: vec![val.into()],
            baseline: vec![base.into()],
        };
        EditForm {
            dn: "cn=a,dc=x".into(),
            mode: FormMode::Edit,
            object_classes: vec!["top".into()],
            // cn: edited (base -> newcn); description: unchanged (still "olddesc")
            fields: vec![
                mk("cn", "newcn", "base"),
                mk("description", "olddesc", "olddesc"),
            ],
            baseline_csn: Some("csn-1".into()),
        }
    }

    #[test]
    fn conflict_overlap_stashes_prompt_without_resubmit() {
        // The other client changed `cn` — which we are ALSO editing → overlap.
        let mut st = empty_state();
        st.assertion_supported = true;
        st.edit_form = Some(form_cn_dirty_desc_clean());
        let mut fresh: BTreeMap<String, Vec<String>> = BTreeMap::new();
        fresh.insert("cn".into(), vec!["theircn".into()]); // they changed cn
        fresh.insert("description".into(), vec!["olddesc".into()]); // unchanged
        fresh.insert("entryCSN".into(), vec!["csn-2".into()]);
        // changed-since-baseline = [cn]
        let changed = vec!["cn".to_string()];
        let out = st.resolve_conflict(
            "cn=a,dc=x".into(),
            false,
            Some((fresh, changed, Some("csn-2".into()))),
        );
        assert!(out.error, "overlap surfaces via the error/conflict channel");
        let c = st
            .last_conflict
            .take()
            .expect("overlap must stash a prompt");
        assert_eq!(c.dn, "cn=a,dc=x");
        assert_eq!(c.fresh_csn.as_deref(), Some("csn-2"));
        assert!(c.text.contains("cn"), "prompt names the conflicting attr");
    }

    #[test]
    fn conflict_disjoint_rebases_without_prompt() {
        // The other client changed `description` — which we did NOT edit → disjoint.
        let mut st = empty_state();
        st.assertion_supported = true;
        st.edit_form = Some(form_cn_dirty_desc_clean());
        let mut fresh: BTreeMap<String, Vec<String>> = BTreeMap::new();
        fresh.insert("cn".into(), vec!["base".into()]); // unchanged from our baseline
        fresh.insert("description".into(), vec!["theirdesc".into()]); // they changed it
        fresh.insert("entryCSN".into(), vec!["csn-2".into()]);
        let changed = vec!["description".to_string()];
        let out = st.resolve_conflict(
            "cn=a,dc=x".into(),
            false,
            Some((fresh, changed, Some("csn-2".into()))),
        );
        assert!(!out.error, "disjoint rebase does not surface an error");
        assert!(st.last_conflict.is_none(), "disjoint must not prompt");
        let form = st.edit_form.as_ref().unwrap();
        assert_eq!(
            form.baseline_csn.as_deref(),
            Some("csn-2"),
            "disjoint rebase adopts the fresh CSN"
        );
        // Their disjoint change was adopted into the untouched `description` field so
        // a resubmit will not revert it; our `cn` edit is preserved.
        let desc = form
            .fields
            .iter()
            .find(|f| f.label == "description")
            .unwrap();
        assert_eq!(desc.values, vec!["theirdesc".to_string()]);
        assert_eq!(desc.baseline, vec!["theirdesc".to_string()]);
        let cn = form.fields.iter().find(|f| f.label == "cn").unwrap();
        assert_eq!(cn.values, vec!["newcn".to_string()], "our edit survives");
    }

    #[test]
    fn conflict_reread_failure_surfaces_write_error() {
        // worker: None → reread yields None → fall back to a plain write error.
        let mut st = empty_state();
        st.edit_form = Some(form_cn_dirty_desc_clean());
        let out = st.apply_write_outcome(WriteOutcome::Conflict {
            dn: "cn=a,dc=x".into(),
            quit_after: false,
        });
        assert!(out.error);
        assert!(st.last_conflict.is_none());
        assert!(st
            .last_write_error
            .as_deref()
            .unwrap()
            .contains("could not be re-read"));
    }

    /// A successful rescan returns `Ok(count)` with the entry count it adopted,
    /// and the structure/status reflect the reload — same assertions
    /// `reload_structure`'s own doc comment already promises the success path.
    #[test]
    fn reload_structure_ok_returns_entry_count() {
        let structure = Structure::build("dc=x", vec![si("dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        let (worker, rx) = WorkerHandle::recording();
        st.worker = Some(worker);

        // `reload_structure` blocks on `worker.request`, so the reply must be sent
        // from another thread while the main thread is waiting on it.
        let responder = std::thread::spawn(move || {
            let (req, reply) = rx.recv().expect("a LoadStructure request must be sent");
            let Request::LoadStructure { id, .. } = req else {
                panic!("expected Request::LoadStructure, got {req:?}");
            };
            let _ = reply.send(Response::StructureEntries {
                id,
                nodes: vec![crate::ldap::worker::StructureNodeRaw {
                    dn: "dc=x".into(),
                    cn: None,
                    description: None,
                    object_classes: vec![],
                    attrs: BTreeMap::new(),
                }],
            });
        });

        let result = st.reload_structure();
        responder.join().expect("responder thread must not panic");

        assert_eq!(result, Ok(1));
        assert_eq!(st.status, "Reloaded 1 entries.");
        assert!(st.list_dirty);
        assert!(st.tree_dirty);
    }

    /// A `StructureError` response (e.g. a size/time/admin-limit failure) must map
    /// to `Err` carrying the same human-readable message written to `status` — the
    /// caller (dispatch's RELOAD arm) needs that message to pop the error modal.
    #[test]
    fn reload_structure_structure_error_returns_err_with_message() {
        let structure = Structure::build("dc=x", vec![si("dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        let (worker, rx) = WorkerHandle::recording();
        st.worker = Some(worker);

        let responder = std::thread::spawn(move || {
            let (req, reply) = rx.recv().expect("a LoadStructure request must be sent");
            let Request::LoadStructure { id, .. } = req else {
                panic!("expected Request::LoadStructure, got {req:?}");
            };
            let _ = reply.send(Response::StructureError {
                id,
                msg: "Size limit exceeded".into(),
                truncated: true,
            });
        });

        let result = st.reload_structure();
        responder.join().expect("responder thread must not panic");

        assert_eq!(result, Err("Size limit exceeded".to_string()));
        assert!(st.status.contains("Size limit exceeded"));
    }

    /// With no worker attached (e.g. offline/read-only mode) there is nothing to
    /// reload, so it is treated as a no-op success rather than a failed action.
    #[test]
    fn reload_structure_without_worker_is_a_no_op_ok() {
        let structure = Structure::build("dc=x", vec![si("dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        assert!(st.worker.is_none());
        assert_eq!(st.reload_structure(), Ok(0));
    }

    /// A dropped reply channel (worker thread gone) surfaces as `Err`, not a panic
    /// or a silently-kept-stale structure.
    #[test]
    fn reload_structure_dropped_worker_returns_err() {
        let structure = Structure::build("dc=x", vec![si("dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        let (worker, rx) = WorkerHandle::recording();
        st.worker = Some(worker);

        // Receive the request and drop its reply sender without answering —
        // simulates the worker thread dying mid-request.
        let responder = std::thread::spawn(move || {
            let (_req, reply) = rx.recv().expect("a LoadStructure request must be sent");
            drop(reply);
        });

        let result = st.reload_structure();
        responder.join().expect("responder thread must not panic");

        assert!(result.is_err());
        assert!(st.status.starts_with("Reload failed:"));
    }

    /// An unexpected response variant (a worker/routing bug, not a real failure
    /// mode) still maps to `Err` rather than being silently swallowed.
    #[test]
    fn reload_structure_unexpected_response_returns_err() {
        let structure = Structure::build("dc=x", vec![si("dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st =
            UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        let (worker, rx) = WorkerHandle::recording();
        st.worker = Some(worker);

        let responder = std::thread::spawn(move || {
            let (_req, reply) = rx.recv().expect("a LoadStructure request must be sent");
            let _ = reply.send(Response::Done);
        });

        let result = st.reload_structure();
        responder.join().expect("responder thread must not panic");

        assert!(result.is_err());
        assert!(st.status.contains("unexpected"));
    }

    /// A `UiState` whose current branch is `ou=p,dc=x` and whose structure holds
    /// `dns` as its children, so `leaf_rows()` returns them in order.
    fn st_with_rows(dns: &[&str]) -> UiState {
        let mut inputs = vec![si("dc=x", None), si("ou=p,dc=x", None)];
        inputs.extend(dns.iter().map(|d| si(d, None)));
        let structure = Structure::build("dc=x", inputs);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let mut st = UiState::new_for_test(
            structure,
            schema,
            "dc=x".into(),
            Vec::new(),
            crate::config::tree_label::compile_tree_rules(&crate::config::TreeConfig::default()),
        );
        st.current_branch = Some("ou=p,dc=x".to_string());
        st
    }

    /// The truth table from the design. `Pin` moves the highlight only; `Follow`
    /// additionally lets the form follow. A dirty form is never followed, so a
    /// find-driven rebuild cannot raise the guard.
    #[test]
    fn highlight_plan_pins_the_open_entry_when_it_is_still_listed() {
        let mut st = st_with_rows(&["cn=a,ou=p,dc=x", "cn=b,ou=p,dc=x"]);
        st.current_leaf = Some("cn=b,ou=p,dc=x".to_string());
        assert_eq!(
            st.leaf_highlight_plan(),
            HighlightPlan::Pin("cn=b,ou=p,dc=x".to_string())
        );
    }

    #[test]
    fn highlight_plan_follows_the_first_row_when_the_open_entry_is_absent_and_clean() {
        let mut st = st_with_rows(&["cn=a,ou=p,dc=x", "cn=b,ou=p,dc=x"]);
        st.current_leaf = Some("cn=gone,ou=p,dc=x".to_string());
        // No edit_form at all == clean.
        assert_eq!(
            st.leaf_highlight_plan(),
            HighlightPlan::Follow("cn=a,ou=p,dc=x".to_string())
        );
    }

    /// The modal-mid-keystroke bug: a find that excludes the open entry must move
    /// the highlight but NOT the form, so `reconcile_selection` is never reached
    /// and the dirty guard never fires.
    #[test]
    fn highlight_plan_only_pins_when_the_form_is_dirty() {
        let mut st = st_with_rows(&["cn=a,ou=p,dc=x", "cn=b,ou=p,dc=x"]);
        st.current_leaf = Some("cn=gone,ou=p,dc=x".to_string());
        st.edit_form = Some(crate::ui::test_support::dirty_form("cn=gone,ou=p,dc=x"));
        assert_eq!(
            st.leaf_highlight_plan(),
            HighlightPlan::Pin("cn=a,ou=p,dc=x".to_string())
        );
    }

    #[test]
    fn highlight_plan_pins_the_first_row_when_no_entry_is_open() {
        let st = st_with_rows(&["cn=a,ou=p,dc=x"]);
        assert_eq!(
            st.leaf_highlight_plan(),
            HighlightPlan::Pin("cn=a,ou=p,dc=x".to_string())
        );
    }

    #[test]
    fn highlight_plan_clears_when_there_are_no_rows() {
        let mut st = st_with_rows(&[]);
        st.current_leaf = Some("cn=gone,ou=p,dc=x".to_string());
        assert_eq!(st.leaf_highlight_plan(), HighlightPlan::Clear);
    }

    /// The `‹self›` row IS a row: an operator with the container's own entry
    /// open (`current_leaf == current_branch`) must still be pinned to it, even
    /// when it is the only row in `leaf_rows()`. The "open entry is in the
    /// rows" check must run before the fallback-first-row (which skips `‹self›`)
    /// ever gets a chance to declare there is nothing to highlight.
    #[test]
    fn highlight_plan_pins_the_self_row_when_the_container_is_open_and_childless() {
        let mut st = st_with_rows(&[]);
        st.current_leaf = Some("ou=p,dc=x".to_string());
        assert_eq!(
            st.leaf_highlight_plan(),
            HighlightPlan::Pin("ou=p,dc=x".to_string())
        );
    }

    /// Companion to the above: a childless container with nothing open at all
    /// still clears (no non-`‹self›` row to fall back to, and no open entry to
    /// pin).
    #[test]
    fn highlight_plan_clears_for_a_childless_container_with_nothing_open() {
        let st = st_with_rows(&[]);
        assert_eq!(st.leaf_highlight_plan(), HighlightPlan::Clear);
    }

    /// The tree shares the enum but must never navigate the form on its own:
    /// a branch change is always operator-driven or an explicit `commit_branch`.
    #[test]
    fn branch_highlight_plan_pins_the_current_branch() {
        let mut st = st_with_rows(&[]);
        st.branch_dns = vec!["dc=x".to_string(), "ou=p,dc=x".to_string()];
        st.current_branch = Some("ou=p,dc=x".to_string());
        assert_eq!(
            st.branch_highlight_plan(),
            HighlightPlan::Pin("ou=p,dc=x".to_string())
        );
    }

    #[test]
    fn branch_highlight_plan_clears_when_the_branch_vanished() {
        let mut st = st_with_rows(&[]);
        st.branch_dns = vec!["dc=x".to_string()];
        st.current_branch = Some("ou=gone,dc=x".to_string());
        assert_eq!(st.branch_highlight_plan(), HighlightPlan::Clear);
    }

    #[test]
    fn branch_highlight_plan_clears_rather_than_falling_back_to_a_first_row() {
        let mut st = st_with_rows(&[]);
        st.branch_dns = vec!["dc=x".to_string(), "ou=p,dc=x".to_string()];
        st.current_branch = Some("ou=gone,dc=x".to_string());
        // Assert the FULL value, not merely "not Follow": the likely wrong
        // implementation is a copy-paste of the leaf policy, which falls back to
        // the first row and would return Pin("dc=x") — and a !matches!(Follow)
        // check would wave that straight through. This is the case where the two
        // policies genuinely diverge, so it is the one worth pinning down.
        assert_eq!(
            st.branch_highlight_plan(),
            HighlightPlan::Clear,
            "a tree rebuild must never navigate the form, nor fall back to a row"
        );
    }
}
