use super::*;

#[test]
fn parses_typescript_items_calls_tests_and_references() {
    let source = br#"import { Thing } from './thing';

interface Shape {
  id: string;
}

type UserPayload = {
  id: string;
  name: string;
};

enum UserStatus {
  active,
  disabled,
}

@Entity()
class UserModel {
  id!: string;
  name!: string;
}

class CardProps {
  title!: string;
  count?: number;
}

class UpdateUserDto {
  id!: string;
}

class Service extends Base {
  run(input: string): void {
    helper(input);
  }
}

function helper(value: string): void {
  console.log(value);
}

const indirect = { helper };
const callbacks = [helper];

describe('Service', () => {
  it('runs', () => {
    helper('x');
  });
});
"#;
    let (nodes, edges) = parse_javascript_like("service.test.ts", source, "typescript");
    let node_names = nodes
        .iter()
        .map(|node| {
            (
                node.kind.as_str(),
                node.name.as_str(),
                node.parent_name.as_deref(),
            )
        })
        .collect::<Vec<_>>();
    assert!(node_names.contains(&("Class", "Shape", None)));
    assert!(node_names.contains(&("Type", "UserPayload", None)));
    assert!(node_names.contains(&("Class", "UserStatus", None)));
    assert!(node_names.contains(&("Class", "UserModel", None)));
    assert!(node_names.contains(&("Class", "CardProps", None)));
    assert!(node_names.contains(&("Class", "UpdateUserDto", None)));
    assert!(node_names.contains(&("Class", "Service", None)));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "Shape"
            && node.extra["type_role"] == "interface"
            && node.extra["is_contract"] == true
    }));
    for name in [
        "UserPayload",
        "UserStatus",
        "UserModel",
        "CardProps",
        "UpdateUserDto",
    ] {
        assert!(nodes.iter().any(|node| {
            matches!(node.kind.as_str(), "Class" | "Type")
                && node.name == name
                && node.extra["container_role"] == "data_container"
                && node.extra["value_semantics"] == true
        }));
    }
    assert!(nodes.iter().any(|node| {
        node.kind == "Type"
            && node.name == "UserPayload"
            && node.extra["type_role"] == "alias"
            && node.extra["alias_form"] == "object"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class" && node.name == "UserStatus" && node.extra["type_role"] == "enum"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "Service"
            && node.extra["type_role"] == "class"
            && node.extra.get("container_role").is_none()
    }));
    assert!(
        node_names
            .iter()
            .any(|(_, name, parent)| *name == "run" && *parent == Some("Service"))
    );
    assert!(
        node_names
            .iter()
            .any(|(_, name, parent)| *name == "helper" && parent.is_none())
    );
    assert!(
        node_names
            .iter()
            .any(|(kind, name, _)| *kind == "Test" && name.starts_with("it:runs@L"))
    );
    assert!(
        edges
            .iter()
            .any(|edge| edge.kind == "IMPORTS_FROM" && edge.target == "./thing")
    );
    assert!(edges.iter().any(|edge| {
        edge.kind == "INHERITS"
            && edge.source == "service.test.ts::Service"
            && edge.target == "Base"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "service.test.ts::Service.run"
            && edge.target == "service.test.ts::helper"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "REFERENCES"
            && edge.source == "service.test.ts"
            && edge.target == "service.test.ts::helper"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "TESTED_BY"
            && edge.source == "service.test.ts::helper"
            && edge.target.contains("it:runs")
    }));
}

#[test]
fn parses_typescript_abstract_classes() {
    let source = br#"export abstract class AbstractShape {
  abstract area(): number;
  abstract get label(): string;
  protected abstract readonly sides: number;
  describe(): string {
    return this.label + this.area();
  }
}

abstract class B {}

export default class DefaultShape extends AbstractShape {
  area(): number {
    return 1;
  }
  get label(): string {
    return "shape";
  }
}
"#;
    let (nodes, edges) = parse_javascript_like("shapes.ts", source, "typescript");
    for name in ["AbstractShape", "B"] {
        assert!(
            nodes.iter().any(|node| {
                node.kind == "Class"
                    && node.name == name
                    && node.parent_name.is_none()
                    && node.extra["type_role"] == "abstract_class"
                    && node.extra["is_abstract"] == true
            }),
            "{name}: {nodes:?}"
        );
    }
    for name in ["area", "label"] {
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == name
                && node.parent_name.as_deref() == Some("AbstractShape")
                && node.extra["is_abstract"] == true
        }));
    }
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "describe"
            && node.parent_name.as_deref() == Some("AbstractShape")
            && node.extra.get("is_abstract").is_none()
    }));
    assert!(
        !nodes
            .iter()
            .any(|node| node.name == "describe" && node.parent_name.is_none())
    );
    assert!(edges.iter().any(|edge| {
        edge.kind == "CONTAINS"
            && edge.source == "shapes.ts::AbstractShape"
            && edge.target == "shapes.ts::AbstractShape.area"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "shapes.ts::AbstractShape.describe"
            && edge.target == "shapes.ts::AbstractShape.area"
    }));
    assert!(!edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "shapes.ts::AbstractShape.describe"
            && edge.target == "shapes.ts::DefaultShape.area"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "INHERITS"
            && edge.source == "shapes.ts::DefaultShape"
            && edge.target == "AbstractShape"
    }));
}

