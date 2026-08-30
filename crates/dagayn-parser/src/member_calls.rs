use std::collections::{HashMap, HashSet};

/// Same-file local type bindings for member CALLS.
///
/// Tree-sitter extractors record constructor / annotation bindings such as
/// `const store = new Store()`, `let repo = Repo::new()`, or
/// `var repo = new Repo()`, then rewrite `store.find()` / `repo.Find()` to
/// `Store::find` so [`super::resolve_rust_call_targets`] attaches the
/// implementation method instead of the first same-named trait, interface,
/// or protocol method.
#[derive(Debug, Default)]
pub(super) struct MemberCallBindings {
    type_names: HashSet<String>,
    bindings: HashMap<String, String>,
}

impl MemberCallBindings {
    pub(super) fn with_types(type_names: HashSet<String>) -> Self {
        Self {
            type_names,
            bindings: HashMap::new(),
        }
    }

    pub(super) fn snapshot(&self) -> HashMap<String, String> {
        self.bindings.clone()
    }

    pub(super) fn restore(&mut self, bindings: HashMap<String, String>) {
        self.bindings = bindings;
    }

    pub(super) fn bind(&mut self, var: impl Into<String>, type_name: impl Into<String>) {
        let type_name = type_name.into();
        if self.type_names.contains(type_name.as_str()) {
            self.bindings.insert(var.into(), type_name);
        }
    }

    pub(super) fn bind_implicit_receivers(&mut self, type_name: &str) {
        if type_name.is_empty() {
            return;
        }
        for receiver in ["self", "this", "cls", "Self"] {
            self.bindings
                .insert(receiver.to_string(), type_name.to_string());
        }
    }

    pub(super) fn resolve_member(&self, receiver: &str, method: &str) -> Option<String> {
        let type_name = self.bindings.get(receiver)?;
        Some(format!("{type_name}::{method}"))
    }

    pub(super) fn constructor_type<'a>(&self, call_name: &'a str) -> Option<&'a str> {
        let type_name = call_name
            .split("::")
            .next()
            .filter(|name| !name.is_empty())?;
        self.type_names.contains(type_name).then_some(type_name)
    }
}
