//! edaptor — a schema-driven OpenLDAP TUI. M1 exposes a headless check pipeline.

pub mod config;
pub mod ldap;
pub mod schema;

use anyhow::{anyhow, Result};

use crate::config::Config;
use crate::ldap::worker::{Request, Response, WorkerHandle};

/// Result of the M1 connectivity + schema-fetch check.
pub struct CheckSummary {
    pub uri: String,
    pub bind_dn: Option<String>,
    pub object_class_count: usize,
    pub attribute_type_count: usize,
    pub ldap_syntax_count: usize,
}

/// Connect, bind, fetch the raw subschema, and summarize. Drives both the CLI
/// and the integration test. The worker is shut down cleanly when `handle` drops.
pub fn run_check(config: Config, password: String) -> Result<CheckSummary> {
    let uri = config.server.uri.clone();
    let bind_dn = config.auth.bind_dn.clone();

    let handle = WorkerHandle::spawn(config, password)?;
    match handle.request(Request::FetchSubschema)? {
        Response::Subschema(raw) => Ok(CheckSummary {
            uri,
            bind_dn,
            object_class_count: raw.object_classes.len(),
            attribute_type_count: raw.attribute_types.len(),
            ldap_syntax_count: raw.ldap_syntaxes.len(),
        }),
        Response::Error(e) => Err(anyhow!(e)),
        Response::Done => Err(anyhow!("unexpected Done response to FetchSubschema")),
    }
}
