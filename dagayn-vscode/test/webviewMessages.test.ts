import * as assert from "node:assert";
import { describe, it } from "mocha";
import { parseIncomingWebviewMessage } from "../src/views/webviewMessages";

describe("parseIncomingWebviewMessage", () => {
  it("parses a valid ready message", () => {
    const result = parseIncomingWebviewMessage({ command: "ready" });
    assert.deepStrictEqual(result, { command: "ready" });
  });

  it("parses a valid nodeClicked message", () => {
    const result = parseIncomingWebviewMessage({
      command: "nodeClicked",
      qualifiedName: "src/auth.py::login",
      filePath: "src/auth.py",
      lineStart: 10,
      kind: "Function",
    });
    assert.deepStrictEqual(result, {
      command: "nodeClicked",
      qualifiedName: "src/auth.py::login",
      filePath: "src/auth.py",
      lineStart: 10,
      kind: "Function",
    });
  });

  it("accepts nodeClicked without qualifiedName", () => {
    const result = parseIncomingWebviewMessage({
      command: "nodeClicked",
      filePath: "src/auth.py",
      lineStart: 10,
      kind: "Function",
    });
    assert.ok(result);
    assert.strictEqual(result?.command, "nodeClicked");
    assert.strictEqual((result as { filePath: string }).filePath, "src/auth.py");
  });

  it("accepts null or undefined lineStart", () => {
    assert.ok(
      parseIncomingWebviewMessage({
        command: "nodeClicked",
        filePath: "src/auth.py",
        lineStart: null,
        kind: "Function",
      }),
    );
    assert.ok(
      parseIncomingWebviewMessage({
        command: "nodeClicked",
        filePath: "src/auth.py",
        kind: "Function",
      }),
    );
  });

  it("parses exportSvg and exportPng messages", () => {
    assert.deepStrictEqual(parseIncomingWebviewMessage({ command: "exportSvg", svg: "<svg/>" }), {
      command: "exportSvg",
      svg: "<svg/>",
    });
    assert.deepStrictEqual(
      parseIncomingWebviewMessage({ command: "exportPng", data: "data:image/png;base64,abc" }),
      { command: "exportPng", data: "data:image/png;base64,abc" },
    );
  });

  it("rejects non-object payloads", () => {
    assert.strictEqual(parseIncomingWebviewMessage(null), null);
    assert.strictEqual(parseIncomingWebviewMessage("ready"), null);
    assert.strictEqual(parseIncomingWebviewMessage(42), null);
  });

  it("rejects messages without a command", () => {
    assert.strictEqual(parseIncomingWebviewMessage({}), null);
    assert.strictEqual(parseIncomingWebviewMessage({ command: 123 }), null);
  });

  it("rejects unknown commands", () => {
    assert.strictEqual(parseIncomingWebviewMessage({ command: "unknown" }), null);
  });

  it("rejects nodeClicked with wrong types", () => {
    assert.strictEqual(
      parseIncomingWebviewMessage({
        command: "nodeClicked",
        filePath: undefined,
        lineStart: 1,
        kind: "Function",
      }),
      null,
    );
    assert.strictEqual(
      parseIncomingWebviewMessage({
        command: "nodeClicked",
        filePath: "src/auth.py",
        lineStart: "abc",
        kind: "Function",
      }),
      null,
    );
    assert.strictEqual(
      parseIncomingWebviewMessage({
        command: "nodeClicked",
        filePath: "src/auth.py",
        lineStart: 1,
        kind: 123,
      }),
      null,
    );
  });

  it("rejects exportSvg and exportPng with wrong payload types", () => {
    assert.strictEqual(parseIncomingWebviewMessage({ command: "exportSvg", svg: 123 }), null);
    assert.strictEqual(parseIncomingWebviewMessage({ command: "exportPng", data: null }), null);
  });
});
