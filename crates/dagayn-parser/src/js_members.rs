//! Same-file and cross-file class shapes for JavaScript / TypeScript member
//! calls: which members and typed fields a class or interface declares, and
//! what it extends, so `this.repo.find()`, `super.m()`, `Box.create()`, and
//! `x.m()` on a typed receiver bind to the declaring member.

use std::collections::{HashMap, HashSet};

use super::js_like::{javascript_member_name, javascript_member_path};
use super::js_modules::{
    JavaScriptExportResolution, JavaScriptParseContext, javascript_module_index,
    resolve_javascript_import_path_in,
};
use super::qualify;
use super::util::node_text;

/// Members, typed fields, and bases of one class or interface.
#[derive(Clone, Debug, Default)]
pub(super) struct JavaScriptClassInfo {
    /// Methods, accessors, fields, and parameter properties (static ones
    /// included).
    pub(super) members: HashSet<String>,
    /// Field -> type name as written (`repo` -> `Repo`, `cache` ->
    /// `DefaultShape` for `cache = new DefaultShape()`).
    pub(super) fields: HashMap<String, String>,
    /// Extended types as written (`Base`, `ns.Base`); interfaces may have
    /// several.
    pub(super) bases: Vec<String>,
}

/// Owner path (`Box`, `Outer.Inner`) -> class shape.
pub(super) type JavaScriptClassTable = HashMap<String, JavaScriptClassInfo>;

pub(super) fn collect_javascript_class_table(
    root: tree_sitter::Node<'_>,
    source: &[u8],
) -> JavaScriptClassTable {
    let mut table = JavaScriptClassTable::new();
    collect_scope(root, source, None, &mut table);
    table
}

fn collect_scope(
    scope: tree_sitter::Node<'_>,
    source: &[u8],
    owner: Option<&str>,
    table: &mut JavaScriptClassTable,
) {
    let mut cursor = scope.walk();
    for statement in scope.named_children(&mut cursor) {
        collect_statement(statement, source, owner, table);
    }
}

fn member_path(owner: Option<&str>, name: &str) -> String {
    match owner {
        Some(owner) => format!("{owner}.{name}"),
        None => name.to_string(),
    }
}

fn collect_statement(
    statement: tree_sitter::Node<'_>,
    source: &[u8],
    owner: Option<&str>,
    table: &mut JavaScriptClassTable,
) {
    match statement.kind() {
        "export_statement" | "expression_statement" | "ambient_declaration" => {
            let mut cursor = statement.walk();
            let is_global = statement
                .children(&mut cursor)
                .any(|child| child.kind() == "global");
            let mut cursor = statement.walk();
            for child in statement.named_children(&mut cursor) {
                if is_global && child.kind() == "statement_block" {
                    let path = member_path(owner, "global");
                    collect_scope(child, source, Some(&path), table);
                } else {
                    collect_statement(child, source, owner, table);
                }
            }
        }
        "internal_module" | "module" => {
            let Some(name) = statement.child_by_field_name("name") else {
                return;
            };
            let segments = match name.kind() {
                "string" => vec![
                    node_text(name, source)
                        .trim_matches(|c| c == '"' || c == '\'')
                        .to_string(),
                ],
                _ => node_text(name, source)
                    .split('.')
                    .map(|segment| segment.trim().to_string())
                    .collect(),
            };
            let mut path = owner.map(str::to_string);
            for segment in &segments {
                path = Some(member_path(path.as_deref(), segment));
            }
            if let (Some(body), Some(path)) = (statement.child_by_field_name("body"), path) {
                collect_scope(body, source, Some(&path), table);
            }
        }
        "class_declaration" | "abstract_class_declaration" | "interface_declaration" => {
            if let Some(name) = statement.child_by_field_name("name") {
                let path = member_path(owner, &node_text(name, source));
                merge_class_info(table, path, class_info(statement, source));
            }
        }
        "lexical_declaration" | "variable_declaration" => {
            let mut cursor = statement.walk();
            for declarator in statement.named_children(&mut cursor) {
                if let (Some(name), Some(value)) = (
                    declarator
                        .child_by_field_name("name")
                        .filter(|name| name.kind() == "identifier"),
                    declarator
                        .child_by_field_name("value")
                        .filter(|value| value.kind() == "class"),
                ) {
                    let path = member_path(owner, &node_text(name, source));
                    merge_class_info(table, path, class_info(value, source));
                }
            }
        }
        _ => {}
    }
}

