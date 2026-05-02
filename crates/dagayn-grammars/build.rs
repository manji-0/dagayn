use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct GrammarSpec {
    language: &'static str,
    symbol: &'static str,
    required_paths: &'static [&'static str],
    parser_subdirectory: Option<&'static str>,
}

const MARKDOWN: GrammarSpec = GrammarSpec {
    language: "markdown",
    symbol: "markdown",
    required_paths: &[
        "src/parser.c",
        "src/scanner.c",
        "src/tree_sitter/alloc.h",
        "src/tree_sitter/array.h",
        "src/tree_sitter/parser.h",
        "bindings/python/binding.c",
    ],
    parser_subdirectory: None,
};

const TERRAFORM: GrammarSpec = GrammarSpec {
    language: "terraform",
    symbol: "terraform",
    required_paths: &[
        "src/parser.c",
        "src/scanner.c",
        "src/tree_sitter/alloc.h",
        "src/tree_sitter/array.h",
        "src/tree_sitter/parser.h",
        "bindings/python/binding.c",
    ],
    parser_subdirectory: None,
};

const RUST: GrammarSpec = GrammarSpec {
    language: "rust",
    symbol: "rust",
    required_paths: &[
        "src/parser.c",
        "src/scanner.c",
        "src/tree_sitter/alloc.h",
        "src/tree_sitter/array.h",
        "src/tree_sitter/parser.h",
        "bindings/python/binding.c",
    ],
    parser_subdirectory: None,
};

const PYTHON: GrammarSpec = GrammarSpec {
    language: "python",
    symbol: "python",
    required_paths: &[
        "src/parser.c",
        "src/scanner.c",
        "src/tree_sitter/alloc.h",
        "src/tree_sitter/array.h",
        "src/tree_sitter/parser.h",
        "bindings/python/binding.c",
    ],
    parser_subdirectory: None,
};

const JAVASCRIPT: GrammarSpec = GrammarSpec {
    language: "javascript",
    symbol: "javascript",
    required_paths: &[
        "src/parser.c",
        "src/scanner.c",
        "src/tree_sitter/alloc.h",
        "src/tree_sitter/array.h",
        "src/tree_sitter/parser.h",
        "bindings/python/binding.c",
    ],
    parser_subdirectory: None,
};

const TYPESCRIPT: GrammarSpec = GrammarSpec {
    language: "typescript",
    symbol: "typescript",
    required_paths: &[
        "typescript/src/parser.c",
        "typescript/src/scanner.c",
        "typescript/src/tree_sitter/alloc.h",
        "typescript/src/tree_sitter/array.h",
        "typescript/src/tree_sitter/parser.h",
        "common/scanner.h",
        "bindings/python/binding.c",
    ],
    parser_subdirectory: Some("typescript"),
};

const TSX: GrammarSpec = GrammarSpec {
    language: "tsx",
    symbol: "tsx",
    required_paths: &[
        "tsx/src/parser.c",
        "tsx/src/scanner.c",
        "tsx/src/tree_sitter/alloc.h",
        "tsx/src/tree_sitter/array.h",
        "tsx/src/tree_sitter/parser.h",
        "common/scanner.h",
        "bindings/python/binding.c",
    ],
    parser_subdirectory: Some("tsx"),
};

const BASH: GrammarSpec = GrammarSpec {
    language: "bash",
    symbol: "bash",
    required_paths: &[
        "src/parser.c",
        "src/scanner.c",
        "src/tree_sitter/parser.h",
        "bindings/python/binding.c",
    ],
    parser_subdirectory: None,
};

const GO: GrammarSpec = GrammarSpec {
    language: "go",
    symbol: "go",
    required_paths: &[
        "src/parser.c",
        "src/tree_sitter/alloc.h",
        "src/tree_sitter/array.h",
        "src/tree_sitter/parser.h",
        "bindings/python/binding.c",
    ],
    parser_subdirectory: None,
};

const JAVA: GrammarSpec = GrammarSpec {
    language: "java",
    symbol: "java",
    required_paths: &[
        "src/parser.c",
        "src/tree_sitter/alloc.h",
        "src/tree_sitter/array.h",
        "src/tree_sitter/parser.h",
        "bindings/python/binding.c",
    ],
    parser_subdirectory: None,
};

const RUBY: GrammarSpec = GrammarSpec {
    language: "ruby",
    symbol: "ruby",
    required_paths: &[
        "src/parser.c",
        "src/scanner.c",
        "src/tree_sitter/alloc.h",
        "src/tree_sitter/array.h",
        "src/tree_sitter/parser.h",
        "bindings/python/binding.c",
    ],
    parser_subdirectory: None,
};

