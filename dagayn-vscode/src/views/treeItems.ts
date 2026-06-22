import * as vscode from "vscode";
import * as path from "node:path";
import { getNodeDocumentation } from "../features/nodeDocs";

// ---------------------------------------------------------------------------
// WorkspaceFolderTreeItem – groups files when multiple folders have a graph
// ---------------------------------------------------------------------------

export class WorkspaceFolderTreeItem extends vscode.TreeItem {
  public readonly folderFsPath: string;

  constructor(folderFsPath: string, fileCount: number) {
    super(path.basename(folderFsPath), vscode.TreeItemCollapsibleState.Collapsed);

    this.folderFsPath = folderFsPath;
    this.description = `${fileCount} files`;
    this.iconPath = new vscode.ThemeIcon("folder");
    this.contextValue = "workspace-folder";
    this.tooltip = folderFsPath;
  }
}

// ---------------------------------------------------------------------------
// FileTreeItem – represents a source file in the code graph
// ---------------------------------------------------------------------------

export class FileTreeItem extends vscode.TreeItem {
  public readonly filePath: string;
  public readonly qualifiedName: string;
  public readonly folderFsPath: string;

  constructor(filePath: string, workspaceRoot: string) {
    const fileName = path.basename(filePath);
    super(fileName, vscode.TreeItemCollapsibleState.Collapsed);

    this.filePath = filePath;
    this.qualifiedName = filePath;
    this.folderFsPath = workspaceRoot;

    const relativePath = path.relative(workspaceRoot, filePath);
    this.description = relativePath !== fileName ? relativePath : "";
    this.iconPath = new vscode.ThemeIcon("file");
    this.contextValue = "node-file";
    this.tooltip = filePath;

    this.command = {
      title: "Open File",
      command: "vscode.open",
      arguments: [vscode.Uri.file(filePath)],
    };
  }
}

// ---------------------------------------------------------------------------
// SymbolTreeItem – represents a class, function, type, or test node
// ---------------------------------------------------------------------------

const KIND_ICON_MAP: Record<string, string> = {
  Function: "symbol-method",
  Class: "symbol-class",
  Type: "symbol-interface",
  Test: "testing-run-icon",
};

const KIND_CONTEXT_MAP: Record<string, string> = {
  Function: "node-function",
  Class: "node-class",
  Type: "node-type",
  Test: "node-test",
};

function formatSymbolLabel(name: string, kind: string): string {
  if (kind === "Function" || kind === "Test") {
    return `${name}()`;
  }
  return name;
}

function formatSymbolDescription(
  kind: string,
  lineStart: number | null,
  lineEnd: number | null,
): string {
  const kindLower = kind.toLowerCase();
  if (lineStart != null && lineEnd != null) {
    return `${kindLower} \u00b7 L${lineStart}\u2013${lineEnd}`;
  }
  if (lineStart != null) {
    return `${kindLower} \u00b7 L${lineStart}`;
  }
  return kindLower;
}

export class SymbolTreeItem extends vscode.TreeItem {
  public readonly qualifiedName: string;
  public readonly filePath: string;
  public readonly lineStart: number | null;
  public readonly kind: string;
  public readonly extra: Record<string, unknown>;

  constructor(
    qualifiedName: string,
    name: string,
    kind: string,
    filePath: string,
    lineStart: number | null,
    lineEnd: number | null,
    extra: Record<string, unknown> = {},
  ) {
    const label = formatSymbolLabel(name, kind);
    super(label, vscode.TreeItemCollapsibleState.Collapsed);

    this.qualifiedName = qualifiedName;
    this.filePath = filePath;
    this.lineStart = lineStart;
    this.kind = kind;

    this.description = formatSymbolDescription(kind, lineStart, lineEnd);
    this.iconPath = new vscode.ThemeIcon(KIND_ICON_MAP[kind] ?? "symbol-misc");
    this.contextValue = KIND_CONTEXT_MAP[kind] ?? "node-function";

    const docs = getNodeDocumentation({ extra });
    if (docs.length > 0) {
      const tooltip = new vscode.MarkdownString();
      tooltip.appendMarkdown(`**${qualifiedName}**\n\n---\n\n${docs}`);
      this.tooltip = tooltip;
    } else {
      this.tooltip = qualifiedName;
    }
    this.extra = extra;

    const line = lineStart != null ? lineStart - 1 : 0;
    this.command = {
      title: "Go to Symbol",
      command: "vscode.open",
      arguments: [
        vscode.Uri.file(filePath),
        { selection: new vscode.Range(line, 0, line, 0) } as vscode.TextDocumentShowOptions,
      ],
    };
  }
}

// ---------------------------------------------------------------------------
// EdgeTreeItem – represents a relationship edge (leaf node)
// ---------------------------------------------------------------------------

