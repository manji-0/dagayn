use super::*;

#[test]
fn parses_julia_modules_types_functions_macros_and_bridges() {
    let source = br#"module SampleModule

using LinearAlgebra
using Statistics: mean, std
import Base: show, print
import JSON

export greet, Dog, process
public square, add

@enum Color RED BLUE GREEN

abstract type AbstractAnimal end

struct Dog <: AbstractAnimal
    name::String
    age::Int
end

mutable struct MutablePoint
    x::Float64
    y::Float64
end

function greet(name::String)
    println("Hello, $name")
end

function Base.show(io::IO, d::Dog)
    print(io, "Dog($(d.name))")
end

add(a, b) = a + b

square(x) = x^2

macro sayhello(name)
    :(println("Hello, ", $name))
end

function outer()
    function inner()
        return 1
    end
    x = inner()
    result = map(v -> v^2, [1,2,3])
    return x
end

function process(data::Vector{Float64}; verbose=false)
    if verbose
        println("Processing...")
    end
    normed = data ./ maximum(data)
    return sum(normed) / length(normed)
end

include("utils.jl")

@testset "Arithmetic" begin
    @test add(1, 2) == 3
    @test square(4) == 16
end

end # module
"#;
    let (nodes, edges) = parse_julia("sample.jl", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Class" && node.name == "SampleModule" && node.language == "julia"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class" && node.name == "Color" && node.extra["julia_kind"] == "enum"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "GREEN"
            && node.parent_name.as_deref() == Some("Color")
            && node.extra["julia_kind"] == "enum_variant"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "AbstractAnimal"
            && node.extra["type_role"] == "abstract_type"
            && node.extra["is_abstract"] == true
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "show"
            && node.parent_name.as_deref() == Some("SampleModule")
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "inner"
            && node.parent_name.as_deref() == Some("SampleModule.outer")
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Test"
            && node.name.starts_with("testset:Arithmetic@L")
            && node.parent_name.as_deref() == Some("SampleModule")
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM"
            && edge.target == "Statistics"
            && edge.extra["external_symbol"] == "Statistics.mean"
    }));
    assert!(
        edges
            .iter()
            .any(|edge| { edge.kind == "IMPORTS_FROM" && edge.target == "utils.jl" })
    );
    assert!(edges.iter().any(|edge| {
        edge.kind == "INHERITS"
            && edge.source == "sample.jl::SampleModule.Dog"
            && edge.target == "AbstractAnimal"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "REFERENCES"
            && edge.source == "sample.jl::SampleModule.show"
            && edge.target == "Base"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.jl::SampleModule.outer"
            && edge.target == "sample.jl::SampleModule.outer.inner"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge
                .source
                .starts_with("sample.jl::SampleModule.testset:Arithmetic@L")
            && edge.target == "sample.jl::SampleModule.add"
    }));

    let bridge_source = br#"function run_command()
    run(`git status`)
end

function read_config()
    open("config.yaml", "r")
end

function write_output()
    write("output.json", "{}")
end

function load_lib()
    Libdl.dlopen("mylib.so")
end
"#;
    let (_nodes, bridge_edges) = parse_julia("bridge.jl", bridge_source);
    assert!(bridge_edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.extra["evidence_source"] == "run"
            && edge.extra["confidence_tier"] == "LOW"
    }));
    assert!(bridge_edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "mylib.so"
            && edge.extra["evidence_source"] == "Libdl.dlopen"
    }));
}

