use std::sync::{Mutex, MutexGuard};

use dagayn_graph::{
    EdgeInput, FileBatchItem, GraphEdge, GraphNode, GraphStats, GraphStore as NativeGraphStore,
    ImpactRadius, NodeInput, NodeSignatureRow,
};
use dagayn_postproc::{
    detect_communities_json as native_detect_communities_json,
    incremental_detect_communities as native_incremental_detect_communities,
    prune_orphaned_graph_structures as native_prune_orphaned_graph_structures,
    refresh_community_stats_json as native_refresh_community_stats_json,
    run_post_processing_json as native_run_post_processing_json,
};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{
    IntoPyDict, PyAny, PyBool, PyDict, PyIterator, PyList, PyModule, PySet, PyTuple,
};
use serde_json::Value;
use std::collections::HashMap;

mod from_py;
mod hints;
mod pending;
mod to_py;

use dagayn_build::parse_batch::*;
use from_py::*;
use to_py::*;

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
        Ok(pathlib_path(py, &self.db_path)?.unbind())
    }

    #[getter(_pinned)]
    fn get_pinned(&self) -> PyResult<bool> {
        lock(&self.pinned).map(|value| *value)
    }

    #[setter(_pinned)]
    fn set_pinned(&self, value: bool) -> PyResult<()> {
        *lock(&self.pinned)? = value;
        Ok(())
    }

    #[getter(_leases)]
    fn get_leases(&self) -> PyResult<i64> {
        lock(&self.leases).map(|value| *value)
    }

    #[setter(_leases)]
    fn set_leases(&self, value: i64) -> PyResult<()> {
        *lock(&self.leases)? = value;
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
            Some(value) => Ok(Some(pathlib_path(py, &value)?.unbind())),
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
        node_map_to_py(py, nodes)
    }

    fn get_nodes_by_ids(&self, py: Python<'_>, node_ids: Vec<i64>) -> PyResult<Py<PyAny>> {
        let nodes = self.with_store(|store| store.get_nodes_by_ids(&node_ids))?;
        node_map_to_py(py, nodes)
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
        Ok(community_ids.into_py_dict(py)?.unbind().into_any())
    }

    fn get_community_ids_by_qualified_names(
        &self,
        py: Python<'_>,
        qns: Vec<String>,
    ) -> PyResult<Py<PyAny>> {
        let community_ids =
            self.with_store(|store| store.get_community_ids_by_qualified_names(&qns))?;
        Ok(community_ids.into_py_dict(py)?.unbind().into_any())
    }

    fn get_all_community_member_qns(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let members = self.with_store(|store| store.get_all_community_member_qns())?;
        Ok(members.into_py_dict(py)?.unbind().into_any())
    }

    fn get_all_community_ids(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let community_ids = self.with_store(|store| store.get_all_community_ids())?;
        Ok(community_ids.into_py_dict(py)?.unbind().into_any())
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

    fn resolve_native_bindings(&self) -> PyResult<i64> {
        self.with_store_mut(|store| store.resolve_native_bindings())
    }

    fn resolve_bare_call_targets(&self) -> PyResult<i64> {
        self.with_store_mut(|store| store.resolve_bare_call_targets())
    }

    /// Settles CALLS edges by a SCIP index; returns the overlay's counts as
    /// JSON.
    #[pyo3(signature = (index_path, prefix, repo_root, authoritative = true))]
    fn apply_scip_overlay_json(
        &self,
        index_path: &str,
        prefix: &str,
        repo_root: &str,
        authoritative: bool,
    ) -> PyResult<String> {
        self.with_store_mut(|store| {
            let stats = store.apply_scip_overlay(
                std::path::Path::new(index_path),
                prefix,
                std::path::Path::new(repo_root),
                authoritative,
            )?;
            Ok(serde_json::to_string(&stats)?)
        })
    }

    fn resolve_bare_inheritance_targets(&self) -> PyResult<i64> {
        self.with_store_mut(|store| store.resolve_bare_inheritance_targets())
    }

    fn resolve_terraform_module_references(&self) -> PyResult<i64> {
        self.with_store_mut(|store| store.resolve_terraform_module_references())
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

    /// The dead-code candidates `refactor_tool(mode="dead_code")` reports.
    #[pyo3(signature = (kind = None, file_pattern = None))]
    fn find_dead_code_json(
        &self,
        kind: Option<&str>,
        file_pattern: Option<&str>,
    ) -> PyResult<String> {
        self.report_json(
            |store| dagayn_tools::find_dead_code_json(store, kind, file_pattern),
            "dead-code analysis could not read the graph",
        )
    }

    /// The refactoring suggestions `refactor_tool(mode="suggest")` starts from.
    fn suggest_refactorings_json(&self) -> PyResult<String> {
        self.report_json(
            dagayn_tools::suggest_refactorings_json,
            "refactoring suggestions could not read the graph",
        )
    }

    /// The graph's candidates before the repository check (tests only).
    #[pyo3(signature = (kind = None, file_pattern = None))]
    fn graph_dead_code_candidates_json(
        &self,
        kind: Option<&str>,
        file_pattern: Option<&str>,
    ) -> PyResult<String> {
        self.report_json(
            |store| dagayn_tools::graph_dead_code_candidates_json(store, kind, file_pattern),
            "dead-code analysis could not read the graph",
        )
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
        let nodes_map = node_map_to_py(py, nodes)?;
        let adjacency_map = adjacency.into_py_dict(py)?;
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

    fn resolve_file_path(&self, py: Python<'_>, file_path: &str) -> PyResult<Py<PyAny>> {
        let resolved = self.with_store(|store| store.resolve_file_path(file_path))?;
        Ok(pathlib_path(py, &resolved.to_string_lossy())?.unbind())
    }

    #[pyo3(signature = (query, limit = 50))]
    fn fts_query(&self, py: Python<'_>, query: &str, limit: i64) -> PyResult<Py<PyAny>> {
        let (hits, match_mode) = self.with_store(|store| store.fts_query(query, limit))?;
        Ok(graph_type(py, "FtsQueryResult")?
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

    fn prune_orphaned_graph_structures(&self) -> PyResult<std::collections::HashMap<String, i64>> {
        self.with_store_mut(native_prune_orphaned_graph_structures)
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
            let mut leases = lock(&self.leases)?;
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
        let mut guard = self.store_guard()?;
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
    fn store_guard(&self) -> PyResult<MutexGuard<'_, Option<NativeGraphStore>>> {
        self.inner
            .lock()
            .map_err(|_| PyRuntimeError::new_err("GraphStore lock poisoned"))
    }

    fn pending_guard(&self) -> PyResult<MutexGuard<'_, HashMap<String, CachedRustChangedFile>>> {
        self.pending_rust_changed
            .lock()
            .map_err(|_| PyRuntimeError::new_err("Rust changed-file cache lock poisoned"))
    }

    /// A tools JSON report from the store, or `PyRuntimeError(message)` when
    /// the report could not read the graph.
    fn report_json(
        &self,
        report: impl FnOnce(&NativeGraphStore) -> Option<String>,
        message: &'static str,
    ) -> PyResult<String> {
        self.with_store(|store| Ok(report(store)))?
            .ok_or_else(|| PyRuntimeError::new_err(message))
    }

    fn with_store<T>(
        &self,
        f: impl FnOnce(&NativeGraphStore) -> dagayn_graph::Result<T>,
    ) -> PyResult<T> {
        let guard = self.store_guard()?;
        let store = guard.as_ref().ok_or_else(closed_store_error)?;
        f(store).map_err(to_py_runtime_error)
    }

    fn with_store_mut<T>(
        &self,
        f: impl FnOnce(&mut NativeGraphStore) -> dagayn_graph::Result<T>,
    ) -> PyResult<T> {
        let mut guard = self.store_guard()?;
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
        self.pending_guard()?.extend(changed);
        Ok(())
    }

    fn take_pending_rust_changed(
        &self,
        file_paths: &[String],
    ) -> PyResult<HashMap<String, CachedRustChangedFile>> {
        let mut pending = self.pending_guard()?;
        Ok(file_paths
            .iter()
            .filter_map(|file_path| Some((file_path.clone(), pending.remove(file_path)?)))
            .collect())
    }
}

/// Lock `mutex`, mapping a poisoned lock to `PyRuntimeError`.
fn lock<T>(mutex: &Mutex<T>) -> PyResult<MutexGuard<'_, T>> {
    mutex
        .lock()
        .map_err(|err| PyRuntimeError::new_err(err.to_string()))
}

/// `pathlib.Path(value)`.
fn pathlib_path<'py>(py: Python<'py>, value: &str) -> PyResult<Bound<'py, PyAny>> {
    PyModule::import(py, "pathlib")?
        .getattr("Path")?
        .call1((value,))
}

