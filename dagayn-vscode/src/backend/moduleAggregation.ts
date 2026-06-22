/**
 * Pure module/package-level aggregation over a dagayn graph database.
 *
 * Reads nodes/edges from a SqliteReader and produces directory-level nodes
 * and edges. This module has no VS Code dependency and is fully unit-testable.
 */

import * as path from "node:path";
import type { EdgeKind, SqliteReader } from "./sqlite";

export interface ModuleNode {
  id: number;
  name: string;
  dirPath: string;
  fileCount: number;
  language: string | null;
}

export interface ModuleEdge {
  kind: EdgeKind;
  sourceDir: string;
  targetDir: string;
  count: number;
}

export interface ModuleGraph {
  nodes: ModuleNode[];
  edges: ModuleEdge[];
}

export const DEFAULT_MODULE_EDGE_KINDS: EdgeKind[] = ["CALLS", "IMPORTS_FROM", "DEPENDS_ON"];

/**
 * Aggregate files by parent directory and derive directory-to-directory edges.
 *
 * @param reader      Open SqliteReader.
 * @param edgeKinds   Edge kinds to include. Defaults to CALLS, IMPORTS_FROM, DEPENDS_ON.
 */
export function aggregateModules(
  reader: SqliteReader,
  edgeKinds: EdgeKind[] = DEFAULT_MODULE_EDGE_KINDS,
): ModuleGraph {
  const files = reader.getAllFiles();

  // Map directory -> file count and language (use first file node's language).
  const dirToFileCount = new Map<string, number>();
  const dirToLanguage = new Map<string, string | null>();

  // Map qualified name -> parent directory for every node we encounter.
  const qnToDir = new Map<string, string>();

  for (const file of files) {
    const dir = path.dirname(file);
    dirToFileCount.set(dir, (dirToFileCount.get(dir) ?? 0) + 1);

    const nodes = reader.getNodesByFile(file);

    // Language comes from the File node itself, if present.
    const fileNode = nodes.find((n) => n.kind === "File");
    if (!dirToLanguage.has(dir)) {
      dirToLanguage.set(dir, fileNode?.language ?? null);
    }

    for (const node of nodes) {
      qnToDir.set(node.qualifiedName, path.dirname(node.filePath));
    }

    // Ensure File nodes whose qualifiedName is the file path map to the dir.
    qnToDir.set(file, dir);
  }

  const edgeKindSet = new Set(edgeKinds);
  const moduleEdges = new Map<string, ModuleEdge>();
  const processedPairs = new Set<string>();

  for (const file of files) {
    const nodes = reader.getNodesByFile(file);
    for (const node of nodes) {
      const sourceQn = node.qualifiedName;
      const edges = reader.getEdgesBySource(sourceQn);
      for (const edge of edges) {
        if (!edgeKindSet.has(edge.kind)) {
          continue;
        }

        // Avoid processing the same source/target/kind triplet twice.
        const pairKey = `${edge.kind}|${sourceQn}|${edge.targetQualified}`;
        if (processedPairs.has(pairKey)) {
          continue;
        }
        processedPairs.add(pairKey);

        const sourceDir = qnToDir.get(sourceQn);
        const targetDir = qnToDir.get(edge.targetQualified);
        if (!sourceDir || !targetDir) {
          continue;
        }
        if (sourceDir === targetDir) {
          continue;
        }

        const moduleKey = `${edge.kind}|${sourceDir}|${targetDir}`;
        const existing = moduleEdges.get(moduleKey);
        if (existing) {
          existing.count += 1;
        } else {
          moduleEdges.set(moduleKey, {
            kind: edge.kind,
            sourceDir,
            targetDir,
            count: 1,
          });
        }
      }
    }
  }

  // Assign sequential ids to directories sorted alphabetically.
  const sortedDirs = [...dirToFileCount.keys()].sort();
  const dirToId = new Map<string, number>();
  for (let i = 0; i < sortedDirs.length; i++) {
    dirToId.set(sortedDirs[i], i + 1);
  }

  const nodes: ModuleNode[] = sortedDirs.map((dir) => ({
    id: dirToId.get(dir) ?? 0,
    name: path.basename(dir) || dir,
    dirPath: dir,
    fileCount: dirToFileCount.get(dir) ?? 0,
    language: dirToLanguage.get(dir) ?? null,
  }));

  const edges: ModuleEdge[] = [...moduleEdges.values()].sort((a, b) => {
    if (a.kind !== b.kind) return a.kind.localeCompare(b.kind);
    if (a.sourceDir !== b.sourceDir) return a.sourceDir.localeCompare(b.sourceDir);
    return a.targetDir.localeCompare(b.targetDir);
  });

  return { nodes, edges };
}