#[test]
fn parses_typescript_declared_abstract_classes() {
    let source = br#"declare abstract class DeclaredAbstract {
  abstract m(): void;
}
export declare abstract class ExportedAbstract {
  abstract run(): void;
}
"#;
    let (nodes, edges) = parse_javascript_like("ambient.d.ts", source, "typescript");
    for (class_name, method) in [("DeclaredAbstract", "m"), ("ExportedAbstract", "run")] {
        assert!(nodes.iter().any(|node| {
            node.kind == "Class"
                && node.name == class_name
                && node.extra["type_role"] == "abstract_class"
                && node.extra["is_abstract"] == true
        }));
        assert!(nodes.iter().any(|node| {
            node.kind == "Function"
                && node.name == method
                && node.parent_name.as_deref() == Some(class_name)
                && node.extra["is_abstract"] == true
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CONTAINS"
                && edge.source == "ambient.d.ts"
                && edge.target == format!("ambient.d.ts::{class_name}")
        }));
    }
}

#[test]
fn parses_javascript_and_typescript_generator_declarations() {
    let source = br#"export function* gen() {
  yield helper();
}
export async function* agen() {
  yield* gen();
}
export const genExpr = function* () {
  helper();
};
class Items {
  *items() {
    helper();
  }
  async *aitems() {}
}
function helper() {}
"#;
    for (file, language) in [("gen.js", "javascript"), ("gen.ts", "typescript")] {
        let (nodes, edges) = parse_javascript_like(file, source, language);
        for name in ["gen", "agen", "genExpr", "helper"] {
            assert!(
                nodes.iter().any(|node| {
                    node.kind == "Function" && node.name == name && node.parent_name.is_none()
                }),
                "{file}: missing {name}: {nodes:?}"
            );
        }
        for name in ["items", "aitems"] {
            assert!(nodes.iter().any(|node| {
                node.kind == "Function"
                    && node.name == name
                    && node.parent_name.as_deref() == Some("Items")
            }));
        }
        for (caller, callee) in [
            ("gen", "helper"),
            ("agen", "gen"),
            ("genExpr", "helper"),
            ("Items.items", "helper"),
        ] {
            assert!(
                edges.iter().any(|edge| {
                    edge.kind == "CALLS"
                        && edge.source == format!("{file}::{caller}")
                        && edge.target == format!("{file}::{callee}")
                }),
                "{file}: missing CALLS {caller} -> {callee}: {edges:?}"
            );
        }
        assert!(
            !edges
                .iter()
                .any(|edge| edge.kind == "CALLS" && edge.source == file)
        );
    }
}

#[test]
fn parses_typescript_heritage_forms() {
    let mut repo_root = std::env::temp_dir();
    repo_root.push(format!(
        "dagayn-parser-ts-heritage-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&repo_root);
    std::fs::create_dir_all(repo_root.join("src/lib")).unwrap();
    std::fs::write(
        repo_root.join("src/lib/base.ts"),
        b"export class Base {}\nexport interface Marker {}\nexport interface X<T> {}\n",
    )
    .unwrap();

    let source = br#"import * as ns from "./lib/base";

function Mixin<T>(base: T): T { return base; }
class Local {}

class A extends ns.Base implements Service<string>, ns.Marker {}
class M extends Mixin(Local) {}
class G extends Array<number> {}
interface I2 extends Repo, Service<number>, ns.X<T> {}
"#;
    let mut parser = RustOwnedParser::new();
    let (_nodes, edges) = parser.parse_file_in_repo(Some(&repo_root), "src/heritage.ts", source);
    let has = |kind: &str, source: &str, target: &str| {
        edges
            .iter()
            .any(|edge| edge.kind == kind && edge.source == source && edge.target == target)
    };
    assert!(
        has("INHERITS", "src/heritage.ts::A", "src/lib/base.ts::Base"),
        "{edges:?}"
    );
    assert!(edges.iter().any(|edge| {
        edge.kind == "INHERITS"
            && edge.source == "src/heritage.ts::A"
            && edge.extra["heritage_expression"] == "ns.Base"
            && edge.extra["relationship_role"] == "extends"
    }));
    assert!(has("IMPLEMENTS", "src/heritage.ts::A", "Service"));
    assert!(has(
        "IMPLEMENTS",
        "src/heritage.ts::A",
        "src/lib/base.ts::Marker"
    ));
    assert!(!edges.iter().any(|edge| {
        edge.source == "src/heritage.ts::A" && (edge.target == "string" || edge.target == "ns")
    }));

    assert!(edges.iter().any(|edge| {
        edge.kind == "INHERITS"
            && edge.source == "src/heritage.ts::M"
            && edge.target == "Local"
            && edge.extra["heritage_expression"] == "Mixin(Local)"
    }));
    assert!(has("CALLS", "src/heritage.ts::M", "src/heritage.ts::Mixin"));
    assert!(!edges.iter().any(|edge| {
        edge.source == "src/heritage.ts" && matches!(edge.kind.as_str(), "CALLS" | "REFERENCES")
    }));

    assert!(has("INHERITS", "src/heritage.ts::G", "Array"));
    assert!(!has("INHERITS", "src/heritage.ts::G", "number"));

    for target in ["Repo", "Service", "src/lib/base.ts::X"] {
        assert!(
            edges.iter().any(|edge| {
                edge.kind == "INHERITS"
                    && edge.source == "src/heritage.ts::I2"
                    && edge.target == target
                    && edge.extra["relationship_role"] == "extends"
            }),
            "I2 -> {target}: {edges:?}"
        );
    }
    assert!(!edges.iter().any(|edge| {
        edge.source == "src/heritage.ts::I2" && (edge.target == "T" || edge.target == "number")
    }));

    let _ = std::fs::remove_dir_all(&repo_root);
}

#[test]
fn parses_javascript_class_extends() {
    let source = br#"import * as ns from "./base";

class Legacy extends Base {}
class Namespaced extends ns.Base {}
class Mixed extends Mixin(Base) {
  run() {
    class Inner extends Other {}
  }
}
"#;
    let (_nodes, edges) = parse_javascript_like("legacy.js", source, "javascript");
    let inherits = |source: &str, target: &str| {
        edges.iter().any(|edge| {
            edge.kind == "INHERITS"
                && edge.source == source
                && edge.target == target
                && edge.extra["relationship_role"] == "extends"
        })
    };
    assert!(inherits("legacy.js::Legacy", "Base"), "{edges:?}");
    assert!(inherits("legacy.js::Namespaced", "Base"));
    assert!(inherits("legacy.js::Mixed", "Base"));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS" && edge.source == "legacy.js::Mixed" && edge.target == "Mixin"
    }));
    assert!(!inherits("legacy.js::Mixed", "Other"));
}

