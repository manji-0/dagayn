/**
 * Tests for the moduleAggregation module.
 *
 * Validates directory-level aggregation of graph nodes and edges using the
 * same buildTestDb helper used by the SqliteReader tests.
 */

import * as assert from "node:assert";
import * as fs from "node:fs";
import * as path from "node:path";
import { SqliteReader } from "../src/backend/sqlite";
import { aggregateModules, DEFAULT_MODULE_EDGE_KINDS } from "../src/backend/moduleAggregation";
import { buildTestDb, TestNode, TestEdge } from "./helpers/schema";

const NOW = Date.now() / 1000;

function node(
  kind: string,
  name: string,
  qualifiedName: string,
  filePath: string,
  language: string,
): TestNode {
  return {
    kind,
    name,
    qualified_name: qualifiedName,
    file_path: filePath,
    line_start: 1,
    line_end: 1,
    language,
    parent_name: null,
    params: null,
    return_type: null,
    modifiers: null,
    is_test: 0,
    file_hash: "hash",
    extra: "{}",
    updated_at: NOW,
  };
}

function fileNode(filePath: string, language: string): TestNode {
  return {
    kind: "File",
    name: path.basename(filePath),
    qualified_name: filePath,
    file_path: filePath,
    line_start: 1,
    line_end: 1,
    language,
    parent_name: null,
    params: null,
    return_type: null,
    modifiers: null,
    is_test: 0,
    file_hash: "hash",
    extra: "{}",
    updated_at: NOW,
  };
}

function edge(kind: string, source: string, target: string, filePath: string): TestEdge {
  return {
    kind,
    source_qualified: source,
    target_qualified: target,
    file_path: filePath,
    line: 1,
    extra: "{}",
    updated_at: NOW,
  };
}

function cleanup(dbPath: string): void {
  try {
    const dir = path.dirname(dbPath);
    fs.rmSync(dir, { recursive: true, force: true });
  } catch {
    // best effort
  }
}

