use crate::*;

impl GraphStore {
    pub fn resolve_markdown_artifact_refs(&mut self) -> Result<(i64, i64, i64, i64)> {
        let tx = self.conn.transaction()?;
        let rows = {
            let mut stmt = tx.prepare(
                "SELECT id, target_qualified, extra FROM edges \
                 WHERE kind='CROSS_ARTIFACT' \
                   AND (extra LIKE '%original_symbol_name%' \
                        OR extra LIKE '%unresolved_target_name%')",
            )?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows
        };

        let mut resolved = 0_i64;
        let mut demoted = 0_i64;
        let mut re_resolved = 0_i64;
        let mut still_unresolved = 0_i64;
        let mut edge_data = Vec::new();
        let mut symbols = HashSet::new();
        for (edge_id, current_target, raw_extra) in rows {
            let Ok(mut extra) = serde_json::from_str::<Value>(&raw_extra) else {
                continue;
            };
            let Some(extra_obj) = extra.as_object_mut() else {
                continue;
            };
            let sym = extra_obj
                .get("original_symbol_name")
                .or_else(|| extra_obj.get("unresolved_target_name"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            let Some(sym) = sym else { continue };
            extra_obj.remove("unresolved_target_name");
            extra_obj.insert(
                "original_symbol_name".to_string(),
                Value::String(sym.clone()),
            );
            symbols.insert(sym.clone());
            edge_data.push((edge_id, current_target, raw_extra, sym, extra));
        }

        let mut matches_by_symbol = HashMap::<String, Vec<(String, Option<String>)>>::new();
        let symbols = symbols.into_iter().collect::<Vec<_>>();
        for chunk in symbols.chunks(450) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT name, qualified_name, language \
                 FROM nodes \
                 WHERE name IN ({placeholders}) AND language != 'markdown'"
            );
            let mut stmt = tx.prepare(&sql)?;
            let rows = stmt.query_map(rusqlite::params_from_iter(chunk), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?;
            for row in rows {
                let (name, qualified_name, language) = row?;
                matches_by_symbol
                    .entry(name)
                    .or_default()
                    .push((qualified_name, language));
            }
        }

        for (edge_id, current_target, raw_extra, sym, mut extra) in edge_data {
            let matches = matches_by_symbol
                .get(&sym)
                .map(Vec::as_slice)
                .unwrap_or(&[]);

            if matches.len() == 1 {
                let (target, language) = &matches[0];
                let Some(extra_obj) = extra.as_object_mut() else {
                    continue;
                };
                extra_obj.insert(
                    "target_language".to_string(),
                    Value::String(language.clone().unwrap_or_else(|| "unknown".to_string())),
                );
                extra_obj.insert("confidence".to_string(), Value::from(0.8));
                extra_obj.insert(
                    "confidence_tier".to_string(),
                    Value::String("HIGH".to_string()),
                );
                if current_target == *target && !raw_extra.contains("unresolved_target_name") {
                    continue;
                }
                tx.execute(
                    "UPDATE edges \
                     SET target_qualified = ?, extra = ?, confidence = 0.8, confidence_tier = 'HIGH' \
                     WHERE id = ?",
                    params![target, serde_json::to_string(&extra)?, edge_id],
                )?;
                if current_target.starts_with("<unresolved:") {
                    resolved += 1;
                } else if current_target != *target {
                    re_resolved += 1;
                }
            } else {
                let is_implicit_code_span = extra
                    .as_object()
                    .map(|obj| {
                        obj.get("evidence_kind").and_then(Value::as_str)
                            == Some("markdown_code_span")
                            && obj.get("evidence_source").and_then(Value::as_str)
                                == Some("code_span")
                    })
                    .unwrap_or(false);
                if is_implicit_code_span {
                    tx.execute("DELETE FROM edges WHERE id = ?", params![edge_id])?;
                    demoted += 1;
                    continue;
                }
                let unresolved_target = format!("<unresolved:{sym}>");
                if current_target == unresolved_target
                    && !raw_extra.contains("unresolved_target_name")
                {
                    still_unresolved += 1;
                    continue;
                }
                let Some(extra_obj) = extra.as_object_mut() else {
                    continue;
                };
                extra_obj.remove("target_language");
                extra_obj.insert("confidence".to_string(), Value::from(0.2));
                extra_obj.insert(
                    "confidence_tier".to_string(),
                    Value::String("LOW".to_string()),
                );
                tx.execute(
                    "UPDATE edges \
                     SET target_qualified = ?, extra = ?, confidence = 0.2, confidence_tier = 'LOW' \
                     WHERE id = ?",
                    params![unresolved_target, serde_json::to_string(&extra)?, edge_id],
                )?;
                demoted += 1;
            }
        }

        tx.commit()?;
        Ok((resolved, demoted, re_resolved, still_unresolved))
    }
}
