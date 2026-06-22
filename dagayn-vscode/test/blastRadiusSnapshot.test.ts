import * as assert from "node:assert";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import * as vscode from "vscode";
import Database from "better-sqlite3";
import { WorkspaceGraphRegistry } from "../src/backend/registry";
import {
  BlastRadiusSnapshot,
  BlastRadiusSnapshotProvider,
  buildSnapshot,
  compareBlastRadiusSnapshot,
  compareSnapshots,
  loadSnapshot,
  listSnapshots,
  saveBlastRadiusSnapshot,
  saveSnapshot,
} from "../src/features/blastRadiusSnapshot";
import { buildTestGraphDb, TestEdge, TestNode } from "./helpers/schema";

const NOW = Date.now() / 1000;

function makeSampleGraph(root: string): { nodes: TestNode[]; edges: TestEdge[] } {
  const authPath = path.join(root, "src/auth.py");
  const routesPath = path.join(root, "src/routes.py");
  const testPath = path.join(root, "tests/test_auth.py");

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
      kind: "Function",
      name: "logout",
      qualified_name: `${authPath}::logout`,
      file_path: authPath,
      line_start: 22,
      line_end: 35,
      language: "python",
      parent_name: null,
      params: "(session)",
      return_type: "None",
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
    {
      kind: "File",
      name: "test_auth.py",
      qualified_name: testPath,
      file_path: testPath,
      line_start: 1,
      line_end: 30,
      language: "python",
      parent_name: null,
      params: null,
      return_type: null,
      modifiers: null,
      is_test: 0,
      file_hash: "ccc",
      extra: "{}",
      updated_at: NOW,
    },
    {
      kind: "Test",
      name: "test_login",
      qualified_name: `${testPath}::test_login`,
      file_path: testPath,
      line_start: 5,
      line_end: 25,
      language: "python",
      parent_name: null,
      params: "()",
      return_type: "None",
      modifiers: null,
      is_test: 1,
      file_hash: "ccc",
      extra: "{}",
      updated_at: NOW,
    },
  ];

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
      kind: "IMPORTS_FROM",
      source_qualified: routesPath,
      target_qualified: authPath,
      file_path: routesPath,
      line: 1,
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
      kind: "CONTAINS",
      source_qualified: authPath,
      target_qualified: `${authPath}::logout`,
      file_path: authPath,
      line: 22,
      extra: "{}",
      updated_at: NOW,
    },
    {
      kind: "TESTED_BY",
      source_qualified: `${authPath}::login`,
      target_qualified: `${testPath}::test_login`,
      file_path: testPath,
      line: 5,
      extra: "{}",
      updated_at: NOW,
    },
  ];

  return { nodes, edges };
}

function makeSnapshot(
  label: string,
  nodes: BlastRadiusSnapshot["nodes"],
  impactedFiles: string[],
): BlastRadiusSnapshot {
  return {
    schemaVersion: 1,
    createdAt: new Date().toISOString(),
    label,
    sourceFiles: [],
    depth: 2,
    graphDbPath: null,
    changedNodeQualifiedNames: [],
    impactedNodeQualifiedNames: nodes.map((n) => n.qualifiedName),
    impactedFiles,
    nodes,
  };
}

function resetStubState(): void {
  (vscode.window as unknown as { __warningCalls: unknown[] }).__warningCalls.length = 0;
  (vscode.window as unknown as { __inputBoxCalls: unknown[] }).__inputBoxCalls.length = 0;
  (vscode.window as unknown as { __quickPickCalls: unknown[] }).__quickPickCalls.length = 0;
  (
    vscode.window as unknown as { __outputChannels: Map<string, string[]> }
  ).__outputChannels.clear();
  (vscode.workspace as unknown as { __resetConfigStore: () => void }).__resetConfigStore();
  (
    vscode.window as unknown as { __setInputBoxResult: (value: unknown) => void }
  ).__setInputBoxResult(undefined);
  (
    vscode.window as unknown as { __setQuickPickResult: (value: unknown) => void }
  ).__setQuickPickResult(undefined);
  (
    vscode.workspace as unknown as { __setWorkspaceFolders: (folders: unknown[]) => void }
  ).__setWorkspaceFolders([]);
  (vscode.window as unknown as { activeTextEditor: unknown }).activeTextEditor = undefined;
}

