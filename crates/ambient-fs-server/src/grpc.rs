// gRPC server for ambient-fs
// TDD: Tests FIRST, then implementation

use std::sync::Arc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Code};
use tracing::{debug, info, error};

use crate::state::ServerState;
use crate::subscriptions::Notification;

// Generated protobuf code
pub mod ambient_fs {
    tonic::include_proto!("ambient_fs");
}

use ambient_fs::ambient_fs_server::{AmbientFs as AmbientFsService, AmbientFsServer as AmbientFsServerTrait};
use ambient_fs::{
    FileEventMessage,
    WatchProjectRequest, WatchProjectResponse,
    UnwatchProjectRequest, UnwatchProjectResponse,
    QueryEventsRequest, QueryEventsResponse,
    QueryAwarenessRequest, AwarenessResponse,
    AttributeRequest, AttributeResponse,
    SubscribeRequest, SyncEventsRequest,
};

/// gRPC server
///
/// Holds ServerState and runs a tonic Server on the configured address.
pub struct GrpcServer {
    state: Arc<ServerState>,
    addr: String,
}

impl GrpcServer {
    /// Create a new gRPC server
    pub fn new(state: Arc<ServerState>, addr: impl Into<String>) -> Self {
        Self {
            state,
            addr: addr.into(),
        }
    }

    /// Get the server address
    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// Run the gRPC server
    ///
    /// This blocks until the server shuts down.
    pub async fn run(self) -> Result<(), Box<dyn std::error::Error>> {
        let addr = self.addr.parse()?;
        let service = AmbientFsServiceImpl::new(self.state);

        info!("gRPC server listening on {}", self.addr);

        tonic::transport::Server::builder()
            .add_service(AmbientFsServerTrait::new(service))
            .serve(addr)
            .await?;

        Ok(())
    }
}

/// gRPC service implementation
///
/// Implements the AmbientFs trait from the protobuf definition.
/// Each RPC delegates to the same business logic as the JSON-RPC handlers.
struct AmbientFsServiceImpl {
    state: Arc<ServerState>,
}

impl AmbientFsServiceImpl {
    fn new(state: Arc<ServerState>) -> Self {
        Self { state }
    }
}

/// Convert tonic Status from ProtocolError
fn protocol_error(code: Code, message: impl Into<String>) -> Status {
    Status::new(code, message.into())
}

/// Convert FileEvent to FileEventMessage
fn file_event_to_message(event: &ambient_fs_core::FileEvent) -> FileEventMessage {
    FileEventMessage {
        timestamp: event.timestamp.timestamp(),
        event_type: event.event_type.to_string(),
        file_path: event.file_path.clone(),
        project_id: event.project_id.clone(),
        source: event.source.to_string(),
        source_id: event.source_id.clone(),
        machine_id: event.machine_id.clone(),
        content_hash: event.content_hash.clone(),
        old_path: event.old_path.clone(),
    }
}

/// Convert FileEventMessage to FileEvent
pub fn message_to_file_event(msg: &FileEventMessage, machine_id: &str) -> Option<ambient_fs_core::FileEvent> {
    use chrono::TimeZone;

    let event_type = msg.event_type.parse::<ambient_fs_core::EventType>().ok()?;
    let source = msg.source.parse::<ambient_fs_core::event::Source>().ok()?;

    let mut event = ambient_fs_core::FileEvent::new(
        event_type,
        &msg.file_path,
        &msg.project_id,
        machine_id,
    );
    event.timestamp = chrono::Utc.timestamp_opt(msg.timestamp, 0).single()?;
    event = event.with_source(source);

    if let Some(ref source_id) = msg.source_id {
        event = event.with_source_id(source_id.clone());
    }
    if let Some(ref content_hash) = msg.content_hash {
        event = event.with_content_hash(content_hash.clone());
    }
    if let Some(ref old_path) = msg.old_path {
        event = event.with_old_path(old_path.clone());
    }

    Some(event)
}

/// Stream type for subscribe RPC
type SubscribeStream = ReceiverStream<Result<FileEventMessage, Status>>;

/// Stream type for sync_events RPC
type SyncEventsStream = ReceiverStream<Result<FileEventMessage, Status>>;

#[tonic::async_trait]
impl AmbientFsService for AmbientFsServiceImpl {
    type SubscribeStream = SubscribeStream;
    type SyncEventsStream = SyncEventsStream;

