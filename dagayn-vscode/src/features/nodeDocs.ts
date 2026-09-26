import * as vscode from "vscode";
import type { GraphNode } from "../backend/sqlite";
import type { SqliteReader } from "../backend/sqlite";
import { resolveNodeAtCursor } from "./cursorResolver";
import { NodeDocsPanel } from "../views/nodeDocsPanel";
import { getNodeDocumentation } from "../backend/nodeDocumentation";

export { getNodeDocumentation };

export const NO_DOCUMENTATION_MESSAGE = "No documentation available.";

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

/**
 * Register an editor hover provider that shows a graph node's documentation
 * when the cursor hovers over a symbol known to the graph database.
 */
export function registerNodeHover(
  context: vscode.ExtensionContext,
  getReader: () => SqliteReader | undefined,
): void {
  context.subscriptions.push(
    vscode.languages.registerHoverProvider("*", {
      provideHover(document, position): vscode.Hover | null {
        const reader = getReader();
        if (!reader) {
          return null;
        }

        const node = reader.getNodeAtCursor(document.uri.fsPath, position.line + 1);
        if (!node) {
          return null;
        }

        const docs = getNodeDocumentation(node);
        if (docs.length === 0) {
          return null;
        }

        return new vscode.Hover(formatNodeDocumentationMarkdown(node));
      },
    }),
  );
}
