import * as assert from "node:assert";
import * as vscode from "vscode";
import {
  getNodeDocumentation,
  formatNodeDocumentationMarkdown,
  NO_DOCUMENTATION_MESSAGE,
  registerNodeDocsCommand,
  registerNodeHover,
} from "../src/features/nodeDocs";
import type { GraphNode } from "../src/backend/sqlite";
import type { SqliteReader } from "../src/backend/sqlite";

const window = vscode.window as typeof vscode.window & {
  __createdWebviewPanels: unknown[];
  __clearCreatedWebviewPanels: () => void;
};

const languages = vscode.languages as typeof vscode.languages & {
  __hoverProviders: Array<{
    provideHover: (document: unknown, position: { line: number }) => unknown;
  }>;
  __clearHoverProviders: () => void;
};

describe("getNodeDocumentation", () => {
  it("returns the docstring when present", () => {
    assert.strictEqual(getNodeDocumentation({ extra: { docstring: "x" } }), "x");
  });

  it("returns doc when docstring is absent", () => {
    assert.strictEqual(getNodeDocumentation({ extra: { doc: "y" } }), "y");
  });

  it("returns comment when docstring/doc are absent", () => {
    assert.strictEqual(getNodeDocumentation({ extra: { comment: "z" } }), "z");
  });

  it("joins comments arrays", () => {
    assert.strictEqual(getNodeDocumentation({ extra: { comments: ["a", "b"] } }), "a\nb");
  });

  it("ignores empty objects", () => {
    assert.strictEqual(getNodeDocumentation({ extra: {} }), "");
  });

  it("returns empty string for undefined input", () => {
    assert.strictEqual(getNodeDocumentation(undefined), "");
  });

  it("prefers docstring over doc", () => {
    assert.strictEqual(getNodeDocumentation({ extra: { docstring: "a", doc: "b" } }), "a");
  });

  it("trims whitespace", () => {
    assert.strictEqual(getNodeDocumentation({ extra: { docstring: "  padded  " } }), "padded");
  });
});

describe("formatNodeDocumentationMarkdown", () => {
  it("includes the node name and docstring", () => {
    const node = makeNode({ docstring: "Does the thing." });
    const md = formatNodeDocumentationMarkdown(node);
    assert.ok(md.value.includes("### doThing"));
    assert.ok(md.value.includes("Does the thing."));
  });

  it("falls back to the no-documentation message", () => {
    const node = makeNode({});
    const md = formatNodeDocumentationMarkdown(node);
    assert.ok(md.value.includes(NO_DOCUMENTATION_MESSAGE));
  });
});

describe("registerNodeDocsCommand", () => {
  beforeEach(() => {
    window.__clearCreatedWebviewPanels();
  });

  afterEach(() => {
    // Dispose any panel created by NodeDocsPanel so static state is reset.
    for (const panel of window.__createdWebviewPanels) {
      (panel as { dispose: () => void }).dispose();
    }
    window.__clearCreatedWebviewPanels();
  });

  it("opens a webview panel with the docstring", async () => {
    const node = makeNode({ docstring: "Authenticate a user." });
    const reader = makeReader(node);
    vscode.window.activeTextEditor = makeEditor();

    const context = { extensionUri: vscode.Uri.file("/extension"), subscriptions: [] };

    // The stub's registerCommand returns a disposable, so we cannot invoke the
    // callback directly. Re-register with a captured callback instead.
    let handler: (() => Promise<void>) | undefined;
    const originalRegister = vscode.commands.registerCommand;
    const disposable = originalRegister("dagayn.showNodeDocumentation", async () => {
      /* noop */
    });
    try {
      vscode.commands.registerCommand = (
        command: string,
        callback: (...args: unknown[]) => unknown,
      ) => {
        if (command === "dagayn.showNodeDocumentation") {
          handler = callback as () => Promise<void>;
        }
        return { dispose: () => undefined };
      };

      registerNodeDocsCommand(
        context as unknown as vscode.ExtensionContext,
        () => reader as unknown as SqliteReader,
      );

      assert.ok(handler, "command handler should be registered");
      await handler!();

      assert.strictEqual(window.__createdWebviewPanels.length, 1);
      const panel = window.__createdWebviewPanels[0] as {
        title: string;
        webview: { html: string };
      };
      assert.strictEqual(panel.title, "doThing");
      assert.ok(panel.webview.html.includes("Authenticate a user."));
      assert.ok(!panel.webview.html.includes("<pre>"));
    } finally {
      vscode.commands.registerCommand = originalRegister;
      disposable.dispose();
    }
  });

  it("shows the fallback message when no docstring exists", async () => {
    const node = makeNode({});
    const reader = makeReader(node);
    vscode.window.activeTextEditor = makeEditor();

    const context = { extensionUri: vscode.Uri.file("/extension"), subscriptions: [] };
    let handler: (() => Promise<void>) | undefined;
    const originalRegister = vscode.commands.registerCommand;
    const disposable = originalRegister("dagayn.showNodeDocumentation", async () => {
      /* noop */
    });
    try {
      vscode.commands.registerCommand = (
        command: string,
        callback: (...args: unknown[]) => unknown,
      ) => {
        if (command === "dagayn.showNodeDocumentation") {
          handler = callback as () => Promise<void>;
        }
        return { dispose: () => undefined };
      };

      registerNodeDocsCommand(
        context as unknown as vscode.ExtensionContext,
        () => reader as unknown as SqliteReader,
      );

      assert.ok(handler);
      await handler!();

      assert.strictEqual(window.__createdWebviewPanels.length, 1);
      const panel = window.__createdWebviewPanels[0] as { webview: { html: string } };
      assert.ok(panel.webview.html.includes(NO_DOCUMENTATION_MESSAGE));
      // The fallback text is rendered as a paragraph, not inside a <pre> block.
      assert.ok(panel.webview.html.includes("<p>"));
    } finally {
      vscode.commands.registerCommand = originalRegister;
      disposable.dispose();
    }
  });
});