#[test]
fn julia_ccall_records_library_and_symbol() {
    let source = br#"function a(x)
    ccall(("sin", libm), Cdouble, (Cdouble,), x)
end
function b(x)
    @ccall "libm".cos(x::Cdouble)::Cdouble
end
"#;
    let mut parser = RustOwnedParser::new();
    let (_, edges) = parser.parse_file("src/m.jl", source);
    let loads: Vec<(&str, &str, &str)> = edges
        .iter()
        .filter(|edge| edge.extra["relationship_role"] == "loads_shared_library")
        .map(|edge| {
            (
                edge.source.as_str(),
                edge.target.as_str(),
                edge.extra["symbol"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        loads,
        vec![
            ("src/m.jl::a", "libm", "sin"),
            ("src/m.jl::b", "libm", "cos")
        ]
    );
}

#[test]
fn julia_local_short_function_calls_come_from_its_node() {
    let source = b"function outer(xs)\n    f(x) = helper(x)\n    map(f, xs)\nend\n";
    let mut parser = RustOwnedParser::new();
    let (nodes, edges) = parser.parse_file("a.jl", source);
    let qualified: Vec<String> = nodes
        .iter()
        .map(|node| match &node.parent_name {
            Some(parent) => format!("a.jl::{parent}.{}", node.name),
            None => format!("a.jl::{}", node.name),
        })
        .collect();
    let helper = edges
        .iter()
        .find(|edge| edge.kind == "CALLS" && edge.target == "helper")
        .expect("call to helper");
    assert!(
        qualified.contains(&helper.source),
        "{} not in {qualified:?}",
        helper.source
    );
}

#[test]
fn julia_standard_library_calls_target_their_package() {
    let source = br#"using LinearAlgebra
using Statistics: mean
using DataFrames

function stats(xs)
    m = mean(xs)
    n = norm(xs)
    top = Base.max(m, n)
    println("mean=", m)
    push!(xs, top)
    nrow(xs)
    return sum(xs)
end

sum(xs) = 0
"#;
    let (_, edges) = parse_julia("stats.jl", source);
    let tier = |kind: &str, target: &str, symbol: &str| {
        edges
            .iter()
            .find(|edge| {
                edge.kind == kind
                    && edge.target == target
                    && edge.extra["external_symbol"].as_str().unwrap_or_default() == symbol
            })
            .map(|edge| edge.extra["confidence_tier"].as_str().unwrap_or_default())
    };
    // Imports of stdlib modules: certain; a registered package is not.
    assert_eq!(tier("IMPORTS_FROM", "LinearAlgebra", ""), Some("HIGH"));
    assert_eq!(
        tier("IMPORTS_FROM", "Statistics", "Statistics.mean"),
        Some("HIGH")
    );
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM"
            && edge.target == "DataFrames"
            && edge.extra.get("stdlib").is_none()
    }));
    // Named through the module or imported by name: certain.
    assert_eq!(tier("CALLS", "Base", "Base.max"), Some("HIGH"));
    assert_eq!(tier("CALLS", "Statistics", "Statistics.mean"), Some("HIGH"));
    // Exported by a `using`'d module, or by `Base`: likely.
    assert_eq!(
        tier("CALLS", "LinearAlgebra", "LinearAlgebra.norm"),
        Some("MEDIUM")
    );
    assert_eq!(tier("CALLS", "Base", "println"), Some("MEDIUM"));
    assert_eq!(tier("CALLS", "Base", "push!"), Some("MEDIUM"));
    // A name the file defines, and a package's, stay as they were.
    let calls = edges
        .iter()
        .filter(|edge| edge.kind == "CALLS" && edge.extra.get("stdlib").is_none())
        .map(|edge| edge.target.as_str())
        .collect::<Vec<_>>();
    for expected in ["stats.jl::sum", "nrow"] {
        assert!(calls.contains(&expected), "{expected:?} not in {calls:?}");
    }
}

#[test]
fn julia_receivers_record_the_call_they_came_from() {
    let source = b"struct Repo end\nfunction make(x::Int)::Store\n    Store(x)\nend\nshort(x)::Repo = Repo()\nsave(x) = x\nfunction run(s::Store, r::Repo, q)\n    s.save(1)\n    r.save(1)\n    q.save()\n    c = make(1)\n    c.close()\n    Base.max(1, 2)\nend\n";
    let (nodes, edges) = parse_julia("src/app.jl", source);
    let returns = |name: &str| {
        nodes
            .iter()
            .find(|node| node.name == name)
            .and_then(|node| node.return_type.clone())
    };
    assert_eq!(returns("make").as_deref(), Some("Store"));
    assert_eq!(returns("short").as_deref(), Some("Repo"));
    assert_eq!(returns("save"), None);
    let call = |target: &str, line: i64| {
        edges
            .iter()
            .find(|edge| edge.kind == "CALLS" && edge.target == target && edge.line == line)
            .unwrap_or_else(|| panic!("no {target} at {line} in {edges:?}"))
    };
    assert_eq!(call("save", 8).extra["receiver_type"], "Store");
    // `Repo` is this file's.
    assert!(
        call("src/app.jl::save", 9)
            .extra
            .get("receiver_type")
            .is_none()
    );
    assert_eq!(call("save", 10).extra["receiver_unknown"], true);
    assert_eq!(
        call("close", 12).extra["receiver_from"],
        serde_json::json!({"call": "make", "line": 11, "unwrap": false})
    );
    assert!(call("Base", 13).extra.get("receiver_unknown").is_none());
}
