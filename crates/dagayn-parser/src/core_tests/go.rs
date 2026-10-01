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

#[test]
fn methods_on_a_type_from_another_file_are_contained_by_the_file() {
    // `Checker` is declared in a sibling file of the package.
    let source = b"package types\n\nfunc (c *Checker) collect() {}\n\ntype Local struct{}\n\nfunc (l Local) run() {}\n";
    let mut parser = RustOwnedParser::new();
    let (_, edges) = parser.parse_file("resolver.go", source);
    let contains: Vec<(&str, &str)> = edges
        .iter()
        .filter(|edge| edge.kind == "CONTAINS")
        .map(|edge| (edge.source.as_str(), edge.target.as_str()))
        .collect();
    assert!(contains.contains(&("resolver.go", "resolver.go::Checker.collect")));
    assert!(contains.contains(&("resolver.go::Local", "resolver.go::Local.run")));
    assert!(
        !contains
            .iter()
            .any(|(source, _)| *source == "resolver.go::Checker")
    );
}

#[test]
fn blank_identifier_declarations_are_not_nodes() {
    let source = b"package p\n\ntype _ int\n\nfunc _() { helper() }\n\nfunc real() {}\n";
    let mut parser = RustOwnedParser::new();
    let (nodes, edges) = parser.parse_file("p.go", source);
    let names: Vec<&str> = nodes.iter().map(|node| node.name.as_str()).collect();
    assert_eq!(names, vec!["p.go", "real"]);
    // The body is still walked; its calls come from the file.
    assert!(
        edges
            .iter()
            .any(|edge| edge.kind == "CALLS" && edge.source == "p.go" && edge.target == "helper")
    );
}

#[test]
fn go_standard_library_calls_target_their_package() {
    let source = br#"package main

import (
	"fmt"
	"net/http"
	str "strings"
	"math/rand/v2"
	"github.com/acme/tool"
	"myapp/internal/fmtutil"
)

func min(a, b int) int { return a }

func run(items []string) {
	fmt.Println(len(items))
	http.Get("x")
	str.ToUpper("a")
	str.NewReader("a").Read(nil)
	rand.IntN(3)
	tool.Do()
	fmtutil.Format()
	min(1, 2)
	helper()
}
"#;
    let (_, edges) = parse_go("cmd/main.go", source);
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
        ("fmt", "fmt.Println", "HIGH"),
        ("net/http", "net/http.Get", "HIGH"),
        // A renamed import names the package it imports.
        ("strings", "strings.ToUpper", "HIGH"),
        // A member of a value the package just built.
        ("strings", "strings.NewReader.Read", "HIGH"),
        ("math/rand/v2", "math/rand/v2.IntN", "HIGH"),
        // A predeclared function is inferred from its name alone.
        ("builtin", "len", "MEDIUM"),
        // Third-party and repository packages are not the standard library,
        // and `min` is this file's own function.
        ("Do", "", ""),
        ("Format", "", ""),
        ("cmd/main.go::min", "", ""),
        ("helper", "", ""),
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
    assert_eq!(import("net/http")["stdlib"], true);
    assert_eq!(import("net/http")["external_package"], "net/http");
    assert_eq!(import("net/http")["confidence_tier"], "HIGH");
    assert_eq!(import("github.com/acme/tool").get("stdlib"), None);
    assert_eq!(import("myapp/internal/fmtutil").get("stdlib"), None);
}