describe("aggregateModules", () => {
  it("produces one node per parent directory and derives cross-dir edges", () => {
    const dbPath = buildTestDb(
      [
        fileNode("src/auth.py", "python"),
        node("Function", "login", "src/auth.py::login", "src/auth.py", "python"),
        fileNode("tests/test_auth.py", "python"),
        node(
          "Test",
          "test_login",
          "tests/test_auth.py::test_login",
          "tests/test_auth.py",
          "python",
        ),
      ],
      [edge("CALLS", "tests/test_auth.py::test_login", "src/auth.py::login", "tests/test_auth.py")],
    );
    const reader = new SqliteReader(dbPath);

    const graph = aggregateModules(reader);

    assert.strictEqual(graph.nodes.length, 2);
    const srcNode = graph.nodes.find((n) => n.dirPath === "src");
    const testsNode = graph.nodes.find((n) => n.dirPath === "tests");
    assert.ok(srcNode);
    assert.ok(testsNode);
    assert.strictEqual(srcNode.fileCount, 1);
    assert.strictEqual(testsNode.fileCount, 1);

    assert.strictEqual(graph.edges.length, 1);
    assert.strictEqual(graph.edges[0].kind, "CALLS");
    assert.strictEqual(graph.edges[0].sourceDir, "tests");
    assert.strictEqual(graph.edges[0].targetDir, "src");
    assert.strictEqual(graph.edges[0].count, 1);

    reader.close();
    cleanup(dbPath);
  });

  it("accumulates multiple edges between the same directory pair", () => {
    const dbPath = buildTestDb(
      [
        fileNode("src/auth.py", "python"),
        node("Function", "login", "src/auth.py::login", "src/auth.py", "python"),
        node("Function", "logout", "src/auth.py::logout", "src/auth.py", "python"),
        fileNode("tests/test_auth.py", "python"),
        node(
          "Test",
          "test_login",
          "tests/test_auth.py::test_login",
          "tests/test_auth.py",
          "python",
        ),
      ],
      [
        edge("CALLS", "tests/test_auth.py::test_login", "src/auth.py::login", "tests/test_auth.py"),
        edge(
          "CALLS",
          "tests/test_auth.py::test_login",
          "src/auth.py::logout",
          "tests/test_auth.py",
        ),
      ],
    );
    const reader = new SqliteReader(dbPath);

    const graph = aggregateModules(reader);

    assert.strictEqual(graph.edges.length, 1);
    assert.strictEqual(graph.edges[0].count, 2);

    reader.close();
    cleanup(dbPath);
  });

  it("excludes edges whose endpoints resolve to the same directory", () => {
    const dbPath = buildTestDb(
      [
        fileNode("src/auth.py", "python"),
        node("Function", "login", "src/auth.py::login", "src/auth.py", "python"),
        node("Function", "logout", "src/auth.py::logout", "src/auth.py", "python"),
      ],
      [edge("CALLS", "src/auth.py::login", "src/auth.py::logout", "src/auth.py")],
    );
    const reader = new SqliteReader(dbPath);

    const graph = aggregateModules(reader);

    assert.strictEqual(graph.nodes.length, 1);
    assert.strictEqual(graph.edges.length, 0);

    reader.close();
    cleanup(dbPath);
  });

  it("skips unresolved edge endpoints", () => {
    const dbPath = buildTestDb(
      [
        fileNode("src/auth.py", "python"),
        node("Function", "login", "src/auth.py::login", "src/auth.py", "python"),
      ],
      [edge("CALLS", "src/auth.py::login", "nonexistent::target", "src/auth.py")],
    );
    const reader = new SqliteReader(dbPath);

    const graph = aggregateModules(reader);

    assert.strictEqual(graph.nodes.length, 1);
    assert.strictEqual(graph.edges.length, 0);

    reader.close();
    cleanup(dbPath);
  });

  it("filters edges by the requested edge kinds", () => {
    const dbPath = buildTestDb(
      [
        fileNode("src/auth.py", "python"),
        node("Function", "login", "src/auth.py::login", "src/auth.py", "python"),
        fileNode("src/routes.py", "python"),
        node("Function", "handle", "src/routes.py::handle", "src/routes.py", "python"),
        fileNode("tests/test_auth.py", "python"),
        node(
          "Test",
          "test_login",
          "tests/test_auth.py::test_login",
          "tests/test_auth.py",
          "python",
        ),
      ],
      [
        edge("CALLS", "src/routes.py::handle", "src/auth.py::login", "src/routes.py"),
        edge(
          "IMPORTS_FROM",
          "tests/test_auth.py::test_login",
          "src/auth.py::login",
          "tests/test_auth.py",
        ),
      ],
    );
    const reader = new SqliteReader(dbPath);

    const graph = aggregateModules(reader, ["IMPORTS_FROM"]);

    assert.strictEqual(graph.edges.length, 1);
    assert.strictEqual(graph.edges[0].kind, "IMPORTS_FROM");

    reader.close();
    cleanup(dbPath);
  });

  it("returns empty arrays for an empty graph", () => {
    const dbPath = buildTestDb();
    const reader = new SqliteReader(dbPath);

    const graph = aggregateModules(reader);

    assert.deepStrictEqual(graph.nodes, []);
    assert.deepStrictEqual(graph.edges, []);

    reader.close();
    cleanup(dbPath);
  });

  it("reports file count and language per module", () => {
    const dbPath = buildTestDb(
      [
        fileNode("src/auth.py", "python"),
        fileNode("src/routes.py", "python"),
        fileNode("app/main.ts", "typescript"),
      ],
      [],
    );
    const reader = new SqliteReader(dbPath);

    const graph = aggregateModules(reader);

    const src = graph.nodes.find((n) => n.dirPath === "src");
    const app = graph.nodes.find((n) => n.dirPath === "app");
    assert.ok(src);
    assert.ok(app);
    assert.strictEqual(src.fileCount, 2);
    assert.strictEqual(src.language, "python");
    assert.strictEqual(app.fileCount, 1);
    assert.strictEqual(app.language, "typescript");

    reader.close();
    cleanup(dbPath);
  });

  it("sorts nodes alphabetically and assigns sequential ids", () => {
    const dbPath = buildTestDb(
      [
        fileNode("z/module.py", "python"),
        fileNode("a/module.py", "python"),
        fileNode("m/module.py", "python"),
      ],
      [],
    );
    const reader = new SqliteReader(dbPath);

    const graph = aggregateModules(reader);

    assert.deepStrictEqual(
      graph.nodes.map((n) => ({ id: n.id, dirPath: n.dirPath })),
      [
        { id: 1, dirPath: "a" },
        { id: 2, dirPath: "m" },
        { id: 3, dirPath: "z" },
      ],
    );

    reader.close();
    cleanup(dbPath);
  });

  it("handles file-level edges via File node qualified names", () => {
    const dbPath = buildTestDb(
      [fileNode("routes/routes.py", "python"), fileNode("auth/auth.py", "python")],
      [edge("IMPORTS_FROM", "routes/routes.py", "auth/auth.py", "routes/routes.py")],
    );
    const reader = new SqliteReader(dbPath);

    const graph = aggregateModules(reader);

    assert.strictEqual(graph.edges.length, 1);
    assert.strictEqual(graph.edges[0].sourceDir, "routes");
    assert.strictEqual(graph.edges[0].targetDir, "auth");
    assert.strictEqual(graph.edges[0].count, 1);

    reader.close();
    cleanup(dbPath);
  });

  it("uses DEFAULT_MODULE_EDGE_KINDS as the default edge set", () => {
    const dbPath = buildTestDb(
      [
        fileNode("auth/auth.py", "python"),
        node("Function", "login", "auth/auth.py::login", "auth/auth.py", "python"),
        fileNode("routes/routes.py", "python"),
        node("Function", "handle", "routes/routes.py::handle", "routes/routes.py", "python"),
      ],
      [
        edge("CALLS", "routes/routes.py::handle", "auth/auth.py::login", "routes/routes.py"),
        edge("CONTAINS", "auth/auth.py", "auth/auth.py::login", "auth/auth.py"),
      ],
    );
    const reader = new SqliteReader(dbPath);

    const graph = aggregateModules(reader);
    const kinds = graph.edges.map((e) => e.kind);

    assert.ok(kinds.includes("CALLS"));
    assert.ok(!kinds.includes("CONTAINS"));
    assert.deepStrictEqual(DEFAULT_MODULE_EDGE_KINDS, ["CALLS", "IMPORTS_FROM", "DEPENDS_ON"]);

    reader.close();
    cleanup(dbPath);
  });
});
