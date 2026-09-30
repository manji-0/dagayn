//! Reading Python objects (dataclasses, dicts, JSON-able values) into graph inputs.

use super::*;

pub(crate) fn collect_nodes(py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<Vec<NodeInput>> {
    let iterator = PyIterator::from_object(value)?;
    iterator
        .map(|item| node_from_py(py, &item?))
        .collect::<PyResult<Vec<_>>>()
}

pub(crate) fn collect_edges(py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<Vec<EdgeInput>> {
    let iterator = PyIterator::from_object(value)?;
    iterator
        .map(|item| edge_from_py(py, &item?))
        .collect::<PyResult<Vec<_>>>()
}

pub(crate) fn collect_batch(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
) -> PyResult<Vec<FileBatchItem>> {
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
