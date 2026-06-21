import * as vscode from "vscode";
import { CliWrapper, CliResult } from "../backend/cli";
import { SqliteReader } from "../backend/sqlite";

export const DEFAULT_FAILURE_THRESHOLD = 3;
const AUTO_UPDATE_DEBOUNCE_MS = 2000;

export class AutoUpdateController implements vscode.Disposable {
  private timer: ReturnType<typeof setTimeout> | undefined;
  private consecutiveFailures = 0;
  private notifiedThisSession = false;
  private readonly disposables: vscode.Disposable[] = [];
  private disposed = false;
  private readonly debounceMs: number;

  constructor(
    private readonly cli: CliWrapper,
    private readonly getWorkspaceRoot: () => string | undefined,
    private readonly getReader: () => SqliteReader | undefined,
    private readonly outputChannel: vscode.OutputChannel,
    debounceMs?: number,
  ) {
    this.debounceMs = debounceMs ?? AUTO_UPDATE_DEBOUNCE_MS;
    const saveListener = vscode.workspace.onDidSaveTextDocument(() => {
      this.scheduleUpdate();
    });
    this.disposables.push(saveListener);
  }

  get failureCount(): number {
    return this.consecutiveFailures;
  }

  private scheduleUpdate(): void {
    if (this.disposed) {
      return;
    }

    const config = vscode.workspace.getConfiguration("dagayn");
    if (!config.get<boolean>("autoUpdate", true)) {
      return;
    }

    if (this.timer) {
      clearTimeout(this.timer);
    }

    this.timer = setTimeout(() => {
      void this.runUpdate();
    }, this.debounceMs);
  }

  private async runUpdate(): Promise<void> {
    const wsRoot = this.getWorkspaceRoot();
    if (!wsRoot || !this.getReader()) {
      return;
    }

    let result: CliResult;
    try {
      result = await this.cli.updateGraph(wsRoot);
    } catch (err) {
      if (!this.disposed) {
        this.handleFailure(`Auto-update threw: ${this.toMessage(err)}`);
      }
      return;
    }

    if (this.disposed) {
      return;
    }

    if (result.success) {
      this.handleSuccess();
    } else {
      this.handleFailure(result.stderr || `Auto-update failed (${result.errorKind})`);
    }
  }

  private handleSuccess(): void {
    this.consecutiveFailures = 0;
    this.notifiedThisSession = false;
  }

  private handleFailure(detail: string): void {
    this.consecutiveFailures += 1;

    const timestamp = new Date().toISOString();
    this.outputChannel.appendLine(`[${timestamp}] ${detail}`);

    const threshold = vscode.workspace
      .getConfiguration("dagayn")
      .get<number>("autoUpdateFailureThreshold", DEFAULT_FAILURE_THRESHOLD);

    if (this.consecutiveFailures >= threshold && !this.notifiedThisSession) {
      this.notifiedThisSession = true;
      void this.notify();
    }
  }

  private async notify(): Promise<void> {
    const choice = await vscode.window.showWarningMessage(
      `Code Graph: Auto-update has failed ${this.consecutiveFailures} time(s) in a row. See "Code Graph" output for details.`,
      "Open Settings",
      "Disable Auto-Update",
      "Dismiss",
    );

    if (choice === "Open Settings") {
      await vscode.commands.executeCommand("workbench.action.openSettings", "dagayn.autoUpdate");
    } else if (choice === "Disable Auto-Update") {
      await vscode.workspace
        .getConfiguration("dagayn")
        .update("autoUpdate", false, vscode.ConfigurationTarget.Workspace);
    }
  }

  private toMessage(err: unknown): string {
    return err instanceof Error ? err.message : String(err);
  }

  dispose(): void {
    if (this.disposed) {
      return;
    }
    this.disposed = true;
    if (this.timer) {
      clearTimeout(this.timer);
      this.timer = undefined;
    }
    for (const d of this.disposables) {
      d.dispose();
    }
  }
}
