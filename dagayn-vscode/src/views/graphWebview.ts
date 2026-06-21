/**
 * Webview panel for the interactive graph visualization.
 * Uses D3.js (bundled via esbuild) to render a force-directed graph.
 *
 * Hosts the toolbar HTML, CSS, and manages communication with the
 * browser-side graph.ts script.
 */

import * as vscode from "vscode";
import * as path from "node:path";
import * as fs from "node:fs";
import * as crypto from "node:crypto";
import type { SqliteReader, ImpactRadius } from "../backend/sqlite";

type PanelState =
  | { status: "idle" }
  | {
      status: "loading";
      panel: vscode.WebviewPanel;
      reader: SqliteReader;
      sourceId: string;
      impactRadius?: ImpactRadius;
      pendingHighlight?: string;
    }
  | {
      status: "ready";
      panel: vscode.WebviewPanel;
      reader: SqliteReader;
      sourceId: string;
      impactRadius?: ImpactRadius;
      pendingHighlight?: string;
    };

export class GraphWebviewPanel {
  private static lifecycle: PanelState = { status: "idle" };

  private readonly panel: vscode.WebviewPanel;
  private readonly reader: SqliteReader;
  private readonly impactRadius?: ImpactRadius;
  private pendingHighlight?: string;
  private disposables: vscode.Disposable[] = [];

  private constructor(
    panel: vscode.WebviewPanel,
    extensionUri: vscode.Uri,
    reader: SqliteReader,
    private readonly sourceId: string,
    impactRadius?: ImpactRadius,
    pendingHighlight?: string,
  ) {
    this.panel = panel;
    this.reader = reader;
    this.impactRadius = impactRadius;
    this.pendingHighlight = pendingHighlight;

    this.panel.onDidDispose(() => this.dispose(), null, this.disposables);

    this.panel.webview.html = this.getHtmlContent(this.panel.webview, extensionUri);

    this.panel.webview.onDidReceiveMessage(
      (message) => this.handleMessage(message),
      null,
      this.disposables,
    );

    // Listen for theme changes
    this.disposables.push(
      vscode.window.onDidChangeActiveColorTheme((theme) => {
        const themeKind =
          theme.kind === vscode.ColorThemeKind.Light ||
          theme.kind === vscode.ColorThemeKind.HighContrastLight
            ? "light"
            : "dark";
        this.panel.webview.postMessage({
          command: "setTheme",
          theme: themeKind,
        });
      }),
    );
  }

  static createOrShow(
    extensionUri: vscode.Uri,
    reader: SqliteReader,
    sourceId: string,
    impactRadius?: ImpactRadius,
    highlightQualifiedName?: string,
  ): void {
    const column = vscode.ViewColumn.Beside;

    if (
      (GraphWebviewPanel.lifecycle.status === "loading" ||
        GraphWebviewPanel.lifecycle.status === "ready") &&
      GraphWebviewPanel.lifecycle.sourceId === sourceId
    ) {
      GraphWebviewPanel.lifecycle.panel.reveal(column);

      // Re-send data if a new highlight is requested
      if (highlightQualifiedName) {
        GraphWebviewPanel.lifecycle.panel.webview.postMessage({
          command: "highlightNode",
          qualifiedName: highlightQualifiedName,
        });
      }

      return;
    }

    // Dispose any existing panel from a different source so the new graph is shown.
    if (
      GraphWebviewPanel.lifecycle.status === "loading" ||
      GraphWebviewPanel.lifecycle.status === "ready"
    ) {
      GraphWebviewPanel.lifecycle.panel.dispose();
    }

    const panel = vscode.window.createWebviewPanel("dagayn.graph", "Code Graph", column, {
      enableScripts: true,
      retainContextWhenHidden: true,
      localResourceRoots: [
        vscode.Uri.joinPath(extensionUri, "dist"),
        vscode.Uri.joinPath(extensionUri, "media"),
      ],
    });

    GraphWebviewPanel.lifecycle = {
      status: "loading",
      panel,
      reader,
      sourceId,
      impactRadius,
      pendingHighlight: highlightQualifiedName,
    };

    new GraphWebviewPanel(
      panel,
      extensionUri,
      reader,
      sourceId,
      impactRadius,
      highlightQualifiedName,
    );
  }

  private dispose(): void {
    GraphWebviewPanel.lifecycle = { status: "idle" };
    this.panel.dispose();
    for (const d of this.disposables) {
      d.dispose();
    }
    this.disposables = [];
  }

  // -----------------------------------------------------------------------
  // Message handling
  // -----------------------------------------------------------------------

  private handleMessage(message: { command: string; [key: string]: unknown }): void {
    switch (message.command) {
      case "ready":
        if (GraphWebviewPanel.lifecycle.status === "loading") {
          GraphWebviewPanel.lifecycle = {
            ...GraphWebviewPanel.lifecycle,
            status: "ready",
          };
        }
        this.sendGraphData();
        break;

      case "nodeClicked":
        this.openFileAtLine(message.filePath as string, message.lineStart as number);
        // Bidirectional sync: reveal in tree view
        if (message.qualifiedName) {
          vscode.commands.executeCommand("dagayn.revealInTree", message.qualifiedName as string);
        }
        break;

      case "exportSvg":
        this.exportSvgToClipboard(message.svg as string);
        break;

      case "exportPng":
        this.savePngToFile(message.data as string);
        break;
    }
  }

