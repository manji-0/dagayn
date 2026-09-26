use super::*;

#[test]
fn parses_typescript_type_aliases_and_enums() {
    let source = br#"export type UserId = string;
export type Shape = { kind: "circle"; r: number } | { kind: "sq"; s: number };
export type Props = { label: string; onClick(): void };
export type ReadonlyAll<T> = { readonly [K in keyof T]: T[K] };
export type Unwrap<T> = T extends Promise<infer U> ? U : T;
export type Handler = (req: Request) => Promise<Response>;
export type Both = Props & Shape;
export type Pair = [string, number];
export type Named = Map<string, Props>;
export type Key = keyof Props;
export enum Color { Red, Green = "g" }
export const enum Direction { Up = 1, Down }
declare enum Ambient { A }
namespace N { export type Inner = { a: 1 }; }
function use(c: Color): Props { return {} as Props; }
"#;
    let (nodes, edges) = parse_javascript_like("types.ts", source, "typescript");
    let alias = |name: &str| {
        nodes
            .iter()
            .find(|node| node.name == name)
            .unwrap_or_else(|| panic!("{name}: {nodes:?}"))
    };
    for (name, form) in [
        ("UserId", "primitive"),
        ("Shape", "union"),
        ("Props", "object"),
        ("ReadonlyAll", "mapped"),
        ("Unwrap", "conditional"),
        ("Handler", "function"),
        ("Both", "intersection"),
        ("Pair", "tuple"),
        ("Named", "reference"),
        ("Key", "operator"),
    ] {
        let node = alias(name);
        assert_eq!(node.kind, "Type", "{name}");
        assert_eq!(node.extra["type_role"], "alias", "{name}");
        assert_eq!(node.extra["alias_form"], form, "{name}");
        if form == "object" {
            assert_eq!(node.extra["container_role"], "data_container", "{name}");
        } else {
            assert!(node.extra.get("container_role").is_none(), "{name}");
        }
    }
    // Alias members are not nodes: an alias is not a container.
    assert!(!nodes.iter().any(|node| node.name == "onClick"));
    let inner = alias("Inner");
    assert_eq!(inner.kind, "Type");
    assert_eq!(inner.parent_name.as_deref(), Some("N"));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CONTAINS"
            && edge.source == "types.ts::N"
            && edge.target == "types.ts::N.Inner"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CONTAINS" && edge.source == "types.ts" && edge.target == "types.ts::Props"
    }));

    let color = alias("Color");
    assert_eq!(color.kind, "Class");
    assert_eq!(color.extra["type_role"], "enum");
    assert!(color.extra.get("const_enum").is_none());
    let direction = alias("Direction");
    assert_eq!(direction.extra["type_role"], "enum");
    assert_eq!(direction.extra["const_enum"], true);
    let ambient = alias("Ambient");
    assert_eq!(ambient.extra["type_role"], "enum");
    assert_eq!(ambient.extra["ambient"], true);
    assert!(color.extra.get("ambient").is_none());
}

