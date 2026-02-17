use serde::{Deserialize, Serialize};

/// Analysis results for a single file
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileAnalysis {
    pub file_path: String,
    pub project_id: String,
    /// blake3 hash of content that was analyzed
    pub content_hash: String,
    /// Exported symbols
    pub exports: Vec<String>,
    /// Import references
    pub imports: Vec<ImportRef>,
    /// TODO/FIXME/HACK comment count
    pub todo_count: u32,
    /// Lint hints from static analysis
    pub lint_hints: Vec<LintHint>,
    /// Total line count
    pub line_count: u32,
}

impl FileAnalysis {
    pub fn empty(
        file_path: impl Into<String>,
        project_id: impl Into<String>,
        content_hash: impl Into<String>,
    ) -> Self {
        Self {
            file_path: file_path.into(),
            project_id: project_id.into(),
            content_hash: content_hash.into(),
            exports: Vec::new(),
            imports: Vec::new(),
            todo_count: 0,
            lint_hints: Vec::new(),
            line_count: 0,
        }
    }

    /// True if this analysis is still valid for the given content hash
    pub fn is_valid_for(&self, hash: &str) -> bool {
        self.content_hash == hash
    }
}

/// A reference to an imported module/symbol
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportRef {
    /// The import path/module (e.g. "@/composables/useChat")
    pub path: String,
    /// Specific symbols imported (empty = entire module)
    pub symbols: Vec<String>,
    /// Line number of the import statement
    pub line: u32,
}

/// A lint-level hint from static analysis
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LintHint {
    pub line: u32,
    pub column: u32,
    pub severity: LintSeverity,
    pub message: String,
    /// Rule identifier (e.g. "no-unused-vars", "dead_code")
    pub rule: Option<String>,
}

/// Severity level for lint hints
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LintSeverity {
    Info,
    Warning,
    Error,
}

impl LintSeverity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

impl std::fmt::Display for LintSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn file_analysis_empty() {
        let analysis = FileAnalysis::empty("src/lib.rs", "proj-1", "hash123");
        assert_eq!(analysis.file_path, "src/lib.rs");
        assert_eq!(analysis.content_hash, "hash123");
        assert!(analysis.exports.is_empty());
        assert!(analysis.imports.is_empty());
        assert_eq!(analysis.todo_count, 0);
        assert_eq!(analysis.line_count, 0);
    }

    #[test]
    fn file_analysis_valid_for_hash() {
        let analysis = FileAnalysis::empty("f.rs", "p", "abc");
        assert!(analysis.is_valid_for("abc"));
        assert!(!analysis.is_valid_for("xyz"));
    }

    #[test]
    fn file_analysis_with_data() {
        let analysis = FileAnalysis {
            file_path: "src/routes.ts".to_string(),
            project_id: "proj-1".to_string(),
            content_hash: "hash456".to_string(),
            exports: vec!["Router".to_string(), "createRoute".to_string()],
            imports: vec![
                ImportRef {
                    path: "express".to_string(),
                    symbols: vec!["Router".to_string()],
                    line: 1,
                },
                ImportRef {
                    path: "./middleware".to_string(),
                    symbols: vec![],
                    line: 2,
                },
            ],
            todo_count: 2,
            lint_hints: vec![LintHint {
                line: 42,
                column: 5,
                severity: LintSeverity::Warning,
                message: "unused variable".to_string(),
                rule: Some("no-unused-vars".to_string()),
            }],
            line_count: 87,
        };

        assert_eq!(analysis.exports.len(), 2);
        assert_eq!(analysis.imports.len(), 2);
        assert_eq!(analysis.imports[0].symbols, vec!["Router"]);
        assert_eq!(analysis.lint_hints[0].severity, LintSeverity::Warning);
    }

    #[test]
    fn lint_severity_display() {
        assert_eq!(LintSeverity::Info.to_string(), "info");
        assert_eq!(LintSeverity::Warning.to_string(), "warning");
        assert_eq!(LintSeverity::Error.to_string(), "error");
    }

    #[test]
    fn file_analysis_serde_roundtrip() {
        let analysis = FileAnalysis {
            file_path: "src/lib.rs".to_string(),
            project_id: "proj-1".to_string(),
            content_hash: "abc".to_string(),
            exports: vec!["Foo".to_string()],
            imports: vec![ImportRef {
                path: "bar".to_string(),
                symbols: vec!["Baz".to_string()],
                line: 1,
            }],
            todo_count: 1,
            lint_hints: vec![],
            line_count: 50,
        };

        let json = serde_json::to_string(&analysis).unwrap();
        let deserialized: FileAnalysis = serde_json::from_str(&json).unwrap();
        assert_eq!(analysis, deserialized);
    }

    #[test]
    fn import_ref_empty_symbols_means_whole_module() {
        let imp = ImportRef {
            path: "./utils".to_string(),
            symbols: vec![],
            line: 5,
        };
        assert!(imp.symbols.is_empty());
    }
}
