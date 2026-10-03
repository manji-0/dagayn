//! A Python literal parser covering what `ast.literal_eval` accepts in
//! `binding.gyp` files: dicts, lists, tuples, sets, strings (with prefixes,
//! escapes, and implicit concatenation), numbers, `True` / `False` /
//! `None`, and comments. Anything else is an error, as it is to
//! `literal_eval`.

#[derive(Clone, Debug, PartialEq)]
pub(super) enum PyLiteral {
    None,
    Bool(bool),
    Number,
    Str(String),
    Bytes,
    List(Vec<PyLiteral>),
    Tuple(Vec<PyLiteral>),
    Set(Vec<PyLiteral>),
    Dict(Vec<(PyLiteral, PyLiteral)>),
}

impl PyLiteral {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            PyLiteral::Str(value) => Some(value),
            _ => None,
        }
    }

    /// `dict.get(key)` for a string key; the last duplicate wins.
    pub fn get(&self, key: &str) -> Option<&PyLiteral> {
        match self {
            PyLiteral::Dict(items) => items
                .iter()
                .rev()
                .find(|(name, _)| name.as_str() == Some(key))
                .map(|(_, value)| value),
            _ => None,
        }
    }
}

/// `ast.literal_eval(source)`.
pub(super) fn literal_eval(source: &str) -> Option<PyLiteral> {
    let source = source.trim_start_matches([' ', '\t']);
    let mut parser = Parser {
        chars: source.chars().collect(),
        pos: 0,
        depth: 0,
    };
    // The first logical line may not be indented.
    let first_code = parser.chars.split(|&ch| ch == '\n').find(|line| {
        let code = line
            .iter()
            .position(|&ch| !matches!(ch, ' ' | '\t' | '\r' | '\u{0c}'));
        code.is_some_and(|at| line[at] != '#')
    })?;
    if matches!(first_code.first(), Some(' ' | '\t')) {
        return None;
    }
    parser.skip_blank(true);
    let value = parser.expression_list()?;
    parser.skip_blank(true);
    (parser.pos == parser.chars.len()).then_some(value)
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
    /// Bracket nesting: newlines only separate tokens inside brackets.
    depth: usize,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    /// Skip spaces, comments, line continuations, and (inside brackets or
    /// when `newlines`) line breaks.
    fn skip_blank(&mut self, newlines: bool) {
        while let Some(ch) = self.peek() {
            match ch {
                ' ' | '\t' | '\u{0c}' | '\r' => self.pos += 1,
                '\n' if newlines || self.depth > 0 => self.pos += 1,
                '#' => {
                    while self.peek().is_some_and(|ch| ch != '\n') {
                        self.pos += 1;
                    }
                }
                '\\' if self.chars.get(self.pos + 1) == Some(&'\n') => self.pos += 2,
                _ => break,
            }
        }
    }

    fn skip(&mut self) {
        self.skip_blank(false);
    }

    fn eat(&mut self, ch: char) -> bool {
        self.skip();
        if self.peek() == Some(ch) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    /// A top-level `a, b` is a tuple, as in `eval` mode.
    fn expression_list(&mut self) -> Option<PyLiteral> {
        let first = self.value()?;
        self.skip();
        if self.peek() != Some(',') {
            return Some(first);
        }
        let mut items = vec![first];
        while self.eat(',') {
            self.skip();
            if self.peek().is_none_or(|ch| ch == '\n') {
                break;
            }
            items.push(self.value()?);
        }
        Some(PyLiteral::Tuple(items))
    }

    fn value(&mut self) -> Option<PyLiteral> {
        self.skip();
        let ch = self.peek()?;
        match ch {
            '{' => self.braces(),
            '[' => self.sequence(']').map(|(items, _)| PyLiteral::List(items)),
            '(' => {
                let (items, trailing_comma) = self.sequence(')')?;
                if items.len() == 1 && !trailing_comma {
                    items.into_iter().next()
                } else {
                    Some(PyLiteral::Tuple(items))
                }
            }
            '-' | '+' => {
                self.pos += 1;
                match self.value()? {
                    PyLiteral::Number => Some(PyLiteral::Number),
                    _ => None,
                }
            }
            '0'..='9' | '.' => self.number(),
            _ if ch.is_alphabetic() || ch == '_' => self.word(),
            '"' | '\'' => self.strings(String::new()),
            _ => None,
        }
    }

    /// `[...]` / `(...)` items and whether a comma trails the last one.
    fn sequence(&mut self, close: char) -> Option<(Vec<PyLiteral>, bool)> {
        self.pos += 1;
        self.depth += 1;
        let mut items = Vec::new();
        let mut trailing_comma = false;
        loop {
            if self.eat(close) {
                break;
            }
            items.push(self.value()?);
            trailing_comma = self.eat(',');
            if !trailing_comma {
                if self.eat(close) {
                    break;
                }
                return None;
            }
        }
        self.depth -= 1;
        Some((items, trailing_comma))
    }

    fn braces(&mut self) -> Option<PyLiteral> {
        self.pos += 1;
        self.depth += 1;
        if self.eat('}') {
            self.depth -= 1;
            return Some(PyLiteral::Dict(Vec::new()));
        }
        let first = self.value()?;
        if self.eat(':') {
            let mut items = vec![(first, self.value()?)];
            loop {
                if !self.eat(',') {
                    if self.eat('}') {
                        break;
                    }
                    return None;
                }
                if self.eat('}') {
                    break;
                }
                let key = self.value()?;
                if !self.eat(':') {
                    return None;
                }
                items.push((key, self.value()?));
            }
            self.depth -= 1;
            return Some(PyLiteral::Dict(items));
        }
        let mut items = vec![first];
        loop {
            if !self.eat(',') {
                if self.eat('}') {
                    break;
                }
                return None;
            }
            if self.eat('}') {
                break;
            }
            items.push(self.value()?);
        }
        self.depth -= 1;
        Some(PyLiteral::Set(items))
    }

    fn number(&mut self) -> Option<PyLiteral> {
        let start = self.pos;
        while let Some(ch) = self.peek() {
            let exponent_sign = matches!(ch, '+' | '-')
                && self.pos > start
                && matches!(self.chars[self.pos - 1], 'e' | 'E')
                && !self.chars[start..self.pos]
                    .iter()
                    .any(|ch| matches!(ch, 'x' | 'X'));
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || exponent_sign {
                self.pos += 1;
            } else {
                break;
            }
        }
        let text: String = self.chars[start..self.pos]
            .iter()
            .filter(|&&ch| ch != '_')
            .collect();
        let lower = text.to_ascii_lowercase();
        let digits =
            |radix: u32, body: &str| !body.is_empty() && body.chars().all(|ch| ch.is_digit(radix));
        let valid = if let Some(body) = lower.strip_prefix("0x") {
            digits(16, body)
        } else if let Some(body) = lower.strip_prefix("0o") {
            digits(8, body)
        } else if let Some(body) = lower.strip_prefix("0b") {
            digits(2, body)
        } else {
            let body = lower.strip_suffix('j').unwrap_or(&lower);
            body != "." && body.parse::<f64>().is_ok() && !body.starts_with(['+', '-'])
        };
        valid.then_some(PyLiteral::Number)
    }

    fn word(&mut self) -> Option<PyLiteral> {
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
        {
            self.pos += 1;
        }
        let word: String = self.chars[start..self.pos].iter().collect();
        match word.as_str() {
            "True" => Some(PyLiteral::Bool(true)),
            "False" => Some(PyLiteral::Bool(false)),
            "None" => Some(PyLiteral::None),
            _ if matches!(self.peek(), Some('"' | '\'')) => {
                let prefix = word.to_ascii_lowercase();
                if matches!(prefix.as_str(), "r" | "u" | "b" | "br" | "rb") {
                    self.pos = start;
                    self.strings(String::new())
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// One or more adjacent string literals, concatenated.
    fn strings(&mut self, mut joined: String) -> Option<PyLiteral> {
        let mut bytes: Option<bool> = None;
        loop {
            self.skip();
            let start = self.pos;
            while self.peek().is_some_and(|ch| ch.is_ascii_alphabetic()) {
                self.pos += 1;
            }
            let prefix: String = self.chars[start..self.pos]
                .iter()
                .collect::<String>()
                .to_ascii_lowercase();
            if !matches!(self.peek(), Some('"' | '\'')) {
                self.pos = start;
                break;
            }
            if !matches!(prefix.as_str(), "" | "r" | "u" | "b" | "br" | "rb") {
                return None;
            }
            let is_bytes = prefix.contains('b');
            if bytes.is_some_and(|known| known != is_bytes) {
                return None; // mixing bytes and str is a SyntaxError
            }
            bytes = Some(is_bytes);
            joined.push_str(&self.string_literal(prefix.contains('r'))?);
        }
        match bytes? {
            true => Some(PyLiteral::Bytes),
            false => Some(PyLiteral::Str(joined)),
        }
    }

    fn string_literal(&mut self, raw: bool) -> Option<String> {
        let quote = self.peek()?;
        let triple = self.chars.get(self.pos + 1) == Some(&quote)
            && self.chars.get(self.pos + 2) == Some(&quote);
        self.pos += if triple { 3 } else { 1 };
        let mut value = String::new();
        loop {
            let ch = self.peek()?;
            if ch == quote {
                if !triple {
                    self.pos += 1;
                    return Some(value);
                }
                if self.chars.get(self.pos + 1) == Some(&quote)
                    && self.chars.get(self.pos + 2) == Some(&quote)
                {
                    self.pos += 3;
                    return Some(value);
                }
            }
            if ch == '\n' && !triple {
                return None;
            }
            self.pos += 1;
            if ch != '\\' {
                value.push(ch);
                continue;
            }
            let next = self.peek()?;
            self.pos += 1;
            if raw {
                value.push('\\');
                value.push(next);
                continue;
            }
            match next {
                '\n' => {}
                '\\' | '\'' | '"' => value.push(next),
                'n' => value.push('\n'),
                't' => value.push('\t'),
                'r' => value.push('\r'),
                'a' => value.push('\u{07}'),
                'b' => value.push('\u{08}'),
                'f' => value.push('\u{0c}'),
                'v' => value.push('\u{0b}'),
                '0'..='7' => {
                    let mut code = next.to_digit(8)?;
                    for _ in 0..2 {
                        match self.peek().and_then(|ch| ch.to_digit(8)) {
                            Some(digit) => {
                                code = code * 8 + digit;
                                self.pos += 1;
                            }
                            None => break,
                        }
                    }
                    value.push(char::from_u32(code)?);
                }
                'x' | 'u' | 'U' => {
                    let width = match next {
                        'x' => 2,
                        'u' => 4,
                        _ => 8,
                    };
                    let hex: String = self.chars.get(self.pos..self.pos + width)?.iter().collect();
                    let code = u32::from_str_radix(&hex, 16).ok()?;
                    if hex.len() != width || !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
                        return None;
                    }
                    self.pos += width;
                    value.push(char::from_u32(code)?);
                }
                // `\N{NAME}` would need the Unicode name table.
                'N' => return None,
                other => {
                    value.push('\\');
                    value.push(other);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gyp_style_literals() {
        let source = "{\n  'targets': [\n    { 'target_name': 'addon', # trailing\n      \"sources\": ['a.cc', 'b' 'c.cc',], 'n': -1.5e3, 'ok': True },\n  ],\n}\n";
        let value = literal_eval(source).unwrap();
        let targets = value.get("targets").unwrap();
        let PyLiteral::List(targets) = targets else {
            panic!("targets is a list");
        };
        assert_eq!(
            targets[0].get("target_name").and_then(PyLiteral::as_str),
            Some("addon")
        );
        assert_eq!(
            targets[0].get("sources"),
            Some(&PyLiteral::List(vec![
                PyLiteral::Str("a.cc".to_string()),
                PyLiteral::Str("bc.cc".to_string()),
            ]))
        );
    }

    #[test]
    fn rejects_what_literal_eval_rejects() {
        assert_eq!(literal_eval("{'a': foo}"), None);
        assert_eq!(literal_eval("\n  {}"), None);
        assert_eq!(literal_eval("{} {}"), None);
        assert_eq!(
            literal_eval("{'a': 1}, 2"),
            Some(PyLiteral::Tuple(vec![
                PyLiteral::Dict(vec![(PyLiteral::Str("a".into()), PyLiteral::Number)]),
                PyLiteral::Number,
            ]))
        );
    }
}
