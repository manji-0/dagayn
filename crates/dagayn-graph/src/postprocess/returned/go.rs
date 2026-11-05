//! Go: the type of a function's first result, the standard-library
//! packages whose generic APIs may return the caller's own type, and
//! member calls typed by declared field and embedded types under Go's
//! selector rule.

use crate::*;

use super::{Graph, split_arguments, update_call};

/// The type of a Go function's first result, without its name:
/// `io.ReadCloser` for `(rc io.ReadCloser, err error)`, which read whole
/// names no type (`rc io` is no package).
pub(super) fn go_first_result(return_type: &str) -> String {
    let text = return_type.trim();
    let Some(inner) = text
        .strip_prefix('(')
        .and_then(|rest| rest.strip_suffix(')'))
    else {
        return text.to_string();
    };
    let Some(first) = split_arguments(inner).into_iter().next() else {
        return text.to_string();
    };
    match first.trim().split_once(char::is_whitespace) {
        Some((name, ty))
            if name.chars().all(|c| c.is_alphanumeric() || c == '_') && !ty.trim().is_empty() =>
        {
            ty.trim().to_string()
        }
        _ => first.trim().to_string(),
    }
}

/// Standard-library packages with type parameters in their API, whose
/// functions and generic types' methods may return a value of the caller's
/// own type (`slices.MaxFunc(nodes, f)`, `cmp.Or(a, b)`, `p.Load()` on `p
/// atomic.Pointer[Node]`, `sync.OnceValue(f)`, `unique.Make(v).Value()`),
/// and the builtins (`min`, `max`). The graph does not index the standard
/// library, so it cannot tell a generic function from another one there.
const GO_GENERIC_STD_PACKAGES: &[&str] = &[
    "builtin",
    "cmp",
    "container/heap",
    "container/list",
    "container/ring",
    "iter",
    "maps",
    "math/rand/v2",
    "slices",
    "sync",
    "sync/atomic",
    "unique",
    "weak",
];

/// Whether a call into the Go package `package` may return a value of the
/// calling module's own type, so a method on its result may be the module's.
pub(super) fn go_may_return_callers_type(package: &str) -> bool {
    GO_GENERIC_STD_PACKAGES.contains(&package)
}

/// The directory of a file or of a node's QN: a Go package.
pub(crate) fn go_directory(qualified: &str) -> &str {
    let file = qualified
        .split_once("::")
        .map_or(qualified, |(file, _)| file);
    file.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// What a Go selector `x.name` names on a value of the type `class`.
#[derive(Debug, PartialEq, Eq)]
enum GoMember<'g> {
    Method(&'g String),
    /// A field, with its type as `field_types` spells it.
    Field(&'g str),
}

/// How deep embedded types are followed.
const GO_MAX_EMBEDDING_DEPTH: usize = 4;

/// The Go type of the name `name` in the directory `dir`, if exactly one.
fn go_class_in<'g>(graph: &'g Graph, name: &str, dir: &str) -> Option<&'g String> {
    let classes = graph.classes.get(&("go", name.to_string()))?;
    let mut found = classes.iter().filter(|class| go_directory(class) == dir);
    let only = found.next()?;
    found.next().is_none().then_some(only)
}

/// The Go type a `field_types` or `embedded` entry names, seen from the
/// directory `dir`: `Name` is that package's, `import/path.Name` the
/// package's of that import path.
fn go_type_class<'g>(graph: &'g Graph, ty: &str, dir: &str) -> Option<&'g String> {
    match ty.rsplit_once('.') {
        Some((path, name)) => {
            let dir = graph.go_packages.get(path)?.as_deref()?;
            go_class_in(graph, name, dir)
        }
        None => go_class_in(graph, ty, dir),
    }
}

/// The member `name` of a value of the Go type `class` by Go's selector
/// rule: the type's own method or field, else the one member of that name
/// among the types it embeds at the shallowest depth. Two at one depth are
/// ambiguous, and a depth with an embedded type whose members are unknown
/// (of a package outside the repository, an interface, a type of no
/// `field_types`) hides the deeper ones; both give none.
fn go_member<'g>(graph: &'g Graph, class: &str, name: &str) -> Option<GoMember<'g>> {
    let mut level = vec![class.to_string()];
    let mut seen = HashSet::new();
    for depth in 0..=GO_MAX_EMBEDDING_DEPTH {
        let mut hits = Vec::new();
        let mut opaque = false;
        let mut next = Vec::new();
        for class in &level {
            if !seen.insert(class.clone()) {
                continue;
            }
            let dir = go_directory(class);
            let symbol = class
                .split_once("::")
                .map_or(class.as_str(), |(_, symbol)| symbol);
            let type_name = symbol.rsplit('.').next().unwrap_or(symbol);
            hits.extend(
                graph
                    .methods
                    .get(&("go", type_name.to_string(), name.to_string()))
                    .into_iter()
                    .flatten()
                    .filter(|method| go_directory(method) == dir)
                    .map(GoMember::Method),
            );
            let Some(fields) = graph.fields.get(class) else {
                opaque |= depth > 0;
                continue;
            };
            if let Some(field_type) = fields.get(name) {
                hits.push(GoMember::Field(field_type));
            }
            for embedded in graph.embedded.get(class).into_iter().flatten() {
                match go_type_class(graph, embedded, dir) {
                    Some(inner) => next.push(inner.clone()),
                    None => opaque = true,
                }
            }
        }
        if !hits.is_empty() {
            return (hits.len() == 1).then(|| hits.pop()).flatten();
        }
        if opaque || next.is_empty() {
            return None;
        }
        level = next;
    }
    None
}

