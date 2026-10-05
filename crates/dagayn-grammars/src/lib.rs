//! Compiled grammar provisioning for the Rust parser.
//!
//! This crate compiles the same pinned grammar sources that the Python parser
//! path provisions through `dagayn.vendor_grammars`.
//!
//! Each grammar sits behind its own `lang-<language>` Cargo feature (all on
//! by default through `all-languages`): with a feature off, its C sources are
//! not compiled and its `<language>_language()` function does not exist.

unsafe extern "C" {
    #[cfg(feature = "lang-markdown")]
    fn tree_sitter_markdown() -> *const ();
    #[cfg(feature = "lang-terraform")]
    fn tree_sitter_terraform() -> *const ();
    #[cfg(feature = "lang-rust")]
    fn tree_sitter_rust() -> *const ();
    #[cfg(feature = "lang-javascript")]
    fn tree_sitter_javascript() -> *const ();
    #[cfg(feature = "lang-typescript")]
    fn tree_sitter_typescript() -> *const ();
    #[cfg(feature = "lang-tsx")]
    fn tree_sitter_tsx() -> *const ();
    #[cfg(feature = "lang-bash")]
    fn tree_sitter_bash() -> *const ();
    #[cfg(feature = "lang-go")]
    fn tree_sitter_go() -> *const ();
    #[cfg(feature = "lang-java")]
    fn tree_sitter_java() -> *const ();
    #[cfg(feature = "lang-ruby")]
    fn tree_sitter_ruby() -> *const ();
    #[cfg(feature = "lang-csharp")]
    fn tree_sitter_c_sharp() -> *const ();
    #[cfg(feature = "lang-php")]
    fn tree_sitter_php() -> *const ();
    #[cfg(feature = "lang-kotlin")]
    fn tree_sitter_kotlin() -> *const ();
    #[cfg(feature = "lang-scala")]
    fn tree_sitter_scala() -> *const ();
    #[cfg(feature = "lang-dart")]
    fn tree_sitter_dart() -> *const ();
    #[cfg(feature = "lang-lua")]
    fn tree_sitter_lua() -> *const ();
    #[cfg(feature = "lang-c")]
    fn tree_sitter_c() -> *const ();
    #[cfg(feature = "lang-cpp")]
    fn tree_sitter_cpp() -> *const ();
    #[cfg(feature = "lang-objc")]
    fn tree_sitter_objc() -> *const ();
    #[cfg(feature = "lang-elixir")]
    fn tree_sitter_elixir() -> *const ();
    #[cfg(feature = "lang-gdscript")]
    fn tree_sitter_gdscript() -> *const ();
    #[cfg(feature = "lang-r")]
    fn tree_sitter_r() -> *const ();
    #[cfg(feature = "lang-julia")]
    fn tree_sitter_julia() -> *const ();
    #[cfg(feature = "lang-perl")]
    fn tree_sitter_perl() -> *const ();
    #[cfg(feature = "lang-vue")]
    fn tree_sitter_vue() -> *const ();
    #[cfg(feature = "lang-svelte")]
    fn tree_sitter_svelte() -> *const ();
    #[cfg(feature = "lang-zig")]
    fn tree_sitter_zig() -> *const ();
    #[cfg(feature = "lang-swift")]
    fn tree_sitter_swift() -> *const ();
}

#[cfg(feature = "lang-markdown")]
pub const MARKDOWN_LANGUAGE: tree_sitter_language::LanguageFn =
    unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_markdown) };
#[cfg(feature = "lang-terraform")]
pub const TERRAFORM_LANGUAGE: tree_sitter_language::LanguageFn =
    unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_terraform) };
#[cfg(feature = "lang-rust")]
pub const RUST_LANGUAGE: tree_sitter_language::LanguageFn =
    unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_rust) };
#[cfg(feature = "lang-javascript")]
pub const JAVASCRIPT_LANGUAGE: tree_sitter_language::LanguageFn =
    unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_javascript) };
