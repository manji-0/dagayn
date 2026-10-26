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
    // A package qualifier (or a variable of the package) names the call's
    // receiver: it is no plain call.
    assert_eq!(
        call("Helper", 23).extra,
        serde_json::json!({"receiver": "models"})
    );
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

#[test]
fn go_test_files_are_test_code_and_their_test_functions_are_tests() {
    let source = br#"package store_test

import "testing"

type fixture struct{}

func newFixture() *fixture { return &fixture{} }

func TestSave(t *testing.T) {
	s := newFixture()
	Save(s)
}

func Test(t *testing.T) {}

func Test_open(t *testing.T) { Open() }

func BenchmarkSave(b *testing.B) { Save(nil) }

func FuzzParse(f *testing.F) { Parse("") }

func Example() {}

func ExampleStore_Save() { Save(nil) }

func Testify() {}

func (f *fixture) TestMethod(t *testing.T) {}
"#;
    let (nodes, edges) = parse_go("store/store_test.go", source);
    let kind_of = |name: &str| {
        let node = nodes
            .iter()
            .find(|node| node.name == name)
            .unwrap_or_else(|| panic!("no {name} in {nodes:#?}"));
        (node.kind.as_str(), node.is_test)
    };
    for name in [
        "TestSave",
        "Test",
        "Test_open",
        "BenchmarkSave",
        "FuzzParse",
        "Example",
        "ExampleStore_Save",
    ] {
        assert_eq!(kind_of(name), ("Test", true), "{name}");
    }
    // A helper, a name that only starts with `Test`, and a method are test
    // code, not tests `go test` runs.
    for name in ["newFixture", "Testify", "TestMethod"] {
        assert_eq!(kind_of(name), ("Function", true), "{name}");
    }
    assert_eq!(kind_of("fixture"), ("Class", true));
    assert!(nodes.iter().all(|node| node.is_test), "{nodes:#?}");
    let tested_by = |target: &str, test: &str| {
        edges.iter().any(|edge| {
            edge.kind == "TESTED_BY"
                && edge.source == target
                && edge.target == format!("store/store_test.go::{test}")
        })
    };
    assert!(tested_by("Save", "TestSave"), "{edges:#?}");
    assert!(tested_by("Save", "BenchmarkSave"));
    assert!(tested_by("Parse", "FuzzParse"));
    assert!(tested_by("Save", "ExampleStore_Save"));
    assert!(tested_by("store/store_test.go::newFixture", "TestSave"));
}

