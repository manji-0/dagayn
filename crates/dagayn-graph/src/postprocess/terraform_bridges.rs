//! Terraform -> application code `CROSS_ARTIFACT` bridges.
//!
//! The parser records on each Terraform block node the values it hands to
//! code (`extra.terraform_values`: the attribute, its line span, the literal
//! paths and symbol it carries, and the `var.` / `local.` values it passes
//! on). This step turns them into edges. Every edge it writes is tagged
//! `extractor = "terraform_bridges"`, and each run replaces the previous set,
//! so a bridge follows the code it names as files come and go.

use crate::helpers::*;
use crate::*;

#[cfg(test)]
#[path = "terraform_bridges_tests.rs"]
mod tests;

pub(crate) const TERRAFORM_BRIDGES_EXTRACTOR: &str = "terraform_bridges";
const TERRAFORM_BRIDGE_CONFIDENCE: f64 = 0.8;

/// How the consumer of a value reads it (`role` in the recorded value).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

/// One recorded value of a Terraform block node.
#[derive(Clone, Debug)]
struct TerraformValue {
    /// The block node holding it: the bridge's source.
    node: String,
    file_path: String,
    role: SinkRole,
    attr: String,
    lines: (i64, i64),
    paths: Vec<String>,
    symbol: Option<String>,
}

fn load_values(tx: &Transaction<'_>) -> Result<Vec<TerraformValue>> {
    let mut stmt = tx.prepare(
        "SELECT qualified_name, file_path, extra FROM nodes \
         WHERE language = 'terraform' AND extra LIKE '%terraform_values%' \
         ORDER BY file_path, line_start, qualified_name",
    )?;
    let rows = stmt.query_map([], |row| <(String, String, Option<String>)>::try_from(row))?;
    let mut values = Vec::new();
    for row in rows {
        let (node, file_path, extra) = row?;
        let extra = parse_json_column(extra)?;
        for value in extra["terraform_values"].as_array().into_iter().flatten() {
            let Some(role) = value["role"].as_str().and_then(SinkRole::parse) else {
                continue;
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
            values.push(TerraformValue {
                node: node.clone(),
                file_path: file_path.clone(),
                role,
                attr: value["attr"].as_str().unwrap_or_default().to_string(),
                lines: (line(0), line(1)),
                paths: strings("paths"),
                symbol: value["symbol"].as_str().map(str::to_string),
            });
        }
    }
    Ok(values)
}

/// An edge this step writes.
struct Bridge {
    source: String,
    target: String,
    file_path: String,
    line: i64,
    extra: Value,
}

fn bridge_extra(value: &TerraformValue, role: &str, bridge_kind: &str, evidence: &str) -> Value {
    json!({
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
    })
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

fn path_bridges(value: &TerraformValue, bridges: &mut Vec<Bridge>) {
    let (role, bridge_kind, evidence) = match value.role {
        SinkRole::CodePath => ("maps_entrypoint", "manifest_link", "config"),
        SinkRole::Command => ("invokes_binary", "subprocess", "syntax"),
        SinkRole::Entrypoint => return,
    };
    for path in &value.paths {
        let mut extra = bridge_extra(value, role, bridge_kind, evidence);
        extra["target_language"] = json!(target_language_for_path(path));
        bridges.push(Bridge {
            source: value.node.clone(),
            target: path.clone(),
            file_path: value.file_path.clone(),
            line: value.lines.0,
            extra,
        });
    }
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

/// The one function in the graph a handler names, with its language.
fn entrypoint_match(tx: &Transaction<'_>, symbol: &str) -> Result<Option<(String, String)>> {
    let symbol = symbol.trim();
    if symbol.is_empty() || symbol.starts_with('<') {
        return Ok(None);
    }
    let (module, name) = match symbol.rsplit_once('.') {
        Some((module, name)) if module.is_empty() || name.is_empty() => return Ok(None),
        Some((module, name)) => (Some(module), name),
        None => (None, symbol),
    };
    let mut stmt = tx.prepare(
        "SELECT qualified_name, language, file_path FROM nodes \
         WHERE name = ? AND kind IN ('Function', 'Test') AND language != 'markdown'",
    )?;
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
        if module.is_none_or(|module| module_matches_file(module, &file_path)) {
            matches.push((qualified_name, language));
        }
    }
    Ok((matches.len() == 1).then(|| matches.remove(0)))
}

fn entrypoint_bridge(
    tx: &Transaction<'_>,
    value: &TerraformValue,
    counts: &mut (i64, i64),
) -> Result<Option<Bridge>> {
    let Some(symbol) = value.symbol.as_deref() else {
        return Ok(None);
    };
    let mut extra = bridge_extra(value, "maps_entrypoint", "manifest_link", "config");
    extra["original_symbol_name"] = json!(symbol);
    let target = match entrypoint_match(tx, symbol)? {
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

impl GraphStore {
    /// Write the Terraform -> code bridges from the values the parser
    /// recorded, replacing the previous set. Returns the entrypoints bound
    /// to a function and the ones left `<unresolved:...>`.
    pub fn resolve_terraform_artifact_refs(&mut self) -> Result<(i64, i64)> {
        let now = now_seconds()?;
        let tx = write_tx(&mut self.conn)?;
        tx.execute(
            "DELETE FROM edges WHERE kind = 'CROSS_ARTIFACT' \
             AND json_extract(extra, '$.extractor') = ?",
            [TERRAFORM_BRIDGES_EXTRACTOR],
        )?;
        let values = load_values(&tx)?;
        let mut bridges = Vec::new();
        let mut counts = (0_i64, 0_i64);
        for value in &values {
            path_bridges(value, &mut bridges);
            if value.role == SinkRole::Entrypoint
                && let Some(bridge) = entrypoint_bridge(&tx, value, &mut counts)?
            {
                bridges.push(bridge);
            }
        }
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
        tx.commit()?;
        Ok(counts)
    }
}
