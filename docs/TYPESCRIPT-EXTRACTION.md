# TypeScript / JavaScript extraction model

<!-- constrained-by ./SCHEMA.md -->
<!-- constrained-by ./ARCHITECTURE.md#parsing-model -->

This document specifies which nodes and edges dagayn extracts from TypeScript,
TSX, JavaScript, and JSX sources, how their qualified names are formed, and how
references are resolved. It is the contract for `crates/dagayn-parser`
(`js_like.rs`, `js_modules.rs`, `member_calls.rs`, `js_sfc.rs`) and for the
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
| `.ts`, `.d.ts` | tree-sitter-typescript (`typescript`) | |
| `.tsx` | tree-sitter-typescript (`tsx`) | JSX enabled |
| `.js`, `.jsx`, `.mjs` | tree-sitter-javascript | JSX enabled for all JavaScript files |
| `.cjs`, `.mts`, `.cts`, `.d.mts`, `.d.cts` | JavaScript / TypeScript | parsed: planned (part 2/3, #20); already used as import-resolution candidates |
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
   and same-file declaration merging collapse into one node.
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
| `type T = ...` | `Type` | `T` | owner path | `type_role: "alias"` (see §7.1) |
| `enum E`, `const enum E`, `declare enum E` | `Class` | `E` | owner path | `type_role: "enum"`, `container_role: "data_container"`, `value_semantics: true` |
| `namespace N`, `module N` | `Class` | segment name | owner path | `type_role: "namespace"`; `namespace A.B.C` yields nested `A`, `A.B`, `A.B.C` |
| `declare module "x"`, `declare global` | `Class` | `x` / `global` | owner path | `type_role: "ambient_module"`, `ambient: true` |
| module-scope `const X = { ... }` or `export default { ... }` with at least one function-valued member | `Class` | `X` / `default` | owner path | `type_role: "object"` (see §7.3) |
| `function f`, `function* f`, `async function* f`, `declare function f` | `Function` | `f` | owner path | |
| module-scope `const f = () => {}` / `function () {}` / `function* () {}` | `Function` | `f` (the binding) | owner path | |
| `export default function () {}`, `export default () => ...` (anonymous) | `Function` | `default` | owner path | `export_default: true`, `anonymous: true` |
| named default export `export default function Page() {}` | `Function` | `Page` | owner path | `export_default: true` |
| method, getter/setter, `#private` method, function-valued class field | `Function` | member name | class owner path | getter/setter pairs: `member_role: "accessor"` |
| `abstract m()`, `abstract get x()` | `Function` | `m` / `x` | class owner path | `is_abstract: true` |
| interface method signature | `Function` | `m` | interface owner path | `is_abstract: true` |
| object-container member (`get() {}`, `k: () => {}`, `k: function () {}`) | `Function` | member name | container owner path | |
| test-runner call (`describe`, `it`, `test`) in a test file | `Test` | `it:description@L6` | owner path | synthetic; `describe` blocks are not part of the owner path |

Nodes that are **not** created:

- declarations inside a function or method body (nested functions, local arrow
  handlers, local classes, local object literals); see §7.2
- class fields whose value is not a function, index signatures, enum members,
  interface property / call / construct / index signatures
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
- **External package symbols** are not nodes. Edges that target a symbol
  imported from an unresolved, non-relative specifier use `pkg::symbol`
  (`react::useState`, `express::default`,
  `@testing-library/react::render`). The `::` keeps these targets away from
  the bare-name resolver, so they are never attached to an unrelated local
  symbol of the same name. Post-processing marks them LOW because no node
  exists.
- **Synthetic tests** keep the existing `runner:description@Lline` form.

## 5. Edges

| Edge | Source | Target | Notes |
|---|---|---|---|
| `CONTAINS` | nearest existing container QN (File, Class, namespace, object container) | child QN | |
| `CALLS` | the node that owns the call site (§5.1) | resolved QN, `pkg::symbol`, or a bare name | includes `new X()`, `super(...)`, JSX elements, tagged templates |
| `IMPORTS_FROM` | File | resolved repo-relative file, or the raw specifier for external modules | static `import` and `export ... from`; `require`, dynamic `import()`, and `import x = require()` are planned (part 2/3, #19) |
| `REFERENCES` (value) | owning node | function or class used as a value | object `pair` values, shorthand properties, array elements, call arguments, assignment right-hand sides |
| `REFERENCES` (type) | owning node | type QN | `relationship_role: "type_reference"` or `"type_query"` (§7.4) |
| `REFERENCES` (decorator) | decorated node | decorator function | `relationship_role: "decorator"` (§7.7) |
| `INHERITS` | class or interface | base QN or bare name | `relationship_role: "extends"`, `syntax_source` |
| `IMPLEMENTS` | class | interface QN or bare name | `relationship_role: "implements"`, `syntax_source` |
| `TESTED_BY` | production symbol | `Test` node | derived from calls made by tests |
| `CROSS_ARTIFACT` | owning node | path or command | `child_process.*` and `fs.*` bridges |

### 5.1 Call attribution

| Call site | Source of the `CALLS` edge |
|---|---|
| function or method body, including nested closures and callbacks | that function or method |
| class field initializer, static block, class or member decorator argument | the class |
| class heritage expression (`extends Mixin(Base)`) | the class |
| module scope, including IIFEs | the File |
| test-runner callback | the synthetic `Test` node |

### 5.2 Call kinds

- **`new X()`** is a `CALLS` edge to the class `X` (not to its constructor),
  as with Python's `X()`. Every class has a node even without an explicit
  constructor, and "this code uses class X" is the useful impact granularity.
- **`super(...)`** is a `CALLS` edge to the resolved base class (planned,
  part 2/3, #17).
- **JSX** `<Comp />` and `<UI.Comp />` are `CALLS` edges to the component, so
  flows follow the component tree. Lowercase intrinsic elements (`<div />`)
  are ignored.
- **Tagged templates** (``tag`x` ``) are `CALLS` edges to `tag`.

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
QN. When several members share the name, the member of the caller's own class
wins. Receivers bound by `const x = new X()` or an annotation `x: X` rewrite
`x.m()` to `X.m`.

### 6.2 Imports

Imports are bound by `(module, exported name)`:

| Import | Binding |
|---|---|
| `import { a } from "./m"` | `./m`, `a` |
| `import { a as b } from "./m"` | local `b` -> `./m`, `a` |
| `import X from "./m"` | `./m`, `default`; resolves to the symbol the module exports as default |
| `import * as ns from "./m"` | `./m`, namespace; `<ns.C />` resolves to `m::C` |

The export index of the target module maps exported names to declarations:
local declarations, `export { a as b }`, `export { a } from`, `export * from`,
and `default` (`export default <declaration>`, an anonymous default named
`default`, `export default ident`, `export { x as default }`). If a module has
no default export at all, a default import falls back to the importer's local
name, which keeps code that relies on bundler interop resolvable.

### 6.3 Module paths

Relative specifiers are resolved against the importing file by trying, in
order:

1. the path as written, if it is a file
2. runtime-to-source extension mapping: `.js` -> `.ts` / `.tsx`,
   `.jsx` -> `.tsx`, `.mjs` -> `.mts`, `.cjs` -> `.cts`
3. the path with `.ts`, `.tsx`, `.d.ts`, `.js`, `.jsx`, `.mjs`, `.cjs`,
   `.mts`, `.cts`, `.vue` appended, so `./user.service` finds
   `user.service.ts`
4. `index.*` with the same extensions when the path is a directory

A file wins over a directory with the same stem, and an implementation file
wins over its `.d.ts`. Non-relative specifiers go through the nearest
`tsconfig.json` / `tsconfig.app.json` `paths` (relative to `baseUrl`), using
the same candidate list. Anything else is external.

### 6.4 Confidence

- Resolutions that the parser derives from syntax (same file, import plus
  export index, receiver bindings) carry no confidence override (`EXTRACTED`).
- Bare names resolved in post-processing through import visibility are
  `MEDIUM`.
- Targets without a node (`pkg::symbol`, unresolved names) become `LOW`
  through `demote_unresolved_endpoint_edges`.

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
carry `container_role: "data_container"`. Planned (part 2/3, #13).

### 7.2 Local declarations are not nodes

Nested functions, local arrow handlers, and local classes inside a function
body are not nodes; their calls are attributed to the enclosing function
(§5.1). In React and callback-heavy code these locals are numerous, collide by
name across functions (many components define `handleClick`), and otherwise
show up as false flow entry points because nothing calls them by name. Python
flattens nested functions today; TypeScript deliberately differs. Planned
(part 2/3, #15).

### 7.3 Module-scope object literals are containers

`export const api = { get() {}, post: () => {}, put: function () {} }` yields
`Class api` (`type_role: "object"`) containing `api.get`, `api.post`, and
`api.put`. Before this, `get` became a top-level function, `post` and `put`
were not nodes at all, and a full build resolved Express's `app.get(...)` to
the flattened `get`. A container is created only when the object has at least
one function-valued member; `as const`, `satisfies T`, and parentheses are
unwrapped. One nesting level is modeled (`api.nested` becomes a container for
`api.nested.deep`). Dead-code analysis excludes `object`, `namespace`, and
`ambient_module` containers from its candidates.

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
Planned (part 2/3, #23 and #24).

### 7.5 Anonymous default exports are named `default`

`default` is the ES module export name itself, so `import X from "./m"`
resolves mechanically to `m::default` without depending on the importer's
local name. Names derived from the file name were rejected because they
collide and break on rename.

### 7.6 External package symbols are `pkg::symbol`

A bare `render` or `get` imported from a package used to be resolved by
post-processing to an unrelated local method of the same name (a test's
`render` attached to a component's `render` method). Qualifying with the
package specifier keeps the dependency explicit and stops name-based
misresolution. Planned (part 2/3, #25).

### 7.7 Decorators are metadata plus `REFERENCES`

Decorated classes and members record `extra.decorators` (callee names, the
format Python uses), which enables framework entry-point detection and
dead-code exclusion for NestJS and Angular. The decorator itself becomes
`REFERENCES decorated -> decorator` (`relationship_role: "decorator"`) instead
of a `CALLS` edge from the File. Planned (part 2/3, #16).

## 8. JavaScript parity

| Topic | Behavior |
|---|---|
| class heritage | JavaScript `class_heritage` holds `extends <expression>` directly; it is read with the same expression rules as TypeScript |
| class fields | JavaScript `field_definition` is treated like `public_field_definition` (planned, part 2/3, #26) |
| generators | `function*` / `async function*` declarations and generator methods are nodes in both languages |
| JSX | `.js`, `.jsx`, and `.mjs` parse with JSX; component calls behave as in TSX |
| test naming | `Test*` / `test_*` / `*_test` / `*_spec` names mark `Test` nodes only inside test files |
| CommonJS | `require`, `module.exports`, and `exports.x` are planned (part 2/3, #18 and #19) |
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
| class / member decorators | `decorators` metadata, `REFERENCES -> decorator` | planned (part 2/3, #16) |
| `constructor(private repo: Repo)` | `Function Box.constructor`; `this.repo` bound to `Repo` | node implemented (existing); binding planned (part 2/3, #17) |
| `super(repo)` | `CALLS Box.constructor -> Base` | planned (part 2/3, #17) |
| field initializer `svc = new UserService()` | `CALLS Class -> UserService` | planned (part 2/3, #15) |
| function-valued field `handler = () => this.helper()` | `Function Box.handler`, `CALLS -> Box.helper` | implemented (existing) for TS; JS planned (part 2/3, #26) |
| `static create()`, `async load()` | `Function Box.create` / `Box.load` | implemented (existing) |
| generator method `*items()` | `Function Box.items` | implemented (existing; covered by #3 tests) |
| `get value()` + `set value(v)` | one `Function Box.value` (`member_role: "accessor"`) | planned (part 2/3, #14) |
| overloads `m(a: string); m(a: number); m(a) {}` | one `Function Box.m` | planned (part 2/3, #14) |
| `#privateMethod() {}` | `Function Box.#privateMethod` | planned (part 2/3, #14) |
| computed member names | string-literal names only | deferred |
| `this.helper()` (same class) | `CALLS -> Box.helper` | implemented (existing) |
| `this.helper()` (inherited) | `CALLS -> Base.helper` (MEDIUM) | planned (part 2/3, #17) |
| `this.repo.find()` | `CALLS -> interfaces.ts::Repo.find` | planned (part 2/3, #17) |
| `super.m()` | `CALLS -> Base.m` | planned (part 2/3, #17) |

### 10.2 Interfaces, type aliases, enums

| Construct | Expected | Status |
|---|---|---|
| `interface Repo {}` | `Class Repo` (`interface`, `is_abstract`, `is_contract`) | implemented (existing) |
| interface `extends Repo, Logger, ns.X<T>` | one `INHERITS` per base (`extends`) | implemented (#4) |
| method signature `find(): string;` | `Function Repo.find` (`is_abstract`) | implemented (existing) |
| same-file interface merging | one `Class Repo` holding the members of both declarations | planned (part 2/3, #14) |
| `type Props = { a: A }` | `Type Props` (`alias`, `data_container`) | planned (part 2/3, #13) |
| `type U = A \| B` | `Type U` (`alias`) | planned (part 2/3, #13) |
| `enum Color {}` | `Class Color` (`enum`, `data_container`) | implemented (existing) |
| `const enum`, `declare enum` | `Class` plus `const_enum` / `ambient` | planned (part 2/3, #13) |

### 10.3 Namespaces, ambient declarations, `.d.ts`

| Construct | Expected | Status |
|---|---|---|
| `namespace Outer { export function helper() {} }` | `Class Outer` (`namespace`), `Function Outer.helper` | planned (part 2/3, #11, #12) |
| `namespace A.B.C {}` | nested `A`, `A.B`, `A.B.C` | planned (part 2/3, #11, #12) |
| `declare module "external-lib" {}` | `Class external-lib` (`ambient_module`) | planned (part 2/3, #12) |
| `declare global {}` | `Class global` (`ambient_module`) | planned (part 2/3, #12) |
| `declare function f(): void;` | `Function f` (`ambient`, no `is_abstract`) | planned (part 2/3, #12) |
| `.d.ts` file | File `declaration_file: true` | planned (part 2/3, #12) |

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
| nested `function inner() {}` inside a function | no node; calls attributed to the outer function | planned (part 2/3, #15) |
| local `const handle = () => ...` inside a function | no node; calls attributed to the outer function | planned (part 2/3, #15) |
| `export const api = { get() {}, post: () => {}, put: function () {} }` | `Class api` (`object`), `Function api.get` / `api.post` / `api.put` | implemented (#8) |
| `api = { nested: { deep() {} } }` | `Class api.nested`, `Function api.nested.deep` | implemented (#8, one level) |
| object literal inside a function body or passed as an argument | no node; its methods are not flattened to the top level, and their calls belong to the enclosing node | implemented (#8) |
| `export const Memo = React.memo(function X() {})` | `Function Memo` (`wrapped_by`) | planned (part 2/3, #26) |
| inline callbacks `items.map(x => f(x))` | no node; `CALLS outer -> f` | implemented (existing) |

### 10.5 Calls and JSX

| Construct | Expected | Status |
|---|---|---|
| same-file call `decl()` | `CALLS -> decl` | implemented (existing) |
| named import `decl()` | `CALLS -> functions.ts::decl` | implemented (existing) |
| aliased import `import { decl as renamed }` | `CALLS -> functions.ts::decl` | implemented (#9) |
| default import `import Card from "./Button"` | `CALLS -> Button.tsx::DefaultCard` (the default-exported symbol) | implemented (#9) |
| default import of an anonymous default | `CALLS -> m.ts::default` | implemented (#7, #9) |
| namespace import `fns.decl()` | `CALLS -> functions.ts::decl` | planned (part 2/3, #17) |
| namespace import JSX `<UI.Button />` | `CALLS -> Button.tsx::Button` | implemented (existing) |
| static member `Box.create()` | `CALLS -> classes.ts::Box.create` | planned (part 2/3, #17) |
| `const s = new Store(); s.find()` (same-file type) | `CALLS -> Store.find` | implemented (existing) |
| receiver of an imported type | `CALLS -> classes.ts::DefaultShape.area` | planned (part 2/3, #17) |
| unknown receiver `res.json()` | stays bare, never the first same-named method | planned (part 2/3, #17) |
| `new Box()` | `CALLS -> Box` | implemented (existing) |
| `new UserService()` imported from `./user.service` | `CALLS -> user.service.ts::UserService` | implemented (#5) |
| JSX `<Button />` | `CALLS -> Button` | implemented (existing) |
| JSX intrinsic `<div />` | no edge | implemented (existing) |
| Express `app.get(...)` | never resolved to an unrelated object-literal method | implemented (#8); `express::...` qualification planned (part 2/3, #25) |
| external `useState(0)` | `CALLS -> react::useState` | planned (part 2/3, #25) |
| tagged template | `CALLS -> tag` | implemented (existing) |
| `require("./x")`, `import("./x")` | `IMPORTS_FROM` (`import_kind`); no `CALLS -> require` | planned (part 2/3, #19) |

### 10.6 Imports, exports, module resolution

| Construct | Expected | Status |
|---|---|---|
| default / named / namespace / side-effect import | `IMPORTS_FROM` | implemented (existing) |
| `./user.service`, `./hero.component` | `IMPORTS_FROM -> user.service.ts` | implemented (#5) |
| `./esm-compat.js` backed by `.ts` | `IMPORTS_FROM -> esm-compat.ts` | implemented (existing; kept by #5) |
| `./types` backed by `types.d.ts` | `IMPORTS_FROM -> types.d.ts` | implemented (#5) |
| `./util.mjs` backed by `.mts`, `./conf` backed by `.cjs` | resolved | implemented (#5) |
| `./lib` directory | `IMPORTS_FROM -> lib/index.ts` | implemented (existing) |
| tsconfig `paths` | resolved | implemented (existing) |
| tsconfig `baseUrl` without `paths` | resolved | planned (part 2/3, #27) |
| `export { a as b }`, `export * from` | export index | implemented (existing) |
| `export { x as default }`, `export default <decl>` | export index `default` | implemented (#9) |
| local re-export of an import, `export * as ns from`, `export { default as x } from` | followed to the origin | planned (part 2/3, #18) |
| `import fs = require("fs")`, `export =` | `IMPORTS_FROM`, export index default | planned (part 2/3, #18, #19) |

### 10.7 Type references

| Construct | Expected | Status |
|---|---|---|
| parameter / return types | `REFERENCES fn -> Type` (`type_reference`) | planned (part 2/3, #23) |
| type parameters, constraints, heritage type arguments | `REFERENCES` | planned (part 2/3, #23) |
| field and parameter-property types | `REFERENCES` | planned (part 2/3, #23) |
| alias right-hand side, interface member types | `REFERENCES` | planned (part 2/3, #23) |
| `typeof X` | `REFERENCES -> X` (`type_query`) | planned (part 2/3, #23) |
| body annotations, `as`, `satisfies`, call type arguments | `REFERENCES outer -> Type` | planned (part 2/3, #24) |
| builtin and global types | no edge | planned (part 2/3, #23) |

### 10.8 Tests

| Construct | Expected | Status |
|---|---|---|
| `describe` / `it` / `test` | synthetic `Test`, `CALLS`, `TESTED_BY` | implemented (existing) |
| `function TestimonialCard()` in a non-test file | `Function` | implemented (#6) |
| `function TestHelper()` in a test file | `Test` | implemented (existing; kept by #6) |
| `test.each(...)("name", fn)` | `Test test:name@L9` covering the outer call | planned (part 2/3, #21) |
| `*.test.tsx`, `*.spec.jsx`, `__tests__/`, `*.cy.ts` | File `is_test` | planned (part 2/3, #21) |
| `TESTED_BY` for calls resolved in post-processing | follows the resolved `CALLS` target | planned (part 2/3, #22) |
| `TESTED_BY` to runner / assertion APIs (`expect`, `beforeEach`) | not emitted | planned (part 2/3, #21) |

### 10.9 Frameworks and entry points

| Construct | Expected | Status |
|---|---|---|
| NestJS `@Controller` / `@Get`, Angular `@Component` | decorator metadata; framework entry points | planned (part 2/3, #16) |
| Next.js `route.ts` `GET` / `POST`, `page.tsx` default | uncalled `Function` nodes (entry points) | implemented (existing); anonymous defaults by #7 |
| Express / Fastify inline route handlers | synthetic route-handler nodes | deferred |
| React components and hooks | `Function` nodes, JSX `CALLS` | implemented (existing) |

## 11. Upgrading an existing graph

Several of these changes rename existing QNs (for example `file::describe`
becomes `file::AbstractShape.describe`, and `file::get` becomes
`file::api.get`). Incremental updates re-parse changed files and their
importers, but unchanged files can keep edges that point at the old QNs. Run
`dagayn build --force-full-build` once after upgrading.
