use std::sync::Mutex;

use dagayn_core::{
    EdgeInput, FileBatchItem, GraphEdge, GraphNode, GraphStats, GraphStore as NativeGraphStore,
    ImpactRadius, NodeInput, NodeSignatureRow,
    detect_communities_json as native_detect_communities_json,
    incremental_detect_communities as native_incremental_detect_communities,
    prune_orphaned_graph_structures_json as native_prune_orphaned_graph_structures_json,
    refresh_community_stats_json as native_refresh_community_stats_json,
    run_post_processing_json as native_run_post_processing_json,
};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyBool, PyDict, PyIterator, PyList, PyModule, PySet, PyTuple};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

#[pyclass(name = "GraphStore")]
struct PyGraphStore {
    /// `None` once the store has been closed. `close()` used to be a no-op,
    /// which leaked one SQLite connection per store for the life of the
    /// process: a later connection to the same file could then fail to open
    /// with `disk I/O error`, because the leaked handles still hold the
    /// database mmap'd while another connection checkpoints and truncates the
    /// WAL underneath them.
    inner: Mutex<Option<NativeGraphStore>>,
    db_path: String,
    pinned: Mutex<bool>,
    leases: Mutex<i64>,
    pending_rust_changed: Mutex<HashMap<String, CachedRustChangedFile>>,
}

struct CachedRustChangedFile {
    source: Vec<u8>,
    file_hash: String,
    mtime_ns: i64,
}

type RustStoreSummary = (usize, usize, Vec<(String, String)>);
type RustChangedFilesSummary = (Vec<String>, Vec<(String, String)>);
type RustFileBatchSummary = (Vec<FileBatchItem>, usize, usize, Vec<(String, String)>);
type RustChangedFileBatchSummary = (
    Vec<FileBatchItem>,
    Vec<(String, i64)>,
    usize,
    usize,
    Vec<(String, String)>,
);
type RustClassifiedFilesSummary = (
    Vec<(String, CachedRustChangedFile)>,
    Vec<(String, i64)>,
    Vec<(String, String)>,
);

#[pymethods]
impl PyGraphStore {
    #[new]
    fn new(py: Python<'_>, db_path: &Bound<'_, PyAny>) -> PyResult<Self> {
        let os = PyModule::import(py, "os")?;
        let db_path: String = os.getattr("fspath")?.call1((db_path,))?.extract()?;
        let inner = NativeGraphStore::open(&db_path).map_err(to_py_runtime_error)?;
        Ok(Self {
            inner: Mutex::new(Some(inner)),
            db_path,
            pinned: Mutex::new(false),
            leases: Mutex::new(0),
            pending_rust_changed: Mutex::new(HashMap::new()),
        })
    }

    /// A `pathlib.Path`, matching `dagayn.graph.GraphStore.db_path`.
    ///
    /// Returning the raw string here made callers that use it as a path (the
    /// embedding store's mtime check, for one) fail with `'str' object has no
    /// attribute 'stat'`.
    #[getter]
    fn db_path(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let pathlib = PyModule::import(py, "pathlib")?;
        Ok(pathlib.getattr("Path")?.call1((&self.db_path,))?.unbind())
    }

    #[getter(_pinned)]
    fn get_pinned(&self) -> PyResult<bool> {
        self.pinned
            .lock()
            .map(|value| *value)
            .map_err(|err| PyRuntimeError::new_err(err.to_string()))
    }

    #[setter(_pinned)]
    fn set_pinned(&self, value: bool) -> PyResult<()> {
        let mut pinned = self
            .pinned
            .lock()
            .map_err(|err| PyRuntimeError::new_err(err.to_string()))?;
        *pinned = value;
        Ok(())
    }

    #[getter(_leases)]
    fn get_leases(&self) -> PyResult<i64> {
        self.leases
            .lock()
            .map(|value| *value)
            .map_err(|err| PyRuntimeError::new_err(err.to_string()))
    }

    #[setter(_leases)]
    fn set_leases(&self, value: i64) -> PyResult<()> {
        let mut leases = self
            .leases
            .lock()
            .map_err(|err| PyRuntimeError::new_err(err.to_string()))?;
        *leases = value;
        Ok(())
    }

    fn set_metadata(&self, key: &str, value: &str) -> PyResult<()> {
        self.with_store(|store| store.set_metadata(key, value))
    }

    fn get_metadata(&self, key: &str) -> PyResult<Option<String>> {
        self.with_store(|store| store.get_metadata(key))
    }

    /// Match the Python ``GraphStore.get_repo_root`` contract used by postprocess.
    fn get_repo_root(&self, py: Python<'_>) -> PyResult<Option<Py<PyAny>>> {
        let raw = self.with_store(|store| store.get_metadata("repo_root"))?;
        match raw {
            Some(value) => {
                let pathlib = PyModule::import(py, "pathlib")?;
                let path = pathlib.getattr("Path")?.call1((value,))?;
                Ok(Some(path.unbind()))
            }
            None => Ok(None),
        }
    }

