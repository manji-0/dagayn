// A SCIP index of a TypeScript project from TypeScript 7 (the Go compiler,
// `tsgo`) through its `typescript/unstable/sync` API, for the SCIP overlay
// (docs/plans/SCIP-FIRST-COMPLETENESS.md#typescript-without-scip-typescript-a-tsgo-resolver).
//
//   node scip_tsgo.mjs index --output <index.scip> [tsconfig.json ...]
//   node scip_tsgo.mjs serve [tsconfig.json ...]
//
// `index` takes scip-typescript's arguments and indexes every file of the
// projects. `serve` keeps the projects open and reads one JSON request per
// line on stdin, `{"changed": [paths], "files": [paths], "output": path}`:
// it tells the compiler which files changed, indexes only `files` (paths
// relative to the working directory), and answers one JSON line on stdout
// (`{"ok": true, "documents": n, "occurrences": m}`). `DAGAYN_TSGO_TYPESCRIPT`
// names the directory of the `typescript` package (7 or later) to load.
//
// Only what the overlay reads is written: per document its occurrences
// (range, symbol, definition role), in UTF-16 units. Every identifier of the
// files indexed is asked for its symbol, many per request; a declaration's
// position comes from its source file's syntax tree, so a symbol costs one
// round trip at most (an alias's target, a parent's name). Symbols declared
// inside a function are `local`; the others are global, named as
// scip-typescript names them (`scip-typescript npm <package> .
// <descriptors>`), the package of a declaration outside the project read
// from its `node_modules` path (`typescript` for the default library). A
// project symbol referenced from the files indexed also gets its definition,
// in its own file's document, so the overlay finds its node.

import fs from "node:fs";
import path from "node:path";
import readline from "node:readline";
import { pathToFileURL } from "node:url";

const args = process.argv.slice(2);
const mode = args[0];
if (mode !== "index" && mode !== "serve") {
  process.stderr.write(
    "usage: scip_tsgo.mjs index --output <index.scip> [tsconfig.json ...]\n" +
      "       scip_tsgo.mjs serve [tsconfig.json ...]\n",
  );
  process.exit(2);
}
let output = "index.scip";
const configs = [];
for (let i = 1; i < args.length; i++) {
  if (args[i] === "--output") output = args[++i];
  else if (!args[i].startsWith("--")) configs.push(args[i]);
}
const cwd = process.cwd();
if (configs.length === 0) configs.push("tsconfig.json");

const typescript = process.env.DAGAYN_TSGO_TYPESCRIPT;
if (!typescript) {
  process.stderr.write("DAGAYN_TSGO_TYPESCRIPT is not set\n");
  process.exit(2);
}
const load = (relative) => import(pathToFileURL(path.join(typescript, relative)).href);
const { API, SymbolFlags } = await load("dist/api/sync/api.js");
const { isIdentifier } = await load("dist/ast/is.js");
const { SyntaxKind } = await load("dist/ast/index.js");

const FUNCTION_LIKE = new Set(
  [
    "FunctionDeclaration",
    "FunctionExpression",
    "ArrowFunction",
    "MethodDeclaration",
    "Constructor",
    "GetAccessor",
    "SetAccessor",
  ].map((name) => SyntaxKind[name]),
);
const CHUNK = 5000;

