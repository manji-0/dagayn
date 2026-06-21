import * as vscode from "vscode";
import * as path from "node:path";
import { SqliteReader, GraphNode } from "../backend/sqlite";
import { navigateToNode } from "./cursorResolver";

const KIND_ICON: Record<string, string> = {
  Function: "$(symbol-method)",
  Class: "$(symbol-class)",
  File: "$(file)",
  Test: "$(beaker)",
  Type: "$(symbol-interface)",
};

function nodeToQuickPickItem(
  node: GraphNode,
  workspaceRoot: string | undefined,
): vscode.QuickPickItem & { node: GraphNode } {
  const icon = KIND_ICON[node.kind] ?? "$(symbol-misc)";
  const relativePath = workspaceRoot ? path.relative(workspaceRoot, node.filePath) : node.filePath;
  const lineInfo = node.lineStart != null ? `:${node.lineStart}` : "";

  return {
    label: `${icon} ${node.name}`,
    description: node.kind,
    detail: `${relativePath}${lineInfo}`,
    node,
  };
}

export function registerSearchCommand(
  context: vscode.ExtensionContext,
  getReader: () => SqliteReader | undefined,
): void {
  const disposable = vscode.commands.registerCommand("dagayn.search", async () => {
    const reader = getReader();
    if (!reader) {
      vscode.window.showWarningMessage("Code Graph: No graph database loaded.");
      return;
    }

    const workspaceRoot = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;

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
        const currentReader = getReader();
        if (!currentReader) {
          return;
        }
        const results = currentReader.searchNodes(value, 20);
        quickPick.items = results.map((node) => nodeToQuickPickItem(node, workspaceRoot));
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
