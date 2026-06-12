//! edaptor — a schema-driven OpenLDAP TUI. M1 exposes a headless check pipeline;
//! the ratatui UI adds the three-pane browser (tree / leaf list / entry form).

pub mod app;
pub mod config;
pub mod form;
pub mod ldap;
pub mod passwd;
pub mod samba;
pub mod schema;
pub mod testdata;
pub mod ui;
pub mod workflows;

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Result};

use crate::config::Config;
use crate::ldap::worker::{RawSubschema, Request, Response, SearchScope, WorkerHandle};
use crate::schema::{FieldKind, SchemaModel};

/// Result of the M1 connectivity + schema-fetch check.
pub struct CheckSummary {
    pub uri: String,
    pub bind_dn: Option<String>,
    pub object_class_count: usize,
    pub attribute_type_count: usize,
    pub ldap_syntax_count: usize,
}

/// Connect, bind, and fetch the raw subschema. Shared by run_check / run_schema.
fn fetch_raw(config: Config, password: String) -> Result<RawSubschema> {
    let handle = WorkerHandle::spawn(config, password)?;
    match handle.request(Request::FetchSubschema)? {
        Response::Subschema(raw) => Ok(raw),
        Response::Error(e) => Err(anyhow!(e)),
        other => Err(anyhow!(
            "unexpected response to FetchSubschema: {}",
            describe_response(&other)
        )),
    }
}

/// A short label for an unexpected [`Response`] variant, for diagnostics.
fn describe_response(resp: &Response) -> &'static str {
    match resp {
        Response::Subschema(_) => "Subschema",
        Response::Entries { .. } => "Entries",
        Response::SearchError { .. } => "SearchError",
        Response::StructureEntries { .. } => "StructureEntries",
        Response::StructureError { .. } => "StructureError",
        Response::WriteOk { .. } => "WriteOk",
        Response::WriteError { .. } => "WriteError",
        Response::Done => "Done",
        Response::Error(_) => "Error",
    }
}

/// Connect, bind, fetch the raw subschema, and summarize counts.
pub fn run_check(config: Config, password: String) -> Result<CheckSummary> {
    let uri = config.server.uri.clone();
    let bind_dn = config.auth.bind_dn.clone();
    let raw = fetch_raw(config, password)?;
    Ok(CheckSummary {
        uri,
        bind_dn,
        object_class_count: raw.object_classes.len(),
        attribute_type_count: raw.attribute_types.len(),
        ldap_syntax_count: raw.ldap_syntaxes.len(),
    })
}

/// One attribute of a resolved object class.
pub struct SchemaAttrReport {
    pub name: String,
    pub required: bool,
    pub kind: FieldKind,
    pub single_value: bool,
}

/// The effective attribute set of an object class, for display.
pub struct SchemaReport {
    pub object_class: String,
    pub attributes: Vec<SchemaAttrReport>,
    pub parse_warnings: usize,
}

/// Fetch the schema and resolve the effective attributes of one object class.
pub fn run_schema(config: Config, password: String, object_class: &str) -> Result<SchemaReport> {
    let raw = fetch_raw(config, password)?;
    let model = SchemaModel::from_raw(&raw);
    if model.object_class(object_class).is_none() {
        return Err(anyhow!(
            "object class '{object_class}' not found in the server schema"
        ));
    }
    let resolved = model.effective_attributes(&[object_class]);

    let mut attributes = Vec::new();
    for name in &resolved.must {
        attributes.push(make_row(&model, name, true));
    }
    for name in &resolved.may {
        attributes.push(make_row(&model, name, false));
    }

    Ok(SchemaReport {
        object_class: object_class.to_string(),
        attributes,
        parse_warnings: model.warnings.len(),
    })
}

fn make_row(model: &SchemaModel, name: &str, required: bool) -> SchemaAttrReport {
    let single_value = model
        .attribute_type(name)
        .map(|at| at.single_value)
        .unwrap_or(false);
    SchemaAttrReport {
        name: name.to_string(),
        required,
        kind: model.field_kind(name),
        single_value,
    }
}

/// Current Unix time in whole seconds (for `sambaPwdLastSet`). Injected into the
/// pure `build_password_mods` so the builder itself never touches the clock.
fn now_unix_secs() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

/// True when `object_classes` contains `sambaSamAccount` (case-insensitive). Pure
/// so the samba-detection decision is unit-testable without a server.
fn is_samba_account(object_classes: &[String]) -> bool {
    object_classes
        .iter()
        .any(|oc| oc.eq_ignore_ascii_case("sambaSamAccount"))
}

/// Set a synced Unix + Samba password on `target_dn` (spec §9/§10).
///
/// TLS-gated: refuses with an `Err` (before any network I/O) when the server
/// connection is not encrypted (`!samba::password::is_secure`). Then binds,
/// reads the target's `objectClass` to detect a `sambaSamAccount`, builds the
/// synced mod-set (`userPassword` always; `sambaNTPassword` + `sambaPwdLastSet`
/// for samba accounts), applies it in one atomic MODIFY, and re-reads the entry
/// to confirm (no silent success). Returns a human confirmation string.
///
/// Factored out of `main` (no `rpassword`, no terminal) so the live test can drive
/// it with a known password.
/// Search `base` (subtree/base scope per `scope`) for `filter`, returning the
/// matching entries with their `objectClass` values. The shared LDAP round-trip
/// behind both passwd-target resolution branches.
fn search_object_classes(
    worker: &WorkerHandle,
    base: &str,
    scope: SearchScope,
    filter: &str,
) -> Result<Vec<crate::ldap::worker::LdapEntry>> {
    match worker.request(Request::Search {
        id: 1,
        base: base.to_string(),
        scope,
        filter: filter.to_string(),
        attrs: vec!["objectClass".to_string()],
        size_limit: None,
    })? {
        Response::Entries { entries, .. } => Ok(entries),
        Response::SearchError { msg, .. } => Err(anyhow!("searching {base}: {msg}")),
        other => Err(anyhow!(
            "unexpected response searching {base}: {}",
            describe_response(&other)
        )),
    }
}

