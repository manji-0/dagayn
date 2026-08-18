//! Optional Lindera-backed wakati tokenization for FTS (parity with fugashi/mecab).

use std::sync::OnceLock;

use lindera::dictionary::load_dictionary;
use lindera::mode::Mode;
use lindera::segmenter::Segmenter;
use lindera_analysis::tokenizer::Tokenizer;

static WAKATI_TOKENIZER: OnceLock<Option<Tokenizer>> = OnceLock::new();

pub(crate) fn wakati_available() -> bool {
    wakati_tokenizer().is_some()
}

pub(crate) fn detect_wakati_segmenter() -> &'static str {
    if wakati_available() {
        "lindera"
    } else {
        "bigram"
    }
}

pub(crate) fn is_wakati_segmenter(segmenter: &str) -> bool {
    matches!(
        segmenter,
        "lindera" | "fugashi" | "mecab" | "janome"
    )
}

pub(crate) fn segment_wakati_text(text: &str) -> Option<String> {
    let tokenizer = wakati_tokenizer()?;
    let tokens = tokenizer.tokenize(text).ok()?;
    let surfaces: Vec<&str> = tokens
        .iter()
        .map(|token| token.surface.as_ref())
        .filter(|surface| !surface.is_empty())
        .collect();
    if surfaces.is_empty() {
        return None;
    }
    Some(surfaces.join(" "))
}

fn wakati_tokenizer() -> Option<&'static Tokenizer> {
    WAKATI_TOKENIZER
        .get_or_init(|| {
            let dictionary = load_dictionary("embedded://ipadic").ok()?;
            let segmenter = Segmenter::new(Mode::Normal, dictionary, None);
            Some(Tokenizer::new(segmenter))
        })
        .as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lindera_wakati_splits_japanese_identifier() {
        let segmented = segment_wakati_text("ユーザー取得").expect("wakati output");
        assert!(segmented.contains(' '));
        assert!(segmented.contains("ユーザー"));
        assert!(segmented.contains("取得"));
    }
}
