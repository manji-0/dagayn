import * as vscode from "vscode";
import * as fs from "node:fs/promises";
import * as path from "node:path";
import { SqliteReader, GraphNode, NodeKind, ImpactRadius } from "../backend/sqlite";
import { WorkspaceGraphRegistry } from "../backend/registry";

// ---------------------------------------------------------------------------
// Snapshot types and pure helpers (no vscode dependency)
// ---------------------------------------------------------------------------

export interface NodeSummary {
  qualifiedName: string;
  name: string;
  kind: NodeKind;
  filePath: string;
  lineStart: number | null;
  lineEnd: number | null;
}

export interface BlastRadiusSnapshot {
  schemaVersion: 1;
  createdAt: string;
  label: string;
  sourceFiles: string[];
  depth: number;
  graphDbPath: string | null;
  changedNodeQualifiedNames: string[];
  impactedNodeQualifiedNames: string[];
  impactedFiles: string[];
  nodes: NodeSummary[];
}

export interface SnapshotComparison {
  added: NodeSummary[];
  removed: NodeSummary[];
  unchanged: NodeSummary[];
  addedFiles: string[];
  removedFiles: string[];
  unchangedFiles: string[];
  previous: {
    impactedNodeCount: number;
    impactedFileCount: number;
  };
  current: {
    impactedNodeCount: number;
    impactedFileCount: number;
  };
  summary: {
    added: number;
    removed: number;
    unchanged: number;
    addedFiles: number;
    removedFiles: number;
  };
}

function toNodeSummary(node: GraphNode): NodeSummary {
  return {
    qualifiedName: node.qualifiedName,
    name: node.name,
    kind: node.kind,
    filePath: node.filePath,
    lineStart: node.lineStart,
    lineEnd: node.lineEnd,
  };
}

function fallbackKey(node: NodeSummary): string {
  return `${node.filePath}#${node.name}`;
}

function makeRelative(workspaceRoot: string, filePath: string): string {
  if (!path.isAbsolute(filePath)) {
    return filePath;
  }
  const rel = path.relative(workspaceRoot, filePath);
  return rel && !rel.startsWith("..") ? rel : filePath;
}

export function buildSnapshot(
  workspaceRoot: string,
  label: string,
  sourceFiles: string[],
  depth: number,
  graphDbPath: string | null,
  impact: ImpactRadius,
): BlastRadiusSnapshot {
  const createdAt = new Date().toISOString();
  return {
    schemaVersion: 1,
    createdAt,
    label,
    sourceFiles: sourceFiles.map((f) => makeRelative(workspaceRoot, f)),
    depth,
    graphDbPath,
    changedNodeQualifiedNames: impact.changedNodes.map((n) =>
      makeRelative(workspaceRoot, n.qualifiedName),
    ),
    impactedNodeQualifiedNames: impact.impactedNodes.map((n) =>
      makeRelative(workspaceRoot, n.qualifiedName),
    ),
    impactedFiles: [
      ...new Set(impact.impactedNodes.map((n) => makeRelative(workspaceRoot, n.filePath))),
    ],
    nodes: impact.impactedNodes.map((n) => ({
      ...toNodeSummary(n),
      filePath: makeRelative(workspaceRoot, n.filePath),
    })),
  };
}