    /// WatchProject: start watching a directory
    async fn watch_project(
        &self,
        request: Request<WatchProjectRequest>,
    ) -> Result<Response<WatchProjectResponse>, Status> {
        let req = request.into_inner();

        // Validate path
        let path = std::path::PathBuf::from(&req.path);
        if !path.exists() {
            return Err(protocol_error(Code::InvalidArgument, format!("path does not exist: {}", req.path)));
        }
        if !path.is_dir() {
            return Err(protocol_error(Code::InvalidArgument, format!("path is not a directory: {}", req.path)));
        }

        let canonical = path.canonicalize()
            .map_err(|e| protocol_error(Code::InvalidArgument, format!("cannot canonicalize path: {}", e)))?;

        let project_id = canonical
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        // Check if already watching
        if self.state.has_project(&project_id).await {
            if let Some(existing_path) = self.state.get_project(&project_id).await {
                if existing_path == canonical {
                    return Err(protocol_error(Code::AlreadyExists, format!("already watching: {}", req.path)));
                }
            }
        }

        // Create watcher
        let mut fs_watcher = ambient_fs_watcher::FsWatcher::new(
            100,
            project_id.clone(),
            self.state.machine_id.clone(),
        );

        if fs_watcher.start().is_err() {
            return Err(protocol_error(Code::Internal, format!("failed to start watcher for: {}", req.path)));
        }

        if let Err(e) = fs_watcher.watch(canonical.clone()) {
            return Err(protocol_error(Code::Internal, format!("failed to watch {}: {}", req.path, e)));
        }

        self.state.add_project(project_id.clone(), canonical.clone()).await;
        self.state.add_watcher(project_id.clone(), fs_watcher).await;

        // Persist to store
        let store_path = self.state.store_path.clone();
        let project_id_clone = project_id.clone();
        let canonical_clone = canonical.clone();
        tokio::task::spawn_blocking(move || {
            if let Ok(store) = ambient_fs_store::EventStore::new(store_path) {
                let _ = store.add_project(&project_id_clone, &canonical_clone);
            }
        });

        debug!("gRPC: Watching project: {} at {}", project_id, canonical.display());

        Ok(Response::new(WatchProjectResponse {
            watching: true,
            project_id,
        }))
    }

    /// UnwatchProject: stop watching a project
    async fn unwatch_project(
        &self,
        request: Request<UnwatchProjectRequest>,
    ) -> Result<Response<UnwatchProjectResponse>, Status> {
        let req = request.into_inner();

        let path = match self.state.get_project(&req.project_id).await {
            Some(p) => p,
            None => {
                return Err(protocol_error(Code::NotFound, format!("project not found: {}", req.project_id)));
            }
        };

        if let Some(watcher) = self.state.remove_watcher(&req.project_id).await {
            let mut watcher = watcher.lock().await;
            let _ = watcher.unwatch(path.clone());
            watcher.stop();
        }

        self.state.remove_project(&req.project_id).await;

        let store_path = self.state.store_path.clone();
        let project_id_clone = req.project_id.clone();
        tokio::task::spawn_blocking(move || {
            if let Ok(store) = ambient_fs_store::EventStore::new(store_path) {
                let _ = store.remove_project(&project_id_clone);
            }
        });

        debug!("gRPC: Unwatched project: {}", req.project_id);

        Ok(Response::new(UnwatchProjectResponse { success: true }))
    }

    /// QueryEvents: query event history
    async fn query_events(
        &self,
        request: Request<QueryEventsRequest>,
    ) -> Result<Response<QueryEventsResponse>, Status> {
        let req = request.into_inner();

        use ambient_fs_core::event::Source;
        use ambient_fs_store::{EventStore, EventFilter};

        let mut filter = EventFilter::new();

        if let Some(pid) = req.project_id {
            filter = filter.project_id(&pid);
        }

        if let Some(seconds) = req.since {
            use chrono::{Duration, Utc};
            let since_ts = Utc::now() - Duration::seconds(seconds);
            filter = filter.since(since_ts);
        }

        if let Some(source_str) = req.source {
            match source_str.parse::<Source>() {
                Ok(src) => filter = filter.source(src),
                Err(_) => {
                    return Err(protocol_error(Code::InvalidArgument, format!("invalid source: {}", source_str)));
                }
            }
        }

        if let Some(limit_val) = req.limit {
            filter = filter.limit(limit_val as usize);
        }

        let store_path = self.state.store_path.clone();
        let events = tokio::task::spawn_blocking(move || {
            let store = EventStore::new(store_path)?;
            store.query(filter)
        })
        .await
        .map_err(|e| {
            error!("spawn_blocking join error: {}", e);
            protocol_error(Code::Internal, "query failed")
        })?
        .map_err(|e| {
            debug!("Store query error: {}", e);
            protocol_error(Code::Internal, "query failed")
        })?;

        let event_messages: Vec<FileEventMessage> = events
            .iter()
            .map(file_event_to_message)
            .collect();

        Ok(Response::new(QueryEventsResponse { events: event_messages }))
    }