const EDGE_DIRECTION_INFO: Record<
  string,
  { outgoingLabel: string; incomingLabel: string; outgoingIcon: string; incomingIcon: string }
> = {
  CALLS: {
    outgoingLabel: "calls",
    incomingLabel: "called by",
    outgoingIcon: "arrow-right",
    incomingIcon: "arrow-left",
  },
  IMPORTS_FROM: {
    outgoingLabel: "imports",
    incomingLabel: "imported by",
    outgoingIcon: "package",
    incomingIcon: "package",
  },
  INHERITS: {
    outgoingLabel: "inherits from",
    incomingLabel: "inherited by",
    outgoingIcon: "type-hierarchy",
    incomingIcon: "type-hierarchy",
  },
  IMPLEMENTS: {
    outgoingLabel: "implements",
    incomingLabel: "implemented by",
    outgoingIcon: "symbol-interface",
    incomingIcon: "symbol-interface",
  },
  TESTED_BY: {
    outgoingLabel: "tested by",
    incomingLabel: "tests",
    outgoingIcon: "testing-run-icon",
    incomingIcon: "testing-run-icon",
  },
  CONTAINS: {
    outgoingLabel: "contains",
    incomingLabel: "contained in",
    outgoingIcon: "symbol-namespace",
    incomingIcon: "symbol-namespace",
  },
  DEPENDS_ON: {
    outgoingLabel: "depends on",
    incomingLabel: "depended on by",
    outgoingIcon: "references",
    incomingIcon: "references",
  },
};

function extractShortName(qualifiedName: string): string {
  // Qualified names are like "/path/to/file.py::ClassName.method" or "/path/to/file.py"
  const colonIdx = qualifiedName.lastIndexOf("::");
  if (colonIdx >= 0) {
    return qualifiedName.substring(colonIdx + 2);
  }
  return path.basename(qualifiedName);
}

export class EdgeTreeItem extends vscode.TreeItem {
  public readonly targetQualifiedName: string;
  public readonly targetFilePath: string;
  public readonly targetLine: number;

  constructor(
    edgeKind: string,
    direction: "outgoing" | "incoming",
    targetQualifiedName: string,
    targetFilePath: string,
    targetLine: number,
  ) {
    const shortName = extractShortName(targetQualifiedName);
    const info = EDGE_DIRECTION_INFO[edgeKind];
    const verb = info
      ? direction === "outgoing"
        ? info.outgoingLabel
        : info.incomingLabel
      : edgeKind.toLowerCase();
    const icon = info
      ? direction === "outgoing"
        ? info.outgoingIcon
        : info.incomingIcon
      : "arrow-right";
    const arrow = direction === "outgoing" ? "\u2192" : "\u2190";
    const label = `${arrow} ${verb} ${shortName}`;

    super(label, vscode.TreeItemCollapsibleState.None);

    this.targetQualifiedName = targetQualifiedName;
    this.targetFilePath = targetFilePath;
    this.targetLine = targetLine;

    this.iconPath = new vscode.ThemeIcon(icon);
    this.contextValue = "edge";
    this.tooltip = `${arrow} ${verb} ${targetQualifiedName}`;

    const line = targetLine > 0 ? targetLine - 1 : 0;
    this.command = {
      title: "Go to Target",
      command: "vscode.open",
      arguments: [
        vscode.Uri.file(targetFilePath),
        { selection: new vscode.Range(line, 0, line, 0) } as vscode.TextDocumentShowOptions,
      ],
    };
  }
}

// ---------------------------------------------------------------------------
// BlastRadiusGroupItem – groups "Changed" and "Impacted" results
// ---------------------------------------------------------------------------

export class BlastRadiusGroupItem extends vscode.TreeItem {
  public readonly groupKind: "changed" | "impacted";

  constructor(groupKind: "changed" | "impacted", count: number) {
    const label = groupKind === "changed" ? `Changed (${count})` : `Impacted (${count})`;
    super(label, vscode.TreeItemCollapsibleState.Expanded);

    this.groupKind = groupKind;
    this.iconPath = new vscode.ThemeIcon(groupKind === "changed" ? "flame" : "broadcast");
    this.contextValue = `blast-radius-${groupKind}`;
    this.tooltip =
      groupKind === "changed"
        ? `${count} directly changed node(s)`
        : `${count} transitively impacted node(s)`;
  }
}

// ---------------------------------------------------------------------------
// StatsItem – displays a single statistic line (leaf node)
// ---------------------------------------------------------------------------

export class StatsItem extends vscode.TreeItem {
  constructor(label: string, value: string) {
    super(label, vscode.TreeItemCollapsibleState.None);
    this.description = value;
    this.contextValue = "stat";
    this.tooltip = `${label}: ${value}`;
  }
}
