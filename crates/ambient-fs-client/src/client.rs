use ambient_fs_core::{awareness::FileAwareness, FileEvent};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;

// Platform-specific stream types: Unix socket on unix, TCP on windows.
// Both OwnedWriteHalf variants implement AsyncWrite, so the rest of the
// code is identical across platforms.
#[cfg(unix)]
use tokio::net::unix::OwnedWriteHalf;
#[cfg(unix)]
use tokio::net::UnixStream;

#[cfg(windows)]
use tokio::net::tcp::OwnedWriteHalf;
#[cfg(windows)]
use tokio::net::TcpStream;

/// Default socket path for ambient-fsd (Unix)
#[cfg(unix)]
pub const DEFAULT_SOCKET_PATH: &str = "/tmp/ambient-fs.sock";

/// Default TCP address for ambient-fsd (Windows)
#[cfg(windows)]
pub const DEFAULT_ADDR: &str = "127.0.0.1:9851";

/// Default notification channel buffer size
const DEFAULT_NOTIFICATION_BUFFER: usize = 256;

/// A generic notification pushed by the server (JSON-RPC notification: has method, no id).
///
/// This is the raw wire type. For typed parsing, see [`Notification`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClientNotification {
    pub method: String,
    #[serde(default)]
    pub params: JsonValue,
}

/// Typed notification variants received from daemon (pushed to subscribers).
///
/// Use [`AmbientFsClient::recv_notification`] for typed access, or
/// [`AmbientFsClient::take_notification_stream`] for raw [`ClientNotification`] access.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "method")]
pub enum Notification {
    /// Raw file event from the watcher
    #[serde(rename = "event")]
    Event { params: FileEvent },
    /// File awareness state changed
    #[serde(rename = "awareness_changed")]
    AwarenessChanged {
        params: AwarenessChangedParams,
    },
    /// File analysis completed
    #[serde(rename = "analysis_complete")]
    AnalysisComplete {
        params: AnalysisCompleteParams,
    },
    /// Tree structure changed (patch)
    #[serde(rename = "tree_patch")]
    TreePatch { params: TreePatchParams },
}

/// Params for awareness_changed notification
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AwarenessChangedParams {
    pub project_id: String,
    pub file_path: String,
    pub awareness: FileAwareness,
}

/// Params for analysis_complete notification
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnalysisCompleteParams {
    pub project_id: String,
    pub file_path: String,
    pub line_count: u32,
    pub todo_count: u32,
}

/// Params for tree_patch notification
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TreePatchParams {
    pub project_id: String,
    #[serde(flatten)]
    pub patch: serde_json::Value,
}

/// Client for connecting to ambient-fsd daemon.
///
/// Internally splits the Unix socket into read/write halves. A background task
/// reads incoming messages and routes them: responses (has `id`) go to the
/// matching pending request via oneshot, notifications (has `method`, no `id`)
/// go to an mpsc channel.
pub struct AmbientFsClient {
    socket_path: PathBuf,
    writer: OwnedWriteHalf,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<JsonValue>>>>>,
    notification_rx: Option<mpsc::Receiver<ClientNotification>>,
    reader_handle: JoinHandle<()>,
    next_id: AtomicU64,
}

impl std::fmt::Debug for AmbientFsClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AmbientFsClient")
            .field("socket_path", &self.socket_path)
            .field("next_id", &self.next_id)
            .finish_non_exhaustive()
    }
}

impl Drop for AmbientFsClient {
    fn drop(&mut self) {
        self.reader_handle.abort();
    }
}

/// JSON-RPC request envelope
#[derive(Debug, Serialize)]
struct JsonRpcRequest<'a, T> {
    jsonrpc: &'static str,
    method: &'a str,
    params: T,
    id: u64,
}

/// JSON-RPC response envelope (used in tests for parsing validation)
#[cfg(test)]
#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    #[allow(dead_code)]
    jsonrpc: String,
    #[serde(flatten)]
    payload: ResponsePayload,
}

#[cfg(test)]
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ResponsePayload {
    Success { result: JsonValue },
    Error { error: JsonRpcError },
}

#[cfg(test)]
#[derive(Debug, Deserialize)]
struct JsonRpcError {
    #[allow(dead_code)]
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

    #[error("request failed: connection closed")]
    ConnectionClosed,
}

pub type Result<T> = std::result::Result<T, ClientError>;

