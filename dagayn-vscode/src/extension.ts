import * as vscode from "vscode";
import { CliWrapper } from "./backend/cli";
import { WorkspaceGraphRegistry } from "./backend/registry";
import { findGraphDb } from "./backend/paths";
import {
  CodeGraphTreeProvider,
  BlastRadiusTreeProvider,
  StatsTreeProvider,
} from "./views/treeView";
import { Installer } from "./onboarding/installer";
import { registerWalkthroughCommands, showWelcomeIfNeeded } from "./onboarding/welcome";
import { StatusBar } from "./views/statusBar";
import { ScmDecorationProvider } from "./features/scmDecorations";
import { registerGraphLifecycleCommands } from "./features/graphLifecycle";
import { registerGraphViewerCommands } from "./features/graphViewer";
import { registerModuleDependenciesCommand } from "./features/moduleDependencies";
import { registerBlastRadiusCommand } from "./features/blastRadius";
import { registerBlastRadiusSnapshotCommands } from "./features/blastRadiusSnapshot";
import { registerNavigationCommands } from "./features/navigation";
import { registerNodeDocsCommand } from "./features/nodeDocs";
import { registerSearchCommand } from "./features/search";
import { registerReviewCommand } from "./features/reviewAssistant";
import { AutoUpdateController } from "./features/autoUpdate";

let registry: WorkspaceGraphRegistry | undefined;
let codeGraphProvider: CodeGraphTreeProvider | undefined;
let blastRadiusProvider: BlastRadiusTreeProvider | undefined;
let statsProvider: StatsTreeProvider | undefined;
let statusBar: StatusBar | undefined;
let scmDecorationProvider: ScmDecorationProvider | undefined;

function ensureProviders(context: vscode.ExtensionContext): void {
  if (codeGraphProvider) {
    return;
  }

  codeGraphProvider = new CodeGraphTreeProvider(() => registry);
  blastRadiusProvider = new BlastRadiusTreeProvider();
  statsProvider = new StatsTreeProvider(() => registry);

  context.subscriptions.push(
    vscode.window.registerTreeDataProvider("dagayn.codeGraph", codeGraphProvider),
    vscode.window.registerTreeDataProvider("dagayn.blastRadius", blastRadiusProvider),
    vscode.window.registerTreeDataProvider("dagayn.stats", statsProvider),
  );

  statusBar = new StatusBar();
  statusBar.update(registry?.getReaderForActiveEditor() ?? registry?.getAllReaders()[0]?.reader);
  statusBar.show();
  context.subscriptions.push(statusBar);

  scmDecorationProvider = new ScmDecorationProvider();
  context.subscriptions.push(vscode.window.registerFileDecorationProvider(scmDecorationProvider));
}

function onReaderChanged(context: vscode.ExtensionContext): void {
  if (registry && registry.foldersWithGraph().length > 0) {
    ensureProviders(context);
  }
  codeGraphProvider?.refresh();
  statsProvider?.refresh();
  statusBar?.update(registry?.getReaderForActiveEditor());
}

async function reinitializeFolder(
  context: vscode.ExtensionContext,
  folderFsPath: string,
): Promise<void> {
  if (!registry) {
    return;
  }
  await registry.reinitializeFolder(folderFsPath);
  onReaderChanged(context);
}

function registerCommands(context: vscode.ExtensionContext, cli: CliWrapper): void {
  if (!registry) {
    return;
  }

  context.subscriptions.push(
    vscode.commands.registerCommand("dagayn.codeGraph.refresh", () => {
      onReaderChanged(context);
    }),
  );

  registerGraphLifecycleCommands(context, cli, registry, reinitializeFolder);
  registerGraphViewerCommands(context, registry);
  registerModuleDependenciesCommand(context, registry);
  registerBlastRadiusCommand(
    context,
    () => registry?.getReaderForActiveEditor(),
    () => blastRadiusProvider,
  );
  registerBlastRadiusSnapshotCommands(context, registry, () => blastRadiusProvider);
  registerNavigationCommands(context, () => registry?.getReaderForActiveEditor());
  registerNodeDocsCommand(context, () => registry?.getReaderForActiveEditor());
  registerSearchCommand(context, registry);
  registerReviewCommand(context, registry, () => scmDecorationProvider);
}