export function compareSnapshots(
  previous: BlastRadiusSnapshot,
  current: BlastRadiusSnapshot,
): SnapshotComparison {
  const currentByQualified = new Map(current.nodes.map((n) => [n.qualifiedName, n]));
  const currentByFallback = new Map(current.nodes.map((n) => [fallbackKey(n), n]));

  const matchedCurrentQualifiedNames = new Set<string>();
  const unchanged: NodeSummary[] = [];
  const removed: NodeSummary[] = [];

  for (const prevNode of previous.nodes) {
    const match =
      currentByQualified.get(prevNode.qualifiedName) ??
      currentByFallback.get(fallbackKey(prevNode));
    if (match) {
      unchanged.push(match);
      matchedCurrentQualifiedNames.add(match.qualifiedName);
    } else {
      removed.push(prevNode);
    }
  }

  const added: NodeSummary[] = [];
  for (const currentNode of current.nodes) {
    if (!matchedCurrentQualifiedNames.has(currentNode.qualifiedName)) {
      added.push(currentNode);
    }
  }

  const previousFiles = new Set(previous.impactedFiles);
  const currentFiles = new Set(current.impactedFiles);

  const addedFiles = current.impactedFiles.filter((f) => !previousFiles.has(f));
  const removedFiles = previous.impactedFiles.filter((f) => !currentFiles.has(f));
  const unchangedFiles = current.impactedFiles.filter((f) => previousFiles.has(f));

  return {
    added,
    removed,
    unchanged,
    addedFiles,
    removedFiles,
    unchangedFiles,
    previous: {
      impactedNodeCount: previous.impactedNodeQualifiedNames.length,
      impactedFileCount: previous.impactedFiles.length,
    },
    current: {
      impactedNodeCount: current.impactedNodeQualifiedNames.length,
      impactedFileCount: current.impactedFiles.length,
    },
    summary: {
      added: added.length,
      removed: removed.length,
      unchanged: unchanged.length,
      addedFiles: addedFiles.length,
      removedFiles: removedFiles.length,
    },
  };
}

// ---------------------------------------------------------------------------
// Snapshot persistence (node:fs/promises)
// ---------------------------------------------------------------------------

const SNAPSHOT_DIR = ".dagayn/snapshots";

function getSnapshotDir(workspaceRoot: string): string {
  return path.join(workspaceRoot, SNAPSHOT_DIR);
}

function sanitizeLabel(label: string): string {
  return label
    .trim()
    .replace(/\s+/g, "_")
    .replace(/[^a-zA-Z0-9_-]/g, "");
}

function formatTimestamp(date: Date): string {
  const pad = (n: number) => n.toString().padStart(2, "0");
  const yyyy = date.getFullYear();
  const mm = pad(date.getMonth() + 1);
  const dd = pad(date.getDate());
  const hh = pad(date.getHours());
  const mi = pad(date.getMinutes());
  const ss = pad(date.getSeconds());
  return `${yyyy}${mm}${dd}-${hh}${mi}${ss}`;
}

export interface SnapshotListItem {
  path: string;
  label: string;
  createdAt: string;
  impactedNodeCount: number;
}

export async function saveSnapshot(
  workspaceRoot: string,
  snapshot: BlastRadiusSnapshot,
): Promise<string> {
  const dir = getSnapshotDir(workspaceRoot);
  await fs.mkdir(dir, { recursive: true });

  const timestamp = formatTimestamp(new Date(snapshot.createdAt));
  const safeLabel = sanitizeLabel(snapshot.label) || "snapshot";
  const fileName = `${safeLabel}-${timestamp}.json`;
  const filePath = path.join(dir, fileName);

  await fs.writeFile(filePath, JSON.stringify(snapshot, null, 2), "utf-8");
  return filePath;
}

export async function listSnapshots(workspaceRoot: string): Promise<SnapshotListItem[]> {
  const dir = getSnapshotDir(workspaceRoot);
  const items: SnapshotListItem[] = [];

  let entries: string[] = [];
  try {
    entries = await fs.readdir(dir);
  } catch {
    return [];
  }

  for (const entry of entries) {
    if (!entry.endsWith(".json")) {
      continue;
    }
    const filePath = path.join(dir, entry);
    try {
      const raw = await fs.readFile(filePath, "utf-8");
      const snapshot = JSON.parse(raw) as BlastRadiusSnapshot;
      if (snapshot.schemaVersion !== 1) {
        continue;
      }
      items.push({
        path: filePath,
        label: snapshot.label || entry,
        createdAt: snapshot.createdAt,
        impactedNodeCount: snapshot.impactedNodeQualifiedNames.length,
      });
    } catch {
      // Skip unreadable or invalid snapshot files.
    }
  }

  return items.sort((a, b) => new Date(b.createdAt).getTime() - new Date(a.createdAt).getTime());
}

