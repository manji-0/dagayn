# TypeScript / JavaScript extraction model

<!-- constrained-by ./SCHEMA.md -->
<!-- constrained-by ./ARCHITECTURE.md#parsing-model -->

This document specifies which nodes and edges dagayn extracts from TypeScript,
TSX, JavaScript, and JSX sources, how their qualified names are formed, and how
references are resolved. It is the contract for `crates/dagayn-parser`
(`js_like.rs`, `js_modules.rs`, `js_members.rs`, `js_types.rs`,
`member_calls.rs`, `js_sfc.rs`) and for the
analysis code that reads the resulting metadata (`dagayn/sap.py`,
`dagayn/refactor/dead_code.py`, `dagayn/entry_point_heuristics.py`,
`crates/dagayn-graph` post-processing).

The coverage matrix at the end records, per construct, whether the behavior is
implemented and which change delivers it. Rows marked
`planned (part 2/3, #N)` describe the target model that a later change (track
TODO `#N`) implements; until then the parser may still emit the older output.

## 1. Scope

| Input | Grammar | Notes |
|---|---|---|
| `.ts`, `.mts`, `.cts`, `.d.ts`, `.d.mts`, `.d.cts` | tree-sitter-typescript (`typescript`) | language `typescript`; `.d.mts` / `.d.cts` are declaration files like `.d.ts` |
| `.tsx` | tree-sitter-typescript (`tsx`) | JSX enabled |
| `.js`, `.jsx`, `.mjs`, `.cjs` | tree-sitter-javascript | language `javascript`; JSX enabled for all JavaScript files |
| Vue / Svelte `<script>` blocks | TypeScript or JavaScript | `js_sfc.rs` hands each block to the same extractor and shifts line numbers |

JavaScript and TypeScript share one extraction path. Grammar differences
(JavaScript `field_definition` versus TypeScript `public_field_definition`,
JavaScript `class_heritage` holding the base expression directly, the absence
of type syntax in JavaScript) are absorbed by node-kind aliases rather than by
separate extractors.

## 2. Principles

1. **Model the declaration space.** A node exists for every declaration that
   another file can reach by name: module-scope declarations and members of
   classes, namespaces, and object containers. Declarations local to a function
   body are not nodes; calls inside them belong to the nearest enclosing node.
2. **Owner paths.** `parent_name` is the dotted path of the enclosing named
   containers (`Outer.Deep`). The qualified name (QN) is
   `file::owner.path.name`. The `CONTAINS` source is always the QN of the
   nearest container that exists as a node.
3. **One QN, one node.** The parser never emits two nodes with the same QN
   (the graph store keeps only the last write). Overloads, getter/setter pairs,
   and same-file declaration merging collapse into one node
   (`javascript_collapse_duplicate_nodes`): the implementation, class, or
   enum survives over signatures, interfaces, and namespaces, and records
   `overloads`, `accessors`, or `merged_declarations`.
4. **Reuse the shared vocabulary.** No new `NodeKind` or `EdgeKind` values.
   TypeScript-specific facts go into `extra` keys that other languages already
   use (`type_role`, `is_abstract`, `is_contract`, `decorators`, `member_role`,
   `relationship_role`, `container_role`).
5. **Unresolved beats wrong.** The parser does not guess. A name it cannot bind
   stays bare, and graph post-processing may resolve it at MEDIUM confidence
   through import visibility. The parser does not invent a QN such as
   `module::localAlias` that no declaration defines.
6. **TypeScript and JavaScript behave the same** wherever the syntax overlaps.

## 3. Nodes

| Declaration | Kind | `name` | `parent_name` | Required `extra` |
|---|---|---|---|---|
| `class C`, `export default class C`, `declare class C` | `Class` | `C` | owner path | `type_role: "class"` |
| `abstract class C`, `declare abstract class C` | `Class` | `C` | owner path | `type_role: "abstract_class"`, `is_abstract: true` |
| `export default class {}` (anonymous) | `Class` | `default` | owner path | `type_role` as above, `export_default: true`, `anonymous: true` |
| `const X = class [Inner] {}` at module scope | `Class` | `X` (the binding) | owner path | `type_role: "class"`, `class_expression: true`, optional `expression_name: "Inner"` |
| `interface I` | `Class` | `I` | owner path | `type_role: "interface"`, `is_abstract: true`, `is_contract: true` |
| `type T = ...` | `Type` | `T` | owner path | `type_role: "alias"`, `alias_form` (`object`, `union`, `intersection`, `function`, `conditional`, `mapped`, `tuple`, `reference`, `primitive`, `literal`, `operator`, `other`); `object` also carries `container_role: "data_container"`, `value_semantics: true` (see §7.1) |
| `enum E`, `const enum E`, `declare enum E` | `Class` | `E` | owner path | `type_role: "enum"`, `container_role: "data_container"`, `value_semantics: true`; `const_enum: true` for `const enum`; `ambient: true` when declared |
| `namespace N`, `module N` | `Class` | segment name | owner path | `type_role: "namespace"`; `namespace A.B.C` yields nested `A`, `A.B`, `A.B.C` |
| `declare module "x"`, `declare global` | `Class` | `x` / `global` | owner path | `type_role: "ambient_module"`, `ambient: true` |
| module-scope `const X = { ... }` or `export default { ... }` with at least one function-valued member | `Class` | `X` / `default` | owner path | `type_role: "object"` (see §7.3) |
| `function f`, `function* f`, `async function* f` | `Function` | `f` | owner path | |
| `declare function f(): T;`, a bodiless method of a `declare class` | `Function` | `f` | owner path | `declaration_only: true` (never `is_abstract`) |
| module-scope `const f = () => {}` / `function () {}` / `function* () {}` | `Function` | `f` (the binding) | owner path | |
| module-scope `const C = memo(function Inner() {})`, `forwardRef((p, r) => ...)`, `React.memo(...)`, `observer(...)` (§7.8) | `Function` | `C` (the binding) | owner path | `wrapped_by: ["memo"]` (wrapper callees, outermost first); `expression_name: "Inner"` when the wrapped function is named |
| `export default memo(function Page() {})` | `Function` | `default` | owner path | `export_default: true`, `anonymous: true`, `wrapped_by`, `expression_name: "Page"` |
| `export default function () {}`, `export default () => ...` (anonymous) | `Function` | `default` | owner path | `export_default: true`, `anonymous: true` |
| named default export `export default function Page() {}` | `Function` | `Page` | owner path | `export_default: true` |
| method, getter/setter, `#private` method, function-valued class field (TypeScript `public_field_definition`, JavaScript `field_definition`) | `Function` | member name (`#x`, string / number literal and literal computed keys included) | class owner path | getter/setter: `member_role: "accessor"`, `accessors`; overloads: `overloads: n` |
| `abstract m()`, `abstract get x()` | `Function` | `m` / `x` | class owner path | `is_abstract: true` |
| interface method signature | `Function` | `m` | interface owner path | `is_abstract: true` |
| any declaration inside `declare ...`, `declare module` / `declare global`, or a `.d.ts` / `.d.mts` / `.d.cts` file | (as above) | | | `ambient: true`; the `.d.ts` File node has `declaration_file: true`, and `export as namespace X` records `umd_global: "X"` on it |
| object-container member (`get() {}`, `k: () => {}`, `k: function () {}`) | `Function` | member name | container owner path | |
| test-runner call in a test file (§5.4) | `Test` | `it:description@L6` | owner path | synthetic; `describe` blocks are not part of the owner path; `test_modifiers` (`only`, `skip`, `each`, ...) |

Nodes that are **not** created:

- declarations inside a function or method body (nested functions, local arrow
  handlers, local classes, local object literals); see §7.2
- class fields whose value is not a function, index signatures, enum members,
  interface property / call / construct / index signatures
- members of a type alias's object type (`type Props = { onClick(): void }`
  gives only `Type Props`)
- object literals passed as arguments or created inside function bodies

## 4. Qualified names

- A QN is `file::name` for module-scope declarations and
  `file::owner.path.name` for members, where `file` is the repo-relative path.
- The owner path contains every named container: classes, interfaces,
  namespaces, ambient modules, and object containers
  (`src/api.ts::api.get`, `src/ns.ts::Outer.Deep.deepFn`).
- **`default`.** An anonymous default export is named `default`
  (`src/page.tsx::default`, `src/anon.ts::default.hello`). A named default
  export keeps its own name and records `export_default: true`.
