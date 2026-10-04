//! `_core.HintSession`: `dagayn.hints`' session, kept in `dagayn-tools` so the
//! tools `dagayn serve` answers in Rust and the Python server record into
//! one state.

use std::collections::HashSet;

use dagayn_tools::hints;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

/// A handle on the process's session, with `dagayn.hints.SessionState`'s
/// attributes and methods. Every handle sees the same state.
#[pyclass(name = "HintSession")]
pub(crate) struct PyHintSession;

#[pymethods]
impl PyHintSession {
    #[new]
    fn new() -> Self {
        Self
    }

    /// The recorded tool names, oldest first (capped at 100).
    #[getter]
    fn tools_called(&self) -> Vec<String> {
        hints::session().tools_called.iter().cloned().collect()
    }

    #[getter]
    fn nodes_queried(&self) -> HashSet<String> {
        hints::session().nodes_queried.clone()
    }

    #[getter]
    fn files_touched(&self) -> HashSet<String> {
        hints::session().files_touched.clone()
    }

    #[getter]
    fn inferred_intent(&self) -> Option<String> {
        hints::session().inferred_intent.clone()
    }

    #[setter]
    fn set_inferred_intent(&self, intent: Option<String>) {
        hints::session().inferred_intent = intent;
    }

    #[getter]
    fn last_tool_time(&self) -> f64 {
        hints::session().last_tool_time
    }

    fn record_tool_call(&self, tool_name: &str) {
        hints::session().record_tool_call(tool_name);
    }

    fn record_nodes(&self, node_ids: Vec<String>) {
        hints::session().record_nodes(node_ids.iter().map(String::as_str));
    }

    fn record_files(&self, files: Vec<String>) {
        hints::session().record_files(files.iter().map(String::as_str));
    }

    /// Rust's `generate_hints` on this session, for the tools `dagayn serve`
    /// answers in Rust; `exposed` is the tool surface, `None` for all tools.
    #[pyo3(signature = (tool_name, result_json, exposed=None))]
    fn generate_hints(
        &self,
        tool_name: &str,
        result_json: &str,
        exposed: Option<HashSet<String>>,
    ) -> PyResult<String> {
        let result: serde_json::Value = serde_json::from_str(result_json)
            .map_err(|err| PyValueError::new_err(err.to_string()))?;
        let is_exposed = |tool: &str| exposed.as_ref().is_none_or(|names| names.contains(tool));
        let hints = hints::generate_hints(tool_name, &result, &mut hints::session(), &is_exposed);
        Ok(hints.to_string())
    }
}

/// Clear the process's session (`dagayn.hints.reset_session`).
#[pyfunction]
pub(crate) fn reset_hint_session() {
    hints::reset_session();
}