#[test]
fn go_packages_of_the_own_module_are_not_the_standard_library() {
    // A module named like a standard package root (`crypto/...` has no dot)
    // is this repository's, as `go.mod` says.
    let mut root = std::env::temp_dir();
    root.push(format!("dagayn-go-module-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("cmd")).unwrap();
    std::fs::write(root.join("go.mod"), "module crypto/vault\n\ngo 1.22\n").unwrap();
    let source = b"package main\n\nimport (\n\t\"crypto/sha256\"\n\t\"crypto/vault/store\"\n)\n\nfunc main() {\n\tsha256.Sum256(nil)\n\tstore.Open()\n}\n";
    std::fs::write(root.join("cmd/main.go"), source).unwrap();
    let mut parser = RustOwnedParser::new();
    let (_, edges) = parser.parse_file_in_repo(Some(&root), "cmd/main.go", source);
    let imports = edges
        .iter()
        .filter(|edge| edge.kind == "IMPORTS_FROM")
        .map(|edge| (edge.target.as_str(), edge.extra["stdlib"] == true))
        .collect::<Vec<_>>();
    assert!(imports.contains(&("crypto/sha256", true)), "{imports:?}");
    assert!(
        imports.contains(&("crypto/vault/store", false)),
        "{imports:?}"
    );
    assert!(
        !edges
            .iter()
            .any(|edge| edge.kind == "CALLS" && edge.target == "crypto/vault/store"),
        "{edges:?}"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn go_receivers_record_the_call_they_came_from() {
    let source = br#"package app

import "net/http"

type Store struct {
	repo Repo
}

func Open(p string) (*Store, error) { return nil, nil }
func (s *Store) Save() error { return s.repo.Find() }
func run(r *Repo, req *http.Request, p string) {
	s, err := Open(p)
	s.Save()
	NewConn(p).Close()
	c := NewConn(p)
	c.Close()
	local := &Store{}
	local.Save()
	r.Find()
	req.Cookie("id")
	b := NewBuilder()
	b.Flag(1).Flag(2)
	models.Helper()
	_ = err
}
"#;
    let (nodes, edges) = parse_go("app/run.go", source);
    let call = |target: &str, line: i64| {
        edges
            .iter()
            .find(|edge| edge.kind == "CALLS" && edge.target == target && edge.line == line)
            .unwrap_or_else(|| panic!("no {target} at {line} in {edges:#?}"))
    };
    let open = nodes.iter().find(|node| node.name == "Open").expect("Open");
    assert_eq!(open.return_type.as_deref(), Some("(*Store, error)"));
    // `s, err := Open(p)` takes the first result.
    assert_eq!(
        call("Save", 13).extra["receiver_from"],
        serde_json::json!({"call": "Open", "line": 12, "unwrap": true})
    );
    assert_eq!(call("Save", 13).extra["receiver_unknown"], true);
    assert_eq!(
        call("Close", 14).extra["receiver_from"],
        serde_json::json!({"call": "NewConn", "line": 14, "unwrap": false})
    );
    assert_eq!(
        call("Close", 16).extra["receiver_from"],
        serde_json::json!({"call": "NewConn", "line": 15, "unwrap": false})
    );
    // A type of this file declaring the method: the method itself.
    assert_eq!(
        call("app/run.go::Store.Save", 18)
            .extra
            .get("receiver_unknown"),
        None
    );
    // A type of another file, also through a field of the receiver's struct.
    assert_eq!(call("Find", 19).extra["receiver_type"], "Repo");
    assert_eq!(call("Find", 10).extra["receiver_type"], "Repo");
    // A standard-library type stays the standard library's.
    assert!(edges.iter().any(|edge| edge.kind == "CALLS"
        && edge.line == 20
        && edge.extra["external_symbol"] == "net/http.Request.Cookie"));
    // A chain repeating a method points past the repeats.
    assert!(
        edges
            .iter()
            .filter(|edge| edge.target == "Flag")
            .all(|edge| {
                edge.extra["receiver_from"]
                    == serde_json::json!({"call": "NewBuilder", "line": 21, "unwrap": false})
            })
    );
    // A package qualifier is no receiver.
    assert_eq!(call("Helper", 23).extra, serde_json::json!({}));
}

#[test]
fn go_variables_copied_from_a_call_result_keep_its_origin() {
    // `b := a` where `a := Open(p)`: `b.Save()` came from `Open` too.
    let source = b"package main\n\nfunc run(p string) {\n\ta := Open(p)\n\tb := a\n\tb.Save()\n}\n";
    let (_, edges) = parse_go("main.go", source);
    let save = edges
        .iter()
        .find(|edge| edge.kind == "CALLS" && edge.target == "Save")
        .expect("Save");
    assert_eq!(save.extra["receiver_from"]["call"], "Open");
}
