import * as vscode from "vscode";
import { Installer } from "./installer";

/**
 * Register command handlers for the walkthrough steps defined in
 * `package.json` contributes.walkthroughs.
 *
 * Each walkthrough step button triggers one of these commands so the user
 * can install the CLI, build the graph, and explore the sidebar without
 * leaving the walkthrough.
 */
export function registerWalkthroughCommands(
  context: vscode.ExtensionContext,
  installer: Installer,
): void {
  context.subscriptions.push(
    vscode.commands.registerCommand("dagayn.walkthrough.install", async () => {
      await installer.autoInstall();
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand("dagayn.walkthrough.build", async () => {
      const workspaceFolders = vscode.workspace.workspaceFolders;
      if (!workspaceFolders || workspaceFolders.length === 0) {
        vscode.window.showWarningMessage("No workspace folder is open. Open a folder first.");
        return;
      }

      await vscode.window.withProgress(
        {
          location: vscode.ProgressLocation.Notification,
          title: "Code Graph: Building graph...",
          cancellable: false,
        },
        () => vscode.commands.executeCommand("dagayn.buildGraph"),
      );

      vscode.window.showInformationMessage("Code Graph: Build complete.");
    }),
  );

  context.subscriptions.push(
    vscode.commands.registerCommand("dagayn.walkthrough.explore", async () => {
      await vscode.commands.executeCommand("dagayn.codeGraph.focus");
    }),
  );
}

/**
 * Show a welcome notification if no graph database has been built yet
 * in any of the open workspace folders.
 *
 * Checks for `.dagayn/graph.db` in every workspace folder.
 * When none is found, a notification is shown with a button that opens
 * the built-in walkthrough.
 */
export async function showWelcomeIfNeeded(_context: vscode.ExtensionContext): Promise<void> {
  const workspaceFolders = vscode.workspace.workspaceFolders;
  if (!workspaceFolders || workspaceFolders.length === 0) {
    return;
  }

  for (const folder of workspaceFolders) {
    const dbUri = vscode.Uri.joinPath(folder.uri, ".dagayn", "graph.db");

    try {
      await vscode.workspace.fs.stat(dbUri);
      // Database exists in at least one folder -- no need to prompt.
      return;
    } catch {
      // File does not exist in this folder -- continue checking.
    }
  }

  // No graph.db found in any workspace folder.
  const selection = await vscode.window.showInformationMessage(
    "Welcome to Dagayn! Get started by building your code graph.",
    "Get Started",
  );

  if (selection === "Get Started") {
    await vscode.commands.executeCommand(
      "workbench.action.openWalkthrough",
      "tirth8205.dagayn#dagayn.welcome",
    );
  }
}
