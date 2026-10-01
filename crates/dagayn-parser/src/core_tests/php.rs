use super::*;

#[test]
fn parses_php_types_calls_imports_and_bridges() {
    let source = br#"<?php
use Exception;

interface Repository {
    public function save(User $user): void;
}

class User {
    public function __construct(int $id) {}
    public function toString(): string { return "u"; }
}

class ExtendedRepo implements Repository {
    public function save(User $user): void {
        $user->toString();
        file_put_contents("output.json", "{}");
    }

    public function run($path): void {
        sqlQuery("SELECT 1");
        $this->save(new User(1));
        parent::__construct();
        FFI::cdef("", "mylib.so");
        file_get_contents($path);
        Broker::build(1);
    }
}

function sqlQuery(string $query): array { return []; }

class Broker {
    public static function build(int $id): User { return new User($id); }
}
"#;
    let (nodes, edges) = parse_php("sample.php", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "Repository"
            && node.extra["type_role"] == "interface"
            && node.extra["is_contract"] == true
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "save"
            && node.parent_name.as_deref() == Some("Repository")
            && node.params.as_deref() == Some("(User $user)")
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "sqlQuery"
            && node.parent_name.is_none()
            && node.params.as_deref() == Some("(string $query)")
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM"
            && edge.source == "sample.php"
            && edge.target == "php"
            && edge.extra["external_symbol"] == "Exception"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPLEMENTS"
            && edge.source == "sample.php::ExtendedRepo"
            && edge.target == "Repository"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.php::ExtendedRepo.save"
            && edge.target == "sample.php::User.toString"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.php::ExtendedRepo.run"
            && edge.target == "sample.php::sqlQuery"
    }));
    // A static call targets the method so it can bind to a node; the
    // `Class::method` form is kept only as bridge evidence.
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.php::ExtendedRepo.run"
            && edge.target == "sample.php::Broker.build"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "output.json"
            && edge.extra["evidence_source"] == "file_put_contents"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "<dynamic:FFI::cdef@sample.php:23>"
            && edge.extra["confidence_tier"] == "LOW"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "<dynamic:file_get_contents@sample.php:24>"
            && edge.extra["confidence_tier"] == "LOW"
    }));
}

