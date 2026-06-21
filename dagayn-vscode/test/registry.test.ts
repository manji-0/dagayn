import * as assert from "node:assert";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import * as vscode from "vscode";
import { WorkspaceGraphRegistry } from "../src/backend/registry";
import { buildTestGraphDb } from "./helpers/schema";

const workspace = vscode.workspace as typeof vscode.workspace & {
  __setWorkspaceFolders: (
    folders: Array<{ uri: typeof vscode.Uri.prototype; name: string; index: number }>,
  ) => void;
};

function makeFolder(fsPath: string, index: number) {
  return { uri: vscode.Uri.file(fsPath), name: path.basename(fsPath), index };
}

describe("WorkspaceGraphRegistry", () => {
  let tmpDirs: string[] = [];

  afterEach(() => {
    for (const dir of tmpDirs) {
      try {
        fs.rmSync(dir, { recursive: true, force: true });
      } catch {
        // ignore
      }
    }
    tmpDirs = [];
    workspace.__setWorkspaceFolders([]);
  });

  function tempFolderWithGraph(): string {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "dagayn-reg-"));
    buildTestGraphDb(dir);
    tmpDirs.push(dir);
    return dir;
  }

  it("opens readers for every folder that has a graph.db", () => {
    const folderA = tempFolderWithGraph();
    const folderB = tempFolderWithGraph();
    workspace.__setWorkspaceFolders([makeFolder(folderA, 0), makeFolder(folderB, 1)]);

    const registry = new WorkspaceGraphRegistry();

    assert.strictEqual(registry.foldersWithGraph().length, 2);
    assert.ok(registry.getReaderForFolder(folderA));
    assert.ok(registry.getReaderForFolder(folderB));
    assert.notStrictEqual(
      registry.getReaderForFolder(folderA),
      registry.getReaderForFolder(folderB),
    );

    registry.dispose();
  });

  it("resolves a reader by URI using the containing workspace folder", () => {
    const folderA = tempFolderWithGraph();
    const folderB = tempFolderWithGraph();
    workspace.__setWorkspaceFolders([makeFolder(folderA, 0), makeFolder(folderB, 1)]);

    const registry = new WorkspaceGraphRegistry();
    const reader = registry.getReaderForUri(vscode.Uri.file(path.join(folderB, "src", "x.py")));

    assert.strictEqual(reader, registry.getReaderForFolder(folderB));

    registry.dispose();
  });

  it("returns undefined for URIs outside all workspace folders", () => {
    const folderA = tempFolderWithGraph();
    workspace.__setWorkspaceFolders([makeFolder(folderA, 0)]);

    const registry = new WorkspaceGraphRegistry();
    const reader = registry.getReaderForUri(vscode.Uri.file("/outside/project/file.py"));

    assert.strictEqual(reader, undefined);

    registry.dispose();
  });

  it("reinitializeFolder closes and reopens the reader", async () => {
    const folderA = tempFolderWithGraph();
    workspace.__setWorkspaceFolders([makeFolder(folderA, 0)]);

    const registry = new WorkspaceGraphRegistry();
    const original = registry.getReaderForFolder(folderA);
    assert.ok(original);

    const next = await registry.reinitializeFolder(folderA);
    assert.ok(next);
    assert.notStrictEqual(original, next);
    assert.strictEqual(registry.getReaderForFolder(folderA), next);

    registry.dispose();
  });

  it("dispose closes all readers", () => {
    const folderA = tempFolderWithGraph();
    const folderB = tempFolderWithGraph();
    workspace.__setWorkspaceFolders([makeFolder(folderA, 0), makeFolder(folderB, 1)]);

    const registry = new WorkspaceGraphRegistry();
    const readerA = registry.getReaderForFolder(folderA)!;
    const readerB = registry.getReaderForFolder(folderB)!;

    registry.dispose();

    assert.strictEqual(readerA.isValid(), false);
    assert.strictEqual(readerB.isValid(), false);
    assert.strictEqual(registry.foldersWithGraph().length, 0);
  });
});
