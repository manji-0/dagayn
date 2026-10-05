//! The types Python code writes down for the receivers of member calls:
//! parameter annotations (`def f(store: GraphStore)`), and the attributes a
//! class gives `self` (`self.store = GraphStore(...)`, `self.path: Path`,
//! `store: GraphStore` in the class body, `self.store = store` of an
//! annotated parameter). `store.upsert_node()` then names the type's
//! method instead of a bare `upsert_node` any class may define.

use std::collections::HashMap;

use ruff_python_ast::statement_visitor::{StatementVisitor, walk_stmt};
use ruff_python_ast::{self as ast, Expr, Operator, Stmt};

use super::source::{PySource, for_each_statement};

/// The type an annotation names, as written: `GraphStore`, `pkg.Store`;
/// `Optional[T]`, `T | None`, and `"T"` give `T`; a generic `list[str]`
/// gives `list`. `None` for `None` or anything else.
pub(super) fn python_annotation_type(expr: &Expr, src: &PySource<'_>) -> Option<String> {
    match expr {
        Expr::Name(_) | Expr::Attribute(_) => Some(src.slice(expr).to_string()),
        Expr::StringLiteral(_) => {
            let text = src.slice(expr);
            let inner = text.trim_matches(|c| c == '"' || c == '\'').trim();
            let valid = !inner.is_empty()
                && inner
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '.');
            valid.then(|| inner.to_string())
        }
        // `T | None`
        Expr::BinOp(binary) if binary.op == Operator::BitOr => {
            let types = [&*binary.left, &*binary.right]
                .into_iter()
                .filter_map(|side| python_annotation_type(side, src))
                .collect::<Vec<_>>();
            match types.as_slice() {
                [single] => Some(single.clone()),
                _ => None,
            }
        }
        // `Optional[T]` / `Union[T, None]` give `T`; `list[str]` gives `list`.
        Expr::Subscript(subscript) => {
            let base_name = python_annotation_type(&subscript.value, src)?;
            let wrapper = base_name.rsplit('.').next().unwrap_or(&base_name);
            if matches!(wrapper, "Optional" | "Union") {
                let arguments = match &*subscript.slice {
                    Expr::Tuple(tuple) => tuple.elts.iter().collect::<Vec<_>>(),
                    slice => vec![slice],
                };
                let arguments = arguments
                    .into_iter()
                    .filter_map(|argument| python_annotation_type(argument, src))
                    .collect::<Vec<_>>();
                return match arguments.as_slice() {
                    [single] => Some(single.clone()),
                    _ => None,
                };
            }
            Some(base_name)
        }
        _ => None,
    }
}

/// `(name, annotated type)` of each annotated parameter that is neither
/// `*args` nor `**kwargs`.
pub(super) fn python_parameter_types(
    parameters: &ast::Parameters,
    src: &PySource<'_>,
) -> Vec<(String, String)> {
    parameters
        .iter_non_variadic_params()
        .filter_map(|parameter| {
            let annotation = parameter.parameter.annotation.as_deref()?;
            let annotation = python_annotation_type(annotation, src)?;
            Some((parameter.parameter.name.to_string(), annotation))
        })
        .collect()
}

/// Class name -> attribute -> type, for every class of the file.
pub(super) type AttributeTypes = HashMap<String, HashMap<String, String>>;

pub(super) fn collect_python_attribute_types(body: &[Stmt], src: &PySource<'_>) -> AttributeTypes {
    let mut types = AttributeTypes::new();
    for_each_statement(body, |stmt| {
        if let Stmt::ClassDef(class) = stmt
            && !class.name.is_empty()
        {
            let attributes = collect_class_attributes(class, src);
            types
                .entry(class.name.to_string())
                .or_default()
                .extend(attributes);
        }
    });
    types
}

fn collect_class_attributes(
    class: &ast::StmtClassDef,
    src: &PySource<'_>,
) -> HashMap<String, String> {
    let mut attributes = HashMap::new();
    // `store: GraphStore` in the class body (dataclasses, attrs).
    for statement in &class.body {
        if let Stmt::AnnAssign(assignment) = statement
            && let Expr::Name(name) = &*assignment.target
            && let Some(annotation) = python_annotation_type(&assignment.annotation, src)
        {
            attributes.insert(name.id.to_string(), annotation);
        }
    }
    for statement in &class.body {
        let Stmt::FunctionDef(function) = statement else {
            continue;
        };
        let parameters = python_parameter_types(&function.parameters, src)
            .into_iter()
            .collect::<HashMap<_, _>>();
        let mut collector = SelfAssignments {
            src,
            parameters: &parameters,
            attributes: &mut attributes,
        };
        collector.visit_body(&function.body);
    }
    attributes
}

/// `self.x: T = ...`, `self.x = T(...)`, `self.x = param` (annotated).
struct SelfAssignments<'a, 's> {
    src: &'a PySource<'s>,
    parameters: &'a HashMap<String, String>,
    attributes: &'a mut HashMap<String, String>,
}

impl<'ast> StatementVisitor<'ast> for SelfAssignments<'_, '_> {
    fn visit_stmt(&mut self, stmt: &'ast Stmt) {
        match stmt {
            // Nested functions and classes have their own `self`.
            Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {}
            // `self.a = self.b = T()`: only the last target takes the value.
            Stmt::Assign(assignment) => {
                if let Some(target) = assignment.targets.last() {
                    self.record(target, None, Some(&assignment.value));
                }
            }
            Stmt::AnnAssign(assignment) => self.record(
                &assignment.target,
                Some(&assignment.annotation),
                assignment.value.as_deref(),
            ),
            _ => walk_stmt(self, stmt),
        }
    }
}

impl SelfAssignments<'_, '_> {
    fn record(&mut self, target: &Expr, annotation: Option<&Expr>, value: Option<&Expr>) {
        let Expr::Attribute(attribute) = target else {
            return;
        };
        if self.src.slice(&*attribute.value) != "self" || attribute.attr.is_empty() {
            return;
        }
        let annotated =
            annotation.and_then(|annotation| python_annotation_type(annotation, self.src));
        let assigned = value.and_then(|value| match value {
            // A class is capitalized; `self.x = make()` says nothing.
            Expr::Call(call) => matches!(&*call.func, Expr::Name(_) | Expr::Attribute(_))
                .then(|| self.src.slice(&*call.func).to_string())
                .filter(|callee| {
                    callee
                        .rsplit('.')
                        .next()
                        .is_some_and(|name| name.starts_with(|c: char| c.is_ascii_uppercase()))
                }),
            Expr::Name(name) => self.parameters.get(name.id.as_str()).cloned(),
            _ => None,
        });
        if let Some(type_name) = annotated.or(assigned) {
            self.attributes
                .entry(attribute.attr.to_string())
                .or_insert(type_name);
        }
    }
}
