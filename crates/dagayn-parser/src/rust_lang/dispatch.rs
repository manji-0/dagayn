//! What a call passes and which string arm it sits in, for dispatch-aware
//! test reach (docs/plans/TEST-REACH-TARGET.md#dispatch-aware-reach).
//!
//! A `CALLS` edge records `arguments`, one entry per argument: the text of a
//! string literal, `{"parameter": i, "of": n}` for the caller's own `i`-th
//! string parameter (of `n`, `self` not counted) passed on, or `null`. A
//! call inside a `match` arm on a string parameter (`"review_tool" =>
//! review(..)`) or an `if p == "x"` branch records `dispatch`: one
//! `{"parameter": i, "of": n, "values": [..]}` per enclosing arm. Both are
//! written only when they say something; a call in a closure records only
//! its literals, since a closure's parameters are not the function's.

use serde_json::{Value, json};

use super::super::util::node_text;

/// `extra["arguments"]` and `extra["dispatch"]` for `call` (a
/// `call_expression`), when there is anything to record.
pub(super) fn rust_dispatch_facts(call: tree_sitter::Node<'_>, source: &[u8], extra: &mut Value) {
    let list = call.child_by_field_name("arguments");
    record(call, source, extra, |parameters| {
        list.map(|list| {
            let mut cursor = list.walk();
            list.named_children(&mut cursor)
                .filter(|argument| !matches!(argument.kind(), "line_comment" | "block_comment"))
                .map(|argument| argument_entry(call, &[argument], source, parameters))
                .collect()
        })
    });
}

/// The same for a call inside a macro's tokens (`assert!(run("double"))`),
/// `arguments` the call's parenthesised token tree.
pub(super) fn rust_token_dispatch_facts(
    arguments: tree_sitter::Node<'_>,
    source: &[u8],
    extra: &mut Value,
) {
    record(arguments, source, extra, |parameters| {
        let mut cursor = arguments.walk();
        let tokens: Vec<_> = arguments.children(&mut cursor).collect();
        let inner = tokens.get(1..tokens.len().checked_sub(1)?)?;
        if inner.is_empty() {
            return Some(Vec::new());
        }
        let mut entries: Vec<Value> = inner
            .split(|token| token.kind() == ",")
            .map(|piece| argument_entry(arguments, piece, source, parameters))
            .collect();
        // A trailing comma leaves an empty last piece.
        if inner.last().is_some_and(|token| token.kind() == ",") {
            entries.pop();
        }
        Some(entries)
    });
}

fn record(
    anchor: tree_sitter::Node<'_>,
    source: &[u8],
    extra: &mut Value,
    arguments: impl FnOnce(&Parameters) -> Option<Vec<Value>>,
) {
    // In a closure only the literals are read: its parameters are not the
    // function's.
    let function = enclosing_function(anchor);
    let parameters = function.map_or(Parameters { names: Vec::new() }, |function| {
        string_parameters(function, source)
    });
    if let Some(entries) = arguments(&parameters)
        && entries.iter().any(|entry| !entry.is_null())
    {
        extra["arguments"] = Value::Array(entries);
    }
    let Some(function) = function else {
        return;
    };
    if parameters.names.iter().all(Option::is_none) {
        return;
    }
    let guards = enclosing_guards(anchor, function, source, &parameters);
    if !guards.is_empty() {
        extra["dispatch"] = Value::Array(guards);
    }
}

