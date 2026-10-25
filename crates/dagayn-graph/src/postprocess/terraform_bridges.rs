//! Terraform -> application code `CROSS_ARTIFACT` bridges.
//!
//! The parser records on each Terraform block node the values it hands on
//! (`extra.terraform_values`): what a resource or data block gives to code
//! (a sink: `filename`, `handler`, a `local-exec` command), what a `module`
//! call passes to the module's variables, and what a `locals` entry holds.
//! Each value carries its attribute, line span, literal paths and symbol,
//! and the `var.` / `local.` values it takes. A module call also records its
//! local source directory (`extra.module_dir`).
//!
//! This step joins them across files. A literal reaches code when its slot
//! does: a sink directly, a local or module input through every value that
//! takes it, followed into the called module's directory, until a sink. So
//! `source_path = "${path.module}/src"` in a module call bridges to `src`
//! when the module hands `var.source_path` to a program that packages it,
//! and the call's `handler` is looked up in the code that call names.
//!
//! Every edge it writes is tagged `extractor = "terraform_bridges"` (module
//! sources: `terraform_modules`), and each run replaces the previous set, so
//! a bridge follows the code it names as files come and go.

use std::collections::BTreeSet;

use crate::helpers::*;
use crate::*;

#[cfg(test)]
#[path = "terraform_bridges_tests.rs"]
mod tests;

pub(crate) const TERRAFORM_BRIDGES_EXTRACTOR: &str = "terraform_bridges";
const TERRAFORM_MODULES_EXTRACTOR: &str = "terraform_modules";
const TERRAFORM_BRIDGE_CONFIDENCE: f64 = 0.8;

/// How the consumer of a value reads it (`role` in the recorded value).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum SinkRole {
    CodePath,
    Entrypoint,
    Command,
}

impl SinkRole {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "code_path" => Some(Self::CodePath),
            "entrypoint" => Some(Self::Entrypoint),
            "command" => Some(Self::Command),
            _ => None,
        }
    }
}

/// Where a recorded value goes (`slot` in the recorded value).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Sink(SinkRole),
    /// An input of a module call: `var.<attr>` in the module's directory.
    ModuleInput,
    /// `local.<attr>` in the value's directory.
    Local,
}

/// One recorded value of a Terraform block node.
#[derive(Clone, Debug)]
struct TerraformValue {
    /// The node holding it: the bridge's source.
    node: String,
    file_path: String,
    /// The module (directory) the node belongs to.
    dir: String,
    /// For a module input, the called module's directory, when local.
    module_dir: Option<String>,
    slot: Slot,
    attr: String,
    lines: (i64, i64),
    paths: Vec<String>,
    symbol: Option<String>,
    refs: Vec<String>,
}

fn module_dir_of(file_path: &str) -> &str {
    file_path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

struct Recorded {
    values: Vec<TerraformValue>,
    /// Module call node -> `(file_path, line, called directory)`.
    module_calls: Vec<(String, String, i64, String)>,
}

fn load_values(tx: &Transaction<'_>) -> Result<Recorded> {
    let mut stmt = tx.prepare(
        "SELECT qualified_name, file_path, line_start, extra FROM nodes \
         WHERE language = 'terraform' \
         AND (extra LIKE '%terraform_values%' OR extra LIKE '%module_dir%') \
         ORDER BY file_path, line_start, qualified_name",
    )?;
    let rows = stmt.query_map([], |row| {
        <(String, String, i64, Option<String>)>::try_from(row)
    })?;
    let mut recorded = Recorded {
        values: Vec::new(),
        module_calls: Vec::new(),
    };
    for row in rows {
        let (node, file_path, line, extra) = row?;
        let extra = parse_json_column(extra)?;
        let module_dir = extra["module_dir"].as_str().map(str::to_string);
        if let Some(dir) = &module_dir {
            recorded
                .module_calls
                .push((node.clone(), file_path.clone(), line, dir.clone()));
        }
        for value in extra["terraform_values"].as_array().into_iter().flatten() {
            let slot = match value["slot"].as_str() {
                Some("sink") => match value["role"].as_str().and_then(SinkRole::parse) {
                    Some(role) => Slot::Sink(role),
                    None => continue,
                },
                Some("module_input") => Slot::ModuleInput,
                Some("local") => Slot::Local,
                _ => continue,
            };
            let strings = |key: &str| -> Vec<String> {
                value[key]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            };
            let line = |index: usize| value["lines"][index].as_i64().unwrap_or(0);
            recorded.values.push(TerraformValue {
                node: node.clone(),
                file_path: file_path.clone(),
                dir: module_dir_of(&file_path).to_string(),
                module_dir: module_dir.clone(),
                slot,
                attr: value["attr"].as_str().unwrap_or_default().to_string(),
                lines: (line(0), line(1)),
                paths: strings("paths"),
                symbol: value["symbol"].as_str().map(str::to_string),
                refs: strings("refs"),
            });
        }
    }
    Ok(recorded)
}

/// A sink a value reaches: how it is read and where (`node.attr`).
type Reached = BTreeSet<(SinkRole, String)>;

/// Which values take which, per module directory.
struct Flows<'a> {
    /// `(dir, "var.x" | "local.x")` -> the values taking it.
    takers: HashMap<(String, String), Vec<&'a TerraformValue>>,
}

