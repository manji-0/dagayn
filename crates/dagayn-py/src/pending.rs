//! `_core.pending_refactor_*`: the pending refactor store kept in
//! `dagayn-tools`, so a `rename` preview answered in Rust is one the Python
//! `apply_refactor_tool` can apply. Values cross as JSON text.

use dagayn_tools::pending;
use pyo3::prelude::*;

#[pyfunction]
pub(crate) fn pending_refactor_set(refactor_id: &str, payload_json: String) {
    pending::set(refactor_id, payload_json);
}

#[pyfunction]
pub(crate) fn pending_refactor_get(refactor_id: &str) -> Option<String> {
    pending::get(refactor_id)
}

#[pyfunction]
pub(crate) fn pending_refactor_remove(refactor_id: &str) -> Option<String> {
    pending::remove(refactor_id)
}

#[pyfunction]
pub(crate) fn pending_refactor_keys() -> Vec<String> {
    pending::keys()
}

#[pyfunction]
pub(crate) fn pending_refactor_clear() {
    pending::clear();
}