/// The `function_item` whose body holds `node`, unless a closure comes
/// first.
fn enclosing_function(node: tree_sitter::Node<'_>) -> Option<tree_sitter::Node<'_>> {
    let mut current = node.parent();
    while let Some(parent) = current {
        match parent.kind() {
            "function_item" => return Some(parent),
            "closure_expression" => return None,
            _ => current = parent.parent(),
        }
    }
    None
}

/// A function's parameters without `self`: the name of each that holds a
/// string (`&str`, `String`, `Cow<str>`, ...), `None` for the others.
struct Parameters {
    names: Vec<Option<String>>,
}

impl Parameters {
    fn position(&self, name: &str) -> Option<usize> {
        self.names
            .iter()
            .position(|parameter| parameter.as_deref() == Some(name))
    }

    fn arity(&self) -> usize {
        self.names.len()
    }
}

fn string_parameters(function: tree_sitter::Node<'_>, source: &[u8]) -> Parameters {
    let mut names = Vec::new();
    if let Some(list) = function.child_by_field_name("parameters") {
        let mut cursor = list.walk();
        for parameter in list.named_children(&mut cursor) {
            if parameter.kind() != "parameter" {
                // `self_parameter`, attributes, comments: not positional.
                continue;
            }
            let name = parameter
                .child_by_field_name("pattern")
                .filter(|pattern| pattern.kind() == "identifier")
                .map(|pattern| node_text(pattern, source));
            // `mut name: &str` may be assigned another string first.
            let mut cursor = parameter.walk();
            let mutable = parameter
                .children(&mut cursor)
                .any(|part| part.kind() == "mutable_specifier");
            let holds_string = !mutable
                && parameter
                    .child_by_field_name("type")
                    .is_some_and(|ty| is_string_type(&node_text(ty, source)));
            names.push(name.filter(|_| holds_string));
        }
    }
    Parameters { names }
}

/// `&str`, `&'a str`, `String`, `&String`, `Cow<'_, str>`, `impl AsRef<str>`.
fn is_string_type(text: &str) -> bool {
    let mut bare = text.trim().trim_start_matches('&').trim_start();
    if let Some(lifetime) = bare.strip_prefix('\'') {
        bare = lifetime
            .trim_start_matches(|c: char| c.is_alphanumeric() || c == '_')
            .trim_start();
    }
    let compact: String = bare.chars().filter(|c| !c.is_whitespace()).collect();
    matches!(compact.as_str(), "str" | "String" | "implAsRef<str>")
        || compact.starts_with("Cow<") && compact.ends_with("str>")
}

/// Methods that keep a string's text.
const KEEPS_TEXT: &[&str] = &["as_str", "as_ref", "clone", "to_string", "to_owned", "into"];

/// The parameter an expression is, through `&`, `*`, and the methods that
/// keep a string's text (`name.as_str()`, `name.clone()`).
fn parameter_of(expression: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    match expression.kind() {
        "identifier" => Some(node_text(expression, source)),
        "reference_expression" | "unary_expression" | "parenthesized_expression" => {
            let mut cursor = expression.walk();
            let inner = expression.named_children(&mut cursor).last()?;
            parameter_of(inner, source)
        }
        "call_expression" => {
            let function = expression.child_by_field_name("function")?;
            let arguments = expression.child_by_field_name("arguments")?;
            if function.kind() != "field_expression" || arguments.named_child_count() != 0 {
                return None;
            }
            let method = node_text(function.child_by_field_name("field")?, source);
            if !KEEPS_TEXT.contains(&method.as_str()) {
                return None;
            }
            parameter_of(function.child_by_field_name("value")?, source)
        }
        _ => None,
    }
}

/// A string literal's text, unless it has escapes (then it is not compared).
fn string_literal(node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    if node.kind() != "string_literal" {
        return None;
    }
    let mut cursor = node.walk();
    let mut text = String::new();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "string_content" => text.push_str(&node_text(child, source)),
            _ => return None,
        }
    }
    Some(text)
}

