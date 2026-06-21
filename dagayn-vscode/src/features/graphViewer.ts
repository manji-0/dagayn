import * as vscode from "vscode";
import { SqliteReader } from "../backend/sqlite";
import { WorkspaceGraphRegistry } from "../backend/registry";
import { pickFolderForGlobalOp } from "../backend/folderPicker";
import { GraphWebviewPanel } from "../views/graphWebview";

export function registerGraphViewerCommands(
  context: vscode.ExtensionContext,
  registry: WorkspaceGraphRegistry,
): void {
  context.subscriptions.push(
    vscode.commands.registerCommand("dagayn.showGraph", async () => {
      let reader: SqliteReader | undefined = registry.getReaderForActiveEditor();
      let folder: string | undefined;

      if (reader) {
        const activeEditor = vscode.window.activeTextEditor;
        folder = activeEditor
          ? vscode.workspace.getWorkspaceFolder(activeEditor.document.uri)?.uri.fsPath
          : undefined;
      } else {
        const folders = registry.foldersWithGraph();
        if (folders.length === 1) {
          folder = folders[0];
          reader = registry.getReaderForFolder(folder);
        } else if (folders.length > 1) {
          folder = await pickFolderForGlobalOp(registry, { requireGraph: true });
          reader = folder ? registry.getReaderForFolder(folder) : undefined;
        }
      }

      if (!reader || !folder) {
        vscode.window.showWarningMessage(
          "Code Graph: No graph database loaded. Run 'Code Graph: Build Graph' first.",
        );
        return;
      }
      GraphWebviewPanel.createOrShow(context.extensionUri, reader, folder);
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand("dagayn.revealInTree", (_qualifiedName: string) => {
      GraphWebviewPanel.highlightNode(_qualifiedName);
    }),
  );
}
