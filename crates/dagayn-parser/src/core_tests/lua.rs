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