  /**
   * Send full graph data to the webview.
   * If an impact radius was provided, send only those nodes/edges.
   * Otherwise send the full graph.
   */
  private sendGraphData(): void {
    let nodes;
    let edges;

    if (this.impactRadius) {
      nodes = [...this.impactRadius.changedNodes, ...this.impactRadius.impactedNodes];
      edges = this.impactRadius.edges;
    } else {
      // Load all nodes and edges
      const files = this.reader.getAllFiles();
      nodes = files.flatMap((f) => this.reader.getNodesByFile(f));
      const qualifiedNames = new Set(nodes.map((n) => n.qualifiedName));
      edges = this.reader.getEdgesAmong(qualifiedNames);
    }

    // Enforce maxNodes setting
    const config = vscode.workspace.getConfiguration("dagayn");
    const maxNodes = config.get<number>("graph.maxNodes", 500);
    let truncated = false;
    if (nodes.length > maxNodes) {
      truncated = true;
      nodes = nodes.slice(0, maxNodes);
      const nodeQns = new Set(nodes.map((n: { qualifiedName: string }) => n.qualifiedName));
      edges = edges.filter(
        (e: { sourceQualified: string; targetQualified: string }) =>
          nodeQns.has(e.sourceQualified) && nodeQns.has(e.targetQualified),
      );
    }

    this.panel.webview.postMessage({
      command: "setData",
      nodes,
      edges,
      truncated,
      maxNodes,
    });

    // Send theme
    const themeKind =
      vscode.window.activeColorTheme.kind === vscode.ColorThemeKind.Light ||
      vscode.window.activeColorTheme.kind === vscode.ColorThemeKind.HighContrastLight
        ? "light"
        : "dark";
    this.panel.webview.postMessage({
      command: "setTheme",
      theme: themeKind,
    });

    // Highlight node if requested
    const highlightQualifiedName = this.pendingHighlight;
    if (highlightQualifiedName) {
      this.pendingHighlight = undefined;
      if (GraphWebviewPanel.lifecycle.status !== "idle") {
        GraphWebviewPanel.lifecycle = {
          ...GraphWebviewPanel.lifecycle,
          pendingHighlight: undefined,
        };
      }
      // Small delay to let the graph render first
      setTimeout(() => {
        this.panel.webview.postMessage({
          command: "highlightNode",
          qualifiedName: highlightQualifiedName,
        });
      }, 1000);
    }
  }

  /**
   * Open a file in the editor at a specific line.
   */
  private async openFileAtLine(filePath: string, lineStart: number): Promise<void> {
    let fullPath = filePath;
    if (!path.isAbsolute(filePath)) {
      const workspaceRoot = vscode.workspace.getWorkspaceFolder(vscode.Uri.file(filePath))?.uri
        .fsPath;
      fullPath = workspaceRoot ? path.join(workspaceRoot, filePath) : filePath;
    }

    try {
      const doc = await vscode.workspace.openTextDocument(fullPath);
      const line = Math.max(0, (lineStart ?? 1) - 1);
      await vscode.window.showTextDocument(doc, {
        viewColumn: vscode.ViewColumn.One,
        selection: new vscode.Range(line, 0, line, 0),
        preserveFocus: false,
      });
    } catch {
      vscode.window.showWarningMessage(`Code Graph: Could not open file ${filePath}`);
    }
  }

  /**
   * Copy SVG string to clipboard.
   */
  private async exportSvgToClipboard(svgString: string): Promise<void> {
    await vscode.env.clipboard.writeText(svgString);
    vscode.window.showInformationMessage("Code Graph: SVG copied to clipboard.");
  }

  /**
   * Save PNG data URL to a file.
   */
  private async savePngToFile(dataUrl: string): Promise<void> {
    const uri = await vscode.window.showSaveDialog({
      defaultUri: vscode.Uri.file("code-graph.png"),
      filters: { "PNG Image": ["png"] },
    });
    if (!uri) {
      return;
    }

    const base64 = dataUrl.replace(/^data:image\/png;base64,/, "");
    const buffer = Buffer.from(base64, "base64");
    await vscode.workspace.fs.writeFile(uri, buffer);
    vscode.window.showInformationMessage("Code Graph: PNG saved.");
  }

  /**
   * Highlight a node by qualified name from external code (tree view click).
   */
  static highlightNode(qualifiedName: string): void {
    if (GraphWebviewPanel.lifecycle.status === "ready") {
      GraphWebviewPanel.lifecycle.panel.webview.postMessage({
        command: "highlightNode",
        qualifiedName,
      });
    } else if (GraphWebviewPanel.lifecycle.status === "loading") {
      GraphWebviewPanel.lifecycle = {
        ...GraphWebviewPanel.lifecycle,
        pendingHighlight: qualifiedName,
      };
    }
  }

  // -----------------------------------------------------------------------
  // HTML content
  // -----------------------------------------------------------------------

  private getHtmlContent(webview: vscode.Webview, extensionUri: vscode.Uri): string {
    const scriptUri = webview.asWebviewUri(
      vscode.Uri.joinPath(extensionUri, "dist", "webview", "graph.js"),
    );
    const styleUri = webview.asWebviewUri(
      vscode.Uri.joinPath(extensionUri, "media", "webview", "graph.css"),
    );
    const nonce = getNonce();
    const htmlPath = vscode.Uri.joinPath(extensionUri, "media", "webview", "graph.html").fsPath;
    const template = fs.readFileSync(htmlPath, "utf-8");
    return template
      .replace(/\{\{NONCE\}\}/g, nonce)
      .replace(/\{\{CSP_SOURCE\}\}/g, webview.cspSource)
      .replace(/\{\{SCRIPT_URI\}\}/g, scriptUri.toString())
      .replace(/\{\{STYLE_URI\}\}/g, styleUri.toString());
  }
}

function getNonce(): string {
  return crypto.randomBytes(16).toString("hex");
}