#[test]
fn collapses_typescript_overloads_accessors_and_merged_interfaces() {
    let source = br#"export function over(a: string): string;
export function over(a: number): number;
export function over(a: any): any { return helper(a); }
declare function sig(a: string): void;
declare function sig(a: number): void;
export interface Repo { find(id: string): string; }
export interface Repo { save(item: string): void; }
export class Box {
  static count = 0;
  #secret = 1;
  #handler = () => this.#privateMethod();
  protected override async load(): Promise<void> {}
  static *items() {}
  get value(): number { return this.#secret; }
  set value(v: number) { this.#secret = v; }
  #privateMethod(): void { helper(1); }
  ["computed"](): void { helper(2); }
  42(): void {}
  "quoted-name"(): void {}
  m(a: string): string;
  m(a: number): number;
  m(a: any): any { return a; }
}
export abstract class Shape { protected abstract get label(): string; }
export function buildLabel() {}
export namespace buildLabel { export const suffix = ""; }
function helper(x: unknown) { return x; }
const local = 1;
export { local };
"#;
    let file = "over.ts";
    let (nodes, edges) = parse_javascript_like(file, source, "typescript");
    let qn = |node: &ParsedNode| qualify(file, &node.name, node.parent_name.as_deref());
    let mut seen = HashSet::new();
    for node in &nodes {
        assert!(
            seen.insert(qn(node)),
            "duplicate QN {}: {nodes:?}",
            qn(node)
        );
    }
    let find = |name: &str, parent: Option<&str>| {
        nodes
            .iter()
            .find(|node| node.name == name && node.parent_name.as_deref() == parent)
            .unwrap_or_else(|| panic!("{name}: {nodes:?}"))
    };
    let over = find("over", None);
    assert_eq!(over.extra["overloads"], 2);
    assert_eq!(over.line_start, 1);
    assert_eq!(over.line_end, 3);
    assert!(over.extra.get("declaration_only").is_none(), "{over:?}");
    assert!(over.extra.get("is_abstract").is_none());
    assert_eq!(over.extra["exported"], true);
    let sig = find("sig", None);
    assert_eq!(sig.extra["overloads"], 2);
    assert_eq!(sig.extra["declaration_only"], true);
    let m = find("m", Some("Box"));
    assert_eq!(m.extra["overloads"], 2);
    assert!(m.extra.get("declaration_only").is_none());

    let repo = find("Repo", None);
    assert_eq!(repo.extra["merged_declarations"], 2);
    assert_eq!(repo.extra["type_role"], "interface");
    assert!(
        nodes
            .iter()
            .any(|node| node.name == "find" && node.parent_name.as_deref() == Some("Repo"))
    );
    assert!(
        nodes
            .iter()
            .any(|node| node.name == "save" && node.parent_name.as_deref() == Some("Repo"))
    );

    let value = find("value", Some("Box"));
    assert_eq!(value.extra["member_role"], "accessor");
    assert_eq!(value.extra["accessors"], json!(["get", "set"]));
    let label = find("label", Some("Shape"));
    assert_eq!(label.extra["member_role"], "accessor");
    assert_eq!(label.extra["is_abstract"], true);
    assert_eq!(label.modifiers.as_deref(), Some("protected abstract get"));

    for name in [
        "#privateMethod",
        "#handler",
        "computed",
        "42",
        "quoted-name",
        "items",
        "load",
    ] {
        assert!(
            nodes.iter().any(|node| node.kind == "Function"
                && node.name == name
                && node.parent_name.as_deref() == Some("Box")),
            "{name}: {nodes:?}"
        );
    }
    assert_eq!(
        find("load", Some("Box")).modifiers.as_deref(),
        Some("protected override async")
    );
    assert_eq!(
        find("items", Some("Box")).modifiers.as_deref(),
        Some("static *")
    );
    assert_eq!(
        find("value", Some("Box")).modifiers.as_deref(),
        Some("get set")
    );

    // `function` + `namespace` merging keeps the function.
    let build = find("buildLabel", None);
    assert_eq!(build.kind, "Function");
    assert_eq!(build.extra["merged_declarations"], 2);
    assert!(edges.iter().any(|edge| {
        edge.kind == "CONTAINS" && edge.source == "over.ts" && edge.target == "over.ts::buildLabel"
    }));

    assert_eq!(find("Box", None).extra["exported"], true);
    assert!(find("helper", None).extra.get("exported").is_none());
    assert!(find("load", Some("Box")).extra.get("exported").is_none());

    for (source, target) in [
        ("over.ts::Box.#privateMethod", "over.ts::helper"),
        ("over.ts::Box.computed", "over.ts::helper"),
        ("over.ts::Box.#handler", "over.ts::Box.#privateMethod"),
        ("over.ts::over", "over.ts::helper"),
    ] {
        assert!(
            edges
                .iter()
                .any(|edge| edge.kind == "CALLS" && edge.source == source && edge.target == target),
            "CALLS {source} -> {target}: {edges:?}"
        );
    }
    let contains = edges
        .iter()
        .filter(|edge| edge.kind == "CONTAINS")
        .map(|edge| (edge.source.as_str(), edge.target.as_str()))
        .collect::<Vec<_>>();
    let unique = contains.iter().collect::<HashSet<_>>();
    assert_eq!(
        contains.len(),
        unique.len(),
        "duplicate CONTAINS: {contains:?}"
    );
}

#[test]
fn emits_typescript_signature_type_references() {
    let repo_root = write_type_reference_repo("signature-type-refs");
    let app = br#"import type { User, UserId } from "./models";
import { Repo, Role, Api, Ghost } from "./models";
import * as m from "./models";
import DefaultModel from "./models";
import { Member, models } from "./barrel";
import { External } from "external-pkg";
export interface Service<T> { handle(input: T): void; }
interface Local { owner: User; [key: string]: User | Role; }
export interface Tree { children: Tree[]; ghost: Ghost; }
type Pair = [User, Repo<UserId>] | Promise<Member>;
export function load(id: UserId, repo: Repo<User>): Promise<User | undefined> { return repo.find(id) as any; }
export function load2(id: string): User;
export function load2(id: number): Role;
export function load2(id: any): any { return id; }
export function pick<T extends User = User>(items: T[]): T { return items[0]; }
export function guard(x: unknown): x is m.User { return true; }
export class Holder implements Service<Api.Request> {
  repo!: Repo<DefaultModel>;
  constructor(private readonly owner: models.User, plain: Member) {}
  handle(input: Api.Request): void {}
  handler: (e: External) => Role = () => Role.Admin;
  kind: typeof Role = Role;
  self(): Holder { return this; }
  map: Map<string, External> = new Map();
}
export class Box<T extends User> { value!: T; }
export namespace Shapes {
  export interface Circle { r: number }
  export function area(c: Circle): number { return 0; }
}
"#;
    let mut parser = RustOwnedParser::new();
    let (_nodes, edges) = parser.parse_file_in_repo(Some(&repo_root), "src/app.ts", app);
    let _ = std::fs::remove_dir_all(&repo_root);
    let user = "src/models.ts::User";
    let request = "src/models.ts::Api.Request";
    let role = "src/models.ts::Role";
    let repo = "src/models.ts::Repo";
    let positions = |source: &str, target: &str| {
        type_reference_positions(&edges, &format!("src/app.ts::{source}"), target)
    };

    assert_eq!(positions("Local", user), ["field", "index_signature"]);
    assert_eq!(positions("Local", role), ["index_signature"]);
    // `Member` is `User` re-exported under another name: one edge.
    assert_eq!(positions("Pair", user), ["type_alias"]);
    assert_eq!(positions("Pair", repo), ["type_alias"]);
    assert_eq!(positions("Pair", "src/models.ts::UserId"), ["type_alias"]);
    assert_eq!(positions("load", "src/models.ts::UserId"), ["parameter"]);
    assert_eq!(positions("load", repo), ["parameter"]);
    assert_eq!(positions("load", user), ["parameter", "return"]);
    // Overloads are one node: their signatures merge into its edges.
    assert_eq!(positions("load2", user), ["return"]);
    assert_eq!(positions("load2", role), ["return"]);
    assert_eq!(
        positions("pick", user),
        ["type_parameter_constraint", "type_parameter_default"]
    );
    assert_eq!(positions("guard", user), ["type_predicate"]);
    assert_eq!(positions("Holder", request), ["heritage_type_argument"]);
    assert_eq!(positions("Holder", repo), ["field"]);
    assert_eq!(
        positions("Holder", "src/models.ts::DefaultModel"),
        ["field"]
    );
    assert_eq!(positions("Holder", user), ["parameter_property"]);
    assert_eq!(positions("Holder.constructor", user), ["parameter"]);
    assert_eq!(positions("Holder.handle", request), ["parameter"]);
    assert_eq!(positions("Holder.handler", role), ["field"]);
    assert_eq!(positions("Holder.self", "src/app.ts::Holder"), ["return"]);
    assert_eq!(positions("Box", user), ["type_parameter_constraint"]);
    assert_eq!(
        positions("Shapes.area", "src/app.ts::Shapes.Circle"),
        ["parameter"]
    );
    let holder = type_references(&edges, "src/app.ts::Holder");
    let kind = holder
        .iter()
        .find(|edge| edge.target == role)
        .expect("typeof Role");
    assert_eq!(kind.extra["relationship_role"], "type_query");
    assert!(
        holder
            .iter()
            .filter(|edge| edge.target != role)
            .all(|edge| edge.extra["relationship_role"] == "type_reference"),
        "{holder:#?}"
    );
    // The heritage base itself is IMPLEMENTS, not a type reference.
    assert!(!holder.iter().any(|edge| edge.target.ends_with("::Service")));

    // Type parameters, builtins, external packages, names the module does
    // not declare, and self references emit nothing.
    for source in ["Service", "Service.handle", "Tree", "Box"] {
        let found = type_references(&edges, &format!("src/app.ts::{source}"));
        let unexpected = found
            .iter()
            .filter(|edge| !(source == "Box" && edge.target == user))
            .collect::<Vec<_>>();
        assert!(unexpected.is_empty(), "{source}: {unexpected:#?}");
    }
    let type_edges = edges
        .iter()
        .filter(|edge| {
            edge.kind == "REFERENCES"
                && matches!(
                    edge.extra["relationship_role"].as_str(),
                    Some("type_reference" | "type_query")
                )
        })
        .collect::<Vec<_>>();
    for edge in &type_edges {
        assert!(
            edge.target.starts_with("src/models.ts::") || edge.target.starts_with("src/app.ts::"),
            "dangling type reference: {edge:?}"
        );
        assert!(
            !["Promise", "Map", "External", "Ghost", "T", "Tree"]
                .iter()
                .any(|name| edge.target.ends_with(&format!("::{name}"))),
            "{edge:?}"
        );
        assert_ne!(edge.source, edge.target, "self reference: {edge:?}");
    }
    let mut pairs = type_edges
        .iter()
        .map(|edge| (edge.source.as_str(), edge.target.as_str()))
        .collect::<Vec<_>>();
    let total = pairs.len();
    pairs.sort_unstable();
    pairs.dedup();
    assert_eq!(pairs.len(), total, "one edge per (source, target)");
}

#[test]
fn emits_typescript_body_type_references() {
    let repo_root = write_type_reference_repo("body-type-refs");
    let app = br#"import type { User, UserId } from "./models";
import { Repo, Role, Api } from "./models";
import * as m from "./models";
export function run(input: unknown) {
  const u: User = input as User;
  const r = new Repo<User>();
  const ok = { id: "1" } satisfies m.User;
  const ids = [] as UserId[];
  const legacy = <Api.Request>input;
  if (input instanceof Repo) {}
  const h: typeof m.helper = m.helper;
  interface LocalShape { owner: User; role: Role }
  type LocalAlias = Api.Request | LocalShape;
  const handle = (req: Api.Request): User => u;
  class LocalBox implements m.User { id = "x"; value!: UserId; take(x: Repo<User>): void {} }
  const local: LocalShape = { owner: u, role: Role.Admin };
  pick<UserId>(ids);
  return [u, r, ok, legacy, h, handle, local, LocalBox];
}
function pick<T>(items: T[]): T { return items[0]; }
export const api = {
  get(id: UserId): User { return {} as User; },
};
export const handler: (req: Api.Request) => void = (req) => {};
export const config: Record<string, Role> = {};
"#;
    let mut parser = RustOwnedParser::new();
    let (_nodes, edges) = parser.parse_file_in_repo(Some(&repo_root), "src/app.ts", app);
    let script = b"import { Repo } from \"./models\";\nexport function isRepo(x) { return x instanceof Repo; }\n";
    let (_nodes, js_edges) = parser.parse_file_in_repo(Some(&repo_root), "src/check.js", script);
    let _ = std::fs::remove_dir_all(&repo_root);
    let user = "src/models.ts::User";
    let repo = "src/models.ts::Repo";
    let user_id = "src/models.ts::UserId";
    let request = "src/models.ts::Api.Request";
    let positions = |source: &str, target: &str| {
        type_reference_positions(&edges, &format!("src/app.ts::{source}"), target)
    };

    assert_eq!(
        positions("run", user),
        [
            "variable_annotation",
            "as",
            "type_argument",
            "satisfies",
            "local_declaration",
            "return",
            "heritage",
            "parameter"
        ]
    );
    assert_eq!(positions("run", repo), ["instanceof", "parameter"]);
    assert_eq!(positions("run", user_id), ["as", "field", "type_argument"]);
    assert_eq!(
        positions("run", request),
        ["as", "local_declaration", "parameter"]
    );
    assert_eq!(
        positions("run", "src/models.ts::Role"),
        ["local_declaration"]
    );
    let run = type_references(&edges, "src/app.ts::run");
    let query = run
        .iter()
        .find(|edge| edge.target == "src/models.ts::helper")
        .expect("typeof m.helper");
    assert_eq!(query.extra["relationship_role"], "type_query");
    assert_eq!(
        query.extra["type_positions"],
        json!(["variable_annotation"])
    );
    // Local declarations are not nodes and never targets.
    assert_eq!(run.len(), 6, "{run:#?}");
    assert!(type_references(&edges, "src/app.ts::pick").is_empty());
    assert_eq!(positions("api.get", user_id), ["parameter"]);
    assert_eq!(positions("api.get", user), ["return", "as"]);
    assert_eq!(positions("handler", request), ["variable_annotation"]);
    let file_refs = type_references(&edges, "src/app.ts");
    assert_eq!(file_refs.len(), 1, "{file_refs:#?}");
    assert_eq!(file_refs[0].target, "src/models.ts::Role");
    assert_eq!(
        file_refs[0].extra["type_positions"],
        json!(["variable_annotation"])
    );
    assert_eq!(
        type_reference_positions(&js_edges, "src/check.js::isRepo", repo),
        ["instanceof"]
    );
}

#[test]
fn does_not_turn_type_literal_method_signatures_into_nodes() {
    let repo_root = write_type_reference_repo("type-literal-methods");
    std::fs::write(
        repo_root.join("repo.ts"),
        "export interface Repo { find(): void }\n",
    )
    .unwrap();
    let source = r#"import { Repo } from "./repo";
function f(p: { m(): void }): void;
function f(p: { m(): void; n(x: Repo): Repo }, q?: number): void;
function f(p: any, q?: any) {}
interface I {
  p: { inner(): void; deep: { d(): Repo } };
  m(): void;
  cb: (x: { z(): void }) => void;
}
type T = { tm(): void; nested: { k(): Repo } };
class C {
  field: { handler(): void } = { handler() {} };
  method(opts: { run(): Repo }): { done(): void } { return { done() {} }; }
}
export const g = (o: { w(): void }) => o.w();
declare function h(x: { y(): void }): void;
let v: { lm(): void } = { lm() {} };
function body() { const local: { bm(): Repo } = { bm: () => ({} as Repo) }; return local; }
"#;
    let mut parser = RustOwnedParser::new();
    let (nodes, edges) = parser.parse_file_in_repo(Some(&repo_root), "a.ts", source.as_bytes());
    let names = nodes
        .iter()
        .filter(|node| node.kind != "File")
        .map(|node| match &node.parent_name {
            Some(parent) => format!("{parent}.{}", node.name),
            None => node.name.clone(),
        })
        .collect::<std::collections::BTreeSet<_>>();
    // `v` is a module-scope object container (`= { lm() {} }`), not a type.
    let expected = [
        "C", "C.method", "I", "I.m", "T", "body", "f", "g", "h", "v", "v.lm",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(names, expected, "{nodes:?}");
    let f = nodes.iter().find(|node| node.name == "f").unwrap();
    assert_eq!(f.extra["overloads"], 2, "{f:?}");
    let interface_method = nodes
        .iter()
        .find(|node| node.name == "m" && node.parent_name.as_deref() == Some("I"))
        .unwrap();
    assert_eq!(interface_method.extra["is_abstract"], true);
    // The types named inside the literals still belong to the declaration.
    for (source, position) in [
        ("a.ts::f", "parameter"),
        ("a.ts::I", "field"),
        ("a.ts::T", "type_alias"),
        ("a.ts::C.method", "parameter"),
        ("a.ts::body", "variable_annotation"),
    ] {
        assert!(
            type_reference_positions(&edges, source, "repo.ts::Repo")
                .iter()
                .any(|found| found == position),
            "{source} {position}: {edges:?}"
        );
    }
    let qualified = names
        .iter()
        .map(|name| format!("a.ts::{name}"))
        .collect::<std::collections::BTreeSet<_>>();
    assert!(
        edges
            .iter()
            .filter(|edge| edge.kind == "CONTAINS")
            .all(|edge| qualified.contains(&edge.target)),
        "{edges:?}"
    );

    let _ = std::fs::remove_dir_all(&repo_root);
}
