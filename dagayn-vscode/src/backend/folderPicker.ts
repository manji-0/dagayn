import * as vscode from "vscode";
import * as path from "node:path";
import { WorkspaceGraphRegistry } from "./registry";

interface FolderQuickPickItem extends vscode.QuickPickItem {
  folderFsPath: string;
}

function workspaceFolderCandidates(
  registry: WorkspaceGraphRegistry,
  options: { requireGraph?: boolean },
): string[] {
  if (options.requireGraph) {
    return registry.foldersWithGraph();
  }
  const folders = vscode.workspace.workspaceFolders ?? [];
  return folders.map((f) => f.uri.fsPath);
}

function describeCandidates(folders: string[]): FolderQuickPickItem[] {
  return folders.map((folderFsPath) => ({
    label: path.basename(folderFsPath),
    detail: folderFsPath,
    folderFsPath,
  }));
}

/**
 * Pick a workspace folder for an operation that requires an existing graph.
 *
 * Returns a folder without showing the picker when only one candidate exists
 * or the active editor's folder is a candidate. Falls back to a QuickPick
 * when ambiguous.
 */
export async function pickFolderForGlobalOp(
  registry: WorkspaceGraphRegistry,
  options: { requireGraph?: boolean } = {},
): Promise<string | undefined> {
  const candidates = workspaceFolderCandidates(registry, options);

  if (candidates.length === 0) {
    return undefined;
  }
  if (candidates.length === 1) {
    return candidates[0];
  }

  const activeEditor = vscode.window.activeTextEditor;
  if (activeEditor) {
    const activeFolder = vscode.workspace.getWorkspaceFolder(activeEditor.document.uri);
    if (activeFolder && candidates.includes(activeFolder.uri.fsPath)) {
      return activeFolder.uri.fsPath;
    }
  }

  const pick = await vscode.window.showQuickPick(describeCandidates(candidates), {
    placeHolder: "Select a workspace folder",
  });
  return pick?.folderFsPath;
}

/**
 * Pick a workspace folder for a build operation. A graph DB does not need to
 * exist yet, so all workspace folders are candidates.
 */
export async function pickFolderForBuild(
  registry: WorkspaceGraphRegistry,
): Promise<string | undefined> {
  return pickFolderForGlobalOp(registry, { requireGraph: false });
}
