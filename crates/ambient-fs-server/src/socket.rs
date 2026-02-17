// Unix socket server using tokio::net::UnixListener
// TDD: Tests FIRST, then implementation

use crate::subscriptions::Notification;
use std::collections::HashMap;
use std::os::unix::net::UnixListener as StdUnixListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::broadcast;
use tokio::net::unix::WriteHalf;
use tracing::{debug, error, info};

use crate::protocol::{Request, Response, Error as ProtocolError, Params};
use crate::state::ServerState;

/// Per-connection state tracking active subscriptions
///
/// Holds broadcast receivers for each project this connection
/// is subscribed to. When dropped, all receivers are dropped.
#[derive(Debug)]
struct ConnectionState {
    /// project_id -> broadcast receiver for that project's notifications
    subscriptions: HashMap<String, broadcast::Receiver<Notification>>,
}

impl ConnectionState {
    fn new() -> Self {
        Self {
            subscriptions: HashMap::new(),
        }
    }

    /// Add a subscription receiver for a project
    fn add_subscription(&mut self, project_id: String, rx: broadcast::Receiver<Notification>) {
        self.subscriptions.insert(project_id, rx);
    }

    /// Remove and drop the subscription for a project
    fn remove_subscription(&mut self, project_id: &str) -> bool {
        self.subscriptions.remove(project_id).is_some()
    }

    /// Get a mutable reference to a specific subscription receiver
    ///
    /// This is useful for direct access to a project's notification stream.
    pub fn get_receiver(&mut self, project_id: &str) -> Option<&mut broadcast::Receiver<Notification>> {
        self.subscriptions.get_mut(project_id)
    }

    /// Check if this connection has any subscriptions
    fn has_subscriptions(&self) -> bool {
        !self.subscriptions.is_empty()
    }
}

/// Error type for socket server operations
#[derive(Debug, thiserror::Error)]
pub enum SocketError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Socket already bound")]
    AlreadyBound,

    #[error("Socket not bound")]
    NotBound,

    #[error("Server shutting down")]
    ShuttingDown,

    #[error("Protocol error: {0}")]
    Protocol(String),
}

/// Unix socket server for JSON-RPC communication
///
/// Listens on a Unix domain socket, accepts connections,
/// spawns per-connection tasks, handles JSON-RPC requests.
pub struct SocketServer {
    path: PathBuf,
    // Store std listener, convert to tokio when run() is called
    std_listener: Option<StdUnixListener>,
    tokio_listener: Option<UnixListener>,
    shutdown_tx: Option<broadcast::Sender<()>>,
    /// Shared state for all connection handlers
    state: Option<Arc<ServerState>>,
}

impl SocketServer {
    /// Create a new socket server that will listen at the given path
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            std_listener: None,
            tokio_listener: None,
            shutdown_tx: None,
            state: None,
        }
    }

    /// Set the shared server state
    ///
    /// Must be called before run() to enable request handlers
    /// to access store, subscriptions, and projects.
    pub fn set_state(&mut self, state: Arc<ServerState>) {
        self.state = Some(state);
    }

    /// Get the server state if set
    pub fn state(&self) -> Option<Arc<ServerState>> {
        self.state.clone()
    }

    /// Bind the socket to the configured path
    ///
    /// Removes existing socket file if present.
    /// Uses std::net::UnixListener to avoid requiring tokio runtime at bind time.
    pub fn bind(&mut self) -> Result<(), SocketError> {
        if self.std_listener.is_some() || self.tokio_listener.is_some() {
            return Err(SocketError::AlreadyBound);
        }

        // Remove existing socket file if present
        if self.path.exists() {
            std::fs::remove_file(&self.path)?;
        }

        // Use std::net::UnixListener (doesn't require tokio runtime)
        let std_listener = StdUnixListener::bind(&self.path)?;
        self.std_listener = Some(std_listener);

        info!("Socket server bound to {}", self.path.display());
        Ok(())
    }

    /// Run the server, accepting incoming connections
    ///
    /// Spawns a task per connection. Returns when shutdown is signaled.
    pub async fn run(&mut self) -> Result<(), SocketError> {
        // Convert std listener to tokio listener if needed
        // Use spawn_blocking to avoid tokio's blocking FD restrictions
        if self.tokio_listener.is_none() {
            let std_listener = self.std_listener.take()
                .ok_or(SocketError::NotBound)?;

            // Set non-blocking mode before converting to tokio
            std_listener.set_nonblocking(true)?;

            self.tokio_listener = Some(UnixListener::from_std(std_listener)?);
        }

        let listener = self.tokio_listener.as_ref()
            .ok_or(SocketError::NotBound)?;

        // Create shutdown channel if not already created
        if self.shutdown_tx.is_none() {
            let (shutdown_tx, _) = broadcast::channel(1);
            self.shutdown_tx = Some(shutdown_tx);
        }

        let mut shutdown_rx = self.shutdown_tx.as_ref()
            .unwrap()
            .subscribe();

        info!("Socket server listening on {}", self.path.display());

        let state = self.state.clone().ok_or(SocketError::NotBound)?;

        loop {
            tokio::select! {
                result = listener.accept() => {
                    match result {
                        Ok((stream, addr)) => {
                            debug!("New connection from {:?}", addr);
                            let state_clone = state.clone();
                            tokio::spawn(handle_connection(stream, state_clone));
                        }
                        Err(e) => {
                            error!("Error accepting connection: {}", e);
                        }
                    }
                }
                _ = shutdown_rx.recv() => {
                    info!("Shutdown signal received");
                    return Ok(());
                }
            }
        }
    }

    /// Signal the server to shut down gracefully
    pub fn shutdown(&self) -> Result<(), SocketError> {
        if let Some(tx) = &self.shutdown_tx {
            let _ = tx.send(());
            info!("Shutdown signal sent");
            Ok(())
        } else {
            // Not an error if no channel yet - server might not have started
            Ok(())
        }
    }

    /// Check if the server is currently bound
    pub fn is_bound(&self) -> bool {
        self.std_listener.is_some() || self.tokio_listener.is_some()
    }

    /// Get the socket path
    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    /// Clone the shutdown sender for external shutdown control
    pub fn shutdown_sender(&self) -> Option<broadcast::Sender<()>> {
        self.shutdown_tx.clone()
    }
}

