use dagayn_build::db_path_for_build;
use dagayn_graph::GraphStore;

use crate::common::{Repo, commit_all, counts_as_tested};

/// A Hono worker: `wrangler.jsonc` names `src/index.ts`, whose
/// `default.fetch` calls the app in `src/app.ts`, which mounts the users
/// routes. `src/users.test.ts` sends a request with `SELF.fetch`, and
/// `src/app.test.ts` calls `app.request(..)`.
fn hono_worker(repo: &Repo) {
    repo.write(
        "package.json",
        "{\"name\": \"w\", \"dependencies\": {\"hono\": \"4.0.0\"}}\n",
    );
    repo.write(
        "wrangler.jsonc",
        "{\n  // entry\n  \"main\": \"src/index.ts\",\n}\n",
    );
    repo.write(
        "src/index.ts",
        "import { app } from \"./app\";\n\nexport default {\n  fetch: (request: Request, env: Env, ctx: ExecutionContext) => app.fetch(request, env, ctx),\n};\n",
    );
    repo.write("src/app.ts", HONO_APP);
    repo.write(
        "src/routes/users.ts",
        "import { Hono } from \"hono\";\nimport { findUser } from \"../store\";\n\nexport const users = new Hono();\nusers.get(\"/users/:id\", (c) => c.json(findUser(c.req.param(\"id\"))));\n",
    );
    repo.write(
        "src/store.ts",
        "export function findUser(id: string) {\n  return { id };\n}\n",
    );
    repo.write(
        "src/stamp.ts",
        "export async function stamp(c: unknown, next: () => Promise<void>) {\n  await next();\n}\n",
    );
    // A Durable Object with a `fetch` of its own, which the name alone
    // would bind `app.fetch(..)` to.
    repo.write(
        "src/hub.ts",
        "export class Hub {\n  fetch(request: Request) {\n    return new Response(request.url);\n  }\n}\n",
    );
    // An RPC client names the app's type only.
    repo.write(
        "src/client.ts",
        "import { hc } from \"hono/client\";\nimport { app } from \"./app\";\n\nexport type App = typeof app;\nexport const client = hc<typeof app>(\"https://example.com\");\n",
    );
    repo.write(
        "src/users.test.ts",
        "import { SELF } from \"cloudflare:test\";\nimport { expect, it } from \"vitest\";\n\nconst get = (path: string) => SELF.fetch(`https://example.com${path}`);\n\nit(\"finds a user\", async () => {\n  expect((await get(\"/users/1\")).status).toBe(200);\n});\n",
    );
    repo.write(
        "src/app.test.ts",
        "import { expect, it } from \"vitest\";\nimport { app } from \"./app\";\n\nit(\"serves a user\", async () => {\n  expect((await app.request(\"/users/1\")).status).toBe(200);\n});\n",
    );
}

const HONO_APP: &str = "import { Hono } from \"hono\";\nimport { users } from \"./routes/users\";\nimport { stamp } from \"./stamp\";\n\nexport const app = new Hono();\napp.use(\"*\", stamp);\napp.route(\"/\", users);\n";

const NOT_HONO: &str = "export const app = {\n  route() {},\n  fetch(request: Request) {\n    return request;\n  },\n  request(path: string) {\n    return path;\n  },\n};\n";

/// The graph's edges of `kinds`, as sorted `kind source -> target
/// @file:line` rows.
fn edge_rows(repo: &Repo, kinds: &[&str]) -> Vec<String> {
    let conn = rusqlite::Connection::open(repo.0.join(".dagayn/graph.db")).expect("open");
    let mut stmt = conn
        .prepare("SELECT kind, source_qualified, target_qualified, file_path, line FROM edges")
        .expect("prepare");
    let mut rows: Vec<String> = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .expect("query")
        .map(|row| row.expect("row"))
        .filter(|(kind, ..)| kinds.contains(&kind.as_str()))
        .map(|(kind, source, target, file, line)| {
            format!("{kind} {source} -> {target} @{file}:{line}")
        })
        .collect();
    rows.sort();
    rows
}

/// Rows into `src/app.ts` as a call target or out of it as a TESTED_BY
/// source.
fn into_app(rows: &[String]) -> Vec<String> {
    rows.iter()
        .filter(|row| row.contains(" -> src/app.ts @") || row.starts_with("TESTED_BY src/app.ts "))
        .cloned()
        .collect()
}

