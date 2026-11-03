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
//!   `main` from the `worker_entry` manifest bridge of the nearest wrangler
//!   configuration above the caller's file;
//! * a reference to an app (`app.route("/", sub)`, `testClient(app)`) ->
//!   the File that declares it;
//! * `app.fetch(..)` / `app.request(..)` on an imported app
//!   (`receiver_import`), which name resolution could only guess at
//!   (`StreamHub.fetch`), retargeted to that File.
//!
//! A request reaches every route of the app, not the route its path names.
//! On tstodon, linking each test to only the handlers coverage says ran
//! changes 2 of the 1,001 answers it calls tested
//! (docs/plans/TYPESCRIPT-SUPPORT-MEASUREMENTS.md#t3-request-boundary-edges),
//! and most tests build the path in a helper (`SELF.fetch(base + path)`).

use super::native_bindings::path_under;
use crate::helpers::*;
use crate::*;

const REQUEST_BOUNDARY_EXTRACTOR: &str = "request_boundary";
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
    /// Replace the `request_boundary` calls and retarget dispatches on
    /// imported Hono apps; returns how many edges were written or moved.
    pub fn resolve_request_boundaries(&mut self) -> Result<i64> {
        let now = now_seconds()?;
        let tx = write_tx(&mut self.conn)?;
        tx.execute(
            "DELETE FROM edges WHERE kind = 'CALLS' \
             AND json_extract(extra, '$.extractor') = ?",
            [REQUEST_BOUNDARY_EXTRACTOR],
        )?;
        let apps = hono_apps(&tx)?;
        let mut calls = Vec::new();
        let mut moved = 0_i64;
        if !apps.is_empty() {
            calls.extend(app_references(&tx, &apps)?);
            moved = retarget_dispatches(&tx, &apps)?;
        }
        calls.extend(self_fetches(&tx)?);
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
            let extra = serde_json::json!({
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
        tx.commit()?;
        Ok(written + moved)
    }
}

/// `file::app` of every Hono app -> the File that declares it.
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

/// A reference to an app from outside its File: whoever holds the app can
/// send it a request.
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

/// `app.fetch(..)` on an imported app: Hono's own `fetch`, whatever member
/// of the repository resolution bound the name to.
fn retarget_dispatches(tx: &Transaction<'_>, apps: &HashMap<String, String>) -> Result<i64> {
    let mut stmt = tx.prepare(
        "SELECT id, target_name, json_extract(extra, '$.receiver_import'), extra FROM edges \
         WHERE kind = 'CALLS' AND json_extract(extra, '$.receiver_import') IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    let mut moves = Vec::new();
    for row in rows {
        let (id, target_name, receiver, extra) = row?;
        let member = target_name.rsplit('.').next().unwrap_or(&target_name);
        if let Some(file) = apps.get(&receiver)
            && DISPATCH_MEMBERS.contains(&member)
        {
            let mut extra: Value = serde_json::from_str(&extra).unwrap_or_else(|_| json!({}));
            if let Some(map) = extra.as_object_mut() {
                map.remove("receiver_unknown");
                map.insert("framework".to_string(), json!("hono"));
            }
            moves.push((id, file.clone(), extra));
        }
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

/// A caller of `SELF.fetch` -> the worker's `default.fetch`.
fn self_fetches(tx: &Transaction<'_>) -> Result<Vec<NewCall>> {
    let mut stmt = tx.prepare(
        "SELECT e.source_qualified, e.target_qualified FROM edges e \
         WHERE e.kind = 'CROSS_ARTIFACT' \
           AND json_extract(e.extra, '$.relationship_role') = 'worker_entry'",
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
        let target = format!("{main}::default.fetch");
        let exists: Option<i64> = tx
            .query_row(
                "SELECT 1 FROM nodes WHERE qualified_name = ?",
                [&target],
                |row| row.get(0),
            )
            .optional()?;
        if exists.is_some() {
            calls.push(NewCall {
                source,
                target,
                file_path,
                line,
                framework: "cloudflare:test",
            });
        }
    }
    Ok(calls)
}
