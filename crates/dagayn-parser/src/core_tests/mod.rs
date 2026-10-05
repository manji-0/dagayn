use super::*;

#[cfg(feature = "lang-bash")]
mod bash;
#[cfg(all(feature = "lang-c", feature = "lang-cpp", feature = "lang-objc"))]
mod c_like;
#[cfg(feature = "lang-csharp")]
mod csharp;
#[cfg(feature = "lang-dart")]
mod dart;
mod discovery;
#[cfg(feature = "lang-elixir")]
mod elixir;
#[cfg(feature = "lang-gdscript")]
mod gdscript;
#[cfg(feature = "lang-go")]
mod go;
mod grammar_features;
#[cfg(any(
    feature = "lang-kotlin",
    feature = "lang-swift",
    feature = "lang-dart",
    feature = "lang-java"
))]
mod heritage;
#[cfg(feature = "lang-java")]
mod java;
#[cfg(all(
    feature = "lang-javascript",
    feature = "lang-typescript",
    feature = "lang-tsx"
))]
mod javascript_calls;
#[cfg(all(
    feature = "lang-javascript",
    feature = "lang-typescript",
    feature = "lang-tsx"
))]
mod javascript_modules;
#[cfg(all(feature = "lang-vue", feature = "lang-svelte"))]
mod javascript_sfc;
#[cfg(all(
    feature = "lang-javascript",
    feature = "lang-typescript",
    feature = "lang-tsx"
))]
mod javascript_test_detection;
#[cfg(feature = "lang-julia")]
mod julia;
#[cfg(feature = "lang-kotlin")]
mod kotlin;
mod local_scopes;
#[cfg(feature = "lang-lua")]
mod lua;
#[cfg(feature = "lang-markdown")]
mod markdown;
#[cfg(all(
    feature = "lang-csharp",
    feature = "lang-java",
    feature = "lang-kotlin",
    feature = "lang-scala",
    feature = "lang-php"
))]
mod namespaces;
#[cfg(feature = "lang-perl")]
mod perl;
#[cfg(feature = "lang-php")]
mod php;
mod python;
#[cfg(feature = "lang-r")]
mod r;
#[cfg(feature = "lang-ruby")]
mod ruby;
#[cfg(feature = "lang-rust")]
mod rust_edges;
#[cfg(feature = "lang-rust")]
mod rust_lang;
#[cfg(feature = "lang-scala")]
mod scala;
#[cfg(feature = "lang-swift")]
mod swift;
#[cfg(feature = "lang-terraform")]
mod terraform;
#[cfg(all(
    feature = "lang-javascript",
    feature = "lang-typescript",
    feature = "lang-tsx"
))]
mod typescript_declarations;
#[cfg(all(
    feature = "lang-javascript",
    feature = "lang-typescript",
    feature = "lang-tsx"
))]
mod typescript_types;
#[cfg(feature = "lang-zig")]
mod zig;

#[cfg(all(
    feature = "lang-javascript",
    feature = "lang-typescript",
    feature = "lang-tsx"
))]
fn type_references<'a>(edges: &'a [ParsedEdge], source: &str) -> Vec<&'a ParsedEdge> {
    edges
        .iter()
        .filter(|edge| {
            edge.kind == "REFERENCES"
                && edge.source == source
                && matches!(
                    edge.extra["relationship_role"].as_str(),
                    Some("type_reference" | "type_query")
                )
        })
        .collect()
}

#[cfg(all(
    feature = "lang-javascript",
    feature = "lang-typescript",
    feature = "lang-tsx"
))]
fn type_reference_positions(edges: &[ParsedEdge], source: &str, target: &str) -> Vec<String> {
    let found = type_references(edges, source)
        .into_iter()
        .filter(|edge| edge.target == target)
        .collect::<Vec<_>>();
    assert_eq!(found.len(), 1, "{source} -> {target}: {edges:#?}");
    found[0].extra["type_positions"]
        .as_array()
        .expect("type_positions")
        .iter()
        .map(|position| position.as_str().unwrap().to_string())
        .collect()
}

#[cfg(all(
    feature = "lang-javascript",
    feature = "lang-typescript",
    feature = "lang-tsx"
))]
fn write_type_reference_repo(name: &str) -> std::path::PathBuf {
    let mut repo_root = std::env::temp_dir();
    repo_root.push(format!(
        "dagayn-parser-ts-{name}-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    let _ = std::fs::remove_dir_all(&repo_root);
    std::fs::create_dir_all(repo_root.join("src")).unwrap();
    let models = r#"export interface User { id: string }
export class Repo<T> { find(id: string): T | undefined { return undefined; } }
export type UserId = string;
export enum Role { Admin, Guest }
export namespace Api { export interface Request { user: User } }
export default class DefaultModel { save() {} }
export const helper = () => 1;
"#;
    std::fs::write(repo_root.join("src/models.ts"), models).unwrap();
    std::fs::write(
        repo_root.join("src/barrel.ts"),
        "export { User as Member } from \"./models\";\nexport * as models from \"./models\";\n",
    )
    .unwrap();
    repo_root
}
