import * as assert from "node:assert";
import * as vscode from "vscode";
import { AutoUpdateController } from "../src/features/autoUpdate";
import { CliWrapper, CliResult, CliErrorKind } from "../src/backend/cli";
import { SqliteReader } from "../src/backend/sqlite";

const workspace = vscode.workspace as typeof vscode.workspace & {
  __resetConfigStore: () => void;
  __fireSave: (document: unknown) => void;
  __clearSaveCallbacks: () => void;
};
const window = vscode.window as typeof vscode.window & {
  __warningCalls: Array<{ message: string; buttons: unknown[] }>;
  __setWarningResult: (result: string | undefined) => void;
  __outputChannels: Map<string, string[]>;
};
const commands = vscode.commands as typeof vscode.commands & {
  __calls: Array<{ command: string; args: unknown[] }>;
};

function wait(ms = 10): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function fireSave(): Promise<void> {
  workspace.__fireSave({});
  await wait(0);
}

class FakeCli {
  results: Array<CliResult | (() => never)> = [];
  calls: string[] = [];
  index = 0;

  async updateGraph(workspaceRoot: string): Promise<CliResult> {
    this.calls.push(workspaceRoot);
    const next = this.results[this.index++];
    if (typeof next === "function") {
      next();
    }
    return next as CliResult;
  }
}

function failure(stderr: string, errorKind: CliErrorKind = "exec"): CliResult {
  return { success: false, stdout: "", stderr, errorKind };
}

function success(): CliResult {
  return { success: true, stdout: "ok", stderr: "" };
}

function createController(
  cli: CliWrapper,
  reader: SqliteReader | undefined = {} as SqliteReader,
): AutoUpdateController {
  const outputChannel = vscode.window.createOutputChannel("Code Graph");
  return new AutoUpdateController(
    cli,
    () => "/workspace",
    () => reader,
    outputChannel,
    0,
  );
}

function outputLines(): string[] {
  return window.__outputChannels.get("Code Graph") ?? [];
}

describe("AutoUpdateController", () => {
  beforeEach(() => {
    workspace.__resetConfigStore();
    workspace.__clearSaveCallbacks();
    window.__warningCalls.length = 0;
    commands.__calls.length = 0;
    window.__setWarningResult(undefined);
  });

  it("success resets counter", async () => {
    const cli = new FakeCli();
    cli.results = [failure("a"), failure("b"), success()];
    const controller = createController(cli as unknown as CliWrapper);

    await fireSave();
    await fireSave();
    await fireSave();
    await wait();
    assert.strictEqual(controller.failureCount, 0);
    assert.strictEqual(window.__warningCalls.length, 0);

    controller.dispose();
  });

  it("threshold triggers single notification", async () => {
    const cli = new FakeCli();
    cli.results = [failure("a"), failure("b"), failure("c")];
    const controller = createController(cli as unknown as CliWrapper);

    await fireSave();
    await fireSave();
    await fireSave();
    await wait();

    assert.strictEqual(controller.failureCount, 3);
    assert.strictEqual(window.__warningCalls.length, 1);
    const call = window.__warningCalls[0];
    assert.ok(call.message.includes("3 time(s) in a row"));
    assert.deepStrictEqual(call.buttons, ["Open Settings", "Disable Auto-Update", "Dismiss"]);

    await fireSave();
    await wait();
    assert.strictEqual(window.__warningCalls.length, 1);

    controller.dispose();
  });

  it("thrown error counts as failure", async () => {
    const cli = new FakeCli();
    cli.results = [
      () => {
        throw new Error("boom");
      },
    ];
    const controller = createController(cli as unknown as CliWrapper);

    await fireSave();
    await wait();

    assert.strictEqual(controller.failureCount, 1);
    const lines = outputLines();
    assert.ok(lines.some((line) => line.includes("Auto-update threw: boom")));

    controller.dispose();
  });

  it("result.success === false counts as failure", async () => {
    const cli = new FakeCli();
    cli.results = [failure("parse error", "exec")];
    const controller = createController(cli as unknown as CliWrapper);

    await fireSave();
    await wait();

    assert.strictEqual(controller.failureCount, 1);
    const lines = outputLines();
    assert.ok(lines.some((line) => line.includes("parse error")));

    controller.dispose();
  });

  it("recovery clears notifiedThisSession", async () => {
    const cli = new FakeCli();
    cli.results = [
      failure("a"),
      failure("b"),
      failure("c"),
      success(),
      failure("d"),
      failure("e"),
      failure("f"),
    ];
    const controller = createController(cli as unknown as CliWrapper);

    await fireSave();
    await fireSave();
    await fireSave();
    await wait();
    assert.strictEqual(window.__warningCalls.length, 1);

    await fireSave();
    await wait();
    assert.strictEqual(controller.failureCount, 0);

    await fireSave();
    await fireSave();
    await fireSave();
    await wait();
    assert.strictEqual(window.__warningCalls.length, 2);

    controller.dispose();
  });

  it('"Disable Auto-Update" button writes config', async () => {
    const cli = new FakeCli();
    cli.results = [failure("a"), failure("b"), failure("c")];
    window.__setWarningResult("Disable Auto-Update");
    const controller = createController(cli as unknown as CliWrapper);

    await fireSave();
    await fireSave();
    await fireSave();
    await wait();

    assert.strictEqual(window.__warningCalls.length, 1);
    const config = vscode.workspace.getConfiguration("dagayn");
    assert.strictEqual(config.get<boolean>("autoUpdate"), false);

    controller.dispose();
  });

  it('"Open Settings" button executes command', async () => {
    const cli = new FakeCli();
    cli.results = [failure("a"), failure("b"), failure("c")];
    window.__setWarningResult("Open Settings");
    const controller = createController(cli as unknown as CliWrapper);

    await fireSave();
    await fireSave();
    await fireSave();
    await wait();

    assert.strictEqual(window.__warningCalls.length, 1);
    const openSettingsCall = commands.__calls.find(
      (c) => c.command === "workbench.action.openSettings",
    );
    assert.ok(openSettingsCall);
    assert.deepStrictEqual(openSettingsCall.args, ["dagayn.autoUpdate"]);

    controller.dispose();
  });

  it("autoUpdate disabled skips entirely", async () => {
    const cli = new FakeCli();
    cli.results = [success()];
    await vscode.workspace
      .getConfiguration("dagayn")
      .update("autoUpdate", false, vscode.ConfigurationTarget.Workspace);
    const controller = createController(cli as unknown as CliWrapper);

    await fireSave();
    await wait();

    assert.deepStrictEqual(cli.calls, []);
    assert.strictEqual(controller.failureCount, 0);

    controller.dispose();
  });

  it("threshold of 1 notifies immediately", async () => {
    const cli = new FakeCli();
    cli.results = [failure("a")];
    await vscode.workspace
      .getConfiguration("dagayn")
      .update("autoUpdateFailureThreshold", 1, vscode.ConfigurationTarget.Workspace);
    const controller = createController(cli as unknown as CliWrapper);

    await fireSave();
    await wait();

    assert.strictEqual(window.__warningCalls.length, 1);
    assert.ok(window.__warningCalls[0].message.includes("1 time(s) in a row"));

    await fireSave();
    await wait();
    assert.strictEqual(window.__warningCalls.length, 1);

    controller.dispose();
  });
});