const CSHARP: GrammarSpec = GrammarSpec {
    language: "csharp",
    symbol: "c_sharp",
    required_paths: &[
        "src/parser.c",
        "src/scanner.c",
        "src/tree_sitter/alloc.h",
        "src/tree_sitter/array.h",
        "src/tree_sitter/parser.h",
        "bindings/python/binding.c",
    ],
    parser_subdirectory: None,
};

const PHP: GrammarSpec = GrammarSpec {
    language: "php",
    symbol: "php",
    required_paths: &[
        "php/src/parser.c",
        "php/src/scanner.c",
        "php/src/tree_sitter/alloc.h",
        "php/src/tree_sitter/array.h",
        "php/src/tree_sitter/parser.h",
        "common/scanner.h",
        "bindings/python/binding.c",
    ],
    parser_subdirectory: Some("php"),
};

const KOTLIN: GrammarSpec = GrammarSpec {
    language: "kotlin",
    symbol: "kotlin",
    required_paths: &[
        "src/parser.c",
        "src/scanner.c",
        "src/tree_sitter/alloc.h",
        "src/tree_sitter/array.h",
        "src/tree_sitter/parser.h",
        "bindings/python/binding.c",
    ],
    parser_subdirectory: None,
};

const SCALA: GrammarSpec = GrammarSpec {
    language: "scala",
    symbol: "scala",
    required_paths: &[
        "src/parser.c",
        "src/scanner.c",
        "src/tree_sitter/alloc.h",
        "src/tree_sitter/array.h",
        "src/tree_sitter/parser.h",
        "bindings/python/tree_sitter_scala/binding.c",
    ],
    parser_subdirectory: None,
};

const SOLIDITY: GrammarSpec = GrammarSpec {
    language: "solidity",
    symbol: "solidity",
    required_paths: &[
        "src/parser.c",
        "src/tree_sitter/alloc.h",
        "src/tree_sitter/array.h",
        "src/tree_sitter/parser.h",
        "bindings/python/tree_sitter_solidity/binding.c",
    ],
    parser_subdirectory: None,
};

const DART: GrammarSpec = GrammarSpec {
    language: "dart",
    symbol: "dart",
    required_paths: &[
        "src/parser.c",
        "src/scanner.c",
        "src/tree_sitter/alloc.h",
        "src/tree_sitter/array.h",
        "src/tree_sitter/parser.h",
        "bindings/python/tree_sitter_dart/binding.c",
    ],
    parser_subdirectory: None,
};

const LUA: GrammarSpec = GrammarSpec {
    language: "lua",
    symbol: "lua",
    required_paths: &[
        "src/parser.c",
        "src/scanner.c",
        "src/tree_sitter/alloc.h",
        "src/tree_sitter/array.h",
        "src/tree_sitter/parser.h",
        "bindings/python/tree_sitter_lua/binding.c",
    ],
    parser_subdirectory: None,
};

const LUAU: GrammarSpec = GrammarSpec {
    language: "luau",
    symbol: "luau",
    required_paths: &[
        "src/parser.c",
        "src/scanner.c",
        "src/tree_sitter/alloc.h",
        "src/tree_sitter/array.h",
        "src/tree_sitter/parser.h",
        "bindings/python/binding.c",
    ],
    parser_subdirectory: None,
};

const C: GrammarSpec = GrammarSpec {
    language: "c",
    symbol: "c",
    required_paths: &[
        "src/parser.c",
        "src/tree_sitter/alloc.h",
        "src/tree_sitter/array.h",
        "src/tree_sitter/parser.h",
        "bindings/python/tree_sitter_c/binding.c",
    ],
    parser_subdirectory: None,
};

const CPP: GrammarSpec = GrammarSpec {
    language: "cpp",
    symbol: "cpp",
    required_paths: &[
        "src/parser.c",
        "src/scanner.c",
        "src/tree_sitter/alloc.h",
        "src/tree_sitter/array.h",
        "src/tree_sitter/parser.h",
        "bindings/python/tree_sitter_cpp/binding.c",
    ],
    parser_subdirectory: None,
};

const OBJC: GrammarSpec = GrammarSpec {
    language: "objc",
    symbol: "objc",
    required_paths: &[
        "src/parser.c",
        "src/tree_sitter/alloc.h",
        "src/tree_sitter/array.h",
        "src/tree_sitter/parser.h",
        "bindings/python/tree_sitter_objc/binding.c",
    ],
    parser_subdirectory: None,
};

const ELIXIR: GrammarSpec = GrammarSpec {
    language: "elixir",
    symbol: "elixir",
    required_paths: &[
        "src/parser.c",
        "src/scanner.c",
        "src/tree_sitter/alloc.h",
        "src/tree_sitter/array.h",
        "src/tree_sitter/parser.h",
        "bindings/python/tree_sitter_elixir/binding.c",
    ],
    parser_subdirectory: None,
};

