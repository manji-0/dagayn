use super::*;

#[test]
fn resolves_typescript_imported_call_targets() {
    let mut repo_root = std::env::temp_dir();
    repo_root.push(format!(
        "dagayn-parser-ts-import-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&repo_root);
    std::fs::create_dir_all(repo_root.join("src")).unwrap();
    std::fs::write(
        repo_root.join("src/helper.ts"),
        b"export function helper() { return 1; }\n",
    )
    .unwrap();

    let source = br#"import { helper } from './helper';

export function run() {
  helper();
  const refs = [helper];
}
"#;
    let mut parser = RustOwnedParser::new();
    let (_nodes, edges) = parser.parse_file_in_repo(Some(&repo_root), "src/consumer.ts", source);
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "src/consumer.ts::run"
            && edge.target == "src/helper.ts::helper"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "REFERENCES"
            && edge.source == "src/consumer.ts::run"
            && edge.target == "src/helper.ts::helper"
    }));

    let _ = std::fs::remove_dir_all(&repo_root);
}

#[test]
fn resolves_typescript_tsconfig_alias_imports() {
    let mut repo_root = std::env::temp_dir();
    repo_root.push(format!(
        "dagayn-parser-ts-alias-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&repo_root);
    std::fs::create_dir_all(repo_root.join("src/lib")).unwrap();
    std::fs::write(
        repo_root.join("tsconfig.json"),
        br#"{
  "compilerOptions": {
    "baseUrl": ".",
    "paths": {
      "@/*": ["src/*"],
    },
  },
}
"#,
    )
    .unwrap();
    std::fs::write(
        repo_root.join("src/lib/utils.ts"),
        b"export function cn(...args: string[]): string { return args.join(' '); }\n",
    )
    .unwrap();

    let source = br#"import { cn } from '@/lib/utils';

export function formatUser(name: string): string {
  return cn('user', name);
}
"#;
    let mut parser = RustOwnedParser::new();
    let (_nodes, edges) = parser.parse_file_in_repo(Some(&repo_root), "alias_importer.ts", source);
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM"
            && edge.source == "alias_importer.ts"
            && edge.target == "src/lib/utils.ts"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "alias_importer.ts::formatUser"
            && edge.target == "src/lib/utils.ts::cn"
    }));

    let _ = std::fs::remove_dir_all(&repo_root);
}