function setActiveEditor(filePath: string, lineZeroBased = 0): void {
  (vscode.window as unknown as { activeTextEditor: unknown }).activeTextEditor = {
    document: {
      uri: vscode.Uri.file(filePath),
    },
    selection: {
      active: {
        line: lineZeroBased,
      },
    },
  };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("blastRadiusSnapshot", () => {
  describe("compareSnapshots", () => {
    const node = (qn: string, filePath: string, name: string) => ({
      qualifiedName: qn,
      name,
      kind: "Function" as const,
      filePath,
      lineStart: 1,
      lineEnd: 10,
    });

    it("partitions empty snapshots as empty", () => {
      const prev = makeSnapshot("prev", [], []);
      const curr = makeSnapshot("curr", [], []);
      const result = compareSnapshots(prev, curr);

      assert.strictEqual(result.added.length, 0);
      assert.strictEqual(result.removed.length, 0);
      assert.strictEqual(result.unchanged.length, 0);
      assert.strictEqual(result.addedFiles.length, 0);
      assert.strictEqual(result.removedFiles.length, 0);
    });

    it("marks identical snapshots as unchanged", () => {
      const nodes = [node("src/a.py::f", "src/a.py", "f")];
      const prev = makeSnapshot("prev", nodes, ["src/a.py"]);
      const curr = makeSnapshot("curr", nodes, ["src/a.py"]);
      const result = compareSnapshots(prev, curr);

      assert.strictEqual(result.unchanged.length, 1);
      assert.strictEqual(result.added.length, 0);
      assert.strictEqual(result.removed.length, 0);
    });

    it("detects added and removed nodes", () => {
      const prev = makeSnapshot("prev", [node("src/a.py::a", "src/a.py", "a")], ["src/a.py"]);
      const curr = makeSnapshot("curr", [node("src/a.py::b", "src/a.py", "b")], ["src/a.py"]);
      const result = compareSnapshots(prev, curr);

      assert.strictEqual(result.removed.length, 1);
      assert.strictEqual(result.removed[0].name, "a");
      assert.strictEqual(result.added.length, 1);
      assert.strictEqual(result.added[0].name, "b");
      assert.strictEqual(result.unchanged.length, 0);
    });

    it("uses filePath#name fallback when qualified name changed", () => {
      const prev = makeSnapshot(
        "prev",
        [
          {
            qualifiedName: "old::f",
            name: "f",
            kind: "Function",
            filePath: "src/a.py",
            lineStart: 1,
            lineEnd: 10,
          },
        ],
        ["src/a.py"],
      );
      const curr = makeSnapshot(
        "curr",
        [
          {
            qualifiedName: "new::f",
            name: "f",
            kind: "Function",
            filePath: "src/a.py",
            lineStart: 1,
            lineEnd: 10,
          },
        ],
        ["src/a.py"],
      );
      const result = compareSnapshots(prev, curr);

      assert.strictEqual(result.unchanged.length, 1);
      assert.strictEqual(result.added.length, 0);
      assert.strictEqual(result.removed.length, 0);
    });

    it("partitions files into added, removed, and unchanged", () => {
      const prev = makeSnapshot("prev", [], ["src/a.py", "src/b.py"]);
      const curr = makeSnapshot("curr", [], ["src/b.py", "src/c.py"]);
      const result = compareSnapshots(prev, curr);

      assert.deepStrictEqual(result.addedFiles, ["src/c.py"]);
      assert.deepStrictEqual(result.removedFiles, ["src/a.py"]);
      assert.deepStrictEqual(result.unchangedFiles, ["src/b.py"]);
    });
  });

  describe("snapshot persistence", () => {
    let tmpRoot: string;

    beforeEach(async () => {
      tmpRoot = await fs.mkdtemp(path.join(os.tmpdir(), "dagayn-snapshot-test-"));
    });

    afterEach(async () => {
      await fs.rm(tmpRoot, { recursive: true, force: true });
    });

    it("round-trips a snapshot through save, list, and load", async () => {
      const impact: Parameters<typeof buildSnapshot>[5] = {
        changedNodes: [],
        impactedNodes: [
          {
            id: 1,
            kind: "Function",
            name: "login",
            qualifiedName: "src/auth.py::login",
            filePath: path.join(tmpRoot, "src/auth.py"),
            lineStart: 5,
            lineEnd: 20,
            language: "python",
            parentName: null,
            params: null,
            returnType: null,
            modifiers: null,
            isTest: false,
            fileHash: null,
          },
        ],
        impactedFiles: [path.join(tmpRoot, "src/auth.py")],
        edges: [],
      };
      const snapshot = buildSnapshot(tmpRoot, "baseline", ["src/auth.py"], 2, null, impact);

      const savedPath = await saveSnapshot(tmpRoot, snapshot);
      assert.ok(savedPath.includes(".dagayn/snapshots"));

      const listed = await listSnapshots(tmpRoot);
      assert.strictEqual(listed.length, 1);
      assert.strictEqual(listed[0].label, "baseline");
      assert.strictEqual(listed[0].impactedNodeCount, 1);

      const loaded = await loadSnapshot(listed[0].path);
      assert.strictEqual(loaded.schemaVersion, 1);
      assert.deepStrictEqual(loaded.impactedNodeQualifiedNames, ["src/auth.py::login"]);
      assert.strictEqual(loaded.nodes[0].filePath, "src/auth.py");
    });

    it("rejects snapshots with an unsupported schema version", async () => {
      const filePath = path.join(tmpRoot, ".dagayn", "snapshots", "bad.json");
      await fs.mkdir(path.dirname(filePath), { recursive: true });
      await fs.writeFile(filePath, JSON.stringify({ schemaVersion: 99 }), "utf-8");

      await assert.rejects(async () => loadSnapshot(filePath), /Unsupported blast radius snapshot/);
    });
  });

  describe("save command", () => {
    let tmpRoot: string;
    let registry: WorkspaceGraphRegistry;

    beforeEach(async () => {
      resetStubState();
      tmpRoot = await fs.mkdtemp(path.join(os.tmpdir(), "dagayn-save-test-"));
      const { nodes, edges } = makeSampleGraph(tmpRoot);
      buildTestGraphDb(tmpRoot, nodes, edges);

      (
        vscode.workspace as unknown as { __setWorkspaceFolders: (folders: unknown[]) => void }
      ).__setWorkspaceFolders([{ uri: vscode.Uri.file(tmpRoot) }]);
      registry = new WorkspaceGraphRegistry();

      setActiveEditor(path.join(tmpRoot, "src/auth.py"), 9);
      (
        vscode.window as unknown as { __setInputBoxResult: (value: unknown) => void }
      ).__setInputBoxResult("baseline");
    });

    afterEach(async () => {
      registry.dispose();
      await fs.rm(tmpRoot, { recursive: true, force: true });
    });

    it("saves a snapshot and refreshes the tree provider", async () => {
      let providerChanged: { changed: unknown; impacted: unknown } | undefined;
      const provider: BlastRadiusSnapshotProvider = {
        setResults(changed, impacted) {
          providerChanged = { changed, impacted };
        },
      };

      const savedPath = await saveBlastRadiusSnapshot(registry, () => provider);
      assert.ok(savedPath);
      assert.ok(await fs.stat(savedPath!));

      const snapshot = JSON.parse(await fs.readFile(savedPath!, "utf-8")) as BlastRadiusSnapshot;
      assert.strictEqual(snapshot.label, "baseline");
      assert.strictEqual(snapshot.schemaVersion, 1);
      assert.deepStrictEqual(snapshot.sourceFiles, ["src/auth.py"]);
      assert.ok(!snapshot.impactedNodeQualifiedNames.includes("src/auth.py::login"));
      assert.ok(snapshot.impactedNodeQualifiedNames.includes("src/routes.py::handle_login"));

      assert.ok(providerChanged);
      assert.strictEqual(
        (providerChanged!.impacted as unknown[]).length,
        snapshot.impactedNodeQualifiedNames.length,
      );
    });

    it("warns when no graph database is loaded", async () => {
      (
        vscode.workspace as unknown as { __setWorkspaceFolders: (folders: unknown[]) => void }
      ).__setWorkspaceFolders([]);
      (
        vscode.window as unknown as { __warningCalls: { message: string }[] }
      ).__warningCalls.length = 0;
      const emptyRegistry = new WorkspaceGraphRegistry();
      setActiveEditor(path.join(tmpRoot, "src/auth.py"), 9);

      const result = await saveBlastRadiusSnapshot(emptyRegistry, () => undefined);
      assert.strictEqual(result, undefined);

      const warningCalls = (vscode.window as unknown as { __warningCalls: { message: string }[] })
        .__warningCalls;
      assert.ok(warningCalls.some((call) => call.message.includes("No graph database loaded")));
      emptyRegistry.dispose();
    });
  });

  describe("compare command", () => {
    let tmpRoot: string;
    let registry: WorkspaceGraphRegistry;
    let dbPath: string;

    beforeEach(async () => {
      resetStubState();
      tmpRoot = await fs.mkdtemp(path.join(os.tmpdir(), "dagayn-compare-test-"));
      const { nodes, edges } = makeSampleGraph(tmpRoot);
      dbPath = buildTestGraphDb(tmpRoot, nodes, edges);

      (
        vscode.workspace as unknown as { __setWorkspaceFolders: (folders: unknown[]) => void }
      ).__setWorkspaceFolders([{ uri: vscode.Uri.file(tmpRoot) }]);
      registry = new WorkspaceGraphRegistry();
      setActiveEditor(path.join(tmpRoot, "src/auth.py"), 9);

      // Save a baseline snapshot.
      (
        vscode.window as unknown as { __setInputBoxResult: (value: unknown) => void }
      ).__setInputBoxResult("baseline");
      await saveBlastRadiusSnapshot(registry, () => undefined);
    });

    afterEach(async () => {
      registry.dispose();
      await fs.rm(tmpRoot, { recursive: true, force: true });
    });

    it("compares a saved snapshot against the current blast radius", async () => {
      // Close the current reader and mutate the graph.
      registry.closeFolder(tmpRoot);
      const db = new Database(dbPath);
      const routesPath = path.join(tmpRoot, "src/routes.py");
      db.prepare(
        `INSERT INTO nodes
         (kind, name, qualified_name, file_path, line_start, line_end,
          language, parent_name, params, return_type, modifiers, is_test,
          file_hash, extra, updated_at)
         VALUES
         (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
      ).run(
        "Function",
        "new_helper",
        `${routesPath}::new_helper`,
        routesPath,
        40,
        50,
        "python",
        null,
        null,
        null,
        null,
        0,
        "ddd",
        "{}",
        NOW,
      );
      db.prepare(
        `INSERT INTO edges
         (kind, source_qualified, target_qualified, file_path, line, extra, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)`,
      ).run(
        "CALLS",
        `${routesPath}::handle_login`,
        `${routesPath}::new_helper`,
        routesPath,
        42,
        "{}",
        NOW,
      );
      db.close();

      await registry.reinitializeFolder(tmpRoot);

      const snapshots = await listSnapshots(tmpRoot);
      assert.strictEqual(snapshots.length, 1);
      (
        vscode.window as unknown as { __setQuickPickResult: (value: unknown) => void }
      ).__setQuickPickResult({ snapshot: snapshots[0] });

      await compareBlastRadiusSnapshot(registry);

      const output = (
        vscode.window as unknown as { __outputChannels: Map<string, string[]> }
      ).__outputChannels.get("Code Graph Blast Radius Compare");
      assert.ok(output);
      const text = output!.join("\n");
      assert.ok(text.includes("Newly impacted (1)"), text);
      assert.ok(text.includes("new_helper"), text);
      assert.ok(text.includes("Unchanged impacted"), text);
    });
  });
});
