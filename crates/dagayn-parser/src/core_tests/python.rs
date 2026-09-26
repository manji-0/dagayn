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
