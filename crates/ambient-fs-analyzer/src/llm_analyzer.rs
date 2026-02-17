// LLM-powered file analyzer for imports, exports, and lint hints.
//
// Two-tier analysis:
// - Tier 1 (local): line count, TODO count (always runs)
// - Tier 2 (LLM): imports, exports, lint hints (optional, async)

use ambient_fs_core::analysis::{FileAnalysis, ImportRef, LintHint, LintSeverity};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Error from LLM analysis.
#[derive(Debug, Error)]
pub enum LlmAnalyzerError {
    #[error("Failed to parse LLM JSON response: {0}")]
    ParseError(#[from] serde_json::Error),

    #[error("LLM response was empty")]
    EmptyResponse,
}

/// Import representation in LLM response.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LlmImport {
    pub path: String,
    pub symbols: Vec<String>,
    pub line: u32,
}

/// Lint hint representation in LLM response.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LlmLintHint {
    pub line: u32,
    pub column: u32,
    pub severity: String,
    pub message: String,
    pub rule: Option<String>,
}

/// LLM analysis response structure.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LlmAnalysisResponse {
    pub imports: Vec<LlmImport>,
    pub exports: Vec<String>,
    pub lint_hints: Vec<LlmLintHint>,
}

/// LLM file analyzer - builds prompts, parses responses, merges results.
///
/// This is a pure struct with no async calls. The actual LLM call
/// happens elsewhere (in ambient-fs-server), this just handles:
/// - Prompt construction
/// - Response parsing
/// - Merging LLM results into FileAnalysis
#[derive(Debug, Clone)]
pub struct LlmFileAnalyzer {
    pub enabled: bool,
}

impl LlmFileAnalyzer {
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    /// Build system and user prompts for file analysis.
    ///
    /// Returns (system_prompt, user_prompt).
    pub fn build_prompt(&self, file_path: &str, content: &str, language: &str) -> (String, String) {
        let system = "you analyze source code files. extract imports, exports, and lint hints. respond with JSON only. be concise. only report clear issues for lint."
            .to_string();

        let user = format!(
            "language: {}\nfile: {}\n---\n{}",
            language, file_path, content
        );

        (system, user)
    }

    /// Parse LLM response string into structured data.
    ///
    /// Handles malformed JSON gracefully with error.
    pub fn parse_response(&self, response: &str) -> Result<LlmAnalysisResponse, LlmAnalyzerError> {
        if response.trim().is_empty() {
            return Err(LlmAnalyzerError::EmptyResponse);
        }
        serde_json::from_str(response).map_err(Into::into)
    }

    /// Merge LLM response into base FileAnalysis.
    ///
    /// Base analysis (from tier 1) has line_count and todo_count.
    /// This adds imports, exports, and lint_hints from LLM.
    pub fn to_file_analysis(
        &self,
        response: LlmAnalysisResponse,
        base: FileAnalysis,
    ) -> FileAnalysis {
        let imports = response
            .imports
            .into_iter()
            .map(|llm_imp| ImportRef {
                path: llm_imp.path,
                symbols: llm_imp.symbols,
                line: llm_imp.line,
            })
            .collect();

        let lint_hints = response
            .lint_hints
            .into_iter()
            .map(|llm_hint| {
                let severity = match llm_hint.severity.as_str() {
                    "info" => LintSeverity::Info,
                    "warning" => LintSeverity::Warning,
                    "error" => LintSeverity::Error,
                    _ => LintSeverity::Warning, // default fallback
                };
                LintHint {
                    line: llm_hint.line,
                    column: llm_hint.column,
                    severity,
                    message: llm_hint.message,
                    rule: llm_hint.rule,
                }
            })
            .collect();

        FileAnalysis {
            imports,
            exports: response.exports,
            lint_hints,
            ..base
        }
    }