- **Class expressions** take the name of the binding
  (`const Anon = class {}` gives `file::Anon`), because importers refer to the
  binding, not to the optional inner expression name.
- **External package symbols** are not nodes. `CALLS` and `REFERENCES` to
  a name imported from an external package use `<specifier>::<path>`
  (`react::useState`, `express::default`, `node:fs::readFile`,
  `@testing-library/react::render`) and carry `extra.external: true` and
  `extra.external_package` (§7.6). The `::` keeps these targets away from
  the bare-name resolver, so they are never attached to an unrelated local
  symbol of the same name. Post-processing marks them LOW because no node
  exists.
- **Synthetic tests** keep the existing `runner:description@Lline` form
  (§5.4).

## 5. Edges

| Edge | Source | Target | Notes |
|---|---|---|---|
| `CONTAINS` | nearest existing container QN (File, Class, namespace, object container) | child QN | |
| `CALLS` | the node that owns the call site (§5.1) | resolved QN, `pkg::symbol`, or a bare name | includes `new X()`, `super(...)`, JSX elements, tagged templates |
| `IMPORTS_FROM` | File | resolved repo-relative file, or the raw specifier for external modules | static `import` and `export ... from`; `require("./m")` (`import_kind: "require"`, at any depth), dynamic `import("./m")` (`import_kind: "dynamic"`), and TypeScript `import x = require("./m")` (`import_kind: "import_equals"`) with a string-literal specifier; static imports carry no `import_kind` |
| `REFERENCES` (value) | owning node | function or class used as a value | object `pair` values, shorthand properties, array elements, call arguments, assignment right-hand sides |
| `REFERENCES` (type) | owning node | type QN | `relationship_role: "type_reference"` or `"type_query"`, `type_positions` (§7.4); one edge per `(source, target)` |
| `REFERENCES` (decorator) | decorated node | decorator function | `relationship_role: "decorator"` (§7.7) |
| `INHERITS` | class or interface | base QN or bare name | `relationship_role: "extends"`, `syntax_source` |
| `IMPLEMENTS` | class | interface QN or bare name | `relationship_role: "implements"`, `syntax_source` |
| `TESTED_BY` | production symbol | `Test` node | derived from calls made by tests; follows the call when post-processing resolves a bare target (§5.4) |
| `CROSS_ARTIFACT` | owning node | path or command | `child_process.*` and `fs.*` bridges |

### 5.1 Call attribution

| Call site | Source of the `CALLS` edge |
|---|---|
| function or method body, including nested closures and callbacks | that function or method |
| class field initializer, static block, class decorator argument, non-function field decorator argument | the class |
| method / function-valued field decorator argument, parameter decorator | the decorated member |
| class heritage expression (`extends Mixin(Base)`) | the class |
| module scope, including IIFEs | the File |
| test-runner callback, including `.each` tables | the synthetic `Test` node |
| hook callback (`beforeEach`, `afterAll`, Playwright `test.beforeEach`), `test.step` body | the enclosing `describe` `Test`, or the File at module scope |

### 5.2 Call kinds

- **`new X()`** is a `CALLS` edge to the class `X` (not to its constructor),
  as with Python's `X()`. Every class has a node even without an explicit
  constructor, and "this code uses class X" is the useful impact granularity.