#[test]
fn parses_typescript_default_exports_and_class_expressions() {
    fn find<'a>(
        nodes: &'a [ParsedNode],
        kind: &str,
        name: &str,
        parent: Option<&str>,
    ) -> Option<&'a ParsedNode> {
        nodes.iter().find(|node| {
            node.kind == kind && node.name == name && node.parent_name.as_deref() == parent
        })
    }
    for (file, language) in [("anon.ts", "typescript"), ("anon.js", "javascript")] {
        let (nodes, edges) = parse_javascript_like(
            file,
            b"export default class extends Base {\n  hello() { helper(); }\n}\nfunction helper() {}\n",
            language,
        );
        let class = find(&nodes, "Class", "default", None).expect("anonymous default class");
        assert_eq!(class.extra["export_default"], true);
        assert_eq!(class.extra["anonymous"], true);
        assert!(find(&nodes, "Function", "hello", Some("default")).is_some());
        assert!(
            !nodes
                .iter()
                .any(|node| node.name == "hello" && node.parent_name.is_none())
        );
        let qn = |name: &str| format!("{file}::{name}");
        assert!(edges.iter().any(|edge| {
            edge.kind == "CONTAINS" && edge.source == file && edge.target == qn("default")
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "INHERITS" && edge.source == qn("default") && edge.target == "Base"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == qn("default.hello")
                && edge.target == qn("helper")
        }));

        let (nodes, edges) = parse_javascript_like(
            file,
            b"export default function () {\n  helper();\n}\nfunction helper() {}\n",
            language,
        );
        let function = find(&nodes, "Function", "default", None).expect("anonymous default fn");
        assert_eq!(function.extra["export_default"], true);
        assert_eq!(function.extra["anonymous"], true);
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS" && edge.source == qn("default") && edge.target == qn("helper")
        }));
        assert!(
            !edges
                .iter()
                .any(|edge| edge.kind == "CALLS" && edge.source == file)
        );

        let (nodes, _edges) =
            parse_javascript_like(file, b"export default (x) => x * 2;\n", language);
        assert!(find(&nodes, "Function", "default", None).is_some());

        let (nodes, _edges) =
            parse_javascript_like(file, b"export default async function* () {}\n", language);
        assert!(find(&nodes, "Function", "default", None).is_some());

        let (nodes, _edges) = parse_javascript_like(
            file,
            b"export default function Page() {}\nexport function other() {}\n",
            language,
        );
        let page = find(&nodes, "Function", "Page", None).expect("named default");
        assert_eq!(page.extra["export_default"], true);
        assert!(page.extra.get("anonymous").is_none());
        let other = find(&nodes, "Function", "other", None).unwrap();
        assert!(other.extra.get("export_default").is_none());

        let (nodes, edges) = parse_javascript_like(
            file,
            br#"export const Anon = class {
  run() { this.stop(); }
  stop() {}
};
export const Named = class InnerName extends Base {
  go() {}
};
function make() {
  const anon = new Anon();
  anon.stop();
  return anon;
}
"#,
            language,
        );
        let anon = find(&nodes, "Class", "Anon", None).expect("bound class expression");
        assert_eq!(anon.extra["class_expression"], true);
        assert!(find(&nodes, "Function", "run", Some("Anon")).is_some());
        let named = find(&nodes, "Class", "Named", None).expect("named class expression");
        assert_eq!(named.extra["expression_name"], "InnerName");
        assert!(find(&nodes, "Function", "go", Some("Named")).is_some());
        assert!(!nodes.iter().any(|node| node.name == "InnerName"));
        assert!(
            !nodes
                .iter()
                .any(|node| node.name == "run" && node.parent_name.is_none())
        );
        assert!(edges.iter().any(|edge| {
            edge.kind == "INHERITS" && edge.source == qn("Named") && edge.target == "Base"
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS" && edge.source == qn("Anon.run") && edge.target == qn("Anon.stop")
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS" && edge.source == qn("make") && edge.target == qn("Anon")
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS" && edge.source == qn("make") && edge.target == qn("Anon.stop")
        }));
    }
}

#[test]
fn does_not_flatten_members_of_unbound_class_expressions() {
    let source = br#"export function Mixin(Base) {
  return class extends Base {
    mixed() { helper(); }
  };
}
function helper() {}
"#;
    let (nodes, edges) = parse_javascript_like("mixin.ts", source, "typescript");
    assert!(!nodes.iter().any(|node| node.name == "mixed"), "{nodes:?}");
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "mixin.ts::Mixin"
            && edge.target == "mixin.ts::helper"
    }));
}