/// Declaration merging (`interface Repo` twice, `class C` + `interface C`):
/// one shape holding every body's members, fields, and bases.
fn merge_class_info(table: &mut JavaScriptClassTable, path: String, info: JavaScriptClassInfo) {
    let merged = table.entry(path).or_default();
    merged.members.extend(info.members);
    for (field, ty) in info.fields {
        merged.fields.entry(field).or_insert(ty);
    }
    for base in info.bases {
        if !merged.bases.contains(&base) {
            merged.bases.push(base);
        }
    }
}

fn class_info(node: tree_sitter::Node<'_>, source: &[u8]) -> JavaScriptClassInfo {
    let mut info = JavaScriptClassInfo::default();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "class_heritage" => {
                let mut heritage = child.walk();
                for part in child.named_children(&mut heritage) {
                    match part.kind() {
                        "extends_clause" => {
                            if let Some(value) = part.child_by_field_name("value") {
                                info.bases.extend(expression_type_name(value, source));
                            }
                        }
                        "implements_clause" | "comment" => {}
                        // JavaScript: the expression itself.
                        _ => info.bases.extend(expression_type_name(part, source)),
                    }
                }
            }
            "extends_type_clause" => {
                let mut types = child.walk();
                for ty in child.named_children(&mut types) {
                    info.bases.extend(type_node_name(ty, source));
                }
            }
            _ => {}
        }
    }
    let Some(body) = node.child_by_field_name("body") else {
        return info;
    };
    let mut cursor = body.walk();
    for member in body.named_children(&mut cursor) {
        match member.kind() {
            "method_definition" | "method_signature" | "abstract_method_signature" => {
                let Some(name) = javascript_member_name(member, source) else {
                    continue;
                };
                if name == "constructor" {
                    collect_parameter_properties(member, source, &mut info);
                    collect_constructor_assignments(member, source, &mut info);
                }
                info.members.insert(name);
            }
            "public_field_definition" | "field_definition" | "property_signature" => {
                let Some(name) = javascript_member_name(member, source).or_else(|| {
                    member
                        .child_by_field_name("name")
                        .map(|name| node_text(name, source))
                }) else {
                    continue;
                };
                let ty = member
                    .child_by_field_name("type")
                    .and_then(|annotation| annotation_type_name(annotation, source))
                    .or_else(|| {
                        member
                            .child_by_field_name("value")
                            .and_then(|value| constructed_type_name(value, source))
                    });
                if let Some(ty) = ty {
                    info.fields.insert(name.clone(), ty);
                }
                info.members.insert(name);
            }
            _ => {}
        }
    }
    info
}

/// `constructor(private readonly repo: Repo)` declares field `repo: Repo`.
fn collect_parameter_properties(
    constructor: tree_sitter::Node<'_>,
    source: &[u8],
    info: &mut JavaScriptClassInfo,
) {
    let Some(parameters) = constructor.child_by_field_name("parameters") else {
        return;
    };
    let mut cursor = parameters.walk();
    for parameter in parameters.named_children(&mut cursor) {
        if !matches!(
            parameter.kind(),
            "required_parameter" | "optional_parameter"
        ) {
            continue;
        }
        let mut modifiers = parameter.walk();
        let is_property = parameter.children(&mut modifiers).any(|child| {
            matches!(
                child.kind(),
                "accessibility_modifier" | "readonly" | "override_modifier"
            )
        });
        if !is_property {
            continue;
        }
        let Some(name) = parameter
            .child_by_field_name("pattern")
            .filter(|pattern| pattern.kind() == "identifier")
            .map(|pattern| node_text(pattern, source))
        else {
            continue;
        };
        if let Some(ty) = parameter
            .child_by_field_name("type")
            .and_then(|annotation| annotation_type_name(annotation, source))
        {
            info.fields.insert(name.clone(), ty);
        }
        info.members.insert(name);
    }
}

