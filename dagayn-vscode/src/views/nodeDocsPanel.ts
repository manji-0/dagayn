import * as vscode from "vscode";
import MarkdownIt from "markdown-it";
import type { GraphNode } from "../backend/sqlite";

const md = new MarkdownIt({ html: false, linkify: true, typographer: false });

// biome-ignore lint/complexity/noStaticOnlyClass: mirrors GraphWebviewPanel lifecycle pattern
export class NodeDocsPanel {
  private static panel: vscode.WebviewPanel | undefined;

  static show(extensionUri: vscode.Uri, node: GraphNode, markdownBody: string): void {
    const column = vscode.ViewColumn.Beside;

    if (NodeDocsPanel.panel) {
      NodeDocsPanel.panel.reveal(column);
      NodeDocsPanel.panel.title = node.name;
      NodeDocsPanel.panel.webview.html = NodeDocsPanel.getHtmlContent(node, markdownBody);
      return;
    }

    const panel = vscode.window.createWebviewPanel("dagayn.nodeDocs", node.name, column, {
      enableScripts: false,
      retainContextWhenHidden: false,
      localResourceRoots: [extensionUri],
    });

    NodeDocsPanel.panel = panel;
    panel.webview.html = NodeDocsPanel.getHtmlContent(node, markdownBody);

    panel.onDidDispose(
      () => {
        NodeDocsPanel.panel = undefined;
      },
      null,
      [],
    );
  }

  private static getHtmlContent(node: GraphNode, markdownBody: string): string {
    const bodyHtml = md.render(markdownBody);
    const escapedName = escapeHtml(node.qualifiedName);

    return `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>${escapedName}</title>
  <style>
    body {
      font-family: var(--vscode-font-family), system-ui, sans-serif;
      font-size: var(--vscode-font-size);
      color: var(--vscode-foreground);
      padding: 1rem 1.5rem;
      line-height: 1.6;
    }
    h1 {
      font-size: 1.25rem;
      margin: 0 0 1rem;
      font-weight: 600;
    }
    h2, h3, h4 {
      font-weight: 600;
      margin: 1.25rem 0 0.5rem;
    }
    p {
      margin: 0 0 0.75rem;
    }
    pre {
      white-space: pre-wrap;
      word-wrap: break-word;
      padding: 0.75rem;
      background: var(--vscode-textCodeBlock-background);
      border-radius: 4px;
      font-family: var(--vscode-editor-font-family), monospace;
    }
    code {
      font-family: var(--vscode-editor-font-family), monospace;
      background: var(--vscode-textCodeBlock-background);
      padding: 0.125rem 0.25rem;
      border-radius: 3px;
    }
    ul, ol {
      margin: 0 0 0.75rem;
      padding-left: 1.5rem;
    }
    .fallback {
      font-style: italic;
      opacity: 0.8;
    }
  </style>
</head>
<body>
  <h1>${escapedName}</h1>
  ${bodyHtml}
</body>
</html>`;
  }
}

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}