fn php_calls(edges: &[ParsedEdge], kind: &str) -> Vec<(String, String, String)> {
    edges
        .iter()
        .filter(|edge| edge.kind == kind)
        .map(|edge| {
            (
                edge.target.clone(),
                edge.extra["external_symbol"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                edge.extra["confidence_tier"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

fn has(found: &[(String, String, String)], target: &str, symbol: &str, tier: &str) -> bool {
    found
        .iter()
        .any(|(t, s, c)| t == target && s == symbol && c == tier)
}

#[test]
fn php_standard_library_calls_target_their_package() {
    let source = br#"<?php
use DateTime as DT;
use App\Models\User;
use function App\Support\trim;

function helper(\DateTimeZone $zone) { return $zone->getName(); }
function count($items) { return 0; }

$a = strlen("a");
$b = \str_replace("a", "b", "c");
$d = new \DateTime();
$d->modify('+1 day');
$e = new DT();
$f = new Exception("x");
(new \ArrayObject([]))->getArrayCopy();
\DateTime::createFromFormat('Y', '2020');
$u = new User();
count([1]);
trim(" x ");
isset($a);
App\helper();
"#;
    let (_, edges) = parse_php("script.php", source);
    let imports = php_calls(&edges, "IMPORTS_FROM");
    assert!(has(&imports, "php", "DateTime", "HIGH"), "{imports:?}");
    assert!(has(&imports, "App\\Models\\User", "", ""), "{imports:?}");

    let calls = php_calls(&edges, "CALLS");
    // Fully qualified or imported: certain.
    assert!(has(&calls, "php", "str_replace", "HIGH"), "{calls:?}");
    assert!(has(&calls, "php", "DateTime", "HIGH"), "{calls:?}");
    assert!(
        has(&calls, "php", "ArrayObject::getArrayCopy", "HIGH"),
        "{calls:?}"
    );
    assert!(
        has(&calls, "php", "DateTime::createFromFormat", "HIGH"),
        "{calls:?}"
    );
    // A bare builtin name, a bare class in a file without a namespace, and
    // variables bound to builtin classes: likely.
    assert!(has(&calls, "php", "strlen", "MEDIUM"), "{calls:?}");
    assert!(has(&calls, "php", "Exception", "MEDIUM"), "{calls:?}");
    assert!(
        has(&calls, "php", "DateTime::modify", "MEDIUM"),
        "{calls:?}"
    );
    assert!(
        has(&calls, "php", "DateTimeZone::getName", "MEDIUM"),
        "{calls:?}"
    );
    // The file's own `count`, an imported `trim`, a language construct and
    // repository classes stay unmarked.
    assert!(has(&calls, "script.php::count", "", ""), "{calls:?}");
    assert!(has(&calls, "trim", "", ""), "{calls:?}");
    assert!(has(&calls, "isset", "", ""), "{calls:?}");
    assert!(has(&calls, "User", "", ""), "{calls:?}");
    let instantiation = edges
        .iter()
        .find(|edge| edge.extra["external_symbol"] == "Exception")
        .unwrap();
    assert_eq!(instantiation.extra["call_role"], "instantiation");
    assert_eq!(instantiation.extra["external"], true);
}

#[test]
fn php_namespaced_class_names_are_the_namespaces_own() {
    let source = br#"<?php
namespace App;
use InvalidArgumentException;

function run() {
    $a = new DateTime();
    $b = new \DateTime();
    $c = new InvalidArgumentException("x");
    strlen("a");
}
"#;
    let (_, edges) = parse_php("src/run.php", source);
    let calls = php_calls(&edges, "CALLS");
    // `DateTime` in `App` is `App\DateTime`.
    assert!(has(&calls, "DateTime", "", ""), "{calls:?}");
    assert!(has(&calls, "php", "DateTime", "HIGH"), "{calls:?}");
    assert!(
        has(&calls, "php", "InvalidArgumentException", "HIGH"),
        "{calls:?}"
    );
    // A function falls back to the global one.
    assert!(has(&calls, "php", "strlen", "MEDIUM"), "{calls:?}");
}

#[test]
fn php_receivers_record_the_call_they_came_from() {
    let source = br#"<?php
namespace App;
use App\Data\Repo;
class Svc {
    private Repo $repo;
    public function __construct(private Store $store) {}
    public function make(): ?Store { return null; }
    public function run(Repo $r, $u): void {
        $r->save();
        $this->repo->load();
        $this->store->flush();
        $x = new Repo(); $x->commit();
        $s = makeStore(); $s->close();
        $this->make()->open();
        $u->go();
        $l = new Local(); $l->ping();
        Repo::create();
        $this->helper();
        $u->with(1)->with(2)->done();
    }
    public function helper() {}
}
class Local { public function ping() {} }
function makeStore(): Store { return new Store(); }
"#;
    let (nodes, edges) = parse_php("src/Svc.php", source);
    let returns = |name: &str| {
        nodes
            .iter()
            .find(|node| node.name == name)
            .and_then(|node| node.return_type.clone())
    };
    assert_eq!(returns("make").as_deref(), Some("?Store"));
    assert_eq!(returns("makeStore").as_deref(), Some("Store"));
    assert_eq!(returns("helper"), None);
    let call = |target: &str, line: i64| {
        edges
            .iter()
            .find(|edge| edge.kind == "CALLS" && edge.target == target && edge.line == line)
            .map(|edge| edge.extra.clone())
            .unwrap_or_else(|| panic!("no CALLS {target} on line {line} in {edges:?}"))
    };
    // A typed parameter, typed and promoted properties, and `new` of a
    // class of another file.
    assert_eq!(call("save", 9)["receiver_type"], "Repo");
    assert_eq!(call("load", 10)["receiver_type"], "Repo");
    assert_eq!(call("flush", 11)["receiver_type"], "Store");
    assert_eq!(call("commit", 12)["receiver_type"], "Repo");
    // The result of a call, through a variable or directly.
    assert_eq!(
        call("close", 13),
        serde_json::json!({
            "receiver_unknown": true,
            "receiver_from": {"call": "makeStore", "line": 13, "unwrap": false},
        })
    );
    assert_eq!(
        call("open", 14)["receiver_from"],
        serde_json::json!({"call": "make", "line": 14, "unwrap": false})
    );
    assert_eq!(
        call("go", 15),
        serde_json::json!({"receiver_unknown": true})
    );
    // `$this`, a class of this file, and a static call keep the same-file
    // binding.
    assert_eq!(call("src/Svc.php::Local.ping", 16), serde_json::json!({}));
    assert_eq!(call("create", 17), serde_json::json!({}));
    assert_eq!(call("src/Svc.php::Svc.helper", 18), serde_json::json!({}));
    assert_eq!(call("src/Svc.php::Svc.make", 14), serde_json::json!({}));
    // A repeated method points past its repeats.
    assert_eq!(
        call("done", 19)["receiver_from"],
        serde_json::json!({"call": "with", "line": 19, "unwrap": false})
    );
    assert_eq!(
        call("with", 19),
        serde_json::json!({"receiver_unknown": true})
    );
}
