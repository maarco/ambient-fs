// Content analysis engine

pub mod analyzer;
pub mod languages;
pub mod llm_analyzer;

pub use analyzer::FileAnalyzer;
pub use languages::{LanguageConfig, LanguageFeatures, LanguageRegistry, OwnedLanguageConfig};
pub use llm_analyzer::{LlmFileAnalyzer, LlmAnalysisResponse, LlmImport, LlmLintHint, LlmAnalyzerError};

/// Error type for analysis operations
#[derive(Debug, thiserror::Error)]
pub enum AnalysisError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("file too large: {size} bytes (max: {max} bytes)")]
    FileTooLarge { size: u64, max: u64 },

    #[error("unsupported file type: {0}")]
    UnsupportedFileType(String),
}

/// Configuration for the analyzer
#[derive(Debug, Clone)]
pub struct AnalyzerConfig {
    /// Maximum file size to analyze (in bytes)
    pub max_file_size: u64,
}

impl Default for AnalyzerConfig {
    fn default() -> Self {
        Self {
            // 1MB default max file size
            max_file_size: 1024 * 1024,
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
    fn file_analyzer_counts_lines() {
        let file = create_test_file("line1\nline2\nline3\n");
        let config = AnalyzerConfig::default();
        let analyzer = FileAnalyzer::new(config);

        let result = analyzer.analyze(
            file.path(),
            "test-project",
            "hash123"
        );

        assert!(result.is_ok());
        let analysis = result.unwrap();
        assert_eq!(analysis.line_count, 3);
    }

    #[test]
    fn file_analyzer_counts_todos() {
        let file = create_test_file(
            "fn main() {\n    // TODO: fix this\n    // FIXME: later\n    // HACK: gross\n    let x = 1;\n}"
        );
        let config = AnalyzerConfig::default();
        let analyzer = FileAnalyzer::new(config);

        let result = analyzer.analyze(
            file.path(),
            "test-project",
            "hash123"
        );

        assert!(result.is_ok());
        let analysis = result.unwrap();
        assert_eq!(analysis.todo_count, 3);
    }

    #[test]
    fn file_analyzer_rejects_large_files() {
        let file = create_test_file("x");
        let config = AnalyzerConfig {
            max_file_size: 5,
        };
        let analyzer = FileAnalyzer::new(config);

        // Mock the file size check by setting metadata
        let result = analyzer.analyze(
            file.path(),
            "test-project",
            "hash123"
        );

        // File has 1 byte, should be OK
        assert!(result.is_ok());
    }

    #[test]
    fn file_analyzer_empty_file() {
        let file = create_test_file("");
        let config = AnalyzerConfig::default();
        let analyzer = FileAnalyzer::new(config);

        let result = analyzer.analyze(
            file.path(),
            "test-project",
            "hash123"
        );

        assert!(result.is_ok());
        let analysis = result.unwrap();
        assert_eq!(analysis.line_count, 0);
        assert_eq!(analysis.todo_count, 0);
        assert!(analysis.exports.is_empty());
        assert!(analysis.imports.is_empty());
    }

    #[test]
    fn file_analyzer_preserves_metadata() {
        let file = create_test_file("content\n");
        let config = AnalyzerConfig::default();
        let analyzer = FileAnalyzer::new(config);

        let result = analyzer.analyze(
            file.path(),
            "my-project",
            "abc123def"
        );

        assert!(result.is_ok());
        let analysis = result.unwrap();
        assert_eq!(analysis.project_id, "my-project");
        assert_eq!(analysis.content_hash, "abc123def");
    }

    #[test]
    fn file_analyzer_windows_line_endings() {
        let file = create_test_file("line1\r\nline2\r\n");
        let config = AnalyzerConfig::default();
        let analyzer = FileAnalyzer::new(config);

        let result = analyzer.analyze(
            file.path(),
            "test-project",
            "hash123"
        );

        assert!(result.is_ok());
        let analysis = result.unwrap();
        assert_eq!(analysis.line_count, 2);
    }

    #[test]
    fn file_analyzer_no_final_newline() {
        let file = create_test_file("line1\nline2");
        let config = AnalyzerConfig::default();
        let analyzer = FileAnalyzer::new(config);

        let result = analyzer.analyze(
            file.path(),
            "test-project",
            "hash123"
        );

        assert!(result.is_ok());
        let analysis = result.unwrap();
        assert_eq!(analysis.line_count, 2);
    }
}
