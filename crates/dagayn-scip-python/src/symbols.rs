//! SCIP symbol strings in scip-python's grammar, so the overlay reads
//! dagayn's index as it read scip-python's:
//!
//! `scip-python python <package> <version> <module>/<Class#>method().`
//!
//! The module is ty's name for the file (`` `httpx._client` ``), the package
//! the standard library (`python-stdlib <python version>`), the project, or
//! for an installed module its top-level package. Classes are `Name#`,
//! functions `name().` (a nested function too: `outer().inner().`),
//! parameters `f().(x)`, and a name a module or class body binds `name.`.
//! A name a function body (or a lambda, or a comprehension) binds is
//! `local N`, numbered per document.

use ruff_db::files::{File, FilePath};
use ruff_db::parsed::parsed_module;
use ruff_python_ast::visitor::source_order::{SourceOrderVisitor, TraversalSignal};
use ruff_python_ast::{AnyNodeRef, Expr, ExprContext};
use ruff_text_size::{Ranged, TextSize};
use rustc_hash::FxHashMap;
use ty_project::{ProjectDatabase, SemanticDb as _};

/// What a binding site is named.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Binding {
    /// The descriptors after the module (`Store#save().`).
    Global(String),
    /// A name bound in a function-like scope: one key per scope and name.
    Local(u32),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScopeKind {
    Class,
    Function,
    /// A lambda or a comprehension.
    Anonymous,
}

struct Scope {
    kind: ScopeKind,
    id: usize,
    /// `Name#` or `name().`; empty for an anonymous scope.
    descriptor: String,
    /// A function's first parameter, the receiver of `self.x = ...`.
    receiver: Option<String>,
}

/// The binding sites of one file, by the start of their name: definitions,
/// parameters, and names and receiver attributes a statement assigns.
/// ty's goto-definition focuses a receiver attribute (`self.items = []`) on
/// the whole `self.items`, so those are also found by the receiver's start.
#[derive(Default)]
pub(crate) struct FileBindings {
    pub(crate) at: FxHashMap<TextSize, Binding>,
    pub(crate) attributes: FxHashMap<TextSize, Binding>,
}

impl FileBindings {
    fn target(&self, offset: TextSize) -> Option<&Binding> {
        self.at
            .get(&offset)
            .or_else(|| self.attributes.get(&offset))
    }
}

struct Collector {
    scopes: Vec<Scope>,
    next_scope: usize,
    locals: FxHashMap<(usize, String), u32>,
    bindings: FileBindings,
}

impl Collector {
    fn chain(&self) -> String {
        self.scopes.iter().map(|s| s.descriptor.as_str()).collect()
    }

    fn push(&mut self, kind: ScopeKind, descriptor: String, receiver: Option<String>) {
        self.next_scope += 1;
        self.scopes.push(Scope {
            kind,
            id: self.next_scope,
            descriptor,
            receiver,
        });
    }

    fn local(&mut self, scope: usize, name: &str) -> Binding {
        let next = u32::try_from(self.locals.len()).unwrap_or(u32::MAX);
        Binding::Local(*self.locals.entry((scope, name.to_string())).or_insert(next))
    }

    fn bind_name(&mut self, start: TextSize, name: &str) {
        let binding = match self.scopes.last() {
            Some(scope) if scope.kind != ScopeKind::Class => {
                let id = scope.id;
                self.local(id, name)
            }
            _ => Binding::Global(format!("{}{}.", self.chain(), escape(name))),
        };
        self.bindings.at.entry(start).or_insert(binding);
    }
}

