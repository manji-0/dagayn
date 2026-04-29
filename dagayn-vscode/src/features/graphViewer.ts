import * as vscode from 'vscode';
import { SqliteReader } from '../backend/sqlite';
import { GraphWebviewPanel } from '../views/graphWebview';

export function registerGraphViewerCommands(
    context: vscode.ExtensionContext,
    getReader: () => SqliteReader | undefined,
): void {
    context.subscriptions.push(
        vscode.commands.registerCommand('dagayn.showGraph', async () => {
            const reader = getReader();
            if (!reader) {
                vscode.window.showWarningMessage(
                    "Code Graph: No graph database loaded. Run 'Code Graph: Build Graph' first.",
                );
                return;
            }
            GraphWebviewPanel.createOrShow(context.extensionUri, reader);
        }),
    );

    context.subscriptions.push(
        vscode.commands.registerCommand(
            'dagayn.revealInTree',
            (_qualifiedName: string) => {
                GraphWebviewPanel.highlightNode(_qualifiedName);
            },
        ),
    );
}