#[pymodule]
fn _core(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PyGraphStore>()?;
    module.add_class::<hints::PyHintSession>()?;
    module.add_function(wrap_pyfunction!(bounded_simple_cycles, module)?)?;
    module.add_function(wrap_pyfunction!(pending::pending_refactor_set, module)?)?;
    module.add_function(wrap_pyfunction!(pending::pending_refactor_get, module)?)?;
    module.add_function(wrap_pyfunction!(pending::pending_refactor_remove, module)?)?;
    module.add_function(wrap_pyfunction!(pending::pending_refactor_keys, module)?)?;
    module.add_function(wrap_pyfunction!(pending::pending_refactor_clear, module)?)?;
    module.add_function(wrap_pyfunction!(hints::reset_hint_session, module)?)?;
    module.add_function(wrap_pyfunction!(filter_parseable_files, module)?)?;
    module.add_function(wrap_pyfunction!(filter_ignored_paths, module)?)?;
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
    module.add_function(wrap_pyfunction!(embedding_search, module)?)?;
    module.add_function(wrap_pyfunction!(embedding_search_prewarm, module)?)?;
    module.add_function(wrap_pyfunction!(extractor_versions, module)?)?;
    module.add_function(wrap_pyfunction!(discover_manifest_bridges_json, module)?)?;
    module.add_function(wrap_pyfunction!(resolve_manifest_path, module)?)?;
    module.add_function(wrap_pyfunction!(run_cli, module)?)?;
    module.add_function(wrap_pyfunction!(serve_mcp, module)?)?;
    module.add_function(wrap_pyfunction!(call_tool, module)?)?;
    module.add_function(wrap_pyfunction!(docs_section_json, module)?)?;
    Ok(())
}

