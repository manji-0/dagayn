//! Calls across an HTTP request inside one Cloudflare Worker, for the two
//! frameworks that make them: `cloudflare:test` and Hono.
//!
//! A worker test sends a request (`SELF.fetch(url)`), the Workers runtime
//! runs the default export's `fetch` of the module wrangler's `main` names,
//! that calls a Hono app (`app.fetch(..)`), and Hono dispatches on the path
//! to a handler registered at module scope (`app.get(path, (c) => ..)`),
//! or on a prefix to a mounted app (`app.route(prefix, sub)`). The handlers
//! are arrows with no node, so the File owns their calls, and an app
//! stands for the File that declares it (File `extra.hono_apps`).
//!
//! The edges are `CALLS`, which test reach walks:
//!
//! * a caller of `cloudflare:test::SELF.fetch` -> `<main>::default.fetch`,
//!   or `<main>` itself when its default export is an app, `main` from the
//!   `worker_entry` manifest bridge of the nearest wrangler configuration
//!   above the caller's file;
//! * a reference to an app from another file (`app.route("/", sub)`,
//!   `testClient(app)`) -> the File that declares it;
//! * `app.fetch(..)` / `app.request(..)` on an imported app
//!   (`receiver_import`) or one of the caller's module (`receiver_app`,
//!   from a Worker's `default.fetch`), which name resolution could only guess at
//!   (`StreamHub.fetch`), retargeted to that File. The edge keeps what it
//!   was (`request_boundary_original`), and every run restores it before
//!   deriving again, so an app that stops being one gives the edge back.
//!
//! Each run first removes what the previous one wrote, including the
//! TESTED_BY edges reconciled from those calls, and reconciles TESTED_BY
//! once more at the end, so the result does not depend on how many times
//! post-processing ran.
//!
//! A request reaches every route of the app, not the route its path names.
//! On tstodon, linking each test to only the handlers coverage says ran
//! changes 2 of the 1,001 answers it calls tested
//! (docs/plans/TYPESCRIPT-SUPPORT-MEASUREMENTS.md#t3-request-boundary-edges),
//! and most tests build the path in a helper (`SELF.fetch(base + path)`).

use super::native_bindings::path_under;
use super::tested_by::reconcile_tested_by_with_calls;
use crate::helpers::*;
use crate::*;

const REQUEST_BOUNDARY_EXTRACTOR: &str = "request_boundary";
const ORIGINAL: &str = "request_boundary_original";
const REQUEST_BOUNDARY_CONFIDENCE: f64 = 0.8;
const SELF_FETCH: &str = "cloudflare:test::SELF.fetch";
/// Members of a Hono app that dispatch a request to its handlers.
const DISPATCH_MEMBERS: &[&str] = &["fetch", "request"];

struct NewCall {
    source: String,
    target: String,
    file_path: String,
    line: i64,
    framework: &'static str,
}

impl GraphStore {
    /// Replace the `request_boundary` calls and the retargeted dispatches on
    /// imported Hono apps; returns how many edges were written or moved.
    pub fn resolve_request_boundaries(&mut self) -> Result<i64> {
        let now = now_seconds()?;
        let tx = write_tx(&mut self.conn)?;
        forget_previous_run(&tx)?;
        let apps = hono_apps(&tx)?;
        let mut calls = Vec::new();
        let mut moved = 0_i64;
        if !apps.is_empty() {
            calls.extend(app_references(&tx, &apps)?);
            moved = retarget_dispatches(&tx, &apps)?;
        }
        calls.extend(self_fetches(&tx, &apps)?);
        calls.sort_by(|a, b| {
            (&a.file_path, a.line, &a.source, &a.target).cmp(&(
                &b.file_path,
                b.line,
                &b.source,
                &b.target,
            ))
        });
        calls.dedup_by(|a, b| a.source == b.source && a.target == b.target && a.line == b.line);
        let mut written = 0_i64;
        for call in calls {
            let extra = json!({
                "extractor": REQUEST_BOUNDARY_EXTRACTOR,
                "framework": call.framework,
            });
            tx.execute(
                "INSERT INTO edges
                    (kind, source_qualified, target_qualified, target_name, file_path, line,
                     extra, confidence, confidence_tier, updated_at)
                 VALUES ('CALLS', ?, ?, ?, ?, ?, ?, ?, 'HIGH', ?)",
                params![
                    call.source,
                    call.target,
                    edge_target_name(&call.target),
                    call.file_path,
                    call.line,
                    extra_json(&extra)?,
                    REQUEST_BOUNDARY_CONFIDENCE,
                    now
                ],
            )?;
            written += 1;
        }
        // A test's request is a test's call: TESTED_BY follows, as it does
        // for the calls name resolution binds.
        reconcile_tested_by_with_calls(&tx)?;
        tx.commit()?;
        Ok(written + moved)
    }
}

