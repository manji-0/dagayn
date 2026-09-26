const std = @import("std");
const util = @import("util.zig");

pub const Point = struct {
    x: i32,
    y: i32,

    pub const Origin = struct {
        pub fn get() Point {
            return Point.init(0, 0);
        }
    };

    pub fn init(x: i32, y: i32) Point {
        return .{ .x = x, .y = y };
    }

    pub fn manhattan(self: Point) i32 {
        return self.axis(self.x) + util.abs(self.y);
    }

    fn axis(self: Point, v: i32) i32 {
        _ = self;
        return util.abs(v);
    }
};

const Color = enum {
    red,
    green,

    pub fn isRed(self: Color) bool {
        return self == .red;
    }
};

const Shape = union(enum) { circle: f32, square: f32 };

const ParseError = error{ Empty, Invalid };

pub fn Stack(comptime T: type) type {
    return struct {
        items: []T,

        pub fn push(self: *@This(), v: T) void {
            _ = self;
            _ = v;
        }
    };
}

extern "c" fn puts(s: [*:0]const u8) c_int;

pub fn main() void {
    const p = Point.init(3, -4);
    std.debug.print("{}\n", .{p.manhattan()});
}

test "manhattan distance" {
    const p = Point.init(3, -4);
    try std.testing.expectEqual(@as(i32, 7), p.manhattan());
}

test main {
    main();
}