/// The JSON text `name(arguments)` answers with in `dagayn_tools`, or `None`
/// where the Rust tool leaves the call to Python. The Python tool bodies call
/// it once `_get_store` has resolved, created, or migrated the graph, with
/// the same session facts `serve_mcp` gives the front end.
#[pyfunction]
#[allow(clippy::too_many_arguments)]
#[pyo3(signature = (name, arguments_json, allowed_tools=None, package_root=None, local_embedding=None, embedding_provider=None, embedding_model=None, runtime=None, auto_prepare=false, python_executable=None, prepare_budget_seconds=None))]
fn call_tool(
    py: Python<'_>,
    name: &str,
    arguments_json: &str,
    allowed_tools: Option<Vec<String>>,
    package_root: Option<std::path::PathBuf>,
    local_embedding: Option<String>,
    embedding_provider: Option<String>,
    embedding_model: Option<String>,
    runtime: Option<&str>,
    auto_prepare: bool,
    python_executable: Option<std::path::PathBuf>,
    prepare_budget_seconds: Option<i64>,
) -> PyResult<Option<String>> {
    let invalid = |err: serde_json::Error| pyo3::exceptions::PyValueError::new_err(err.to_string());
    let arguments: serde_json::Value = serde_json::from_str(arguments_json).map_err(invalid)?;
    let context = dagayn_tools::Context {
        pinned_repo: None,
        allowed_tools: allowed_tools.map(|names| names.into_iter().collect()),
        package_root,
        local_embedding,
        embedding_provider,
        embedding_model,
        runtime: runtime
            .map(serde_json::from_str)
            .transpose()
            .map_err(invalid)?,
        auto_prepare: auto_prepare.then_some(dagayn_tools::AutoPrepare {
            python_executable,
            budget_seconds: prepare_budget_seconds,
        }),
    };
    Ok(py.detach(|| dagayn_tools::call(&context, name, &arguments).map(|payload| payload.text)))
}