function makeNode(extra: Record<string, unknown>): GraphNode {
  return {
    id: 1,
    kind: "Function",
    name: "doThing",
    qualifiedName: "/workspace/a.py::doThing",
    filePath: "/workspace/a.py",
    lineStart: 10,
    lineEnd: 20,
    language: "python",
    parentName: null,
    params: null,
    returnType: null,
    modifiers: null,
    isTest: false,
    fileHash: null,
    extra,
  };
}

function makeReader(node: GraphNode): SqliteReader {
  return {
    getNodeAtCursor: () => node,
  } as unknown as SqliteReader;
}

function makeEditor() {
  return {
    document: {
      uri: vscode.Uri.file("/workspace/a.py"),
    },
    selection: {
      active: { line: 9, character: 0 },
    },
  } as unknown as typeof vscode.window.activeTextEditor;
}

describe("registerNodeHover", () => {
  beforeEach(() => {
    languages.__clearHoverProviders();
  });

  it("returns a hover with the docstring for a known node", () => {
    const node = makeNode({ docstring: "Authenticate a user." });
    const reader = makeReader(node);
    const context = { extensionUri: vscode.Uri.file("/extension"), subscriptions: [] };

    registerNodeHover(context as unknown as vscode.ExtensionContext, () => reader);

    assert.strictEqual(languages.__hoverProviders.length, 1);
    const hover = languages.__hoverProviders[0]!.provideHover(
      { uri: vscode.Uri.file("/workspace/a.py") },
      { line: 9 },
    ) as vscode.Hover | null;
    assert.ok(hover, "expected a hover result");
    const content = Array.isArray(hover.contents) ? hover.contents[0] : hover.contents;
    const md = content as vscode.MarkdownString;
    assert.ok(md.value.includes("Authenticate a user."));
  });

  it("returns null when the node has no documentation", () => {
    const node = makeNode({});
    const reader = makeReader(node);
    const context = { extensionUri: vscode.Uri.file("/extension"), subscriptions: [] };

    registerNodeHover(context as unknown as vscode.ExtensionContext, () => reader);

    const hover = languages.__hoverProviders[0]!.provideHover(
      { uri: vscode.Uri.file("/workspace/a.py") },
      { line: 9 },
    );
    assert.strictEqual(hover, null);
  });

  it("returns null without a reader", () => {
    const context = { extensionUri: vscode.Uri.file("/extension"), subscriptions: [] };

    registerNodeHover(context as unknown as vscode.ExtensionContext, () => undefined);

    const hover = languages.__hoverProviders[0]!.provideHover(
      { uri: vscode.Uri.file("/workspace/a.py") },
      { line: 9 },
    );
    assert.strictEqual(hover, null);
  });
});
