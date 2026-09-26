use super::*;

#[test]
fn parses_r_functions_classes_imports_calls_and_bridges() {
    let source = br#"library(dplyr)
require(ggplot2)
source("utils.R")

add <- function(x, y) {
  x + y
}

multiply = function(a, b) {
  a * b
}

MyClass <- setRefClass("MyClass",
  fields = list(name = "character", age = "numeric"),
  methods = list(
    greet = function() {
      cat(paste("Hello", name))
    },
    get_age = function() {
      return(age)
    }
  )
)

process_data <- function(data) {
  result <- dplyr::filter(data, x > 5)
  summary <- dplyr::summarize(result, mean_x = mean(x))
  add(1, 2)
  summary
}
"#;
    let (nodes, edges) = parse_r("sample.R", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Function" && node.name == "add" && node.params.as_deref() == Some("(x, y)")
    }));
    assert!(
        nodes
            .iter()
            .any(|node| { node.kind == "Class" && node.name == "MyClass" && node.language == "r" })
    );
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "greet"
            && node.parent_name.as_deref() == Some("MyClass")
    }));
    assert!(
        edges
            .iter()
            .any(|edge| { edge.kind == "IMPORTS_FROM" && edge.target == "dplyr" })
    );
    assert!(
        edges
            .iter()
            .any(|edge| { edge.kind == "IMPORTS_FROM" && edge.target == "utils.R" })
    );
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.R::process_data"
            && edge.target == "dplyr::filter"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.R::process_data"
            && edge.target == "sample.R::add"
    }));

    let bridge_source = br#"system("./target/release/dagayn-core build .")
system2("./scripts/build.sh", args = c("--strict"))
.Call("dagayn_compute")
.External("dagayn_helper")
dyn.load("./target/release/libdagayn.so")
library.dynam("dagayn", "./target/release")

run_dynamic <- function(cmd) {
  system(cmd)
}
"#;
    let (_nodes, bridge_edges) = parse_r("bridge.R", bridge_source);
    let cross_edges = bridge_edges
        .iter()
        .filter(|edge| edge.kind == "CROSS_ARTIFACT")
        .collect::<Vec<_>>();
    assert_eq!(cross_edges.len(), 7);
    assert!(cross_edges.iter().any(|edge| {
        edge.target == "./target/release/libdagayn.so"
            && edge.extra["evidence_source"] == "dyn.load"
            && edge.extra["confidence_tier"] == "HIGH"
    }));
    assert!(cross_edges.iter().any(|edge| {
        edge.target == "<dynamic:system@bridge.R:9>"
            && edge.extra["evidence_source"] == "system"
            && edge.extra["confidence_tier"] == "LOW"
    }));
}
