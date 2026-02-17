use crate::{AnalysisError, AnalyzerConfig, LlmFileAnalyzer};
use ambient_fs_core::analysis::FileAnalysis;
use std::path::Path;

/// File content analyzer
///
/// Extracts metadata from source files:
/// - Line count
/// - TODO/FIXME/HACK comments
/// - Imports (LLM-enhanced)
/// - Exports (LLM-enhanced)
/// - Lint hints (LLM-enhanced)
pub struct FileAnalyzer {
    config: AnalyzerConfig,
    llm_enabled: bool,
}

impl FileAnalyzer {
    pub fn new(config: AnalyzerConfig) -> Self {
        Self {
            config,
            llm_enabled: false,
        }
    }

    pub fn with_llm(mut self, enabled: bool) -> Self {
        self.llm_enabled = enabled;
        self
    }

    pub fn is_llm_enabled(&self) -> bool {
        self.llm_enabled
    }

    /// Analyze a file and produce structured metadata
    ///
    /// # Arguments
    /// * `path` - Path to the file to analyze
    /// * `project_id` - Project identifier for this file
    /// * `content_hash` - blake3 hash of the file content (computed by caller)
    ///
    /// # Returns
    /// `FileAnalysis` with extracted metadata
    pub fn analyze(
        &self,
        path: &Path,
        project_id: &str,
        content_hash: &str,
    ) -> Result<FileAnalysis, AnalysisError> {
        // Check file size before reading
        let metadata = std::fs::metadata(path)?;
        let file_size = metadata.len();

        if file_size > self.config.max_file_size {
            return Err(AnalysisError::FileTooLarge {
                size: file_size,
                max: self.config.max_file_size,
            });
        }

        // Read file content
        let content = std::fs::read_to_string(path)?;

        // Extract tier 1 metrics (local, fast, always runs)
        let line_count = self.count_lines(&content);
        let todo_count = self.count_todos(&content);

        // Tier 2 (LLM): imports, exports, lint_hints
        // These are populated via enhance_with_llm_response() after the LLM call
        let imports = Vec::new();
        let exports = Vec::new();
        let lint_hints = Vec::new();

        Ok(FileAnalysis {
            file_path: path.to_string_lossy().to_string(),
            project_id: project_id.to_string(),
            content_hash: content_hash.to_string(),
            exports,
            imports,
            todo_count,
            lint_hints,
            line_count,
        })
    }

    /// Enhance a base FileAnalysis with LLM response.
    ///
    /// This parses the LLM JSON response and merges it into the
    /// tier 1 analysis results.
    pub fn enhance_with_llm_response(
        &self,
        base: FileAnalysis,
        llm_response: &str,
    ) -> Result<FileAnalysis, crate::LlmAnalyzerError> {
        let llm = LlmFileAnalyzer::new(self.llm_enabled);
        llm.enhance_with_llm_response(base, llm_response)
    }

    fn count_lines(&self, content: &str) -> u32 {
        if content.is_empty() {
            return 0;
        }
        // Count newlines, handle both \n and \r\n
        content.lines().count() as u32
    }

    fn count_todos(&self, content: &str) -> u32 {
        let mut count = 0;
        let mut chars = content.chars().peekable();

        while let Some(c) = chars.next() {
            // Check for comment markers
            if c == '/' {
                if let Some(&'/') = chars.peek() {
                    chars.next(); // consume second slash
                    self.scan_line_comment_for_keywords(&mut chars, &mut count);
                }
            } else if c == '-' {
                if let Some(&'-') = chars.peek() {
                    chars.next(); // consume second dash
                    self.scan_line_comment_for_keywords(&mut chars, &mut count);
                }
            } else if c == '#' {
                self.scan_line_comment_for_keywords(&mut chars, &mut count);
            }
        }

        count
    }

