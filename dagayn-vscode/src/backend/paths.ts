import * as fs from "node:fs";
import * as path from "node:path";

/**
 * Locate a dagayn graph database inside a workspace folder.
 *
 * Checks for `.dagayn/graph.db` first, then falls back to the legacy
 * `.dagayn.db` at the folder root. Returns `undefined` when neither file
 * exists.
 */
export function findGraphDb(folderFsPath: string): string | undefined {
  const primary = path.join(folderFsPath, ".dagayn", "graph.db");
  if (fs.existsSync(primary)) {
    return primary;
  }
  const fallback = path.join(folderFsPath, ".dagayn.db");
  if (fs.existsSync(fallback)) {
    return fallback;
  }
  return undefined;
}
