//! Compiled grammar provisioning for the Rust parser.
//!
//! This crate compiles the same pinned grammar sources that the Python parser
//! path provisions through `dagayn.vendor_grammars`.

use tree_sitter_language::LanguageFn;

extern "C" {
    fn tree_sitter_markdown() -> *const ();
    fn tree_sitter_terraform() -> *const ();
    fn tree_sitter_rust() -> *const ();
    fn tree_sitter_python() -> *const ();
    fn tree_sitter_javascript() -> *const ();
    fn tree_sitter_typescript() -> *const ();
    fn tree_sitter_tsx() -> *const ();
    fn tree_sitter_bash() -> *const ();
    fn tree_sitter_go() -> *const ();
    fn tree_sitter_java() -> *const ();
    fn tree_sitter_ruby() -> *const ();
    fn tree_sitter_c_sharp() -> *const ();
    fn tree_sitter_php() -> *const ();
    fn tree_sitter_kotlin() -> *const ();
    fn tree_sitter_scala() -> *const ();
    fn tree_sitter_solidity() -> *const ();
    fn tree_sitter_dart() -> *const ();
    fn tree_sitter_lua() -> *const ();
    fn tree_sitter_luau() -> *const ();
    fn tree_sitter_c() -> *const ();
    fn tree_sitter_cpp() -> *const ();
    fn tree_sitter_objc() -> *const ();
    fn tree_sitter_elixir() -> *const ();
    fn tree_sitter_gdscript() -> *const ();
    fn tree_sitter_r() -> *const ();
    fn tree_sitter_julia() -> *const ();
}

pub const MARKDOWN_LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_markdown) };
pub const TERRAFORM_LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_terraform) };
pub const RUST_LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_rust) };
pub const PYTHON_LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_python) };
pub const JAVASCRIPT_LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_javascript) };
pub const TYPESCRIPT_LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_typescript) };
pub const TSX_LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_tsx) };
pub const BASH_LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_bash) };
pub const GO_LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_go) };
pub const JAVA_LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_java) };
pub const RUBY_LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_ruby) };
pub const CSHARP_LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_c_sharp) };
pub const PHP_LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_php) };
pub const KOTLIN_LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_kotlin) };
pub const SCALA_LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_scala) };
pub const SOLIDITY_LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_solidity) };
pub const DART_LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_dart) };
pub const LUA_LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_lua) };
pub const LUAU_LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_luau) };
pub const C_LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_c) };
pub const CPP_LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_cpp) };
pub const OBJC_LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_objc) };
pub const ELIXIR_LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_elixir) };
pub const GDSCRIPT_LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_gdscript) };
pub const R_LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_r) };
pub const JULIA_LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_julia) };

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GrammarStatus {
    Ready,
}

pub fn status() -> GrammarStatus {
    GrammarStatus::Ready
}

pub fn markdown_language() -> tree_sitter::Language {
    MARKDOWN_LANGUAGE.into()
}

pub fn terraform_language() -> tree_sitter::Language {
    TERRAFORM_LANGUAGE.into()
}

pub fn rust_language() -> tree_sitter::Language {
    RUST_LANGUAGE.into()
}

pub fn python_language() -> tree_sitter::Language {
    PYTHON_LANGUAGE.into()
}

pub fn javascript_language() -> tree_sitter::Language {
    JAVASCRIPT_LANGUAGE.into()
}

pub fn typescript_language() -> tree_sitter::Language {
    TYPESCRIPT_LANGUAGE.into()
}

pub fn tsx_language() -> tree_sitter::Language {
    TSX_LANGUAGE.into()
}

pub fn bash_language() -> tree_sitter::Language {
    BASH_LANGUAGE.into()
}

pub fn go_language() -> tree_sitter::Language {
    GO_LANGUAGE.into()
}

pub fn java_language() -> tree_sitter::Language {
    JAVA_LANGUAGE.into()
}

pub fn ruby_language() -> tree_sitter::Language {
    RUBY_LANGUAGE.into()
}

pub fn csharp_language() -> tree_sitter::Language {
    CSHARP_LANGUAGE.into()
}

pub fn php_language() -> tree_sitter::Language {
    PHP_LANGUAGE.into()
}

pub fn kotlin_language() -> tree_sitter::Language {
    KOTLIN_LANGUAGE.into()
}

pub fn scala_language() -> tree_sitter::Language {
    SCALA_LANGUAGE.into()
}

pub fn solidity_language() -> tree_sitter::Language {
    SOLIDITY_LANGUAGE.into()
}

pub fn dart_language() -> tree_sitter::Language {
    DART_LANGUAGE.into()
}

pub fn lua_language() -> tree_sitter::Language {
    LUA_LANGUAGE.into()
}

pub fn luau_language() -> tree_sitter::Language {
    LUAU_LANGUAGE.into()
}

pub fn c_language() -> tree_sitter::Language {
    C_LANGUAGE.into()
}

pub fn cpp_language() -> tree_sitter::Language {
    CPP_LANGUAGE.into()
}

pub fn objc_language() -> tree_sitter::Language {
    OBJC_LANGUAGE.into()
}

pub fn elixir_language() -> tree_sitter::Language {
    ELIXIR_LANGUAGE.into()
}

pub fn gdscript_language() -> tree_sitter::Language {
    GDSCRIPT_LANGUAGE.into()
}

pub fn r_language() -> tree_sitter::Language {
    R_LANGUAGE.into()
}

