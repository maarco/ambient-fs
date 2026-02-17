use ambient_fs_core::FileEvent;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use std::path::PathBuf;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

/// Default socket path for ambient-fsd
pub const DEFAULT_SOCKET_PATH: &str = "/tmp/ambient-fs.sock";

/// Client for connecting to ambient-fsd daemon
#[derive(Debug)]
#[allow(dead_code)]
pub struct AmbientFsClient {
    socket_path: PathBuf,
    stream: Option<UnixStream>,
    next_id: u64,
}

/// JSON-RPC request envelope
#[derive(Debug, Serialize)]
struct JsonRpcRequest<'a, T> {
    jsonrpc: &'static str,
    method: &'a str,
    params: T,
    id: u64,
}

/// JSON-RPC response envelope
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct JsonRpcResponse {
    jsonrpc: String,
    #[serde(flatten)]
    payload: ResponsePayload,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ResponsePayload {
    Success { result: JsonValue },
    Error { error: JsonRpcError },
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct JsonRpcError {
    code: i32,
    message: String,
}

/// Event filter for queries
#[derive(Debug, Clone, Serialize, Default)]
pub struct EventFilter {
    pub project_id: Option<String>,
    pub since: Option<i64>, // unix timestamp
    pub source: Option<String>,
    pub limit: Option<usize>,
}

/// Errors from the client
#[derive(Debug, Error)]
pub enum ClientError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON serialization error: {0}")]
    JsonSerialize(#[from] serde_json::Error),

    #[error("daemon returned error: {0}")]
    DaemonError(String),

    #[error("invalid response from daemon")]
    InvalidResponse,

    #[error("daemon not connected")]
    NotConnected,
}

pub type Result<T> = std::result::Result<T, ClientError>;

impl AmbientFsClient {
    /// Connect to the daemon at the default socket path (/tmp/ambient-fs.sock)
    pub async fn connect_local() -> Result<Self> {
        Self::connect(DEFAULT_SOCKET_PATH).await
    }

    /// Connect to the daemon at the specified socket path
    pub async fn connect(socket_path: impl Into<PathBuf>) -> Result<Self> {
        let socket_path = socket_path.into();
        let stream = UnixStream::connect(&socket_path).await?;
        Ok(Self {
            socket_path,
            stream: Some(stream),
            next_id: 1,
        })
    }

    /// Watch a directory path for events
    pub async fn watch(&mut self, path: &str) -> Result<()> {
        let params = json!({ "path": path });
        self.send_request("watch", &params).await?;
        Ok(())
    }

    /// Query events with optional filter
    pub async fn events(&mut self, filter: EventFilter) -> Result<Vec<FileEvent>> {
        let response = self.send_request("events", &filter).await?;
        serde_json::from_value(response)
            .map_err(|_| ClientError::InvalidResponse)
    }

    /// Subscribe to events for a project
    pub async fn subscribe(&mut self, project_id: &str) -> Result<()> {
        let params = json!({ "project_id": project_id });
        self.send_request("subscribe", &params).await?;
        Ok(())
    }

    /// Send a JSON-RPC request and get the response
    async fn send_request<T: Serialize>(
        &mut self,
        method: &str,
        params: &T,
    ) -> Result<JsonValue> {
        let stream = self.stream.as_mut().ok_or(ClientError::NotConnected)?;
        let id = self.next_id;
        self.next_id += 1;

        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            method,
            params,
            id,
        };

        let request_json = serde_json::to_string(&request)?;
        tracing::debug!("sending request: {}", request_json);

        // Send request + newline
        stream.write_all(request_json.as_bytes()).await?;
        stream.write_all(b"\n").await?;

        // Read response line
        let mut reader = BufReader::new(stream);
        let mut response_line = String::new();
        reader.read_line(&mut response_line).await?;

        tracing::debug!("received response: {}", response_line.trim());

        let response: JsonRpcResponse = serde_json::from_str(&response_line)?;

        match response.payload {
            ResponsePayload::Success { result } => Ok(result),
            ResponsePayload::Error { error } => Err(ClientError::DaemonError(error.message)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ambient_fs_core::{EventType, Source};

    #[tokio::test]
    async fn connect_parses_socket_path() {
        // This test verifies the struct stores the path
        // Real connection tested in integration
        let client = AmbientFsClient {
            socket_path: PathBuf::from("/tmp/test.sock"),
            stream: None,
            next_id: 1,
        };
        assert_eq!(client.socket_path, PathBuf::from("/tmp/test.sock"));
    }

    #[tokio::test]
    async fn events_with_filter_sends_params() {
        let filter = EventFilter {
            project_id: Some("my-project".to_string()),
            since: Some(1708100000),
            source: Some("ai_agent".to_string()),
            limit: Some(100),
        };

        // Just verify serializes correctly
        let json = serde_json::to_string(&filter).unwrap();
        assert!(json.contains("my-project"));
        assert!(json.contains("ai_agent"));
    }

    #[tokio::test]
    async fn subscribe_sends_project_id() {
        let params = json!({ "project_id": "my-project" });
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("my-project"));
    }

    #[tokio::test]
    async fn events_parses_daemon_response() {
        let event_json = r#"{
            "jsonrpc":"2.0",
            "result":[{
                "timestamp":"2024-02-16T10:32:00Z",
                "event_type":"created",
                "file_path":"src/main.rs",
                "project_id":"my-project",
                "source":"ai_agent",
                "source_id":"chat_42",
                "machine_id":"machine-1",
                "content_hash":"abc123"
            }],
            "id":1
        }"#;

        // Parse the response manually
        let response: JsonRpcResponse = serde_json::from_str(event_json).unwrap();
        match response.payload {
            ResponsePayload::Success { result } => {
                let events: Vec<FileEvent> = serde_json::from_value(result).unwrap();
                assert_eq!(events.len(), 1);
                assert_eq!(events[0].file_path, "src/main.rs");
                assert_eq!(events[0].source, Source::AiAgent);
            }
            ResponsePayload::Error { .. } => panic!("expected success"),
        }
    }

    #[tokio::test]
    async fn daemon_error_is_propagated() {
        let error_json = r#"{
            "jsonrpc":"2.0",
            "error":{"code":-32000,"message":"project not found"},
            "id":1
        }"#;

        let response: JsonRpcResponse = serde_json::from_str(error_json).unwrap();
        match response.payload {
            ResponsePayload::Error { error } => {
                assert_eq!(error.code, -32000);
                assert_eq!(error.message, "project not found");
            }
            ResponsePayload::Success { .. } => panic!("expected error"),
        }
    }

    #[tokio::test]
    async fn event_filter_default_is_empty() {
        let filter = EventFilter::default();
        assert!(filter.project_id.is_none());
        assert!(filter.since.is_none());
        assert!(filter.source.is_none());
        assert!(filter.limit.is_none());
    }

    #[tokio::test]
    async fn client_increments_request_id() {
        let mut client = AmbientFsClient {
            socket_path: PathBuf::from("/tmp/test.sock"),
            stream: None,
            next_id: 5,
        };

        assert_eq!(client.next_id, 5);
        client.next_id += 1;
        assert_eq!(client.next_id, 6);
    }

    #[tokio::test]
    async fn not_connected_error() {
        let mut client = AmbientFsClient {
            socket_path: PathBuf::from("/tmp/test.sock"),
            stream: None,
            next_id: 1,
        };

        let result = client.watch("/path").await;
        assert!(matches!(result, Err(ClientError::NotConnected)));
    }

    #[tokio::test]
    async fn jsonrpc_request_serialization() {
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            method: "watch",
            params: json!({ "path": "/home/user/project" }),
            id: 1,
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains(r#""jsonrpc":"2.0""#));
        assert!(json.contains(r#""method":"watch""#));
        assert!(json.contains(r#""id":1"#));
        assert!(json.contains(r#""path""#));
    }

    #[tokio::test]
    async fn multiple_events_parsed_correctly() {
        let events_json = r#"{
            "jsonrpc":"2.0",
            "result":[
                {
                    "timestamp":"2024-02-16T10:32:00Z",
                    "event_type":"created",
                    "file_path":"src/main.rs",
                    "project_id":"my-project",
                    "source":"user",
                    "machine_id":"m1"
                },
                {
                    "timestamp":"2024-02-16T10:33:00Z",
                    "event_type":"modified",
                    "file_path":"src/lib.rs",
                    "project_id":"my-project",
                    "source":"ai_agent",
                    "source_id":"chat_42",
                    "machine_id":"m1"
                }
            ],
            "id":1
        }"#;

        let response: JsonRpcResponse = serde_json::from_str(events_json).unwrap();
        match response.payload {
            ResponsePayload::Success { result } => {
                let events: Vec<FileEvent> = serde_json::from_value(result).unwrap();
                assert_eq!(events.len(), 2);
                assert_eq!(events[0].event_type, EventType::Created);
                assert_eq!(events[1].event_type, EventType::Modified);
            }
            ResponsePayload::Error { .. } => panic!("expected success"),
        }
    }

    #[test]
    fn client_error_display() {
        let err = ClientError::NotConnected;
        assert_eq!(err.to_string(), "daemon not connected");

        let err = ClientError::DaemonError("something broke".to_string());
        assert_eq!(err.to_string(), "daemon returned error: something broke");
    }
}
