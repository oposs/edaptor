//! Live combined-membership-save integration test for the M4 tvision layer.
//!
//! Enable by setting EDAPTOR_TEST_LDAP_URI (e.g. ldap://localhost:1389).
//! Start a server with: scripts/test-ldap.sh start
//! When the env var is unset the test prints SKIP and passes (no silent skip).
//!
//! Drives the combined membership save end-to-end at the WORKFLOW level (no TUI):
//!   ADD temp user + temp group -> build EditForm with a memberOf fan-out field
//!   -> plan_combined_save -> submit_combined -> pump until CombinedSaved
//!   -> assert the group's `member` now contains the user DN AND we never wrote the
//!      user's `memberOf` ourselves (own_mods empty; fan-out targets the GROUP only)
//!   -> remove the membership again and assert it is gone. RAII guard cleans up the
//!      temp user + group even on failure.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use edaptor::config::relation::{CandidateScope, PickerBinding, StoreKey};
use edaptor::config::widget::WidgetKind;
use edaptor::config::{AuthConfig, AuthMethod, Config, PasswordSource, ServerConfig, TlsConfig};
use edaptor::form::changeset::ModOp;
use edaptor::ldap::worker::{Request, Response, SearchScope, WorkerHandle};
use edaptor::schema::{FieldKind, SchemaModel};
use edaptor::workflows::edit_form::{EditField, EditForm, FormMode};
use edaptor::workflows::form_model::WidgetSpec;
use edaptor::workflows::save::{last_member_block, plan_combined_save, PlanCombined};
use edaptor::workflows::write_flow::{fetch_group_members_for_must, WriteFlow, WriteOutcome};

// ---------------------------------------------------------------------------
// DN constants
// ---------------------------------------------------------------------------

const USER_DN: &str = "uid=tvm-user,ou=users,dc=example,dc=org";
const GROUP_DN: &str = "cn=tvm-group,ou=users,dc=example,dc=org";
// A real, always-present DN used as the group's placeholder member so the group is
// never empty (groupOfNames requires >= 1 member). Lets us add/remove the temp user
// without ever tripping the last-member rule.
const PLACEHOLDER_DN: &str = "cn=admin,dc=example,dc=org";
// A separate temp group used by the last-member-block live test; sole member is
// PLACEHOLDER_DN so the block fires when we attempt to remove it.
const SOLE_GROUP_DN: &str = "cn=tvm-sole-group,ou=users,dc=example,dc=org";

// ---------------------------------------------------------------------------
// Helpers (mirrored from tests/tv_edit_write.rs — each tests/ file is standalone)
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

/// Poll until any WriteOk or WriteError arrives (combined-save legs allocate their
/// ids internally, so the caller does not know them).
fn poll_any_write(worker: &WorkerHandle, timeout: Duration) -> Option<Response> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match worker.poll() {
            Some(resp @ Response::WriteOk { .. }) | Some(resp @ Response::WriteError { .. }) => {
                return Some(resp);
            }
            Some(_) => continue,
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    None
}

/// Base-read a DN, returning the requested attrs (None if not found).
fn read_entry(
    worker: &WorkerHandle,
    dn: &str,
    id: u64,
    attrs: &[&str],
) -> Option<BTreeMap<String, Vec<String>>> {
    worker
        .submit(Request::Search {
            id,
            base: dn.to_string(),
            scope: SearchScope::Base,
            filter: "(objectClass=*)".to_string(),
            attrs: attrs.iter().map(|s| s.to_string()).collect(),
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

/// Pump combined-save legs through `wf.on_response` until a terminal outcome
/// (CombinedSaved or Error); BatchProgress legs are non-terminal and looped over.
fn pump_combined(wf: &mut WriteFlow, worker: &WorkerHandle) -> WriteOutcome {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        let Some(resp) = poll_any_write(worker, Duration::from_secs(10)) else {
            return WriteOutcome::Error("timeout waiting for a write leg".into());
        };
        match wf.on_response(&resp) {
            WriteOutcome::BatchProgress { .. } | WriteOutcome::Ignored => continue,
            terminal => return terminal,
        }
    }
    WriteOutcome::Error("timeout waiting for CombinedSaved".into())
}

// ---------------------------------------------------------------------------
// RAII cleanup guard — deletes the temp entries on drop (group before user)
// ---------------------------------------------------------------------------

struct Cleanup<'a> {
    worker: &'a WorkerHandle,
    dns: Vec<String>,
    next_id: Cell<u64>,
}

impl Cleanup<'_> {
    fn delete_now(&self, dn: &str) {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let _ = self.worker.submit(Request::Delete {
            id,
            dn: dn.to_string(),
            assert_csn: None,
        });
        let _ = poll_for_id(self.worker, id, Duration::from_secs(5));
    }
}

impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        let dns = self.dns.clone();
        for dn in dns {
            self.delete_now(&dn);
        }
    }
}

// ---------------------------------------------------------------------------
// Form construction (manual, mirroring the real fan-out picker binding)
// ---------------------------------------------------------------------------

fn plain_field(label: &str, value: &str, must: bool, multi: bool) -> EditField {
    EditField {
        label: label.into(),
        must,
        editable: true,
        multi,
        secret: false,
        ordered: false,
        orphaned: false,
        kind: FieldKind::Text,
        widget: WidgetSpec::ReadOnlyText,
        widget_binding: None,
        values: vec![value.into()],
        baseline: vec![value.into()],
    }
}

fn oc_field(values: &[&str]) -> EditField {
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
        values: values.iter().map(|s| s.to_string()).collect(),
        baseline: values.iter().map(|s| s.to_string()).collect(),
    }
}

/// A memberOf field bound to a fan-out picker (`fanout_attr = member`): the
/// baseline is the user's current group set, `values` the desired set.
fn memberof_field(values: Vec<&str>, baseline: Vec<&str>) -> EditField {
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
                base: "ou=users,dc=example,dc=org".into(),
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

/// User form with no own-attribute change and a memberOf fan-out delta
/// (`baseline_groups` -> `selected_groups`).
fn user_form(selected_groups: Vec<&str>, baseline_groups: Vec<&str>) -> EditForm {
    EditForm {
        dn: USER_DN.into(),
        mode: FormMode::Edit,
        object_classes: vec!["top".into(), "inetOrgPerson".into()],
        fields: vec![
            oc_field(&["top", "inetOrgPerson"]),
            plain_field("uid", "tvm-user", false, false),
            plain_field("cn", "TVM User", true, false),
            plain_field("sn", "User", true, false),
            memberof_field(selected_groups, baseline_groups),
        ],
        baseline_csn: None,
    }
}

// ---------------------------------------------------------------------------
// The gated test
// ---------------------------------------------------------------------------

#[test]
fn combined_membership_save_round_trips_via_write_flow() {
    let Ok(uri) = std::env::var("EDAPTOR_TEST_LDAP_URI") else {
        eprintln!(
            "SKIP combined_membership_save_round_trips_via_write_flow: set EDAPTOR_TEST_LDAP_URI to run"
        );
        return;
    };

    let (config, password) = test_config(uri);
    let worker = WorkerHandle::spawn(config, password).expect("spawn worker");

    // Idempotent cleanup from any prior aborted run (group before user).
    for (id, dn) in [(1u64, GROUP_DN), (2u64, USER_DN)] {
        let _ = worker.submit(Request::Delete {
            id,
            dn: dn.to_string(),
            assert_csn: None,
        });
        let _ = poll_for_id(&worker, id, Duration::from_secs(5));
    }

    // Fetch the subschema (combined planning validates the own entry against it).
    worker
        .submit(Request::FetchSubschema)
        .expect("submit FetchSubschema");
    let raw =
        poll_for_subschema(&worker, Duration::from_secs(10)).expect("subschema within deadline");
    let schema = SchemaModel::from_raw(&raw);

    // ADD the temp user.
    {
        let mut attrs = BTreeMap::new();
        attrs.insert(
            "objectClass".to_string(),
            vec!["top".to_string(), "inetOrgPerson".to_string()],
        );
        attrs.insert("uid".to_string(), vec!["tvm-user".to_string()]);
        attrs.insert("cn".to_string(), vec!["TVM User".to_string()]);
        attrs.insert("sn".to_string(), vec!["User".to_string()]);
        worker
            .submit(Request::Add {
                id: 10,
                dn: USER_DN.to_string(),
                attrs,
            })
            .expect("submit ADD user");
        match poll_for_id(&worker, 10, Duration::from_secs(10)) {
            Some(Response::WriteOk { .. }) => {}
            other => panic!("ADD user failed: {}", describe(&other)),
        }
    }

    // ADD the temp group, seeded with a real placeholder member so it is never empty.
    {
        let mut attrs = BTreeMap::new();
        attrs.insert(
            "objectClass".to_string(),
            vec!["top".to_string(), "groupOfNames".to_string()],
        );
        attrs.insert("cn".to_string(), vec!["tvm-group".to_string()]);
        attrs.insert("member".to_string(), vec![PLACEHOLDER_DN.to_string()]);
        worker
            .submit(Request::Add {
                id: 20,
                dn: GROUP_DN.to_string(),
                attrs,
            })
            .expect("submit ADD group");
        match poll_for_id(&worker, 20, Duration::from_secs(10)) {
            Some(Response::WriteOk { .. }) => {}
            other => {
                // Clean up the user before failing.
                let _ = worker.submit(Request::Delete {
                    id: 21,
                    dn: USER_DN.to_string(),
                    assert_csn: None,
                });
                let _ = poll_for_id(&worker, 21, Duration::from_secs(5));
                panic!("ADD group failed: {}", describe(&other));
            }
        }
    }

    // RAII: from here on the temp entries are cleaned up on any exit (group, user).
    let _cleanup = Cleanup {
        worker: &worker,
        dns: vec![GROUP_DN.to_string(), USER_DN.to_string()],
        next_id: Cell::new(9000),
    };

    // -----------------------------------------------------------------------
    // 1. ADD membership: memberOf [] -> [GROUP_DN].
    // -----------------------------------------------------------------------
    let add_form = user_form(vec![GROUP_DN], vec![]);
    let combined =
        match plan_combined_save(&schema, &add_form, &[], &[], &[], &[], &Default::default()) {
            PlanCombined::Ready(cs) => cs,
            other => panic!("expected Ready for add-membership, got {other:?}"),
        };

    // Proof that we only MODIFY the GROUP (never write the user's memberOf): no own
    // mods, and the single fan-out leg is an Add of `member=USER_DN` on the GROUP.
    assert!(
        combined.own_mods.is_empty(),
        "own entry must not be modified (memberOf is overlay-maintained); got {:?}",
        combined.own_mods
    );
    assert_eq!(
        combined.fanout.len(),
        1,
        "expected exactly one fan-out leg; got {:?}",
        combined.fanout
    );
    let (gdn, op) = &combined.fanout[0];
    assert_eq!(gdn, GROUP_DN, "fan-out must target the GROUP, not the user");
    assert!(
        matches!(op, ModOp::Add { attr, values }
            if attr == "member" && values == &[USER_DN.to_string()]),
        "fan-out leg must Add member=USER_DN; got {op:?}"
    );

    let mut wf = WriteFlow::new();
    let group_members = std::collections::HashMap::new(); // best-effort (server backstop)
    wf.submit_combined(&worker, combined, &group_members, USER_DN, false)
        .expect("submit_combined add must not abort");
    match pump_combined(&mut wf, &worker) {
        WriteOutcome::CombinedSaved { reread_dn, .. } => {
            assert_eq!(reread_dn, USER_DN, "reread_dn must be the user DN");
        }
        other => panic!("expected CombinedSaved after add, got {other:?}"),
    }

    // Assert: the GROUP's member now contains the user DN.
    let group_attrs = read_entry(&worker, GROUP_DN, 50, &["member"]).expect("group must exist");
    let members: Vec<String> = group_attrs
        .get("member")
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|m| m.to_lowercase())
        .collect();
    assert!(
        members.contains(&USER_DN.to_lowercase()),
        "group member must contain the user after the combined add; got {members:?}"
    );

    // -----------------------------------------------------------------------
    // 2. REMOVE membership again: memberOf [GROUP_DN] -> [].
    // -----------------------------------------------------------------------
    let remove_form = user_form(vec![], vec![GROUP_DN]);
    let combined2 = match plan_combined_save(
        &schema,
        &remove_form,
        &[],
        &[],
        &[],
        &[],
        &Default::default(),
    ) {
        PlanCombined::Ready(cs) => cs,
        other => panic!("expected Ready for remove-membership, got {other:?}"),
    };
    assert!(
        combined2.own_mods.is_empty(),
        "remove must not touch the user"
    );
    assert!(
        combined2.fanout.iter().any(|(dn, op)| dn == GROUP_DN
            && matches!(op, ModOp::Delete { attr, values }
                if attr == "member" && values == &[USER_DN.to_string()])),
        "expected a Delete member=USER_DN on the group; got {:?}",
        combined2.fanout
    );

    let mut wf2 = WriteFlow::new();
    wf2.submit_combined(&worker, combined2, &group_members, USER_DN, false)
        .expect("submit_combined remove must not abort");
    match pump_combined(&mut wf2, &worker) {
        WriteOutcome::CombinedSaved { .. } => {}
        other => panic!("expected CombinedSaved after remove, got {other:?}"),
    }

    // Assert: the user DN is gone from the group (placeholder remains, not empty).
    let group_attrs2 =
        read_entry(&worker, GROUP_DN, 60, &["member"]).expect("group must still exist");
    let members2: Vec<String> = group_attrs2
        .get("member")
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|m| m.to_lowercase())
        .collect();
    assert!(
        !members2.contains(&USER_DN.to_lowercase()),
        "group member must NOT contain the user after the combined remove; got {members2:?}"
    );
    assert!(
        members2.contains(&PLACEHOLDER_DN.to_lowercase()),
        "placeholder member must remain so the group is never empty; got {members2:?}"
    );

    // _cleanup drops here -> deletes group then user.
}

