use super::*;

#[test]
fn parses_swift_types_functions_calls_and_bridges() {
    let source = br#"import Foundation

struct User {
    let name: String
}

class Repo {
    func save(_ user: User) {
        print(user.name)
    }
}

func runProcess() {
    let p = Process.run(URL(fileURLWithPath: "/usr/bin/git"), arguments: ["status"])
    _ = p
}

func loadLib() {
    dlopen("mylib.dylib", RTLD_NOW)
}
"#;
    let (nodes, edges) = parse_swift("App.swift", source);

    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "User"
            && node.extra["type_role"] == "struct"
            && node.extra["container_role"] == "data_container"
            && node.extra["value_semantics"] == true
    }));
    assert!(
        nodes
            .iter()
            .any(|node| node.kind == "Function" && node.name == "save")
    );
    assert!(
        edges
            .iter()
            .any(|edge| edge.kind == "IMPORTS_FROM" && edge.target == "Foundation")
    );
    assert!(edges.iter().any(|edge| edge.kind == "CALLS"
        && edge.target == "Swift"
        && edge.extra["external_symbol"] == "print"));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.extra["evidence_source"] == "Process.run"
            && edge.extra["confidence_tier"] == "LOW"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "mylib.dylib"
            && edge.extra["evidence_source"] == "dlopen"
    }));
}

#[test]
fn swift_standard_library_calls_target_their_package() {
    let source = br#"import Foundation
import Alamofire

struct Logger {
    func log(_ message: String) {}
}

func max(_ a: Int, _ b: Int) -> Int { a }

func run(values: [Int]) {
    let now = Date()
    Swift.print(now)
    print(String(values.count))
    let biggest = max(1, 2)
    min(1, 2)
    FileManager.default.fileExists(atPath: "p")
    Logger().log("x")
    AF.request("https://example.com")
}
"#;
    let (_nodes, edges) = parse_swift("Sources/App/Run.swift", source);
    let find = |kind: &str, symbol: &str| {
        edges
            .iter()
            .find(|edge| {
                edge.kind == kind
                    && (edge.extra["external_symbol"] == symbol
                        || (edge.target == symbol && edge.extra.get("external_symbol").is_none()))
            })
            .unwrap_or_else(|| panic!("no {kind} {symbol} in {edges:#?}"))
    };
    let foundation = find("IMPORTS_FROM", "Foundation");
    assert_eq!(foundation.extra["stdlib"], true);
    assert_eq!(foundation.extra["confidence_tier"], "HIGH");
    let alamofire = find("IMPORTS_FROM", "Alamofire");
    assert!(alamofire.extra.get("stdlib").is_none(), "{alamofire:?}");
    // Qualified by the module: certain.
    let print = find("CALLS", "Swift.print");
    assert_eq!(print.target, "Swift");
    assert_eq!(print.extra["confidence_tier"], "HIGH");
    // Standard-library and imported framework names: likely.
    for (symbol, module) in [
        ("print", "Swift"),
        ("String", "Swift"),
        ("min", "Swift"),
        ("Date", "Foundation"),
        ("FileManager.default.fileExists", "Foundation"),
    ] {
        let call = find("CALLS", symbol);
        assert_eq!(call.target, module, "{symbol}");
        assert_eq!(call.extra["confidence_tier"], "MEDIUM", "{symbol}");
    }
    // The file's own `max` and `Logger`, and a dependency's API, are not
    // the standard library's.
    for target in [
        "Sources/App/Run.swift::max",
        "Sources/App/Run.swift::Logger",
        "request",
    ] {
        let call = find("CALLS", target);
        assert!(call.extra.get("stdlib").is_none(), "{call:?}");
    }
    assert!(
        edges
            .iter()
            .all(|edge| edge.extra.get("swift_call_path").is_none()),
        "{edges:#?}"
    );
}

#[test]
fn swift_receivers_record_the_call_they_came_from() {
    let source = br#"class Store {
    func save() {}
}
class Service {
    let repo: Repo
    func open(_ p: String) throws -> Store? { nil }
    func run(cache: Cache) async throws {
        let s = try open("a")
        s.save()
        try open("b").save()
        open("c")?.save()
        let local = Store()
        local.save()
        cache.flush()
        repo.find()
        self.repo.find()
        let z = await fetch()
        z.go()
        Repo.shared.find()
    }
}
"#;
    let (nodes, edges) = parse_swift("Sources/App/Service.swift", source);
    let call = |target: &str, line: i64| {
        edges
            .iter()
            .find(|edge| edge.kind == "CALLS" && edge.target == target && edge.line == line)
            .unwrap_or_else(|| panic!("no {target} at {line} in {edges:#?}"))
    };
    let open = nodes.iter().find(|node| node.name == "open").expect("open");
    assert_eq!(open.return_type.as_deref(), Some("Store?"));
    assert_eq!(
        call("save", 9).extra["receiver_from"],
        serde_json::json!({"call": "open", "line": 8, "unwrap": true})
    );
    assert_eq!(call("save", 9).extra["receiver_unknown"], true);
    assert_eq!(
        call("save", 10).extra["receiver_from"],
        serde_json::json!({"call": "open", "line": 10, "unwrap": true})
    );
    assert_eq!(
        call("save", 11).extra["receiver_from"],
        serde_json::json!({"call": "open", "line": 11, "unwrap": true})
    );
    // A type of this file declaring the method: the method itself.
    assert_eq!(
        call("Sources/App/Service.swift::Store.save", 13).extra,
        serde_json::json!({})
    );
    // Types of other files: a parameter, a stored property, `self.property`.
    assert_eq!(call("flush", 14).extra["receiver_type"], "Cache");
    assert_eq!(call("find", 15).extra["receiver_type"], "Repo");
    assert_eq!(call("find", 16).extra["receiver_type"], "Repo");
    assert_eq!(
        call("go", 18).extra["receiver_from"],
        serde_json::json!({"call": "fetch", "line": 17, "unwrap": false})
    );
    // A member of a type is no unknown receiver.
    assert_eq!(call("find", 19).extra.get("receiver_unknown"), None);
}
