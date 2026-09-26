use super::*;

mod bash;
mod c_like;
mod csharp;
mod dart;
mod discovery;
mod elixir;
mod gdscript;
mod go;
mod java;
mod javascript_calls;
mod javascript_modules;
mod javascript_sfc;
mod javascript_test_detection;
mod julia;
mod kotlin;
mod lua;
mod markdown;
mod namespaces;
mod perl;
mod php;
mod python;
mod r;
mod ruby;
mod rust_lang;
mod scala;
mod swift;
mod terraform;
mod typescript_declarations;
mod typescript_types;
mod zig;

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
