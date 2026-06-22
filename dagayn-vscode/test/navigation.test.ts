import * as assert from "node:assert";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import * as vscode from "vscode";
import { SqliteReader } from "../src/backend/sqlite";
import {
  buildEdgeNavigationItems,
  buildTestNavigationItems,
  executeQuery,
  pickAndNavigate,
} from "../src/features/navigation";
import { buildTestGraphDb, TestEdge, TestNode } from "./helpers/schema";

const NOW = Date.now() / 1000;

function makeSampleGraph(root: string): { nodes: TestNode[]; edges: TestEdge[] } {
  const authPath = path.join(root, "src/auth.py");
  const routesPath = path.join(root, "src/routes.py");

  const nodes: TestNode[] = [
    {
      kind: "File",
      name: "auth.py",
      qualified_name: authPath,
      file_path: authPath,
      line_start: 1,
      line_end: 50,
      language: "python",
      parent_name: null,
      params: null,
      return_type: null,
      modifiers: null,
      is_test: 0,
      file_hash: "aaa",
      extra: "{}",
      updated_at: NOW,
    },
    {
      kind: "Function",
      name: "login",
      qualified_name: `${authPath}::login`,
      file_path: authPath,
      line_start: 5,
      line_end: 20,
      language: "python",
      parent_name: null,
      params: "(username, password)",
      return_type: "bool",
      modifiers: null,
      is_test: 0,
      file_hash: "aaa",
      extra: "{}",
      updated_at: NOW,
    },
    {
      kind: "File",
      name: "routes.py",
      qualified_name: routesPath,
      file_path: routesPath,
      line_start: 1,
      line_end: 40,
      language: "python",
      parent_name: null,
      params: null,
      return_type: null,
      modifiers: null,
      is_test: 0,
      file_hash: "bbb",
      extra: "{}",
      updated_at: NOW,
    },
    {
      kind: "Function",
      name: "handle_login",
      qualified_name: `${routesPath}::handle_login`,
      file_path: routesPath,
      line_start: 10,
      line_end: 30,
      language: "python",
      parent_name: null,
      params: "(request)",
      return_type: "Response",
      modifiers: null,
      is_test: 0,
      file_hash: "bbb",
      extra: "{}",
      updated_at: NOW,
    },
  ];

  const testPath = path.join(root, "src/test_auth.py");
  const testLoginQn = `${testPath}::test_login`;

  const edges: TestEdge[] = [
    {
      kind: "CALLS",
      source_qualified: `${routesPath}::handle_login`,
      target_qualified: `${authPath}::login`,
      file_path: routesPath,
      line: 15,
      extra: "{}",
      updated_at: NOW,
    },
    {
      kind: "CONTAINS",
      source_qualified: authPath,
      target_qualified: `${authPath}::login`,
      file_path: authPath,
      line: 5,
      extra: "{}",
      updated_at: NOW,
    },
    {
      kind: "TESTED_BY",
      source_qualified: testLoginQn,
      target_qualified: `${authPath}::login`,
      file_path: testPath,
      line: 8,
      extra: "{}",
      updated_at: NOW,
    },
  ];

  nodes.push({
    kind: "Test",
    name: "test_login",
    qualified_name: testLoginQn,
    file_path: testPath,
    line_start: 5,
    line_end: 25,
    language: "python",
    parent_name: null,
    params: null,
    return_type: null,
    modifiers: null,
    is_test: 1,
    file_hash: "ccc",
    extra: "{}",
    updated_at: NOW,
  });

  return { nodes, edges };
}

function resetStubState(): void {
  (vscode.window as unknown as { __warningCalls: unknown[] }).__warningCalls.length = 0;
  (vscode.window as unknown as { __inputBoxCalls: unknown[] }).__inputBoxCalls.length = 0;
  (vscode.window as unknown as { __quickPickCalls: unknown[] }).__quickPickCalls.length = 0;
  (
    vscode.window as unknown as { __setInputBoxResult: (value: unknown) => void }
  ).__setInputBoxResult(undefined);
  (
    vscode.window as unknown as { __setQuickPickResult: (value: unknown) => void }
  ).__setQuickPickResult(undefined);
}

