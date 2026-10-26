//! The Python methods a runtime calls without the code naming them, and the
//! classes that are pydantic models.

use std::collections::{HashMap, HashSet};

use dagayn_graph::GraphStore;
use serde_json::Value;

/// Decorators whose methods the runtime calls on an instance's use.
const IMPLICIT_DECORATORS: &[&str] = &[
    "cached_property",
    "computed_field",
    "deleter",
    "field_serializer",
    "field_validator",
    "getter",
    "model_serializer",
    "model_validator",
    "property",
    "root_validator",
    "setter",
    "validator",
];

/// Bases that make a class a pydantic model.
const MODEL_BASES: &[&str] = &["BaseModel", "BaseSettings", "RootModel"];

/// Bases that add no method a model's method could override.
const NEUTRAL_BASES: &[&str] = &["Generic", "object"];

/// pydantic v1's `BaseModel` API, which a subclass may override and pydantic
/// or its callers then call.
const V1_MODEL_API: &[&str] = &[
    "construct",
    "copy",
    "dict",
    "from_orm",
    "json",
    "parse_file",
    "parse_obj",
    "parse_raw",
    "schema",
    "schema_json",
    "update_forward_refs",
    "validate",
];

/// Whether a method named `name` with `decorators` runs wherever its class
/// is used: a dunder (`__init__` on construction,
/// `__get_pydantic_core_schema__` when a model with a field of the class is
/// built), pydantic's `model_post_init`, or a property, validator, or
/// serializer.
pub(crate) fn runs_with_its_class(name: &str, decorators: &Value) -> bool {
    let dunder = name.len() > 4 && name.starts_with("__") && name.ends_with("__");
    dunder
        || name == "model_post_init"
        || decorators.as_array().is_some_and(|decorators| {
            decorators
                .iter()
                .filter_map(Value::as_str)
                .any(|decorator| {
                    let head = decorator.split('(').next().unwrap_or(decorator);
                    IMPLICIT_DECORATORS.contains(&head.rsplit('.').next().unwrap_or(head))
                })
        })
}

/// Whether a model method named `name` overrides a member of pydantic's
/// `BaseModel` (`model_dump`, v1's `dict`), which pydantic or its callers
/// (FastAPI serializing a response) call.
pub(crate) fn overrides_model_api(name: &str) -> bool {
    name.starts_with("model_") || V1_MODEL_API.contains(&name)
}

/// Whether pydantic calls a method of a model named `name` with
/// `decorators`.
pub(crate) fn called_by_pydantic(name: &str, decorators: &Value) -> bool {
    runs_with_its_class(name, decorators) || overrides_model_api(name)
}

/// The pydantic models of the graph: the classes whose every base, followed
/// through the repository's classes, is a pydantic model base imported from
/// pydantic (or `Generic`), with at least one of them. Each maps to the
/// names of the methods its repository ancestors define, which a method of
/// the model may override.
pub(crate) fn models(store: &GraphStore) -> Option<HashMap<String, HashSet<String>>> {
    // The local names each file binds to a model base (`BaseModel`,
    // `PydModel` of `from pydantic import BaseModel as PydModel`).
    let mut model_names: HashMap<String, HashSet<String>> = HashMap::new();
    for edge in store.get_edges_by_kind("IMPORTS_FROM", false).ok()? {
        let module = edge.extra.get("module").and_then(Value::as_str);
        if !module.is_some_and(|module| module.split('.').next().is_some_and(is_pydantic_package)) {
            continue;
        }
        for pair in edge.extra["names"].as_array().into_iter().flatten() {
            if let (Some(original), Some(local)) = (pair[0].as_str(), pair[1].as_str())
                && MODEL_BASES.contains(&original)
            {
                model_names
                    .entry(edge.file_path.clone())
                    .or_default()
                    .insert(local.to_string());
            }
        }
    }
    let mut bases: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for edge in store.get_edges_by_kind("INHERITS", false).ok()? {
        bases
            .entry(edge.source_qualified)
            .or_default()
            .push((edge.target_qualified, edge.file_path));
    }
    let mut verdicts: HashMap<String, Option<Vec<String>>> = HashMap::new();
    let classes: Vec<String> = bases.keys().cloned().collect();
    for class in &classes {
        ancestry(class, &bases, &model_names, &mut verdicts, &mut Vec::new());
    }
    let models: HashMap<String, Vec<String>> = verdicts
        .into_iter()
        .filter_map(|(class, ancestors)| Some((class, ancestors?)))
        .collect();
    let ancestors: HashSet<&str> = models.values().flatten().map(String::as_str).collect();
    let mut members: HashMap<&str, HashSet<String>> = HashMap::new();
    if !ancestors.is_empty() {
        for node in store
            .get_nodes_by_kind(&["Function".to_string()], None)
            .ok()?
        {
            if let Some((class, _)) = node.qualified_name.rsplit_once('.')
                && let Some(class) = ancestors.get(class)
            {
                members.entry(class).or_default().insert(node.name);
            }
        }
    }
    Some(
        models
            .iter()
            .map(|(class, ancestors)| {
                let names = ancestors
                    .iter()
                    .filter_map(|ancestor| members.get(ancestor.as_str()))
                    .flatten()
                    .cloned()
                    .collect();
                (class.clone(), names)
            })
            .collect(),
    )
}

fn is_pydantic_package(package: &str) -> bool {
    matches!(package, "pydantic" | "pydantic_settings")
}

/// The repository ancestors of `class` when it is a pydantic model (see
/// [`models`]), memoized in `verdicts`; `None` when it is not. `path` holds
/// the classes being decided, so an inheritance cycle ends the walk.
fn ancestry(
    class: &str,
    bases: &HashMap<String, Vec<(String, String)>>,
    model_names: &HashMap<String, HashSet<String>>,
    verdicts: &mut HashMap<String, Option<Vec<String>>>,
    path: &mut Vec<String>,
) -> Option<Vec<String>> {
    if let Some(verdict) = verdicts.get(class) {
        return verdict.clone();
    }
    if path.iter().any(|seen| seen == class) {
        return None;
    }
    path.push(class.to_string());
    let mut ancestors = Vec::new();
    let mut model = false;
    let mut verdict = Some(());
    for (base, file) in bases.get(class).into_iter().flatten() {
        if base.contains("::") {
            match ancestry(base, bases, model_names, verdicts, path) {
                Some(more) => {
                    model = true;
                    ancestors.push(base.clone());
                    ancestors.extend(more);
                }
                None => verdict = None,
            }
            continue;
        }
        let last = base.rsplit('.').next().unwrap_or(base);
        let imported = model_names
            .get(file)
            .is_some_and(|names| names.contains(base.as_str()));
        let qualified = base
            .split_once('.')
            .is_some_and(|(package, _)| is_pydantic_package(package))
            && MODEL_BASES.contains(&last);
        if imported || qualified {
            model = true;
        } else if !NEUTRAL_BASES.contains(&last) {
            verdict = None;
        }
    }
    path.pop();
    let result = verdict.filter(|()| model).map(|()| ancestors);
    verdicts.insert(class.to_string(), result.clone());
    result
}