#[test]
fn parses_typescript_object_literal_containers() {
    let source = br#"export const api = {
  get(id: string) { return this.put() + fetchIt(id); },
  post: () => fetchIt("p"),
  put: function () { return 1; },
  "quoted": () => 2,
  nested: { deep() { fetchIt("d"); } },
  value: 3,
  fetchIt,
};
const cfg = { a: 1 } as const;
const routes = { list() { return fetchIt("l"); } } satisfies Routes;
function fetchIt(id: string) { return id; }
function useLocal() {
  const local = { m() { fetchIt("m"); } };
  register({ n() { fetchIt("n"); } });
  return local;
}
"#;
    for (file, language) in [("api.ts", "typescript"), ("api.js", "javascript")] {
        let source = if language == "javascript" {
            String::from_utf8_lossy(source)
                .replace("(id: string)", "(id)")
                .replace(" satisfies Routes", "")
                .replace(" as const", "")
                .into_bytes()
        } else {
            source.to_vec()
        };
        let (nodes, edges) = parse_javascript_like(file, &source, language);
        let has_node = |kind: &str, name: &str, parent: Option<&str>| {
            nodes.iter().any(|node| {
                node.kind == kind && node.name == name && node.parent_name.as_deref() == parent
            })
        };
        let qn = |name: &str| format!("{file}::{name}");
        assert!(
            nodes.iter().any(|node| {
                node.kind == "Class" && node.name == "api" && node.extra["type_role"] == "object"
            }),
            "{file}: {nodes:?}"
        );
        for member in ["get", "post", "put", "quoted"] {
            assert!(
                has_node("Function", member, Some("api")),
                "{file}: api.{member}"
            );
        }
        assert!(has_node("Class", "nested", Some("api")));
        assert!(has_node("Function", "deep", Some("api.nested")));
        assert!(has_node("Class", "routes", None));
        assert!(has_node("Function", "list", Some("routes")));
        assert!(
            !nodes
                .iter()
                .any(|node| node.name == "cfg" || node.name == "value")
        );
        for name in ["get", "deep", "m", "n", "list"] {
            assert!(
                !has_node("Function", name, None),
                "{file}: {name} must not be top-level"
            );
        }
        assert!(
            !nodes
                .iter()
                .any(|node| node.name == "m" || node.name == "n")
        );
        for (source, target) in [
            (file.to_string(), qn("api")),
            (qn("api"), qn("api.get")),
            (qn("api"), qn("api.nested")),
            (qn("api.nested"), qn("api.nested.deep")),
        ] {
            assert!(
                edges.iter().any(|edge| {
                    edge.kind == "CONTAINS" && edge.source == source && edge.target == target
                }),
                "{file}: CONTAINS {source} -> {target}"
            );
        }
        for caller in [
            "api.get",
            "api.post",
            "api.nested.deep",
            "routes.list",
            "useLocal",
        ] {
            assert!(
                edges.iter().any(|edge| {
                    edge.kind == "CALLS"
                        && edge.source == qn(caller)
                        && edge.target == qn("fetchIt")
                }),
                "{file}: CALLS {caller} -> fetchIt: {edges:?}"
            );
        }
        assert!(edges.iter().any(|edge| {
            edge.kind == "REFERENCES" && edge.source == file && edge.target == qn("fetchIt")
        }));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CALLS" && edge.source == qn("api.get") && edge.target == qn("api.put")
        }));
    }
}

#[test]
fn parses_typescript_constructors_reexports_and_interface_methods() {
    let source = br#"
export { Repo } from "./other";

interface Repo {
  find(): void;
}

class Store implements Repo {
  find(): void {}
}

function make(): Repo {
  const store = new Store();
  store.find();
  return store;
}
"#;
    let (nodes, edges) = parse_javascript_like("service.ts", source, "typescript");
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "find"
            && node.parent_name.as_deref() == Some("Repo")
            && node.extra["is_abstract"] == true
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM" && edge.source == "service.ts" && edge.target == "./other"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPLEMENTS" && edge.source == "service.ts::Store" && edge.target == "Repo"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "service.ts::make"
            && edge.target == "service.ts::Store"
    }));
    assert!(
        edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "service.ts::make"
                && edge.target == "service.ts::Store.find"
        }),
        "{edges:?}"
    );
    assert!(!edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "service.ts::make"
            && edge.target == "service.ts::Repo.find"
    }));
    assert!(!edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "service.ts::make"
            && edge.target == "service.ts::find"
    }));
}

