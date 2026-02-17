# ambient-fs-analyzer

Content analysis engine for source files. Two-tier architecture: local metrics (fast) + LLM-enhanced analysis (optional).

## Architecture

Two-tier analysis:

| Tier | What                     | When          | Latency |
|------|--------------------------|---------------|---------|
| 1    | Line count, TODO count   | Always        | <1ms    |
| 2    | Imports, exports, lint   | LLM enabled   | 100-500ms|

Tier 1 runs synchronously. Tier 2 is optional and merges results later.

## Features

### FileAnalyzer
- Line counting (handles \n, \r\n)
- TODO/FIXME/HACK comment detection (//, --, # styles)
- File size limits (rejects huge files)
- LLM response integration

### LanguageRegistry
- Built-in languages: TypeScript, JavaScript, Rust, Python, Vue, Markdown
- Dynamic runtime registration (register custom languages)
- Per-language feature flags
- Extension-based auto-detection

### LlmFileAnalyzer
- Prompt construction for file analysis
- JSON response parsing
- Merges LLM results into base FileAnalysis
- Graceful error handling

## Usage

### Basic File Analysis (Tier 1 only)

```rust
use ambient_fs_analyzer::FileAnalyzer;

let analyzer = FileAnalyzer::new(Default::default());

let analysis = analyzer.analyze(
    PathBuf::from("src/main.rs"),
    "my-project",
    "abc123" // content_hash
)?;

println!("{} lines, {} TODOs", analysis.line_count, analysis.todo_count);
```

### Dynamic Language Registration

```rust
use ambient_fs_analyzer::{LanguageRegistry, OwnedLanguageConfig, LanguageFeatures};

// Register Svelte with custom features
let svelte = OwnedLanguageConfig {
    name: "Svelte".to_string(),
    extensions: vec!["svelte".to_string()],
    features: LanguageFeatures {
        extract_exports: true,
        extract_imports: true,
        count_todos: false,  // don't count TODOs in Svelte
        count_lines: true,
    },
};

LanguageRegistry::register(svelte);

// Now .svelte files are recognized
let config = LanguageRegistry::get_for_path("App.svelte").unwrap();
```

### LLM-Enhanced Analysis (Tier 2)

```rust
use ambient_fs_analyzer::FileAnalyzer;

let analyzer = FileAnalyzer::new(Default::default()).with_llm(true);

// Get tier 1 results immediately
let base = analyzer.analyze(path, project_id, hash)?;

// Later, enhance with LLM response
let llm_json = r#"{
    "imports": [{"path": "std::collections", "symbols": ["HashMap"], "line": 1}],
    "exports": ["pub_func"],
    "lint_hints": [{"line": 5, "column": 9, "severity": "warning", "message": "unused"}]
}"#;

let enhanced = analyzer.enhance_with_llm_response(base, llm_json)?;
```

### Direct LLM Prompt Construction

```rust
use ambient_fs_analyzer::LlmFileAnalyzer;

let llm = LlmFileAnalyzer::new(true);
let (system_prompt, user_prompt) = llm.build_prompt(
    "src/lib.rs",
    "use std::collections::HashMap;\n\npub fn foo() {}",
    "rust"
);

// Send prompts to LLM API, get JSON back
let response = call_llm_api(&system_prompt, &user_prompt).await?;

let parsed = llm.parse_response(&response)?;
```

## Built-in Languages

| Language    | Extensions         | Extract Exports | Extract Imports | Count TODOs | Count Lines |
|-------------|--------------------|-----------------|-----------------|-------------|-------------|
| TypeScript  | .ts, .tsx          | yes             | yes             | yes         | yes         |
| JavaScript  | .js, .jsx, .mjs    | yes             | yes             | yes         | yes         |
| Rust        | .rs                | yes             | yes             | yes         | yes         |
| Python      | .py, .pyi          | yes             | yes             | yes         | yes         |
| Vue         | .vue               | yes             | yes             | yes         | yes         |
| Markdown    | .md, .markdown     | no              | no              | yes         | yes         |

## Public API

### Types
- `FileAnalyzer` - Main analyzer struct
- `LanguageRegistry` - Global language registry
- `LanguageConfig` - Static language config (built-ins)
- `OwnedLanguageConfig` - Heap-allocated config (dynamic registration)
- `LanguageFeatures` - Feature flags for a language
- `LlmFileAnalyzer` - LLM prompt/response handling
- `FileAnalysis` - Analysis result (from ambient-fs-core)

### Functions
- `FileAnalyzer::new(config)` - Create analyzer
- `FileAnalyzer::with_llm(enabled)` - Enable/disable LLM mode
- `FileAnalyzer::analyze(path, project_id, hash)` - Run tier 1 analysis
- `FileAnalyzer::enhance_with_llm_response(base, json)` - Merge tier 2 results
- `LanguageRegistry::get_for_path(path)` - Look up config by file extension
- `LanguageRegistry::register(config)` - Register custom language

## Testing

60 tests covering:
- Line counting (empty files, various line endings)
- TODO detection (multiple comment styles, case-insensitive)
- Language detection (extensions, overrides)
- Dynamic registration (thread safety, isolation)
- LLM response parsing (valid JSON, malformed, empty)
- File size limits

Run tests:
```bash
cargo test -p ambient-fs-analyzer
```

## Design Notes

- **No IO in core**: LanguageRegistry is pure types + lookup
- **Zero-cost builtins**: OnceLock for static languages, no lock overhead for reads
- **Thread-safe**: RwLock for custom registration, usable from multiple threads
- **Graceful degradation**: LLM errors return Err, don't crash analysis

## License

MIT