/// `get_docs_section_tool`'s answer from `search_roots`' reference files,
/// then the package's, without `_repo`: the Python tool's answer when it has
/// no graph, which `call_tool` leaves to it.
#[pyfunction]
#[pyo3(signature = (search_roots, section_name, max_chars, package_root=None))]
fn docs_section_json(
    py: Python<'_>,
    search_roots: Vec<std::path::PathBuf>,
    section_name: &str,
    max_chars: i64,
    package_root: Option<std::path::PathBuf>,
) -> Option<String> {
    py.detach(|| {
        dagayn_tools::docs_section_json(
            search_roots,
            package_root.as_deref(),
            section_name,
            max_chars,
        )
    })
}

/// Serve an MCP session on stdin and stdout until EOF with the front end of
/// `dagayn-mcp`, calling `boot(read_fd, write_fd)` to start the Python server
/// for the first message it does not answer itself. See
/// `dagayn.server.proxy`.
#[pyfunction]
// Python keyword arguments, one per session fact.
#[allow(clippy::too_many_arguments)]
#[pyo3(signature = (surface_json, allowed_tools, version, boot, pinned_repo=None, package_root=None, local_embedding=None, embedding_provider=None, embedding_model=None, runtime=None, python_executable=None))]
fn serve_mcp(
    py: Python<'_>,
    surface_json: &str,
    allowed_tools: Option<Vec<String>>,
    version: &str,
    boot: Py<PyAny>,
    pinned_repo: Option<std::path::PathBuf>,
    package_root: Option<std::path::PathBuf>,
    local_embedding: Option<String>,
    embedding_provider: Option<String>,
    embedding_model: Option<String>,
    runtime: Option<&str>,
    python_executable: Option<std::path::PathBuf>,
) -> PyResult<()> {
    struct RustTools(dagayn_tools::Context);

    impl dagayn_mcp::Native for RustTools {
        fn call_tool(
            &self,
            name: &str,
            arguments: &serde_json::Value,
        ) -> Option<(String, serde_json::Value)> {
            dagayn_tools::call(&self.0, name, arguments)
                .map(|payload| (payload.text, payload.value))
        }

        fn writes_graph(&self, name: &str) -> bool {
            dagayn_tools::writes_graph(name)
        }
    }

    struct PythonBackend(Py<PyAny>);

    impl dagayn_mcp::Backend for PythonBackend {
        fn boot(
            &mut self,
            requests: std::os::fd::OwnedFd,
            replies: std::os::fd::OwnedFd,
        ) -> std::io::Result<()> {
            use std::os::fd::IntoRawFd;
            let (read_fd, write_fd) = (requests.into_raw_fd(), replies.into_raw_fd());
            Python::attach(|py| self.0.call1(py, (read_fd, write_fd)).map(drop))
                .map_err(|err| std::io::Error::other(err.to_string()))
        }
    }

    let surface = dagayn_mcp::Surface::from_json(surface_json)
        .map_err(pyo3::exceptions::PyValueError::new_err)?;
    let config = dagayn_mcp::Config {
        surface,
        allowed_tools: allowed_tools.map(|names| names.into_iter().collect()),
        version: version.to_string(),
    };
    let tools = RustTools(dagayn_tools::Context {
        pinned_repo,
        allowed_tools: config.allowed_tools.clone(),
        package_root,
        local_embedding,
        embedding_provider,
        embedding_model,
        runtime: runtime
            .map(serde_json::from_str)
            .transpose()
            .map_err(|err| pyo3::exceptions::PyValueError::new_err(err.to_string()))?,
        // The MCP tool always runs `get_minimal_context(auto_prepare=True,
        // prepare_budget_seconds=300)`.
        auto_prepare: Some(dagayn_tools::AutoPrepare {
            python_executable,
            budget_seconds: Some(300),
        }),
    });
    let (input, output) = dagayn_mcp::claim_stdio()?;
    py.detach(|| dagayn_mcp::serve(config, input, output, PythonBackend(boot), &tools))?;
    Ok(())
}

