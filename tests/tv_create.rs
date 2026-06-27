//! Live create integration test for the M3 Phase 2b tvision layer (CreateFlow).
//!
//! Enable by setting EDAPTOR_TEST_LDAP_URI (e.g. ldap://localhost:1389).
//! Start a server with: scripts/test-ldap.sh start
//! When the env var is unset the test prints SKIP and passes (no silent skip).
//!
//! Exercises the M3 neutral create path end-to-end against a real LDAP:
//!   FetchSubschema -> build_create_form (People profile, inetOrgPerson+posixAccount)
//!   -> set_value for all MUST fields -> plan_create -> CreatePrep::Confirm
//!   -> Request::Add -> poll WriteOk -> base-read (assert entry+OC)
//!   -> Request::Delete -> verify entry gone.
//!
//! Cleanup guarantee: an `EntryCleanup` RAII guard is constructed right after the
//! ADD succeeds and fires a best-effort DELETE in its `Drop` impl.  This ensures
//! the test entry is removed even when an assertion panics mid-test.  The
//! idempotent pre-cleanup at the top handles any leftover from a prior crashed run.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use edaptor::config::{
    AuthConfig, AuthMethod, Config, EntryProfile, PasswordSource, ServerConfig, TlsConfig,
};
use edaptor::ldap::worker::{Request, Response, SearchScope, WorkerHandle};
use edaptor::schema::SchemaModel;
use edaptor::workflows::create::{build_create_form, plan_create, CreatePrep};

// ---------------------------------------------------------------------------
// RAII guard: ensures the test entry is deleted even on assertion panic.
// ---------------------------------------------------------------------------

/// Fires a best-effort DELETE for `dn` when dropped.
///
/// `submit` takes `&self` on `WorkerHandle`, so holding a shared reference is
/// sufficient.  The guard uses request-id 99 (reserved for cleanup).  No
/// response is awaited inside `drop` — the caller can poll for id 99 separately
/// when an explicit "entry is gone" assertion is desired.
struct EntryCleanup<'a> {
    worker: &'a WorkerHandle,
    dn: String,
}

impl<'a> EntryCleanup<'a> {
    fn new(worker: &'a WorkerHandle, dn: String) -> Self {
        Self { worker, dn }
    }
}

impl Drop for EntryCleanup<'_> {
    fn drop(&mut self) {
        // Best-effort: submit and ignore errors / response.
        // The next-run idempotent pre-cleanup is the final backstop.
        let _ = self.worker.submit(Request::Delete {
            id: 99,
            dn: std::mem::take(&mut self.dn),
        });
    }
}

// ---------------------------------------------------------------------------
// Helpers (mirror of tv_edit_write.rs — each tests/ file is standalone)
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