/// One argument's entry: a string literal's text, the strings a local
/// holds (`{"strings": [..]}`: `let name = "x"`, or `for (name, ..) in
/// [("a", ..), ("b", ..)]`), the caller's string parameter passed on, or
/// `null`. `piece` is one expression node, or the tokens between two commas
/// of a macro's token tree (`&name`, `name.as_str()`).
fn argument_entry(
    anchor: tree_sitter::Node<'_>,
    piece: &[tree_sitter::Node<'_>],
    source: &[u8],
    parameters: &Parameters,
) -> Value {
    let piece: Vec<_> = piece
        .iter()
        .copied()
        .skip_while(|token| matches!(token.kind(), "&" | "*"))
        .collect();
    if let [single] = piece.as_slice()
        && let Some(text) = string_literal(*single, source)
    {
        return json!(text);
    }
    let name = match piece.as_slice() {
        [single] => parameter_of(*single, source),
        [name, dot, method, call]
            if name.kind() == "identifier"
                && dot.kind() == "."
                && KEEPS_TEXT.contains(&node_text(*method, source).as_str())
                && node_text(*call, source).replace(char::is_whitespace, "") == "()" =>
        {
            Some(node_text(*name, source))
        }
        _ => None,
    };
    let Some(name) = name else {
        return Value::Null;
    };
    match local_strings(anchor, &name, source) {
        Some(Some(strings)) => json!({"strings": strings}),
        Some(None) => Value::Null,
        None => match parameters.position(&name) {
            Some(position) => json!({"parameter": position, "of": parameters.arity()}),
            None => Value::Null,
        },
    }
}

/// What a local `name` visible at `anchor` holds: `None` when no `let` or
/// `for` in the function binds it, `Some(None)` when one binds it to
/// something other than string literals.
fn local_strings(
    anchor: tree_sitter::Node<'_>,
    name: &str,
    source: &[u8],
) -> Option<Option<Vec<String>>> {
    let mut child = anchor;
    while let Some(parent) = child.parent() {
        match parent.kind() {
            "function_item" | "closure_expression" => return None,
            "for_expression"
                if parent.child_by_field_name("body").map(|body| body.id()) == Some(child.id()) =>
            {
                let pattern = parent.child_by_field_name("pattern")?;
                if let Some(position) = bound_position(pattern, name, source) {
                    let value = parent.child_by_field_name("value")?;
                    return Some(loop_strings(parent, value, position, source));
                }
            }
            "block" => {
                if let Some(value) = let_in_block(parent, child, name, source) {
                    return Some(
                        value
                            .and_then(|value| string_literal(value, source))
                            .map(|text| vec![text]),
                    );
                }
            }
            // `match e { Some(name) => .. }`, `if let Some(name) = ..`: a
            // binding of another value, not followed.
            "match_arm"
                if parent.child_by_field_name("value").map(|v| v.id()) == Some(child.id())
                    && parent
                        .child_by_field_name("pattern")
                        .is_some_and(|pattern| bound_position(pattern, name, source).is_some()) =>
            {
                return Some(None);
            }
            "if_expression" | "while_expression"
                if parent
                    .child_by_field_name("condition")
                    .is_some_and(|condition| {
                        condition.id() != child.id() && binds_in_let(condition, name, source)
                    }) =>
            {
                return Some(None);
            }
            _ => {}
        }
        child = parent;
    }
    None
}

/// Whether a condition's `let` pattern (`if let Some(name) = ..`, a `let`
/// chain) binds `name`.
fn binds_in_let(condition: tree_sitter::Node<'_>, name: &str, source: &[u8]) -> bool {
    if condition.kind() == "let_condition" {
        return condition
            .child_by_field_name("pattern")
            .is_some_and(|pattern| bound_position(pattern, name, source).is_some());
    }
    let mut cursor = condition.walk();
    condition
        .named_children(&mut cursor)
        .any(|part| binds_in_let(part, name, source))
}

/// The value the last `let name = ..` of `block` before `child` binds:
/// `None` when none does, `Some(None)` when its pattern is not just `name`.
fn let_in_block<'tree>(
    block: tree_sitter::Node<'tree>,
    child: tree_sitter::Node<'tree>,
    name: &str,
    source: &[u8],
) -> Option<Option<tree_sitter::Node<'tree>>> {
    let mut cursor = block.walk();
    let statement = block
        .named_children(&mut cursor)
        .take_while(|statement| statement.end_byte() <= child.start_byte())
        .filter(|statement| statement.kind() == "let_declaration")
        .filter(|statement| {
            statement
                .child_by_field_name("pattern")
                .is_some_and(|pattern| bound_position(pattern, name, source).is_some())
        })
        .last()?;
    // `let mut tool = "a"` may be assigned another string later.
    let mut cursor = statement.walk();
    let mutable = statement
        .children(&mut cursor)
        .any(|part| part.kind() == "mutable_specifier");
    Some(
        statement
            .child_by_field_name("pattern")
            .filter(|pattern| pattern.kind() == "identifier" && !mutable)
            .and_then(|_| statement.child_by_field_name("value")),
    )
}

