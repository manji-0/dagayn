use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use super::member_calls::{CallOrigin, MemberCallBindings};

use super::stdlib::perl::{is_perl_builtin, is_perl_core_module, perl_default_exports};
use super::stdlib::{StdlibEvidence, mark_stdlib_edge};
use super::types::{FilePath, ParsedEdge, ParsedNode};
use super::util::{
    direct_child, direct_child_text, first_descendant_text, line_count, line_of, node_text,
    strip_matching_quotes,
};
use super::{add_tested_by_edges, is_test_function, qualify};

pub(super) fn parse_perl_with_parser(
    file_path: &str,
    source: &[u8],
    parser: Option<&mut tree_sitter::Parser>,
) -> (Vec<ParsedNode>, Vec<ParsedEdge>) {
    let file_path = FilePath::new(file_path);
    let line_end = line_count(source);
    let mut nodes = vec![ParsedNode::file(&file_path, line_end, "perl")];
    let mut edges = Vec::new();
    let context = PerlParseContext {
        source,
        file_path: file_path.clone(),
        imports: RefCell::default(),
        packages: HashSet::new(),
        bindings: RefCell::default(),
    };

    if let Some(parser) = parser
        && let Some(tree) = parser.parse(source, None)
    {
        let mut context = context;
        perl_collect_packages(tree.root_node(), source, &mut context.packages);
        context.bindings = RefCell::new(MemberCallBindings::with_types(context.packages.clone()));
        perl_walk_children(
            tree.root_node(),
            &context,
            None,
            None,
            &mut nodes,
            &mut edges,
        );
        perl_mark_stdlib_calls(&nodes, &mut edges, &context.imports.borrow());
        let mut edges = resolve_perl_call_targets(&nodes, edges, &file_path);
        add_tested_by_edges(&nodes, &mut edges);
        return (nodes, edges);
    }

    (nodes, edges)
}

struct PerlParseContext<'a> {
    source: &'a [u8],
    file_path: FilePath,
    /// Sub names a `use` brings into the file, with the module they come
    /// from (`floor` → `POSIX` after `use POSIX qw(floor)`).
    imports: RefCell<HashMap<String, String>>,
    /// Packages the file declares (`package Store;`).
    packages: HashSet<String>,
    /// The classes of the variables in scope (`my $s = Store->new`), and
    /// the calls they hold the result of (`my $c = connect()`).
    bindings: RefCell<MemberCallBindings>,
}

/// Walks `node`, tracking the current package. A `package X;` statement
/// switches the package for the rest of its enclosing block, while
/// `package X { ... }` scopes it to the block. `main` is the default package
/// and is left unqualified.
fn perl_walk_children(
    node: tree_sitter::Node<'_>,
    context: &PerlParseContext<'_>,
    package: Option<&str>,
    enclosing_func: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut current: Option<String> = package.map(str::to_string);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let package = current.as_deref();
        match child.kind() {
            "use_statement" if enclosing_func.is_none() => {
                perl_emit_use(child, context, package, edges);
                continue;
            }
            "require_expression" if enclosing_func.is_none() => {
                if let Some(target) =
                    direct_child_text(child, context.source, &["bareword", "package"])
                {
                    perl_push_import(child, context, target, edges);
                }
                continue;
            }
            "package_statement" | "class_statement" | "role_statement" => {
                if let Some(name) = perl_package_name(child, context.source) {
                    let scoped = (name != "main").then_some(name.as_str());
                    if let Some(name) = scoped {
                        perl_emit_class(child, context, name, nodes, edges);
                    }
                    if let Some(block) = direct_child(child, &["block"]) {
                        perl_walk_children(block, context, scoped, enclosing_func, nodes, edges);
                    } else {
                        current = scoped.map(str::to_string);
                    }
                }
                continue;
            }
            "subroutine_declaration_statement" | "method_declaration_statement" => {
                if let Some(name) = perl_subroutine_name(child, context.source) {
                    perl_emit_function(child, context, &name, package, nodes, edges);
                    let scope = match package {
                        Some(package) => format!("{package}.{name}"),
                        None => name.clone(),
                    };
                    let saved = context.bindings.borrow().snapshot();
                    perl_walk_children(child, context, package, Some(&scope), nodes, edges);
                    context.bindings.borrow_mut().restore(saved);
                }
                continue;
            }
            "assignment_expression" if enclosing_func.is_none() => {
                perl_emit_isa_assignment(child, context, package, edges);
            }
            "function_call_expression"
            | "ambiguous_function_call_expression"
            | "method_call_expression"
            | "anonymous_function_call_expression" => {
                if let Some(call_name) = perl_call_name(child, context.source) {
                    perl_emit_call(child, context, &call_name, enclosing_func, edges);
                }
            }
            _ => {}
        }
        perl_walk_children(child, context, package, enclosing_func, nodes, edges);
        // After its value is walked: `$s = $s->next` calls `next` on the
        // previous `$s`.
        if child.kind() == "assignment_expression" {
            perl_bind_assignment(child, context);
        }
    }
}

