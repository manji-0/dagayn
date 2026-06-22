import * as vscode from "vscode";
import { SqliteReader, GraphNode, GraphEdge } from "../backend/sqlite";
import { resolveNodeAtCursor, navigateToNode } from "./cursorResolver";
import { deleteSavedQuery, loadSavedQueries, saveSavedQuery, SavedQuery } from "./savedQueries";

export type QueryDirection = "incoming" | "outgoing";
export type QueryDef = { edgeKind: string; direction: QueryDirection };

export const QUERY_PATTERNS: Array<{ label: string; description: string }> = [
  { label: "callers_of", description: "Find functions calling the target" },
  { label: "callees_of", description: "Find functions called by the target" },
  { label: "imports_of", description: "Find modules imported by a file" },
  { label: "importers_of", description: "Find files importing from the target" },
  { label: "children_of", description: "Find nodes contained in a file or class" },
  { label: "tests_for", description: "Find tests for a function or class" },
  { label: "inheritors_of", description: "Find classes inheriting/implementing the target" },
  { label: "file_summary", description: "List all nodes in a file" },
];

export const QUERY_MAP: Record<string, QueryDef> = {
  callers_of: { edgeKind: "CALLS", direction: "incoming" },
  callees_of: { edgeKind: "CALLS", direction: "outgoing" },
  imports_of: { edgeKind: "IMPORTS_FROM", direction: "outgoing" },
  importers_of: { edgeKind: "IMPORTS_FROM", direction: "incoming" },
  children_of: { edgeKind: "CONTAINS", direction: "outgoing" },
  tests_for: { edgeKind: "TESTED_BY", direction: "incoming" },
  inheritors_of: { edgeKind: "INHERITS", direction: "incoming" },
  file_summary: { edgeKind: "CONTAINS", direction: "outgoing" },
};

export type NavigationItem = {
  label: string;
  description: string;
  detail: string;
  node: GraphNode | undefined;
};

export type TargetResolution = {
  node: GraphNode | undefined;
  multiple?: GraphNode[];
};

const VALID_PATTERNS = Object.keys(QUERY_MAP);

export function resolveTarget(reader: SqliteReader, target: string): TargetResolution {
  const node = reader.getNode(target);
  if (node) {
    return { node };
  }

  const matches = reader.searchNodes(target, 5);
  if (matches.length === 1) {
    return { node: matches[0] };
  }
  if (matches.length > 1) {
    return { node: undefined, multiple: matches };
  }

  return { node: undefined };
}

export function runQueryForNode(
  reader: SqliteReader,
  patternLabel: string,
  node: GraphNode,
): NavigationItem[] {
  const qdef = QUERY_MAP[patternLabel];
  if (!qdef) {
    return [];
  }

  const edges =
    qdef.direction === "incoming"
      ? reader.getEdgesByTarget(node.qualifiedName)
      : reader.getEdgesBySource(node.qualifiedName);

  const filtered = edges.filter((e) => e.kind === qdef.edgeKind);
  return buildRelatedItems(reader, filtered, qdef.direction);
}

export async function executeQuery(
  reader: SqliteReader,
  patternLabel: string,
  target: string,
): Promise<NavigationItem[]> {
  const resolution = resolveTarget(reader, target);
  if (!resolution.node) {
    return [];
  }
  return runQueryForNode(reader, patternLabel, resolution.node);
}

export async function pickAndNavigate(items: NavigationItem[], placeHolder: string): Promise<void> {
  if (items.length === 0) {
    vscode.window.showInformationMessage("Code Graph: No results found.");
    return;
  }

  const selected = await vscode.window.showQuickPick(items, { placeHolder });
  if (selected?.node) {
    await navigateToNode(selected.node);
  }
}

export function collectTestQualifiedNames(reader: SqliteReader, node: GraphNode): string[] {
  const incomingEdges = reader.getEdgesByTarget(node.qualifiedName);
  const incomingTestEdges = incomingEdges.filter((e) => e.kind === "TESTED_BY");

  const outgoingEdges = reader.getEdgesBySource(node.qualifiedName);
  const outgoingTestEdges = outgoingEdges.filter((e) => e.kind === "TESTED_BY");

  const testQualifiedNames = new Set<string>([
    ...incomingTestEdges.map((e) => e.sourceQualified),
    ...outgoingTestEdges.map((e) => e.targetQualified),
  ]);

  const conventionPatterns = [`test_${node.name}`, `Test${node.name}`];
  for (const pattern of conventionPatterns) {
    const matches = reader.searchNodes(pattern, 10);
    for (const match of matches) {
      if (match.isTest || match.kind === "Test") {
        testQualifiedNames.add(match.qualifiedName);
      }
    }
  }

  return [...testQualifiedNames].sort();
}

