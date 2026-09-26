use super::*;

#[test]
fn does_not_resolve_unrelated_member_calls_to_object_methods() {
    let source = br#"import express from "express";
const app = express();
export const api = {
  get() { return 1; },
  nested: { deep() { return 2; } },
};
app.get("/users", (req, res) => res.json([]));
export function useApi() {
  api.get();
  api.nested.deep();
  api.missing();
}
"#;
    let (_nodes, edges) = parse_javascript_like("server.ts", source, "typescript");
    assert!(
        !edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "server.ts"
                && (edge.target == "server.ts::get" || edge.target == "server.ts::api.get")
        }),
        "{edges:?}"
    );
    for target in ["server.ts::api.get", "server.ts::api.nested.deep"] {
        assert!(
            edges.iter().any(|edge| {
                edge.kind == "CALLS" && edge.source == "server.ts::useApi" && edge.target == target
            }),
            "missing CALLS useApi -> {target}: {edges:?}"
        );
    }
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS" && edge.source == "server.ts::useApi" && edge.target == "missing"
    }));
}

#[test]
fn parses_tsx_jsx_component_calls() {
    let mut repo_root = std::env::temp_dir();
    repo_root.push(format!(
        "dagayn-parser-tsx-jsx-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&repo_root);
    std::fs::create_dir_all(&repo_root).unwrap();
    std::fs::write(
        repo_root.join("MarkdownMsg.tsx"),
        b"export function MarkdownMsg() { return <div />; }\n",
    )
    .unwrap();

    let source = br#"import MarkdownMsg from './MarkdownMsg';

export function BookWorkspace() {
  return <section><MarkdownMsg text={value} /></section>;
}
"#;
    let mut parser = RustOwnedParser::new();
    let (_nodes, edges) = parser.parse_file_in_repo(Some(&repo_root), "BookWorkspace.tsx", source);
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "BookWorkspace.tsx::BookWorkspace"
            && edge.target == "MarkdownMsg.tsx::MarkdownMsg"
    }));
    assert!(!edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && (edge.target == "section" || edge.target == "div" || edge.target == "span")
    }));

    let _ = std::fs::remove_dir_all(&repo_root);
}

#[test]
fn parses_tsx_namespace_component_calls() {
    let mut repo_root = std::env::temp_dir();
    repo_root.push(format!(
        "dagayn-parser-tsx-namespace-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&repo_root);
    std::fs::create_dir_all(&repo_root).unwrap();
    std::fs::write(
        repo_root.join("MarkdownMsg.tsx"),
        b"export function MarkdownMsg() { return <div />; }\n",
    )
    .unwrap();

    let source = br#"import * as UI from './MarkdownMsg';

export function BookWorkspace() {
  return <UI.Messages.MarkdownMsg text={value} />;
}
"#;
    let mut parser = RustOwnedParser::new();
    let (_nodes, edges) = parser.parse_file_in_repo(Some(&repo_root), "BookWorkspace.tsx", source);
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "BookWorkspace.tsx::BookWorkspace"
            && edge.target == "MarkdownMsg.tsx::MarkdownMsg"
    }));

    let _ = std::fs::remove_dir_all(&repo_root);
}

#[test]
fn parses_jsx_component_calls() {
    let mut repo_root = std::env::temp_dir();
    repo_root.push(format!(
        "dagayn-parser-jsx-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&repo_root);
    std::fs::create_dir_all(&repo_root).unwrap();
    std::fs::write(
        repo_root.join("MarkdownMsg.jsx"),
        b"export function MarkdownMsg() { return <div />; }\n",
    )
    .unwrap();

    let source = br#"import { MarkdownMsg } from './MarkdownMsg';

export function BookWorkspace() {
  return <MarkdownMsg text={value} />;
}
"#;
    let mut parser = RustOwnedParser::new();
    let (_nodes, edges) = parser.parse_file_in_repo(Some(&repo_root), "BookWorkspace.jsx", source);
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "BookWorkspace.jsx::BookWorkspace"
            && edge.target == "MarkdownMsg.jsx::MarkdownMsg"
    }));

    let _ = std::fs::remove_dir_all(&repo_root);
}