const PERL_PRAGMAS: &[&str] = &[
    "strict",
    "warnings",
    "utf8",
    "feature",
    "lib",
    "constant",
    "vars",
    "integer",
    "overload",
    "version",
    "experimental",
    "diagnostics",
    "bytes",
    "locale",
    "open",
];

fn perl_push_import(
    node: tree_sitter::Node<'_>,
    context: &PerlParseContext<'_>,
    target: String,
    edges: &mut Vec<ParsedEdge>,
) {
    let mut edge = ParsedEdge::new(
        crate::core::types::EdgeKind::ImportsFrom,
        context.file_path.to_string(),
        target,
        context.file_path.clone(),
        line_of(node),
    );
    if is_perl_core_module(&edge.target) {
        let package = edge.target.clone();
        mark_stdlib_edge(
            &mut edge.target,
            &mut edge.extra,
            &package,
            StdlibEvidence::Certain,
        );
    }
    edges.push(edge);
}

/// `use Module ...` imports the module; `use parent`/`use base` declare
/// superclasses of the current package instead. Pragmas (`strict`,
/// `warnings`, `constant`) change how the file compiles and import nothing,
/// so they make no edge, standard library or not.
fn perl_emit_use(
    node: tree_sitter::Node<'_>,
    context: &PerlParseContext<'_>,
    package: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(module) = direct_child_text(node, context.source, &["package"]) else {
        return;
    };
    if matches!(module.as_str(), "parent" | "base") {
        let bases = perl_string_values(node, context.source);
        perl_emit_inherits(node, context, package, bases, module.as_str(), edges);
        return;
    }
    if PERL_PRAGMAS.contains(&module.as_str()) {
        return;
    }
    perl_record_imported_names(node, context, &module);
    perl_push_import(node, context, module, edges);
}

/// The subs `use Module LIST` imports: the words of its list, or what the
/// module exports by default when there is none (`use Data::Dumper;`
/// brings `Dumper`). `use Module ()` imports nothing, and a tag (`:all`)
/// names no sub.
fn perl_record_imported_names(
    node: tree_sitter::Node<'_>,
    context: &PerlParseContext<'_>,
    module: &str,
) {
    let mut cursor = node.walk();
    let has_list = node
        .named_children(&mut cursor)
        .any(|child| !matches!(child.kind(), "package" | "version" | "comment"));
    let names = if has_list {
        perl_string_values(node, context.source)
    } else {
        perl_default_exports(module)
            .iter()
            .map(|name| name.to_string())
            .collect()
    };
    let mut imports = context.imports.borrow_mut();
    for name in names {
        let name = name.trim_start_matches('&');
        if perl_is_sub_name(name) && !name.contains("::") {
            imports.insert(name.to_string(), module.to_string());
        }
    }
}

fn perl_emit_isa_assignment(
    node: tree_sitter::Node<'_>,
    context: &PerlParseContext<'_>,
    package: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(left) = node.child(0) else {
        return;
    };
    if first_descendant_text(left, context.source, &["varname"]).as_deref() != Some("ISA") {
        return;
    }
    let bases = perl_string_values(node, context.source);
    perl_emit_inherits(node, context, package, bases, "@ISA", edges);
}

fn perl_emit_inherits(
    node: tree_sitter::Node<'_>,
    context: &PerlParseContext<'_>,
    package: Option<&str>,
    bases: Vec<String>,
    evidence: &str,
    edges: &mut Vec<ParsedEdge>,
) {
    let Some(package) = package else {
        return;
    };
    for base in bases {
        edges.push(ParsedEdge {
            kind: crate::core::types::EdgeKind::Inherits,
            source: qualify(&context.file_path, package, None),
            target: base,
            file_path: context.file_path.clone(),
            line: node.start_position().row as i64 + 1,
            extra: json!({"relationship_role": "extends", "syntax_source": evidence}),
        });
    }
}

