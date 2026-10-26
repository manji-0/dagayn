//! SCIP symbol strings in scip-typescript's grammar, so the overlay reads
//! dagayn's index as it read scip-typescript's:
//!
//! `scip-typescript npm <package> <version> <path>/<Container#>name().`
//!
//! The package and version come from the nearest `package.json` above the
//! declaring file, the bundled TypeScript lib is `npm typescript <ver>
//! lib/<file>`, and a parameter, a type parameter, or a declaration inside a
//! function body is `local N`, numbered per document.

use std::collections::HashMap;
use std::path::Path;

use rustc_hash::FxHashMap;
use tsrs_ast::{self as ast, Kind, Node};
use tsrs_core::P;

/// `(name, version, directory)` of the nearest `package.json` above a file,
/// cached per directory.
#[derive(Default)]
struct Packages {
    cache: HashMap<String, Option<(String, String, String)>>,
}

impl Packages {
    fn of(&mut self, file: &str) -> Option<(String, String, String)> {
        let mut dir = Path::new(file).parent();
        while let Some(d) = dir {
            let key = d.to_string_lossy().to_string();
            if let Some(hit) = self.cache.get(&key) {
                if hit.is_some() {
                    return hit.clone();
                }
            } else {
                let found = std::fs::read_to_string(d.join("package.json"))
                    .ok()
                    .and_then(|text| {
                        let v: serde_json::Value = serde_json::from_str(&text).ok()?;
                        let name = v.get("name")?.as_str()?.to_string();
                        let version = v
                            .get("version")
                            .and_then(|x| x.as_str())
                            .unwrap_or(".")
                            .to_string();
                        Some((name, version, key.clone()))
                    });
                self.cache.insert(key, found.clone());
                if found.is_some() {
                    return found;
                }
            }
            dir = d.parent();
        }
        None
    }
}

fn escape(name: &str) -> String {
    let simple = |c: char| c.is_alphanumeric() || matches!(c, '_' | '+' | '-' | '$');
    if !name.is_empty() && name.chars().all(simple) {
        name.to_string()
    } else {
        format!("`{}`", name.replace('`', "``"))
    }
}

fn decl_name(decl: P<Node>) -> String {
    if decl.kind() == Kind::Constructor {
        return "<constructor>".to_string();
    }
    match ast::get_name_of_declaration(decl) {
        Some(n) => {
            let t = n.text();
            if n.kind() == Kind::StringLiteral {
                format!("\"{t}\"")
            } else {
                t.to_string()
            }
        }
        None => "default".to_string(),
    }
}

/// A type literal inside a union or intersection has no name of its own.
fn type_literal_is_local(lit: P<Node>) -> bool {
    let mut a = lit.parent();
    while let Some(n) = a {
        match n.kind() {
            Kind::UnionType | Kind::IntersectionType => return true,
            Kind::TypeAliasDeclaration
            | Kind::InterfaceDeclaration
            | Kind::PropertySignature
            | Kind::VariableDeclaration
            | Kind::FunctionDeclaration
            | Kind::MethodSignature
            | Kind::MethodDeclaration
            | Kind::Parameter => return false,
            _ => {}
        }
        a = n.parent();
    }
    false
}

fn is_function_like(k: Kind) -> bool {
    matches!(
        k,
        Kind::FunctionDeclaration
            | Kind::FunctionExpression
            | Kind::ArrowFunction
            | Kind::MethodDeclaration
            | Kind::Constructor
            | Kind::GetAccessor
            | Kind::SetAccessor
    )
}

pub(crate) struct SymbolNamer {
    packages: Packages,
    lib_dir: String,
    ts_version: String,
    locals: FxHashMap<usize, u32>,
    next_local: u32,
}

impl SymbolNamer {
    pub(crate) fn new(lib_dir: String, ts_version: String) -> Self {
        Self {
            packages: Packages::default(),
            lib_dir,
            ts_version,
            locals: FxHashMap::default(),
            next_local: 0,
        }
    }

    /// Local ids restart at 1 for each document.
    pub(crate) fn start_document(&mut self) {
        self.locals.clear();
        self.next_local = 0;
    }

