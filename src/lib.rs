//! edaptor — a schema-driven OpenLDAP TUI. M1 exposes a headless check pipeline;
//! M3 adds the read-only TUI shell, browser, and entry form.

pub mod app;
pub mod config;
pub mod form;
pub mod ldap;
pub mod schema;
pub mod ui;
pub mod workflows;

use anyhow::{anyhow, Result};

use crate::config::Config;
use crate::ldap::worker::{RawSubschema, Request, Response, WorkerHandle};
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
