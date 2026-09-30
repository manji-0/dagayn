use super::*;

#[test]
fn parses_python_items_imports_and_calls() {
    let source = br#"from models import User
import os

class Service(Base):
    def run(self, name: str) -> User:
        helper(name)
        os.getenv("ENV")

def helper(value: str) -> None:
    print(value)
"#;
    let (nodes, edges) = parse_python("app.py", source);
    let node_names = nodes
        .iter()
        .map(|node| {
            (
                node.kind.as_str(),
                node.name.as_str(),
                node.parent_name.as_deref(),
                node.params.as_deref(),
                node.return_type.as_deref(),
            )
        })
        .collect::<Vec<_>>();
    assert!(node_names.contains(&("File", "app.py", None, None, None)));
    assert!(node_names.contains(&("Class", "Service", None, None, None)));
    assert!(node_names.contains(&(
        "Function",
        "run",
        Some("Service"),
        Some("(self, name: str)"),
        Some("User")
    )));
    assert!(node_names.contains(&(
        "Function",
        "helper",
        None,
        Some("(value: str)"),
        Some("None")
    )));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM" && edge.source == "app.py" && edge.target == "models"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM" && edge.source == "app.py" && edge.target == "os"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "INHERITS" && edge.source == "app.py::Service" && edge.target == "Base"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "app.py::Service.run"
            && edge.target == "app.py::helper"
    }));
}

#[test]
fn parses_python_protocols_type_aliases_and_decorators() {
    let source = br#"
from typing import Protocol
from dataclasses import dataclass
from abc import ABC, abstractmethod

type UserId = int

class IRepo(Protocol):
    def find(self, id: UserId) -> str: ...

class Base(ABC):
    @abstractmethod
    def load(self) -> str: ...

@dataclass
class Item:
    name: str

class Repo(Base, IRepo):
    def find(self, id: UserId) -> str:
        return self.load()

    def load(self) -> str:
        return Item("x").name

def make() -> Repo:
    repo = Repo()
    return repo.find(1)
"#;
    let (nodes, edges) = parse_python("app.py", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Type" && node.name == "UserId" && node.extra["type_role"] == "alias"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "IRepo"
            && node.extra["type_role"] == "protocol"
            && node.extra["is_contract"] == true
            && node.extra["is_abstract"] == true
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "Base"
            && node.extra["type_role"] == "abstract_class"
            && node.extra["is_abstract"] == true
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "Item"
            && node.extra["decorators"]
                .as_array()
                .is_some_and(|values| values.iter().any(|value| value == "dataclass"))
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "load"
            && node.parent_name.as_deref() == Some("Base")
            && node.extra["is_abstract"] == true
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPLEMENTS" && edge.source == "app.py::Repo" && edge.target == "IRepo"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "INHERITS" && edge.source == "app.py::Repo" && edge.target == "Base"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "app.py::Repo.find"
            && edge.target == "app.py::Repo.load"
    }));
    assert!(
        edges.iter().any(|edge| {
            edge.kind == "CALLS"
                && edge.source == "app.py::make"
                && edge.target == "app.py::Repo.find"
        }),
        "{edges:?}"
    );
    assert!(!edges.iter().any(|edge| {
        edge.kind == "CALLS" && edge.source == "app.py::make" && edge.target == "app.py::IRepo.find"
    }));
}

#[test]
fn records_python_import_names_and_import_receivers() {
    let source = br#"from pkg._core import fast_sum, Store as RustStore
from pkg import _core
import pkg._core as core2
import os

def run(xs):
    core2.helper(xs)
    _core.other(xs)
    os.getenv("X")
    return fast_sum(xs)
"#;
    let (_, edges) = parse_python("pkg/app.py", source);
    let import_extra = |module: &str| {
        edges
            .iter()
            .find(|edge| edge.kind == "IMPORTS_FROM" && edge.extra["module"] == module)
            .map(|edge| edge.extra.clone())
            .unwrap_or_else(|| panic!("no import of {module}"))
    };
    assert_eq!(
        import_extra("pkg._core")["names"],
        serde_json::json!([["fast_sum", "fast_sum"], ["Store", "RustStore"]])
    );
    assert_eq!(
        import_extra("pkg")["names"],
        serde_json::json!([["_core", "_core"]])
    );
    let aliased = edges
        .iter()
        .find(|edge| edge.kind == "IMPORTS_FROM" && edge.extra["alias"] == "core2")
        .expect("aliased import");
    assert_eq!(aliased.extra["module"], "pkg._core");

    let receiver = |target: &str| {
        edges
            .iter()
            .find(|edge| edge.kind == "CALLS" && edge.target == target)
            .map(|edge| edge.extra.get("receiver").cloned())
            .unwrap_or_else(|| panic!("no call to {target}"))
    };
    assert_eq!(receiver("helper"), Some(serde_json::json!("core2")));
    assert_eq!(receiver("other"), Some(serde_json::json!("_core")));
    assert_eq!(receiver("getenv"), Some(serde_json::json!("os")));
    assert_eq!(receiver("fast_sum"), None);
}

#[test]
fn python_ffi_bridge_sees_through_import_aliases() {
    let source = br#"from ctypes import CDLL
import ctypes as ct

def load_a():
    return CDLL("./libfoo.so")

def load_b():
    return ct.cdll.LoadLibrary("./libbar.so")
"#;
    let (_, edges) = parse_python("app.py", source);
    let bridges = edges
        .iter()
        .filter(|edge| edge.kind == "CROSS_ARTIFACT")
        .map(|edge| {
            (
                edge.source.as_str(),
                edge.target.as_str(),
                edge.extra["evidence_source"].as_str().unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();
    assert!(bridges.contains(&("app.py::load_a", "./libfoo.so", "ctypes.CDLL")));
    assert!(bridges.contains(&("app.py::load_b", "./libbar.so", "ctypes.cdll.LoadLibrary")));
}

#[test]
fn python_wasm_hosts_emit_loads_and_export_calls() {
    let source = br#"from wasmtime import Module
def run(store, instance):
    module = Module.from_file(store.engine, "guest.wasm")
    add = instance.exports(store).get("add")
    return instance.exports.mul(1, 2)
"#;
    let mut parser = RustOwnedParser::new();
    let (_, edges) = parser.parse_file("tools/run.py", source);
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
        ]
    );
}