impl<'a> Flows<'a> {
    fn new(values: &'a [TerraformValue]) -> Self {
        let mut takers: HashMap<(String, String), Vec<&TerraformValue>> = HashMap::new();
        for value in values {
            for reference in &value.refs {
                takers
                    .entry((value.dir.clone(), reference.clone()))
                    .or_default()
                    .push(value);
            }
        }
        Self { takers }
    }

    /// The sinks a value reaches: its own when it is one, else those of
    /// every value taking it, transitively.
    fn reached(&self, value: &TerraformValue) -> Reached {
        let mut reached = Reached::new();
        let mut seen = HashSet::new();
        self.walk(value, &mut seen, &mut reached);
        reached
    }

    fn walk(
        &self,
        value: &TerraformValue,
        seen: &mut HashSet<(String, String)>,
        reached: &mut Reached,
    ) {
        let (dir, name) = match value.slot {
            Slot::Sink(role) => {
                reached.insert((role, format!("{}.{}", value.node, value.attr)));
                return;
            }
            Slot::Local => (value.dir.clone(), format!("local.{}", value.attr)),
            Slot::ModuleInput => match &value.module_dir {
                Some(dir) => (dir.clone(), format!("var.{}", value.attr)),
                None => return,
            },
        };
        if !seen.insert((dir.clone(), name.clone())) {
            return;
        }
        for taker in self.takers.get(&(dir, name)).into_iter().flatten() {
            self.walk(taker, seen, reached);
        }
    }
}

/// An edge this step writes.
struct Bridge {
    source: String,
    target: String,
    file_path: String,
    line: i64,
    extra: Value,
}

fn bridge_extra(
    value: &TerraformValue,
    reached: &Reached,
    role: &str,
    bridge_kind: &str,
    evidence: &str,
) -> Value {
    let mut extra = json!({
        "relationship_role": role,
        "bridge_kind": bridge_kind,
        "evidence_kind": evidence,
        "evidence_source": value.attr,
        "source_language": "terraform",
        "target_language": "unknown",
        "confidence": TERRAFORM_BRIDGE_CONFIDENCE,
        "confidence_tier": ConfidenceTier::High.as_str(),
        "extractor": TERRAFORM_BRIDGES_EXTRACTOR,
        "attr_lines": [value.lines.0, value.lines.1],
    });
    match value.slot {
        Slot::Sink(_) => {}
        Slot::ModuleInput => extra["binding"] = json!("module_input"),
        Slot::Local => extra["binding"] = json!("local"),
    }
    if !matches!(value.slot, Slot::Sink(_)) {
        extra["consumers"] = json!(
            reached
                .iter()
                .map(|(_, consumer)| consumer.as_str())
                .collect::<Vec<_>>()
        );
    }
    extra
}

fn target_language_for_path(path: &str) -> &'static str {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".py") {
        "python"
    } else if lower.ends_with(".js") || lower.ends_with(".mjs") || lower.ends_with(".cjs") {
        "javascript"
    } else if lower.ends_with(".ts") || lower.ends_with(".tsx") {
        "typescript"
    } else if lower.ends_with(".go") {
        "go"
    } else if lower.ends_with(".rs") {
        "rust"
    } else if lower.ends_with(".sh") || lower.ends_with(".bash") {
        "bash"
    } else {
        "unknown"
    }
}

