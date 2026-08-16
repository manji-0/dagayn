"use strict";
const fs = require("fs");
/**
 * Minimal vscode API stub for running unit tests under plain Node.
 *
 * Only implements the surface used by src/ modules imported during tests.
 */
Object.defineProperty(exports, "__esModule", { value: true });
exports.FileDecoration = exports.env = exports.commands = exports.workspace = exports.window = exports.ConfigurationTarget = exports.RelativePattern = exports.EventEmitter = exports.TreeItem = exports.ThemeColor = exports.Position = exports.Range = exports.Uri = exports.FileType = exports.ColorThemeKind = exports.ViewColumn = exports.ProgressLocation = exports.StatusBarAlignment = exports.TreeItemCollapsibleState = void 0;
var TreeItemCollapsibleState;
(function (TreeItemCollapsibleState) {
    TreeItemCollapsibleState[TreeItemCollapsibleState["None"] = 0] = "None";
    TreeItemCollapsibleState[TreeItemCollapsibleState["Collapsed"] = 1] = "Collapsed";
    TreeItemCollapsibleState[TreeItemCollapsibleState["Expanded"] = 2] = "Expanded";
})(TreeItemCollapsibleState || (exports.TreeItemCollapsibleState = TreeItemCollapsibleState = {}));
var StatusBarAlignment;
(function (StatusBarAlignment) {
    StatusBarAlignment[StatusBarAlignment["Left"] = 1] = "Left";
    StatusBarAlignment[StatusBarAlignment["Right"] = 2] = "Right";
})(StatusBarAlignment || (exports.StatusBarAlignment = StatusBarAlignment = {}));
var ProgressLocation;
(function (ProgressLocation) {
    ProgressLocation[ProgressLocation["Notification"] = 15] = "Notification";
    ProgressLocation[ProgressLocation["Window"] = 10] = "Window";
})(ProgressLocation || (exports.ProgressLocation = ProgressLocation = {}));
var ViewColumn;
(function (ViewColumn) {
    ViewColumn[ViewColumn["One"] = 1] = "One";
    ViewColumn[ViewColumn["Beside"] = -2] = "Beside";
})(ViewColumn || (exports.ViewColumn = ViewColumn = {}));
var ColorThemeKind;
(function (ColorThemeKind) {
    ColorThemeKind[ColorThemeKind["Light"] = 1] = "Light";
    ColorThemeKind[ColorThemeKind["Dark"] = 2] = "Dark";
    ColorThemeKind[ColorThemeKind["HighContrast"] = 3] = "HighContrast";
    ColorThemeKind[ColorThemeKind["HighContrastLight"] = 4] = "HighContrastLight";
})(ColorThemeKind || (exports.ColorThemeKind = ColorThemeKind = {}));
var FileType;
(function (FileType) {
    FileType[FileType["Unknown"] = 0] = "Unknown";
    FileType[FileType["File"] = 1] = "File";
    FileType[FileType["Directory"] = 2] = "Directory";
    FileType[FileType["SymbolicLink"] = 64] = "SymbolicLink";
})(FileType || (exports.FileType = FileType = {}));
var ConfigurationTarget;
(function (ConfigurationTarget) {
    ConfigurationTarget[ConfigurationTarget["Global"] = 1] = "Global";
    ConfigurationTarget[ConfigurationTarget["Workspace"] = 3] = "Workspace";
    ConfigurationTarget[ConfigurationTarget["WorkspaceFolder"] = 4] = "WorkspaceFolder";
})(ConfigurationTarget || (exports.ConfigurationTarget = ConfigurationTarget = {}));
class Uri {
    scheme = "file";
    authority = "";
    path = "";
    query = "";
    fragment = "";
    fsPath = "";
    static file(path) {
        const u = new Uri();
        u.path = path;
        u.fsPath = path;
        return u;
    }
    static parse(value) {
        const u = new Uri();
        u.path = value;
        u.fsPath = value;
        return u;
    }
    static joinPath(base, ...pathSegments) {
        return Uri.file([base.fsPath, ...pathSegments].join("/"));
    }
}
exports.Uri = Uri;
class Range {
    startLine;
    startCharacter;
    endLine;
    endCharacter;
    constructor(startLine, startCharacter, endLine, endCharacter) {
        this.startLine = startLine;
        this.startCharacter = startCharacter;
        this.endLine = endLine;
        this.endCharacter = endCharacter;
    }
}
exports.Range = Range;
class Position {
    line;
    character;
    constructor(line, character) {
        this.line = line;
        this.character = character;
    }
}
exports.Position = Position;
class ThemeColor {
    id;
    constructor(id) {
        this.id = id;
    }
}
exports.ThemeColor = ThemeColor;
class ThemeIcon {
    id;
    constructor(id) {
        this.id = id;
    }
}
exports.ThemeIcon = ThemeIcon;
class TreeItem {
    label;
    description;
    tooltip;
    iconPath;
    contextValue;
    command;
    collapsibleState = TreeItemCollapsibleState.None;
    constructor(label, collapsibleState) {
        this.label = label;
        if (collapsibleState !== undefined) {
            this.collapsibleState = collapsibleState;
        }
    }
}
exports.TreeItem = TreeItem;
class EventEmitter {
    event = () => () => { };
    fire(_value) { }
    dispose() { }
}
exports.EventEmitter = EventEmitter;
class RelativePattern {
    base;
    pattern;
    constructor(base, pattern) {
        this.base = base;
        this.pattern = pattern;
    }
}
exports.RelativePattern = RelativePattern;
class FileSystemWatcherStub {
    changeCallbacks = [];
    createCallbacks = [];
    deleteCallbacks = [];
    onDidChange = (cb) => {
        this.changeCallbacks.push(cb);
        return { dispose: () => { } };
    };
    onDidCreate = (cb) => {
        this.createCallbacks.push(cb);
        return { dispose: () => { } };
    };
    onDidDelete = (cb) => {
        this.deleteCallbacks.push(cb);
        return { dispose: () => { } };
    };
    async __fireChange(uri) {
        for (const cb of this.changeCallbacks) {
            await cb(uri);
        }
    }
    async __fireCreate(uri) {
        for (const cb of this.createCallbacks) {
            await cb(uri);
        }
    }
    async __fireDelete(uri) {
        for (const cb of this.deleteCallbacks) {
            await cb(uri);
        }
    }
    dispose = () => { };
}
const noopDisposable = { dispose: () => { } };
let workspaceFolders = [];
const configStore = new Map();
function getSectionStore(section) {
    if (!configStore.has(section)) {
        configStore.set(section, new Map());
    }
    return configStore.get(section);
}
function __resetConfigStore() {
    configStore.clear();
}
const commandCalls = [];
const warningCalls = [];
let warningResult = undefined;
function __setWarningResult(result) {
    warningResult = result;
}
const quickPickCalls = [];
let quickPickResult = undefined;
function __setQuickPickResult(result) {
    quickPickResult = result;
}
const inputBoxCalls = [];
let inputBoxResult = undefined;
function __setInputBoxResult(result) {
    inputBoxResult = result;
}
const outputChannels = new Map();
let saveCallbacks = [];
function __fireSave(document) {
    for (const cb of saveCallbacks) {
        cb(document);
    }
}
function __clearSaveCallbacks() {
    saveCallbacks = [];
}
const createdWebviewPanels = [];
function __getCreatedWebviewPanels() {
    return createdWebviewPanels;
}
function __findCreatedWebviewPanel(viewType) {
    return createdWebviewPanels.find((p) => p.viewType === viewType);
}
function __clearCreatedWebviewPanels() {
    createdWebviewPanels.length = 0;
}
const informationCalls = [];
function __resetInformationCalls() {
    informationCalls.length = 0;
}
const errorCalls = [];
function __resetErrorCalls() {
    errorCalls.length = 0;
}
const treeViewRegistrations = [];
function __resetTreeViewRegistrations() {
    treeViewRegistrations.length = 0;
}
const fileDecorationProviders = [];
function __resetFileDecorationProviders() {
    fileDecorationProviders.length = 0;
}
const statusBarItems = [];
function __resetStatusBarItems() {
    statusBarItems.length = 0;
}
const fileSystemWatchers = [];
function __resetFileSystemWatchers() {
    fileSystemWatchers.length = 0;
}
exports.window = {
    activeTextEditor: undefined,
    showWarningMessage: async (message, ...buttons) => {
        warningCalls.push({ message, buttons });
        return warningResult;
    },
    showInformationMessage: async (message, ...buttons) => {
        informationCalls.push({ message, buttons });
        return undefined;
    },
    showErrorMessage: async (message, ...buttons) => {
        errorCalls.push({ message, buttons });
        return undefined;
    },
    showQuickPick: async (items, options) => {
        quickPickCalls.push({ items, options });
        return quickPickResult;
    },
    showInputBox: async (options) => {
        inputBoxCalls.push(options);
        return inputBoxResult;
    },
    showTextDocument: async () => ({}),
    showSaveDialog: async () => undefined,
    createOutputChannel: (name) => {
        if (!outputChannels.has(name)) {
            outputChannels.set(name, []);
        }
        const lines = outputChannels.get(name);
        return {
            appendLine: (line) => {
                lines.push(String(line));
            },
            show: () => { },
            dispose: () => { outputChannels.delete(name); },
        };
    },
    createStatusBarItem: () => {
        const item = {
            text: "",
            tooltip: "",
            command: undefined,
            show: () => { },
            hide: () => { },
            dispose: () => { },
        };
        statusBarItems.push(item);
        return item;
    },
    createTerminal: () => ({
        show: () => { },
        sendText: () => { },
    }),
    createWebviewPanel: (_viewType, title, column, options) => {
        const disposeCallbacks = [];
        let disposed = false;
        const panel = {
            viewType: _viewType,
            title,
            column,
            options,
            webview: (() => {
                const messages = [];
                const receiveCallbacks = [];
                return {
                    html: "",
                    postMessage: (msg) => {
                        messages.push(msg);
                    },
                    onDidReceiveMessage: (cb) => {
                        receiveCallbacks.push(cb);
                        return noopDisposable;
                    },
                    asWebviewUri: (uri) => uri,
                    cspSource: "",
                    __messages: messages,
                    __clearMessages: () => {
                        messages.length = 0;
                    },
                    __receiveCallbacks: receiveCallbacks,
                    __fireReceiveMessage: (msg) => {
                        for (const cb of receiveCallbacks) {
                            cb(msg);
                        }
                    },
                };
            })(),
            reveal: () => { },
            dispose: () => {
                if (disposed) {
                    return;
                }
                disposed = true;
                for (const cb of disposeCallbacks) {
                    cb();
                }
            },
            onDidDispose: (cb) => {
                disposeCallbacks.push(cb);
                return noopDisposable;
            },
            active: true,
            visible: true,
        };
        createdWebviewPanels.push(panel);
        return panel;
    },
    registerTreeDataProvider: (viewId, provider) => {
        treeViewRegistrations.push({ viewId, provider });
        return noopDisposable;
    },
    registerFileDecorationProvider: (provider) => {
        fileDecorationProviders.push(provider);
        return noopDisposable;
    },
    onDidChangeActiveColorTheme: () => noopDisposable,
    onDidChangeActiveTextEditor: () => noopDisposable,
    withProgress: async (_options, task) => task(),
    __warningCalls: warningCalls,
    __setWarningResult,
    __outputChannels: outputChannels,
    __quickPickCalls: quickPickCalls,
    __setQuickPickResult,
    __inputBoxCalls: inputBoxCalls,
    __setInputBoxResult,
    __createdWebviewPanels: createdWebviewPanels,
    __getCreatedWebviewPanels,
    __findCreatedWebviewPanel,
    __clearCreatedWebviewPanels,
    __informationCalls: informationCalls,
    __resetInformationCalls,
    __errorCalls: errorCalls,
    __resetErrorCalls,
    __treeViewRegistrations: treeViewRegistrations,
    __resetTreeViewRegistrations,
    __fileDecorationProviders: fileDecorationProviders,
    __resetFileDecorationProviders,
    __statusBarItems: statusBarItems,
    __resetStatusBarItems,
};
exports.workspace = {
    get workspaceFolders() {
        return workspaceFolders;
    },
    __setWorkspaceFolders(folders) {
        workspaceFolders = folders;
    },
    getWorkspaceFolder(uri) {
        const sorted = [...workspaceFolders].sort((a, b) => b.uri.fsPath.length - a.uri.fsPath.length);
        for (const folder of sorted) {
            const fp = folder.uri.fsPath;
            if (uri.fsPath === fp || uri.fsPath.startsWith(fp + "/")) {
                return folder;
            }
        }
        return undefined;
    },
    openTextDocument: async () => ({}),
    __fireSave,
    __clearSaveCallbacks,
    getConfiguration: (section) => {
        const store = getSectionStore(section);
        const CONFIG_DEFAULTS = new Map([
            ["dagayn.cliPath", ""],
            ["dagayn.autoUpdate", true],
            ["dagayn.autoUpdateFailureThreshold", 3],
            ["dagayn.blastRadiusDepth", 2],
            ["dagayn.graphTheme", "auto"],
            ["dagayn.treeView.showFunctions", true],
            ["dagayn.treeView.showClasses", true],
            ["dagayn.treeView.showFiles", true],
            ["dagayn.treeView.showTypes", true],
            ["dagayn.treeView.showTests", true],
            ["dagayn.graph.defaultEdges", ["CALLS", "IMPORTS_FROM", "INHERITS", "IMPLEMENTS", "TESTED_BY", "DEPENDS_ON"]],
            ["dagayn.graph.maxNodes", 500],
        ]);
        return {
            get: (key, defaultValue) => {
                if (store.has(key)) return store.get(key);
                if (defaultValue !== undefined) return defaultValue;
                return CONFIG_DEFAULTS.get(`${section}.${key}`);
            },
            update: async (key, value, _target) => {
                store.set(key, value);
            },
        };
    },
    createFileSystemWatcher: () => {
        const watcher = new FileSystemWatcherStub();
        fileSystemWatchers.push(watcher);
        return watcher;
    },
    onDidSaveTextDocument: (cb) => {
        saveCallbacks.push(cb);
        return { dispose: () => {
                const idx = saveCallbacks.indexOf(cb);
                if (idx >= 0) {
                    saveCallbacks.splice(idx, 1);
                }
            } };
    },
    fs: {
        stat: async (uri) => {
            if (fs.existsSync(uri.fsPath)) {
                return { type: FileType.File };
            }
            throw new Error("File not found");
        },
        writeFile: async () => { },
        readFile: async () => Buffer.from(""),
    },
    __resetConfigStore,
    __fileSystemWatchers: fileSystemWatchers,
    __resetFileSystemWatchers,
};
exports.commands = {
    registerCommand: () => noopDisposable,
    executeCommand: async (command, ...args) => {
        commandCalls.push({ command, args });
        return undefined;
    },
    __calls: commandCalls,
};
const hoverProviders = [];
exports.languages = {
    registerHoverProvider: (_selector, provider) => {
        hoverProviders.push(provider);
        return noopDisposable;
    },
    __hoverProviders: hoverProviders,
    __clearHoverProviders: () => {
        hoverProviders.length = 0;
    },
};
exports.env = {
    clipboard: {
        writeText: async () => { },
    },
    openExternal: async () => true,
};
const FileDecoration = class {
    badge;
    color;
    tooltip;
    propagate;
    constructor(badge, color, tooltip, propagate) {
        this.badge = badge;
        this.color = color;
        this.tooltip = tooltip;
        this.propagate = propagate;
    }
};
exports.FileDecoration = FileDecoration;
class MarkdownString {
    value;
    isTrusted = false;
    supportThemeIcons = false;
    supportHtml = false;
    constructor(value = "") {
        this.value = value;
    }
    appendMarkdown(s) {
        this.value += s;
        return this;
    }
}
exports.MarkdownString = MarkdownString;
class Hover {
    contents;
    range;
    constructor(contents, range) {
        this.contents = contents;
        this.range = range;
    }
}
exports.Hover = Hover;