/// The Go type of the field at `path` from the type `owner` (`state.jobs`,
/// `nodes[].x`: `[]` takes an element), an element of it when `element`.
fn go_field_class<'g>(
    graph: &'g Graph,
    owner: &'g String,
    path: &str,
    element: bool,
) -> Option<&'g String> {
    let segments = path.split('.').collect::<Vec<_>>();
    let mut class = owner;
    for (index, segment) in segments.iter().enumerate() {
        let (name, mut take_element) = match segment.strip_suffix("[]") {
            Some(name) => (name, true),
            None => (*segment, false),
        };
        take_element |= element && index + 1 == segments.len();
        let Some(GoMember::Field(field_type)) = go_member(graph, class, name) else {
            return None;
        };
        let field_type = if take_element {
            field_type.strip_prefix("[]")?
        } else {
            field_type
        };
        if field_type.is_empty() || field_type.starts_with("[]") {
            return None;
        }
        class = go_type_class(graph, field_type, go_directory(class))?;
    }
    Some(class)
}

/// Go member calls typed by what the code declares, which one file cannot
/// finish: `s.bw.close()`, `e.state.jobs.put()`, and `br := &s.br;
/// br.advance()` on a struct of another file (`receiver_field`: owner,
/// field path, element) take the fields' types from the structs'
/// `field_types`; a method of a typed receiver that
/// its type does not declare (`receiver_type`, or `receiver_class` for one
/// a returned type gave) is the one an embedded type declares. An
/// unqualified type is its own package's, a qualified one
/// (`receiver_package`, `import/path.Name`) the package's of that import
/// path in the repository.
/// Returns how many changed.
pub(super) fn resolve_go_declared_receivers(tx: &Transaction<'_>, graph: &Graph) -> Result<i64> {
    let calls = {
        let mut stmt = tx.prepare(
            "SELECT id, file_path, target_qualified, extra FROM edges \
             WHERE kind = 'CALLS' AND file_path LIKE '%.go' \
               AND (json_extract(extra, '$.receiver_field') IS NOT NULL \
                    OR json_extract(extra, '$.receiver_type') IS NOT NULL) \
               AND COALESCE(json_extract(extra, '$.external'), 0) = 0 \
               AND NOT EXISTS (SELECT 1 FROM nodes n \
                               WHERE n.qualified_name = edges.target_qualified)",
        )?;
        let rows = stmt.query_map([], |row| <(i64, String, String, String)>::try_from(row))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    let mut changed = 0_i64;
    for (id, file, method, extra) in calls {
        let mut extra: Value = serde_json::from_str(&extra)?;
        let dir = go_directory(&file);
        let class = if let Some(owner) = extra["receiver_field"]["owner"].as_str() {
            let path = extra["receiver_field"]["field"]
                .as_str()
                .unwrap_or_default();
            let element = extra["receiver_field"]["element"].as_bool() == Some(true);
            match go_class_in(graph, owner, dir)
                .and_then(|owner| go_field_class(graph, owner, path, element))
            {
                Some(class) => class.clone(),
                None => continue,
            }
        } else if let Some(class) = extra["receiver_class"].as_str() {
            class.to_string()
        } else if extra.get("receiver_unknown").and_then(Value::as_bool) == Some(true) {
            continue;
        } else {
            let package_dir = match extra["receiver_package"].as_str() {
                Some(path) => graph.go_packages.get(path).and_then(Option::as_deref),
                None => Some(dir),
            };
            let Some(class) = package_dir.and_then(|package_dir| {
                go_class_in(graph, extra["receiver_type"].as_str()?, package_dir)
            }) else {
                continue;
            };
            class.clone()
        };
        let Some(GoMember::Method(target)) = go_member(graph, &class, &method) else {
            continue;
        };
        if let Some(map) = extra.as_object_mut() {
            map.remove("receiver_unknown");
        }
        let symbol = class
            .split_once("::")
            .map_or(class.as_str(), |(_, symbol)| symbol);
        extra["receiver_type"] = json!(symbol.rsplit('.').next().unwrap_or(symbol));
        // Declared types and Go's selector rule settle the method.
        update_call(tx, id, target, &extra, ConfidenceTier::High)?;
        changed += 1;
    }
    Ok(changed)
}