/// The value a `let` before `anchor` in the function binds `name` to.
fn let_value<'tree>(
    anchor: tree_sitter::Node<'tree>,
    name: &str,
    source: &[u8],
) -> Option<tree_sitter::Node<'tree>> {
    let mut child = anchor;
    while let Some(parent) = child.parent() {
        match parent.kind() {
            "function_item" | "closure_expression" => return None,
            "block" => {
                if let Some(value) = let_in_block(parent, child, name, source) {
                    return value;
                }
            }
            _ => {}
        }
        child = parent;
    }
    None
}

/// Where a pattern binds `name`: `Some(None)` for the whole value
/// (`name`), `Some(Some(k))` for the `k`-th of a tuple (`(name, args)`).
fn bound_position(
    pattern: tree_sitter::Node<'_>,
    name: &str,
    source: &[u8],
) -> Option<Option<usize>> {
    match pattern.kind() {
        "identifier" => (node_text(pattern, source) == name).then_some(None),
        "tuple_pattern" => {
            let mut cursor = pattern.walk();
            let mut position = None;
            for (index, part) in pattern.named_children(&mut cursor).enumerate() {
                if part.kind() == "identifier" && node_text(part, source) == name {
                    position = Some(Some(index));
                } else if node_text(part, source)
                    .split(|c: char| !c.is_alphanumeric() && c != '_')
                    .any(|word| word == name)
                {
                    // Bound deeper (`(Some(name), _)`): not followed.
                    return Some(Some(usize::MAX));
                }
            }
            position
        }
        _ => node_text(pattern, source)
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .any(|word| word == name)
            .then_some(Some(usize::MAX)),
    }
}

/// The strings a `for` over an array literal gives its pattern's `position`.
/// `value` may name a local array (`let cases = [..]; for (tool, _) in
/// &cases`).
fn loop_strings(
    for_loop: tree_sitter::Node<'_>,
    value: tree_sitter::Node<'_>,
    position: Option<usize>,
    source: &[u8],
) -> Option<Vec<String>> {
    let mut value = value;
    while value.kind() == "reference_expression" {
        value = value.child_by_field_name("value")?;
    }
    if value.kind() == "identifier" {
        value = let_value(for_loop, &node_text(value, source), source)?;
    }
    if value.kind() != "array_expression" {
        return None;
    }
    let mut cursor = value.walk();
    let mut strings = Vec::new();
    for element in value.named_children(&mut cursor) {
        let item = match position {
            None => element,
            Some(index) => {
                if element.kind() != "tuple_expression" {
                    return None;
                }
                let mut cursor = element.walk();
                element.named_children(&mut cursor).nth(index)?
            }
        };
        strings.push(string_literal(item, source)?);
    }
    strings.sort();
    strings.dedup();
    Some(strings)
}

