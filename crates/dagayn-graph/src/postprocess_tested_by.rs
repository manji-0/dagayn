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
pub(super) fn sync_tested_by_with_calls(tx: &Transaction<'_>) -> Result<i64> {
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
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, f64>(6)?,
                row.get::<_, String>(7)?,
            ))
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