impl<'a> SourceOrderVisitor<'a> for Collector {
    fn enter_node(&mut self, node: AnyNodeRef<'a>) -> TraversalSignal {
        match node {
            AnyNodeRef::StmtFunctionDef(function) => {
                let own = format!("{}().", escape(&function.name));
                self.bindings.at.insert(
                    function.name.start(),
                    Binding::Global(format!("{}{own}", self.chain())),
                );
                let parameters = &function.parameters;
                let receiver = parameters
                    .posonlyargs
                    .iter()
                    .chain(parameters.args.iter())
                    .next()
                    .map(|p| p.parameter.name.to_string());
                self.push(ScopeKind::Function, own, receiver);
            }
            AnyNodeRef::StmtClassDef(class) => {
                let own = format!("{}#", escape(&class.name));
                self.bindings.at.insert(
                    class.name.start(),
                    Binding::Global(format!("{}{own}", self.chain())),
                );
                self.push(ScopeKind::Class, own, None);
            }
            AnyNodeRef::ExprLambda(_)
            | AnyNodeRef::ExprListComp(_)
            | AnyNodeRef::ExprSetComp(_)
            | AnyNodeRef::ExprDictComp(_)
            | AnyNodeRef::ExprGenerator(_) => {
                self.push(ScopeKind::Anonymous, String::new(), None);
            }
            AnyNodeRef::Parameter(parameter) => {
                let binding = match self.scopes.last() {
                    Some(scope) if scope.kind == ScopeKind::Function => {
                        Binding::Global(format!("{}({})", self.chain(), escape(&parameter.name)))
                    }
                    Some(scope) => {
                        let id = scope.id;
                        self.local(id, &parameter.name)
                    }
                    None => return TraversalSignal::Traverse,
                };
                self.bindings.at.insert(parameter.name.start(), binding);
            }
            AnyNodeRef::ExprName(name) if name.ctx == ExprContext::Store => {
                self.bind_name(name.start(), &name.id);
            }
            AnyNodeRef::ExprAttribute(attribute) if attribute.ctx == ExprContext::Store => {
                let receiver = self
                    .scopes
                    .iter()
                    .rev()
                    .find(|s| s.kind == ScopeKind::Function)
                    .and_then(|s| s.receiver.as_deref());
                let class = self.scopes.iter().rposition(|s| s.kind == ScopeKind::Class);
                if let (Expr::Name(value), Some(receiver), Some(class)) =
                    (&*attribute.value, receiver, class)
                    && value.id.as_str() == receiver
                {
                    let owner: String = self.scopes[..=class]
                        .iter()
                        .map(|s| s.descriptor.as_str())
                        .collect();
                    let binding = Binding::Global(format!("{owner}{}.", escape(&attribute.attr)));
                    self.bindings
                        .attributes
                        .entry(attribute.start())
                        .or_insert_with(|| binding.clone());
                    self.bindings
                        .at
                        .entry(attribute.attr.start())
                        .or_insert(binding);
                }
            }
            AnyNodeRef::ExceptHandlerExceptHandler(handler) => {
                if let Some(name) = &handler.name {
                    self.bind_name(name.start(), name);
                }
            }
            _ => {}
        }
        TraversalSignal::Traverse
    }

    fn leave_node(&mut self, node: AnyNodeRef<'a>) {
        if matches!(
            node,
            AnyNodeRef::StmtFunctionDef(_)
                | AnyNodeRef::StmtClassDef(_)
                | AnyNodeRef::ExprLambda(_)
                | AnyNodeRef::ExprListComp(_)
                | AnyNodeRef::ExprSetComp(_)
                | AnyNodeRef::ExprDictComp(_)
                | AnyNodeRef::ExprGenerator(_)
        ) {
            self.scopes.pop();
        }
    }
}

pub(crate) fn file_bindings(db: &ProjectDatabase, file: File) -> FileBindings {
    let program_file = db.program_file(file);
    let parsed = parsed_module(db, program_file.python_file(db)).load(db);
    let mut collector = Collector {
        scopes: Vec::new(),
        next_scope: 0,
        locals: FxHashMap::default(),
        bindings: FileBindings::default(),
    };
    collector.visit_body(&parsed.syntax().body);
    collector.bindings
}

pub(crate) fn escape(name: &str) -> String {
    let simple = |c: char| c.is_alphanumeric() || matches!(c, '_' | '+' | '-' | '$');
    if !name.is_empty() && name.chars().all(simple) {
        name.to_string()
    } else {
        format!("`{}`", name.replace('`', "``"))
    }
}

/// The package and module of a file, as the symbol's prefix
/// (`scip-python python httpx . `` `httpx._client` ``/`).
fn prefix(db: &ProjectDatabase, file: File, project: &Project) -> Option<String> {
    let program_file = db.program_file(file);
    let module = ty_module_resolver::file_to_module(db, program_file.resolver_file(db));
    // A file of the project is the project's whichever search path found
    // it: an editable install of the project names the same files.
    let in_project = file.path(db).as_system_path().is_some_and(|path| {
        path.starts_with(&project.root)
            && !path
                .components()
                .any(|c| matches!(c.as_str(), "site-packages" | "dist-packages"))
    });
    let (package, version, name) = match module {
        Some(module) => {
            let name = module.name(db).to_string();
            let search_path = module.search_path(db);
            if in_project || search_path.is_some_and(|s| s.is_first_party()) {
                (project.name.clone(), project.version.clone(), name)
            } else if search_path.is_some_and(|s| s.is_standard_library()) {
                (
                    "python-stdlib".to_string(),
                    project.python_version.clone(),
                    name,
                )
            } else {
                let top = name.split('.').next().unwrap_or(&name).to_string();
                (top, ".".to_string(), name)
            }
        }
        None => match file.path(db) {
            FilePath::System(path) => path_module(path.as_str(), project)?,
            _ => return None,
        },
    };
    let package = package.replace(' ', "  ");
    Some(format!(
        "scip-python python {package} {version} {}/",
        escape(&name)
    ))
}

