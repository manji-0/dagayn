import * as vscode from "vscode";
import * as path from "node:path";
import * as fs from "node:fs";

import { SqliteReader } from "./backend/sqlite";
import { CliWrapper } from "./backend/cli";
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
import { registerBlastRadiusCommand } from "./features/blastRadius";
import { registerNavigationCommands } from "./features/navigation";
import { registerSearchCommand } from "./features/search";
import { registerReviewCommand } from "./features/reviewAssistant";

let sqliteReader: SqliteReader | undefined;
let codeGraphProvider: CodeGraphTreeProvider | undefined;
let blastRadiusProvider: BlastRadiusTreeProvider | undefined;
let statsProvider: StatsTreeProvider | undefined;
let statusBar: StatusBar | undefined;
let autoUpdateTimer: ReturnType<typeof setTimeout> | undefined;
let scmDecorationProvider: ScmDecorationProvider | undefined;

function findGraphDb(workspaceRoot: string): string | undefined {
  const primary = path.join(workspaceRoot, ".dagayn", "graph.db");
  if (fs.existsSync(primary)) {
    return primary;
  }
  const fallback = path.join(workspaceRoot, ".dagayn.db");
  if (fs.existsSync(fallback)) {
    return fallback;
  }
  return undefined;
}

function getWorkspaceRoot(): string | undefined {
  const folders = vscode.workspace.workspaceFolders;
  if (!folders) {
    return undefined;
  }
  for (const folder of folders) {
    if (findGraphDb(folder.uri.fsPath)) {
      return folder.uri.fsPath;
    }
  }
  return folders[0]?.uri.fsPath;
}

function ensureProviders(context: vscode.ExtensionContext, workspaceRoot: string): void {
  if (codeGraphProvider) {
    return;
  }

  codeGraphProvider = new CodeGraphTreeProvider(() => sqliteReader, workspaceRoot);
  blastRadiusProvider = new BlastRadiusTreeProvider();
  statsProvider = new StatsTreeProvider(() => sqliteReader);

  context.subscriptions.push(
    vscode.window.registerTreeDataProvider("dagayn.codeGraph", codeGraphProvider),
    vscode.window.registerTreeDataProvider("dagayn.blastRadius", blastRadiusProvider),
    vscode.window.registerTreeDataProvider("dagayn.stats", statsProvider),
  );

  statusBar = new StatusBar();
  statusBar.update(sqliteReader);
  statusBar.show();
  context.subscriptions.push(statusBar);

  scmDecorationProvider = new ScmDecorationProvider();
  context.subscriptions.push(vscode.window.registerFileDecorationProvider(scmDecorationProvider));
}

function onReaderChanged(context: vscode.ExtensionContext): void {
  const workspaceRoot = getWorkspaceRoot();
  if (workspaceRoot) {
    ensureProviders(context, workspaceRoot);
  }
  codeGraphProvider?.refresh();
  statsProvider?.refresh();
  statusBar?.update(sqliteReader);
}

async function reinitialize(context: vscode.ExtensionContext): Promise<void> {
  const workspaceRoot = getWorkspaceRoot();
  if (!workspaceRoot) {
    return;
  }
  const dbPath = findGraphDb(workspaceRoot);
  if (!dbPath) {
    return;
  }
  sqliteReader?.close();
  sqliteReader = new SqliteReader(dbPath);
  onReaderChanged(context);
}

function registerCommands(context: vscode.ExtensionContext, cli: CliWrapper): void {
  context.subscriptions.push(
    vscode.commands.registerCommand("dagayn.codeGraph.refresh", () => {
      onReaderChanged(context);
    }),
  );

  registerGraphLifecycleCommands(context, cli, getWorkspaceRoot, reinitialize);
  registerBlastRadiusCommand(
    context,
    () => sqliteReader,
    () => blastRadiusProvider,
  );
  registerNavigationCommands(context, () => sqliteReader);
  registerSearchCommand(context, () => sqliteReader);
  registerReviewCommand(
    context,
    () => sqliteReader,
    getWorkspaceRoot,
    () => scmDecorationProvider,
  );
  registerGraphViewerCommands(context, () => sqliteReader);
}

function watchGraphDb(context: vscode.ExtensionContext): void {
  const watcher = vscode.workspace.createFileSystemWatcher("**/.dagayn/graph.db");

  const dbPathRef = { current: "" };
  const workspaceRoot = getWorkspaceRoot();
  if (workspaceRoot) {
    const dbPath = findGraphDb(workspaceRoot);
    if (dbPath) {
      dbPathRef.current = dbPath;
    }
  }

  watcher.onDidChange(() => {
    if (sqliteReader && dbPathRef.current) {
      sqliteReader.close();
      sqliteReader = new SqliteReader(dbPathRef.current);
      onReaderChanged(context);
    }
  });

  watcher.onDidCreate(async () => {
    const wsRoot = getWorkspaceRoot();
    if (wsRoot && !sqliteReader) {
      const dbPath = findGraphDb(wsRoot);
      if (dbPath) {
        dbPathRef.current = dbPath;
        sqliteReader = new SqliteReader(dbPath);
        onReaderChanged(context);
      }
    }
  });

  watcher.onDidDelete(() => {
    sqliteReader?.close();
    sqliteReader = undefined;
    dbPathRef.current = "";
    codeGraphProvider?.refresh();
    statsProvider?.refresh();
    statusBar?.update(undefined);
  });

  context.subscriptions.push(watcher);
}

function setupAutoUpdate(context: vscode.ExtensionContext, cli: CliWrapper): void {
  const AUTO_UPDATE_DEBOUNCE_MS = 2000;

  const onSave = vscode.workspace.onDidSaveTextDocument(() => {
    const config = vscode.workspace.getConfiguration("dagayn");
    if (!config.get<boolean>("autoUpdate", true)) {
      return;
    }

    if (autoUpdateTimer) {
      clearTimeout(autoUpdateTimer);
    }

    autoUpdateTimer = setTimeout(async () => {
      const wsRoot = getWorkspaceRoot();
      if (!wsRoot || !sqliteReader) {
        return;
      }
      try {
        await cli.updateGraph(wsRoot);
      } catch {
        // Silently ignore update errors on save
      }
    }, AUTO_UPDATE_DEBOUNCE_MS);
  });

  context.subscriptions.push(onSave);
}

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  const cli = new CliWrapper();
  const installer = new Installer(cli);

  registerWalkthroughCommands(context, installer);

  const workspaceRoot = getWorkspaceRoot();

  registerCommands(context, cli);

  if (workspaceRoot) {
    const dbPath = findGraphDb(workspaceRoot);

    if (dbPath) {
      sqliteReader = new SqliteReader(dbPath);

      const schemaWarning = sqliteReader.checkSchemaCompatibility();
      if (schemaWarning) {
        const choice = await vscode.window.showWarningMessage(
          `Code Graph: ${schemaWarning}`,
          "Rebuild Graph",
          "Dismiss",
        );
        if (choice === "Rebuild Graph") {
          await vscode.commands.executeCommand("dagayn.buildGraph");
        }
      }

      onReaderChanged(context);
    } else {
      showWelcomeIfNeeded(context);
    }
  }

  watchGraphDb(context);
  setupAutoUpdate(context, cli);
}

export function deactivate(): void {
  if (autoUpdateTimer) {
    clearTimeout(autoUpdateTimer);
    autoUpdateTimer = undefined;
  }
  sqliteReader?.close();
  sqliteReader = undefined;
}
