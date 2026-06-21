/**
 * SCM file decoration provider.
 *
 * Adds badges to files in the Explorer and SCM views:
 *  - IMPACTED (orange) — file is in the blast radius of staged/unstaged changes
 *  - TESTED (green) — changed functions in this file have test coverage
 *  - UNTESTED (red) — changed functions lack test coverage
 */

import * as vscode from "vscode";
import { SqliteReader } from "../backend/sqlite";

type FileClassification =
  | { kind: "changed-tested" }
  | { kind: "changed-untested" }
  | { kind: "impacted" };

export class ScmDecorationProvider implements vscode.FileDecorationProvider {
  private readonly _onDidChange = new vscode.EventEmitter<vscode.Uri | vscode.Uri[] | undefined>();
  readonly onDidChangeFileDecorations = this._onDidChange.event;

  private classifications = new Map<string, FileClassification>();

  /**
   * Recompute decorations from git state and the graph database.
   */
  async update(reader: SqliteReader, workspaceRoot: string): Promise<void> {
    const { execFile } = await import("node:child_process");
    const { promisify } = await import("node:util");
    const execFileAsync = promisify(execFile);
    const path = await import("node:path");

    // 1. Collect changed files
    let unstaged: string[] = [];
    let staged: string[] = [];
    try {
      const r1 = await execFileAsync("git", ["diff", "--name-only", "HEAD"], {
        cwd: workspaceRoot,
        timeout: 10_000,
      });
      unstaged = r1.stdout.trim().split("\n").filter(Boolean);
    } catch {
      /* ignore */
    }
    try {
      const r2 = await execFileAsync("git", ["diff", "--cached", "--name-only"], {
        cwd: workspaceRoot,
        timeout: 10_000,
      });
      staged = r2.stdout.trim().split("\n").filter(Boolean);
    } catch {
      /* ignore */
    }

    const changedRelative = [...new Set([...unstaged, ...staged])];
    const changedAbsolute = changedRelative.map((f) => path.join(workspaceRoot, f));

    // 2. Compute impact radius
    const config = vscode.workspace.getConfiguration("dagayn");
    const depth = config.get<number>("blastRadiusDepth", 2);
    const impact = reader.getImpactRadius(changedAbsolute, depth);

    // 3. Reset classifications
    this.classifications.clear();

    // 4. Test coverage classification for changed files
    for (const filePath of changedAbsolute) {
      const nodes = reader.getNodesByFile(filePath);
      const functions = nodes.filter((n) => n.kind === "Function" && !n.isTest);
      if (functions.length === 0) {
        continue;
      }

      let allTested = true;
      for (const fn of functions) {
        const edges = reader.getEdgesByTarget(fn.qualifiedName);
        const hasTest = edges.some((e) => e.kind === "TESTED_BY");
        if (!hasTest) {
          // Also check outgoing TESTED_BY (reverse direction)
          const outEdges = reader.getEdgesBySource(fn.qualifiedName);
          const hasOutTest = outEdges.some((e) => e.kind === "TESTED_BY");
          if (!hasOutTest) {
            allTested = false;
            break;
          }
        }
      }

      this.classifications.set(
        filePath,
        allTested ? { kind: "changed-tested" } : { kind: "changed-untested" },
      );
    }

    // 5. Classify impacted files (excluding files already classified as changed)
    for (const impactedFile of impact.impactedNodes.map((n) => n.filePath)) {
      if (!this.classifications.has(impactedFile)) {
        this.classifications.set(impactedFile, { kind: "impacted" });
      }
    }

    // 6. Fire change event
    this._onDidChange.fire(undefined);
  }

  /** Clear all decorations. */
  clear(): void {
    this.classifications.clear();
    this._onDidChange.fire(undefined);
  }

  provideFileDecoration(uri: vscode.Uri): vscode.FileDecoration | undefined {
    const filePath = uri.fsPath;
    const classification = this.classifications.get(filePath);
    if (!classification) {
      return undefined;
    }

    switch (classification.kind) {
      case "changed-untested":
        return {
          badge: "!",
          color: new vscode.ThemeColor("editorError.foreground"),
          tooltip: "Code Graph: Changed functions lack test coverage",
          propagate: false,
        };
      case "changed-tested":
        return {
          badge: "\u2713",
          color: new vscode.ThemeColor("testing.iconPassed"),
          tooltip: "Code Graph: All changed functions have test coverage",
          propagate: false,
        };
      case "impacted":
        return {
          badge: "\u25CF",
          color: new vscode.ThemeColor("editorWarning.foreground"),
          tooltip: "Code Graph: In blast radius of current changes",
          propagate: false,
        };
      default: {
        const _exhaustive: never = classification;
        return _exhaustive;
      }
    }
  }

  dispose(): void {
    this._onDidChange.dispose();
  }
}