    /// Convenience: parse response string and merge into base.
    pub fn enhance_with_llm_response(
        &self,
        base: FileAnalysis,
        llm_response: &str,
    ) -> Result<FileAnalysis, LlmAnalyzerError> {
        let parsed = self.parse_response(llm_response)?;
        Ok(self.to_file_analysis(parsed, base))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_base_analysis() -> FileAnalysis {
        FileAnalysis {
            file_path: "src/test.rs".to_string(),
            project_id: "test-proj".to_string(),
            content_hash: "abc123".to_string(),
            exports: vec![],
            imports: vec![],
            todo_count: 2,
            lint_hints: vec![],
            line_count: 42,
        }
    }

    #[test]
    fn build_prompt_rust() {
        let analyzer = LlmFileAnalyzer::new(true);
        let content = "use std::collections::HashMap;\n\npub fn foo() {}";
        let (sys, user) = analyzer.build_prompt("src/lib.rs", content, "rust");

        assert!(sys.contains("extract imports, exports, and lint hints"));
        assert!(user.contains("language: rust"));
        assert!(user.contains("file: src/lib.rs"));
        assert!(user.contains("use std::collections"));
    }

    #[test]
    fn build_prompt_typescript() {
        let analyzer = LlmFileAnalyzer::new(true);
        let content = "import { useState } from 'react';\n\nexport const App = () => {}";
        let (_sys, user) = analyzer.build_prompt("App.tsx", content, "typescript");

        assert!(user.contains("language: typescript"));
        assert!(user.contains("file: App.tsx"));
        assert!(user.contains("import { useState }"));
    }

    #[test]
    fn build_prompt_python() {
        let analyzer = LlmFileAnalyzer::new(true);
        let content = "from typing import List\n\ndef foo() -> List[int]:\n    pass";
        let (_sys, user) = analyzer.build_prompt("main.py", content, "python");

        assert!(user.contains("language: python"));
        assert!(user.contains("file: main.py"));
        assert!(user.contains("from typing"));
    }

    #[test]
    fn parse_response_valid_json() {
        let analyzer = LlmFileAnalyzer::new(true);
        let json = r#"{
            "imports": [
                {"path": "std::collections::HashMap", "symbols": ["HashMap"], "line": 1}
            ],
            "exports": ["foo", "bar"],
            "lint_hints": [
                {"line": 5, "column": 9, "severity": "warning", "message": "unused var", "rule": "unused_variables"}
            ]
        }"#;

        let result = analyzer.parse_response(json).unwrap();
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].path, "std::collections::HashMap");
        assert_eq!(result.exports.len(), 2);
        assert_eq!(result.exports[0], "foo");
        assert_eq!(result.lint_hints.len(), 1);
        assert_eq!(result.lint_hints[0].message, "unused var");
    }

    #[test]
    fn parse_response_empty_arrays() {
        let analyzer = LlmFileAnalyzer::new(true);
        let json = r#"{"imports": [], "exports": [], "lint_hints": []}"#;

        let result = analyzer.parse_response(json).unwrap();
        assert!(result.imports.is_empty());
        assert!(result.exports.is_empty());
        assert!(result.lint_hints.is_empty());
    }

    #[test]
    fn parse_response_malformed_json_error() {
        let analyzer = LlmFileAnalyzer::new(true);
        let bad_json = r#"{"imports": [broken json}"#;

        let result = analyzer.parse_response(bad_json);
        assert!(result.is_err());
        assert!(matches!(result, Err(LlmAnalyzerError::ParseError(_))));
    }

    #[test]
    fn parse_response_empty_string_error() {
        let analyzer = LlmFileAnalyzer::new(true);
        let result = analyzer.parse_response("");
        assert!(matches!(result, Err(LlmAnalyzerError::EmptyResponse)));
    }

    #[test]
    fn parse_response_whitespace_only_error() {
        let analyzer = LlmFileAnalyzer::new(true);
        let result = analyzer.parse_response("   \n  ");
        assert!(matches!(result, Err(LlmAnalyzerError::EmptyResponse)));
    }

    #[test]
    fn to_file_analysis_merges_imports() {
        let analyzer = LlmFileAnalyzer::new(true);
        let base = make_base_analysis();

        let response = LlmAnalysisResponse {
            imports: vec![
                LlmImport {
                    path: "std::collections".to_string(),
                    symbols: vec!["HashMap".to_string()],
                    line: 1,
                },
                LlmImport {
                    path: "crate::models".to_string(),
                    symbols: vec![],
                    line: 2,
                },
            ],
            exports: vec![],
            lint_hints: vec![],
        };

        let merged = analyzer.to_file_analysis(response, base);

        assert_eq!(merged.imports.len(), 2);
        assert_eq!(merged.imports[0].path, "std::collections");
        assert_eq!(merged.imports[0].symbols, vec!["HashMap"]);
        assert_eq!(merged.imports[0].line, 1);
        assert_eq!(merged.imports[1].symbols.len(), 0); // empty symbols
    }

    #[test]
    fn to_file_analysis_merges_exports() {
        let analyzer = LlmFileAnalyzer::new(true);
        let base = make_base_analysis();

        let response = LlmAnalysisResponse {
            imports: vec![],
            exports: vec!["MyStruct".to_string(), "my_function".to_string()],
            lint_hints: vec![],
        };

        let merged = analyzer.to_file_analysis(response, base);

        assert_eq!(merged.exports.len(), 2);
        assert_eq!(merged.exports[0], "MyStruct");
        assert_eq!(merged.exports[1], "my_function");
    }

    #[test]
    fn to_file_analysis_merges_lint_hints() {
        let analyzer = LlmFileAnalyzer::new(true);
        let base = make_base_analysis();

        let response = LlmAnalysisResponse {
            imports: vec![],
            exports: vec![],
            lint_hints: vec![
                LlmLintHint {
                    line: 10,
                    column: 5,
                    severity: "warning".to_string(),
                    message: "unused variable".to_string(),
                    rule: Some("unused_variables".to_string()),
                },
                LlmLintHint {
                    line: 20,
                    column: 1,
                    severity: "error".to_string(),
                    message: "type mismatch".to_string(),
                    rule: Some("type_mismatch".to_string()),
                },
            ],
        };

        let merged = analyzer.to_file_analysis(response, base);

        assert_eq!(merged.lint_hints.len(), 2);
        assert_eq!(merged.lint_hints[0].line, 10);
        assert_eq!(merged.lint_hints[0].severity, LintSeverity::Warning);
        assert_eq!(merged.lint_hints[0].message, "unused variable");
        assert_eq!(merged.lint_hints[1].severity, LintSeverity::Error);
    }

    #[test]
    fn to_file_analysis_preserves_base_fields() {
        let analyzer = LlmFileAnalyzer::new(true);
        let base = make_base_analysis();

        let response = LlmAnalysisResponse {
            imports: vec![],
            exports: vec![],
            lint_hints: vec![],
        };

        let merged = analyzer.to_file_analysis(response, base);

        assert_eq!(merged.file_path, "src/test.rs");
        assert_eq!(merged.project_id, "test-proj");
        assert_eq!(merged.content_hash, "abc123");
        assert_eq!(merged.todo_count, 2);
        assert_eq!(merged.line_count, 42);
    }

    #[test]
    fn to_file_analysis_unknown_severity_falls_back_to_warning() {
        let analyzer = LlmFileAnalyzer::new(true);
        let base = make_base_analysis();

        let response = LlmAnalysisResponse {
            imports: vec![],
            exports: vec![],
            lint_hints: vec![LlmLintHint {
                line: 1,
                column: 1,
                severity: "unknown_severity".to_string(),
                message: "test".to_string(),
                rule: None,
            }],
        };

        let merged = analyzer.to_file_analysis(response, base);

        assert_eq!(merged.lint_hints[0].severity, LintSeverity::Warning);
    }

    #[test]
    fn enhance_with_llm_response_full_flow() {
        let analyzer = LlmFileAnalyzer::new(true);
        let base = make_base_analysis();

        let json = r#"{
            "imports": [{"path": "std", "symbols": ["Vec"], "line": 1}],
            "exports": ["pub_func"],
            "lint_hints": [{"line": 5, "column": 1, "severity": "info", "message": "note", "rule": null}]
        }"#;

        let result = analyzer.enhance_with_llm_response(base, json).unwrap();

        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].path, "std");
        assert_eq!(result.exports.len(), 1);
        assert_eq!(result.exports[0], "pub_func");
        assert_eq!(result.lint_hints.len(), 1);
        assert_eq!(result.lint_hints[0].severity, LintSeverity::Info);
        assert!(result.lint_hints[0].rule.is_none());
    }

    #[test]
    fn new_enabled_false() {
        let analyzer = LlmFileAnalyzer::new(false);
        assert!(!analyzer.enabled);
    }

    #[test]
    fn new_enabled_true() {
        let analyzer = LlmFileAnalyzer::new(true);
        assert!(analyzer.enabled);
    }
}