#[test]
fn resolves_calls_scoped_to_dotted_owner_paths() {
    let file = "ns.ts";
    let node = |kind: NodeKind, name: &str, parent: Option<&str>| ParsedNode {
        kind,
        name: name.to_string(),
        file_path: FilePath::new(file),
        line_start: 1,
        line_end: 1,
        language: "typescript".to_string(),
        parent_name: parent.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({}),
    };
    let call = |source: &str, target: &str| ParsedEdge {
        kind: EdgeKind::Calls,
        source: source.to_string(),
        target: target.to_string(),
        file_path: FilePath::new(file),
        line: 1,
        extra: json!({}),
    };
    let nodes = vec![
        node(NodeKind::Class, "Outer", None),
        node(NodeKind::Class, "Inner", Some("Outer")),
        node(NodeKind::Function, "run", Some("Outer.Inner")),
        node(NodeKind::Function, "help", Some("Outer.Inner")),
        node(NodeKind::Class, "Other", None),
        node(NodeKind::Function, "help", Some("Other")),
        node(NodeKind::Function, "shared", Some("Other")),
        node(NodeKind::Function, "shared", Some("Outer")),
    ];
    let edges = resolve_rust_call_targets(
        &nodes,
        vec![
            // `this.help()` bound to the owner path.
            call("ns.ts::Outer.Inner.run", "Outer.Inner::help"),
            // A bare `help` prefers the caller's own owner path.
            call("ns.ts::Outer.Inner.run", "help"),
            // Not a same-file owner: left alone.
            call("ns.ts::Outer.Inner.run", "lib/util.ts::help"),
            // Nearest enclosing owner that declares it.
            call("ns.ts::Outer.Inner.run", "shared"),
        ],
        file,
    );
    let targets = edges
        .iter()
        .map(|edge| edge.target.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        targets,
        [
            "ns.ts::Outer.Inner.help",
            "ns.ts::Outer.Inner.help",
            "lib/util.ts::help",
            "ns.ts::Outer.shared"
        ]
    );
}

