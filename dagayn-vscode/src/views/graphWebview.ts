/**
 * Webview panel for the interactive graph visualization.
 * Uses D3.js (bundled via esbuild) to render a force-directed graph.
 *
 * Hosts the toolbar HTML, CSS, and manages communication with the
 * browser-side graph.ts script.
 */

import * as vscode from "vscode";
import * as fs from "node:fs";
import * as crypto from "node:crypto";
import type { SqliteReader, ImpactRadius, GraphNode, GraphEdge } from "../backend/sqlite";
import { resolveNodeFilePath } from "../backend/pathResolution";
import type { ModuleGraph } from "../backend/moduleAggregation";
import { parseIncomingWebviewMessage } from "./webviewMessages";

type ViewMode = "symbol" | "module";

type PanelState =
  | { status: "idle" }
  | {
      status: "loading";
      panel: vscode.WebviewPanel;
      reader: SqliteReader;
      sourceId: string;
      impactRadius?: ImpactRadius;
      pendingHighlight?: string;
      moduleGraph?: ModuleGraph;
      viewMode: ViewMode;
    }
  | {
      status: "ready";
      panel: vscode.WebviewPanel;
      reader: SqliteReader;
      sourceId: string;
      impactRadius?: ImpactRadius;
      pendingHighlight?: string;
      moduleGraph?: ModuleGraph;
      viewMode: ViewMode;
    };

export class GraphWebviewPanel {
  private static lifecycle: PanelState = { status: "idle" };
  private static testInstance: GraphWebviewPanel | undefined;

  private readonly panel: vscode.WebviewPanel;
  private readonly reader: SqliteReader;
  private readonly impactRadius?: ImpactRadius;
  private readonly moduleGraph?: ModuleGraph;
  private readonly viewMode: ViewMode;
  private pendingHighlight?: string;
  private disposables: vscode.Disposable[] = [];

  private constructor(
    panel: vscode.WebviewPanel,
    extensionUri: vscode.Uri,
    reader: SqliteReader,
    private readonly sourceId: string,
    impactRadius?: ImpactRadius,
    pendingHighlight?: string,
    moduleGraph?: ModuleGraph,
    viewMode: ViewMode = "symbol",
  ) {
    this.panel = panel;
    this.reader = reader;
    this.impactRadius = impactRadius;
    this.moduleGraph = moduleGraph;
    this.viewMode = viewMode;
    this.pendingHighlight = pendingHighlight;

    this.panel.onDidDispose(() => this.dispose(), null, this.disposables);

    GraphWebviewPanel.testInstance = this;

    this.panel.webview.html = this.getHtmlContent(this.panel.webview, extensionUri);

    this.panel.webview.onDidReceiveMessage(
      (message) => {
        void this.handleMessage(message);
      },
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
    GraphWebviewPanel.createOrShowInternal(
      extensionUri,
      reader,
      sourceId,
      impactRadius,
      highlightQualifiedName,
      undefined,
      "symbol",
    );
  }

  static createOrShowModule(
    extensionUri: vscode.Uri,
    reader: SqliteReader,
    folder: string,
    moduleGraph: ModuleGraph,
  ): void {
    GraphWebviewPanel.createOrShowInternal(
      extensionUri,
      reader,
      `module:${folder}`,
      undefined,
      undefined,
      moduleGraph,
      "module",
    );
  }

  private static createOrShowInternal(
    extensionUri: vscode.Uri,
    reader: SqliteReader,
    sourceId: string,
    impactRadius?: ImpactRadius,
    highlightQualifiedName?: string,
    moduleGraph?: ModuleGraph,
    viewMode: ViewMode = "symbol",
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
      moduleGraph,
      viewMode,
    };

    new GraphWebviewPanel(
      panel,
      extensionUri,
      reader,
      sourceId,
      impactRadius,
      highlightQualifiedName,
      moduleGraph,
      viewMode,
    );
  }

  private dispose(): void {
    GraphWebviewPanel.lifecycle = { status: "idle" };
    GraphWebviewPanel.testInstance = undefined;
    this.panel.dispose();
    for (const d of this.disposables) {
      d.dispose();
    }
    this.disposables = [];
  }

  /** Test-only hook to drive message handling from unit tests. */
  static async __handleMessageForTests(message: unknown): Promise<void> {
    const instance = GraphWebviewPanel.testInstance;
    if (!instance) {
      return Promise.resolve();
    }
    return (
      instance as unknown as { handleMessage: (msg: unknown) => Promise<void> }
    ).handleMessage(message);
  }

  /** Test-only hook to reset the singleton lifecycle between tests. */
  static __resetForTests(): void {
    GraphWebviewPanel.lifecycle = { status: "idle" };
    GraphWebviewPanel.testInstance = undefined;
  }

  // -----------------------------------------------------------------------
  // Message handling
  // -----------------------------------------------------------------------

