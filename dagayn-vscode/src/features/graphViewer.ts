import * as vscode from "vscode";
import { WorkspaceGraphRegistry } from "../backend/registry";
import { resolveReaderAndFolder } from "../backend/readerResolver";
import { GraphWebviewPanel } from "../views/graphWebview";

export function registerGraphViewerCommands(
  context: vscode.ExtensionContext,
  registry: WorkspaceGraphRegistry,
): void {
  context.subscriptions.push(
    vscode.commands.registerCommand("dagayn.showGraph", async () => {
      const resolved = await resolveReaderAndFolder(registry);
      if (!resolved) {
        vscode.window.showWarningMessage(
          "Code Graph: No graph database loaded. Run 'Code Graph: Build Graph' first.",
        );
        return;
      }
      const { reader, folder } = resolved;
      GraphWebviewPanel.createOrShow(context.extensionUri, reader, folder);
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand("dagayn.revealInTree", (_qualifiedName: string) => {
      GraphWebviewPanel.highlightNode(_qualifiedName);
    }),
  );
}