/// One guard per `match` arm and `if` branch between `call` and `function`
/// that selects on a string parameter.
fn enclosing_guards(
    call: tree_sitter::Node<'_>,
    function: tree_sitter::Node<'_>,
    source: &[u8],
    parameters: &Parameters,
) -> Vec<Value> {
    let mut guards = Vec::new();
    let mut child = call;
    while let Some(parent) = child.parent() {
        if parent.id() == function.id() {
            break;
        }
        let selected = match parent.kind() {
            "match_arm"
                if parent.child_by_field_name("value").map(|v| v.id()) == Some(child.id()) =>
            {
                arm_guard(parent, source, parameters)
            }
            "if_expression"
                if parent.child_by_field_name("consequence").map(|c| c.id())
                    == Some(child.id()) =>
            {
                parent
                    .child_by_field_name("condition")
                    .and_then(|condition| condition_guard(condition, source, parameters))
            }
            _ => None,
        };
        if let Some((position, values)) = selected {
            guards.push(json!({"parameter": position, "of": parameters.arity(), "values": values}));
        }
        child = parent;
    }
    guards
}

/// `"a" | "b" => ..` in `match p { .. }` (also `match p.as_str()`).
fn arm_guard(
    arm: tree_sitter::Node<'_>,
    source: &[u8],
    parameters: &Parameters,
) -> Option<(usize, Vec<String>)> {
    let scrutinee = arm.parent()?.parent()?.child_by_field_name("value")?;
    let position = parameters.position(&parameter_of(scrutinee, source)?)?;
    let pattern = arm.child_by_field_name("pattern")?;
    let mut cursor = pattern.walk();
    let inner = pattern
        .named_children(&mut cursor)
        .find(|part| Some(part.id()) != pattern.child_by_field_name("condition").map(|c| c.id()))?;
    let mut values = Vec::new();
    pattern_literals(inner, source, &mut values)?;
    Some((position, values))
}

fn pattern_literals(
    pattern: tree_sitter::Node<'_>,
    source: &[u8],
    values: &mut Vec<String>,
) -> Option<()> {
    if pattern.kind() == "or_pattern" {
        let mut cursor = pattern.walk();
        for part in pattern.named_children(&mut cursor) {
            pattern_literals(part, source, values)?;
        }
        return Some(());
    }
    values.push(string_literal(pattern, source)?);
    Some(())
}

/// `p == "a"`, `"a" == p`, and `||` of those on one parameter.
fn condition_guard(
    condition: tree_sitter::Node<'_>,
    source: &[u8],
    parameters: &Parameters,
) -> Option<(usize, Vec<String>)> {
    match condition.kind() {
        "parenthesized_expression" => {
            let mut cursor = condition.walk();
            let inner = condition.named_children(&mut cursor).next()?;
            condition_guard(inner, source, parameters)
        }
        "binary_expression" => {
            let left = condition.child_by_field_name("left")?;
            let right = condition.child_by_field_name("right")?;
            let operator = condition.child_by_field_name("operator")?;
            match operator.kind() {
                "==" => {
                    let (name, value) =
                        match (string_literal(left, source), string_literal(right, source)) {
                            (None, Some(value)) => (parameter_of(left, source)?, value),
                            (Some(value), None) => (parameter_of(right, source)?, value),
                            _ => return None,
                        };
                    Some((parameters.position(&name)?, vec![value]))
                }
                "||" => {
                    let (position, mut values) = condition_guard(left, source, parameters)?;
                    let (other, more) = condition_guard(right, source, parameters)?;
                    (position == other).then(|| {
                        values.extend(more);
                        (position, values)
                    })
                }
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::is_string_type;

    #[test]
    fn string_types_are_recognised() {
        for text in [
            "&str",
            "&'a str",
            "& 'static str",
            "String",
            "&String",
            "Cow<'_, str>",
            "impl AsRef<str>",
        ] {
            assert!(is_string_type(text), "{text}");
        }
        for text in ["i64", "&[u8]", "Option<&str>", "&Value", "Strings"] {
            assert!(!is_string_type(text), "{text}");
        }
    }
}