#[test]
fn go_files_declare_their_package_import_path_as_namespace() {
    let mut root = std::env::temp_dir();
    root.push(format!("dagayn-go-namespaces-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let files: &[(&str, &str)] = &[
        ("go.mod", "module example.com/m\n\ngo 1.22\n"),
        ("main.go", "package main\n\nfunc main() {}\n"),
        ("zstd/enc.go", "package zstd\n\nfunc encode() {}\n"),
        (
            "zstd/enc_test.go",
            "package zstd\n\nimport \"testing\"\n\nfunc TestEncode(t *testing.T) { encode() }\n",
        ),
        (
            "zstd/x_test.go",
            "package zstd_test\n\nimport \"testing\"\n\nfunc TestX(t *testing.T) {}\n",
        ),
        (
            "zstd/gen.go",
            "//go:build ignore\n\npackage main\n\nfunc main() {}\n",
        ),
        (
            "zstd/old.go",
            "// +build ignore\n\npackage main\n\nfunc main() {}\n",
        ),
        ("tools/go.mod", "module example.com/tools\n"),
        ("tools/cmd/a.go", "package main\n\nfunc main() {}\n"),
    ];
    for (path, text) in files {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let namespaces = |file: &str| {
        let source = std::fs::read(root.join(file)).unwrap();
        let mut parser = RustOwnedParser::new();
        let (nodes, _) = parser.parse_file_in_repo(Some(&root), file, &source);
        nodes[0].extra.get("namespaces").cloned()
    };
    let one = |namespace: &str| Some(serde_json::json!([namespace]));
    assert_eq!(namespaces("main.go"), one("example.com/m"));
    assert_eq!(namespaces("zstd/enc.go"), one("example.com/m/zstd"));
    // A test of the package is in the package; an external test package is
    // a package of its own.
    assert_eq!(namespaces("zstd/enc_test.go"), one("example.com/m/zstd"));
    assert_eq!(namespaces("zstd/x_test.go"), one("example.com/m/zstd_test"));
    // A file the build ignores belongs to no package.
    assert_eq!(namespaces("zstd/gen.go"), None);
    assert_eq!(namespaces("zstd/old.go"), None);
    // The nearest go.mod decides the module.
    assert_eq!(namespaces("tools/cmd/a.go"), one("example.com/tools/cmd"));
    let _ = std::fs::remove_dir_all(root);
    // Without a repository root, the directory stands in for the module path.
    let (nodes, _) = parse_go("zstd/enc.go", b"package zstd\n\nfunc encode() {}\n");
    assert_eq!(nodes[0].extra.get("namespaces").cloned(), one("zstd"));
}

#[test]
fn go_calls_through_a_package_or_a_local_function_value_are_no_plain_calls() {
    let source = br#"package zstd

import (
	"fmt"
	h "example.com/m/huff0"
	"example.com/m/internal/snapref"
	"gopkg.in/yaml.v3"
)

var defaults Options // typed: `defaults.Load()` is `Options.Load`

func encode(f func() int) {
	h.Compress()
	snapref.Encode()
	yaml.Marshal()
	defaults.Load()
	registry.Load()
	fmt.Println()
	f()
	encode(nil)
}
"#;
    let (_, edges) = parse_go("zstd/enc.go", source);
    let extra = |target: &str| {
        edges
            .iter()
            .find(|edge| edge.kind == "CALLS" && edge.target == target)
            .unwrap_or_else(|| panic!("no call {target} in {edges:#?}"))
            .extra
            .clone()
    };
    assert_eq!(
        extra("Compress"),
        serde_json::json!({"receiver": "h", "import_path": "example.com/m/huff0"})
    );
    assert_eq!(
        extra("Encode"),
        serde_json::json!({"receiver": "snapref", "import_path": "example.com/m/internal/snapref"})
    );
    assert_eq!(
        extra("Marshal"),
        serde_json::json!({"receiver": "yaml", "import_path": "gopkg.in/yaml.v3"})
    );
    // A variable of another file of the package.
    assert!(
        edges.iter().any(|edge| edge.target == "Load"
            && edge.extra == serde_json::json!({"receiver": "registry"}))
    );
    // A parameter holding a function is called, not a function of the
    // package.
    assert_eq!(extra("f"), serde_json::json!({"callee_local": true}));
    // A plain call, here to the caller itself, keeps binding in the file.
    assert!(
        edges
            .iter()
            .any(|edge| edge.kind == "CALLS" && edge.target == "zstd/enc.go::encode"),
        "{edges:#?}"
    );
}

#[test]
fn go_test_names_outside_test_files_are_functions() {
    let source = b"package store\n\nimport \"testing\"\n\nfunc TestSave(t *testing.T) { Save() }\n\nfunc Save() {}\n";
    let (nodes, edges) = parse_go("store/store.go", source);
    let test = nodes.iter().find(|node| node.name == "TestSave").unwrap();
    assert_eq!((test.kind.as_str(), test.is_test), ("Function", false));
    assert!(nodes.iter().all(|node| !node.is_test));
    assert!(!edges.iter().any(|edge| edge.kind == "TESTED_BY"));
}

#[test]
fn go_receivers_typed_by_declared_fields_elements_and_embedding() {
    let source = br#"package fse

import "example.com/m/huff0"

type bitWriter struct{}

func (b *bitWriter) reset() {}

type blockEnc struct {
	wr    bitWriter
	codes []hcode
	fn    func()
	fastGen
	*Base
	ext   huff0.Scratch
}

func (b *blockEnc) encode(in []token, table map[string]*hcode) {
	wr := &b.wr
	wr.reset()
	b.wr.reset()
	b.codes[3].set()
	in[0].literal()
	for i, tok := range in {
		tok.offset()
		_ = i
	}
	table["x"].zero()
}

func (s *Scratch) compress() {
	s.bw.close()
	br := &s.br
	br.advance()
	nodes := s.nodes[:4]
	nodes[1].count()
	for _, n := range s.nodes {
		n.parent()
	}
}

func use(h *huff0.Scratch) {
	h.Encode()
}
"#;
    let (nodes, edges) = parse_go("fse/compress.go", source);
    let call = |target: &str, line: i64| {
        edges
            .iter()
            .find(|edge| edge.kind == "CALLS" && edge.target == target && edge.line == line)
            .unwrap_or_else(|| panic!("no {target} at {line} in {edges:#?}"))
    };
    // A field of a struct of this file, read into a variable or not.
    assert_eq!(
        call("fse/compress.go::bitWriter.reset", 20)
            .extra
            .get("receiver_unknown"),
        None
    );
    assert_eq!(
        call("fse/compress.go::bitWriter.reset", 21)
            .extra
            .get("receiver_unknown"),
        None
    );
    // An element of a slice, array, or map the declaration types.
    assert_eq!(call("set", 22).extra["receiver_type"], "hcode");
    assert_eq!(call("literal", 23).extra["receiver_type"], "token");
    assert_eq!(call("offset", 25).extra["receiver_type"], "token");
    assert_eq!(call("zero", 28).extra["receiver_type"], "hcode");
    // A field of a struct of another file: resolution reads its type.
    assert_eq!(
        call("close", 32).extra["receiver_field"],
        serde_json::json!({"owner": "Scratch", "field": "bw"})
    );
    assert_eq!(
        call("advance", 34).extra["receiver_field"],
        serde_json::json!({"owner": "Scratch", "field": "br"})
    );
    assert_eq!(
        call("count", 36).extra["receiver_field"],
        serde_json::json!({"owner": "Scratch", "field": "nodes", "element": true})
    );
    assert_eq!(
        call("parent", 38).extra["receiver_field"],
        serde_json::json!({"owner": "Scratch", "field": "nodes", "element": true})
    );
    for line in [32, 34, 36, 38] {
        let edge = edges
            .iter()
            .find(|edge| edge.kind == "CALLS" && edge.line == line)
            .expect("call");
        assert_eq!(edge.extra["receiver_unknown"], true, "{edge:?}");
    }
    // Another package's type is bound in that package only.
    assert_eq!(call("Encode", 43).extra["receiver_type"], "Scratch");
    assert_eq!(
        call("Encode", 43).extra["receiver_package"],
        "example.com/m/huff0"
    );
    // A struct records its fields' declared types and the types it embeds.
    let block = nodes
        .iter()
        .find(|node| node.name == "blockEnc")
        .expect("blockEnc");
    assert_eq!(
        block.extra["field_types"],
        serde_json::json!({
            "wr": "bitWriter",
            "codes": "[]hcode",
            "fn": "",
            "fastGen": "fastGen",
            "Base": "Base",
            "ext": "example.com/m/huff0.Scratch",
        })
    );
    assert_eq!(
        block.extra["embedded"],
        serde_json::json!(["fastGen", "Base"])
    );
}

#[test]
fn go_receivers_typed_by_channels_make_and_field_paths() {
    let source = br#"package zstd

type blockDec struct{}

func (b *blockDec) decode() {}

func run(e *Encoder, br []bitReader) {
	seqs := make(chan *blockDec, 4)
	for block := range seqs {
		block.decode()
	}
	got := <-seqs
	got.decode()
	js := &e.state.jobs
	js.put()
	e.state.jobs.get()
	for i := range br {
		br := &br[i]
		br.finished()
	}
}
"#;
    let (_, edges) = parse_go("zstd/run.go", source);
    let call = |target: &str, line: i64| {
        edges
            .iter()
            .find(|edge| edge.kind == "CALLS" && edge.target == target && edge.line == line)
            .unwrap_or_else(|| panic!("no {target} at {line} in {edges:#?}"))
    };
    // A channel's values, by `range` or a receive, of a `make`d channel.
    call("zstd/run.go::blockDec.decode", 10);
    call("zstd/run.go::blockDec.decode", 13);
    // A path of fields from a struct of another file.
    assert_eq!(
        call("put", 15).extra["receiver_field"],
        serde_json::json!({"owner": "Encoder", "field": "state.jobs"})
    );
    assert_eq!(
        call("get", 16).extra["receiver_field"],
        serde_json::json!({"owner": "Encoder", "field": "state.jobs"})
    );
    // A variable shadowing the slice it takes an element of.
    assert_eq!(call("finished", 19).extra["receiver_type"], "bitReader");
}