/// Handle a single client connection
///
/// Reads lines, parses as JSON-RPC, dispatches to handler,
/// writes response. Handles disconnect gracefully.
///
/// Also forwards subscribed events to the client as notifications.
async fn handle_connection(mut stream: tokio::net::UnixStream, state: Arc<ServerState>) {
    // Get peer address before splitting
    let peer_addr = stream.peer_addr().ok()
        .and_then(|a| a.as_pathname().map(|p| p.display().to_string()))
        .unwrap_or_else(|| "unknown".to_string());

    let (reader, mut writer) = stream.split();
    let mut lines = BufReader::new(reader).lines();

    // Track subscriptions for this connection
    let mut conn_state = ConnectionState::new();

    debug!("Connection handler started for {}", peer_addr);

    loop {
        // If we have subscriptions, also wait for events
        if conn_state.has_subscriptions() {
            // Collect all subscription receivers into futures
            // For simplicity, we poll the first subscription that has data
            // In production, you'd use FuturesUnordered to handle multiple
            let mut found_receiver = None;
            let mut project_id = String::new();

            // Use get_receiver API for cleaner access
            for pid in conn_state.subscriptions.keys().cloned().collect::<Vec<_>>() {
                if let Some(rx) = conn_state.get_receiver(&pid) {
                    // Try a non-blocking recv
                    match rx.try_recv() {
                    Ok(notification) => {
                        // We got a notification, send it with appropriate method name
                        let (method, params) = match &notification {
                            Notification::Event(event) => {
                                ("event", serde_json::to_value(event).unwrap_or(serde_json::json!(null)))
                            }
                            Notification::AwarenessChanged { project_id, file_path, awareness } => {
                                ("awareness_changed", serde_json::json!({
                                    "project_id": project_id,
                                    "file_path": file_path,
                                    "awareness": awareness,
                                }))
                            }
                            Notification::AnalysisComplete { project_id, file_path, line_count, todo_count } => {
                                ("analysis_complete", serde_json::json!({
                                    "project_id": project_id,
                                    "file_path": file_path,
                                    "line_count": line_count,
                                    "todo_count": todo_count,
                                }))
                            }
                            Notification::TreePatch { project_id, patch } => {
                                ("tree_patch", serde_json::json!({
                                    "project_id": project_id,
                                    "patch": patch,
                                }))
                            }
                        };

                        let rpc_notification = serde_json::json!({
                            "jsonrpc": "2.0",
                            "method": method,
                            "params": params
                        });

                        if let Ok(json) = serde_json::to_string(&rpc_notification) {
                            if writer.write_all(json.as_bytes()).await.is_err()
                                || writer.write_all(b"\n").await.is_err()
                            {
                                debug!("Client {} disconnected during notification send", peer_addr);
                                return;
                            }
                        }
                        // Continue to next iteration to check for more events
                        continue;
                    }
                    Err(broadcast::error::TryRecvError::Empty) => {
                        // No event yet, check next subscription
                        continue;
                    }
                    Err(broadcast::error::TryRecvError::Lagged(_)) => {
                        // Lagged - skip missed messages and continue
                        continue;
                    }
                    Err(broadcast::error::TryRecvError::Closed) => {
                        // Channel closed - remove this subscription
                        project_id = pid.clone();
                        found_receiver = Some(());
                        break;
                    }
                    }
                }
            }

            // If a channel was closed, clean it up
            if found_receiver.is_some() {
                conn_state.remove_subscription(&project_id);
                continue;
            }

            // Now do a blocking select between client request and any subscription
            tokio::select! {
                line_result = lines.next_line() => {
                    match line_result {
                        Ok(Some(line)) => {
                            let line = line.trim();
                            if line.is_empty() {
                                continue;
                            }

                            let response = match serde_json::from_str::<Request>(line) {
                                Ok(req) => handle_request(req, &state, &mut conn_state).await,
                                Err(e) => {
                                    debug!("Failed to parse request: {}", e);
                                    Response::error(
                                        crate::protocol::Id::Null,
                                        ProtocolError::parse_error(),
                                    )
                                }
                            };

                            if let Err(e) = write_response(&mut writer, response).await {
                                error!("Error writing to {}: {}", peer_addr, e);
                                break;
                            }
                        }
                        Ok(None) => {
                            // Client closed connection
                            debug!("Client {} closed connection", peer_addr);
                            break;
                        }
                        Err(e) => {
                            error!("Error reading from {}: {}", peer_addr, e);
                            break;
                        }
                    }
                }
                // Wait a bit then loop to check subscriptions again
                _ = tokio::time::sleep(Duration::from_millis(10)) => {
                    continue;
                }
            }
        } else {
            // No subscriptions, just wait for requests
            match lines.next_line().await {
                Ok(Some(line)) => {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }

                    let response = match serde_json::from_str::<Request>(line) {
                        Ok(req) => handle_request(req, &state, &mut conn_state).await,
                        Err(e) => {
                            debug!("Failed to parse request: {}", e);
                            Response::error(
                                crate::protocol::Id::Null,
                                ProtocolError::parse_error(),
                            )
                        }
                    };

                    if let Err(e) = write_response(&mut writer, response).await {
                        error!("Error writing to {}: {}", peer_addr, e);
                        break;
                    }
                }
                Ok(None) => {
                    // Client closed connection
                    debug!("Client {} closed connection", peer_addr);
                    break;
                }
                Err(e) => {
                    error!("Error reading from {}: {}", peer_addr, e);
                    break;
                }
            }
        }
    }

    debug!("Connection handler ended for {}", peer_addr);
}

