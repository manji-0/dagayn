//! Command-line helpers shared by the build-manifest scanners.

use std::collections::{HashMap, HashSet};

/// Split a command line like POSIX `shlex.split` (no comment stripping),
/// falling back to whitespace splitting on unbalanced quotes or a trailing
/// escape.
pub(super) fn split_command(text: &str) -> Vec<String> {
    shlex_split(text).unwrap_or_else(|| text.split_whitespace().map(str::to_string).collect())
}

#[derive(Clone, Copy, PartialEq)]
enum Lex {
    Space,
    Word,
    Quote(char),
    Escape,
}

fn shlex_split(text: &str) -> Option<Vec<String>> {
    let is_space = |ch: char| matches!(ch, ' ' | '\t' | '\r' | '\n');
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut quoted = false;
    let mut state = Lex::Space;
    // The state an escape returns to: a word, or the double quote it sits in.
    let mut escaped_from = Lex::Word;
    for ch in text.chars() {
        match state {
            Lex::Space => {
                if is_space(ch) {
                } else if ch == '\\' {
                    escaped_from = Lex::Word;
                    state = Lex::Escape;
                } else if ch == '"' || ch == '\'' {
                    state = Lex::Quote(ch);
                } else {
                    token.push(ch);
                    state = Lex::Word;
                }
            }
            Lex::Quote(quote) => {
                quoted = true;
                if ch == quote {
                    state = Lex::Word;
                } else if ch == '\\' && quote == '"' {
                    escaped_from = state;
                    state = Lex::Escape;
                } else {
                    token.push(ch);
                }
            }
            Lex::Escape => {
                // Inside double quotes only `\\` and `\"` are escapes.
                if let Lex::Quote(quote) = escaped_from
                    && ch != '\\'
                    && ch != quote
                {
                    token.push('\\');
                }
                token.push(ch);
                state = escaped_from;
            }
            Lex::Word => {
                if is_space(ch) {
                    state = Lex::Space;
                    if !token.is_empty() || quoted {
                        tokens.push(std::mem::take(&mut token));
                        quoted = false;
                    }
                } else if ch == '"' || ch == '\'' {
                    state = Lex::Quote(ch);
                } else if ch == '\\' {
                    escaped_from = Lex::Word;
                    state = Lex::Escape;
                } else {
                    token.push(ch);
                }
            }
        }
    }
    match state {
        Lex::Quote(_) | Lex::Escape => return None,
        Lex::Word if !token.is_empty() || quoted => tokens.push(token),
        _ => {}
    }
    Some(tokens)
}

/// CLI tokens split into `{flag: value}` and positional arguments.
pub(super) struct CommandOptions {
    pub options: HashMap<String, String>,
    pub positional: Vec<String>,
}

impl CommandOptions {
    pub fn get(&self, flag: &str) -> Option<&str> {
        self.options.get(flag).map(String::as_str)
    }

    /// The value of the first of `flags` given a non-empty value, else the
    /// last flag's value (Python's `options.get(a) or options.get(b)`).
    pub fn first_non_empty(&self, flags: &[&str]) -> Option<&str> {
        let mut value = None;
        for flag in flags {
            value = self.get(flag);
            if value.is_some_and(|value| !value.is_empty()) {
                break;
            }
        }
        value
    }
}

pub(super) fn command_options(tokens: &[String], valued: &HashSet<&str>) -> CommandOptions {
    let mut options = HashMap::new();
    let mut positional = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let token = &tokens[index];
        if token.starts_with('-') {
            if let Some((flag, value)) = token.split_once('=') {
                options.insert(flag.to_string(), value.to_string());
            } else if valued.contains(token.as_str()) && index + 1 < tokens.len() {
                options.insert(token.clone(), tokens[index + 1].clone());
                index += 1;
            } else {
                options.insert(token.clone(), String::new());
            }
        } else if !token.contains('=') || token.starts_with('.') {
            positional.push(token.clone());
        }
        index += 1;
    }
    CommandOptions {
        options,
        positional,
    }
}

pub(super) fn strip_quotes(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 && bytes[0] == bytes[bytes.len() - 1] && matches!(bytes[0], b'\'' | b'"') {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shlex_matches_python_posix_mode() {
        assert_eq!(
            split_command(r#"a "b c" 'd e' f\ g"#),
            ["a", "b c", "d e", "f g"]
        );
        assert_eq!(
            split_command(r#""a\"b" "x\y" '\n'"#),
            [r#"a"b"#, r"x\y", r"\n"]
        );
        assert_eq!(split_command(r#"x "" y"#), ["x", "", "y"]);
        // Unbalanced quote: whitespace split.
        assert_eq!(split_command(r#"a "b c"#), ["a", "\"b", "c"]);
    }

    #[test]
    fn options_split_flags_and_positionals() {
        let tokens = split_command("-o out.wasm --target=wasm32 FOO=1 ./pkg -v");
        let parsed = command_options(&tokens, &HashSet::from(["-o"]));
        assert_eq!(parsed.get("-o"), Some("out.wasm"));
        assert_eq!(parsed.get("--target"), Some("wasm32"));
        assert_eq!(parsed.get("-v"), Some(""));
        assert_eq!(parsed.positional, ["./pkg"]);
    }
}
