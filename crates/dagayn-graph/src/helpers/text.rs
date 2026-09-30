//! Identifier search text, source excerpts, and naming helpers.

use super::*;

pub(crate) fn identifier_search_text<'a>(values: impl IntoIterator<Item = &'a str>) -> String {
    let mut tokens = Vec::new();
    for value in values {
        let mut chunk = String::new();
        for ch in value.chars() {
            if ch.is_ascii_alphanumeric() {
                chunk.push(ch);
            } else if !chunk.is_empty() {
                push_identifier_parts(&chunk, &mut tokens);
                chunk.clear();
            }
        }
        if !chunk.is_empty() {
            push_identifier_parts(&chunk, &mut tokens);
        }
        if crate::japanese_fts::contains_japanese(value) {
            for token in crate::japanese_fts::segment_japanese_fts_index(value).split_whitespace() {
                if !tokens.iter().any(|existing| existing == token) {
                    tokens.push(token.to_string());
                }
            }
        }
    }
    tokens.join(" ")
}

pub(crate) fn push_identifier_parts(chunk: &str, tokens: &mut Vec<String>) {
    let chars = chunk.chars().collect::<Vec<_>>();
    let mut start = 0;
    for idx in 1..chars.len() {
        let prev = chars[idx - 1];
        let current = chars[idx];
        let next = chars.get(idx + 1).copied();
        let lower_to_upper =
            (prev.is_ascii_lowercase() || prev.is_ascii_digit()) && current.is_ascii_uppercase();
        let acronym_boundary = prev.is_ascii_uppercase()
            && current.is_ascii_uppercase()
            && next.is_some_and(|ch| ch.is_ascii_lowercase());
        if lower_to_upper || acronym_boundary {
            tokens.push(
                chars[start..idx]
                    .iter()
                    .collect::<String>()
                    .to_ascii_lowercase(),
            );
            start = idx;
        }
    }
    if start < chars.len() {
        tokens.push(
            chars[start..]
                .iter()
                .collect::<String>()
                .to_ascii_lowercase(),
        );
    }
}

const SOURCE_EXCERPT_MAX_CHARS: usize = 4096;

/// The most recently read source file, split into lines.
///
/// FTS rows are built node by node, and a file with K nodes used to be read
/// and split K times. Callers that visit nodes grouped by file keep one cache
/// across the loop so each file is read once.
#[derive(Default)]
pub(crate) struct SourceCache {
    path: Option<PathBuf>,
    text: String,
    lines: Vec<std::ops::Range<usize>>,
}

impl SourceCache {
    fn lines_of(&mut self, path: PathBuf) -> Vec<&str> {
        if self.path.as_ref() != Some(&path) {
            self.text = std::fs::read_to_string(&path).unwrap_or_default();
            let base = self.text.as_ptr() as usize;
            self.lines = self
                .text
                .lines()
                .map(|line| {
                    let start = line.as_ptr() as usize - base;
                    start..start + line.len()
                })
                .collect();
            self.path = Some(path);
        }
        self.lines
            .iter()
            .map(|range| &self.text[range.clone()])
            .collect()
    }
}

pub(crate) fn read_node_source_excerpt(
    cache: &mut SourceCache,
    repo_root: Option<&Path>,
    kind: &str,
    file_path: &str,
    line_start: Option<i64>,
    line_end: Option<i64>,
) -> String {
    let mut path = PathBuf::from(file_path);
    if !path.is_absolute() {
        let Some(root) = repo_root else {
            return String::new();
        };
        path = root.join(path);
    }
    let lines = cache.lines_of(path);
    if lines.is_empty() {
        return String::new();
    }
    let start = line_start.unwrap_or(1).saturating_sub(1).max(0) as usize;
    let mut end = line_end
        .unwrap_or(line_start.unwrap_or(1))
        .max(line_start.unwrap_or(1)) as usize;
    let start = start.min(lines.len().saturating_sub(1));
    end = end.min(lines.len());
    if kind == "DocSection" {
        let level = markdown_heading_level(lines[start]);
        end = lines.len();
        for (idx, line) in lines.iter().enumerate().skip(start + 1) {
            if let Some(candidate_level) = markdown_heading_level(line)
                && level.is_none_or(|current_level| candidate_level <= current_level)
            {
                end = idx;
                break;
            }
        }
    }
    let mut excerpt = String::new();
    let mut chars = 0_usize;
    for (idx, line) in lines[start..end].iter().enumerate() {
        if idx > 0 {
            excerpt.push('\n');
            chars += 1;
        }
        excerpt.push_str(line);
        chars += line.chars().count();
        if chars >= SOURCE_EXCERPT_MAX_CHARS {
            break;
        }
    }
    if chars > SOURCE_EXCERPT_MAX_CHARS {
        excerpt = excerpt.chars().take(SOURCE_EXCERPT_MAX_CHARS).collect();
    }
    excerpt
}

pub(crate) fn markdown_heading_level(line: &str) -> Option<usize> {
    let trimmed = line.trim_start();
    let level = trimmed.chars().take_while(|ch| *ch == '#').count();
    if (1..=6).contains(&level) && trimmed.chars().nth(level).is_some_and(|ch| ch == ' ') {
        Some(level)
    } else {
        None
    }
}

pub(crate) fn common_prefix(values: &[String]) -> String {
    let Some((first, rest)) = values.split_first() else {
        return String::new();
    };
    let mut prefix = first.clone();
    for value in rest {
        while !value.starts_with(&prefix) {
            if prefix.pop().is_none() {
                return String::new();
            }
        }
    }
    prefix
}

pub(crate) fn community_purpose(paths: &[String]) -> String {
    let prefix = common_prefix(paths);
    if !prefix.contains('/') {
        return String::new();
    }
    prefix
        .rsplit_once('/')
        .map(|(before_last, _)| before_last.rsplit('/').next().unwrap_or(""))
        .unwrap_or("")
        .to_string()
}

pub(crate) fn sanitize_name(value: &str) -> String {
    value
        .chars()
        .filter(|ch| *ch == '\t' || *ch == '\n' || (*ch as u32) >= 0x20)
        .take(256)
        .collect()
}