    /// The symbol of a declaration node, or of a source file (its module).
    pub(crate) fn symbol_for(&mut self, decl: P<Node>) -> Option<String> {
        let sf = ast::get_source_file_of_node(decl)?;
        let kind = decl.kind();
        if self.is_local(decl) {
            let key = &*decl as *const Node as usize;
            let id = *self.locals.entry(key).or_insert_with(|| {
                self.next_local += 1;
                self.next_local
            });
            return Some(format!("local {id}"));
        }
        let file = sf.file_name().to_string();
        let (pkg, ver, rel) = if file.starts_with(&self.lib_dir) {
            let base = file.rsplit('/').next().unwrap_or(&file).to_string();
            (
                "typescript".to_string(),
                self.ts_version.clone(),
                format!("lib/{base}"),
            )
        } else {
            match self.packages.of(&file) {
                Some((n, v, dir)) => {
                    let rel = file
                        .strip_prefix(&format!("{dir}/"))
                        .unwrap_or(&file)
                        .to_string();
                    (n, v, rel)
                }
                None => (
                    ".".to_string(),
                    ".".to_string(),
                    file.trim_start_matches('/').to_string(),
                ),
            }
        };
        let pkg = if pkg.is_empty() {
            ".".to_string()
        } else {
            pkg.replace(' ', "  ")
        };
        let mut desc = String::new();
        for s in rel.split('/') {
            desc.push_str(&escape(s));
            desc.push('/');
        }
        if kind == Kind::SourceFile {
            return Some(format!("scip-typescript npm {pkg} {ver} {desc}"));
        }
        // Enclosing named containers, outermost first.
        let mut chain = Vec::new();
        let mut a = decl.parent();
        while let Some(n) = a {
            match n.kind() {
                Kind::ClassDeclaration
                | Kind::ClassExpression
                | Kind::InterfaceDeclaration
                | Kind::EnumDeclaration
                | Kind::TypeAliasDeclaration => chain.push(format!("{}#", escape(&decl_name(n)))),
                Kind::ModuleDeclaration => chain.push(format!("{}/", escape(&decl_name(n)))),
                Kind::VariableDeclaration | Kind::PropertyAssignment | Kind::PropertySignature => {
                    chain.push(format!("{}.", escape(&decl_name(n))))
                }
                _ => {}
            }
            a = n.parent();
        }
        chain.reverse();
        for c in chain {
            desc.push_str(&c);
        }
        let raw = decl_name(decl);
        let name = escape(&match kind {
            Kind::GetAccessor => format!("<get>{raw}"),
            Kind::SetAccessor => format!("<set>{raw}"),
            _ => raw,
        });
        let own = match kind {
            Kind::FunctionDeclaration
            | Kind::MethodDeclaration
            | Kind::MethodSignature
            | Kind::Constructor => format!("{name}()."),
            Kind::ClassDeclaration
            | Kind::ClassExpression
            | Kind::InterfaceDeclaration
            | Kind::EnumDeclaration
            | Kind::TypeAliasDeclaration => format!("{name}#"),
            Kind::ModuleDeclaration => format!("{name}/"),
            _ => format!("{name}."),
        };
        desc.push_str(&own);
        Some(format!("scip-typescript npm {pkg} {ver} {desc}"))
    }

    fn is_local(&self, decl: P<Node>) -> bool {
        if matches!(decl.kind(), Kind::Parameter | Kind::TypeParameter) {
            return true;
        }
        let mut prev = decl;
        let mut a = decl.parent();
        while let Some(n) = a {
            if (is_function_like(n.kind()) && n.body() == Some(prev))
                || (n.kind() == Kind::TypeLiteral && type_literal_is_local(n))
            {
                return true;
            }
            prev = n;
            a = n.parent();
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::escape;

    #[test]
    fn names_outside_the_simple_alphabet_are_backquoted() {
        assert_eq!(escape("total"), "total");
        assert_eq!(escape("left-pad"), "left-pad");
        assert_eq!(escape("cart.ts"), "`cart.ts`");
        assert_eq!(escape("a`b"), "`a``b`");
        assert_eq!(escape(""), "``");
    }
}