/// Every string literal or `qw(...)` word under `node`.
fn perl_string_values(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let mut values = Vec::new();
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        match current.kind() {
            "string_literal" | "interpolated_string_literal" => {
                values.extend(perl_string_text(current, source));
            }
            "quoted_word_list" => {
                let text =
                    first_descendant_text(current, source, &["string_content"]).unwrap_or_default();
                values.extend(text.split_whitespace().map(str::to_string));
            }
            _ => {
                let mut cursor = current.walk();
                let children: Vec<_> = current.children(&mut cursor).collect();
                stack.extend(children.into_iter().rev());
            }
        }
    }
    values
}

fn perl_emit_class(
    node: tree_sitter::Node<'_>,
    context: &PerlParseContext<'_>,
    name: &str,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let qualified = qualify(&context.file_path, name, None);
    nodes.push(ParsedNode {
        kind: crate::core::types::NodeKind::Class,
        name: name.to_string(),
        file_path: context.file_path.clone(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "perl".to_string(),
        parent_name: None,
        params: None,
        return_type: None,
        modifiers: None,
        is_test: false,
        extra: json!({"type_role": "class"}),
    });
    edges.push(ParsedEdge::new(
        crate::core::types::EdgeKind::Contains,
        context.file_path.to_string(),
        qualified,
        context.file_path.clone(),
        line_of(node),
    ));
}

fn perl_emit_function(
    node: tree_sitter::Node<'_>,
    context: &PerlParseContext<'_>,
    name: &str,
    package: Option<&str>,
    nodes: &mut Vec<ParsedNode>,
    edges: &mut Vec<ParsedEdge>,
) {
    let is_test = is_test_function(name, &context.file_path, node, context.source);
    let qualified = qualify(&context.file_path, name, package);
    nodes.push(ParsedNode {
        kind: if is_test {
            crate::core::types::NodeKind::Test
        } else {
            crate::core::types::NodeKind::Function
        },
        name: name.to_string(),
        file_path: context.file_path.clone(),
        line_start: node.start_position().row as i64 + 1,
        line_end: node.end_position().row as i64 + 1,
        language: "perl".to_string(),
        parent_name: package.map(str::to_string),
        params: None,
        return_type: None,
        modifiers: None,
        is_test,
        extra: json!({}),
    });
    edges.push(ParsedEdge::new(
        crate::core::types::EdgeKind::Contains,
        package
            .map(|package| qualify(&context.file_path, package, None))
            .unwrap_or_else(|| context.file_path.to_string()),
        qualified,
        context.file_path.clone(),
        line_of(node),
    ));
}

fn perl_emit_call(
    node: tree_sitter::Node<'_>,
    context: &PerlParseContext<'_>,
    call_name: &str,
    enclosing_func: Option<&str>,
    edges: &mut Vec<ParsedEdge>,
) {
    let caller = enclosing_func
        .map(|func| qualify(&context.file_path, func, None))
        .unwrap_or_else(|| context.file_path.to_string());
    // `$obj->print(...)`: a method of a value of unknown class, never a
    // builtin. Read (and dropped) by `perl_mark_stdlib_calls`.
    let on_value =
        node.kind() == "method_call_expression" && direct_child(node, &["bareword"]).is_none();
    let mut target = call_name.to_string();
    let mut extra = if on_value {
        json!({"stdlib_method": true})
    } else {
        json!({})
    };
    if on_value {
        perl_mark_receiver(node, context, &mut target, &mut extra);
    }
    edges.push(ParsedEdge {
        kind: crate::core::types::EdgeKind::Calls,
        source: caller.clone(),
        target,
        file_path: context.file_path.clone(),
        line: node.start_position().row as i64 + 1,
        extra,
    });
    // `$obj->exec(...)` is a method, not the builtin.
    if !on_value && let Some(edge) = perl_bridge_edge(node, context, &caller, call_name) {
        edges.push(edge);
    }
}

/// Names of the packages declared anywhere in the file.
fn perl_collect_packages(
    node: tree_sitter::Node<'_>,
    source: &[u8],
    packages: &mut HashSet<String>,
) {
    if matches!(
        node.kind(),
        "package_statement" | "class_statement" | "role_statement"
    ) && let Some(name) = perl_package_name(node, source)
    {
        packages.insert(name);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        perl_collect_packages(child, source, packages);
    }
}

/// Invocants that are the enclosing package or an instance of it.
fn perl_is_self_invocant(text: &str) -> bool {
    matches!(text, "$self" | "$this" | "$class" | "shift" | "__PACKAGE__")
}

/// The class a `Class->new(...)` constructs (`Store`, `My::Store`).
fn perl_constructed_class(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    if node.kind() != "method_call_expression" {
        return None;
    }
    let class = direct_child_text(node, source, &["bareword"])?;
    (direct_child_text(node, source, &["method"])? == "new").then_some(class)
}

/// The call an expression is the result of (`connect($dsn)`,
/// `$db->handle`), or of which a variable holds the result. In a chain
/// repeating `method` (`$q->where(a)->where(b)`) it is the call before the
/// repeats, since the repeats share one edge per line.
fn perl_call_origin(
    expression: tree_sitter::Node<'_>,
    method: Option<&str>,
    context: &PerlParseContext<'_>,
) -> Option<CallOrigin> {
    let source = context.source;
    let name = match expression.kind() {
        "method_call_expression" => direct_child_text(expression, source, &["method"])?,
        "function_call_expression" | "ambiguous_function_call_expression" => {
            let name = perl_call_name(expression, source)?;
            name.rsplit("::").next().unwrap_or(&name).to_string()
        }
        "scalar" => {
            return context
                .bindings
                .borrow()
                .returned_by(&node_text(expression, source))
                .cloned();
        }
        _ => return None,
    };
    if expression.kind() == "method_call_expression" && method == Some(name.as_str()) {
        let invocant = expression.child_by_field_name("invocant")?;
        return perl_call_origin(invocant, method, context);
    }
    Some(CallOrigin {
        name,
        line: expression.start_position().row as i64 + 1,
        unwrap: false,
    })
}

/// Binds the scalar an assignment sets to what its value says: the class
/// it constructs (`my $s = Store->new`), the scalar it copies, or the call
/// it is the result of (`my $c = connect($dsn)`).
fn perl_bind_assignment(node: tree_sitter::Node<'_>, context: &PerlParseContext<'_>) {
    let source = context.source;
    let Some(left) = node.child_by_field_name("left") else {
        return;
    };
    let scalar = match left.kind() {
        "scalar" => left,
        "variable_declaration" => match left.child_by_field_name("variable") {
            Some(variable) if variable.kind() == "scalar" => variable,
            _ => return,
        },
        _ => return,
    };
    let var = node_text(scalar, source);
    let Some(right) = node.child_by_field_name("right") else {
        return;
    };
    if let Some(class) = perl_constructed_class(right, source) {
        let mut bindings = context.bindings.borrow_mut();
        if context.packages.contains(&class) {
            bindings.forget_foreign(&var);
            bindings.bind(var, class);
        } else {
            bindings.bind_any(var, class);
        }
        return;
    }
    if right.kind() == "scalar" {
        let other = node_text(right, source);
        let mut bindings = context.bindings.borrow_mut();
        if let Some(bound) = bindings.bound_type(&other).map(str::to_string) {
            bindings.bind(var, bound);
            return;
        }
        if let Some(foreign) = bindings.foreign_type(&other).map(str::to_string) {
            bindings.bind_any(var, foreign);
            return;
        }
    }
    let origin = perl_call_origin(right, None, context);
    let mut bindings = context.bindings.borrow_mut();
    match origin {
        Some(origin) => bindings.bind_returned(var, origin),
        None => bindings.forget_foreign(&var),
    }
}

/// Rewrites a method call on a value by what its invocant says: an
/// instance of a package of this file calls the package's sub
/// (`$s->save` after `my $s = Store->new` is `Store::save`); of a package
/// of another file, the bare method with `receiver_type`; of an unknown
/// class (a parameter, `$self->{db}`, a call's result), the bare method
/// with `receiver_unknown` (and `receiver_from` for a call's result).
/// `$self`, `$class`, and `shift` are the enclosing package.
fn perl_mark_receiver(
    node: tree_sitter::Node<'_>,
    context: &PerlParseContext<'_>,
    target: &mut String,
    extra: &mut Value,
) {
    let source = context.source;
    let Some(invocant) = node.child_by_field_name("invocant") else {
        return;
    };
    let text = node_text(invocant, source);
    if perl_is_self_invocant(text.trim()) {
        return;
    }
    let class = match invocant.kind() {
        "scalar" => {
            let bindings = context.bindings.borrow();
            bindings
                .bound_type(&text)
                .or_else(|| bindings.foreign_type(&text))
                .map(str::to_string)
        }
        _ => perl_constructed_class(invocant, source),
    };
    match class {
        Some(class) if context.packages.contains(&class) => {
            *target = format!("{class}::{target}");
        }
        Some(class) => extra["receiver_type"] = json!(class),
        None => {
            extra["receiver_unknown"] = json!(true);
            if let Some(origin) = perl_call_origin(invocant, Some(target.as_str()), context) {
                extra["receiver_from"] = origin.to_json();
            }
        }
    }
}

fn perl_bridge_edge(
    node: tree_sitter::Node<'_>,
    context: &PerlParseContext<'_>,
    caller: &str,
    call_name: &str,
) -> Option<ParsedEdge> {
    let (relationship_role, bridge_kind) = match call_name {
        "system" | "exec" => ("invokes_binary", "subprocess"),
        "open" => ("opens_file", "file_io"),
        "File::Slurp::read_file" => ("reads_file", "file_io"),
        "File::Slurp::write_file" => ("writes_file", "file_io"),
        "DynaLoader::dl_load_file" => ("loads_shared_library", "ffi"),
        _ => return None,
    };
    let line = node.start_position().row as i64 + 1;
    let (target, confidence, confidence_tier) = match perl_first_string_arg(node, context.source) {
        Some(target) => (target, 0.8, "HIGH"),
        None => (
            format!("<dynamic:{call_name}@{}:{line}>", context.file_path),
            0.2,
            "LOW",
        ),
    };
    Some(ParsedEdge {
        kind: crate::core::types::EdgeKind::CrossArtifact,
        source: caller.to_string(),
        target,
        file_path: context.file_path.clone(),
        line,
        extra: json!({
            "relationship_role": relationship_role,
            "bridge_kind": bridge_kind,
            "evidence_kind": "syntax",
            "evidence_source": call_name,
            "source_language": "perl",
            "target_language": "unknown",
            "confidence": confidence,
            "confidence_tier": confidence_tier,
        }),
    })
}

fn perl_package_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();

    node.children(&mut cursor)
        .find(|child| child.is_named() && child.kind() == "package")
        .map(|child| node_text(child, source))
}

