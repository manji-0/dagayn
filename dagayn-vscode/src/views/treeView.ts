import * as vscode from "vscode";
import * as path from "node:path";
import { SqliteReader, GraphNode } from "../backend/sqlite";
import { WorkspaceGraphRegistry } from "../backend/registry";
import {
  FileTreeItem,
  SymbolTreeItem,
  EdgeTreeItem,
  BlastRadiusGroupItem,
  StatsItem,
  WorkspaceFolderTreeItem,
} from "./treeItems";

// ---------------------------------------------------------------------------
// CodeGraphTreeProvider -- main file > symbol > edge tree
// ---------------------------------------------------------------------------

export class CodeGraphTreeProvider implements vscode.TreeDataProvider<vscode.TreeItem> {
  private readonly _onDidChangeTreeData = new vscode.EventEmitter<
    vscode.TreeItem | undefined | null
  >();
  readonly onDidChangeTreeData: vscode.Event<vscode.TreeItem | undefined | null> =
    this._onDidChangeTreeData.event;

  constructor(private readonly getRegistry: () => WorkspaceGraphRegistry | undefined) {}

  refresh(): void {
    this._onDidChangeTreeData.fire(undefined);
  }

  getTreeItem(element: vscode.TreeItem): vscode.TreeItem {
    return element;
  }

  getChildren(element?: vscode.TreeItem): vscode.ProviderResult<vscode.TreeItem[]> {
    if (!element) {
      return this.getRootChildren();
    }
    if (element instanceof WorkspaceFolderTreeItem) {
      return this.getFolderChildren(element);
    }
    if (element instanceof FileTreeItem) {
      return this.getFileChildren(element);
    }
    if (element instanceof SymbolTreeItem) {
      return this.getSymbolChildren(element);
    }
    return [];
  }

  // -- Root level: either folder groups or files directly -------------------

  private getRootChildren(): vscode.TreeItem[] {
    const registry = this.getRegistry();
    if (!registry) {
      return [];
    }
    const entries = registry.getAllReaders();
    if (entries.length === 0) {
      return [];
    }
    if (entries.length === 1) {
      return this.filesToItems(entries[0]!.reader, entries[0]!.folderFsPath);
    }
    return entries.map(
      ({ folderFsPath, reader }) =>
        new WorkspaceFolderTreeItem(folderFsPath, this.countFiles(reader)),
    );
  }

  private getFolderChildren(folderItem: WorkspaceFolderTreeItem): vscode.TreeItem[] {
    const registry = this.getRegistry();
    if (!registry) {
      return [];
    }
    const reader = registry.getReaderForFolder(folderItem.folderFsPath);
    if (!reader) {
      return [];
    }
    return this.filesToItems(reader, folderItem.folderFsPath);
  }

  private filesToItems(reader: SqliteReader, workspaceRoot: string): FileTreeItem[] {
    const files = reader.getAllFiles();
    return files
      .slice()
      .sort((a, b) => a.localeCompare(b))
      .map((filePath) => new FileTreeItem(filePath, workspaceRoot));
  }

  private countFiles(reader: SqliteReader): number {
    return reader.getAllFiles().length;
  }

  // -- File level: symbols (non-File nodes) sorted by line ------------------

  private getFileChildren(fileItem: FileTreeItem): vscode.TreeItem[] {
    const registry = this.getRegistry();
    if (!registry) {
      return [];
    }
    const reader = registry.getReaderForFolder(fileItem.folderFsPath);
    if (!reader) {
      // Fall back to any reader that contains the file path for single-folder
      // compatibility when the item was created before the registry was wired.
      const all = registry.getAllReaders();
      const match = all.find((e) => fileItem.filePath.startsWith(e.folderFsPath));
      if (!match) {
        return [];
      }
      return this.nodesForFile(match.reader, fileItem.filePath);
    }
    return this.nodesForFile(reader, fileItem.filePath);
  }

  private nodesForFile(reader: SqliteReader, filePath: string): SymbolTreeItem[] {
    const nodes = reader.getNodesByFile(filePath);
    return nodes
      .filter((n) => n.kind !== "File")
      .sort((a, b) => (a.lineStart ?? 0) - (b.lineStart ?? 0))
      .map(
        (n) =>
          new SymbolTreeItem(
            n.qualifiedName,
            n.name,
            n.kind,
            n.filePath,
            n.lineStart,
            n.lineEnd,
            n.extra,
          ),
      );
  }

  // -- Symbol level: outgoing + incoming edges (skip CONTAINS) --------------

  private getSymbolChildren(symbolItem: SymbolTreeItem): vscode.TreeItem[] {
    const registry = this.getRegistry();
    if (!registry) {
      return [];
    }
    const folder = vscode.workspace.getWorkspaceFolder(vscode.Uri.file(symbolItem.filePath));
    const reader = folder ? registry.getReaderForFolder(folder.uri.fsPath) : undefined;
    if (!reader) {
      return [];
    }
    const items: vscode.TreeItem[] = [];

    // Outgoing edges
    const outgoing = reader.getEdgesBySource(symbolItem.qualifiedName);
    for (const edge of outgoing) {
      if (edge.kind === "CONTAINS") {
        continue;
      }
      const targetNode = reader.getNode(edge.targetQualified);
      const targetFile = targetNode?.filePath ?? edge.filePath;
      const targetLine = targetNode?.lineStart ?? edge.line;
      items.push(
        new EdgeTreeItem(edge.kind, "outgoing", edge.targetQualified, targetFile, targetLine),
      );
    }

    // Incoming edges
    const incoming = reader.getEdgesByTarget(symbolItem.qualifiedName);
    for (const edge of incoming) {
      if (edge.kind === "CONTAINS") {
        continue;
      }
      const sourceNode = reader.getNode(edge.sourceQualified);
      const sourceFile = sourceNode?.filePath ?? edge.filePath;
      const sourceLine = sourceNode?.lineStart ?? edge.line;
      items.push(
        new EdgeTreeItem(edge.kind, "incoming", edge.sourceQualified, sourceFile, sourceLine),
      );
    }

    return items;
  }
}