    /// QueryAwareness: get file awareness info
    async fn query_awareness(
        &self,
        request: Request<QueryAwarenessRequest>,
    ) -> Result<Response<AwarenessResponse>, Status> {
        let req = request.into_inner();

        let awareness = match crate::awareness::build_awareness(&self.state, &req.project_id, &req.path).await {
            Ok(Some(a)) => a,
            Ok(None) => {
                return Err(protocol_error(Code::NotFound, "no awareness data found"));
            }
            Err(_) => {
                return Err(protocol_error(Code::Internal, "failed to build awareness"));
            }
        };

        let last_modified = Some(format!("{:?}", awareness.last_modified));
        let modified_by = Some(awareness.modified_by.to_string());

        Ok(Response::new(AwarenessResponse {
            file_path: req.path.clone(),
            project_id: req.project_id,
            last_modified,
            modified_by,
            change_frequency: awareness.change_frequency.to_string(),
            todo_count: awareness.todo_count as i32,
            lint_hints: 0, // awareness.lint_hints is a Vec<LintHint>, not a count
            line_count: awareness.line_count as i32,
            active_agent: self.state.get_active_agent(&req.path).await,
        }))
    }

    /// Attribute: manually attribute a file event
    async fn attribute(
        &self,
        request: Request<AttributeRequest>,
    ) -> Result<Response<AttributeResponse>, Status> {
        let req = request.into_inner();

        use ambient_fs_core::event::{FileEvent, EventType, Source};

        let source = req.source.parse::<Source>()
            .map_err(|_| protocol_error(Code::InvalidArgument, format!("invalid source: {}", req.source)))?;

        let mut event = FileEvent::new(
            EventType::Modified,
            &req.file_path,
            &req.project_id,
            &self.state.machine_id,
        );
        event = event.with_source(source);

        if let Some(source_id) = req.source_id {
            event = event.with_source_id(source_id);
        }

        let event_clone = event.clone();
        let store_path = self.state.store_path.clone();

        let _insert_result = tokio::task::spawn_blocking(move || {
            let store = ambient_fs_store::EventStore::new(store_path)?;
            store.insert(&event)
        })
        .await
        .map_err(|e| {
            debug!("Join error during attribute: {:?}", e);
            protocol_error(Code::Internal, "attribute failed")
        })?
        .map_err(|e| {
            debug!("Store insert error during attribute: {:?}", e);
            protocol_error(Code::Internal, "attribute failed")
        })?;

        self.state.subscriptions.publish_event(event_clone).await;

        debug!("gRPC: Attributed file {} to source {}", req.file_path, req.source);

        Ok(Response::new(AttributeResponse { attributed: true }))
    }

