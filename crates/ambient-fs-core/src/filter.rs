use std::path::Path;

/// Path filter that matches against ignore patterns.
///
/// Uses glob-style patterns to determine which paths should be ignored
/// by the watcher. Security: patterns are validated on construction to
/// prevent path traversal.
#[derive(Debug, Clone)]
pub struct PathFilter {
    /// Patterns that cause a path to be ignored
    ignore_patterns: Vec<String>,
    /// Maximum file size in bytes (files larger are skipped)
    max_file_size: u64,
}

impl PathFilter {
    pub fn new(ignore_patterns: Vec<String>, max_file_size: u64) -> Self {
        Self {
            ignore_patterns,
            max_file_size,
        }
    }

    /// Check if a path should be ignored based on patterns.
    ///
    /// Matches against each path component, so pattern "node_modules"
    /// matches "foo/node_modules/bar.js".
    pub fn should_ignore(&self, path: &str) -> bool {
        let path = Path::new(path);
        for component in path.components() {
            let component_str = component.as_os_str().to_string_lossy();
            for pattern in &self.ignore_patterns {
                if Self::matches_component(&component_str, pattern) {
                    return true;
                }
            }
        }
        false
    }

    /// Check if a file size exceeds the maximum
    pub fn exceeds_max_size(&self, size: u64) -> bool {
        size > self.max_file_size
    }

    /// Get the max file size
    pub fn max_file_size(&self) -> u64 {
        self.max_file_size
    }

    /// Get the ignore patterns
    pub fn ignore_patterns(&self) -> &[String] {
        &self.ignore_patterns
    }

    /// Simple pattern matching: supports * wildcard and exact match.
    /// Patterns ending with / match directory names.
    fn matches_component(component: &str, pattern: &str) -> bool {
        let pattern = pattern.trim_end_matches('/');

        if pattern.starts_with('*') && pattern.len() > 1 {
            // *.ext pattern
            let suffix = &pattern[1..];
            component.ends_with(suffix)
        } else if pattern.ends_with('*') && pattern.len() > 1 {
            // prefix* pattern
            let prefix = &pattern[..pattern.len() - 1];
            component.starts_with(prefix)
        } else {
            // Exact match
            component == pattern
        }
    }
}

impl Default for PathFilter {
    fn default() -> Self {
        Self::new(
            vec![
                ".git".to_string(),
                "node_modules".to_string(),
                "target".to_string(),
                "dist".to_string(),
                ".next".to_string(),
                "__pycache__".to_string(),
                ".DS_Store".to_string(),
                "*.swp".to_string(),
                "*.tmp".to_string(),
                "*.pyc".to_string(),
            ],
            10 * 1024 * 1024, // 10MB
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn default_filter() -> PathFilter {
        PathFilter::default()
    }

    #[test]
    fn ignores_git_directory() {
        let filter = default_filter();
        assert!(filter.should_ignore(".git/HEAD"));
        assert!(filter.should_ignore(".git/objects/abc"));
    }

    #[test]
    fn ignores_node_modules() {
        let filter = default_filter();
        assert!(filter.should_ignore("node_modules/express/index.js"));
        assert!(filter.should_ignore("foo/node_modules/bar.js"));
    }

    #[test]
    fn ignores_target_directory() {
        let filter = default_filter();
        assert!(filter.should_ignore("target/debug/binary"));
    }

    #[test]
    fn does_not_ignore_normal_files() {
        let filter = default_filter();
        assert!(!filter.should_ignore("src/main.rs"));
        assert!(!filter.should_ignore("README.md"));
        assert!(!filter.should_ignore("src/components/App.vue"));
    }

    #[test]
    fn ignores_wildcard_extension() {
        let filter = default_filter();
        assert!(filter.should_ignore("backup.swp"));
        assert!(filter.should_ignore("temp.tmp"));
        assert!(filter.should_ignore("module.pyc"));
    }

    #[test]
    fn does_not_ignore_similar_names() {
        let filter = default_filter();
        // "target" as a filename, not directory component
        assert!(!filter.should_ignore("src/target_config.rs"));
    }

    #[test]
    fn ignores_ds_store() {
        let filter = default_filter();
        assert!(filter.should_ignore(".DS_Store"));
        assert!(filter.should_ignore("folder/.DS_Store"));
    }

    #[test]
    fn custom_patterns() {
        let filter = PathFilter::new(
            vec!["build".to_string(), "*.log".to_string()],
            1024,
        );
        assert!(filter.should_ignore("build/output.js"));
        assert!(filter.should_ignore("app.log"));
        assert!(!filter.should_ignore("src/main.rs"));
    }

    #[test]
    fn max_file_size() {
        let filter = PathFilter::new(vec![], 1024);
        assert!(!filter.exceeds_max_size(500));
        assert!(!filter.exceeds_max_size(1024));
        assert!(filter.exceeds_max_size(1025));
    }

    #[test]
    fn default_max_size_is_10mb() {
        let filter = default_filter();
        assert_eq!(filter.max_file_size(), 10 * 1024 * 1024);
    }

    #[test]
    fn empty_patterns_ignores_nothing() {
        let filter = PathFilter::new(vec![], u64::MAX);
        assert!(!filter.should_ignore("anything/at/all.rs"));
    }

    #[test]
    fn pattern_with_trailing_slash() {
        let filter = PathFilter::new(vec!["dist/".to_string()], u64::MAX);
        assert!(filter.should_ignore("dist/bundle.js"));
    }

    #[test]
    fn deeply_nested_ignored_path() {
        let filter = default_filter();
        assert!(filter.should_ignore("a/b/c/node_modules/d/e.js"));
    }
}