    fn scan_line_comment_for_keywords<I>(&self, chars: &mut std::iter::Peekable<I>, count: &mut u32)
    where
        I: Iterator<Item = char>,
    {
        let mut keyword_buf = String::new();

        // Skip whitespace after comment start
        while let Some(&c) = chars.peek() {
            if c == ' ' || c == '\t' {
                chars.next();
                continue;
            }
            break;
        }

        // Collect potential keyword
        while let Some(&c) = chars.peek() {
            if c == '\n' {
                break;
            }
            if c.is_alphabetic() || c == ':' {
                keyword_buf.push(c);
                chars.next();
                if keyword_buf.len() > 10 {
                    // Too long to be a keyword we care about
                    break;
                }
            } else if !keyword_buf.is_empty() {
                // End of potential keyword
                break;
            } else {
                chars.next();
            }
        }

        let keyword = keyword_buf.to_uppercase();
        if keyword.contains("TODO") || keyword.contains("FIXME") || keyword.contains("HACK") {
            *count += 1;
        }

        // Skip rest of line
        while let Some(c) = chars.next() {
            if c == '\n' {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;
    use std::io::Write;

    fn create_test_file(content: &str) -> NamedTempFile {
        let file = NamedTempFile::new().unwrap();
        file.as_file().write_all(content.as_bytes()).unwrap();
        file
    }

    #[test]
    fn count_lines_basic() {
        let analyzer = FileAnalyzer::new(AnalyzerConfig::default());
        assert_eq!(analyzer.count_lines("line1\nline2\nline3\n"), 3);
    }

    #[test]
    fn count_lines_empty() {
        let analyzer = FileAnalyzer::new(AnalyzerConfig::default());
        assert_eq!(analyzer.count_lines(""), 0);
    }

    #[test]
    fn count_lines_no_trailing_newline() {
        let analyzer = FileAnalyzer::new(AnalyzerConfig::default());
        assert_eq!(analyzer.count_lines("line1\nline2"), 2);
    }

    #[test]
    fn count_todos_single_slash() {
        let analyzer = FileAnalyzer::new(AnalyzerConfig::default());
        assert_eq!(analyzer.count_todos("// TODO: fix this\nlet x = 1;"), 1);
    }

    #[test]
    fn count_todos_fixme() {
        let analyzer = FileAnalyzer::new(AnalyzerConfig::default());
        assert_eq!(analyzer.count_todos("// FIXME: broken\n"), 1);
    }

    #[test]
    fn count_todos_hack() {
        let analyzer = FileAnalyzer::new(AnalyzerConfig::default());
        assert_eq!(analyzer.count_todos("# HACK: ugly workaround\n"), 1);
    }

    #[test]
    fn count_todos_multiple() {
        let analyzer = FileAnalyzer::new(AnalyzerConfig::default());
        let content = r#"
// TODO: one
fn main() {
    // FIXME: two
    let x = 1;
    // HACK: three
}
"#;
        assert_eq!(analyzer.count_todos(content), 3);
    }

    #[test]
    fn count_todos_case_insensitive() {
        let analyzer = FileAnalyzer::new(AnalyzerConfig::default());
        assert_eq!(analyzer.count_todos("// todo: lowercase\n"), 1);
        assert_eq!(analyzer.count_todos("// ToDo: mixed\n"), 1);
        assert_eq!(analyzer.count_todos("// FIXME: upper\n"), 1);
    }

    #[test]
    fn count_todos_dash_comment() {
        let analyzer = FileAnalyzer::new(AnalyzerConfig::default());
        assert_eq!(analyzer.count_todos("-- TODO: sql style\n"), 1);
    }

    #[test]
    fn count_todos_hash_comment() {
        let analyzer = FileAnalyzer::new(AnalyzerConfig::default());
        assert_eq!(analyzer.count_todos("# TODO: python style\n"), 1);
    }

    #[test]
    fn count_todos_no_false_positives() {
        let analyzer = FileAnalyzer::new(AnalyzerConfig::default());
        assert_eq!(analyzer.count_todos("let todo = 5;\n"), 0);
        assert_eq!(analyzer.count_todos("// not a todo comment\n"), 0);
    }

    // LLM-related tests

    #[test]
    fn file_analyzer_with_llm_enabled() {
        let analyzer = FileAnalyzer::new(AnalyzerConfig::default()).with_llm(true);
        assert!(analyzer.is_llm_enabled());
    }

    #[test]
    fn file_analyzer_with_llm_disabled_default() {
        let analyzer = FileAnalyzer::new(AnalyzerConfig::default());
        assert!(!analyzer.is_llm_enabled());
    }

    #[test]
    fn file_analyzer_tier_1_works_without_llm() {
        let file = create_test_file("fn main() {\n    // TODO: fix this\n    let x = 1;\n}");
        let config = AnalyzerConfig::default();
        let analyzer = FileAnalyzer::new(config);

        let result = analyzer.analyze(file.path(), "test-project", "hash123");

        assert!(result.is_ok());
        let analysis = result.unwrap();
        // Tier 1 data should be present
        assert_eq!(analysis.line_count, 4);
        assert_eq!(analysis.todo_count, 1);
        // Tier 2 data (LLM) should be empty
        assert!(analysis.imports.is_empty());
        assert!(analysis.exports.is_empty());
        assert!(analysis.lint_hints.is_empty());
    }

    #[test]
    fn file_analyzer_enhance_with_llm_response() {
        let analyzer = FileAnalyzer::new(AnalyzerConfig::default()).with_llm(true);

        // Create base analysis (tier 1 only)
        let base = FileAnalysis {
            file_path: "src/test.rs".to_string(),
            project_id: "test-proj".to_string(),
            content_hash: "abc123".to_string(),
            exports: vec![],
            imports: vec![],
            todo_count: 1,
            lint_hints: vec![],
            line_count: 5,
        };

        // LLM response (tier 2 data)
        let llm_response = r#"{
            "imports": [{"path": "std::collections", "symbols": ["HashMap"], "line": 1}],
            "exports": ["pub_fn"],
            "lint_hints": [{"line": 3, "column": 5, "severity": "warning", "message": "unused", "rule": "unused_vars"}]
        }"#;

        let enhanced = analyzer.enhance_with_llm_response(base, llm_response).unwrap();

        // Tier 1 data preserved
        assert_eq!(enhanced.line_count, 5);
        assert_eq!(enhanced.todo_count, 1);
        assert_eq!(enhanced.file_path, "src/test.rs");

        // Tier 2 data merged in
        assert_eq!(enhanced.imports.len(), 1);
        assert_eq!(enhanced.imports[0].path, "std::collections");
        assert_eq!(enhanced.exports.len(), 1);
        assert_eq!(enhanced.exports[0], "pub_fn");
        assert_eq!(enhanced.lint_hints.len(), 1);
        assert_eq!(enhanced.lint_hints[0].message, "unused");
    }

    #[test]
    fn file_analyzer_enhance_with_invalid_llm_response() {
        let analyzer = FileAnalyzer::new(AnalyzerConfig::default()).with_llm(true);

        let base = FileAnalysis::empty("test.rs", "p", "h");
        let bad_response = r#"this is not json"#;

        let result = analyzer.enhance_with_llm_response(base, bad_response);
        assert!(result.is_err());
    }
}