/// `this.repo = new Repo()` directly in a constructor body declares field
/// `repo: Repo` (JavaScript classes have no field types).
fn collect_constructor_assignments(
    constructor: tree_sitter::Node<'_>,
    source: &[u8],
    info: &mut JavaScriptClassInfo,
) {
    let Some(body) = constructor.child_by_field_name("body") else {
        return;
    };
    let mut cursor = body.walk();
    for statement in body.named_children(&mut cursor) {
        let Some(assignment) = statement
            .named_child(0)
            .filter(|_| statement.kind() == "expression_statement")
            .filter(|expression| expression.kind() == "assignment_expression")
        else {
            continue;
        };
        let (Some(left), Some(right)) = (
            assignment.child_by_field_name("left"),
            assignment.child_by_field_name("right"),
        ) else {
            continue;
        };
        let (Some(object), Some(property)) = (
            left.child_by_field_name("object"),
            left.child_by_field_name("property"),
        ) else {
            continue;
        };
        if left.kind() != "member_expression" || object.kind() != "this" {
            continue;
        }
        let name = node_text(property, source);
        if let Some(ty) = constructed_type_name(right, source) {
            info.fields.entry(name.clone()).or_insert(ty);
        }
        info.members.insert(name);
    }
}

/// Type named by a `type_annotation` (`: Repo`, `: Repo<T>`, `: ns.Repo`).
pub(super) fn annotation_type_name(
    annotation: tree_sitter::Node<'_>,
    source: &[u8],
) -> Option<String> {
    let ty = if annotation.kind() == "type_annotation" {
        annotation.named_child(0)?
    } else {
        annotation
    };
    type_node_name(ty, source)
}

/// Name of a type node: `Repo`, `Repo<T>` -> `Repo`, `ns.Repo`.
pub(super) fn type_node_name(ty: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    match ty.kind() {
        "type_identifier" | "identifier" | "nested_type_identifier" => {
            Some(node_text(ty, source).trim().to_string())
        }
        "generic_type" => type_node_name(ty.child_by_field_name("name")?, source),
        _ => None,
    }
}

/// `new Repo()` -> `Repo`, `new ns.Repo()` -> `ns.Repo`.
pub(super) fn constructed_type_name(value: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    if value.kind() != "new_expression" {
        return None;
    }
    let constructor = value.child_by_field_name("constructor")?;
    expression_type_name(constructor, source)
}

/// A base or constructor expression naming a type: `Base`, `ns.Base`.
fn expression_type_name(expression: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    match expression.kind() {
        "identifier" => Some(node_text(expression, source)),
        "member_expression" => {
            let object = expression_type_name(expression.child_by_field_name("object")?, source)?;
            let property = expression.child_by_field_name("property")?;
            Some(format!("{object}.{}", node_text(property, source)))
        }
        _ => None,
    }
}

/// A resolved type: the declaring file and the owner path inside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct JavaScriptTypeRef {
    pub(super) file: String,
    pub(super) path: String,
}

impl JavaScriptTypeRef {
    /// `file::path` split at the first `::` (file paths never contain one).
    pub(super) fn from_qualified(qualified: &str) -> Option<Self> {
        let (file, path) = qualified.split_once("::")?;
        (!file.is_empty() && !path.is_empty()).then(|| Self {
            file: file.to_string(),
            path: path.to_string(),
        })
    }
}

impl JavaScriptTypeRef {
    fn qualified(&self) -> String {
        format!("{}::{}", self.file, self.path)
    }
}

/// Maximum number of bases followed when looking up an inherited member.
const JAVASCRIPT_MAX_BASE_DEPTH: usize = 8;

/// The shape of `ty`: from this file's table, or from the declaring
/// module's export index.
fn lookup_class(
    context: &JavaScriptParseContext<'_>,
    ty: &JavaScriptTypeRef,
) -> Option<JavaScriptClassInfo> {
    if ty.file == context.file_path.as_str() {
        return context.class_table.get(&ty.path).cloned();
    }
    javascript_module_index(&ty.file, context)?
        .class_table
        .get(&ty.path)
        .cloned()
}