#[test]
fn parses_typescript_nested_object_containers() {
    let source = br#"export const api = {
  a: {
    b: {
      c() { return helper(); },
      d: () => this_is_not_bound(),
    },
    e() { return 1; },
  },
  top() { return api.a.b.c(); },
  data: { plain: 1, deeper: { value: 2 } },
};
function helper() { return 0; }
export function use() { api.a.b.c(); api.a.e(); }
"#;
    for (file, language) in [("api.ts", "typescript"), ("api.js", "javascript")] {
        let (nodes, edges) = parse_javascript_like(file, source, language);
        let has_node = |kind: &str, name: &str, parent: Option<&str>| {
            nodes.iter().any(|node| {
                node.kind == kind && node.name == name && node.parent_name.as_deref() == parent
            })
        };
        let qn = |name: &str| format!("{file}::{name}");
        assert!(has_node("Class", "api", None), "{file}: {nodes:?}");
        assert!(has_node("Class", "a", Some("api")), "{file}");
        assert!(has_node("Class", "b", Some("api.a")), "{file}");
        assert!(has_node("Function", "c", Some("api.a.b")), "{file}");
        assert!(has_node("Function", "d", Some("api.a.b")), "{file}");
        assert!(has_node("Function", "e", Some("api.a")), "{file}");
        // Objects without function-valued members anywhere below are data.
        assert!(
            !nodes
                .iter()
                .any(|node| matches!(node.name.as_str(), "data" | "deeper" | "plain")),
            "{file}"
        );
        for (source, target) in [
            (qn("api"), qn("api.a")),
            (qn("api.a"), qn("api.a.b")),
            (qn("api.a.b"), qn("api.a.b.c")),
            (qn("api.a"), qn("api.a.e")),
        ] {
            assert!(
                edges.iter().any(|edge| {
                    edge.kind == "CONTAINS" && edge.source == source && edge.target == target
                }),
                "{file}: CONTAINS {source} -> {target}"
            );
        }
        for (source, target) in [
            (qn("use"), qn("api.a.b.c")),
            (qn("use"), qn("api.a.e")),
            (qn("api.top"), qn("api.a.b.c")),
            (qn("api.a.b.c"), qn("helper")),
        ] {
            assert!(
                edges.iter().any(|edge| {
                    edge.kind == "CALLS" && edge.source == source && edge.target == target
                }),
                "{file}: CALLS {source} -> {target}: {edges:?}"
            );
        }
    }
}

#[test]
fn parses_typescript_namespaces_and_ambient_modules() {
    let source = br#"export namespace Outer {
  export const x = 1;
  export function helper(): number { return x; }
  export class Inner {
    run() { helper(); }
  }
  export namespace Deep {
    export function deepFn() {}
  }
  export const api = { get() { return helper(); } };
}
namespace Outer {
  export function more() { return helper(); }
}
namespace A.B.C {
  export function abc() {}
}
module Legacy {
  export function old() {}
}
declare module "external-lib" {
  export function ext(): void;
  export interface ExtOptions { a: number }
}
declare global {
  interface Window { myGlobal: string }
  function globalFn(): void;
}
declare namespace NS {
  function nsFn(): void;
}
declare function declaredFn(a: number): string;
declare class DeclaredClass { method(): void; }
export function useNs() {
  Outer.helper();
  const inner = new Outer.Inner();
  inner.run();
  A.B.C.abc();
  Outer.Deep.deepFn();
  Outer.api.get();
}
"#;
    let file = "ns.ts";
    let (nodes, edges) = parse_javascript_like(file, source, "typescript");
    let find = |kind: &str, name: &str, parent: Option<&str>| {
        nodes.iter().find(|node| {
            node.kind == kind && node.name == name && node.parent_name.as_deref() == parent
        })
    };
    let qn = |name: &str| format!("{file}::{name}");
    for (name, parent, role) in [
        ("Outer", None, "namespace"),
        ("Deep", Some("Outer"), "namespace"),
        ("A", None, "namespace"),
        ("B", Some("A"), "namespace"),
        ("C", Some("A.B"), "namespace"),
        ("Legacy", None, "namespace"),
        ("external-lib", None, "ambient_module"),
        ("global", None, "ambient_module"),
        ("NS", None, "namespace"),
    ] {
        let node = find("Class", name, parent).unwrap_or_else(|| panic!("{name}: {nodes:?}"));
        assert_eq!(node.extra["type_role"], role, "{name}");
    }
    // One QN, one node: `namespace Outer` is declared twice.
    assert_eq!(
        nodes
            .iter()
            .filter(|node| node.kind == "Class" && node.name == "Outer")
            .count(),
        1
    );
    for (name, parent) in [
        ("helper", Some("Outer")),
        ("more", Some("Outer")),
        ("deepFn", Some("Outer.Deep")),
        ("abc", Some("A.B.C")),
        ("old", Some("Legacy")),
        ("ext", Some("external-lib")),
        ("globalFn", Some("global")),
        ("nsFn", Some("NS")),
        ("run", Some("Outer.Inner")),
        ("get", Some("Outer.api")),
    ] {
        assert!(
            find("Function", name, parent).is_some(),
            "{name}: {nodes:?}"
        );
        assert!(
            find("Function", name, None).is_none(),
            "{name} is not top-level"
        );
    }
    for (name, parent) in [
        ("Inner", Some("Outer")),
        ("ExtOptions", Some("external-lib")),
        ("Window", Some("global")),
        ("api", Some("Outer")),
    ] {
        assert!(find("Class", name, parent).is_some(), "{name}: {nodes:?}");
    }
    for name in ["ext", "globalFn", "nsFn"] {
        let node = nodes.iter().find(|node| node.name == name).unwrap();
        assert_eq!(node.extra["ambient"], true, "{name}");
    }
    for name in ["external-lib", "global", "NS", "DeclaredClass"] {
        let node = nodes.iter().find(|node| node.name == name).unwrap();
        assert_eq!(node.extra["ambient"], true, "{name}");
    }
    assert!(
        find("Class", "Outer", None)
            .unwrap()
            .extra
            .get("ambient")
            .is_none()
    );
    let declared = find("Function", "declaredFn", None).unwrap();
    assert_eq!(declared.extra["ambient"], true);
    assert_eq!(declared.extra["declaration_only"], true);
    assert!(declared.extra.get("is_abstract").is_none(), "{declared:?}");
    let method = find("Function", "method", Some("DeclaredClass")).unwrap();
    assert!(method.extra.get("is_abstract").is_none(), "{method:?}");
    assert_eq!(method.extra["declaration_only"], true);
    for (source, target) in [
        (qn("Outer"), qn("Outer.helper")),
        (qn("Outer"), qn("Outer.Deep")),
        (qn("Outer.Deep"), qn("Outer.Deep.deepFn")),
        (qn("Outer"), qn("Outer.Inner")),
        (qn("Outer.Inner"), qn("Outer.Inner.run")),
        (qn("A"), qn("A.B")),
        (qn("A.B"), qn("A.B.C")),
        (qn("A.B.C"), qn("A.B.C.abc")),
        (file.to_string(), qn("Outer")),
        (qn("global"), qn("global.Window")),
    ] {
        assert!(
            edges.iter().any(|edge| {
                edge.kind == "CONTAINS" && edge.source == source && edge.target == target
            }),
            "CONTAINS {source} -> {target}"
        );
    }
    for (source, target) in [
        (qn("Outer.Inner.run"), qn("Outer.helper")),
        (qn("Outer.more"), qn("Outer.helper")),
        (qn("Outer.api.get"), qn("Outer.helper")),
        (qn("useNs"), qn("Outer.helper")),
        (qn("useNs"), qn("Outer.Inner")),
        (qn("useNs"), qn("Outer.Inner.run")),
        (qn("useNs"), qn("A.B.C.abc")),
        (qn("useNs"), qn("Outer.Deep.deepFn")),
        (qn("useNs"), qn("Outer.api.get")),
    ] {
        assert!(
            edges.iter().any(|edge| {
                edge.kind == "CALLS" && edge.source == source && edge.target == target
            }),
            "CALLS {source} -> {target}: {edges:?}"
        );
    }
}