describe("navigation query execution", () => {
  let tmpRoot: string;
  let reader: SqliteReader;

  beforeEach(async () => {
    resetStubState();
    tmpRoot = await fs.mkdtemp(path.join(os.tmpdir(), "dagayn-nav-test-"));
    const { nodes, edges } = makeSampleGraph(tmpRoot);
    buildTestGraphDb(tmpRoot, nodes, edges);
    reader = new SqliteReader(path.join(tmpRoot, ".dagayn", "graph.db"));
  });

  afterEach(async () => {
    reader.close();
    await fs.rm(tmpRoot, { recursive: true, force: true });
  });

  it("executeQuery returns callers for a known target", async () => {
    const target = path.join(tmpRoot, "src/auth.py::login");
    const items = await executeQuery(reader, "callers_of", target);

    assert.strictEqual(items.length, 1);
    assert.strictEqual(items[0].label, "handle_login");
    assert.ok(items[0].description.includes("routes.py"));
  });

  it("executeQuery returns an empty array for an unknown target", async () => {
    const items = await executeQuery(reader, "callers_of", "no-such-node");
    assert.deepStrictEqual(items, []);
  });

  it("executeQuery returns an empty array for an unknown pattern", async () => {
    const target = path.join(tmpRoot, "src/auth.py::login");
    const items = await executeQuery(reader, "unknown_pattern", target);
    assert.deepStrictEqual(items, []);
  });
});

describe("buildEdgeNavigationItems", () => {
  let tmpRoot: string;
  let reader: SqliteReader;

  beforeEach(async () => {
    tmpRoot = await fs.mkdtemp(path.join(os.tmpdir(), "dagayn-nav-test-"));
    const { nodes, edges } = makeSampleGraph(tmpRoot);
    buildTestGraphDb(tmpRoot, nodes, edges);
    reader = new SqliteReader(path.join(tmpRoot, ".dagayn", "graph.db"));
  });

  afterEach(async () => {
    reader.close();
    await fs.rm(tmpRoot, { recursive: true, force: true });
  });

  it("uses the filePath fallback when the related node is missing", () => {
    const edge = {
      id: 99,
      kind: "CALLS" as const,
      sourceQualified: "missing-source",
      targetQualified: "missing-target",
      filePath: "/fallback/path.py",
      line: 42,
    };
    const items = buildEdgeNavigationItems(reader, [edge], "outgoing");
    assert.strictEqual(items.length, 1);
    assert.strictEqual(items[0].label, "missing-target");
    assert.strictEqual(items[0].description, "/fallback/path.py");
    assert.strictEqual(items[0].detail, "Line 42");
    assert.strictEqual(items[0].node, undefined);
  });

  it("keeps the callers/callees description format (no kind prefix)", () => {
    const loginNode = reader.getNode(path.join(tmpRoot, "src/auth.py::login"));
    assert.ok(loginNode);
    const callerEdges = reader
      .getEdgesByTarget(loginNode!.qualifiedName)
      .filter((e) => e.kind === "CALLS");
    const items = buildEdgeNavigationItems(reader, callerEdges, "incoming");
    assert.strictEqual(items.length, 1);
    assert.strictEqual(items[0].label, "handle_login");
    assert.strictEqual(items[0].description, path.join(tmpRoot, "src/routes.py"));
    assert.strictEqual(items[0].detail, "Line 10");
    assert.ok(items[0].node);
  });
});

describe("buildTestNavigationItems", () => {
  let tmpRoot: string;
  let reader: SqliteReader;

  beforeEach(async () => {
    tmpRoot = await fs.mkdtemp(path.join(os.tmpdir(), "dagayn-nav-test-"));
    const { nodes, edges } = makeSampleGraph(tmpRoot);
    buildTestGraphDb(tmpRoot, nodes, edges);
    reader = new SqliteReader(path.join(tmpRoot, ".dagayn", "graph.db"));
  });

  afterEach(async () => {
    reader.close();
    await fs.rm(tmpRoot, { recursive: true, force: true });
  });

  it("shows 'Line ?' when the test node has no line start", () => {
    const loginNode = reader.getNode(path.join(tmpRoot, "src/auth.py::login"));
    assert.ok(loginNode);
    const items = buildTestNavigationItems(reader, loginNode!);
    assert.strictEqual(items.length, 1);
    assert.strictEqual(items[0].label, "test_login");
    assert.strictEqual(items[0].detail, "Line 5");
    assert.strictEqual(
      items[0].node?.qualifiedName,
      path.join(tmpRoot, "src/test_auth.py::test_login"),
    );
  });
});

describe("pickAndNavigate", () => {
  beforeEach(() => {
    resetStubState();
  });

  it("passes items and placeholder to showQuickPick", async () => {
    const item = {
      label: "handle_login",
      description: "Function · src/routes.py",
      detail: "Line 10",
      node: undefined,
    };
    (
      vscode.window as unknown as { __setQuickPickResult: (value: unknown) => void }
    ).__setQuickPickResult(item);

    await pickAndNavigate([item], "Callers of login");

    const calls = (
      vscode.window as unknown as {
        __quickPickCalls: { items: unknown[]; options?: { placeHolder?: string } }[];
      }
    ).__quickPickCalls;
    assert.strictEqual(calls.length, 1);
    assert.strictEqual(calls[0].items.length, 1);
    assert.strictEqual(calls[0].items[0], item);
    assert.strictEqual(calls[0].options?.placeHolder, "Callers of login");
  });
});
