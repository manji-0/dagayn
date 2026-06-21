import * as vscode from "vscode";
import * as path from "node:path";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { WorkspaceGraphRegistry } from "../backend/registry";
import { pickFolderForGlobalOp } from "../backend/folderPicker";
import { GraphWebviewPanel } from "../views/graphWebview";
import { ScmDecorationProvider } from "./scmDecorations";

const execFileAsync = promisify(execFile);
const GIT_TIMEOUT_MS = 10_000;

async function gitLines(args: string[], cwd: string): Promise<string[]> {
  try {
    const { stdout } = await execFileAsync("git", args, { cwd, timeout: GIT_TIMEOUT_MS });
    return stdout
      .trim()
      .split("\n")
      .filter((l) => l.length > 0);
  } catch {
    return [];
  }
}

export function registerReviewCommand(
  context: vscode.ExtensionContext,
  registry: WorkspaceGraphRegistry,
  getScmProvider: () => ScmDecorationProvider | undefined,
): void {
  context.subscriptions.push(
    vscode.commands.registerCommand("dagayn.reviewChanges", async () => {
      const folder = await pickFolderForGlobalOp(registry, { requireGraph: true });
      if (!folder) {
        vscode.window.showWarningMessage("Code Graph: No graph database loaded.");
        return;
      }

      const reader = registry.getReaderForFolder(folder);
      if (!reader) {
        vscode.window.showWarningMessage(`Code Graph: No graph database loaded for ${folder}.`);
        return;
      }

      await vscode.window.withProgress(
        {
          location: vscode.ProgressLocation.Notification,
          title: "Code Graph: Analyzing changes...",
          cancellable: false,
        },
        async () => {
          const [unstaged, staged] = await Promise.all([
            gitLines(["diff", "--name-only"], folder),
            gitLines(["diff", "--cached", "--name-only"], folder),
          ]);

          const changedFiles = [...new Set([...unstaged, ...staged])];

          if (changedFiles.length === 0) {
            vscode.window.showInformationMessage("Code Graph: No changes detected.");
            return;
          }

          const absFiles = changedFiles.map((f) => path.join(folder, f));
          const impact = reader.getImpactRadius(absFiles);

          const guidance: string[] = [];
          const impactedFileCount = new Set(impact.impactedNodes.map((n) => n.filePath)).size;

          // Test coverage check
          const untestedFns: string[] = [];
          for (const node of impact.changedNodes) {
            if (node.kind !== "Function" || node.isTest) {
              continue;
            }
            const inEdges = reader.getEdgesByTarget(node.qualifiedName);
            if (inEdges.some((e) => e.kind === "TESTED_BY")) {
              continue;
            }
            const outEdges = reader.getEdgesBySource(node.qualifiedName);
            if (!outEdges.some((e) => e.kind === "TESTED_BY")) {
              untestedFns.push(node.name);
            }
          }
          if (untestedFns.length > 0) {
            guidance.push(
              `⚠️ **${untestedFns.length} changed function(s) lack test coverage**: ${untestedFns.slice(0, 5).join(", ")}${untestedFns.length > 5 ? "..." : ""}`,
            );
          }

          if (impactedFileCount > 10) {
            guidance.push(
              `⚠️ **Wide blast radius**: ${impactedFileCount} files impacted — consider splitting this change.`,
            );
          }

          const inheritanceChanges = impact.edges.filter(
            (e) => e.kind === "INHERITS" || e.kind === "IMPLEMENTS",
          );
          if (inheritanceChanges.length > 0) {
            guidance.push(
              `⚠️ **Inheritance chain affected**: ${inheritanceChanges.length} inheritance/implementation edge(s) touched.`,
            );
          }

          if (impact.impactedNodes.length > 0) {
            guidance.push(
              `ℹ️ ${impact.impactedNodes.length} nodes in ${impactedFileCount} file(s) may be affected by these changes.`,
            );
          }

          const channel = vscode.window.createOutputChannel("Code Graph Review", { log: true });
          channel.appendLine("# Review Guidance");
          channel.appendLine("");
          channel.appendLine(`Changed files: ${changedFiles.length}`);
          channel.appendLine(`Changed nodes: ${impact.changedNodes.length}`);
          channel.appendLine(`Impacted nodes: ${impact.impactedNodes.length}`);
          channel.appendLine(`Impacted files: ${impactedFileCount}`);
          channel.appendLine("");
          if (guidance.length > 0) {
            for (const g of guidance) {
              channel.appendLine(g);
            }
          } else {
            channel.appendLine("✅ No concerns detected.");
          }
          channel.show(true);

          GraphWebviewPanel.createOrShow(context.extensionUri, reader, folder, impact);

          const scmProvider = getScmProvider();
          if (scmProvider) {
            await scmProvider.update(reader, folder);
          }
        },
      );
    }),
  );
}