    #[pyo3(signature = (file_path, nodes, edges, fhash = "", mtime_ns = 0))]
    fn store_file_nodes_edges(
        &self,
        py: Python<'_>,
        file_path: &str,
        nodes: &Bound<'_, PyAny>,
        edges: &Bound<'_, PyAny>,
        fhash: &str,
        mtime_ns: i64,
    ) -> PyResult<()> {
        let node_inputs = collect_nodes(py, nodes)?;
        let edge_inputs = collect_edges(py, edges)?;
        self.with_store_mut(|store| {
            store.store_file_nodes_edges(file_path, &node_inputs, &edge_inputs, fhash, mtime_ns)
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

    fn get_file_meta_map(&self) -> PyResult<std::collections::HashMap<String, (String, i64)>> {
        self.with_store(|store| store.get_file_meta_map())
    }

    fn get_file_meta_for_files(
        &self,
        file_paths: Vec<String>,
    ) -> PyResult<std::collections::HashMap<String, (String, i64)>> {
        self.with_store(|store| store.get_file_meta_for_files(&file_paths))
    }

    fn update_file_mtime(&self, file_path: &str, mtime_ns: i64) -> PyResult<()> {
        self.with_store(|store| store.update_file_mtime(file_path, mtime_ns))
    }

    fn update_file_mtimes(&self, updates: Vec<(i64, String)>) -> PyResult<()> {
        let updates = updates
            .into_iter()
            .map(|(mtime_ns, file_path)| (file_path, mtime_ns))
            .collect::<Vec<_>>();
        self.with_store_mut(|store| store.update_file_mtimes(&updates))
    }

    fn get_node(&self, py: Python<'_>, qualified_name: &str) -> PyResult<Option<Py<PyAny>>> {
        self.with_store(|store| store.get_node(qualified_name))
            .and_then(|node| node.map(|node| graph_node_to_py(py, node)).transpose())
    }

    fn get_nodes_by_qualified_names(
        &self,
        py: Python<'_>,
        qualified_names: Vec<String>,
    ) -> PyResult<Py<PyAny>> {
        let nodes =
            self.with_store(|store| store.get_nodes_by_qualified_names(&qualified_names))?;
        node_map_by_string_to_py(py, nodes)
    }

    fn get_nodes_by_ids(&self, py: Python<'_>, node_ids: Vec<i64>) -> PyResult<Py<PyAny>> {
        let nodes = self.with_store(|store| store.get_nodes_by_ids(&node_ids))?;
        node_map_by_id_to_py(py, nodes)
    }

    fn get_nodes_by_file(&self, py: Python<'_>, file_path: &str) -> PyResult<Vec<Py<PyAny>>> {
        let nodes = self.with_store(|store| store.get_nodes_by_file(file_path))?;
        graph_nodes_to_py_vec(py, nodes)
    }

    fn get_nodes_by_files(&self, py: Python<'_>, file_paths: Vec<String>) -> PyResult<Py<PyAny>> {
        let nodes_by_file = self.with_store(|store| store.get_nodes_by_files(&file_paths))?;
        node_list_map_to_py(py, nodes_by_file)
    }

    #[pyo3(signature = (kinds, file_pattern = None))]
    fn get_nodes_by_kind(
        &self,
        py: Python<'_>,
        kinds: Vec<String>,
        file_pattern: Option<&str>,
    ) -> PyResult<Vec<Py<PyAny>>> {
        let nodes = self.with_store(|store| store.get_nodes_by_kind(&kinds, file_pattern))?;
        graph_nodes_to_py_vec(py, nodes)
    }

    #[pyo3(signature = (exclude_files = false))]
    fn get_all_nodes(&self, py: Python<'_>, exclude_files: bool) -> PyResult<Vec<Py<PyAny>>> {
        let nodes = self.with_store(|store| store.get_all_nodes_filtered(exclude_files))?;
        graph_nodes_to_py_vec(py, nodes)
    }

    fn get_all_edges(&self, py: Python<'_>) -> PyResult<Vec<Py<PyAny>>> {
        let edges = self.with_store(|store| store.get_all_edges())?;
        graph_edges_to_py_vec(py, edges)
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
        let nodes = self.with_store(|store| store.get_nodes_by_community_id(community_id))?;
        graph_nodes_to_py_vec(py, nodes)
    }

    fn get_files_matching(&self, pattern: &str) -> PyResult<Vec<String>> {
        self.with_store(|store| store.get_files_matching(pattern))
    }

    fn count_flow_memberships(&self, node_id: i64) -> PyResult<i64> {
        self.with_store(|store| store.count_flow_memberships(node_id))
    }

    fn count_flow_memberships_for_nodes(
        &self,
        node_ids: Vec<i64>,
    ) -> PyResult<std::collections::HashMap<i64, i64>> {
        self.with_store(|store| store.count_flow_memberships_for_nodes(&node_ids))
    }

    fn get_flow_criticalities_for_node(&self, node_id: i64) -> PyResult<Vec<f64>> {
        self.with_store(|store| store.get_flow_criticalities_for_node(node_id))
    }

    fn get_flow_criticalities_for_nodes(
        &self,
        node_ids: Vec<i64>,
    ) -> PyResult<std::collections::HashMap<i64, Vec<f64>>> {
        self.with_store(|store| store.get_flow_criticalities_for_nodes(&node_ids))
    }

    fn get_flow_qualified_names_for_flows(
        &self,
        py: Python<'_>,
        flow_ids: Vec<i64>,
    ) -> PyResult<Py<PyAny>> {
        let flow_qns =
            self.with_store(|store| store.get_flow_qualified_names_for_flows(&flow_ids))?;
        let out = PyDict::new(py);
        for (flow_id, qualified_names) in flow_qns {
            out.set_item(flow_id, PySet::new(py, qualified_names)?)?;
        }
        Ok(out.unbind().into_any())
    }

    fn get_node_community_id(&self, node_id: i64) -> PyResult<Option<i64>> {
        self.with_store(|store| store.get_node_community_id(node_id))
    }

    fn get_community_ids_by_node_ids(
        &self,
        py: Python<'_>,
        node_ids: Vec<i64>,
    ) -> PyResult<Py<PyAny>> {
        let community_ids =
            self.with_store(|store| store.get_community_ids_by_node_ids(&node_ids))?;
        let out = PyDict::new(py);
        for (node_id, community_id) in community_ids {
            out.set_item(node_id, community_id)?;
        }
        Ok(out.unbind().into_any())
    }

    fn get_community_ids_by_qualified_names(
        &self,
        py: Python<'_>,
        qns: Vec<String>,
    ) -> PyResult<Py<PyAny>> {
        let community_ids =
            self.with_store(|store| store.get_community_ids_by_qualified_names(&qns))?;
        let out = PyDict::new(py);
        for (qualified_name, community_id) in community_ids {
            out.set_item(qualified_name, community_id)?;
        }
        Ok(out.unbind().into_any())
    }

    fn get_all_community_member_qns(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let members = self.with_store(|store| store.get_all_community_member_qns())?;
        let out = PyDict::new(py);
        for (community_id, qualified_names) in members {
            out.set_item(community_id, qualified_names)?;
        }
        Ok(out.unbind().into_any())
    }

    fn get_all_community_ids(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let community_ids = self.with_store(|store| store.get_all_community_ids())?;
        let out = PyDict::new(py);
        for (qualified_name, community_id) in community_ids {
            out.set_item(qualified_name, community_id)?;
        }
        Ok(out.unbind().into_any())
    }

    #[pyo3(signature = (qualified_name, max_depth = 1))]
    fn get_transitive_tests(
        &self,
        py: Python<'_>,
        qualified_name: &str,
        max_depth: i64,
    ) -> PyResult<Vec<Py<PyAny>>> {
        self.with_store(|store| store.get_transitive_tests(qualified_name, max_depth))?
            .into_iter()
            .map(|value| json_value_to_py(py, &value))
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
        let edges = self.with_store(|store| store.get_edges_by_source(qualified_name))?;
        graph_edges_to_py_vec(py, edges)
    }

    fn get_edges_by_target(
        &self,
        py: Python<'_>,
        qualified_name: &str,
    ) -> PyResult<Vec<Py<PyAny>>> {
        let edges = self.with_store(|store| store.get_edges_by_target(qualified_name))?;
        graph_edges_to_py_vec(py, edges)
    }

    fn get_edges_by_endpoints(
        &self,
        py: Python<'_>,
        qualified_names: Vec<String>,
    ) -> PyResult<Py<PyAny>> {
        let (outgoing, incoming) =
            self.with_store(|store| store.get_edges_by_endpoints(&qualified_names))?;
        let py_outgoing = edge_map_to_py(py, outgoing)?;
        let py_incoming = edge_map_to_py(py, incoming)?;
        Ok(
            PyTuple::new(py, [py_outgoing.bind(py), py_incoming.bind(py)])?
                .unbind()
                .into_any(),
        )
    }

    fn get_direct_dependents(&self, file_paths: Vec<String>) -> PyResult<Vec<String>> {
        self.with_store(|store| store.get_direct_dependents(&file_paths))
    }

    fn store_file_batch(&self, py: Python<'_>, batch: &Bound<'_, PyAny>) -> PyResult<()> {
        let batch_items = collect_batch(py, batch)?;
        self.with_store_mut(|store| store.store_file_batch(&batch_items))
    }

    fn store_file_batch_json(&self, batch_json: &str) -> PyResult<()> {
        self.with_store_mut(|store| store.store_file_batch_json(batch_json))
    }

    fn begin_bulk_load(&self) -> PyResult<()> {
        self.with_store_mut(|store| store.begin_bulk_load())
    }

    fn finish_bulk_load(&self) -> PyResult<()> {
        self.with_store_mut(|store| store.finish_bulk_load())
    }

    fn store_rust_owned_files(
        &self,
        py: Python<'_>,
        repo_root: &Bound<'_, PyAny>,
        file_paths: Vec<String>,
    ) -> PyResult<RustStoreSummary> {
        let os = PyModule::import(py, "os")?;
        let repo_root: String = os.getattr("fspath")?.call1((repo_root,))?.extract()?;
        let repo_root = std::path::PathBuf::from(repo_root);
        let (batch, total_nodes, total_edges, errors) =
            py.detach(|| collect_rust_owned_file_batch(&repo_root, file_paths));

        if !batch.is_empty() {
            self.with_store_mut(|store| store.store_file_batch(&batch))?;
        }
        Ok((total_nodes, total_edges, errors))
    }

    fn store_changed_rust_owned_files(
        &self,
        py: Python<'_>,
        repo_root: &Bound<'_, PyAny>,
        file_paths: Vec<String>,
    ) -> PyResult<RustStoreSummary> {
        let os = PyModule::import(py, "os")?;
        let repo_root: String = os.getattr("fspath")?.call1((repo_root,))?.extract()?;
        let repo_root = std::path::PathBuf::from(repo_root);
        let file_meta = self.with_store(|store| store.get_file_meta_for_files(&file_paths))?;
        let cached = self.take_pending_rust_changed(&file_paths)?;
        let (batch, mtime_updates, total_nodes, total_edges, errors) = py.detach(|| {
            collect_changed_rust_owned_file_batch(&repo_root, file_paths, &file_meta, cached)
        });

        if !mtime_updates.is_empty() || !batch.is_empty() {
            self.with_store_mut(|store| {
                store.update_file_mtimes(&mtime_updates)?;
                if !batch.is_empty() {
                    store.store_file_batch(&batch)?;
                }
                Ok(())
            })?;
        }
        Ok((total_nodes, total_edges, errors))
    }

    fn classify_changed_rust_owned_files(
        &self,
        py: Python<'_>,
        repo_root: &Bound<'_, PyAny>,
        file_paths: Vec<String>,
    ) -> PyResult<RustChangedFilesSummary> {
        let os = PyModule::import(py, "os")?;
        let repo_root: String = os.getattr("fspath")?.call1((repo_root,))?.extract()?;
        let repo_root = std::path::PathBuf::from(repo_root);
        let file_meta = self.with_store(|store| store.get_file_meta_for_files(&file_paths))?;
        let (changed, mtime_updates, errors) = py
            .detach(|| classify_changed_rust_owned_file_batch(&repo_root, file_paths, &file_meta));
        let changed_files = changed
            .iter()
            .map(|(file_path, _)| file_path.clone())
            .collect::<Vec<_>>();
        self.extend_pending_rust_changed(changed)?;

        if !mtime_updates.is_empty() {
            self.with_store_mut(|store| store.update_file_mtimes(&mtime_updates))?;
        }
        Ok((changed_files, errors))
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

    fn resolve_markdown_artifact_refs(&self) -> PyResult<(i64, i64, i64, i64)> {
        self.with_store_mut(|store| store.resolve_markdown_artifact_refs())
    }

    fn demote_unresolved_endpoint_edges(&self) -> PyResult<i64> {
        self.with_store_mut(|store| store.demote_unresolved_endpoint_edges())
    }

    fn resolve_terraform_artifact_refs(&self) -> PyResult<(i64, i64)> {
        self.with_store_mut(|store| store.resolve_terraform_artifact_refs())
    }

    fn resolve_bare_call_targets(&self) -> PyResult<i64> {
        self.with_store_mut(|store| store.resolve_bare_call_targets())
    }

    fn resolve_bare_inheritance_targets(&self) -> PyResult<i64> {
        self.with_store_mut(|store| store.resolve_bare_inheritance_targets())
    }

    fn import_targets_by_file(&self) -> PyResult<std::collections::HashMap<String, Vec<String>>> {
        self.with_store(|store| store.import_targets_by_file())
    }

    #[allow(clippy::type_complexity)]
    fn symbol_visibility_by_file(
        &self,
    ) -> PyResult<(
        std::collections::HashMap<String, Vec<String>>,
        std::collections::HashMap<String, Vec<String>>,
        std::collections::HashMap<String, Vec<String>>,
    )> {
        self.with_store(|store| store.symbol_visibility_by_file())
    }

    fn replace_manifest_bridges_json(
        &self,
        extractor_id: &str,
        nodes_json: &str,
        edges_json: &str,
    ) -> PyResult<i64> {
        self.with_store_mut(|store| {
            store.replace_manifest_bridges_json(extractor_id, nodes_json, edges_json)
        })
    }

    #[pyo3(signature = (changed_files = None))]
    fn persist_centrality_scores(
        &self,
        changed_files: Option<Vec<String>>,
    ) -> PyResult<std::collections::HashMap<String, i64>> {
        self.with_store_mut(|store| {
            store.persist_centrality_scores_filtered(changed_files.as_deref())
        })
    }

    fn sync_fts_for_file_paths(&self, file_paths: Vec<String>) -> PyResult<i64> {
        self.with_store_mut(|store| store.sync_fts_for_file_paths(&file_paths))
    }

    #[pyo3(signature = (manifest_extractor_id, manifest_nodes_json, manifest_edges_json, min_community_size = 2, changed_files = None))]
    fn run_post_processing_json(
        &self,
        manifest_extractor_id: &str,
        manifest_nodes_json: &str,
        manifest_edges_json: &str,
        min_community_size: i64,
        changed_files: Option<Vec<String>>,
    ) -> PyResult<String> {
        self.with_store_mut(|store| {
            native_run_post_processing_json(
                store,
                manifest_extractor_id,
                manifest_nodes_json,
                manifest_edges_json,
                min_community_size,
                changed_files.as_deref(),
            )
        })
    }

    fn generate_suggested_questions_json(&self) -> PyResult<String> {
        self.with_store(|store| store.generate_suggested_questions_json())
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

    fn update_flow_criticalities_json(&self, updates_json: &str) -> PyResult<i64> {
        self.with_store_mut(|store| store.update_flow_criticalities_json(updates_json))
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

    fn analyze_changes_json(
        &self,
        changed_files: Vec<String>,
        changed_ranges_json: Option<&str>,
    ) -> PyResult<String> {
        self.with_store(|store| store.analyze_changes_json(&changed_files, changed_ranges_json))
    }

    fn delete_affected_flows(&self, changed_files: Vec<String>) -> PyResult<Vec<i64>> {
        self.with_store_mut(|store| store.delete_affected_flows(&changed_files))
    }

    #[pyo3(signature = (include_tests = false))]
    fn detect_entry_points_json(&self, include_tests: bool) -> PyResult<String> {
        self.with_store(|store| store.detect_entry_points_json(include_tests))
    }

    #[pyo3(signature = (max_depth = 15, include_tests = false))]
    fn rebuild_flows_json(&self, max_depth: i64, include_tests: bool) -> PyResult<String> {
        self.with_store_mut(|store| store.rebuild_flows_json(max_depth, include_tests))
    }

    #[pyo3(signature = (changed_files, max_depth = 15))]
    fn incremental_trace_flows_json(
        &self,
        changed_files: Vec<String>,
        max_depth: i64,
    ) -> PyResult<String> {
        self.with_store_mut(|store| store.incremental_trace_flows_json(&changed_files, max_depth))
    }

    fn get_node_kind_by_id(&self, node_id: i64) -> PyResult<Option<String>> {
        self.with_store(|store| store.get_node_kind_by_id(node_id))
    }

    fn store_communities_json(&self, communities_json: &str) -> PyResult<i64> {
        self.with_store_mut(|store| store.store_communities_json(communities_json))
    }

    #[pyo3(signature = (min_size = 2))]
    fn detect_communities_json(&self, min_size: i64) -> PyResult<String> {
        self.with_store(|store| native_detect_communities_json(store, min_size))
    }

    #[pyo3(signature = (changed_files, min_size = 2, pre_affected_count = None))]
    fn incremental_detect_communities(
        &self,
        changed_files: Vec<String>,
        min_size: i64,
        pre_affected_count: Option<i64>,
    ) -> PyResult<i64> {
        self.with_store_mut(|store| {
            native_incremental_detect_communities(
                store,
                &changed_files,
                min_size,
                pre_affected_count,
            )
        })
    }

    fn refresh_community_stats_json(&self) -> PyResult<String> {
        self.with_store_mut(native_refresh_community_stats_json)
    }

    #[pyo3(signature = (sort_by = "size", min_size = 0))]
    fn get_communities_json(&self, sort_by: &str, min_size: i64) -> PyResult<String> {
        self.with_store(|store| store.get_communities_json(sort_by, min_size))
    }

    // -----------------------------------------------------------------------
    // Python `GraphStore` parity surface
    //
    // These mirror the Python mixins one-for-one so tools can hold either store
    // without probing for `_conn` or a method's existence. See: #153
    // -----------------------------------------------------------------------

    fn get_node_by_id(&self, py: Python<'_>, node_id: i64) -> PyResult<Option<Py<PyAny>>> {
        let node = self.with_store(|store| store.get_node_by_id(node_id))?;
        node.map(|node| graph_node_to_py(py, node)).transpose()
    }

    #[pyo3(signature = (min_lines = 50, max_lines = None, kind = None, file_path_pattern = None, limit = 50))]
    fn get_nodes_by_size(
        &self,
        py: Python<'_>,
        min_lines: i64,
        max_lines: Option<i64>,
        kind: Option<&str>,
        file_path_pattern: Option<&str>,
        limit: i64,
    ) -> PyResult<Vec<Py<PyAny>>> {
        let nodes = self.with_store(|store| {
            store.get_nodes_by_size(min_lines, max_lines, kind, file_path_pattern, limit)
        })?;
        graph_nodes_to_py_vec(py, nodes)
    }

    fn get_node_ids_by_files(
        &self,
        file_paths: Vec<String>,
    ) -> PyResult<std::collections::HashSet<i64>> {
        self.with_store(|store| store.get_node_ids_by_files(&file_paths))
    }

    #[pyo3(signature = (kinds, include_tests = false))]
    fn count_nodes_by_name(
        &self,
        kinds: Vec<String>,
        include_tests: bool,
    ) -> PyResult<std::collections::HashMap<String, i64>> {
        self.with_store(|store| store.count_nodes_by_name(&kinds, include_tests))
    }

    fn get_nodes_by_parent_and_name(
        &self,
        py: Python<'_>,
        parent_name: &str,
        name: &str,
        kinds: Vec<String>,
    ) -> PyResult<Vec<Py<PyAny>>> {
        let nodes =
            self.with_store(|store| store.get_nodes_by_parent_and_name(parent_name, name, &kinds))?;
        graph_nodes_to_py_vec(py, nodes)
    }

    fn get_nodes_without_signature(&self) -> PyResult<Vec<NodeSignatureRow>> {
        self.with_store(|store| store.get_nodes_without_signature())
    }

    fn update_node_signature(&self, node_id: i64, signature: &str) -> PyResult<()> {
        self.with_store(|store| store.update_node_signature(node_id, signature))
    }

    #[pyo3(signature = (kind, unresolved_target_only = false))]
    fn get_edges_by_kind(
        &self,
        py: Python<'_>,
        kind: &str,
        unresolved_target_only: bool,
    ) -> PyResult<Vec<Py<PyAny>>> {
        let edges =
            self.with_store(|store| store.get_edges_by_kind(kind, unresolved_target_only))?;
        graph_edges_to_py_vec(py, edges)
    }

    #[pyo3(signature = (source_qns, kinds = None))]
    fn get_edges_by_sources(
        &self,
        py: Python<'_>,
        source_qns: Vec<String>,
        kinds: Option<Vec<String>>,
    ) -> PyResult<Py<PyAny>> {
        let kinds = kinds.unwrap_or_default();
        let edges = self.with_store(|store| store.get_edges_by_sources(&source_qns, &kinds))?;
        edge_map_to_py(py, edges)
    }

    #[pyo3(signature = (target_qns, kinds = None))]
    fn get_edges_by_targets(
        &self,
        py: Python<'_>,
        target_qns: Vec<String>,
        kinds: Option<Vec<String>>,
    ) -> PyResult<Py<PyAny>> {
        let kinds = kinds.unwrap_or_default();
        let edges = self.with_store(|store| store.get_edges_by_targets(&target_qns, &kinds))?;
        edge_map_to_py(py, edges)
    }

    #[pyo3(signature = (names, kind = "CALLS", qualified_only = false))]
    fn get_edges_by_target_names(
        &self,
        py: Python<'_>,
        names: Vec<String>,
        kind: &str,
        qualified_only: bool,
    ) -> PyResult<Py<PyAny>> {
        let edges =
            self.with_store(|store| store.get_edges_by_target_names(&names, kind, qualified_only))?;
        edge_map_to_py(py, edges)
    }

    #[pyo3(signature = (prefix, kind = "CALLS"))]
    fn count_edges_by_target_name_prefix(&self, prefix: &str, kind: &str) -> PyResult<i64> {
        self.with_store(|store| store.count_edges_by_target_name_prefix(prefix, kind))
    }

    #[pyo3(signature = (target_qualified, kind = "CALLS"))]
    fn has_edge_to_target(&self, target_qualified: &str, kind: &str) -> PyResult<bool> {
        self.with_store(|store| store.has_edge_to_target(target_qualified, kind))
    }

    #[pyo3(signature = (name, kind = "CALLS"))]
    fn search_edges_by_target_name(
        &self,
        py: Python<'_>,
        name: &str,
        kind: &str,
    ) -> PyResult<Vec<Py<PyAny>>> {
        let edges = self.with_store(|store| store.search_edges_by_target_name(name, kind))?;
        graph_edges_to_py_vec(py, edges)
    }

    /// `_symbol_name` is accepted for call-site symmetry with the Python store,
    /// which also ignores it: `IMPORTS_FROM` targets are module file paths.
    #[pyo3(signature = (defining_file, _symbol_name = None))]
    fn search_import_edges_for_symbol(
        &self,
        py: Python<'_>,
        defining_file: &str,
        _symbol_name: Option<&str>,
    ) -> PyResult<Vec<Py<PyAny>>> {
        let edges = self.with_store(|store| store.search_import_edges_for_symbol(defining_file))?;
        graph_edges_to_py_vec(py, edges)
    }

    fn get_outgoing_targets(&self, source_qns: Vec<String>) -> PyResult<Vec<String>> {
        self.with_store(|store| store.get_outgoing_targets(&source_qns))
    }

    fn get_incoming_sources(&self, target_qns: Vec<String>) -> PyResult<Vec<String>> {
        self.with_store(|store| store.get_incoming_sources(&target_qns))
    }

    fn get_edges_among(
        &self,
        py: Python<'_>,
        qualified_names: std::collections::HashSet<String>,
    ) -> PyResult<Vec<Py<PyAny>>> {
        let edges = self.with_store(|store| store.get_edges_among(&qualified_names))?;
        graph_edges_to_py_vec(py, edges)
    }

    fn get_subgraph(&self, py: Python<'_>, qualified_names: Vec<String>) -> PyResult<Py<PyAny>> {
        let (nodes, edges) = self.with_store(|store| store.get_subgraph(&qualified_names))?;
        let out = PyDict::new(py);
        out.set_item("nodes", graph_nodes_to_py_vec(py, nodes)?)?;
        out.set_item("edges", graph_edges_to_py_vec(py, edges)?)?;
        Ok(out.unbind().into_any())
    }

    fn get_local_subgraph(
        &self,
        py: Python<'_>,
        start_qn: &str,
        max_depth: i64,
    ) -> PyResult<Py<PyAny>> {
        let (nodes, adjacency) =
            self.with_store(|store| store.get_local_subgraph(start_qn, max_depth))?;
        let nodes_map = node_map_by_string_to_py(py, nodes)?;
        let adjacency_map = PyDict::new(py);
        for (qualified_name, neighbors) in adjacency {
            adjacency_map.set_item(qualified_name, neighbors)?;
        }
        Ok(
            PyTuple::new(py, [nodes_map.bind(py).clone(), adjacency_map.into_any()])?
                .unbind()
                .into_any(),
        )
    }

    fn get_communities_list(&self, py: Python<'_>) -> PyResult<Vec<Py<PyAny>>> {
        let communities = self.with_store(|store| store.get_communities_list())?;
        // Python returns `sqlite3.Row`s that callers index as `row["id"]` /
        // `row["name"]`, so dicts are the faithful stand-in.
        communities
            .into_iter()
            .map(|(id, name)| {
                let row = PyDict::new(py);
                row.set_item("id", id)?;
                row.set_item("name", name)?;
                Ok(row.unbind().into_any())
            })
            .collect()
    }

    fn get_community_member_qns(&self, community_id: i64) -> PyResult<Vec<String>> {
        self.with_store(|store| store.get_community_member_qns(community_id))
    }

    fn get_flow_ids_by_node_ids(
        &self,
        node_ids: std::collections::HashSet<i64>,
    ) -> PyResult<Vec<i64>> {
        self.with_store(|store| store.get_flow_ids_by_node_ids(&node_ids))
    }

    fn get_flow_qualified_names(
        &self,
        flow_id: i64,
    ) -> PyResult<std::collections::HashSet<String>> {
        self.with_store(|store| store.get_flow_qualified_names(flow_id))
    }

    fn resolve_file_path(&self, py: Python<'_>, file_path: &str) -> PyResult<Py<PyAny>> {
        let resolved = self.with_store(|store| store.resolve_file_path(file_path))?;
        let pathlib = PyModule::import(py, "pathlib")?;
        Ok(pathlib
            .getattr("Path")?
            .call1((resolved.to_string_lossy().as_ref(),))?
            .unbind())
    }

    #[pyo3(signature = (query, limit = 50))]
    fn fts_query(&self, py: Python<'_>, query: &str, limit: i64) -> PyResult<Py<PyAny>> {
        let (hits, match_mode) = self.with_store(|store| store.fts_query(query, limit))?;
        let types = PyModule::import(py, "dagayn.graph.types")?;
        Ok(types
            .getattr("FtsQueryResult")?
            .call1((hits, match_mode))?
            .unbind())
    }

    fn fts_index_health(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let (status, nodes_count, fts_count, watermark) =
            self.with_store(|store| store.fts_index_health())?;
        let out = PyDict::new(py);
        out.set_item("status", status)?;
        out.set_item("nodes_count", nodes_count)?;
        out.set_item("fts_count", fts_count)?;
        out.set_item("watermark_count", watermark)?;
        Ok(out.unbind().into_any())
    }

    fn count_non_file_nodes(&self) -> PyResult<i64> {
        self.with_store(|store| store.count_non_file_nodes())
    }

    #[pyo3(signature = (query, limit = 50))]
    fn keyword_query(&self, query: &str, limit: i64) -> PyResult<Vec<(i64, f64)>> {
        self.with_store(|store| store.keyword_query(query, limit))
    }

    #[pyo3(signature = (query, limit = 20))]
    fn search_nodes(&self, py: Python<'_>, query: &str, limit: i64) -> PyResult<Vec<Py<PyAny>>> {
        let nodes = self.with_store(|store| store.search_nodes(query, limit))?;
        graph_nodes_to_py_vec(py, nodes)
    }

    #[pyo3(signature = (changed_files, max_depth = 2, max_nodes = 500))]
    fn get_impact_radius(
        &self,
        py: Python<'_>,
        changed_files: Vec<String>,
        max_depth: i64,
        max_nodes: i64,
    ) -> PyResult<Py<PyAny>> {
        let radius =
            self.with_store(|store| store.get_impact_radius(&changed_files, max_depth, max_nodes))?;
        impact_radius_to_py(py, radius)
    }

    /// The SQL engine is the only implementation here; Python keeps a NetworkX
    /// variant behind `CRG_BFS_ENGINE=networkx` for the same result.
    #[pyo3(signature = (changed_files, max_depth = 2, max_nodes = 500))]
    fn get_impact_radius_sql(
        &self,
        py: Python<'_>,
        changed_files: Vec<String>,
        max_depth: i64,
        max_nodes: i64,
    ) -> PyResult<Py<PyAny>> {
        self.get_impact_radius(py, changed_files, max_depth, max_nodes)
    }

    fn remove_node_keyed_rows_for_files(&self, file_keys: Vec<String>) -> PyResult<()> {
        self.with_store(|store| store.remove_node_keyed_rows_for_files(&file_keys))
    }

    fn prune_orphaned_graph_structures(&self) -> PyResult<std::collections::HashMap<String, i64>> {
        let raw = self.with_store_mut(native_prune_orphaned_graph_structures_json)?;
        serde_json::from_str(&raw).map_err(|err| PyValueError::new_err(err.to_string()))
    }

    #[pyo3(signature = (node, file_hash = "", mtime_ns = 0))]
    fn upsert_node(
        &self,
        py: Python<'_>,
        node: &Bound<'_, PyAny>,
        file_hash: &str,
        mtime_ns: i64,
    ) -> PyResult<i64> {
        let node = collect_nodes(py, PyList::new(py, [node])?.as_any())?
            .pop()
            .ok_or_else(|| PyValueError::new_err("node is required"))?;
        self.with_store_mut(|store| store.upsert_node(&node, file_hash, mtime_ns))
    }

    /// Named with the Python store's underscore because callers outside the
    /// graph package (`dagayn.flows`) use it as part of the store contract.
    #[pyo3(name = "_normalize_file_path_key")]
    fn normalize_file_path_key(&self, file_path: &str) -> PyResult<String> {
        self.with_store(|store| store.normalize_file_path_key(file_path))
    }

    /// See [`Self::normalize_file_path_key`]; used by `dagayn.incremental_build`.
    #[pyo3(name = "_normalize_qualified_key")]
    fn normalize_qualified_key(&self, qualified_name: &str) -> PyResult<String> {
        self.with_store(|store| store.normalize_qualified_key(qualified_name))
    }

    /// No-op: the native store keeps no derived in-memory graph to invalidate.
    ///
    /// Present so write paths can call it unconditionally instead of probing.
    #[pyo3(name = "_invalidate_cache")]
    fn invalidate_cache(&self) {}

    fn upsert_edge(&self, py: Python<'_>, edge: &Bound<'_, PyAny>) -> PyResult<i64> {
        let edge = collect_edges(py, PyList::new(py, [edge])?.as_any())?
            .pop()
            .ok_or_else(|| PyValueError::new_err("edge is required"))?;
        self.with_store_mut(|store| store.upsert_edge(&edge))
    }

    /// Release one lease, closing the connection once nothing holds it.
    ///
    /// Idle stores close even when `_pinned` is set: a leftover reader
    /// connection overlapping a writer used to tear `sqlite_master`. Concurrent
    /// overlapping leases still share the handle until the last `close()`.
    fn close(&self) -> PyResult<()> {
        let remaining = {
            let mut leases = self
                .leases
                .lock()
                .map_err(|err| PyRuntimeError::new_err(err.to_string()))?;
            if *leases > 0 {
                *leases -= 1;
            }
            *leases
        };
        if remaining > 0 {
            return Ok(());
        }
        self._force_close()
    }

    /// Drop the SQLite connection regardless of leases.
    fn _force_close(&self) -> PyResult<()> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| PyRuntimeError::new_err("GraphStore lock poisoned"))?;
        // Dropping the native store closes the connection, releasing its file
        // locks and its mmap of the database.
        guard.take();
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
        let store = guard.as_ref().ok_or_else(closed_store_error)?;
        f(store).map_err(to_py_runtime_error)
    }

    fn with_store_mut<T>(
        &self,
        f: impl FnOnce(&mut NativeGraphStore) -> dagayn_core::Result<T>,
    ) -> PyResult<T> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| PyRuntimeError::new_err("GraphStore lock poisoned"))?;
        let store = guard.as_mut().ok_or_else(closed_store_error)?;
        f(store).map_err(to_py_runtime_error)
    }

    fn extend_pending_rust_changed(
        &self,
        changed: Vec<(String, CachedRustChangedFile)>,
    ) -> PyResult<()> {
        if changed.is_empty() {
            return Ok(());
        }
        let mut pending = self
            .pending_rust_changed
            .lock()
            .map_err(|_| PyRuntimeError::new_err("Rust changed-file cache lock poisoned"))?;
        pending.extend(changed);
        Ok(())
    }

    fn take_pending_rust_changed(
        &self,
        file_paths: &[String],
    ) -> PyResult<HashMap<String, CachedRustChangedFile>> {
        let mut pending = self
            .pending_rust_changed
            .lock()
            .map_err(|_| PyRuntimeError::new_err("Rust changed-file cache lock poisoned"))?;
        let mut cached = HashMap::new();
        for file_path in file_paths {
            if let Some(entry) = pending.remove(file_path) {
                cached.insert(file_path.clone(), entry);
            }
        }
        Ok(cached)
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
    let mtime_ns = match obj.get_item(4) {
        Ok(value) => value.extract()?,
        Err(_) => 0,
    };
    Ok((
        file_path,
        collect_nodes(py, &nodes_obj)?,
        collect_edges(py, &edges_obj)?,
        file_hash,
        mtime_ns,
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
    parser: &mut dagayn_core::parser::RustOwnedParser,
    repo_root: &std::path::Path,
    file_path: &str,
    source: &[u8],
) -> (Vec<NodeInput>, Vec<EdgeInput>) {
    let (nodes, edges) = parser.parse_file_in_repo(Some(repo_root), file_path, source);
    (
        nodes.into_iter().map(parsed_node_to_input).collect(),
        edges.into_iter().map(parsed_edge_to_input).collect(),
    )
}

fn collect_rust_owned_file_batch(
    repo_root: &std::path::Path,
    file_paths: Vec<String>,
) -> RustFileBatchSummary {
    let mut batch = Vec::new();
    let mut errors = Vec::new();
    let mut total_nodes = 0_usize;
    let mut total_edges = 0_usize;
    let mut parser = dagayn_core::parser::RustOwnedParser::new();

    for file_path in file_paths {
        let full_path = repo_root.join(&file_path);
        let source = match std::fs::read(&full_path) {
            Ok(source) => source,
            Err(err) => {
                errors.push((file_path, err.to_string()));
                continue;
            }
        };
        if !dagayn_core::parser::rust_parser_owns_source(&file_path, &source) {
            errors.push((file_path, "unsupported Rust parser path".to_string()));
            continue;
        }
        let mtime_ns = file_mtime_ns(&full_path).unwrap_or(0);
        let (nodes, edges) =
            parse_rust_owned_file_inputs(&mut parser, repo_root, &file_path, &source);
        total_nodes += nodes.len();
        total_edges += edges.len();
        batch.push((file_path, nodes, edges, sha256_hex(&source), mtime_ns));
    }

    (batch, total_nodes, total_edges, errors)
}

fn collect_changed_rust_owned_file_batch(
    repo_root: &std::path::Path,
    file_paths: Vec<String>,
    file_meta: &HashMap<String, (String, i64)>,
    mut cached: HashMap<String, CachedRustChangedFile>,
) -> RustChangedFileBatchSummary {
    let mut batch = Vec::new();
    let mut mtime_updates = Vec::new();
    let mut errors = Vec::new();
    let mut total_nodes = 0_usize;
    let mut total_edges = 0_usize;
    let mut parser = dagayn_core::parser::RustOwnedParser::new();

    for file_path in file_paths {
        let cached_entry = cached.remove(&file_path).and_then(|entry| {
            file_mtime_ns(&repo_root.join(&file_path))
                .ok()
                .filter(|mtime_ns| *mtime_ns == entry.mtime_ns)
                .map(|_| (entry.source, entry.file_hash, entry.mtime_ns))
        });
        let Some((source, file_hash, mtime_ns)) = cached_entry.or_else(|| {
            changed_rust_owned_file_source(
                repo_root,
                &file_path,
                file_meta,
                &mut mtime_updates,
                &mut errors,
            )
        }) else {
            continue;
        };
        if !dagayn_core::parser::rust_parser_owns_source(&file_path, &source) {
            errors.push((file_path, "unsupported Rust parser path".to_string()));
            continue;
        }
        let (nodes, edges) =
            parse_rust_owned_file_inputs(&mut parser, repo_root, &file_path, &source);
        total_nodes += nodes.len();
        total_edges += edges.len();
        batch.push((file_path, nodes, edges, file_hash, mtime_ns));
    }

    (batch, mtime_updates, total_nodes, total_edges, errors)
}

fn classify_changed_rust_owned_file_batch(
    repo_root: &std::path::Path,
    file_paths: Vec<String>,
    file_meta: &HashMap<String, (String, i64)>,
) -> RustClassifiedFilesSummary {
    let mut changed_files = Vec::new();
    let mut mtime_updates = Vec::new();
    let mut errors = Vec::new();

    for file_path in file_paths {
        if let Some((source, file_hash, mtime_ns)) = changed_rust_owned_file_source(
            repo_root,
            &file_path,
            file_meta,
            &mut mtime_updates,
            &mut errors,
        ) {
            if !dagayn_core::parser::rust_parser_owns_source(&file_path, &source) {
                errors.push((file_path, "unsupported Rust parser path".to_string()));
                continue;
            }
            changed_files.push((
                file_path,
                CachedRustChangedFile {
                    source,
                    file_hash,
                    mtime_ns,
                },
            ));
        }
    }

    (changed_files, mtime_updates, errors)
}

fn changed_rust_owned_file_source(
    repo_root: &std::path::Path,
    file_path: &str,
    file_meta: &HashMap<String, (String, i64)>,
    mtime_updates: &mut Vec<(String, i64)>,
    errors: &mut Vec<(String, String)>,
) -> Option<(Vec<u8>, String, i64)> {
    let full_path = repo_root.join(file_path);
    let mtime_ns = match file_mtime_ns(&full_path) {
        Ok(mtime_ns) => mtime_ns,
        Err(err) => {
            errors.push((file_path.to_string(), err.to_string()));
            return None;
        }
    };
    // No mtime short-circuit here: these paths come from `git diff`/`git
    // status`, which has already reported them changed. An mtime can be equal
    // for a file whose bytes differ (`cp -p`/`rsync -a`/`tar x` restore it, and
    // coarse filesystem granularity hides two writes in one tick), and skipping
    // on that basis left the file un-indexed forever, because the stored hash
    // stayed stale too. The hash comparison below still avoids re-parsing when
    // the content really is unchanged.
    let source = match std::fs::read(&full_path) {
        Ok(source) => source,
        Err(err) => {
            errors.push((file_path.to_string(), err.to_string()));
            return None;
        }
    };
    let file_hash = sha256_hex(&source);
    if file_meta
        .get(file_path)
        .is_some_and(|(stored_hash, _)| *stored_hash == file_hash)
    {
        mtime_updates.push((file_path.to_string(), mtime_ns));
        return None;
    }
    Some((source, file_hash, mtime_ns))
}

fn file_mtime_ns(path: &std::path::Path) -> std::io::Result<i64> {
    let modified = std::fs::metadata(path)?.modified()?;
    // A pre-1970 mtime makes `duration_since(UNIX_EPOCH)` fail. Returning 0
    // there disagreed with Python's `st_mtime_ns`, which returns the negative
    // offset — so the same file got different metadata depending on which
    // backend indexed it, and the mtime fast paths compare exactly that value.
    Ok(match modified.duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos().min(i64::MAX as u128) as i64,
        Err(err) => -(err.duration().as_nanos().min(i64::MAX as u128) as i64),
    })
}

fn parsed_node_to_input(node: dagayn_core::parser::ParsedNode) -> NodeInput {
    NodeInput {
        kind: node.kind.as_str().to_string(),
        name: node.name,
        file_path: node.file_path.to_string(),
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
        kind: edge.kind.as_str().to_string(),
        source: edge.source,
        target: edge.target,
        file_path: edge.file_path.to_string(),
        line: edge.line,
        extra: edge.extra,
    }
}

fn sha256_hex(source: &[u8]) -> String {
    hex_digest(Sha256::digest(source))
}

fn hex_digest(digest: impl AsRef<[u8]>) -> String {
    let digest = digest.as_ref();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(out, "{:02x}", *byte);
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
    if let Some(value) = py_any_to_json(&value)? {
        return Ok(value);
    }
    let json = PyModule::import(py, "json")?;
    let raw: String = json.getattr("dumps")?.call1((value,))?.extract()?;
    serde_json::from_str(&raw).map_err(|err| PyValueError::new_err(err.to_string()))
}

fn py_any_to_json(value: &Bound<'_, PyAny>) -> PyResult<Option<Value>> {
    if value.is_none() {
        return Ok(Some(Value::Null));
    }
    if let Ok(value) = value.extract::<bool>() {
        return Ok(Some(Value::Bool(value)));
    }
    if let Ok(value) = value.extract::<i64>() {
        return Ok(Some(Value::Number(value.into())));
    }
    if let Ok(value) = value.extract::<u64>() {
        return Ok(Some(Value::Number(value.into())));
    }
    if let Ok(value) = value.extract::<f64>() {
        return serde_json::Number::from_f64(value)
            .map(|value| Some(Value::Number(value)))
            .ok_or_else(|| PyValueError::new_err("invalid JSON number"));
    }
    if let Ok(value) = value.extract::<String>() {
        return Ok(Some(Value::String(value)));
    }
    if let Ok(dict) = value.cast::<PyDict>() {
        let mut out = serde_json::Map::with_capacity(dict.len());
        for (key, value) in dict.iter() {
            let key: String = key.extract()?;
            let Some(value) = py_any_to_json(&value)? else {
                return Ok(None);
            };
            out.insert(key, value);
        }
        return Ok(Some(Value::Object(out)));
    }
    if let Ok(list) = value.cast::<PyList>() {
        let mut out = Vec::with_capacity(list.len());
        for value in list.iter() {
            let Some(value) = py_any_to_json(&value)? else {
                return Ok(None);
            };
            out.push(value);
        }
        return Ok(Some(Value::Array(out)));
    }
    if let Ok(tuple) = value.cast::<PyTuple>() {
        let mut out = Vec::with_capacity(tuple.len());
        for value in tuple.iter() {
            let Some(value) = py_any_to_json(&value)? else {
                return Ok(None);
            };
            out.push(value);
        }
        return Ok(Some(Value::Array(out)));
    }
    Ok(None)
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
    let node_cls = types.getattr("GraphNode")?;
    for node in nodes {
        let node_id = node.id;
        let qualified_name = node.qualified_name.clone();
        let py_node = graph_node_to_py_with_cls(py, &node_cls, node)?;
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

/// The `ImpactRadiusResult` TypedDict the Python store returns.
fn impact_radius_to_py(py: Python<'_>, radius: ImpactRadius) -> PyResult<Py<PyAny>> {
    let out = PyDict::new(py);
    out.set_item(
        "changed_nodes",
        graph_nodes_to_py_vec(py, radius.changed_nodes)?,
    )?;
    out.set_item(
        "impacted_nodes",
        graph_nodes_to_py_vec(py, radius.impacted_nodes)?,
    )?;
    out.set_item("impacted_files", radius.impacted_files)?;
    out.set_item("edges", graph_edges_to_py_vec(py, radius.edges)?)?;
    out.set_item(
        "bridge_transitions",
        radius
            .bridge_transitions
            .iter()
            .map(|value| json_value_to_py(py, value))
            .collect::<PyResult<Vec<_>>>()?,
    )?;
    out.set_item(
        "low_confidence_bridges",
        radius
            .low_confidence_bridges
            .iter()
            .map(|value| json_value_to_py(py, value))
            .collect::<PyResult<Vec<_>>>()?,
    )?;
    out.set_item("truncated", radius.truncated)?;
    out.set_item("total_impacted", radius.total_impacted)?;
    Ok(out.unbind().into_any())
}

fn graph_node_to_py(py: Python<'_>, node: GraphNode) -> PyResult<Py<PyAny>> {
    let types = PyModule::import(py, "dagayn.graph.types")?;
    let cls = types.getattr("GraphNode")?;
    graph_node_to_py_with_cls(py, &cls, node)
}

fn graph_node_to_py_with_cls(
    py: Python<'_>,
    cls: &Bound<'_, PyAny>,
    node: GraphNode,
) -> PyResult<Py<PyAny>> {
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
            node.signature.into_pyobject(py)?.into_any(),
        ],
    )?;
    Ok(cls.call1(args)?.unbind())
}

fn graph_nodes_to_py_vec(py: Python<'_>, nodes: Vec<GraphNode>) -> PyResult<Vec<Py<PyAny>>> {
    let types = PyModule::import(py, "dagayn.graph.types")?;
    let cls = types.getattr("GraphNode")?;
    nodes
        .into_iter()
        .map(|node| graph_node_to_py_with_cls(py, &cls, node))
        .collect()
}

fn node_map_by_string_to_py(
    py: Python<'_>,
    nodes_by_key: std::collections::HashMap<String, GraphNode>,
) -> PyResult<Py<PyAny>> {
    let types = PyModule::import(py, "dagayn.graph.types")?;
    let cls = types.getattr("GraphNode")?;
    let out = PyDict::new(py);
    for (key, node) in nodes_by_key {
        out.set_item(key, graph_node_to_py_with_cls(py, &cls, node)?.bind(py))?;
    }
    Ok(out.unbind().into_any())
}

fn node_list_map_to_py(
    py: Python<'_>,
    nodes_by_key: std::collections::HashMap<String, Vec<GraphNode>>,
) -> PyResult<Py<PyAny>> {
    let types = PyModule::import(py, "dagayn.graph.types")?;
    let cls = types.getattr("GraphNode")?;
    let out = PyDict::new(py);
    for (key, nodes) in nodes_by_key {
        let list = PyList::empty(py);
        for node in nodes {
            list.append(graph_node_to_py_with_cls(py, &cls, node)?.bind(py))?;
        }
        out.set_item(key, list)?;
    }
    Ok(out.unbind().into_any())
}

fn node_map_by_id_to_py(
    py: Python<'_>,
    nodes_by_id: std::collections::HashMap<i64, GraphNode>,
) -> PyResult<Py<PyAny>> {
    let types = PyModule::import(py, "dagayn.graph.types")?;
    let cls = types.getattr("GraphNode")?;
    let out = PyDict::new(py);
    for (node_id, node) in nodes_by_id {
        out.set_item(node_id, graph_node_to_py_with_cls(py, &cls, node)?.bind(py))?;
    }
    Ok(out.unbind().into_any())
}

fn graph_edge_to_py_with_cls(
    py: Python<'_>,
    cls: &Bound<'_, PyAny>,
    edge: GraphEdge,
) -> PyResult<Py<PyAny>> {
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
            edge.confidence_tier.as_str(),
        ))?
        .unbind())
}

fn graph_edges_to_py_vec(py: Python<'_>, edges: Vec<GraphEdge>) -> PyResult<Vec<Py<PyAny>>> {
    let types = PyModule::import(py, "dagayn.graph.types")?;
    let cls = types.getattr("GraphEdge")?;
    edges
        .into_iter()
        .map(|edge| graph_edge_to_py_with_cls(py, &cls, edge))
        .collect()
}

fn edge_map_to_py(
    py: Python<'_>,
    edges_by_key: std::collections::HashMap<String, Vec<GraphEdge>>,
) -> PyResult<Py<PyAny>> {
    let types = PyModule::import(py, "dagayn.graph.types")?;
    let cls = types.getattr("GraphEdge")?;
    let out = PyDict::new(py);
    for (key, edges) in edges_by_key {
        let list = PyList::empty(py);
        for edge in edges {
            list.append(graph_edge_to_py_with_cls(py, &cls, edge)?.bind(py))?;
        }
        out.set_item(key, list)?;
    }
    Ok(out.unbind().into_any())
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
            stats.languages.into_vec(),
            stats.files_count,
            stats.last_updated,
        ))?
        .unbind())
}