/// Resolves a type name as written in `file` (`Repo`, `Outer.Inner`,
/// `ns.Repo`, a named / default import) to a class or interface declared
/// there or in the module it is imported from. Anything else (type aliases,
/// external packages, unknown names) is `None`.
pub(super) fn resolve_javascript_type_name(
    context: &JavaScriptParseContext<'_>,
    file: &str,
    name: &str,
) -> Option<JavaScriptTypeRef> {
    let local = file == context.file_path.as_str();
    let index = if local {
        None
    } else {
        Some(javascript_module_index(file, context)?)
    };
    let (class_table, import_map) = match &index {
        Some(index) => (index.class_table.as_ref(), index.import_map.as_ref()),
        None => (context.class_table, context.import_map),
    };
    if class_table.contains_key(name) {
        return Some(JavaScriptTypeRef {
            file: file.to_string(),
            path: name.to_string(),
        });
    }
    let (root, rest) = match name.split_once('.') {
        Some((root, rest)) => (root, Some(rest)),
        None => (name, None),
    };
    let binding = import_map.get(root)?;
    let segments = rest
        .map(|rest| rest.split('.').collect::<Vec<_>>())
        .unwrap_or_default();
    // `ns.Type` / `bns.Type` (a re-exported namespace) enter the module
    // object; a declaration reached first keeps the rest (`Outer.Inner`).
    let (resolved, consumed) = resolve_javascript_import_path_in(
        file,
        root,
        binding,
        &segments,
        context.repo_root,
        context.caches,
    )?;
    let JavaScriptExportResolution::Symbol(target) = resolved else {
        return None;
    };
    let qualified = if consumed == segments.len() {
        target
    } else {
        format!("{target}.{}", segments[consumed..].join("."))
    };
    let ty = JavaScriptTypeRef::from_qualified(&qualified)?;
    lookup_class(context, &ty).map(|_| ty)
}

/// Where a member call landed: the member's QN, and whether it was found on
/// a base rather than on the receiver's own type.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct JavaScriptMemberTarget {
    pub(super) qualified: String,
    pub(super) inherited: bool,
}

/// `member` declared by `ty` or, failing that, by its bases (nearest first).
pub(super) fn javascript_type_member(
    context: &JavaScriptParseContext<'_>,
    ty: &JavaScriptTypeRef,
    member: &str,
) -> Option<JavaScriptMemberTarget> {
    find_member(context, ty, member, 0, &mut HashSet::new()).map(|(qualified, depth)| {
        JavaScriptMemberTarget {
            qualified,
            inherited: depth > 0,
        }
    })
}

/// `member` declared by a base of `ty` (`super.m()`).
pub(super) fn javascript_base_member(
    context: &JavaScriptParseContext<'_>,
    ty: &JavaScriptTypeRef,
    member: &str,
) -> Option<JavaScriptMemberTarget> {
    let info = lookup_class(context, ty)?;
    let mut seen = HashSet::from([ty.qualified()]);
    info.bases.iter().find_map(|base| {
        let base = resolve_javascript_type_name(context, &ty.file, base)?;
        find_member(context, &base, member, 1, &mut seen).map(|(qualified, _)| {
            JavaScriptMemberTarget {
                qualified,
                inherited: true,
            }
        })
    })
}

/// The first base of `ty` that resolves to a class or interface
/// (`super(...)`).
pub(super) fn javascript_resolved_base(
    context: &JavaScriptParseContext<'_>,
    ty: &JavaScriptTypeRef,
) -> Option<JavaScriptTypeRef> {
    let info = lookup_class(context, ty)?;
    info.bases
        .iter()
        .find_map(|base| resolve_javascript_type_name(context, &ty.file, base))
}

/// The bases of `ty` as written.
pub(super) fn javascript_written_bases(
    context: &JavaScriptParseContext<'_>,
    ty: &JavaScriptTypeRef,
) -> Vec<String> {
    lookup_class(context, ty)
        .map(|info| info.bases)
        .unwrap_or_default()
}

fn find_member(
    context: &JavaScriptParseContext<'_>,
    ty: &JavaScriptTypeRef,
    member: &str,
    depth: usize,
    seen: &mut HashSet<String>,
) -> Option<(String, usize)> {
    if depth > JAVASCRIPT_MAX_BASE_DEPTH || !seen.insert(ty.qualified()) {
        return None;
    }
    let info = lookup_class(context, ty)?;
    if info.members.contains(member) {
        return Some((qualify(&ty.file, member, Some(&ty.path)), depth));
    }
    info.bases.iter().find_map(|base| {
        let base = resolve_javascript_type_name(context, &ty.file, base)?;
        find_member(context, &base, member, depth + 1, seen)
    })
}