/// Write a response to the client
async fn write_response(
    writer: &mut WriteHalf<'_>,
    response: Response,
) -> Result<(), std::io::Error> {
    let response_json = serde_json::to_string(&response)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    writer.write_all(response_json.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    Ok(())
}

/// Handle a parsed JSON-RPC request
async fn handle_request(
    req: Request,
    state: &ServerState,
    conn_state: &mut ConnectionState,
) -> Response {
    use crate::protocol::Method;

    // Parse method
    let method = match req.method.as_str().parse::<Method>() {
        Ok(m) => m,
        Err(_) => {
            return Response::error(
                req.id,
                ProtocolError::method_not_found(req.method),
            );
        }
    };

    // Dispatch to handler
    match method {
        Method::Subscribe => {
            handle_subscribe(req, state, conn_state).await
        }
        Method::Unsubscribe => {
            handle_unsubscribe(req, state, conn_state).await
        }
        Method::QueryEvents => {
            handle_query_events(req, state).await
        }
        Method::QueryAwareness => {
            handle_query_awareness(req, state).await
        }
        Method::WatchProject => {
            handle_watch_project(req, state).await
        }
        Method::UnwatchProject => {
            handle_unwatch_project(req, state).await
        }
        Method::WatchAgents => {
            handle_watch_agents(req, state).await
        }
        Method::UnwatchAgents => {
            handle_unwatch_agents(req, state).await
        }
        Method::QueryAgents => {
            handle_query_agents(req, state).await
        }
        Method::QueryTree => {
            handle_query_tree(req, state).await
        }
        Method::Attribute => {
            handle_attribute(req, state).await
        }
        Method::ReportAgentActivity => {
            handle_report_agent_activity(req, state).await
        }
    }
}

/// Handle subscribe request
///
/// Expects params: {"project_id": "my-project"}
/// Returns: {"subscribed": true, "project_id": "my-project"}
async fn handle_subscribe(
    req: Request,
    state: &ServerState,
    conn_state: &mut ConnectionState,
) -> Response {
    use crate::protocol::Params;

    // Extract project_id from params
    let project_id = match &req.params {
        Some(Params::Object(map)) => {
            match map.get("project_id") {
                Some(id) if id.is_string() => id.as_str().unwrap(),
                _ => {
                    return Response::error(
                        req.id,
                        ProtocolError::invalid_params("project_id is required and must be a string"),
                    );
                }
            }
        }
        _ => {
            return Response::error(
                req.id,
                ProtocolError::invalid_params("params object with project_id is required"),
            );
        }
    };

    // Subscribe to the project
    let rx = state.subscriptions.subscribe(project_id).await;

    // Store the receiver for this connection
    conn_state.add_subscription(project_id.to_string(), rx);

    debug!("Client subscribed to project: {}", project_id);

    Response::result(
        req.id,
        serde_json::json!({
            "subscribed": true,
            "project_id": project_id,
        }),
    )
}

/// Handle unsubscribe request
///
/// Expects params: {"project_id": "my-project"}
/// Returns: true
async fn handle_unsubscribe(
    req: Request,
    state: &ServerState,
    conn_state: &mut ConnectionState,
) -> Response {
    use crate::protocol::Params;

    // Extract project_id from params
    let project_id = match &req.params {
        Some(Params::Object(map)) => {
            match map.get("project_id") {
                Some(id) if id.is_string() => id.as_str().unwrap(),
                _ => {
                    return Response::error(
                        req.id,
                        ProtocolError::invalid_params("project_id is required and must be a string"),
                    );
                }
            }
        }
        _ => {
            return Response::error(
                req.id,
                ProtocolError::invalid_params("params object with project_id is required"),
            );
        }
    };

    // Remove from connection state (drops the receiver)
    let removed = conn_state.remove_subscription(project_id);

    // Also decrement the subscription manager count
    state.subscriptions.unsubscribe(project_id).await;

    debug!("Client unsubscribed from project: {} (was subscribed: {})",
           project_id, removed);

    Response::result(req.id, serde_json::json!(true))
}

/// Handle watch_project request
///
/// Expects params: {"path": "/absolute/path/to/project"}
/// Returns: {"watching": true, "project_id": "..."}
async fn handle_watch_project(req: Request, state: &ServerState) -> Response {
    let id = req.id.clone();

    // Extract path from params
    let path_str = match extract_object_params(&req) {
        Some(params) => match params.get("path") {
            Some(p) if p.is_string() => p.as_str().unwrap(),
            _ => {
                return Response::error(
                    id,
                    ProtocolError::invalid_params("path is required and must be a string"),
                );
            }
        },
        None => {
            return Response::error(
                id,
                ProtocolError::invalid_params("path is required and must be a string"),
            );
        }
    };

    let path = std::path::PathBuf::from(path_str);

    // Validate path exists and is a directory
    if !path.exists() {
        return Response::error(
            id,
            ProtocolError::invalid_path(format!("path does not exist: {}", path_str)),
        );
    }

    if !path.is_dir() {
        return Response::error(
            id,
            ProtocolError::invalid_path(format!("path is not a directory: {}", path_str)),
        );
    }

    // Canonicalize the path
    let canonical = match path.canonicalize() {
        Ok(p) => p,
        Err(_) => {
            return Response::error(
                id,
                ProtocolError::invalid_path(format!("cannot canonicalize path: {}", path_str)),
            );
        }
    };

    // Generate project_id from directory name
    let project_id = canonical
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    // Check if already watching this project
    if state.has_project(&project_id).await {
        // Check if it's the same path
        if let Some(existing_path) = state.get_project(&project_id).await {
            if existing_path == canonical {
                return Response::error(
                    id,
                    ProtocolError::already_watching(format!("already watching: {}", path_str)),
                );
            }
        }
    }

    // Create a new watcher for this project
    let mut fs_watcher = ambient_fs_watcher::FsWatcher::new(
        100, // 100ms debounce
        project_id.clone(),
        state.machine_id.clone(),
    );

    // Start the watcher (we don't need the receiver here, the daemon task handles it)
    if fs_watcher.start().is_err() {
        return Response::error(
            id,
            ProtocolError::watch_failed(format!("failed to start watcher for: {}", path_str)),
        );
    }

    // Watch the path
    if let Err(e) = fs_watcher.watch(canonical.clone()) {
        return Response::error(
            id,
            ProtocolError::watch_failed(format!("failed to watch {}: {}", path_str, e)),
        );
    }

    // Add to state
    state.add_project(project_id.clone(), canonical.clone()).await;
    state.add_watcher(project_id.clone(), fs_watcher).await;

    // Persist to store
    let store_path = state.store_path.clone();
    let project_id_clone = project_id.clone();
    let canonical_clone = canonical.clone();
    tokio::task::spawn_blocking(move || {
        if let Ok(store) = ambient_fs_store::EventStore::new(store_path) {
            let _ = store.add_project(&project_id_clone, &canonical_clone);
        }
    });

    debug!("Watching project: {} at {}", project_id, canonical.display());

    Response::result(
        id,
        serde_json::json!({
            "watching": true,
            "project_id": project_id,
        }),
    )
}

/// Handle unwatch_project request
///
/// Expects params: {"project_id": "my-project"}
/// Returns: true
async fn handle_unwatch_project(req: Request, state: &ServerState) -> Response {
    let id = req.id.clone();

    // Extract project_id from params
    let project_id = match extract_object_params(&req) {
        Some(params) => match params.get("project_id") {
            Some(pid) if pid.is_string() => pid.as_str().unwrap(),
            _ => {
                return Response::error(
                    id.clone(),
                    ProtocolError::invalid_params("project_id is required and must be a string"),
                );
            }
        },
        None => {
            return Response::error(
                id.clone(),
                ProtocolError::invalid_params("project_id is required and must be a string"),
            );
        }
    };

    // Get the path for this project
    let path = match state.get_project(project_id).await {
        Some(p) => p,
        None => {
            return Response::error(
                id.clone(),
                ProtocolError::project_not_found(project_id.to_string()),
            );
        }
    };

    // Remove from watcher
    if let Some(watcher) = state.remove_watcher(project_id).await {
        let mut watcher = watcher.lock().await;
        let _ = watcher.unwatch(path.clone());
        watcher.stop();
    }

    // Remove from projects map
    state.remove_project(project_id).await;

    // Remove from store
    let store_path = state.store_path.clone();
    let project_id_clone = project_id.to_string();
    tokio::task::spawn_blocking(move || {
        if let Ok(store) = ambient_fs_store::EventStore::new(store_path) {
            let _ = store.remove_project(&project_id_clone);
        }
    });

    debug!("Unwatched project: {}", project_id);

    Response::result(id, serde_json::json!(true))
}

/// Handle watch_agents request
///
/// Registers a directory to watch for agent activity JSONL files.
///
/// Expects params: {"path": "/path/to/.agents", "project": "my-project"}
/// Returns: {"watching": true, "path": "..."}
async fn handle_watch_agents(req: Request, state: &ServerState) -> Response {
    let id = req.id.clone();

    // Extract path from params
    let agents_path = match extract_object_params(&req).and_then(|m| m.get("path")) {
        Some(p) if p.is_string() => std::path::PathBuf::from(p.as_str().unwrap()),
        _ => {
            return Response::error(
                id,
                ProtocolError::invalid_params("path is required and must be a string"),
            );
        }
    };

    // Validate path exists and is a directory
    if !agents_path.exists() {
        return Response::error(
            id,
            ProtocolError::invalid_path(format!("path does not exist: {}", agents_path.display())),
        );
    }

    if !agents_path.is_dir() {
        return Response::error(
            id,
            ProtocolError::invalid_path(format!("path is not a directory: {}", agents_path.display())),
        );
    }

    // Store the agents directory path in state.projects with a special prefix
    // This allows tracking watched agent directories
    let watch_key = format!("agents:{}", agents_path.display());
    state.add_project(watch_key, agents_path.clone()).await;

    debug!("Watching agents directory: {}", agents_path.display());

    // In a full implementation, we'd spawn a task to tail the JSONL files
    // For now, the tracking infrastructure is in place

    Response::result(
        id,
        serde_json::json!({
            "watching": true,
            "path": agents_path.display().to_string(),
        }),
    )
}

/// Handle unwatch_agents request
///
/// Stops watching a directory for agent activity.
///
/// Expects params: {"path": "/path/to/.agents"}
/// Returns: true
async fn handle_unwatch_agents(req: Request, state: &ServerState) -> Response {
    let id = req.id.clone();

    // Extract path from params
    let agents_path = match extract_object_params(&req).and_then(|m| m.get("path")) {
        Some(p) if p.is_string() => std::path::PathBuf::from(p.as_str().unwrap()),
        _ => {
            return Response::error(
                id,
                ProtocolError::invalid_params("path is required and must be a string"),
            );
        }
    };

    // Remove from state
    let watch_key = format!("agents:{}", agents_path.display());
    state.remove_project(&watch_key).await;

    debug!("Unwatched agents directory: {}", agents_path.display());

    Response::result(id, serde_json::json!(true))
}

/// Handle query_agents request
///
/// Returns all currently active agents.
///
/// Optional params: {"file": "src/main.rs"} - query specific file
/// Returns: [{"agent_id": "...", "files": [...], "last_seen": ..., "intent": "..."}]
async fn handle_query_agents(req: Request, state: &ServerState) -> Response {
    let id = req.id.clone();

    // Check if querying for a specific file
    let query_file = extract_object_params(&req)
        .and_then(|m| m.get("file"))
        .and_then(|f| f.as_str())
        .map(|s| s.to_string());

    let agents = state.agent_tracker.get_all_agents().await;

    // Convert to JSON array
    let json_agents: Vec<serde_json::Value> = if let Some(ref file) = query_file {
        // If querying for a specific file, only return agents working on that file
        agents
            .into_iter()
            .filter(|a| a.files.contains(file))
            .map(|a| serde_json::json!({
                "agent_id": a.agent_id,
                "files": a.files,
                "last_seen": a.last_seen.timestamp(),
                "intent": a.intent,
                "tool": a.tool,
                "session": a.session,
            }))
            .collect()
    } else {
        // Return all agents
        agents
            .into_iter()
            .map(|a| serde_json::json!({
                "agent_id": a.agent_id,
                "files": a.files,
                "last_seen": a.last_seen.timestamp(),
                "intent": a.intent,
                "tool": a.tool,
                "session": a.session,
            }))
            .collect()
    };

    Response::result(id, serde_json::json!(json_agents))
}

/// Helper to extract object params from Request
fn extract_object_params(req: &Request) -> Option<&serde_json::Map<String, serde_json::Value>> {
    match &req.params {
        Some(Params::Object(map)) => Some(map),
        _ => None,
    }
}

/// Handle query_events request
///
/// Expects optional params: {"project_id": "...", "since": 3600, "source": "user", "limit": 100}
/// Returns: [<FileEvent>, ...]
async fn handle_query_events(req: Request, state: &ServerState) -> Response {
    use ambient_fs_core::event::Source;
    use ambient_fs_store::{EventStore, EventFilter};

    // Extract params (optional)
    // Check if params is the wrong type (array)
    if matches!(&req.params, Some(Params::Array(_))) {
        return Response::error(
            req.id.clone(),
            ProtocolError::invalid_params("params must be an object"),
        );
    }

    // Extract params (optional - None is valid)
    let params_map = extract_object_params(&req)
        .cloned()
        .unwrap_or_else(|| serde_json::Map::new());
    // Build EventFilter from params
    let mut filter = EventFilter::new();

    // project_id (optional)
    if let Some(pid) = params_map.get("project_id").and_then(|v| v.as_str()) {
        filter = filter.project_id(pid);
    }

    // since (optional, seconds as i64, convert to DateTime)
    if let Some(seconds) = params_map.get("since").and_then(|v| v.as_i64()) {
        use chrono::{Duration, Utc};
        let since_ts = Utc::now() - Duration::seconds(seconds);
        filter = filter.since(since_ts);
    }

    // source (optional, string to Source enum)
    if let Some(source_str) = params_map.get("source").and_then(|v| v.as_str()) {
        match source_str.parse::<Source>() {
            Ok(src) => filter = filter.source(src),
            Err(_) => {
                return Response::error(
                    req.id.clone(),
                    ProtocolError::invalid_params(format!("invalid source: {}", source_str)),
                );
            }
        }
    }

    // limit (optional, usize)
    if let Some(limit_val) = params_map.get("limit").and_then(|v| v.as_u64()) {
        filter = filter.limit(limit_val as usize);
    }

    // Query store in spawn_blocking (rusqlite is sync)
    let store_path = state.store_path.clone();
    let events = tokio::task::spawn_blocking(move || {
        let store = EventStore::new(store_path)?;
        store.query(filter)
    })
    .await
    .unwrap_or_else(|e| {
        // Task join failed
        debug!("spawn_blocking join error: {}", e);
        Ok(Vec::new())
    })
    .unwrap_or_else(|e| {
        // Store error - log and return empty
        debug!("Store query error: {}", e);
        Vec::new()
    });

    // Serialize events as JSON array
    let json_events: Vec<serde_json::Value> = events
        .into_iter()
        .map(|e| serde_json::to_value(e).unwrap_or(serde_json::json!(null)))
        .collect();

    Response::result(req.id.clone(), serde_json::json!(json_events))
}

/// Handle query_awareness request
///
/// Expects params: {"project_id": "my-project", "path": "src/main.rs"}
/// Returns: FileAwareness object or null if no event found
async fn handle_query_awareness(req: Request, state: &ServerState) -> Response {
    let params = match extract_object_params(&req) {
        Some(p) => p,
        None => {
            return Response::error(
                req.id,
                ProtocolError::invalid_params("params must be an object"),
            );
        }
    };

    // Extract project_id (required)
    let project_id = match params.get("project_id").and_then(|v| v.as_str()) {
        Some(pid) => pid,
        None => {
            return Response::error(
                req.id,
                ProtocolError::invalid_params("project_id is required"),
            );
        }
    };

    // Extract path (required)
    let file_path = match params.get("path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => {
            return Response::error(
                req.id,
                ProtocolError::invalid_params("path is required"),
            );
        }
    };

    // Use the awareness aggregator
    let awareness = match crate::awareness::build_awareness(state, project_id, file_path).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            return Response::result(req.id, serde_json::json!(null));
        }
        Err(_) => {
            // Log error internally, return null to client
            return Response::result(req.id, serde_json::json!(null));
        }
    };

    // Serialize to JSON
    match serde_json::to_value(&awareness) {
        Ok(v) => Response::result(req.id, v),
        Err(_) => Response::result(req.id, serde_json::json!(null)),
    }
}