- **`super(...)`** is a `CALLS` edge from the constructor to the resolved
  base class (`call_kind: "super"`); an unresolved base keeps its rightmost
  written name (implemented, #17).
- **JSX** `<Comp />` and `<UI.Comp />` are `CALLS` edges to the component, so
  flows follow the component tree. Lowercase intrinsic elements (`<div />`)
  are ignored.
- **Tagged templates** (``tag`x` ``) are `CALLS` edges to `tag`.

### 5.4 Tests

- **Test files.** A JavaScript or TypeScript file is a test file (File
  `is_test: true`) when any of these holds:
  - it matches the shared rules used by every language (`test/`, `tests/`,
    a leading `test_`)
  - its name ends in `.test.<ext>`, `.spec.<ext>`, or Cypress `.cy.<ext>`,
    where `<ext>` is `js`, `jsx`, `ts`, `tsx`, `mjs`, `cjs`, `mts`, or `cts`
  - a directory on its path is `__tests__`, `e2e`, `e2e-tests` /
    `e2e_tests`, or `cypress`
  The flow-tracing and dead-code test-file patterns
  (`crates/dagayn-graph/src/flow_trace.rs`, `dagayn/refactor/dead_code.py`)
  accept the same suffixes.
- **Test declarations.** Only inside test files, a call whose callee chain
  starts with a runner name is a synthetic `Test` node:
  - runners: `describe`, `it`, `test`, `suite`, `specify`, Jasmine
    `fdescribe` / `xdescribe` / `fit` / `xit`, and Mocha `context` (only
    with a literal title, and not when the file defines `context`)
  - chained modifiers: `only`, `skip`, `todo`, `concurrent`, `sequential`,
    `shuffle`, `fails`, `failing`, `fixme`, `slow`, `serial`, `parallel`;
    they are recorded in source order as `extra.test_modifiers`
  - Playwright `test.describe(...)` (and `test.describe.only(...)`) is a
    `describe` test; the name uses the last runner in the chain
  - table and conditional factories `each`, `for`, `skipIf`, `runIf`:
    `test.each(table)("adds %i", fn)` and
    ``it.each`a | b`("adds $a", fn)`` are one test named after the outer
    call's title, spanning the whole outer call. The inner
    `test.each(table)` call is not a node.
- **Names** are `runner:title@Lline`, where `title` is the first argument:
  a string or template literal, or an identifier / member expression
  (`describe(UserService, fn)` gives `describe:UserService@L3`). Without a
  title the name is `runner@Lline`. A call with neither a literal title nor
  a function argument (Playwright `test.skip()` or
  `test.skip(isMobile, "reason")` inside a test body) is not a
  declaration.
- **Nesting.** A nested test is `CONTAINS`ed by the enclosing `describe`
  test (or the File at module scope). The owner path stays that of the
  surrounding declarations, so `describe` titles never enter QNs.
- **Other runner APIs** (hooks `beforeEach` / `afterEach` / `beforeAll` /
  `afterAll` / `before` / `after`, `test.step`, `test.use`,
  `test.describe.configure`, `test.extend`, and the factory call itself)
  get no node and no edge; the calls inside their callbacks belong to the
  enclosing node (§5.1).
- **Assertion and mock APIs.** Calls whose callee chain starts with
  `expect`, `assert`, `vi`, `vitest`, `jest`, `sinon`, `chai`, `cy`, or
  `Cypress` keep their `CALLS` edge with `test_api: true`, and never produce
  `TESTED_BY`. Calls into external packages (`extra.external`, §7.6) do not
  either. `TESTED_BY` is derived from every other `CALLS` edge whose source
  is a `Test` node.
- **Resolved in post-processing.** A bare call target (`box.helper()` on an
  untyped local) gives a bare `TESTED_BY helper -> test`. When
  `resolve_bare_call_targets` binds the call (`src/classes.ts::Box.helper`,
  `MEDIUM`), the `TESTED_BY` edge from the same test, file, and line with
  the same name takes the resolved QN and confidence. This runs for every
  language, is idempotent, and also repairs graphs whose calls were resolved
  by an earlier run.

### 5.3 Inheritance

- TypeScript `extends_clause` and JavaScript `class_heritage` are read by the
  shape of the base expression:
  - identifier: `Base`
  - member expression: `ns.Base` resolves through a namespace import when
    possible; otherwise the rightmost name `Base` is used, and the full text
    is kept as `heritage_expression`
  - generic: `Array<number>` gives `Array`; the type arguments are type
    references (§7.4), not bases
  - call (mixin): `extends Mixin(Base)` gives `INHERITS -> Base` (the
    innermost identifiable argument) with `heritage_expression: "Mixin(Base)"`,
    plus `CALLS class -> Mixin`. Neither edge is sourced from the File.
- `implements A, Service<T>, ns.Marker` yields one `IMPLEMENTS` per type.
- Interface `extends A, B<T>` yields `INHERITS` edges
  (`relationship_role: "extends"`), as Java does for interface extension.
- Identifier bases stay bare; `resolve_bare_inheritance_targets` binds them
  through same-file declarations and import visibility. Member-expression and
  qualified-type bases rooted at an imported module binding (`ns.Base`,
  `ns.Marker`) resolve to the exporting module's QN.
- Only the declaration's own heritage is read: a class declared inside a
  method body never contributes bases to the enclosing class.

## 6. Resolution

### 6.1 Same file

A bare target that names a same-file declaration becomes that declaration's
QN. When several members share the name, the member of the caller's nearest
owner wins (`Outer.Inner.run` tries `Outer.Inner`, then `Outer`). A receiver
bound to an owner path (`this` inside `Outer.Inner`) resolves `this.m()`
through `Outer.Inner::m` (implemented, #11). Receivers bound by `const x = new X()` or an annotation `x: X` rewrite
`x.m()` to `X.m`.

#### 6.1.1 Member calls

A member call `recv.m()` binds only with evidence about the receiver
(implemented, #17):

- a same-file object container or namespace (`api.get()`, `Outer.helper()`)
- a namespace import (`fns.decl()`, `fns.api.get()`) or a named / default
  import of a namespace or object container (`Outer.Deep.deepFn()`)
- a receiver whose class or interface is known, in this file or imported:
  `this`; a class named directly (`Box.create()`, `ns.Box.create()`); a
  variable bound by `new X()`, a `x: X` annotation, or a typed parameter;
  `new X().m()`; a field of a known class with a declared type (parameter
  properties `constructor(private repo: Repo)`, field annotations
  `repo: Repo`, `repo = new Repo()`, and JavaScript
  `this.repo = new Repo()` in the constructor), so `this.repo.find()` is
  `Repo.find`
- `super.m()`: the nearest base declaring `m`

The member is looked up on the type, then on its bases (nearest first, up to
eight levels, across files through each module's imports); a member found
on a base (`this.helper()` inherited, `super.m()`) is `MEDIUM`
(`confidence: 0.6`). Merged declarations (`interface Repo` twice) are one
shape. Any other receiver (`res.json()`, an untyped `this.users`, a known
class without that member) keeps the bare member name with
`receiver_unknown: true`, and same-file resolution leaves such edges alone:
they never bind to the first same-named method or to the caller itself.

### 6.2 Imports

Imports are bound by `(module, exported name)`:

| Import | Binding |
|---|---|
| `import { a } from "./m"` | `./m`, `a` |
| `import { a as b } from "./m"` | local `b` -> `./m`, `a` |
| `import X from "./m"` | `./m`, `default`; resolves to the symbol the module exports as default; `<X.C />` and `extends X.C` are read like namespace members (CommonJS interop) |
| `import { default as X } from "./m"` | same as `import X from "./m"` |
| `import * as ns from "./m"` | `./m`, namespace; `<ns.C />` and `extends ns.C` resolve to `m::C` |
| `const m = require("./m")`, TypeScript `import m = require("./m")` | `./m`, what `require` returns: the CommonJS value (`module.exports`, `export =`) of a module that has one, otherwise its namespace; `m.a()` and `m()` (for `module.exports = fn`) resolve |
| `const { a, b: c } = require("./m")` | `a` -> `./m`, `a`; local `c` -> `./m`, `b` |
| `const c = require("./m").b` | local `c` -> `./m`, `b` |

`require` bindings are read at module scope only (`const` / `let` / `var`,
including `export const`); a `require` inside a function still emits
`IMPORTS_FROM`, but its binding is a local of the function. A `require` that
the file shadows (a declaration, import, or local named `require`) is an
ordinary call. `require(expr)` and `import(expr)` with a non-literal
specifier emit no edge, and `require` never gets a `CALLS` edge.
`require.resolve(...)` is not an import. `await import("./m")` bindings are
not read.

The export index of the target module maps exported names to declarations:
local declarations, `export { a as b }`, `export { a } from`, `export * from`,
and `default` (`export default <declaration>`, an anonymous default named
`default`, `export default ident`, `export { x as default }`, TypeScript
`export = ident`). If a module has no default export at all, a default import
falls back to the importer's local name, which keeps code that relies on
bundler interop resolvable.

Re-exports are followed to the origin:

- A local re-export of an import (`import { a } from "./x"; export { a as b }`,
  `import X from "./x"; export default X`) follows that import.
- `export * as ns from "./m"` (and `import * as ns from "./m"; export { ns }`)
  exports the module object of `m`: `import { ns } from "./barrel"; ns.f()`
  and `import * as b from "./barrel"; b.ns.f()` resolve to `m::f`, and
  `ns.Type` to `m::Type`.
- `export * from` follows ES semantics: sources are searched transitively
  (cycles stop), explicit exports of the module win, `default` is never
  re-exported, a source contributes only names it exports (declared with
  `export` or listed in an export clause; every declaration of a `.d.ts`),
  and a name that two sources export as different bindings is ambiguous and
  binds to neither.

CommonJS exports in `.js`, `.jsx`, and `.cjs` files feed the same index, for
ES imports of CommonJS modules and for `require` bindings: top-level
`module.exports = { a, b: fn, c() {} }` exports `a`, `b` (-> `fn`), and `c`;
`module.exports.x = V` and `exports.x = V` export `x` (-> `V` when it is an
identifier); a `require("./m")` value is `m`'s module object. The exports
object is the module's `default` (so `import m from "./cjs"; m.a()` resolves
to `a`), unless `module.exports = X` names `X`. Dynamic patterns
(`module.exports[k] =`, `Object.assign(module.exports, ...)`, assignments
inside functions or blocks, `module.exports = require("./m")` re-exporting all
of `m`'s names) are not read.

### 6.3 Module paths

Relative specifiers are resolved against the importing file by trying, in
order:

1. the path as written, if it is a file
2. runtime-to-source extension mapping: `.js` -> `.ts` / `.tsx` / `.d.ts`,
   `.jsx` -> `.tsx`, `.mjs` -> `.mts` / `.d.mts`, `.cjs` -> `.cts` / `.d.cts`
3. the path with `.ts`, `.tsx`, `.d.ts`, `.js`, `.jsx`, `.mjs`, `.cjs`,
   `.mts`, `.cts`, `.vue` appended, so `./user.service` finds
   `user.service.ts`
4. `index.*` with the same extensions when the path is a directory

A file wins over a directory with the same stem, and an implementation file
wins over its `.d.ts`. Non-relative specifiers go through the nearest
project config, using the same candidate list:

- **Which config.** The nearest directory from the importer up that holds a
  `tsconfig.json`, `tsconfig.app.json`, or `jsconfig.json` wins, so each
  package of a monorepo uses its own config and a package without one uses
  the root's. Within that directory, the first of those files that sets
  `paths` or `baseUrl` is used (a solution-style `tsconfig.json` with
  `files: []` and `references` does not hide `tsconfig.app.json`).
- **`paths`.** Patterns are tried from the most specific; replacements are
  relative to `baseUrl`, or to the config's directory without one.
- **`baseUrl`.** A specifier that no `paths` pattern matches is looked up
  under `baseUrl`, as TypeScript does: `"baseUrl": "src"` resolves
  `import "services/user"` to `src/services/user.ts`. Only an existing file
  counts, so `react` stays a package unless `src/react.ts` (or
  `src/react/index.ts`) exists.
- **Not followed.** `extends` chains are not read (deferred): a package config
  that inherits its `paths` from a shared `tsconfig.base.json` resolves
  only its own `paths` / `baseUrl`, and the inherited aliases stay
  unresolved (a scoped alias such as `@shared/log` is then treated as a
  package). Project references and `include` / `files` selection are out of
  scope (§9).

A specifier that is a package name (`react`,
`@scope/name`, `lodash/fp`, `node:fs`), resolves to no file, matches no
`paths` pattern other than `*`, and whose first segment does not exist
under `baseUrl` is external (§7.6). Anything else that fails to resolve
(`./gone`, an alias whose file is missing) is an unresolved in-repo module:
its names stay bare.

### 6.4 Confidence

- Resolutions that the parser derives from syntax (same file, import plus
  export index, receiver bindings) carry no confidence override (`EXTRACTED`).
- Bare names resolved in post-processing through import visibility are
  `MEDIUM`.
- Targets without a node (`pkg::symbol`, unresolved names) become `LOW`
  through `demote_unresolved_endpoint_edges`. There is no separate tier for
  external packages: `LOW` says "no node in this graph", and
  `extra.external: true` tells an external dependency apart from a dangling
  in-repo name (§7.6).

## 7. Decisions

### 7.1 Interfaces and enums stay `Class`; type aliases become `Type`

Interfaces keep `Class` with `type_role: "interface"`. Java, C#, Go, Rust
traits, and Python protocols all model contracts this way, and SAP
abstractness, dead-code structural exclusions, and the `IMPLEMENTS` /
`INHERITS` bare resolver (which indexes `Class` nodes only) depend on it.
Making interfaces `Type` would drop SAP abstractness to zero and break
`class Foo implements IBar` resolution. Enums stay `Class`, as in Java, C#,
Rust, and Dart.

Type aliases become `Type` with `type_role: "alias"`, matching Python
`type X = ...`, Rust `type`, and C# `using X = ...`. An alias is not nominal,
is never called or subclassed, and is not an SAP-eligible role, so SAP numbers
do not change. Object-shaped aliases (`type Props = { ... }`) additionally
carry `container_role: "data_container"`. Because the bare inheritance
resolver indexes `Class` nodes, it falls back to `Type` nodes when no class
matches, so `interface X extends Props` still resolves through imports.
Implemented (#13).

### 7.2 Local declarations are not nodes

Nested functions, local arrow handlers, and local classes inside a function
body are not nodes; their calls are attributed to the enclosing function
(§5.1), and calls of the local declarations themselves (`inner()`,
`new Local()`, `<Local />`) are not edges, because the target has no node and
a bare name could otherwise be resolved to an unrelated symbol. Local
declarations are also left out of the file's defined-name and type-name
indexes. In React and callback-heavy code these locals are numerous, collide by
name across functions (many components define `handleClick`), and otherwise
show up as false flow entry points because nothing calls them by name. Python
flattens nested functions today; TypeScript deliberately differs.
Implemented (#15).

### 7.3 Module-scope object literals are containers

`export const api = { get() {}, post: () => {}, put: function () {} }` yields
`Class api` (`type_role: "object"`) containing `api.get`, `api.post`, and
`api.put`. Before this, `get` became a top-level function, `post` and `put`
were not nodes at all, and a full build resolved Express's `app.get(...)` to
the flattened `get`. A container is created only when the object has at least
one function-valued member; `as const`, `satisfies T`, and parentheses are
unwrapped. Nested objects become containers when they hold a function-valued
member at some depth below them, up to six levels (`api.a.b.c` gives
`Class api.a`, `Class api.a.b`, and `Function api.a.b.c`); data-only nested
objects are not nodes. Container members are reachable only through the
container: same-file `api.get()`, `api.nested.deep()`, and `this.m()` inside
a member bind to the member, but a bare `get` never does, neither in the
parser's same-file fallback nor in post-processing's bare-name resolution.
Dead-code analysis excludes `object`, `namespace`, and `ambient_module`
containers from its candidates.

### 7.4 Type references are `REFERENCES` everywhere

References to types become `REFERENCES` edges with
`relationship_role: "type_reference"` (or `"type_query"` for `typeof X`), both
in declaration signatures (parameters, return types, type parameters and
their constraints, heritage type arguments, field and parameter-property
types, alias right-hand sides, interface member types) and in bodies
(variable annotations, `as`, `satisfies`, call type arguments). Targets are
limited to types declared in the same file or imported (including
`import type`); builtin and global types (`string`, `Promise`, `Map`) are
skipped, and edges are de-duplicated per `(source, target)`. TypeScript code
often depends on another module only through types, so without these edges
the blast radius of an interface change is visible only at file granularity.
Using `REFERENCES` keeps these dependencies out of `CALLS`-based flows.
Implemented for signatures (#23) and bodies (#24) in `js_types.rs`.

**Edge shape.** `REFERENCES source -> type` with
`extra.relationship_role` (`"type_reference"`, or `"type_query"` when every
occurrence is a `typeof X` operand) and `extra.type_positions`, the list of
positions the type appears in, in source order and each once. One edge exists
per `(source, target)`; its `line` is the first occurrence. Aggregating the
positions (rather than keeping one) keeps the edge count independent of how
often a type is repeated while still telling a return-type dependency from a
field one. Positions:

| `type_positions` value | Written as | Source |
|---|---|---|
| `parameter` | `f(a: Repo)`, `this: Window` | the function or method |
| `parameter_property` | `constructor(private repo: Repo)` | the class (the parameter declares a field) |
| `return` | `f(): Repo`, method / call / construct signature return | the function, method, or interface |
| `type_predicate` | `x is Repo`, `asserts x is Repo` | the function |
| `field` | class field, interface property signature, the annotation of a function-valued field (`handler: Handler = () => ...`) | the class / interface; the field's own node for a function-valued field |
| `index_signature` | `[key: string]: Repo` | the class or interface |
| `type_parameter_constraint`, `type_parameter_default` | `<T extends Repo = Repo>` | the declaration owning the type parameters |
| `heritage_type_argument` | `implements Service<User>`, `extends Base<Props>` | the class or interface (the base itself stays `INHERITS` / `IMPLEMENTS`) |
| `type_alias` | `type Pair = [User, Repo]` | the `Type` alias |
| `variable_annotation` | `const u: User = ...`; a module-scope `const h: Handler = () => ...` / `const api: Api = { ... }` | the enclosing function; the function / object container / class the binding becomes; else the enclosing namespace or the file |
| `as` | `x as User`, `<User>x` | the enclosing node |
| `satisfies` | `x satisfies Shape` | the enclosing node |
| `type_argument` | `pick<UserId>(ids)`, `new Repo<User>()` (the class itself stays `CALLS`) | the enclosing node |
| `instanceof` | `x instanceof Repo`, `x instanceof ns.Repo` (also JavaScript) | the enclosing node |
| `local_declaration` | an interface, type alias, or enum declared in a function body | the enclosing function |
| `heritage` | `extends` type arguments and `implements` types of a local or unbound class expression | the enclosing node |

Overload signatures collapse into one node (§2), so their types merge into
that node's edges. Code that is not a node follows the attribution of calls
(§5.1, §7.2): a local function's or callback's parameter and return types,
a local class's member signatures, and an object-literal method's signature
in a body belong to the enclosing node with their own positions
(`parameter`, `return`, `field`); module-scope code belongs to the file. The
names of local declarations are never targets, and neither are types they
shadow. `instanceof` is included because its right-hand side can only be a
class, so the resolver's type-only targets never mistake a value for it.

**Targets.** A name resolves where TypeScript would look it up: first
through the enclosing namespaces (`Circle` inside `namespace Shapes` is
`Shapes.Circle`), then as a same-file declaration, then through imports
(named, aliased, default, namespace `m.User`, re-exports, `export * as`),
walking `ns.Type` / `Outer.Inner` paths like member calls do (§6.1.1). The
target must be a class, interface, enum, or type alias the target module
declares at module or namespace scope (`typeof X` may also name a function,
class, or namespace member). Builtin and global types (`string`, `Promise`,
`Map`, DOM types), external packages, names the module does not declare,
type parameters, mapped-type keys (`[K in keyof T]`), `infer U`, and names of
function-local declarations produce no edge. This is a deliberate exception
to §2's "unresolved stays bare": a bare type name could not be bound safely
later and would only add LOW-confidence dangling edges. External types
stay without an edge as well, even though calls into packages became
`pkg::symbol` (§7.6). Self references
(`interface Tree { children: Tree[] }`) are dropped.

**Analysis consumers.** The `strict_static` and `implementation` dependency
profiles do not count `REFERENCES`, so SAP / SDP and cycle metrics under them
do not change; `infra_dataflow` counts them. Flows follow `CALLS` only.
Impact radius expands through every non-bridge edge kind, so an interface
change now reaches the declarations that use it as a type. Dead-code analysis
counts an incoming `REFERENCES` as usage, so a class named only in another
file's field or return type is no longer reported. Rename previews gain the
first reference line of each edge (not every occurrence).

**Graph size.** Measured on the TypeScript parity fixture
(`tests/fixtures/parity/typescript`, 36 files): 377 edges before, 395 with
signature references (+18), 400 with body references (+23 in total,
+6.1%; body references add 5 edges and 2 positions on existing ones). On
`dagayn-vscode/` (49 TypeScript files): 4,646 edges before, 4,867 with
signatures (+221), 4,966 with bodies (+320 in total, +6.9%). Its 320 type
edges carry these positions: 102 parameter, 76 return, 57 as, 50 variable
annotation, 26 field, 12 type alias, 11 type argument, 8 instanceof,
5 parameter property, 2 type predicate, 1 heritage type argument, and
1 local declaration. Nodes do not change; the JavaScript parity fixture
has no `instanceof` of a repository class, so its output does not change.

### 7.5 Anonymous default exports are named `default`

`default` is the ES module export name itself, so `import X from "./m"`
resolves mechanically to `m::default` without depending on the importer's
local name. Names derived from the file name were rejected because they
collide and break on rename.

### 7.6 External package symbols are `pkg::symbol`

A bare `render` or `get` imported from a package used to be resolved by
post-processing to an unrelated local method of the same name: in the
sample project, a test's `render` became `CALLS -> ClassComp.render`
(`MEDIUM`), and its `TESTED_BY` marked that method as tested. Qualifying the
name with the package keeps the dependency explicit and takes it out of
every name-based resolution. Implemented (#25).

**Which imports.** A binding from `import`, `require`, or
`import x = require` whose specifier is external (§6.3): a package name that
resolves to no file and is not a tsconfig alias. Relative specifiers that do
not resolve and aliases whose file is missing are in-repo modules, so their
names stay bare.

**Target.** `<specifier>::<path>`, with the specifier as written so the
target lines up with the file's `IMPORTS_FROM` edge:

- named import: the exported name (`import { useState as useLocal }` gives
  `react::useState`), and member chains keep the path
  (`z.object()` from `zod` gives `zod::z.object`)
- namespace import: the member path (`fs.readFile()` gives
  `node:fs::readFile`, `path.posix.join()` gives `path::posix.join`); the
  namespace alone is not callable and stays bare
- default import and `require` binding: alone it is `pkg::default`
  (`express()` gives `express::default`); its members are the module's own,
  as for in-repo default imports (CommonJS interop), so `React.useEffect()`
  and `import { useEffect }` both give `react::useEffect`
- the same rule covers `CALLS` (calls, `new`, JSX `<Button />` /
  `<Form.Item />`, class-heritage mixin calls), value `REFERENCES`
  (`app.use(cors)` gives `cors::default`), and decorator `REFERENCES`
  (`@nestjs/common::Injectable`). `INHERITS` / `IMPLEMENTS` keep the written
  base name.
- a value returned by an external call has no known type:
  `const app = express(); app.get("/")` keeps the bare `get` with
  `receiver_unknown: true` (§6.1.1). Guessing `express::default().get`
  would need the package's typings.
- a local that shadows an import (`const pick = ...; pick()`) is the local,
  so it produces no edge.

**Metadata.** These edges carry `extra.external: true` and
`extra.external_package`, the package name without a subpath
(`@testing-library/react`, `lodash` for `lodash/fp`, `node:fs`). Rust call
targets (`serde_json::to_string`) already use the same `crate::path` shape
without the flag; Python and Go keep bare names.

**Consumers.**

- Parse time: same-file resolution skips external edges, and a call from a
  test into a package produces no `TESTED_BY` (an external package is never
  the code under test).
- Post-processing: `resolve_bare_call_targets` and
  `load_bare_name_index` only consider targets without `::`, so external
  targets are never re-bound. `demote_unresolved_endpoint_edges` makes them
  `LOW` (§6.4).
- Dead code and the query-time name fallback of `callers_of` /
  `inheritors_of` ignore external edges, so `date-fns::format` no longer
  keeps a project `format` alive or shows up as its caller.
  `callers_of("react::useState")` lists the callers of the package symbol
  (`resolution: "external_package"`).
- Flows count a call whose target is not a node as external in the
  criticality score, so a call that used to be misresolved to a local method
  now counts as external. Risk scores and the risk index key on node QNs;
  they lose only the callers and tests that misresolution had attached.
- Impact radius walks edges by QN and never returns a non-node, as it
  already did for bare names; the callers of one package symbol stay linked
  through its target, as they were through the bare name.

**Types stay without an edge.** External types (`FC` from `react`,
`vscode.Uri`) produce no `REFERENCES` (§7.4). Type references exist to connect
in-repo declarations for impact radius and dead code, and a package type
connects nothing; `IMPORTS_FROM` already records the dependency. Measured
with a `pkg::Type` variant, they would add 107 edges on `dagayn-vscode/`
(102 of them `vscode::*`, +33% over its 320 in-repo type edges) and 1 on
the TypeScript parity fixture, all `LOW`.

**Measured impact.** The TypeScript parity fixture keeps 400 edges: 8
`CALLS` and 6 decorator `REFERENCES` change target (`react::useState`,
`express::Router`, `@nestjs/common::Get`, ...). On `dagayn-vscode/` (49
TypeScript files) 833 `CALLS` and 2 `REFERENCES` become external (388
`node:assert`, 237 `vscode`, 98 `node:path`, 67 `node:fs`, ...); `CALLS`
grow by 2 (4,942 -> 4,824 edges in total, both built from the same copy) because a chained
`d3.zoomIdentity.translate().scale().translate()` no longer merges its two
bare `translate` calls, and 120 dangling `TESTED_BY` edges from tests into
packages disappear. `MEDIUM` edges and flows do not change there, since no
imported package name matched a project symbol. In the sample project the
test's `render` is a global; with
`import { render } from "@testing-library/react"` added, the call becomes
`@testing-library/react::render` (`LOW`) and the `ClassComp.render` `CALLS`
/ `TESTED_BY` edges are gone.

### 7.7 Decorators are metadata plus `REFERENCES`

Decorated classes and members record `extra.decorators` (callee names, the
format Python uses), which enables framework entry-point detection and
dead-code exclusion for NestJS and Angular. The decorator itself becomes
`REFERENCES decorated -> decorator` (`relationship_role: "decorator"`) instead
of a `CALLS` edge from the File. The entry-point decorator patterns live in
both `dagayn/entry_point_heuristics.py` and
`crates/dagayn-graph/src/flow_trace.rs`; `tests/test_flows.py` checks that
the two lists are identical. Implemented (#16).

### 7.8 Wrapped functions and function-valued fields are functions

A module-scope binding whose value is a wrapper call around an inline
function literal becomes the `Function` node of that binding, with the
wrapped function's parameters, return type, and body:
`const Comp = memo(function Inner() {...})`, `memo(() => ...)`,
`forwardRef((props, ref) => ...)`, `React.memo(...)`, `observer(...)`,
`memo(forwardRef(fn), areEqual)`. Importers and JSX name the binding, so
`<Comp />` and `import { Comp }` resolve to it; before, the binding was not a
node and the body's calls belonged to the File.

The rule is syntactic, not a list of known HOCs:

- the call's first argument is a function literal (`function`, arrow,
  generator), or another call that satisfies the rule (up to four levels);
- the callee is a plain identifier (`memo`, `observer`, `debounce`,
  `asyncHandler`), or a member of an imported / required binding or of the
  `React` global (`React.memo`, `mobx.observer`). A method of a local value
  (`items.map(x => ...)`, `promise.then(...)`) is not a wrapper, and
  neither is a call without an inline function (`compose(a, b)`,
  `withRouter(Page)`), `new`, or a tagged template.

The wrappers are recorded as `wrapped_by` (callee text, outermost first).
The wrapper calls run where the binding is declared, so their `CALLS`
(`react::memo`) and their other arguments (`areEqual`) belong to the
container (the File, or the namespace), while the wrapper's type arguments
(`forwardRef<HTMLInputElement, Props>`) are `REFERENCES` from the new node.
The inner function name is only `expression_name`, as for class
expressions. `export default memo(function Page() {})` is `Function default`
(there is no binding), and the export index maps `default` to it. Inside a
function body such bindings stay locals (§7.2).

A class field holding a function literal is a method in both languages:
`handle = () => {...}` and `handle = function () {...}` give
`Function Class.handle`, and `this.handle()` resolves to it. JavaScript's
`field_definition` is read like TypeScript's `public_field_definition`. A
function-valued field no longer counts as a data field, so a class with one
is not a property-only `data_container`. Implemented (#26).

## 8. JavaScript parity

| Topic | Behavior |
|---|---|
| class heritage | JavaScript `class_heritage` holds `extends <expression>` directly; it is read with the same expression rules as TypeScript |
| class fields | JavaScript `field_definition` is treated like `public_field_definition`: function-valued fields are methods (§7.8, #26) |
| generators | `function*` / `async function*` declarations and generator methods are nodes in both languages |
| JSX | `.js`, `.jsx`, and `.mjs` parse with JSX; component calls behave as in TSX |
| test naming | `Test*` / `test_*` / `*_test` / `*_spec` names mark `Test` nodes only inside test files |
| CommonJS | `module.exports` and `exports.x` feed the export index (§6.2, #18); `require("./m")` is `IMPORTS_FROM` and module-scope `require` bindings resolve through the index (§6.2, #19) |
| types | JavaScript has no type syntax; JSDoc types are out of scope |

## 9. Out of scope

| Item | Reason |
|---|---|
| type checking and inference (return-type propagation, generic instantiation, overload selection, control-flow narrowing) | needs a TypeScript compiler; bindings stay limited to annotations and `new` |
| tsconfig project references and per-file tsconfig selection through `include` / `files` | requires a project graph; the nearest tsconfig covers most repositories |
| `package.json` `exports` / conditions, indexing `node_modules` typings | dagayn does not index external dependencies |
| runtime DI resolution (which provider a NestJS token binds to) | depends on runtime configuration |
| dynamic dispatch: `obj[key]()`, `eval`, `Proxy` | not statically decidable |
| JSDoc types, Flow | separate grammar or low priority |
| `.astro` frontmatter and MDX | need embedded-language extraction; tracked separately |
| cross-language alignment (Java `new` not being `CALLS`, Java nested-class `CONTAINS` sources, Python decorator `CALLS`) | belongs to other extractors |

## 10. Coverage matrix

Status values:

- `implemented (existing)`: already correct before this effort
- `implemented (#N)`: delivered by track TODO `#N`
- `planned (part 2/3, #N)`: specified here and delivered by a later TODO
- `deferred`: specified but not scheduled

QNs omit the `file::` prefix.

### 10.1 Classes and members

| Construct | Expected | Status |
|---|---|---|
| `class Box {}` | `Class Box` (`class`), `CONTAINS File -> Box` | implemented (existing) |
| `abstract class B {}` | `Class B` (`abstract_class`, `is_abstract`) | implemented (#2) |
| `declare abstract class D {}` | same as above | implemented (#2) |
| `abstract area(): number;`, `abstract get label()` | `Function B.area` / `B.label` (`is_abstract`) | implemented (#2) |
| concrete method of an abstract class | `Function B.describe` with parent `B`; `this.area()` -> `B.area` | implemented (#2) |
| `export default class DefaultShape` | `Class DefaultShape` (`export_default`) | implemented (#7) |
| `export default class { hello() {} }` | `Class default`, `Function default.hello` | implemented (#7) |
| `const Anon = class { run() {} }` | `Class Anon` (`class_expression`), `Function Anon.run` | implemented (#7) |
| `const Named = class InnerName {}` | `Class Named` (`expression_name: "InnerName"`) | implemented (#7) |
| `extends Base` | `INHERITS -> Base` | implemented (existing) |
| `extends ns.Base` | `INHERITS -> lib/base.ts::Base` (bare `Base` if unbound) | implemented (#4) |
| `extends Array<number>` | `INHERITS -> Array` | implemented (existing) |
| `extends Mixin(Base)` | `INHERITS -> Base` (`heritage_expression`), `CALLS class -> Mixin` | implemented (#4) |
| `implements Repo, Logger` | two `IMPLEMENTS` | implemented (existing) |
| `implements Service<string>, ns.Marker` | `IMPLEMENTS -> Service`, `-> Marker` | implemented (#4) |
| `export default class extends Base {}` | `Class default`, `INHERITS default -> Base` | implemented (#4, #7) |
| class / member decorators | `decorators: ["Injectable"]` on the class, method, or function-valued field (callee names, as Python records them; `ns.Dec` stays dotted); non-function field decorators in the class's `member_decorators`; `REFERENCES decorated -> decorator` (`relationship_role: "decorator"`), no `CALLS` | implemented (#16) |
| parameter decorators `m(@Inject(T) x)` | `REFERENCES m -> Inject` (`decorator`); not in `decorators` metadata | implemented (#16) |
| `@Entity()` / `@ObjectType()` on an exported class | `container_role: "data_container"` | implemented (#16) |
| `constructor(private repo: Repo)` | `Function Box.constructor`; `this.repo` bound to `Repo` | node implemented (existing); binding implemented (#17) |
| `super(repo)` | `CALLS Box.constructor -> Base` (`call_kind: "super"`) | implemented (#17) |
| field initializer `svc = new UserService()`, `static {}` block, `[Symbol.iterator]() {}` body | `CALLS Class -> UserService` (the class is the caller) | implemented (#15) |
| function-valued field `handler = () => this.helper()`, `h = function () {}` | `Function Box.handler`, `CALLS -> Box.helper`; the class is not a property-only `data_container` | implemented (existing) for TS; JS and the `data_container` fix (#26) |
| `static create()`, `async load()` | `Function Box.create` / `Box.load` | implemented (existing) |
| generator method `*items()` | `Function Box.items` | implemented (existing; covered by #3 tests) |
| `get value()` + `set value(v)` | one `Function Box.value` (`member_role: "accessor"`, `accessors: ["get", "set"]`), spanning both | implemented (#14) |
| overloads `m(a: string); m(a: number); m(a) {}` | one `Function Box.m` (`overloads: 2`), spanning the signatures and the implementation, no `declaration_only` | implemented (#14) |
| function overloads, `declare function` overloads | one `Function f` (`overloads: n`; `declaration_only` kept when there is no implementation) | implemented (#14) |
| `#privateMethod() {}`, `#handler = () => ...`, `this.#privateMethod()` | `Function Box.#privateMethod` / `Box.#handler`, `CALLS -> Box.#privateMethod` | implemented (#14) |
| `"quoted-name"() {}`, `42() {}`, `["computed"]() {}` | `Function Box.quoted-name` / `Box.42` / `Box.computed` | implemented (#14) |
| other computed member names (`[Symbol.iterator]()`) | no node | deferred |
| `function f` + `namespace f`, `class C` + `interface C`, `enum E` + `namespace E` | one node (the function / class / enum), `merged_declarations: 2` | implemented (#14) |
| declaration modifiers | `modifiers` column: `static`, `async`, `*`, `get`, `set`, `readonly`, `public` / `private` / `protected`, `override`, `declare`, `abstract`, `accessor` (space-separated, source order) | implemented (#14) |
| `export ...` / `export { local }` | `exported: true` on the declaration (module or namespace scope) | implemented (#14) |
| `this.helper()` (same class) | `CALLS -> Box.helper` | implemented (existing) |
| `this.helper()` (inherited) | `CALLS -> Base.helper` (MEDIUM) | implemented (#17) |
| `this.repo.find()` | `CALLS -> interfaces.ts::Repo.find` | implemented (#17) |
| `super.m()` | `CALLS -> Base.m` (MEDIUM) | implemented (#17) |

### 10.2 Interfaces, type aliases, enums

| Construct | Expected | Status |
|---|---|---|
| `interface Repo {}` | `Class Repo` (`interface`, `is_abstract`, `is_contract`) | implemented (existing) |
| interface `extends Repo, Logger, ns.X<T>` | one `INHERITS` per base (`extends`) | implemented (#4) |
| method signature `find(): string;` | `Function Repo.find` (`is_abstract`) | implemented (existing) |
| same-file interface merging | one `Class Repo` (`merged_declarations: 2`) holding the members of both declarations | implemented (#14) |
| `type Props = { a: A }` | `Type Props` (`alias`, `alias_form: "object"`, `data_container`) | implemented (#13) |
| `type U = A \| B` | `Type U` (`alias`, `alias_form: "union"`) | implemented (#13) |
| `interface X extends Props` (imported alias) | `INHERITS -> types.ts::Props` (MEDIUM) | implemented (#13) |
| `enum Color {}` | `Class Color` (`enum`, `data_container`) | implemented (existing) |
| `const enum`, `declare enum` | `Class` plus `const_enum` / `ambient` | implemented (#12, #13) |

### 10.3 Namespaces, ambient declarations, `.d.ts`

| Construct | Expected | Status |
|---|---|---|
| `namespace Outer { export function helper() {} }` | `Class Outer` (`namespace`), `Function Outer.helper` | implemented (#11, #12) |
| `namespace A.B.C {}` | nested `A`, `A.B`, `A.B.C` | implemented (#11, #12) |
| `module Legacy {}` | `Class Legacy` (`namespace`) | implemented (#12) |
| `namespace Outer {}` declared twice in one file | one `Class Outer` holding both bodies' members | implemented (#12) |
| class / object container inside a namespace | `Class Outer.Inner`, `Function Outer.Inner.run`, `Function Outer.api.get` | implemented (#12) |
| same-file `Outer.helper()`, `A.B.C.abc()`, `new Outer.Inner()`, `x.run()` on `x = new Outer.Inner()` | `CALLS -> Outer.helper` / `A.B.C.abc` / `Outer.Inner` / `Outer.Inner.run` | implemented (#12) |
| bare `helper()` inside `Outer.Inner.run` | `CALLS -> Outer.helper` (nearest owner) | implemented (#11, #12) |
| `import { Outer } from "./ns"; Outer.helper()` | `CALLS -> ns.ts::Outer.helper` | implemented (#17) |
| `declare module "external-lib" {}` | `Class external-lib` (`ambient_module`, `ambient`), members `external-lib.ext` | implemented (#12) |
| `declare global {}` | `Class global` (`ambient_module`, `ambient`), members `global.Window` | implemented (#12) |
| `declare namespace NS {}` | `Class NS` (`namespace`, `ambient`) | implemented (#12) |
| `declare function f(): void;` | `Function f` (`ambient`, `declaration_only`, no `is_abstract`) | implemented (#12) |
| `declare class D { m(): void; }` | `Function D.m` (`ambient`, `declaration_only`, no `is_abstract`) | implemented (#12) |
| `.d.ts` file | File `declaration_file: true`; every node `ambient` | implemented (#12) |
| `.d.mts` / `.d.cts` file | parsed like `.d.ts` | implemented (#20) |
| `export as namespace MyLib` | File `umd_global: "MyLib"` | implemented (#12) |
| `declare module "./x"` augmentation | `IMPORTS_FROM` (`augmentation`) | deferred |

### 10.4 Functions and object literals

| Construct | Expected | Status |
|---|---|---|
| `function decl() {}` | `Function decl` | implemented (existing) |
| `function* gen() {}`, `async function* agen() {}` | `Function gen` / `agen` | implemented (#3) |
| `export const arrow = () => {}` | `Function arrow` | implemented (existing) |
| `export default function () { decl() }` | `Function default`, `CALLS default -> decl` | implemented (#7) |
| `export default () => 1` | `Function default` | implemented (#7) |
| `export default impl;` | no node; export index `default -> impl` | implemented (#9) |
| IIFE at module scope | calls attributed to the File | implemented (existing) |
| nested `function inner() {}` inside a function | no node; calls attributed to the outer function; `inner()` itself is not a `CALLS` edge | implemented (#15) |
| local `const handle = () => ...`, local classes, interfaces, type aliases, enums | no node; calls attributed to the outer function | implemented (#15) |
| `items.map(x => f(x))` (unparenthesized parameter) | no node named `x`; `CALLS outer -> f` | implemented (#15) |
| `export const api = { get() {}, post: () => {}, put: function () {} }` | `Class api` (`object`), `Function api.get` / `api.post` / `api.put` | implemented (#8) |
| `api = { nested: { deep() {} } }` | `Class api.nested`, `Function api.nested.deep` | implemented (#8) |
| `api = { a: { b: { c() {} } } }` | `Class api.a`, `Class api.a.b`, `Function api.a.b.c`; `api.a.b.c()` resolves | implemented (#11, up to six levels) |
| object literal inside a function body or passed as an argument | no node; its methods are not flattened to the top level, and their calls belong to the enclosing node | implemented (#8) |
| `export const Memo = React.memo(function X() {})`, `memo(() => ...)`, `forwardRef((p, r) => ...)`, `observer(...)`, nested wrappers | `Function Memo` (`wrapped_by: ["React.memo"]`, `expression_name: "X"`); body calls from `Memo`; `CALLS File -> react::memo`; `<Memo />` and imports resolve | implemented (#26) |
| `export default memo(function Page() {})` | `Function default` (`wrapped_by`, `expression_name: "Page"`) | implemented (#26) |
| `const doubled = items.map(x => ...)`, `compose(a, b)` at module scope | no node (not a wrapper, §7.8) | implemented (#26) |
| inline callbacks `items.map(x => f(x))` | no node; `CALLS outer -> f` | implemented (existing) |

### 10.5 Calls and JSX

| Construct | Expected | Status |
|---|---|---|
| same-file call `decl()` | `CALLS -> decl` | implemented (existing) |
| named import `decl()` | `CALLS -> functions.ts::decl` | implemented (existing) |
| aliased import `import { decl as renamed }` | `CALLS -> functions.ts::decl` | implemented (#9) |
| default import `import Card from "./Button"` | `CALLS -> Button.tsx::DefaultCard` (the default-exported symbol) | implemented (#9) |
| default import of an anonymous default | `CALLS -> m.ts::default` | implemented (#7, #9) |
| namespace import `fns.decl()` | `CALLS -> functions.ts::decl` | implemented (#17) |
| namespace import JSX `<UI.Button />` | `CALLS -> Button.tsx::Button` | implemented (existing) |
| static member `Box.create()` | `CALLS -> classes.ts::Box.create` | implemented (#17) |
| `const s = new Store(); s.find()` (same-file type) | `CALLS -> Store.find` | implemented (existing) |
| receiver of an imported type | `CALLS -> classes.ts::DefaultShape.area` | implemented (#17) |
| unknown receiver `res.json()` | stays bare (`receiver_unknown: true`), never the first same-named method | implemented (#17) |
| `new Box()` | `CALLS -> Box` | implemented (existing) |
| `new UserService()` imported from `./user.service` | `CALLS -> user.service.ts::UserService` | implemented (#5) |
| JSX `<Button />` | `CALLS -> Button` | implemented (existing) |
| JSX intrinsic `<div />` | no edge | implemented (existing) |
| Express `app.get(...)` on `const app = express()` | never resolved to an unrelated object-literal method; bare `get` with `receiver_unknown` (the value's type is unknown); `express()` is `CALLS -> express::default` | implemented (#8, #25) |
| external `useState(0)` | `CALLS -> react::useState` (`external`, `external_package: "react"`) | implemented (#25) |
| external namespace / default members `fs.readFile()`, `React.useEffect()`, `z.object()` | `CALLS -> node:fs::readFile` / `react::useEffect` / `zod::z.object` | implemented (#25) |
| `require("pkg")` bindings, subpaths `lodash/fp`, scoped packages | `CALLS -> pkg::default` / `lodash/fp::map` (`external_package: "lodash"`) / `@scope/name::x` | implemented (#25) |
| external JSX `<Button />`, `<Form.Item />`, decorators `@Injectable()` | `CALLS -> antd::Form.Item`; `REFERENCES -> @nestjs/common::Injectable` | implemented (#25) |
| tagged template | `CALLS -> tag` | implemented (existing) |
| `require("./x")`, `import("./x")` | `IMPORTS_FROM` (`import_kind`); no `CALLS -> require` | implemented (#19) |
| `const { a } = require("./x"); a()`, `const x = require("./x"); x.a()` | `CALLS -> x::a` | implemented (#19) |

### 10.6 Imports, exports, module resolution

| Construct | Expected | Status |
|---|---|---|
| default / named / namespace / side-effect import | `IMPORTS_FROM` | implemented (existing) |
| `./user.service`, `./hero.component` | `IMPORTS_FROM -> user.service.ts` | implemented (#5) |
| `./esm-compat.js` backed by `.ts` | `IMPORTS_FROM -> esm-compat.ts` | implemented (existing; kept by #5) |
| `./types` backed by `types.d.ts` | `IMPORTS_FROM -> types.d.ts` | implemented (#5) |
| `./util.mjs` backed by `.mts`, `./conf` backed by `.cjs` | resolved | implemented (#5) |
| `.cjs`, `.mts`, `.cts` files themselves | parsed (JavaScript / TypeScript), so their declarations are nodes and imports of them resolve to those nodes | implemented (#20) |
| `./lib` directory | `IMPORTS_FROM -> lib/index.ts` | implemented (existing) |
| tsconfig `paths` | resolved | implemented (existing) |
| tsconfig `baseUrl` without `paths`, and `baseUrl` as the fallback when no `paths` pattern matches | `import "services/user"` -> `src/services/user.ts` for `"baseUrl": "src"`; a package name without such a file stays external | implemented (#27) |
| monorepo configs | the nearest `tsconfig.json` / `tsconfig.app.json` / `jsconfig.json` directory from the importer; solution-style `tsconfig.json` defers to a sibling config with `paths` / `baseUrl` | implemented (#27) |
| `jsconfig.json` | read like `tsconfig.json` | implemented (#27) |
| tsconfig `extends` chains | not followed; inherited `paths` / `baseUrl` are not applied | deferred |
| `export { a as b }`, `export * from` | export index | implemented (existing) |
| `export { x as default }`, `export default <decl>` | export index `default` | implemented (#9) |
| `export { default as x } from "./b"` | followed to `b`'s default export | implemented (#9) |
| local re-export of an import, `export * as ns from` | followed to the origin | implemented (#18) |
| `import { ns } from "./barrel"; ns.f()` for `export * as ns from "./m"` | `CALLS -> m::f` | implemented (#18) |
| a name exported by two `export *` sources | ambiguous, bound to neither; an explicit export wins | implemented (#18) |
| `module.exports = { a, b: fn }`, `module.exports.x =`, `exports.x =` | export index (named exports, exports object as `default`) | implemented (#18) |
| `export =` | export index default | implemented (#18) |
| `import fs = require("fs")` | `IMPORTS_FROM` (`import_kind: "import_equals"`); `fs.f()` resolves for repo modules | implemented (#19) |
| `import A = B.C` (alias of a namespace member) | not read | out of scope |

### 10.7 Type references

| Construct | Expected | Status |
|---|---|---|
| parameter / return types, `this: T`, type predicates | `REFERENCES fn -> Type` (`type_reference`, `type_positions: ["parameter", "return"]` / `["type_predicate"]`) | implemented (#23) |
| overloads `f(a: A): X; f(a: B): Y; f(a) {}` | one set of edges from the merged `Function f` | implemented (#23) |
| type parameter constraints and defaults `<T extends Repo = Repo>` | `REFERENCES -> Repo` (`type_parameter_constraint`, `type_parameter_default`); `T` itself is no edge | implemented (#23) |
| heritage type arguments `implements Service<User>` | `REFERENCES class -> User` (`heritage_type_argument`); `Service` stays `IMPLEMENTS` | implemented (#23) |
| class field types, interface property signatures, index signatures | `REFERENCES class -> Type` (`field`, `index_signature`) | implemented (#23) |
| parameter properties `constructor(private repo: Repo)` | `REFERENCES class -> Repo` (`parameter_property`) | implemented (#23) |
| function-valued field annotation `handler: Handler = () => ...` | `REFERENCES handler -> Handler` (`field`) | implemented (#23) |
| alias right-hand side `type Pair = [User, Repo]` | `REFERENCES Pair -> User`, `-> Repo` (`type_alias`) | implemented (#23) |
| `ns.Type`, `Outer.Inner`, aliased / default / re-exported imports, namespace-local names | resolved to the declaring QN (§7.4) | implemented (#23) |
| `typeof X` in a signature | `REFERENCES -> X` (`type_query`) | implemented (#23) |
| a type repeated in several positions of one declaration | one edge, every position in `type_positions` | implemented (#23) |
| builtin and global types, external packages, undeclared names, type parameters, `infer U`, mapped keys, self references | no edge (external types stay out after #25, §7.6) | implemented (#23, #25) |
| body annotations `const u: User`, `x as User`, `<User>x`, `x satisfies Shape` | `REFERENCES outer -> User` (`variable_annotation`, `as`, `satisfies`) | implemented (#24) |
| call / `new` type arguments `pick<UserId>()`, `new Repo<User>()` | `REFERENCES outer -> UserId` (`type_argument`); `new Repo` stays `CALLS` | implemented (#24) |
| `x instanceof Repo` (TypeScript and JavaScript) | `REFERENCES outer -> Repo` (`instanceof`) | implemented (#24) |
| `typeof X` in a body annotation | `REFERENCES outer -> X` (`type_query`) | implemented (#24) |
| local interface / type alias / enum in a function body | no node; the types it names are `REFERENCES outer -> Type` (`local_declaration`); its own name is never a target | implemented (#24) |
| signatures written in a body (local functions, callbacks, local classes, object-literal methods) | `REFERENCES outer -> Type` (`parameter`, `return`, `field`, `heritage`) | implemented (#24) |
| module-scope annotation `const api: Api = { ... }`, `const h: Handler = () => ...`, `const config: Config = {}` | `REFERENCES api -> Api` / `h -> Handler` (`variable_annotation`); a binding without a node gives `File -> Config` | implemented (#24) |
| enum member references `Role.Admin` | no type reference (a value access) | deferred |

### 10.8 Tests

| Construct | Expected | Status |
|---|---|---|
| `describe` / `it` / `test` | synthetic `Test`, `CALLS`, `TESTED_BY` | implemented (existing) |
| `suite` / `specify` / `context` / `fit` / `xit` / `fdescribe` / `xdescribe` | synthetic `Test` | implemented (#21) |
| `.only` / `.skip` / `.todo` / `.concurrent` | synthetic `Test` with `test_modifiers` | implemented (#21) |
| Playwright `test.describe`, `test.describe.only` | `Test describe:title@L1` | implemented (#21) |
| hooks, `test.step`, Playwright `test.skip()` in a body | no node and no edge; callback calls go to the enclosing test | implemented (#21) |
| `function TestimonialCard()` in a non-test file | `Function` | implemented (#6) |
| `function TestHelper()` in a test file | `Test` | implemented (existing; kept by #6) |
| `test.each(...)("name", fn)`, tagged-template `.each` | `Test test:name@L9` covering the outer call | implemented (#21) |
| `*.test.tsx`, `*.spec.jsx`, `*.test.mjs`, `__tests__/`, `e2e/`, `*.cy.ts` | File `is_test` | implemented (#21) |
| `TESTED_BY` for calls resolved in post-processing | follows the resolved `CALLS` target | implemented (#22) |
| `TESTED_BY` to runner / assertion APIs (`expect`, `beforeEach`) | not emitted | implemented (#21) |
| `TESTED_BY` to external packages (`render` from `@testing-library/react`) | not emitted; the `CALLS` edge stays | implemented (#25) |

### 10.9 Frameworks and entry points

| Construct | Expected | Status |
|---|---|---|
| NestJS `@Controller` / `@Get`, Angular `@Component` | decorator metadata; framework entry points (flow tracing and dead code) even when called | implemented (#16) |
| NestJS `@Post` / `@MessagePattern` / `@EventPattern` / `@Cron` / `@Interval` / `@Timeout` / `@OnEvent` / `@Process` / `@Processor` / `@SubscribeMessage` / `@WebSocketGateway`, Angular `@HostListener` | entry points (exact, case-sensitive names) | implemented (#16) |
| Next.js `route.ts` `GET` / `POST`, `page.tsx` default | uncalled `Function` nodes (entry points) | implemented (existing); anonymous defaults by #7 |
| Express / Fastify inline route handlers | synthetic route-handler nodes | deferred |
| React components and hooks | `Function` nodes, JSX `CALLS` | implemented (existing) |

## 11. Upgrading an existing graph

Several of these changes rename existing QNs (for example `file::describe`
becomes `file::AbstractShape.describe`, and `file::get` becomes
`file::api.get`). Incremental updates re-parse changed files and their
importers, but unchanged files would keep nodes and edges under the old QNs.

The extractor version stamp covers this. `crates/dagayn-parser/src/extractor_version.rs`
declares the `javascript` extractor's output version (it parses the
`javascript`, `typescript`, `tsx`, `vue`, and `svelte` languages), and every
full build and successful update records it in graph metadata
(`extractor_versions`, for example `javascript=1`). When the stored version
is older, or missing because the graph predates stamps:

- the sync assessment reports `commit_drift` with
  `extractor_drift: ["javascript"]` (reason code
  `graph_built_by_older_extractor`), so session prepare and MCP auto-prepare
  run an update;
- `dagayn update` re-parses every indexed file of that extractor, changed or
  not, and then records the current version.

Bump the version in the same change whenever an extractor change renames QNs
or otherwise changes the nodes and edges produced for unchanged source.

### 11.1 Parity snapshots

`tests/fixtures/parity/typescript/` (TS, TSX, `.d.ts`, `.mts`, `.cts`) and
`tests/fixtures/parity/javascript/` (JS, JSX, `.mjs`, `.cjs`) are built by `tests/test_parity_export.py` and
`tests/test_rust_backend_parity.py` and compared with
`tests/fixtures/parity/__snapshots__/{typescript,javascript}.json`. These
snapshots hold one node or edge per line, so an extractor change shows up as a
reviewable diff. Regenerate them after an intentional change:

```bash
uv run python tools/parity_export.py --regenerate typescript javascript
```