#[test]
fn a_worker_test_request_reaches_the_hono_routes() {
    // dagayn: tests crates/dagayn-graph/src/postprocess/request_boundary.rs::GraphStore.resolve_request_boundaries
    let repo = Repo::new("worker-request", false);
    hono_worker(&repo);
    repo.build();
    // The handler's call, the middleware, and the entry are under the
    // request; the Durable Object's `fetch` is not.
    assert!(counts_as_tested(&repo, "src/store.ts::findUser"));
    assert!(counts_as_tested(&repo, "src/stamp.ts::stamp"));
    assert!(counts_as_tested(&repo, "src/index.ts::default.fetch"));
    assert!(!counts_as_tested(&repo, "src/hub.ts::Hub.fetch"));
    let calls = edge_rows(&repo, &["CALLS"]);
    assert!(
        !calls
            .iter()
            .any(|row| row.starts_with("CALLS src/client.ts") && row.contains("-> src/app.ts @")),
        "a type reference is no request: {calls:?}"
    );
    // A test's request is a test's call from the first build on.
    let tested_by = edge_rows(&repo, &["TESTED_BY"]);
    assert!(
        tested_by
            .iter()
            .any(|row| row.starts_with("TESTED_BY src/app.ts -> src/app.test.ts::")),
        "{tested_by:?}"
    );
    // Another post-processing run writes the same graph.
    let before = edge_rows(&repo, &["CALLS", "TESTED_BY", "REFERENCES"]);
    let db = db_path_for_build(&repo.0).expect("db path");
    let mut store = GraphStore::open(&db).expect("store");
    store.resolve_bare_call_targets().expect("resolution");
    store.resolve_request_boundaries().expect("pass");
    drop(store);
    assert_eq!(
        edge_rows(&repo, &["CALLS", "TESTED_BY", "REFERENCES"]),
        before
    );

    // Without a wrangler configuration naming the worker, `SELF.fetch`
    // reaches nothing.
    std::fs::remove_file(repo.0.join("wrangler.jsonc")).unwrap();
    std::fs::remove_file(repo.0.join("src/app.test.ts")).unwrap();
    repo.build();
    assert!(!counts_as_tested(&repo, "src/store.ts::findUser"));
}

#[test]
fn a_create_hono_worker_exports_the_app() {
    // dagayn: tests crates/dagayn-graph/src/postprocess/request_boundary.rs::GraphStore.resolve_request_boundaries
    let repo = Repo::new("worker-default-app", false);
    repo.write("wrangler.toml", "name = \"w\"\nmain = \"src/index.ts\"\n");
    repo.write(
        "src/index.ts",
        "import { Hono } from \"hono\";\nimport { findUser } from \"./store\";\n\nconst app = new Hono().basePath(\"/api\");\napp.get(\"/users/:id\", (c) => c.json(findUser(c.req.param(\"id\"))));\n\nexport default app;\n",
    );
    repo.write(
        "src/store.ts",
        "export function findUser(id: string) {\n  return { id };\n}\n",
    );
    repo.write(
        "src/index.test.ts",
        "import { SELF } from \"cloudflare:test\";\nimport { it } from \"vitest\";\n\nit(\"finds a user\", async () => {\n  await SELF.fetch(\"https://example.com/api/users/1\");\n});\n",
    );
    repo.write(
        "src/default.test.ts",
        "import { it } from \"vitest\";\nimport server from \"./index\";\n\nit(\"serves\", async () => {\n  await server.request(\"/api/users/1\");\n});\n",
    );
    repo.build();
    assert!(counts_as_tested(&repo, "src/store.ts::findUser"));
    let calls = edge_rows(&repo, &["CALLS"]);
    for source in ["src/index.test.ts::", "src/default.test.ts::"] {
        assert!(
            calls
                .iter()
                .any(|row| row.starts_with(&format!("CALLS {source}"))
                    && row.contains("-> src/index.ts @")),
            "{source}: {calls:?}"
        );
    }
}

#[test]
fn a_worker_dispatches_to_an_app_of_its_own_module() {
    // dagayn: tests crates/dagayn-graph/src/postprocess/request_boundary.rs::GraphStore.resolve_request_boundaries
    let repo = Repo::new("worker-own-app", false);
    repo.write("wrangler.toml", "name = \"w\"\nmain = \"src/index.ts\"\n");
    // The usual shape when the Worker also exports `scheduled`.
    repo.write(
        "src/index.ts",
        "import { Hono } from \"hono\";\nimport { findUser } from \"./store\";\n\nconst app = new Hono();\napp.get(\"/users/:id\", (c) => c.json(findUser(c.req.param(\"id\"))));\n\nexport default {\n  fetch(request: Request, env: Env, ctx: ExecutionContext) {\n    return app.fetch(request, env, ctx);\n  },\n  scheduled() {},\n};\n",
    );
    repo.write(
        "src/store.ts",
        "export function findUser(id: string) {\n  return { id };\n}\n",
    );
    repo.write(
        "src/index.test.ts",
        "import { SELF } from \"cloudflare:test\";\nimport { it } from \"vitest\";\n\nit(\"finds a user\", async () => {\n  await SELF.fetch(\"https://example.com/users/1\");\n});\n",
    );
    repo.build();
    assert!(counts_as_tested(&repo, "src/store.ts::findUser"));
    let calls = edge_rows(&repo, &["CALLS"]);
    let into_file: Vec<_> = calls
        .iter()
        .filter(|row| row.contains("-> src/index.ts @"))
        .collect();
    // `default.fetch` dispatches to the app.
    assert!(
        into_file.len() == 1 && into_file[0].starts_with("CALLS src/index.ts::default.fetch "),
        "{calls:?}"
    );
}

