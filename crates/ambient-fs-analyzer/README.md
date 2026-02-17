# ambient-fs-analyzer

Content analysis engine for source files. Extracts imports, exports, TODOs, and metrics.

## Features

- **FileAnalyzer** - Analyze file contents
  - Line counting
  - TODO/FIXME/HACK comment detection
  - Import extraction (per-language patterns)
  - Export extraction (stub for ast-grep integration)

- **LanguageRegistry** - Per-language configs
  - TypeScript, JavaScript, Rust, Python, Vue, Markdown
  - Configurable features per language
  - `get_for_path(path)` - Auto-detect from extension

## Usage

```rust
use ambient_fs_analyzer::FileAnalyzer;

let analyzer = FileAnalyzer::new(1_000_000); // 1MB max

let analysis = analyzer.analyze(
    PathBuf::from("src/main.rs"),
    "my-project",
    "abc123" // content_hash
)?;
```

## Supported Languages

| Language  | Extensions          | Features                     |
|-----------|---------------------|------------------------------|
| TypeScript| .ts, .tsx            | imports, exports, TODOs      |
| JavaScript| .js, .jsx            | imports, exports, TODOs      |
| Rust      | .rs                 | imports, TODOs               |
| Python    | .py                 | imports, TODOs               |
| Vue       | .vue                | TODOs                        |
| Markdown  | .md                 | TODOs                        |

## License

MIT
