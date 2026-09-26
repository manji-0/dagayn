use crate::helpers::*;
use crate::postprocess_bridges::extra_json;
use crate::*;

/// A bare Terraform reference bound to the only same-module declaration.
const TERRAFORM_MODULE_SCOPE_CONFIDENCE: f64 = 0.8;

/// The Terraform module a file belongs to: its directory (`""` at the root).
fn terraform_module_dir(file_path: &str) -> &str {
    file_path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

fn terraform_module_matches_file(module: &str, file_path: &str) -> bool {
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

fn terraform_entrypoint_match(
    tx: &Transaction<'_>,
    symbol: &str,
) -> Result<Option<(String, String)>> {
    let symbol = symbol.trim();
    if symbol.is_empty() || symbol.starts_with('<') {
        return Ok(None);
    }
    let mut stmt = tx.prepare(
        "SELECT qualified_name, language, file_path FROM nodes \
         WHERE name = ? AND kind IN ('Function', 'Test') AND language != 'markdown'",
    )?;
    if let Some((module, attr)) = symbol.rsplit_once('.') {
        if module.is_empty() || attr.is_empty() {
            return Ok(None);
        }
        let rows = stmt.query_map([attr], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?
                    .unwrap_or_else(|| "unknown".to_string()),
                row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            ))
        })?;
        let matches: Vec<(String, String)> = rows
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|(_, _, file_path)| terraform_module_matches_file(module, file_path))
            .map(|(qn, lang, _)| (qn, lang))
            .collect();
        return Ok(unique_pair(matches));
    }
    let rows = stmt.query_map([symbol], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?
                .unwrap_or_else(|| "unknown".to_string()),
        ))
    })?;
    let matches: Vec<(String, String)> = rows.collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(unique_pair(matches))
}

fn unique_pair(matches: Vec<(String, String)>) -> Option<(String, String)> {
    let mut iter = matches.into_iter();
    let first = iter.next()?;
    if iter.next().is_some() {
        None
    } else {
        Some(first)
    }
}

impl GraphStore {
    pub fn resolve_terraform_artifact_refs(&mut self) -> Result<(i64, i64)> {
        let tx = write_tx(&mut self.conn)?;
        let rows = {
            let mut stmt = tx.prepare(
                "SELECT id, target_qualified, extra FROM edges \
                 WHERE kind='CROSS_ARTIFACT' AND extra LIKE '%original_symbol_name%'",
            )?;
            let mapped = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?;
            mapped.collect::<std::result::Result<Vec<_>, _>>()?
        };

        let mut resolved = 0_i64;
        let mut still_unresolved = 0_i64;
        for (edge_id, current_target, extra_raw) in rows {
            let extra = parse_json_column(extra_raw)?;
            let obj = match extra.as_object() {
                Some(obj) => obj,
                None => continue,
            };
            if obj.get("source_language").and_then(Value::as_str) != Some("terraform") {
                continue;
            }
            if !matches!(
                obj.get("evidence_source").and_then(Value::as_str),
                Some("handler" | "entry_point")
            ) {
                continue;
            }
            if obj.get("relationship_role").and_then(Value::as_str) != Some("maps_entrypoint") {
                continue;
            }
            let Some(sym) = obj.get("original_symbol_name").and_then(Value::as_str) else {
                continue;
            };
            if sym.is_empty() {
                continue;
            }
            match terraform_entrypoint_match(&tx, sym)? {
                None => still_unresolved += 1,
                Some((qname, _)) if qname == current_target => {}
                Some((qname, lang)) => {
                    let mut new_extra = extra.clone();
                    if let Some(obj) = new_extra.as_object_mut() {
                        obj.insert("target_language".to_string(), Value::String(lang));
                        obj.insert("confidence".to_string(), Value::from(0.8));
                        obj.insert(
                            "confidence_tier".to_string(),
                            Value::String(ConfidenceTier::High.as_str().to_string()),
                        );
                    }
                    tx.execute(
                        "UPDATE edges
                         SET target_qualified=?, target_name=?, extra=?, confidence=?, confidence_tier=?
                         WHERE id=?",
                        params![
                            qname,
                            edge_target_name(&qname),
                            extra_json(&new_extra)?,
                            0.8,
                            ConfidenceTier::High.as_str(),
                            edge_id
                        ],
                    )?;
                    resolved += 1;
                }
            }
        }
        tx.commit()?;
        Ok((resolved, still_unresolved))
    }

    /// Qualify bare Terraform `REFERENCES` targets declared in another file
    /// of the same module.
    ///
    /// Terraform merges every file of one directory into a single module, so
    /// `var.region` in `main.tf` refers to the `variable "region"` block in
    /// `variables.tf`. The parser only qualifies names defined in the file it
    /// is parsing; this step binds the remaining bare targets to the unique
    /// Terraform node with that name in the edge's directory. Ambiguous
    /// names (for example an `override.tf` redefining a block) stay bare.
    pub fn resolve_terraform_module_references(&mut self) -> Result<i64> {
        let tx = write_tx(&mut self.conn)?;
        let index = {
            let mut stmt = tx.prepare(
                "SELECT name, qualified_name, file_path FROM nodes \
                 WHERE language = 'terraform' AND kind != 'File'",
            )?;
            let mapped = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            let mut index = HashMap::<String, Vec<(String, String)>>::new();
            for row in mapped {
                let (name, qualified_name, file_path) = row?;
                index
                    .entry(name)
                    .or_default()
                    .push((terraform_module_dir(&file_path).to_string(), qualified_name));
            }
            index
        };
        let edges = {
            let mut stmt = tx.prepare(
                "SELECT e.id, e.target_qualified, e.file_path FROM edges e \
                 WHERE e.kind = 'REFERENCES' AND e.target_qualified NOT LIKE '%::%' \
                 AND EXISTS (SELECT 1 FROM nodes f WHERE f.qualified_name = e.file_path \
                             AND f.kind = 'File' AND f.language = 'terraform')",
            )?;
            let mapped = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            mapped.collect::<std::result::Result<Vec<_>, _>>()?
        };
        let mut resolved = 0_i64;
        for (edge_id, target, file_path) in edges {
            let Some(candidates) = index.get(&target) else {
                continue;
            };
            let module_dir = terraform_module_dir(&file_path);
            let mut in_module = candidates.iter().filter(|(dir, _)| dir == module_dir);
            let (Some((_, qualified)), None) = (in_module.next(), in_module.next()) else {
                continue;
            };
            tx.execute(
                "UPDATE edges SET target_qualified = ?, target_name = ?, \
                 confidence = ?, confidence_tier = ? WHERE id = ?",
                params![
                    qualified,
                    edge_target_name(qualified),
                    TERRAFORM_MODULE_SCOPE_CONFIDENCE,
                    ConfidenceTier::High.as_str(),
                    edge_id
                ],
            )?;
            resolved += 1;
        }
        tx.commit()?;
        Ok(resolved)
    }
}
