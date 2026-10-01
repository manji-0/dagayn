use super::*;

#[test]
fn parses_zig_containers_functions_tests_imports_and_calls() {
    let source = br#"const std = @import("std");

pub const Point = struct {
    x: i32,

    pub fn init(x: i32) Point {
        return .{ .x = x };
    }

    pub fn double(self: Point) i32 {
        return self.scale(2);
    }

    fn scale(self: Point, k: i32) i32 {
        return self.x * k;
    }
};

const Color = enum { red, green };

pub fn main() void {
    const p = Point.init(1);
    std.debug.print("{}\n", .{p.double()});
}

test "point doubles" {
    try std.testing.expect(Point.init(2).double() == 4);
}
"#;
    let (nodes, edges) = parse_zig("src/main.zig", source);

    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "Point"
            && node.language == "zig"
            && node.extra["type_role"] == "struct"
            && node.modifiers.as_deref() == Some("pub")
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class" && node.name == "Color" && node.extra["type_role"] == "enum"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "init"
            && node.parent_name.as_deref() == Some("Point")
            && node.return_type.as_deref() == Some("Point")
    }));
    assert!(
        nodes
            .iter()
            .any(|node| node.kind == "Test" && node.name == "point doubles")
    );
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM" && edge.source == "src/main.zig" && edge.target == "std"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CONTAINS"
            && edge.source == "src/main.zig::Point"
            && edge.target == "src/main.zig::Point.init"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "src/main.zig::Point.double"
            && edge.target == "src/main.zig::Point.scale"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "src/main.zig::main"
            && edge.target == "src/main.zig::Point.init"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "src/main.zig::main"
            && edge.target == "std"
            && edge.extra["external_symbol"] == "std.debug.print"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "TESTED_BY"
            && edge.source == "src/main.zig::Point.init"
            && edge.target == "src/main.zig::point doubles"
    }));
}

#[test]
fn records_zig_ffi_exports_imports_and_c_import_calls() {
    let source = br#"const c = @cImport({
    @cInclude("sum.h");
});
extern fn scale(x: f64) f64;
extern "fastsum" fn legacy(x: c_int) c_int;
export fn add(a: i32, b: i32) i32 {
    return a + b;
}
pub fn total(n: c_int) f64 {
    return c.fast_sum(n) + scale(1.0);
}
"#;
    let mut parser = RustOwnedParser::new();
    let (nodes, edges) = parser.parse_file("src/main.zig", source);
    let extra = |name: &str| {
        nodes
            .iter()
            .find(|node| node.name == name)
            .unwrap_or_else(|| panic!("no node {name}"))
            .extra
            .clone()
    };
    assert_eq!(
        extra("add").get("ffi_export").cloned(),
        Some(serde_json::json!({"abi": "c", "kind": "function", "name": "add"}))
    );
    assert_eq!(
        extra("scale").get("ffi_import").cloned(),
        Some(serde_json::json!({"abi": "c", "name": "scale"}))
    );
    assert_eq!(
        extra("legacy").get("ffi_import").cloned(),
        Some(serde_json::json!({"abi": "c", "name": "legacy", "library": "fastsum"}))
    );
    assert_eq!(extra("total").get("ffi_import"), None);
    let c_import = edges
        .iter()
        .find(|edge| edge.kind == "CALLS" && edge.target == "c.fast_sum")
        .expect("c.fast_sum call");
    assert_eq!(
        c_import.extra.get("c_import"),
        Some(&serde_json::json!(true))
    );
}