// ---------------------------------------------------------------------------
// BlastRadiusTreeProvider -- shows changed + impacted nodes
// ---------------------------------------------------------------------------

export class BlastRadiusTreeProvider implements vscode.TreeDataProvider<vscode.TreeItem> {
  private readonly _onDidChangeTreeData = new vscode.EventEmitter<
    vscode.TreeItem | undefined | null
  >();
  readonly onDidChangeTreeData: vscode.Event<vscode.TreeItem | undefined | null> =
    this._onDidChangeTreeData.event;

  private changedNodes: GraphNode[] = [];
  private impactedNodes: GraphNode[] = [];

  setResults(changed: GraphNode[], impacted: GraphNode[]): void {
    this.changedNodes = changed;
    this.impactedNodes = impacted;
    this._onDidChangeTreeData.fire(undefined);
  }

  clear(): void {
    this.changedNodes = [];
    this.impactedNodes = [];
    this._onDidChangeTreeData.fire(undefined);
  }

  getTreeItem(element: vscode.TreeItem): vscode.TreeItem {
    return element;
  }

  getChildren(element?: vscode.TreeItem): vscode.ProviderResult<vscode.TreeItem[]> {
    if (!element) {
      return this.getRootChildren();
    }
    if (element instanceof BlastRadiusGroupItem) {
      return this.getGroupChildren(element);
    }
    return [];
  }

  private getRootChildren(): vscode.TreeItem[] {
    if (this.changedNodes.length === 0 && this.impactedNodes.length === 0) {
      return [];
    }
    const groups: vscode.TreeItem[] = [];
    if (this.changedNodes.length > 0) {
      groups.push(new BlastRadiusGroupItem("changed", this.changedNodes.length));
    }
    if (this.impactedNodes.length > 0) {
      groups.push(new BlastRadiusGroupItem("impacted", this.impactedNodes.length));
    }
    return groups;
  }

  private getGroupChildren(group: BlastRadiusGroupItem): vscode.TreeItem[] {
    const nodes = group.groupKind === "changed" ? this.changedNodes : this.impactedNodes;
    return nodes.map(
      (n) =>
        new SymbolTreeItem(
          n.qualifiedName,
          n.name,
          n.kind,
          n.filePath,
          n.lineStart,
          n.lineEnd,
          n.extra,
        ),
    );
  }
}

// ---------------------------------------------------------------------------
// StatsTreeProvider -- graph statistics overview
// ---------------------------------------------------------------------------

export class StatsTreeProvider implements vscode.TreeDataProvider<vscode.TreeItem> {
  private readonly _onDidChangeTreeData = new vscode.EventEmitter<
    vscode.TreeItem | undefined | null
  >();
  readonly onDidChangeTreeData: vscode.Event<vscode.TreeItem | undefined | null> =
    this._onDidChangeTreeData.event;

  constructor(private readonly getRegistry: () => WorkspaceGraphRegistry | undefined) {}

  refresh(): void {
    this._onDidChangeTreeData.fire(undefined);
  }

  getTreeItem(element: vscode.TreeItem): vscode.TreeItem {
    return element;
  }

  getChildren(): vscode.ProviderResult<vscode.TreeItem[]> {
    const registry = this.getRegistry();
    if (!registry) {
      return [];
    }
    const folders = registry.foldersWithGraph();
    if (folders.length === 0) {
      return [];
    }
    if (folders.length === 1) {
      const reader = registry.getReaderForFolder(folders[0]!);
      if (!reader) {
        return [];
      }
      return this.statsItems(reader);
    }
    return folders.map((folderFsPath) => {
      const reader = registry.getReaderForFolder(folderFsPath);
      const stats = reader?.getStats();
      const label = stats
        ? `${path.basename(folderFsPath)} — ${stats.totalNodes} nodes / ${stats.totalEdges} edges`
        : path.basename(folderFsPath);
      return new StatsItem(label, "");
    });
  }

  private statsItems(reader: SqliteReader): StatsItem[] {
    const stats = reader.getStats();
    const items: StatsItem[] = [];

    items.push(new StatsItem("Files", stats.filesCount.toLocaleString()));
    items.push(new StatsItem("Total Nodes", stats.totalNodes.toLocaleString()));
    items.push(new StatsItem("Total Edges", stats.totalEdges.toLocaleString()));
    items.push(
      new StatsItem("Languages", stats.languages.length > 0 ? stats.languages.join(", ") : "none"),
    );
    items.push(new StatsItem("Last Updated", stats.lastUpdated ?? "unknown"));
    items.push(
      new StatsItem(
        "Embeddings",
        stats.embeddingsCount > 0 ? stats.embeddingsCount.toLocaleString() : "none",
      ),
    );

    return items;
  }
}