#[test]
fn marks_typescript_declaration_files() {
    let source = br#"declare function declaredFn(a: number): string;
export interface Exported { e: 1 }
export declare function exportedDeclared(): void;
export as namespace MyLib;
"#;
    let (nodes, _) = parse_javascript_like("types/lib.d.ts", source, "typescript");
    let file = nodes.iter().find(|node| node.kind == "File").unwrap();
    assert_eq!(file.extra["declaration_file"], true);
    assert_eq!(file.extra["umd_global"], "MyLib");
    for name in ["declaredFn", "Exported", "exportedDeclared"] {
        let node = nodes.iter().find(|node| node.name == name).unwrap();
        assert_eq!(node.extra["ambient"], true, "{name}");
    }
    let (nodes, _) = parse_javascript_like("src/lib.ts", source, "typescript");
    let file = nodes.iter().find(|node| node.kind == "File").unwrap();
    assert!(file.extra.get("declaration_file").is_none());
    let exported = nodes.iter().find(|node| node.name == "Exported").unwrap();
    assert!(exported.extra.get("ambient").is_none());
}

#[test]
fn parses_cjs_mts_cts_and_declaration_variants() {
    for (path, language) in [
        ("conf.cjs", "javascript"),
        ("CONF.CJS", "javascript"),
        ("util.mts", "typescript"),
        ("legacy.cts", "typescript"),
        ("types.d.mts", "typescript"),
        ("types.d.cts", "typescript"),
    ] {
        assert_eq!(detect_language(Path::new(path)), Some(language), "{path}");
        assert!(rust_parser_owns_path(path), "{path}");
    }

    let mut repo_root = std::env::temp_dir();
    repo_root.push(format!(
        "dagayn-parser-cjs-mts-cts-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&repo_root);
    std::fs::create_dir_all(repo_root.join("src")).unwrap();
    let files = [
        (
            "src/conf.cjs",
            "function helper() {}\nmodule.exports = { helper };\n",
        ),
        (
            "src/util.mts",
            "export function utilFn(x: number): number { return x; }\n",
        ),
        (
            "src/legacy.cts",
            "import conf = require(\"./conf.cjs\");\nexport function legacy(): void { conf.helper(); }\n",
        ),
        (
            "src/types.d.mts",
            "export declare function declaredM(a: number): string;\n",
        ),
        (
            "src/types.d.cts",
            "export declare function declaredC(a: number): string;\n",
        ),
        (
            "src/app.mts",
            "import { utilFn } from \"./util.mjs\";\nimport { legacy } from \"./legacy.cjs\";\nexport function main(): void {\n  utilFn(1);\n  legacy();\n}\n",
        ),
    ];
    for (path, body) in files {
        std::fs::write(repo_root.join(path), body).unwrap();
    }
    let mut collected = collect_parseable_files(&repo_root, None);
    collected.sort();
    let mut expected = files
        .iter()
        .map(|(path, _)| path.to_string())
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(collected, expected);

    let mut parser = RustOwnedParser::new();
    let mut parse = |path: &str| {
        let source = std::fs::read(repo_root.join(path)).unwrap();
        parser.parse_file_in_repo(Some(&repo_root), path, &source)
    };
    for (path, name, language) in [
        ("src/conf.cjs", "helper", "javascript"),
        ("src/util.mts", "utilFn", "typescript"),
        ("src/legacy.cts", "legacy", "typescript"),
        ("src/types.d.mts", "declaredM", "typescript"),
        ("src/types.d.cts", "declaredC", "typescript"),
    ] {
        let (nodes, _) = parse(path);
        let node = nodes
            .iter()
            .find(|node| node.kind == "Function" && node.name == name)
            .unwrap_or_else(|| panic!("{path}: {nodes:?}"));
        assert_eq!(node.language, language, "{path}");
        let file = nodes.iter().find(|node| node.kind == "File").unwrap();
        let declaration = path.contains(".d.");
        assert_eq!(
            file.extra.get("declaration_file").is_some(),
            declaration,
            "{path}"
        );
        assert_eq!(
            node.extra.get("ambient").is_some(),
            declaration,
            "{path}: {node:?}"
        );
    }

    let (_, edges) = parse("src/legacy.cts");
    assert!(
        edges
            .iter()
            .any(|edge| edge.kind == "IMPORTS_FROM" && edge.target == "src/conf.cjs")
    );
    assert!(edges.iter().any(|edge| edge.kind == "CALLS"
        && edge.source == "src/legacy.cts::legacy"
        && edge.target == "src/conf.cjs::helper"));

    let (_, edges) = parse("src/app.mts");
    let mut imports = edges
        .iter()
        .filter(|edge| edge.kind == "IMPORTS_FROM")
        .map(|edge| edge.target.as_str())
        .collect::<Vec<_>>();
    imports.sort_unstable();
    assert_eq!(imports, vec!["src/legacy.cts", "src/util.mts"], "{edges:?}");
    let mut calls = edges
        .iter()
        .filter(|edge| edge.kind == "CALLS" && edge.source == "src/app.mts::main")
        .map(|edge| edge.target.as_str())
        .collect::<Vec<_>>();
    calls.sort_unstable();
    assert_eq!(
        calls,
        vec!["src/legacy.cts::legacy", "src/util.mts::utilFn"],
        "{edges:?}"
    );

    let _ = std::fs::remove_dir_all(&repo_root);
}

#[test]
fn attributes_local_declarations_to_enclosing_function() {
    let source = br#"import { UserService } from "./user.service";
function log(value: unknown) { return value; }
function nested() { return 0; }
export function App() {
  const handle = () => log("click");
  function inner() { return log("inner"); }
  class Local { run() { log("local"); } }
  interface Shape { a: number }
  type Alias = { b: string };
  const obj = { m() { log("m"); } };
  items.map(x => log(x));
  handle();
  inner();
  new Local().run();
  return nested();
}
export function outer() {
  function nested() { return log("shadow"); }
  return nested();
}
export class Caller {
  private svc = new UserService();
  static registry = register(Caller);
  handler = () => log("handler");
  static { log("static"); }
  [Symbol.iterator]() { return log("iter"); }
  method() {
    const local = () => this.helper();
    local();
  }
  helper() {}
}
function register(value: unknown) { return value; }
"#;
    for (file, language) in [("app.ts", "typescript"), ("app.js", "javascript")] {
        let source = if language == "javascript" {
            String::from_utf8_lossy(source)
                .replace("(value: unknown)", "(value)")
                .replace("  interface Shape { a: number }\n", "")
                .replace("  type Alias = { b: string };\n", "")
                .replace("private svc", "svc")
                .into_bytes()
        } else {
            source.to_vec()
        };
        let (nodes, edges) = parse_javascript_like(file, &source, language);
        let qn = |name: &str| format!("{file}::{name}");
        for local in [
            "handle", "inner", "Local", "Shape", "Alias", "obj", "m", "x", "local", "run",
        ] {
            assert!(
                !nodes.iter().any(|node| node.name == local),
                "{file}: {local} is local: {nodes:?}"
            );
        }
        // Only the top-level `nested` exists; `outer`'s local one is not a node.
        assert_eq!(
            nodes.iter().filter(|node| node.name == "nested").count(),
            1,
            "{file}"
        );
        let calls = |source: &str, target: &str| {
            edges
                .iter()
                .any(|edge| edge.kind == "CALLS" && edge.source == source && edge.target == target)
        };
        for message in ["click", "inner", "local", "m"] {
            let _ = message;
        }
        assert!(calls(&qn("App"), &qn("log")), "{file}: {edges:?}");
        assert!(calls(&qn("App"), &qn("nested")), "{file}");
        assert!(calls(&qn("outer"), &qn("log")), "{file}");
        // Calls of local declarations are internal to the function.
        for local in ["handle", "inner", "Local", "nested"] {
            assert!(
                !edges.iter().any(|edge| edge.kind == "CALLS"
                    && edge.source == qn("outer")
                    && edge.target.ends_with(local)),
                "{file}: outer -> {local}"
            );
        }
        for local in ["handle", "inner", "Local"] {
            assert!(
                !edges.iter().any(|edge| edge.kind == "CALLS"
                    && edge.source == qn("App")
                    && (edge.target == local || edge.target == qn(local))),
                "{file}: App -> {local}: {edges:?}"
            );
        }
        // Class-level code is attributed to the class.
        assert!(
            calls(&qn("Caller"), "user.service::UserService")
                || edges.iter().any(|edge| edge.kind == "CALLS"
                    && edge.source == qn("Caller")
                    && edge.target.ends_with("UserService")),
            "{file}: {edges:?}"
        );
        assert!(calls(&qn("Caller"), &qn("register")), "{file}");
        assert!(calls(&qn("Caller"), &qn("log")), "{file}");
        assert!(
            calls(&qn("Caller.method"), &qn("Caller.helper")),
            "{file}: {edges:?}"
        );
        if language == "typescript" {
            assert!(calls(&qn("Caller.handler"), &qn("log")), "{file}");
        }
        assert!(
            !edges.iter().any(|edge| edge.kind == "CALLS"
                && edge.source == file
                && edge.target == qn("log")),
            "{file}: no file-sourced log calls: {edges:?}"
        );
    }
}

#[test]
fn parses_typescript_decorators_metadata() {
    let source = br#"import { Controller, Get, UseGuards, Injectable } from "@nestjs/common";
import * as ng from "@angular/core";
@sealed
@Injectable({ providedIn: "root", factory: makeFactory() })
export class Decorated {
  @Input() title = "";
  @Output() changed = makeEmitter();
  @HostListener("click", ["$event"])
  onClick(@Inject(TOKEN) e: Event) { track(); }
  @Get(":id") @UseGuards(AuthGuard) find() {}
  @Debounce(300) handler = () => track();
}
@ng.Component({ selector: "app-root" })
class NgRoot {}
function sealed(ctor: Function) {}
function makeFactory() { return 1; }
function makeEmitter() { return 2; }
function track() {}
function Debounce(ms: number) { return (target: unknown) => target; }
"#;
    let file = "dec.ts";
    let (nodes, edges) = parse_javascript_like(file, source, "typescript");
    let find = |name: &str, parent: Option<&str>| {
        nodes
            .iter()
            .find(|node| node.name == name && node.parent_name.as_deref() == parent)
            .unwrap_or_else(|| panic!("{name}: {nodes:?}"))
    };
    let qn = |name: &str| format!("{file}::{name}");
    let decorated = find("Decorated", None);
    assert_eq!(
        decorated.extra["decorators"],
        json!(["sealed", "Injectable"])
    );
    assert_eq!(
        decorated.extra["member_decorators"],
        json!(["Input", "Output"])
    );
    assert_eq!(
        find("onClick", Some("Decorated")).extra["decorators"],
        json!(["HostListener"])
    );
    assert_eq!(
        find("find", Some("Decorated")).extra["decorators"],
        json!(["Get", "UseGuards"])
    );
    assert_eq!(
        find("handler", Some("Decorated")).extra["decorators"],
        json!(["Debounce"])
    );
    assert_eq!(
        find("NgRoot", None).extra["decorators"],
        json!(["ng.Component"])
    );
    let references = |source: &str, target_suffix: &str| {
        edges.iter().any(|edge| {
            edge.kind == "REFERENCES"
                && edge.source == source
                && edge.target.ends_with(target_suffix)
                && edge.extra["relationship_role"] == "decorator"
        })
    };
    assert!(references(&qn("Decorated"), &qn("sealed")), "{edges:?}");
    assert!(references(&qn("Decorated"), "Injectable"));
    assert!(references(&qn("Decorated"), "Input"));
    assert!(references(&qn("Decorated.onClick"), "HostListener"));
    assert!(references(&qn("Decorated.onClick"), "Inject"));
    assert!(references(&qn("Decorated.find"), "Get"));
    assert!(references(&qn("Decorated.find"), "UseGuards"));
    assert!(references(&qn("Decorated.handler"), &qn("Debounce")));
    assert!(references(&qn("NgRoot"), "Component"));
    // Decorators are not CALLS; calls in their arguments belong to the
    // decorated node.
    for decorator in [
        "sealed",
        "Injectable",
        "Input",
        "HostListener",
        "Get",
        "UseGuards",
        "Inject",
        "Debounce",
        "Component",
    ] {
        assert!(
            !edges
                .iter()
                .any(|edge| edge.kind == "CALLS" && edge.target.ends_with(decorator)),
            "{decorator}: {edges:?}"
        );
    }
    let calls = |source: &str, target: &str| {
        edges
            .iter()
            .any(|edge| edge.kind == "CALLS" && edge.source == source && edge.target == target)
    };
    assert!(calls(&qn("Decorated"), &qn("makeFactory")), "{edges:?}");
    assert!(calls(&qn("Decorated"), &qn("makeEmitter")));
    assert!(calls(&qn("Decorated.onClick"), &qn("track")));
    assert!(calls(&qn("Decorated.handler"), &qn("track")));
    assert!(
        !edges
            .iter()
            .any(|edge| edge.source == file && edge.kind == "CALLS"),
        "{edges:?}"
    );
}

#[test]
fn parses_javascript_class_field_functions() {
    let source = br#"class Legacy {
  handle = () => { helper(); };
  other = function () { this.handle(); };
  static make = () => new Legacy();
  #secret = () => 1;
  plain = compute();
}
function helper() {}
function compute() {}
"#;
    for (file, language) in [("legacy.js", "javascript"), ("legacy.ts", "typescript")] {
        let (nodes, edges) = parse_javascript_like(file, source, language);
        for member in ["handle", "other", "make", "#secret"] {
            let found = nodes
                .iter()
                .find(|node| node.name == member && node.parent_name.as_deref() == Some("Legacy"))
                .unwrap_or_else(|| panic!("{file}: Legacy.{member}: {nodes:?}"));
            assert_eq!(found.kind, "Function");
        }
        assert!(!nodes.iter().any(|node| node.name == "plain"));
        let class = nodes.iter().find(|node| node.name == "Legacy").unwrap();
        // Function-valued fields are methods: not a property-only class.
        assert!(
            class.extra.get("container_role").is_none(),
            "{file}: {class:?}"
        );
        let qn = |name: &str| format!("{file}::{name}");
        let calls = |source: &str, target: &str| {
            edges
                .iter()
                .any(|edge| edge.kind == "CALLS" && edge.source == source && edge.target == target)
        };
        assert!(
            calls(&qn("Legacy.handle"), &qn("helper")),
            "{file}: {edges:?}"
        );
        assert!(calls(&qn("Legacy.other"), &qn("Legacy.handle")));
        assert!(calls(&qn("Legacy.make"), &qn("Legacy")));
        assert!(calls(&qn("Legacy"), &qn("compute")));
        assert!(!calls(&qn("Legacy"), &qn("helper")));
        assert!(edges.iter().any(|edge| {
            edge.kind == "CONTAINS"
                && edge.source == qn("Legacy")
                && edge.target == qn("Legacy.handle")
        }));
    }
}