/// Undo the previous run: its TESTED_BY edges, its calls, and its
/// retargets.
fn forget_previous_run(tx: &Transaction<'_>) -> Result<()> {
    tx.execute(
        "DELETE FROM edges WHERE kind = 'TESTED_BY' AND EXISTS ( \
             SELECT 1 FROM edges c WHERE c.kind = 'CALLS' \
               AND (json_extract(c.extra, '$.extractor') = ?1 \
                    OR json_extract(c.extra, '$.' || ?2) IS NOT NULL) \
               AND c.target_qualified = edges.source_qualified \
               AND c.source_qualified = edges.target_qualified \
               AND c.file_path = edges.file_path AND c.line = edges.line)",
        params![REQUEST_BOUNDARY_EXTRACTOR, ORIGINAL],
    )?;
    tx.execute(
        "DELETE FROM edges WHERE kind = 'CALLS' \
         AND json_extract(extra, '$.extractor') = ?",
        [REQUEST_BOUNDARY_EXTRACTOR],
    )?;
    let mut stmt = tx.prepare(&format!(
        "SELECT id, json_extract(extra, '$.{ORIGINAL}') FROM edges \
         WHERE kind = 'CALLS' AND json_extract(extra, '$.{ORIGINAL}') IS NOT NULL"
    ))?;
    let retargeted: Vec<(i64, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<std::result::Result<_, _>>()?;
    drop(stmt);
    for (id, original) in retargeted {
        let original: Value = serde_json::from_str(&original).unwrap_or_else(|_| json!({}));
        let mut target = original["target"].as_str().unwrap_or_default();
        let mut target_name = original["target_name"].as_str().unwrap_or_default();
        let mut confidence = original["confidence"].as_f64().unwrap_or(1.0);
        let mut tier = original["confidence_tier"].as_str().unwrap_or("EXTRACTED");
        // A target deleted since goes back to the member the call names
        // (`fetch`, not `StreamHub.fetch`), unresolved, which the next name
        // resolution binds again.
        let gone = tx
            .query_row(
                "SELECT 1 FROM nodes WHERE qualified_name = ?",
                [target],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .is_none();
        if gone && target.contains("::") {
            let extra = &original["extra"];
            target = extra["receiver_import"]["member"]
                .as_str()
                .or_else(|| extra["receiver_app"]["member"].as_str())
                .unwrap_or(target_name);
            target_name = target;
            confidence = confidence.min(0.2);
            tier = "LOW";
        }
        tx.execute(
            "UPDATE edges SET target_qualified = ?, target_name = ?, confidence = ?, \
             confidence_tier = ?, extra = ? WHERE id = ?",
            params![
                target,
                target_name,
                confidence,
                tier,
                extra_json(&original["extra"])?,
                id
            ],
        )?;
    }
    Ok(())
}

/// `file::app` of every Hono app (`file::default` for a default export
/// that is one) -> the File that declares it.
fn hono_apps(tx: &Transaction<'_>) -> Result<HashMap<String, String>> {
    let mut stmt = tx.prepare(
        "SELECT qualified_name, json_extract(extra, '$.hono_apps') FROM nodes \
         WHERE kind = 'File' AND json_extract(extra, '$.hono_apps') IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut apps = HashMap::new();
    for row in rows {
        let (file, names) = row?;
        for name in serde_json::from_str::<Vec<String>>(&names).unwrap_or_default() {
            apps.insert(format!("{file}::{name}"), file.clone());
        }
    }
    Ok(apps)
}

/// A value reference to an app from outside its File: whoever holds the
/// app can send it a request. A type reference (`hc<typeof app>`, an RPC
/// client's type) carries a role and sends nothing.
fn app_references(tx: &Transaction<'_>, apps: &HashMap<String, String>) -> Result<Vec<NewCall>> {
    let mut stmt = tx.prepare(
        "SELECT source_qualified, target_qualified, file_path, line FROM edges \
         WHERE kind = 'REFERENCES' AND json_extract(extra, '$.relationship_role') IS NULL",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?;
    let mut calls = Vec::new();
    for row in rows {
        let (source, target, file_path, line) = row?;
        if let Some(file) = apps.get(&target)
            && *file != file_path
        {
            calls.push(NewCall {
                source,
                target: file.clone(),
                file_path,
                line,
                framework: "hono",
            });
        }
    }
    Ok(calls)
}

/// `app.fetch(..)` on an imported app, or one the caller's module declares
/// (`receiver_app`): Hono's own `fetch`, whatever member of the repository
/// resolution bound the name to.
fn retarget_dispatches(tx: &Transaction<'_>, apps: &HashMap<String, String>) -> Result<i64> {
    let mut stmt = tx.prepare(
        "SELECT id, target_qualified, target_name, confidence, confidence_tier, extra, \
                json_extract(receiver, '$.value'), json_extract(receiver, '$.member') FROM ( \
             SELECT *, COALESCE(json_extract(extra, '$.receiver_import'), \
                                json_extract(extra, '$.receiver_app')) AS receiver \
             FROM edges WHERE kind = 'CALLS') \
         WHERE receiver IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, f64>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, Option<String>>(7)?,
        ))
    })?;
    let mut moves = Vec::new();
    for row in rows {
        let (id, target, target_name, confidence, tier, extra, receiver, member) = row?;
        let Some(file) = apps.get(&receiver) else {
            continue;
        };
        if !member.is_some_and(|member| DISPATCH_MEMBERS.contains(&member.as_str())) {
            continue;
        }
        let original: Value = serde_json::from_str(&extra).unwrap_or_else(|_| json!({}));
        let mut moved = original.clone();
        if let Some(map) = moved.as_object_mut() {
            map.remove("receiver_unknown");
            map.insert("framework".to_string(), json!("hono"));
            map.insert(
                ORIGINAL.to_string(),
                json!({
                    "target": target,
                    "target_name": target_name,
                    "confidence": confidence,
                    "confidence_tier": tier,
                    "extra": original,
                }),
            );
        }
        moves.push((id, file.clone(), moved));
    }
    drop(stmt);
    for (id, file, extra) in &moves {
        tx.execute(
            "UPDATE edges SET target_qualified = ?, target_name = ?, extra = ?, \
             confidence = ?, confidence_tier = 'HIGH' WHERE id = ?",
            params![
                file,
                edge_target_name(file),
                extra_json(extra)?,
                REQUEST_BOUNDARY_CONFIDENCE,
                id
            ],
        )?;
    }
    Ok(moves.len() as i64)
}

/// A caller of `SELF.fetch` -> the worker's `default.fetch`, or the File
/// whose default export is the app (`export default app`).
fn self_fetches(tx: &Transaction<'_>, apps: &HashMap<String, String>) -> Result<Vec<NewCall>> {
    let mut stmt = tx.prepare(
        "SELECT source_qualified, target_qualified FROM edges \
         WHERE kind = 'CROSS_ARTIFACT' \
           AND json_extract(extra, '$.relationship_role') = 'worker_entry'",
    )?;
    let entries: Vec<(String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<std::result::Result<_, _>>()?;
    drop(stmt);
    if entries.is_empty() {
        return Ok(Vec::new());
    }
    let mut stmt = tx.prepare(
        "SELECT source_qualified, file_path, line FROM edges \
         WHERE kind = 'CALLS' AND target_qualified = ?",
    )?;
    let callers: Vec<(String, String, i64)> = stmt
        .query_map([SELF_FETCH], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<std::result::Result<_, _>>()?;
    drop(stmt);
    let mut calls = Vec::new();
    for (source, file_path, line) in callers {
        let entry = entries
            .iter()
            .filter(|(config, _)| {
                path_under(
                    config.rsplit_once('/').map_or("", |(dir, _)| dir),
                    &file_path,
                )
            })
            .max_by_key(|(config, _)| (config.matches('/').count(), std::cmp::Reverse(config)));
        let Some((_, main)) = entry else {
            continue;
        };
        let handler = format!("{main}::default.fetch");
        let exists: Option<i64> = tx
            .query_row(
                "SELECT 1 FROM nodes WHERE qualified_name = ?",
                [&handler],
                |row| row.get(0),
            )
            .optional()?;
        let target = if exists.is_some() {
            handler
        } else if let Some(file) = apps.get(&format!("{main}::default")) {
            file.clone()
        } else {
            continue;
        };
        calls.push(NewCall {
            source,
            target,
            file_path,
            line,
            framework: "cloudflare:test",
        });
    }
    Ok(calls)
}