fn json_value_to_py(py: Python<'_>, value: &Value) -> PyResult<Py<PyAny>> {
    match value {
        Value::Null => Ok(py.None()),
        Value::Bool(value) => Ok(PyBool::new(py, *value).to_owned().unbind().into_any()),
        Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                Ok(value.into_pyobject(py)?.unbind().into_any())
            } else if let Some(value) = value.as_u64() {
                Ok(value.into_pyobject(py)?.unbind().into_any())
            } else if let Some(value) = value.as_f64() {
                Ok(value.into_pyobject(py)?.unbind().into_any())
            } else {
                Err(PyValueError::new_err("invalid JSON number"))
            }
        }
        Value::String(value) => Ok(value.into_pyobject(py)?.unbind().into_any()),
        Value::Array(values) => {
            let list = PyList::empty(py);
            for value in values {
                let value = json_value_to_py(py, value)?;
                list.append(value.bind(py))?;
            }
            Ok(list.unbind().into_any())
        }
        Value::Object(values) => {
            let dict = PyDict::new(py);
            for (key, value) in values {
                let value = json_value_to_py(py, value)?;
                dict.set_item(key, value.bind(py))?;
            }
            Ok(dict.unbind().into_any())
        }
    }
}

fn to_py_runtime_error(err: dagayn_core::GraphError) -> PyErr {
    PyRuntimeError::new_err(err.to_string())
}

