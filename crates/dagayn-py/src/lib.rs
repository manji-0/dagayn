use std::sync::Mutex;

use dagayn_core::{
    EdgeInput, FileBatchItem, GraphEdge, GraphNode, GraphStats, GraphStore as NativeGraphStore,
    NodeInput,
};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyBool, PyDict, PyIterator, PyList, PyModule, PySet, PyTuple};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[pyclass(name = "GraphStore")]
struct PyGraphStore {
    inner: Mutex<NativeGraphStore>,
}

type RustStoreSummary = (usize, usize, Vec<(String, String)>);

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

    fn get_node(&self, py: Python<'_>, qualified_name: &str) -> PyResult<Option<Py<PyAny>>> {
        self.with_store(|store| store.get_node(qualified_name))
            .and_then(|node| node.map(|node| graph_node_to_py(py, node)).transpose())
    }

    fn get_nodes_by_file(&self, py: Python<'_>, file_path: &str) -> PyResult<Vec<Py<PyAny>>> {
        self.with_store(|store| store.get_nodes_by_file(file_path))?
            .into_iter()
            .map(|node| graph_node_to_py(py, node))
            .collect()
    }

    #[pyo3(signature = (kinds, file_pattern = None))]
    fn get_nodes_by_kind(
        &self,
        py: Python<'_>,
        kinds: Vec<String>,
        file_pattern: Option<&str>,
    ) -> PyResult<Vec<Py<PyAny>>> {
        self.with_store(|store| store.get_nodes_by_kind(&kinds, file_pattern))?
            .into_iter()
            .map(|node| graph_node_to_py(py, node))
            .collect()
    }

    #[pyo3(signature = (exclude_files = false))]
    fn get_all_nodes(&self, py: Python<'_>, exclude_files: bool) -> PyResult<Vec<Py<PyAny>>> {
        self.with_store(|store| store.get_all_nodes_filtered(exclude_files))?
            .into_iter()
            .map(|node| graph_node_to_py(py, node))
            .collect()
    }

    fn get_all_edges(&self, py: Python<'_>) -> PyResult<Vec<Py<PyAny>>> {
        self.with_store(|store| store.get_all_edges())?
            .into_iter()
            .map(|edge| graph_edge_to_py(py, edge))
            .collect()
    }

    fn get_stats(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.with_store(|store| store.get_stats())
            .and_then(|stats| graph_stats_to_py(py, stats))
    }

    fn get_nodes_by_community_id(
        &self,
        py: Python<'_>,
        community_id: i64,
    ) -> PyResult<Vec<Py<PyAny>>> {
        self.with_store(|store| store.get_nodes_by_community_id(community_id))?
            .into_iter()
            .map(|node| graph_node_to_py(py, node))
            .collect()
    }

    fn count_affected_communities(&self, file_paths: Vec<String>) -> PyResult<i64> {
        self.with_store(|store| store.count_affected_communities(&file_paths))
    }

    #[pyo3(signature = (include_file_sources = true))]
    fn get_all_call_targets(
        &self,
        include_file_sources: bool,
    ) -> PyResult<std::collections::HashSet<String>> {
        self.with_store(|store| store.get_all_call_targets(include_file_sources))
    }

    fn load_flow_adjacency(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let (nodes, (calls_out, has_tested_by)) =
            self.with_store(|store| Ok((store.get_all_nodes()?, store.get_flow_edge_data()?)))?;
        flow_adjacency_to_py(py, nodes, calls_out, has_tested_by)
    }

    fn get_edges_by_source(
        &self,
        py: Python<'_>,
        qualified_name: &str,
    ) -> PyResult<Vec<Py<PyAny>>> {
        self.with_store(|store| store.get_edges_by_source(qualified_name))?
            .into_iter()
            .map(|edge| graph_edge_to_py(py, edge))
            .collect()
    }

    fn get_edges_by_target(
        &self,
        py: Python<'_>,
        qualified_name: &str,
    ) -> PyResult<Vec<Py<PyAny>>> {
        self.with_store(|store| store.get_edges_by_target(qualified_name))?
            .into_iter()
            .map(|edge| graph_edge_to_py(py, edge))
            .collect()
    }

    fn store_file_batch(&self, py: Python<'_>, batch: &Bound<'_, PyAny>) -> PyResult<()> {
        let batch_items = collect_batch(py, batch)?;
        self.with_store_mut(|store| store.store_file_batch(&batch_items))
    }

    fn store_file_batch_json(&self, batch_json: &str) -> PyResult<()> {
        self.with_store_mut(|store| store.store_file_batch_json(batch_json))
    }

    fn store_rust_owned_files(
        &self,
        py: Python<'_>,
        repo_root: &Bound<'_, PyAny>,
        file_paths: Vec<String>,
    ) -> PyResult<RustStoreSummary> {
        let os = PyModule::import(py, "os")?;
        let repo_root: String = os.getattr("fspath")?.call1((repo_root,))?.extract()?;
        let repo_root = std::path::Path::new(&repo_root);
        let mut batch = Vec::new();
        let mut errors = Vec::new();
        let mut total_nodes = 0_usize;
        let mut total_edges = 0_usize;

        for file_path in file_paths {
            if !dagayn_core::parser::rust_parser_owns_path(&file_path) {
                errors.push((file_path, "unsupported Rust parser path".to_string()));
                continue;
            }
            let source = match std::fs::read(repo_root.join(&file_path)) {
                Ok(source) => source,
                Err(err) => {
                    errors.push((file_path, err.to_string()));
                    continue;
                }
            };
            let (nodes, edges) = parse_rust_owned_file_inputs(&file_path, &source);
            total_nodes += nodes.len();
            total_edges += edges.len();
            batch.push((file_path, nodes, edges, sha256_hex(&source)));
        }

        if !batch.is_empty() {
            self.with_store_mut(|store| store.store_file_batch(&batch))?;
        }
        Ok((total_nodes, total_edges, errors))
    }

    fn remove_file_data(&self, file_path: &str) -> PyResult<()> {
        self.with_store_mut(|store| store.remove_file_data(file_path))
    }

    fn remove_files_data(&self, file_paths: Vec<String>) -> PyResult<()> {
        self.with_store_mut(|store| store.remove_files_data(&file_paths))
    }

    fn rebuild_fts_index(&self) -> PyResult<i64> {
        self.with_store_mut(|store| store.rebuild_fts_index())
    }

    fn compute_missing_signatures(&self) -> PyResult<i64> {
        self.with_store_mut(|store| store.compute_missing_signatures())
    }

    fn resolve_markdown_artifact_refs(&self) -> PyResult<(i64, i64)> {
        self.with_store_mut(|store| store.resolve_markdown_artifact_refs())
    }

    fn compute_summaries(&self) -> PyResult<()> {
        self.with_store_mut(|store| store.compute_summaries())
    }

    fn store_flows_json(&self, flows_json: &str) -> PyResult<i64> {
        self.with_store_mut(|store| store.store_flows_json(flows_json))
    }

    fn insert_flows_json(&self, flows_json: &str) -> PyResult<i64> {
        self.with_store_mut(|store| store.insert_flows_json(flows_json))
    }

    #[pyo3(signature = (sort_by = "criticality", limit = 50))]
    fn get_flows_json(&self, sort_by: &str, limit: i64) -> PyResult<String> {
        self.with_store(|store| store.get_flows_json(sort_by, limit))
    }

    fn get_flow_by_id_json(&self, flow_id: i64) -> PyResult<Option<String>> {
        self.with_store(|store| store.get_flow_by_id_json(flow_id))
    }

    fn get_affected_flows_json(&self, changed_files: Vec<String>) -> PyResult<String> {
        self.with_store(|store| store.get_affected_flows_json(&changed_files))
    }

    fn delete_affected_flows(&self, changed_files: Vec<String>) -> PyResult<Vec<i64>> {
        self.with_store_mut(|store| store.delete_affected_flows(&changed_files))
    }

    fn get_node_kind_by_id(&self, node_id: i64) -> PyResult<Option<String>> {
        self.with_store(|store| store.get_node_kind_by_id(node_id))
    }

    fn store_communities_json(&self, communities_json: &str) -> PyResult<i64> {
        self.with_store_mut(|store| store.store_communities_json(communities_json))
    }

    #[pyo3(signature = (sort_by = "size", min_size = 0))]
    fn get_communities_json(&self, sort_by: &str, min_size: i64) -> PyResult<String> {
        self.with_store(|store| store.get_communities_json(sort_by, min_size))
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

fn parse_rust_owned_file_inputs(
    file_path: &str,
    source: &[u8],
) -> (Vec<NodeInput>, Vec<EdgeInput>) {
    let lowered = file_path.to_ascii_lowercase();
    let (nodes, edges) = if lowered.ends_with(".md") || lowered.ends_with(".markdown") {
        dagayn_core::parser::parse_markdown(file_path, source)
    } else if lowered.ends_with(".tf") || lowered.ends_with(".tfvars") {
        dagayn_core::parser::parse_terraform(file_path, source)
    } else {
        (Vec::new(), Vec::new())
    };
    (
        nodes.into_iter().map(parsed_node_to_input).collect(),
        edges.into_iter().map(parsed_edge_to_input).collect(),
    )
}

fn parsed_node_to_input(node: dagayn_core::parser::ParsedNode) -> NodeInput {
    NodeInput {
        kind: node.kind,
        name: node.name,
        file_path: node.file_path,
        line_start: node.line_start,
        line_end: node.line_end,
        language: node.language,
        parent_name: node.parent_name,
        params: node.params,
        return_type: node.return_type,
        modifiers: node.modifiers,
        is_test: node.is_test,
        extra: node.extra,
    }
}

fn parsed_edge_to_input(edge: dagayn_core::parser::ParsedEdge) -> EdgeInput {
    EdgeInput {
        kind: edge.kind,
        source: edge.source,
        target: edge.target,
        file_path: edge.file_path,
        line: edge.line,
        extra: edge.extra,
    }
}

fn sha256_hex(source: &[u8]) -> String {
    let digest = Sha256::digest(source);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
    }
    out
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

fn flow_adjacency_to_py(
    py: Python<'_>,
    nodes: Vec<GraphNode>,
    calls_out: std::collections::HashMap<String, Vec<String>>,
    has_tested_by: std::collections::HashSet<String>,
) -> PyResult<Py<PyAny>> {
    let types = PyModule::import(py, "dagayn.graph.types")?;
    let cls = types.getattr("FlowAdjacency")?;

    let py_calls_out = PyDict::new(py);
    for (source, targets) in calls_out {
        let py_targets = PyList::new(py, targets)?;
        py_calls_out.set_item(source, py_targets)?;
    }

    let py_has_tested_by = PySet::new(py, has_tested_by)?;
    let py_nodes_by_qn = PyDict::new(py);
    let py_nodes_by_id = PyDict::new(py);
    for node in nodes {
        let node_id = node.id;
        let qualified_name = node.qualified_name.clone();
        let py_node = graph_node_to_py(py, node)?;
        py_nodes_by_qn.set_item(qualified_name, py_node.bind(py))?;
        py_nodes_by_id.set_item(node_id, py_node.bind(py))?;
    }

    Ok(cls
        .call1((
            py_calls_out,
            py_has_tested_by,
            py_nodes_by_qn,
            py_nodes_by_id,
        ))?
        .unbind())
}

fn graph_node_to_py(py: Python<'_>, node: GraphNode) -> PyResult<Py<PyAny>> {
    let types = PyModule::import(py, "dagayn.graph.types")?;
    let cls = types.getattr("GraphNode")?;
    let extra = json_value_to_py(py, &node.extra)?;
    let args = PyTuple::new(
        py,
        [
            node.id.into_pyobject(py)?.into_any(),
            node.kind.into_pyobject(py)?.into_any(),
            node.name.into_pyobject(py)?.into_any(),
            node.qualified_name.into_pyobject(py)?.into_any(),
            node.file_path.into_pyobject(py)?.into_any(),
            node.line_start.into_pyobject(py)?.into_any(),
            node.line_end.into_pyobject(py)?.into_any(),
            node.language.into_pyobject(py)?.into_any(),
            node.parent_name.into_pyobject(py)?.into_any(),
            node.params.into_pyobject(py)?.into_any(),
            node.return_type.into_pyobject(py)?.into_any(),
            PyBool::new(py, node.is_test).to_owned().into_any(),
            node.file_hash.into_pyobject(py)?.into_any(),
            extra.bind(py).clone().into_any(),
        ],
    )?;
    Ok(cls.call1(args)?.unbind())
}

fn graph_edge_to_py(py: Python<'_>, edge: GraphEdge) -> PyResult<Py<PyAny>> {
    let types = PyModule::import(py, "dagayn.graph.types")?;
    let cls = types.getattr("GraphEdge")?;
    let extra = json_value_to_py(py, &edge.extra)?;
    Ok(cls
        .call1((
            edge.id,
            edge.kind,
            edge.source_qualified,
            edge.target_qualified,
            edge.file_path,
            edge.line,
            extra,
            edge.confidence,
            edge.confidence_tier,
        ))?
        .unbind())
}

fn graph_stats_to_py(py: Python<'_>, stats: GraphStats) -> PyResult<Py<PyAny>> {
    let types = PyModule::import(py, "dagayn.graph.types")?;
    let cls = types.getattr("GraphStats")?;
    let nodes_by_kind = PyDict::new(py);
    for (kind, count) in stats.nodes_by_kind {
        nodes_by_kind.set_item(kind, count)?;
    }
    let edges_by_kind = PyDict::new(py);
    for (kind, count) in stats.edges_by_kind {
        edges_by_kind.set_item(kind, count)?;
    }
    Ok(cls
        .call1((
            stats.total_nodes,
            stats.total_edges,
            nodes_by_kind,
            edges_by_kind,
            stats.languages,
            stats.files_count,
            stats.last_updated,
        ))?
        .unbind())
}

fn json_value_to_py(py: Python<'_>, value: &Value) -> PyResult<Py<PyAny>> {
    let json = PyModule::import(py, "json")?;
    let raw = serde_json::to_string(value).map_err(|err| PyValueError::new_err(err.to_string()))?;
    Ok(json.getattr("loads")?.call1((raw,))?.unbind())
}

fn to_py_runtime_error(err: dagayn_core::GraphError) -> PyErr {
    PyRuntimeError::new_err(err.to_string())
}

#[pymodule]
fn _core(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PyGraphStore>()?;
    module.add_function(wrap_pyfunction!(filter_parseable_files, module)?)?;
    module.add_function(wrap_pyfunction!(collect_parseable_files, module)?)?;
    module.add_function(wrap_pyfunction!(
        parse_rust_owned_files_compact_json,
        module
    )?)?;
    module.add_function(wrap_pyfunction!(parse_markdown_compact_json, module)?)?;
    module.add_function(wrap_pyfunction!(parse_terraform_compact_json, module)?)?;
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

#[pyfunction]
fn parse_rust_owned_files_compact_json(
    py: Python<'_>,
    repo_root: &Bound<'_, PyAny>,
    file_paths: Vec<String>,
) -> PyResult<String> {
    let os = PyModule::import(py, "os")?;
    let repo_root: String = os.getattr("fspath")?.call1((repo_root,))?.extract()?;
    Ok(dagayn_core::parser::parse_rust_owned_files_compact_json(
        std::path::Path::new(&repo_root),
        &file_paths,
    ))
}

#[pyfunction]
fn parse_markdown_compact_json(file_path: &str, source: &[u8]) -> PyResult<String> {
    Ok(dagayn_core::parser::parse_markdown_compact_json(
        file_path, source,
    ))
}

#[pyfunction]
fn parse_terraform_compact_json(file_path: &str, source: &[u8]) -> PyResult<String> {
    Ok(dagayn_core::parser::parse_terraform_compact_json(
        file_path, source,
    ))
}