const GDSCRIPT: GrammarSpec = GrammarSpec {
    language: "gdscript",
    symbol: "gdscript",
    required_paths: &[
        "src/parser.c",
        "src/scanner.c",
        "src/tree_sitter/alloc.h",
        "src/tree_sitter/array.h",
        "src/tree_sitter/parser.h",
        "bindings/python/tree_sitter_gdscript/binding.c",
    ],
    parser_subdirectory: None,
};

const R: GrammarSpec = GrammarSpec {
    language: "r",
    symbol: "r",
    required_paths: &[
        "src/parser.c",
        "src/scanner.c",
        "src/tree_sitter/alloc.h",
        "src/tree_sitter/array.h",
        "src/tree_sitter/parser.h",
        "bindings/python/tree_sitter_r/binding.c",
    ],
    parser_subdirectory: None,
};

const JULIA: GrammarSpec = GrammarSpec {
    language: "julia",
    symbol: "julia",
    required_paths: &[
        "src/parser.c",
        "src/scanner.c",
        "src/tree_sitter/alloc.h",
        "src/tree_sitter/array.h",
        "src/tree_sitter/parser.h",
        "bindings/python/tree_sitter_julia/binding.c",
    ],
    parser_subdirectory: None,
};

const PERL: GrammarSpec = GrammarSpec {
    language: "perl",
    symbol: "perl",
    required_paths: &[
        "src/parser.c",
        "src/scanner.c",
        "src/tree_sitter/alloc.h",
        "src/tree_sitter/array.h",
        "src/tree_sitter/parser.h",
        "src/bsearch.h",
        "src/tsp_unicode.h",
    ],
    parser_subdirectory: None,
};

const VUE: GrammarSpec = GrammarSpec {
    language: "vue",
    symbol: "vue",
    required_paths: &[
        "src/parser.c",
        "src/scanner.c",
        "src/tag.h",
        "src/tree_sitter/alloc.h",
        "src/tree_sitter/array.h",
        "src/tree_sitter/parser.h",
    ],
    parser_subdirectory: None,
};

const SVELTE: GrammarSpec = GrammarSpec {
    language: "svelte",
    symbol: "svelte",
    required_paths: &[
        "src/parser.c",
        "src/scanner.c",
        "src/allocator.h",
        "src/ekstring.h",
        "src/tag.h",
        "src/tree_sitter/parser.h",
        "src/uthash.h",
        "src/vc_vector.h",
    ],
    parser_subdirectory: None,
};

const ZIG: GrammarSpec = GrammarSpec {
    language: "zig",
    symbol: "zig",
    required_paths: &[
        "src/parser.c",
        "src/tree_sitter/alloc.h",
        "src/tree_sitter/array.h",
        "src/tree_sitter/parser.h",
    ],
    parser_subdirectory: None,
};

const POWERSHELL: GrammarSpec = GrammarSpec {
    language: "powershell",
    symbol: "powershell",
    required_paths: &[
        "src/parser.c",
        "src/scanner.c",
        "src/tree_sitter/alloc.h",
        "src/tree_sitter/array.h",
        "src/tree_sitter/parser.h",
    ],
    parser_subdirectory: None,
};

const SWIFT: GrammarSpec = GrammarSpec {
    language: "swift",
    symbol: "swift",
    required_paths: &[
        "src/parser.c",
        "src/scanner.c",
        "src/tree_sitter/alloc.h",
        "src/tree_sitter/array.h",
        "src/tree_sitter/parser.h",
    ],
    parser_subdirectory: None,
};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("dagayn-grammars must live under crates/")
        .to_path_buf();

    println!("cargo:rerun-if-changed=build.rs");
    println!(
        "cargo:rerun-if-changed={}",
        repo_root.join("dagayn/vendor_grammars.py").display()
    );

    compile_grammar(&repo_root, &MARKDOWN);
    compile_grammar(&repo_root, &TERRAFORM);
    compile_grammar(&repo_root, &RUST);
    compile_grammar(&repo_root, &PYTHON);
    compile_grammar(&repo_root, &JAVASCRIPT);
    compile_grammar(&repo_root, &TYPESCRIPT);
    compile_grammar(&repo_root, &TSX);
    compile_grammar(&repo_root, &BASH);
    compile_grammar(&repo_root, &GO);
    compile_grammar(&repo_root, &JAVA);
    compile_grammar(&repo_root, &RUBY);
    compile_grammar(&repo_root, &CSHARP);
    compile_grammar(&repo_root, &PHP);
    compile_grammar(&repo_root, &KOTLIN);
    compile_grammar(&repo_root, &SCALA);
    compile_grammar(&repo_root, &SOLIDITY);
    compile_grammar(&repo_root, &DART);
    compile_grammar(&repo_root, &LUA);
    compile_grammar(&repo_root, &LUAU);
    compile_grammar(&repo_root, &C);
    compile_grammar(&repo_root, &CPP);
    compile_grammar(&repo_root, &OBJC);
    compile_grammar(&repo_root, &ELIXIR);
    compile_grammar(&repo_root, &GDSCRIPT);
    compile_grammar(&repo_root, &R);
    compile_grammar(&repo_root, &JULIA);
    compile_grammar(&repo_root, &PERL);
    compile_grammar(&repo_root, &VUE);
    compile_grammar(&repo_root, &SVELTE);
    compile_grammar(&repo_root, &ZIG);
    compile_grammar(&repo_root, &POWERSHELL);
    compile_grammar(&repo_root, &SWIFT);
}