#[test]
fn resolves_typescript_dotted_basename_imports() {
    let mut repo_root = std::env::temp_dir();
    repo_root.push(format!(
        "dagayn-parser-ts-dotted-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&repo_root);
    std::fs::create_dir_all(repo_root.join("src/lib")).unwrap();
    std::fs::create_dir_all(repo_root.join("src/dir")).unwrap();
    std::fs::write(
        repo_root.join("tsconfig.json"),
        br#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@/*": ["src/*"] } } }"#,
    )
    .unwrap();
    for (path, body) in [
        ("src/user.service.ts", "export class UserService {}\n"),
        ("src/hero.component.ts", "export class HeroComponent {}\n"),
        ("src/esm-compat.ts", "export function compat() {}\n"),
        ("src/lib/index.ts", "export function fromLib() {}\n"),
        (
            "src/types.d.ts",
            "export declare function declared(): void;\n",
        ),
        ("src/both.ts", "export function both() {}\n"),
        ("src/both.d.ts", "export declare function both(): void;\n"),
        ("src/dir.ts", "export function dirFile() {}\n"),
        ("src/dir/index.ts", "export function dirIndex() {}\n"),
        ("src/util.mts", "export function utilFn() {}\n"),
        ("src/conf.cjs", "module.exports = {};\n"),
    ] {
        std::fs::write(repo_root.join(path), body).unwrap();
    }

    let source = br#"import { UserService } from "./user.service";
import { HeroComponent } from "@/hero.component";
import { compat } from "./esm-compat.js";
import { fromLib } from "./lib";
import { declared } from "./types";
import { both } from "./both";
import { dirFile } from "./dir";
import { utilFn } from "./util.mjs";
import conf from "./conf";

export function run() {
  new UserService();
  new HeroComponent();
  compat();
  fromLib();
  declared();
  both();
  dirFile();
  utilFn();
}
"#;
    let mut parser = RustOwnedParser::new();
    let (_nodes, edges) = parser.parse_file_in_repo(Some(&repo_root), "src/consumer.ts", source);
    for target in [
        "src/user.service.ts",
        "src/hero.component.ts",
        "src/esm-compat.ts",
        "src/lib/index.ts",
        "src/types.d.ts",
        "src/both.ts",
        "src/dir.ts",
        "src/util.mts",
        "src/conf.cjs",
    ] {
        assert!(
            edges.iter().any(|edge| {
                edge.kind == "IMPORTS_FROM"
                    && edge.source == "src/consumer.ts"
                    && edge.target == target
            }),
            "missing IMPORTS_FROM {target}: {edges:?}"
        );
    }
    for target in [
        "src/user.service.ts::UserService",
        "src/hero.component.ts::HeroComponent",
        "src/esm-compat.ts::compat",
        "src/lib/index.ts::fromLib",
        "src/types.d.ts::declared",
        "src/both.ts::both",
        "src/dir.ts::dirFile",
        "src/util.mts::utilFn",
    ] {
        assert!(
            edges.iter().any(|edge| {
                edge.kind == "CALLS"
                    && edge.source == "src/consumer.ts::run"
                    && edge.target == target
            }),
            "missing CALLS {target}: {edges:?}"
        );
    }

    let _ = std::fs::remove_dir_all(&repo_root);
}

#[test]
fn resolves_typescript_aliased_and_default_imports() {
    let mut repo_root = std::env::temp_dir();
    repo_root.push(format!(
        "dagayn-parser-ts-default-import-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&repo_root);
    std::fs::create_dir_all(repo_root.join("src")).unwrap();
    for (path, body) in [
        ("src/functions.ts", "export function decl() {}\n"),
        (
            "src/Button.tsx",
            "export function Button() { return <b />; }\nexport default function DefaultCard() { return <div />; }\n",
        ),
        (
            "src/default-arrow.ts",
            "export default (x: number) => x * 2;\n",
        ),
        (
            "src/default-anon-class.ts",
            "export default class { hello() {} }\n",
        ),
        (
            "src/default-ident.ts",
            "function impl() {}\nexport default impl;\n",
        ),
        (
            "src/default-alias.ts",
            "function aliased() {}\nexport { aliased as default };\n",
        ),
        (
            "src/default-alias-js.js",
            "function aliasedJs() {}\nexport { aliasedJs as default };\n",
        ),
        (
            "src/named-only.tsx",
            "export function MarkdownMsg() { return <div />; }\n",
        ),
    ] {
        std::fs::write(repo_root.join(path), body).unwrap();
    }

    let source = br#"import { decl as renamed } from "./functions";
import Card from "./Button";
import def from "./default-arrow";
import Anon from "./default-anon-class";
import impl2 from "./default-ident";
import Aliased from "./default-alias";
import AliasedJs from "./default-alias-js";
import MarkdownMsg from "./named-only";
import { default as Explicit } from "./default-ident";
import * as UI from "./Button";

export function App() {
  renamed();
  def(1);
  new Anon();
  impl2();
  Aliased();
  AliasedJs();
  Explicit();
  const refs = [renamed];
  return <><Card /><MarkdownMsg /><UI.Button /></>;
}
"#;
    let mut parser = RustOwnedParser::new();
    let (_nodes, edges) = parser.parse_file_in_repo(Some(&repo_root), "src/App.tsx", source);
    let calls = |target: &str| {
        edges.iter().any(|edge| {
            edge.kind == "CALLS" && edge.source == "src/App.tsx::App" && edge.target == target
        })
    };
    for target in [
        "src/functions.ts::decl",
        "src/Button.tsx::DefaultCard",
        "src/default-arrow.ts::default",
        "src/default-anon-class.ts::default",
        "src/default-ident.ts::impl",
        "src/default-alias.ts::aliased",
        "src/default-alias-js.js::aliasedJs",
        "src/named-only.tsx::MarkdownMsg",
        "src/Button.tsx::Button",
    ] {
        assert!(calls(target), "missing CALLS App -> {target}: {edges:?}");
    }
    for wrong in [
        "src/functions.ts::renamed",
        "src/Button.tsx::Card",
        "src/default-arrow.ts::def",
        "src/default-anon-class.ts::Anon",
        "src/default-ident.ts::impl2",
        "src/default-ident.ts::Explicit",
    ] {
        assert!(
            !edges.iter().any(|edge| edge.target == wrong),
            "unexpected target {wrong}"
        );
    }
    assert!(edges.iter().any(|edge| {
        edge.kind == "REFERENCES"
            && edge.source == "src/App.tsx::App"
            && edge.target == "src/functions.ts::decl"
    }));

    let _ = std::fs::remove_dir_all(&repo_root);
}

#[test]
fn resolves_typescript_barrel_reexports_to_origin() {
    let mut repo_root = std::env::temp_dir();
    repo_root.push(format!(
        "dagayn-parser-ts-barrel-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&repo_root);
    std::fs::create_dir_all(repo_root.join("src/components")).unwrap();
    std::fs::write(
        repo_root.join("src/components/MarkdownMsg.ts"),
        b"export function MarkdownMsg() { return 'ok'; }\n",
    )
    .unwrap();
    std::fs::write(
        repo_root.join("src/components/index.ts"),
        b"export { MarkdownMsg as Msg } from './MarkdownMsg';\n",
    )
    .unwrap();

    let source = br#"import { Msg } from './components';

export function render() {
  return Msg();
}
"#;
    let mut parser = RustOwnedParser::new();
    let (_nodes, edges) = parser.parse_file_in_repo(Some(&repo_root), "src/app.ts", source);
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "src/app.ts::render"
            && edge.target == "src/components/MarkdownMsg.ts::MarkdownMsg"
    }));

    let _ = std::fs::remove_dir_all(&repo_root);
}

#[test]
fn resolves_typescript_star_barrel_reexports_to_origin() {
    let mut repo_root = std::env::temp_dir();
    repo_root.push(format!(
        "dagayn-parser-ts-star-barrel-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&repo_root);
    std::fs::create_dir_all(repo_root.join("src/components")).unwrap();
    std::fs::write(
        repo_root.join("src/components/MarkdownMsg.ts"),
        b"export function MarkdownMsg() { return 'ok'; }\n",
    )
    .unwrap();
    std::fs::write(
        repo_root.join("src/components/index.ts"),
        b"export * from './MarkdownMsg';\n",
    )
    .unwrap();

    let source = br#"import { MarkdownMsg } from './components';

export function render() {
  return MarkdownMsg();
}
"#;
    let mut parser = RustOwnedParser::new();
    let (_nodes, edges) = parser.parse_file_in_repo(Some(&repo_root), "src/app.ts", source);
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "src/app.ts::render"
            && edge.target == "src/components/MarkdownMsg.ts::MarkdownMsg"
    }));

    let _ = std::fs::remove_dir_all(&repo_root);
}

#[test]
fn resolves_typescript_reexports_and_namespace_reexports() {
    let mut repo_root = std::env::temp_dir();
    repo_root.push(format!(
        "dagayn-parser-ts-reexports-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&repo_root);
    std::fs::create_dir_all(repo_root.join("src/barrel")).unwrap();
    for (path, body) in [
        (
            "src/barrel/a.ts",
            "export function fromA() {}\nexport class ClassA {}\nexport function shared() {}\nexport function winner() {}\nfunction hidden() {}\n",
        ),
        (
            "src/barrel/b.ts",
            "export function fromB() {}\nexport default function defaultB() {}\nexport function shared() {}\nexport function winner() {}\nexport function hidden() {}\nexport class Klass { m() {} }\n",
        ),
        (
            "src/barrel/c.ts",
            "export * from \"./index\";\nexport function fromC() {}\n",
        ),
        (
            "src/barrel/index.ts",
            r#"export * from "./a";
export * from "./b";
export * from "./c";
export * as bns from "./b";
export { fromB as renamedB, default as defB } from "./b";
import { fromA } from "./a";
export { fromA as localRenamed };
import * as nsA from "./a";
export { nsA };
import defaultOfB from "./b";
export default defaultOfB;
export function winner() {}
"#,
        ),
        (
            "src/export-assign.ts",
            "function main() {}\nexport = main;\n",
        ),
    ] {
        std::fs::write(repo_root.join(path), body).unwrap();
    }

    let source = br#"import { fromA, renamedB, localRenamed, bns, ClassA, defB } from "./barrel";
import { shared, winner, hidden, fromC, nsA } from "./barrel";
import barrelDefault from "./barrel";
import * as all from "./barrel";
import assigned from "./export-assign";

export function useBarrel() {
  fromA();
  renamedB();
  localRenamed();
  bns.fromB();
  new ClassA();
  defB();
  shared();
  winner();
  hidden();
  fromC();
  nsA.fromA();
  all.bns.fromB();
  barrelDefault();
  assigned();
}

export function typed(k: bns.Klass) {
  k.m();
}
"#;
    let mut parser = RustOwnedParser::new();
    let (_nodes, edges) = parser.parse_file_in_repo(Some(&repo_root), "src/app.ts", source);
    let call_at = |line: i64| {
        edges
            .iter()
            .find(|edge| {
                edge.kind == "CALLS" && edge.source == "src/app.ts::useBarrel" && edge.line == line
            })
            .map(|edge| edge.target.as_str())
    };
    for (line, target) in [
        (8, "src/barrel/a.ts::fromA"),
        (9, "src/barrel/b.ts::fromB"),
        (10, "src/barrel/a.ts::fromA"),
        (11, "src/barrel/b.ts::fromB"),
        (12, "src/barrel/a.ts::ClassA"),
        (13, "src/barrel/b.ts::defaultB"),
        (15, "src/barrel/index.ts::winner"),
        (16, "src/barrel/b.ts::hidden"),
        (17, "src/barrel/c.ts::fromC"),
        (18, "src/barrel/a.ts::fromA"),
        (19, "src/barrel/b.ts::fromB"),
        (20, "src/barrel/b.ts::defaultB"),
        (21, "src/export-assign.ts::main"),
    ] {
        assert_eq!(call_at(line), Some(target), "line {line}: {edges:?}");
    }
    // `shared` is exported by both `export *` sources: ambiguous, so it
    // binds to neither origin.
    let shared = call_at(14);
    assert!(
        !matches!(
            shared,
            Some("src/barrel/a.ts::shared" | "src/barrel/b.ts::shared")
        ),
        "ambiguous star export resolved: {shared:?}"
    );
    // `bns.Klass` as a type enters the re-exported namespace.
    assert!(
        edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "src/app.ts::typed"
                && edge.target == "src/barrel/b.ts::Klass.m"
        }),
        "missing CALLS typed -> b.ts::Klass.m: {edges:?}"
    );

    let _ = std::fs::remove_dir_all(&repo_root);
}

#[test]
fn resolves_javascript_commonjs_exports() {
    let mut repo_root = std::env::temp_dir();
    repo_root.push(format!(
        "dagayn-parser-js-commonjs-exports-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&repo_root);
    std::fs::create_dir_all(repo_root.join("src")).unwrap();
    for (path, body) in [
        (
            "src/helpers.js",
            "function helper() {}\nfunction other() {}\nmodule.exports = { helper, renamed: other };\n",
        ),
        (
            "src/single.js",
            "function config() {}\nmodule.exports = config;\n",
        ),
        (
            "src/props.cjs",
            "function one() {}\nfunction two() {}\nexports.one = one;\nmodule.exports.two = two;\n",
        ),
        ("src/barrel.js", "export * from \"./props.cjs\";\n"),
    ] {
        std::fs::write(repo_root.join(path), body).unwrap();
    }

    let source = br#"import helpers from "./helpers";
import { helper, renamed } from "./helpers";
import cfg from "./single";
import { one, two } from "./props.cjs";
import * as props from "./props.cjs";
import { two as viaBarrel } from "./barrel";

export function main() {
  helper();
  renamed();
  cfg();
  one();
  two();
  helpers.renamed();
  props.one();
  viaBarrel();
}
"#;
    let mut parser = RustOwnedParser::new();
    let (_nodes, edges) = parser.parse_file_in_repo(Some(&repo_root), "src/app.js", source);
    let call_at = |line: i64| {
        edges
            .iter()
            .find(|edge| {
                edge.kind == "CALLS" && edge.source == "src/app.js::main" && edge.line == line
            })
            .map(|edge| edge.target.as_str())
    };
    for (line, target) in [
        (9, "src/helpers.js::helper"),
        (10, "src/helpers.js::other"),
        (11, "src/single.js::config"),
        (12, "src/props.cjs::one"),
        (13, "src/props.cjs::two"),
        (14, "src/helpers.js::other"),
        (15, "src/props.cjs::one"),
        (16, "src/props.cjs::two"),
    ] {
        assert_eq!(call_at(line), Some(target), "line {line}: {edges:?}");
    }

    let _ = std::fs::remove_dir_all(&repo_root);
}

#[test]
fn parses_javascript_commonjs_and_dynamic_imports() {
    let mut repo_root = std::env::temp_dir();
    repo_root.push(format!(
        "dagayn-parser-js-require-imports-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&repo_root);
    std::fs::create_dir_all(repo_root.join("src")).unwrap();
    for (path, body) in [
        (
            "src/helpers.js",
            "function helper() {}\nfunction other() {}\nmodule.exports = { helper, renamed: other };\n",
        ),
        (
            "src/single.js",
            "function config() {}\nmodule.exports = config;\n",
        ),
        (
            "src/esm.ts",
            "export function esmFn() {}\nexport default function main() {}\n",
        ),
        ("src/inner.js", "exports.inner = function () {};\n"),
        ("src/lazy.js", "export function lazy() {}\n"),
        (
            "src/typed.ts",
            "function typedMain() {}\nexport = typedMain;\n",
        ),
    ] {
        std::fs::write(repo_root.join(path), body).unwrap();
    }

    let source = br#"const helpers = require("./helpers");
const { helper, renamed: alias } = require("./helpers");
const cfg = require("./single");
const esm = require("./esm");
const picked = require("./helpers").renamed;
const path = require("path");
const dynamicName = "./lazy";
function main() {
  helpers.renamed();
  helper();
  alias();
  cfg();
  esm.esmFn();
  picked();
  const inner = require("./inner");
  import("./lazy");
  require(dynamicName);
  import(dynamicName);
  require.resolve("./lazy");
}
"#;
    let mut parser = RustOwnedParser::new();
    let (_nodes, edges) = parser.parse_file_in_repo(Some(&repo_root), "src/app.js", source);
    let import_at = |line: i64| {
        edges
            .iter()
            .filter(|edge| edge.kind == "IMPORTS_FROM" && edge.line == line)
            .map(|edge| {
                assert_eq!(edge.source, "src/app.js");
                (
                    edge.target.as_str(),
                    edge.extra
                        .get("import_kind")
                        .and_then(|kind| kind.as_str())
                        .unwrap_or(""),
                )
            })
            .collect::<Vec<_>>()
    };
    for (line, target, kind) in [
        (1, "src/helpers.js", "require"),
        (2, "src/helpers.js", "require"),
        (3, "src/single.js", "require"),
        (4, "src/esm.ts", "require"),
        (5, "src/helpers.js", "require"),
        (6, "path", "require"),
        (15, "src/inner.js", "require"),
        (16, "src/lazy.js", "dynamic"),
    ] {
        assert_eq!(
            import_at(line),
            vec![(target, kind)],
            "line {line}: {edges:?}"
        );
    }
    for line in [17, 18, 19] {
        assert!(import_at(line).is_empty(), "line {line}: {edges:?}");
    }
    assert!(
        !edges.iter().any(
            |edge| edge.kind == "CALLS" && matches!(edge.target.as_str(), "require" | "import")
        ),
        "{edges:?}"
    );

    let call_at = |line: i64| {
        edges
            .iter()
            .find(|edge| {
                edge.kind == "CALLS" && edge.source == "src/app.js::main" && edge.line == line
            })
            .map(|edge| edge.target.as_str())
    };
    for (line, target) in [
        (9, "src/helpers.js::other"),
        (10, "src/helpers.js::helper"),
        (11, "src/helpers.js::other"),
        (12, "src/single.js::config"),
        (13, "src/esm.ts::esmFn"),
        (14, "src/helpers.js::other"),
    ] {
        assert_eq!(call_at(line), Some(target), "line {line}: {edges:?}");
    }

    let ts_source = br#"import lib = require("./helpers");
import typed = require("./typed");
import fs = require("fs");
export function run() {
  lib.helper();
  typed();
}
"#;
    let (_nodes, edges) = parser.parse_file_in_repo(Some(&repo_root), "src/run.ts", ts_source);
    let imports = edges
        .iter()
        .filter(|edge| edge.kind == "IMPORTS_FROM")
        .map(|edge| {
            (
                edge.line,
                edge.target.as_str(),
                edge.extra.get("import_kind").and_then(|kind| kind.as_str()),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        imports,
        vec![
            (1, "src/helpers.js", Some("import_equals")),
            (2, "src/typed.ts", Some("import_equals")),
            (3, "fs", Some("import_equals")),
        ],
        "{edges:?}"
    );
    let calls = edges
        .iter()
        .filter(|edge| edge.kind == "CALLS" && edge.source == "src/run.ts::run")
        .map(|edge| (edge.line, edge.target.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        calls,
        vec![
            (5, "src/helpers.js::helper"),
            (6, "src/typed.ts::typedMain")
        ],
        "{edges:?}"
    );

    let _ = std::fs::remove_dir_all(&repo_root);
}

#[test]
fn parses_javascript_cross_artifact_edges() {
    let source = br#"child_process.spawn("./bin/tool", ["--flag"]);

function runDynamic(cmd) {
  child_process.exec(cmd);
}
"#;
    let (nodes, edges) = parse_javascript_like("bridge.js", source, "javascript");
    assert!(
        nodes
            .iter()
            .any(|node| node.kind == "Function" && node.name == "runDynamic")
    );
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.source == "bridge.js"
            && edge.target == "./bin/tool"
            && edge.extra["evidence_source"] == "child_process.spawn"
            && edge.extra["confidence_tier"] == "HIGH"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.source == "bridge.js::runDynamic"
            && edge.target == "<dynamic:child_process.exec@bridge.js:4>"
            && edge.extra["confidence_tier"] == "LOW"
    }));
}

#[test]
fn qualifies_external_package_symbols() {
    let mut repo_root = std::env::temp_dir();
    repo_root.push(format!(
        "dagayn-parser-ts-external-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&repo_root);
    std::fs::create_dir_all(repo_root.join("src/lib")).unwrap();
    std::fs::write(
        repo_root.join("tsconfig.json"),
        br#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@app/*": ["src/*"] } } }"#,
    )
    .unwrap();
    for (path, body) in [
        ("src/lib/util.ts", "export function util() {}\n"),
        ("src/helper.ts", "export function helper() {}\n"),
        (
            "src/ClassComp.tsx",
            "export class ClassComp { render() { return null; } }\n",
        ),
    ] {
        std::fs::write(repo_root.join(path), body).unwrap();
    }
    let source = br#"import React, { useState as useLocalState, type FC } from "react";
import * as fs from "node:fs";
import { map } from "lodash/fp";
import { Button, Form } from "antd";
import { Injectable } from "@nestjs/common";
import express from "express";
import cors from "cors";
import { z } from "zod";
import { helper } from "./helper";
import { util } from "@app/lib/util";
import { missing } from "@app/lib/missing";
import { gone } from "./gone";
const lib = require("lib-cjs");
const { pick } = require("lodash");

@Injectable()
export class Svc {}

export const Label: FC = () => null;

export function App() {
  const [n] = useLocalState(0);
  React.useEffect(() => {});
  fs.readFile("x", () => {});
  map(helper);
  z.object({});
  lib();
  lib.run();
  pick();
  util();
  missing();
  gone();
  const app = express();
  app.use(cors);
  app.get("/");
  return <Form.Item><Button /></Form.Item>;
}

function shadow() {
  const pick = () => 1;
  pick();
}
"#;
    let mut parser = RustOwnedParser::new();
    let (_nodes, edges) = parser.parse_file_in_repo(Some(&repo_root), "src/App.tsx", source);
    let from_app = |kind: &str| {
        edges
            .iter()
            .filter(|edge| edge.kind == kind && edge.source == "src/App.tsx::App")
            .map(|edge| (edge.line, edge.target.as_str(), &edge.extra))
            .collect::<Vec<_>>()
    };
    let calls = from_app("CALLS");
    let call_at = |line: i64| {
        calls
            .iter()
            .filter(|(at, _, _)| *at == line)
            .map(|(_, target, extra)| (*target, *extra))
            .collect::<Vec<_>>()
    };
    let external = |line: i64, target: &str, package: &str| {
        let found = call_at(line);
        let (_, extra) = found
            .iter()
            .find(|(written, _)| *written == target)
            .unwrap_or_else(|| panic!("line {line}: {target} not in {found:#?}"));
        assert_eq!(extra["external"], true, "{target}");
        assert_eq!(extra["external_package"], package, "{target}");
    };
    external(22, "react::useState", "react");
    // A default import's members are the module's (CommonJS interop).
    external(23, "react::useEffect", "react");
    external(24, "node:fs::readFile", "node:fs");
    // The target keeps the specifier as written; the package drops the subpath.
    external(25, "lodash/fp::map", "lodash");
    external(26, "zod::z.object", "zod");
    external(27, "lib-cjs::default", "lib-cjs");
    external(28, "lib-cjs::run", "lib-cjs");
    external(29, "lodash::pick", "lodash");
    external(33, "express::default", "express");
    external(36, "antd::Form.Item", "antd");
    external(36, "antd::Button", "antd");
    // In-repo and unresolvable in-repo specifiers are never external.
    for (line, target) in [(30, "src/lib/util.ts::util"), (31, "missing"), (32, "gone")] {
        let found = call_at(line);
        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(found[0].0, target);
        assert!(found[0].1.get("external").is_none(), "{found:#?}");
    }
    // A method on a value returned by an external call has no evidence of
    // its type: the bare name stays, marked as an unknown receiver.
    let get = call_at(35);
    assert_eq!(get.len(), 1, "{get:#?}");
    assert_eq!(get[0].0, "get");
    assert_eq!(get[0].1["receiver_unknown"], true);
    assert!(get[0].1.get("external").is_none());

    let references = from_app("REFERENCES");
    let reference = |target: &str| {
        references
            .iter()
            .find(|(_, written, _)| *written == target)
            .unwrap_or_else(|| panic!("{target} not in {references:#?}"))
            .2
    };
    assert_eq!(reference("cors::default")["external_package"], "cors");
    assert!(reference("src/helper.ts::helper").get("external").is_none());
    let decorator = edges
        .iter()
        .find(|edge| edge.kind == "REFERENCES" && edge.source == "src/App.tsx::Svc")
        .expect("decorator reference");
    assert_eq!(decorator.target, "@nestjs/common::Injectable");
    assert_eq!(decorator.extra["relationship_role"], "decorator");
    assert_eq!(decorator.extra["external_package"], "@nestjs/common");
    // External types stay out of the graph (no `react::FC` edge).
    assert!(
        edges.iter().all(|edge| !edge.target.ends_with("FC")),
        "{edges:#?}"
    );
    // A local shadowing an imported name is not the import.
    assert!(
        edges
            .iter()
            .all(|edge| !(edge.source.ends_with("::shadow") && edge.kind == "CALLS")),
        "{edges:#?}"
    );

    let test_source = br#"import { render } from "@testing-library/react";
import { ClassComp } from "./ClassComp";
test("renders", () => {
  render(<ClassComp />);
});
"#;
    let (_nodes, edges) =
        parser.parse_file_in_repo(Some(&repo_root), "src/App.test.tsx", test_source);
    let render = edges
        .iter()
        .find(|edge| edge.kind == "CALLS" && edge.line == 4 && edge.target.ends_with("render"))
        .expect("render call");
    assert_eq!(render.target, "@testing-library/react::render");
    assert_eq!(render.extra["external_package"], "@testing-library/react");
    let tested = edges
        .iter()
        .filter(|edge| edge.kind == "TESTED_BY")
        .map(|edge| edge.source.as_str())
        .collect::<Vec<_>>();
    // An external package is never the code under test.
    assert_eq!(tested, ["src/ClassComp.tsx::ClassComp"]);

    let _ = std::fs::remove_dir_all(&repo_root);
}

#[test]
fn resolves_typescript_base_url_and_nearest_config_imports() {
    let mut repo_root = std::env::temp_dir();
    repo_root.push(format!(
        "dagayn-parser-ts-base-url-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&repo_root);
    for dir in [
        "shared",
        "packages/web/src/services",
        "packages/web/src/lib",
        "packages/api/src",
        "apps/vite/src/lib",
        "apps/legacy/src/utils",
    ] {
        std::fs::create_dir_all(repo_root.join(dir)).unwrap();
    }
    for (path, body) in [
        // Root config: `paths` relative to `baseUrl`.
        (
            "tsconfig.json",
            r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@shared/*": ["shared/*"] } } }"#,
        ),
        ("shared/log.ts", "export function log() {}\n"),
        // Nearest config: `baseUrl` only, plus a `paths` alias under it.
        (
            "packages/web/tsconfig.json",
            r#"{ "compilerOptions": { "baseUrl": "src", "paths": { "~/*": ["lib/*"] } } }"#,
        ),
        (
            "packages/web/src/services/user.ts",
            "export function getUser() {}\n",
        ),
        ("packages/web/src/lib/fmt.ts", "export function fmt() {}\n"),
        // Solution-style tsconfig.json: the aliases live in tsconfig.app.json.
        (
            "apps/vite/tsconfig.json",
            r#"{ "files": [], "references": [{ "path": "./tsconfig.app.json" }] }"#,
        ),
        (
            "apps/vite/tsconfig.app.json",
            r#"{ "compilerOptions": { "paths": { "@/*": ["./src/*"] } } }"#,
        ),
        ("apps/vite/src/lib/cn.ts", "export function cn() {}\n"),
        // JavaScript project: jsconfig.json.
        (
            "apps/legacy/jsconfig.json",
            r#"{ "compilerOptions": { "baseUrl": "src" } }"#,
        ),
        (
            "apps/legacy/src/utils/date.js",
            "export function day() {}\n",
        ),
    ] {
        std::fs::write(repo_root.join(path), body).unwrap();
    }
    let mut parser = RustOwnedParser::new();
    let mut check = |file: &str, source: &str, expected: &[(&str, &str)]| {
        std::fs::write(repo_root.join(file), source).unwrap();
        let (_nodes, edges) = parser.parse_file_in_repo(Some(&repo_root), file, source.as_bytes());
        for (imported, target) in expected {
            assert!(
                edges.iter().any(|edge| edge.kind == "IMPORTS_FROM"
                    && edge.source == file
                    && edge.target == *imported),
                "{file}: IMPORTS_FROM {imported}: {edges:?}"
            );
            assert!(
                edges
                    .iter()
                    .any(|edge| edge.kind == "CALLS" && edge.target == *target),
                "{file}: CALLS {target}: {edges:?}"
            );
        }
        edges
    };
    let edges = check(
        "packages/web/src/app.ts",
        r#"import { getUser } from "services/user";
import { fmt } from "~/fmt";
import { useState } from "react";
export function app() { getUser(); fmt(); useState(); }
"#,
        &[
            (
                "packages/web/src/services/user.ts",
                "packages/web/src/services/user.ts::getUser",
            ),
            (
                "packages/web/src/lib/fmt.ts",
                "packages/web/src/lib/fmt.ts::fmt",
            ),
        ],
    );
    // A package name with no file under `baseUrl` stays external.
    assert!(
        edges
            .iter()
            .any(|edge| edge.kind == "CALLS" && edge.target == "react::useState")
    );
    // Without its own tsconfig, a package uses the root one.
    check(
        "packages/api/src/server.ts",
        "import { log } from \"@shared/log\";\nexport function serve() { log(); }\n",
        &[("shared/log.ts", "shared/log.ts::log")],
    );
    check(
        "apps/vite/src/main.ts",
        "import { cn } from \"@/lib/cn\";\nexport function main() { cn(); }\n",
        &[("apps/vite/src/lib/cn.ts", "apps/vite/src/lib/cn.ts::cn")],
    );
    check(
        "apps/legacy/src/index.js",
        "import { day } from \"utils/date\";\nexport function run() { day(); }\n",
        &[(
            "apps/legacy/src/utils/date.js",
            "apps/legacy/src/utils/date.js::day",
        )],
    );

    let _ = std::fs::remove_dir_all(&repo_root);
}
