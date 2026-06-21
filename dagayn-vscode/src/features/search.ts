import * as vscode from "vscode";
import * as path from "node:path";
import { GraphNode } from "../backend/sqlite";
import { WorkspaceGraphRegistry } from "../backend/registry";
import { navigateToNode } from "./cursorResolver";

const KIND_ICON: Record<string, string> = {
  Function: "$(symbol-method)",
  Class: "$(symbol-class)",
  File: "$(file)",
  Test: "$(beaker)",
  Type: "$(symbol-interface)",
};

function nodeToQuickPickItem(node: GraphNode): vscode.QuickPickItem & { node: GraphNode } {
  const icon = KIND_ICON[node.kind] ?? "$(symbol-misc)";
  const workspaceRoot = vscode.workspace.getWorkspaceFolder(vscode.Uri.file(node.filePath))?.uri
    .fsPath;
  const relativePath = workspaceRoot ? path.relative(workspaceRoot, node.filePath) : node.filePath;
  const lineInfo = node.lineStart != null ? `:${node.lineStart}` : "";

  return {
    label: `${icon} ${node.name}`,
    description: node.kind,
    detail: `${relativePath}${lineInfo}`,
    node,
  };
}

function searchAllFolders(
  registry: WorkspaceGraphRegistry,
  query: string,
  limit: number,
): GraphNode[] {
  const all: GraphNode[] = [];
  for (const { reader } of registry.getAllReaders()) {
    all.push(...reader.searchNodes(query, limit));
  }
  return all.slice(0, limit);
}

export function registerSearchCommand(
  context: vscode.ExtensionContext,
  registry: WorkspaceGraphRegistry,
): void {
  const disposable = vscode.commands.registerCommand("dagayn.search", async () => {
    if (registry.getAllReaders().length === 0) {
      vscode.window.showWarningMessage("Code Graph: No graph database loaded.");
      return;
    }

    const quickPick = vscode.window.createQuickPick<vscode.QuickPickItem & { node: GraphNode }>();
    quickPick.placeholder = "Search for functions, classes, files, types...";
    quickPick.matchOnDescription = true;
    quickPick.matchOnDetail = true;

    let debounceTimer: ReturnType<typeof setTimeout> | undefined;

    quickPick.onDidChangeValue((value) => {
      if (debounceTimer) {
        clearTimeout(debounceTimer);
      }
      if (!value) {
        quickPick.items = [];
        return;
      }

      debounceTimer = setTimeout(() => {
        const results = searchAllFolders(registry, value, 20);
        quickPick.items = results.map((node) => nodeToQuickPickItem(node));
      }, 100);
    });

    quickPick.onDidAccept(async () => {
      const selected = quickPick.selectedItems[0];
      quickPick.dispose();
      if (selected?.node) {
        await navigateToNode(selected.node);
      }
    });

    quickPick.onDidHide(() => {
      if (debounceTimer) {
        clearTimeout(debounceTimer);
      }
      quickPick.dispose();
    });

    quickPick.show();
  });

  context.subscriptions.push(disposable);
}
