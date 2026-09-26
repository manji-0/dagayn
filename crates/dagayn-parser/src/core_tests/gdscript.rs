use super::*;

#[test]
fn parses_gdscript_classes_functions_imports_and_calls() {
    let source = br#"extends Node
class_name SampleManager

const MAX_SIZE = 10
const OtherScript = preload("res://scripts/other.gd")

signal item_added(item: Item)

@export var speed: float = 2.5
@onready var timer: Timer = $Timer

var items: Array[Item] = []


class Item:
	var name: String
	var level: int

	func promote() -> void:
		level += 1


func _ready() -> void:
	timer.start()
	_load_items()
	OtherScript.register(self)


func _load_items() -> void:
	for i in range(MAX_SIZE):
		var item := Item.new()
		items.append(item)
		item_added.emit(item)


func get_item(idx: int) -> Item:
	return items[idx]


static func helper() -> int:
	return 42
"#;
    let (nodes, edges) = parse_gdscript("sample.gd", source);
    assert!(nodes.iter().any(|node| {
        node.kind == "Class"
            && node.name == "SampleManager"
            && node.language == "gdscript"
            && node.extra["type_role"] == "class"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Class" && node.name == "Item" && node.language == "gdscript"
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "promote"
            && node.parent_name.as_deref() == Some("Item")
            && node.params.as_deref() == Some("()")
            && node.return_type.as_deref() == Some("void")
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function" && node.name == "_load_items" && node.parent_name.is_none()
    }));
    assert!(nodes.iter().any(|node| {
        node.kind == "Function"
            && node.name == "get_item"
            && node.params.as_deref() == Some("(idx: int)")
            && node.return_type.as_deref() == Some("Item")
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "IMPORTS_FROM" && edge.source == "sample.gd" && edge.target == "Node"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS" && edge.source == "sample.gd" && edge.target == "preload"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.gd::_ready"
            && edge.target == "sample.gd::_load_items"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS" && edge.source == "sample.gd::_ready" && edge.target == "start"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS" && edge.source == "sample.gd::_load_items" && edge.target == "append"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CONTAINS"
            && edge.source == "sample.gd::Item"
            && edge.target == "sample.gd::Item.promote"
    }));
}
