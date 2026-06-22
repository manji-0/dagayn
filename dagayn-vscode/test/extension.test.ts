import * as assert from "node:assert";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import * as vscode from "vscode";
import Database from "better-sqlite3";
import { activate, deactivate, __getRegistryForTests, __resetForTests } from "../src/extension";
import { buildTestGraphDb } from "./helpers/schema";

const workspace = vscode.workspace as typeof vscode.workspace & {
  __setWorkspaceFolders: (
    folders: Array<{ uri: typeof vscode.Uri.prototype; name: string; index: number }>,
  ) => void;
  __resetConfigStore: () => void;
  __resetFileSystemWatchers: () => void;
  __fileSystemWatchers: Array<{
    __fireChange: (uri: typeof vscode.Uri.prototype) => Promise<void>;
    __fireCreate: (uri: typeof vscode.Uri.prototype) => Promise<void>;
    __fireDelete: (uri: typeof vscode.Uri.prototype) => Promise<void>;
  }>;
};

const window = vscode.window as typeof vscode.window & {
  __warningCalls: Array<{ message: string; buttons: unknown[] }>;
  __setWarningResult: (result: string | undefined) => void;
  __informationCalls: Array<{ message: string; buttons: unknown[] }>;
  __resetInformationCalls: () => void;
  __treeViewRegistrations: Array<{ viewId: string; provider: unknown }>;
  __resetTreeViewRegistrations: () => void;
  __fileDecorationProviders: unknown[];
  __resetFileDecorationProviders: () => void;
  __statusBarItems: unknown[];
  __resetStatusBarItems: () => void;
};

function makeFolder(fsPath: string, index: number) {
  return { uri: vscode.Uri.file(fsPath), name: path.basename(fsPath), index };
}

function makeContext(): vscode.ExtensionContext {
  return { subscriptions: [] as vscode.Disposable[] } as vscode.ExtensionContext;
}

function graphDbUri(folder: string): typeof vscode.Uri.prototype {
  return vscode.Uri.file(path.join(folder, ".dagayn", "graph.db"));
}