#[cfg(feature = "lang-typescript")]
pub const TYPESCRIPT_LANGUAGE: tree_sitter_language::LanguageFn =
    unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_typescript) };
#[cfg(feature = "lang-tsx")]
pub const TSX_LANGUAGE: tree_sitter_language::LanguageFn =
    unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_tsx) };
#[cfg(feature = "lang-bash")]
pub const BASH_LANGUAGE: tree_sitter_language::LanguageFn =
    unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_bash) };
#[cfg(feature = "lang-go")]
pub const GO_LANGUAGE: tree_sitter_language::LanguageFn =
    unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_go) };
#[cfg(feature = "lang-java")]
pub const JAVA_LANGUAGE: tree_sitter_language::LanguageFn =
    unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_java) };
#[cfg(feature = "lang-ruby")]
pub const RUBY_LANGUAGE: tree_sitter_language::LanguageFn =
    unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_ruby) };
#[cfg(feature = "lang-csharp")]
pub const CSHARP_LANGUAGE: tree_sitter_language::LanguageFn =
    unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_c_sharp) };
#[cfg(feature = "lang-php")]
pub const PHP_LANGUAGE: tree_sitter_language::LanguageFn =
    unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_php) };
#[cfg(feature = "lang-kotlin")]
pub const KOTLIN_LANGUAGE: tree_sitter_language::LanguageFn =
    unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_kotlin) };
#[cfg(feature = "lang-scala")]
pub const SCALA_LANGUAGE: tree_sitter_language::LanguageFn =
    unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_scala) };
#[cfg(feature = "lang-dart")]
pub const DART_LANGUAGE: tree_sitter_language::LanguageFn =
    unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_dart) };
#[cfg(feature = "lang-lua")]
pub const LUA_LANGUAGE: tree_sitter_language::LanguageFn =
    unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_lua) };
#[cfg(feature = "lang-c")]
pub const C_LANGUAGE: tree_sitter_language::LanguageFn =
    unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_c) };
#[cfg(feature = "lang-cpp")]
pub const CPP_LANGUAGE: tree_sitter_language::LanguageFn =
    unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_cpp) };
#[cfg(feature = "lang-objc")]
pub const OBJC_LANGUAGE: tree_sitter_language::LanguageFn =
    unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_objc) };
#[cfg(feature = "lang-elixir")]
pub const ELIXIR_LANGUAGE: tree_sitter_language::LanguageFn =
    unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_elixir) };
#[cfg(feature = "lang-gdscript")]
pub const GDSCRIPT_LANGUAGE: tree_sitter_language::LanguageFn =
    unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_gdscript) };
#[cfg(feature = "lang-r")]
pub const R_LANGUAGE: tree_sitter_language::LanguageFn =
    unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_r) };
#[cfg(feature = "lang-julia")]
pub const JULIA_LANGUAGE: tree_sitter_language::LanguageFn =
    unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_julia) };
#[cfg(feature = "lang-perl")]
pub const PERL_LANGUAGE: tree_sitter_language::LanguageFn =
    unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_perl) };
#[cfg(feature = "lang-vue")]
pub const VUE_LANGUAGE: tree_sitter_language::LanguageFn =
    unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_vue) };
#[cfg(feature = "lang-svelte")]
pub const SVELTE_LANGUAGE: tree_sitter_language::LanguageFn =
    unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_svelte) };
#[cfg(feature = "lang-zig")]
pub const ZIG_LANGUAGE: tree_sitter_language::LanguageFn =
    unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_zig) };
#[cfg(feature = "lang-swift")]
pub const SWIFT_LANGUAGE: tree_sitter_language::LanguageFn =
    unsafe { tree_sitter_language::LanguageFn::from_raw(tree_sitter_swift) };

#[cfg(feature = "lang-markdown")]
pub fn markdown_language() -> tree_sitter::Language {
    MARKDOWN_LANGUAGE.into()
}