fn perl_subroutine_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    direct_child_text(node, source, &["bareword", "identifier"])
}

fn perl_call_name(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    if node.kind() == "method_call_expression" {
        let method = direct_child_text(node, source, &["method"])?;
        // `Class->method` names its package; `$obj->method` does not.
        return Some(match direct_child_text(node, source, &["bareword"]) {
            Some(class) => format!("{class}::{method}"),
            None => method,
        });
    }
    let name = direct_child_text(node, source, &["function", "bareword", "identifier"])?;
    // Error recovery can wrap a whole expression in a `function` node
    // (`input_avail && do { ... }`); only a sub name is a callee.
    let name = name.trim().trim_start_matches('&');
    perl_is_sub_name(name).then(|| name.to_string())
}

/// `name`, `Pkg::name`, `::name` (Perl identifiers are word characters).
fn perl_is_sub_name(name: &str) -> bool {
    let name = name.strip_prefix("::").unwrap_or(name);
    !name.is_empty()
        && name.split("::").all(|segment| {
            !segment.is_empty()
                && !segment.starts_with(|c: char| c.is_ascii_digit())
                && segment.chars().all(|c| c.is_alphanumeric() || c == '_')
        })
}

fn perl_first_string_arg(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let mut skipped_callee = false;
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "function" | "method") && !skipped_callee {
            skipped_callee = true;
            continue;
        }
        if matches!(child.kind(), "," | "(" | ")") {
            continue;
        }
        if matches!(
            child.kind(),
            "interpolated_string_literal" | "string_literal" | "quoted_word_list"
        ) {
            return perl_string_text(child, source);
        }
        if child.is_named() {
            return None;
        }
    }
    None
}

