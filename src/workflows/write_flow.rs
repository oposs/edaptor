//! Async write flow: validate + diff an [`EditForm`] save (via
//! [`crate::workflows::save::prepare_save`]) and correlate the worker's write
//! responses. `prepare` and `on_response` are pure; `submit`/`submit_followup`
//! are thin worker wrappers. Mirrors `read_flow` but for writes; the two never
//! collide because read and write responses are disjoint `Response` variants.

use std::collections::{BTreeMap, HashMap};

use anyhow::Result;

use crate::config::widget::{password_widget_for, ResolvedWidget};
use crate::form::changeset::{EditEntry, ModOp};
use crate::form::validate::SavePlan;
use crate::ldap::worker::{Request, Response, SearchScope, WorkerHandle};
use crate::schema::SchemaModel;
use crate::workflows::create::now_unix_secs_or_zero;
use crate::workflows::edit_form::EditForm;
use crate::workflows::save::{
    compose_renamed_dn, membership_attr_is_must, plan_combined_save, prepare_save,
    stage_pending_password, CombinedSave, PlanCombined, PrepareSave,
};

/// Blocking, schema-gated populate of the `group_members` map that
/// [`WriteFlow::submit_combined`] consumes. For each fan-out `Delete` op, fetch
/// the group's `objectClass` + membership attr (a single Base-scoped search) and
/// — only when that attr is MUST for the group ([`membership_attr_is_must`]) —
/// record the group's current members. MAY-membership groups (e.g. `posixGroup`
/// `memberUid`) are deliberately omitted so emptying them is allowed. Best-effort:
/// a failed/empty fetch leaves the group out (the server remains the backstop).
pub fn fetch_group_members_for_must(
    worker: &WorkerHandle,
    schema: &SchemaModel,
    fanout: &[(String, ModOp)],
) -> std::collections::HashMap<String, Vec<String>> {
    let mut map = std::collections::HashMap::new();
    for (group_dn, op) in fanout {
        let ModOp::Delete { attr, .. } = op else {
            continue;
        };
        let resp = worker.request(Request::Search {
            id: 0,
            base: group_dn.clone(),
            scope: SearchScope::Base,
            filter: "(objectClass=*)".to_string(),
            attrs: vec!["objectClass".to_string(), attr.clone()],
            size_limit: Some(1),
        });
        let Ok(Response::Entries { entries, .. }) = resp else {
            continue;
        };
        let Some(entry) = entries.first() else {
            continue;
        };
        let ocs: Vec<&str> = entry
            .attrs
            .get("objectClass")
            .map(|v| v.iter().map(String::as_str).collect())
            .unwrap_or_default();
        if membership_attr_is_must(schema, &ocs, attr) {
            let members = entry.attrs.get(attr).cloned().unwrap_or_default();
            map.insert(group_dn.clone(), members);
        }
    }
    map
}

/// Blocking, per-group `entryCSN` pre-read for [`WriteFlow::submit_combined`]'s
/// membership fan-out legs. For every distinct group DN in `fanout`, do a
/// Base-scoped search for `entryCSN` (operational — must be requested
/// explicitly, unlike `fetch_group_members_for_must`'s regular attrs) so that
/// leg's `Request::Modify` can assert it. Best-effort and separate from the
/// MUST-check fetch: a failed/empty fetch just leaves the group out of the map,
/// so that leg's `assert_csn` becomes `None` (a blind write) rather than
/// blocking the batch — the server remains the backstop.
pub fn fetch_group_csns(
    worker: &WorkerHandle,
    fanout: &[(String, ModOp)],
) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for (group_dn, _) in fanout {
        if map.contains_key(group_dn) {
            continue;
        }
        let resp = worker.request(Request::Search {
            id: 0,
            base: group_dn.clone(),
            scope: SearchScope::Base,
            filter: "(objectClass=*)".to_string(),
            attrs: vec!["entryCSN".to_string()],
            size_limit: Some(1),
        });
        let Ok(Response::Entries { entries, .. }) = resp else {
            continue;
        };
        let Some(entry) = entries.first() else {
            continue;
        };
        let csn = entry
            .attrs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("entryCSN"))
            .and_then(|(_, v)| v.first().cloned());
        if let Some(csn) = csn {
            map.insert(group_dn.clone(), csn);
        }
    }
    map
}

/// What a pending write means once its `WriteOk` arrives.
#[derive(Debug, Clone)]
enum WriteIntent {
    /// A plain save (or a rename's final leg): re-read `reread_dn` afterwards.
    Save { reread_dn: String, quit_after: bool },
    /// A rename's first leg: on success, submit `mods` against `new_dn`.
    RenameThenModify {
        new_dn: String,
        mods: Vec<ModOp>,
        quit_after: bool,
    },
    /// An ADD (create new entry): on success, yield [`WriteOutcome::Created`].
    Create { dn: String, quit_after: bool },
    /// One leg of a combined membership save (own MODIFY + one MODIFY per touched
    /// group). All legs of one save share a `batch_id`; the batch completes — and a
    /// terminal [`WriteOutcome::CombinedSaved`] is yielded — only when the LAST
    /// outstanding leg's `WriteOk` arrives. See [`WriteFlow::submit_combined`].
    CombinedLeg {
        batch_id: u64,
        reread_dn: String,
        quit_after: bool,
    },
    /// Sequential fallback (no server txn): the companion ADD was submitted; on its
    /// success submit the primary ADD carried here. See [`WriteFlow::submit_create_with_companion`].
    CompanionThenPrimary {
        primary_dn: String,
        primary_attrs: BTreeMap<String, Vec<String>>,
        quit_after: bool,
    },
    /// Sequential fallback: the primary ADD, submitted after the companion succeeded.
    /// On success → [`WriteOutcome::Created`]; on failure → an error naming the orphaned
    /// `companion_dn` that was already created.
    PrimaryAfterCompanion {
        primary_dn: String,
        companion_dn: String,
        quit_after: bool,
    },
}

/// The app-facing result of correlating one write response.
#[derive(Debug, Clone)]
pub enum WriteOutcome {
    /// Not one of our pending writes.
    Ignored,
    /// A write completed; re-read `reread_dn` (unless quitting).
    Saved { reread_dn: String, quit_after: bool },
    /// A rename's MODRDN landed; caller must submit the deferred `mods` via
    /// [`WriteFlow::submit_followup`].
    NeedFollowupModify {
        dn: String,
        mods: Vec<ModOp>,
        quit_after: bool,
    },
    /// A write failed; `msg` is already human-mapped by the worker.
    Error(String),
    /// A new entry was successfully created (ADD).
    Created { dn: String, quit_after: bool },
    /// A combined membership save completed: every leg (own MODIFY + each touched
    /// group's MODIFY) landed successfully. Re-read `reread_dn` (the user entry),
    /// exactly like [`WriteOutcome::Saved`].
    CombinedSaved { reread_dn: String, quit_after: bool },
    /// One non-final leg of a combined membership save landed; the batch is not yet
    /// complete. Non-terminal — no user-visible effect until [`CombinedSaved`].
    BatchProgress { remaining: usize },
    /// The companion ADD landed; the caller must now submit the primary ADD via
    /// [`WriteFlow::submit_followup_create`]. Sequential fallback only. `companion_dn`
    /// is the just-created companion, carried so a later primary failure can name the
    /// orphan.
    NeedFollowupCreate {
        dn: String,
        attrs: BTreeMap<String, Vec<String>>,
        companion_dn: String,
        quit_after: bool,
    },
    /// A MODIFY/DELETE was refused because the entry changed since it was read
    /// (rc 122). The caller must re-read `dn` and decide rebase-vs-prompt.
    Conflict { dn: String, quit_after: bool },
}

/// The masked sentinel set in a password field by `CommitOutcome::StageSecret`.
/// Defined here (neutral module, no UI imports) so the create and edit submit
/// paths can both reference the same byte sequence. Must never reach the server.
pub const STAGED_PASSWORD_SENTINEL: &str = "••••••"; // 6 × U+2022 BULLET

/// Tracks in-flight writes and turns the edit form into a save plan.
pub struct WriteFlow {
    next_id: u64,
    pending: HashMap<u64, WriteIntent>,
    /// Outstanding-leg count per combined-save batch, keyed by `batch_id` (the
    /// first leg id allocated for that save). Decremented as each leg's `WriteOk`
    /// lands; removed when it reaches zero (→ `CombinedSaved`) or on any leg error.
    batches: HashMap<u64, usize>,
}

