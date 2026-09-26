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
        edge.kind == "IMPORTS_FROM" && edge.source == "sample.php" && edge.target == "Exception"
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