/// The declared type of field `field` of `ty` (or of a base), resolved in
/// the file that declares the field.
fn field_type(
    context: &JavaScriptParseContext<'_>,
    ty: &JavaScriptTypeRef,
    field: &str,
    depth: usize,
    seen: &mut HashSet<String>,
) -> Option<JavaScriptTypeRef> {
    if depth > JAVASCRIPT_MAX_BASE_DEPTH || !seen.insert(ty.qualified()) {
        return None;
    }
    let info = lookup_class(context, ty)?;
    if let Some(written) = info.fields.get(field) {
        return resolve_javascript_type_name(context, &ty.file, written);
    }
    if info.members.contains(field) {
        // Declared here without a usable type: do not look further up.
        return None;
    }
    info.bases.iter().find_map(|base| {
        let base = resolve_javascript_type_name(context, &ty.file, base)?;
        field_type(context, &base, field, depth + 1, seen)
    })
}

/// The class `this` is bound to in the code being walked.
pub(super) fn javascript_this_type(
    context: &JavaScriptParseContext<'_>,
) -> Option<JavaScriptTypeRef> {
    let owner = context.bindings.borrow().bound_type("this")?.to_string();
    context
        .class_table
        .contains_key(&owner)
        .then(|| JavaScriptTypeRef {
            file: context.file_path.to_string(),
            path: owner,
        })
}

/// The class or interface a receiver expression evaluates to, with
/// evidence only: `this`, a variable bound by `new X()` or `x: X`, a class
/// named directly (`Box.create()`, `ns.Box.create()`), or a field of one of
/// those declared with a type (`this.repo`, `this.repo.inner`).
pub(super) fn javascript_receiver_type(
    context: &JavaScriptParseContext<'_>,
    expression: tree_sitter::Node<'_>,
) -> Option<JavaScriptTypeRef> {
    match expression.kind() {
        "this" => javascript_this_type(context),
        "identifier" => {
            let name = node_text(expression, context.source);
            let bound = context
                .bindings
                .borrow()
                .bound_type(&name)
                .map(str::to_string);
            match bound {
                Some(bound) => match JavaScriptTypeRef::from_qualified(&bound) {
                    Some(ty) => Some(ty),
                    None => context
                        .class_table
                        .contains_key(&bound)
                        .then(|| JavaScriptTypeRef {
                            file: context.file_path.to_string(),
                            path: bound,
                        }),
                },
                None if javascript_is_shadowed(context, &name) => None,
                None => resolve_javascript_type_name(context, context.file_path.as_str(), &name),
            }
        }
        "member_expression" => {
            if let Some(path) = javascript_member_path(expression, context.source)
                && !javascript_is_shadowed(context, path.split('.').next().unwrap_or_default())
                && !context
                    .bindings
                    .borrow()
                    .is_bound(path.split('.').next().unwrap_or_default())
                && let Some(ty) =
                    resolve_javascript_type_name(context, context.file_path.as_str(), &path)
            {
                return Some(ty);
            }
            let object = expression.child_by_field_name("object")?;
            let property = expression.child_by_field_name("property")?;
            let owner = javascript_receiver_type(context, object)?;
            let field = node_text(property, context.source);
            field_type(context, &owner, &field, 0, &mut HashSet::new())
        }
        "non_null_expression" | "parenthesized_expression" => {
            javascript_receiver_type(context, expression.named_child(0)?)
        }
        // `new Outer.Inner().run()`, `(new Repo()).find()`.
        "new_expression" => {
            let constructor = expression.child_by_field_name("constructor")?;
            let path = javascript_member_path(constructor, context.source)?;
            if javascript_is_shadowed(context, path.split('.').next().unwrap_or_default()) {
                return None;
            }
            resolve_javascript_type_name(context, context.file_path.as_str(), &path)
        }
        _ => None,
    }
}

/// A local declaration of the function being walked hides a module-level
/// name.
fn javascript_is_shadowed(context: &JavaScriptParseContext<'_>, name: &str) -> bool {
    context
        .local_scopes
        .borrow()
        .iter()
        .any(|scope| scope.contains(name))
}