impl Default for WriteFlow {
    fn default() -> Self {
        Self::new()
    }
}

impl WriteFlow {
    pub fn new() -> Self {
        // Start above ReadFlow's range as defence in depth; correctness does not
        // rely on it (read/write response variants are disjoint).
        WriteFlow {
            next_id: 1_000_000,
            pending: HashMap::new(),
            batches: HashMap::new(),
        }
    }

    fn alloc(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Validate + diff `form` into a [`PrepareSave`], folding in any staged
    /// `pending_password` (cleartext from the password editor) via `resolved_widgets`.
    /// Pass `None`/`&[]` when no password is staged. Uses the wall clock for the
    /// Samba `sambaPwdLastSet` timestamp; the test path passes `None` to skip it.
    pub fn prepare(
        &self,
        form: &EditForm,
        schema: &SchemaModel,
        pending_password: Option<&str>,
        resolved_widgets: &[ResolvedWidget],
    ) -> PrepareSave {
        // Uniform field handling: objectClass is a regular form field — no special-casing.
        let mut original = EditEntry {
            dn: form.dn.clone(),
            attrs: form
                .fields
                .iter()
                .map(|f| (f.label.clone(), f.baseline.clone()))
                .collect(),
        };
        let mut edited = form.to_edit_entry();
        let secret_attrs: Vec<String> = form
            .fields
            .iter()
            .filter(|f| f.secret)
            .map(|f| f.label.clone())
            .collect();
        let orphaned: Vec<&str> = form
            .fields
            .iter()
            .filter(|f| f.orphaned)
            .map(|f| f.label.as_str())
            .collect();
        let x_ordered: std::collections::HashSet<String> = form
            .fields
            .iter()
            .filter(|f| f.ordered)
            .map(|f| f.label.clone())
            .collect();
        // Fold pending password into password_mods; strip primary+derived from both
        // sides so the plain diff never double-writes or emits a spurious Delete.
        let (password_mods, mask_attrs) =
            match password_widget_for(resolved_widgets, &form.object_classes) {
                Some(pw) => stage_pending_password(
                    pending_password,
                    &pw.primary,
                    &pw.derived,
                    pw.samba,
                    now_unix_secs_or_zero(),
                    &mut original.attrs,
                    &mut edited.attrs,
                ),
                None => {
                    // No matching widget: strip the staged sentinel from secret fields so
                    // the diff cannot emit ••••••. Only removes attrs whose edited value is
                    // exactly the sentinel; leaves all other attrs intact.
                    for f in form.fields.iter().filter(|f| f.secret) {
                        if f.values.iter().any(|v| v == STAGED_PASSWORD_SENTINEL) {
                            original.attrs.remove(&f.label);
                            edited.attrs.remove(&f.label);
                        }
                    }
                    (Vec::new(), Vec::new())
                }
            };
        prepare_save(
            schema,
            &original,
            &edited,
            &form.object_classes,
            &password_mods,
            &mask_attrs,
            &secret_attrs,
            &orphaned,
            &x_ordered,
        )
    }

    /// Plan a combined membership save: the own-entry diff plus the per-holder
    /// fan-out MODIFYs, for a form whose fan-out (membership) field changed.
    /// Mirrors [`Self::prepare`] but routes through
    /// [`plan_combined_save`](crate::workflows::save::plan_combined_save).
    ///
    /// **Caller contract (honoured here):** the password primary and every derived
    /// attr are placed in `mask_attrs` UNCONDITIONALLY — even when no password is
    /// staged — so the stored baseline hash is stripped from BOTH sides and can
    /// never diff into a spurious `Delete userPassword`. `password_mods` is
    /// non-empty only when a cleartext is actually staged. See the caller-contract
    /// section on `plan_combined_save`.
    pub fn prepare_combined(
        &self,
        form: &EditForm,
        schema: &SchemaModel,
        pending_password: Option<&str>,
        resolved_widgets: &[ResolvedWidget],
    ) -> PlanCombined {
        let secret_attrs: Vec<String> = form
            .fields
            .iter()
            .filter(|f| f.secret)
            .map(|f| f.label.clone())
            .collect();
        let orphaned: Vec<&str> = form
            .fields
            .iter()
            .filter(|f| f.orphaned)
            .map(|f| f.label.as_str())
            .collect();
        let x_ordered: std::collections::HashSet<String> = form
            .fields
            .iter()
            .filter(|f| f.ordered)
            .map(|f| f.label.clone())
            .collect();
        // Caller contract: ALWAYS mask (and thus strip) the password primary +
        // derived attrs, even with no staged change — otherwise a stored baseline
        // hash diffs to a spurious Delete. `password_mods` is populated only when a
        // cleartext is staged (mirrors `stage_pending_password`'s REPLACE set).
        let (password_mods, mask_attrs) =
            match password_widget_for(resolved_widgets, &form.object_classes) {
                Some(pw) => {
                    let mut mask = vec![pw.primary.clone()];
                    mask.extend(pw.derived.iter().cloned());
                    let mods = match pending_password {
                        Some(cleartext) => crate::samba::password::password_add_attrs(
                            cleartext,
                            &pw.primary,
                            pw.samba,
                            now_unix_secs_or_zero(),
                        )
                        .into_iter()
                        .map(|(attr, values)| ModOp::Replace { attr, values })
                        .collect(),
                        None => Vec::new(),
                    };
                    (mods, mask)
                }
                None => (Vec::new(), Vec::new()),
            };
        plan_combined_save(
            schema,
            form,
            &password_mods,
            &mask_attrs,
            &secret_attrs,
            &orphaned,
            &x_ordered,
        )
    }

    /// Submit a [`SavePlan`] to the worker, tracking what each id means.
    pub fn submit(
        &mut self,
        worker: &WorkerHandle,
        plan: SavePlan,
        old_dn: &str,
        assert_csn: Option<String>,
        quit_after: bool,
    ) -> Result<()> {
        match plan {
            SavePlan::Nothing => {}
            SavePlan::Modify(mods) => {
                let id = self.alloc();
                worker.submit(Request::Modify {
                    id,
                    dn: old_dn.to_string(),
                    changes: mods,
                    assert_csn,
                })?;
                self.pending.insert(
                    id,
                    WriteIntent::Save {
                        reread_dn: old_dn.to_string(),
                        quit_after,
                    },
                );
            }
            SavePlan::RenameOnly(modrdn) => {
                let id = self.alloc();
                let new_dn = compose_renamed_dn(old_dn, &modrdn.new_rdn);
                worker.submit(Request::ModRdn {
                    id,
                    dn: old_dn.to_string(),
                    new_rdn: modrdn.new_rdn,
                    delete_old: modrdn.delete_old,
                    new_superior: modrdn.new_superior,
                })?;
                self.pending.insert(
                    id,
                    WriteIntent::Save {
                        reread_dn: new_dn,
                        quit_after,
                    },
                );
            }
            SavePlan::Rename { modrdn, then_mods } => {
                let id = self.alloc();
                let new_dn = compose_renamed_dn(old_dn, &modrdn.new_rdn);
                worker.submit(Request::ModRdn {
                    id,
                    dn: old_dn.to_string(),
                    new_rdn: modrdn.new_rdn,
                    delete_old: modrdn.delete_old,
                    new_superior: modrdn.new_superior,
                })?;
                self.pending.insert(
                    id,
                    WriteIntent::RenameThenModify {
                        new_dn,
                        mods: then_mods,
                        quit_after,
                    },
                );
            }
        }
        Ok(())
    }

    /// Submit a new entry (ADD). On [`Response::WriteOk`], [`on_response`]
    /// yields [`WriteOutcome::Created`].
    pub fn submit_create(
        &mut self,
        worker: &WorkerHandle,
        dn: &str,
        attrs: std::collections::BTreeMap<String, Vec<String>>,
        quit_after: bool,
    ) -> Result<()> {
        let id = self.alloc();
        worker.submit(Request::Add {
            id,
            dn: dn.to_string(),
            attrs,
        })?;
        self.pending.insert(
            id,
            WriteIntent::Create {
                dn: dn.to_string(),
                quit_after,
            },
        );
        Ok(())
    }

    /// Atomic path: create `entries` (companion first, primary last) in one RFC 5805
    /// transaction. `reread_dn` is the primary DN to re-read after success. One
    /// `WriteOk` → [`WriteOutcome::Created`]; one `WriteError` → [`WriteOutcome::Error`]
    /// (nothing written).
    pub fn submit_create_atomic(
        &mut self,
        worker: &WorkerHandle,
        entries: Vec<(String, BTreeMap<String, Vec<String>>)>,
        reread_dn: &str,
        quit_after: bool,
    ) -> Result<()> {
        let id = self.alloc();
        worker.submit(Request::AddAtomic { id, entries })?;
        self.pending.insert(
            id,
            WriteIntent::Create {
                dn: reread_dn.to_string(),
                quit_after,
            },
        );
        Ok(())
    }

    /// Sequential fallback: submit the companion ADD first, carrying the primary ADD to
    /// submit on the companion's success (via [`WriteOutcome::NeedFollowupCreate`] →
    /// [`submit_followup_create`](Self::submit_followup_create)).
    pub fn submit_create_with_companion(
        &mut self,
        worker: &WorkerHandle,
        companion_dn: &str,
        companion_attrs: BTreeMap<String, Vec<String>>,
        primary_dn: &str,
        primary_attrs: BTreeMap<String, Vec<String>>,
        quit_after: bool,
    ) -> Result<()> {
        let id = self.alloc();
        worker.submit(Request::Add {
            id,
            dn: companion_dn.to_string(),
            attrs: companion_attrs,
        })?;
        self.pending.insert(
            id,
            WriteIntent::CompanionThenPrimary {
                primary_dn: primary_dn.to_string(),
                primary_attrs,
                quit_after,
            },
        );
        Ok(())
    }

    /// Sequential fallback second phase: submit the primary ADD after the companion
    /// (`companion_dn`) landed. Tracked as [`WriteIntent::PrimaryAfterCompanion`], so its
    /// success → [`WriteOutcome::Created`] and its failure → an error naming the orphaned
    /// companion.
    pub fn submit_followup_create(
        &mut self,
        worker: &WorkerHandle,
        dn: &str,
        attrs: BTreeMap<String, Vec<String>>,
        companion_dn: &str,
        quit_after: bool,
    ) -> Result<()> {
        let id = self.alloc();
        worker.submit(Request::Add {
            id,
            dn: dn.to_string(),
            attrs,
        })?;
        self.pending.insert(
            id,
            WriteIntent::PrimaryAfterCompanion {
                primary_dn: dn.to_string(),
                companion_dn: companion_dn.to_string(),
                quit_after,
            },
        );
        Ok(())
    }

    /// Submit a combined membership save: the own-entry MODIFY (when `own_mods` is
    /// non-empty) plus one MODIFY per touched group. All legs are tracked under one
    /// batch so [`on_response`](Self::on_response) can report
    /// [`WriteOutcome::CombinedSaved`] only once every leg's `WriteOk` has landed.
    ///
    /// **Last-member pre-validation (before any submit):** for every group from
    /// which the user is being removed (`ModOp::Delete`), check
    /// [`would_empty`](crate::workflows::save::would_empty) against that group's
    /// current member DNs in `group_members`. If any removal would leave a
    /// `groupOfNames` empty, this returns `Err(msg)` (naming the group) having
    /// submitted **nothing** — the check is purely on the passed-in data, so a
    /// partial batch can never be submitted and then found invalid. Adds never
    /// trigger this. A group missing from `group_members` is treated as having no
    /// known members (conservative: `would_empty` returns false on an empty slice,
    /// so we do not block on data we were not given).
    ///
    /// `batch_id` is the first leg's allocated id — deterministic, no clock/random.
    ///
    /// **Group CSN assertion:** each per-group `Request::Modify` asserts
    /// `group_csns.get(&group_dn)` (from [`fetch_group_csns`]) so a concurrent
    /// membership change on that group surfaces as a [`WriteOutcome::Conflict`]
    /// leg instead of a raw LDAP error. A group
    /// missing from the map submits blind (`assert_csn: None`).
    ///
    /// **Own-entry CSN assertion:** the own-entry leg (when `own_mods` is
    /// non-empty) asserts `own_assert_csn` — the caller's job to populate with
    /// the form's `baseline_csn` (mirroring the plain-save path), or `None` for
    /// a blind write. This function does not itself decide whether assertion is
    /// supported by the server; both `own_assert_csn` and `group_csns` are
    /// expected to already be gated (e.g. on `assertion_supported`) by the
    /// caller — see `do_combined_save` in `src/ui/app.rs`.
    #[allow(clippy::too_many_arguments)]
    pub fn submit_combined(
        &mut self,
        worker: &WorkerHandle,
        combined: CombinedSave,
        group_members: &std::collections::HashMap<String, Vec<String>>,
        group_csns: &std::collections::HashMap<String, String>,
        own_assert_csn: Option<String>,
        reread_dn: &str,
        quit_after: bool,
    ) -> std::result::Result<(), String> {
        let CombinedSave {
            own_dn,
            own_mods,
            fanout,
            ..
        } = combined;

        // Last-member pre-validation (schema-gated by the caller's populate of
        // `group_members`): refuse before submitting anything.
        if let Some(msg) =
            crate::workflows::save::last_member_block(&fanout, group_members, &own_dn)
        {
            return Err(msg);
        }

        // 2. Assemble the legs: own entry first (if any own changes), then groups.
        //    The own-entry leg's assert_csn is the caller-supplied
        //    `own_assert_csn`; each group leg's comes from `group_csns`.
        let mut legs: Vec<(String, Vec<ModOp>, Option<String>)> = Vec::new();
        if !own_mods.is_empty() {
            legs.push((own_dn, own_mods, own_assert_csn));
        }
        for (group_dn, op) in fanout {
            let assert_csn = group_csns.get(&group_dn).cloned();
            legs.push((group_dn, vec![op], assert_csn));
        }
        // Nothing to do (no own changes, no membership changes): a no-op success.
        if legs.is_empty() {
            return Ok(());
        }

        // 3. Allocate ids; the first leg id is the deterministic batch id. Register
        //    the batch BEFORE submitting so a response can never underflow the count.
        let count = legs.len();
        let leg_ids: Vec<(u64, String, Vec<ModOp>, Option<String>)> = legs
            .into_iter()
            .map(|(dn, changes, assert_csn)| (self.alloc(), dn, changes, assert_csn))
            .collect();
        let batch_id = leg_ids[0].0;
        self.batches.insert(batch_id, count);

        // 4. Submit every leg, recording its intent under the shared batch.
        for (id, dn, changes, assert_csn) in leg_ids {
            worker
                .submit(Request::Modify {
                    id,
                    dn,
                    changes,
                    assert_csn,
                })
                .map_err(|e| e.to_string())?;
            self.pending.insert(
                id,
                WriteIntent::CombinedLeg {
                    batch_id,
                    reread_dn: reread_dn.to_string(),
                    quit_after,
                },
            );
        }
        Ok(())
    }

    /// Submit the deferred modifications of a rename's second leg.
    pub fn submit_followup(
        &mut self,
        worker: &WorkerHandle,
        dn: &str,
        mods: Vec<ModOp>,
        quit_after: bool,
    ) -> Result<()> {
        let id = self.alloc();
        worker.submit(Request::Modify {
            id,
            dn: dn.to_string(),
            changes: mods,
            assert_csn: None,
        })?;
        self.pending.insert(
            id,
            WriteIntent::Save {
                reread_dn: dn.to_string(),
                quit_after,
            },
        );
        Ok(())
    }

    /// Correlate one polled [`Response`]. Pure; ignores non-write variants.
    pub fn on_response(&mut self, resp: &Response) -> WriteOutcome {
        match resp {
            Response::WriteOk {
                id,
                dn: resp_dn,
                new_csn: _,
            } => match self.pending.remove(id) {
                Some(WriteIntent::Save {
                    reread_dn,
                    quit_after,
                }) => WriteOutcome::Saved {
                    reread_dn,
                    quit_after,
                },
                Some(WriteIntent::RenameThenModify {
                    new_dn,
                    mods,
                    quit_after,
                }) => WriteOutcome::NeedFollowupModify {
                    dn: new_dn,
                    mods,
                    quit_after,
                },
                Some(WriteIntent::Create { dn, quit_after }) => {
                    WriteOutcome::Created { dn, quit_after }
                }
                Some(WriteIntent::CompanionThenPrimary {
                    primary_dn,
                    primary_attrs,
                    quit_after,
                }) => WriteOutcome::NeedFollowupCreate {
                    dn: primary_dn,
                    attrs: primary_attrs,
                    companion_dn: resp_dn.clone(), // the companion just created
                    quit_after,
                },
                Some(WriteIntent::PrimaryAfterCompanion {
                    primary_dn,
                    quit_after,
                    ..
                }) => WriteOutcome::Created {
                    dn: primary_dn,
                    quit_after,
                },
                Some(WriteIntent::CombinedLeg {
                    batch_id,
                    reread_dn,
                    quit_after,
                }) => match self.batches.get_mut(&batch_id) {
                    Some(remaining) => {
                        *remaining = remaining.saturating_sub(1);
                        if *remaining == 0 {
                            self.batches.remove(&batch_id);
                            WriteOutcome::CombinedSaved {
                                reread_dn,
                                quit_after,
                            }
                        } else {
                            WriteOutcome::BatchProgress {
                                remaining: *remaining,
                            }
                        }
                    }
                    // Batch already resolved (completed earlier, or aborted by a
                    // sibling leg's error): a late/extra WriteOk is not ours to act on.
                    None => WriteOutcome::Ignored,
                },
                None => WriteOutcome::Ignored,
            },
            Response::WriteConflict { id, dn } => match self.pending.remove(id) {
                // A plain single-entry save (or a rename's final leg) conflicted:
                // the working own-entry rebase/prompt path in `src/ui/state.rs`
                // (`resolve_conflict` et al.) re-reads `dn` — which IS the entry the
                // edit form is for — and handles it correctly.
                Some(WriteIntent::Save { quit_after, .. }) => WriteOutcome::Conflict {
                    dn: dn.clone(),
                    quit_after,
                },
                // A combined-save leg conflicted (rc 122): `dn` is THAT LEG's dn,
                // which for a group leg is a GROUP dn, not the user entry the edit
                // form is for. Routing this through the single-entry Conflict path
                // would re-read the group and diff it against the user form —
                // garbling the overlap prompt and possibly adopting the group's CSN
                // into the user form. Instead, mirror the CombinedLeg case in the
                // WriteError arm below: abort the batch (so sibling legs' later
                // responses become Ignored) and surface a reload-and-retry error.
                // Rebasing just the own leg and resubmitting the batch is a
                // deliberately deferred enhancement.
                Some(WriteIntent::CombinedLeg { batch_id, .. }) => {
                    self.batches.remove(&batch_id);
                    WriteOutcome::Error(format!(
                        "Membership change refused: {dn} was changed by another client \
                         during this save. Because the save is non-atomic, other \
                         membership changes in the same save may already have been \
                         applied — reload the entry and review membership before \
                         retrying."
                    ))
                }
                // Any other intent (rename, create, ...) reaching a conflict is
                // unexpected — no own-entry rebase path applies to it. Surface a
                // safe error rather than misrouting into the own-entry Conflict path.
                Some(_) => WriteOutcome::Error(format!(
                    "Write refused: {dn} was changed since it was read. Reload and retry."
                )),
                None => WriteOutcome::Ignored,
            },
            Response::WriteError { id, msg } => match self.pending.remove(id) {
                // A combined leg failed: abort the batch (drop its counter so any
                // sibling WriteOks become Ignored) and surface that the membership
                // change is only PARTIALLY applied — earlier legs may have landed.
                Some(WriteIntent::CombinedLeg { batch_id, .. }) => {
                    self.batches.remove(&batch_id);
                    WriteOutcome::Error(format!(
                        "Membership change only partially applied: one entry failed ({msg}). \
                         Other entries in the same save may already have been modified — \
                         review membership before retrying."
                    ))
                }
                Some(WriteIntent::PrimaryAfterCompanion { companion_dn, .. }) => {
                    WriteOutcome::Error(format!(
                        "The primary entry failed to create ({msg}). Its companion \
                         {companion_dn} was already created — remove it or retry."
                    ))
                }
                Some(_) => WriteOutcome::Error(msg.clone()),
                None => WriteOutcome::Ignored,
            },
            _ => WriteOutcome::Ignored,
        }
    }

    #[cfg(test)]
    pub(crate) fn insert_save_intent_for_test(
        &mut self,
        reread_dn: String,
        quit_after: bool,
    ) -> u64 {
        let id = self.alloc();
        self.pending.insert(
            id,
            WriteIntent::Save {
                reread_dn,
                quit_after,
            },
        );
        id
    }
    #[cfg(test)]
    pub(crate) fn insert_create_intent_for_test(&mut self, id: u64, dn: &str, quit_after: bool) {
        self.pending.insert(
            id,
            WriteIntent::Create {
                dn: dn.to_string(),
                quit_after,
            },
        );
    }
    #[cfg(test)]
    pub(crate) fn insert_companion_intent_for_test(
        &mut self,
        id: u64,
        primary_dn: &str,
        primary_attrs: std::collections::BTreeMap<String, Vec<String>>,
        quit_after: bool,
    ) {
        self.pending.insert(
            id,
            WriteIntent::CompanionThenPrimary {
                primary_dn: primary_dn.to_string(),
                primary_attrs,
                quit_after,
            },
        );
    }
    #[cfg(test)]
    pub(crate) fn insert_primary_after_companion_for_test(
        &mut self,
        id: u64,
        primary_dn: &str,
        companion_dn: &str,
        quit_after: bool,
    ) {
        self.pending.insert(
            id,
            WriteIntent::PrimaryAfterCompanion {
                primary_dn: primary_dn.to_string(),
                companion_dn: companion_dn.to_string(),
                quit_after,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldap::worker::{RawSubschema, Response};
    use crate::schema::{FieldKind, SchemaModel};
    use crate::workflows::edit_form::{EditField, EditForm, FormMode};
    use crate::workflows::form_model::WidgetSpec;
    use std::collections::BTreeMap;

    fn schema() -> SchemaModel {
        SchemaModel::from_raw(&RawSubschema {
            object_classes: vec![
                "( 2.5.6.0 NAME 'top' ABSTRACT MUST objectClass )".to_string(),
                "( 2.5.6.6 NAME 'person' SUP top STRUCTURAL MUST ( sn $ cn ) MAY description )"
                    .to_string(),
            ],
            attribute_types: vec![
                "( 2.5.4.3 NAME 'cn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".to_string(),
                "( 2.5.4.4 NAME 'sn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".to_string(),
            ],
            ldap_syntaxes: vec![],
        })
    }

    fn field(label: &str, val: &str, base: &str) -> EditField {
        EditField {
            label: label.into(),
            must: true,
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
        }
    }

    fn form_with(fields: Vec<EditField>) -> EditForm {
        EditForm {
            dn: "cn=Alice,dc=example,dc=org".into(),
            mode: FormMode::Edit,
            object_classes: vec!["top".into(), "person".into()],
            fields,
            baseline_csn: None,
        }
    }

    /// The MUST objectClass attribute is a real form field (multi-valued,
    /// read-only in M2), exactly as `build_form_model` emits it.
    fn oc_field() -> EditField {
        EditField {
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
            values: vec!["top".into(), "person".into()],
            baseline: vec!["top".into(), "person".into()],
        }
    }

    #[test]
    fn prepare_no_change_is_nochanges() {
        let wf = WriteFlow::new();
        let f = form_with(vec![
            oc_field(),
            field("cn", "Alice", "Alice"),
            field("sn", "Adams", "Adams"),
        ]);
        assert!(matches!(
            wf.prepare(&f, &schema(), None, &[]),
            PrepareSave::NoChanges
        ));
    }

    #[test]
    fn prepare_modify_yields_ready() {
        let wf = WriteFlow::new();
        let f = form_with(vec![
            oc_field(),
            field("cn", "Alice", "Alice"),
            field("sn", "Allen", "Adams"),
        ]);
        match wf.prepare(&f, &schema(), None, &[]) {
            PrepareSave::Ready { dn, ldif, .. } => {
                assert_eq!(dn, "cn=Alice,dc=example,dc=org");
                assert!(ldif.contains("sn"));
            }
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    /// A memberOf field bound to a fan-out picker (`fanout_attr = member`).
    fn memberof_field(values: Vec<&str>, baseline: Vec<&str>) -> EditField {
        use crate::config::relation::{CandidateScope, PickerBinding, StoreKey};
        use crate::config::widget::WidgetKind;
        EditField {
            label: "memberOf".into(),
            must: false,
            editable: true,
            multi: true,
            secret: false,
            ordered: false,
            orphaned: false,
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: Some(WidgetKind::Picker(PickerBinding {
                attr: "memberOf".into(),
                scope: CandidateScope {
                    base: "ou=groups,dc=example,dc=org".into(),
                    object_classes: vec!["groupOfNames".into()],
                    search_attrs: vec!["cn".into()],
                    label_template: None,
                },
                store: StoreKey::Dn,
                select: None,
                fanout_attr: Some("member".into()),
            })),
            values: values.into_iter().map(|s| s.to_string()).collect(),
            baseline: baseline.into_iter().map(|s| s.to_string()).collect(),
        }
    }

    /// A resolved password widget bound to the `person` object class.
    fn person_password_widget() -> ResolvedWidget {
        use crate::config::widget::{PasswordWidget, WidgetKind};
        ResolvedWidget {
            owner_object_classes: vec!["person".into()],
            attr: "userPassword".into(),
            kind: WidgetKind::Password(PasswordWidget {
                primary: "userPassword".into(),
                derived: Vec::new(),
                samba: false,
            }),
        }
    }

    /// Caller contract: with NO staged password, `prepare_combined` must still
    /// mask (and strip) the password primary so a stored baseline hash never diffs
    /// into a spurious `Delete userPassword`. The fan-out change is still captured.
    #[test]
    fn prepare_combined_no_pending_password_keeps_baseline_hash() {
        let wf = WriteFlow::new();
        let f = form_with(vec![
            oc_field(),
            field("cn", "Alice", "Alice"),
            field("sn", "Adams", "Adams"),
            // Hidden-hash state: directory holds a hash on the baseline, the widget
            // surfaces no value to the form editor.
            EditField {
                values: vec![],
                baseline: vec!["{SSHA}oldhash".into()],
                ..field("userPassword", "", "")
            },
            memberof_field(
                vec!["cn=g2,ou=groups,dc=example,dc=org"],
                vec!["cn=g1,ou=groups,dc=example,dc=org"],
            ),
        ]);
        match wf.prepare_combined(&f, &schema(), None, &[person_password_widget()]) {
            PlanCombined::Ready(cs) => {
                let touches_pw = cs.own_mods.iter().any(|m| {
                    let attr = match m {
                        ModOp::Add { attr, .. }
                        | ModOp::Delete { attr, .. }
                        | ModOp::Replace { attr, .. } => attr,
                    };
                    attr.eq_ignore_ascii_case("userPassword")
                });
                assert!(
                    !touches_pw,
                    "no staged password must not emit a userPassword mod; got {:?}",
                    cs.own_mods
                );
                assert_eq!(
                    cs.fanout.len(),
                    2,
                    "expected g1 Delete + g2 Add fan-out; got {:?}",
                    cs.fanout
                );
            }
            other => panic!("expected PlanCombined::Ready, got {other:?}"),
        }
    }

    #[test]
    fn write_ok_for_save_intent_reads_back() {
        let mut wf = WriteFlow::new();
        wf.pending.insert(
            7,
            WriteIntent::Save {
                reread_dn: "cn=Bob,dc=x".into(),
                quit_after: false,
            },
        );
        match wf.on_response(&Response::WriteOk {
            id: 7,
            dn: "cn=Bob,dc=x".into(),
            new_csn: None,
        }) {
            WriteOutcome::Saved {
                reread_dn,
                quit_after,
            } => {
                assert_eq!(reread_dn, "cn=Bob,dc=x");
                assert!(!quit_after);
            }
            other => panic!("expected Saved, got {other:?}"),
        }
        assert!(wf.pending.is_empty());
    }

    #[test]
    fn write_ok_for_rename_then_modify_requests_followup() {
        let mut wf = WriteFlow::new();
        wf.pending.insert(
            3,
            WriteIntent::RenameThenModify {
                new_dn: "cn=New,dc=x".into(),
                mods: vec![ModOp::Replace {
                    attr: "sn".into(),
                    values: vec!["Z".into()],
                }],
                quit_after: true,
            },
        );
        match wf.on_response(&Response::WriteOk {
            id: 3,
            dn: "cn=New,dc=x".into(),
            new_csn: None,
        }) {
            WriteOutcome::NeedFollowupModify {
                dn,
                mods,
                quit_after,
            } => {
                assert_eq!(dn, "cn=New,dc=x");
                assert_eq!(mods.len(), 1);
                assert!(quit_after);
            }
            other => panic!("expected NeedFollowupModify, got {other:?}"),
        }
    }

    #[test]
    fn write_error_surfaces_message() {
        let mut wf = WriteFlow::new();
        wf.pending.insert(
            9,
            WriteIntent::Save {
                reread_dn: "x".into(),
                quit_after: false,
            },
        );
        match wf.on_response(&Response::WriteError {
            id: 9,
            msg: "constraint".into(),
        }) {
            WriteOutcome::Error(m) => assert_eq!(m, "constraint"),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn create_submit_tracks_then_reports_created() {
        let mut wf = WriteFlow::new();
        // Inject a Create intent directly (same idiom as other tests — private
        // field access from the in-file test module, no *_for_test seams needed).
        wf.pending.insert(
            42,
            WriteIntent::Create {
                dn: "uid=bob,ou=people,dc=example,dc=org".into(),
                quit_after: false,
            },
        );
        match wf.on_response(&Response::WriteOk {
            id: 42,
            dn: "uid=bob,ou=people,dc=example,dc=org".into(),
            new_csn: None,
        }) {
            WriteOutcome::Created { dn, quit_after } => {
                assert_eq!(dn, "uid=bob,ou=people,dc=example,dc=org");
                assert!(!quit_after);
            }
            other => panic!("expected Created, got {other:?}"),
        }
        assert!(wf.pending.is_empty());
    }

    /// Task 17 RED: prepare with a non-empty pending_password yields a SavePlan::Modify
    /// containing the password REPLACE mod.
    #[test]
    fn prepare_with_pending_password_yields_modify_with_password_mod() {
        use crate::config::widget::{PasswordWidget, ResolvedWidget, WidgetKind};
        use crate::form::validate::SavePlan;

        let wf = WriteFlow::new();
        let mut fields = vec![
            oc_field(),
            field("cn", "Alice", "Alice"),
            field("sn", "Adams", "Adams"),
        ];
        fields.push(EditField {
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
            values: vec!["••••••".into()], // sentinel from StageSecret
            baseline: vec!["{SSHA}old".into()],
        });
        let f = form_with(fields);
        let widgets = vec![ResolvedWidget {
            owner_object_classes: vec!["person".into()],
            attr: "userPassword".into(),
            kind: WidgetKind::Password(PasswordWidget {
                primary: "userPassword".into(),
                derived: vec![],
                samba: false,
            }),
        }];
        match wf.prepare(&f, &schema(), Some("hunter2"), &widgets) {
            PrepareSave::Ready { plan, .. } => match plan {
                SavePlan::Modify(mods) => assert!(
                    mods.contains(&ModOp::Replace {
                        attr: "userPassword".into(),
                        values: vec!["hunter2".into()],
                    }),
                    "password mod must be in plan"
                ),
                _ => panic!("expected Modify"),
            },
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    /// Fix 1 TDD (RED→GREEN): a staged sentinel with object_classes that match NO
    /// password widget must never appear in the SavePlan. Before the fix, `prepare`
    /// emits `Replace userPassword ["••••••"]` — this test catches that regression.
    #[test]
    fn prepare_sentinel_not_submitted_when_no_widget_matches() {
        use crate::config::widget::{PasswordWidget as PwCfg, ResolvedWidget, WidgetKind};
        use crate::form::validate::SavePlan;

        let wf = WriteFlow::new();

        // Password field showing the staged sentinel; baseline is the old hash.
        let pw_field = EditField {
            label: "userPassword".into(),
            must: false,
            editable: false,
            multi: false,
            secret: true,
            ordered: false,
            orphaned: false,
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: None,
            values: vec![STAGED_PASSWORD_SENTINEL.to_string()],
            baseline: vec!["{SSHA}oldhash".into()],
        };
        // sn changed so we get a Ready result (not just NoChanges).
        let sn_changed = field("sn", "Allen", "Adams");
        // objectClass is only "top" → the inetOrgPerson widget does NOT match.
        let oc_top = EditField {
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
        let f = EditForm {
            dn: "cn=Alice,dc=example,dc=org".into(),
            mode: FormMode::Edit,
            object_classes: vec!["top".into()], // no inetOrgPerson → no widget match
            fields: vec![oc_top, field("cn", "Alice", "Alice"), sn_changed, pw_field],
            baseline_csn: None,
        };

        // Widget requires inetOrgPerson; form has only "top" → no match.
        let widgets = vec![ResolvedWidget {
            owner_object_classes: vec!["inetOrgPerson".into()],
            attr: "userPassword".into(),
            kind: WidgetKind::Password(PwCfg {
                primary: "userPassword".into(),
                derived: vec![],
                samba: false,
            }),
        }];

        let result = wf.prepare(&f, &schema(), Some("staged"), &widgets);

        let mods = match result {
            PrepareSave::Ready {
                plan: SavePlan::Modify(m),
                ..
            } => m,
            PrepareSave::Ready { .. } => vec![],
            PrepareSave::NoChanges => panic!("sn changed → expected Ready, not NoChanges"),
            PrepareSave::Invalid(e) => panic!("unexpected Invalid: {e:?}"),
            PrepareSave::DiffError(e) => panic!("DiffError: {e}"),
        };

        // The sentinel must not appear in any mod value.
        for m in &mods {
            let (attr, values) = match m {
                ModOp::Replace { attr, values } | ModOp::Add { attr, values } => (attr, values),
                _ => continue,
            };
            assert!(
                !values.iter().any(|v| v == STAGED_PASSWORD_SENTINEL),
                "sentinel must not appear in mod for {attr}: {values:?}"
            );
        }
        // No Replace/Delete for userPassword at all — the attr is stripped entirely.
        assert!(
            !mods.iter().any(|m| matches!(
                m,
                ModOp::Replace { attr, .. } | ModOp::Add { attr, .. } | ModOp::Delete { attr, .. }
                if attr.eq_ignore_ascii_case("userPassword")
            )),
            "userPassword must not appear in the plan when no widget matches; mods={mods:?}"
        );
    }

    // --- submit_combined (multi-entry membership write) -------------------

    fn combined_with(own_mods: Vec<ModOp>, fanout: Vec<(String, ModOp)>) -> CombinedSave {
        CombinedSave {
            own_dn: "uid=ann,ou=people,dc=x".into(),
            own_mods,
            fanout,
            ldif: String::new(),
        }
    }

    fn add_op(group: &str) -> (String, ModOp) {
        (
            group.into(),
            ModOp::Add {
                attr: "member".into(),
                values: vec!["uid=ann,ou=people,dc=x".into()],
            },
        )
    }

    fn del_op(group: &str) -> (String, ModOp) {
        (
            group.into(),
            ModOp::Delete {
                attr: "member".into(),
                values: vec!["uid=ann,ou=people,dc=x".into()],
            },
        )
    }

    /// own MODIFY + one Add group + one Delete group → three distinct Modify legs,
    /// all recorded in `pending` and `batches`.
    #[test]
    fn submit_combined_submits_all_legs_with_distinct_ids() {
        let mut wf = WriteFlow::new();
        let (worker, rx) = WorkerHandle::recording();
        let combined = combined_with(
            vec![ModOp::Replace {
                attr: "description".into(),
                values: vec!["new".into()],
            }],
            vec![
                add_op("cn=g2,ou=groups,dc=x"),
                del_op("cn=g1,ou=groups,dc=x"),
            ],
        );
        // g1 has two members → removing ann does not empty it.
        let mut members: HashMap<String, Vec<String>> = HashMap::new();
        members.insert(
            "cn=g1,ou=groups,dc=x".into(),
            vec![
                "uid=ann,ou=people,dc=x".into(),
                "uid=bob,ou=people,dc=x".into(),
            ],
        );

        wf.submit_combined(
            &worker,
            combined,
            &members,
            &HashMap::new(),
            None,
            "uid=ann,ou=people,dc=x",
            false,
        )
        .expect("valid combined save submits");

        // Drain the recorded requests: three Modifys with distinct ids.
        let mut ids = Vec::new();
        let mut leg_dns = Vec::new();
        while let Ok((req, _)) = rx.try_recv() {
            match req {
                Request::Modify { id, dn, .. } => {
                    ids.push(id);
                    leg_dns.push(dn);
                }
                _ => panic!("expected only Request::Modify legs"),
            }
        }
        assert_eq!(ids.len(), 3, "own + 2 group legs submitted");
        let distinct: std::collections::HashSet<u64> = ids.iter().copied().collect();
        assert_eq!(distinct.len(), 3, "every leg id must be distinct: {ids:?}");
        assert!(leg_dns.contains(&"uid=ann,ou=people,dc=x".to_string()));
        assert!(leg_dns.contains(&"cn=g2,ou=groups,dc=x".to_string()));
        assert!(leg_dns.contains(&"cn=g1,ou=groups,dc=x".to_string()));
        // Bookkeeping: three pending legs, one batch counting all three.
        assert_eq!(wf.pending.len(), 3);
        assert_eq!(wf.batches.len(), 1);
        assert_eq!(*wf.batches.values().next().unwrap(), 3);
    }

    /// Each group leg's `Request::Modify` carries that group's `entryCSN` from
    /// `group_csns`; a group missing from the map gets `assert_csn: None` (blind
    /// write, server remains the backstop). The own-entry leg carries whatever
    /// `own_assert_csn` the caller passed (the UI gates this on
    /// `assertion_supported` before calling; `submit_combined` itself just
    /// threads it through).
    #[test]
    fn combined_legs_carry_group_csn() {
        let mut wf = WriteFlow::new();
        let (worker, rx) = WorkerHandle::recording();
        let combined = combined_with(
            vec![ModOp::Replace {
                attr: "description".into(),
                values: vec!["new".into()],
            }],
            vec![add_op("cn=staff,dc=example,dc=org")],
        );
        let members: HashMap<String, Vec<String>> = HashMap::new();
        let mut csns = std::collections::HashMap::new();
        csns.insert(
            "cn=staff,dc=example,dc=org".to_string(),
            "G-CSN-1".to_string(),
        );

        wf.submit_combined(
            &worker,
            combined,
            &members,
            &csns,
            Some("OWN-CSN-1".to_string()),
            "uid=ann,ou=people,dc=x",
            false,
        )
        .expect("valid combined save submits");

        let mut own_csn = None;
        let mut group_csn = None;
        while let Ok((req, _)) = rx.try_recv() {
            match req {
                Request::Modify { dn, assert_csn, .. } if dn == "cn=staff,dc=example,dc=org" => {
                    group_csn = Some(assert_csn)
                }
                Request::Modify { dn, assert_csn, .. } if dn == "uid=ann,ou=people,dc=x" => {
                    own_csn = Some(assert_csn)
                }
                other => panic!("unexpected leg: {other:?}"),
            }
        }
        assert_eq!(
            group_csn,
            Some(Some("G-CSN-1".to_string())),
            "group leg asserts its entryCSN"
        );
        assert_eq!(
            own_csn,
            Some(Some("OWN-CSN-1".to_string())),
            "own-entry leg asserts the caller-supplied CSN when assertion is supported"
        );
    }

    /// When the caller passes `own_assert_csn: None` (assertion unsupported, or no
    /// baseline CSN available), the own-entry leg is a blind write — mirrors the
    /// plain-save gating idiom in `do_save` (`src/ui/app.rs`).
    #[test]
    fn combined_own_leg_blind_when_no_own_csn() {
        let mut wf = WriteFlow::new();
        let (worker, rx) = WorkerHandle::recording();
        let combined = combined_with(
            vec![ModOp::Replace {
                attr: "description".into(),
                values: vec!["new".into()],
            }],
            vec![add_op("cn=staff,dc=example,dc=org")],
        );
        let members: HashMap<String, Vec<String>> = HashMap::new();

        wf.submit_combined(
            &worker,
            combined,
            &members,
            &HashMap::new(),
            None,
            "uid=ann,ou=people,dc=x",
            false,
        )
        .expect("valid combined save submits");

        let mut own_csn = None;
        while let Ok((req, _)) = rx.try_recv() {
            if let Request::Modify { dn, assert_csn, .. } = req {
                if dn == "uid=ann,ou=people,dc=x" {
                    own_csn = Some(assert_csn);
                }
            }
        }
        assert_eq!(
            own_csn,
            Some(None),
            "own-entry leg is blind when own_assert_csn is None"
        );
    }

    /// A Delete that would empty a groupOfNames aborts with Err and submits NOTHING.
    #[test]
    fn submit_combined_last_member_aborts_with_nothing_submitted() {
        let mut wf = WriteFlow::new();
        let (worker, rx) = WorkerHandle::recording();
        let combined = combined_with(
            vec![ModOp::Replace {
                attr: "description".into(),
                values: vec!["new".into()],
            }],
            vec![
                add_op("cn=g2,ou=groups,dc=x"),
                del_op("cn=g1,ou=groups,dc=x"),
            ],
        );
        // g1's sole member is ann → removing her empties the group.
        let mut members: HashMap<String, Vec<String>> = HashMap::new();
        members.insert(
            "cn=g1,ou=groups,dc=x".into(),
            vec!["uid=ann,ou=people,dc=x".into()],
        );

        let err = wf
            .submit_combined(
                &worker,
                combined,
                &members,
                &HashMap::new(),
                None,
                "uid=ann,ou=people,dc=x",
                false,
            )
            .expect_err("last-member removal must abort");
        assert!(
            err.contains("cn=g1,ou=groups,dc=x"),
            "error names the offending group: {err}"
        );
        // Nothing submitted, nothing tracked.
        assert!(rx.try_recv().is_err(), "no request may be submitted");
        assert!(wf.pending.is_empty(), "no pending legs");
        assert!(wf.batches.is_empty(), "no batch registered");
    }

    /// The batch yields `CombinedSaved` only after the LAST leg's `WriteOk`.
    #[test]
    fn combined_batch_completes_only_on_last_leg() {
        let mut wf = WriteFlow::new();
        let batch_id = 1000;
        wf.batches.insert(batch_id, 2);
        for id in [1000u64, 1001] {
            wf.pending.insert(
                id,
                WriteIntent::CombinedLeg {
                    batch_id,
                    reread_dn: "uid=ann,ou=people,dc=x".into(),
                    quit_after: false,
                },
            );
        }
        // First leg → non-terminal BatchProgress.
        match wf.on_response(&Response::WriteOk {
            id: 1000,
            dn: "uid=ann,ou=people,dc=x".into(),
            new_csn: None,
        }) {
            WriteOutcome::BatchProgress { remaining } => assert_eq!(remaining, 1),
            other => panic!("expected BatchProgress, got {other:?}"),
        }
        // Last leg → terminal CombinedSaved.
        match wf.on_response(&Response::WriteOk {
            id: 1001,
            dn: "cn=g1,ou=groups,dc=x".into(),
            new_csn: None,
        }) {
            WriteOutcome::CombinedSaved {
                reread_dn,
                quit_after,
            } => {
                assert_eq!(reread_dn, "uid=ann,ou=people,dc=x");
                assert!(!quit_after);
            }
            other => panic!("expected CombinedSaved, got {other:?}"),
        }
        assert!(wf.pending.is_empty());
        assert!(wf.batches.is_empty());
    }

    /// A `WriteError` on any leg aborts the batch with a partial-application Error,
    /// and a subsequent sibling `WriteOk` is then Ignored (never CombinedSaved).
    #[test]
    fn combined_leg_error_reports_partial_and_aborts_batch() {
        let mut wf = WriteFlow::new();
        let batch_id = 2000;
        wf.batches.insert(batch_id, 2);
        for id in [2000u64, 2001] {
            wf.pending.insert(
                id,
                WriteIntent::CombinedLeg {
                    batch_id,
                    reread_dn: "uid=ann,ou=people,dc=x".into(),
                    quit_after: false,
                },
            );
        }
        match wf.on_response(&Response::WriteError {
            id: 2000,
            msg: "constraint violation".into(),
        }) {
            WriteOutcome::Error(m) => {
                assert!(
                    m.contains("constraint violation"),
                    "carries server msg: {m}"
                );
                assert!(
                    m.to_lowercase().contains("partial"),
                    "signals partial application: {m}"
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }
        assert!(wf.batches.is_empty(), "batch aborted on error");
        // A sibling leg's late success must NOT complete the (gone) batch.
        match wf.on_response(&Response::WriteOk {
            id: 2001,
            dn: "cn=g1,ou=groups,dc=x".into(),
            new_csn: None,
        }) {
            WriteOutcome::Ignored => {}
            other => panic!("expected Ignored after batch abort, got {other:?}"),
        }
        assert!(wf.pending.is_empty());
    }

    #[test]
    fn fetch_populates_only_must_membership_groups() {
        use crate::form::changeset::ModOp;
        use crate::ldap::worker::{LdapEntry, Response, SearchScope};
        use std::collections::BTreeMap;

        let (worker, rx) = WorkerHandle::recording();
        // Responder: answer each Base search by the requested base DN.
        let responder = std::thread::spawn(move || {
            while let Ok((req, reply)) = rx.recv() {
                let crate::ldap::worker::Request::Search { base, scope, .. } = req else {
                    continue;
                };
                assert!(matches!(scope, SearchScope::Base));
                let mut attrs = BTreeMap::new();
                if base.starts_with("cn=admins") {
                    // groupOfNames: member is MUST.
                    attrs.insert("objectClass".to_string(), vec!["groupOfNames".to_string()]);
                    attrs.insert("member".to_string(), vec!["uid=ann,ou=people".to_string()]);
                } else {
                    // posixGroup: memberUid is MAY.
                    attrs.insert("objectClass".to_string(), vec!["posixGroup".to_string()]);
                    attrs.insert("memberUid".to_string(), vec!["ann".to_string()]);
                }
                let _ = reply.send(Response::Entries {
                    id: 0,
                    entries: vec![LdapEntry {
                        dn: base.clone(),
                        attrs,
                        bin_attrs: BTreeMap::new(),
                    }],
                    truncated: false,
                });
            }
        });

        let schema = group_schema_for_write_flow();
        let fanout = vec![
            (
                "cn=admins,ou=groups".to_string(),
                ModOp::Delete {
                    attr: "member".into(),
                    values: vec!["uid=ann,ou=people".into()],
                },
            ),
            (
                "cn=staff,ou=groups".to_string(),
                ModOp::Delete {
                    attr: "memberUid".into(),
                    values: vec!["ann".into()],
                },
            ),
        ];
        let map = fetch_group_members_for_must(&worker, &schema, &fanout);
        // MUST group included; MAY group omitted.
        assert!(map.contains_key("cn=admins,ou=groups"));
        assert!(!map.contains_key("cn=staff,ou=groups"));
        assert_eq!(
            map["cn=admins,ou=groups"],
            vec!["uid=ann,ou=people".to_string()]
        );

        drop(worker); // closes rx so the responder thread exits
        let _ = responder.join();
    }

    /// `fetch_group_csns` requests `entryCSN` explicitly (operational attr) and
    /// maps each group DN to the CSN returned; a group whose search comes back
    /// empty is simply absent from the map (best-effort, server is the backstop).
    #[test]
    fn fetch_group_csns_reads_entry_csn_per_group() {
        use crate::ldap::worker::{LdapEntry, Response, SearchScope};
        use std::collections::BTreeMap;

        let (worker, rx) = WorkerHandle::recording();
        let responder = std::thread::spawn(move || {
            while let Ok((req, reply)) = rx.recv() {
                let crate::ldap::worker::Request::Search {
                    base, scope, attrs, ..
                } = req
                else {
                    continue;
                };
                assert!(matches!(scope, SearchScope::Base));
                assert_eq!(attrs, vec!["entryCSN".to_string()]);
                if base.starts_with("cn=admins") {
                    let mut attrs = BTreeMap::new();
                    attrs.insert("entryCSN".to_string(), vec!["CSN-ADMINS".to_string()]);
                    let _ = reply.send(Response::Entries {
                        id: 0,
                        entries: vec![LdapEntry {
                            dn: base.clone(),
                            attrs,
                            bin_attrs: BTreeMap::new(),
                        }],
                        truncated: false,
                    });
                } else {
                    // cn=gone: empty result → left out of the map.
                    let _ = reply.send(Response::Entries {
                        id: 0,
                        entries: vec![],
                        truncated: false,
                    });
                }
            }
        });

        let fanout = vec![
            (
                "cn=admins,ou=groups".to_string(),
                ModOp::Add {
                    attr: "member".into(),
                    values: vec!["uid=ann,ou=people".into()],
                },
            ),
            (
                "cn=gone,ou=groups".to_string(),
                ModOp::Add {
                    attr: "member".into(),
                    values: vec!["uid=ann,ou=people".into()],
                },
            ),
        ];
        let map = fetch_group_csns(&worker, &fanout);
        assert_eq!(
            map.get("cn=admins,ou=groups"),
            Some(&"CSN-ADMINS".to_string())
        );
        assert!(!map.contains_key("cn=gone,ou=groups"));

        drop(worker);
        let _ = responder.join();
    }

    fn group_schema_for_write_flow() -> SchemaModel {
        use crate::ldap::worker::RawSubschema;
        SchemaModel::from_raw(&RawSubschema {
            object_classes: vec![
                "( 2.5.6.0 NAME 'top' ABSTRACT MUST objectClass )".to_string(),
                "( 2.5.6.9 NAME 'groupOfNames' SUP top STRUCTURAL MUST ( member $ cn ) )"
                    .to_string(),
                "( 1.3.6.1.1.1.2.2 NAME 'posixGroup' SUP top STRUCTURAL \
                  MUST ( cn $ gidNumber ) MAY ( memberUid ) )"
                    .to_string(),
            ],
            attribute_types: vec![],
            ldap_syntaxes: vec![],
        })
    }

    #[test]
    fn entries_response_is_ignored_even_on_id_overlap() {
        // A read response with the same id as a pending write must NOT be consumed.
        let mut wf = WriteFlow::new();
        wf.pending.insert(
            1,
            WriteIntent::Save {
                reread_dn: "x".into(),
                quit_after: false,
            },
        );
        assert!(matches!(
            wf.on_response(&Response::Entries {
                id: 1,
                entries: vec![],
                truncated: false
            }),
            WriteOutcome::Ignored
        ));
        assert_eq!(wf.pending.len(), 1);
    }

    // --- Task 5: atomic create + sequential companion-then-primary fallback ---

    #[test]
    fn atomic_create_yields_created() {
        let mut wf = WriteFlow::new();
        wf.insert_create_intent_for_test(7, "uid=alice,ou=people,dc=x", true);
        match wf.on_response(&Response::WriteOk {
            id: 7,
            dn: "uid=alice,ou=people,dc=x".into(),
            new_csn: None,
        }) {
            WriteOutcome::Created { dn, quit_after } => {
                assert_eq!(dn, "uid=alice,ou=people,dc=x");
                assert!(quit_after);
            }
            other => panic!("expected Created, got {other:?}"),
        }
    }

    #[test]
    fn companion_ok_yields_needfollowupcreate() {
        let mut wf = WriteFlow::new();
        let mut attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        attrs.insert("uid".into(), vec!["alice".into()]);
        wf.insert_companion_intent_for_test(3, "uid=alice,ou=people,dc=x", attrs.clone(), false);
        match wf.on_response(&Response::WriteOk {
            id: 3,
            dn: "cn=alice,ou=groups,dc=x".into(),
            new_csn: None,
        }) {
            WriteOutcome::NeedFollowupCreate {
                dn,
                attrs: got,
                companion_dn,
                quit_after,
            } => {
                assert_eq!(dn, "uid=alice,ou=people,dc=x");
                assert_eq!(companion_dn, "cn=alice,ou=groups,dc=x");
                assert_eq!(got.get("uid"), Some(&vec!["alice".to_string()]));
                assert!(!quit_after);
            }
            other => panic!("expected NeedFollowupCreate, got {other:?}"),
        }
    }

    #[test]
    fn companion_error_yields_error_and_no_followup() {
        let mut wf = WriteFlow::new();
        let attrs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        wf.insert_companion_intent_for_test(4, "uid=alice,ou=people,dc=x", attrs, false);
        match wf.on_response(&Response::WriteError {
            id: 4,
            msg: "already exists".into(),
        }) {
            WriteOutcome::Error(msg) => assert!(msg.contains("already exists")),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn primary_after_companion_error_names_orphan() {
        let mut wf = WriteFlow::new();
        wf.insert_primary_after_companion_for_test(
            5,
            "uid=alice,ou=people,dc=x",
            "cn=alice,ou=groups,dc=x",
            false,
        );
        match wf.on_response(&Response::WriteError {
            id: 5,
            msg: "boom".into(),
        }) {
            WriteOutcome::Error(m) => {
                assert!(
                    m.contains("cn=alice,ou=groups,dc=x"),
                    "names the orphan: {m}"
                );
                assert!(m.contains("boom"));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn save_submit_carries_assert_csn() {
        let (worker, rx) = WorkerHandle::recording();
        let mut wf = WriteFlow::new();
        let plan = SavePlan::Modify(vec![ModOp::Replace {
            attr: "description".to_string(),
            values: vec!["x".to_string()],
        }]);
        wf.submit(
            &worker,
            plan,
            "cn=a,dc=example,dc=org",
            Some("CSN-123".to_string()),
            false,
        )
        .unwrap();
        let (req, _tx) = rx.recv().unwrap();
        match req {
            Request::Modify { assert_csn, .. } => {
                assert_eq!(assert_csn.as_deref(), Some("CSN-123"));
            }
            other => panic!("expected Modify, got {other:?}"),
        }
    }

    #[test]
    fn write_conflict_maps_to_conflict_outcome() {
        let mut wf = WriteFlow::new();
        let id = wf.insert_save_intent_for_test("cn=a,dc=example,dc=org".to_string(), false);
        let out = wf.on_response(&Response::WriteConflict {
            id,
            dn: "cn=a,dc=example,dc=org".to_string(),
        });
        assert!(matches!(out, WriteOutcome::Conflict { .. }));
    }

    /// A `WriteConflict` on a combined-save leg must NOT be routed through the
    /// single-entry `Conflict` path (the leg's dn may be a group, not the user
    /// entry the edit form is for). It aborts the batch and surfaces a
    /// reload-and-retry `Error`, mirroring the `WriteError` CombinedLeg case. A
    /// sibling leg's later response must then be `Ignored` (batch already gone).
    #[test]
    fn combined_leg_conflict_aborts_batch_and_reports_error() {
        let mut wf = WriteFlow::new();
        let batch_id = 3000;
        wf.batches.insert(batch_id, 2);
        for id in [3000u64, 3001] {
            wf.pending.insert(
                id,
                WriteIntent::CombinedLeg {
                    batch_id,
                    reread_dn: "uid=ann,ou=people,dc=x".into(),
                    quit_after: false,
                },
            );
        }
        match wf.on_response(&Response::WriteConflict {
            id: 3000,
            dn: "cn=g1,ou=groups,dc=x".into(),
        }) {
            WriteOutcome::Error(m) => {
                assert!(
                    m.contains("cn=g1,ou=groups,dc=x"),
                    "names the conflicting leg's dn: {m}"
                );
                assert!(
                    m.to_lowercase().contains("reload"),
                    "tells the user to reload and retry: {m}"
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }
        assert!(wf.batches.is_empty(), "batch aborted on leg conflict");
        // A sibling leg's late success must NOT complete the (gone) batch.
        match wf.on_response(&Response::WriteOk {
            id: 3001,
            dn: "uid=ann,ou=people,dc=x".into(),
            new_csn: None,
        }) {
            WriteOutcome::Ignored => {}
            other => panic!("expected Ignored after batch abort, got {other:?}"),
        }
        assert!(wf.pending.is_empty());
    }

    /// A `WriteConflict` on a genuine single-entry `Save` intent still yields
    /// `Conflict` — unchanged by the CombinedLeg fix above.
    #[test]
    fn save_conflict_still_yields_conflict_outcome() {
        let mut wf = WriteFlow::new();
        let id = wf.insert_save_intent_for_test("cn=a,dc=example,dc=org".to_string(), true);
        match wf.on_response(&Response::WriteConflict {
            id,
            dn: "cn=a,dc=example,dc=org".to_string(),
        }) {
            WriteOutcome::Conflict { dn, quit_after } => {
                assert_eq!(dn, "cn=a,dc=example,dc=org");
                assert!(quit_after);
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
    }
}
