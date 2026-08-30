use std::collections::HashMap;

use dagayn_graph::GraphNode;
use regex::Regex;

const COMMON_WORDS: &[&str] = &[
    "get", "set", "self", "init", "new", "create", "update", "delete", "add", "remove", "make",
    "build", "from", "to", "for", "with", "the", "and", "test", "main", "run", "do", "is", "has",
    "on", "of", "in", "at", "by", "my", "this", "that", "all", "none",
];

pub(crate) fn generate_community_name(members: &[GraphNode]) -> String {
    if members.is_empty() {
        return "empty".to_string();
    }

    let file_paths: Vec<String> = members.iter().map(|node| node.file_path.clone()).collect();
    let prefix = extract_file_prefix(&file_paths);

    let class_names: Vec<&str> = members
        .iter()
        .filter(|node| node.kind == "Class")
        .map(|node| node.name.as_str())
        .collect();
    if !class_names.is_empty() {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for name in class_names {
            *counts.entry(name).or_default() += 1;
        }
        if let Some((top_class, top_count)) = counts.into_iter().max_by_key(|(_, count)| *count)
            && top_count > members.len() * 40 / 100
        {
            if prefix.is_empty() {
                return to_slug(top_class);
            }
            return format!("{}-{}", prefix, to_slug(top_class));
        }
    }

    let keywords = extract_keywords(members);
    let keyword = keywords.first().map(String::as_str).unwrap_or("");

    if !prefix.is_empty() && !keyword.is_empty() {
        format!("{prefix}-{keyword}")
    } else if !prefix.is_empty() {
        prefix
    } else if !keyword.is_empty() {
        keyword.to_string()
    } else {
        "cluster".to_string()
    }
}

fn extract_file_prefix(file_paths: &[String]) -> String {
    if file_paths.is_empty() {
        return String::new();
    }

    let mut parts = Vec::new();
    for fp in file_paths {
        let normalized = fp.replace('\\', "/");
        let segments: Vec<&str> = normalized.split('/').collect();
        if segments.len() >= 2 {
            parts.push(segments[segments.len() - 2].to_string());
        } else {
            let stem = segments
                .last()
                .and_then(|name| name.rsplit_once('.').map(|(stem, _)| stem))
                .unwrap_or("");
            parts.push(stem.to_string());
        }
    }

    let mut counts: HashMap<String, usize> = HashMap::new();
    for part in parts {
        *counts.entry(part).or_default() += 1;
    }
    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(part, _)| to_slug(&part))
        .unwrap_or_default()
}

fn extract_keywords(members: &[GraphNode]) -> Vec<String> {
    let mut word_counts: HashMap<String, usize> = HashMap::new();
    for node in members {
        if matches!(node.kind.as_str(), "Function" | "Class" | "Test" | "Type") {
            for word in split_name(&node.name) {
                let lower = word.to_lowercase();
                if !COMMON_WORDS.contains(&lower.as_str()) && lower.chars().count() > 1 {
                    *word_counts.entry(lower).or_default() += 1;
                }
            }
        }
    }

    let mut ranked: Vec<(String, usize)> = word_counts.into_iter().collect();
    ranked.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    ranked.into_iter().take(5).map(|(word, _)| word).collect()
}

fn split_name(name: &str) -> Vec<String> {
    let camel = Regex::new(r"([a-z])([A-Z])")
        .expect("valid camelCase regex")
        .replace_all(name, "$1_$2");
    Regex::new(r"[_\-.\s]+")
        .expect("valid split regex")
        .split(&camel)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

fn to_slug(value: &str) -> String {
    Regex::new(r"[^a-z0-9]+")
        .expect("valid slug regex")
        .replace_all(&value.to_lowercase(), "-")
        .trim_matches('-')
        .chars()
        .take(30)
        .collect()
}
