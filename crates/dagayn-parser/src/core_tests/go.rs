use super::*;

#[test]
fn parses_go_types_methods_calls_and_bridges() {
    let source = br#"package main

import (
  "os"
  "os/exec"
  "plugin"
)

type Repo struct {}

func NewRepo() *Repo {
  return &Repo{}
}

func (r *Repo) Save() {
  os.WriteFile("output.json", []byte("ok"), 0644)
}

func runCommand(path string) {
  exec.Command("git", "status")
  os.ReadFile(path)
  plugin.Open("mylib.so")
}
"#;
    let (nodes, edges) = parse_go("main.go", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "Repo"
            && node.language == "go"
            && node.extra["type_role"] == "struct"
            && node.extra["container_role"] == "data_container"
            && node.extra["value_semantics"] == true
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "Save"
            && node.parent_name.as_deref() == Some("Repo")
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM" && edge.source == "main.go" && edge.target == "os/exec"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CONTAINS"
            && edge.source == "main.go::Repo"
            && edge.target == "main.go::Repo.Save"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "git"
            && edge.extra["evidence_source"] == "exec.Command"
            && edge.extra["confidence_tier"] == "HIGH"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "<dynamic:os.ReadFile@main.go:21>"
            && edge.extra["confidence_tier"] == "LOW"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CROSS_ARTIFACT"
            && edge.target == "mylib.so"
            && edge.extra["evidence_source"] == "plugin.Open"
    }));
}

#[test]
fn records_go_webassembly_exports() {
    let source = br#"package main

import "syscall/js"

//go:wasmexport add
func add(a, b int32) int32 { return a + b }

// mul multiplies.
//export mul
func mul(a, b int32) int32 { return a * b }

//export stale

func notExported() {}

func fastSum(this js.Value, args []js.Value) any { return nil }

func main() {
	js.Global().Set("goFastSum", js.FuncOf(fastSum))
	js.Global().Set("goMean", js.FuncOf(func(this js.Value, args []js.Value) any {
		return nil
	}))
}
"#;
    let (nodes, _) = parse_go("wasm/main.go", source);
    let extra = |name: &str| {
        nodes
            .iter()
            .find(|node| node.name == name)
            .unwrap_or_else(|| panic!("no node {name}"))
            .extra
            .clone()
    };
    assert_eq!(
        extra("add")["ffi_export"],
        serde_json::json!({"abi": "wasm", "kind": "function", "name": "add"})
    );
    assert_eq!(
        extra("mul")["ffi_export"],
        serde_json::json!({"abi": "c", "kind": "function", "name": "mul"})
    );
    // A directive separated by a blank line is not attached.
    assert!(extra("notExported").get("ffi_export").is_none());
    assert_eq!(
        extra("fastSum")["ffi_exports"],
        serde_json::json!([{"abi": "js_global", "kind": "function", "name": "goFastSum"}])
    );
    // A function literal is exposed through the function registering it.
    assert_eq!(
        extra("main")["ffi_exports"],
        serde_json::json!([{"abi": "js_global", "kind": "function", "name": "goMean"}])
    );
}

#[test]
fn records_go_wasmimport_declarations() {
    let source = b"package main\n\n//go:wasmimport env log_value\nfunc logValue(v int32)\n\nfunc main() { logValue(1) }\n";
    let mut parser = RustOwnedParser::new();
    let (nodes, _) = parser.parse_file("gowasm/main.go", source);
    let log = nodes.iter().find(|node| node.name == "logValue").unwrap();
    assert_eq!(
        log.extra.get("ffi_import").cloned(),
        Some(serde_json::json!({"abi": "wasmimport", "module": "env", "name": "log_value"}))
    );
}

#[test]
fn records_cgo_calls_and_preamble_libraries() {
    let source = br#"package fastsum

/*
#cgo CFLAGS: -O2
#cgo linux LDFLAGS: -lmathx -L${SRCDIR}/lib -lm
#include "sum.h"
*/
import "C"

func Total(n int) float64 {
	return float64(C.fast_sum(C.int(n)))
}

func Local() int { return helper() }
"#;
    let mut parser = RustOwnedParser::new();
    let (nodes, edges) = parser.parse_file("fastsum/sum.go", source);
    assert_eq!(
        nodes[0].extra.get("cgo_libraries").cloned(),
        Some(serde_json::json!(["mathx", "m"]))
    );
    let receiver = |target: &str| {
        edges
            .iter()
            .find(|edge| edge.kind == "CALLS" && edge.target == target)
            .unwrap_or_else(|| panic!("no call {target}"))
            .extra
            .get("receiver")
            .cloned()
    };
    assert_eq!(receiver("fast_sum"), Some(serde_json::json!("C")));
    assert_eq!(receiver("helper"), None);
}

#[test]
fn go_wasm_hosts_emit_loads_and_export_calls() {
    let source = br#"package main

func run() {
	wasm, _ := os.ReadFile("guest.wasm")
	mod, _ := r.Instantiate(ctx, wasm)
	mod.ExportedFunction("add").Call(ctx, 1, 2)
	f := instance.GetFunc(store, "mul")
	data, _ := os.ReadFile("config.json")
}
"#;
    let mut parser = RustOwnedParser::new();
    let (_, edges) = parser.parse_file("gohost/main.go", source);
    let bridges: Vec<(&str, &str)> = edges
        .iter()
        .filter(|edge| edge.kind == "CROSS_ARTIFACT")
        .map(|edge| {
            (
                edge.extra["relationship_role"].as_str().unwrap_or_default(),
                edge.target.as_str(),
            )
        })
        .collect();
    assert_eq!(
        bridges,
        vec![
            ("loads_wasm_module", "guest.wasm"),
            ("calls_wasm_export", "add"),
            ("calls_wasm_export", "mul"),
            ("reads_file", "config.json"),
        ]
    );
}
