// LLM client for haiku-class API calls
//
// Shared infrastructure for analyzer and agent tracker.

use reqwest::Client;
use serde::de::DeserializeOwned;
use serde_json::json;
use thiserror::Error;

/// Configuration for the LLM client.
#[derive(Debug, Clone)]
pub struct LlmConfig {
    /// API key for Anthropic (None = disabled)
    pub api_key: Option<String>,
    /// Model to use (default: claude-haiku-4-5-20251001)
    pub model: String,
    /// Base URL for API (default: https://api.anthropic.com/v1)
    pub base_url: String,
    /// Max tokens in response (default: 512)
    pub max_tokens: usize,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            api_key: None,
            model: "claude-haiku-4-5-20251001".to_string(),
            base_url: "https://api.anthropic.com/v1".to_string(),
            max_tokens: 512,
        }
    }
}

/// Errors from LLM calls.
#[derive(Debug, Error)]
pub enum LlmError {
    /// LLM is disabled (no API key configured)
    #[error("LLM client is disabled (no API key)")]
    Disabled,

    /// HTTP request failed
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    /// API returned an error status
    #[error("API error {status}: {message}")]
    Api { status: u16, message: String },

    /// Failed to parse JSON response
    #[error("Failed to parse JSON: {0}")]
    Parse(#[from] serde_json::Error),
}

/// LLM client for Anthropic API (haiku-class models).
#[derive(Debug)]
pub struct LlmClient {
    config: LlmConfig,
    client: Client,
}

impl LlmClient {
    /// Create a new LLM client with the given config.
    pub fn new(config: LlmConfig) -> Self {
        Self {
            config,
            client: Client::new(),
        }
    }

    /// Check if the LLM client is enabled (has API key).
    pub fn is_enabled(&self) -> bool {
        self.config.api_key.is_some()
    }

    /// Make an async LLM call, returning the raw text response.
    ///
    /// # Errors
    /// - `Disabled` if no API key is configured
    /// - `Http` if the request fails
    /// - `Api` if the API returns an error status
    pub async fn call(&self, system: &str, user: &str) -> Result<String, LlmError> {
        let api_key = self
            .config
            .api_key
            .as_ref()
            .ok_or(LlmError::Disabled)?;

        let url = format!("{}/messages", self.config.base_url);

        let body = json!({
            "model": self.config.model,
            "max_tokens": self.config.max_tokens,
            "system": system,
            "messages": [
                {
                    "role": "user",
                    "content": user
                }
            ]
        });

        let response = self
            .client
            .post(&url)
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        let response_text = response.text().await?;

        if !status.is_success() {
            return Err(LlmError::Api {
                status: status.as_u16(),
                message: response_text,
            });
        }

        // Parse response to extract content
        let parsed: serde_json::Value = serde_json::from_str(&response_text)?;

        parsed["content"]
            .get(0)
            .and_then(|v| v.get("text"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| {
                LlmError::Api {
                    status: 200,
                    message: format!("Unexpected response format: {}", response_text),
                }
            })
    }

    /// Make an async LLM call and parse the response as JSON.
    ///
    /// # Type Parameters
    /// - `T`: The type to deserialize the response into
    ///
    /// # Errors
    /// - `Disabled` if no API key is configured
    /// - `Http` if the request fails
    /// - `Api` if the API returns an error status
    /// - `Parse` if the response is not valid JSON for type T
    pub async fn call_json<T: DeserializeOwned>(
        &self,
        system: &str,
        user: &str,
    ) -> Result<T, LlmError> {
        let text = self.call(system, user).await?;
        serde_json::from_str(&text).map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_llm_config_defaults() {
        let config = LlmConfig::default();
        assert!(config.api_key.is_none());
        assert_eq!(config.model, "claude-haiku-4-5-20251001");
        assert_eq!(config.base_url, "https://api.anthropic.com/v1");
        assert_eq!(config.max_tokens, 512);
    }

    #[test]
    fn test_is_enabled_with_api_key() {
        let config = LlmConfig {
            api_key: Some("sk-test-key".to_string()),
            ..Default::default()
        };
        let client = LlmClient::new(config);
        assert!(client.is_enabled());
    }

    #[test]
    fn test_is_enabled_without_api_key() {
        let config = LlmConfig::default();
        let client = LlmClient::new(config);
        assert!(!client.is_enabled());
    }

    #[test]
    fn test_call_json_parsing_valid_json() {
        let json = r#"{"imports": [], "exports": [], "lint_hints": []}"#;
        let result: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(result["imports"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn test_call_json_error_on_invalid_json() {
        let json = r#"not valid json"#;
        let result: Result<serde_json::Value, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }
}
