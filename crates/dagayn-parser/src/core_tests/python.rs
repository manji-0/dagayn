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
    // A call into the standard library targets its package.
    assert_eq!(receiver("os"), Some(serde_json::json!("os")));
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

#[test]
fn a_file_mid_edit_yields_no_multi_line_symbols() {
    // An unterminated string swallows the following lines.
    let source = b"import \"os\nfrom x import (\ndef f():\n    return 1\n";
    let mut parser = RustOwnedParser::new();
    let (nodes, edges) = parser.parse_file("mid_edit.py", source);
    assert!(nodes.iter().all(|node| !node.name.contains('\n')));
    assert!(
        edges
            .iter()
            .all(|edge| !edge.target.trim().is_empty() && !edge.target.contains('\n'))
    );
}

#[test]
fn an_unparseable_notebook_keeps_its_file_node() {
    let mut parser = RustOwnedParser::new();
    let (nodes, _) = parser.parse_file("draft.ipynb", b"{\"cells\": [");
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].kind, NodeKind::File);
}

#[test]
fn python_standard_library_calls_target_their_package() {
    let source = br#"import importlib
import subprocess
from pathlib import Path
from os.path import join

def run(cmd):
    plugin = importlib.import_module(cmd)
    plugin.read_text()
    subprocess.run(cmd)
    Path("x").read_bytes()
    p = Path("y")
    p.read_text()
    p = make()
    p.read_text()
    join("a", "b")
    len(cmd)
    sorted(cmd)
    open(cmd).read()
    format.upper()

def read_bytes():
    pass

def read_text():
    pass

def sorted(xs):
    pass
"#;
    let (_, edges) = parse_python("app.py", source);
    let calls = edges
        .iter()
        .filter(|edge| edge.kind == "CALLS" && edge.source == "app.py::run")
        .map(|edge| {
            (
                edge.target.as_str(),
                edge.extra["external_symbol"].as_str().unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();
    // `p` was reassigned and `plugin` is any module: receivers of unknown
    // type, never this file's `read_text`.
    let unknown = edges
        .iter()
        .filter(|edge| edge.kind == "CALLS" && edge.target == "read_text")
        .filter(|edge| edge.extra["receiver_unknown"] == true)
        .count();
    assert_eq!(unknown, 2, "{calls:?}");
    assert!(!calls.contains(&("app.py::read_text", "")), "{calls:?}");
    for expected in [
        ("subprocess", "subprocess.run"),
        ("pathlib", "pathlib.Path.read_bytes"),
        ("pathlib", "pathlib.Path"),
        ("pathlib", "pathlib.Path.read_text"),
        ("os", "os.path.join"),
        ("builtins", "len"),
        ("builtins", "open.read"),
        // A builtin the file shadows is its own function.
        ("app.py::sorted", ""),
        // A variable named like a builtin is not one.
        ("upper", ""),
        // `import_module` returns any module, not one of the standard library.
        ("importlib", "importlib.import_module"),
    ] {
        assert!(calls.contains(&expected), "{expected:?} not in {calls:?}");
    }
    // Rooted at an import: certain; a builtin or a bound variable: likely.
    let tier = |symbol: &str| {
        edges
            .iter()
            .find(|edge| edge.extra["external_symbol"] == symbol)
            .map(|edge| edge.extra["confidence_tier"].clone())
            .unwrap_or_else(|| panic!("no call to {symbol}"))
    };
    assert_eq!(tier("subprocess.run"), "HIGH");
    assert_eq!(tier("pathlib.Path.read_bytes"), "HIGH");
    assert_eq!(tier("os.path.join"), "HIGH");
    assert_eq!(tier("pathlib.Path.read_text"), "MEDIUM");
    assert_eq!(tier("len"), "MEDIUM");
    // The standard library's `run` / `read_bytes` are not this file's.
    assert!(!calls.contains(&("app.py::run", "")), "{calls:?}");
    assert!(!calls.contains(&("app.py::read_bytes", "")), "{calls:?}");
    assert!(
        edges
            .iter()
            .filter(|edge| edge.extra["external"] == true)
            .all(|edge| edge.extra["external_package"] == edge.target.as_str())
    );

    let import = edges
        .iter()
        .find(|edge| edge.kind == "IMPORTS_FROM" && edge.extra["module"] == "os.path")
        .expect("import of os.path");
    assert_eq!(import.target, "os");
    assert_eq!(import.extra["external"], true);
}

#[test]
fn python_standard_library_calls_are_not_tested_by() {
    let source = br#"import json

def test_dump():
    json.dumps({})
    len([])
"#;
    let (_, edges) = parse_python("tests/test_app.py", source);
    assert!(
        edges
            .iter()
            .any(|edge| edge.kind == "CALLS" && edge.target == "json")
    );
    assert!(
        !edges.iter().any(|edge| edge.kind == "TESTED_BY"),
        "{edges:?}"
    );
}

#[test]
fn python_receivers_are_typed_by_annotations_and_attributes() {
    let source = br#"from pathlib import Path
from typing import Any
from app.graph import GraphStore

class Repo:
    def save(self):
        pass

class Service:
    cache: Repo

    def __init__(self, store: GraphStore, root: Path):
        self.store = store
        self.root = root
        self.local = Repo()

    def run(self, names: list[str], anything: Any, maybe: GraphStore | None):
        self.store.upsert_node()
        self.root.read_text()
        self.local.save()
        self.cache.save()
        names.append("x")
        anything.save()
        maybe.commit()
        other = GraphStore()
        other.close()
"#;
    let (_, edges) = parse_python("app/service.py", source);
    let calls = edges
        .iter()
        .filter(|edge| edge.kind == "CALLS" && edge.source == "app/service.py::Service.run")
        .map(|edge| {
            (
                edge.target.as_str(),
                edge.extra["receiver_type"].as_str().unwrap_or_default(),
                edge.extra["receiver_unknown"] == true,
                edge.extra["external_symbol"].as_str().unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();
    for expected in [
        // A class of another module: resolved across files by its type.
        ("upsert_node", "GraphStore", false, ""),
        ("commit", "GraphStore", false, ""),
        ("close", "GraphStore", false, ""),
        // Standard-library types of an attribute or a parameter.
        ("pathlib", "", false, "pathlib.Path.read_text"),
        ("builtins", "", false, "list.append"),
        // A class of this file, through `self.x = Repo()` and a class-body
        // annotation.
        ("app/service.py::Repo.save", "", false, ""),
        // `Any` says nothing about methods.
        ("save", "", true, ""),
    ] {
        assert!(calls.contains(&expected), "{expected:?} not in {calls:?}");
    }
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.0 == "app/service.py::Repo.save")
            .count(),
        2,
        "{calls:?}"
    );
}

#[test]
fn python_third_party_calls_target_their_package() {
    // `yaml` and `numpy` are in neither the repository nor the standard
    // library; `app` is the repository's own package.
    let mut root = std::env::temp_dir();
    root.push(format!("dagayn-python-third-party-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("app")).unwrap();
    std::fs::write(root.join("app/__init__.py"), "").unwrap();
    std::fs::write(root.join("app/util.py"), "def helper():\n    pass\n").unwrap();
    let source = b"import yaml\nimport numpy as np\nfrom pytest import raises\nfrom app.util import helper\n\ndef run():\n    yaml.safe_load('x')\n    np.array([1])\n    raises(ValueError)\n    helper()\n";
    std::fs::write(root.join("app/main.py"), source).unwrap();
    let mut parser = RustOwnedParser::new();
    let (_, edges) = parser.parse_file_in_repo(Some(&root), "app/main.py", source);
    let external = edges
        .iter()
        .filter(|edge| edge.extra["external"] == true)
        .map(|edge| {
            (
                edge.kind.as_str(),
                edge.target.as_str(),
                edge.extra["external_symbol"].as_str().unwrap_or_default(),
                edge.extra["confidence_tier"].as_str().unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();
    for expected in [
        ("IMPORTS_FROM", "yaml", "", "MEDIUM"),
        ("IMPORTS_FROM", "numpy", "", "MEDIUM"),
        ("CALLS", "yaml", "yaml.safe_load", "MEDIUM"),
        ("CALLS", "numpy", "numpy.array", "MEDIUM"),
        ("CALLS", "pytest", "pytest.raises", "MEDIUM"),
    ] {
        assert!(
            external.contains(&expected),
            "{expected:?} not in {external:?}"
        );
    }
    assert!(
        !edges
            .iter()
            .any(|edge| edge.extra["external"] == true && edge.extra["stdlib"] == true),
        "{external:?}"
    );
    assert!(
        !external.iter().any(|(_, target, _, _)| *target == "app"),
        "{external:?}"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn python_lazy_exports_are_imports() {
    let mut root = std::env::temp_dir();
    root.push(format!("dagayn-python-lazy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("pkg")).unwrap();
    std::fs::write(root.join("pkg/types.py"), "class Node:\n    pass\n").unwrap();
    std::fs::write(root.join("pkg/core.py"), "class Parser:\n    pass\n").unwrap();
    let source = b"_LAZY = {\n    \"Node\": (\".types\", \"Node\"),\n    \"CodeParser\": (\".core\", \"Parser\"),\n}\n\ndef __getattr__(name):\n    module_name, attr_name = _LAZY[name]\n    return attr_name\n";
    std::fs::write(root.join("pkg/__init__.py"), source).unwrap();
    let mut parser = RustOwnedParser::new();
    let (_, edges) = parser.parse_file_in_repo(Some(&root), "pkg/__init__.py", source);
    let imports = edges
        .iter()
        .filter(|edge| edge.kind == "IMPORTS_FROM")
        .map(|edge| (edge.target.as_str(), edge.extra["names"].clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        imports,
        vec![
            ("pkg/types.py", serde_json::json!([["Node", "Node"]])),
            ("pkg/core.py", serde_json::json!([["Parser", "CodeParser"]])),
        ]
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn python_imports_record_whether_they_run_on_import() {
    let source = br#"import os
from typing import TYPE_CHECKING
import typing

if TYPE_CHECKING:
    from app.models import User
else:
    from app.fallback import User

if typing.TYPE_CHECKING:
    import app.types

class Service:
    import json

    def run(self):
        from app.cli import main

        def inner():
            import app.inner

def top():
    import app.lazy
"#;
    let (_, edges) = parse_python("app/service.py", source);
    let scopes = edges
        .iter()
        .filter(|edge| edge.kind == "IMPORTS_FROM")
        .map(|edge| {
            (
                edge.line,
                edge.extra.get("import_scope").and_then(|s| s.as_str()),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        scopes,
        vec![
            (1, None),
            (2, None),
            (3, None),
            (6, Some("type_checking")),
            (8, None),
            (11, Some("type_checking")),
            (14, None),
            (17, Some("function")),
            (20, Some("function")),
            (23, Some("function")),
        ],
        "{edges:?}"
    );
}

#[test]
fn python_typed_receivers_never_bind_to_another_class_of_the_file() {
    // `store: GraphStore` of another module: its `close` is not the
    // `close` of a class this file declares.
    let source = b"from app.graph import GraphStore\n\nclass _NoClose:\n    def close(self):\n        pass\n\ndef run(store: GraphStore):\n    store.close()\n";
    let (_, edges) = parse_python("tests/test_store.py", source);
    let call = edges
        .iter()
        .find(|edge| edge.kind == "CALLS" && edge.source == "tests/test_store.py::run")
        .expect("call");
    assert_eq!(call.target, "close");
    assert_eq!(call.extra["receiver_type"], "GraphStore");
}

#[test]
fn python_pytest_fixtures_type_their_parameters() {
    let source = br#"import pytest

def test_run(tmp_path, monkeypatch, capsys, other):
    tmp_path.mkdir()
    monkeypatch.setattr("a.b", 1)
    capsys.readouterr()
    other.setattr("x", 1)

def helper(mp: pytest.MonkeyPatch):
    mp.setenv("A", "1")
"#;
    let (_, edges) = parse_python("tests/test_app.py", source);
    let calls = edges
        .iter()
        .filter(|edge| edge.kind == "CALLS")
        .map(|edge| {
            (
                edge.target.as_str(),
                edge.extra["external_symbol"].as_str().unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();
    for expected in [
        ("pathlib", "pathlib.Path.mkdir"),
        ("pytest", "pytest.MonkeyPatch.setattr"),
        ("pytest", "pytest.CaptureFixture.readouterr"),
        // No fixture of that name: unknown.
        ("setattr", ""),
    ] {
        assert!(calls.contains(&expected), "{expected:?} not in {calls:?}");
    }
    // Outside a test file, a parameter named like a fixture is not one.
    let (_, edges) = parse_python("app/run.py", b"def run(tmp_path):\n    tmp_path.mkdir()\n");
    assert!(
        edges
            .iter()
            .any(|edge| edge.kind == "CALLS" && edge.target == "mkdir"),
        "{edges:?}"
    );
}

#[test]
fn python_relative_imports_resolve_calls() {
    // `from .graph import helper` binds `helper` as `from pkg.graph import
    // helper` does.
    let mut root = std::env::temp_dir();
    root.push(format!("dagayn-python-relative-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("pkg")).unwrap();
    std::fs::write(root.join("pkg/__init__.py"), "").unwrap();
    std::fs::write(root.join("pkg/graph.py"), "def helper():\n    pass\n").unwrap();
    let source = b"from .graph import helper\n\ndef run():\n    helper()\n";
    std::fs::write(root.join("pkg/app.py"), source).unwrap();
    let mut parser = RustOwnedParser::new();
    let (_, edges) = parser.parse_file_in_repo(Some(&root), "pkg/app.py", source);
    assert!(
        edges
            .iter()
            .any(|edge| edge.kind == "CALLS" && edge.target == "pkg/graph.py::helper"),
        "{edges:?}"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn python_receivers_record_the_call_they_came_from() {
    let source = b"def run(store):\n    store_conn(store).execute('x')\n    conn = store_conn(store)\n    conn.commit()\n    store.pool().get()\n";
    let (_, edges) = parse_python("app.py", source);
    let from = |method: &str| {
        edges
            .iter()
            .find(|edge| edge.kind == "CALLS" && edge.target == method)
            .map(|edge| edge.extra["receiver_from"].clone())
            .unwrap_or_else(|| panic!("no {method}"))
    };
    assert_eq!(
        from("execute"),
        serde_json::json!({"call": "store_conn", "line": 2, "unwrap": false})
    );
    assert_eq!(
        from("commit"),
        serde_json::json!({"call": "store_conn", "line": 3, "unwrap": false})
    );
    assert_eq!(
        from("get"),
        serde_json::json!({"call": "pool", "line": 5, "unwrap": false})
    );
}

#[test]
fn python_super_calls_are_typed_by_the_base_class_never_the_callers_own() {
    // `super().__init__(name)` in `AuthService(BaseService)` is
    // `BaseService.__init__`, not `AuthService.__init__` itself; with no
    // base it is `object`'s, so it binds to nothing in the file.
    let source = br#"class BaseService:
    def __init__(self, name):
        self.name = name

class AuthService(BaseService):
    def __init__(self, name):
        super().__init__(name)

class Plain:
    def __init__(self):
        super().__init__()
"#;
    let (_, edges) = parse_python("svc.py", source);
    let init_calls = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::Calls && edge.target.ends_with("__init__"))
        .map(|edge| {
            (
                edge.source.as_str(),
                edge.target.as_str(),
                edge.extra["receiver_type"].as_str(),
                edge.extra["receiver_unknown"].as_bool(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        init_calls,
        vec![
            (
                "svc.py::AuthService.__init__",
                "__init__",
                Some("BaseService"),
                None
            ),
            ("svc.py::Plain.__init__", "__init__", None, Some(true)),
        ]
    );
}

#[test]
fn python_calls_on_an_imported_module_never_bind_to_a_same_file_method() {
    // `query_module.get_impact_radius()` is the module's function, not the
    // `get_impact_radius` of a stub class the test declares.
    let source = br#"from dagayn.tools import query as query_module

class _DummyStore:
    def get_impact_radius(self):
        return {}

def test_budget():
    query_module.get_impact_radius()
"#;
    let (_, edges) = parse_python("tests/test_tools.py", source);
    let call = edges
        .iter()
        .find(|edge| edge.kind == EdgeKind::Calls && edge.target.ends_with("get_impact_radius"))
        .expect("call");
    assert_ne!(
        call.target,
        "tests/test_tools.py::_DummyStore.get_impact_radius"
    );
    assert_eq!(call.extra["receiver"], "query_module");
}

fn python_definitions(nodes: &[ParsedNode]) -> Vec<(&str, &str, Option<&str>, i64, i64)> {
    nodes
        .iter()
        .filter(|node| node.kind != NodeKind::File)
        .map(|node| {
            (
                node.kind.as_str(),
                node.name.as_str(),
                node.parent_name.as_deref(),
                node.line_start,
                node.line_end,
            )
        })
        .collect()
}

fn python_calls(edges: &[ParsedEdge]) -> Vec<(&str, &str)> {
    edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::Calls)
        .map(|edge| (edge.source.as_str(), edge.target.as_str()))
        .collect()
}

#[test]
fn python_definitions_after_a_syntax_error_are_kept() {
    let source = br#"import os

def ok():
    return os.path.join("a")

def broken(x:
    foo(

class After:
    def m(self):
        bar()
"#;
    let (nodes, edges) = parse_python("app.py", source);
    let definitions = python_definitions(&nodes)
        .into_iter()
        .map(|(kind, name, parent, _, _)| (kind, name, parent))
        .collect::<Vec<_>>();
    assert_eq!(
        definitions,
        vec![
            ("Function", "ok", None),
            ("Function", "broken", None),
            ("Class", "After", None),
            ("Function", "m", Some("After")),
        ]
    );
    let calls = python_calls(&edges);
    assert!(calls.contains(&("app.py::broken", "foo")));
    assert!(calls.contains(&("app.py::After.m", "bar")));
}

#[test]
fn python_statements_after_an_unexpected_indent_keep_their_scope() {
    // The stray indented line nests the rest of the file in `f` in Ruff's
    // tree; the definitions after it are walked where their indentation
    // puts them.
    let source = br#"class A:
    def f(self):
        x = 1
            y = 2
    def g(self):
        helper()

def helper():
    pass
"#;
    let (nodes, edges) = parse_python("app.py", source);
    assert_eq!(
        python_definitions(&nodes),
        vec![
            ("Class", "A", None, 1, 6),
            ("Function", "f", Some("A"), 2, 4),
            ("Function", "g", Some("A"), 5, 6),
            ("Function", "helper", None, 8, 9),
        ]
    );
    assert!(python_calls(&edges).contains(&("app.py::A.g", "app.py::helper")));
}

#[test]
fn python_decorated_definitions_start_at_their_keyword() {
    let source = br#"import functools

@functools.cache
# a comment between
@register(
    "name",
)
async def load():
    return 1

@dataclass
class Item:
    name: str
"#;
    let (nodes, edges) = parse_python("app.py", source);
    assert_eq!(
        python_definitions(&nodes),
        vec![
            ("Function", "load", None, 8, 9),
            ("Class", "Item", None, 12, 13)
        ]
    );
    let load = nodes.iter().find(|node| node.name == "load").unwrap();
    assert_eq!(
        load.extra["decorators"],
        json!(["functools.cache", "register"])
    );
    // The decorator's call runs in the module, not in `load`.
    assert!(python_calls(&edges).contains(&("app.py", "register")));
}

#[test]
fn python_string_arguments_are_read_decoded() {
    let source = br#"def run(root):
    open("C:\\data\\in.csv")
    open("out" ".csv")
    open(f"{root}/x.txt")
"#;
    let (_, edges) = parse_python("app.py", source);
    let targets = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::CrossArtifact)
        .map(|edge| edge.target.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        targets,
        vec![
            r"C:\data\in.csv",
            "out.csv",
            // An interpolated f-string names no fixed file.
            "<dynamic:open@app.py:4>",
        ]
    );
}

#[test]
fn marimo_sql_split_over_concatenated_literals_is_one_query() {
    let source = br#"import marimo

app = marimo.App()

@app.cell
def _(mo):
    _df = mo.sql("SELECT * FROM sales" ".orders")
    return (_df,)
"#;
    let mut parser = RustOwnedParser::new();
    let (_, edges) = parser.parse_file("notebook.py", source);
    let tables = edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::ImportsFrom && edge.line == 1)
        .map(|edge| edge.target.as_str())
        .collect::<Vec<_>>();
    assert_eq!(tables, vec!["sales.orders"]);
}

#[test]
fn python_union_annotations_type_receivers() {
    let source = br#"def lookup(table: dict[str, int] | None, key):
    return table.get(key)
"#;
    let (_, edges) = parse_python("app.py", source);
    let call = edges
        .iter()
        .find(|edge| edge.kind == EdgeKind::Calls && edge.line == 2)
        .expect("call");
    assert_eq!(call.extra["external_symbol"], "dict.get");
}

#[test]
fn python_future_imports_are_not_dependencies() {
    let source = b"from __future__ import annotations\n\nx = {\"a\": annotations}\n";
    let (_, edges) = parse_python("app.py", source);
    assert!(
        edges.iter().all(|edge| edge.kind == EdgeKind::Contains),
        "{edges:?}"
    );
}

#[test]
fn python_same_code_ignores_comments_and_layout() {
    let before =
        "    def total(values, sep):\n        return sep.join(values) + str(len(values))\n";
    let after = "    def total(\n        values,\n        sep,\n    ):\n        # joined\n        return (\n            sep.join(values)\n            + str(len(values))\n        )\n";
    assert_eq!(python_same_code(before, after), Some(true));
    let changed =
        "    def total(values, sep):\n        return sep.join(values) + str(len(values) + 1)\n";
    assert_eq!(python_same_code(before, changed), Some(false));
    assert_eq!(python_same_code(before, "def broken(:\n"), None);
    let documented = "    def total(values, sep):\n        \"\"\"Join and count.\"\"\"\n        return sep.join(values) + str(len(values))\n";
    assert_eq!(python_same_code(before, documented), Some(true));
    let reworded = documented.replace("Join and count.", "Join, then count.");
    assert_eq!(python_same_code(documented, &reworded), Some(true));
}

#[test]
fn functions_passed_as_arguments_are_references() {
    // dagayn: tests crates/dagayn-parser/src/python/mod.rs::PythonWalker.visit_expr
    // dagayn: tests crates/dagayn-parser/src/python/mod.rs::PythonWalker.emit_argument_references
    // dagayn: tests crates/dagayn-parser/src/python/mod.rs::PythonWalker.emit_value_reference
    // dagayn: tests crates/dagayn-parser/src/python/mod.rs::python_keep_checked_references
    let source = br#"import threading


def run_guarded(args, action):
    return action()


def main(args):
    def dispatch():
        return 1

    def other():
        return 2

    for _, action in [("other", other)]:
        action()
    return run_guarded(args, dispatch)


class Daemon:
    def __init__(self):
        self.store = None

    def start(self):
        threading.Thread(target=self._loop, daemon=True).start()
        print(self.store)
        consume(self.store)

    def _loop(self):
        pass
"#;
    let (_, edges) = parse_python("app.py", source);
    let references: Vec<(&str, &str)> = edges
        .iter()
        .filter(|edge| edge.kind == "REFERENCES")
        .map(|edge| (edge.source.as_str(), edge.target.as_str()))
        .collect();
    assert!(
        references.contains(&("app.py::main", "app.py::main.dispatch")),
        "{references:?}"
    );
    assert!(
        references.contains(&("app.py::main", "app.py::main.other")),
        "{references:?}"
    );
    assert!(
        references.contains(&("app.py::Daemon.start", "app.py::Daemon._loop")),
        "{references:?}"
    );
    // `self.store` is an attribute, not a method: no reference.
    assert!(
        !references
            .iter()
            .any(|(_, target)| target.ends_with("Daemon.store")),
        "{references:?}"
    );
    assert!(
        edges
            .iter()
            .all(|edge| edge.extra.get("checked_local").is_none())
    );
}

#[test]
fn calls_on_a_module_imported_in_a_function_record_its_file() {
    // dagayn: tests crates/dagayn-parser/src/python/mod.rs::PythonWalker.emit_call
    let mut repo_root = std::env::temp_dir();
    repo_root.push(format!(
        "dagayn-parser-python-module-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&repo_root);
    std::fs::create_dir_all(repo_root.join("pkg")).unwrap();
    std::fs::write(repo_root.join("pkg/__init__.py"), b"").unwrap();
    std::fs::write(
        repo_root.join("pkg/helper.py"),
        b"def run():\n    return 1\n",
    )
    .unwrap();

    let source = br#"def test_run():
    from pkg import helper

    assert helper.run() == 1
"#;
    let mut parser = RustOwnedParser::new();
    let (_, edges) = parser.parse_file_in_repo(Some(&repo_root), "tests/test_app.py", source);
    let call = edges
        .iter()
        .find(|edge| edge.kind == "CALLS" && edge.target.ends_with("run"))
        .expect("call to helper.run");
    assert_eq!(
        call.extra
            .get("module_file")
            .and_then(|value| value.as_str()),
        Some("pkg/helper.py"),
        "{call:?}"
    );
    let _ = std::fs::remove_dir_all(&repo_root);
}
