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
            && edge.target == "std.debug.print"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "TESTED_BY"
            && edge.source == "src/main.zig::Point.init"
            && edge.target == "src/main.zig::point doubles"
    }));
}