#[test]
fn resolves_typescript_member_calls() {
    let mut repo_root = std::env::temp_dir();
    repo_root.push(format!(
        "dagayn-parser-ts-member-calls-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&repo_root);
    std::fs::create_dir_all(repo_root.join("src/lib")).unwrap();
    let classes = r#"import { Repo } from "./interfaces";
import { Base } from "./lib/base";
export class DefaultShape { find(id: string) { return id; } area() { return 1; } }
export class Box extends Base {
  private cache = new DefaultShape();
  shape!: DefaultShape;
  constructor(private readonly repo: Repo) { super(repo); }
  static create(): Box { return new Box({} as Repo); }
  async load() {
    await this.repo.find("x");
    this.cache.area();
    this.shape.find("y");
    this.helper();
    super.helper();
    this.own();
  }
  own() {}
}
"#;
    let controller = r#"export class UsersService { findAll() { return []; } }
export class UsersController {
  constructor(private readonly users: UsersService) {}
  findAll() { return this.users.findAll(); }
}
"#;
    for (path, body) in [
        (
            "src/interfaces.ts",
            "export interface Repo { find(id: string): string; }\nexport interface Repo { count(): number; }\n",
        ),
        (
            "src/lib/base.ts",
            "export class Base { constructor(public dep: unknown) {} helper(): void {} }\n",
        ),
        ("src/classes.ts", classes),
        (
            "src/functions.ts",
            "export function decl() {}\nexport const api = { get() { return 1; } };\n",
        ),
        (
            "src/namespaces.ts",
            "export namespace Outer {\n  export function helper() {}\n  export class Inner { run() {} }\n  export namespace Deep { export function deepFn() {} }\n}\n",
        ),
        ("src/users.controller.ts", controller),
    ] {
        std::fs::write(repo_root.join(path), body).unwrap();
    }
    let app = br#"import * as fns from "./functions";
import { Box, DefaultShape } from "./classes";
import { Outer } from "./namespaces";
import type { Repo } from "./interfaces";
export function run(r: Repo) {
  fns.decl();
  fns.api.get();
  Box.create();
  Outer.helper();
  Outer.Deep.deepFn();
  const shape = new DefaultShape();
  shape.area();
  r.find("id");
  const typed: Repo = r;
  typed.find("x");
  res.json();
  r.count();
  new Outer.Inner().run();
}
function json() {}
"#;
    let mut parser = RustOwnedParser::new();
    let (_nodes, edges) = parser.parse_file_in_repo(Some(&repo_root), "src/app.ts", app);
    fn calls<'a>(edges: &'a [ParsedEdge], source: &str, target: &str) -> Option<&'a ParsedEdge> {
        edges
            .iter()
            .find(|edge| edge.kind == "CALLS" && edge.source == source && edge.target == target)
    }
    for target in [
        "src/functions.ts::decl",
        "src/functions.ts::api.get",
        "src/classes.ts::Box.create",
        "src/namespaces.ts::Outer.helper",
        "src/namespaces.ts::Outer.Deep.deepFn",
        "src/classes.ts::DefaultShape.area",
        "src/interfaces.ts::Repo.find",
        "src/interfaces.ts::Repo.count",
        "src/namespaces.ts::Outer.Inner.run",
    ] {
        assert!(
            calls(&edges, "src/app.ts::run", target).is_some(),
            "run -> {target}: {edges:?}"
        );
    }
    let unknown = edges
        .iter()
        .find(|edge| edge.kind == "CALLS" && edge.line == 16)
        .expect("res.json()");
    assert_eq!(unknown.target, "json", "{unknown:?}");
    assert_eq!(unknown.extra["receiver_unknown"], true);

    let (_nodes, edges) =
        parser.parse_file_in_repo(Some(&repo_root), "src/classes.ts", classes.as_bytes());
    let load = "src/classes.ts::Box.load";
    assert!(
        calls(&edges, load, "src/interfaces.ts::Repo.find").is_some(),
        "{edges:?}"
    );
    assert!(calls(&edges, load, "src/classes.ts::DefaultShape.find").is_some());
    assert_eq!(
        edges
            .iter()
            .filter(|edge| edge.kind == "CALLS"
                && edge.source == load
                && edge.target == "src/classes.ts::DefaultShape.find")
            .count(),
        1,
        "only this.shape.find() binds to DefaultShape.find"
    );
    assert!(calls(&edges, load, "src/classes.ts::DefaultShape.area").is_some());
    assert!(calls(&edges, load, "src/classes.ts::Box.own").is_some());
    let inherited = edges
        .iter()
        .filter(|edge| {
            edge.kind == "CALLS"
                && edge.source == load
                && edge.target == "src/lib/base.ts::Base.helper"
        })
        .collect::<Vec<_>>();
    assert_eq!(
        inherited.len(),
        2,
        "this.helper() and super.helper(): {edges:?}"
    );
    for edge in inherited {
        assert_eq!(edge.extra["confidence_tier"], "MEDIUM");
    }
    let super_call = calls(
        &edges,
        "src/classes.ts::Box.constructor",
        "src/lib/base.ts::Base",
    )
    .unwrap_or_else(|| panic!("super(repo): {edges:?}"));
    assert_eq!(super_call.extra["call_kind"], "super");

    let (_nodes, edges) = parser.parse_file_in_repo(
        Some(&repo_root),
        "src/users.controller.ts",
        controller.as_bytes(),
    );
    let find_all = "src/users.controller.ts::UsersController.findAll";
    assert!(
        calls(
            &edges,
            find_all,
            "src/users.controller.ts::UsersService.findAll"
        )
        .is_some(),
        "{edges:?}"
    );
    assert!(calls(&edges, find_all, find_all).is_none(), "no self-loop");

    let _ = std::fs::remove_dir_all(&repo_root);
}