fn compile_grammar(repo_root: &Path, spec: &GrammarSpec) {
    let source_dir = ensure_source_dir(repo_root, spec);
    stage_packaged_source(repo_root, spec, &source_dir);
    for required in spec.required_paths {
        println!(
            "cargo:rerun-if-changed={}",
            source_dir.join(required).display()
        );
    }

    let parser_root = spec
        .parser_subdirectory
        .map(|subdir| source_dir.join(subdir))
        .unwrap_or_else(|| source_dir.to_path_buf());
    let src_dir = parser_root.join("src");
    let mut build = cc::Build::new();
    build
        .include(&src_dir)
        .include(src_dir.join("tree_sitter"))
        .file(src_dir.join("parser.c"))
        .warnings(false)
        .flag_if_supported("-Wno-unused-parameter")
        .flag_if_supported("-Wno-unused-but-set-variable");
    let scanner = src_dir.join("scanner.c");
    if scanner.exists() {
        build.file(scanner);
    }
    if spec.language == "bash" {
        // tree-sitter-bash's scanner includes generic tree_sitter helper
        // headers that are staged with the earlier JavaScript grammar.
        build.include(
            repo_root
                .join("dagayn")
                .join("_vendor_grammars")
                .join("javascript")
                .join("src"),
        );
    }
    build.compile(&format!("dagayn_tree_sitter_{}", spec.symbol));
}

fn stage_packaged_source(repo_root: &Path, spec: &GrammarSpec, source_dir: &Path) {
    let packaged = repo_root
        .join("dagayn")
        .join("_vendor_grammars")
        .join(spec.language);
    if same_path(source_dir, &packaged) {
        return;
    }
    if is_ready(&packaged, spec) {
        return;
    }
    if packaged.exists() {
        fs::remove_dir_all(&packaged).unwrap_or_else(|err| {
            panic!(
                "failed to clear packaged {} grammar source at {}: {err}",
                spec.language,
                packaged.display()
            )
        });
    }
    for required in spec.required_paths {
        let source = source_dir.join(required);
        let target = packaged.join(required);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).unwrap_or_else(|err| {
                panic!(
                    "failed to create packaged {} grammar directory {}: {err}",
                    spec.language,
                    parent.display()
                )
            });
        }
        fs::copy(&source, &target).unwrap_or_else(|err| {
            panic!(
                "failed to stage packaged {} grammar file {} -> {}: {err}",
                spec.language,
                source.display(),
                target.display()
            )
        });
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn ensure_source_dir(repo_root: &Path, spec: &GrammarSpec) -> PathBuf {
    let packaged = repo_root
        .join("dagayn")
        .join("_vendor_grammars")
        .join(spec.language);
    if is_ready(&packaged, spec) {
        return packaged;
    }

    let script = format!(
        "from dagayn.vendor_grammars import ensure_vendor_grammar_source; print(ensure_vendor_grammar_source({:?}))",
        spec.language
    );
    let mut errors = Vec::new();
    let candidates = env::var("PYTHON")
        .ok()
        .into_iter()
        .chain(["python3".to_string(), "python".to_string()]);
    for candidate in candidates {
        let output = Command::new(&candidate)
            .arg("-c")
            .arg(&script)
            .current_dir(repo_root)
            .env("PYTHONPATH", repo_root)
            .output();
        match output {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let Some(path) = stdout
                    .lines()
                    .last()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                else {
                    errors.push(format!("{candidate}: empty stdout"));
                    continue;
                };
                let source_dir = PathBuf::from(path);
                if is_ready(&source_dir, spec) {
                    return source_dir;
                }
                errors.push(format!("{candidate}: incomplete grammar source at {path}"));
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                errors.push(format!(
                    "{candidate}: exited with {}: {}",
                    output.status,
                    stderr.trim()
                ));
            }
            Err(err) => errors.push(format!("{candidate}: {err}")),
        }
    }

    panic!(
        "failed to prepare pinned {} grammar source via dagayn.vendor_grammars: {}",
        spec.language,
        errors.join("; ")
    );
}

fn is_ready(path: &Path, spec: &GrammarSpec) -> bool {
    path.exists()
        && spec
            .required_paths
            .iter()
            .all(|required| path.join(required).exists())
}
