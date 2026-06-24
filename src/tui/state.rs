//! Shared application state for the tvision UI and the blocking bootstrap that
//! builds it. State is held in `Rc<RefCell<UiState>>` (alias `Shared` in mod.rs).

use anyhow::{anyhow, Result};

use crate::config::tree_label::CompiledTreeRule;
use crate::config::{Config, EntryProfile};
use crate::ldap::worker::{Request, Response, WorkerHandle};
use crate::schema::SchemaModel;
use crate::workflows::form_model::FormModel;
use crate::workflows::labels::LabelRule;
use crate::workflows::read_flow::ReadFlow;
use crate::workflows::structure::Structure;
#[cfg(test)]
use crate::workflows::structure::StructureInput;

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
    /// DFS pre-order index → branch DN, matching `Outline`'s `foc` numbering.
    pub branch_dns: Vec<String>,
    pub current_branch: Option<String>,
    pub current_leaf: Option<String>,
    pub search: String,
    /// The loaded read-only form (None until a leaf is read).
    pub form: Option<FormModel>,
    pub list_dirty: bool,
    pub form_dirty: bool,
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
            branch_dns: Vec::new(),
            current_branch: None,
            current_leaf: None,
            search: String::new(),
            form: None,
            list_dirty: false,
            form_dirty: false,
        }
    }
}

/// Blocking startup: spawn the worker, fetch schema + eager structure, build the
/// compiled label rules and the ReadFlow. Mirrors `ui::app::run`'s bootstrap.
pub(crate) fn bootstrap(config: Config, password: String) -> Result<UiState> {
    use crate::workflows::labels::{label_rules, structure_inputs, structure_scan_attrs};
    let base_dn = config.server.base_dn.clone();
    let profiles = config.profiles.clone();
    let label_rules = label_rules(&profiles);
    let tree_rules = crate::config::tree_label::compile_tree_rules(&config.tree);
    // Fetch the attributes the label/tree templates reference, so labels render.
    let scan_attrs = structure_scan_attrs(&label_rules, &tree_rules);

    let worker = WorkerHandle::spawn(config, password)?;

    let raw = match worker.request(Request::FetchSubschema)? {
        Response::Subschema(raw) => raw,
        other => return Err(anyhow!("FetchSubschema: unexpected {other:?}")),
    };
    let schema = SchemaModel::from_raw(&raw);

    let nodes = match worker.request(Request::LoadStructure {
        id: 0,
        base: base_dn.clone(),
        page_size: 500,
        attrs: scan_attrs,
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
        branch_dns: Vec::new(),
        current_branch: None,
        current_leaf: None,
        search: String::new(),
        form: None,
        list_dirty: false,
        form_dirty: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ldap::worker::RawSubschema;
    use std::collections::BTreeMap;

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
    fn test_state_starts_empty() {
        let structure = Structure::build("dc=x", vec![si("dc=x", None)]);
        let schema = SchemaModel::from_raw(&RawSubschema::default());
        let st = UiState::new_for_test(structure, schema, "dc=x".into(), Vec::new(), Vec::new());
        assert!(st.current_leaf_dn().is_none());
        assert!(st.form.is_none());
        assert!(!st.list_dirty);
    }
}
