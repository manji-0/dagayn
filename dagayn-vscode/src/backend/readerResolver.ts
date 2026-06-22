import * as vscode from "vscode";
import type { SqliteReader } from "./sqlite";
import type { WorkspaceGraphRegistry } from "./registry";
import { pickFolderForGlobalOp } from "./folderPicker";

export interface ReaderFolderResolution {
  reader: SqliteReader;
  folder: string;
}

/**
 * Resolve a reader+folder pair for commands that display a graph view.
 * Prefers the active editor's folder when it already has a reader; otherwise
 * auto-selects a single graph-enabled folder, or prompts when ambiguous.
 * Returns undefined when no reader/folder can be resolved; the caller owns
 * the user-facing warning message.
 */
export async function resolveReaderAndFolder(
  registry: WorkspaceGraphRegistry,
): Promise<ReaderFolderResolution | undefined> {
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
      folder = folders[0]!;
      reader = registry.getReaderForFolder(folder);
    } else if (folders.length > 1) {
      folder = await pickFolderForGlobalOp(registry, { requireGraph: true });
      reader = folder ? registry.getReaderForFolder(folder) : undefined;
    }
  }

  if (!reader || !folder) {
    return undefined;
  }
  return { reader, folder };
}
