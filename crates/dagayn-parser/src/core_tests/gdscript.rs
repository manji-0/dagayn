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
        edge.kind == "IMPORTS_FROM"
            && edge.source == "sample.gd"
            && edge.target == "godot"
            && edge.extra["external_symbol"] == "Node"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.gd"
            && edge.target == "godot"
            && edge.extra["external_symbol"] == "preload"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.gd::_ready"
            && edge.target == "sample.gd::_load_items"
    }));
    // `timer: Timer`: the engine's `Timer.start`.
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.gd::_ready"
            && edge.target == "godot"
            && edge.extra["external_symbol"] == "Timer.start"
    }));
    // `items: Array[Item]`: the engine's `Array.append`.
    assert!(edges.iter().any(|edge| {
        edge.kind == "CALLS"
            && edge.source == "sample.gd::_load_items"
            && edge.target == "godot"
            && edge.extra["external_symbol"] == "Array.append"
    }));
    assert!(edges.iter().any(|edge| {
        edge.kind == "CONTAINS"
            && edge.source == "sample.gd::Item"
            && edge.target == "sample.gd::Item.promote"
    }));
}

#[test]
fn gdscript_standard_library_calls_target_their_package() {
    let source = br#"extends CharacterBody2D

const Enemy = preload("res://enemy.gd")
const Timer = preload("res://my_timer.gd")

func lerp(a, b, t):
	return a


func _ready() -> void:
	var v := Vector2(1, 2)
	print(v)
	var n = Node2D.new()
	randi()
	lerp(1, 2, 0.5)
	Enemy.new()
	Timer.new()
	n.queue_free()
"#;
    let (_nodes, edges) = parse_gdscript("scripts/player.gd", source);
    let find = |kind: &str, symbol: &str| {
        edges
            .iter()
            .find(|edge| {
                edge.kind == kind
                    && (edge.extra["external_symbol"] == symbol
                        || (edge.target == symbol && edge.extra.get("external_symbol").is_none()))
            })
            .unwrap_or_else(|| panic!("no {kind} {symbol} in {edges:#?}"))
    };
    // `extends` of an engine class: the engine, likely (it is not imported).
    let extends = find("IMPORTS_FROM", "CharacterBody2D");
    assert_eq!(extends.target, "godot");
    assert_eq!(extends.extra["stdlib"], true);
    assert_eq!(extends.extra["confidence_tier"], "MEDIUM");
    // Global functions, built-in types, and engine classes.
    for symbol in ["Vector2", "print", "Node2D.new", "randi", "preload"] {
        let call = find("CALLS", symbol);
        assert_eq!(call.target, "godot", "{symbol}");
        assert_eq!(call.extra["confidence_tier"], "MEDIUM", "{symbol}");
    }
    // `res://` preloads stay paths; the script's own `lerp` and constants
    // bound by `preload` are not the engine's; a method of a variable is
    // its type's (`n = Node2D.new()`: `Node2D.queue_free`).
    let preload = find("IMPORTS_FROM", "res://enemy.gd");
    assert!(preload.extra.get("stdlib").is_none());
    let lerp = find("CALLS", "scripts/player.gd::lerp");
    assert!(lerp.extra.get("stdlib").is_none());
    let constructors = edges
        .iter()
        .filter(|edge| edge.kind == "CALLS" && edge.target == "new")
        .count();
    assert_eq!(constructors, 2, "{edges:#?}");
    assert_eq!(find("CALLS", "Node2D.queue_free").target, "godot");
    assert!(
        edges
            .iter()
            .all(|edge| edge.extra.get("gdscript_call_path").is_none()),
        "{edges:#?}"
    );
}

#[test]
fn gdscript_receivers_record_the_call_they_came_from() {
    let source = b"class_name Repo\nvar store: Store\nfunc make() -> Store:\n\treturn Store.new()\nfunc save():\n\tpass\nfunc run(p: Store, q):\n\tvar s := Store.new()\n\ts.save()\n\tvar r: Repo = get_repo()\n\tr.save()\n\tvar c = make()\n\tc.query(1)\n\tmake().close()\n\tself.save()\n\tstore.save()\n\tq.save()\n\t$Label.set_text(\"x\")\n\tStore.create()\n\tself.store.flush()\n";
    let (nodes, edges) = parse_gdscript("repo.gd", source);
    let make = nodes.iter().find(|node| node.name == "make").expect("make");
    assert_eq!(make.return_type.as_deref(), Some("Store"));
    let call = |target: &str, line: i64| {
        edges
            .iter()
            .find(|edge| edge.kind == "CALLS" && edge.target == target && edge.line == line)
            .unwrap_or_else(|| panic!("no {target} at {line} in {edges:?}"))
    };
    // `Store` is a class of another script.
    assert_eq!(call("save", 9).extra["receiver_type"], "Store");
    // `Repo` is this script's.
    assert!(
        call("repo.gd::save", 11)
            .extra
            .get("receiver_type")
            .is_none()
    );
    assert_eq!(
        call("query", 13).extra["receiver_from"],
        serde_json::json!({"call": "make", "line": 12, "unwrap": false})
    );
    assert_eq!(
        call("close", 14).extra["receiver_from"],
        serde_json::json!({"call": "make", "line": 14, "unwrap": false})
    );
    assert!(
        call("repo.gd::save", 15)
            .extra
            .get("receiver_unknown")
            .is_none()
    );
    assert_eq!(call("save", 16).extra["receiver_type"], "Store");
    assert_eq!(call("save", 17).extra["receiver_unknown"], true);
    assert_eq!(call("set_text", 18).extra["receiver_unknown"], true);
    assert_eq!(call("create", 19).extra["receiver_type"], "Store");
    assert_eq!(call("flush", 20).extra["receiver_type"], "Store");
}
