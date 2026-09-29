//! Output versions of the extractors.
//!
//! A graph records the versions it was parsed with (graph metadata key
//! `extractor_versions`). When an extractor's version moves past the stored
//! one, the next incremental update re-parses every file that extractor
//! owns, even though the files themselves did not change: without that,
//! unchanged files would keep nodes and edges under qualified names the new
//! extractor no longer produces.

/// One extractor's output version and the file languages it parses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtractorVersion {
    /// Stable extractor name used in graph metadata (`javascript=1`).
    pub extractor: &'static str,
    /// Bump when a change renames qualified names or otherwise changes the
    /// nodes / edges produced for unchanged source.
    pub version: u32,
    /// `detect_language` values of the files this extractor parses.
    pub languages: &'static [&'static str],
}

/// Extractors with a tracked output version. Extractors not listed here are
/// treated as never changing their output.
pub const EXTRACTOR_VERSIONS: &[ExtractorVersion] = &[
    ExtractorVersion {
        // TypeScript / JavaScript rework (docs/TYPESCRIPT-EXTRACTION.md). Vue,
        // Svelte, and Astro script blocks run through the same extractor.
        extractor: "javascript",
        version: 1,
        languages: &["javascript", "typescript", "tsx", "vue", "svelte"],
    },
    ExtractorVersion {
        // 1: IMPORTS_FROM records the module as written and the names it
        // binds; CALLS on an import alias record the receiver.
        extractor: "python",
        version: 1,
        languages: &["python", "notebook"],
    },
    ExtractorVersion {
        // 1: functions and types record `ffi_export` (`#[pyfunction]`,
        // `#[pyclass]`, `#[pymethods]`, `#[no_mangle]`, `#[export_name]`).
        extractor: "rust",
        version: 1,
        languages: &["rust"],
    },
];

/// The tracked extractor versions.
pub fn extractor_versions() -> &'static [ExtractorVersion] {
    EXTRACTOR_VERSIONS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extractor_versions_are_unique_and_positive() {
        let mut names = std::collections::HashSet::new();
        for entry in extractor_versions() {
            assert!(names.insert(entry.extractor), "{}", entry.extractor);
            assert!(entry.version > 0, "{}", entry.extractor);
            assert!(!entry.languages.is_empty(), "{}", entry.extractor);
        }
    }
}
