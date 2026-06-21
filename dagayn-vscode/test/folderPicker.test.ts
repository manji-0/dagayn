import * as assert from "node:assert";
import * as vscode from "vscode";
import { WorkspaceGraphRegistry } from "../src/backend/registry";
import { pickFolderForBuild, pickFolderForGlobalOp } from "../src/backend/folderPicker";

const workspace = vscode.workspace as typeof vscode.workspace & {
  __setWorkspaceFolders: (
    folders: Array<{ uri: typeof vscode.Uri.prototype; name: string; index: number }>,
  ) => void;
};
const window = vscode.window as typeof vscode.window & {
  __quickPickCalls: Array<{ items: unknown; options: unknown }>;
  __setQuickPickResult: (result: unknown) => void;
  activeTextEditor: { document: { uri: typeof vscode.Uri.prototype } } | undefined;
};

function makeFolder(fsPath: string, index: number) {
  return { uri: vscode.Uri.file(fsPath), name: fsPath.split("/").pop() ?? fsPath, index };
}

function makeRegistry(folders: string[]): WorkspaceGraphRegistry {
  return {
    foldersWithGraph: () => folders,
    getReaderForFolder: () => undefined,
    getReaderForUri: () => undefined,
    getReaderForActiveEditor: () => undefined,
    getAllReaders: () =>
      folders.map((f) => ({
        folderFsPath: f,
        reader: {} as import("../src/backend/sqlite").SqliteReader,
      })),
    reinitializeFolder: async () => undefined,
    closeFolder: () => {},
    dispose: () => {},
  } as unknown as WorkspaceGraphRegistry;
}

describe("folderPicker", () => {
  beforeEach(() => {
    workspace.__setWorkspaceFolders([]);
    window.__quickPickCalls.length = 0;
    window.__setQuickPickResult(undefined);
    window.activeTextEditor = undefined;
  });

  it("returns the single folder without showing a picker", async () => {
    workspace.__setWorkspaceFolders([makeFolder("/workspace", 0)]);
    const registry = makeRegistry(["/workspace"]);

    const picked = await pickFolderForGlobalOp(registry, { requireGraph: true });

    assert.strictEqual(picked, "/workspace");
    assert.strictEqual(window.__quickPickCalls.length, 0);
  });

  it("returns the active editor's folder when it has a graph", async () => {
    workspace.__setWorkspaceFolders([makeFolder("/folder-a", 0), makeFolder("/folder-b", 1)]);
    window.activeTextEditor = {
      document: { uri: vscode.Uri.file("/folder-b/src/x.py") },
    } as typeof window.activeTextEditor;
    const registry = makeRegistry(["/folder-a", "/folder-b"]);

    const picked = await pickFolderForGlobalOp(registry, { requireGraph: true });

    assert.strictEqual(picked, "/folder-b");
    assert.strictEqual(window.__quickPickCalls.length, 0);
  });

  it("shows a picker when multiple folders exist and no active editor", async () => {
    workspace.__setWorkspaceFolders([makeFolder("/folder-a", 0), makeFolder("/folder-b", 1)]);
    const registry = makeRegistry(["/folder-a", "/folder-b"]);
    window.__setQuickPickResult({ folderFsPath: "/folder-a" });

    const picked = await pickFolderForGlobalOp(registry, { requireGraph: true });

    assert.strictEqual(picked, "/folder-a");
    assert.strictEqual(window.__quickPickCalls.length, 1);
  });

  it("pickFolderForBuild includes folders without an existing graph", async () => {
    workspace.__setWorkspaceFolders([makeFolder("/folder-a", 0), makeFolder("/folder-b", 1)]);
    const registry = makeRegistry(["/folder-a"]);
    window.__setQuickPickResult({ folderFsPath: "/folder-b" });

    const picked = await pickFolderForBuild(registry);

    assert.strictEqual(picked, "/folder-b");
    assert.strictEqual(window.__quickPickCalls.length, 1);
  });
});
