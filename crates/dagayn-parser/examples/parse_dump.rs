//! Print the nodes and edges the parser produces for each file argument, or
//! with `--tree`, the tree-sitter syntax tree.
//!
//! ```text
//! cargo run -p dagayn-parser --example parse_dump -- [--tree] FILE...
//! ```

fn language(path: &str) -> Option<tree_sitter::Language> {
    use dagayn_grammars::*;
    let ext = path.rsplit('.').next()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "rs" => rust_language(),
        "js" | "mjs" | "cjs" | "jsx" => javascript_language(),
        "ts" | "mts" | "cts" => typescript_language(),
        "tsx" => tsx_language(),
        "go" => go_language(),
        "java" => java_language(),
        "rb" => ruby_language(),
        "cs" => csharp_language(),
        "php" => php_language(),
        "kt" | "kts" => kotlin_language(),
        "scala" => scala_language(),
        "dart" => dart_language(),
        "lua" => lua_language(),
        "c" | "h" => c_language(),
        "cc" | "cpp" | "hpp" => cpp_language(),
        "m" => objc_language(),
        "ex" | "exs" => elixir_language(),
        "gd" => gdscript_language(),
        "r" => r_language(),
        "jl" => julia_language(),
        "pl" | "pm" => perl_language(),
        "zig" => zig_language(),
        "swift" => swift_language(),
        "sh" | "bash" => bash_language(),
        "tf" => terraform_language(),
        _ => return None,
    })
}

fn print_tree(node: tree_sitter::Node<'_>, field: Option<&str>, source: &[u8], depth: usize) {
    let field = field.map(|f| format!("{f}: ")).unwrap_or_default();
    let text = if node.child_count() == 0 {
        format!(" {:?}", node.utf8_text(source).unwrap_or(""))
    } else {
        String::new()
    };
    if node.is_named() || node.child_count() == 0 && depth > 0 {
        println!("{}{field}{}{text}", "  ".repeat(depth), node.kind());
    }
    let mut cursor = node.walk();
    for (index, child) in node.children(&mut cursor).enumerate() {
        print_tree(
            child,
            node.field_name_for_child(index as u32),
            source,
            depth + 1,
        );
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let tree = args.iter().any(|a| a == "--tree");
    let mut parser = dagayn_parser::RustOwnedParser::new();
    for path in args.iter().filter(|a| *a != "--tree") {
        let source = std::fs::read(path).expect("readable file");
        println!("== {path}");
        if tree {
            let mut ts = tree_sitter::Parser::new();
            ts.set_language(&language(path).expect("known extension"))
                .expect("language");
            let parsed = ts.parse(&source, None).expect("tree");
            print_tree(parsed.root_node(), None, &source, 0);
            continue;
        }
        let (nodes, edges) = parser.parse_file(path, &source);
        for node in &nodes {
            let parent = node
                .parent_name
                .as_deref()
                .map(|p| format!("{p}."))
                .unwrap_or_default();
            let test = if node.is_test { " [test]" } else { "" };
            println!(
                "  {:?} {parent}{} L{}-{}{test}",
                node.kind, node.name, node.line_start, node.line_end
            );
        }
        for edge in &edges {
            println!(
                "  {:?} {} -> {} L{}",
                edge.kind, edge.source, edge.target, edge.line
            );
        }
    }
}