#[test]
fn a_dispatch_whose_resolved_target_is_deleted_goes_back_to_the_bare_name() {
    // dagayn: tests crates/dagayn-graph/src/postprocess/request_boundary.rs::GraphStore.resolve_request_boundaries
    let repo = Repo::new("worker-target-deleted", true);
    hono_worker(&repo);
    // `Hub` reaches the entry, so name resolution binds `app.fetch(..)`
    // to `Hub.fetch` before the pass moves it to the app.
    repo.write(
        "src/index.ts",
        "import { app } from \"./app\";\nexport { Hub } from \"./hub\";\n\nexport default {\n  fetch: (request: Request, env: Env, ctx: ExecutionContext) => app.fetch(request, env, ctx),\n};\n",
    );
    commit_all(&repo, "worker");
    repo.build();
    // The Durable Object and the app both go; the entry is not re-parsed.
    std::fs::remove_file(repo.0.join("src/hub.ts")).unwrap();
    repo.write("src/app.ts", NOT_HONO);
    commit_all(&repo, "no hub, not hono");
    repo.update("HEAD~1");
    let rows = edge_rows(&repo, &["CALLS"]);
    assert!(
        rows.iter()
            .any(|row| row.starts_with("CALLS src/index.ts::default.fetch -> fetch @")),
        "{rows:?}"
    );
    assert!(
        !rows.iter().any(|row| row.contains("-> src/hub.ts::")),
        "{rows:?}"
    );
    let conn = rusqlite::Connection::open(repo.0.join(".dagayn/graph.db")).expect("open");
    let tier: String = conn
        .query_row(
            "SELECT confidence_tier FROM edges WHERE kind = 'CALLS' \
             AND source_qualified = 'src/index.ts::default.fetch' AND target_qualified = 'fetch'",
            [],
            |row| row.get(0),
        )
        .expect("restored edge");
    assert_eq!(tier, "LOW");
}

#[test]
fn request_boundary_calls_follow_incremental_updates() {
    // dagayn: tests crates/dagayn-graph/src/postprocess/request_boundary.rs::GraphStore.resolve_request_boundaries
    let repo = Repo::new("worker-update", true);
    hono_worker(&repo);
    commit_all(&repo, "worker");
    repo.build();
    let full = edge_rows(&repo, &["CALLS", "TESTED_BY"]);
    assert!(counts_as_tested(&repo, "src/store.ts::findUser"));

    // An unrelated change: the update leaves the request edges as the full
    // build has them.
    repo.write(
        "src/hub.ts",
        "export class Hub {\n  fetch(request: Request) {\n    return new Response(request.method);\n  }\n}\n",
    );
    commit_all(&repo, "hub");
    repo.update("HEAD~1");
    assert_eq!(edge_rows(&repo, &["CALLS", "TESTED_BY"]), full);

    // `app` stops being a Hono app. Its importers are not re-parsed, and
    // their dispatches go back to what name resolution bound.
    repo.write("src/app.ts", NOT_HONO);
    commit_all(&repo, "not hono");
    repo.update("HEAD~1");
    let rows = edge_rows(&repo, &["CALLS", "TESTED_BY"]);
    assert_eq!(into_app(&rows), Vec::<String>::new(), "{rows:?}");
    assert!(!counts_as_tested(&repo, "src/store.ts::findUser"));

    // Hono again; then the wrangler configuration goes, and `SELF.fetch`
    // no longer reaches the worker.
    repo.write("src/app.ts", HONO_APP);
    commit_all(&repo, "hono again");
    repo.update("HEAD~1");
    assert!(!into_app(&edge_rows(&repo, &["CALLS"])).is_empty());
    std::fs::remove_file(repo.0.join("wrangler.jsonc")).unwrap();
    commit_all(&repo, "no wrangler");
    repo.update("HEAD~1");
    let rows = edge_rows(&repo, &["CALLS", "CROSS_ARTIFACT"]);
    assert!(
        !rows
            .iter()
            .any(|row| row.contains("wrangler.jsonc")
                || row.contains("-> src/index.ts::default.fetch")),
        "{rows:?}"
    );
    // And it comes back with the configuration.
    repo.write("wrangler.jsonc", "{\n  \"main\": \"src/index.ts\",\n}\n");
    commit_all(&repo, "wrangler again");
    repo.update("HEAD~1");
    assert!(
        edge_rows(&repo, &["CALLS"])
            .iter()
            .any(|row| row.contains("-> src/index.ts::default.fetch"))
    );
}