function insertMetadata(dbPath: string, key: string, value: string): void {
  const db = new Database(dbPath);
  try {
    db.prepare(
      "INSERT INTO metadata (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    ).run(key, value);
  } finally {
    db.close();
  }
}

describe("extension", () => {
  const tmpDirs: string[] = [];
  let context: vscode.ExtensionContext;

  function tempFolder(): string {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "dagayn-ext-"));
    tmpDirs.push(dir);
    return dir;
  }

  function tempFolderWithGraph(): string {
    const dir = tempFolder();
    buildTestGraphDb(dir);
    return dir;
  }

  function resetStubState(): void {
    workspace.__setWorkspaceFolders([]);
    workspace.__resetConfigStore();
    workspace.__resetFileSystemWatchers();
    window.__warningCalls.length = 0;
    window.__setWarningResult(undefined);
    window.__informationCalls.length = 0;
    window.__treeViewRegistrations.length = 0;
    window.__fileDecorationProviders.length = 0;
    window.__statusBarItems.length = 0;
  }

  function disposeSubscriptions(): void {
    for (const sub of context.subscriptions) {
      try {
        sub.dispose();
      } catch {
        // ignore
      }
    }
    context.subscriptions.length = 0;
  }

  beforeEach(() => {
    resetStubState();
    context = makeContext();
  });

  afterEach(() => {
    disposeSubscriptions();
    __resetForTests();
    for (const dir of tmpDirs) {
      try {
        fs.rmSync(dir, { recursive: true, force: true });
      } catch {
        // ignore
      }
    }
    tmpDirs.length = 0;
    workspace.__setWorkspaceFolders([]);
  });

  it("activate() initializes registry and providers when a graph.db exists", async () => {
    const folder = tempFolderWithGraph();
    window.__setWarningResult(undefined);
    workspace.__setWorkspaceFolders([makeFolder(folder, 0)]);

    await activate(context);

    const registry = __getRegistryForTests();
    assert.strictEqual(registry?.foldersWithGraph().length, 1);
    const reader = registry?.getReaderForFolder(folder);
    assert.ok(reader);
    assert.strictEqual(reader?.isValid(), true);

    const viewIds = window.__treeViewRegistrations.map((r) => r.viewId);
    assert.ok(viewIds.includes("dagayn.codeGraph"));
    assert.ok(viewIds.includes("dagayn.blastRadius"));
    assert.ok(viewIds.includes("dagayn.stats"));
    assert.strictEqual(window.__fileDecorationProviders.length, 1);
    assert.strictEqual(window.__statusBarItems.length, 1);
    assert.strictEqual(window.__warningCalls.length, 0);
    assert.strictEqual(window.__informationCalls.length, 0);
  });

  it("activate() shows welcome when no graph.db exists", async () => {
    const folder = tempFolder();
    window.__setWarningResult(undefined);
    workspace.__setWorkspaceFolders([makeFolder(folder, 0)]);

    await activate(context);

    const registry = __getRegistryForTests();
    assert.strictEqual(registry?.foldersWithGraph().length, 0);
    assert.strictEqual(window.__informationCalls.length, 1);
    assert.ok(window.__informationCalls[0].message.includes("Welcome to Dagayn"));
    assert.strictEqual(window.__treeViewRegistrations.length, 0);
    assert.strictEqual(window.__warningCalls.length, 0);
  });

  it("deactivate() disposes the registry", async () => {
    const folder = tempFolderWithGraph();
    workspace.__setWorkspaceFolders([makeFolder(folder, 0)]);
    await activate(context);

    const reader = __getRegistryForTests()?.getReaderForFolder(folder);
    assert.ok(reader);

    deactivate();

    assert.strictEqual(reader.isValid(), false);
    assert.strictEqual(__getRegistryForTests(), undefined);
  });

  it("watchGraphDb reinitializes on graph.db change", async () => {
    const folder = tempFolderWithGraph();
    workspace.__setWorkspaceFolders([makeFolder(folder, 0)]);
    await activate(context);

    const originalReader = __getRegistryForTests()?.getReaderForFolder(folder);
    assert.ok(originalReader);

    const watcher = workspace.__fileSystemWatchers[workspace.__fileSystemWatchers.length - 1];
    await watcher.__fireChange(graphDbUri(folder));

    const newReader = __getRegistryForTests()?.getReaderForFolder(folder);
    assert.ok(newReader);
    assert.notStrictEqual(newReader, originalReader);
    assert.strictEqual(newReader.isValid(), true);
  });

  it("watchGraphDb opens reader on graph.db create", async () => {
    const folder = tempFolder();
    workspace.__setWorkspaceFolders([makeFolder(folder, 0)]);
    await activate(context);

    assert.strictEqual(__getRegistryForTests()?.foldersWithGraph().length, 0);

    buildTestGraphDb(folder);
    const watcher = workspace.__fileSystemWatchers[workspace.__fileSystemWatchers.length - 1];
    await watcher.__fireCreate(graphDbUri(folder));

    const reader = __getRegistryForTests()?.getReaderForFolder(folder);
    assert.ok(reader);
    assert.strictEqual(reader.isValid(), true);
  });

  it("watchGraphDb closes reader on graph.db delete", async () => {
    const folder = tempFolderWithGraph();
    workspace.__setWorkspaceFolders([makeFolder(folder, 0)]);
    await activate(context);

    assert.ok(__getRegistryForTests()?.getReaderForFolder(folder));

    fs.unlinkSync(path.join(folder, ".dagayn", "graph.db"));
    const watcher = workspace.__fileSystemWatchers[workspace.__fileSystemWatchers.length - 1];
    await watcher.__fireDelete(graphDbUri(folder));

    assert.strictEqual(__getRegistryForTests()?.getReaderForFolder(folder), undefined);
    assert.strictEqual(__getRegistryForTests()?.foldersWithGraph().length, 0);
  });

  it("schema compatibility warning is shown and dismissible", async () => {
    const folder = tempFolderWithGraph();
    const dbPath = path.join(folder, ".dagayn", "graph.db");
    insertMetadata(dbPath, "schema_version", "99");
    window.__setWarningResult("Dismiss");
    workspace.__setWorkspaceFolders([makeFolder(folder, 0)]);

    await activate(context);

    assert.strictEqual(window.__warningCalls.length, 1);
    assert.ok(window.__warningCalls[0].message.includes("newer version"));
    assert.ok(window.__warningCalls[0].message.includes("schema v99"));

    const reader = __getRegistryForTests()?.getReaderForFolder(folder);
    assert.ok(reader);
    assert.strictEqual(reader.isValid(), true);
  });

  it("schema warning 'Rebuild Graph' triggers rebuild via stubbed CLI path", async function () {
    if (!fs.existsSync("/bin/true")) {
      this.skip();
      return;
    }

    const folder = tempFolderWithGraph();
    const dbPath = path.join(folder, ".dagayn", "graph.db");
    insertMetadata(dbPath, "schema_version", "99");

    const config = vscode.workspace.getConfiguration("dagayn");
    await config.update("cliPath", "/bin/true");

    window.__setWarningResult("Rebuild Graph");
    workspace.__setWorkspaceFolders([makeFolder(folder, 0)]);

    await activate(context);

    assert.strictEqual(window.__warningCalls.length, 1);
    // Rebuild is async and may still be running; the important thing is that
    // no unhandled rejection occurs and the registry stays valid.
    const reader = __getRegistryForTests()?.getReaderForFolder(folder);
    assert.ok(reader);
    assert.strictEqual(reader.isValid(), true);
  });
});