/// Resolve the `passwd` argument to a concrete `(dn, objectClass values)`. A DN
/// argument is base-read to confirm it exists; a bare username is searched across
/// every configured profile's `search_base`, requiring a single match.
fn resolve_passwd_target(
    worker: &WorkerHandle,
    profiles: &[crate::config::EntryProfile],
    arg: &str,
) -> Result<(String, Vec<String>)> {
    if passwd::looks_like_dn(arg) {
        let entry = search_object_classes(worker, arg, SearchScope::Base, "(objectClass=*)")?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("entry not found: {arg}"))?;
        let ocs = entry.attrs.get("objectClass").cloned().unwrap_or_default();
        return Ok((entry.dn, ocs));
    }

    // Bare username: gather candidates across every profile's search base.
    let mut matched: Vec<crate::ldap::worker::LdapEntry> = Vec::new();
    for (base, filter) in passwd::username_searches(profiles, arg) {
        matched.extend(search_object_classes(
            worker,
            &base,
            SearchScope::Subtree,
            &filter,
        )?);
    }
    let dns = matched.iter().map(|e| e.dn.clone()).collect();
    match passwd::resolve_outcome(dns) {
        passwd::Resolution::Unique(dn) => {
            let ocs = matched
                .into_iter()
                .find(|e| e.dn.eq_ignore_ascii_case(&dn))
                .and_then(|e| e.attrs.get("objectClass").cloned())
                .unwrap_or_default();
            Ok((dn, ocs))
        }
        passwd::Resolution::NotFound => Err(anyhow!(
            "no entry found for username \"{arg}\" in any configured profile; \
             pass a full DN instead"
        )),
        passwd::Resolution::Ambiguous(dns) => Err(anyhow!(
            "username \"{arg}\" matches multiple entries:\n  {}\npass a full DN instead",
            dns.join("\n  ")
        )),
    }
}

pub fn run_passwd(
    config: Config,
    bind_password: String,
    target_arg: &str,
    prompt: impl FnOnce(&str) -> Result<String>,
) -> Result<String> {
    // TLS gate FIRST — before spawning the worker or touching the network.
    if !samba::password::is_secure(&config.server) {
        return Err(anyhow!(
            "refusing to set a password over an unencrypted connection: \
             use ldaps:// or enable start_tls in [server]"
        ));
    }

    // Profiles are needed for username resolution after `config` moves into the
    // worker, so capture them first.
    let profiles = config.profiles.clone();
    let worker = WorkerHandle::spawn(config, bind_password)?;

    // Resolve the target to a concrete DN (and its objectClass values, used for
    // samba detection) BEFORE prompting for the new password, so an unknown or
    // ambiguous username fails fast instead of after two password entries.
    let (target_dn, object_classes) = resolve_passwd_target(&worker, &profiles, target_arg)?;
    let is_samba = is_samba_account(&object_classes);

    // Now that the target is known, prompt for the new password.
    let new_password = prompt(&target_dn)?;
    let target_dn = target_dn.as_str();

    // Domain discovery is intentionally omitted from the password path:
    // `build_password_mods` derives `sambaNTPassword` from the cleartext alone and
    // never needs the domain SID / RID base (those are only required when
    // *creating* a samba account, not when re-setting its password). Keeping the
    // flow free of best-effort discovery keeps it correct and simple.

    let mods = samba::password::build_password_mods(&new_password, is_samba, now_unix_secs()?);

    match worker.request(Request::Modify {
        id: 2,
        dn: target_dn.to_string(),
        changes: mods,
    })? {
        Response::WriteOk { .. } => {}
        Response::WriteError { msg, .. } => return Err(anyhow!(msg)),
        other => {
            return Err(anyhow!(
                "unexpected response modifying {target_dn}: {}",
                describe_response(&other)
            ))
        }
    }

    // Re-read the entry to confirm it still resolves (no silent success).
    match worker.request(Request::Search {
        id: 3,
        base: target_dn.to_string(),
        scope: SearchScope::Base,
        filter: "(objectClass=*)".to_string(),
        attrs: vec!["objectClass".to_string()],
        size_limit: None,
    })? {
        Response::Entries { entries, .. } if !entries.is_empty() => {}
        Response::Entries { .. } => return Err(anyhow!("entry vanished after write: {target_dn}")),
        Response::SearchError { msg, .. } => return Err(anyhow!(msg)),
        other => {
            return Err(anyhow!(
                "unexpected response re-reading {target_dn}: {}",
                describe_response(&other)
            ))
        }
    }

    Ok(format!(
        "Password updated for {target_dn} (samba: {})",
        if is_samba { "yes" } else { "no" }
    ))
}

#[cfg(test)]
mod tests {
    use super::is_samba_account;

    #[test]
    fn detects_samba_account_case_insensitively() {
        assert!(is_samba_account(&[
            "top".to_string(),
            "person".to_string(),
            "sambaSamAccount".to_string(),
        ]));
        // Case-insensitive: servers may echo a different case.
        assert!(is_samba_account(&["SAMBASAMACCOUNT".to_string()]));
        assert!(is_samba_account(&["sambasamaccount".to_string()]));
    }

    #[test]
    fn non_samba_entry_is_not_detected() {
        assert!(!is_samba_account(&[
            "top".to_string(),
            "inetOrgPerson".to_string(),
        ]));
        assert!(!is_samba_account(&[]));
    }
}