/// Poll until the `Subschema` reply arrives (it carries no correlation id).
fn poll_for_subschema(
    worker: &WorkerHandle,
    timeout: Duration,
) -> Option<edaptor::ldap::worker::RawSubschema> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match worker.poll() {
            Some(Response::Subschema(raw)) => return Some(raw),
            Some(_) => continue,
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
fn create_entry_via_neutral_create_path() {
    let Ok(uri) = std::env::var("EDAPTOR_TEST_LDAP_URI") else {
        eprintln!("SKIP create_entry_via_neutral_create_path: set EDAPTOR_TEST_LDAP_URI to run");
        return;
    };

    let (config, password) = test_config(uri);
    let worker = WorkerHandle::spawn(config, password).expect("spawn worker");

    let container = "ou=people,dc=example,dc=org";
    let rdn_uid = "zz-tv-create-test";
    let test_dn = format!("uid={rdn_uid},{container}");

    // -----------------------------------------------------------------------
    // Idempotent cleanup from any prior aborted run.
    // -----------------------------------------------------------------------
    let _ = worker.submit(Request::Delete {
        id: 1,
        dn: test_dn.clone(),
    });
    let _ = poll_for_id(&worker, 1, Duration::from_secs(5));

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
    // Step 2: Build a People-style profile (inetOrgPerson + posixAccount).
    //
    // posixAccount MUST: uid, uidNumber, gidNumber, homeDirectory
    // person MUST (via inetOrgPerson): cn, sn
    // We set all MUST fields manually so plan_create passes validation.
    // -----------------------------------------------------------------------
    let profile = EntryProfile {
        name: "People".to_string(),
        object_classes: vec!["inetOrgPerson".to_string(), "posixAccount".to_string()],
        rdn_attr: "uid".to_string(),
        search_base: container.to_string(),
        show: vec![
            "uid".to_string(),
            "cn".to_string(),
            "sn".to_string(),
            "uidNumber".to_string(),
            "gidNumber".to_string(),
            "homeDirectory".to_string(),
        ],
        search_attrs: vec!["cn".to_string(), "uid".to_string()],
        defaults: Default::default(),
        widgets: Default::default(),
        label: None,
    };

    let (mut create_form, _autonum) = build_create_form(&schema, &profile, 0, container);

    // Set every MUST field so plan_create sees a valid entry.
    // Helper: set field by case-insensitive label.
    let set_field =
        |form: &mut edaptor::workflows::edit_form::EditForm, label: &str, value: &str| {
            if let Some(idx) = form
                .fields
                .iter()
                .position(|f| f.label.eq_ignore_ascii_case(label))
            {
                form.set_value(idx, value.to_string());
            } else {
                panic!(
                    "field '{label}' not found in create form; available: {:?}",
                    form.fields.iter().map(|f| &f.label).collect::<Vec<_>>()
                );
            }
        };

    set_field(&mut create_form, "uid", rdn_uid);
    set_field(&mut create_form, "cn", "ZZ TV Create Test");
    set_field(&mut create_form, "sn", "TvCreateTest");
    // posixAccount MUST fields (manual values — Block B provides autonumber later).
    set_field(&mut create_form, "uidNumber", "99997");
    set_field(&mut create_form, "gidNumber", "99997");
    set_field(&mut create_form, "homeDirectory", "/home/zz-tv-create-test");

    // -----------------------------------------------------------------------
    // Step 3: plan_create → must produce Confirm (not Error).
    // -----------------------------------------------------------------------
    let edited = create_form.to_edit_entry();
    let prep = plan_create(&schema, &profile, container, &edited);
    let (confirm_dn, confirm_attrs) = match prep {
        CreatePrep::Confirm { dn, attrs, .. } => (dn, attrs),
        CreatePrep::Error(msg) => panic!("plan_create returned Error: {msg}"),
    };

    assert_eq!(
        confirm_dn, test_dn,
        "plan_create must compose the correct DN"
    );
    let oc = confirm_attrs
        .get("objectClass")
        .expect("objectClass must be in composed attrs");
    assert!(
        oc.iter().any(|c| c.eq_ignore_ascii_case("inetOrgPerson")),
        "objectClass must include inetOrgPerson; got {oc:?}"
    );
    assert!(
        oc.iter().any(|c| c.eq_ignore_ascii_case("posixAccount")),
        "objectClass must include posixAccount; got {oc:?}"
    );

    // -----------------------------------------------------------------------
    // Step 4: Submit the ADD via the worker.
    // -----------------------------------------------------------------------
    worker
        .submit(Request::Add {
            id: 10,
            dn: confirm_dn.clone(),
            attrs: confirm_attrs,
        })
        .expect("submit ADD");
    match poll_for_id(&worker, 10, Duration::from_secs(10)) {
        Some(Response::WriteOk { .. }) => {}
        other => panic!("ADD failed: {}", describe(&other)),
    }

    // Construct the cleanup guard NOW, immediately after the ADD is confirmed.
    // If any later assertion panics, `drop` fires a best-effort DELETE so the
    // entry is not left behind indefinitely.  Request-id 99 is reserved for
    // this guard; no other submit in this test uses that id.
    let cleanup = EntryCleanup::new(&worker, confirm_dn.clone());

    // -----------------------------------------------------------------------
    // Step 5: Read the new entry back and assert it exists with the OC set.
    // -----------------------------------------------------------------------
    let entry_attrs = read_entry(&worker, &confirm_dn, 11)
        .expect("newly created entry must exist at the composed DN");

    let entry_oc = entry_attrs
        .get("objectClass")
        .expect("created entry must have objectClass attribute");
    assert!(
        entry_oc
            .iter()
            .any(|c| c.eq_ignore_ascii_case("inetOrgPerson")),
        "created entry must have inetOrgPerson objectClass; got {entry_oc:?}"
    );
    assert!(
        entry_oc
            .iter()
            .any(|c| c.eq_ignore_ascii_case("posixAccount")),
        "created entry must have posixAccount objectClass; got {entry_oc:?}"
    );

    // Sanity-check a key attribute.
    assert_eq!(
        entry_attrs.get("uid").map(|v| v.as_slice()),
        Some([rdn_uid.to_string()].as_slice()),
        "created entry must have the correct uid"
    );
    assert_eq!(
        entry_attrs.get("homeDirectory").map(|v| v.as_slice()),
        Some(["/home/zz-tv-create-test".to_string()].as_slice()),
        "created entry must have the correct homeDirectory"
    );

    // -----------------------------------------------------------------------
    // Step 6: DELETE the test entry — let the guard own the deletion.
    //
    // Dropping the guard submits Request::Delete{id:99} (best-effort).  We then
    // poll for id 99 to wait for the server's acknowledgement before verifying
    // the entry is gone.
    // -----------------------------------------------------------------------
    drop(cleanup); // fires DELETE id 99
    match poll_for_id(&worker, 99, Duration::from_secs(10)) {
        Some(Response::WriteOk { .. }) => {}
        other => panic!("DELETE failed: {}", describe(&other)),
    }

    // Verify the entry is gone.
    assert!(
        read_entry(&worker, &confirm_dn, 21).is_none(),
        "deleted entry must no longer exist"
    );
}
