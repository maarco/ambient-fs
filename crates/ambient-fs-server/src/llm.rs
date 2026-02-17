// LLM client - provider selected by AMBIENT_FS_LLM_MODEL env var
//
// Env vars:
//   AMBIENT_FS_LLM_MODEL    required - enables LLM, model name routes to provider
//   AMBIENT_FS_LLM_BASE_URL optional - custom OpenAI-compatible endpoint
//
// Standard API keys read automatically by genai:
//   ANTHROPIC_API_KEY, OPENAI_API_KEY, GROQ_API_KEY, etc.
//
// Model routing (via genai):
//   claude-*   -> Anthropic
//   gpt-*      -> OpenAI
//   gemini-*   -> Gemini
//   llama*, mistral*, etc. -> Ollama (local)
//
// Custom base URL (OpenRouter, lm-studio, any OpenAI-compatible):
//   AMBIENT_FS_LLM_BASE_URL=https://openrouter.ai/api/v1
//   OPENAI_API_KEY=sk-or-...
//   AMBIENT_FS_LLM_MODEL=anthropic/claude-haiku-4:beta

use genai::Client as GenaiClient;
use genai::chat::{ChatMessage, ChatRequest};
use reqwest::Client as HttpClient;
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LlmError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("API error {status}: {message}")]
    Api { status: u16, message: String },

    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("provider error: {0}")]
    Provider(String),
}

enum Provider {
    /// Standard provider via genai - model name routes automatically
    Genai(GenaiClient),
    /// Custom OpenAI-compatible endpoint (OpenRouter, lm-studio, ollama, etc.)
    Custom {
        http: HttpClient,
        base_url: String,
        api_key: Option<String>,
    },
}

/// LLM client driven by env vars.
///
/// Feature is enabled by setting AMBIENT_FS_LLM_MODEL.
/// Absent = disabled, from_env() returns None.
pub struct LlmClient {
    provider: Provider,
    model: String,
}

impl LlmClient {
    /// Build from env vars. Returns None if AMBIENT_FS_LLM_MODEL is not set.
    ///
    /// Provider selection:
    /// - AMBIENT_FS_LLM_BASE_URL set -> custom OpenAI-compatible HTTP client
    /// - Otherwise -> genai (model name determines provider)
    pub fn from_env() -> Option<Self> {
        let model = std::env::var("AMBIENT_FS_LLM_MODEL").ok()?;

        let provider = if let Ok(base_url) = std::env::var("AMBIENT_FS_LLM_BASE_URL") {
            // Custom base URL: use OpenAI-compatible format via reqwest
            // Try OPENAI_API_KEY first (covers OpenRouter), then ANTHROPIC_API_KEY
            let api_key = std::env::var("OPENAI_API_KEY")
                .or_else(|_| std::env::var("ANTHROPIC_API_KEY"))
                .ok();
            Provider::Custom {
                http: HttpClient::new(),
                base_url,
                api_key,
            }
        } else {
            // Standard provider: genai reads API keys from env automatically
            Provider::Genai(GenaiClient::default())
        };

        Some(Self { provider, model })
    }

    /// The model name being used.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Whether a custom base URL is in use.
    pub fn is_custom_endpoint(&self) -> bool {
        matches!(self.provider, Provider::Custom { .. })
    }