#[cfg(feature = "lang-terraform")]
pub fn terraform_language() -> tree_sitter::Language {
    TERRAFORM_LANGUAGE.into()
}

#[cfg(feature = "lang-rust")]
pub fn rust_language() -> tree_sitter::Language {
    RUST_LANGUAGE.into()
}

#[cfg(feature = "lang-javascript")]
pub fn javascript_language() -> tree_sitter::Language {
    JAVASCRIPT_LANGUAGE.into()
}

#[cfg(feature = "lang-typescript")]
pub fn typescript_language() -> tree_sitter::Language {
    TYPESCRIPT_LANGUAGE.into()
}

#[cfg(feature = "lang-tsx")]
pub fn tsx_language() -> tree_sitter::Language {
    TSX_LANGUAGE.into()
}

#[cfg(feature = "lang-bash")]
pub fn bash_language() -> tree_sitter::Language {
    BASH_LANGUAGE.into()
}

#[cfg(feature = "lang-go")]
pub fn go_language() -> tree_sitter::Language {
    GO_LANGUAGE.into()
}

#[cfg(feature = "lang-java")]
pub fn java_language() -> tree_sitter::Language {
    JAVA_LANGUAGE.into()
}

#[cfg(feature = "lang-ruby")]
pub fn ruby_language() -> tree_sitter::Language {
    RUBY_LANGUAGE.into()
}

#[cfg(feature = "lang-csharp")]
pub fn csharp_language() -> tree_sitter::Language {
    CSHARP_LANGUAGE.into()
}

#[cfg(feature = "lang-php")]
pub fn php_language() -> tree_sitter::Language {
    PHP_LANGUAGE.into()
}

#[cfg(feature = "lang-kotlin")]
pub fn kotlin_language() -> tree_sitter::Language {
    KOTLIN_LANGUAGE.into()
}

#[cfg(feature = "lang-scala")]
pub fn scala_language() -> tree_sitter::Language {
    SCALA_LANGUAGE.into()
}

#[cfg(feature = "lang-dart")]
pub fn dart_language() -> tree_sitter::Language {
    DART_LANGUAGE.into()
}

#[cfg(feature = "lang-lua")]
pub fn lua_language() -> tree_sitter::Language {
    LUA_LANGUAGE.into()
}

#[cfg(feature = "lang-c")]
pub fn c_language() -> tree_sitter::Language {
    C_LANGUAGE.into()
}

#[cfg(feature = "lang-cpp")]
pub fn cpp_language() -> tree_sitter::Language {
    CPP_LANGUAGE.into()
}

#[cfg(feature = "lang-objc")]
pub fn objc_language() -> tree_sitter::Language {
    OBJC_LANGUAGE.into()
}

#[cfg(feature = "lang-elixir")]
pub fn elixir_language() -> tree_sitter::Language {
    ELIXIR_LANGUAGE.into()
}

#[cfg(feature = "lang-gdscript")]
pub fn gdscript_language() -> tree_sitter::Language {
    GDSCRIPT_LANGUAGE.into()
}

#[cfg(feature = "lang-r")]
pub fn r_language() -> tree_sitter::Language {
    R_LANGUAGE.into()
}

#[cfg(feature = "lang-julia")]
pub fn julia_language() -> tree_sitter::Language {
    JULIA_LANGUAGE.into()
}

#[cfg(feature = "lang-perl")]
pub fn perl_language() -> tree_sitter::Language {
    PERL_LANGUAGE.into()
}

#[cfg(feature = "lang-vue")]
pub fn vue_language() -> tree_sitter::Language {
    VUE_LANGUAGE.into()
}

#[cfg(feature = "lang-svelte")]
pub fn svelte_language() -> tree_sitter::Language {
    SVELTE_LANGUAGE.into()
}

#[cfg(feature = "lang-zig")]
pub fn zig_language() -> tree_sitter::Language {
    ZIG_LANGUAGE.into()
}

