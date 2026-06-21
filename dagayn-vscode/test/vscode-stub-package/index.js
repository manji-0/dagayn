"use strict";
/**
 * Minimal vscode API stub for running unit tests under plain Node.
 *
 * Only implements the surface used by src/ modules imported during tests.
 */
Object.defineProperty(exports, "__esModule", { value: true });
exports.FileDecoration = exports.env = exports.commands = exports.workspace = exports.window = exports.RelativePattern = exports.EventEmitter = exports.TreeItem = exports.ThemeColor = exports.Position = exports.Range = exports.Uri = exports.FileType = exports.ColorThemeKind = exports.ViewColumn = exports.ProgressLocation = exports.StatusBarAlignment = exports.TreeItemCollapsibleState = void 0;
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
    onDidChange = () => ({ dispose: () => { } });
    onDidCreate = () => ({ dispose: () => { } });
    onDidDelete = () => ({ dispose: () => { } });
    dispose = () => { };
}
const noopDisposable = { dispose: () => { } };
exports.window = {
    activeTextEditor: undefined,
    showWarningMessage: async () => undefined,
    showInformationMessage: async () => undefined,
    showErrorMessage: async () => undefined,
    showQuickPick: async () => undefined,
    showTextDocument: async () => ({}),
    showSaveDialog: async () => undefined,
    createOutputChannel: () => ({
        appendLine: () => { },
        show: () => { },
    }),
    createStatusBarItem: () => ({
        text: "",
        tooltip: "",
        command: undefined,
        show: () => { },
        hide: () => { },
        dispose: () => { },
    }),
    createTerminal: () => ({
        show: () => { },
        sendText: () => { },
    }),
    registerTreeDataProvider: () => noopDisposable,
    registerFileDecorationProvider: () => noopDisposable,
    onDidChangeActiveColorTheme: () => noopDisposable,
    withProgress: async (_options, task) => task(),
};
exports.workspace = {
    workspaceFolders: [],
    openTextDocument: async () => ({}),
    getConfiguration: () => ({
        get: (_key, defaultValue) => defaultValue,
    }),
    createFileSystemWatcher: () => new FileSystemWatcherStub(),
    onDidSaveTextDocument: () => noopDisposable,
    fs: {
        stat: async () => ({ type: FileType.File }),
        writeFile: async () => { },
        readFile: async () => Buffer.from(""),
    },
};
exports.commands = {
    registerCommand: () => noopDisposable,
    executeCommand: async () => undefined,
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