    /// Make an LLM call with system + user prompts. Returns raw text response.
    pub async fn call(&self, system: &str, user: &str) -> Result<String, LlmError> {
        match &self.provider {
            Provider::Genai(client) => {
                let req = ChatRequest::from_system(system)
                    .append_message(ChatMessage::user(user));

                let res = client
                    .exec_chat(&self.model, req, None)
                    .await
                    .map_err(|e| LlmError::Provider(e.to_string()))?;

                Ok(res.first_text().unwrap_or_default().to_string())
            }

            Provider::Custom { http, base_url, api_key } => {
                // Support both full URL and base URL forms:
                //   https://api.z.ai/api/coding/paas/v4/chat/completions  (full)
                //   https://openrouter.ai/api/v1                           (base, we append)
                let url = if base_url.ends_with("/chat/completions") {
                    base_url.clone()
                } else {
                    format!("{}/chat/completions", base_url.trim_end_matches('/'))
                };

                let body = json!({
                    "model": self.model,
                    "max_tokens": 16096,
                    "messages": [
                        {"role": "system", "content": system},
                        {"role": "user", "content": user}
                    ]
                });

                let mut req = http.post(&url).json(&body);
                if let Some(key) = api_key {
                    req = req.header("Authorization", format!("Bearer {}", key));
                }

                let resp = req.send().await?;
                let status = resp.status();
                let text = resp.text().await?;

                if !status.is_success() {
                    return Err(LlmError::Api {
                        status: status.as_u16(),
                        message: text,
                    });
                }

                let parsed: serde_json::Value = serde_json::from_str(&text)?;
                Ok(parsed["choices"][0]["message"]["content"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    fn clear_llm_env() {
        std::env::remove_var("AMBIENT_FS_LLM_MODEL");
        std::env::remove_var("AMBIENT_FS_LLM_BASE_URL");
        std::env::remove_var("OPENAI_API_KEY");
    }

    #[test]
    fn from_env_returns_none_when_model_not_set() {
        let _lock = ENV_MUTEX.lock().unwrap();
        clear_llm_env();

        assert!(LlmClient::from_env().is_none());
    }

    #[test]
    fn from_env_returns_some_when_model_set() {
        let _lock = ENV_MUTEX.lock().unwrap();
        clear_llm_env();
        std::env::set_var("AMBIENT_FS_LLM_MODEL", "claude-haiku-4-5-20251001");

        let client = LlmClient::from_env();
        assert!(client.is_some());

        clear_llm_env();
    }

    #[test]
    fn model_returns_env_value() {
        let _lock = ENV_MUTEX.lock().unwrap();
        clear_llm_env();
        std::env::set_var("AMBIENT_FS_LLM_MODEL", "gpt-4o-mini");

        let client = LlmClient::from_env().unwrap();
        assert_eq!(client.model(), "gpt-4o-mini");

        clear_llm_env();
    }

    #[test]
    fn uses_genai_provider_without_base_url() {
        let _lock = ENV_MUTEX.lock().unwrap();
        clear_llm_env();
        std::env::set_var("AMBIENT_FS_LLM_MODEL", "claude-haiku-4-5-20251001");

        let client = LlmClient::from_env().unwrap();
        assert!(!client.is_custom_endpoint());

        clear_llm_env();
    }

    #[test]
    fn uses_custom_provider_with_base_url() {
        let _lock = ENV_MUTEX.lock().unwrap();
        clear_llm_env();
        std::env::set_var("AMBIENT_FS_LLM_MODEL", "anthropic/claude-haiku-4:beta");
        std::env::set_var("AMBIENT_FS_LLM_BASE_URL", "https://openrouter.ai/api/v1");

        let client = LlmClient::from_env().unwrap();
        assert!(client.is_custom_endpoint());

        clear_llm_env();
    }

    #[test]
    fn custom_provider_reads_openai_api_key() {
        let _lock = ENV_MUTEX.lock().unwrap();
        clear_llm_env();
        std::env::set_var("AMBIENT_FS_LLM_MODEL", "gpt-4o-mini");
        std::env::set_var("AMBIENT_FS_LLM_BASE_URL", "http://localhost:11434/v1");
        std::env::set_var("OPENAI_API_KEY", "sk-test-key");

        // Should not panic - just verify client builds correctly
        let client = LlmClient::from_env().unwrap();
        assert!(client.is_custom_endpoint());

        clear_llm_env();
    }

    #[test]
    fn different_models_all_enable_feature() {
        let _lock = ENV_MUTEX.lock().unwrap();

        for model in &[
            "claude-haiku-4-5-20251001",
            "gpt-4o-mini",
            "llama3.2",
            "anthropic/claude-haiku-4:beta",
        ] {
            clear_llm_env();
            std::env::set_var("AMBIENT_FS_LLM_MODEL", model);
            assert!(LlmClient::from_env().is_some(), "model {} should enable feature", model);
        }

        clear_llm_env();
    }
}
