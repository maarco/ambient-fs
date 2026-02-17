// LLM end-to-end test - explicit step by step, errors visible
//
// Requires env vars:
//   AMBIENT_FS_LLM_MODEL=glm-5
//   AMBIENT_FS_LLM_BASE_URL=https://api.z.ai/api/coding/paas/v4/chat/completions
//   OPENAI_API_KEY=your-key

use std::io::Write;
use std::sync::Arc;
use tempfile::TempDir;

use ambient_fs_core::event::{EventType, FileEvent};
use ambient_fs_analyzer::{LlmFileAnalyzer, LanguageRegistry};
use ambient_fs_server::{AnalysisPipeline, LlmClient};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== LLM E2E Test ===\n");

    // --- env ---
    let model = std::env::var("AMBIENT_FS_LLM_MODEL").unwrap_or_else(|_| "(not set)".to_string());
    let base_url = std::env::var("AMBIENT_FS_LLM_BASE_URL").unwrap_or_else(|_| "(not set)".to_string());
    println!("model:    {}", model);
    println!("base_url: {}", base_url);

    let llm = match LlmClient::from_env() {
        Some(c) => {
            println!("provider: {}\n", if c.is_custom_endpoint() { "custom endpoint" } else { "genai" });
            Arc::new(c)
        }
        None => {
            eprintln!("FAIL: AMBIENT_FS_LLM_MODEL not set");
            std::process::exit(1);
        }
    };

    // --- step 1: raw LLM call ---
    println!("[1/3] raw LLM call...");
    let system = "you analyze source code files. extract imports, exports, and lint hints. respond with JSON only. be concise. only report clear issues for lint.";
    let user = "language: rust\nfile: src/lib.rs\n---\nuse std::io;\npub fn add(a: i32, b: i32) -> i32 { a + b }";

    match llm.call(system, user).await {
        Ok(response) => {
            println!("response ({} chars):\n{}\n", response.len(), response);
        }
        Err(e) => {
            eprintln!("FAIL at step 1: {}", e);
            std::process::exit(1);
        }
    }

    // --- step 2: tier 1 (local) ---
    println!("[2/3] tier 1 analysis (local)...");
    let dir = TempDir::new()?;
    let src_dir = dir.path().join("src");
    std::fs::create_dir_all(&src_dir)?;
    let file_path = src_dir.join("lib.rs");
    std::fs::File::create(&file_path)?.write_all(
        b"use std::collections::HashMap;\nuse std::io::{self, Read};\n\npub struct Cache {\n    data: HashMap<String, String>,\n}\n\npub fn load(path: &str) -> io::Result<String> {\n    // TODO: handle errors\n    let mut f = std::fs::File::open(path)?;\n    let mut s = String::new();\n    f.read_to_string(&mut s)?;\n    Ok(s)\n}\n",
    )?;

    let cache_path = dir.path().join("analysis.db");
    let pipeline = AnalysisPipeline::with_defaults(cache_path.clone())
        .with_llm(Arc::clone(&llm));

    let event = FileEvent::new(EventType::Created, "src/lib.rs", "test-project", "test-machine")
        .with_content_hash("hash-abc");

    match pipeline.analyze_file(&event, dir.path()).await {
        Some(a) => println!("lines: {}  todos: {}  imports: {}  exports: {}\n",
            a.line_count, a.todo_count, a.imports.len(), a.exports.len()),
        None => println!("(cache hit)\n"),
    }

    // --- step 3: tier 2 (LLM) directly ---
    println!("[3/3] tier 2 LLM analysis...");
    let content = std::fs::read_to_string(&file_path)?;
    let lang = LanguageRegistry::get_for_path(&file_path)
        .ok_or("unsupported file type")?;

    println!("language: {}", lang.name);
    println!("calling {}...", model);

    let llm_analyzer = LlmFileAnalyzer::new(true);
    let (system, user) = llm_analyzer.build_prompt("src/lib.rs", &content, lang.name);

    let response = match llm.call(&system, &user).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("FAIL at step 3 - LLM call: {}", e);
            std::process::exit(1);
        }
    };

    println!("raw response:\n{}\n", response);

    // strip markdown fences if model wraps in ```json
    let json_str = response
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    match llm_analyzer.parse_response(json_str) {
        Ok(parsed) => {
            println!("imports:  {:?}", parsed.imports.iter().map(|i| &i.path).collect::<Vec<_>>());
            println!("exports:  {:?}", parsed.exports);
            println!("lint:     {:?}", parsed.lint_hints.iter().map(|l| &l.message).collect::<Vec<_>>());
            println!("\nPASS");
        }
        Err(e) => {
            eprintln!("FAIL at step 3 - parse: {}", e);
            eprintln!("raw response was:\n{}", response);
            std::process::exit(1);
        }
    }

    Ok(())
}
