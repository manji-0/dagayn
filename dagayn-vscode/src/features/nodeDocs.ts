import * as vscode from "vscode";
import type { GraphNode } from "../backend/sqlite";
import type { SqliteReader } from "../backend/sqlite";
import { resolveNodeAtCursor } from "./cursorResolver";
import { NodeDocsPanel } from "../views/nodeDocsPanel";

export const NO_DOCUMENTATION_MESSAGE = "No documentation available.";

const DOCUMENTATION_KEYS = ["docstring", "doc", "comment", "comments"] as const;

/**
 * Extract a human-readable documentation string from a node's `extra` object.
 *
 * Priority order: docstring > doc > comment > comments.
 * Arrays under `comments` are joined with newlines. Empty values are ignored.
 */
export function getNodeDocumentation(
  node: { extra?: Record<string, unknown> } | undefined,
): string {
  if (!node?.extra || typeof node.extra !== "object") {
    return "";
  }

  for (const key of DOCUMENTATION_KEYS) {
    const raw = node.extra[key];
    if (raw === undefined || raw === null) {
      continue;
    }

    if (key === "comments" && Array.isArray(raw)) {
      const joined = raw
        .filter((item): item is string => typeof item === "string")
        .join("\n")
        .trim();
      if (joined.length > 0) {
        return joined;
      }
      continue;
    }

    if (typeof raw === "string") {
      const trimmed = raw.trim();
      if (trimmed.length > 0) {
        return trimmed;
      }
    }
  }

  return "";
}

/**
 * Build a MarkdownString suitable for a tree-item tooltip or webview body.
 */
export function formatNodeDocumentationMarkdown(node: GraphNode): vscode.MarkdownString {
  const docs = getNodeDocumentation(node);
  const md = new vscode.MarkdownString();

  if (docs.length > 0) {
    md.appendMarkdown(`### ${node.name}\n\n${docs}`);
  } else {
    md.appendMarkdown(NO_DOCUMENTATION_MESSAGE);
  }

  return md;
}

/**
 * Register the "Code Graph: Show Node Documentation" command.
 */
export function registerNodeDocsCommand(
  context: vscode.ExtensionContext,
  getReader: () => SqliteReader | undefined,
): void {
  context.subscriptions.push(
    vscode.commands.registerCommand("dagayn.showNodeDocumentation", async () => {
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

      const docs = getNodeDocumentation(node);
      const body = docs.length > 0 ? docs : NO_DOCUMENTATION_MESSAGE;

      NodeDocsPanel.show(context.extensionUri, node, body);
    }),
  );
}
