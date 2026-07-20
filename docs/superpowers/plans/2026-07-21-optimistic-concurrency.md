# Optimistic Concurrency Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make eDAPtor's MODIFY and DELETE writes fail safely when another client changed the entry since it was read, instead of silently overwriting — using `entryCSN` + the RFC 4528 Assertion control, with a rebase-or-prompt retry.

**Architecture:** Every entry read captures the server's `entryCSN` (an operational attribute) as a version token stored on the `EditForm`. Each MODIFY/DELETE attaches a *critical* Assertion control `(entryCSN=<captured>)` plus a Post-Read control that returns the new `entryCSN` in the write response. The server applies the write only if the CSN still matches (else result code 122, `assertionFailed`). On 122 the UI re-reads, and if the other party's changes do not overlap the attributes we are writing it rebases and retries silently; on overlap it prompts. A connect-time capability probe disables the mechanism (with a one-time warning) against servers lacking the control, so writes never break there.

**Tech Stack:** Rust, `ldap3` 0.12.1 (`Assertion`, `PostRead`, `PostReadResp`, `RawControl`, `Control`, `ControlType`, `ldap_escape` — all first-class, no manual BER), OpenLDAP (test server via `scripts/test-ldap.sh`).

## Global Constraints

- **Cap build/test parallelism at 4 cores** (shared machine): `cargo test -j4`, `cargo clippy -- -D warnings`.
- **`make check`** (fmt + clippy `-D warnings` + tests) must pass before any task is "done".
- **ldap3 pinned at 0.12.1**; do not bump. Use only the control APIs it exposes.
- **`ldap3` types must not leak past the worker** (`src/ldap/worker.rs`): all control construction/parsing stays inside the worker; cross-thread `Request`/`Response` carry only plain Rust types (`String`, `Option<String>`, `Vec`, `BTreeMap`).
- **Comments, identifiers, keys in English.** User-facing strings may be prose but here are English.
- **`Assertion::new()` panics on a malformed filter.** Always build the filter as `format!("(entryCSN={})", ldap3::ldap_escape(csn))` and use the struct-literal `Assertion { filter }.critical().into()` route (the `new()` route cannot be marked critical).
- **entryCSN is operational** — it is only returned when explicitly named in the requested attribute list (`"*"` alone does not include it).
- **Keep `CHANGES.md` and docs in sync** (see Task 9); a behaviour change is not done until they are updated.

---

## File Structure

- `src/ldap/result.rs` — add rc 122 / rc 12 human messages (Task 1).
- `src/ldap/worker.rs` — `assertion_supported()` pure fn + `Response::RootDse.supported_controls` (Task 2); assertion+post-read on `Request::Modify` and `Request::Delete`, new `Response::WriteConflict`, `Response::WriteOk.new_csn` (Tasks 4, 5).
- `src/ui/state.rs` — capability flag + one-time warning (Task 2); store `baseline_csn` on read (Task 3); conflict handling in `apply_write_outcome` (Task 7).
- `src/workflows/read_flow.rs` — request `entryCSN`, carry it out of `ReadOutcome::Form` (Task 3).
- `src/workflows/edit_form.rs` — `EditForm.baseline_csn` field (Task 3).
- `src/workflows/write_flow.rs` — thread `assert_csn` into `submit`, new `WriteOutcome::Conflict`, new-CSN passthrough (Task 6).
- `src/ui/dialog/conflict.rs` — new conflict dialog (Task 7).
- `src/ui/dialog/mod.rs` — register the new module (Task 7).
- `docs/src/concepts/`, `CHANGES.md`, `examples/config.toml` — docs (Task 9).

## Interfaces locked across tasks

These names/types are introduced by an early task and consumed later. Use them verbatim.

- `crate::ldap::worker::assertion_supported(controls: &[String]) -> bool` (Task 2).
- `Response::RootDse { supported_extensions: Vec<String>, supported_controls: Vec<String> }` (Task 2).
- `Request::Modify { id, dn, changes, assert_csn: Option<String> }` (Task 4).
- `Request::Delete { id, dn, assert_csn: Option<String> }` (Task 5).
- `Response::WriteOk { id, dn, new_csn: Option<String> }` (Task 4).
- `Response::WriteConflict { id, dn }` (Task 4).
- `EditForm.baseline_csn: Option<String>` (Task 3).
- `ReadOutcome::Form { model, object_classes, baseline_csn: Option<String> }` (Task 3).
- `WriteOutcome::Conflict { dn, quit_after }` (Task 6).
- `UiState.assertion_supported: bool` and `UiState.concurrency_warned: bool` (Task 2).

---

## Task 1: Human messages for rc 122 and rc 12

