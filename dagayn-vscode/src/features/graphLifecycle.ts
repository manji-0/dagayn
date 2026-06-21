import * as vscode from "vscode";
import { CliWrapper } from "../backend/cli";

export function registerGraphLifecycleCommands(
  context: vscode.ExtensionContext,
  cli: CliWrapper,
  getWorkspaceRoot: () => string | undefined,
  reinitialize: (context: vscode.ExtensionContext) => Promise<void>,
): void {
  context.subscriptions.push(
    vscode.commands.registerCommand("dagayn.buildGraph", async () => {
      const workspaceRoot = getWorkspaceRoot();
      if (!workspaceRoot) {
        vscode.window.showErrorMessage("No workspace folder is open.");
        return;
      }
      const result = await cli.buildGraph(workspaceRoot);
      if (result.success) {
        await reinitialize(context);
        vscode.window.showInformationMessage("Code Graph: Build complete.");
      } else {
        vscode.window.showErrorMessage(`Code Graph: Build failed. ${result.stderr}`);
      }
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand("dagayn.updateGraph", async () => {
      const workspaceRoot = getWorkspaceRoot();
      if (!workspaceRoot) {
        vscode.window.showErrorMessage("No workspace folder is open.");
        return;
      }
      await vscode.window.withProgress(
        {
          location: vscode.ProgressLocation.Notification,
          title: "Code Graph: Updating graph...",
          cancellable: false,
        },
        async () => {
          const result = await cli.updateGraph(workspaceRoot);
          if (!result.success) {
            vscode.window.showErrorMessage(`Code Graph: Update failed. ${result.stderr}`);
          }
        },
      );
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand("dagayn.embedGraph", async () => {
      const workspaceRoot = getWorkspaceRoot();
      if (!workspaceRoot) {
        vscode.window.showErrorMessage("No workspace folder is open.");
        return;
      }
      const result = await cli.embedGraph(workspaceRoot);
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
      const workspaceRoot = getWorkspaceRoot();
      if (!workspaceRoot) {
        vscode.window.showErrorMessage("No workspace folder is open.");
        return;
      }
      vscode.window.showInformationMessage("Code Graph: Watch mode started.");
      const result = await cli.watchGraph(workspaceRoot);
      if (!result.success) {
        vscode.window.showErrorMessage(`Code Graph: Watch failed. ${result.stderr}`);
      }
    }),
  );
}