#[cfg(feature = "lang-swift")]
pub fn swift_language() -> tree_sitter::Language {
    SWIFT_LANGUAGE.into()
}

#[cfg(test)]
mod tests {
    // Unused when every `lang-*` feature is off.
    #[allow(unused_imports)]
    use super::*;

    #[cfg(feature = "lang-markdown")]
    #[test]
    fn loads_markdown_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&markdown_language())
            .expect("load pinned Markdown grammar");
        let tree = parser.parse("# Heading\n", None).expect("parse Markdown");
        assert!(!tree.root_node().has_error());
    }

    #[cfg(feature = "lang-terraform")]
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

    #[cfg(feature = "lang-rust")]
    #[test]
    fn loads_rust_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&rust_language())
            .expect("load pinned Rust grammar");
        let tree = parser.parse("fn main() {}\n", None).expect("parse Rust");
        assert!(!tree.root_node().has_error());
    }

    #[cfg(feature = "lang-javascript")]
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

    #[cfg(feature = "lang-typescript")]
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

    #[cfg(feature = "lang-tsx")]
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

    #[cfg(feature = "lang-bash")]
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

    #[cfg(feature = "lang-go")]
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

    #[cfg(feature = "lang-java")]
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

    #[cfg(feature = "lang-ruby")]
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

    #[cfg(feature = "lang-csharp")]
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

    #[cfg(feature = "lang-php")]
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

    #[cfg(feature = "lang-kotlin")]
    #[test]
    fn loads_kotlin_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&kotlin_language())
            .expect("load pinned Kotlin grammar");
        let tree = parser.parse("fun main() {}\n", None).expect("parse Kotlin");
        assert!(!tree.root_node().has_error());
    }

    #[cfg(feature = "lang-scala")]
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

    #[cfg(feature = "lang-dart")]
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

    #[cfg(feature = "lang-lua")]
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

    #[cfg(feature = "lang-c")]
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

    #[cfg(feature = "lang-cpp")]
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

    #[cfg(feature = "lang-objc")]
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

    #[cfg(feature = "lang-elixir")]
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

    #[cfg(feature = "lang-gdscript")]
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

    #[cfg(feature = "lang-r")]
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

    #[cfg(feature = "lang-julia")]
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

    #[cfg(feature = "lang-perl")]
    #[test]
    fn loads_perl_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&perl_language())
            .expect("load pinned Perl grammar");
        let tree = parser
            .parse("package Animal;\nsub speak { return \"...\"; }\n", None)
            .expect("parse Perl");
        assert!(!tree.root_node().has_error());
    }

    #[cfg(feature = "lang-vue")]
    #[test]
    fn loads_vue_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&vue_language())
            .expect("load pinned Vue grammar");
        let tree = parser
            .parse(
                "<template><div /></template>\n<script>const x = 1</script>\n",
                None,
            )
            .expect("parse Vue");
        assert!(!tree.root_node().has_error());
    }

    #[cfg(feature = "lang-svelte")]
    #[test]
    fn loads_svelte_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&svelte_language())
            .expect("load pinned Svelte grammar");
        let tree = parser
            .parse("<script>const x = 1</script>\n<button>{x}</button>\n", None)
            .expect("parse Svelte");
        assert!(!tree.root_node().has_error());
    }

    #[cfg(feature = "lang-zig")]
    #[test]
    fn loads_zig_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&zig_language())
            .expect("load pinned Zig grammar");
        let tree = parser
            .parse(
                "const std = @import(\"std\");\npub fn main() void {}\n",
                None,
            )
            .expect("parse Zig");
        assert!(!tree.root_node().has_error());
    }

    #[cfg(feature = "lang-swift")]
    #[test]
    fn loads_swift_language() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&swift_language())
            .expect("load pinned Swift grammar");
        let tree = parser
            .parse(
                "import Foundation\nstruct User { let name: String }\n",
                None,
            )
            .expect("parse Swift");
        assert!(!tree.root_node().has_error());
    }
}