/// Code packages Terraform uploads as they are; they are not parsed, so
/// they are files in the tree rather than graph nodes.
const ARCHIVE_EXTENSIONS: &[&str] = &[".zip", ".jar", ".whl", ".tar.gz", ".tgz"];

/// The checked-out repository, to tell a committed archive from a build
/// output that only exists after a build.
struct Tree {
    root: Option<PathBuf>,
}

impl Tree {
    fn load(tx: &Transaction<'_>) -> Result<Self> {
        let root = tx
            .query_row(
                "SELECT value FROM metadata WHERE key = 'repo_root'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        Ok(Self {
            root: root.map(PathBuf::from),
        })
    }

    fn has_archive(&self, path: &str) -> bool {
        let relative = Path::new(path);
        let lower = path.to_ascii_lowercase();
        ARCHIVE_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
            && relative.is_relative()
            && self
                .root
                .as_ref()
                .is_some_and(|root| root.join(relative).is_file())
    }
}

/// One edge per path that names code in the tree, and for a directory one
/// per code file in it: the files are graph nodes, so review and impact
/// reach them like any other target. `reference` keeps the path as written,
/// the one place to check.
///
/// A path naming nothing in the tree (a build output, a file of a submodule
/// that is not checked out) or no code (a Dockerfile, `requirements.txt`, a
/// directory of them) gets no edge: there is nothing on the other side to
/// check or to reach, and the value stays recorded on the block node.
fn path_bridges(
    tx: &Transaction<'_>,
    tree: &Tree,
    value: &TerraformValue,
    reached: &Reached,
    bridges: &mut Vec<Bridge>,
) -> Result<()> {
    let roles: BTreeSet<SinkRole> = reached.iter().map(|(role, _)| *role).collect();
    let (role, bridge_kind, evidence) = if roles.contains(&SinkRole::CodePath) {
        ("maps_entrypoint", "manifest_link", "config")
    } else if roles.contains(&SinkRole::Command) {
        ("invokes_binary", "subprocess", "syntax")
    } else {
        return Ok(());
    };
    for path in &value.paths {
        let targets = match path_target(tx, path)? {
            PathTarget::Directory(files) => files,
            PathTarget::File => vec![path.clone()],
            PathTarget::Unindexed if tree.has_archive(path) => vec![path.clone()],
            PathTarget::Unindexed => continue,
        };
        for target in targets {
            let mut extra = bridge_extra(value, reached, role, bridge_kind, evidence);
            extra["target_language"] = json!(target_language_for_path(&target));
            extra["reference"] = json!(path);
            bridges.push(Bridge {
                source: value.node.clone(),
                target,
                file_path: value.file_path.clone(),
                line: value.lines.0,
                extra,
            });
        }
    }
    Ok(())
}

/// Where a handler of one block can live: the code its own values name.
enum HandlerScope {
    /// It names no code: any function of that name in the repository.
    Repository,
    /// Files and directories in the tree holding its code.
    Paths(Vec<String>),
    /// It names code that is not in the tree (a zip, a build output).
    Elsewhere,
}

/// What a path names in the graph.
enum PathTarget {
    /// An indexed code file.
    File,
    /// A directory: the indexed code files under it.
    Directory(Vec<String>),
    /// Nothing indexed: an archive, a data file, a build output.
    Unindexed,
}

