import * as fs from "node:fs";
import * as path from "node:path";

/**
 * Resolve a node file path against the workspace folders.
 *
 * If the path is already absolute and exists, it is returned as-is.
 * Otherwise the path is joined to each workspace folder (longest match
 * first, to support nested roots) and the first existing candidate wins.
 */
export function resolveNodeFilePath(
  filePath: string,
  workspaceFolders: string[],
): { candidate: string | undefined; tried: string[] } {
  const tried: string[] = [];

  if (path.isAbsolute(filePath)) {
    tried.push(filePath);
    if (fs.existsSync(filePath)) {
      return { candidate: filePath, tried };
    }
  }

  const sorted = [...workspaceFolders].sort((a, b) => b.length - a.length);
  for (const folder of sorted) {
    const candidate = path.join(folder, filePath);
    tried.push(candidate);
    if (fs.existsSync(candidate)) {
      return { candidate, tried };
    }
  }

  return { candidate: undefined, tried };
}
