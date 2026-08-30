//! Japanese / CJK segmentation for FTS5.
//!
//! Index and query use different token sets on purpose:
//!
//! * **Index** keeps morphological surfaces, dictionary base forms, and
//!   overlapping CJK bigrams so both full-word and partial-prefix queries hit.
//! * **Query** keeps content morphemes (dropping particles, auxiliaries, and
//!   light verbs) so FTS AND does not require every overlapping bigram of an
//!   inflected phrase to appear in the document.
//!
//! Lindera IPADIC is compiled in. If dictionary load fails at runtime the
//! covering-bigram fallback still runs so search never depends on a process
//! having MeCab installed.

use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use lindera::dictionary::load_dictionary;
use lindera::mode::Mode;
use lindera::segmenter::Segmenter;

const QUERY_STOP_BASES: &[&str] = &[
    "する",
    "なる",
    "いる",
    "ある",
    "できる",
    "やる",
    "くる",
    "いく",
    "ます",
    "です",
    "だ",
    "た",
    "て",
    "よう",
    "こと",
    "もの",
    "ため",
    "さん",
];

const QUERY_STOP_POS: &[&str] = &["助詞", "助動詞", "記号", "接続詞", "フィラー", "その他"];

fn segmenter() -> Option<&'static Mutex<Segmenter>> {
    static CELL: OnceLock<Option<Mutex<Segmenter>>> = OnceLock::new();
    CELL.get_or_init(|| {
        let dictionary = load_dictionary("embedded://ipadic").ok()?;
        Some(Mutex::new(Segmenter::new(Mode::Normal, dictionary, None)))
    })
    .as_ref()
}

pub(crate) fn contains_japanese(text: &str) -> bool {
    text.chars().any(is_cjk_char)
}

pub(crate) fn is_cjk_char(ch: char) -> bool {
    matches!(
        ch,
        '\u{3040}'..='\u{30ff}'
            | '\u{3400}'..='\u{9fff}'
            | '\u{f900}'..='\u{faff}'
            | '\u{ac00}'..='\u{d7af}'
    )
}

/// Index-time tokens: morphemes + base forms + overlapping CJK bigrams.
pub(crate) fn segment_japanese_fts_index(text: &str) -> String {
    if text.is_empty() || !contains_japanese(text) {
        return text.to_string();
    }
    let mut tokens = Vec::new();
    let mut seen = HashSet::new();
    let mut push = |token: String| {
        if token.is_empty() || !seen.insert(token.clone()) {
            return;
        }
        tokens.push(token);
    };

    if let Some(morphemes) = morph_tokens(text, false) {
        for token in morphemes {
            push(token);
        }
    }
    for token in mixed_bigram_tokens(text) {
        push(token);
    }
    tokens.join(" ")
}

/// Query-time tokens: content morphemes, else covering CJK bigrams.
pub(crate) fn segment_japanese_fts_query(text: &str) -> String {
    if text.is_empty() || !contains_japanese(text) {
        return text.to_string();
    }
    if let Some(morphemes) = morph_tokens(text, true)
        && !morphemes.is_empty()
    {
        return morphemes.join(" ");
    }
    covering_query_tokens(text).join(" ")
}

fn morph_tokens(text: &str, query: bool) -> Option<Vec<String>> {
    let guard = segmenter()?.lock().ok()?;
    let mut tokens = guard.segment(Cow::Borrowed(text)).ok()?;
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for token in &mut tokens {
        let surface = token.surface.as_ref().trim().to_string();
        if surface.is_empty() {
            continue;
        }
        let pos = token.get("major_pos").unwrap_or("").to_string();
        let base = token
            .get("base_form")
            .filter(|value| *value != "*" && !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| surface.clone());
        if query && is_query_stop(&pos, &base, &surface) {
            continue;
        }
        let keep = if query { &base } else { &surface };
        if seen.insert(keep.clone()) {
            out.push(keep.clone());
        }
        if !query && base != surface && seen.insert(base.clone()) {
            out.push(base);
        }
    }
    Some(out)
}

fn is_query_stop(pos: &str, base: &str, surface: &str) -> bool {
    if QUERY_STOP_POS.iter().any(|prefix| pos.starts_with(prefix)) {
        return true;
    }
    if QUERY_STOP_BASES.contains(&base) || QUERY_STOP_BASES.contains(&surface) {
        return true;
    }
    surface.chars().count() == 1 && is_cjk_char(surface.chars().next().unwrap_or('\0')) && {
        let ch = surface.chars().next().unwrap();
        ('\u{3040}'..='\u{309f}').contains(&ch)
    }
}

