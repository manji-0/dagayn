use super::*;

#[test]
fn parses_lua_functions_methods_imports_tests_and_bridges() {
    let source = br#"local json = require("cjson")
local log = require("logging").getLogger("sample")

function greet(name)
    print("Hello, " .. name)
    return name
end

local transform = function(data)
    return json.encode(data)
end

function Animal.new(name)
    return setmetatable({}, Animal)
end

function Animal:speak()
    log:info(self.name)
end

function Dog:fetch(item)
    self:speak()
    os.execute("git status")
    return item
end

local function test_greet()
    local result = greet("World")
    assert(result == "World")
end
"#;
    let (nodes, edges) = parse_lua("sample.lua", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "new"
            && node.parent_name.as_deref() == Some("Animal")
            && node.params.as_deref() == Some("(name)")
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "fetch"
            && node.parent_name.as_deref() == Some("Dog")
    }));
    assert!(
        nodes
            .iter()
            .any(|node| { node.kind == "Test" && node.name == "test_greet" && node.is_test })
    );
    assert!(
        edges
            .iter()
            .any(|edge| { edge.kind == "IMPORTS_FROM" && edge.target == "cjson" })
    );
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.lua::Dog.fetch"
            && edge.target == "sample.lua::Animal.speak"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.source == "sample.lua::Dog.fetch"
            && edge.target == "git status"
            && edge.extra["evidence_source"] == "os.execute"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "TESTED_BY"
            && edge.source == "sample.lua::greet"
            && edge.target == "sample.lua::test_greet"
    }));
}

#[test]
fn lua_standard_library_calls_target_their_package() {
    let source = br#"local utf8 = require("utf8")
local str = require("string")
local json = require("cjson")
local table = {}

local function tostring(value)
    return "x"
end

function Widget:render()
    self:print()
    print(string.format("%d", 1))
    io.stdout:write("x")
    pairs(self)
    tostring(self)
    table.insert(self, 1)
    utf8.char(72)
    str.rep("x", 2)
    json.encode(self)
end
"#;
    let (_, edges) = parse_lua("widget.lua", source);
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
        // Through a library table: certain.
        ("string", "string.format", "HIGH"),
        ("io", "io.stdout.write", "HIGH"),
        // Through a local requiring a library.
        ("utf8", "utf8.char", "HIGH"),
        ("string", "string.rep", "HIGH"),
        // A base function called bare: likely.
        ("_G", "print", "MEDIUM"),
        ("_G", "pairs", "MEDIUM"),
        // A method is not the base function of its name; a local function and
        // a local table shadow the standard library's.
        ("print", "", ""),
        ("widget.lua::tostring", "", ""),
        ("insert", "", ""),
        ("encode", "", ""),
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
    assert_eq!(import("utf8")["stdlib"], true);
    assert_eq!(import("utf8")["external_package"], "utf8");
    assert_eq!(import("utf8")["confidence_tier"], "HIGH");
    assert_eq!(import("cjson").get("stdlib"), None);
}

#[test]
fn lua_receivers_record_the_call_they_came_from() {
    let source = br#"local Store = require("store")
local Repo = {}
function Repo.new() return setmetatable({}, Repo) end
function Repo:save() end
local M = {}
function M.save() end
function M.run(conn)
  local s = Store.new(conn)
  s:save()
  local r = Repo.new()
  r:save()
  local c = connect(conn)
  c:query(1)
  connect(conn):close()
  conn:save()
  self.db:exec()
  M.save()
  q:where(1):where(2):first()
end
"#;
    let (_, edges) = parse_lua("app.lua", source);
    let call = |target: &str, line: i64| {
        edges
            .iter()
            .find(|edge| edge.kind == "CALLS" && edge.target == target && edge.line == line)
            .unwrap_or_else(|| panic!("no {target} at {line} in {edges:?}"))
    };
    // `Store` is a table of another file.
    assert_eq!(call("save", 9).extra["receiver_type"], "Store");
    // `Repo` is this file's.
    assert!(
        call("app.lua::Repo.save", 11)
            .extra
            .get("receiver_type")
            .is_none()
    );
    assert_eq!(
        call("query", 13).extra["receiver_from"],
        serde_json::json!({"call": "connect", "line": 12, "unwrap": false})
    );
    assert_eq!(
        call("close", 14).extra["receiver_from"],
        serde_json::json!({"call": "connect", "line": 14, "unwrap": false})
    );
    // A parameter's method is not the file's `M.save`.
    assert_eq!(call("save", 15).extra["receiver_unknown"], true);
    assert_eq!(call("exec", 16).extra["receiver_unknown"], true);
    assert!(
        call("app.lua::M.save", 17)
            .extra
            .get("receiver_unknown")
            .is_none()
    );
    // `q` is a global, possibly a module table: `q:where(1)` is left alone.
    assert!(edges.iter().any(|edge| edge.target == "where"
        && edge.line == 18
        && edge.extra.get("receiver_unknown").is_none()));
    assert_eq!(call("first", 18).extra["receiver_from"]["call"], "where");
}
