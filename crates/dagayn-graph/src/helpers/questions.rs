//! Inputs to suggested-question generation.

use super::*;

pub(crate) struct SurprisingQuestionInput {
    pub(crate) source_name: String,
    pub(crate) source_qualified: String,
    pub(crate) target_name: String,
    pub(crate) source_community: i64,
    pub(crate) target_community: i64,
    pub(crate) score: i64,
}

pub(crate) struct QuestionCommunity {
    pub(crate) id: i64,
    pub(crate) name: String,
    pub(crate) size: i64,
}

pub(crate) struct QuestionHotspot {
    pub(crate) name: String,
    pub(crate) qualified_name: String,
    pub(crate) degree: i64,
}

pub(crate) struct QuestionGaps {
    pub(crate) thin_communities: Vec<QuestionCommunity>,
    pub(crate) untested_hotspots: Vec<QuestionHotspot>,
}

pub(crate) struct QuestionNode {
    pub(crate) kind: String,
    pub(crate) name: String,
    pub(crate) qualified_name: String,
    pub(crate) file_path: String,
    pub(crate) language: String,
    pub(crate) is_test: bool,
}

pub(crate) struct QuestionEdge {
    pub(crate) kind: String,
    pub(crate) source_qualified: String,
    pub(crate) target_qualified: String,
}

pub(crate) fn nearest_rank_percentile(values: &[i64], percentile: f64) -> i64 {
    if values.is_empty() {
        return 0;
    }
    let rank = (percentile * values.len() as f64).ceil() as usize;
    let index = rank.saturating_sub(1).min(values.len() - 1);
    values[index]
}

pub(crate) fn is_analysis_excluded_from_test_gap(node: &QuestionNode) -> bool {
    if node.is_test || node.kind == "Test" || node.language == "markdown" {
        return true;
    }
    let normalized = node.file_path.replace('\\', "/");
    let name = normalized
        .rsplit('/')
        .next()
        .unwrap_or(normalized.as_str())
        .to_lowercase();
    let parts = normalized
        .split('/')
        .map(|part| part.to_lowercase())
        .collect::<HashSet<_>>();
    parts.contains("tests")
        || parts.contains("test")
        || parts.contains("__tests__")
        || name.starts_with("test_")
        || matches!(name.as_str(), "test.rs" | "tests.rs")
        || name.ends_with("_test.py")
        || name.ends_with("_tests.py")
        || name.ends_with("_test.rs")
        || name.ends_with("_tests.rs")
        || name.contains(".test.")
        || name.contains(".spec.")
}
