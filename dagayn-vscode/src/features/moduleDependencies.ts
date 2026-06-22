import * as vscode from "vscode";
import { WorkspaceGraphRegistry } from "../backend/registry";
import { pickFolderForGlobalOp } from "../backend/folderPicker";
import { GraphWebviewPanel } from "../views/graphWebview";
import {
  aggregateModules,
  DEFAULT_MODULE_EDGE_KINDS,
  type ModuleGraph,
} from "../backend/moduleAggregation";
import type { EdgeKind, SqliteReader } from "../backend/sqlite";

export function registerModuleDependenciesCommand(
  context: vscode.ExtensionContext,
  registry: WorkspaceGraphRegistry,
): void {
  context.subscriptions.push(
    vscode.commands.registerCommand("dagayn.showModuleDependencies", async () => {
      let reader: SqliteReader | undefined = registry.getReaderForActiveEditor();
      let folder: string | undefined;

      if (reader) {
        const activeEditor = vscode.window.activeTextEditor;
        folder = activeEditor
          ? vscode.workspace.getWorkspaceFolder(activeEditor.document.uri)?.uri.fsPath
          : undefined;
      } else {
        const folders = registry.foldersWithGraph();
        if (folders.length === 1) {
          folder = folders[0];
          reader = registry.getReaderForFolder(folder);
        } else if (folders.length > 1) {
          folder = await pickFolderForGlobalOp(registry, { requireGraph: true });
          reader = folder ? registry.getReaderForFolder(folder) : undefined;
        }
      }

      if (!reader || !folder) {
        vscode.window.showWarningMessage(
          "Code Graph: No graph database loaded. Run 'Code Graph: Build Graph' first.",
        );
        return;
      }

      const config = vscode.workspace.getConfiguration("dagayn");
      const defaultEdges = config.get<EdgeKind[]>("graph.defaultEdges", DEFAULT_MODULE_EDGE_KINDS);
      const allowedKinds = new Set(DEFAULT_MODULE_EDGE_KINDS);
      const kinds = defaultEdges.filter((k) => allowedKinds.has(k));
      const edgeKinds = kinds.length > 0 ? kinds : DEFAULT_MODULE_EDGE_KINDS;

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
