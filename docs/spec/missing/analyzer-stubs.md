analyzer stubs (ambient-fs-analyzer)
=====================================

status: implemented -- LLM-first approach complete
updated: 2026-02-16
affects: crates/ambient-fs-analyzer/src/analyzer.rs,
         crates/ambient-fs-analyzer/src/languages.rs,
         crates/ambient-fs-analyzer/src/llm_analyzer.rs


design change
-------------

original plan: tree-sitter per language for imports/exports,
regex per language for lint hints. separate implementations
per stub (t3z, 1nd, ar2).

new plan: one haiku-class LLM call per file that returns
imports, exports, AND lint hints in a single response.
same pattern as the agent-activity-protocol: fast model,
structured JSON output, optional enhancement layer.

this collapses t3z + 1nd + ar2 into one implementation.


architecture: two-tier analysis
-------------------------------

tier 1: local (fast, free, always runs)
  - line count (existing, works)
  - TODO/FIXME/HACK count (existing, works)
  - language detection via extension (existing, works)
  these are simple counts. no LLM needed.

tier 2: LLM-enhanced (optional, async, batched)
  - imports extraction
  - exports extraction
  - lint hints
  - one API call per file, returns all three

when LLM is disabled (default), tier 2 fields return
empty. when enabled, they populate on file change with
a short delay (batching).


LLM analysis call
-----------------

input:

  system: you analyze source code files. extract imports,
          exports, and lint hints. respond with JSON only.
          be concise. only report clear issues for lint.

  user: language: rust
        file: src/auth.rs
        ---
        use std::collections::HashMap;
        use crate::models::User;

        pub fn validate_token(token: &str) -> bool {
            let x = 5;
            token.len() > 0
        }

        pub struct AuthService {
            users: HashMap<String, User>,
        }

output:

  {
    "imports": [
      {"path": "std::collections::HashMap", "symbols": ["HashMap"], "line": 1},
      {"path": "crate::models::User", "symbols": ["User"], "line": 2}
    ],
    "exports": ["validate_token", "AuthService"],
    "lint_hints": [
      {"line": 5, "column": 9, "severity": "warning",
       "message": "unused variable x", "rule": "unused_variable"},
      {"line": 6, "column": 5, "severity": "warning",
       "message": "use !token.is_empty() instead of token.len() > 0",
       "rule": "len_zero"}
    ]
  }

maps directly to FileAnalysis fields:
  imports  -> Vec<ImportRef>
  exports  -> Vec<String>
  lint_hints -> Vec<LintHint>


cost estimate
-------------

haiku input: ~300-500 tokens per file (code + prompt)
haiku output: ~100-200 tokens per file
at $0.25/MTok input, $1.25/MTok output:
  ~$0.0001 per file analysis
  100 file changes/hour = $0.01/hour

same order of magnitude as agent activity enhancement.
share the same LLM client config.


implementation
--------------

status: done

crates/ambient-fs-analyzer/src/llm_analyzer.rs (implemented):

  LlmFileAnalyzer struct:
    - enabled: bool
    - build_prompt(file_path, content, language) -> (String, String)
      constructs system and user prompts for LLM analysis
    - parse_response(response: &str) -> Result<LlmAnalysisResponse>
      parses JSON response, handles empty/malformed input
    - to_file_analysis(response, base: FileAnalysis) -> FileAnalysis
      merges LLM results into base FileAnalysis (tier 1 + tier 2)
    - enhance_with_llm_response(base, llm_response) -> Result<FileAnalysis>
      convenience method combining parse + merge

  LlmAnalysisResponse:
    - imports: Vec<LlmImport>  (path, symbols, line)
    - exports: Vec<String>
    - lint_hints: Vec<LlmLintHint> (line, column, severity, message, rule)

  LlmAnalyzerError:
    - ParseError(serde_json::Error)
    - EmptyResponse

