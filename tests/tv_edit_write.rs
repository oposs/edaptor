//! Live edit+write integration test for the M2 tvision layer (WriteFlow + EditForm).
//!
//! Enable by setting EDAPTOR_TEST_LDAP_URI (e.g. ldap://localhost:1389).
//! Start a server with: scripts/test-ldap.sh start
//! When the env var is unset the test prints SKIP and passes (no silent skip).
//!
//! Exercises the M2 domain layer end-to-end against a real LDAP:
//!   FetchSubschema -> ADD -> build_form_model -> build_edit_form -> set_value
//!   -> WriteFlow::prepare -> WriteFlow::submit -> poll WriteOk -> on_response
//!   -> re-read (MODIFY case), then MODRDN (rename case), then DELETE (cleanup).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use edaptor::config::{AuthConfig, AuthMethod, Config, PasswordSource, ServerConfig, TlsConfig};
use edaptor::ldap::worker::{Request, Response, SearchScope, WorkerHandle};
use edaptor::schema::SchemaModel;
use edaptor::workflows::edit_form::build_edit_form;
use edaptor::workflows::form_model::build_form_model;
use edaptor::workflows::save::PrepareSave;
use edaptor::workflows::write_flow::{WriteFlow, WriteOutcome};

// ---------------------------------------------------------------------------
// Helpers (mirror of live_write.rs — each tests/ file is standalone)
// ---------------------------------------------------------------------------

fn test_config(uri: String) -> (Config, String) {
    let config = Config {
        server: ServerConfig {
            uri,
            base_dn: "dc=example,dc=org".to_string(),
            start_tls: false,
            read_only: false,
            timeout_secs: 10,
            tls: TlsConfig::default(),
        },
        auth: AuthConfig {
            method: AuthMethod::Simple,
            bind_dn: Some("cn=admin,dc=example,dc=org".to_string()),
            password_source: PasswordSource::Env("EDAPTOR_TEST_ADMIN_PW".to_string()),
        },
        profiles: Vec::new(),
        meta: Default::default(),
        samba: Default::default(),
        tree: Default::default(),
    };
    let password =
        std::env::var("EDAPTOR_TEST_ADMIN_PW").unwrap_or_else(|_| "adminpassword".to_string());
    (config, password)
}

/// Poll the worker channel until a reply correlated to `want_id` arrives.
fn poll_for_id(worker: &WorkerHandle, want_id: u64, timeout: Duration) -> Option<Response> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match worker.poll() {
            Some(resp) => match &resp {
                Response::Entries { id, .. }
                | Response::SearchError { id, .. }
                | Response::WriteOk { id, .. }
                | Response::WriteError { id, .. }
                    if *id == want_id =>
                {
                    return Some(resp);
                }
                _ => continue,
            },
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    None
}

/// Poll until the `Subschema` reply arrives (it carries no correlation id, so it
/// cannot use `poll_for_id`). Deadline loop with sleep, mirroring `poll_for_id`,
/// so a momentarily-empty channel on a loaded server does not spuriously fail.
fn poll_for_subschema(
    worker: &WorkerHandle,
    timeout: Duration,
) -> Option<edaptor::ldap::worker::RawSubschema> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match worker.poll() {
            Some(Response::Subschema(raw)) => return Some(raw),
            Some(_) => continue, // discard unrelated replies, like poll_for_id
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    None
}

/// Poll until any WriteOk or WriteError arrives (for WriteFlow-owned requests
/// whose ids are allocated internally and not known to the caller).
fn poll_any_write(worker: &WorkerHandle, timeout: Duration) -> Option<Response> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match worker.poll() {
            Some(resp @ Response::WriteOk { .. }) | Some(resp @ Response::WriteError { .. }) => {
                return Some(resp);
            }
            Some(_) => continue, // discard unrelated responses
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    None
}

/// Base-read a DN, returning the entry's string attrs (None if not found).
fn read_entry(worker: &WorkerHandle, dn: &str, id: u64) -> Option<BTreeMap<String, Vec<String>>> {
    worker
        .submit(Request::Search {
            id,
            base: dn.to_string(),
            scope: SearchScope::Base,
            filter: "(objectClass=*)".to_string(),
            attrs: vec!["*".to_string()],
            size_limit: None,
        })
        .expect("submit base search");
    match poll_for_id(worker, id, Duration::from_secs(10)) {
        Some(Response::Entries { entries, .. }) => entries.into_iter().next().map(|e| e.attrs),
        _ => None,
    }
}