/// The package, version, and module of a file on no search path, by its
/// path: goto-definition goes from a standard library stub to the
/// interpreter's own source (`<prefix>/lib/python3.14/json/__init__.py`).
fn path_module(path: &str, project: &Project) -> Option<(String, String, String)> {
    if let Some(relative) = path.strip_prefix(&format!("{}/", project.root)) {
        return Some((
            project.name.clone(),
            project.version.clone(),
            module_of(relative),
        ));
    }
    for marker in ["/site-packages/", "/dist-packages/"] {
        if let Some((_, relative)) = path.rsplit_once(marker) {
            let module = module_of(relative);
            let top = module.split('.').next().unwrap_or(&module).to_string();
            return Some((top, ".".to_string(), module));
        }
    }
    let (_, rest) = path.rsplit_once("/lib/python3.")?;
    let (minor, relative) = rest.split_once('/')?;
    minor.trim_end_matches('t').parse::<u32>().ok()?;
    Some((
        "python-stdlib".to_string(),
        project.python_version.clone(),
        module_of(relative),
    ))
}

/// `pkg/mod.py` is `pkg.mod`, `pkg/__init__.py` is `pkg`.
fn module_of(relative: &str) -> String {
    let stem = relative
        .strip_suffix(".pyi")
        .or_else(|| relative.strip_suffix(".py"))
        .unwrap_or(relative);
    let stem = stem.strip_suffix("/__init__").unwrap_or(stem);
    stem.replace('/', ".")
}

pub(crate) struct Project {
    pub(crate) root: ruff_db::system::SystemPathBuf,
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) python_version: String,
}

/// Names the binding sites of every file a document references; one per
/// worker thread.
pub(crate) struct BindingNamer<'p> {
    project: &'p Project,
    files: FxHashMap<File, (Option<String>, FileBindings)>,
    /// Local key -> `local N` of the current document.
    locals: FxHashMap<u32, u32>,
    document: Option<File>,
}

impl<'p> BindingNamer<'p> {
    pub(crate) fn new(project: &'p Project) -> Self {
        Self {
            project,
            files: FxHashMap::default(),
            locals: FxHashMap::default(),
            document: None,
        }
    }

    pub(crate) fn start_document(&mut self, file: File) {
        self.locals.clear();
        self.document = Some(file);
    }

    fn file(&mut self, db: &ProjectDatabase, file: File) -> &(Option<String>, FileBindings) {
        let project = self.project;
        self.files
            .entry(file)
            .or_insert_with(|| (prefix(db, file, project), file_bindings(db, file)))
    }

    /// The module's own symbol (`` `pkg.mod`/__init__: ``).
    pub(crate) fn module(&mut self, db: &ProjectDatabase, file: File) -> Option<String> {
        let (prefix, _) = self.file(db, file);
        prefix.as_ref().map(|p| format!("{p}__init__:"))
    }

    /// The symbol of the binding whose name starts at `offset` in `file`; a
    /// local only in the current document.
    pub(crate) fn binding(
        &mut self,
        db: &ProjectDatabase,
        file: File,
        offset: TextSize,
    ) -> Option<String> {
        let in_document = self.document == Some(file);
        let (prefix, bindings) = self.file(db, file);
        match bindings.target(offset)? {
            Binding::Global(descriptors) => Some(format!("{}{descriptors}", prefix.as_ref()?)),
            Binding::Local(key) if in_document => {
                let key = *key;
                let next = u32::try_from(self.locals.len()).unwrap_or(u32::MAX) + 1;
                Some(format!("local {}", self.locals.entry(key).or_insert(next)))
            }
            Binding::Local(_) => None,
        }
    }

    pub(crate) fn is_binding(
        &mut self,
        db: &ProjectDatabase,
        file: File,
        offset: TextSize,
    ) -> bool {
        self.file(db, file).1.at.contains_key(&offset)
    }
}

#[cfg(test)]
mod tests {
    use super::{escape, module_of};

    #[test]
    fn names_outside_the_simple_alphabet_are_backquoted() {
        assert_eq!(escape("total"), "total");
        assert_eq!(escape("httpx._client"), "`httpx._client`");
        assert_eq!(escape("a`b"), "`a``b`");
    }

    #[test]
    fn a_package_module_is_its_directory() {
        assert_eq!(module_of("pkg/__init__.py"), "pkg");
        assert_eq!(module_of("pkg/mod.pyi"), "pkg.mod");
        assert_eq!(module_of("main.py"), "main");
    }
}
