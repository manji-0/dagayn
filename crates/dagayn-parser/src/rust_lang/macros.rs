//! `macro_rules!` definitions of a file, and the rule an invocation of one
//! of them expands.
//!
//! tree-sitter keeps a macro's body as a token tree, so its calls are read
//! from the tokens (`rust_emit_token_tree_calls`). A path the body spells
//! out (`tree_sitter::Parser::new()`, `$crate::util::f()`) is called from
//! the definition, once. A path a metavariable completes
//! (`dagayn_grammars::$language()`) is called at each invocation whose
//! arguments a rule matches. The matcher reads flat rules only: literal
//! tokens, nested delimited groups, and `$name:fragment` bindings. A rule
//! with a repetition (`$(...)*`) stops it, since whether that rule applies
//! is unknown; so does a name the file defines twice.

use std::collections::HashMap;

use super::super::util::node_text;

/// The macros the file defines by name; `None` when it defines a name more
/// than once (textual scoping decides which one an invocation reaches).
pub(super) type LocalMacros<'tree> = HashMap<String, Option<tree_sitter::Node<'tree>>>;

pub(super) fn rust_local_macros<'tree>(
    root: tree_sitter::Node<'tree>,
    source: &[u8],
) -> LocalMacros<'tree> {
    let mut macros = LocalMacros::new();
    if !source
        .windows(b"macro_rules!".len())
        .any(|window| window == b"macro_rules!")
    {
        return macros;
    }
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "macro_definition" {
            if let Some(name) = node.child_by_field_name("name") {
                macros
                    .entry(node_text(name, source))
                    .and_modify(|seen| *seen = None)
                    .or_insert(Some(node));
            }
            continue;
        }
        let mut cursor = node.walk();
        stack.extend(node.children(&mut cursor));
    }
    macros
}

/// The bodies (`right` token trees) of a definition's rules.
pub(super) fn rust_macro_bodies<'tree>(
    definition: tree_sitter::Node<'tree>,
) -> Vec<tree_sitter::Node<'tree>> {
    let mut cursor = definition.walk();
    definition
        .children(&mut cursor)
        .filter(|rule| rule.kind() == "macro_rule")
        .filter_map(|rule| rule.child_by_field_name("right"))
        .collect()
}

/// The body of the first rule whose matcher accepts the invocation's
/// arguments, with the text of each metavariable it binds (`$name` ->
/// `text`). `None` when no rule matches, or when a rule the matcher cannot
/// read comes before the one that would.
pub(super) fn rust_expand_macro<'tree>(
    definition: tree_sitter::Node<'tree>,
    arguments: tree_sitter::Node<'tree>,
    source: &[u8],
) -> Option<(tree_sitter::Node<'tree>, HashMap<String, String>)> {
    let tokens = delimited_contents(arguments);
    let mut cursor = definition.walk();
    for rule in definition
        .children(&mut cursor)
        .filter(|rule| rule.kind() == "macro_rule")
    {
        let pattern = rule.child_by_field_name("left")?;
        let body = rule.child_by_field_name("right")?;
        let mut bindings = HashMap::new();
        match match_sequence(&delimited_contents(pattern), &tokens, source, &mut bindings) {
            Matched::Yes => return Some((body, bindings)),
            Matched::No => {}
            Matched::Unreadable => return None,
        }
    }
    None
}

enum Matched {
    Yes,
    No,
    /// The rule has a form the matcher does not read.
    Unreadable,
}