fn perl_string_text(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    first_descendant_text(node, source, &["string_content"])
        .or_else(|| Some(strip_matching_quotes(node_text(node, source).trim()).to_string()))
        .filter(|value| !value.is_empty())
}

/// Points the calls into Perl's standard library at it: a sub of a core
/// module named through it (`POSIX::floor`, `File::Spec->catfile`) or
/// imported from it (`floor` after `use POSIX qw(floor)`) at the module,
/// certain; a builtin (`print`, `push`, `join`) at `CORE`, likely unless the
/// file declares a sub of that name or imports one from another module.
/// `CORE::say` is certain. A package this file declares is its own.
fn perl_mark_stdlib_calls(
    nodes: &[ParsedNode],
    edges: &mut [ParsedEdge],
    imports: &HashMap<String, String>,
) {
    let defined_subs = nodes
        .iter()
        .filter(|node| matches!(node.kind.as_str(), "Function" | "Test"))
        .map(|node| node.name.as_str())
        .collect::<HashSet<_>>();
    let defined_packages = nodes
        .iter()
        .filter(|node| node.kind.as_str() == "Class")
        .map(|node| node.name.as_str())
        .collect::<HashSet<_>>();
    for edge in edges.iter_mut() {
        if edge.kind != crate::core::types::EdgeKind::Calls {
            continue;
        }
        let on_value = edge
            .extra
            .as_object_mut()
            .and_then(|extra| extra.remove("stdlib_method"))
            .is_some();
        if on_value {
            continue;
        }
        let (package, evidence) = match edge.target.rsplit_once("::") {
            Some((module, _)) if module == "CORE" || module.starts_with("CORE::") => {
                ("CORE".to_string(), StdlibEvidence::Certain)
            }
            Some((module, _))
                if is_perl_core_module(module) && !defined_packages.contains(module) =>
            {
                (module.to_string(), StdlibEvidence::Certain)
            }
            Some(_) => continue,
            None => match imports.get(&edge.target) {
                Some(module) if is_perl_core_module(module) => {
                    edge.target = format!("{module}::{}", edge.target);
                    (module.clone(), StdlibEvidence::Certain)
                }
                Some(_) => continue,
                None if is_perl_builtin(&edge.target)
                    && !defined_subs.contains(edge.target.as_str()) =>
                {
                    ("CORE".to_string(), StdlibEvidence::Likely)
                }
                None => continue,
            },
        };
        mark_stdlib_edge(&mut edge.target, &mut edge.extra, &package, evidence);
    }
}

