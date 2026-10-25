//! `architecture_analysis_tool`
//! (`dagayn.tools.architecture_analysis.architecture_analysis_func`), in
//! every mode; the Python function opens the graph and returns this answer.

use serde_json::{Map, Value, json};

use crate::architecture::{Artifact, Snapshot};
use crate::{Args, Context, Ordered, Payload, open_graph, resolve_repo, seal_dispatch};

const DECLARED: &[&str] = &[
    "mode",
    "detail_level",
    "scope_kind",
    "unit_filter",
    "repo_root",
    "artifact_scope",
    "import_kind",
];
const MODES: &[&str] = &["overview", "import_cycles"];
/// `import_kind` values `import_cycles` accepts, as `query_graph_tool`'s.
const IMPORT_KINDS: &[&str] = &["directive", "link", "lazy_export", "path", "statement"];

/// The arguments once fastmcp has validated them against the Python
/// signature.
struct Request<'a> {
    mode: &'a str,
    detail_level: &'a str,
    scope_kind: &'a str,
    unit_filter: Option<Vec<String>>,
    artifact_scope: &'a str,
    import_kind: Option<&'a str>,
    /// The repository, for the manifests that declare units.
    root: std::path::PathBuf,
}

impl<'a> Request<'a> {
    fn parse(args: &Args<'a>, arguments: &'a Map<String, Value>) -> Option<Self> {
        let literal = |key: &str, default: &'a str, allowed: &[&str]| -> Option<&'a str> {
            match arguments.get(key) {
                None => Some(default),
                Some(Value::String(value)) if allowed.contains(&value.as_str()) => {
                    Some(value.as_str())
                }
                Some(_) => None,
            }
        };
        let unit_filter = match arguments.get("unit_filter") {
            None | Some(Value::Null) => None,
            Some(Value::Array(items)) => Some(
                items
                    .iter()
                    .map(|item| item.as_str().map(str::to_string))
                    .collect::<Option<Vec<_>>>()?,
            ),
            Some(_) => return None,
        };
        Some(Self {
            mode: literal("mode", "overview", MODES)?,
            detail_level: literal(
                "detail_level",
                "minimal",
                &["minimal", "standard", "verbose"],
            )?,
            scope_kind: literal("scope_kind", "package", &["file", "package", "directory"])?,
            unit_filter,
            artifact_scope: literal("artifact_scope", "code", &["code", "docs", "all"])?,
            import_kind: args.optional_string("import_kind")?,
            root: std::path::PathBuf::new(),
        })
    }
}

pub(crate) fn architecture(context: &Context, arguments: &Map<String, Value>) -> Option<Payload> {
    let args = Args::new(arguments, DECLARED)?;
    let mut request = Request::parse(&args, arguments)?;
    let root = resolve_repo(context, args.optional_string("repo_root")?)?;
    request.root = root.path.clone();
    if let Some(kind) = request.import_kind
        && !IMPORT_KINDS.contains(&kind)
    {
        return crate::dispatcher_error(
            context,
            &root,
            request.mode,
            &format!(
                "import_kind must be 'directive', 'link', 'lazy_export', 'path', or 'statement', \
                 got '{kind}'."
            ),
        );
    }
    let runtime = context.runtime.clone()?;
    let graph = open_graph(&root)?;
    let artifact = Artifact::parse(request.artifact_scope)?;
    let answerability = graph.answerability()?;
    let (subtool, out, trailing) = match request.mode {
        "overview" => (
            "get_architecture_overview_func",
            with_unit_map(
                &root.path,
                &graph.store,
                request.detail_level,
                request.artifact_scope,
                &answerability,
            )?,
            false,
        ),
        _ => (
            "find_import_cycles_func",
            crate::cycles::import_cycles(
                &crate::cycles::CycleRequest {
                    scope_kind: request.scope_kind,
                    unit_filter: request.unit_filter.as_deref(),
                    import_kind: request.import_kind,
                    artifact,
                    artifact_scope: request.artifact_scope,
                    detail_level: request.detail_level,
                    root: &request.root,
                },
                &Snapshot::read(&graph.store)?,
            ),
            true,
        ),
    };
    // `attach_answerability` for a subtool that reports none.
    let trailing = if trailing {
        vec![
            ("answerability", answerability.full()),
            ("missingness", json!(answerability.missingness())),
        ]
    } else {
        Vec::new()
    };
    Some(seal_dispatch(
        out,
        crate::Dispatch {
            mode: request.mode,
            subtool,
            runtime,
            trailing,
            repo: graph.repo_context(),
        },
    ))
}

/// Units the overview lists; the rest are counted in `units_omitted`.
const MAX_UNITS: usize = 40;
/// Unit edges the overview lists, heaviest first.
const MAX_UNIT_EDGES: usize = 30;

/// The overview led by the map of declared units
/// (docs/plans/ARCHITECTURE-TOOL-TARGET.md#the-map); `surface` from
/// `standard` up.
fn with_unit_map(
    root: &std::path::Path,
    store: &dagayn_graph::GraphStore,
    detail_level: &str,
    artifact_scope: &str,
    answerability: &crate::answerability::Answerability,
) -> Option<Ordered> {
    let snapshot = crate::architecture::Snapshot::read(store)?;
    let (mut units, mut unit_edges) = crate::units::unit_map(
        root,
        &snapshot.all_nodes,
        &snapshot.edges,
        detail_level != "minimal",
    );
    let (findings, findings_omitted) = crate::arch_findings::architecture_findings(root, &snapshot);
    let (unit_count, edge_count) = (units.len(), unit_edges.len());
    units.truncate(MAX_UNITS);
    unit_edges.truncate(MAX_UNIT_EDGES);
    let mut by_kind: Vec<(&str, usize)> = Vec::new();
    for finding in &findings {
        let kind = finding["kind"].as_str().unwrap_or("");
        match by_kind.iter_mut().find(|(known, _)| *known == kind) {
            Some((_, count)) => *count += 1,
            None => by_kind.push((kind, 1)),
        }
    }
    for (kind, count) in &mut by_kind {
        *count += findings_omitted.get(kind).copied().unwrap_or(0);
    }
    let findings_summary = if by_kind.is_empty() {
        "No structural findings.".to_string()
    } else {
        format!(
            "Findings: {}.",
            by_kind
                .iter()
                .map(|(kind, count)| format!("{count} {kind}"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let summary = format!(
        "{unit_count} unit(s), {edge_count} dependency pair(s) between them. {findings_summary}"
    );
    let mut out = Ordered::default()
        .put("status", "ok")
        .put("summary", summary);
    out = out.put("units", json!(units));
    if unit_count > units.len() {
        out = out.put("units_omitted", unit_count - units.len());
    }
    out = out.put("unit_edges", json!(unit_edges));
    if edge_count > unit_edges.len() {
        out = out.put("unit_edges_omitted", edge_count - unit_edges.len());
    }
    out = out
        .put("findings", json!(findings))
        .put("next", crate::next::from_findings(&findings));
    if !findings_omitted.is_empty() {
        out = out.put("findings_omitted", json!(findings_omitted));
    }
    out = out.put("artifact_scope", artifact_scope);
    // The map reads no community or flow: their gaps are not its caveats.
    out = out.put("missingness", json!(answerability.missingness()));
    if detail_level == "verbose" {
        out = out.put("answerability", answerability.full());
    }
    Some(out)
}
