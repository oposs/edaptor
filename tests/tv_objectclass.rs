//! Gated live test: editing objectClass regenerates the neutral form's fields.
//! Skips unless EDAPTOR_TEST_LDAP_URI is set.
//!
//! Enable by setting EDAPTOR_TEST_LDAP_URI (e.g. ldap://localhost:1389).
//! Start a server with: scripts/test-ldap.sh start
//! When the env var is unset the test prints SKIP and passes (no silent skip).
//!
//! Exercises the objectClass resync path end-to-end against a real LDAP:
//!   FetchSubschema -> base-read jsmith -> build_form_model -> build_edit_form
//!   -> remove sambaSamAccount from objectClass field -> sync_schema_fields
//!   -> assert sambaSID is now orphaned (no writes submitted).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use edaptor::config::{AuthConfig, AuthMethod, Config, PasswordSource, ServerConfig, TlsConfig};
use edaptor::ldap::worker::{
    LdapEntry, RawSubschema, Request, Response, SearchScope, WorkerHandle,
};
use edaptor::schema::SchemaModel;
use edaptor::workflows::edit_form::build_edit_form;
use edaptor::workflows::form_model::build_form_model;

// ---------------------------------------------------------------------------
// Helpers (mirroring tv_edit_write.rs — each tests/ file is standalone)
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

/// Poll until the `Subschema` reply arrives (carries no correlation id).
fn poll_for_subschema(worker: &WorkerHandle, timeout: Duration) -> Option<RawSubschema> {
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

// ---------------------------------------------------------------------------
// The gated test
// ---------------------------------------------------------------------------

#[test]
fn objectclass_change_regenerates_fields() {
    let Ok(uri) = std::env::var("EDAPTOR_TEST_LDAP_URI") else {
        eprintln!("SKIP objectclass_change_regenerates_fields: set EDAPTOR_TEST_LDAP_URI to run");
        return;
    };

    let (config, password) = test_config(uri);
    let worker = WorkerHandle::spawn(config, password).expect("spawn worker");

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
    // Step 2: Read a demo entry that carries sambaSamAccount.
    // uid=jsmith is seeded by scripts/test-ldap.sh with objectClasses:
    //   inetOrgPerson, posixAccount, shadowAccount, sambaSamAccount.
    // sambaSamAccount has MUST: sambaSID — so dropping it should orphan sambaSID.
    // This is a READ-ONLY probe; no writes are ever submitted.
    // -----------------------------------------------------------------------
    let dn = "uid=jsmith,ou=people,dc=example,dc=org";
    let raw_entry = read_entry(&worker, dn, 1).expect("jsmith must exist in the demo seed");

    let object_classes: Vec<String> = raw_entry
        .get("objectClass")
        .cloned()
        .expect("objectClass must be present in the entry");
    assert!(
        object_classes
            .iter()
            .any(|oc| oc.eq_ignore_ascii_case("sambaSamAccount")),
        "jsmith must carry sambaSamAccount; check demo seed"
    );

    // -----------------------------------------------------------------------
    // Step 3: Build a FormModel + EditForm (read-only probe — no writes).
    // -----------------------------------------------------------------------
    let ldap_entry = LdapEntry {
        dn: dn.to_string(),
        attrs: raw_entry.clone(),
        bin_attrs: Default::default(),
    };
    let oc_refs: Vec<&str> = object_classes.iter().map(|s| s.as_str()).collect();
    let form_model = build_form_model(&schema, &oc_refs, &ldap_entry, &[]);
    let mut edit_form = build_edit_form(&form_model, &schema, false);
    edit_form.object_classes = object_classes.clone();

    // Verify sambaSID is present and NOT orphaned before the change.
    // sambaSID is a MUST field of sambaSamAccount and is set in the seed entry.
    let samba_sid_before = edit_form
        .fields
        .iter()
        .find(|f| f.label.eq_ignore_ascii_case("sambaSID"))
        .expect("sambaSID must appear in the EditForm (jsmith seed entry has it)");
    assert!(
        !samba_sid_before.orphaned,
        "sambaSID must not be orphaned before the objectClass change"
    );

    // -----------------------------------------------------------------------
    // Step 4: Simulate removing sambaSamAccount from the objectClass field.
    // This mirrors exactly what the objectClass picker UI does when the user
    // un-ticks sambaSamAccount and the dialog commits.
    // -----------------------------------------------------------------------
    let oc_field = edit_form
        .fields
        .iter_mut()
        .find(|f| f.label.eq_ignore_ascii_case("objectClass"))
        .expect("objectClass must be a field in the EditForm");

    // Retain all classes except sambaSamAccount.
    oc_field.values = oc_field
        .values
        .iter()
        .filter(|oc| !oc.eq_ignore_ascii_case("sambaSamAccount"))
        .cloned()
        .collect();
    assert!(
        oc_field
            .values
            .iter()
            .all(|oc| !oc.eq_ignore_ascii_case("sambaSamAccount")),
        "sambaSamAccount must have been removed from the objectClass field values"
    );

    // -----------------------------------------------------------------------
    // Step 5: Resync the form fields from the updated objectClass values.
    // -----------------------------------------------------------------------
    edit_form.sync_schema_fields(&schema);

    // -----------------------------------------------------------------------
    // Step 6: Assert that sambaSID is now orphaned.
    // sambaSID is MUST for sambaSamAccount; after removing that class it is
    // no longer in MUST∪MAY → sync_schema_fields must mark it orphaned.
    // -----------------------------------------------------------------------
    let samba_sid_after = edit_form
        .fields
        .iter()
        .find(|f| f.label.eq_ignore_ascii_case("sambaSID"))
        .expect("sambaSID must still be present in the fields (orphaned, not deleted)");
    assert!(
        samba_sid_after.orphaned,
        "sambaSID must be orphaned after removing sambaSamAccount from objectClass"
    );

    // Verify objectClass itself is never orphaned (invariant of sync_schema_fields).
    let oc_after = edit_form
        .fields
        .iter()
        .find(|f| f.label.eq_ignore_ascii_case("objectClass"))
        .expect("objectClass must still be present in the EditForm");
    assert!(
        !oc_after.orphaned,
        "objectClass must never be marked orphaned"
    );

    // Read-only: no write submitted to the server. Demo data is intact.
}
