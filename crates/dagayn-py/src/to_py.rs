//! Building Python objects from graph results, and error conversion.

use super::*;

pub(crate) fn flow_adjacency_to_py(
    py: Python<'_>,
    nodes: Vec<GraphNode>,
    calls_out: std::collections::HashMap<String, Vec<String>>,
    has_tested_by: std::collections::HashSet<String>,
) -> PyResult<Py<PyAny>> {
    let cls = graph_type(py, "FlowAdjacency")?;

    let py_calls_out = PyDict::new(py);
    for (source, targets) in calls_out {
        let py_targets = PyList::new(py, targets)?;
        py_calls_out.set_item(source, py_targets)?;
    }

    let py_has_tested_by = PySet::new(py, has_tested_by)?;
    let py_nodes_by_qn = PyDict::new(py);
    let py_nodes_by_id = PyDict::new(py);
    let node_cls = graph_type(py, "GraphNode")?;
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
pub(crate) fn impact_radius_to_py(py: Python<'_>, radius: ImpactRadius) -> PyResult<Py<PyAny>> {
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

pub(crate) fn graph_node_to_py(py: Python<'_>, node: GraphNode) -> PyResult<Py<PyAny>> {
    let cls = graph_type(py, "GraphNode")?;
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

pub(crate) fn graph_nodes_to_py_vec(
    py: Python<'_>,
    nodes: Vec<GraphNode>,
) -> PyResult<Vec<Py<PyAny>>> {
    let cls = graph_type(py, "GraphNode")?;
    nodes
        .into_iter()
        .map(|node| graph_node_to_py_with_cls(py, &cls, node))
        .collect()
}

pub(crate) fn node_map_to_py<'py, K: IntoPyObject<'py>>(
    py: Python<'py>,
    nodes_by_key: std::collections::HashMap<K, GraphNode>,
) -> PyResult<Py<PyAny>> {
    let cls = graph_type(py, "GraphNode")?;
    let out = PyDict::new(py);
    for (key, node) in nodes_by_key {
        out.set_item(key, graph_node_to_py_with_cls(py, &cls, node)?.bind(py))?;
    }
    Ok(out.unbind().into_any())
}

pub(crate) fn node_list_map_to_py(
    py: Python<'_>,
    nodes_by_key: std::collections::HashMap<String, Vec<GraphNode>>,
) -> PyResult<Py<PyAny>> {
    let cls = graph_type(py, "GraphNode")?;
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

pub(crate) fn graph_edges_to_py_vec(
    py: Python<'_>,
    edges: Vec<GraphEdge>,
) -> PyResult<Vec<Py<PyAny>>> {
    let cls = graph_type(py, "GraphEdge")?;
    edges
        .into_iter()
        .map(|edge| graph_edge_to_py_with_cls(py, &cls, edge))
        .collect()
}

pub(crate) fn edge_map_to_py(
    py: Python<'_>,
    edges_by_key: std::collections::HashMap<String, Vec<GraphEdge>>,
) -> PyResult<Py<PyAny>> {
    let cls = graph_type(py, "GraphEdge")?;
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

pub(crate) fn graph_stats_to_py(py: Python<'_>, stats: GraphStats) -> PyResult<Py<PyAny>> {
    let cls = graph_type(py, "GraphStats")?;
    let nodes_by_kind = stats.nodes_by_kind.into_py_dict(py)?;
    let edges_by_kind = stats.edges_by_kind.into_py_dict(py)?;
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

pub(crate) fn json_value_to_py(py: Python<'_>, value: &Value) -> PyResult<Py<PyAny>> {
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

/// A class from `dagayn.graph.types`.
pub(crate) fn graph_type<'py>(py: Python<'py>, name: &str) -> PyResult<Bound<'py, PyAny>> {
    PyModule::import(py, "dagayn.graph.types")?.getattr(name)
}

pub(crate) fn to_py_runtime_error(err: dagayn_graph::GraphError) -> PyErr {
    PyRuntimeError::new_err(err.to_string())
}

pub(crate) fn closed_store_error() -> PyErr {
    PyRuntimeError::new_err("GraphStore is closed: open a new store for further queries")
}