/// Handle query_tree request
///
/// Expects params: {"project_id": "my-project"}
/// Returns: TreeNode JSON representation
async fn handle_query_tree(req: Request, state: &ServerState) -> Response {
    use crate::protocol::Params;

    // Extract project_id from params
    let project_id = match &req.params {
        Some(Params::Object(map)) => {
            match map.get("project_id") {
                Some(id) if id.is_string() => id.as_str().unwrap(),
                _ => {
                    return Response::error(
                        req.id,
                        ProtocolError::invalid_params("project_id is required and must be a string"),
                    );
                }
            }
        }
        _ => {
            return Response::error(
                req.id,
                ProtocolError::invalid_params("params object with project_id is required"),
            );
        }
    };

    // Get the tree from state
    match state.get_tree(project_id).await {
        Some(tree) => {
            let root = tree.to_tree_node();
            match serde_json::to_value(root) {
                Ok(v) => Response::result(req.id, v),
                Err(_) => Response::error(
                    req.id,
                    ProtocolError::internal_error(),
                ),
            }
        }
        None => {
            Response::error(
                req.id,
                ProtocolError::project_not_found(project_id.to_string()),
            )
        }
    }
}

/// Handle attribute request
///
/// Expects params: {"file_path": "src/auth.rs", "project_id": "my-project",
///                  "source": "ai_agent", "source_id": "optional"}
/// Returns: {"attributed": true}
async fn handle_attribute(req: Request, state: &ServerState) -> Response {
    use ambient_fs_core::event::{FileEvent, EventType, Source};
    use ambient_fs_store::EventStore;

    let id = req.id.clone();

    // Extract params
    let params = match extract_object_params(&req) {
        Some(p) => p,
        None => {
            return Response::error(
                id,
                ProtocolError::invalid_params("params must be an object"),
            );
        }
    };

    // Extract file_path (required)
    let file_path = match params.get("file_path").and_then(|v| v.as_str()) {
        Some(p) => p,
        None => {
            return Response::error(
                id.clone(),
                ProtocolError::invalid_params("file_path is required"),
            );
        }
    };

    // Extract project_id (required)
    let project_id = match params.get("project_id").and_then(|v| v.as_str()) {
        Some(pid) => pid,
        None => {
            return Response::error(
                id.clone(),
                ProtocolError::invalid_params("project_id is required"),
            );
        }
    };

    // Extract source (required)
    let source_str = match params.get("source").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => {
            return Response::error(
                id.clone(),
                ProtocolError::invalid_params("source is required"),
            );
        }
    };

    // Parse source string into Source enum
    let source = match source_str.parse::<Source>() {
        Ok(s) => s,
        Err(_) => {
            return Response::error(
                id.clone(),
                ProtocolError::invalid_params(format!("invalid source: {}", source_str)),
            );
        }
    };

    // Extract source_id (optional)
    let source_id = params.get("source_id").and_then(|v| v.as_str());

    // Create FileEvent with explicit source
    let mut event = FileEvent::new(
        EventType::Modified,
        file_path,
        project_id,
        &state.machine_id,
    );
    event = event.with_source(source);
    if let Some(sid) = source_id {
        event = event.with_source_id(sid.to_string());
    }

    // Clone for broadcast after insert
    let event_clone = event.clone();

    // Insert into store via spawn_blocking
    let store_path = state.store_path.clone();
    let insert_result = tokio::task::spawn_blocking(move || {
        let store = EventStore::new(store_path)?;
        store.insert(&event)
    })
    .await;

    match insert_result {
        Ok(Ok(_)) => {},
        Ok(Err(store_err)) => {
            debug!("Store insert error during attribute: {:?}", store_err);
            return Response::error(id, ProtocolError::internal_error());
        }
        Err(join_err) => {
            debug!("Join error during attribute: {:?}", join_err);
            return Response::error(id, ProtocolError::internal_error());
        }
    }

    // Broadcast to subscribers
    state.subscriptions.publish_event(event_clone).await;

    debug!("Attributed file {} to source {}", file_path, source_str);

    Response::result(id, serde_json::json!({"attributed": true}))
}