impl AmbientFsClient {
    /// Connect to the daemon at the default endpoint.
    ///
    /// On Unix, connects to `/tmp/ambient-fs.sock` (Unix socket).
    /// On Windows, connects to `127.0.0.1:9851` (TCP).
    pub async fn connect_local() -> Result<Self> {
        #[cfg(unix)]
        {
            Self::connect(DEFAULT_SOCKET_PATH).await
        }
        #[cfg(windows)]
        {
            Self::connect(DEFAULT_ADDR).await
        }
    }

    /// Connect to the daemon at the specified endpoint.
    ///
    /// On Unix, this is a socket path (e.g. `/tmp/ambient-fs.sock`).
    /// On Windows, this is a TCP address (e.g. `127.0.0.1:9851`).
    pub async fn connect(endpoint: impl Into<PathBuf>) -> Result<Self> {
        let endpoint = endpoint.into();
        #[cfg(unix)]
        let stream = UnixStream::connect(&endpoint).await?;
        #[cfg(windows)]
        let stream = {
            let addr = endpoint.to_string_lossy().into_owned();
            TcpStream::connect(&addr).await?
        };
        Ok(Self::from_stream(stream, endpoint, DEFAULT_NOTIFICATION_BUFFER))
    }

    /// Build a client from a pre-connected UnixStream (unix only).
    #[cfg(unix)]
    pub(crate) fn from_stream(
        stream: UnixStream,
        socket_path: PathBuf,
        notification_buffer: usize,
    ) -> Self {
        let (read_half, write_half) = stream.into_split();
        Self::from_halves(read_half, write_half, socket_path, notification_buffer)
    }

    /// Build a client from a pre-connected TcpStream (windows only).
    #[cfg(windows)]
    pub(crate) fn from_stream(
        stream: TcpStream,
        socket_path: PathBuf,
        notification_buffer: usize,
    ) -> Self {
        let (read_half, write_half) = stream.into_split();
        Self::from_halves(read_half, write_half, socket_path, notification_buffer)
    }

