use std::sync::Mutex;

use dagayn_core::{EdgeInput, FileBatchItem, GraphStore as NativeGraphStore, NodeInput};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyIterator, PyModule};
use serde_json::Value;

#[pyclass(name = "GraphStore")]
struct PyGraphStore {
    inner: Mutex<NativeGraphStore>,
}

#[pymethods]
impl PyGraphStore {
    #[new]
    fn new(py: Python<'_>, db_path: &Bound<'_, PyAny>) -> PyResult<Self> {
        let os = PyModule::import(py, "os")?;
        let db_path: String = os.getattr("fspath")?.call1((db_path,))?.extract()?;
        let inner = NativeGraphStore::open(db_path).map_err(to_py_runtime_error)?;
        Ok(Self {
            inner: Mutex::new(inner),
        })
    }

    fn set_metadata(&self, key: &str, value: &str) -> PyResult<()> {
        self.with_store(|store| store.set_metadata(key, value))
    }

    fn get_metadata(&self, key: &str) -> PyResult<Option<String>> {
        self.with_store(|store| store.get_metadata(key))
    }

    #[pyo3(signature = (file_path, nodes, edges, fhash = ""))]
    fn store_file_nodes_edges(
        &self,
        py: Python<'_>,
        file_path: &str,
        nodes: &Bound<'_, PyAny>,
        edges: &Bound<'_, PyAny>,
        fhash: &str,
    ) -> PyResult<()> {
        let node_inputs = collect_nodes(py, nodes)?;
        let edge_inputs = collect_edges(py, edges)?;
        self.with_store_mut(|store| {
            store.store_file_nodes_edges(file_path, &node_inputs, &edge_inputs, fhash)
        })
    }

    fn get_all_files(&self) -> PyResult<Vec<String>> {
        self.with_store(|store| store.get_all_files())
    }

    fn get_file_hashes(
        &self,
        file_paths: Vec<String>,
    ) -> PyResult<std::collections::HashMap<String, String>> {
        self.with_store(|store| store.get_file_hashes(&file_paths))
    }

    fn store_file_batch(&self, py: Python<'_>, batch: &Bound<'_, PyAny>) -> PyResult<()> {
        let batch_items = collect_batch(py, batch)?;
        self.with_store_mut(|store| store.store_file_batch(&batch_items))
    }

    fn store_file_batch_json(&self, batch_json: &str) -> PyResult<()> {
        self.with_store_mut(|store| store.store_file_batch_json(batch_json))
    }

    fn remove_file_data(&self, file_path: &str) -> PyResult<()> {
        self.with_store_mut(|store| store.remove_file_data(file_path))
    }

    fn remove_files_data(&self, file_paths: Vec<String>) -> PyResult<()> {
        self.with_store_mut(|store| store.remove_files_data(&file_paths))
    }

    fn close(&self) -> PyResult<()> {
        Ok(())
    }

    fn commit(&self) -> PyResult<()> {
        self.with_store(|store| store.commit())
    }

    fn rollback(&self) -> PyResult<()> {
        self.with_store(|store| store.rollback())
    }
}

impl PyGraphStore {
    fn with_store<T>(
        &self,
        f: impl FnOnce(&NativeGraphStore) -> dagayn_core::Result<T>,
    ) -> PyResult<T> {
        let guard = self
            .inner
            .lock()
            .map_err(|_| PyRuntimeError::new_err("GraphStore lock poisoned"))?;
        f(&guard).map_err(to_py_runtime_error)
    }

    fn with_store_mut<T>(
        &self,
        f: impl FnOnce(&mut NativeGraphStore) -> dagayn_core::Result<T>,
    ) -> PyResult<T> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| PyRuntimeError::new_err("GraphStore lock poisoned"))?;
        f(&mut guard).map_err(to_py_runtime_error)
    }
}