#[test]
fn resolves_javascript_member_calls_with_evidence_only() {
    let source = br#"class Repo { find() {} }
class Base { helper() {} }
class Service extends Base {
  constructor(users) {
    super();
    this.repo = new Repo();
    this.users = users;
  }
  findAll() {
    this.repo.find();
    this.users.findAll();
    this.helper();
    super.helper();
    this.setState();
  }
}
class Widget extends External {
  render() { this.setState(); }
}
function find() {}
"#;
    let mut parser = RustOwnedParser::new();
    let (_nodes, edges) = parser.parse_file("src/service.js", source);
    let calls_on = |line: i64| {
        edges
            .iter()
            .filter(|edge| edge.kind == "CALLS" && edge.line == line)
            .collect::<Vec<_>>()
    };
    let target_on = |line: i64| {
        let found = calls_on(line);
        assert_eq!(found.len(), 1, "line {line}: {edges:?}");
        found[0]
    };
    let super_call = target_on(5);
    assert_eq!(super_call.source, "src/service.js::Service.constructor");
    assert_eq!(super_call.target, "src/service.js::Base");
    assert_eq!(super_call.extra["call_kind"], "super");
    assert_eq!(target_on(10).target, "src/service.js::Repo.find");
    let untyped = target_on(11);
    assert_eq!(untyped.target, "findAll", "no self-loop: {untyped:?}");
    assert_eq!(untyped.extra["receiver_unknown"], true);
    for line in [12, 13] {
        let inherited = target_on(line);
        assert_eq!(inherited.target, "src/service.js::Base.helper");
        assert_eq!(inherited.extra["confidence_tier"], "MEDIUM");
    }
    for line in [14, 18] {
        let unknown = target_on(line);
        assert_eq!(unknown.target, "setState", "{unknown:?}");
        assert_eq!(unknown.extra["receiver_unknown"], true);
    }
    let external_super = edges
        .iter()
        .filter(|edge| edge.kind == "CALLS" && edge.extra["call_kind"] == "super")
        .count();
    assert_eq!(external_super, 1);
}

