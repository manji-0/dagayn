import * as vscode from "vscode";
import { CliWrapper } from "../backend/cli";
import { WorkspaceGraphRegistry } from "../backend/registry";
import { pickFolderForGlobalOp, pickFolderForBuild } from "../backend/folderPicker";

export function registerGraphLifecycleCommands(
  context: vscode.ExtensionContext,
  cli: CliWrapper,
  registry: WorkspaceGraphRegistry,
  reinitializeFolder: (context: vscode.ExtensionContext, folderFsPath: string) => Promise<void>,
): void {
  context.subscriptions.push(
    vscode.commands.registerCommand("dagayn.buildGraph", async () => {
      const folder = await pickFolderForBuild(registry);
      if (!folder) {
        vscode.window.showErrorMessage("No workspace folder is open.");
        return;
      }
      const result = await cli.buildGraph(folder);
      if (result.success) {
        await reinitializeFolder(context, folder);
        vscode.window.showInformationMessage("Code Graph: Build complete.");
      } else {
        vscode.window.showErrorMessage(`Code Graph: Build failed. ${result.stderr}`);
      }
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand("dagayn.updateGraph", async () => {
      const folder = await pickFolderForGlobalOp(registry, { requireGraph: true });
      if (!folder) {
        vscode.window.showErrorMessage("No workspace folder with a graph is open.");
        return;
      }
      await vscode.window.withProgress(
        {
          location: vscode.ProgressLocation.Notification,
          title: "Code Graph: Updating graph...",
          cancellable: false,
        },
        async () => {
          const result = await cli.updateGraph(folder);
          if (result.success) {
            await reinitializeFolder(context, folder);
          } else {
            vscode.window.showErrorMessage(`Code Graph: Update failed. ${result.stderr}`);
          }
        },
      );
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand("dagayn.embedGraph", async () => {
      const folder = await pickFolderForGlobalOp(registry, { requireGraph: true });
      if (!folder) {
        vscode.window.showErrorMessage("No workspace folder with a graph is open.");
        return;
      }
      const result = await cli.embedGraph(folder);
      if (result.success) {
        vscode.window.showInformationMessage("Code Graph: Embeddings computed.");
      } else {
        const msg =
          result.errorKind === "enoent"
            ? "Install embeddings support: pip install dagayn[embeddings]"
            : `Embedding failed: ${result.stderr}`;
        vscode.window.showErrorMessage(`Code Graph: ${msg}`);
      }
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand("dagayn.watchGraph", async () => {
      const folder = await pickFolderForBuild(registry);
      if (!folder) {
        vscode.window.showErrorMessage("No workspace folder is open.");
        return;
      }
      vscode.window.showInformationMessage("Code Graph: Watch mode started.");
      const result = await cli.watchGraph(folder);
      if (!result.success) {
        vscode.window.showErrorMessage(`Code Graph: Watch failed. ${result.stderr}`);
      }
    }),
  );
}