export async function loadSnapshot(filePath: string): Promise<BlastRadiusSnapshot> {
  const raw = await fs.readFile(filePath, "utf-8");
  const snapshot = JSON.parse(raw) as BlastRadiusSnapshot;
  if (snapshot.schemaVersion !== 1) {
    throw new Error(
      `Unsupported blast radius snapshot schema version: ${snapshot.schemaVersion ?? "missing"}`,
    );
  }
  return snapshot;
}

// ---------------------------------------------------------------------------
// VS Code command handlers
// ---------------------------------------------------------------------------

export interface BlastRadiusSnapshotProvider {
  setResults(changed: GraphNode[], impacted: GraphNode[]): void;
}

interface ResolvedImpact {
  reader: SqliteReader;
  workspaceRoot: string;
  impact: ImpactRadius;
  sourceFiles: string[];
  depth: number;
}

async function resolveCurrentImpact(
  registry: WorkspaceGraphRegistry,
): Promise<ResolvedImpact | undefined> {
  const reader = registry.getReaderForActiveEditor();
  if (!reader) {
    vscode.window.showWarningMessage("Code Graph: No graph database loaded.");
    return undefined;
  }

  const editor = vscode.window.activeTextEditor;
  if (!editor) {
    vscode.window.showWarningMessage("Open a file first");
    return undefined;
  }

  const workspaceFolder = vscode.workspace.getWorkspaceFolder(editor.document.uri);
  if (!workspaceFolder) {
    vscode.window.showWarningMessage("Code Graph: File is not inside the workspace.");
    return undefined;
  }

  const absFilePath = editor.document.uri.fsPath;
  const cursorLine = editor.selection.active.line + 1;

  const nodeAtCursor = reader.getNodeAtCursor(absFilePath, cursorLine);
  const filePath = nodeAtCursor ? nodeAtCursor.filePath : absFilePath;

  const config = vscode.workspace.getConfiguration("dagayn");
  const depth = config.get<number>("blastRadiusDepth", 2);

  const impact = reader.getImpactRadius([filePath], depth);
  const sourceFiles = [makeRelative(workspaceFolder.uri.fsPath, filePath)];

  return {
    reader,
    workspaceRoot: workspaceFolder.uri.fsPath,
    impact,
    sourceFiles,
    depth,
  };
}

export async function saveBlastRadiusSnapshot(
  registry: WorkspaceGraphRegistry,
  getProvider: () => BlastRadiusSnapshotProvider | undefined,
): Promise<string | undefined> {
  const resolved = await resolveCurrentImpact(registry);
  if (!resolved) {
    return undefined;
  }

  const label = await vscode.window.showInputBox({
    prompt: "Label for this blast radius snapshot",
    placeHolder: "e.g. before-refactor",
  });
  if (!label?.trim()) {
    return undefined;
  }

  const snapshot = buildSnapshot(
    resolved.workspaceRoot,
    label.trim(),
    resolved.sourceFiles,
    resolved.depth,
    null,
    resolved.impact,
  );

  let filePath: string;
  try {
    filePath = await saveSnapshot(resolved.workspaceRoot, snapshot);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    vscode.window.showErrorMessage(`Code Graph: Failed to save snapshot: ${message}`);
    return undefined;
  }

  getProvider()?.setResults(resolved.impact.changedNodes, resolved.impact.impactedNodes);

  vscode.window.showInformationMessage(
    `Code Graph: Saved blast radius snapshot to ${path.basename(filePath)}`,
  );
  return filePath;
}

interface SnapshotQuickPickItem extends vscode.QuickPickItem {
  snapshot: SnapshotListItem;
}

export async function compareBlastRadiusSnapshot(registry: WorkspaceGraphRegistry): Promise<void> {
  const resolved = await resolveCurrentImpact(registry);
  if (!resolved) {
    return;
  }

  const snapshots = await listSnapshots(resolved.workspaceRoot);
  if (snapshots.length === 0) {
    vscode.window.showInformationMessage("Code Graph: No blast radius snapshots found.");
    return;
  }

  const items: SnapshotQuickPickItem[] = snapshots.map((snapshot) => ({
    label: snapshot.label,
    detail: `${snapshot.impactedNodeCount} impacted nodes · ${new Date(snapshot.createdAt).toLocaleString()}`,
    snapshot,
  }));

  const picked = await vscode.window.showQuickPick(items, {
    placeHolder: "Select a blast radius snapshot to compare",
  });
  if (!picked) {
    return;
  }

  let previous: BlastRadiusSnapshot;
  try {
    previous = await loadSnapshot(picked.snapshot.path);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    vscode.window.showErrorMessage(`Code Graph: Failed to load snapshot: ${message}`);
    return;
  }

  const current = buildSnapshot(
    resolved.workspaceRoot,
    "current",
    resolved.sourceFiles,
    resolved.depth,
    null,
    resolved.impact,
  );

  const comparison = compareSnapshots(previous, current);
  renderComparison(comparison);
}

