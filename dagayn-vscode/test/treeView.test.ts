import * as assert from "node:assert";
import * as vscode from "vscode";
import { CodeGraphTreeProvider, StatsTreeProvider } from "../src/views/treeView";
import {
  EdgeTreeItem,
  WorkspaceFolderTreeItem,
  FileTreeItem,
  SymbolTreeItem,
} from "../src/views/treeItems";
import { WorkspaceGraphRegistry } from "../src/backend/registry";
import { SqliteReader, GraphNode } from "../src/backend/sqlite";

const workspace = vscode.workspace as typeof vscode.workspace & {
  __setWorkspaceFolders: (
    folders: Array<{ uri: typeof vscode.Uri.prototype; name: string; index: number }>,
  ) => void;
};

function _makeFolder(fsPath: string, index: number) {
  return { uri: vscode.Uri.file(fsPath), name: fsPath.split("/").pop() ?? fsPath, index };
}

function makeReader(files: string[], nodesForFile: GraphNode[] = []): SqliteReader {
  return {
    getAllFiles: () => files,
    getNodesByFile: () => nodesForFile,
    getStats: () => ({
      totalNodes: 10,
      totalEdges: 5,
      filesCount: files.length,
      lastUpdated: "2025-06-15T10:30:00Z",
      languages: ["python"],
      nodesByKind: {},
      edgesByKind: {},
      embeddingsCount: 0,
    }),
  } as unknown as SqliteReader;
}

function makeRegistry(
  entries: Array<{ folder: string; files: string[]; nodes?: GraphNode[] }>,
): WorkspaceGraphRegistry {
  const readers = new Map<string, SqliteReader>();
  for (const { folder, files, nodes } of entries) {
    readers.set(folder, makeReader(files, nodes));
  }
  return {
    foldersWithGraph: () => [...readers.keys()],
    getReaderForFolder: (f: string) => readers.get(f),
    getReaderForUri: () => undefined,
    getReaderForActiveEditor: () => undefined,
    getAllReaders: () =>
      [...readers.entries()].map(([folderFsPath, reader]) => ({ folderFsPath, reader })),
    reinitializeFolder: async () => undefined,
    closeFolder: () => {},
    dispose: () => {},
  } as unknown as WorkspaceGraphRegistry;
}

describe("CodeGraphTreeProvider", () => {
  beforeEach(() => {
    workspace.__setWorkspaceFolders([]);
  });

  it("renders files directly for a single folder", async () => {
    const registry = makeRegistry([{ folder: "/workspace", files: ["/workspace/a.py"] }]);
    const provider = new CodeGraphTreeProvider(() => registry);

    const children = await provider.getChildren();
    assert.ok(children);
    assert.strictEqual(children!.length, 1);
    assert.ok(children![0] instanceof FileTreeItem);
  });

  it("renders SymbolTreeItem tooltips with docstring markdown", async () => {
    const nodes: GraphNode[] = [
      {
        id: 1,
        kind: "Function",
        name: "doThing",
        qualifiedName: "/workspace/a.py::doThing",
        filePath: "/workspace/a.py",
        lineStart: 10,
        lineEnd: 20,
        language: "python",
        parentName: null,
        params: null,
        returnType: null,
        modifiers: null,
        isTest: false,
        fileHash: null,
        extra: { docstring: "Does the thing." },
      },
    ];
    const registry = makeRegistry([{ folder: "/workspace", files: ["/workspace/a.py"], nodes }]);
    const provider = new CodeGraphTreeProvider(() => registry);

    const fileItem = (await provider.getChildren())![0] as FileTreeItem;
    const children = await provider.getChildren(fileItem);
    assert.ok(children);
    assert.strictEqual(children!.length, 1);
    assert.ok(children![0] instanceof SymbolTreeItem);

    const symbolItem = children![0] as SymbolTreeItem;
    assert.ok(symbolItem.tooltip instanceof vscode.MarkdownString);
    assert.ok(symbolItem.tooltip.value.includes("Does the thing."));
    assert.deepStrictEqual(symbolItem.extra, { docstring: "Does the thing." });
  });

  it("renders folder groups for multiple folders", async () => {
    const registry = makeRegistry([
      { folder: "/folder-a", files: ["/folder-a/a.py"] },
      { folder: "/folder-b", files: ["/folder-b/b.py"] },
    ]);
    const provider = new CodeGraphTreeProvider(() => registry);

    const children = await provider.getChildren();
    assert.ok(children);
    assert.strictEqual(children!.length, 2);
    assert.ok(children![0] instanceof WorkspaceFolderTreeItem);
    assert.ok(children![1] instanceof WorkspaceFolderTreeItem);

    const folderA = children![0] as WorkspaceFolderTreeItem;
    const files = await provider.getChildren(folderA);
    assert.ok(files);
    assert.strictEqual(files!.length, 1);
    assert.ok(files![0] instanceof FileTreeItem);
  });
});

describe("EdgeTreeItem", () => {
  it("renders CALLS outgoing and incoming labels and icons", () => {
    const outgoing = new EdgeTreeItem("CALLS", "outgoing", "/a.py::foo", "/a.py", 10);
    assert.strictEqual(outgoing.label, "\u2192 calls foo");
    assert.strictEqual((outgoing.iconPath as vscode.ThemeIcon).id, "arrow-right");

    const incoming = new EdgeTreeItem("CALLS", "incoming", "/a.py::foo", "/a.py", 10);
    assert.strictEqual(incoming.label, "\u2190 called by foo");
    assert.strictEqual((incoming.iconPath as vscode.ThemeIcon).id, "arrow-left");
  });

  it("uses the same icon for both directions when the edge is direction-agnostic", () => {
    const outgoing = new EdgeTreeItem("IMPORTS_FROM", "outgoing", "/a.py::foo", "/a.py", 1);
    const incoming = new EdgeTreeItem("IMPORTS_FROM", "incoming", "/a.py::foo", "/a.py", 1);
    assert.strictEqual((outgoing.iconPath as vscode.ThemeIcon).id, "package");
    assert.strictEqual((incoming.iconPath as vscode.ThemeIcon).id, "package");
  });

  it("falls back to the lower-case edge kind and arrow-right icon for unknown edges", () => {
    const item = new EdgeTreeItem("UNKNOWN", "outgoing", "/a.py::foo", "/a.py", 1);
    assert.strictEqual(item.label, "\u2192 unknown foo");
    assert.strictEqual((item.iconPath as vscode.ThemeIcon).id, "arrow-right");
  });
});

describe("StatsTreeProvider", () => {
  beforeEach(() => {
    workspace.__setWorkspaceFolders([]);
  });

  it("renders detailed stats for a single folder", async () => {
    const registry = makeRegistry([{ folder: "/workspace", files: ["/workspace/a.py"] }]);
    const provider = new StatsTreeProvider(() => registry);

    const children = await provider.getChildren();
    assert.ok(children);
    const labels = children!.map((c) => (c as { label?: string }).label);
    assert.ok(labels.includes("Files"));
    assert.ok(labels.includes("Total Nodes"));
  });

  it("renders one summary item per folder for multiple folders", async () => {
    const registry = makeRegistry([
      { folder: "/folder-a", files: ["/folder-a/a.py"] },
      { folder: "/folder-b", files: ["/folder-b/b.py"] },
    ]);
    const provider = new StatsTreeProvider(() => registry);

    const children = await provider.getChildren();
    assert.ok(children);
    assert.strictEqual(children!.length, 2);
  });
});
