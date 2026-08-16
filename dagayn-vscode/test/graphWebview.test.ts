import * as assert from "node:assert";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { describe, it, beforeEach, afterEach } from "mocha";
import * as vscode from "vscode";
import { GraphWebviewPanel } from "../src/views/graphWebview";
import { SqliteReader } from "../src/backend/sqlite";
import { buildTestDb, type TestNode, type TestEdge } from "./helpers/schema";

type WebviewPanel = {
  webview: {
    __messages: Array<{ command: string; [key: string]: unknown }>;
    __clearMessages: () => void;
  };
  dispose: () => void;
};

const windowWithStubs = vscode.window as typeof vscode.window & {
  activeColorTheme: { kind: number };
  __createdWebviewPanels: WebviewPanel[];
  __clearCreatedWebviewPanels: () => void;
  __errorCalls: Array<{ message: string; buttons: unknown[] }>;
  __resetErrorCalls: () => void;
};

const workspaceWithStubs = vscode.workspace as typeof vscode.workspace & {
  __resetConfigStore: () => void;
};

function makeExtensionUri(tmpDir: string): typeof vscode.Uri.prototype {
  const htmlDir = path.join(tmpDir, "media", "webview");
  fs.mkdirSync(htmlDir, { recursive: true });
  fs.writeFileSync(
    path.join(htmlDir, "graph.html"),
    "{{NONCE}}{{CSP_SOURCE}}{{SCRIPT_URI}}{{STYLE_URI}}",
  );
  return vscode.Uri.file(tmpDir);
}

function buildLargeDb(nodeCount: number): string {
  const nodes: TestNode[] = [];
  const edges: TestEdge[] = [];
  const now = Date.now() / 1000;
  for (let i = 1; i <= nodeCount; i++) {
    const filePath = `src/file_${i}.py`;
    nodes.push({
      kind: "Function",
      name: `fn_${i}`,
      qualified_name: `src/file_${i}.py::fn_${i}`,
      file_path: filePath,
      line_start: i,
      line_end: i + 5,
      language: "python",
      parent_name: null,
      params: "()",
      return_type: "None",
      modifiers: null,
      is_test: 0,
      file_hash: "abc",
      extra: "{}",
      updated_at: now,
    });
    if (i > 1) {
      edges.push({
        kind: "CALLS",
        source_qualified: `src/file_${i}.py::fn_${i}`,
        target_qualified: `src/file_${i - 1}.py::fn_${i - 1}`,
        file_path: filePath,
        line: i,
        extra: "{}",
        updated_at: now,
      });
    }
  }
  return buildTestDb(nodes, edges);
}

describe("GraphWebviewPanel", () => {
  let tmpDir: string;
  let dbPath: string;
  let reader: SqliteReader;

  beforeEach(() => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "dagayn-webview-test-"));
    dbPath = buildLargeDb(600);
    reader = new SqliteReader(dbPath);
    windowWithStubs.activeColorTheme = { kind: vscode.ColorThemeKind.Dark };
    workspaceWithStubs.__resetConfigStore();
    windowWithStubs.__clearCreatedWebviewPanels();
    windowWithStubs.__resetErrorCalls();
    GraphWebviewPanel.__resetForTests();
  });

  afterEach(() => {
    for (const panel of windowWithStubs.__createdWebviewPanels) {
      panel.dispose();
    }
    GraphWebviewPanel.__resetForTests();
    reader.close();
    try {
      fs.rmSync(path.dirname(dbPath), { recursive: true, force: true });
      fs.rmSync(tmpDir, { recursive: true, force: true });
    } catch {
      // best effort
    }
  });

  it("sendGraphData truncates symbol graphs to graph.maxNodes", async () => {
    const extensionUri = makeExtensionUri(tmpDir);
    GraphWebviewPanel.createOrShow(extensionUri, reader, "test");

    const panel = windowWithStubs.__createdWebviewPanels[0];
    assert.ok(panel);

    await GraphWebviewPanel.__handleMessageForTests({ command: "ready" });

    const setData = panel.webview.__messages.find((m) => m.command === "setData") as
      | {
          command: string;
          nodes: unknown[];
          edges: unknown[];
          truncated: boolean;
          maxNodes: number;
        }
      | undefined;
    assert.ok(setData, "expected a setData message");
    assert.strictEqual(setData.nodes.length, 500);
    assert.strictEqual(setData.truncated, true);
    assert.strictEqual(setData.maxNodes, 500);

    const qns = new Set(
      (setData.nodes as Array<{ qualifiedName: string }>).map((n) => n.qualifiedName),
    );
    for (const edge of setData.edges as Array<{
      sourceQualified: string;
      targetQualified: string;
    }>) {
      assert.ok(qns.has(edge.sourceQualified), "edge source should be in retained nodes");
      assert.ok(qns.has(edge.targetQualified), "edge target should be in retained nodes");
    }
  });

  it("honours dagayn.graph.defaultEdges in symbol mode", async () => {
    const extensionUri = makeExtensionUri(tmpDir);
    // The fixture DB only has CALLS edges; restricting to DEPENDS_ON must
    // drop them all (and NOT fall back to an unfiltered edge list).
    await vscode.workspace.getConfiguration("dagayn").update("graph.defaultEdges", ["DEPENDS_ON"]);
    GraphWebviewPanel.createOrShow(extensionUri, reader, "test");

    const panel = windowWithStubs.__createdWebviewPanels[0];
    assert.ok(panel);

    await GraphWebviewPanel.__handleMessageForTests({ command: "ready" });

    const setData = panel.webview.__messages.find((m) => m.command === "setData") as
      | { command: string; edges: Array<{ kind: string }> }
      | undefined;
    assert.ok(setData, "expected a setData message");
    assert.strictEqual(setData.edges.length, 0, "CALLS-only edges must be filtered out");
  });

  it("honours dagayn.graphTheme forced theme", async () => {
    const extensionUri = makeExtensionUri(tmpDir);
    windowWithStubs.activeColorTheme = { kind: vscode.ColorThemeKind.Dark };
    await vscode.workspace.getConfiguration("dagayn").update("graphTheme", "light");
    GraphWebviewPanel.createOrShow(extensionUri, reader, "test");

    const panel = windowWithStubs.__createdWebviewPanels[0];
    assert.ok(panel);

    await GraphWebviewPanel.__handleMessageForTests({ command: "ready" });

    const setTheme = panel.webview.__messages.find((m) => m.command === "setTheme") as
      | { command: string; theme: string }
      | undefined;
    assert.ok(setTheme, "expected a setTheme message");
    assert.strictEqual(
      setTheme.theme,
      "light",
      "forced light theme should override dark active theme",
    );
  });

  it("handleMessage isolates errors and keeps the panel responsive", async () => {
    const extensionUri = makeExtensionUri(tmpDir);
    GraphWebviewPanel.createOrShow(extensionUri, reader, "test");

    const panel = windowWithStubs.__createdWebviewPanels[0];
    await GraphWebviewPanel.__handleMessageForTests({ command: "ready" });
    panel.webview.__clearMessages();

    // Malformed payload should be rejected without throwing.
    await GraphWebviewPanel.__handleMessageForTests({
      command: "nodeClicked",
      filePath: undefined,
      kind: "Function",
    });
    assert.ok(
      windowWithStubs.__errorCalls.length > 0,
      "expected an error message for invalid payload",
    );

    // A subsequent valid message should still be processed.
    await GraphWebviewPanel.__handleMessageForTests({ command: "ready" });
    const setData = panel.webview.__messages.find((m) => m.command === "setData");
    assert.ok(setData, "expected setData after a valid message following an error");
  });
});