  private async handleMessage(message: unknown): Promise<void> {
    const command =
      message != null && typeof message === "object" && "command" in message
        ? String((message as { command?: unknown }).command)
        : "unknown";

    try {
      const parsed = parseIncomingWebviewMessage(message);
      if (!parsed) {
        console.error(`[dagayn] invalid webview message (${command}):`, message);
        await vscode.window.showErrorMessage(
          `Code Graph: invalid webview message (${command}). See console for details.`,
        );
        return;
      }

      switch (parsed.command) {
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
          if (parsed.kind === "Module") {
            await vscode.window.showInformationMessage(`Code Graph: Module ${parsed.filePath}`);
          } else {
            await this.openFileAtLine(parsed.filePath, parsed.lineStart);
            // Bidirectional sync: reveal in tree view
            if (parsed.qualifiedName) {
              await vscode.commands.executeCommand("dagayn.revealInTree", parsed.qualifiedName);
            }
          }
          break;

        case "exportSvg":
          await this.exportSvgToClipboard(parsed.svg);
          break;

        case "exportPng":
          await this.savePngToFile(parsed.data);
          break;
      }
    } catch (err) {
      console.error(`[dagayn] failed to handle webview message (${command}):`, err);
      await vscode.window.showErrorMessage(
        `Code Graph: failed to handle webview message (${command}). See console for details.`,
      );
    }
  }

  /**
   * Send full graph data to the webview.
   * If an impact radius was provided, send only those nodes/edges.
   * If module mode is active, send the precomputed module graph.
   * Otherwise send the full symbol graph.
   */
  private sendGraphData(): void {
    const config = vscode.workspace.getConfiguration("dagayn");
    const configuredMaxNodes = config.get<number>("graph.maxNodes", 500);

    let nodes: GraphNode[];
    let edges: GraphEdge[];
    let truncated = false;

    if (this.impactRadius) {
      nodes = [...this.impactRadius.changedNodes, ...this.impactRadius.impactedNodes];
      edges = this.impactRadius.edges;
    } else if (this.viewMode === "module" && this.moduleGraph) {
      nodes = this.moduleGraph.nodes.map((m) => ({
        id: m.id,
        kind: "Module" as const,
        name: m.name,
        qualifiedName: m.dirPath,
        filePath: m.dirPath,
        lineStart: null,
        lineEnd: null,
        language: m.language,
        parentName: null,
        params: null,
        returnType: null,
        modifiers: null,
        isTest: false,
        fileHash: null,
        extra: {},
      }));
      edges = this.moduleGraph.edges.map((e, index) => ({
        id: index + 1,
        kind: e.kind,
        sourceQualified: e.sourceDir,
        targetQualified: e.targetDir,
        filePath: e.sourceDir,
        line: 0,
      }));
    } else {
      // Bounded load: ask for one extra row so we can detect truncation
      // without materialising the whole table.
      const nodesPlus = this.reader.getNodesLimited(configuredMaxNodes + 1);
      truncated = nodesPlus.length > configuredMaxNodes;
      nodes = truncated ? nodesPlus.slice(0, configuredMaxNodes) : nodesPlus;
      const nodeQns = new Set(nodes.map((n) => n.qualifiedName));
      edges = this.reader.getEdgesForNodes(nodeQns);
    }

    // Enforce maxNodes setting in symbol/impact mode only (module graphs are small).
    const maxNodes = this.viewMode === "module" ? nodes.length : configuredMaxNodes;
    if (this.viewMode !== "module" && nodes.length > maxNodes) {
      truncated = true;
      nodes = nodes.slice(0, maxNodes);
      const nodeQns = new Set(nodes.map((n) => n.qualifiedName));
      edges = edges.filter((e) => nodeQns.has(e.sourceQualified) && nodeQns.has(e.targetQualified));
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
  private async openFileAtLine(
    filePath: string,
    lineStart: number | null | undefined,
  ): Promise<void> {
    const workspaceFolders =
      vscode.workspace.workspaceFolders?.map((folder) => folder.uri.fsPath) ?? [];
    const { candidate, tried } = resolveNodeFilePath(filePath, workspaceFolders);
    const fullPath = candidate ?? filePath;

    try {
      const doc = await vscode.workspace.openTextDocument(fullPath);
      const line = Math.max(
        0,
        (typeof lineStart === "number" && Number.isFinite(lineStart) ? lineStart : 1) - 1,
      );
      await vscode.window.showTextDocument(doc, {
        viewColumn: vscode.ViewColumn.One,
        selection: new vscode.Range(line, 0, line, 0),
        preserveFocus: false,
      });
    } catch (err) {
      const detail = err instanceof Error ? err.message : String(err);
      console.error(
        `Code Graph: Could not open file ${filePath}\nResolved to: ${fullPath}\nTried: ${tried.join(", ")}\n${detail}`,
      );
      const action = await vscode.window.showErrorMessage(
        `Code Graph: Could not open file ${filePath}. ${candidate ? `Resolved to ${candidate}.` : "See console for candidates tried."}`,
        "Copy Path",
        "Dismiss",
      );
      if (action === "Copy Path") {
        await vscode.env.clipboard.writeText(candidate ?? filePath);
      }
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