const escape = (name) => (/^[\w$+-]+$/.test(name) ? name : `\`${name.replaceAll("`", "``")}\``);
const suffixOf = (flags) => {
  if (flags & (SymbolFlags.Class | SymbolFlags.Interface | SymbolFlags.Enum | SymbolFlags.TypeAlias)) return "#";
  if (flags & (SymbolFlags.Function | SymbolFlags.Method | SymbolFlags.Constructor)) return "().";
  return ".";
};

function packageOf(file) {
  const at = file.lastIndexOf("/node_modules/");
  if (at < 0) return null;
  const parts = file.slice(at + "/node_modules/".length).split("/");
  const scoped = parts[0].startsWith("@");
  const name = scoped ? `${parts[0]}/${parts[1]}` : parts[0];
  const inner = parts.slice(scoped ? 2 : 1).join("/");
  // TypeScript's own library ships with the compiler package (tsrs, the Rust
  // port, ships it with its platform binary).
  const compiler = name === "typescript" || name.startsWith("@typescript/typescript-") || name === "tsrs" || name.startsWith("@tsrs/");
  if (compiler) return { name: "typescript", inner: path.basename(inner) };
  return { name, inner };
}

function insideFunction(node) {
  for (let current = node.parent; current; current = current.parent) {
    if (FUNCTION_LIKE.has(current.kind)) return true;
  }
  return false;
}

/**
 * The SCIP index of `only` (absolute paths; every project file when null)
 * in `snapshot`, as bytes, with its document and occurrence counts.
 */
function indexSnapshot(snapshot, only) {
  const projects = snapshot.getProjects();

  // The projects' own files; their syntax trees are fetched when needed.
  const own = new Map(); // absolute path -> { project, relative, sourceFile }
  for (const project of projects) {
    for (const file of project.program.getSourceFileNames()) {
      if (own.has(file) || file.endsWith(".d.ts") || file.includes("/node_modules/")) continue;
      const relative = path.relative(cwd, file);
      if (relative.startsWith("..") || path.isAbsolute(relative)) continue;
      own.set(file, { project, relative, sourceFile: undefined });
    }
  }
  const sourceFileOf = (file) => {
    const entry = own.get(file);
    if (!entry) return undefined;
    entry.sourceFile ??= entry.project.program.getSourceFile(file);
    return entry.sourceFile;
  };
  const declarationNode = (handle) => sourceFileOf(handle.path)?.getOrCreateNodeAtIndex(handle.index);

  const lineStarts = new Map();
  const position = (file, offset) => {
    let starts = lineStarts.get(file);
    if (!starts) {
      const text = sourceFileOf(file).text;
      starts = [0];
      for (let i = 0; i < text.length; i++) if (text.charCodeAt(i) === 10) starts.push(i + 1);
      lineStarts.set(file, starts);
    }
    let low = 0;
    let high = starts.length - 1;
    while (low < high) {
      const middle = (low + high + 1) >> 1;
      if (starts[middle] <= offset) low = middle;
      else high = middle - 1;
    }
    return [low, offset - starts[low]];
  };

  // Identifiers by project, so each request goes to one checker.
  const byProject = new Map();
  for (const file of only ?? own.keys()) {
    const entry = own.get(file);
    if (!entry) continue;
    const list = byProject.get(entry.project) ?? [];
    const visit = (node) => {
      if (isIdentifier(node)) list.push({ file, node });
      node.forEachChild(visit);
      return undefined;
    };
    sourceFileOf(file).forEachChild(visit);
    byProject.set(entry.project, list);
  }

  const aliasTarget = new Map();
  const names = new Map(); // symbol id -> SCIP symbol, or null
  const ownDeclarations = new Map(); // SCIP symbol -> its declaration's handle
  const chains = new Map(); // symbol id -> its descriptors
  let localCounter = 0;

  const descriptorChain = (symbol, depth = 0) => {
    if (chains.has(symbol.id)) return chains.get(symbol.id);
    let chain = "";
    if (!symbol.name.startsWith('"')) {
      // a module's name is quoted: it ends the chain
      let parent;
      if (depth < 8) {
        try {
          parent = symbol.getParent();
        } catch {
          parent = undefined;
        }
      }
      const name = symbol.name === "__constructor" ? "<constructor>" : symbol.name;
      chain = `${parent ? descriptorChain(parent, depth + 1) : ""}${escape(name)}${suffixOf(symbol.flags)}`;
    }
    chains.set(symbol.id, chain);
    return chain;
  };

  const scipName = (symbol) => {
    if (names.has(symbol.id)) return names.get(symbol.id);
    let name = null;
    const handle = symbol.declarations?.[0];
    if (handle) {
      const node = declarationNode(handle);
      if (node && insideFunction(node) && !(symbol.flags & (SymbolFlags.Method | SymbolFlags.Property))) {
        name = `local ${localCounter++}`;
      } else {
        const entry = own.get(handle.path);
        const pkg = entry ? { name: "dagayn-project", inner: entry.relative } : packageOf(handle.path);
        if (pkg) name = `scip-typescript npm ${pkg.name} . ${escape(pkg.inner)}/${descriptorChain(symbol)}`;
        if (name && entry) ownDeclarations.set(name, handle);
      }
    }
    names.set(symbol.id, name);
    return name;
  };

  const documents = new Map(); // relative path -> occurrences
  const definitionsWritten = new Set();
  const occurrence = (file, start, end, symbol, roles) => {
    const relative = own.get(file).relative;
    const list = documents.get(relative) ?? [];
    const [line, column] = position(file, start);
    const [, endColumn] = position(file, end);
    list.push({ range: [line, column, endColumn], symbol, roles });
    documents.set(relative, list);
  };

  for (const [project, identifiers] of byProject) {
    for (let i = 0; i < identifiers.length; i += CHUNK) {
      const batch = identifiers.slice(i, i + CHUNK);
      const symbols = project.checker.getSymbolAtLocation(batch.map((entry) => entry.node));
      batch.forEach(({ file, node }, k) => {
        let symbol = symbols[k];
        if (!symbol) return;
        if (symbol.flags & SymbolFlags.Alias) {
          if (!aliasTarget.has(symbol.id)) {
            let target;
            try {
              target = project.checker.getAliasedSymbol(symbol);
            } catch {
              target = undefined;
            }
            aliasTarget.set(symbol.id, target);
          }
          symbol = aliasTarget.get(symbol.id);
          if (!symbol) return;
        }
        const name = scipName(symbol);
        if (!name) return;
        const start = node.end - node.text.length;
        // The identifier that names one of the symbol's declarations (an
        // overload, a merged interface) defines the symbol there.
        const defines = (symbol.declarations ?? []).some((handle) => {
          if (handle.path !== file) return false;
          const declaration = declarationNode(handle);
          const declared = declaration?.name ?? declaration;
          return declared !== undefined && declared.end === node.end;
        });
        if (defines) definitionsWritten.add(name);
        occurrence(file, start, node.end, name, defines ? 1 : 0);
      });
    }
  }

  // Every project symbol named gets a definition, even when no identifier
  // indexed named it there (a default export, a string-named member, a file
  // not indexed): without one, the overlay would take it for a package's.
  for (const [name, handle] of ownDeclarations) {
    if (definitionsWritten.has(name)) continue;
    const node = declarationNode(handle);
    const declared = node?.name ?? node;
    if (!declared) continue;
    const length = declared.text?.length ?? 0;
    occurrence(handle.path, declared.end - length, declared.end, name, 1);
  }

  let occurrences = 0;
  for (const list of documents.values()) occurrences += list.length;
  return { bytes: encode(documents), documents: documents.size, occurrences };
}

// --- protobuf ---------------------------------------------------------------

class Writer {
  constructor(size = 1 << 16) {
    this.bytes = new Uint8Array(size);
    this.length = 0;
  }
  reserve(n) {
    if (this.length + n <= this.bytes.length) return;
    let size = this.bytes.length * 2;
    while (size < this.length + n) size *= 2;
    const bytes = new Uint8Array(size);
    bytes.set(this.bytes.subarray(0, this.length));
    this.bytes = bytes;
  }
  varint(value) {
    this.reserve(5);
    let v = value >>> 0;
    while (v > 0x7f) {
      this.bytes[this.length++] = (v & 0x7f) | 0x80;
      v >>>= 7;
    }
    this.bytes[this.length++] = v;
  }
  tag(field, wire) {
    this.varint((field << 3) | wire);
  }
  bytesField(field, payload) {
    this.tag(field, 2);
    this.varint(payload.length);
    this.reserve(payload.length);
    this.bytes.set(payload, this.length);
    this.length += payload.length;
  }
  view() {
    return this.bytes.subarray(0, this.length);
  }
}

const encoded = new Map(); // text -> its UTF-8 bytes
const utf8 = (text) => {
  let bytes = encoded.get(text);
  if (!bytes) encoded.set(text, (bytes = Buffer.from(text, "utf8")));
  return bytes;
};

function encode(documents) {
  const index = new Writer(1 << 20);
  const document = new Writer();
  const message = new Writer(256);
  const packed = new Writer(32);
  for (const [relative, occurrences] of documents) {
    document.length = 0;
    document.bytesField(1, utf8(relative));
    for (const { range, symbol, roles } of occurrences) {
      message.length = 0;
      packed.length = 0;
      for (const value of range) packed.varint(value);
      message.bytesField(1, packed.view());
      message.bytesField(2, utf8(symbol));
      if (roles) {
        message.tag(3, 0);
        message.varint(roles);
      }
      document.bytesField(2, message.view());
    }
    document.tag(6, 0);
    document.varint(2); // UTF16CodeUnitOffsetFromLineStart
    index.bytesField(2, document.view());
  }
  return index.view();
}

// --- main -------------------------------------------------------------------

const api = new API({ cwd });
const openProjects = configs.map((config) => path.resolve(cwd, config));
// TypeScript 7.0 updates one snapshot in place; 7.1 (and tsrs) create
// snapshots, each from the server's current state and the file changes.
const nextApi = typeof api.createSnapshot === "function";
let snapshot = nextApi ? api.createSnapshot({ openProjects }) : api.updateSnapshot({ openProjects });
for (const project of snapshot.getProjects()) {
  for (const diagnostic of project.program.getConfigFileParsingDiagnostics()) {
    process.stderr.write(`warning: ${project.configFileName}: TS${diagnostic.code} ${diagnostic.text ?? ""}\n`);
  }
}

if (mode === "index") {
  const result = indexSnapshot(snapshot, null);
  fs.writeFileSync(path.resolve(cwd, output), result.bytes);
  process.stderr.write(`tsgo index: ${result.documents} documents, ${result.occurrences} occurrences\n`);
  api.close();
} else {
  process.stdout.write(`${JSON.stringify({ ready: true })}\n`);
  const lines = readline.createInterface({ input: process.stdin });
  for await (const line of lines) {
    if (!line.trim()) continue;
    let answer;
    try {
      const request = JSON.parse(line);
      const absolute = (file) => path.resolve(cwd, file);
      const changed = (request.changed ?? []).map(absolute);
      if (changed.length) {
        const existing = changed.filter((file) => fs.existsSync(file));
        const deleted = changed.filter((file) => !fs.existsSync(file));
        if (nextApi) {
          // A new snapshot rather than `snapshot.update`, which tsrs 0.16
          // answers from the old file contents.
          const previous = snapshot;
          snapshot = api.createSnapshot({ openProjects, fileNotifications: { changed: existing, deleted } });
          previous.dispose();
        } else {
          snapshot = api.updateSnapshot({ fileChanges: { changed: existing, deleted } });
        }
      }
      const result = indexSnapshot(snapshot, (request.files ?? []).map(absolute));
      fs.writeFileSync(absolute(request.output), result.bytes);
      answer = { ok: true, documents: result.documents, occurrences: result.occurrences };
    } catch (error) {
      answer = { ok: false, error: String(error?.stack ?? error) };
    }
    process.stdout.write(`${JSON.stringify(answer)}\n`);
  }
  api.close();
}