fn closed_store_error() -> PyErr {
    PyRuntimeError::new_err("GraphStore is closed: open a new store for further queries")
}

#[pymodule]
fn _core(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PyGraphStore>()?;
    module.add_function(wrap_pyfunction!(filter_parseable_files, module)?)?;
    module.add_function(wrap_pyfunction!(filter_incremental_candidates, module)?)?;
    module.add_function(wrap_pyfunction!(collect_parseable_files, module)?)?;
    module.add_function(wrap_pyfunction!(
        parse_rust_owned_files_compact_json,
        module
    )?)?;
    module.add_function(wrap_pyfunction!(
        parse_rust_owned_file_compact_json,
        module
    )?)?;
    module.add_function(wrap_pyfunction!(parse_markdown_compact_json, module)?)?;
    module.add_function(wrap_pyfunction!(parse_terraform_compact_json, module)?)?;
    module.add_function(wrap_pyfunction!(parse_rust_compact_json, module)?)?;
    module.add_function(wrap_pyfunction!(parse_python_compact_json, module)?)?;
    module.add_function(wrap_pyfunction!(embedding_search, module)?)?;
    module.add_function(wrap_pyfunction!(embedding_search_prewarm, module)?)?;
    Ok(())
}

#[pyfunction]
fn embedding_search(
    py: Python<'_>,
    db_path: &Bound<'_, PyAny>,
    provider: &str,
    query_vec: Vec<f32>,
    limit: usize,
) -> PyResult<Vec<(String, f32)>> {
    let os = PyModule::import(py, "os")?;
    let db_path: String = os.getattr("fspath")?.call1((db_path,))?.extract()?;
    py.detach(|| dagayn_core::embedding_search(db_path, provider, &query_vec, limit))
        .map_err(to_py_runtime_error)
}