// ---------------------------------------------------------------------------
// M5c B4: gated live assertion — last-member removal is blocked client-side
// ---------------------------------------------------------------------------
//
// Creates a temporary `groupOfNames` with PLACEHOLDER_DN as its sole member,
// then calls `fetch_group_members_for_must` + `last_member_block` to confirm
// the client-side pre-validation fires before any write. No permanent writes
// are made to demo data; cleanup deletes the temp group on exit.

#[test]
fn last_member_removal_blocked_client_side() {
    let Ok(uri) = std::env::var("EDAPTOR_TEST_LDAP_URI") else {
        eprintln!("SKIP last_member_removal_blocked_client_side: set EDAPTOR_TEST_LDAP_URI to run");
        return;
    };

    let (config, password) = test_config(uri);
    let worker = WorkerHandle::spawn(config, password).expect("spawn worker");

    // Idempotent cleanup from any prior aborted run.
    {
        let _ = worker.submit(Request::Delete {
            id: 1,
            dn: SOLE_GROUP_DN.to_string(),
            assert_csn: None,
        });
        let _ = poll_for_id(&worker, 1, Duration::from_secs(5));
    }

    // Fetch the subschema so fetch_group_members_for_must can check membership_attr_is_must.
    worker
        .submit(Request::FetchSubschema)
        .expect("submit FetchSubschema");
    let raw =
        poll_for_subschema(&worker, Duration::from_secs(10)).expect("subschema within deadline");
    let schema = SchemaModel::from_raw(&raw);

    // Create a groupOfNames with PLACEHOLDER_DN as its sole member.
    {
        let mut attrs = BTreeMap::new();
        attrs.insert(
            "objectClass".to_string(),
            vec!["top".to_string(), "groupOfNames".to_string()],
        );
        attrs.insert("cn".to_string(), vec!["tvm-sole-group".to_string()]);
        attrs.insert("member".to_string(), vec![PLACEHOLDER_DN.to_string()]);
        worker
            .submit(Request::Add {
                id: 10,
                dn: SOLE_GROUP_DN.to_string(),
                attrs,
            })
            .expect("submit ADD sole group");
        match poll_for_id(&worker, 10, Duration::from_secs(10)) {
            Some(Response::WriteOk { .. }) => {}
            other => panic!("ADD sole group failed: {}", describe(&other)),
        }
    }

    // RAII: delete the temp group on any exit path.
    let _cleanup = Cleanup {
        worker: &worker,
        dns: vec![SOLE_GROUP_DN.to_string()],
        next_id: Cell::new(9000),
    };

    // Build the fan-out that would remove PLACEHOLDER_DN (the sole member).
    let fanout: Vec<(String, ModOp)> = vec![(
        SOLE_GROUP_DN.to_string(),
        ModOp::Delete {
            attr: "member".into(),
            values: vec![PLACEHOLDER_DN.to_string()],
        },
    )];

    // Schema-gated live fetch: `member` is MUST for groupOfNames, so the map is populated.
    let group_members = fetch_group_members_for_must(&worker, &schema, &fanout);
    assert!(
        group_members.contains_key(SOLE_GROUP_DN),
        "fetch_group_members_for_must must populate the map for a MUST-membership group \
         (groupOfNames); got {group_members:?}"
    );
    let stored = group_members
        .get(SOLE_GROUP_DN)
        .expect("group must be in map");
    assert_eq!(
        stored.len(),
        1,
        "sole group must have exactly one member; got {stored:?}"
    );
    assert!(
        stored[0].eq_ignore_ascii_case(PLACEHOLDER_DN),
        "stored member must be PLACEHOLDER_DN (case-insensitive); got {:?}",
        stored[0]
    );

    // Pre-validation: last_member_block must refuse removing the sole member.
    let block = last_member_block(&fanout, &group_members, PLACEHOLDER_DN);
    assert!(
        block.is_some(),
        "last_member_block must block removing the sole member of a groupOfNames"
    );
    let msg = block.unwrap();
    assert!(
        msg.contains("would leave"),
        "block message must contain \"would leave\"; got: {msg}"
    );

    // Verify demo data is intact: the group still has its member (no write was submitted).
    let group_attrs =
        read_entry(&worker, SOLE_GROUP_DN, 50, &["member"]).expect("sole group must still exist");
    let members: Vec<String> = group_attrs
        .get("member")
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|m| m.to_lowercase())
        .collect();
    assert!(
        members.contains(&PLACEHOLDER_DN.to_lowercase()),
        "sole group member must be intact after a client-side block (no write happened); \
         got {members:?}"
    );

    // _cleanup drops here -> deletes SOLE_GROUP_DN.
}