**Files:**
- Modify: `src/ldap/result.rs:13-30`
- Test: `src/ldap/result.rs` (existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: nothing.
- Produces: `result_code_message(122, _)` / `result_code_message(12, _)` return stable prose used by the worker's error mapping.

- [ ] **Step 1: Write the failing tests** — add to the `tests` module in `src/ldap/result.rs`:

```rust
    #[test]
    fn maps_assertion_failed() {
        let m = result_code_message(122, "assertion failed");
        assert!(
            m.starts_with("Entry was modified by someone else"),
            "m={m}"
        );
    }

    #[test]
    fn maps_unavailable_critical_extension() {
        let m = result_code_message(12, "");
        assert!(
            m.starts_with("Server does not support a required control"),
            "m={m}"
        );
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -j4 -p edaptor --lib ldap::result 2>&1 | tail -20`
Expected: FAIL — both new tests fail (the messages currently fall through to `"LDAP error 122: ..."` / `"LDAP error 12"`).

- [ ] **Step 3: Add the two arms** — in `result_code_message`, insert into the `match rc` block (keep numeric order — 12 before 16, 122 after 68):

```rust
        12 => "Server does not support a required control",
```
(place directly above the `16 => ...` arm), and

```rust
        122 => "Entry was modified by someone else since you loaded it",
```
(place directly below the `68 => ...` arm, before the `_ =>` fallback).

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -j4 -p edaptor --lib ldap::result 2>&1 | tail -20`
Expected: PASS — all `ldap::result` tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/ldap/result.rs
git commit -m "feat(ldap): map rc 122 (assertionFailed) and rc 12 to human messages"
```

---

## Task 2: Capability probe for the Assertion control

The root DSE fetch currently returns only `supportedExtension`. Extend it to also return `supportedControl`, add a pure `assertion_supported()`, store a flag on `UiState`, and prepare a one-time warning field. (The warning is *shown* in Task 7 when the first blind write happens; here we only detect and store.)

**Files:**
- Modify: `src/ldap/worker.rs` (`Response::RootDse` variant ~183-185; `fetch_root_dse` ~804-820; add `assertion_supported` near `txn_supported` ~39-43)
- Modify: `src/ui/state.rs` (bootstrap probe ~923-930; `UiState` struct fields; struct construction ~987)
- Test: `src/ldap/worker.rs` `#[cfg(test)]`

**Interfaces:**
- Consumes: nothing.
- Produces: `assertion_supported(&[String]) -> bool`; `Response::RootDse.supported_controls`; `UiState.assertion_supported: bool`; `UiState.concurrency_warned: bool`.

- [ ] **Step 1: Write the failing test** — add near the existing worker unit tests (search for `mod tests` in `src/ldap/worker.rs`; if none, add `#[cfg(test)] mod cap_tests { use super::*; ... }`):

```rust
    #[test]
    fn assertion_control_detected() {
        // RFC 4528 Assertion control OID.
        let with = vec!["1.3.6.1.1.12".to_string(), "1.2.3".to_string()];
        let without = vec!["1.2.3".to_string()];
        assert!(assertion_supported(&with));
        assert!(!assertion_supported(&without));
        assert!(!assertion_supported(&[]));
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -j4 -p edaptor --lib assertion_control_detected 2>&1 | tail -20`
Expected: FAIL — `assertion_supported` not found.

- [ ] **Step 3: Add the OID const + pure fn** — in `src/ldap/worker.rs`, directly below `txn_supported` (after line 43):

```rust
/// RFC 4528 Assertion control OID.
pub const ASSERTION_CONTROL_OID: &str = "1.3.6.1.1.12";

/// True iff the server's `supportedControl` advertises the RFC 4528 Assertion
/// control — the prerequisite for optimistic-concurrency writes. Pure.
pub fn assertion_supported(controls: &[String]) -> bool {
    controls.iter().any(|c| c == ASSERTION_CONTROL_OID)
}
```

- [ ] **Step 4: Extend `Response::RootDse`** — change the variant (worker.rs ~183):

```rust
    /// Root DSE `supportedExtension` + `supportedControl` values (reply to
    /// [`Request::FetchRootDse`]).
    RootDse {
        supported_extensions: Vec<String>,
        supported_controls: Vec<String>,
    },
```

- [ ] **Step 5: Extend `fetch_root_dse`** — it currently requests only `supportedExtension` and returns `Vec<String>`. Change it to request both and return a tuple. Replace the body (worker.rs ~804-820) so it requests `vec!["supportedExtension", "supportedControl"]`, extracts both, and returns `Result<(Vec<String>, Vec<String>)>`:

```rust
/// Read the root DSE (`""`, base scope) and return `(supportedExtension,
/// supportedControl)` values.
fn fetch_root_dse(conn: &mut LdapConn) -> Result<(Vec<String>, Vec<String>)> {
    let (entries, _res) = conn
        .search(
            "",
            Scope::Base,
            "(objectClass=*)",
            vec!["supportedExtension", "supportedControl"],
        )?
        .success()
        .context("reading root DSE")?;
    let entry = entries.into_iter().map(SearchEntry::construct).next();
    let exts = entry
        .as_ref()
        .and_then(|e| e.attrs.get("supportedExtension").cloned())
        .unwrap_or_default();
    let ctrls = entry
        .and_then(|e| e.attrs.get("supportedControl").cloned())
        .unwrap_or_default();
    Ok((exts, ctrls))
}
```

- [ ] **Step 6: Update the `FetchRootDse` handler** — worker.rs ~414-422:

```rust
            Request::FetchRootDse => {
                let resp = match fetch_root_dse(conn) {
                    Ok((supported_extensions, supported_controls)) => Response::RootDse {
                        supported_extensions,
                        supported_controls,
                    },
                    Err(e) => Response::Error(e.to_string()),
                };
                let _ = reply.send(resp);
            }
```

- [ ] **Step 7: Store the flag in bootstrap** — `src/ui/state.rs` ~923-930, replace the probe so it captures both:

```rust
    // Tolerant capability probe: a failed/absent root DSE just means "no
    // support" for txn / assertion (never fail bootstrap over it).
    let (server_supports_txn, assertion_supported) =
        match worker.request(Request::FetchRootDse) {
            Ok(Response::RootDse {
                supported_extensions,
                supported_controls,
            }) => (
                crate::ldap::worker::txn_supported(&supported_extensions),
                crate::ldap::worker::assertion_supported(&supported_controls),
            ),
            _ => (false, false),
        };
```

- [ ] **Step 8: Add the `UiState` fields** — near `server_supports_txn`'s declaration in the `UiState` struct (`src/ui/state.rs`), add:

```rust
    /// Whether the server advertises the RFC 4528 Assertion control. When false,
    /// writes fall back to blind (no optimistic-concurrency protection).
    pub assertion_supported: bool,
    /// Set once the first blind (unprotected) write has warned the operator, so
    /// the "concurrent edits may be lost" notice is shown only once per session.
    pub concurrency_warned: bool,
```

And in the `UiState { .. }` construction (~987, where `server_supports_txn` is set), add:

```rust
            assertion_supported,
            concurrency_warned: false,
```

- [ ] **Step 9: Fix the other `Response::RootDse` match sites** — the variant gained a field, so any other match must bind it. Search and update:

Run: `grep -rn "RootDse" src/ tests/`
For each match on `Response::RootDse { supported_extensions }` add `, supported_controls: _` (or bind it). Expect one in `src/lib.rs` and/or headless test helpers.

- [ ] **Step 10: Run tests + clippy**

Run: `cargo test -j4 -p edaptor --lib 2>&1 | tail -25 && cargo clippy --all-targets -- -D warnings 2>&1 | tail -15`
Expected: PASS, no warnings. The new field must be constructed everywhere `Response::RootDse` is built (headless test fixtures included).

- [ ] **Step 11: Commit**

```bash
git add src/ldap/worker.rs src/ui/state.rs src/lib.rs
git commit -m "feat(ldap): probe supportedControl for the RFC 4528 assertion control"
```

---

## Task 3: Capture entryCSN on read and store it on the edit form

**Files:**
- Modify: `src/workflows/read_flow.rs` (`request_entry` ~65-72; `ReadOutcome::Form` variant ~22-27; `on_response` ~83-94; add a CSN extractor)
- Modify: `src/workflows/edit_form.rs` (`EditForm` struct ~92-97; `build_edit_form` ~376-381)
- Modify: `src/ui/state.rs` (`ReadOutcome::Form` arm ~239-263)
- Test: `src/workflows/read_flow.rs` tests

**Interfaces:**
- Consumes: nothing.
- Produces: `ReadOutcome::Form { model, object_classes, baseline_csn: Option<String> }`; `EditForm.baseline_csn: Option<String>`.

- [ ] **Step 1: Write the failing test** — in `src/workflows/read_flow.rs` tests, extend `entry()` to include an `entryCSN` and add a test. First add the attr inside the existing `entry()` helper (after the `sn` insert):

```rust
        attrs.insert(
            "entryCSN".to_string(),
            vec!["20260717071723.439475Z#000000#000#000000".to_string()],
        );
```

Then add:

```rust
    #[test]
    fn form_carries_entry_csn() {
        let mut flow = ReadFlow::new(schema());
        flow.pending.insert(7, vec![]);
        let resp = Response::Entries {
            id: 7,
            entries: vec![entry()],
            truncated: false,
        };
        match flow.on_response(&resp) {
            ReadOutcome::Form { baseline_csn, .. } => {
                assert_eq!(
                    baseline_csn.as_deref(),
                    Some("20260717071723.439475Z#000000#000#000000")
                );
            }
            _ => panic!("expected a form"),
        }
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -j4 -p edaptor --lib read_flow 2>&1 | tail -20`
Expected: FAIL — `ReadOutcome::Form` has no `baseline_csn` field.

- [ ] **Step 3: Request the operational attribute** — `read_flow.rs` `request_entry`, change the `attrs` line (~70):

```rust
            attrs: vec!["*".to_string(), "entryCSN".to_string()],
```

- [ ] **Step 4: Add the field to `ReadOutcome::Form`** — `read_flow.rs` ~22-27:

```rust
    Form {
        /// The schema-driven form model.
        model: FormModel,
        /// The entry's objectClass values.
        object_classes: Vec<String>,
        /// The entry's `entryCSN` at read time (version token for optimistic
        /// concurrency). `None` if the server did not return it.
        baseline_csn: Option<String>,
    },
```

- [ ] **Step 5: Add a CSN extractor + populate it** — in `read_flow.rs`, add a helper beside `object_classes_of`:

```rust
/// Extract an entry's `entryCSN` (case-insensitive attribute lookup).
fn entry_csn_of(entry: &LdapEntry) -> Option<String> {
    entry
        .attrs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("entryCSN"))
        .and_then(|(_, v)| v.first().cloned())
}
```

And in `on_response`, the `Entries` arm that builds `ReadOutcome::Form` (~90-93):

```rust
                ReadOutcome::Form {
                    model: self.form_for(entry, &show),
                    object_classes: object_classes_of(entry),
                    baseline_csn: entry_csn_of(entry),
                }
```

- [ ] **Step 6: Add `baseline_csn` to `EditForm`** — `edit_form.rs` struct (~92-97):

```rust
pub struct EditForm {
    pub dn: String,
    pub mode: FormMode,
    pub object_classes: Vec<String>,
    pub fields: Vec<EditField>,
    /// `entryCSN` at load time — the version asserted on save. `None` disables
    /// optimistic concurrency for this form (create mode, or server without it).
    pub baseline_csn: Option<String>,
}
```

And in `build_edit_form` construction (~376-381), add the field (it defaults to `None`; the caller fills it in, like `object_classes`):

```rust
    EditForm {
        dn: model.title.clone(),
        mode: FormMode::Edit,
        object_classes: Vec::new(),
        fields,
        baseline_csn: None,
    }
```

- [ ] **Step 7: Thread it through the state arm** — `src/ui/state.rs` `ReadOutcome::Form` arm (~239-244). Change the destructure to bind `baseline_csn` and assign it onto the form right after `object_classes`:

```rust
                ReadOutcome::Form {
                    model,
                    object_classes,
                    baseline_csn,
                } => {
                    let mut form = build_edit_form(&model, self.read_flow.schema(), self.read_only);
                    form.object_classes = object_classes;
                    form.baseline_csn = baseline_csn;
```

- [ ] **Step 8: Fix any `FormMode::Create` / other `EditForm { .. }` constructions** — the struct gained a field.

Run: `grep -rn "EditForm {" src/`
For each literal construction (e.g. the create flow), add `baseline_csn: None,`. Create forms have no CSN by definition.

- [ ] **Step 9: Run tests + clippy**

Run: `cargo test -j4 -p edaptor --lib read_flow 2>&1 | tail -20 && cargo clippy --all-targets -- -D warnings 2>&1 | tail -15`
Expected: PASS, no warnings.

- [ ] **Step 10: Commit**

```bash
git add src/workflows/read_flow.rs src/workflows/edit_form.rs src/ui/state.rs
git commit -m "feat(read): capture entryCSN at read time onto EditForm.baseline_csn"
```

---

## Task 4: Assertion + Post-Read on MODIFY (worker)

Add `assert_csn` to `Request::Modify`; when present, attach a critical Assertion + a Post-Read; map rc 122 to a new `Response::WriteConflict`; carry the post-read `entryCSN` back in `Response::WriteOk.new_csn`.

**Files:**
- Modify: `src/ldap/worker.rs` — imports (~26); `Request::Modify` (~121-129); `Response::WriteOk` (~217-223); add `Response::WriteConflict`; `run_modify` (~527-530); `write_response` usage; the `Modify` handler (~456-458)
- Test: `src/ldap/worker.rs` `#[cfg(test)]` (pure control-building helper) + a `tests/live_write.rs` live test

**Interfaces:**
- Consumes: `result_code_message` (Task 1).
- Produces: `Request::Modify { id, dn, changes, assert_csn: Option<String> }`; `Response::WriteOk { id, dn, new_csn: Option<String> }`; `Response::WriteConflict { id, dn }`.

- [ ] **Step 1: Add imports** — `src/ldap/worker.rs` line 26, extend the controls import:

```rust
use ldap3::controls::{Assertion, Control, ControlType, PostRead, PostReadResp, RawControl, TxnSpec};
```

- [ ] **Step 2: Add `assert_csn` to `Request::Modify`** — worker.rs ~121-129:

```rust
    Modify {
        /// Correlation id.
        id: u64,
        /// Target DN.
        dn: String,
        /// The attribute modifications (pure domain type from `form::changeset`).
        changes: Vec<ModOp>,
        /// When set, assert `(entryCSN=<this>)` (RFC 4528, critical) so the write
        /// applies only if the entry is unchanged since it was read. `None` = blind.
        assert_csn: Option<String>,
    },
```

- [ ] **Step 3: Add `new_csn` to `WriteOk` and add `WriteConflict`** — worker.rs ~217-231:

```rust
    /// A successful write (Modify/Add/ModRdn/Delete); `id` echoes the request.
    WriteOk {
        /// Correlation id.
        id: u64,
        /// The affected DN (post-rename DN for ModRdn is computed by the caller).
        dn: String,
        /// The entry's new `entryCSN` from the Post-Read control, when requested
        /// and returned. Refreshes the edit-form baseline without a re-read.
        new_csn: Option<String>,
    },
    /// A write refused because the asserted `entryCSN` no longer matched (rc 122):
    /// the entry changed since it was read. Distinct from `WriteError` so the flow
    /// can trigger the rebase-or-prompt path. `id` echoes the request.
    WriteConflict {
        /// Correlation id.
        id: u64,
        /// The affected DN (for the re-read).
        dn: String,
    },
```

- [ ] **Step 4: Update `write_response` to carry `new_csn`** — the existing helper (worker.rs ~510-525) is shared by Add/ModRdn/Delete blind paths. Add a `new_csn` parameter defaulting to `None` at those call sites, and detect rc 122:

```rust
fn write_response(
    id: u64,
    dn: &str,
    new_csn: Option<String>,
    res: ldap3::result::Result<ldap3::LdapResult>,
) -> Response {
    match res {
        Ok(r) if r.rc == 0 => Response::WriteOk {
            id,
            dn: dn.to_string(),
            new_csn,
        },
        Ok(r) if r.rc == 122 => Response::WriteConflict {
            id,
            dn: dn.to_string(),
        },
        Ok(r) => Response::WriteError {
            id,
            msg: result_code_message(r.rc, &r.text),
        },
        Err(e) => Response::WriteError {
            id,
            msg: format!("{e}"),
        },
    }
}
```

- [ ] **Step 5: Update the blind `write_response` callers** — `run_add` (~542), `run_modrdn` (~629), `run_delete` (~633) currently call `write_response(id, dn, conn.<op>(...))`. Insert `None` as the new third arg, e.g.:

```rust
    write_response(id, dn, None, conn.add(dn, entry))
```
```rust
    write_response(id, dn, None, conn.modifydn(dn, new_rdn, delete_old, new_superior))
```
(Delete gets its own assertion in Task 5 — leave it `None` here; Task 5 replaces `run_delete`.)

- [ ] **Step 6: Add a pure post-read CSN extractor** — near `write_response` in worker.rs:

```rust
/// Pull the `entryCSN` from a write result's Post-Read response control, if the
/// server returned one. ldap3 parses control values; we only read the string.
fn post_read_csn(ctrls: &[Control]) -> Option<String> {
    for c in ctrls {
        if let Control(Some(ControlType::PostReadResp), raw) = c {
            if raw.val.is_some() {
                let resp: PostReadResp = raw.parse();
                if let Some(v) = resp.attrs.get("entryCSN").and_then(|v| v.first()) {
                    return Some(v.clone());
                }
            }
        }
    }
    None
}
```

- [ ] **Step 7: Rewrite `run_modify`** — worker.rs ~527-530:

```rust
fn run_modify(
    conn: &mut LdapConn,
    id: u64,
    dn: &str,
    changes: &[ModOp],
    assert_csn: Option<&str>,
) -> Response {
    let mods: Vec<Mod<String>> = changes.iter().map(mod_op_to_ldap3).collect();
    let Some(csn) = assert_csn else {
        // Blind path (server lacks the control, or no baseline CSN): unchanged.
        return write_response(id, dn, None, conn.modify(dn, mods));
    };
    let filter = format!("(entryCSN={})", ldap3::ldap_escape(csn));
    let ctrls: Vec<RawControl> = vec![
        Assertion { filter }.critical().into(),
        PostRead::new(vec!["entryCSN"]),
    ];
    match conn.with_controls(ctrls).modify(dn, mods) {
        Ok(r) if r.rc == 0 => Response::WriteOk {
            id,
            dn: dn.to_string(),
            new_csn: post_read_csn(&r.ctrls),
        },
        Ok(r) if r.rc == 122 => Response::WriteConflict {
            id,
            dn: dn.to_string(),
        },
        Ok(r) => Response::WriteError {
            id,
            msg: result_code_message(r.rc, &r.text),
        },
        Err(e) => Response::WriteError {
            id,
            msg: format!("{e}"),
        },
    }
}
```

- [ ] **Step 8: Update the `Modify` handler** — worker.rs ~456-458:

```rust
            Request::Modify {
                id,
                dn,
                changes,
                assert_csn,
            } => {
                let _ = reply.send(run_modify(conn, id, &dn, &changes, assert_csn.as_deref()));
            }
```

- [ ] **Step 9: Fix every `Request::Modify` / `Response::WriteOk` construction site** — the shapes changed.

Run: `grep -rn "Request::Modify\|WriteOk {" src/ tests/`
For each `Request::Modify { .. }` builder add `assert_csn: None,` (Task 6 sets the real value for the save path; `fetch_group_members_for_must` and any other caller stay `None` for now). For each `Response::WriteOk { id, dn }` pattern/const add `new_csn` (bind `..` in matches, add `new_csn: None` in constructions). Expect sites in `src/workflows/write_flow.rs` (multiple `Request::Modify`/`Request::Add`; the `on_response` match on `WriteOk`).

- [ ] **Step 10: Write a live test** — append to `tests/live_write.rs` (which already gates on `EDAPTOR_TEST_LDAP_URI` and SKIP-passes when unset; mirror an existing test's connection setup):

```rust
#[test]
fn modify_with_stale_csn_conflicts() {
    let Some(_uri) = std::env::var("EDAPTOR_TEST_LDAP_URI").ok() else {
        eprintln!("SKIP modify_with_stale_csn_conflicts: EDAPTOR_TEST_LDAP_URI unset");
        return;
    };
    // Reuse this file's helper that yields a bound WorkerHandle + a scratch DN
    // seeded with a known entry. (Follow the pattern of the existing delete test.)
    // 1. Read the entry's current entryCSN.
    // 2. Submit Request::Modify with assert_csn = a deliberately wrong CSN
    //    ("19700101000000.000000Z#000000#000#000000").
    // 3. Assert the response is Response::WriteConflict { .. }.
    // 4. Submit again with the correct CSN; assert Response::WriteOk { new_csn: Some(_), .. }.
    //    (See scripts/test-ldap.sh; assertion control verified present on that server.)
}
```

Fill the body against the same fixture the existing `delete` test in this file uses (read that test first for the exact handle/DN setup — do not invent a new harness).

- [ ] **Step 11: Run**

Run: `cargo test -j4 -p edaptor --lib 2>&1 | tail -20 && cargo clippy --all-targets -- -D warnings 2>&1 | tail -15`
Then, with the server up (`scripts/test-ldap.sh start`; `export EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389` — confirm the port/creds the other live tests use):
Run: `cargo test -j4 -p edaptor --test live_write modify_with_stale_csn 2>&1 | tail -20`
Expected: unit/clippy PASS; live test PASS (conflict then success).

- [ ] **Step 12: Commit**

```bash
git add src/ldap/worker.rs src/workflows/write_flow.rs tests/live_write.rs
git commit -m "feat(ldap): assert entryCSN + post-read on MODIFY; add WriteConflict"
```

---

## Task 5: Assertion + Post-Read on DELETE (worker)

**Files:**
- Modify: `src/ldap/worker.rs` — `Request::Delete` (~160-166); `run_delete` (~632-634); `Delete` handler (~481-483)
- Test: `tests/live_write.rs`

**Interfaces:**
- Consumes: `write_response`, `post_read_csn`, `Response::WriteConflict` (Task 4).
- Produces: `Request::Delete { id, dn, assert_csn: Option<String> }`.

- [ ] **Step 1: Add `assert_csn` to `Request::Delete`** — worker.rs ~160-166:

```rust
    Delete {
        /// Correlation id.
        id: u64,
        /// DN to delete.
        dn: String,
        /// When set, assert `(entryCSN=<this>)` (RFC 4528, critical) so the delete
        /// applies only if the entry is unchanged since it was read. `None` = blind.
        assert_csn: Option<String>,
    },
```

- [ ] **Step 2: Rewrite `run_delete`** — worker.rs ~632-634:

```rust
fn run_delete(conn: &mut LdapConn, id: u64, dn: &str, assert_csn: Option<&str>) -> Response {
    let Some(csn) = assert_csn else {
        return write_response(id, dn, None, conn.delete(dn));
    };
    let filter = format!("(entryCSN={})", ldap3::ldap_escape(csn));
    let ctrl: RawControl = Assertion { filter }.critical().into();
    match conn.with_controls(vec![ctrl]).delete(dn) {
        Ok(r) if r.rc == 0 => Response::WriteOk {
            id,
            dn: dn.to_string(),
            new_csn: None,
        },
        Ok(r) if r.rc == 122 => Response::WriteConflict {
            id,
            dn: dn.to_string(),
        },
        Ok(r) => Response::WriteError {
            id,
            msg: result_code_message(r.rc, &r.text),
        },
        Err(e) => Response::WriteError {
            id,
            msg: format!("{e}"),
        },
    }
}
```

- [ ] **Step 3: Update the `Delete` handler** — worker.rs ~481-483:

```rust
            Request::Delete { id, dn, assert_csn } => {
                let _ = reply.send(run_delete(conn, id, &dn, assert_csn.as_deref()));
            }
```

- [ ] **Step 4: Fix `Request::Delete` construction sites**

Run: `grep -rn "Request::Delete" src/ tests/`
Add `assert_csn: None,` to each builder (there are no production callers yet per the delete spec being shelved; live tests may build one — set `None` unless the test is specifically for the assertion).

- [ ] **Step 5: Run + clippy**

Run: `cargo test -j4 -p edaptor --lib 2>&1 | tail -15 && cargo clippy --all-targets -- -D warnings 2>&1 | tail -15`
Expected: PASS, no warnings.

- [ ] **Step 6: Commit**

```bash
git add src/ldap/worker.rs tests/live_write.rs
git commit -m "feat(ldap): assert entryCSN on DELETE"
```

---

## Task 6: Thread the CSN through the write flow

`WriteFlow::submit` must pass the form's `baseline_csn` into `Request::Modify.assert_csn`; `on_response` must turn `WriteConflict` into a new `WriteOutcome::Conflict` and carry `new_csn` through `Saved`. The rebase/prompt decision itself lives in state (Task 7); here we only plumb.

**Files:**
- Modify: `src/workflows/write_flow.rs` — `submit` signature + `SavePlan::Modify` arm (~332-355); `WriteOutcome` (add `Conflict`); `on_response` (`WriteOk` arm ~610, add `WriteConflict` arm); `WriteIntent::Save` (add CSN passthrough if needed for reread)
- Modify: `src/ui/state.rs` — the call site of `write_flow.submit(...)` (pass the CSN)
- Test: `src/workflows/write_flow.rs` `#[cfg(test)]` (uses `WorkerHandle::recording()` + `insert_*_intent_for_test`)

**Interfaces:**
- Consumes: `Response::WriteConflict`, `Response::WriteOk.new_csn` (Task 4); `EditForm.baseline_csn` (Task 3).
- Produces: `WriteOutcome::Conflict { dn, quit_after }`; `WriteFlow::submit(.., assert_csn: Option<String>, ..)`.

- [ ] **Step 1: Write the failing test** — in `write_flow.rs` tests, assert that a submitted Modify carries the CSN and that a `WriteConflict` maps to `Conflict`. Use the existing `recording()` harness pattern (see the file's other tests for the exact setup):

```rust
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
```

(If `insert_save_intent_for_test` does not exist, add it next to the other `insert_*_intent_for_test` helpers, mirroring their shape: alloc an id, insert `WriteIntent::Save { reread_dn, quit_after }`, return the id.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -j4 -p edaptor --lib write_flow 2>&1 | tail -20`
Expected: FAIL — `submit` has 4 params, no `Conflict` variant.

- [ ] **Step 3: Add `WriteOutcome::Conflict`** — write_flow.rs, in the `WriteOutcome` enum (~108-141):

```rust
    /// A MODIFY/DELETE was refused because the entry changed since it was read
    /// (rc 122). The caller must re-read `dn` and decide rebase-vs-prompt.
    Conflict { dn: String, quit_after: bool },
```

- [ ] **Step 4: Add `assert_csn` param to `submit`** — write_flow.rs ~332-338, and pass it into the `SavePlan::Modify` arm's `Request::Modify`:

```rust
    pub fn submit(
        &mut self,
        worker: &WorkerHandle,
        plan: SavePlan,
        old_dn: &str,
        assert_csn: Option<String>,
        quit_after: bool,
    ) -> Result<()> {
```

In the `SavePlan::Modify(mods)` arm (~341-347):

```rust
            SavePlan::Modify(mods) => {
                let id = self.alloc();
                worker.submit(Request::Modify {
                    id,
                    dn: old_dn.to_string(),
                    changes: mods,
                    assert_csn,
                })?;
```

(The `RenameOnly`/`Rename` arms use `Request::ModRdn`, which has no CSN assertion in this round — leave them unchanged. MODRDN conflict protection is out of scope for Spec 1.)

- [ ] **Step 5: Handle `WriteConflict` and `new_csn` in `on_response`** — write_flow.rs. Update the `WriteOk` arm (~610) to ignore `new_csn` for the `Save` intent's outcome mapping (the outcome already carries `reread_dn`; the reread refreshes the baseline including CSN, so `new_csn` is not strictly needed for `Save` — bind it with `..`). Add a new top-level arm for `WriteConflict` beside the `WriteError` arm (~673):

```rust
            Response::WriteConflict { id, dn } => match self.pending.remove(id) {
                Some(WriteIntent::Save { quit_after, .. }) => WriteOutcome::Conflict {
                    dn: dn.clone(),
                    quit_after,
                },
                Some(WriteIntent::CombinedLeg { batch_id, .. }) => {
                    self.batches.remove(&batch_id);
                    WriteOutcome::Conflict {
                        dn: dn.clone(),
                        quit_after: false,
                    }
                }
                Some(_) => WriteOutcome::Conflict {
                    dn: dn.clone(),
                    quit_after: false,
                },
                None => WriteOutcome::Ignored,
            },
```

Ensure the `WriteOk` match binds the new field: `Response::WriteOk { id, dn: resp_dn, new_csn: _ } => ...`.

- [ ] **Step 6: Update the state call site** — `src/ui/state.rs`, wherever `write_flow.submit(...)` is called for a save. Pass the form's CSN, but only when the server supports the control (else `None` → blind path):

```rust
        let assert_csn = if self.assertion_supported {
            self.edit_form.as_ref().and_then(|f| f.baseline_csn.clone())
        } else {
            None
        };
        // ... existing split-borrow of worker/write_flow ...
        write_flow.submit(worker, plan, &old_dn, assert_csn, quit_after)?;
```

Run: `grep -rn "write_flow.submit(" src/ui/` to find the exact call and adapt to its surrounding borrow idiom.

- [ ] **Step 7: Run tests + clippy**

Run: `cargo test -j4 -p edaptor --lib write_flow 2>&1 | tail -20 && cargo clippy --all-targets -- -D warnings 2>&1 | tail -15`
Expected: PASS, no warnings.

- [ ] **Step 8: Commit**

```bash
git add src/workflows/write_flow.rs src/ui/state.rs
git commit -m "feat(write): thread entryCSN assertion through save; add Conflict outcome"
```

---

## Task 7: Conflict handling — rebase-or-prompt + one-time warning

`apply_write_outcome` gains a `Conflict` arm. It re-reads the entry, compares the other party's changes against the attributes we are writing, and either resubmits silently (disjoint) or opens a conflict dialog (overlap). Also: when a save runs on the blind path (`!assertion_supported`) for the first time, show the one-time warning.

**Files:**
- Create: `src/ui/dialog/conflict.rs`
- Modify: `src/ui/dialog/mod.rs` (register module)
- Modify: `src/ui/state.rs` — `apply_write_outcome` (`Conflict` arm); the save dispatch (one-time warning); a helper to recompute the diff against a fresh baseline
- Modify: `src/ui/app.rs` — surface the conflict dialog from the dispatch layer (mirror the `last_write_error` → `error::build` path)
- Test: `src/ui/state.rs` `#[cfg(test)]` (pure overlap decision) using `pump_responses_for_test`

**Interfaces:**
- Consumes: `WriteOutcome::Conflict` (Task 6); `EditForm.baseline_csn`, `EditField.baseline`/`current_values` (existing); `assertion_supported`/`concurrency_warned` (Task 2).
- Produces: `conflict::build(text: &str) -> (Box<dyn View>, ViewId)`; `UiState::attrs_in_flight()` / overlap helper (below).

- [ ] **Step 1: Write the failing unit test for the overlap decision** — the load-bearing pure logic is "given the attributes we are writing and the attributes that changed on the server, do they overlap?". Add to `src/ui/state.rs` tests:

```rust
    #[test]
    fn conflict_overlap_detection() {
        // attrs we are writing vs attrs changed by the other client.
        let ours = ["description", "telephoneNumber"];
        let theirs_disjoint = ["mail"];
        let theirs_overlap = ["telephoneNumber"];
        assert!(!crate::ui::state::attrs_overlap(&ours, &theirs_disjoint));
        assert!(crate::ui::state::attrs_overlap(&ours, &theirs_overlap));
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -j4 -p edaptor --lib conflict_overlap_detection 2>&1 | tail -15`
Expected: FAIL — `attrs_overlap` not found.

- [ ] **Step 3: Add the pure overlap helper** — in `src/ui/state.rs` (module-level, case-insensitive attribute names):

```rust
/// True if any attribute name appears in both sets (case-insensitive). Used to
/// decide whether a concurrent modification can be silently rebased (disjoint)
/// or must be surfaced to the operator (overlap).
pub fn attrs_overlap(ours: &[&str], theirs: &[&str]) -> bool {
    ours.iter().any(|a| {
        theirs.iter().any(|b| a.eq_ignore_ascii_case(b))
    })
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -j4 -p edaptor --lib conflict_overlap_detection 2>&1 | tail -15`
Expected: PASS.

- [ ] **Step 5: Create the conflict dialog** — `src/ui/dialog/conflict.rs`, mirroring `error::build` but with Reload / Overwrite / Cancel. Reload → `Command::CANCEL` (drop our edit, re-read); Overwrite → a custom command; Cancel → keep editing:

```rust
//! The concurrent-modification dialog: shown when a save is refused because the
//! entry changed on the server since it was read (rc 122) and the change overlaps
//! the attributes we are writing. Offers Reload (discard our edit and re-read),
//! Overwrite (re-assert against the new version, keeping our values), or Cancel.

use tvision as tv;
use tv::views::{Dialog, StaticText};
use tv::{ButtonFlags, ButtonRowAlign, Command, Rect, View, ViewId};

/// Custom command returned when the operator chooses to overwrite (re-apply our
/// edit on top of the other client's version).
pub const OVERWRITE: Command = Command::custom("edaptor.conflict.overwrite");

/// Build the conflict dialog. Returns the view and the button id to focus (Reload,
/// the safe default). Reload → `Command::CANCEL`; Overwrite → [`OVERWRITE`];
/// Cancel → `Command::custom("edaptor.conflict.keep")`.
pub fn build(text: &str) -> (Box<dyn View>, ViewId) {
    let mut dlg = Dialog::new(Rect::new(0, 0, 64, 12), Some("Entry changed".to_string()));
    dlg.state_mut().options.center_x = true;
    dlg.state_mut().options.center_y = true;
    dlg.insert_child(Box::new(StaticText::new(Rect::new(2, 2, 62, 8), text.to_string())));
    let ids = dlg.button_row(
        &[
            ("~R~eload", Command::CANCEL, ButtonFlags { default: true, ..ButtonFlags::new() }),
            ("~O~verwrite", OVERWRITE, ButtonFlags::new()),
            ("~C~ancel", Command::custom("edaptor.conflict.keep"), ButtonFlags::new()),
        ],
        ButtonRowAlign::Center,
    );
    (Box::new(dlg), ids[0])
}
```

(Confirm the exact `tvision` import paths and `Dialog`/`StaticText`/`button_row` signatures against `src/ui/dialog/error.rs` — copy its `use` block verbatim rather than trusting the above.)

- [ ] **Step 6: Register the module** — `src/ui/dialog/mod.rs`, add beside the others:

```rust
pub mod conflict;
```

- [ ] **Step 7: Add the `Conflict` arm to `apply_write_outcome`** — `src/ui/state.rs`, beside the `Saved`/`Error` arms (~536-561). The silent-rebase path re-reads and resubmits; the overlap path defers to the dispatch layer by stashing a message (like `last_write_error`). Add a field `pub last_conflict: Option<ConflictPrompt>` to `UiState` (with `pub struct ConflictPrompt { pub dn: String, pub text: String, pub quit_after: bool }`), then:

```rust
            WriteOutcome::Conflict { dn, quit_after } => {
                // Re-read the entry fresh to learn its new baseline + which attrs
                // the other client changed.
                match self.reread_blocking_for_conflict(&dn) {
                    Some((fresh_baseline, changed_attrs, fresh_csn)) => {
                        let ours = self.attrs_in_flight();
                        let ours_refs: Vec<&str> = ours.iter().map(String::as_str).collect();
                        let theirs_refs: Vec<&str> =
                            changed_attrs.iter().map(String::as_str).collect();
                        if !attrs_overlap(&ours_refs, &theirs_refs) {
                            // Disjoint → rebase silently: adopt the fresh CSN and
                            // resubmit our unchanged edit against it.
                            if let Some(f) = self.edit_form.as_mut() {
                                f.baseline_csn = fresh_csn;
                                rebase_baselines(f, &fresh_baseline);
                            }
                            self.resubmit_save(quit_after);
                        } else {
                            self.last_conflict = Some(ConflictPrompt {
                                dn: dn.clone(),
                                text: format!(
                                    "This entry was changed by someone else since you \
                                     opened it. Conflicting attribute(s): {}.\n\n\
                                     Reload to discard your edits, Overwrite to force \
                                     your version, or Cancel to keep editing.",
                                    changed_attrs.join(", ")
                                ),
                                quit_after,
                            });
                            out.error = true;
                        }
                    }
                    None => {
                        self.last_write_error = Some(
                            "Entry changed on the server and could not be re-read.".to_string(),
                        );
                        out.error = true;
                    }
                }
            }
```

This references three new `UiState` methods you must add in this task: `attrs_in_flight()` (the labels of fields whose `current_values() != baseline` — reuse the existing dirty logic), `reread_blocking_for_conflict(dn)` (a synchronous base read requesting `*` + `entryCSN`, returning the fresh per-attr values, the list of attribute names differing from the form's stored baseline, and the fresh CSN), and `resubmit_save`/`rebase_baselines` (re-run the existing save-prepare+submit against the current form; overwrite each field's `baseline` with the server's fresh values so only our edits diff). Model `attrs_in_flight` on `EditForm::is_dirty` (edit_form.rs:118), which already walks fields comparing `current_values()` to `baseline`.

- [ ] **Step 8: One-time blind-write warning** — in the save dispatch (where `write_flow.submit` is called, Task 6 Step 6), before submitting on the blind path:

```rust
        if !self.assertion_supported && !self.concurrency_warned {
            self.status =
                "Server does not support optimistic concurrency; concurrent edits may be lost."
                    .to_string();
            self.concurrency_warned = true;
        }
```

- [ ] **Step 9: Surface the conflict dialog from dispatch** — `src/ui/app.rs`, wherever `state.last_write_error` is drained into `error::build` (the `out.error` path). Add, before/after that, a drain of `last_conflict`:

```rust
        if let Some(c) = state.borrow_mut().last_conflict.take() {
            let (view, focus) = crate::ui::dialog::conflict::build(&c.text);
            match prog.exec_view_focused(view, focus) {
                crate::ui::dialog::conflict::OVERWRITE => {
                    // Force our version: adopt the fresh CSN captured on re-read,
                    // then resubmit. (The Conflict arm already refreshed baselines;
                    // overwrite re-runs the save keeping our values.)
                    force_overwrite(prog, state, &c.dn, c.quit_after);
                }
                Command::CANCEL => {
                    // Reload: discard edits, re-read the entry fresh.
                    state.borrow_mut().reread(&c.dn, &[]);
                }
                _ => { /* keep editing: do nothing */ }
            }
        }
```

Implement `force_overwrite` as: set the form's `baseline_csn` to the freshly-read CSN (already stored during the Conflict arm) and call the same save path used by `SAVE`, so the re-assertion now matches. Follow `do_create`'s borrow discipline (plan under borrow, drop before `exec_view_focused`, re-borrow to submit).

- [ ] **Step 10: Write an end-to-end state test** — using `pump_responses_for_test` (state.rs:308) and `insert_save_intent_for_test`, drive a `Response::WriteConflict` and assert that a disjoint change resubmits (a new Modify is queued / no `last_conflict`) while an overlapping change sets `last_conflict`. Model it on the existing `pump_responses_for_test` tests. Because the rebase path needs a re-read, this test may need the `worker: None` headless branch to short-circuit `reread_blocking_for_conflict` — if so, assert the overlap branch (which does not require the worker) and cover the disjoint branch in the live test below.

- [ ] **Step 11: Run everything**

Run: `make check 2>&1 | tail -30`
Expected: fmt clean, clippy no warnings, all tests pass.

- [ ] **Step 12: Manual live check** — with the server up, run the TUI (`EDAPTOR_TEST_ADMIN_PW=adminpassword cargo run -- --config examples/demo-config.toml`), open an entry, and from a second shell modify the same entry's `description` via `ldapmodify`, then edit a *different* field in eDAPtor and Save (expect silent success — rebased), then repeat editing the *same* field (expect the conflict dialog). Note the result in the commit message.

- [ ] **Step 13: Commit**

```bash
git add src/ui/dialog/conflict.rs src/ui/dialog/mod.rs src/ui/state.rs src/ui/app.rs
git commit -m "feat(ui): rebase-or-prompt on concurrent modification; one-time blind-write warning"
```

---

## Task 8: Assertion on membership fan-out legs

The combined membership save (`submit_combined`) fires one MODIFY per touched group with blind `Add`/`Delete`. Concurrent membership change currently errors mid-batch. Give each group leg its own assertion so a conflict is reported as a `Conflict` (already handled by Task 6's `CombinedLeg` → `Conflict` mapping) rather than a raw `noSuchAttribute`/`attributeOrValueExists`.

**Files:**
- Modify: `src/workflows/write_flow.rs` — `submit_combined` (~506-584): request each group's `entryCSN` when building the leg, attach it as `assert_csn`.
- Test: `src/workflows/write_flow.rs` `#[cfg(test)]`

**Interfaces:**
- Consumes: `Request::Modify.assert_csn` (Task 4); `fetch_group_members_for_must` pattern (existing blocking group read).
- Produces: no new public types.

- [ ] **Step 1: Decide the CSN source** — the group DNs are known at fan-out time but their `entryCSN` is not loaded. Extend the existing blocking pre-read: `fetch_group_members_for_must` already does a Base search per group. Add a sibling `fetch_group_csns(worker, fanout) -> HashMap<String, String>` that Base-reads each group's `entryCSN`. (Keep it separate; do not overload the MUST-check fetch.)

- [ ] **Step 2: Write the failing test** — assert each combined leg carries the group's CSN:

```rust
    #[test]
    fn combined_legs_carry_group_csn() {
        let (worker, rx) = WorkerHandle::recording();
        let mut wf = WriteFlow::new();
        let mut csns = std::collections::HashMap::new();
        csns.insert("cn=staff,dc=example,dc=org".to_string(), "G-CSN-1".to_string());
        // Build a combined save with one own-MODIFY + one group Add leg for cn=staff,
        // passing `csns` as the new group-CSN map argument to submit_combined.
        // Then drain rx and assert the group's Request::Modify has assert_csn = Some("G-CSN-1").
        // (Model the setup on the existing submit_combined test in this file.)
    }
```

Fill in against the existing `submit_combined` test's construction.

- [ ] **Step 3: Thread the CSN map into `submit_combined`** — add a `group_csns: &HashMap<String, String>` parameter; for each per-group `Request::Modify`, set `assert_csn: group_csns.get(&group_dn).cloned()`. The own-entry MODIFY leg uses the form's `baseline_csn` as in Task 6.

- [ ] **Step 4: Wire the fetch at the call site** — in `src/ui/state.rs` where the combined save is launched, call `fetch_group_csns` (blocking, like the existing `fetch_group_members_for_must` call) and pass the map in.

- [ ] **Step 5: Run + clippy**

Run: `cargo test -j4 -p edaptor --lib write_flow 2>&1 | tail -20 && cargo clippy --all-targets -- -D warnings 2>&1 | tail -15`
Expected: PASS, no warnings.

- [ ] **Step 6: Commit**

```bash
git add src/workflows/write_flow.rs src/ui/state.rs
git commit -m "feat(write): assert group entryCSN on membership fan-out legs"
```

---

## Task 9: Documentation

**Files:**
- Modify: `CHANGES.md` (unreleased section)
- Create: `docs/src/concepts/optimistic-concurrency.md`
- Modify: `docs/src/SUMMARY.md` (add the page to the ToC)
- Modify: `docs/src/concepts/ldap-constraints.md` (cross-reference)

**Interfaces:** none (docs).

- [ ] **Step 1: CHANGES.md** — add under the current unreleased heading:

```markdown
- **Optimistic concurrency for edits.** Saves and deletes now attach the entry's
  `entryCSN` as an RFC 4528 assertion, so a write is refused (rather than silently
  overwriting) if another client changed the entry since it was loaded. On such a
  conflict eDAPtor rebases and retries automatically when the changes do not
  overlap your edits, and prompts (Reload / Overwrite / Cancel) when they do.
  Against a directory that does not advertise the assertion control, edits fall
  back to the previous behaviour with a one-time warning.
```

- [ ] **Step 2: Concept page** — write `docs/src/concepts/optimistic-concurrency.md` explaining: the lost-update problem; `entryCSN` as the version token (note it is finer-grained than `modifyTimestamp`); the assertion + post-read mechanism; the rebase-vs-prompt rule; and the capability fallback. Keep it conceptual, not code.

- [ ] **Step 3: Add to SUMMARY.md** — under the Concepts section, add:

```markdown
- [Optimistic concurrency](concepts/optimistic-concurrency.md)
```

- [ ] **Step 4: Cross-reference** — in `docs/src/concepts/ldap-constraints.md`, add a line pointing to the new page where it discusses live-vs-cached data / syncrepl.

- [ ] **Step 5: Build the docs**

Run: `make docs 2>&1 | tail -15`
Expected: mdBook builds with no broken-link errors.

- [ ] **Step 6: Commit**

```bash
git add CHANGES.md docs/src/
git commit -m "docs: document optimistic-concurrency writes"
```

---

## Self-Review

**Spec coverage (against `2026-07-21-realtime-consistency-design.md` §"Spec 1"):**
- entryCSN captured at read → Task 3. ✓
- Assertion (critical) + Post-Read on MODIFY → Task 4; DELETE → Task 5. ✓
- rc 122 mapped → Task 1; rebase-on-no-overlap, prompt-on-overlap → Task 7. ✓
- Blind rebase-and-retry explicitly rejected → Task 7 only rebases on *disjoint* changes. ✓
- Capability probe + one-time warning + no critical assertion when unsupported → Task 2 (probe/flag), Task 6 Step 6 (`None` on blind path → no control attached), Task 7 Step 8 (warning). ✓
- Membership fan-out legs get assertions → Task 8. ✓
- Touch points listed in the spec (worker, result.rs, read_flow, edit_form, save/write_flow, state, new dialog) → all covered. ✓

**Placeholder scan:** The two live-test bodies (Task 4 Step 10, Task 8 Step 2) and the Task 7 Step 10 state test intentionally defer to "the existing test's fixture" rather than inventing a harness — this is a real constraint (the `recording()`/live fixtures are pre-existing and must be matched), not a hand-wave; each names the exact existing test to copy and the exact assertion to make. Acceptable.

**Type consistency:** `assert_csn: Option<String>` on both `Request::Modify` (Task 4) and `Request::Delete` (Task 5); `new_csn: Option<String>` on `WriteOk`; `WriteConflict { id, dn }` produced in Task 4, consumed in Task 6; `WriteOutcome::Conflict { dn, quit_after }` produced in Task 6, consumed in Task 7; `EditForm.baseline_csn: Option<String>` produced in Task 3, consumed in Tasks 6–7; `attrs_overlap`/`assertion_supported` signatures consistent. ✓

**Ordering:** Task 1 (messages) and Task 2 (probe) are independent; 3 (read) precedes 4/6/7 (which need `baseline_csn`); 4 precedes 5/6; 6 precedes 7; 8 depends on 4+6; 9 last. A blocked reviewer could reject any single task without unwinding its neighbours.

**Known risk to flag at execution:** whether the server honours Post-Read *inside* the write is verified for plain MODIFY (test server advertises `1.3.6.1.1.13.2`); combining assertion+post-read under an RFC 5805 transaction is NOT covered here (companion create stays on its existing path). If a future task combines them, add a live test first.
