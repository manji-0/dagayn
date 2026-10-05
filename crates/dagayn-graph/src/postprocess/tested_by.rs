use crate::helpers::*;
use crate::*;

/// Last name segment of an edge endpoint: `helper` for `helper`,
/// `obj.helper`, and `src/a.ts::Box.helper`.
fn endpoint_leaf(endpoint: &str) -> &str {
    let path = endpoint.rsplit("::").next().unwrap_or(endpoint);
    path.rsplit('.').next().unwrap_or(path)
}

/// Points parse-time `TESTED_BY` edges at the target their `CALLS` edge
/// resolved to, and returns how many edges changed.
///
/// Parsers derive `TESTED_BY target -> test` from each `CALLS test -> target`
/// (same file and line), so a bare call target gives a bare `TESTED_BY`
/// source. Once bare-name resolution binds the call (in this run, or in an
/// earlier one for graphs built before this sync existed), the bare
/// `TESTED_BY` has no bare `CALLS` left; it then takes the qualified name and
/// confidence of the one resolved call of the same name that the test makes
/// on that line. A bare `TESTED_BY` whose bare `CALLS` still exists, or with
/// several such resolved calls, is left alone. A rewrite that would
/// duplicate an existing `TESTED_BY` edge drops the bare edge instead. Both
/// edges live in the test's file, so file-scoped replacement on re-parse
/// keeps working.
pub(crate) fn sync_tested_by_with_calls(tx: &Transaction<'_>) -> Result<i64> {
    let rows = {
        let mut stmt = tx.prepare(
            "SELECT tb.id, tb.source_qualified, tb.target_qualified, tb.file_path, tb.line, \
                    c.target_qualified, c.confidence, c.confidence_tier \
             FROM edges tb JOIN edges c \
               ON c.kind = 'CALLS' AND c.source_qualified = tb.target_qualified \
              AND c.file_path = tb.file_path AND c.line = tb.line \
             WHERE tb.kind = 'TESTED_BY' AND tb.source_qualified NOT LIKE '%::%' \
               AND c.target_qualified LIKE '%::%' \
               AND NOT EXISTS ( \
                   SELECT 1 FROM edges bare \
                   WHERE bare.kind = 'CALLS' AND bare.source_qualified = tb.target_qualified \
                     AND bare.target_qualified = tb.source_qualified \
                     AND bare.file_path = tb.file_path AND bare.line = tb.line) \
             ORDER BY tb.id",
        )?;
        let mapped = stmt.query_map([], |row| {
            <(i64, String, String, String, i64, String, f64, String)>::try_from(row)
        })?;
        mapped.collect::<std::result::Result<Vec<_>, _>>()?
    };
    // Per bare TESTED_BY edge: its endpoints and the resolved calls whose
    // target has the bare name.
    type Candidate = (String, f64, String);
    let mut groups: Vec<(i64, String, String, i64, Vec<Candidate>)> = Vec::new();
    for (id, bare, test, file_path, line, target, confidence, tier) in rows {
        if groups.last().is_none_or(|group| group.0 != id) {
            groups.push((id, test, file_path, line, Vec::new()));
        }
        if endpoint_leaf(&target) != endpoint_leaf(&bare) {
            continue;
        }
        if let Some(group) = groups.last_mut()
            && !group.4.iter().any(|candidate| candidate.0 == target)
        {
            group.4.push((target, confidence, tier));
        }
    }
    let mut changed = 0_i64;
    for (id, test, file_path, line, candidates) in groups {
        let [(target, confidence, tier)] = candidates.as_slice() else {
            continue;
        };
        let duplicate = tx
            .query_row(
                "SELECT 1 FROM edges WHERE kind = 'TESTED_BY' AND source_qualified = ? \
                 AND target_qualified = ? AND file_path = ? AND line = ? LIMIT 1",
                params![target, test, file_path, line],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if duplicate {
            tx.execute("DELETE FROM edges WHERE id = ?", params![id])?;
        } else {
            tx.execute(
                "UPDATE edges SET source_qualified = ?, confidence = ?, confidence_tier = ? \
                 WHERE id = ?",
                params![target, confidence, tier, id],
            )?;
        }
        changed += 1;
    }
    Ok(changed)
}

/// Keeps `TESTED_BY` in step with the calls tests make once resolution is
/// done: drops the edges whose tested symbol is not a node (a bare `helper`,
/// a call into a package), which say nothing about any code of the
/// repository, and adds `TESTED_BY target -> test` for each call a test
/// makes to a node that has none (a call an earlier run could not resolve,
/// whose bare `TESTED_BY` that run dropped). Calls to assertion / mock APIs
/// and external packages make none, as at parse time. Returns how many
/// edges were dropped and added.
pub(crate) fn reconcile_tested_by_with_calls(tx: &Transaction<'_>) -> Result<(i64, i64)> {
    let dropped = tx.execute(
        "DELETE FROM edges WHERE kind = 'TESTED_BY' \
           AND NOT EXISTS (SELECT 1 FROM nodes n \
                           WHERE n.qualified_name = edges.source_qualified)",
        [],
    )? as i64;
    let missing = {
        let mut stmt = tx.prepare(
            // From the test nodes (a few thousand) rather than every call:
            // `CROSS JOIN` keeps SQLite from starting at `edges`, and the
            // `ORDER BY` keeps the insertion order that start gave.
            "SELECT c.target_qualified, c.source_qualified, c.file_path, c.line, \
                    c.confidence, c.confidence_tier \
             FROM nodes test \
             CROSS JOIN edges c ON c.source_qualified = test.qualified_name AND c.kind = 'CALLS' \
             JOIN nodes target ON target.qualified_name = c.target_qualified \
             WHERE test.is_test = 1 \
               AND COALESCE(json_extract(c.extra, '$.external'), 0) = 0 \
               AND COALESCE(json_extract(c.extra, '$.test_api'), 0) = 0 \
               AND NOT EXISTS ( \
                   SELECT 1 FROM edges tb \
                   WHERE tb.kind = 'TESTED_BY' AND tb.source_qualified = c.target_qualified \
                     AND tb.target_qualified = c.source_qualified \
                     AND tb.file_path = c.file_path AND tb.line = c.line) \
             ORDER BY c.id",
        )?;
        let rows = stmt.query_map([], |row| {
            <(String, String, String, i64, f64, String)>::try_from(row)
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    let now = now_seconds()?;
    let mut added = 0_i64;
    let mut seen = HashSet::new();
    for (target, test, file_path, line, confidence, tier) in missing {
        if !seen.insert((target.clone(), test.clone(), file_path.clone(), line)) {
            continue;
        }
        tx.execute(
            "INSERT INTO edges (kind, source_qualified, target_qualified, target_name, \
                                file_path, line, extra, confidence, confidence_tier, updated_at) \
             VALUES ('TESTED_BY', ?, ?, ?, ?, ?, '{}', ?, ?, ?)",
            params![
                target,
                test,
                edge_target_name(&test),
                file_path,
                line,
                confidence,
                tier,
                now
            ],
        )?;
        added += 1;
    }
    Ok((dropped, added))
}
