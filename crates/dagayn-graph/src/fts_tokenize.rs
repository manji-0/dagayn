//! FTS text normalization helpers (parity with ``dagayn.graph._fts_tokenize``).

pub(crate) const FTS_SEGMENTER_METADATA_KEY: &str = "fts_segmenter";

#[derive(Clone, Copy, Eq, PartialEq)]
enum BigramChunkKind {
    Ascii,
    Hiragana,
    Katakana,
    Cjk,
    Hangul,
    Other,
}

pub(crate) fn detect_fts_segmenter() -> &'static str {
    // Rust wheels do not bundle fugashi/mecab/janome; index/query with bigram.
    "bigram"
}

pub(crate) fn contains_japanese(text: &str) -> bool {
    text.chars().any(|ch| matches!(
        bigram_chunk_kind(ch),
        BigramChunkKind::Hiragana
            | BigramChunkKind::Katakana
            | BigramChunkKind::Cjk
            | BigramChunkKind::Hangul
    ))
}

pub(crate) fn segment_cjk_identifier_tokens(text: &str, segmenter: Option<&str>) -> String {
    if text.is_empty() || !contains_japanese(text) {
        return String::new();
    }
    let resolved = match segmenter {
        Some(value) => value,
        None => detect_fts_segmenter(),
    };
    let bigram = segment_bigram(text, true);
    if resolved == "bigram" {
        return bigram;
    }
    let wakati = segment_japanese_fts_text(text, Some(resolved));
    let mut tokens = Vec::new();
    for part in wakati.split_whitespace().chain(bigram.split_whitespace()) {
        if !part.is_empty() && !tokens.iter().any(|existing| existing == part) {
            tokens.push(part.to_string());
        }
    }
    tokens.join(" ")
}

pub(crate) fn segment_japanese_fts_text(text: &str, segmenter: Option<&str>) -> String {
    if text.is_empty() || !contains_japanese(text) {
        return text.to_string();
    }
    let resolved = match segmenter {
        Some(value) => value,
        None => detect_fts_segmenter(),
    };
    if resolved == "bigram" {
        return segment_bigram(text, false);
    }
    // No wakati runtime in Rust; mirror Python fallback when tokenizer is unavailable.
    segment_bigram(text, false)
}

fn segment_bigram(text: &str, cjk_only: bool) -> String {
    let mut tokens = Vec::new();
    let mut chunk = String::new();
    let mut chunk_kind = BigramChunkKind::Other;
    for ch in text.chars() {
        let next_kind = bigram_chunk_kind(ch);
        if next_kind == BigramChunkKind::Other {
            flush_bigram_chunk(&mut chunk, chunk_kind, cjk_only, &mut tokens);
            chunk_kind = BigramChunkKind::Other;
            continue;
        }
        if chunk_kind != BigramChunkKind::Other && next_kind != chunk_kind {
            flush_bigram_chunk(&mut chunk, chunk_kind, cjk_only, &mut tokens);
        }
        chunk.push(ch);
        chunk_kind = next_kind;
    }
    flush_bigram_chunk(&mut chunk, chunk_kind, cjk_only, &mut tokens);
    tokens.join(" ")
}

fn bigram_chunk_kind(ch: char) -> BigramChunkKind {
    if ch.is_ascii_alphanumeric() || ch == '_' {
        BigramChunkKind::Ascii
    } else if ('\u{3040}'..='\u{309f}').contains(&ch) {
        BigramChunkKind::Hiragana
    } else if ('\u{30a0}'..='\u{30ff}').contains(&ch) {
        BigramChunkKind::Katakana
    } else if ('\u{3400}'..='\u{9fff}').contains(&ch) || ('\u{f900}'..='\u{faff}').contains(&ch) {
        BigramChunkKind::Cjk
    } else if ('\u{ac00}'..='\u{d7af}').contains(&ch) {
        BigramChunkKind::Hangul
    } else {
        BigramChunkKind::Other
    }
}

fn flush_bigram_chunk(
    chunk: &mut String,
    kind: BigramChunkKind,
    cjk_only: bool,
    tokens: &mut Vec<String>,
) {
    if chunk.is_empty() {
        return;
    }
    match kind {
        BigramChunkKind::Ascii => {
            if !cjk_only {
                tokens.push(std::mem::take(chunk));
            } else {
                chunk.clear();
            }
        }
        BigramChunkKind::Hiragana
        | BigramChunkKind::Katakana
        | BigramChunkKind::Cjk
        | BigramChunkKind::Hangul => {
            let chars: Vec<char> = chunk.chars().filter(|ch| !ch.is_whitespace()).collect();
            chunk.clear();
            if chars.is_empty() {
                return;
            }
            if chars.len() <= 2 {
                tokens.push(chars.into_iter().collect());
            } else {
                for idx in 0..chars.len() - 1 {
                    tokens.push(chars[idx..idx + 2].iter().collect());
                }
            }
        }
        BigramChunkKind::Other => chunk.clear(),
    }
}

pub(crate) fn build_identifier_tokens(
    name: &str,
    qualified_name: &str,
    file_path: &str,
    display_name: &str,
    segmenter: &str,
) -> String {
    let base = super::helpers::identifier_search_text([name, qualified_name, file_path, display_name]);
    let cjk_tokens = [
        segment_cjk_identifier_tokens(name, Some(segmenter)),
        segment_cjk_identifier_tokens(qualified_name, Some(segmenter)),
        segment_cjk_identifier_tokens(file_path, Some(segmenter)),
    ];
    let mut parts = Vec::new();
    if !base.is_empty() {
        parts.push(base);
    }
    for tokens in cjk_tokens {
        if !tokens.is_empty() {
            parts.push(tokens);
        }
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contains_japanese_includes_hangul() {
        assert!(contains_japanese("안녕하세요"));
        assert!(!contains_japanese("Hello"));
    }

    #[test]
    fn segment_cjk_identifier_tokens_bigrams_hangul() {
        assert_eq!(
            segment_cjk_identifier_tokens("안녕하세요", Some("bigram")),
            "안녕 녕하 하세 세요"
        );
    }

    #[test]
    fn segment_japanese_splits_mixed_ascii_and_cjk_runs() {
        let segmented = segment_japanese_fts_text("GraphStoreで自然言語検索", Some("bigram"));
        assert!(segmented.contains("GraphStore"));
        assert!(segmented.contains("自然"));
        assert!(!segmented.contains("で自"));
    }

    #[test]
    fn segment_bigram_matches_python_reference_vectors() {
        assert_eq!(
            segment_japanese_fts_text("GraphStoreで自然言語検索", Some("bigram")),
            "GraphStore で 自然 然言 言語 語検 検索"
        );
        assert_eq!(
            segment_japanese_fts_text("안녕하세요", Some("bigram")),
            "안녕 녕하 하세 세요"
        );
        assert_eq!(
            segment_cjk_identifier_tokens("ユーザー取得", Some("bigram")),
            "ユー ーザ ザー 取得"
        );
    }

    #[test]
    fn detect_fts_segmenter_is_bigram() {
        assert_eq!(detect_fts_segmenter(), "bigram");
    }
}