/// A group's children without its delimiters.
fn delimited_contents(group: tree_sitter::Node<'_>) -> Vec<tree_sitter::Node<'_>> {
    let mut cursor = group.walk();
    let children: Vec<_> = group.children(&mut cursor).collect();
    match children.len() {
        0 | 1 => Vec::new(),
        count => children[1..count - 1].to_vec(),
    }
}

fn opening(group: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
    group.child(0).map(|open| node_text(open, source))
}

fn match_sequence(
    pattern: &[tree_sitter::Node<'_>],
    tokens: &[tree_sitter::Node<'_>],
    source: &[u8],
    bindings: &mut HashMap<String, String>,
) -> Matched {
    let mut at = 0;
    for (index, element) in pattern.iter().enumerate() {
        match element.kind() {
            "token_repetition_pattern" => return Matched::Unreadable,
            "token_tree_pattern" => {
                let Some(group) = tokens.get(at) else {
                    return Matched::No;
                };
                if group.kind() != "token_tree"
                    || opening(*group, source) != opening(*element, source)
                {
                    return Matched::No;
                }
                match match_sequence(
                    &delimited_contents(*element),
                    &delimited_contents(*group),
                    source,
                    bindings,
                ) {
                    Matched::Yes => at += 1,
                    other => return other,
                }
            }
            "token_binding_pattern" => {
                let (Some(name), Some(fragment)) = (
                    element.child_by_field_name("name"),
                    element.child_by_field_name("type"),
                ) else {
                    return Matched::Unreadable;
                };
                let taken = match take_fragment(
                    &node_text(fragment, source),
                    pattern.get(index + 1),
                    &tokens[at..],
                    source,
                ) {
                    Ok(Some(taken)) => taken,
                    Ok(None) => return Matched::No,
                    Err(()) => return Matched::Unreadable,
                };
                let text: String = tokens[at..at + taken]
                    .iter()
                    .map(|token| node_text(*token, source))
                    .collect();
                if bindings.insert(node_text(name, source), text).is_some() {
                    return Matched::Unreadable;
                }
                at += taken;
            }
            _ => {
                let Some(token) = tokens.get(at) else {
                    return Matched::No;
                };
                if token.kind() == "token_tree"
                    || node_text(*token, source) != node_text(*element, source)
                {
                    return Matched::No;
                }
                at += 1;
            }
        }
    }
    if at == tokens.len() {
        Matched::Yes
    } else {
        Matched::No
    }
}

fn is_identifier(text: &str) -> bool {
    text.starts_with(|c: char| c.is_alphabetic() || c == '_')
        && text != "_"
        && text.chars().all(|c| c.is_alphanumeric() || c == '_')
}

/// How many tokens a `$name:fragment` binding takes from the front of
/// `tokens`: `Ok(None)` when they cannot start that fragment, `Err` when the
/// matcher cannot tell where it ends. A fragment of several tokens (`expr`,
/// `ty`, `path`, ...) ends right before the literal token that follows it
/// in the pattern (`,` in `$a:expr, $b:expr`), or at the end of the
/// arguments when it is last.
fn take_fragment(
    fragment: &str,
    next: Option<&tree_sitter::Node<'_>>,
    tokens: &[tree_sitter::Node<'_>],
    source: &[u8],
) -> Result<Option<usize>, ()> {
    let text = |at: usize| tokens.get(at).map(|token| node_text(*token, source));
    let leaf = |at: usize| {
        tokens
            .get(at)
            .is_some_and(|token| token.kind() != "token_tree")
    };
    Ok(match fragment {
        "ident" => (leaf(0) && text(0).is_some_and(|text| is_identifier(&text))).then_some(1),
        "lifetime" => (text(0).as_deref() == Some("'")
            && text(1).is_some_and(|text| is_identifier(&text)))
        .then_some(2),
        "literal" => {
            let literal = |at: usize| {
                tokens.get(at).is_some_and(|token| {
                    token.kind().ends_with("_literal")
                        || matches!(node_text(*token, source).as_str(), "true" | "false")
                })
            };
            if literal(0) {
                Some(1)
            } else if text(0).as_deref() == Some("-") && literal(1) {
                Some(2)
            } else {
                None
            }
        }
        "tt" => (!tokens.is_empty()).then_some(1),
        "block" => tokens
            .first()
            .is_some_and(|token| {
                token.kind() == "token_tree" && opening(*token, source).as_deref() == Some("{")
            })
            .then_some(1),
        "vis" => Some(match text(0).as_deref() {
            Some("pub")
                if tokens.get(1).is_some_and(|token| {
                    token.kind() == "token_tree" && opening(*token, source).as_deref() == Some("(")
                }) =>
            {
                2
            }
            Some("pub") => 1,
            _ => 0,
        }),
        "expr" | "ty" | "path" | "pat" | "pat_param" | "stmt" | "item" | "meta" => {
            let stop = match next {
                None => None,
                Some(next)
                    if matches!(
                        next.kind(),
                        "token_binding_pattern" | "token_tree_pattern" | "token_repetition_pattern"
                    ) =>
                {
                    return Err(());
                }
                Some(next) => Some(node_text(*next, source)),
            };
            // Generic arguments are not a group: `HashMap<K, V>` holds a
            // comma outside one.
            let angles = matches!(fragment, "ty" | "path");
            let mut depth = 0i32;
            let mut taken = 0;
            while let Some(token) = tokens.get(taken) {
                let token_text = node_text(*token, source);
                if depth == 0 && stop.as_deref() == Some(token_text.as_str()) {
                    break;
                }
                if angles && token.kind() != "token_tree" {
                    match token_text.as_str() {
                        "<" => depth += 1,
                        ">" => depth -= 1,
                        ">>" => depth -= 2,
                        _ => {}
                    }
                }
                taken += 1;
            }
            let ended = stop.is_none() || taken < tokens.len();
            (taken > 0 && ended).then_some(taken)
        }
        _ => return Err(()),
    })
}