fn resolve_perl_call_targets(
    nodes: &[ParsedNode],
    edges: Vec<ParsedEdge>,
    file_path: &FilePath,
) -> Vec<ParsedEdge> {
    // `Pkg::name` -> qualified node, plus bare names grouped by package.
    let mut by_path = HashMap::<String, String>::new();
    let mut by_name = HashMap::<String, Vec<(Option<String>, String)>>::new();
    for node in nodes
        .iter()
        .filter(|node| matches!(node.kind.as_str(), "Function" | "Test"))
    {
        let qualified = qualify(file_path, &node.name, node.parent_name.as_deref());
        if let Some(package) = node.parent_name.as_deref() {
            by_path
                .entry(format!("{package}::{}", node.name))
                .or_insert_with(|| qualified.clone());
        }
        by_name
            .entry(node.name.clone())
            .or_default()
            .push((node.parent_name.clone(), qualified));
    }
    let prefix = format!("{file_path}::");
    edges
        .into_iter()
        .map(|mut edge| {
            if edge.kind != "CALLS" || edge.extra["external"] == true {
                return edge;
            }
            // A method of a value (`$conn->close`) is none of the file's
            // subs, unless its class is a package of the file, which
            // `perl_mark_receiver` already wrote as `Store::save`.
            if edge.extra["receiver_unknown"] == true || edge.extra.get("receiver_type").is_some() {
                return edge;
            }
            if let Some(target) = by_path.get(&edge.target) {
                edge.target = target.clone();
            } else if !edge.target.contains("::")
                && let Some(candidates) = by_name.get(&edge.target)
            {
                let caller_package = edge
                    .source
                    .strip_prefix(&prefix)
                    .and_then(|rest| rest.rsplit_once('.').map(|(package, _)| package));
                let chosen = candidates
                    .iter()
                    .find(|(package, _)| package.as_deref() == caller_package)
                    .unwrap_or(&candidates[0]);
                edge.target = chosen.1.clone();
            }
            edge
        })
        .collect()
}