crates/ambient-fs-analyzer/src/analyzer.rs (modified):

  FileAnalyzer changes:
    - added llm_enabled: bool field
    - with_llm(enabled: bool) -> Self (builder method)
    - is_llm_enabled() -> bool (getter)
    - enhance_with_llm_response(base, llm_response) -> Result<FileAnalysis>
      delegates to LlmFileAnalyzer for parsing and merging
    - analyze() returns tier 1 data immediately (line_count, todo_count)
      imports/exports/lint_hints are empty until enhanced via LLM response

  exports from lib.rs:
    - LlmFileAnalyzer
    - LlmAnalysisResponse
    - LlmImport
    - LlmLintHint
    - LlmAnalyzerError


beads mapping
-------------

t3z (import extraction):
  → handled by LlmAnalyzer, "imports" field in response

1nd (export extraction):
  → handled by LlmAnalyzer, "exports" field in response

ar2 (lint hints):
  → handled by LlmAnalyzer, "lint_hints" field in response

up0 (dynamic language registration):
  → COMPLETE (2026-02-16)
  - implemented in crates/ambient-fs-analyzer/src/languages.rs
  - OwnedLanguageConfig struct for heap-allocated configs
  - LanguageRegistry.register() writes to RwLock<HashMap<String, OwnedLanguageConfig>>
  - LanguageRegistry.get_for_path() checks custom registry first, then builtins
  - Thread-safe via RwLock, allows custom languages to override builtins
  - reset_custom() for test isolation
  - 8 new tests: register_custom_language, custom_language_multiple_extensions,
    custom_overrides_builtin, builtin_still_works_after_custom_registration,
    register_is_thread_safe, owned_language_config_clone,
    owned_to_language_config_conversion, reset_custom calls in existing tests
  - OwnedLanguageConfig exported from lib.rs as public API


shared LLM infrastructure
--------------------------

the LLM client should be shared between:
  1. analyzer (file analysis)
  2. agent tracker (activity parsing)

status: DONE (2026-02-16)

  crates/ambient-fs-server/src/llm.rs:

  LlmConfig struct:
    - api_key: Option<String>  (None = disabled)
    - model: String  (default: "claude-haiku-4-5-20251001")
    - base_url: String (default: "https://api.anthropic.com/v1")
    - max_tokens: usize (default: 512)
    - implements Default

  LlmClient struct:
    - config: LlmConfig
    - client: reqwest::Client
    - new(config: LlmConfig) -> Self
    - is_enabled(&self) -> bool (checks api_key is Some)
    - async call(system: &str, user: &str) -> Result<String, LlmError>
      POST to {base_url}/messages with Anthropic headers
      (x-api-key, anthropic-version: 2023-06-01)
      Returns the text content from response[0].text
    - async call_json<T: DeserializeOwned>(system: &str, user: &str) -> Result<T, LlmError>
      Calls call(), then serde_json::from_str on the result

  LlmError enum:
    - Disabled (no API key)
    - Http(reqwest::Error)
    - Api { status: u16, message: String }
    - Parse(serde_json::Error)

  tests (5 passing):
    - test_llm_config_defaults
    - test_is_enabled_with_api_key
    - test_is_enabled_without_api_key
    - test_call_json_parsing_valid_json
    - test_call_json_error_on_invalid_json

  exported from lib.rs: LlmClient, LlmConfig, LlmError

  used by both LlmAnalyzer (in ambient-fs-analyzer) and
  AgentTracker (in ambient-fs-server). single API key config,
  single HTTP client.


test strategy
-------------

status: done (60 tests passing)

unit tests (no LLM, mock responses):
  - test prompt construction per language (rust, typescript, python)
  - test JSON response parsing into ImportRef/LintHint
  - test graceful fallback on malformed response
  - test empty/whitespace response handling
  - test merge behavior for imports, exports, lint_hints
  - test tier 1 analysis still works without LLM
  - test severity fallback for unknown values
  - test full flow with enhance_with_llm_response

integration tests:
  - not implemented (requires API key, would be in ambient-fs-server)