/// Indexed code files: not documentation, not Terraform itself.
fn path_target(tx: &Transaction<'_>, path: &str) -> Result<PathTarget> {
    let prefix = format!("{path}/");
    let mut stmt = tx.prepare_cached(
        "SELECT file_path FROM nodes WHERE kind = 'File' \
         AND language NOT IN ('markdown', 'terraform') \
         AND (file_path = ?1 OR substr(file_path, 1, length(?2)) = ?2) \
         ORDER BY file_path",
    )?;
    let files = stmt
        .query_map(params![path, prefix], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(if files.iter().any(|file| file == path) {
        PathTarget::File
    } else if files.is_empty() {
        PathTarget::Unindexed
    } else {
        PathTarget::Directory(files)
    })
}

fn handler_scope(tx: &Transaction<'_>, code_paths: &[&String]) -> Result<HandlerScope> {
    if code_paths.is_empty() {
        return Ok(HandlerScope::Repository);
    }
    let mut in_tree = Vec::new();
    for path in code_paths {
        if !matches!(path_target(tx, path)?, PathTarget::Unindexed) {
            in_tree.push((*path).clone());
        }
    }
    Ok(if in_tree.is_empty() {
        HandlerScope::Elsewhere
    } else {
        HandlerScope::Paths(in_tree)
    })
}

/// The module part of a handler (`index` of `index.handler`) names the
/// file's stem or one of its directories.
fn module_matches_file(module: &str, file_path: &str) -> bool {
    if file_path.is_empty() {
        return false;
    }
    let path = file_path.replace('\\', "/");
    let stem = path.rsplit_once('/').map(|(_, name)| name).unwrap_or(&path);
    let stem = stem.rsplit_once('.').map(|(name, _)| name).unwrap_or(stem);
    if stem == module {
        return true;
    }
    path.split('/').rev().skip(1).any(|part| part == module)
}

fn under_any(file_path: &str, paths: &[String]) -> bool {
    paths.iter().any(|path| {
        file_path == path
            || file_path
                .strip_prefix(path.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

/// The one function (in a scope, also class: `example.Handler` in Java)
/// a handler names, with its language.
fn entrypoint_match(
    tx: &Transaction<'_>,
    symbol: &str,
    scope: &HandlerScope,
) -> Result<Option<(String, String)>> {
    let symbol = symbol.trim();
    if symbol.is_empty() || symbol.starts_with('<') {
        return Ok(None);
    }
    let (module, name) = match symbol.rsplit_once('.') {
        Some((module, name)) if module.is_empty() || name.is_empty() => return Ok(None),
        Some((module, name)) => (Some(module), name),
        None => (None, symbol),
    };
    let kinds = match scope {
        HandlerScope::Repository => "('Function', 'Test')",
        HandlerScope::Paths(_) => "('Function', 'Test', 'Class')",
        HandlerScope::Elsewhere => return Ok(None),
    };
    let mut stmt = tx.prepare(&format!(
        "SELECT qualified_name, language, file_path FROM nodes \
         WHERE name = ? AND kind IN {kinds} AND language NOT IN ('markdown', 'terraform')"
    ))?;
    let rows = stmt.query_map([name], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?
                .unwrap_or_else(|| "unknown".to_string()),
            row.get::<_, Option<String>>(2)?.unwrap_or_default(),
        ))
    })?;
    let mut matches = Vec::new();
    for row in rows {
        let (qualified_name, language, file_path) = row?;
        let in_scope = match scope {
            HandlerScope::Paths(paths) => under_any(&file_path, paths),
            HandlerScope::Repository | HandlerScope::Elsewhere => true,
        };
        if in_scope && module.is_none_or(|module| module_matches_file(module, &file_path)) {
            matches.push((qualified_name, language));
        }
    }
    Ok((matches.len() == 1).then(|| matches.remove(0)))
}

fn entrypoint_bridge(
    tx: &Transaction<'_>,
    value: &TerraformValue,
    reached: &Reached,
    scope: &HandlerScope,
    counts: &mut (i64, i64),
) -> Result<Option<Bridge>> {
    let Some(symbol) = value.symbol.as_deref() else {
        return Ok(None);
    };
    let mut extra = bridge_extra(value, reached, "maps_entrypoint", "manifest_link", "config");
    extra["original_symbol_name"] = json!(symbol);
    let target = match entrypoint_match(tx, symbol, scope)? {
        Some((qualified_name, language)) => {
            counts.0 += 1;
            extra["target_language"] = json!(language);
            qualified_name
        }
        None => {
            counts.1 += 1;
            format!("<unresolved:{symbol}>")
        }
    };
    Ok(Some(Bridge {
        source: value.node.clone(),
        target,
        file_path: value.file_path.clone(),
        line: value.lines.0,
        extra,
    }))
}

fn bridges_for(
    tx: &Transaction<'_>,
    values: &[TerraformValue],
) -> Result<(Vec<Bridge>, (i64, i64))> {
    let flows = Flows::new(values);
    let reached: Vec<Reached> = values.iter().map(|value| flows.reached(value)).collect();
    let reaches = |index: usize, role: SinkRole| reached[index].iter().any(|(r, _)| *r == role);

    let tree = Tree::load(tx)?;
    let mut bridges = Vec::new();
    let mut counts = (0_i64, 0_i64);
    for (index, value) in values.iter().enumerate() {
        path_bridges(tx, &tree, value, &reached[index], &mut bridges)?;
        if !reaches(index, SinkRole::Entrypoint) || value.symbol.is_none() {
            continue;
        }
        // The code this block names, through any of its values.
        let code_paths: Vec<&String> = values
            .iter()
            .enumerate()
            .filter(|(other, candidate)| {
                candidate.node == value.node && reaches(*other, SinkRole::CodePath)
            })
            .flat_map(|(_, candidate)| &candidate.paths)
            .collect();
        let scope = handler_scope(tx, &code_paths)?;
        if let Some(bridge) = entrypoint_bridge(tx, value, &reached[index], &scope, &mut counts)? {
            bridges.push(bridge);
        }
    }
    Ok((bridges, counts))
}

/// IMPORTS_FROM a local module call to each Terraform file of the module.
fn module_source_edges(
    tx: &Transaction<'_>,
    module_calls: &[(String, String, i64, String)],
) -> Result<Vec<Bridge>> {
    let mut files_by_dir: HashMap<String, Vec<String>> = HashMap::new();
    {
        let mut stmt = tx.prepare(
            "SELECT file_path FROM nodes WHERE kind = 'File' AND language = 'terraform' \
             ORDER BY file_path",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        for row in rows {
            let file = row?;
            files_by_dir
                .entry(module_dir_of(&file).to_string())
                .or_default()
                .push(file);
        }
    }
    let mut edges = Vec::new();
    for (node, file_path, line, dir) in module_calls {
        for file in files_by_dir.get(dir).into_iter().flatten() {
            edges.push(Bridge {
                source: node.clone(),
                target: file.clone(),
                file_path: file_path.clone(),
                line: *line,
                extra: json!({"extractor": TERRAFORM_MODULES_EXTRACTOR, "module_dir": dir}),
            });
        }
    }
    Ok(edges)
}

impl GraphStore {
    /// Write the Terraform -> code bridges and the local module sources from
    /// the values the parser recorded, replacing the previous set. Returns
    /// the entrypoints bound to a function and the ones left
    /// `<unresolved:...>`.
    pub fn resolve_terraform_artifact_refs(&mut self) -> Result<(i64, i64)> {
        let now = now_seconds()?;
        let tx = write_tx(&mut self.conn)?;
        tx.execute(
            "DELETE FROM edges WHERE kind IN ('CROSS_ARTIFACT', 'IMPORTS_FROM') \
             AND json_extract(extra, '$.extractor') IN (?, ?)",
            [TERRAFORM_BRIDGES_EXTRACTOR, TERRAFORM_MODULES_EXTRACTOR],
        )?;
        let recorded = load_values(&tx)?;
        let (mut bridges, counts) = bridges_for(&tx, &recorded.values)?;
        let mut seen = HashSet::new();
        bridges.retain(|bridge| seen.insert((bridge.source.clone(), bridge.target.clone())));
        for bridge in bridges {
            let tier = bridge.extra["confidence_tier"]
                .as_str()
                .unwrap_or("HIGH")
                .to_string();
            let confidence = bridge.extra["confidence"]
                .as_f64()
                .unwrap_or(TERRAFORM_BRIDGE_CONFIDENCE);
            tx.execute(
                "INSERT INTO edges
                    (kind, source_qualified, target_qualified, target_name, file_path, line,
                     extra, confidence, confidence_tier, updated_at)
                 VALUES ('CROSS_ARTIFACT', ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    bridge.source,
                    bridge.target,
                    edge_target_name(&bridge.target),
                    bridge.file_path,
                    bridge.line,
                    extra_json(&bridge.extra)?,
                    confidence,
                    tier,
                    now
                ],
            )?;
        }
        for edge in module_source_edges(&tx, &recorded.module_calls)? {
            tx.execute(
                "INSERT INTO edges
                    (kind, source_qualified, target_qualified, target_name, file_path, line,
                     extra, confidence, confidence_tier, updated_at)
                 VALUES ('IMPORTS_FROM', ?, ?, ?, ?, ?, ?, 1.0, 'EXTRACTED', ?)",
                params![
                    edge.source,
                    edge.target,
                    edge_target_name(&edge.target),
                    edge.file_path,
                    edge.line,
                    extra_json(&edge.extra)?,
                    now
                ],
            )?;
        }
        tx.commit()?;
        Ok(counts)
    }
}
