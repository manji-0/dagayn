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
    assert!(
        edges
            .iter()
            .any(|edge| { edge.kind == "IMPORTS_FROM" && edge.target == "Statistics.mean" })
    );
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