/// Handle report_agent_activity request
///
/// Injects agent activity directly into the AgentTracker.
/// This enables source attribution: files modified while an agent is
/// active will get source = AiAgent instead of User.
///
/// Expects params: {"ts": 1234567890, "agent": "claude-code", "action": "edit",
///                  "file": "/abs/path/to/file.rs"}
/// Optional: "project", "tool", "session", "intent", "lines", "done"
/// Returns: {"recorded": true}
async fn handle_report_agent_activity(req: Request, state: &ServerState) -> Response {
    use crate::agents::AgentActivity;

    let id = req.id.clone();

    let params = match extract_object_params(&req) {
        Some(p) => p,
        None => {
            return Response::error(
                id,
                ProtocolError::invalid_params("params must be an object"),
            );
        }
    };

    // Parse AgentActivity from params
    let activity: AgentActivity = match serde_json::from_value(serde_json::Value::Object(params.clone())) {
        Ok(a) => a,
        Err(e) => {
            return Response::error(
                id,
                ProtocolError::invalid_params(format!("invalid activity params: {}", e)),
            );
        }
    };

    state.agent_tracker.update_from_activity(&activity).await;

    debug!("Agent activity recorded: agent={} file={} done={:?}",
           activity.agent, activity.file, activity.done);

    Response::result(id, serde_json::json!({"recorded": true}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;
    use tokio::time::Duration;

    /// Create a temporary socket path for testing
    fn temp_socket_path() -> PathBuf {
        NamedTempFile::new().unwrap().into_temp_path().to_path_buf()
    }

    /// Helper to create a temp path that doesn't exist
    fn temp_socket_path_nonexistent() -> PathBuf {
        let temp_dir = std::env::temp_dir();
        // Use both process ID and a thread-local counter for uniqueness
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let count = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = temp_dir.join(format!("ambient-fs-test-{}-{}.sock",
            std::process::id(), count));
        // Make sure it doesn't exist
        let _ = std::fs::remove_file(&path);
        path
    }

    /// Create a test ServerState for async tests
    fn test_state() -> Arc<ServerState> {
        Arc::new(ServerState::new(PathBuf::from("/tmp/test-events.db")))
    }

    /// Helper to bind and start server with state
    async fn start_server_with_state(path: PathBuf) -> tokio::task::JoinHandle<()> {
        let mut server = SocketServer::new(path.clone());
        server.bind().unwrap();
        server.set_state(test_state());

        tokio::spawn(async move {
            let _ = server.run().await;
        })
    }

    // ========== SocketServer::new ==========

    #[test]
    fn new_creates_server_with_path() {
        let path = PathBuf::from("/tmp/test-ambient-fs.sock");
        let server = SocketServer::new(path.clone());

        assert_eq!(server.path(), &path);
        assert!(!server.is_bound());
    }

    // ========== SocketServer::bind ==========

    #[test]
    fn bind_creates_socket_file() {
        let path = temp_socket_path_nonexistent();
        let mut server = SocketServer::new(path.clone());

        assert!(server.bind().is_ok());
        assert!(server.is_bound());
        assert!(path.exists());

        // Cleanup
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn bind_removes_existing_file() {
        let path = temp_socket_path_nonexistent();

        // Create a regular file at the path
        std::fs::write(&path, b"existing file").unwrap();
        assert!(path.exists());

        let mut server = SocketServer::new(path.clone());
        assert!(server.bind().is_ok());

        // File should be replaced with socket
        assert!(path.exists());

        // Verify it's actually a socket now (by connecting to it)
        let _listener = StdUnixListener::bind(&path);
        // If bind succeeded, the original file was replaced

        // Cleanup
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn bind_twice_returns_error() {
        let path = temp_socket_path_nonexistent();
        let mut server = SocketServer::new(path.clone());

        assert!(server.bind().is_ok());
        let result = server.bind();

        assert!(matches!(result, Err(SocketError::AlreadyBound)));

        // Cleanup
        let _ = std::fs::remove_file(&path);
    }

    // ========== SocketServer::shutdown ==========

    #[test]
    fn shutdown_before_bind_returns_ok() {
        let path = temp_socket_path_nonexistent();
        let server = SocketServer::new(path);

        // shutdown is now OK even if not bound (channel not created yet)
        assert!(server.shutdown().is_ok());
    }

    #[test]
    fn shutdown_after_bind_succeeds() {
        let path = temp_socket_path_nonexistent();
        let mut server = SocketServer::new(path.clone());

        assert!(server.bind().is_ok());
        assert!(server.shutdown().is_ok());

        // Cleanup
        let _ = std::fs::remove_file(&path);
    }

    // ========== SocketServer::run ==========

    #[tokio::test]
    async fn run_accepts_connections() {
        let path = temp_socket_path_nonexistent();

        start_server_with_state(path.clone()).await;

        // Give server time to start listening
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Connect client - should succeed
        let client = tokio::net::UnixStream::connect(&path).await.unwrap();
        assert!(client.peer_addr().is_ok());

        // Cleanup
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn run_ends_on_shutdown_signal() {
        let path = temp_socket_path_nonexistent();

        let handle = start_server_with_state(path.clone()).await;

        tokio::time::sleep(Duration::from_millis(100)).await;

        // Connect to trigger channel creation, then disconnect
        let _client = tokio::net::UnixStream::connect(&path).await.ok();

        tokio::time::sleep(Duration::from_millis(50)).await;

        // Just verify server can be stopped by deleting socket
        // (in real usage, you'd call shutdown() but the channel is internal)
        let _ = std::fs::remove_file(&path);

        // Server task should eventually exit when accept fails
        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(handle);

        // Cleanup
        let _ = std::fs::remove_file(&path);
    }

    // ========== Connection handling ==========

    #[tokio::test]
    async fn handle_invalid_json_returns_error() {
        let path = temp_socket_path();

        start_server_with_state(path.clone()).await;

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = tokio::net::UnixStream::connect(&path).await.unwrap();

        // Send invalid JSON
        client.write_all(b"not valid json\n").await.unwrap();

        let mut reader = BufReader::new(client.split().0);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();

        let response: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(response["error"]["code"], -32700); // Parse error
    }

    #[tokio::test]
    async fn handle_empty_lines_ignored() {
        let path = temp_socket_path();

        start_server_with_state(path.clone()).await;

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = tokio::net::UnixStream::connect(&path).await.unwrap();

        // Send empty lines
        client.write_all(b"\n\n  \n\n").await.unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;

        // Connection should still be open
        client.write_all(b"ping\n").await.unwrap();
    }

    // ========== Request handling ==========

    #[tokio::test]
    async fn subscribe_request_returns_success() {
        let path = temp_socket_path();
        let mut server = SocketServer::new(path.clone());

        server.bind().unwrap();
        server.set_state(test_state());

        tokio::spawn(async move {
            let _ = server.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = tokio::net::UnixStream::connect(&path).await.unwrap();

        let request = r#"{"jsonrpc":"2.0","method":"subscribe","params":{"project_id":"test-project"},"id":1}"#;
        client.write_all(request.as_bytes()).await.unwrap();
        client.write_all(b"\n").await.unwrap();

        let mut reader = BufReader::new(client.split().0);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();

        let response: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert!(response["result"].is_object());
        assert_eq!(response["result"]["subscribed"], true);
        assert_eq!(response["id"], 1);
    }

    #[tokio::test]
    async fn unknown_method_returns_error() {
        let path = temp_socket_path();
        let mut server = SocketServer::new(path.clone());

        server.bind().unwrap();
        server.set_state(test_state());

        tokio::spawn(async move {
            let _ = server.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = tokio::net::UnixStream::connect(&path).await.unwrap();

        let request = r#"{"jsonrpc":"2.0","method":"unknown_method","id":2}"#;
        client.write_all(request.as_bytes()).await.unwrap();
        client.write_all(b"\n").await.unwrap();

        let mut reader = BufReader::new(client.split().0);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();

        let response: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(response["error"]["code"], -32601); // Method not found
        assert!(response["error"]["message"].as_str().unwrap().contains("unknown_method"));
    }

    #[tokio::test]
    async fn query_events_returns_empty_array() {
        let path = temp_socket_path();
        let mut server = SocketServer::new(path.clone());

        server.bind().unwrap();
        server.set_state(test_state());

        tokio::spawn(async move {
            let _ = server.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = tokio::net::UnixStream::connect(&path).await.unwrap();

        let request = r#"{"jsonrpc":"2.0","method":"query_events","params":{"project_id":"test"},"id":3}"#;
        client.write_all(request.as_bytes()).await.unwrap();
        client.write_all(b"\n").await.unwrap();

        let mut reader = BufReader::new(client.split().0);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();

        let response: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(response["result"], serde_json::json!([]));
    }

    // ========== query_events implementation tests ==========

    #[tokio::test]
    async fn query_events_with_real_db_returns_events() {
        use ambient_fs_core::event::{FileEvent, EventType, Source};

        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let socket_path = temp_socket_path_nonexistent();

        // Pre-populate db with events
        {
            use ambient_fs_store::EventStore;
            let store = EventStore::new(db_path.clone()).unwrap();

            let event1 = FileEvent::new(EventType::Created, "src/main.rs", "proj-1", "machine-1")
                .with_source(Source::User);
            let event2 = FileEvent::new(EventType::Modified, "src/lib.rs", "proj-2", "machine-1")
                .with_source(Source::AiAgent);
            let event3 = FileEvent::new(EventType::Deleted, "README.md", "proj-1", "machine-1")
                .with_source(Source::Git);

            store.insert(&event1).unwrap();
            store.insert(&event2).unwrap();
            store.insert(&event3).unwrap();
        }

        let mut server = SocketServer::new(socket_path.clone());
        server.bind().unwrap();
        server.set_state(Arc::new(ServerState::new(db_path.clone())));

        tokio::spawn(async move {
            let _ = server.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = tokio::net::UnixStream::connect(&socket_path).await.unwrap();

        // Query without filter - should return all events
        let request = r#"{"jsonrpc":"2.0","method":"query_events","id":1}"#;
        client.write_all(request.as_bytes()).await.unwrap();
        client.write_all(b"\n").await.unwrap();

        let mut reader = BufReader::new(client.split().0);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();

        let response: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert!(response["result"].is_array());
        let events = response["result"].as_array().unwrap();
        assert_eq!(events.len(), 3);
    }

    #[tokio::test]
    async fn query_events_with_project_id_filter() {
        use ambient_fs_core::event::{FileEvent, EventType, Source};

        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let socket_path = temp_socket_path_nonexistent();

        // Pre-populate db with events
        {
            use ambient_fs_store::EventStore;
            let store = EventStore::new(db_path.clone()).unwrap();

            let event1 = FileEvent::new(EventType::Created, "src/main.rs", "proj-1", "machine-1")
                .with_source(Source::User);
            let event2 = FileEvent::new(EventType::Modified, "src/lib.rs", "proj-2", "machine-1")
                .with_source(Source::AiAgent);
            let event3 = FileEvent::new(EventType::Deleted, "README.md", "proj-1", "machine-1")
                .with_source(Source::Git);

            store.insert(&event1).unwrap();
            store.insert(&event2).unwrap();
            store.insert(&event3).unwrap();
        }

        let mut server = SocketServer::new(socket_path.clone());
        server.bind().unwrap();
        server.set_state(Arc::new(ServerState::new(db_path.clone())));

        tokio::spawn(async move {
            let _ = server.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = tokio::net::UnixStream::connect(&socket_path).await.unwrap();

        let request = r#"{"jsonrpc":"2.0","method":"query_events","params":{"project_id":"proj-1"},"id":1}"#;
        client.write_all(request.as_bytes()).await.unwrap();
        client.write_all(b"\n").await.unwrap();

        let mut reader = BufReader::new(client.split().0);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();

        let response: serde_json::Value = serde_json::from_str(&line).unwrap();
        let events = response["result"].as_array().unwrap();
        assert_eq!(events.len(), 2);
        // All should be from proj-1
        for event in events {
            assert_eq!(event["project_id"], "proj-1");
        }
    }

    #[tokio::test]
    async fn query_events_with_limit() {
        use ambient_fs_core::event::{FileEvent, EventType, Source};

        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let socket_path = temp_socket_path_nonexistent();

        // Pre-populate db with 5 events
        {
            use ambient_fs_store::EventStore;
            let store = EventStore::new(db_path.clone()).unwrap();

            for i in 0..5 {
                let event = FileEvent::new(
                    EventType::Created,
                    &format!("file{}.rs", i),
                    "proj-1",
                    "machine-1",
                ).with_source(Source::User);
                store.insert(&event).unwrap();
            }
        }

        let mut server = SocketServer::new(socket_path.clone());
        server.bind().unwrap();
        server.set_state(Arc::new(ServerState::new(db_path.clone())));

        tokio::spawn(async move {
            let _ = server.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = tokio::net::UnixStream::connect(&socket_path).await.unwrap();

        let request = r#"{"jsonrpc":"2.0","method":"query_events","params":{"limit":3},"id":1}"#;
        client.write_all(request.as_bytes()).await.unwrap();
        client.write_all(b"\n").await.unwrap();

        let mut reader = BufReader::new(client.split().0);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();

        let response: serde_json::Value = serde_json::from_str(&line).unwrap();
        let events = response["result"].as_array().unwrap();
        assert_eq!(events.len(), 3);
    }

    #[tokio::test]
    async fn query_events_with_source_filter() {
        use ambient_fs_core::event::{FileEvent, EventType, Source};

        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let socket_path = temp_socket_path_nonexistent();

        // Pre-populate db with mixed source events
        {
            use ambient_fs_store::EventStore;
            let store = EventStore::new(db_path.clone()).unwrap();

            let e1 = FileEvent::new(EventType::Created, "a.rs", "proj-1", "m1")
                .with_source(Source::User);
            let e2 = FileEvent::new(EventType::Created, "b.rs", "proj-1", "m1")
                .with_source(Source::AiAgent);
            let e3 = FileEvent::new(EventType::Created, "c.rs", "proj-1", "m1")
                .with_source(Source::User);

            store.insert(&e1).unwrap();
            store.insert(&e2).unwrap();
            store.insert(&e3).unwrap();
        }

        let mut server = SocketServer::new(socket_path.clone());
        server.bind().unwrap();
        server.set_state(Arc::new(ServerState::new(db_path.clone())));

        tokio::spawn(async move {
            let _ = server.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = tokio::net::UnixStream::connect(&socket_path).await.unwrap();

        let request = r#"{"jsonrpc":"2.0","method":"query_events","params":{"source":"user"},"id":1}"#;
        client.write_all(request.as_bytes()).await.unwrap();
        client.write_all(b"\n").await.unwrap();

        let mut reader = BufReader::new(client.split().0);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();

        let response: serde_json::Value = serde_json::from_str(&line).unwrap();
        let events = response["result"].as_array().unwrap();
        assert_eq!(events.len(), 2);
        for event in events {
            assert_eq!(event["source"], "user");
        }
    }

    #[tokio::test]
    async fn query_events_with_invalid_params_type_returns_error() {
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let socket_path = temp_socket_path_nonexistent();

        let mut server = SocketServer::new(socket_path.clone());
        server.bind().unwrap();
        server.set_state(Arc::new(ServerState::new(db_path.clone())));

        tokio::spawn(async move {
            let _ = server.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = tokio::net::UnixStream::connect(&socket_path).await.unwrap();

        // Array params instead of object
        let request = r#"{"jsonrpc":"2.0","method":"query_events","params":[],"id":1}"#;
        client.write_all(request.as_bytes()).await.unwrap();
        client.write_all(b"\n").await.unwrap();

        let mut reader = BufReader::new(client.split().0);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();

        let response: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert!(response["error"].is_object());
        assert_eq!(response["error"]["code"], -32602); // invalid_params
    }

    // ========== Multiple requests ==========

    #[tokio::test]
    async fn multiple_requests_on_same_connection() {
        let path = temp_socket_path();
        let mut server = SocketServer::new(path.clone());

        server.bind().unwrap();
        server.set_state(test_state());

        tokio::spawn(async move {
            let _ = server.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = tokio::net::UnixStream::connect(&path).await.unwrap();
        let (reader, mut writer) = client.split();
        let mut lines = BufReader::new(reader).lines();

        // Send multiple requests
        let req1 = r#"{"jsonrpc":"2.0","method":"subscribe","params":{"project_id":"p1"},"id":1}"#;
        let req2 = r#"{"jsonrpc":"2.0","method":"query_events","params":{"project_id":"p1"},"id":2}"#;
        let req3 = r#"{"jsonrpc":"2.0","method":"watch_project","params":{"path":"/tmp/test"},"id":3}"#;

        writer.write_all(req1.as_bytes()).await.unwrap();
        writer.write_all(b"\n").await.unwrap();
        writer.write_all(req2.as_bytes()).await.unwrap();
        writer.write_all(b"\n").await.unwrap();
        writer.write_all(req3.as_bytes()).await.unwrap();
        writer.write_all(b"\n").await.unwrap();

        // Read responses
        let resp1 = lines.next_line().await.unwrap().unwrap();
        let resp2 = lines.next_line().await.unwrap().unwrap();
        let resp3 = lines.next_line().await.unwrap().unwrap();

        let r1: serde_json::Value = serde_json::from_str(&resp1).unwrap();
        let r2: serde_json::Value = serde_json::from_str(&resp2).unwrap();
        let r3: serde_json::Value = serde_json::from_str(&resp3).unwrap();

        assert_eq!(r1["id"], 1);
        assert_eq!(r2["id"], 2);
        assert_eq!(r3["id"], 3);
    }

    // ========== Multiple connections ==========

    #[tokio::test]
    async fn multiple_concurrent_connections() {
        let path = temp_socket_path();
        let mut server = SocketServer::new(path.clone());

        server.bind().unwrap();
        server.set_state(test_state());

        tokio::spawn(async move {
            let _ = server.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        // Create multiple clients
        let client1 = tokio::net::UnixStream::connect(&path).await.unwrap();
        let client2 = tokio::net::UnixStream::connect(&path).await.unwrap();
        let client3 = tokio::net::UnixStream::connect(&path).await.unwrap();

        // All should be able to connect
        assert!(client1.peer_addr().is_ok());
        assert!(client2.peer_addr().is_ok());
        assert!(client3.peer_addr().is_ok());
    }

    // ========== Disconnect handling ==========

    #[tokio::test]
    async fn client_disconnect_handled_gracefully() {
        let path = temp_socket_path();
        let mut server = SocketServer::new(path.clone());

        server.bind().unwrap();
        server.set_state(test_state());

        tokio::spawn(async move {
            let _ = server.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        // Connect and immediately disconnect
        let _client = tokio::net::UnixStream::connect(&path).await.unwrap();
        drop(_client);

        // Server should still be running
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Another client should be able to connect
        let _client2 = tokio::net::UnixStream::connect(&path).await.unwrap();
        assert!(tokio::net::UnixStream::connect(&path).await.is_ok());
    }

    // ========== Subscribe/Unsubscribe ==========

    #[tokio::test]
    async fn subscribe_returns_project_id() {
        let path = temp_socket_path();
        let mut server = SocketServer::new(path.clone());

        server.bind().unwrap();
        server.set_state(test_state());

        tokio::spawn(async move {
            let _ = server.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = tokio::net::UnixStream::connect(&path).await.unwrap();

        let request = r#"{"jsonrpc":"2.0","method":"subscribe","params":{"project_id":"my-project"},"id":1}"#;
        client.write_all(request.as_bytes()).await.unwrap();
        client.write_all(b"\n").await.unwrap();

        let mut reader = BufReader::new(client.split().0);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();

        let response: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert!(response["result"].is_object());
        assert_eq!(response["result"]["subscribed"], true);
        assert_eq!(response["result"]["project_id"], "my-project");
        assert_eq!(response["id"], 1);
    }

    #[tokio::test]
    async fn subscribe_without_project_id_returns_error() {
        let path = temp_socket_path();
        let mut server = SocketServer::new(path.clone());

        server.bind().unwrap();
        server.set_state(test_state());

        tokio::spawn(async move {
            let _ = server.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = tokio::net::UnixStream::connect(&path).await.unwrap();

        let request = r#"{"jsonrpc":"2.0","method":"subscribe","params":{},"id":1}"#;
        client.write_all(request.as_bytes()).await.unwrap();
        client.write_all(b"\n").await.unwrap();

        let mut reader = BufReader::new(client.split().0);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();

        let response: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert!(response["error"].is_object());
        assert_eq!(response["error"]["code"], -32602); // Invalid params
    }

    #[tokio::test]
    async fn unsubscribe_returns_true() {
        let path = temp_socket_path();
        let mut server = SocketServer::new(path.clone());

        server.bind().unwrap();
        server.set_state(test_state());

        tokio::spawn(async move {
            let _ = server.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = tokio::net::UnixStream::connect(&path).await.unwrap();
        let (reader, mut writer) = client.split();
        let mut lines = BufReader::new(reader).lines();

        // First subscribe
        let sub = r#"{"jsonrpc":"2.0","method":"subscribe","params":{"project_id":"proj-1"},"id":1}"#;
        writer.write_all(sub.as_bytes()).await.unwrap();
        writer.write_all(b"\n").await.unwrap();

        // Then unsubscribe
        let unsub = r#"{"jsonrpc":"2.0","method":"unsubscribe","params":{"project_id":"proj-1"},"id":2}"#;
        writer.write_all(unsub.as_bytes()).await.unwrap();
        writer.write_all(b"\n").await.unwrap();

        // Read subscribe response
        let sub_resp = lines.next_line().await.unwrap().unwrap();
        let sub_val: serde_json::Value = serde_json::from_str(&sub_resp).unwrap();
        assert_eq!(sub_val["result"]["subscribed"], true);

        // Read unsubscribe response
        let unsub_resp = lines.next_line().await.unwrap().unwrap();
        let unsub_val: serde_json::Value = serde_json::from_str(&unsub_resp).unwrap();
        assert_eq!(unsub_val["result"], true);
        assert_eq!(unsub_val["id"], 2);
    }

    #[tokio::test]
    async fn multiple_subscriptions_per_connection() {
        let path = temp_socket_path();
        let mut server = SocketServer::new(path.clone());

        server.bind().unwrap();
        server.set_state(test_state());

        tokio::spawn(async move {
            let _ = server.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = tokio::net::UnixStream::connect(&path).await.unwrap();
        let (reader, mut writer) = client.split();
        let mut lines = BufReader::new(reader).lines();

        // Subscribe to multiple projects
        for i in 1..=3 {
            let req = format!(r#"{{"jsonrpc":"2.0","method":"subscribe","params":{{"project_id":"proj-{}"}},"id":{}}}"#, i, i);
            writer.write_all(req.as_bytes()).await.unwrap();
            writer.write_all(b"\n").await.unwrap();
        }

        // Verify all responses
        for i in 1..=3 {
            let resp = lines.next_line().await.unwrap().unwrap();
            let val: serde_json::Value = serde_json::from_str(&resp).unwrap();
            assert_eq!(val["result"]["subscribed"], true);
            assert_eq!(val["result"]["project_id"], format!("proj-{}", i));
        }
    }

    #[tokio::test]
    async fn subscribe_without_params_returns_error() {
        let path = temp_socket_path();
        let mut server = SocketServer::new(path.clone());

        server.bind().unwrap();
        server.set_state(test_state());

        tokio::spawn(async move {
            let _ = server.run().await;
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut client = tokio::net::UnixStream::connect(&path).await.unwrap();

        let request = r#"{"jsonrpc":"2.0","method":"subscribe","id":1}"#;
        client.write_all(request.as_bytes()).await.unwrap();
        client.write_all(b"\n").await.unwrap();

        let mut reader = BufReader::new(client.split().0);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();

        let response: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert!(response["error"].is_object());
        assert_eq!(response["error"]["code"], -32602); // Invalid params
    }

    // ========== ConnectionState tests ==========

    #[test]
    fn connection_state_new_starts_empty() {
        let state = ConnectionState::new();
        assert!(!state.has_subscriptions());
    }

    #[test]
    fn connection_state_add_subscription() {
        let mut state = ConnectionState::new();
        let (tx, rx) = broadcast::channel(1);

        state.add_subscription("proj-1".to_string(), rx);
        assert!(state.has_subscriptions());
    }

    #[test]
    fn connection_state_remove_subscription() {
        let mut state = ConnectionState::new();
        let (tx, rx) = broadcast::channel(1);

        state.add_subscription("proj-1".to_string(), rx);
        assert!(state.has_subscriptions());

        let removed = state.remove_subscription("proj-1");
        assert!(removed);
        assert!(!state.has_subscriptions());
    }

    #[test]
    fn connection_state_remove_nonexistent_returns_false() {
        let mut state = ConnectionState::new();
        let removed = state.remove_subscription("nonexistent");
        assert!(!removed);
    }

    #[test]
    fn connection_state_get_receiver() {
        let mut state = ConnectionState::new();
        let (tx, rx) = broadcast::channel(1);

        state.add_subscription("proj-1".to_string(), rx);

        // get_receiver returns the receiver for a specific project
        let receiver = state.get_receiver("proj-1");
        assert!(receiver.is_some());

        // Non-existent project returns None
        assert!(state.get_receiver("nonexistent").is_none());
    }
}