function renderComparison(comparison: SnapshotComparison): void {
  const channel = vscode.window.createOutputChannel("Code Graph Blast Radius Compare", {
    log: true,
  });

  channel.appendLine("# Blast Radius Snapshot Comparison");
  channel.appendLine("");
  channel.appendLine(
    `Previous: ${comparison.previous.impactedNodeCount} impacted nodes across ${comparison.previous.impactedFileCount} files`,
  );
  channel.appendLine(
    `Current:  ${comparison.current.impactedNodeCount} impacted nodes across ${comparison.current.impactedFileCount} files`,
  );
  channel.appendLine("");

  channel.appendLine(`## Newly impacted (${comparison.added.length})`);
  if (comparison.added.length === 0) {
    channel.appendLine("None");
  } else {
    for (const node of comparison.added) {
      channel.appendLine(
        `+ ${node.name} (${node.kind}) · ${node.filePath}:${node.lineStart ?? "-"}`,
      );
    }
  }
  channel.appendLine("");

  channel.appendLine(`## No longer impacted (${comparison.removed.length})`);
  if (comparison.removed.length === 0) {
    channel.appendLine("None");
  } else {
    for (const node of comparison.removed) {
      channel.appendLine(
        `- ${node.name} (${node.kind}) · ${node.filePath}:${node.lineStart ?? "-"}`,
      );
    }
  }
  channel.appendLine("");

  channel.appendLine(`## Unchanged impacted (${comparison.unchanged.length})`);
  if (comparison.unchanged.length === 0) {
    channel.appendLine("None");
  } else {
    for (const node of comparison.unchanged) {
      channel.appendLine(
        `  ${node.name} (${node.kind}) · ${node.filePath}:${node.lineStart ?? "-"}`,
      );
    }
  }
  channel.appendLine("");

  channel.appendLine(`## Added files (${comparison.addedFiles.length})`);
  if (comparison.addedFiles.length === 0) {
    channel.appendLine("None");
  } else {
    for (const file of comparison.addedFiles) {
      channel.appendLine(`+ ${file}`);
    }
  }
  channel.appendLine("");

  channel.appendLine(`## Removed files (${comparison.removedFiles.length})`);
  if (comparison.removedFiles.length === 0) {
    channel.appendLine("None");
  } else {
    for (const file of comparison.removedFiles) {
      channel.appendLine(`- ${file}`);
    }
  }
  channel.appendLine("");

  channel.appendLine("## Summary");
  channel.appendLine(`Added nodes: ${comparison.summary.added}`);
  channel.appendLine(`Removed nodes: ${comparison.summary.removed}`);
  channel.appendLine(`Unchanged nodes: ${comparison.summary.unchanged}`);
  channel.appendLine(`Added files: ${comparison.summary.addedFiles}`);
  channel.appendLine(`Removed files: ${comparison.summary.removedFiles}`);

  channel.show(true);
}

export function registerBlastRadiusSnapshotCommands(
  context: vscode.ExtensionContext,
  registry: WorkspaceGraphRegistry,
  getProvider: () => BlastRadiusSnapshotProvider | undefined,
): void {
  context.subscriptions.push(
    vscode.commands.registerCommand("dagayn.saveBlastRadiusSnapshot", async () => {
      await saveBlastRadiusSnapshot(registry, getProvider);
    }),
    vscode.commands.registerCommand("dagayn.compareBlastRadiusSnapshot", async () => {
      await compareBlastRadiusSnapshot(registry);
    }),
  );
}
