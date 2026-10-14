// A SCIP index of a TypeScript project from TypeScript 7 (the Go compiler,
// `tsgo`) through its `typescript/unstable/sync` API, for the SCIP overlay
// (docs/plans/SCIP-FIRST-COMPLETENESS.md#typescript-without-scip-typescript-a-tsgo-resolver).
//
//   node scip_tsgo.mjs index --output <index.scip> [tsconfig.json ...]
//
// The arguments are scip-typescript's. `DAGAYN_TSGO_TYPESCRIPT` names the
// directory of the `typescript` package (7 or later) to load.
//
// Only what the overlay reads is written: per document its occurrences
// (range, symbol, definition role), in UTF-16 units. Every identifier of the
// project's own files is asked for its symbol, many per request; a
// declaration's position comes from the source files already fetched, so a
// symbol costs one round trip at most (an alias's target, a parent's name).
// Symbols declared inside a function are `local`; the others are global,
// named as scip-typescript names them (`scip-typescript npm <package> .
// <descriptors>`), the package of a declaration outside the project read
// from its `node_modules` path (`typescript` for the default library).

import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const args = process.argv.slice(2);
if (args[0] !== "index") {
  process.stderr.write("usage: scip_tsgo.mjs index --output <index.scip> [tsconfig.json ...]\n");
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

const api = new API({ cwd });
const snapshot = api.updateSnapshot({
  openProjects: configs.map((config) => path.resolve(cwd, config)),
});
const projects = snapshot.getProjects();
for (const project of projects) {
  for (const diagnostic of project.program.getConfigFileParsingDiagnostics()) {
    process.stderr.write(`warning: ${project.configFileName}: TS${diagnostic.code} ${diagnostic.text ?? ""}\n`);
  }
}

// --- the project's own files and their identifiers -------------------------

const own = new Map(); // absolute path -> { project, sourceFile, relative }
for (const project of projects) {
  for (const file of project.program.getSourceFileNames()) {
    if (own.has(file) || file.endsWith(".d.ts") || file.includes("/node_modules/")) continue;
    const relative = path.relative(cwd, file);
    if (relative.startsWith("..") || path.isAbsolute(relative)) continue;
    own.set(file, { project, sourceFile: project.program.getSourceFile(file), relative });
  }
}

const lineStarts = new Map();
function position(file, offset) {
  let starts = lineStarts.get(file);
  if (!starts) {
    const text = own.get(file).sourceFile.text;
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
}

// Identifiers by project, so each request goes to one checker.
const byProject = new Map();
for (const [file, entry] of own) {
  const list = byProject.get(entry.project) ?? [];
  const visit = (node) => {
    if (isIdentifier(node)) list.push({ file, node });
    node.forEachChild(visit);
    return undefined;
  };
  entry.sourceFile.forEachChild(visit);
  byProject.set(entry.project, list);
}

// --- symbols ----------------------------------------------------------------

const aliasTarget = new Map();
const names = new Map(); // symbol id -> SCIP symbol, or null
const ownDeclarations = new Map(); // SCIP symbol -> its declaration's handle
let localCounter = 0;

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
  // TypeScript's own library ships with the compiler package.
  if (name === "typescript" || name.startsWith("@typescript/typescript-")) return { name: "typescript", inner: path.basename(inner) };
  return { name, inner };
}

function declarationNode(handle) {
  const entry = own.get(handle.path);
  return entry ? entry.sourceFile.getOrCreateNodeAtIndex(handle.index) : undefined;
}

function insideFunction(node) {
  for (let current = node.parent; current; current = current.parent) {
    if (FUNCTION_LIKE.has(current.kind)) return true;
  }
  return false;
}

const chains = new Map(); // symbol id -> its descriptors
function descriptorChain(symbol, depth = 0) {
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
}

function scipName(symbol) {
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
}

// --- occurrences ------------------------------------------------------------

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

// Every symbol declared in the project gets a definition, even when no
// identifier named it there (a default export, a string-named member):
// without one, the overlay would take the symbol for a package's.
for (const [name, handle] of ownDeclarations) {
  if (definitionsWritten.has(name)) continue;
  const node = declarationNode(handle);
  const declared = node?.name ?? node;
  if (!declared) continue;
  const length = declared.text?.length ?? 0;
  occurrence(handle.path, declared.end - length, declared.end, name, 1);
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

const encoded = new Map(); // SCIP symbol -> its UTF-8 bytes
const utf8 = (text) => {
  let bytes = encoded.get(text);
  if (!bytes) encoded.set(text, (bytes = Buffer.from(text, "utf8")));
  return bytes;
};

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
fs.writeFileSync(path.resolve(cwd, output), index.view());
process.stderr.write(
  `tsgo index: ${documents.size} documents, ${[...documents.values()].reduce((n, list) => n + list.length, 0)} occurrences\n`,
);
api.close();
