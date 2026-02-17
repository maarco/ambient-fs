analyzer stubs (ambient-fs-analyzer)
=====================================

status: redesigned -- LLM-first approach
updated: 2026-02-16
affects: crates/ambient-fs-analyzer/src/analyzer.rs,
         crates/ambient-fs-analyzer/src/languages.rs


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

crates/ambient-fs-analyzer/src/llm_analyzer.rs (new):

  LlmAnalyzer struct:
    - config: LlmConfig (api_key, model, enabled, max_file_size)
    - analyze_file(path, content, language) -> FileAnalysis
    - analyze_batch(files) -> Vec<FileAnalysis>
    - builds prompt from file content + language
    - parses JSON response into ImportRef/LintHint structs
    - falls back to empty vecs on parse failure (graceful)

  LlmConfig:
    - api_key: Option<String>  (None = disabled)
    - model: String  (default: "claude-haiku-4-5-20251001")
    - max_tokens: usize (default: 512)
    - max_file_size: u64 (skip files over this, default 50KB)
    - batch_delay_ms: u64 (default: 2000, collect files then send)

crates/ambient-fs-analyzer/src/analyzer.rs (modify):

  FileAnalyzer changes:
    - add optional llm_analyzer: Option<LlmAnalyzer>
    - analyze() still does tier 1 locally (lines, todos)
    - if llm_analyzer is Some, schedule tier 2 async
    - return FileAnalysis immediately with tier 1 data
    - tier 2 results merged in when LLM responds

  two-phase return:
    1. immediate: FileAnalysis with line_count, todo_count
       (imports=[], exports=[], lint_hints=[])
    2. async update: FileAnalysis with all fields populated
       (broadcast via SubscriptionManager or callback)


beads mapping
-------------

t3z (import extraction):
  → handled by LlmAnalyzer, "imports" field in response

1nd (export extraction):
  → handled by LlmAnalyzer, "exports" field in response

ar2 (lint hints):
  → handled by LlmAnalyzer, "lint_hints" field in response

up0 (dynamic language registration):
  → still needed independently. the LLM doesn't need
    language configs to analyze code (it detects language
    from content), but the LanguageRegistry is used by
    other parts of the system (feature flags, extension
    mapping). implement as originally spec'd.


shared LLM infrastructure
--------------------------

the LLM client should be shared between:
  1. analyzer (file analysis)
  2. agent tracker (activity parsing)

create a shared module:

  crates/ambient-fs-server/src/llm.rs:
    LlmClient struct:
      - config: LlmConfig
      - client: reqwest::Client
      - call(system_prompt, user_prompt) -> String
      - call_json<T>(system, user) -> T  (parse response)

    used by both LlmAnalyzer and AgentTracker.
    single API key config, single HTTP client.


test strategy
-------------

unit tests (no LLM, mock responses):
  - test prompt construction per language
  - test JSON response parsing into ImportRef/LintHint
  - test graceful fallback on malformed response
  - test batch collection and delay logic
  - test max_file_size skip

integration tests (optional, needs API key):
  - real haiku call with a small rust file
  - verify imports/exports/lint hints populated
  - gated behind #[cfg(feature = "llm-integration-tests")]
