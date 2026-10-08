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
    /// Variables bound to a type this file does not declare (`var n = new
    /// Native()` with `Native` in another file): the type name only, which
    /// resolution across files matches (`receiver_type`).
    foreign: HashMap<String, String>,
    /// Variables holding what a call returned, of a type the file does not
    /// say (`conn = store_conn(store)`): the call, whose declared return
    /// type resolution across files reads.
    returned: HashMap<String, CallOrigin>,
}

/// A call whose result a receiver is: the called name as written
/// (`store_conn`, `open`), the line of the call, and whether the result was
/// unwrapped (`?`, `.unwrap()`, `.expect(..)`), which takes the `T` out of a
/// `Result<T, _>` / `Option<T>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CallOrigin {
    pub(super) name: String,
    pub(super) line: i64,
    pub(super) unwrap: bool,
    /// The receiver is an element of what the call returned (`f().iter()
    /// .filter(|x| x.m())`): resolution takes the `T` out of a `Vec<T>`,
    /// `HashSet<T>`, `Option<T>`, ... after any unwrap.
    pub(super) element: bool,
}

impl CallOrigin {
    /// The `receiver_from` metadata of a member call on this result.
    pub(super) fn to_json(&self) -> serde_json::Value {
        let mut json =
            serde_json::json!({"call": self.name, "line": self.line, "unwrap": self.unwrap});
        if self.element {
            json["element"] = serde_json::json!(true);
        }
        json
    }
}

/// The bindings in scope, saved around a nested scope.
#[derive(Debug, Default)]
pub(super) struct BindingsSnapshot {
    bindings: HashMap<String, String>,
    foreign: HashMap<String, String>,
    returned: HashMap<String, CallOrigin>,
}

impl MemberCallBindings {
    pub(super) fn with_types(type_names: HashSet<String>) -> Self {
        Self {
            type_names,
            ..Self::default()
        }
    }

    pub(super) fn snapshot(&self) -> BindingsSnapshot {
        BindingsSnapshot {
            bindings: self.bindings.clone(),
            foreign: self.foreign.clone(),
            returned: self.returned.clone(),
        }
    }

    pub(super) fn restore(&mut self, snapshot: BindingsSnapshot) {
        self.bindings = snapshot.bindings;
        self.foreign = snapshot.foreign;
        self.returned = snapshot.returned;
    }

    pub(super) fn bind(&mut self, var: impl Into<String>, type_name: impl Into<String>) {
        let type_name = type_name.into();
        if self.type_names.contains(type_name.as_str()) {
            let var = var.into();
            self.foreign.remove(&var);
            self.returned.remove(&var);
            self.bindings.insert(var, type_name);
        }
    }

    /// Binds `var` to the result of a call whose type the file does not
    /// say (see [`CallOrigin`]).
    pub(super) fn bind_returned(&mut self, var: impl Into<String>, origin: CallOrigin) {
        let var = var.into();
        self.bindings.remove(&var);
        self.foreign.remove(&var);
        self.returned.insert(var, origin);
    }

    /// The call `var` holds the result of.
    pub(super) fn returned_by(&self, var: &str) -> Option<&CallOrigin> {
        self.returned.get(var)
    }

    /// Binds `var` to `type_name` whether or not this file declares it: a
    /// declared type goes through [`Self::bind`], any other is remembered
    /// by name for [`Self::foreign_type`].
    pub(super) fn bind_any(&mut self, var: impl Into<String>, type_name: impl Into<String>) {
        let (var, type_name) = (var.into(), type_name.into());
        self.returned.remove(&var);
        if self.type_names.contains(type_name.as_str()) {
            self.bind(var, type_name);
        } else {
            self.bindings.remove(&var);
            self.foreign.insert(var, type_name);
        }
    }

    /// Drops `var`'s binding to a type this file does not declare, when it is
    /// assigned something else.
    pub(super) fn forget_foreign(&mut self, var: &str) {
        self.foreign.remove(var);
        self.returned.remove(var);
    }

    /// The name of the type in another file `var` is bound to.
    pub(super) fn foreign_type(&self, var: &str) -> Option<&str> {
        self.foreign.get(var).map(String::as_str)
    }

    /// Binds `var` to a same-file owner path the caller already verified
    /// (`Outer.Inner` for `new Outer.Inner()`), bypassing the bare type-name
    /// check.
    pub(super) fn bind_path(&mut self, var: impl Into<String>, owner_path: impl Into<String>) {
        self.bindings.insert(var.into(), owner_path.into());
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

    pub(super) fn is_bound(&self, receiver: &str) -> bool {
        self.bindings.contains_key(receiver)
    }

    /// The type bound to `var`: a same-file owner path, or a `file::path`
    /// QN for a type declared in another module.
    pub(super) fn bound_type(&self, var: &str) -> Option<&str> {
        self.bindings.get(var).map(String::as_str)
    }

    pub(super) fn resolve_member(&self, receiver: &str, method: &str) -> Option<String> {
        let type_name = self.bindings.get(receiver)?;
        // A type of another module is resolved by the extractor itself.
        if type_name.contains("::") {
            return None;
        }
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