#[test]
fn zig_standard_library_calls_target_their_package() {
    let source = br#"const std = @import("std");
const builtin = @import("builtin");
const util = @import("util.zig");
const known = @import("known");
const mem = std.mem;
const print = std.debug.print;

fn init() void {}

pub fn main() !void {
    var list = std.ArrayList(u8).init(std.heap.page_allocator);
    _ = mem.eql(u8, "a", "b");
    print("{}\n", .{builtin.os.tag});
    util.helper();
    known.run();
    init();
}
"#;
    let (_, edges) = parse_zig("src/main.zig", source);
    let calls = edges
        .iter()
        .filter(|edge| edge.kind == "CALLS")
        .map(|edge| {
            (
                edge.target.as_str(),
                edge.extra["external_symbol"].as_str().unwrap_or_default(),
                edge.extra["confidence_tier"].as_str().unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();
    for expected in [
        ("std", "std.ArrayList", "HIGH"),
        // A member of the type the standard library just built.
        ("std", "std.ArrayList.init", "HIGH"),
        // Through constants bound to paths into `std`.
        ("std", "std.mem.eql", "HIGH"),
        ("std", "std.debug.print", "HIGH"),
        // Files and build modules are not the standard library; `init` is
        // this file's own.
        ("util.helper", "", ""),
        ("known.run", "", ""),
        ("src/main.zig::init", "", ""),
    ] {
        assert!(calls.contains(&expected), "{expected:?} not in {calls:?}");
    }
    let import = |target: &str| {
        edges
            .iter()
            .find(|edge| edge.kind == "IMPORTS_FROM" && edge.target == target)
            .unwrap_or_else(|| panic!("no import {target}"))
            .extra
            .clone()
    };
    for module in ["std", "builtin"] {
        assert_eq!(import(module)["stdlib"], true, "{module}");
        assert_eq!(import(module)["external_package"], module);
        assert_eq!(import(module)["confidence_tier"], "HIGH");
    }
    assert_eq!(import("known").get("stdlib"), None);
    assert!(edges.iter().any(|edge| edge.kind == "IMPORTS_FROM"
        && edge.target.ends_with("util.zig")
        && edge.extra.get("stdlib").is_none()));
}

#[test]
fn zig_receivers_record_the_call_they_came_from() {
    let source = br#"const Store = @import("store.zig").Store;
const Repo = struct {
    pub fn save(self: *Repo) void { _ = self; }
};
fn make() !Store { return Store.init(); }
fn find() callconv(.C) ?*Store { return null; }
fn run(p: *Store, q: anytype) !void {
    const s: Store = Store.init(1);
    s.save();
    var r = Repo{};
    r.save();
    const c = try make();
    c.query(1);
    find().?.close();
    p.save();
    q.save();
    q.pool.get();
    const t = Store.init(2);
    t.flush();
    q.where(1).where(2).first();
}
"#;
    let (nodes, edges) = parse_zig("src/app.zig", source);
    let returns = |name: &str| {
        nodes
            .iter()
            .find(|node| node.name == name)
            .and_then(|node| node.return_type.clone())
    };
    assert_eq!(returns("make").as_deref(), Some("!Store"));
    assert_eq!(returns("find").as_deref(), Some("?*Store"));
    let call = |target: &str, line: i64| {
        edges
            .iter()
            .find(|edge| edge.kind == "CALLS" && edge.target == target && edge.line == line)
            .unwrap_or_else(|| panic!("no {target} at {line} in {edges:?}"))
    };
    // `Store` is a type of another file.
    assert_eq!(call("save", 9).extra["receiver_type"], "Store");
    // `Repo` is this file's.
    assert!(
        call("src/app.zig::Repo.save", 11)
            .extra
            .get("receiver_type")
            .is_none()
    );
    assert_eq!(
        call("query", 13).extra["receiver_from"],
        serde_json::json!({"call": "make", "line": 12, "unwrap": true})
    );
    assert_eq!(
        call("close", 14).extra["receiver_from"],
        serde_json::json!({"call": "find", "line": 14, "unwrap": true})
    );
    assert_eq!(call("save", 15).extra["receiver_type"], "Store");
    // `anytype`: unknown, and not `Repo.save`.
    assert_eq!(call("save", 16).extra["receiver_unknown"], true);
    assert_eq!(call("get", 17).extra["receiver_unknown"], true);
    assert_eq!(call("flush", 19).extra["receiver_type"], "Store");
    assert_eq!(call("first", 20).extra["receiver_from"]["call"], "where");
}
