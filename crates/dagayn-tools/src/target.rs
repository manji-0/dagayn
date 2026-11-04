//! A target written the way its language writes a member, rather than as the
//! graph's qualified name `path::Type.member` (#275).

use dagayn_graph::{GraphNode, GraphStore};

use crate::query::looks_like_file_target;

/// The nodes `target` names in a spelling other than their qualified name:
/// a Go receiver (`(*T).M`, `(T).M`, `*T.M`), a Rust path (`T::m`), or a
/// member without its file (`T.M`). `exact` looks a qualified name up.
/// Empty when no other spelling names a node.
pub(crate) fn respelled(
    store: &GraphStore,
    target: &str,
    exact: impl Fn(&str) -> Option<Option<GraphNode>>,
) -> Option<Vec<GraphNode>> {
    match target.split_once("::") {
        Some((path, symbol)) if looks_like_file_target(path) => {
            let member = member(&symbol.replace("::", "."));
            if member == symbol {
                return Some(Vec::new());
            }
            Some(exact(&format!("{path}::{member}"))?.into_iter().collect())
        }
        _ => {
            let segments: Vec<&str> = target.split("::").collect();
            let symbol = segments[segments.len().saturating_sub(2)..].join(".");
            members(store, &member(&symbol))
        }
    }
}

/// `T.M` for a Go method written with its receiver: `(*T).M`, `(T).M`,
/// `*T.M`, and a generic receiver's `(*T[K]).M`.
fn member(symbol: &str) -> String {
    let (receiver, rest) = match symbol
        .strip_prefix('(')
        .and_then(|inner| inner.split_once(")."))
    {
        Some(split) => split,
        None => match symbol.strip_prefix('*') {
            Some(rest) => return rest.to_string(),
            None => return symbol.to_string(),
        },
    };
    let receiver = receiver.trim_start_matches('*');
    let receiver = receiver.split('[').next().unwrap_or(receiver);
    format!("{receiver}.{rest}")
}

/// The nodes whose qualified name ends in `symbol` (`T.M`) after a `::` or
/// a `.`, so `T.M` also names a member of a nested `Outer.T`.
fn members(store: &GraphStore, symbol: &str) -> Option<Vec<GraphNode>> {
    let Some((_, name)) = symbol.rsplit_once('.') else {
        return Some(Vec::new());
    };
    let nodes = store.get_nodes_by_name(name).ok()?;
    let in_file = format!("::{symbol}");
    let nested = format!(".{symbol}");
    Some(
        nodes
            .into_iter()
            .filter(|node| {
                node.qualified_name.ends_with(&in_file) || node.qualified_name.ends_with(&nested)
            })
            .collect(),
    )
}