function watchGraphDb(context: vscode.ExtensionContext): void {
  const watcher = vscode.workspace.createFileSystemWatcher("**/.dagayn/graph.db");

  watcher.onDidChange(async (uri) => {
    if (!registry || !uri) {
      return;
    }
    const folder = vscode.workspace.getWorkspaceFolder(uri);
    if (!folder) {
      return;
    }
    try {
      await registry.reinitializeFolder(folder.uri.fsPath);
    } catch (err) {
      console.error(`[dagayn] Failed to reinitialize graph for ${folder.uri.fsPath}:`, err);
    }
    onReaderChanged(context);
  });

  watcher.onDidCreate(async (uri) => {
    if (!registry || !uri) {
      return;
    }
    const folder = vscode.workspace.getWorkspaceFolder(uri);
    if (!folder) {
      return;
    }
    if (!registry.getReaderForFolder(folder.uri.fsPath)) {
      try {
        await registry.reinitializeFolder(folder.uri.fsPath);
      } catch (err) {
        console.error(`[dagayn] Failed to open new graph for ${folder.uri.fsPath}:`, err);
      }
    }
    onReaderChanged(context);
  });

  watcher.onDidDelete(async (uri) => {
    if (!registry || !uri) {
      return;
    }
    const folder = vscode.workspace.getWorkspaceFolder(uri);
    if (!folder) {
      return;
    }
    if (!findGraphDb(folder.uri.fsPath)) {
      registry.closeFolder(folder.uri.fsPath);
    }
    onReaderChanged(context);
  });

  context.subscriptions.push(watcher);
}

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  const cli = new CliWrapper();
  const installer = new Installer(cli);

  registerWalkthroughCommands(context, installer);

  registry = new WorkspaceGraphRegistry();

  registerCommands(context, cli);

  const foldersWithGraph = registry.foldersWithGraph();
  if (foldersWithGraph.length > 0) {
    // Collect distinct schema warnings and the folders that produced them.
    const warnings = new Map<string, string[]>();
    for (const folder of foldersWithGraph) {
      const reader = registry.getReaderForFolder(folder);
      const warning = reader?.checkSchemaCompatibility();
      if (warning) {
        const folders = warnings.get(warning) ?? [];
        folders.push(folder);
        warnings.set(warning, folders);
      }
    }

    for (const [warning, affectedFolders] of warnings) {
      const choice = await vscode.window.showWarningMessage(
        affectedFolders.length > 1
          ? `Code Graph: ${warning} (${affectedFolders.length} folders)`
          : `Code Graph: ${warning}`,
        "Rebuild Graph",
        "Dismiss",
      );
      if (choice === "Rebuild Graph") {
        for (const folder of affectedFolders) {
          const result = await cli.buildGraph(folder);
          if (result.success) {
            await reinitializeFolder(context, folder);
          }
        }
      }
    }

    onReaderChanged(context);
  } else {
    showWelcomeIfNeeded(context);
  }

  watchGraphDb(context);

  const outputChannel = vscode.window.createOutputChannel("Code Graph");
  context.subscriptions.push(outputChannel);
  const autoUpdate = new AutoUpdateController(
    cli,
    registry,
    outputChannel,
    undefined,
    (folderFsPath) => reinitializeFolder(context, folderFsPath),
  );
  context.subscriptions.push(autoUpdate);

  context.subscriptions.push(
    vscode.window.onDidChangeActiveTextEditor(() => {
      statusBar?.update(
        registry?.getReaderForActiveEditor() ?? registry?.getAllReaders()[0]?.reader,
      );
    }),
  );
}

export function deactivate(): void {
  registry?.dispose();
  registry = undefined;
}

/** @internal — test-only: access the active registry. */
export function __getRegistryForTests(): WorkspaceGraphRegistry | undefined {
  return registry;
}

/** @internal — test-only: reset module-level singletons between tests. */
export function __resetForTests(): void {
  registry?.dispose();
  registry = undefined;
  codeGraphProvider = undefined;
  blastRadiusProvider = undefined;
  statsProvider = undefined;
  statusBar = undefined;
  scmDecorationProvider = undefined;
}