fn describe(resp: &Option<Response>) -> String {
    match resp {
        Some(Response::WriteOk { dn, .. }) => format!("WriteOk({dn})"),
        Some(Response::WriteError { msg, .. }) => format!("WriteError({msg})"),
        Some(Response::Entries { entries, .. }) => format!("Entries({})", entries.len()),
        Some(Response::SearchError { msg, .. }) => format!("SearchError({msg})"),
        Some(_) => "other".to_string(),
        None => "timeout".to_string(),
    }
}

// ---------------------------------------------------------------------------
// The gated test
// ---------------------------------------------------------------------------

#[test]
fn edit_persists_and_rename_via_write_flow() {
    let Ok(uri) = std::env::var("EDAPTOR_TEST_LDAP_URI") else {
        eprintln!("SKIP edit_persists_and_rename_via_write_flow: set EDAPTOR_TEST_LDAP_URI to run");
        return;
    };

    let (config, password) = test_config(uri);
    let worker = WorkerHandle::spawn(config, password).expect("spawn worker");

    let container = "ou=users,dc=example,dc=org";
    let cn1 = "edaptor-m2-it";
    let cn2 = "edaptor-m2-it-renamed";
    let dn1 = format!("cn={cn1},{container}");
    let dn2 = format!("cn={cn2},{container}");

    // -----------------------------------------------------------------------
    // Idempotent cleanup from any prior aborted run.
    // -----------------------------------------------------------------------
    for (id, d) in [(1u64, &dn1), (2u64, &dn2)] {
        let _ = worker.submit(Request::Delete {
            id,
            dn: d.clone(),
            assert_csn: None,
        });
        let _ = poll_for_id(&worker, id, Duration::from_secs(5));
    }

    // -----------------------------------------------------------------------
    // Step 1: Fetch the subschema.
    // -----------------------------------------------------------------------
    worker
        .submit(Request::FetchSubschema)
        .expect("submit FetchSubschema");
    let raw = poll_for_subschema(&worker, Duration::from_secs(10))
        .expect("subschema should arrive within the deadline");
    let schema = SchemaModel::from_raw(&raw);

    // -----------------------------------------------------------------------
    // Step 2: ADD a temp entry.
    // -----------------------------------------------------------------------
    let mut attrs = BTreeMap::new();
    attrs.insert(
        "objectClass".to_string(),
        vec!["top".to_string(), "inetOrgPerson".to_string()],
    );
    attrs.insert("cn".to_string(), vec![cn1.to_string()]);
    attrs.insert("sn".to_string(), vec!["M2IT".to_string()]);
    worker
        .submit(Request::Add {
            id: 10,
            dn: dn1.clone(),
            attrs,
        })
        .expect("submit ADD");
    match poll_for_id(&worker, 10, Duration::from_secs(10)) {
        Some(Response::WriteOk { .. }) => {}
        other => panic!("ADD failed: {}", describe(&other)),
    }

    // -----------------------------------------------------------------------
    // Step 3: Base-read it; build FormModel -> EditForm.
    // -----------------------------------------------------------------------
    let raw_entry = read_entry(&worker, &dn1, 11).expect("added entry must exist");

    // Reconstruct an LdapEntry for build_form_model.
    let ldap_entry = edaptor::ldap::worker::LdapEntry {
        dn: dn1.clone(),
        attrs: raw_entry.clone(),
        bin_attrs: Default::default(),
    };

    let object_classes: Vec<String> = raw_entry.get("objectClass").cloned().unwrap_or_default();
    let oc_refs: Vec<&str> = object_classes.iter().map(|s| s.as_str()).collect();

    let form_model = build_form_model(&schema, &oc_refs, &ldap_entry, &[]);
    let mut edit_form = build_edit_form(&form_model, &schema, false);
    edit_form.object_classes = object_classes.clone();

    // -----------------------------------------------------------------------
    // Step 4: Set the description field (add it if not present as a MAY).
    // inetOrgPerson allows 'description', so it will appear in the MAY set.
    // -----------------------------------------------------------------------
    let desc_idx = edit_form
        .fields
        .iter()
        .position(|f| f.label.eq_ignore_ascii_case("description"))
        .expect("description must appear as a MAY field for inetOrgPerson");

    edit_form.set_value(desc_idx, "edaptor m2 live".to_string());
    assert!(edit_form.is_dirty(), "form must be dirty after set_value");

    // -----------------------------------------------------------------------
    // Step 5: WriteFlow prepare -> submit -> poll -> on_response (MODIFY).
    // -----------------------------------------------------------------------
    let mut wf = WriteFlow::new();
    let (plan, _ldif) = match wf.prepare(&edit_form, &schema, None, &[]) {
        PrepareSave::Ready { plan, ldif, .. } => (plan, ldif),
        other => panic!("expected Ready for description change, got {other:?}"),
    };

    wf.submit(&worker, plan, &dn1, false)
        .expect("submit WriteFlow modify");

    let write_resp = poll_any_write(&worker, Duration::from_secs(10));
    match wf.on_response(write_resp.as_ref().expect("WriteFlow modify must respond")) {
        WriteOutcome::Saved { reread_dn, .. } => {
            assert_eq!(reread_dn, dn1, "reread_dn must be the same DN after MODIFY");
        }
        other => panic!("expected Saved, got {other:?}"),
    }

    // Re-read and assert description was written.
    let after_modify = read_entry(&worker, &dn1, 12).expect("entry must exist after modify");
    assert_eq!(
        after_modify.get("description"),
        Some(&vec!["edaptor m2 live".to_string()]),
        "description must be updated after WriteFlow MODIFY"
    );

    // -----------------------------------------------------------------------
    // Step 6: RENAME (MODRDN) via WriteFlow.
    // Build a fresh EditForm for the rename: baseline cn = cn1, value cn = cn2.
    // The edit_form's dn must be dn1 (old DN) so prepare sees the rename.
    // -----------------------------------------------------------------------

    // Rebuild the entry from the re-read attrs, incorporating the new description.
    let ldap_entry2 = edaptor::ldap::worker::LdapEntry {
        dn: dn1.clone(),
        attrs: after_modify.clone(),
        bin_attrs: Default::default(),
    };
    let form_model2 = build_form_model(&schema, &oc_refs, &ldap_entry2, &[]);
    let mut edit_form2 = build_edit_form(&form_model2, &schema, false);
    edit_form2.object_classes = object_classes.clone();

    // Change the cn field to trigger a rename.
    let cn_idx = edit_form2
        .fields
        .iter()
        .position(|f| f.label.eq_ignore_ascii_case("cn"))
        .expect("cn must be a field");

    edit_form2.set_value(cn_idx, cn2.to_string());
    assert!(edit_form2.is_dirty(), "form must be dirty after cn change");

    let mut wf2 = WriteFlow::new();
    let (plan2, _ldif2) = match wf2.prepare(&edit_form2, &schema, None, &[]) {
        PrepareSave::Ready { plan, ldif, .. } => (plan, ldif),
        other => panic!("expected Ready for rename, got {other:?}"),
    };

    wf2.submit(&worker, plan2, &dn1, false)
        .expect("submit WriteFlow rename");

    let rename_resp = poll_any_write(&worker, Duration::from_secs(10));
    let rename_outcome =
        wf2.on_response(rename_resp.as_ref().expect("WriteFlow rename must respond"));

    // Handle both RenameOnly (WriteOutcome::Saved) and Rename (NeedFollowupModify).
    let final_dn = match rename_outcome {
        WriteOutcome::Saved { reread_dn, .. } => {
            // RenameOnly: no attribute changes beyond the RDN.
            reread_dn
        }
        WriteOutcome::NeedFollowupModify {
            dn,
            mods,
            quit_after,
        } => {
            // Rename + then_mods: submit the followup modify.
            wf2.submit_followup(&worker, &dn, mods, quit_after)
                .expect("submit WriteFlow followup modify");
            let followup_resp = poll_any_write(&worker, Duration::from_secs(10));
            match wf2.on_response(
                followup_resp
                    .as_ref()
                    .expect("WriteFlow followup must respond"),
            ) {
                WriteOutcome::Saved { reread_dn, .. } => reread_dn,
                other => panic!("expected Saved after followup modify, got {other:?}"),
            }
        }
        other => panic!("expected Saved or NeedFollowupModify for rename, got {other:?}"),
    };

    assert_eq!(final_dn, dn2, "final DN after rename must be dn2");

    // Verify new DN exists; old DN is gone.
    assert!(
        read_entry(&worker, &dn2, 13).is_some(),
        "renamed entry must exist at new DN"
    );
    assert!(
        read_entry(&worker, &dn1, 14).is_none(),
        "old DN must no longer resolve after rename"
    );

    // -----------------------------------------------------------------------
    // Step 7: DELETE the temp entry (cleanup).
    // -----------------------------------------------------------------------
    worker
        .submit(Request::Delete {
            id: 40,
            dn: dn2.clone(),
            assert_csn: None,
        })
        .expect("submit DELETE");
    match poll_for_id(&worker, 40, Duration::from_secs(10)) {
        Some(Response::WriteOk { .. }) => {}
        other => panic!("DELETE failed: {}", describe(&other)),
    }
    assert!(
        read_entry(&worker, &dn2, 41).is_none(),
        "deleted entry must be gone"
    );
}