export function buildRelatedItems(
  reader: SqliteReader,
  edges: GraphEdge[],
  direction: QueryDirection,
): NavigationItem[] {
  return edges.map((edge) => {
    const relatedQn = direction === "incoming" ? edge.sourceQualified : edge.targetQualified;
    const relatedNode = reader.getNode(relatedQn);
    return {
      label: relatedNode?.name ?? relatedQn,
      description: relatedNode ? `${relatedNode.kind} · ${relatedNode.filePath}` : "",
      detail: `Line ${relatedNode?.lineStart ?? edge.line}`,
      node: relatedNode,
    };
  });
}

/**
 * Register the navigation commands: findCallers, findTests, findCallees,
 * queryGraph, and findLargeFunctions.
 */
export function registerNavigationCommands(
  context: vscode.ExtensionContext,
  getReader: () => SqliteReader | undefined,
): void {
  // -----------------------------------------------------------------
  // dagayn.findCallers
  // -----------------------------------------------------------------
  context.subscriptions.push(
    vscode.commands.registerCommand("dagayn.findCallers", async () => {
      const reader = getReader();
      if (!reader) {
        vscode.window.showWarningMessage("Code Graph: No graph database loaded.");
        return;
      }

      // Resolve node at cursor
      const node = resolveNodeAtCursor(reader);
      if (!node) {
        vscode.window.showWarningMessage(
          "Code Graph: No graph node found at the current cursor position.",
        );
        return;
      }

      // Query incoming CALLS edges
      const edges = reader.getEdgesByTarget(node.qualifiedName);
      const callerEdges = edges.filter((e) => e.kind === "CALLS");

      if (callerEdges.length === 0) {
        vscode.window.showInformationMessage(`Code Graph: No callers found for "${node.name}".`);
        return;
      }

      // Build QuickPick items, resolving each caller to its full node
      const items: Array<{
        label: string;
        description: string;
        detail: string;
        node: GraphNode | undefined;
      }> = [];

      for (const edge of callerEdges) {
        const callerNode = reader.getNode(edge.sourceQualified);
        items.push({
          label: callerNode?.name ?? edge.sourceQualified,
          description: callerNode?.filePath ?? edge.filePath,
          detail: `Line ${callerNode?.lineStart ?? edge.line}`,
          node: callerNode,
        });
      }

      const selected = await vscode.window.showQuickPick(items, {
        placeHolder: `Callers of ${node.name}`,
      });

      if (selected?.node) {
        await navigateToNode(selected.node);
      }
    }),
  );

  // -----------------------------------------------------------------
  // dagayn.findTests
  // -----------------------------------------------------------------
  context.subscriptions.push(
    vscode.commands.registerCommand("dagayn.findTests", async () => {
      const reader = getReader();
      if (!reader) {
        vscode.window.showWarningMessage("Code Graph: No graph database loaded.");
        return;
      }

      // Resolve node at cursor
      const node = resolveNodeAtCursor(reader);
      if (!node) {
        vscode.window.showWarningMessage(
          "Code Graph: No graph node found at the current cursor position.",
        );
        return;
      }

      // --- Collect test qualified names from TESTED_BY edges (both directions) ---
      const testQualifiedNames = collectTestQualifiedNames(reader, node);

      if (testQualifiedNames.length === 0) {
        vscode.window.showInformationMessage(`Code Graph: No tests found for "${node.name}".`);
        return;
      }

      // --- Build QuickPick items ---
      const items: Array<{
        label: string;
        description: string;
        detail: string;
        node: GraphNode | undefined;
      }> = [];

      for (const tqn of testQualifiedNames) {
        const testNode = reader.getNode(tqn);
        items.push({
          label: testNode?.name ?? tqn,
          description: testNode?.filePath ?? "",
          detail: `Line ${testNode?.lineStart ?? "?"}`,
          node: testNode,
        });
      }

      const selected = await vscode.window.showQuickPick(items, {
        placeHolder: `Tests for ${node.name}`,
      });

      if (selected?.node) {
        await navigateToNode(selected.node);
      }
    }),
  );

  // -----------------------------------------------------------------
  // dagayn.findCallees
  // -----------------------------------------------------------------
  context.subscriptions.push(
    vscode.commands.registerCommand("dagayn.findCallees", async () => {
      const reader = getReader();
      if (!reader) {
        vscode.window.showWarningMessage("Code Graph: No graph database loaded.");
        return;
      }

      const node = resolveNodeAtCursor(reader);
      if (!node) {
        vscode.window.showWarningMessage(
          "Code Graph: No graph node found at the current cursor position.",
        );
        return;
      }

      const edges = reader.getEdgesBySource(node.qualifiedName);
      const calleeEdges = edges.filter((e) => e.kind === "CALLS");

      if (calleeEdges.length === 0) {
        vscode.window.showInformationMessage(`Code Graph: No callees found for "${node.name}".`);
        return;
      }

      const items: Array<{
        label: string;
        description: string;
        detail: string;
        node: GraphNode | undefined;
      }> = [];

      for (const edge of calleeEdges) {
        const calleeNode = reader.getNode(edge.targetQualified);
        items.push({
          label: calleeNode?.name ?? edge.targetQualified,
          description: calleeNode?.filePath ?? edge.filePath,
          detail: `Line ${calleeNode?.lineStart ?? edge.line}`,
          node: calleeNode,
        });
      }

      const selected = await vscode.window.showQuickPick(items, {
        placeHolder: `Callees of ${node.name}`,
      });

      if (selected?.node) {
        await navigateToNode(selected.node);
      }
    }),
  );

  // -----------------------------------------------------------------
  // dagayn.queryGraph — expose all 8 query patterns
  // -----------------------------------------------------------------
  context.subscriptions.push(
    vscode.commands.registerCommand("dagayn.queryGraph", async () => {
      const reader = getReader();
      if (!reader) {
        vscode.window.showWarningMessage("Code Graph: No graph database loaded.");
        return;
      }

      const pattern = await vscode.window.showQuickPick(QUERY_PATTERNS, {
        placeHolder: "Select a query pattern",
      });
      if (!pattern) {
        return;
      }

      const target = await vscode.window.showInputBox({
        prompt: `Enter the target for ${pattern.label}`,
        placeHolder: "e.g., my_module.py::my_function or path/to/file.py",
      });
      if (!target) {
        return;
      }

      const resolution = resolveTarget(reader, target);
      if (resolution.multiple) {
        const picked = await vscode.window.showQuickPick(
          resolution.multiple.map((m) => ({
            label: m.name,
            description: `${m.kind} · ${m.filePath}`,
            node: m,
          })),
          { placeHolder: `Multiple matches for "${target}" — select one` },
        );
        if (!picked?.node) {
          return;
        }
        const items = runQueryForNode(reader, pattern.label, picked.node);
        await pickAndNavigate(
          items,
          `${pattern.label}: ${picked.node.name} (${items.length} results)`,
        );
        return;
      }

      if (!resolution.node) {
        vscode.window.showInformationMessage(`Code Graph: "${target}" not found.`);
        return;
      }

      const items = runQueryForNode(reader, pattern.label, resolution.node);
      await pickAndNavigate(
        items,
        `${pattern.label}: ${resolution.node.name} (${items.length} results)`,
      );
    }),
  );

  // -----------------------------------------------------------------
  // dagayn.findLargeFunctions
  // -----------------------------------------------------------------
  context.subscriptions.push(
    vscode.commands.registerCommand("dagayn.findLargeFunctions", async () => {
      const reader = getReader();
      if (!reader) {
        vscode.window.showWarningMessage("Code Graph: No graph database loaded.");
        return;
      }

      const minLinesStr = await vscode.window.showInputBox({
        prompt: "Minimum line count threshold",
        placeHolder: "50",
        value: "50",
      });
      if (!minLinesStr) {
        return;
      }

      const minLines = parseInt(minLinesStr, 10);
      if (Number.isNaN(minLines) || minLines < 1) {
        vscode.window.showWarningMessage("Code Graph: Invalid line count.");
        return;
      }

      const results = reader.getNodesBySize(minLines, undefined, undefined, 50);

      if (results.length === 0) {
        vscode.window.showInformationMessage(
          `Code Graph: No functions found with ${minLines}+ lines.`,
        );
        return;
      }

      const items = results.map((r) => ({
        label: `${r.name} (${r.lineCount} lines)`,
        description: `${r.kind} · ${r.filePath}`,
        detail: `Lines ${r.lineStart ?? "?"}–${r.lineEnd ?? "?"}`,
        node: r as GraphNode,
      }));

      const selected = await vscode.window.showQuickPick(items, {
        placeHolder: `${results.length} nodes with ${minLines}+ lines`,
      });

      if (selected?.node) {
        await navigateToNode(selected.node);
      }
    }),
  );

  // -----------------------------------------------------------------
  // Saved custom queries
  // -----------------------------------------------------------------

  function getWorkspaceFsPath(): string | undefined {
    const activeEditor = vscode.window.activeTextEditor;
    if (activeEditor) {
      const folder = vscode.workspace.getWorkspaceFolder(activeEditor.document.uri);
      if (folder) {
        return folder.uri.fsPath;
      }
    }
    return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
  }

  async function deleteSavedQueryFlow(workspaceFsPath: string): Promise<void> {
    let queries: SavedQuery[];
    try {
      queries = await loadSavedQueries(workspaceFsPath, VALID_PATTERNS);
    } catch (err: unknown) {
      vscode.window.showErrorMessage(
        `Code Graph: Could not load saved queries. ${
          err instanceof Error ? err.message : String(err)
        }`,
      );
      return;
    }

    if (queries.length === 0) {
      vscode.window.showInformationMessage("Code Graph: No saved queries to delete.");
      return;
    }

    const picked = await vscode.window.showQuickPick(
      queries.map((q) => ({
        label: q.label,
        description: `${q.pattern} · ${q.target}`,
        query: q,
      })),
      { placeHolder: "Delete which saved query?" },
    );

    if (!picked?.query) {
      return;
    }

    try {
      await deleteSavedQuery(workspaceFsPath, picked.query.label);
      vscode.window.showInformationMessage(
        `Code Graph: Deleted saved query "${picked.query.label}".`,
      );
    } catch (err: unknown) {
      vscode.window.showErrorMessage(
        `Code Graph: Could not delete query. ${err instanceof Error ? err.message : String(err)}`,
      );
    }
  }

  // -----------------------------------------------------------------
  // dagayn.saveCustomQuery
  // -----------------------------------------------------------------
  context.subscriptions.push(
    vscode.commands.registerCommand("dagayn.saveCustomQuery", async () => {
      const workspaceFsPath = getWorkspaceFsPath();
      if (!workspaceFsPath) {
        vscode.window.showWarningMessage("Code Graph: Open a workspace folder to save queries.");
        return;
      }

      const pattern = await vscode.window.showQuickPick(QUERY_PATTERNS, {
        placeHolder: "Select a query pattern to save",
      });
      if (!pattern) {
        return;
      }

      const target = await vscode.window.showInputBox({
        prompt: `Enter the target for ${pattern.label}`,
        placeHolder: "e.g., my_module.py::my_function or path/to/file.py",
      });
      if (!target) {
        return;
      }

      const label = await vscode.window.showInputBox({
        prompt: "Name for this saved query",
        placeHolder: "e.g., callers of login",
        validateInput: (value) => (value?.trim() ? undefined : "Label is required"),
      });
      if (!label) {
        return;
      }

      try {
        await saveSavedQuery(workspaceFsPath, {
          label: label.trim(),
          pattern: pattern.label,
          target,
        });
        vscode.window.showInformationMessage(`Code Graph: Saved query "${label.trim()}".`);
      } catch (err: unknown) {
        vscode.window.showErrorMessage(
          `Code Graph: Could not save query. ${err instanceof Error ? err.message : String(err)}`,
        );
      }
    }),
  );

  // -----------------------------------------------------------------
  // dagayn.runSavedQuery
  // -----------------------------------------------------------------
  context.subscriptions.push(
    vscode.commands.registerCommand("dagayn.runSavedQuery", async () => {
      const reader = getReader();
      if (!reader) {
        vscode.window.showWarningMessage("Code Graph: No graph database loaded.");
        return;
      }

      const workspaceFsPath = getWorkspaceFsPath();
      if (!workspaceFsPath) {
        vscode.window.showWarningMessage(
          "Code Graph: Open a workspace folder to run saved queries.",
        );
        return;
      }

      let queries: SavedQuery[];
      try {
        queries = await loadSavedQueries(workspaceFsPath, VALID_PATTERNS);
      } catch (err: unknown) {
        vscode.window.showErrorMessage(
          `Code Graph: Could not load saved queries. ${
            err instanceof Error ? err.message : String(err)
          }`,
        );
        return;
      }

      if (queries.length === 0) {
        vscode.window.showInformationMessage(
          "Code Graph: No saved queries yet. Run Code Graph: Save Custom Query first.",
        );
        return;
      }

      type SavedQueryItem = vscode.QuickPickItem & {
        query?: SavedQuery;
        action?: "run" | "delete";
      };

      const items: SavedQueryItem[] = [
        ...queries.map((q) => ({
          label: q.label,
          description: `${q.pattern} · ${q.target}`,
          query: q,
          action: "run" as const,
        })),
        { label: "— Delete a saved query —", action: "delete" as const },
      ];

      const picked = await vscode.window.showQuickPick(items, {
        placeHolder: "Select a saved query to run",
      });
      if (!picked) {
        return;
      }

      if (picked.action === "delete") {
        await deleteSavedQueryFlow(workspaceFsPath);
        return;
      }

      const query = picked.query!;
      const resolution = resolveTarget(reader, query.target);

      if (resolution.multiple) {
        vscode.window.showInformationMessage(
          `Code Graph: Saved query "${query.label}" matched multiple nodes — refine the target.`,
        );
        return;
      }

      if (!resolution.node) {
        vscode.window.showInformationMessage(
          `Code Graph: Saved query "${query.label}" did not match any node.`,
        );
        return;
      }

      const results = runQueryForNode(reader, query.pattern, resolution.node);
      await pickAndNavigate(
        results,
        `${query.pattern}: ${resolution.node.name} (${results.length} results)`,
      );
    }),
  );
}