pub fn julia_language() -> tree_sitter::Language {
    JULIA_LANGUAGE.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_markdown_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&markdown_language())
            .expect("load pinned Markdown grammar");
        let tree = parser.parse("# Heading\n", None).expect("parse Markdown");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn loads_terraform_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&terraform_language())
            .expect("load pinned Terraform grammar");
        let tree = parser
            .parse("resource \"aws_s3_bucket\" \"main\" {}\n", None)
            .expect("parse Terraform");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn loads_rust_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&rust_language())
            .expect("load pinned Rust grammar");
        let tree = parser.parse("fn main() {}\n", None).expect("parse Rust");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn loads_python_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&python_language())
            .expect("load pinned Python grammar");
        let tree = parser
            .parse("def main():\n    return 1\n", None)
            .expect("parse Python");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn loads_javascript_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&javascript_language())
            .expect("load pinned JavaScript grammar");
        let tree = parser
            .parse("export function main() { return 1; }\n", None)
            .expect("parse JavaScript");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn loads_typescript_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&typescript_language())
            .expect("load pinned TypeScript grammar");
        let tree = parser
            .parse(
                "export function main(value: number): number { return value; }\n",
                None,
            )
            .expect("parse TypeScript");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn loads_tsx_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tsx_language())
            .expect("load pinned TSX grammar");
        let tree = parser
            .parse("export const View = () => <div />;\n", None)
            .expect("parse TSX");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn loads_bash_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&bash_language())
            .expect("load pinned Bash grammar");
        let tree = parser
            .parse("main() { echo hi; }\nmain\n", None)
            .expect("parse Bash");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn loads_go_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&go_language())
            .expect("load pinned Go grammar");
        let tree = parser
            .parse("package main\nfunc main() { println(\"hi\") }\n", None)
            .expect("parse Go");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn loads_java_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&java_language())
            .expect("load pinned Java grammar");
        let tree = parser
            .parse(
                "class Main { void run() { System.out.println(\"hi\"); } }\n",
                None,
            )
            .expect("parse Java");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn loads_ruby_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&ruby_language())
            .expect("load pinned Ruby grammar");
        let tree = parser
            .parse(
                "class User\n  def save\n    puts \"ok\"\n  end\nend\n",
                None,
            )
            .expect("parse Ruby");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn loads_csharp_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&csharp_language())
            .expect("load pinned C# grammar");
        let tree = parser
            .parse(
                "class User { void Save() { System.Console.WriteLine(\"ok\"); } }\n",
                None,
            )
            .expect("parse C#");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn loads_php_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&php_language())
            .expect("load pinned PHP grammar");
        let tree = parser
            .parse(
                "<?php\nclass User { function save() { echo \"ok\"; } }\n",
                None,
            )
            .expect("parse PHP");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn loads_kotlin_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&kotlin_language())
            .expect("load pinned Kotlin grammar");
        let tree = parser.parse("fun main() {}\n", None).expect("parse Kotlin");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn loads_scala_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&scala_language())
            .expect("load pinned Scala grammar");
        let tree = parser
            .parse("class User:\n  def save(): Unit = println(\"ok\")\n", None)
            .expect("parse Scala");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn loads_solidity_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&solidity_language())
            .expect("load pinned Solidity grammar");
        let tree = parser
            .parse("contract Vault { function stake() external {} }\n", None)
            .expect("parse Solidity");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn loads_dart_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&dart_language())
            .expect("load pinned Dart grammar");
        let tree = parser
            .parse("class Dog { void bark() { print('woof'); } }\n", None)
            .expect("parse Dart");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn loads_lua_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&lua_language())
            .expect("load pinned Lua grammar");
        let tree = parser
            .parse("function greet(name)\n  print(name)\nend\n", None)
            .expect("parse Lua");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn loads_luau_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&luau_language())
            .expect("load pinned Luau grammar");
        let tree = parser
            .parse("type Callback = (input: string) -> string\n", None)
            .expect("parse Luau");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn loads_c_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&c_language())
            .expect("load pinned C grammar");
        let tree = parser
            .parse("int main() { return 0; }\n", None)
            .expect("parse C");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn loads_cpp_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&cpp_language())
            .expect("load pinned C++ grammar");
        let tree = parser
            .parse("class Dog { public: void bark() {} };\n", None)
            .expect("parse C++");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn loads_objc_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&objc_language())
            .expect("load pinned Objective-C grammar");
        let tree = parser
            .parse("@interface Calculator : NSObject\n@end\n", None)
            .expect("parse Objective-C");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn loads_elixir_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&elixir_language())
            .expect("load pinned Elixir grammar");
        let tree = parser
            .parse("defmodule Calculator do\nend\n", None)
            .expect("parse Elixir");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn loads_gdscript_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&gdscript_language())
            .expect("load pinned GDScript grammar");
        let tree = parser
            .parse(
                "extends Node\nclass_name Player\nfunc _ready():\n\tpass\n",
                None,
            )
            .expect("parse GDScript");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn loads_r_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&r_language())
            .expect("load pinned R grammar");
        let tree = parser
            .parse("add <- function(x, y) {\n  x + y\n}\n", None)
            .expect("parse R");
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn loads_julia_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&julia_language())
            .expect("load pinned Julia grammar");
        let tree = parser
            .parse("module Sample\nfunction greet()\nend\nend\n", None)
            .expect("parse Julia");
        assert!(!tree.root_node().has_error());
    }
}