#[test]
fn parses_react_hoc_wrapped_components() {
    let mut repo_root = std::env::temp_dir();
    repo_root.push(format!(
        "dagayn-parser-ts-hoc-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&repo_root);
    std::fs::create_dir_all(repo_root.join("src")).unwrap();
    let component = br#"import React, { memo, forwardRef } from "react";
import { observer } from "mobx-react";
import type { Props } from "./types";
export const Memo = memo(function Inner({ x }: Props) { return helper(x); });
export const Arrow = memo(() => <div>{helper()}</div>);
export const Fwd = React.forwardRef<HTMLInputElement, Props>((props, ref) => <input ref={ref} />);
export const Obs = observer(() => { helper(); return <Fwd />; });
export const Nested = memo(forwardRef(function N(p, r) { return helper(); }), areEqual);
const items = [1, 2];
export const doubled = items.map((x) => x * 2);
export const composed = compose(helper, areEqual);
export default memo(function Page() { return <Memo />; });
function helper(_x?: unknown) { return null; }
function areEqual() { return true; }
"#;
    for (path, body) in [
        (
            "src/types.ts",
            &b"export interface Props { x: number }\n"[..],
        ),
        ("src/Comp.tsx", &component[..]),
    ] {
        std::fs::write(repo_root.join(path), body).unwrap();
    }
    let mut parser = RustOwnedParser::new();
    let (nodes, edges) = parser.parse_file_in_repo(Some(&repo_root), "src/Comp.tsx", component);
    let node = |name: &str| {
        nodes
            .iter()
            .find(|node| node.name == name && node.parent_name.is_none())
    };
    for name in ["Memo", "Arrow", "Fwd", "Obs", "Nested", "default"] {
        let found = node(name).unwrap_or_else(|| panic!("{name}: {nodes:?}"));
        assert_eq!(found.kind, "Function", "{name}");
    }
    for name in ["doubled", "composed", "Inner", "N", "Page", "items"] {
        assert!(node(name).is_none(), "{name} must not be a node");
    }
    assert_eq!(
        node("Memo").unwrap().extra["wrapped_by"],
        serde_json::json!(["memo"])
    );
    assert_eq!(node("Memo").unwrap().extra["expression_name"], "Inner");
    assert_eq!(
        node("Memo").unwrap().params.as_deref(),
        Some("({ x }: Props)")
    );
    assert_eq!(
        node("Fwd").unwrap().extra["wrapped_by"],
        serde_json::json!(["React.forwardRef"])
    );
    assert!(
        node("Arrow")
            .unwrap()
            .extra
            .get("expression_name")
            .is_none()
    );
    assert_eq!(
        node("Nested").unwrap().extra["wrapped_by"],
        serde_json::json!(["memo", "forwardRef"])
    );
    let default = node("default").unwrap();
    assert_eq!(default.extra["export_default"], true);
    assert_eq!(default.extra["wrapped_by"], serde_json::json!(["memo"]));
    assert_eq!(default.extra["expression_name"], "Page");

    let qn = |name: &str| format!("src/Comp.tsx::{name}");
    let has = |kind: &str, source: &str, target: &str| {
        edges
            .iter()
            .any(|edge| edge.kind == kind && edge.source == source && edge.target == target)
    };
    for caller in ["Memo", "Arrow", "Obs", "Nested"] {
        assert!(
            has("CALLS", &qn(caller), &qn("helper")),
            "{caller}: {edges:?}"
        );
    }
    assert!(has("CALLS", &qn("Obs"), &qn("Fwd")));
    assert!(has("CALLS", &qn("default"), &qn("Memo")));
    // The wrapper runs at module scope: the File calls it.
    assert!(has("CALLS", "src/Comp.tsx", "react::memo"));
    assert!(has("CALLS", "src/Comp.tsx", "react::forwardRef"));
    assert!(has("CALLS", "src/Comp.tsx", "mobx-react::observer"));
    assert!(has("REFERENCES", "src/Comp.tsx", &qn("areEqual")));
    assert!(!has("CALLS", "src/Comp.tsx", &qn("helper")));
    assert!(has("CALLS", "src/Comp.tsx", "compose"));
    // Types of the wrapped function and of the wrapper's type arguments.
    assert!(has("REFERENCES", &qn("Memo"), "src/types.ts::Props"));
    assert!(has("REFERENCES", &qn("Fwd"), "src/types.ts::Props"));
    assert!(
        edges
            .iter()
            .any(|edge| edge.kind == "CONTAINS" && edge.target == qn("Fwd"))
    );

    let usage = br#"import Page, { Fwd, Nested } from "./Comp";
export function App() { return <><Fwd /><Nested /><Page /></>; }
"#;
    std::fs::write(repo_root.join("src/App.tsx"), usage).unwrap();
    let (_nodes, edges) = parser.parse_file_in_repo(Some(&repo_root), "src/App.tsx", usage);
    for target in ["Fwd", "Nested", "default"] {
        assert!(
            edges.iter().any(|edge| edge.kind == "CALLS"
                && edge.source == "src/App.tsx::App"
                && edge.target == qn(target)),
            "{target}: {edges:?}"
        );
    }

    // The same rule in JavaScript, including a CommonJS-less `React` global.
    let (nodes, edges) = parse_javascript_like(
        "hoc.jsx",
        b"export const Card = React.memo(function () { return run(); });\nfunction run() {}\n",
        "javascript",
    );
    let card = nodes.iter().find(|node| node.name == "Card").expect("Card");
    assert_eq!(card.extra["wrapped_by"], serde_json::json!(["React.memo"]));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS" && edge.source == "hoc.jsx::Card" && edge.target == "hoc.jsx::run"
    }));

    let _ = std::fs::remove_dir_all(&repo_root);
}
