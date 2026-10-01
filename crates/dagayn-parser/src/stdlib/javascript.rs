//! The JavaScript / TypeScript standard library: the Node.js builtin modules
//! (`fs`, `node:path`) and the ECMAScript / host globals (`JSON`, `Math`,
//! `console`, `setTimeout`).

/// Node.js builtin modules importable without the `node:` prefix (`node:test`
/// and `node:sqlite` exist only with it). A subpath (`fs/promises`,
/// `stream/web`) belongs to its module.
const NODE_BUILTIN_MODULES: &[&str] = &[
    "assert",
    "async_hooks",
    "buffer",
    "child_process",
    "cluster",
    "console",
    "constants",
    "crypto",
    "dgram",
    "diagnostics_channel",
    "dns",
    "domain",
    "events",
    "fs",
    "http",
    "http2",
    "https",
    "inspector",
    "module",
    "net",
    "os",
    "path",
    "perf_hooks",
    "process",
    "punycode",
    "querystring",
    "readline",
    "repl",
    "stream",
    "string_decoder",
    "sys",
    "timers",
    "tls",
    "trace_events",
    "tty",
    "url",
    "util",
    "v8",
    "vm",
    "wasi",
    "worker_threads",
    "zlib",
];

/// Globals every ECMAScript host provides (`JSON.parse`, `new Map()`,
/// `parseInt(s)`), the timers and web APIs browsers and Node share
/// (`setTimeout`, `fetch`, `new URL(s)`), and Node's own (`process.exit`,
/// `Buffer.from`).
const JAVASCRIPT_GLOBALS: &[&str] = &[
    "AbortController",
    "Array",
    "ArrayBuffer",
    "BigInt",
    "Boolean",
    "Buffer",
    "DataView",
    "Date",
    "Error",
    "EvalError",
    "Float32Array",
    "Float64Array",
    "Int8Array",
    "Int16Array",
    "Int32Array",
    "Intl",
    "JSON",
    "Map",
    "Math",
    "Number",
    "Object",
    "Promise",
    "Proxy",
    "RangeError",
    "ReferenceError",
    "Reflect",
    "RegExp",
    "Set",
    "String",
    "Symbol",
    "SyntaxError",
    "TextDecoder",
    "TextEncoder",
    "TypeError",
    "URIError",
    "URL",
    "URLSearchParams",
    "Uint8Array",
    "Uint16Array",
    "Uint32Array",
    "WeakMap",
    "WeakRef",
    "WeakSet",
    "atob",
    "btoa",
    "clearImmediate",
    "clearInterval",
    "clearTimeout",
    "console",
    "decodeURI",
    "decodeURIComponent",
    "encodeURI",
    "encodeURIComponent",
    "fetch",
    "globalThis",
    "isFinite",
    "isNaN",
    "parseFloat",
    "parseInt",
    "process",
    "queueMicrotask",
    "setImmediate",
    "setInterval",
    "setTimeout",
    "structuredClone",
];

/// The package of a Node.js builtin module `specifier` names, with the
/// `node:` prefix and without a subpath: `fs`, `node:fs`, and
/// `fs/promises` are all `node:fs`. Anything else (`lodash`, `./fs`) is
/// `None`. `node:` always names a builtin, even one missing from the table
/// (`node:sea`).
pub(crate) fn node_builtin_package(specifier: &str) -> Option<String> {
    let (bare, prefixed) = match specifier.strip_prefix("node:") {
        Some(bare) => (bare, true),
        None => (specifier, false),
    };
    let module = bare.split('/').next().filter(|module| !module.is_empty())?;
    (prefixed || NODE_BUILTIN_MODULES.contains(&module)).then(|| format!("node:{module}"))
}

/// Whether `name` is a global of every JavaScript host (see
/// [`JAVASCRIPT_GLOBALS`]).
pub(crate) fn is_javascript_global(name: &str) -> bool {
    JAVASCRIPT_GLOBALS.contains(&name)
}