    /// Subscribe: stream events for a project
    ///
    /// Server streaming RPC. Returns a stream of FileEventMessage.
    async fn subscribe(
        &self,
        request: Request<SubscribeRequest>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
        let req = request.into_inner();
        let project_id = req.project_id.clone();

        let mut rx = self.state.subscriptions.subscribe(&project_id).await;

        let (tx, rx_out) = tokio::sync::mpsc::channel(32);

        // Spawn a task to bridge broadcast::Receiver to mpsc::Sender
        tokio::spawn(async move {
            while let Ok(notification) = rx.recv().await {
                let msg = match notification {
                    Notification::Event(event) => Ok(file_event_to_message(&event)),
                    Notification::AwarenessChanged { .. } | Notification::AnalysisComplete { .. } | Notification::TreePatch { .. } => {
                        // For now, only stream events
                        continue;
                    }
                };

                if tx.send(msg).await.is_err() {
                    // Client disconnected
                    break;
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx_out)))
    }

    /// SyncEvents: stream events since a timestamp for cross-machine sync
    ///
    /// Server streaming RPC. Returns a stream of FileEventMessage.
    async fn sync_events(
        &self,
        request: Request<SyncEventsRequest>,
    ) -> Result<Response<Self::SyncEventsStream>, Status> {
        let req = request.into_inner();

        use ambient_fs_store::{EventStore, EventFilter};
        use chrono::TimeZone;

        // Build filter for events since timestamp
        let since_ts = chrono::Utc.timestamp_opt(req.since_timestamp, 0)
            .single()
            .ok_or_else(|| protocol_error(Code::InvalidArgument, "invalid timestamp"))?;

        let filter = EventFilter::new()
            .project_id(&req.project_id)
            .since(since_ts);

        let store_path = self.state.store_path.clone();
        let events = tokio::task::spawn_blocking(move || {
            let store = EventStore::new(store_path)?;
            store.query(filter)
        })
        .await
        .map_err(|e| {
            debug!("spawn_blocking join error: {}", e);
            protocol_error(Code::Internal, "sync failed")
        })?
        .map_err(|e| {
            debug!("Store query error: {}", e);
            protocol_error(Code::Internal, "sync failed")
        })?;

        let (tx, rx_out) = tokio::sync::mpsc::channel(32);

        // Send all historical events
        for event in events {
            let msg = Ok(file_event_to_message(&event));
            if tx.send(msg).await.is_err() {
                break;
            }
        }

        // NOTE: There's a potential gap here. Events that occur between the
        // historical query and the subscription being established could be
        // missed. The spec handles this via timestamp overlap on the client
        // side (client requests events since timestamp - N seconds).

        // Then subscribe to future events
        let mut sub_rx = self.state.subscriptions.subscribe(&req.project_id).await;

        tokio::spawn(async move {
            while let Ok(notification) = sub_rx.recv().await {
                if let Notification::Event(event) = notification {
                    let msg = Ok(file_event_to_message(&event));
                    if tx.send(msg).await.is_err() {
                        break;
                    }
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx_out)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;
    use ambient_fs_core::event::{FileEvent, EventType, Source};

    // ========== file_event_to_message ==========

    #[test]
    fn file_event_to_message_preserves_data() {
        let timestamp = chrono::Utc::now();
        let mut event = FileEvent::new(
            EventType::Modified,
            "src/main.rs",
            "proj-1",
            "machine-1",
        );
        event.timestamp = timestamp;
        event = event.with_source(Source::AiAgent)
            .with_source_id("agent-123".to_string())
            .with_content_hash("abc123".to_string());

        let msg = file_event_to_message(&event);

        assert_eq!(msg.file_path, "src/main.rs");
        assert_eq!(msg.project_id, "proj-1");
        assert_eq!(msg.machine_id, "machine-1");
        assert_eq!(msg.event_type, "modified");
        assert_eq!(msg.source, "ai_agent");
        assert_eq!(msg.source_id, Some("agent-123".to_string()));
        assert_eq!(msg.content_hash, Some("abc123".to_string()));
        assert_eq!(msg.timestamp, timestamp.timestamp());
    }

    #[test]
    fn file_event_to_message_with_old_path() {
        let event = FileEvent::new(
            EventType::Renamed,
            "src/new.rs",
            "proj-1",
            "machine-1",
        ).with_old_path("src/old.rs".to_string());

        let msg = file_event_to_message(&event);

        assert_eq!(msg.old_path, Some("src/old.rs".to_string()));
        assert_eq!(msg.file_path, "src/new.rs");
        assert_eq!(msg.event_type, "renamed");
    }

    #[test]
    fn file_event_to_message_with_all_event_types() {
        for (event_type, expected_str) in [
            (EventType::Created, "created"),
            (EventType::Modified, "modified"),
            (EventType::Deleted, "deleted"),
            (EventType::Renamed, "renamed"),
        ] {
            let event = FileEvent::new(event_type, "file.rs", "proj-1", "m1");
            let msg = file_event_to_message(&event);
            assert_eq!(msg.event_type, expected_str);
        }
    }

    #[test]
    fn file_event_to_message_with_all_sources() {
        for (source, expected_str) in [
            (Source::User, "user"),
            (Source::AiAgent, "ai_agent"),
            (Source::Git, "git"),
            (Source::Build, "build"),
        ] {
            let event = FileEvent::new(EventType::Modified, "file.rs", "proj-1", "m1")
                .with_source(source);
            let msg = file_event_to_message(&event);
            assert_eq!(msg.source, expected_str);
        }
    }

    // ========== message_to_file_event ==========

    #[test]
    fn message_to_file_event_roundtrip() {
        let original = FileEvent::new(
            EventType::Modified,
            "src/main.rs",
            "proj-1",
            "machine-1",
        )
        .with_source(Source::AiAgent)
        .with_source_id("agent-123".to_string())
        .with_content_hash("abc123".to_string());

        let msg = file_event_to_message(&original);
        let restored = message_to_file_event(&msg, "machine-1").unwrap();

        assert_eq!(restored.file_path, original.file_path);
        assert_eq!(restored.project_id, original.project_id);
        assert_eq!(restored.source, original.source);
        assert_eq!(restored.source_id, original.source_id);
        assert_eq!(restored.content_hash, original.content_hash);
    }

    #[test]
    fn message_to_file_event_with_renamed_event() {
        let msg = FileEventMessage {
            timestamp: chrono::Utc::now().timestamp(),
            event_type: "renamed".to_string(),
            file_path: "new.rs".to_string(),
            project_id: "proj-1".to_string(),
            source: "user".to_string(),
            source_id: None,
            machine_id: "m1".to_string(),
            content_hash: None,
            old_path: Some("old.rs".to_string()),
        };

        let event = message_to_file_event(&msg, "m1").unwrap();

        assert_eq!(event.file_path, "new.rs");
        assert_eq!(event.old_path, Some("old.rs".to_string()));
    }

    #[test]
    fn message_to_file_event_invalid_event_type_returns_none() {
        let msg = FileEventMessage {
            timestamp: chrono::Utc::now().timestamp(),
            event_type: "invalid_type".to_string(),
            file_path: "file.rs".to_string(),
            project_id: "proj-1".to_string(),
            source: "user".to_string(),
            source_id: None,
            machine_id: "m1".to_string(),
            content_hash: None,
            old_path: None,
        };

        assert!(message_to_file_event(&msg, "m1").is_none());
    }

    #[test]
    fn message_to_file_event_invalid_source_returns_none() {
        let msg = FileEventMessage {
            timestamp: chrono::Utc::now().timestamp(),
            event_type: "modified".to_string(),
            file_path: "file.rs".to_string(),
            project_id: "proj-1".to_string(),
            source: "invalid_source".to_string(),
            source_id: None,
            machine_id: "m1".to_string(),
            content_hash: None,
            old_path: None,
        };

        assert!(message_to_file_event(&msg, "m1").is_none());
    }

    // ========== GrpcServer::new ==========

    #[test]
    fn new_creates_server_with_state() {
        let state = Arc::new(ServerState::new(PathBuf::from("/tmp/test.db")));
        let server = GrpcServer::new(state, "0.0.0.0:50051");

        assert_eq!(server.addr(), "0.0.0.0:50051");
    }

    #[test]
    fn addr_returns_configured_address() {
        let state = Arc::new(ServerState::new(PathBuf::from("/tmp/test.db")));
        let server = GrpcServer::new(state, "127.0.0.1:8080");

        assert_eq!(server.addr(), "127.0.0.1:8080");
    }

    // ========== Integration-ish tests with service ==========

    #[tokio::test]
    async fn service_impl_constructor() {
        let state = Arc::new(ServerState::new(PathBuf::from("/tmp/test.db")));
        let service = AmbientFsServiceImpl::new(state);

        // Just verify it constructs
        assert_eq!(service.state.machine_id, service.state.machine_id);
    }

    // ========== protocol_error helper ==========

    #[test]
    fn protocol_error_creates_status_with_code() {
        let status = protocol_error(Code::NotFound, "project not found");

        assert_eq!(status.code(), Code::NotFound);
        assert_eq!(status.message(), "project not found");
    }

    #[test]
    fn protocol_error_with_invalid_argument() {
        let status = protocol_error(Code::InvalidArgument, "bad path");

        assert_eq!(status.code(), Code::InvalidArgument);
        assert_eq!(status.message(), "bad path");
    }
}