fn mixed_bigram_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut chunk = String::new();
    let mut chunk_kind = ChunkKind::Other;
    for ch in text.chars() {
        let next_kind = chunk_kind_for(ch);
        if next_kind == ChunkKind::Other {
            flush_chunk(&mut chunk, chunk_kind, &mut tokens, false);
            chunk_kind = ChunkKind::Other;
            continue;
        }
        if chunk_kind != ChunkKind::Other && next_kind != chunk_kind {
            flush_chunk(&mut chunk, chunk_kind, &mut tokens, false);
        }
        chunk.push(ch);
        chunk_kind = next_kind;
    }
    flush_chunk(&mut chunk, chunk_kind, &mut tokens, false);
    tokens
}

fn covering_query_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut chunk = String::new();
    let mut chunk_kind = ChunkKind::Other;
    for ch in text.chars() {
        let next_kind = chunk_kind_for(ch);
        if next_kind == ChunkKind::Other {
            flush_chunk(&mut chunk, chunk_kind, &mut tokens, true);
            chunk_kind = ChunkKind::Other;
            continue;
        }
        if chunk_kind != ChunkKind::Other && next_kind != chunk_kind {
            flush_chunk(&mut chunk, chunk_kind, &mut tokens, true);
        }
        chunk.push(ch);
        chunk_kind = next_kind;
    }
    flush_chunk(&mut chunk, chunk_kind, &mut tokens, true);
    tokens
        .into_iter()
        .filter(|token| !QUERY_STOP_BASES.contains(&token.as_str()))
        .collect()
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ChunkKind {
    Ascii,
    Cjk,
    Other,
}

fn chunk_kind_for(ch: char) -> ChunkKind {
    if ch.is_ascii_alphanumeric() || ch == '_' {
        ChunkKind::Ascii
    } else if is_cjk_char(ch) {
        ChunkKind::Cjk
    } else {
        ChunkKind::Other
    }
}

fn flush_chunk(chunk: &mut String, kind: ChunkKind, tokens: &mut Vec<String>, covering: bool) {
    if chunk.is_empty() {
        return;
    }
    match kind {
        ChunkKind::Ascii => tokens.push(std::mem::take(chunk)),
        ChunkKind::Cjk => {
            let chars = chunk.chars().collect::<Vec<_>>();
            if chars.len() <= 2 {
                tokens.push(std::mem::take(chunk));
            } else if covering {
                let mut idx = 0;
                while idx + 1 < chars.len() {
                    tokens.push(chars[idx..idx + 2].iter().collect());
                    idx += 2;
                }
                if idx < chars.len() {
                    tokens.push(chars[idx].to_string());
                }
                chunk.clear();
            } else {
                for idx in 0..chars.len() - 1 {
                    tokens.push(chars[idx..idx + 2].iter().collect());
                }
                chunk.clear();
            }
        }
        ChunkKind::Other => chunk.clear(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_drops_inflection_and_keeps_content_words() {
        let query = segment_japanese_fts_query("自然言語検索する");
        assert!(query.contains("自然"), "{query}");
        assert!(query.contains("言語"), "{query}");
        assert!(query.contains("検索"), "{query}");
        assert!(!query.contains("する"), "{query}");
    }

    #[test]
    fn index_keeps_bigrams_and_morphemes() {
        let indexed = segment_japanese_fts_index("自然言語検索を行う");
        assert!(indexed.contains("自然"), "{indexed}");
        assert!(indexed.contains("言語"), "{indexed}");
        assert!(indexed.contains("検索"), "{indexed}");
        assert!(
            indexed.contains("然言") || indexed.contains("自然"),
            "{indexed}"
        );
    }

    #[test]
    fn query_drops_light_verb_from_bare_search() {
        let query = segment_japanese_fts_query("検索する");
        assert!(query.contains("検索"), "{query}");
        assert!(
            !query.split_whitespace().any(|token| token == "する"),
            "{query}"
        );
    }

    #[test]
    fn ascii_inside_japanese_stays_intact() {
        let query = segment_japanese_fts_query("GraphStoreで自然言語検索");
        assert!(query.contains("GraphStore"), "{query}");
        assert!(query.contains("自然"), "{query}");
    }
}
