import * as vscode from 'vscode';
import { SqliteReader } from '../backend/sqlite';
import { BlastRadiusTreeProvider } from '../views/treeView';
import { resolveNodeAtCursor } from './cursorResolver';

export function registerBlastRadiusCommand(
    context: vscode.ExtensionContext,
    getReader: () => SqliteReader | undefined,
    getBlastRadiusProvider: () => BlastRadiusTreeProvider | undefined,
): void {
    context.subscriptions.push(
        vscode.commands.registerCommand('dagayn.showBlastRadius', async () => {
            const reader = getReader();
            if (!reader) {
                vscode.window.showWarningMessage('Code Graph: No graph database loaded.');
                return;
            }

            const editor = vscode.window.activeTextEditor;
            if (!editor) {
                vscode.window.showWarningMessage('Open a file first');
                return;
            }

            const absFilePath = editor.document.uri.fsPath;
            const cursorLine = editor.selection.active.line + 1;

            const nodeAtCursor = reader.getNodeAtCursor(absFilePath, cursorLine);
            const filePath = nodeAtCursor ? nodeAtCursor.filePath : absFilePath;

            const config = vscode.workspace.getConfiguration('dagayn');
            const depth = config.get<number>('blastRadiusDepth', 2);

            const impact = reader.getImpactRadius([filePath], depth);

            const provider = getBlastRadiusProvider();
            if (provider) {
                provider.setResults(impact.changedNodes, impact.impactedNodes);
            }

            await vscode.commands.executeCommand('dagayn.blastRadius.focus');

            const impactedFileCount = new Set(impact.impactedNodes.map((n) => n.filePath)).size;
            vscode.window.showInformationMessage(
                `Blast radius: ${impact.impactedNodes.length} nodes impacted across ${impactedFileCount} files`,
            );
        }),
    );
}
