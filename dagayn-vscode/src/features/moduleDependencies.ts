import * as vscode from "vscode";
import { WorkspaceGraphRegistry } from "../backend/registry";
import { resolveReaderAndFolder } from "../backend/readerResolver";
import { GraphWebviewPanel } from "../views/graphWebview";
import {
  aggregateModules,
  DEFAULT_MODULE_EDGE_KINDS,
  type ModuleGraph,
} from "../backend/moduleAggregation";

export function registerModuleDependenciesCommand(
  context: vscode.ExtensionContext,
  registry: WorkspaceGraphRegistry,
): void {
  context.subscriptions.push(
    vscode.commands.registerCommand("dagayn.showModuleDependencies", async () => {
      const resolved = await resolveReaderAndFolder(registry);
      if (!resolved) {
        vscode.window.showWarningMessage(
          "Code Graph: No graph database loaded. Run 'Code Graph: Build Graph' first.",
        );
        return;
      }
      const { reader, folder } = resolved;

      // Module edges are aggregated from a fixed set of directory-relevant
      // kinds; `dagayn.graph.defaultEdges` governs the symbol graph view only.
      const edgeKinds = DEFAULT_MODULE_EDGE_KINDS;

      const graph: ModuleGraph = aggregateModules(reader, edgeKinds);

      if (graph.nodes.length === 0) {
        vscode.window.showWarningMessage("Code Graph: No modules to show.");
        return;
      }

      GraphWebviewPanel.createOrShowModule(context.extensionUri, reader, folder, graph);

      vscode.window.showInformationMessage(
        `Code Graph: Module dependencies: ${graph.nodes.length} modules, ${graph.edges.length} edges`,
      );
    }),
  );
}