    /// Internal: wire up the reader task and channels from pre-split halves.
    fn from_halves<R: tokio::io::AsyncRead + Unpin + Send + 'static>(
        read_half: R,
        write_half: OwnedWriteHalf,
        socket_path: PathBuf,
        notification_buffer: usize,
    ) -> Self {
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<JsonValue>>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (notification_tx, notification_rx) = mpsc::channel(notification_buffer);

        let reader_pending = pending.clone();
        let reader_handle = tokio::spawn(async move {
            let mut reader = BufReader::new(read_half);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => break, // EOF
                    Ok(_) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        match serde_json::from_str::<JsonValue>(trimmed) {
                            Ok(msg) => {
                                if let Some(id) = msg.get("id").and_then(|v| v.as_u64()) {
                                    // Response to a pending request
                                    let mut map = reader_pending.lock().await;
                                    if let Some(tx) = map.remove(&id) {
                                        let result = if let Some(err) = msg.get("error") {
                                            let message = err
                                                .get("message")
                                                .and_then(|m| m.as_str())
                                                .unwrap_or("unknown error")
                                                .to_string();
                                            Err(ClientError::DaemonError(message))
                                        } else if let Some(result) = msg.get("result") {
                                            Ok(result.clone())
                                        } else {
                                            Err(ClientError::InvalidResponse)
                                        };
                                        let _ = tx.send(result);
                                    }
                                } else if msg.get("method").is_some() {
                                    // Server-pushed notification (no id)
                                    let method = msg["method"]
                                        .as_str()
                                        .unwrap_or("")
                                        .to_string();
                                    let params = msg
                                        .get("params")
                                        .cloned()
                                        .unwrap_or(JsonValue::Null);
                                    let notif = ClientNotification { method, params };
                                    match notification_tx.try_send(notif) {
                                        Ok(()) => {}
                                        Err(mpsc::error::TrySendError::Full(_)) => {
                                            tracing::warn!(
                                                "notification channel full, dropping"
                                            );
                                        }
                                        Err(mpsc::error::TrySendError::Closed(_)) => {
                                            break;
                                        }
                                    }
                                } else {
                                    tracing::warn!(
                                        "unknown message from daemon: {}",
                                        trimmed
                                    );
                                }
                            }
                            Err(e) => {
                                tracing::warn!("failed to parse daemon message: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("reader error: {}", e);
                        break;
                    }
                }
            }
            // Connection closed - fail all pending requests
            let mut map = reader_pending.lock().await;
            for (_, tx) in map.drain() {
                let _ = tx.send(Err(ClientError::ConnectionClosed));
            }
        });

        Self {
            socket_path,
            writer: write_half,
            pending,
            notification_rx: Some(notification_rx),
            reader_handle,
            next_id: AtomicU64::new(1),
        }
    }

    /// Take ownership of the raw notification receiver.
    ///
    /// Returns the mpsc::Receiver for server-pushed notifications as generic
    /// [`ClientNotification`] values. Can only be called once; subsequent
    /// calls return None.
    ///
    /// Note: after calling this, [`recv_notification`](Self::recv_notification)
    /// will return `Err(NotConnected)` since the receiver has been moved out.
    pub fn take_notification_stream(&mut self) -> Option<mpsc::Receiver<ClientNotification>> {
        self.notification_rx.take()
    }

    /// Receive a typed notification from the daemon (blocking).
    ///
    /// After subscribing to projects, call this in a loop to receive pushed
    /// notifications. Returns `Ok(None)` if the connection closed.
    ///
    /// This is mutually exclusive with [`take_notification_stream`](Self::take_notification_stream).
    /// If the notification stream has been taken, returns `Err(NotConnected)`.
    pub async fn recv_notification(&mut self) -> Result<Option<Notification>> {
        let rx = self.notification_rx.as_mut().ok_or(ClientError::NotConnected)?;
        match rx.recv().await {
            Some(raw) => {
                let value = json!({
                    "method": raw.method,
                    "params": raw.params,
                });
                let notification: Notification = serde_json::from_value(value)
                    .map_err(|e| ClientError::DaemonError(
                        format!("invalid notification: {}", e),
                    ))?;
                Ok(Some(notification))
            }
            None => Ok(None),
        }
    }

    /// Check if the client's reader task is still running (connection alive).
    pub fn is_connected(&self) -> bool {
        !self.reader_handle.is_finished()
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
        serde_json::from_value(response).map_err(|_| ClientError::InvalidResponse)
    }

    /// Subscribe to events for a project
    pub async fn subscribe(&mut self, project_id: &str) -> Result<()> {
        let params = json!({ "project_id": project_id });
        self.send_request("subscribe", &params).await?;
        Ok(())
    }

    // ===== protocol methods matching server =====

    /// Watch a project directory and get its project_id
    pub async fn watch_project(&mut self, path: &str) -> Result<String> {
        let params = json!({ "path": path });
        let response = self.send_request("watch_project", &params).await?;
        serde_json::from_value(response).map_err(|_| ClientError::InvalidResponse)
    }

    /// Unwatch a project by ID
    pub async fn unwatch_project(&mut self, project_id: &str) -> Result<()> {
        let params = json!({ "project_id": project_id });
        self.send_request("unwatch_project", &params).await?;
        Ok(())
    }

    /// Unsubscribe from project notifications
    pub async fn unsubscribe(&mut self, project_id: &str) -> Result<()> {
        let params = json!({ "project_id": project_id });
        self.send_request("unsubscribe", &params).await?;
        Ok(())
    }

    /// Query events with filter (renamed from events)
    pub async fn query_events(&mut self, filter: EventFilter) -> Result<Vec<FileEvent>> {
        let response = self.send_request("query_events", &filter).await?;
        serde_json::from_value(response).map_err(|_| ClientError::InvalidResponse)
    }

    /// Query awareness for a file in a project
    pub async fn query_awareness(
        &mut self,
        project_id: &str,
        path: &str,
    ) -> Result<FileAwareness> {
        let params = json!({
            "project_id": project_id,
            "path": path,
        });
        let response = self.send_request("query_awareness", &params).await?;
        serde_json::from_value(response).map_err(|_| ClientError::InvalidResponse)
    }

    /// Attribute a file change to a specific source
    pub async fn attribute(
        &mut self,
        project_id: &str,
        file_path: &str,
        source: &str,
        source_id: Option<&str>,
    ) -> Result<()> {
        let mut params = json!({
            "project_id": project_id,
            "file_path": file_path,
            "source": source,
        });
        if let Some(sid) = source_id {
            params["source_id"] = json!(sid);
        }
        self.send_request("attribute", &params).await?;
        Ok(())
    }

    /// Query active agents (returns generic JSON since AgentInfo not defined yet)
    pub async fn query_agents(&mut self) -> Result<Vec<serde_json::Value>> {
        let empty = json!({});
        let response = self.send_request("query_agents", &empty).await?;
        serde_json::from_value(response).map_err(|_| ClientError::InvalidResponse)
    }

    /// Send a JSON-RPC request and get the response.
    ///
    /// Registers a oneshot channel in the pending map, writes the request,
    /// and awaits the response routed by the background reader task.
    async fn send_request<T: Serialize>(
        &mut self,
        method: &str,
        params: &T,
    ) -> Result<JsonValue> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            method,
            params,
            id,
        };

        let mut request_json = serde_json::to_string(&request)?;
        request_json.push('\n');
        tracing::debug!("sending request: {}", request_json.trim());

        // Register pending response before writing
        let (tx, rx) = oneshot::channel();
        {
            let mut map = self.pending.lock().await;
            map.insert(id, tx);
        }

        // Write request - clean up pending on failure
        if let Err(e) = self.writer.write_all(request_json.as_bytes()).await {
            self.pending.lock().await.remove(&id);
            return Err(ClientError::Io(e));
        }

        // Wait for response from reader task
        rx.await.map_err(|_| ClientError::ConnectionClosed)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ambient_fs_core::{EventType, Source};

    /// Create a client connected to a mock server via UnixStream::pair()
    fn mock_client() -> (AmbientFsClient, UnixStream) {
        let (client_stream, server_stream) = UnixStream::pair().unwrap();
        let client = AmbientFsClient::from_stream(
            client_stream,
            PathBuf::from("/tmp/test.sock"),
            256,
        );
        (client, server_stream)
    }

    // ===== notification / routing tests =====

    #[tokio::test]
    async fn notification_serde_roundtrip() {
        let notif = ClientNotification {
            method: "event".to_string(),
            params: json!({"project_id": "proj-1", "path": "src/main.rs"}),
        };
        let serialized = serde_json::to_string(&notif).unwrap();
        let parsed: ClientNotification = serde_json::from_str(&serialized).unwrap();
        assert_eq!(parsed, notif);
    }

    #[tokio::test]
    async fn notification_deserialize_without_params() {
        let raw = r#"{"method":"ping"}"#;
        let notif: ClientNotification = serde_json::from_str(raw).unwrap();
        assert_eq!(notif.method, "ping");
        assert_eq!(notif.params, JsonValue::Null);
    }

    #[tokio::test]
    async fn reader_routes_response_by_id() {
        let (client, mut server) = mock_client();

        let (tx10, rx10) = oneshot::channel();
        let (tx20, rx20) = oneshot::channel();
        {
            let mut map = client.pending.lock().await;
            map.insert(10, tx10);
            map.insert(20, tx20);
        }

        // Send responses out of order: 20 first, then 10
        let r20 = format!("{}\n", json!({"jsonrpc":"2.0","result":"twenty","id":20}));
        let r10 = format!("{}\n", json!({"jsonrpc":"2.0","result":"ten","id":10}));
        server.write_all(r20.as_bytes()).await.unwrap();
        server.write_all(r10.as_bytes()).await.unwrap();

        let result20 = rx20.await.unwrap().unwrap();
        let result10 = rx10.await.unwrap().unwrap();
        assert_eq!(result20, json!("twenty"));
        assert_eq!(result10, json!("ten"));
    }

    #[tokio::test]
    async fn reader_routes_error_response() {
        let (client, mut server) = mock_client();

        let (tx, rx) = oneshot::channel();
        {
            client.pending.lock().await.insert(1, tx);
        }

        let resp = format!(
            "{}\n",
            json!({"jsonrpc":"2.0","error":{"code":-32000,"message":"not found"},"id":1})
        );
        server.write_all(resp.as_bytes()).await.unwrap();

        let result = rx.await.unwrap();
        assert!(matches!(result, Err(ClientError::DaemonError(msg)) if msg == "not found"));
    }

    #[tokio::test]
    async fn reader_routes_notification_to_channel() {
        let (mut client, mut server) = mock_client();
        let mut rx = client.take_notification_stream().unwrap();

        let notif = format!(
            "{}\n",
            json!({"jsonrpc":"2.0","method":"event","params":{"path":"src/lib.rs"}})
        );
        server.write_all(notif.as_bytes()).await.unwrap();

        let received = rx.recv().await.unwrap();
        assert_eq!(received.method, "event");
        assert_eq!(received.params["path"], "src/lib.rs");
    }

    #[tokio::test]
    async fn take_notification_stream_returns_none_on_second_call() {
        let (mut client, _server) = mock_client();
        assert!(client.take_notification_stream().is_some());
        assert!(client.take_notification_stream().is_none());
    }

    #[tokio::test]
    async fn send_request_receives_response() {
        let (mut client, server) = mock_client();

        tokio::spawn(async move {
            let (read_half, mut write_half) = server.into_split();
            let mut reader = BufReader::new(read_half);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let req: JsonValue = serde_json::from_str(&line).unwrap();
            let id = req["id"].as_u64().unwrap();
            assert_eq!(req["method"], "watch");

            let resp = format!("{}\n", json!({"jsonrpc":"2.0","result":"ok","id":id}));
            write_half.write_all(resp.as_bytes()).await.unwrap();
        });

        client.watch("/test/path").await.unwrap();
    }

    #[tokio::test]
    async fn send_request_receives_error_response() {
        let (mut client, server) = mock_client();

        tokio::spawn(async move {
            let (read_half, mut write_half) = server.into_split();
            let mut reader = BufReader::new(read_half);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let req: JsonValue = serde_json::from_str(&line).unwrap();
            let id = req["id"].as_u64().unwrap();

            let resp = format!(
                "{}\n",
                json!({"jsonrpc":"2.0","error":{"code":-32000,"message":"project not found"},"id":id})
            );
            write_half.write_all(resp.as_bytes()).await.unwrap();
        });

        let result = client.watch_project("/nonexistent").await;
        assert!(matches!(result, Err(ClientError::DaemonError(msg)) if msg == "project not found"));
    }

    #[tokio::test]
    async fn connection_closed_fails_pending_requests() {
        let (mut client, server) = mock_client();
        drop(server);

        // Give reader task time to detect EOF
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let result = client.watch("/test").await;
        assert!(matches!(
            result,
            Err(ClientError::ConnectionClosed) | Err(ClientError::Io(_))
        ));
    }

    #[tokio::test]
    async fn notification_channel_closed_after_drop() {
        let (mut client, mut server) = mock_client();
        let mut rx = client.take_notification_stream().unwrap();

        let notif = format!("{}\n", json!({"jsonrpc":"2.0","method":"ping","params":{}}));
        server.write_all(notif.as_bytes()).await.unwrap();
        let _ = rx.recv().await.unwrap();

        drop(client);
        drop(server);

        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn request_ids_increment() {
        let (mut client, server) = mock_client();

        tokio::spawn(async move {
            let (read_half, mut write_half) = server.into_split();
            let mut reader = BufReader::new(read_half);
            for _ in 0..2 {
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                let req: JsonValue = serde_json::from_str(&line).unwrap();
                let id = req["id"].as_u64().unwrap();
                let resp = format!("{}\n", json!({"jsonrpc":"2.0","result":"ok","id":id}));
                write_half.write_all(resp.as_bytes()).await.unwrap();
            }
        });

        client.watch("/path1").await.unwrap();
        client.watch("/path2").await.unwrap();
        assert_eq!(client.next_id.load(Ordering::Relaxed), 3);
    }

    #[tokio::test]
    async fn client_stores_socket_path() {
        let (client, _server) = mock_client();
        assert_eq!(client.socket_path, PathBuf::from("/tmp/test.sock"));
    }

    #[tokio::test]
    async fn is_connected_reflects_reader_state() {
        let (client, server) = mock_client();
        assert!(client.is_connected());

        drop(server);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!client.is_connected());
    }

    #[tokio::test]
    async fn interleaved_notifications_and_responses() {
        let (mut client, server) = mock_client();
        let mut rx = client.take_notification_stream().unwrap();

        tokio::spawn(async move {
            let (read_half, mut write_half) = server.into_split();
            let mut reader = BufReader::new(read_half);

            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let req: JsonValue = serde_json::from_str(&line).unwrap();
            let id = req["id"].as_u64().unwrap();

            // Send notification before response
            let notif = format!(
                "{}\n",
                json!({"jsonrpc":"2.0","method":"event","params":{"type":"created"}})
            );
            write_half.write_all(notif.as_bytes()).await.unwrap();

            // Then the response
            let resp = format!("{}\n", json!({"jsonrpc":"2.0","result":"ok","id":id}));
            write_half.write_all(resp.as_bytes()).await.unwrap();
        });

        client.watch("/test").await.unwrap();

        let notif = rx.recv().await.unwrap();
        assert_eq!(notif.method, "event");
        assert_eq!(notif.params["type"], "created");
    }

    #[tokio::test]
    async fn recv_notification_returns_typed() {
        let (mut client, mut server) = mock_client();

        let notif = format!(
            "{}\n",
            json!({
                "jsonrpc": "2.0",
                "method": "analysis_complete",
                "params": {
                    "project_id": "proj-1",
                    "file_path": "src/main.rs",
                    "line_count": 42,
                    "todo_count": 3
                }
            })
        );
        server.write_all(notif.as_bytes()).await.unwrap();

        let typed = client.recv_notification().await.unwrap().unwrap();
        match typed {
            Notification::AnalysisComplete { params } => {
                assert_eq!(params.project_id, "proj-1");
                assert_eq!(params.line_count, 42);
            }
            _ => panic!("expected AnalysisComplete"),
        }
    }

    #[tokio::test]
    async fn recv_notification_fails_after_take() {
        let (mut client, _server) = mock_client();
        let _rx = client.take_notification_stream().unwrap();

        let result = client.recv_notification().await;
        assert!(matches!(result, Err(ClientError::NotConnected)));
    }

    // ===== preserved serialization/parsing tests =====

    #[tokio::test]
    async fn events_with_filter_sends_params() {
        let filter = EventFilter {
            project_id: Some("my-project".to_string()),
            since: Some(1708100000),
            source: Some("ai_agent".to_string()),
            limit: Some(100),
        };
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

        let err = ClientError::ConnectionClosed;
        assert_eq!(err.to_string(), "request failed: connection closed");
    }

    #[tokio::test]
    async fn attribute_request_serialization() {
        let params = json!({
            "project_id": "my-project",
            "file_path": "src/auth.rs",
            "source": "ai_agent",
            "source_id": "chat-42"
        });
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("my-project"));
        assert!(json.contains("src/auth.rs"));
        assert!(json.contains("ai_agent"));
        assert!(json.contains("chat-42"));
    }

    #[tokio::test]
    async fn attribute_request_without_source_id() {
        let params = json!({
            "project_id": "my-project",
            "file_path": "src/auth.rs",
            "source": "user"
        });
        let json = serde_json::to_string(&params).unwrap();
        assert!(json.contains("user"));
        assert!(!json.contains("source_id"));
    }

    #[tokio::test]
    async fn watch_project_response_parsing() {
        let response_json = r#"{
            "jsonrpc":"2.0",
            "result":"proj-abc-123",
            "id":1
        }"#;

        let response: JsonRpcResponse = serde_json::from_str(response_json).unwrap();
        match response.payload {
            ResponsePayload::Success { result } => {
                let project_id: String = serde_json::from_value(result).unwrap();
                assert_eq!(project_id, "proj-abc-123");
            }
            ResponsePayload::Error { .. } => panic!("expected success"),
        }
    }

    #[tokio::test]
    async fn query_awareness_response_parsing() {
        let awareness_json = r#"{
            "jsonrpc":"2.0",
            "result":{
                "file_path":"src/main.rs",
                "project_id":"proj-123",
                "last_modified":"2024-02-16T10:32:00Z",
                "change_frequency":"hot",
                "modified_by":"ai_agent",
                "todo_count":0,
                "chat_references":0,
                "lint_hints":0,
                "line_count":100
            },
            "id":1
        }"#;

        let response: JsonRpcResponse = serde_json::from_str(awareness_json).unwrap();
        match response.payload {
            ResponsePayload::Success { result } => {
                let awareness: FileAwareness = serde_json::from_value(result).unwrap();
                assert_eq!(awareness.file_path, "src/main.rs");
                assert_eq!(awareness.modified_by, ambient_fs_core::Source::AiAgent);
            }
            ResponsePayload::Error { .. } => panic!("expected success"),
        }
    }

    #[tokio::test]
    async fn query_agents_response_parsing() {
        let agents_json = r#"{
            "jsonrpc":"2.0",
            "result":[
                {"id":"agent-1","name":"claude","status":"active"},
                {"id":"agent-2","name":"cursor","status":"idle"}
            ],
            "id":1
        }"#;

        let response: JsonRpcResponse = serde_json::from_str(agents_json).unwrap();
        match response.payload {
            ResponsePayload::Success { result } => {
                let agents: Vec<serde_json::Value> = serde_json::from_value(result).unwrap();
                assert_eq!(agents.len(), 2);
                assert_eq!(agents[0]["name"], "claude");
            }
            ResponsePayload::Error { .. } => panic!("expected success"),
        }
    }
}
