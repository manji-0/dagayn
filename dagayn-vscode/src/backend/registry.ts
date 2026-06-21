import * as vscode from "vscode";
import { SqliteReader } from "./sqlite";
import { findGraphDb } from "./paths";

export interface WorkspaceReaderEntry {
  folderFsPath: string;
  reader: SqliteReader;
}

/**
 * Owns one `SqliteReader` per workspace folder that has a graph database.
 *
 * Provides resolution helpers for cursor-bound operations, global operations,
 * and tree view rendering. Per-folder open errors are caught so one bad DB
 * does not block readers for other folders.
 */
export class WorkspaceGraphRegistry implements vscode.Disposable {
  private readonly readers = new Map<string, SqliteReader>();

  constructor() {
    const folders = vscode.workspace.workspaceFolders ?? [];
    for (const folder of folders) {
      const dbPath = findGraphDb(folder.uri.fsPath);
      if (dbPath) {
        try {
          this.readers.set(folder.uri.fsPath, new SqliteReader(dbPath));
        } catch (err) {
          console.error(`[dagayn] Failed to open graph database for ${folder.uri.fsPath}:`, err);
        }
      }
    }
  }

  /** Reader for a specific workspace folder, if one is open. */
  getReaderForFolder(folderFsPath: string): SqliteReader | undefined {
    return this.readers.get(folderFsPath);
  }

  /** Reader that contains the given URI, based on VS Code's workspace folder mapping. */
  getReaderForUri(uri: vscode.Uri): SqliteReader | undefined {
    const folder = vscode.workspace.getWorkspaceFolder(uri);
    if (!folder) {
      return undefined;
    }
    return this.readers.get(folder.uri.fsPath);
  }

  /** Reader for the currently active editor's workspace folder. */
  getReaderForActiveEditor(): SqliteReader | undefined {
    const editor = vscode.window.activeTextEditor;
    if (!editor) {
      return undefined;
    }
    return this.getReaderForUri(editor.document.uri);
  }

  /** All currently open folder/reader pairs. */
  getAllReaders(): ReadonlyArray<WorkspaceReaderEntry> {
    return [...this.readers.entries()].map(([folderFsPath, reader]) => ({
      folderFsPath,
      reader,
    }));
  }

  /** Folders that currently have an open reader. */
  foldersWithGraph(): string[] {
    return [...this.readers.keys()];
  }

  /** Close and reopen a folder's reader. Returns `undefined` if no DB exists. */
  async reinitializeFolder(folderFsPath: string): Promise<SqliteReader | undefined> {
    this.closeFolder(folderFsPath);
    const dbPath = findGraphDb(folderFsPath);
    if (!dbPath) {
      return undefined;
    }
    const reader = await SqliteReader.create(dbPath);
    this.readers.set(folderFsPath, reader);
    return reader;
  }

  /** Close and remove a folder's reader. */
  closeFolder(folderFsPath: string): void {
    const reader = this.readers.get(folderFsPath);
    if (reader) {
      reader.close();
      this.readers.delete(folderFsPath);
    }
  }

  /** Close all readers. */
  dispose(): void {
    for (const reader of this.readers.values()) {
      reader.close();
    }
    this.readers.clear();
  }
}
