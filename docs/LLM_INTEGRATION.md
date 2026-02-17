# LLM Integration

## Overview

ambient-fs includes an optional LLM client for haiku-class API calls. This is used for:

1. **File Analysis** (`ambient-fs-analyzer`): Extract imports, exports, and lint hints
2. **Agent Activity Parsing** (`ambient-fs-server`): Enhance non-conforming agent JSONL logs

The LLM is **disabled by default**. When disabled, tier-1 analysis (line counts, TODO counting) still works locally.

## Architecture

```
ambient-fs-server/src/llm.rs (shared LLM client)
           |
           +---> ambient-fs-analyzer (file analysis)
           |      LlmFileAnalyzer uses LlmClient.call_json()
           |
           +---> ambient-fs-server (agent tracker)
                  AgentTracker uses LlmClient.call_json()
```

## API Reference

### LlmConfig

```rust
use ambient_fs_server::LlmConfig;

let config = LlmConfig {
    api_key: std::env::var("ANTHROPIC_API_KEY").ok(),
    model: "claude-haiku-4-5-20251001".to_string(),
    base_url: "https://api.anthropic.com/v1".to_string(),
    max_tokens: 512,
};

// Or use defaults (no API key = disabled)
let config = LlmConfig::default();
```

Fields:
- `api_key: Option<String>` - Anthropic API key (None = disabled)
- `model: String` - Model identifier (default: claude-haiku-4-5-20251001)
- `base_url: String` - API base URL (default: https://api.anthropic.com/v1)
- `max_tokens: usize` - Max tokens in response (default: 512)

### LlmClient

```rust
use ambient_fs_server::{LlmClient, LlmConfig, LlmError};

let client = LlmClient::new(config);

// Check if enabled
if !client.is_enabled() {
    return;
}

// Raw text response
let response: String = client.call(
    "you are a code analyzer",
    "analyze this file"
).await?;

// Structured JSON response
#[derive(Deserialize)]
struct Analysis {
    imports: Vec<String>,
    exports: Vec<String>,
}

let analysis: Analysis = client.call_json(
    "extract imports and exports as JSON",
    code_content
).await?;
```

Methods:
- `new(config: LlmConfig) -> Self`
- `is_enabled(&self) -> bool`
- `call(&self, system: &str, user: &str) -> Result<String, LlmError>`
- `call_json<T>(&self, system: &str, user: &str) -> Result<T, LlmError>`

### LlmError

```rust
match client.call(system, user).await {
    Ok(text) => println!("Response: {}", text),
    Err(LlmError::Disabled) => {
        // No API key - expected in default setup
    }
    Err(LlmError::Api { status, message }) => {
        eprintln!("API error {}: {}", status, message);
    }
    Err(LlmError::Http(e)) => {
        eprintln!("Network error: {}", e);
    }
    Err(LlmError::Parse(e)) => {
        eprintln!("JSON parse error: {}", e);
    }
}
```

Variants:
- `Disabled` - No API key configured
- `Http(reqwest::Error)` - Network error
- `Api { status: u16, message: String }` - API returned error status
- `Parse(serde_json::Error)` - Failed to parse response as JSON

## File Analysis Usage

The analyzer uses the LLM client to extract code structure:

```rust
use ambient_fs_analyzer::{FileAnalyzer, LlmAnalysisResponse};
use ambient_fs_server::LlmClient;

let llm_client = LlmClient::new(llm_config);
let analyzer = FileAnalyzer::new().with_llm(llm_client.is_enabled());

// Tier 1: immediate local analysis
let base_analysis = analyzer.analyze("src/main.rs", code_content)?;

// Tier 2: LLM-enhanced analysis (async, optional)
if llm_client.is_enabled() {
    let llm_response: LlmAnalysisResponse = llm_client.call_json(
        &analyzer.build_system_prompt(),
        &analyzer.build_user_prompt("src/main.rs", code_content)
    ).await?;

    let full_analysis = analyzer.enhance_with_llm_response(base_analysis, &llm_response)?;
    // Now has imports, exports, lint_hints populated
}
```

## Agent Activity Parsing

The agent tracker uses the LLM to parse non-conforming logs:

```rust
use ambient_fs_server::LlmClient;

let llm_client = LlmClient::new(llm_config);

if llm_client.is_enabled() {
    let raw_log_lines = "..."; // non-conforming agent output

    #[derive(Deserialize)]
    struct AgentActivities {
        activities: Vec<AgentActivity>,
    }

    let parsed: AgentActivities = llm_client.call_json(
        "you parse AI agent activity logs. extract: agent, file, action, intent. respond with JSON only.",
        raw_log_lines
    ).await?;
}
```

## Cost Estimates

Haiku pricing (as of Feb 2026):
- Input: $0.25/M tokens
- Output: $1.25/M tokens

### File Analysis
- Input: ~300-500 tokens (code + prompt)
- Output: ~100-200 tokens (structured JSON)
- Cost: ~$0.0001 per file
- At 100 file changes/hour: ~$0.01/hour

### Agent Activity Parsing
- Input: ~200 tokens per batch
- Output: ~50 tokens per batch
- Cost: ~$0.0001 per batch
- At 100 batches/hour: ~$0.01/hour

## Configuration

Add to `~/.config/ambient-fs/config.toml`:

```toml
[llm]
enabled = false                        # Set to true to enable
api_key = ""                           # Or use ANTHROPIC_API_KEY env var
model = "claude-haiku-4-5-20251001"
base_url = "https://api.anthropic.com/v1"
max_tokens = 512
max_file_size_bytes = 51_200           # Skip files > 50KB for LLM
batch_delay_ms = 2000                  # Batch files before sending
```

## Testing

The LLM client includes unit tests that don't make real API calls:

```bash
cargo test -p ambient-fs-server --lib llm
```

For integration testing with real API calls (requires API key):

```bash
ANTHROPIC_API_KEY=sk-... cargo test --features llm-integration-tests
```

## Implementation Details

### HTTP Request Format

The client sends POST requests to Anthropic's Messages API:

```http
POST /v1/messages HTTP/1.1
Host: api.anthropic.com
x-api-key: sk-ant-...
anthropic-version: 2023-06-01
content-type: application/json

{
  "model": "claude-haiku-4-5-20251001",
  "max_tokens": 512,
  "system": "you analyze source code...",
  "messages": [
    {
      "role": "user",
      "content": "language: rust\n..."
    }
  ]
}
```

### Response Parsing

The client extracts text from the response:

```json
{
  "id": "msg_...",
  "type": "message",
  "role": "assistant",
  "content": [
    {
      "type": "text",
      "text": "{\"imports\": [...], \"exports\": [...]}"
    }
  ]
}
```

The `call_json()` method then parses the `text` field as JSON.

## Future Enhancements

- Streaming responses for faster feedback
- Retry logic with exponential backoff
- Request batching for multiple files
- Caching responses based on content hash
- Support for other providers (OpenAI, etc.)