#[pyfunction]
fn embedding_search_prewarm(
    py: Python<'_>,
    db_path: &Bound<'_, PyAny>,
    provider: &str,
) -> PyResult<usize> {
    let os = PyModule::import(py, "os")?;
    let db_path: String = os.getattr("fspath")?.call1((db_path,))?.extract()?;
    py.detach(|| dagayn_core::embedding_search_prewarm(db_path, provider))
        .map_err(to_py_runtime_error)
}

#[pyfunction]
fn filter_incremental_candidates(
    py: Python<'_>,
    repo_root: &Bound<'_, PyAny>,
    candidates: Vec<String>,
    ignore_patterns: Vec<String>,
) -> PyResult<(Vec<String>, Vec<String>)> {
    let os = PyModule::import(py, "os")?;
    let repo_root: String = os.getattr("fspath")?.call1((repo_root,))?.extract()?;
    Ok(dagayn_core::parser::filter_incremental_candidates(
        std::path::Path::new(&repo_root),
        &candidates,
        &ignore_patterns,
    ))
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
fn parse_rust_owned_file_compact_json(file_path: &str, source: &[u8]) -> PyResult<String> {
    Ok(dagayn_core::parser::parse_rust_owned_file_compact_json(
        file_path, source,
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

#[pyfunction]
fn parse_rust_compact_json(file_path: &str, source: &[u8]) -> PyResult<String> {
    Ok(dagayn_core::parser::parse_rust_compact_json(
        file_path, source,
    ))
}

#[pyfunction]
fn parse_python_compact_json(file_path: &str, source: &[u8]) -> PyResult<String> {
    Ok(dagayn_core::parser::parse_python_compact_json(
        file_path, source,
    ))
}
