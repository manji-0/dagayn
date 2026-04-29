use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

struct GrammarSpec {
    language: &'static str,
    symbol: &'static str,
    required_paths: &'static [&'static str],
}

const MARKDOWN: GrammarSpec = GrammarSpec {
    language: "markdown",
    symbol: "markdown",
    required_paths: &[
        "src/parser.c",
        "src/scanner.c",
        "src/tree_sitter/parser.h",
        "bindings/python/binding.c",
    ],
};

const TERRAFORM: GrammarSpec = GrammarSpec {
    language: "terraform",
    symbol: "terraform",
    required_paths: &[
        "src/parser.c",
        "src/scanner.c",
        "src/tree_sitter/parser.h",
        "bindings/python/binding.c",
    ],
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
}

fn compile_grammar(repo_root: &Path, spec: &GrammarSpec) {
    let source_dir = ensure_source_dir(repo_root, spec);
    for required in spec.required_paths {
        println!(
            "cargo:rerun-if-changed={}",
            source_dir.join(required).display()
        );
    }

    let src_dir = source_dir.join("src");
    let mut build = cc::Build::new();
    build
        .include(&src_dir)
        .include(src_dir.join("tree_sitter"))
        .file(src_dir.join("parser.c"))
        .file(src_dir.join("scanner.c"))
        .warnings(false)
        .flag_if_supported("-Wno-unused-parameter")
        .flag_if_supported("-Wno-unused-but-set-variable");
    build.compile(&format!("dagayn_tree_sitter_{}", spec.symbol));
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