/// Run `argv` (program name first) with the Rust CLI: its exit status, or
/// `None` when the Python CLI should run it instead, in which case nothing
/// was changed or printed. See `dagayn._cli_launcher`.
#[pyfunction]
fn run_cli(py: Python<'_>, argv: Vec<std::ffi::OsString>) -> Option<u8> {
    if !dagayn_cli::handles_command(&argv) {
        return None;
    }
    match py.detach(|| dagayn_cli::run(argv)) {
        dagayn_cli::Outcome::Exit(code) => Some(code),
        dagayn_cli::Outcome::Fallback(_) => None,
    }
}

/// Manifest bridges under `repo_root` as `{"nodes": [...], "edges": [...]}`,
/// shaped like `dataclasses.asdict` of `NodeInfo` / `EdgeInfo`, with File
/// node line ends already read from disk.
#[pyfunction]
#[pyo3(signature = (repo_root, scope=None))]
fn discover_manifest_bridges_json(
    py: Python<'_>,
    repo_root: &Bound<'_, PyAny>,
    scope: Option<Vec<String>>,
) -> PyResult<String> {
    let os = PyModule::import(py, "os")?;
    let repo_root: String = os.getattr("fspath")?.call1((repo_root,))?.extract()?;
    let scope = scope.map(|paths| paths.into_iter().collect::<std::collections::HashSet<_>>());
    let result = py.detach(|| {
        dagayn_postproc::manifest_bridges::discover_manifest_bridges(
            std::path::Path::new(&repo_root),
            scope.as_ref(),
        )
    });
    serde_json::to_string(&result).map_err(|err| PyRuntimeError::new_err(err.to_string()))
}

/// `declared` resolved against the repo-relative `base_dir`; `None` when it
/// escapes the repository root.
/// `dagayn_graph::bounded_simple_cycles`: `(cycles as indices into names,
/// cycles examined, truncated)`.
#[pyfunction]
#[pyo3(signature = (names, edges, min_size, max_length, max_cycles, max_steps))]
fn bounded_simple_cycles(
    py: Python<'_>,
    names: Vec<String>,
    edges: Vec<(usize, usize)>,
    min_size: usize,
    max_length: usize,
    max_cycles: usize,
    max_steps: usize,
) -> (Vec<Vec<usize>>, usize, bool) {
    let found = py.detach(|| {
        dagayn_graph::bounded_simple_cycles(
            &names, &edges, min_size, max_length, max_cycles, max_steps,
        )
    });
    (found.cycles, found.examined, found.truncated)
}

#[pyfunction]
fn resolve_manifest_path(base_dir: &str, declared: &str) -> Option<String> {
    dagayn_postproc::manifest_bridges::resolve_rel(base_dir, declared)
}

/// `(extractor, version, languages)` for every extractor with a tracked
/// output version; see `dagayn.extractor_versions`.
#[pyfunction]
fn extractor_versions() -> Vec<(String, u32, Vec<String>)> {
    dagayn_parser::extractor_versions()
        .iter()
        .map(|entry| {
            (
                entry.extractor.to_string(),
                entry.version,
                entry
                    .languages
                    .iter()
                    .map(|language| language.to_string())
                    .collect(),
            )
        })
        .collect()
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
    py.detach(|| dagayn_graph::embedding_search(db_path, provider, &query_vec, limit))
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
    py.detach(|| dagayn_graph::embedding_search_prewarm(db_path, provider))
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
    Ok(dagayn_parser::filter_incremental_candidates(
        std::path::Path::new(&repo_root),
        &candidates,
        &ignore_patterns,
    ))
}

#[pyfunction]
fn filter_ignored_paths(
    py: Python<'_>,
    candidates: Vec<String>,
    ignore_patterns: Vec<String>,
) -> Vec<String> {
    py.detach(|| dagayn_parser::filter_ignored_paths(&candidates, &ignore_patterns))
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
    Ok(dagayn_parser::filter_parseable_files(
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
    Ok(dagayn_parser::collect_parseable_files(
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
    Ok(dagayn_parser::parse_rust_owned_files_compact_json(
        std::path::Path::new(&repo_root),
        &file_paths,
    ))
}

#[pyfunction]
fn parse_rust_owned_file_compact_json(file_path: &str, source: &[u8]) -> PyResult<String> {
    Ok(dagayn_parser::parse_rust_owned_file_compact_json(
        file_path, source,
    ))
}