fn collect_nodes(py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<Vec<NodeInput>> {
    let iterator = PyIterator::from_object(value)?;
    iterator
        .map(|item| node_from_py(py, &item?))
        .collect::<PyResult<Vec<_>>>()
}

fn collect_edges(py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<Vec<EdgeInput>> {
    let iterator = PyIterator::from_object(value)?;
    iterator
        .map(|item| edge_from_py(py, &item?))
        .collect::<PyResult<Vec<_>>>()
}

fn collect_batch(py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<Vec<FileBatchItem>> {
    let iterator = PyIterator::from_object(value)?;
    iterator
        .map(|item| batch_item_from_py(py, &item?))
        .collect::<PyResult<Vec<_>>>()
}

fn batch_item_from_py(py: Python<'_>, obj: &Bound<'_, PyAny>) -> PyResult<FileBatchItem> {
    let file_path: String = obj.get_item(0)?.extract()?;
    let nodes_obj = obj.get_item(1)?;
    let edges_obj = obj.get_item(2)?;
    let file_hash: String = obj.get_item(3)?.extract()?;
    Ok((
        file_path,
        collect_nodes(py, &nodes_obj)?,
        collect_edges(py, &edges_obj)?,
        file_hash,
    ))
}

fn node_from_py(py: Python<'_>, obj: &Bound<'_, PyAny>) -> PyResult<NodeInput> {
    Ok(NodeInput {
        kind: string_attr(obj, "kind")?,
        name: string_attr(obj, "name")?,
        file_path: string_attr(obj, "file_path")?,
        line_start: i64_attr(obj, "line_start")?,
        line_end: i64_attr(obj, "line_end")?,
        language: string_attr_or_default(obj, "language")?,
        parent_name: optional_string_attr(obj, "parent_name")?,
        params: optional_string_attr(obj, "params")?,
        return_type: optional_string_attr(obj, "return_type")?,
        modifiers: optional_string_attr(obj, "modifiers")?,
        is_test: bool_attr_or_default(obj, "is_test")?,
        extra: json_attr(py, obj, "extra")?,
    })
}

fn edge_from_py(py: Python<'_>, obj: &Bound<'_, PyAny>) -> PyResult<EdgeInput> {
    Ok(EdgeInput {
        kind: string_attr(obj, "kind")?,
        source: string_attr(obj, "source")?,
        target: string_attr(obj, "target")?,
        file_path: string_attr(obj, "file_path")?,
        line: i64_attr_or_default(obj, "line")?,
        extra: json_attr(py, obj, "extra")?,
    })
}

fn string_attr(obj: &Bound<'_, PyAny>, name: &str) -> PyResult<String> {
    obj.getattr(name)?.extract()
}

fn string_attr_or_default(obj: &Bound<'_, PyAny>, name: &str) -> PyResult<String> {
    match obj.getattr(name) {
        Ok(value) if !value.is_none() => value.extract(),
        _ => Ok(String::new()),
    }
}

fn optional_string_attr(obj: &Bound<'_, PyAny>, name: &str) -> PyResult<Option<String>> {
    match obj.getattr(name) {
        Ok(value) if !value.is_none() => value.extract().map(Some),
        _ => Ok(None),
    }
}

fn i64_attr(obj: &Bound<'_, PyAny>, name: &str) -> PyResult<i64> {
    obj.getattr(name)?.extract()
}

fn i64_attr_or_default(obj: &Bound<'_, PyAny>, name: &str) -> PyResult<i64> {
    match obj.getattr(name) {
        Ok(value) if !value.is_none() => value.extract(),
        _ => Ok(0),
    }
}

fn bool_attr_or_default(obj: &Bound<'_, PyAny>, name: &str) -> PyResult<bool> {
    match obj.getattr(name) {
        Ok(value) if !value.is_none() => value.extract(),
        _ => Ok(false),
    }
}

fn json_attr(py: Python<'_>, obj: &Bound<'_, PyAny>, name: &str) -> PyResult<Value> {
    let value = match obj.getattr(name) {
        Ok(value) if !value.is_none() => value,
        _ => return Ok(Value::Object(Default::default())),
    };
    let json = PyModule::import(py, "json")?;
    let raw: String = json.getattr("dumps")?.call1((value,))?.extract()?;
    serde_json::from_str(&raw).map_err(|err| PyValueError::new_err(err.to_string()))
}

fn to_py_runtime_error(err: dagayn_core::GraphError) -> PyErr {
    PyRuntimeError::new_err(err.to_string())
}

#[pymodule]
fn _core(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PyGraphStore>()?;
    module.add_function(wrap_pyfunction!(filter_parseable_files, module)?)?;
    module.add_function(wrap_pyfunction!(collect_parseable_files, module)?)?;
    Ok(())
}

#[pyfunction]
fn filter_parseable_files(
    py: Python<'_>,
    repo_root: &Bound<'_, PyAny>,
    candidates: Vec<String>,
    ignore_patterns: Vec<String>,
) -> PyResult<Vec<String>> {
    let os = PyModule::import(py, "os")?;
    let repo_root: String = os.getattr("fspath")?.call1((repo_root,))?.extract()?;
    Ok(dagayn_core::parser::filter_parseable_files(
        std::path::Path::new(&repo_root),
        &candidates,
        &ignore_patterns,
    ))
}

#[pyfunction]
#[pyo3(signature = (repo_root, recurse_submodules = None))]
fn collect_parseable_files(
    py: Python<'_>,
    repo_root: &Bound<'_, PyAny>,
    recurse_submodules: Option<bool>,
) -> PyResult<Vec<String>> {
    let os = PyModule::import(py, "os")?;
    let repo_root: String = os.getattr("fspath")?.call1((repo_root,))?.extract()?;
    Ok(dagayn_core::parser::collect_parseable_files(
        std::path::Path::new(&repo_root),
        recurse_submodules,
    ))
}
