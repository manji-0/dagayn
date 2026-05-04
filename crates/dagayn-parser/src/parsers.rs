pub(super) fn new_terraform_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::terraform_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

pub(super) fn new_markdown_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::markdown_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

pub(super) fn new_rust_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::rust_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

pub(super) fn new_python_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::python_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

pub(super) fn new_javascript_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::javascript_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

pub(super) fn new_typescript_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::typescript_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

pub(super) fn new_tsx_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::tsx_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

pub(super) fn new_bash_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::bash_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

pub(super) fn new_go_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&dagayn_grammars::go_language()).is_ok() {
        Some(parser)
    } else {
        None
    }
}

pub(super) fn new_java_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::java_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

pub(super) fn new_ruby_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::ruby_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

pub(super) fn new_csharp_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::csharp_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

pub(super) fn new_php_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::php_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

pub(super) fn new_kotlin_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::kotlin_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

pub(super) fn new_scala_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::scala_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

pub(super) fn new_solidity_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::solidity_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

pub(super) fn new_dart_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::dart_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

pub(super) fn new_lua_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::lua_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

pub(super) fn new_luau_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::luau_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

pub(super) fn new_c_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&dagayn_grammars::c_language()).is_ok() {
        Some(parser)
    } else {
        None
    }
}

pub(super) fn new_cpp_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::cpp_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

pub(super) fn new_objc_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::objc_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

pub(super) fn new_elixir_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::elixir_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

pub(super) fn new_gdscript_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::gdscript_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

pub(super) fn new_r_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&dagayn_grammars::r_language()).is_ok() {
        Some(parser)
    } else {
        None
    }
}

pub(super) fn new_julia_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::julia_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

pub(super) fn new_perl_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::perl_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

pub(super) fn new_vue_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::vue_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

pub(super) fn new_svelte_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::svelte_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

pub(super) fn new_zig_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::zig_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

pub(super) fn new_powershell_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::powershell_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}

pub(super) fn new_swift_parser() -> Option<tree_sitter::Parser> {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&dagayn_grammars::swift_language())
        .is_ok()
    {
        Some(parser)
    } else {
        None
    }
}
